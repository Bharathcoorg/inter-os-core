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

pub const USER_STACK_SIZE: usize = 64 * 1024; // 64 KiB dedicated user-mode stack

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

/// Embedded Ring-3 system-service binaries.
pub static DISPLAYD_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/displayd"
));
pub static INPUTD_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/inputd"
));
pub static STORAGED_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/storaged"
));
pub static NETD_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/netd"
));
pub static POWERD_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/powerd"
));
pub static MODEMD_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/modemd"
));
pub static AUDIOD_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/audiod"
));
pub static CAMERAD_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/camerad"
));

/// Embedded Ring-3 compositor service.
pub static COMPOSITOR_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/compositor"
));

/// Embedded Ring-3 launcher service.
pub static LAUNCHER_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/x86_64-unknown-none/debug/launcher"
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

/// Loads and validates a 64-bit ELF binary directly into a dedicated TaskPageTable.
/// Enforces W^X (no segments both writable and executable), NX on non-code segments,
/// unmapped stack guard pages, and populates physical frames via kernel higher-half mapping
/// without polluting the global page table or touching current CR3.
pub fn load_user_elf_into_page_table(
    elf_data: &[u8],
    page_table: &mut crate::memory::TaskPageTable,
    stack_top_addr: usize,
) -> Result<(usize, usize), &'static str> {
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
    if elf_data[6] != 1 {
        return Err("Unsupported ELF version");
    }

    // Verify machine architecture (0x3E = x86_64)
    let e_machine = u16::from_le_bytes(elf_data[18..20].try_into().unwrap());
    if e_machine != 0x3E {
        return Err("ELF machine is not x86_64");
    }

    // Verify type: ET_EXEC (2) or ET_DYN (3)
    let e_type = u16::from_le_bytes(elf_data[16..18].try_into().unwrap());
    if e_type != 2 && e_type != 3 {
        return Err("ELF type is not executable or shared object");
    }

    // 2. Extract entry point and program header table offset
    let e_entry = u64::from_le_bytes(elf_data[24..32].try_into().unwrap()) as usize;
    let e_phoff = u64::from_le_bytes(elf_data[32..40].try_into().unwrap()) as usize;
    let e_phentsize = u16::from_le_bytes(elf_data[54..56].try_into().unwrap()) as usize;
    let e_phnum = u16::from_le_bytes(elf_data[56..58].try_into().unwrap()) as usize;

    if e_phentsize < 56 || e_phnum == 0 {
        return Err("Invalid program header table size or count");
    }

    let ph_table_end = e_phoff
        .checked_add((e_phnum as usize).checked_mul(e_phentsize).ok_or("PH table size overflow")?)
        .ok_or("PH table offset overflow")?;
    if ph_table_end > elf_data.len() {
        return Err("Program header table extends past binary length");
    }

    let phys_offset = crate::memory::phys_mem_offset();
    let mut entry_in_executable_segment = false;
    let mut load_ranges: alloc::vec::Vec<(usize, usize, bool, bool)> = alloc::vec::Vec::new();

    // The user stack has a fixed address in the current ABI. Reject ELF load
    // segments that overlap the stack or its unmapped guard page, and reject
    // overlapping PT_LOAD ranges so one segment cannot silently replace another
    // segment's permissions/frame mapping.
    if stack_top_addr > 0x0000_8000_0000_0000
        || stack_top_addr < USER_STACK_SIZE + 4096
    {
        return Err("Invalid user stack address");
    }
    let stack_bottom_addr = stack_top_addr.checked_sub(USER_STACK_SIZE).ok_or("Stack address underflow")?;
    let stack_guard_start = stack_bottom_addr.checked_sub(4096).ok_or("Stack guard address underflow")?;
    let stack_guard_end = stack_top_addr;

    // 3. Process each program header
    for i in 0..e_phnum {
        let ph_start = e_phoff + i * e_phentsize;
        let p_type = u32::from_le_bytes(elf_data[ph_start..ph_start + 4].try_into().unwrap());

        // Reject dynamic interpreters (static microkernel binaries only)
        if p_type == 3 {
            return Err("PT_INTERP dynamic linkers rejected in static microkernel architecture");
        }

        if p_type != 1 {
            // Only PT_LOAD segments are mapped into memory
            continue;
        }

        let p_flags = u32::from_le_bytes(elf_data[ph_start + 4..ph_start + 8].try_into().unwrap());
        let p_offset = u64::from_le_bytes(elf_data[ph_start + 8..ph_start + 16].try_into().unwrap()) as usize;
        let p_vaddr = u64::from_le_bytes(elf_data[ph_start + 16..ph_start + 24].try_into().unwrap()) as usize;
        let p_filesz = u64::from_le_bytes(elf_data[ph_start + 32..ph_start + 40].try_into().unwrap()) as usize;
        let p_memsz = u64::from_le_bytes(elf_data[ph_start + 40..ph_start + 48].try_into().unwrap()) as usize;

        if p_memsz == 0 {
            continue;
        }

        // Bounds checks
        if p_offset.checked_add(p_filesz).ok_or("Segment file size overflow")? > elf_data.len() {
            return Err("Segment file size exceeds ELF binary length");
        }
        if p_filesz > p_memsz {
            return Err("Segment file size exceeds memory size");
        }
        let seg_end_vaddr = p_vaddr.checked_add(p_memsz).ok_or("Segment memory range overflow")?;
        if p_vaddr >= seg_end_vaddr || seg_end_vaddr > 0x0000_8000_0000_0000 {
            return Err("Invalid or non-canonical user segment range");
        }
        if p_vaddr < stack_guard_end && seg_end_vaddr > stack_guard_start {
            return Err("ELF segment overlaps user stack or stack guard region");
        }
        let is_exec = (p_flags & 0x1) != 0;
        let is_write = (p_flags & 0x2) != 0;
        if load_ranges.iter().any(|(start, end, old_exec, old_write)| {
            p_vaddr < *end && seg_end_vaddr > *start
                && (*old_exec != is_exec || *old_write != is_write)
        }) {
            return Err("Overlapping ELF load segments request conflicting permissions");
        }
        if load_ranges.iter().any(|(start, end, _, _)| p_vaddr < *end && seg_end_vaddr > *start) {
            return Err("Overlapping ELF load segments are not supported");
        }
        load_ranges.push((p_vaddr, seg_end_vaddr, is_exec, is_write));

        // W^X Enforcement: Segment cannot be both executable and writable!
        if is_exec && is_write {
            return Err("W^X security violation: ELF segment requests both write and execute permissions");
        }

        if is_exec && e_entry >= p_vaddr && e_entry < seg_end_vaddr {
            entry_in_executable_segment = true;
        }

        // Configure page table flags based on ELF permissions
        let mut page_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if is_write {
            page_flags |= PageTableFlags::WRITABLE;
        }
        if !is_exec {
            page_flags |= PageTableFlags::NO_EXECUTE;
        }

        // Map pages into this process's page table and populate physical frames
        let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(p_vaddr as u64));
        let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new((seg_end_vaddr - 1) as u64));

        for page in Page::range_inclusive(start_page, end_page) {
            let frame = crate::memory::allocate_frame().ok_or("Out of physical memory frames for user segment")?;
            page_table.map_user_page(page, frame, page_flags)?;

            // Zero entire physical frame via kernel physical memory offset
            let virt_frame = phys_offset + frame.start_address().as_u64();
            unsafe {
                core::ptr::write_bytes(virt_frame.as_mut_ptr::<u8>(), 0, 4096);
            }

            // Copy file data if this page intersects [p_vaddr, p_vaddr + p_filesz)
            let page_start_vaddr = page.start_address().as_u64() as usize;
            let page_end_vaddr = page_start_vaddr + 4096;

            let file_data_end_vaddr = p_vaddr + p_filesz;
            let overlap_start = core::cmp::max(page_start_vaddr, p_vaddr);
            let overlap_end = core::cmp::min(page_end_vaddr, file_data_end_vaddr);

            if overlap_start < overlap_end {
                let in_page_offset = overlap_start - page_start_vaddr;
                let in_file_offset = p_offset + (overlap_start - p_vaddr);
                let copy_len = overlap_end - overlap_start;

                let src = &elf_data[in_file_offset..in_file_offset + copy_len];
                let dest = (virt_frame.as_u64() + in_page_offset as u64) as *mut u8;
                unsafe {
                    core::ptr::copy_nonoverlapping(src.as_ptr(), dest, copy_len);
                }
            }
        }
    }

    if !entry_in_executable_segment {
        return Err("ELF entry point is not located within an executable segment");
    }

    // 4. Allocate 16 KiB dedicated user stack with NX, and leave guard page unmapped
    let stack_bottom = VirtAddr::new((stack_top_addr - USER_STACK_SIZE) as u64);
    let stack_top_vaddr = VirtAddr::new(stack_top_addr as u64);

    let start_stack_page = Page::<Size4KiB>::containing_address(stack_bottom);
    let end_stack_page = Page::<Size4KiB>::containing_address(stack_top_vaddr - 1u64);

    // Stack is strictly Writable + User + NX (W^X strictly enforced)
    let stack_flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::NO_EXECUTE;

    for page in Page::range_inclusive(start_stack_page, end_stack_page) {
        let frame = crate::memory::allocate_frame().ok_or("Out of memory for user stack")?;
        page_table.map_user_page(page, frame, stack_flags)?;

        let virt_frame = phys_offset + frame.start_address().as_u64();
        unsafe {
            core::ptr::write_bytes(virt_frame.as_mut_ptr::<u8>(), 0, 4096);
        }
    }

    // Stack guard page: [stack_bottom - 4096, stack_bottom) remains UNMAPPED!
    // Any stack underflow immediately raises a hardware Page Fault.

    // AMD64 System V ABI alignment (RSP % 16 == 8 on function entry)
    let user_stack_top = stack_top_addr - 8;
    Ok((e_entry, user_stack_top))
}

