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
use super::images::*;

pub const USER_STACK_SIZE: usize = 256 * 1024;
/// Where every user stack ends.
///
/// WHY 64 MiB and not the old 16 MiB: a user image's writable segment sits
/// immediately above its text, so the shell's fixed buffers grow upward from
/// 4 MiB. With the vector-text coverage cache sized for the hero type rung,
/// the launcher's segment now ends at 0x806b60 and video_player's at
/// 0x80d650. In addition, applications such as Gallery allocate static frame
/// buffers and thumbnail caches whose writable/BSS segments extend up to ~16 MiB
/// (0xfc694c), encroaching upon the 16 MiB stack guard. Raising the stack top
/// to 64 MiB (0x0400_0000) provides ample headroom for user images while staying
/// well within the userspace address limit.
pub const USER_STACK_TOP: usize = 0x0400_0000;
pub const USER_ADDRESS_SPACE_LIMIT: usize = 256 * 1024 * 1024;

/// User Code Segment Selector with RPL = 3 (GDT Index 4: 0x20 | 3 = 0x23)
pub const USER_CS_SELECTOR: u64 = 0x23;

/// User Data Segment Selector with RPL = 3 (GDT Index 3: 0x18 | 3 = 0x1B)
pub const USER_SS_SELECTOR: u64 = 0x1B;

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

    // This loader intentionally implements a fixed-address static ELF ABI.
    // ET_DYN requires load-bias/relocation processing, which is not part of this
    // loader contract; accepting it would execute relocation-bearing images with
    // unrelocated addresses.
    let e_type = u16::from_le_bytes(elf_data[16..18].try_into().unwrap());
    if e_type != 2 {
        return Err("ELF must be ET_EXEC; dynamic/shared objects are unsupported");
    }

    // 2. Extract entry point and program header table offset
    let e_entry = u64::from_le_bytes(elf_data[24..32].try_into().unwrap()) as usize;
    let e_phoff = u64::from_le_bytes(elf_data[32..40].try_into().unwrap()) as usize;
    let e_phentsize = u16::from_le_bytes(elf_data[54..56].try_into().unwrap()) as usize;
    let e_phnum = u16::from_le_bytes(elf_data[56..58].try_into().unwrap()) as usize;

    // ELF64 program headers are exactly 56 bytes in this static loader ABI.
    if e_phentsize != 56 || e_phnum == 0 {
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
    if e_entry < 0x1000 || e_entry >= 0x0000_8000_0000_0000 {
        return Err("ELF entry point is outside the canonical user range");
    }
    let mut mapped_address_space = 0usize;
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
        let p_align = u64::from_le_bytes(elf_data[ph_start + 48..ph_start + 56].try_into().unwrap());

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
        if p_vaddr < 0x1000 {
            return Err("ELF segment overlaps the reserved null page");
        }
        if p_align != 0 && (p_align & (p_align - 1)) != 0 {
            return Err("ELF segment alignment is not a power of two");
        }
        if p_align > 1 && (p_vaddr as u64) % p_align != (p_offset as u64) % p_align {
            return Err("ELF segment virtual/file alignment mismatch");
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
        mapped_address_space = mapped_address_space
            .checked_add(((seg_end_vaddr - p_vaddr + 4095) / 4096) * 4096)
            .ok_or("userspace address-space size overflow")?;
        if mapped_address_space.checked_add(USER_STACK_SIZE).ok_or("userspace address-space size overflow")? > USER_ADDRESS_SPACE_LIMIT {
            return Err("userspace address-space limit exceeded");
        }

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
            if let Err(error) = page_table.map_user_page(page, frame, page_flags) {
                // map_user_page does not consume the leaf frame on failure.
                crate::memory::recycle_frame(frame);
                return Err(error);
            }

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
        if let Err(error) = page_table.map_user_page(page, frame, stack_flags) {
            crate::memory::recycle_frame(frame);
            return Err(error);
        }

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

/// Maps a distinct guarded stack for a thread in an existing userspace address space.
pub fn map_user_thread_stack(
    page_table: &mut crate::memory::TaskPageTable,
    task_id: TaskId,
) -> Result<usize, &'static str> {
    let stride = 1024 * 1024usize;
    let base = 0x0000_7000_0000_0000usize;
    let offset = (task_id.0 as usize).checked_mul(stride).ok_or("thread stack address overflow")?;
    let top = base.checked_sub(offset).ok_or("thread stack address underflow")?;
    if top <= USER_STACK_SIZE + 4096 || top >= 0x0000_8000_0000_0000 {
        return Err("thread stack outside user range");
    }

    let bottom = top - USER_STACK_SIZE;
    let start = Page::<Size4KiB>::containing_address(VirtAddr::new(bottom as u64));
    let end = Page::<Size4KiB>::containing_address(VirtAddr::new((top - 1) as u64));
    let flags = PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::NO_EXECUTE;
    let mut mapped = alloc::vec::Vec::new();

    for page in Page::range_inclusive(start, end) {
        if page_table.translate(page.start_address()).is_some() {
            for mapped_page in mapped.drain(..) {
                let _ = page_table.unmap_user_page(mapped_page);
            }
            return Err("thread stack virtual range already mapped");
        }

        let frame = match crate::memory::allocate_frame() {
            Some(frame) => frame,
            None => {
                for mapped_page in mapped.drain(..) {
                    let _ = page_table.unmap_user_page(mapped_page);
                }
                return Err("out of memory for thread stack");
            }
        };

        if let Err(error) = page_table.map_user_page(page, frame, flags) {
            crate::memory::recycle_frame(frame);
            for mapped_page in mapped.drain(..) {
                let _ = page_table.unmap_user_page(mapped_page);
            }
            return Err(error);
        }

        let virt = crate::memory::phys_mem_offset() + frame.start_address().as_u64();
        unsafe { core::ptr::write_bytes(virt.as_mut_ptr::<u8>(), 0, 4096); }
        mapped.push(page);
    }

    Ok(top - 8)
}

/// Unmaps every page owned by a userspace thread stack.
///
/// Rollback attempts the complete deterministic stack range even if one mapping
/// is already missing or corrupted. The first unexpected error is returned
/// after all pages have been attempted.
pub fn unmap_user_thread_stack(
    page_table: &mut crate::memory::TaskPageTable,
    task_id: TaskId,
) -> Result<(), &'static str> {
    let stride = 1024 * 1024usize;
    let base = 0x0000_7000_0000_0000usize;
    let offset = (task_id.0 as usize).checked_mul(stride).ok_or("thread stack address overflow")?;
    let top = base.checked_sub(offset).ok_or("thread stack address underflow")?;
    if top <= USER_STACK_SIZE + 4096 || top >= 0x0000_8000_0000_0000 {
        return Err("thread stack outside user range");
    }

    let bottom = top - USER_STACK_SIZE;
    let start = Page::<Size4KiB>::containing_address(VirtAddr::new(bottom as u64));
    let end = Page::<Size4KiB>::containing_address(VirtAddr::new((top - 1) as u64));
    let mut first_error = None;

    for page in Page::range_inclusive(start, end) {
        match page_table.unmap_user_page(page) {
            Ok(()) | Err("userspace page is not mapped") => {}
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
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

/// Drops CPU execution privilege from Ring 0 to Ring 3 for a newly spawned thread,
/// setting the thread entry point, user stack top, TLS base (via MSR FS_BASE), and entry argument in RDI.
#[inline(never)]
pub unsafe fn jump_to_ring3_thread(
    user_entry: usize,
    user_stack_top: usize,
    tls_base: u64,
    user_arg: u64,
) -> ! {
    if tls_base != 0 {
        crate::syscall::entry::wrmsr(crate::syscall::entry::msr::IA32_FS_BASE, tls_base);
    }

    core::arch::asm!(
        "mov ds, ax",
        "mov es, ax",
        "mov fs, ax",
        "mov gs, ax",

        "push {ss:r}",
        "push {rsp}",
        "push 0x202",
        "push {cs:r}",
        "push {rip}",

        "xor rax, rax",
        "xor rbx, rbx",
        "xor rcx, rcx",
        "xor rdx, rdx",
        "xor rsi, rsi",
        "mov rdi, {arg}",
        "xor rbp, rbp",
        "xor r8, r8",
        "xor r9, r9",
        "xor r10, r10",
        "xor r11, r11",
        "xor r12, r12",
        "xor r13, r13",
        "xor r14, r14",
        "xor r15, r15",

        "iretq",

        ss = in(reg) USER_SS_SELECTOR,
        rsp = in(reg) user_stack_top as u64,
        cs = in(reg) USER_CS_SELECTOR,
        rip = in(reg) user_entry as u64,
        arg = in(reg) user_arg,
        in("ax") (USER_SS_SELECTOR as u16),
        options(noreturn)
    );
}

/// Universal entry point for userspace threads launched in Ring 3.
pub extern "C" fn user_thread_entry() -> ! {
    let (user_entry, user_stack_top, tls_base, user_arg) = {
        let sched = SCHEDULER.lock();
        let task = sched.current_task().expect("No current task in user_thread_entry");
        (task.user_entry, task.user_stack_top, task.tls_base, task.user_arg)
    };

    serial_println!("[THREAD ENTRY] Dropping CPU privilege level to Ring 3 (User Mode)...");
    serial_println!("               Entry RIP: {:#010X}", user_entry);
    serial_println!("               User RSP:  {:#010X}", user_stack_top);
    serial_println!("               TLS Base:  {:#010X}", tls_base);

    unsafe {
        jump_to_ring3_thread(user_entry, user_stack_top, tls_base, user_arg);
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
    let parent_id = SCHEDULER.lock().current_task_id().unwrap_or(TaskId(0));
    spawn_user_process_with_parent(name, elf_data, caps, priority, parent_id)
}

/// Creates an isolated userspace process with an explicit parent identity.
///
/// Keeping parent assignment inside task creation makes the process table and
/// scheduler agree on ownership; callers no longer need to mutate a TCB after
/// it has already been registered as a process.
pub fn spawn_user_process_with_parent(
    name: &'static str,
    elf_data: &[u8],
    caps: CapabilityTable,
    priority: u8,
    parent_id: TaskId,
) -> Result<TaskId, &'static str> {
    spawn_user_process_with_limits(
        name,
        elf_data,
        caps,
        priority,
        parent_id,
        crate::task::process::ResourceLimits::default(),
    )
}

/// Creates a userspace process with an explicit resource budget. The budget is
/// enforced before scheduler registration so oversized images cannot transiently
/// enter the process table under a looser default policy.
pub fn spawn_user_process_with_limits(
    name: &'static str,
    elf_data: &[u8],
    caps: CapabilityTable,
    priority: u8,
    parent_id: TaskId,
    limits: crate::task::process::ResourceLimits,
) -> Result<TaskId, &'static str> {
    // WHY re-key here, at the single funnel every spawner passes through: app
    // capability tables are built from hardcoded constants (CapabilityId(2601),
    // CapabilityId(1701), ... -- 101 of them across 12 spawn functions), and
    // `revoke_all` publishes those exact ids into the kernel-wide revocation set on
    // task exit without ever evicting them. A relaunch therefore created a table
    // whose every slot was already revoked: the app was born with no authorizable
    // capability, `surface_create` was refused, and it rendered nothing. Re-keying
    // once here restores the invariant that an identity is minted at most once per
    // boot, and covers every present and future spawner at the same time.
    let mut caps = caps;
    caps.rekey_all();

    let mut pt = crate::memory::TaskPageTable::create_user_page_table()?;
    let stack_top = USER_STACK_TOP;
    let (user_entry, user_stack_top) = load_user_elf_into_page_table(elf_data, &mut pt, stack_top)?;
    let address_space_bytes = pt.user_mapped_bytes();
    if address_space_bytes as u64 > limits.max_address_space_bytes {
        return Err("userspace address-space budget exceeded");
    }

    let mut tcb = crate::task::TaskControlBlock::new(name, user_process_entry, caps, priority);
    tcb.cr3_frame = Some(pt.frame());
    tcb.page_table = Some(alloc::sync::Arc::new(spin::Mutex::new(pt)));
    tcb.user_entry = user_entry;
    tcb.user_stack_top = user_stack_top;
    tcb.parent_id = parent_id;

    let _id = tcb.id;
    let spawned = SCHEDULER.lock().spawn_task(tcb);
    if spawned == TaskId(0) {
        return Err("process registration failed");
    }
    let process = match crate::task::process::process_for_task(spawned) {
        Some(process) => process,
        None => {
            let mut scheduler = SCHEDULER.lock();
            let _ = scheduler.terminate_task(spawned, -1);
            let _ = scheduler.reap_child(parent_id, spawned);
            return Err("process identity missing after spawn");
        }
    };
    if let Err(error) = crate::task::process::set_address_space_usage(process, address_space_bytes) {
        let mut scheduler = SCHEDULER.lock();
        let _ = scheduler.terminate_task(spawned, -1);
        let _ = scheduler.reap_child(parent_id, spawned);
        return Err(error);
    }

    let is_application = crate::package::first_party_package_by_process_name(name).is_some()
        || name.starts_with("org.interos.");
    if is_application {
        let user_id = crate::user::USER_MANAGER.lock().active_user().0;
        if user_id == crate::user::UserId::SYSTEM.0
            || crate::task::process::bind_user_for_task(spawned, user_id).is_err()
        {
            let mut scheduler = SCHEDULER.lock();
            let _ = scheduler.terminate_task(spawned, -1);
            let _ = scheduler.reap_child(parent_id, spawned);
            return Err("application user identity unavailable");
        }
    }

    Ok(spawned)
}

/// Loads a 64-bit ELF binary into a new page table with the default stack top.
pub fn load_user_elf(elf_data: &[u8]) -> Result<(usize, usize), &'static str> {
    let mut pt = crate::memory::TaskPageTable::create_user_page_table()?;
    load_user_elf_into_page_table(elf_data, &mut pt, USER_STACK_TOP)
}

/// Maps canonical RAPP v2 capability identifiers into kernel capabilities.
pub(crate) fn capabilities_from_rapp(ids: &[u16], ipc_target: u64) -> Result<CapabilityTable, &'static str> {
    capabilities_from_rapp_with_trust(ids, ipc_target, false)
}

