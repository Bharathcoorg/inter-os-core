//! Ring 3 Userspace Task Execution & Hardware Privilege Switching
//!
//! Provides the mechanism to drop CPU privileges from Ring 0 to Ring 3 (User Mode)
//! using `iretq`, and handles hardware fast system calls (`syscall` / `sysretq`).
//!
//! Non-Negotiable Principle #2: Security is enforced by the kernel below user workloads.

use x86_64::structures::paging::{Page, PageTableFlags, Size4KiB};
use x86_64::VirtAddr;
use crate::capability::{Capability, CapabilityId, CapabilityTable, CapabilityType, Rights};
use crate::task::{SCHEDULER, TaskId};
use crate::serial_println;

pub const USER_STACK_SIZE: usize = 16 * 1024; // 16 KiB dedicated user-mode stack

/// User Code Segment Selector with RPL = 3 (GDT Index 4: 0x20 | 3 = 0x23)
pub const USER_CS_SELECTOR: u64 = 0x23;

/// User Data Segment Selector with RPL = 3 (GDT Index 3: 0x18 | 3 = 0x1B)
pub const USER_SS_SELECTOR: u64 = 0x1B;

/// Embedded static ELF binary of the primordial userspace init process (`userspace/init`).
pub static USER_INIT_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/init"
));

/// Embedded static ELF binary of the Sovereign Settings app (`apps/settings`).
pub static APP_SETTINGS_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/settings"
));

/// Embedded static ELF binary of the Sovereign Intent Terminal app (`apps/terminal`).
pub static APP_TERMINAL_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/terminal"
));

/// Embedded static ELF binary of the Sovereign Dialer app (`apps/dialer`).
pub static APP_DIALER_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/dialer"
));

/// Embedded static ELF binary of the Sovereign Vault & Storage app (`apps/vault`).
pub static APP_VAULT_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/vault"
));

/// Embedded static ELF binary of the Sovereign Camera & Neural Vision app (`apps/camera`).
pub static APP_CAMERA_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/camera"
));

/// Embedded static ELF binary of the Open-Source Touch Calculator app (`apps/calculator`).
pub static APP_CALCULATOR_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/calculator"
));

/// Embedded static ELF binary of the Sovereign Browser app (`apps/browser`).
pub static APP_BROWSER_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/browser"
));

/// Embedded static ELF binary of the Sovereign Messages app (`apps/messages`).
pub static APP_MESSAGES_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/messages"
));

/// Embedded static ELF binary of the Sovereign Calendar app (`apps/calendar`).
pub static APP_CALENDAR_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/calendar"
));

/// Embedded static ELF binary of the Sovereign Music app (`apps/music`).
pub static APP_MUSIC_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/music"
));

/// Drops CPU execution privilege from Ring 0 to Ring 3 via an `iretq` stack frame.
///
/// Hardware sets:
/// - CS <- 0x23 (User 64-bit Code, CPL = 3)
/// - SS <- 0x1B (User Data, CPL = 3)
/// - RSP <- user_stack_top
/// - RIP <- user_entry
/// - RFLAGS <- 0x202 (IF = 1, interrupts enabled in userspace)
#[inline(never)]
pub unsafe fn jump_to_ring3(user_entry: usize, user_stack_top: usize) -> ! {
    core::arch::asm!(
        // 1. Load User Data Segment into data segment registers
        "mov ds, ax",
        "mov es, ax",
        "mov fs, ax",
        "mov gs, ax",

        // 2. Build 64-bit iretq stack frame (pushed in reverse order):
        // [RSP + 32] = SS      (0x1B)
        // [RSP + 24] = RSP     (user_stack_top)
        // [RSP + 16] = RFLAGS  (0x202)
        // [RSP + 8]  = CS      (0x23)
        // [RSP + 0]  = RIP     (user_entry)
        "push {ss:r}",
        "push {rsp}",
        "push 0x202",
        "push {cs:r}",
        "push {rip}",

        // 3. Clear general purpose registers for security isolation (Principle #2)
        "xor rax, rax",
        "xor rbx, rbx",
        "xor rcx, rcx",
        "xor rdx, rdx",
        "xor rsi, rsi",
        "xor rdi, rdi",
        "xor rbp, rbp",
        "xor r8, r8",
        "xor r9, r9",
        "xor r10, r10",
        "xor r11, r11",
        "xor r12, r12",
        "xor r13, r13",
        "xor r14, r14",
        "xor r15, r15",

        // 4. Return from interrupt -> transitions CPU to Ring 3 (CPL = 3)
        "iretq",

        ss = in(reg) USER_SS_SELECTOR,
        rsp = in(reg) user_stack_top as u64,
        cs = in(reg) USER_CS_SELECTOR,
        rip = in(reg) user_entry as u64,
        in("ax") (USER_SS_SELECTOR as u16),
        options(noreturn)
    );
}