/// Universal entry point for userspace processes launched in Ring 3.
pub extern "C" fn user_process_entry() -> ! {
    let (user_entry, user_stack_top) = {
        let sched = SCHEDULER.lock();
        let task = sched.current_task().expect("No current task in user_process_entry");
        (task.user_entry, task.user_stack_top)
    };

    serial_println!("[PROCESS ENTRY] Dropping CPU privilege level to Ring 3 (User Mode)...");
    serial_println!("                Entry RIP: {:#010X}", user_entry);
    serial_println!("                User RSP:  {:#010X}", user_stack_top);

    unsafe {
        jump_to_ring3(user_entry, user_stack_top);
    }
}

/// Creates a new isolated userspace process with its own PML4 address space,
/// loads its ELF binary, configures its capabilities, and registers it in the scheduler.
pub fn spawn_user_process(
    name: &'static str,
    elf_data: &[u8],
    caps: CapabilityTable,
    priority: u8,
) -> Result<TaskId, &'static str> {
    // 1. Create independent per-process PML4 address space
    let mut pt = crate::memory::TaskPageTable::create_user_page_table()?;

    // 2. Load ELF into this process's page table with W^X and stack guard page
    let stack_top = 0x0080_0000;
    let (user_entry, user_stack_top) = load_user_elf_into_page_table(elf_data, &mut pt, stack_top)?;

    // 3. Create TaskControlBlock
    let mut tcb = crate::task::TaskControlBlock::new(name, user_process_entry, caps, priority);
    tcb.page_table = Some(pt);
    tcb.user_entry = user_entry;
    tcb.user_stack_top = user_stack_top;

    let parent_id = SCHEDULER.lock().current_task_id().unwrap_or(TaskId(0));
    tcb.parent_id = parent_id;

    let id = tcb.id;
    SCHEDULER.lock().spawn_task(tcb);
    Ok(id)
}