pub(crate) fn capabilities_from_rapp_with_trust(
    ids: &[u16],
    ipc_target: u64,
    is_system_authority: bool,
) -> Result<CapabilityTable, &'static str> {
    let mut caps = CapabilityTable::new(ids.len().max(1));
    for (slot, id) in ids.iter().copied().enumerate() {
        if id == 18 && !is_system_authority {
            return Err("Capability 18 (SystemControl) requires system platform trust authority");
        }
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
            19 => (CapabilityType::SystemControl, Rights::INVOKE, crate::services::SERVICE_POWER_CONTROL_OBJECT),
            20 => (CapabilityType::SystemControl, Rights::INVOKE, crate::services::SERVICE_TELEPHONY_CONTROL_OBJECT),
            21 => (CapabilityType::SystemControl, Rights::INVOKE, crate::services::SERVICE_AUDIO_CONTROL_OBJECT),
            22 => (CapabilityType::SystemControl, Rights::INVOKE, crate::services::SERVICE_CAMERA_CONTROL_OBJECT),
            23 => (CapabilityType::SystemControl, Rights::INVOKE, crate::services::SERVICE_SENSOR_CONTROL_OBJECT),
            24 => (CapabilityType::RealtimeClockRead, Rights::READ, 0),
            25 => (CapabilityType::ContentIndexRead, Rights::READ, 0),
            26 => (CapabilityType::ContentIntentDispatch, Rights::INVOKE, 0),
            27 => (CapabilityType::SurfaceCapture, Rights::READ, 0),
            // 28 is the explicit PCM-output grant: it authorises open, write
            // and close of the kernel's single AC'97 stream and a read-only
            // status query. It is deliberately separate from 21 (the
            // SystemControl grant bound to the audio-control object), so a
            // media player can emit PCM without acquiring power or telephony
            // authority.
            28 => (CapabilityType::AudioStream, Rights::READ | Rights::WRITE, 0),
            // 29 is the anonymous memory mapping and userspace heap capability ("memory.heap").
            29 => (CapabilityType::MemoryHeap, Rights::READ | Rights::WRITE, 0),
            // 30 is the thread spawning and thread-local storage capability ("process.threads").
            30 => (CapabilityType::ProcessThreads, Rights::INVOKE, 0),
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
        crate::package::RappArch::Aarch64 | crate::package::RappArch::Riscv64 | crate::package::RappArch::Wasm => false,
    };
    if !expected_arch {
        return Err("RAPP architecture does not match x86_64 kernel");
    }
    if manifest.payload_len != elf_payload.len() {
        return Err("RAPP payload length mismatch");
    }
    let is_system = crate::package::REVOCATION_REGISTRY.lock().is_system_publisher(&manifest.publisher_key);
    let caps = capabilities_from_rapp_with_trust(&manifest.capabilities, parent_id.0, is_system)?;
    let process_name: &'static str = alloc::boxed::Box::leak(
        manifest.package_id.clone().into_boxed_str()
    );
    spawn_user_process_with_parent(process_name, elf_payload, caps, 6, parent_id)
}