/// Loads a 64-bit ELF binary into user virtual address space with default stack top (0x0080_0000).
pub fn load_user_elf(elf_data: &[u8]) -> Result<(usize, usize), &'static str> {
    load_user_elf_with_stack(elf_data, 0x0080_0000)
}

/// Loads a 64-bit ELF binary into user virtual address space, mapping code & data pages
/// with `USER_ACCESSIBLE` flags, allocating a dedicated user stack at `stack_top_addr`,
/// and returning `(entry_point, user_stack_top)`.
pub fn load_user_elf_with_stack(elf_data: &[u8], stack_top_addr: usize) -> Result<(usize, usize), &'static str> {
    if elf_data.len() < 64 {
        return Err("ELF header too small");
    }

    // 1. Verify ELF magic: \x7f E L F
    if &elf_data[0..4] != b"\x7fELF" {
        return Err("Invalid ELF magic");
    }
    if elf_data[4] != 2 {
        return Err("Not a 64-bit ELF");
    }
    if elf_data[5] != 1 {
        return Err("Not little-endian");
    }

    // 2. Extract entry point and program header table offset
    let e_entry = u64::from_le_bytes(elf_data[24..32].try_into().unwrap()) as usize;
    let e_phoff = u64::from_le_bytes(elf_data[32..40].try_into().unwrap()) as usize;
    let e_phentsize = u16::from_le_bytes(elf_data[54..56].try_into().unwrap()) as usize;
    let e_phnum = u16::from_le_bytes(elf_data[56..58].try_into().unwrap()) as usize;

    let phys_offset = crate::memory::phys_mem_offset();

    // 3. Process each program header (PT_LOAD segments)
    for i in 0..e_phnum {
        let ph_start = e_phoff + i * e_phentsize;
        if ph_start + 56 > elf_data.len() {
            return Err("Program header out of bounds");
        }

        let p_type = u32::from_le_bytes(elf_data[ph_start..ph_start + 4].try_into().unwrap());
        if p_type != 1 {
            // Only PT_LOAD segments are loaded into memory
            continue;
        }

        let p_offset = u64::from_le_bytes(elf_data[ph_start + 8..ph_start + 16].try_into().unwrap()) as usize;
        let p_vaddr = u64::from_le_bytes(elf_data[ph_start + 16..ph_start + 24].try_into().unwrap()) as usize;
        let p_filesz = u64::from_le_bytes(elf_data[ph_start + 32..ph_start + 40].try_into().unwrap()) as usize;
        let p_memsz = u64::from_le_bytes(elf_data[ph_start + 40..ph_start + 48].try_into().unwrap()) as usize;

        if p_memsz == 0 {
            continue;
        }

        // Calculate virtual page range
        let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(p_vaddr as u64));
        let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new((p_vaddr + p_memsz - 1) as u64));

        let flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE;

        for page in Page::range_inclusive(start_page, end_page) {
            let frame = crate::memory::allocate_frame().ok_or("Out of memory for user segment")?;
            crate::memory::map_user_page(page, frame, flags)?;

            // Zero physical frame
            let virt_frame = phys_offset + frame.start_address().as_u64();
            unsafe {
                core::ptr::write_bytes(virt_frame.as_mut_ptr::<u8>(), 0, 4096);
            }
        }

        // Copy segment payload from ELF into user virtual memory
        if p_filesz > 0 {
            if p_offset + p_filesz > elf_data.len() {
                return Err("Segment file size exceeds ELF binary length");
            }
            let src = &elf_data[p_offset..p_offset + p_filesz];
            let dest = p_vaddr as *mut u8;
            unsafe {
                core::ptr::copy_nonoverlapping(src.as_ptr(), dest, p_filesz);
            }
        }
    }

    // 4. Allocate 16 KiB dedicated user stack
    let stack_bottom = VirtAddr::new((stack_top_addr.saturating_sub(USER_STACK_SIZE)) as u64);
    let stack_top_vaddr = VirtAddr::new(stack_top_addr as u64);
    let start_stack_page = Page::<Size4KiB>::containing_address(stack_bottom);
    let end_stack_page = Page::<Size4KiB>::containing_address(stack_top_vaddr - 1u64);

    let stack_flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    for page in Page::range_inclusive(start_stack_page, end_stack_page) {
        let frame = crate::memory::allocate_frame().ok_or("Out of memory for user stack")?;
        crate::memory::map_user_page(page, frame, stack_flags)?;

        let virt_frame = phys_offset + frame.start_address().as_u64();
        unsafe {
            core::ptr::write_bytes(virt_frame.as_mut_ptr::<u8>(), 0, 4096);
        }
    }

    // User stack top aligned for 64-bit AMD64 System V ABI (RSP % 16 == 8 on entry)
    let user_stack_top = stack_top_addr - 8;
    Ok((e_entry, user_stack_top))
}