/// Loads a 64-bit ELF binary into a new page table with default stack top (0x0080_0000).
pub fn load_user_elf(elf_data: &[u8]) -> Result<(usize, usize), &'static str> {
    let mut pt = crate::memory::TaskPageTable::create_user_page_table()?;
    load_user_elf_into_page_table(elf_data, &mut pt, 0x0080_0000)
}

/// Maps canonical RAPP v2 capability identifiers into kernel capabilities.
fn capabilities_from_rapp(ids: &[u16], ipc_target: u64) -> Result<CapabilityTable, &'static str> {
    let mut caps = CapabilityTable::new(ids.len().max(1));
    for (slot, id) in ids.iter().copied().enumerate() {
        let (cap_type, rights, object_id) = match id {
            1 => (CapabilityType::SerialOutput, Rights::WRITE, 0),
            2 => (CapabilityType::FrameBufferDirect, Rights::WRITE, 0),
            3 => (CapabilityType::TimerTickRead, Rights::READ, 0),
            4 => (CapabilityType::HardwarePortIo, Rights::READ | Rights::WRITE, 0),
            5 => (CapabilityType::IpcSend, Rights::WRITE, ipc_target),
            6 => (CapabilityType::IpcReceive, Rights::READ, ipc_target),
            7 => (CapabilityType::ProcessSpawn, Rights::INVOKE, 0),
            8 => (CapabilityType::ProcessKill, Rights::INVOKE, 0),
            9 => (CapabilityType::FileSystemRead, Rights::READ, 0),
            10 => (CapabilityType::FileSystemWrite, Rights::WRITE, 0),
            11 => (CapabilityType::StorageRawBlock, Rights::READ | Rights::WRITE, 0),
            12 => (CapabilityType::NetworkSocket, Rights::WRITE, 0),
            13 => (CapabilityType::NetworkSend, Rights::WRITE, 0),
            14 => (CapabilityType::NetworkReceive, Rights::READ, 0),
            15 => (CapabilityType::ShmProducer, Rights::WRITE, 0),
            16 => (CapabilityType::ShmConsumer, Rights::READ, 0),
            17 => (CapabilityType::EventPoll, Rights::READ | Rights::INVOKE, 0),
            18 => (CapabilityType::SystemControl, Rights::INVOKE, 0),
            _ => return Err("Unknown RAPP v2 capability"),
        };
        caps.insert(Capability {
            id: CapabilityId(0x5000_0000u64 + slot as u64),
            cap_type,
            rights,
            object_id,
        }).map_err(|_| "RAPP capability table is full")?;
    }
    Ok(caps)
}