/// Spawns a first-party system service as an isolated Ring-3 process owned by PID1.
pub fn spawn_userspace_service(service_id: u8, parent_id: TaskId) -> Result<TaskId, &'static str> {
    let service = match service_id {
        1 => crate::services::ServiceId::Display,
        2 => crate::services::ServiceId::Input,
        3 => crate::services::ServiceId::Storage,
        4 => crate::services::ServiceId::Network,
        5 => crate::services::ServiceId::Power,
        6 => crate::services::ServiceId::Modem,
        7 => crate::services::ServiceId::Audio,
        8 => crate::services::ServiceId::Camera,
        9 => crate::services::ServiceId::Compositor,
        10 => crate::services::ServiceId::Launcher,
        11 => crate::services::ServiceId::Sensor,
        12 => crate::services::ServiceId::Inference,
        13 => crate::services::ServiceId::Search,
        14 => crate::services::ServiceId::Agent,
        15 => crate::services::ServiceId::Voice,
        _ => return Err("unknown userspace service"),
    };
    let image = service_image(service_id).ok_or("unknown userspace service image")?;
    let policy = crate::services::capability_policy(service, parent_id)?;
    let mut caps = CapabilityTable::new(policy.len().max(8));
    for (slot, spec) in policy.iter().enumerate() {
        caps.insert(Capability {
            id: CapabilityId(0x7000 + slot as u64),
            cap_type: spec.cap_type,
            rights: spec.rights,
            object_id: spec.object_id,
        }).map_err(|_| "service capability table full")?;
    }
    spawn_user_process_with_limits(
        image.name,
        image.elf,
        caps,
        5,
        parent_id,
        crate::services::resource_limits(service),
    )
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
    spawn_user_process_with_parent(process_name, elf_payload, caps, 6, parent_id)
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

    let id = spawn_user_process_with_parent(
        "user_init",
        USER_INIT_ELF,
        user_caps,
        1, // Normal priority
        TaskId(0),
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
    let mut caps = CapabilityTable::new(15);
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
    // WHY a separate read capability: the manifest already declares
    // CAP_FILESYSTEM_READ, and the registry and storage-capacity gates match
    // the capability *type*, so the READ right on FileSystemWrite does not
    // satisfy them. Without this grant the package count and the root volume
    // size are refused rather than reported.
    caps.insert(Capability {
        id: CapabilityId(1713),
        cap_type: CapabilityType::FileSystemRead,
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
    // Credential authority, attenuated to the ACTIVE account only.
    let active_uid = crate::user::USER_MANAGER.lock().active_user().0;
    caps.insert(Capability {
        id: CapabilityId(1708),
        cap_type: CapabilityType::SystemControl,
        rights: Rights::INVOKE,
        object_id: crate::syscall::user_control_object(active_uid),
    }).ok();
    // Settings is the sovereign System Space component: grant sensor and power diagnostics authority
    caps.insert(Capability {
        id: CapabilityId(1709),
        cap_type: CapabilityType::SystemControl,
        rights: Rights::INVOKE,
        object_id: crate::services::SERVICE_SENSOR_CONTROL_OBJECT,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1710),
        cap_type: CapabilityType::SystemControl,
        rights: Rights::INVOKE,
        object_id: crate::services::SERVICE_POWER_CONTROL_OBJECT,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1711),
        cap_type: CapabilityType::ProcessSpawn,
        rights: Rights::INVOKE,
        object_id: 0,
    }).ok();

    // WHY the content index: this app owns the wallpaper and the ringtone
    // slots, so it has to know whether the document it was handed is one the
    // platform actually holds before it writes the slot.
    caps.insert(Capability {
        id: CapabilityId(1712),
        cap_type: CapabilityType::ContentIndexRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    let id = spawn_user_process_with_parent(
        "app_settings",
        APP_SETTINGS_ELF,
        caps,
        6,
        id_security,
    ).expect("Failed to spawn Settings app");
    serial_println!("[OK] Spawned Task 17: app_settings (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}



/// Trampoline for Sovereign Dialer App (Ring 3)
pub extern "C" fn app_dialer_trampoline() -> ! {
    user_process_entry()
}

/// Spawns the native Sovereign Dialer App in Ring 3.
pub fn spawn_app_dialer(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(12);
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
    // WHY the dialer is granted FileSystemWrite: it holds a real persisted
    // contact store -- 36-byte records, eight of them, 290 bytes of blob -- and
    // the platform's settings store is where a flat array of equal records
    // belongs, the same choice messages, calendar and music already make. The
    // grant the app actually held was FileSystemRead with `Rights::READ`, so
    // `sys_settings_set` answered PermissionDenied and every contact the user
    // typed was reported honestly as unsaved while the store was otherwise
    // complete. A read-only grant to an app whose job is to remember who you
    // call is not a capability boundary, it is a missing one.
    //
    // WHY this is safe: the VFS confines a ring-3 task to its own app storage
    // (`enforce_app_path`), so the grant cannot reach another app's data or the
    // system partition. The other four first-party apps with persisted stores
    // hold the identical grant.
    caps.insert(Capability {
        id: CapabilityId(1908),
        cap_type: CapabilityType::FileSystemWrite,
        rights: Rights::READ | Rights::WRITE | Rights::GRANT,
        object_id: 0,
    }).ok();

    // WHY the content index: the dialer shows the ringtone the user chose, and
    // the ringtone is a document the platform holds, so the dialer reads the
    // document's own record rather than the name alone.
    caps.insert(Capability {
        id: CapabilityId(1909),
        cap_type: CapabilityType::ContentIndexRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1910),
        cap_type: CapabilityType::AudioStream,
        rights: Rights::READ | Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(1911),
        cap_type: CapabilityType::SystemControl,
        rights: Rights::INVOKE,
        object_id: crate::services::SERVICE_AUDIO_CONTROL_OBJECT,
    }).ok();
    let id = spawn_user_process_with_parent(
        "app_dialer",
        APP_DIALER_ELF,
        caps,
        6,
        id_security,
    ).expect("Failed to spawn Dialer app");
    serial_println!("[OK] Spawned Task 19: app_dialer (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Vault App (Ring 3)
pub extern "C" fn app_vault_trampoline() -> ! {
    user_process_entry()
}

/// Spawns the native Sovereign Vault & Storage App in Ring 3.
pub fn spawn_app_vault(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(16);
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
        id: CapabilityId(2008),
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ,
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
    let active_uid = crate::user::USER_MANAGER.lock().active_user().0;
    caps.insert(Capability {
        id: CapabilityId(2009),
        cap_type: CapabilityType::StorageRawBlock,
        rights: Rights::INVOKE,
        object_id: crate::syscall::vault_object_for_user(active_uid),
    }).ok();

    let id = spawn_user_process_with_parent(
        "app_vault",
        APP_VAULT_ELF,
        caps,
        6,
        id_security,
    ).expect("Failed to spawn Vault app");
    serial_println!("[OK] Spawned Task 20: app_vault (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Camera App (Ring 3)
pub extern "C" fn app_camera_trampoline() -> ! {
    user_process_entry()
}

/// Spawns the native Sovereign Camera & Neural Vision App in Ring 3.
pub fn spawn_app_camera(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(12);
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
    let active_uid = crate::user::USER_MANAGER.lock().active_user().0;
    caps.insert(Capability {
        id: CapabilityId(2108),
        cap_type: CapabilityType::StorageRawBlock,
        rights: Rights::INVOKE,
        object_id: crate::syscall::vault_object_for_user(active_uid),
    }).ok();

    let id = spawn_user_process_with_parent(
        "app_camera",
        APP_CAMERA_ELF,
        caps,
        6,
        id_security,
    ).expect("Failed to spawn Camera app");
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

    let id = spawn_user_process_with_parent(
        "app_calculator",
        APP_CALCULATOR_ELF,
        caps,
        6,
        id_security,
    ).expect("Failed to spawn Calculator app");
    serial_println!("[OK] Spawned Task 22: app_calculator (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Browser App (Ring 3)
pub extern "C" fn app_browser_trampoline() -> ! {
    user_process_entry()
}

/// Spawns the native Sovereign Browser & Decentralized Web Client in Ring 3.
pub fn spawn_app_browser(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(16);
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
    // WHY the browser is granted both filesystem capabilities: it holds a real
    // persisted bookmark store and a real history, and its `parse_uri` now
    // genuinely produces `file://` and `interlayer://` — both of which need a
    // file handle. The grant it actually held was none at all, so even
    // `sys_settings_get` was refused and the two finished stores reported
    // themselves unsaved rather than looking real.
    //
    // Read and write are granted together because they are one capability here:
    // a bookmark store you cannot write is not a store, and a local document you
    // cannot open is not a browser. The VFS confines a ring-3 task to its own
    // app storage (`enforce_app_path`), so neither reaches another app's data.
    caps.insert(Capability {
        id: CapabilityId(2309),
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    // A download has to land somewhere the rest of the device can see, and the
    // shared storage area is the one place every app may look. The browser
    // saves into it and the file manager lists what the browser saved, through
    // the platform's document index.
    caps.insert(Capability {
        id: CapabilityId(2310),
        cap_type: CapabilityType::FileSystemWrite,
        rights: Rights::READ | Rights::WRITE | Rights::GRANT,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2311),
        cap_type: CapabilityType::ContentIndexRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    // WHY the intent dispatch: the Downloads screen offers Open, and opening a
    // downloaded document means handing it to the package that declared a
    // handler for its MIME type. Without this grant the platform refuses the
    // dispatch at runtime, so the button cannot keep the promise it makes.
    caps.insert(Capability {
        id: CapabilityId(2312),
        cap_type: CapabilityType::ContentIntentDispatch,
        rights: Rights::INVOKE,
        object_id: 0,
    }).ok();
    // WHY the clock: the band across the top of the screen belongs to the
    // platform, but on this kernel the app that owns the screen is the one that
    // paints it, so a browser drawing the band has to read the clock to fill
    // its lane. Read-only, and the same narrow authority the launcher, Notes
    // and Calendar already hold; with it the lane shows the time the platform
    // reported, and without it the band is drawn with the lane left blank.
    caps.insert(Capability {
        id: CapabilityId(2313),
        cap_type: CapabilityType::RealtimeClockRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    let id = spawn_user_process_with_parent(
        "app_browser",
        APP_BROWSER_ELF,
        caps,
        6,
        id_security,
    ).expect("Failed to spawn Browser app");
    serial_println!("[OK] Spawned Task 23: app_browser (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Messages App (Ring 3)
pub extern "C" fn app_messages_trampoline() -> ! {
    user_process_entry()
}

/// Spawns the native Sovereign Messages & E2E Encrypted Wire Client in Ring 3.
pub fn spawn_app_messages(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(12);
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
    let active_uid = crate::user::USER_MANAGER.lock().active_user().0;
    caps.insert(Capability {
        id: CapabilityId(2409),
        cap_type: CapabilityType::StorageRawBlock,
        rights: Rights::INVOKE,
        object_id: crate::syscall::vault_object_for_user(active_uid),
    }).ok();

    let id = spawn_user_process_with_parent(
        "app_messages",
        APP_MESSAGES_ELF,
        caps,
        6,
        id_security,
    ).expect("Failed to spawn Messages app");
    serial_println!("[OK] Spawned Task 24: app_messages (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Calendar App (Ring 3)
pub extern "C" fn app_calendar_trampoline() -> ! {
    user_process_entry()
}

/// Spawns the native Sovereign Calendar & Chrono-Planner in Ring 3.
pub fn spawn_app_calendar(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(12);
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
    caps.insert(Capability {
        id: CapabilityId(2509),
        cap_type: CapabilityType::RealtimeClockRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    let active_uid = crate::user::USER_MANAGER.lock().active_user().0;
    caps.insert(Capability {
        id: CapabilityId(2510),
        cap_type: CapabilityType::StorageRawBlock,
        rights: Rights::INVOKE,
        object_id: crate::syscall::vault_object_for_user(active_uid),
    }).ok();

    let id = spawn_user_process_with_parent(
        "app_calendar",
        APP_CALENDAR_ELF,
        caps,
        6,
        id_security,
    ).expect("Failed to spawn Calendar app");
    serial_println!("[OK] Spawned Task 25: app_calendar (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Music App (Ring 3)
pub extern "C" fn app_music_trampoline() -> ! {
    user_process_entry()
}

/// Spawns the native Sovereign Music & Hi-Fi Audio Synthesizer in Ring 3.
pub fn spawn_app_music(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(12);
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
    // WHY the content index: a downloaded file is handed to this player
    // through the platform's pending-intent slot, and reading that slot is a
    // content-index read. Without the grant the platform refuses the read and
    // the dispatched audio never reaches the library.
    caps.insert(Capability {
        id: CapabilityId(2609),
        cap_type: CapabilityType::ContentIndexRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    // WHY the music app gets explicit PCM authority: it is the system player,
    // and the only way it may emit samples is to open the AC'97 stream with
    // this capability. The grant does not include SystemControl, so it still
    // cannot change power, telephony or device policy.
    caps.insert(Capability {
        id: CapabilityId(2610),
        cap_type: CapabilityType::AudioStream,
        rights: Rights::READ | Rights::WRITE,
        object_id: 0,
    }).ok();

    let id = spawn_user_process_with_parent(
        "app_music",
        APP_MUSIC_ELF,
        caps,
        6,
        id_security,
    ).expect("Failed to spawn Music app");
    serial_println!("[OK] Spawned Task 26: app_music (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign File Manager App (Ring 3)
pub extern "C" fn app_file_manager_trampoline() -> ! {
    user_process_entry()
}

/// Spawns the native Sovereign File Manager & VFS Explorer in Ring 3.
pub fn spawn_app_file_manager(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(14);
    caps.insert(Capability {
        id: CapabilityId(2701),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2702),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: id_security.0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2703),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2704),
        cap_type: CapabilityType::StorageRawBlock,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2704),
        cap_type: CapabilityType::HardwarePortIo,
        rights: Rights::READ | Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2705),
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2706),
        cap_type: CapabilityType::FileSystemWrite,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2707),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 1,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2708),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2709),
        cap_type: CapabilityType::ProcessSpawn,
        rights: Rights::INVOKE,
        object_id: 0,
    }).ok();

    // WHY the content capabilities: the file manager lists the documents the
    // platform's index holds, so the downloads the browser saved and the
    // photographs the camera wrote are the same documents the gallery shows.
    caps.insert(Capability {
        id: CapabilityId(2709),
        cap_type: CapabilityType::ContentIndexRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2710),
        cap_type: CapabilityType::ContentIntentDispatch,
        rights: Rights::INVOKE,
        object_id: 0,
    }).ok();
    let id = spawn_user_process_with_parent(
        "app_file_manager",
        APP_FILE_MANAGER_ELF,
        caps,
        6,
        id_security,
    ).expect("Failed to spawn File Manager app");
    serial_println!("[OK] Spawned Task 27: app_file_manager (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Trampoline for Sovereign Gallery App (Ring 3)
pub extern "C" fn app_gallery_trampoline() -> ! {
    user_process_entry()
}

/// Spawns the native Sovereign Gallery & Photo Studio in Ring 3.
pub fn spawn_app_gallery(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(12);
    caps.insert(Capability {
        id: CapabilityId(2801),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2802),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: id_security.0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2803),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2804),
        cap_type: CapabilityType::HardwarePortIo,
        rights: Rights::READ | Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2805),
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2806),
        cap_type: CapabilityType::FileSystemWrite,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2807),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 1,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2808),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();

    // WHY the content capabilities: the gallery reads the platform's document
    // index so that it and the file manager describe the same documents, and
    // it dispatches intents so that a document it shows can be handed to the
    // program that declared a handler for it.
    caps.insert(Capability {
        id: CapabilityId(2809),
        cap_type: CapabilityType::ContentIndexRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2810),
        cap_type: CapabilityType::ContentIntentDispatch,
        rights: Rights::INVOKE,
        object_id: 0,
    }).ok();
    let id = spawn_user_process_with_parent(
        "app_gallery",
        APP_GALLERY_ELF,
        caps,
        6,
        id_security,
    ).expect("Failed to spawn Gallery app");
    serial_println!("[OK] Spawned Task 28: app_gallery (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Spawns the Sovereign Notes application (Ring 3).
pub fn spawn_app_notes(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(10);
    caps.insert(Capability {
        id: CapabilityId(2901),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2902),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: id_security.0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2903),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2904),
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2905),
        cap_type: CapabilityType::FileSystemWrite,
        rights: Rights::READ.union(Rights::WRITE),
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2906),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2907),
        cap_type: CapabilityType::IpcReceive,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2908),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 1,
    }).ok();
    // WHY the clock: a note records when it was edited, and the only honest
    // source for that is the device's real-time clock. The calendar is
    // already granted RealtimeClockRead for the same reason. Without this
    // the notes app can only say "No timestamp" for every note it holds.
    caps.insert(Capability {
        id: CapabilityId(2909),
        cap_type: CapabilityType::RealtimeClockRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();

    let id = spawn_user_process_with_parent(
        "app_notes",
        APP_NOTES_ELF,
        caps,
        6,
        id_security,
    ).expect("Failed to spawn Notes app");
    serial_println!("[OK] Spawned app_notes (Ring 3 User Mode, Isolated CR3, ID: {:?})", id);
    id
}

/// Spawns the agent manager.
///
/// WHY these capabilities and no others: the app's whole job is to report
/// what the device holds, so it reads the model directory, the settings
/// store, the document index and the service registry, and it draws. It
/// writes nothing, it does not drive hardware, and it holds no authority
/// over the device beyond reading. A supervisor that could change what it
/// supervises would not be a supervisor.
///
/// WHY the content index: the search surface reports the platform's
/// document layer, and reading the index is reading a description of
/// documents rather than the documents themselves.
pub fn spawn_app_agent_manager(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(16);
    caps.insert(Capability {
        id: CapabilityId(2910),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2911),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 1,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2912),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2913),
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2914),
        cap_type: CapabilityType::ContentIndexRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2915),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2916),
        cap_type: CapabilityType::FileSystemWrite,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2917),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2918),
        cap_type: CapabilityType::IpcReceive,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2919),
        cap_type: CapabilityType::InferenceService,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2923),
        cap_type: CapabilityType::SearchService,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();

    let id = spawn_user_process_with_parent(
        "app_agent_manager",
        APP_AGENT_MANAGER_ELF,
        caps,
        6,
        id_security,
    ).expect("Failed to spawn the agent manager");
    serial_println!(
        "[OK] Spawned app_agent_manager (Ring 3 User Mode, Isolated CR3, ID: {:?})",
        id
    );
    id
}

/// Trampoline for Sovereign Video Player App (Ring 3)
pub extern "C" fn app_video_player_trampoline() -> ! {
    user_process_entry()
}

/// Spawns the native Sovereign Video Player & Media Center in Ring 3.
pub fn spawn_app_video_player(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(12);
    caps.insert(Capability {
        id: CapabilityId(2920),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2921),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: id_security.0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2922),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2923),
        cap_type: CapabilityType::HardwarePortIo,
        rights: Rights::READ | Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2924),
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2925),
        cap_type: CapabilityType::FileSystemWrite,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2926),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 1,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2927),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();
    // WHY the content index: a downloaded file is handed to this player
    // through the platform's pending-intent slot, and reading that slot is a
    // content-index read. Without the grant the platform refuses the read and
    // the dispatched video never reaches the playlist.
    caps.insert(Capability {
        id: CapabilityId(2928),
        cap_type: CapabilityType::ContentIndexRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();

    let id = spawn_user_process_with_parent(
        "app_video_player",
        APP_VIDEO_PLAYER_ELF,
        caps,
        6,
        id_security,
    ).expect("Failed to spawn Video Player app");
    serial_println!(
        "[OK] Spawned app_video_player (Ring 3 User Mode, Isolated CR3, ID: {:?})",
        id
    );
    id
}

pub extern "C" fn app_webview_demo_trampoline() -> ! {
    user_process_entry()
}

pub fn spawn_app_webview_demo(id_security: TaskId) -> TaskId {
    let mut caps = CapabilityTable::new(12);
    caps.insert(Capability {
        id: CapabilityId(2930),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2931),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: id_security.0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2932),
        cap_type: CapabilityType::TimerTickRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2933),
        cap_type: CapabilityType::HardwarePortIo,
        rights: Rights::READ | Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2934),
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2935),
        cap_type: CapabilityType::FileSystemWrite,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2936),
        cap_type: CapabilityType::FrameBufferDirect,
        rights: Rights::WRITE,
        object_id: 1,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2937),
        cap_type: CapabilityType::EventPoll,
        rights: Rights::READ.union(Rights::INVOKE),
        object_id: 0,
    }).ok();
    caps.insert(Capability {
        id: CapabilityId(2938),
        cap_type: CapabilityType::WebViewBridge,
        rights: Rights::INVOKE,
        object_id: 0,
    }).ok();

    let id = spawn_user_process_with_parent(
        "app_webview_demo",
        APP_WEBVIEW_DEMO_ELF,
        caps,
        6,
        id_security,
    ).expect("Failed to spawn WebView Demo app");
    serial_println!(
        "[OK] Spawned app_webview_demo (Ring 3 User Mode, Isolated CR3, ID: {:?})",
        id
    );
    id
}