/// Trampoline function running in Ring 0 that loads the userspace init ELF and drops into Ring 3.
pub extern "C" fn user_task_trampoline() -> ! {
    serial_println!("[RING 0] Loading userspace primordial init ELF binary...");

    let (user_entry, user_stack_top) = match load_user_elf(USER_INIT_ELF) {
        Ok(res) => res,
        Err(err) => {
            serial_println!("[FATAL] Failed to load userspace init ELF: {}", err);
            crate::task::terminate_current();
        }
    };

    serial_println!("[OK] Userspace init binary mapped at virtual base: {:#010X}", user_entry);
    serial_println!("[OK] Userspace stack mapped at virtual top:        {:#010X}", user_stack_top);
    serial_println!("[RING 0] Dropping CPU privilege level from Ring 0 to Ring 3 (User Mode)...");

    unsafe {
        jump_to_ring3(user_entry, user_stack_top);
    }
}

/// Spawns the primordial userspace task (Task 9: `user_init`) into the scheduler.
pub fn spawn_user_init(id_security: TaskId) -> TaskId {
    let mut user_caps = CapabilityTable::new(24);
    // Slot 0: Serial logging capability
    user_caps.insert(Capability {
        id: CapabilityId(901),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();

    // Slot 1: IPC Send capability targeting Security Sentinel (Task 2)
    user_caps.insert(Capability {
        id: CapabilityId(902),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: id_security.0,
    }).ok();

    // Slot 2: Timer tick read capability
    user_caps.insert(Capability {
        id: CapabilityId(903),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();

    // Slot 3: Filesystem capability (with GRANT right for subcontracted task delegation)
    user_caps.insert(Capability {
        id: CapabilityId(904),
        cap_type: CapabilityType::FileSystemWrite,
        rights: Rights::READ.union(Rights::WRITE).union(Rights::GRANT),
        object_id: 0,
    }).ok();

    // Slot 4: Network Socket capability
    user_caps.insert(Capability {
        id: CapabilityId(905),
        cap_type: CapabilityType::NetworkSocket,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();

    // Slot 5: Network Send capability
    user_caps.insert(Capability {
        id: CapabilityId(906),
        cap_type: CapabilityType::NetworkSend,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();

    // Slot 6: ShmProducer capability (Zero-Copy DMA ring buffer writes)
    user_caps.insert(Capability {
        id: CapabilityId(907),
        cap_type: CapabilityType::ShmProducer,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();

    // Slot 7: ShmConsumer capability (Zero-Copy DMA ring buffer reads)
    user_caps.insert(Capability {
        id: CapabilityId(908),
        cap_type: CapabilityType::ShmConsumer,
        rights: Rights::READ,
        object_id: 0,
    }).ok();

    // Slot 8: EventPoll capability (Sovereign Async Event Reactor & epoll/kqueue)
    user_caps.insert(Capability {
        id: CapabilityId(909),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();

    let id = SCHEDULER.lock().spawn(
        "user_init",
        user_task_trampoline,
        user_caps,
        1, // Normal priority
    );

    serial_println!("[OK] Spawned Task 9: user_init (Ring 3 User Mode, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Settings App (Ring 3)
pub extern "C" fn app_settings_trampoline() -> ! {
    serial_println!("[RING 0] Loading Sovereign Settings App binary...");
    let (user_entry, user_stack_top) = match load_user_elf_with_stack(APP_SETTINGS_ELF, 0x0084_0000) {
        Ok(res) => res,
        Err(err) => {
            serial_println!("[FATAL] Failed to load Settings ELF: {}", err);
            crate::task::terminate_current();
        }
    };
    serial_println!("[OK] Settings app mapped at virtual base: {:#010X}, stack: {:#010X}", user_entry, user_stack_top);
    serial_println!("[RING 0] Launching Settings App in Ring 3 (User Mode)...");
    unsafe {
        jump_to_ring3(user_entry, user_stack_top);
    }
}

/// Spawns the native Sovereign Settings App in Ring 3.
pub fn spawn_app_settings(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(8);
    caps.insert(Capability {
        id: CapabilityId(1701),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1702),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: id_security.0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1703),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1704),
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();

    let id = SCHEDULER.lock().spawn(
        "app_settings",
        app_settings_trampoline,
        caps,
        1,
    );
    serial_println!("[OK] Spawned Task 17: app_settings (Ring 3 User Mode, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Intent Terminal App (Ring 3)
pub extern "C" fn app_terminal_trampoline() -> ! {
    serial_println!("[RING 0] Loading Sovereign Intent Terminal binary...");
    let (user_entry, user_stack_top) = match load_user_elf_with_stack(APP_TERMINAL_ELF, 0x0088_0000) {
        Ok(res) => res,
        Err(err) => {
            serial_println!("[FATAL] Failed to load Terminal ELF: {}", err);
            crate::task::terminate_current();
        }
    };
    serial_println!("[OK] Terminal app mapped at virtual base: {:#010X}, stack: {:#010X}", user_entry, user_stack_top);
    serial_println!("[RING 0] Launching Intent Terminal App in Ring 3 (User Mode)...");
    unsafe {
        jump_to_ring3(user_entry, user_stack_top);
    }
}

/// Spawns the native Sovereign Intent Terminal App in Ring 3.
pub fn spawn_app_terminal(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(8);
    caps.insert(Capability {
        id: CapabilityId(1801),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1802),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: id_security.0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1803),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1804),
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();

    let id = SCHEDULER.lock().spawn(
        "app_terminal",
        app_terminal_trampoline,
        caps,
        1,
    );
    serial_println!("[OK] Spawned Task 18: app_terminal (Ring 3 User Mode, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Dialer App (Ring 3)
pub extern "C" fn app_dialer_trampoline() -> ! {
    serial_println!("[RING 0] Loading Sovereign Dialer App binary...");
    let (user_entry, user_stack_top) = match load_user_elf_with_stack(APP_DIALER_ELF, 0x008C_0000) {
        Ok(res) => res,
        Err(err) => {
            serial_println!("[FATAL] Failed to load Dialer ELF: {}", err);
            crate::task::terminate_current();
        }
    };
    serial_println!("[OK] Dialer app mapped at virtual base: {:#010X}, stack: {:#010X}", user_entry, user_stack_top);
    serial_println!("[RING 0] Launching Sovereign Dialer App in Ring 3 (User Mode)...");
    unsafe {
        jump_to_ring3(user_entry, user_stack_top);
    }
}

/// Spawns the native Sovereign Dialer App in Ring 3.
pub fn spawn_app_dialer(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(8);
    caps.insert(Capability {
        id: CapabilityId(1901),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1902),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: id_security.0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1903),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();

    let id = SCHEDULER.lock().spawn(
        "app_dialer",
        app_dialer_trampoline,
        caps,
        1,
    );
    serial_println!("[OK] Spawned Task 19: app_dialer (Ring 3 User Mode, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Vault App (Ring 3)
pub extern "C" fn app_vault_trampoline() -> ! {
    serial_println!("[RING 0] Loading Sovereign Vault & Storage App binary...");
    let (user_entry, user_stack_top) = match load_user_elf_with_stack(APP_VAULT_ELF, 0x009C_0000) {
        Ok(res) => res,
        Err(err) => {
            serial_println!("[FATAL] Failed to load Vault ELF: {}", err);
            crate::task::terminate_current();
        }
    };
    serial_println!("[OK] Vault app mapped at virtual base: {:#010X}, stack: {:#010X}", user_entry, user_stack_top);
    serial_println!("[RING 0] Launching Sovereign Vault App in Ring 3 (User Mode)...");
    unsafe {
        jump_to_ring3(user_entry, user_stack_top);
    }
}

/// Spawns the native Sovereign Vault & Storage App in Ring 3.
pub fn spawn_app_vault(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(8);
    caps.insert(Capability {
        id: CapabilityId(2001),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2002),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: id_security.0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2003),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2004),
        cap_type: CapabilityType::FileSystemWrite,
        rights: Rights::READ | Rights::WRITE | Rights::GRANT,
        object_id: 0,
    }).ok();

    let id = SCHEDULER.lock().spawn(
        "app_vault",
        app_vault_trampoline,
        caps,
        1,
    );
    serial_println!("[OK] Spawned Task 20: app_vault (Ring 3 User Mode, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Camera App (Ring 3)
pub extern "C" fn app_camera_trampoline() -> ! {
    serial_println!("[RING 0] Loading Sovereign Camera App binary...");
    let (user_entry, user_stack_top) = match load_user_elf_with_stack(APP_CAMERA_ELF, 0x00AC_0000) {
        Ok(res) => res,
        Err(err) => {
            serial_println!("[FATAL] Failed to load Camera ELF: {}", err);
            crate::task::terminate_current();
        }
    };
    serial_println!("[OK] Camera app mapped at virtual base: {:#010X}, stack: {:#010X}", user_entry, user_stack_top);
    serial_println!("[RING 0] Launching Sovereign Camera App in Ring 3 (User Mode)...");
    unsafe {
        jump_to_ring3(user_entry, user_stack_top);
    }
}

/// Spawns the native Sovereign Camera & Neural Vision App in Ring 3.
pub fn spawn_app_camera(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(8);
    caps.insert(Capability {
        id: CapabilityId(2101),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2102),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: id_security.0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2103),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2104),
        cap_type: CapabilityType::HardwarePortIo,
        rights: Rights::READ | Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2105),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();

    let id = SCHEDULER.lock().spawn(
        "app_camera",
        app_camera_trampoline,
        caps,
        1,
    );
    serial_println!("[OK] Spawned Task 21: app_camera (Ring 3 User Mode, ID: {:?})", id);
    id
}

/// Trampoline for Open-Source Touch Calculator App (Ring 3)
pub extern "C" fn app_calculator_trampoline() -> ! {
    serial_println!("[RING 0] Loading Open-Source Touch Calculator App binary...");
    let (user_entry, user_stack_top) = match load_user_elf_with_stack(APP_CALCULATOR_ELF, 0x00BC_0000) {
        Ok(res) => res,
        Err(err) => {
            serial_println!("[FATAL] Failed to load Calculator ELF: {}", err);
            crate::task::terminate_current();
        }
    };
    serial_println!("[OK] Calculator app mapped at virtual base: {:#010X}, stack: {:#010X}", user_entry, user_stack_top);
    serial_println!("[RING 0] Launching Open-Source Touch Calculator App in Ring 3 (User Mode)...");
    unsafe {
        jump_to_ring3(user_entry, user_stack_top);
    }
}

/// Spawns the native Open-Source Touch Calculator App in Ring 3.
pub fn spawn_app_calculator(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(8);
    caps.insert(Capability {
        id: CapabilityId(2201),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2202),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: id_security.0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2203),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2204),
        cap_type: CapabilityType::HardwarePortIo,
        rights: Rights::READ | Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2205),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();

    let id = SCHEDULER.lock().spawn(
        "app_calculator",
        app_calculator_trampoline,
        caps,
        1,
    );
    serial_println!("[OK] Spawned Task 22: app_calculator (Ring 3 User Mode, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Browser App (Ring 3)
pub extern "C" fn app_browser_trampoline() -> ! {
    serial_println!("[RING 0] Loading Sovereign Browser App binary...");
    let (user_entry, user_stack_top) = match load_user_elf_with_stack(APP_BROWSER_ELF, 0x00CC_0000) {
        Ok(res) => res,
        Err(err) => {
            serial_println!("[FATAL] Failed to load Browser ELF: {}", err);
            crate::task::terminate_current();
        }
    };
    serial_println!("[OK] Browser app mapped at virtual base: {:#010X}, stack: {:#010X}", user_entry, user_stack_top);
    serial_println!("[RING 0] Launching Sovereign Browser App in Ring 3 (User Mode)...");
    unsafe {
        jump_to_ring3(user_entry, user_stack_top);
    }
}

/// Spawns the native Sovereign Browser & Decentralized Web Client in Ring 3.
pub fn spawn_app_browser(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(8);
    caps.insert(Capability {
        id: CapabilityId(2301),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2302),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: id_security.0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2303),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2304),
        cap_type: CapabilityType::NetworkSocket,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2305),
        cap_type: CapabilityType::NetworkSend,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2306),
        cap_type: CapabilityType::NetworkReceive,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2307),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();

    let id = SCHEDULER.lock().spawn(
        "app_browser",
        app_browser_trampoline,
        caps,
        1,
    );
    serial_println!("[OK] Spawned Task 23: app_browser (Ring 3 User Mode, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Messages App (Ring 3)
pub extern "C" fn app_messages_trampoline() -> ! {
    serial_println!("[RING 0] Loading Sovereign Messages App binary...");
    let (user_entry, user_stack_top) = match load_user_elf_with_stack(APP_MESSAGES_ELF, 0x00DC_0000) {
        Ok(res) => res,
        Err(err) => {
            serial_println!("[FATAL] Failed to load Messages ELF: {}", err);
            crate::task::terminate_current();
        }
    };
    serial_println!("[OK] Messages app mapped at virtual base: {:#010X}, stack: {:#010X}", user_entry, user_stack_top);
    serial_println!("[RING 0] Launching Sovereign Messages App in Ring 3 (User Mode)...");
    unsafe {
        jump_to_ring3(user_entry, user_stack_top);
    }
}

/// Spawns the native Sovereign Messages & E2E Encrypted Wire Client in Ring 3.
pub fn spawn_app_messages(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(8);
    caps.insert(Capability {
        id: CapabilityId(2401),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2402),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: id_security.0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2403),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2404),
        cap_type: CapabilityType::HardwarePortIo,
        rights: Rights::READ | Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2405),
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2406),
        cap_type: CapabilityType::FileSystemWrite,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2407),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();

    let id = SCHEDULER.lock().spawn(
        "app_messages",
        app_messages_trampoline,
        caps,
        1,
    );
    serial_println!("[OK] Spawned Task 24: app_messages (Ring 3 User Mode, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Calendar App (Ring 3)
pub extern "C" fn app_calendar_trampoline() -> ! {
    serial_println!("[RING 0] Loading Sovereign Calendar App binary...");
    let (user_entry, user_stack_top) = match load_user_elf_with_stack(APP_CALENDAR_ELF, 0x00EC_0000) {
        Ok(res) => res,
        Err(err) => {
            serial_println!("[FATAL] Failed to load Calendar ELF: {}", err);
            crate::task::terminate_current();
        }
    };
    serial_println!("[OK] Calendar app mapped at virtual base: {:#010X}, stack: {:#010X}", user_entry, user_stack_top);
    serial_println!("[RING 0] Launching Sovereign Calendar App in Ring 3 (User Mode)...");
    unsafe {
        jump_to_ring3(user_entry, user_stack_top);
    }
}

/// Spawns the native Sovereign Calendar & Chrono-Planner in Ring 3.
pub fn spawn_app_calendar(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(8);
    caps.insert(Capability {
        id: CapabilityId(2501),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2502),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: id_security.0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2503),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2504),
        cap_type: CapabilityType::HardwarePortIo,
        rights: Rights::READ | Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2505),
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2506),
        cap_type: CapabilityType::FileSystemWrite,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2507),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();

    let id = SCHEDULER.lock().spawn(
        "app_calendar",
        app_calendar_trampoline,
        caps,
        1,
    );
    serial_println!("[OK] Spawned Task 25: app_calendar (Ring 3 User Mode, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Music App (Ring 3)
pub extern "C" fn app_music_trampoline() -> ! {
    serial_println!("[RING 0] Loading Sovereign Music App binary...");
    let (user_entry, user_stack_top) = match load_user_elf_with_stack(APP_MUSIC_ELF, 0x00FC_0000) {
        Ok(res) => res,
        Err(err) => {
            serial_println!("[FATAL] Failed to load Music ELF: {}", err);
            crate::task::terminate_current();
        }
    };
    serial_println!("[OK] Music app mapped at virtual base: {:#010X}, stack: {:#010X}", user_entry, user_stack_top);
    serial_println!("[RING 0] Launching Sovereign Music App in Ring 3 (User Mode)...");
    unsafe {
        jump_to_ring3(user_entry, user_stack_top);
    }
}

/// Spawns the native Sovereign Music & Hi-Fi Audio Synthesizer in Ring 3.
pub fn spawn_app_music(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(8);
    caps.insert(Capability {
        id: CapabilityId(2601),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2602),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: id_security.0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2603),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2604),
        cap_type: CapabilityType::HardwarePortIo,
        rights: Rights::READ | Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2605),
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2606),
        cap_type: CapabilityType::FileSystemWrite,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2607),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();

    let id = SCHEDULER.lock().spawn(
        "app_music",
        app_music_trampoline,
        caps,
        1,
    );
    serial_println!("[OK] Spawned Task 26: app_music (Ring 3 User Mode, ID: {:?})", id);
    id
}