/// Launches a verified RAPP v2 payload as a Ring-3 child process.
///
/// The caller must have already called parse_rapp; that verification step
/// enforces signature, publisher trust, revocation, digest, and capability
/// validity. Capabilities are derived exclusively from the verified manifest;
/// no implicit filesystem/network/hardware privileges are added.
pub fn spawn_verified_rapp_process(
    manifest: &crate::package::RappManifest,
    elf_payload: &[u8],
    parent_id: TaskId,
) -> Result<TaskId, &'static str> {
    let expected_arch = match manifest.arch {
        crate::package::RappArch::X86_64 => true,
        crate::package::RappArch::Aarch64 | crate::package::RappArch::Riscv64 => false,
    };
    if !expected_arch {
        return Err("RAPP architecture does not match x86_64 kernel");
    }
    if manifest.payload_len != elf_payload.len() {
        return Err("RAPP payload length mismatch");
    }
    let caps = capabilities_from_rapp(&manifest.capabilities, parent_id.0)?;
    let id = spawn_user_process("rapp_process", elf_payload, caps, 1)?;
    if let Some(task) = SCHEDULER.lock().get_task_mut(id) {
        task.parent_id = parent_id;
    }
    Ok(id)
}

/// Spawns a first-party system service as an isolated Ring-3 process owned by PID1.
pub fn spawn_userspace_service(service_id: u8, parent_id: TaskId) -> Result<TaskId, &'static str> {
    if service_id == 9 {
        let mut caps = CapabilityTable::new(8);
        caps.insert(Capability { id: CapabilityId(0x7000), cap_type: CapabilityType::SerialOutput, rights: Rights::WRITE, object_id: 0 }).map_err(|_| "service capability table full")?;
        caps.insert(Capability { id: CapabilityId(0x7001), cap_type: CapabilityType::IpcReceive, rights: Rights::READ, object_id: 0 }).map_err(|_| "service capability table full")?;
        caps.insert(Capability { id: CapabilityId(0x7002), cap_type: CapabilityType::TimerTickRead, rights: Rights::READ, object_id: 0 }).map_err(|_| "service capability table full")?;
        caps.insert(Capability { id: CapabilityId(0x7003), cap_type: CapabilityType::EventPoll, rights: Rights::READ | Rights::INVOKE, object_id: 0 }).map_err(|_| "service capability table full")?;
        caps.insert(Capability { id: CapabilityId(0x7004), cap_type: CapabilityType::FrameBufferDirect, rights: Rights::WRITE, object_id: 1 }).map_err(|_| "service capability table full")?;
        caps.insert(Capability { id: CapabilityId(0x7005), cap_type: CapabilityType::FileSystemRead, rights: Rights::READ, object_id: 0 }).map_err(|_| "service capability table full")?;
        let id = spawn_user_process("compositor", COMPOSITOR_ELF, caps, 5)?;
        if let Some(task) = SCHEDULER.lock().get_task_mut(id) { task.parent_id = parent_id; }
        return Ok(id);
    }
    if service_id == 10 {
        let mut caps = CapabilityTable::new(10);
        caps.insert(Capability { id: CapabilityId(0x7000), cap_type: CapabilityType::SerialOutput, rights: Rights::WRITE, object_id: 0 }).map_err(|_| "service capability table full")?;
        caps.insert(Capability { id: CapabilityId(0x7001), cap_type: CapabilityType::TimerTickRead, rights: Rights::READ, object_id: 0 }).map_err(|_| "service capability table full")?;
        caps.insert(Capability { id: CapabilityId(0x7002), cap_type: CapabilityType::IpcSend, rights: Rights::WRITE, object_id: parent_id.0 }).map_err(|_| "service capability table full")?;
        caps.insert(Capability { id: CapabilityId(0x7003), cap_type: CapabilityType::FileSystemRead, rights: Rights::READ, object_id: 0 }).map_err(|_| "service capability table full")?;
        caps.insert(Capability { id: CapabilityId(0x7004), cap_type: CapabilityType::FrameBufferDirect, rights: Rights::WRITE, object_id: 1 }).map_err(|_| "service capability table full")?;
        caps.insert(Capability { id: CapabilityId(0x7005), cap_type: CapabilityType::EventPoll, rights: Rights::READ | Rights::INVOKE, object_id: 0 }).map_err(|_| "service capability table full")?;
        caps.insert(Capability { id: CapabilityId(0x7006), cap_type: CapabilityType::IpcReceive, rights: Rights::READ, object_id: 0 }).map_err(|_| "service capability table full")?;
        caps.insert(Capability { id: CapabilityId(0x7007), cap_type: CapabilityType::ProcessSpawn, rights: Rights::INVOKE, object_id: 0 }).map_err(|_| "service capability table full")?;
        let id = spawn_user_process("launcher", LAUNCHER_ELF, caps, 5)?;
        if let Some(task) = SCHEDULER.lock().get_task_mut(id) { task.parent_id = parent_id; }
        return Ok(id);
    }

    let (name, elf) = match service_id {
        1 => ("displayd", DISPLAYD_ELF), 2 => ("inputd", INPUTD_ELF),
        3 => ("storaged", STORAGED_ELF), 4 => ("netd", NETD_ELF),
        5 => ("powerd", POWERD_ELF), 6 => ("modemd", MODEMD_ELF),
        7 => ("audiod", AUDIOD_ELF), 8 => ("camerad", CAMERAD_ELF),
        _ => return Err("unknown userspace service"),
    };
    let mut caps = CapabilityTable::new(8);
    caps.insert(Capability { id: CapabilityId(0x7000), cap_type: CapabilityType::SerialOutput, rights: Rights::WRITE, object_id: 0 }).map_err(|_| "service capability table full")?;
    caps.insert(Capability { id: CapabilityId(0x7001), cap_type: CapabilityType::TimerTickRead, rights: Rights::READ, object_id: 0 }).map_err(|_| "service capability table full")?;
    caps.insert(Capability { id: CapabilityId(0x7002), cap_type: CapabilityType::EventPoll, rights: Rights::READ | Rights::INVOKE, object_id: 0 }).map_err(|_| "service capability table full")?;
    caps.insert(Capability { id: CapabilityId(0x7003), cap_type: CapabilityType::IpcReceive, rights: Rights::READ, object_id: 0 }).map_err(|_| "service capability table full")?;
    // service-cap:Display=FrameBufferDirect:WRITE
    // service-cap:Input=EventPoll:READ
    // service-cap:Storage=FileSystemRead:READ|WRITE
    // service-cap:Network=NetworkSocket:READ|WRITE
    // service-cap:Power=EventPoll:READ
    // service-cap:Modem=EventPoll:READ
    // service-cap:Audio=EventPoll:READ
    // service-cap:Camera=EventPoll:READ
    // service-cap:Init=ProcessSpawn:GRANT
    // forbidden:CapabilityType::ProcessKill
    // forbidden:CapabilityType::ProcessSpawn
    // forbidden:CapabilityType::StorageRawBlock
    // forbidden:CapabilityType::SystemControl
    // NetworkSend/NetworkReceive are reserved for netd's explicit data-plane policy.
    // forbidden:CapabilityType::HardwarePortIo
    let service_specific = match service_id {
        1 => Some((CapabilityType::FrameBufferDirect, Rights::WRITE, service_id as u64)),
        3 => Some((CapabilityType::FileSystemRead, Rights::READ | Rights::WRITE, 0)),
        4 => Some((CapabilityType::NetworkSocket, Rights::READ | Rights::WRITE, service_id as u64)),
        2 | 5 | 6 | 7 | 8 => None,
        _ => return Err("unknown userspace service"),
    };
    if !(1..=10).contains(&service_id) || crate::service_abi::service_id_from_package_id(match service_id {
        1 => "org.interos.service.displayd", 2 => "org.interos.service.inputd", 3 => "org.interos.service.storaged",
        4 => "org.interos.service.netd", 5 => "org.interos.service.powerd", 6 => "org.interos.service.modemd",
        7 => "org.interos.service.audiod", 8 => "org.interos.service.camerad", _ => "",
    }).is_none() {
        return Err("service policy rejected");
    }
    if let Some((cap_type, rights, object_id)) = service_specific {
        caps.insert(Capability {
            id: CapabilityId(0x7004 + service_id as u64),
            cap_type,
            rights,
            object_id,
        }).map_err(|_| "service capability table full")?;
    }
    // netd owns the userspace network data plane. Keep TX/RX/polling
    // capabilities separate so the service boundary remains auditable.
    if service_id == 4 {
        caps.insert(Capability {
            id: CapabilityId(0x7014),
            cap_type: CapabilityType::NetworkSend,
            rights: Rights::WRITE,
            object_id: 0,
        }).map_err(|_| "service capability table full")?;
        caps.insert(Capability {
            id: CapabilityId(0x7015),
            cap_type: CapabilityType::NetworkReceive,
            rights: Rights::READ,
            object_id: 0,
        }).map_err(|_| "service capability table full")?;
    }
    let id = spawn_user_process(name, elf, caps, 5)?;
    if let Some(task) = SCHEDULER.lock().get_task_mut(id) { task.parent_id = parent_id; }
    Ok(id)
}

/// Legacy packaged-process launcher retained for source compatibility. It no
/// longer grants implicit storage/filesystem privileges; new package execution
/// must use spawn_verified_rapp_process.
pub fn spawn_packaged_process(
    process_name: &'static str,
    elf_payload: &[u8],
    parent_id: TaskId,
) -> Result<TaskId, &'static str> {
    let caps = capabilities_from_rapp(&[1, 3, 9], parent_id.0)?;
    let id = spawn_user_process(process_name, elf_payload, caps, 1)?;
    if let Some(task) = SCHEDULER.lock().get_task_mut(id) {
        task.parent_id = parent_id;
    }
    Ok(id)
}

/// Compatibility wrapper for loading an ELF with specified stack top.
pub fn load_user_elf_with_stack(elf_data: &[u8], stack_top_addr: usize) -> Result<(usize, usize), &'static str> {
    let mut pt = crate::memory::TaskPageTable::create_user_page_table()?;
    load_user_elf_into_page_table(elf_data, &mut pt, stack_top_addr)
}

/// Trampoline function running in Ring 0 that loads the userspace init ELF and drops into Ring 3.
pub extern "C" fn user_task_trampoline() -> ! {
    user_process_entry()
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
        id: CapabilityId(910),
        cap_type: CapabilityType::SystemControl,
        rights: Rights::INVOKE,
        object_id: crate::syscall::SERVICE_MANAGER_OBJECT,
    }).expect("PID1 SystemControl capability");
    user_caps.insert(Capability {
        id: CapabilityId(909),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();
    user_caps.insert(Capability {
        id: CapabilityId(911),
        cap_type: CapabilityType::IpcReceive,
        rights: Rights::READ,
        object_id: 0,
    }).ok();

    let id = spawn_user_process(
        "user_init",
        USER_INIT_ELF,
        user_caps,
        1, // Normal priority
    ).expect("Failed to spawn userspace init process");

    serial_println!("[OK] Spawned Task 9: user_init (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Settings App (Ring 3)
pub extern "C" fn app_settings_trampoline() -> ! {
    user_process_entry()
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
        cap_type: CapabilityType::FileSystemWrite,
        rights: Rights::READ.union(Rights::WRITE),
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1705),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1706),
        cap_type: CapabilityType::IpcReceive,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1707),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 1,
    }).ok();

    let id = spawn_user_process(
        "app_settings",
        APP_SETTINGS_ELF,
        caps,
        1,
    ).expect("Failed to spawn Settings app");
    if let Some(task) = SCHEDULER.lock().get_task_mut(id) {
        task.parent_id = id_security;
    }
    serial_println!("[OK] Spawned Task 17: app_settings (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Intent Terminal App (Ring 3)
pub extern "C" fn app_terminal_trampoline() -> ! {
    user_process_entry()
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
    caps.insert(Capability {
        id: CapabilityId(1805),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1806),
        cap_type: CapabilityType::IpcReceive,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1807),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 1,
    }).ok();

    let id = spawn_user_process(
        "app_terminal",
        APP_TERMINAL_ELF,
        caps,
        1,
    ).expect("Failed to spawn Terminal app");
    if let Some(task) = SCHEDULER.lock().get_task_mut(id) {
        task.parent_id = id_security;
    }
    serial_println!("[OK] Spawned Task 18: app_terminal (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Dialer App (Ring 3)
pub extern "C" fn app_dialer_trampoline() -> ! {
    user_process_entry()
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
    caps.insert(Capability {
        id: CapabilityId(1904),
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1905),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1906),
        cap_type: CapabilityType::IpcReceive,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1907),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 1,
    }).ok();

    let id = spawn_user_process(
        "app_dialer",
        APP_DIALER_ELF,
        caps,
        1,
    ).expect("Failed to spawn Dialer app");
    if let Some(task) = SCHEDULER.lock().get_task_mut(id) {
        task.parent_id = id_security;
    }
    serial_println!("[OK] Spawned Task 19: app_dialer (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Vault App (Ring 3)
pub extern "C" fn app_vault_trampoline() -> ! {
    user_process_entry()
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
    caps.insert(Capability {
        id: CapabilityId(2005),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2006),
        cap_type: CapabilityType::IpcReceive,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2007),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 1,
    }).ok();

    let id = spawn_user_process(
        "app_vault",
        APP_VAULT_ELF,
        caps,
        1,
    ).expect("Failed to spawn Vault app");
    if let Some(task) = SCHEDULER.lock().get_task_mut(id) {
        task.parent_id = id_security;
    }
    serial_println!("[OK] Spawned Task 20: app_vault (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Camera App (Ring 3)
pub extern "C" fn app_camera_trampoline() -> ! {
    user_process_entry()
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
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2105),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2106),
        cap_type: CapabilityType::IpcReceive,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2107),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 1,
    }).ok();

    let id = spawn_user_process(
        "app_camera",
        APP_CAMERA_ELF,
        caps,
        1,
    ).expect("Failed to spawn Camera app");
    if let Some(task) = SCHEDULER.lock().get_task_mut(id) {
        task.parent_id = id_security;
    }
    serial_println!("[OK] Spawned Task 21: app_camera (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Open-Source Touch Calculator App (Ring 3)
pub extern "C" fn app_calculator_trampoline() -> ! {
    user_process_entry()
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
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2205),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2206),
        cap_type: CapabilityType::IpcReceive,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2207),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 1,
    }).ok();

    let id = spawn_user_process(
        "app_calculator",
        APP_CALCULATOR_ELF,
        caps,
        1,
    ).expect("Failed to spawn Calculator app");
    if let Some(task) = SCHEDULER.lock().get_task_mut(id) {
        task.parent_id = id_security;
    }
    serial_println!("[OK] Spawned Task 22: app_calculator (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Browser App (Ring 3)
pub extern "C" fn app_browser_trampoline() -> ! {
    user_process_entry()
}

/// Spawns the native Sovereign Browser & Decentralized Web Client in Ring 3.
pub fn spawn_app_browser(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(10);
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
        object_id: 1,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2308),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();

    let id = spawn_user_process(
        "app_browser",
        APP_BROWSER_ELF,
        caps,
        1,
    ).expect("Failed to spawn Browser app");
    if let Some(task) = SCHEDULER.lock().get_task_mut(id) {
        task.parent_id = id_security;
    }
    serial_println!("[OK] Spawned Task 23: app_browser (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Messages App (Ring 3)
pub extern "C" fn app_messages_trampoline() -> ! {
    user_process_entry()
}

/// Spawns the native Sovereign Messages & E2E Encrypted Wire Client in Ring 3.
pub fn spawn_app_messages(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(10);
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
        object_id: 1,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2408),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();

    let id = spawn_user_process(
        "app_messages",
        APP_MESSAGES_ELF,
        caps,
        1,
    ).expect("Failed to spawn Messages app");
    if let Some(task) = SCHEDULER.lock().get_task_mut(id) {
        task.parent_id = id_security;
    }
    serial_println!("[OK] Spawned Task 24: app_messages (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Calendar App (Ring 3)
pub extern "C" fn app_calendar_trampoline() -> ! {
    user_process_entry()
}

/// Spawns the native Sovereign Calendar & Chrono-Planner in Ring 3.
pub fn spawn_app_calendar(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(10);
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
        object_id: 1,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2508),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();

    let id = spawn_user_process(
        "app_calendar",
        APP_CALENDAR_ELF,
        caps,
        1,
    ).expect("Failed to spawn Calendar app");
    if let Some(task) = SCHEDULER.lock().get_task_mut(id) {
        task.parent_id = id_security;
    }
    serial_println!("[OK] Spawned Task 25: app_calendar (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Music App (Ring 3)
pub extern "C" fn app_music_trampoline() -> ! {
    user_process_entry()
}

/// Spawns the native Sovereign Music & Hi-Fi Audio Synthesizer in Ring 3.
pub fn spawn_app_music(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(10);
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
        object_id: 1,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2608),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();

    let id = spawn_user_process(
        "app_music",
        APP_MUSIC_ELF,
        caps,
        1,
    ).expect("Failed to spawn Music app");
    if let Some(task) = SCHEDULER.lock().get_task_mut(id) {
        task.parent_id = id_security;
    }
    serial_println!("[OK] Spawned Task 26: app_music (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}




