// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V 64 Sv39 Virtual Memory Management & Page Tables
//!
//! Provides bare-metal Sv39 3-level page table setup with strict W^X enforcement,
//! user/kernel permission boundaries (PTE_U), and SATP management.

use core::arch::asm;

pub const PAGE_SIZE: usize = 4096;

// Sv39 Page Table Entry (PTE) Flags
pub const PTE_V: u64 = 1 << 0; // Valid
pub const PTE_R: u64 = 1 << 1; // Readable
pub const PTE_W: u64 = 1 << 2; // Writable
pub const PTE_X: u64 = 1 << 3; // Executable
pub const PTE_U: u64 = 1 << 4; // User accessible
pub const PTE_G: u64 = 1 << 5; // Global
pub const PTE_A: u64 = 1 << 6; // Accessed
pub const PTE_D: u64 = 1 << 7; // Dirty

// SATP Mode
pub const SATP_MODE_SV39: u64 = 8 << 60;

/// `sstatus.SUM`: when set, supervisor loads and stores to user pages bypass
/// page permissions. Firmware routinely leaves it set, so the kernel clears it
/// and re-enables it only around an explicit, bounds-checked copy.
pub const SSTATUS_SUM: usize = 1 << 18;

/// Runs `f` with supervisor access to user memory temporarily enabled.
///
/// SUM is the only way S-mode can touch a `PTE_U` page while SUM is clear, which
/// is what keeps the W^X split between the code and stack pages enforced by the
/// hardware rather than only by convention.
#[inline(always)]
pub unsafe fn with_user_memory<R>(f: impl FnOnce() -> R) -> R {
    asm!("csrrs zero, sstatus, {0}", in(reg) SSTATUS_SUM);
    let result = f();
    asm!("csrrc zero, sstatus, {0}", in(reg) SSTATUS_SUM);
    result
}

/// Returns the encoded length in bytes of the instruction at `addr`.
///
/// With the C extension enabled a 16-bit compressed instruction is distinguished
/// from a 32-bit one by the low two bits being `11` for the long form.
pub unsafe fn instruction_len(addr: usize) -> usize {
    let halfword = with_user_memory(|| core::ptr::read_volatile(addr as *const u16));
    if halfword & 0x3 != 0x3 { 2 } else { 4 }
}

#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [u64; 512],
}

impl PageTable {
    pub const fn empty() -> Self {
        Self { entries: [0; 512] }
    }

    pub fn clear(&mut self) {
        for entry in self.entries.iter_mut() {
            *entry = 0;
        }
    }
}

#[repr(C, align(4096))]
pub struct PageFrame(pub [u8; PAGE_SIZE]);

impl PageFrame {
    pub const fn new() -> Self {
        Self([0; PAGE_SIZE])
    }
}

// Static kernel root page table and secondary/tertiary tables for bare-metal boot
static mut KERNEL_ROOT_PT: PageTable = PageTable::empty();
static mut USER_LVL1_PT: PageTable = PageTable::empty();
static mut USER_LVL0_PT: PageTable = PageTable::empty();
static mut USER_PAGE_FRAME: PageFrame = PageFrame::new();
static mut USER_STACK_FRAME: PageFrame = PageFrame::new();

static mut MMIO_USER_LVL1_PT: PageTable = PageTable::empty();
static mut USER_ELF_LVL0_CODE_PT: PageTable = PageTable::empty();
static mut USER_ELF_LVL0_STACK_PT: PageTable = PageTable::empty();
pub static mut USER_ELF_CODE_PAGES: [PageFrame; 64] = [const { PageFrame::new() }; 64];
pub static mut USER_ELF_STACK_PAGES: [PageFrame; 16] = [const { PageFrame::new() }; 16];

pub const USER_CODE_VADDR: usize = 0x4000_0000;
pub const USER_STACK_TOP: usize = 0x4000_2000;

pub const ELF_USER_CODE_VADDR: usize = 0x0040_0000;
pub const ELF_USER_STACK_TOP: usize = 0x0400_0000;

/// Copies userspace code payload into the dedicated physical code frame.
pub unsafe fn load_user_code(code: &[u8]) {
    assert!(code.len() <= PAGE_SIZE, "User code exceeds 4KiB page size");
    let dest = (&raw mut USER_PAGE_FRAME.0) as *mut u8;
    core::ptr::copy_nonoverlapping(code.as_ptr(), dest, code.len());
    #[cfg(target_arch = "riscv64")]
    core::arch::asm!("fence.i");
}

/// Initializes Sv39 virtual memory, maps kernel RAM and MMIO, and activates SATP.
pub unsafe fn init_sv39() {
    let root = unsafe { &mut *(&raw mut KERNEL_ROOT_PT) };
    root.clear();

    // 1. Gigapage 0 (0x0000_0000 - 0x3FFF_FFFF): MMIO & Ring 3 ELF Userspace
    // Map Level 1 page table for Gigapage 0 so individual 2MB megapages can be configured.
    let mmio_user_lvl1 = unsafe { &mut *(&raw mut MMIO_USER_LVL1_PT) };
    mmio_user_lvl1.clear();

    // Identity-map all 2MB blocks as MMIO supervisor entries (UART, CLINT, PLIC, VirtIO)
    for i in 0..512u64 {
        if i == 2 || i == 4 {
            continue; // Reserved for user ELF code (0x0040_0000) and stack (0x0080_0000)
        }
        mmio_user_lvl1.entries[i as usize] = (i << 19) | PTE_V | PTE_R | PTE_W | PTE_A | PTE_D;
    }

    // Leaf 4KiB pages for User ELF Code at 0x0040_0000 (entry 2 in mmio_user_lvl1)
    let code_lvl0 = unsafe { &mut *(&raw mut USER_ELF_LVL0_CODE_PT) };
    code_lvl0.clear();
    for p in 0..64 {
        let paddr = (&raw const USER_ELF_CODE_PAGES[p].0) as *const u8 as u64;
        code_lvl0.entries[p] = ((paddr >> 12) << 10) | PTE_V | PTE_R | PTE_X | PTE_U | PTE_A | PTE_D;
    }
    let code_lvl0_ppn = (code_lvl0 as *const PageTable as u64) >> 12;
    mmio_user_lvl1.entries[2] = (code_lvl0_ppn << 10) | PTE_V;

    // Leaf 4KiB pages for User ELF Stack at 0x0080_0000 (entry 4 in mmio_user_lvl1)
    let stack_lvl0 = unsafe { &mut *(&raw mut USER_ELF_LVL0_STACK_PT) };
    stack_lvl0.clear();
    for p in 0..16 {
        let paddr = (&raw const USER_ELF_STACK_PAGES[p].0) as *const u8 as u64;
        stack_lvl0.entries[p] = ((paddr >> 12) << 10) | PTE_V | PTE_R | PTE_W | PTE_U | PTE_A | PTE_D;
    }
    let stack_lvl0_ppn = (stack_lvl0 as *const PageTable as u64) >> 12;
    mmio_user_lvl1.entries[4] = (stack_lvl0_ppn << 10) | PTE_V;

    // Point root entry 0 to mmio_user_lvl1
    let mmio_user_lvl1_ppn = (mmio_user_lvl1 as *const PageTable as u64) >> 12;
    root.entries[0] = (mmio_user_lvl1_ppn << 10) | PTE_V;

    // 2. Identity map Gigapage 2 (0x8000_0000 - 0xBFFF_FFFF) for Kernel DRAM (1 GiB)
    // PPN = 0x80000 (0x8000_0000 >> 12), Flags: Valid + Read + Write + Exec + Accessed + Dirty (Kernel only: NO PTE_U)
    let dram_ppn = 0x80000u64;
    root.entries[2] = (dram_ppn << 10) | PTE_V | PTE_R | PTE_W | PTE_X | PTE_A | PTE_D;

    // 3. Smoke Test lower-half mapping (Gigapage 1: 0x4000_0000 .. 0x7FFF_FFFF)
    let user_lvl1 = unsafe { &mut *(&raw mut USER_LVL1_PT) };
    user_lvl1.clear();
    let user_lvl0 = unsafe { &mut *(&raw mut USER_LVL0_PT) };
    user_lvl0.clear();

    // Leaf 0 (0x4000_0000): User Code Region (RX only: Read + Exec + User, NO Write -> W^X enforced!)
    let user_frame_paddr = (&raw const USER_PAGE_FRAME.0) as *const u8 as u64;
    let user_frame_ppn = user_frame_paddr >> 12;
    user_lvl0.entries[0] = (user_frame_ppn << 10) | PTE_V | PTE_R | PTE_X | PTE_U | PTE_A | PTE_D;

    // Leaf 1 (0x4000_1000): User Stack Region (RW only: Read + Write + User, NO Exec -> W^X enforced!)
    let stack_frame_paddr = (&raw const USER_STACK_FRAME.0) as *const u8 as u64;
    let stack_frame_ppn = stack_frame_paddr >> 12;
    user_lvl0.entries[1] = (stack_frame_ppn << 10) | PTE_V | PTE_R | PTE_W | PTE_U | PTE_A | PTE_D;

    // Point user_lvl1 entry 0 to user_lvl0 page table
    let user_lvl0_ppn = (user_lvl0 as *const PageTable as u64) >> 12;
    user_lvl1.entries[0] = (user_lvl0_ppn << 10) | PTE_V; // Branch pointer (no R/W/X flags)

    // Identity-map remaining megapages in Gigapage 1 for supervisor PCIe MMIO (entries 1..512)
    for i in 1..512u64 {
        let paddr = 0x4000_0000u64 + (i * 0x20_0000u64);
        user_lvl1.entries[i as usize] = ((paddr >> 12) << 10) | PTE_V | PTE_R | PTE_W | PTE_A | PTE_D;
    }

    // Point root entry 1 to user_lvl1 page table
    let user_lvl1_ppn = (user_lvl1 as *const PageTable as u64) >> 12;
    root.entries[1] = (user_lvl1_ppn << 10) | PTE_V; // Branch pointer (no R/W/X flags)

    // 4. Activate SATP with Sv39 mode
    let root_paddr = root as *const PageTable as u64;
    let satp_val = SATP_MODE_SV39 | ((root_paddr >> 12) & 0x0FFF_FFFF_FFFF);

    asm!(
        "sfence.vma zero, zero",
        "csrw satp, {0}",
        "sfence.vma zero, zero",
        in(reg) satp_val,
        options(nostack)
    );
}

/// Creates a new per-task Sv39 page table with higher-half kernel entries cloned.
pub unsafe fn create_user_page_table(task_root: &mut PageTable) {
    task_root.clear();
    // Clone kernel mappings (entry 0 for MMIO, entry 2 for DRAM)
    task_root.entries[0] = KERNEL_ROOT_PT.entries[0];
    task_root.entries[2] = KERNEL_ROOT_PT.entries[2];
    // User lower-half entries are zeroed by default
}

/// Switches the active address space to the given SATP register value.
#[inline(always)]
pub unsafe fn switch_satp(satp: usize) {
    asm!(
        "sfence.vma zero, zero",
        "csrw satp, {0}",
        "sfence.vma zero, zero",
        in(reg) satp,
        options(nostack)
    );
}

pub const USER_SPACE_MIN: usize = 0x0000_1000;
pub const USER_SPACE_MAX: usize = 0x7FFF_FFFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameOwnership {
    Kernel = 0,
    Mmio = 1,
    Borrowed = 2,
    Shared = 3,
    PrivateOwned = 4,
}

pub struct RiscvFrameAllocator {
    pub base_paddr: u64,
    pub total_frames: usize,
    pub allocated_frames: usize,
    pub recycled_frames: usize,
    pub bitmap: [u64; 1024], // 65,536 frames = 256 MiB
    pub ownership: [u8; 65536],
    pub ref_counts: [u16; 65536],
}

impl RiscvFrameAllocator {
    pub const fn empty() -> Self {
        Self {
            base_paddr: 0x8800_0000,
            total_frames: 65536,
            allocated_frames: 0,
            recycled_frames: 0,
            bitmap: [0; 1024],
            ownership: [0; 65536],
            ref_counts: [0; 65536],
        }
    }

    pub fn allocate_frame_with_ownership(&mut self, ownership: FrameOwnership) -> Option<u64> {
        for (word_idx, word) in self.bitmap.iter_mut().enumerate() {
            if *word != !0u64 {
                let bit_idx = (!*word).trailing_zeros() as usize;
                let frame_idx = word_idx * 64 + bit_idx;
                if frame_idx >= self.total_frames {
                    return None;
                }
                *word |= 1u64 << bit_idx;
                self.allocated_frames += 1;
                self.ownership[frame_idx] = ownership as u8;
                self.ref_counts[frame_idx] = 0;
                let paddr = self.base_paddr + (frame_idx as u64) * 4096;
                unsafe {
                    core::ptr::write_bytes(paddr as *mut u8, 0, 4096);
                }
                return Some(paddr);
            }
        }
        None
    }

    pub fn mark_private_frame_mapped(&mut self, paddr: u64) -> Result<(), &'static str> {
        if (paddr & 0xFFF) != 0 || paddr < self.base_paddr {
            return Err("Invalid frame address for private mapping");
        }
        let frame_idx = ((paddr - self.base_paddr) / 4096) as usize;
        if frame_idx >= self.total_frames {
            return Err("Frame exceeds allocator range");
        }
        let word_idx = frame_idx / 64;
        let bit_idx = frame_idx % 64;
        if (self.bitmap[word_idx] & (1u64 << bit_idx)) == 0 {
            return Err("Cannot map unallocated frame");
        }
        if self.ownership[frame_idx] != FrameOwnership::PrivateOwned as u8 {
            return Err("Cannot map non-private frame as private");
        }
        if self.ref_counts[frame_idx] != 0 {
            return Err("Private frame already mapped to another virtual address");
        }
        self.ref_counts[frame_idx] = 1;
        Ok(())
    }

    pub fn mark_private_frame_unmapped(&mut self, paddr: u64) -> Result<(), &'static str> {
        if (paddr & 0xFFF) != 0 || paddr < self.base_paddr {
            return Err("Invalid frame address");
        }
        let frame_idx = ((paddr - self.base_paddr) / 4096) as usize;
        if frame_idx >= self.total_frames {
            return Err("Frame exceeds allocator range");
        }
        let word = frame_idx / 64;
        let bit = frame_idx % 64;
        if (self.bitmap[word] & (1 << bit)) == 0 {
            return Err("Frame is not allocated");
        }
        if self.ownership[frame_idx] != FrameOwnership::PrivateOwned as u8 {
            return Err("Not a private frame");
        }
        if self.ref_counts[frame_idx] != 1 {
            return Err("Private frame was not mapped or ref_count invalid");
        }
        self.ref_counts[frame_idx] = 0;
        Ok(())
    }

    pub fn allocate_frame(&mut self) -> Option<u64> {
        self.allocate_frame_with_ownership(FrameOwnership::PrivateOwned)
    }

    pub fn retain_shared_frame(&mut self, paddr: u64) -> Result<(), &'static str> {
        if (paddr & 0xFFF) != 0 || paddr < self.base_paddr {
            return Err("Invalid frame address for retain");
        }
        let frame_idx = ((paddr - self.base_paddr) / 4096) as usize;
        if frame_idx >= self.total_frames {
            return Err("Frame exceeds allocator range");
        }
        let word_idx = frame_idx / 64;
        let bit_idx = frame_idx % 64;
        if (self.bitmap[word_idx] & (1u64 << bit_idx)) == 0 {
            return Err("Cannot retain unallocated frame");
        }
        if self.ownership[frame_idx] != FrameOwnership::Shared as u8 {
            return Err("Cannot retain non-shared frame");
        }
        if self.ref_counts[frame_idx] >= 1024 {
            return Err("Reference count overflow on shared frame");
        }
        self.ref_counts[frame_idx] += 1;
        Ok(())
    }

    pub fn release_shared_frame(&mut self, paddr: u64) -> Result<bool, &'static str> {
        if (paddr & 0xFFF) != 0 || paddr < self.base_paddr {
            return Err("Invalid frame address for release");
        }
        let frame_idx = ((paddr - self.base_paddr) / 4096) as usize;
        if frame_idx >= self.total_frames {
            return Err("Frame exceeds allocator range");
        }
        let word_idx = frame_idx / 64;
        let bit_idx = frame_idx % 64;
        if (self.bitmap[word_idx] & (1u64 << bit_idx)) == 0 {
            return Err("Double-free rejected: frame is not allocated");
        }
        if self.ownership[frame_idx] != FrameOwnership::Shared as u8 {
            return Err("Cannot release non-shared frame via release_shared_frame");
        }
        if self.ref_counts[frame_idx] == 0 {
            return Err("Reference count underflow on shared frame");
        }
        if self.ref_counts[frame_idx] == 1 && is_quarantined(paddr) {
            return Err("Cannot recycle quarantined shared frame");
        }
        self.ref_counts[frame_idx] -= 1;
        if self.ref_counts[frame_idx] == 0 {
            self.bitmap[word_idx] &= !(1u64 << bit_idx);
            self.ownership[frame_idx] = 0;
            self.recycled_frames += 1;
            Ok(true) // Frame was freed
        } else {
            Ok(false) // Frame still held by other references
        }
    }

    pub fn recycle_frame(&mut self, paddr: u64) -> Result<(), &'static str> {
        if is_quarantined(paddr) {
            return Err("Cannot recycle quarantined frame");
        }
        if (paddr & 0xFFF) != 0 {
            return Err("Physical frame unaligned");
        }
        if paddr < self.base_paddr {
            return Err("Physical frame below allocator base");
        }
        let frame_idx = ((paddr - self.base_paddr) / 4096) as usize;
        if frame_idx >= self.total_frames {
            return Err("Physical frame exceeds allocator range");
        }
        let word_idx = frame_idx / 64;
        let bit_idx = frame_idx % 64;
        if (self.bitmap[word_idx] & (1u64 << bit_idx)) == 0 {
            return Err("Double-free rejected: frame is not allocated");
        }
        let own = self.ownership[frame_idx];
        if own == FrameOwnership::Mmio as u8 || own == FrameOwnership::Borrowed as u8 {
            return Err("MMIO or borrowed frames cannot be recycled via allocator");
        }
        if own == FrameOwnership::Shared as u8 {
            return Err("Shared frames must be released via release_shared_frame, not recycle_frame");
        }
        if own != FrameOwnership::PrivateOwned as u8 && own != FrameOwnership::Kernel as u8 {
            return Err("Only PrivateOwned or Kernel table frames may be recycled");
        }
        if own == FrameOwnership::PrivateOwned as u8 && self.ref_counts[frame_idx] != 0 {
            return Err("Cannot recycle actively mapped private frame");
        }
        self.bitmap[word_idx] &= !(1u64 << bit_idx);
        self.ownership[frame_idx] = 0;
        self.ref_counts[frame_idx] = 0;
        self.recycled_frames += 1;
        Ok(())
    }
}

pub static FRAME_ALLOCATOR: spin::Mutex<RiscvFrameAllocator> = spin::Mutex::new(RiscvFrameAllocator::empty());

pub fn allocate_frame() -> Option<u64> {
    FRAME_ALLOCATOR.lock().allocate_frame()
}

pub fn allocate_frame_with_ownership(ownership: FrameOwnership) -> Option<u64> {
    FRAME_ALLOCATOR.lock().allocate_frame_with_ownership(ownership)
}

pub fn recycle_frame(paddr: u64) -> Result<(), &'static str> {
    FRAME_ALLOCATOR.lock().recycle_frame(paddr)
}

pub fn mark_private_frame_mapped(paddr: u64) -> Result<(), &'static str> {
    FRAME_ALLOCATOR.lock().mark_private_frame_mapped(paddr)
}

pub fn mark_private_frame_unmapped(paddr: u64) -> Result<(), &'static str> {
    FRAME_ALLOCATOR.lock().mark_private_frame_unmapped(paddr)
}

pub fn total_recycled_frames() -> usize {
    FRAME_ALLOCATOR.lock().recycled_frames
}

pub fn total_allocated_frames() -> usize {
    FRAME_ALLOCATOR.lock().allocated_frames
}

pub fn active_allocated_frames() -> usize {
    let alloc = FRAME_ALLOCATOR.lock();
    alloc.allocated_frames.saturating_sub(alloc.recycled_frames)
}

pub fn frame_ownership(paddr: u64) -> Option<FrameOwnership> {
    let alloc = FRAME_ALLOCATOR.lock();
    if paddr < alloc.base_paddr { return None; }
    let frame_idx = ((paddr - alloc.base_paddr) / 4096) as usize;
    if frame_idx >= alloc.total_frames { return None; }
    match alloc.ownership[frame_idx] {
        0 => Some(FrameOwnership::Kernel),
        1 => Some(FrameOwnership::Mmio),
        2 => Some(FrameOwnership::Borrowed),
        3 => Some(FrameOwnership::Shared),
        4 => Some(FrameOwnership::PrivateOwned),
        _ => None,
    }
}

pub fn frame_refcount(paddr: u64) -> u16 {
    let alloc = FRAME_ALLOCATOR.lock();
    if paddr < alloc.base_paddr { return 0; }
    let frame_idx = ((paddr - alloc.base_paddr) / 4096) as usize;
    if frame_idx >= alloc.total_frames { return 0; }
    alloc.ref_counts[frame_idx]
}

pub fn retain_shared_frame(paddr: u64) -> Result<(), &'static str> {
    FRAME_ALLOCATOR.lock().retain_shared_frame(paddr)
}

pub fn release_shared_frame(paddr: u64) -> Result<bool, &'static str> {
    FRAME_ALLOCATOR.lock().release_shared_frame(paddr)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuarantinedFrame {
    pub paddr: u64,
    pub ownership: FrameOwnership,
    pub reason: &'static str,
}

pub static QUARANTINED_FRAMES: spin::Mutex<Vec<QuarantinedFrame>> = spin::Mutex::new(Vec::new());

pub fn quarantine_frame_with_meta(paddr: u64, ownership: FrameOwnership, reason: &'static str) {
    let mut q = QUARANTINED_FRAMES.lock();
    if !q.iter().any(|entry| entry.paddr == paddr) {
        q.push(QuarantinedFrame { paddr, ownership, reason });
    }
}

pub fn quarantine_frame(paddr: u64) {
    let own = frame_ownership(paddr).unwrap_or(FrameOwnership::Kernel);
    quarantine_frame_with_meta(paddr, own, "unspecified");
}

pub fn quarantined_frame_count() -> usize {
    QUARANTINED_FRAMES.lock().len()
}

pub fn is_quarantined(paddr: u64) -> bool {
    QUARANTINED_FRAMES.lock().iter().any(|entry| entry.paddr == paddr)
}

pub static ONLINE_HARTS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);
pub static BOOT_HART_ID: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

pub fn init_hart(hart_id: usize) {
    BOOT_HART_ID.store(hart_id, core::sync::atomic::Ordering::SeqCst);
    if hart_id < 64 {
        ONLINE_HARTS.fetch_or(1u64 << hart_id, core::sync::atomic::Ordering::SeqCst);
    }
    unsafe {
        core::arch::asm!("mv tp, {0}", in(reg) hart_id);
    }
}

pub fn register_online_hart(hart_id: usize) {
    if hart_id < 64 {
        ONLINE_HARTS.fetch_or(1u64 << hart_id, core::sync::atomic::Ordering::SeqCst);
    }
}

pub fn current_hart_id() -> usize {
    let hartid: usize;
    unsafe {
        core::arch::asm!("mv {0}, tp", out(reg) hartid);
    }
    if hartid < 62 {
        hartid
    } else {
        BOOT_HART_ID.load(core::sync::atomic::Ordering::Relaxed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingKind {
    PrivateOwned,
    Shared,
    Borrowed,
}

impl From<MappingKind> for FrameOwnership {
    fn from(kind: MappingKind) -> Self {
        match kind {
            MappingKind::PrivateOwned => FrameOwnership::PrivateOwned,
            MappingKind::Shared => FrameOwnership::Shared,
            MappingKind::Borrowed => FrameOwnership::Borrowed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserMapping {
    pub vaddr: usize,
    pub paddr: u64,
    pub kind: MappingKind,
    pub is_write: bool,
    pub is_exec: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeardownError {
    AlreadyDestroyed,
    ActiveOnRemoteHart(u64),
    SbiRemoteFenceFailed(crate::sbi::SbiError),
    QuiescenceTimeout,
    FrameRecycleFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationError {
    ClosingOrDestroyed,
    HartIdOutOfRange(usize),
}

pub const LIFECYCLE_ACTIVE_HARTS_MASK: u64 = 0x3FFF_FFFF_FFFF_FFFF; // bits 0..61: bitmask of active harts
pub const LIFECYCLE_CLOSING_BIT: u64 = 1u64 << 63;
pub const LIFECYCLE_DESTROYED_BIT: u64 = 1u64 << 62;

pub unsafe fn switch_to_kernel_satp() {
    let kernel_root_phys = core::ptr::addr_of!(KERNEL_ROOT_PT) as u64;
    let kernel_satp = (SATP_MODE_SV39 | ((kernel_root_phys >> 12) & 0x0FFF_FFFF_FFFF)) as usize;
    switch_satp(kernel_satp);
}

/// Validates that a physical address belongs to a documented QEMU virt RISC-V MMIO aperture.
pub fn is_valid_mmio_peripheral(paddr: u64, len: usize) -> bool {
    if (paddr & 0xFFF) != 0 || len == 0 || (len & 0xFFF) != 0 {
        return false;
    }
    let end = match paddr.checked_add(len as u64) {
        Some(e) => e,
        None => return false,
    };
    (paddr >= 0x0200_0000 && end <= 0x0201_0000) // CLINT
        || (paddr >= 0x0c00_0000 && end <= 0x0c20_0000) // PLIC
        || (paddr >= 0x1000_0000 && end <= 0x1000_1000) // UART0
        || (paddr >= 0x1000_1000 && end <= 0x1000_9000) // VirtIO MMIO slots 0..7
        || (paddr >= 0x6000_0000 && end <= 0x6100_0000) // PCIe Display scanout BAR aperture
}

extern crate alloc;
use alloc::vec::Vec;

pub struct UserAddressSpace {
    pub satp: usize,
    pub root_paddr: u64,
    pub table_frames: Vec<u64>,
    pub mappings: Vec<UserMapping>,
    pub mapped_bytes: usize,
    pub destroyed: bool,
    pub lifecycle: core::sync::atomic::AtomicU64,
    pub executed_harts: core::sync::atomic::AtomicU64,
    pub acknowledged_harts: core::sync::atomic::AtomicU64,
}

impl UserAddressSpace {
    pub fn is_closing(&self) -> bool {
        (self.lifecycle.load(core::sync::atomic::Ordering::Acquire) & LIFECYCLE_CLOSING_BIT) != 0
    }

    pub fn is_destroyed(&self) -> bool {
        self.destroyed || (self.lifecycle.load(core::sync::atomic::Ordering::Acquire) & LIFECYCLE_DESTROYED_BIT) != 0
    }

    pub fn active_harts(&self) -> u64 {
        self.lifecycle.load(core::sync::atomic::Ordering::Acquire) & LIFECYCLE_ACTIVE_HARTS_MASK
    }

    pub fn is_active_on_any_hart(&self) -> bool {
        self.active_harts() != 0
    }

    pub fn has_unacknowledged_or_active_harts(&self) -> bool {
        let active = self.active_harts();
        let executed = self.executed_harts.load(core::sync::atomic::Ordering::Acquire);
        let acknowledged = self.acknowledged_harts.load(core::sync::atomic::Ordering::Acquire);
        active != 0 || (executed & !acknowledged) != 0
    }

    pub fn is_evacuated_for_current(&self) -> bool {
        let hid = current_hart_id();
        if hid >= 62 {
            return true;
        }
        let hart_bit = 1u64 << hid;
        let active = self.active_harts();
        let executed = self.executed_harts.load(core::sync::atomic::Ordering::Acquire);
        let acknowledged = self.acknowledged_harts.load(core::sync::atomic::Ordering::Acquire);
        (active & hart_bit) == 0 && (executed & !acknowledged & hart_bit) == 0
    }

    pub fn new() -> Option<Self> {
        let root_paddr = allocate_frame_with_ownership(FrameOwnership::Kernel)?;
        let lvl1_paddr = match allocate_frame_with_ownership(FrameOwnership::Kernel) {
            Some(f) => f,
            None => {
                let _ = recycle_frame(root_paddr);
                return None;
            }
        };
        let root = unsafe { &mut *(root_paddr as *mut PageTable) };
        root.clear();

        // Level 1 table for Gigapage 0 (MMIO & User ELF)
        let lvl1 = unsafe { &mut *(lvl1_paddr as *mut PageTable) };
        lvl1.clear();

        // Identity-map supervisor MMIO megapages (without PTE_U)
        for &i in &[0, 16, 96, 128, 129, 130, 131, 132] {
            lvl1.entries[i] = ((i as u64) << 19) | PTE_V | PTE_R | PTE_W | PTE_A | PTE_D;
        }
        // Identity-map PCIe ECAM space (0x3000_0000 .. 0x3FFF_FFFF: megapages 384..512)
        for i in 384..512 {
            lvl1.entries[i] = ((i as u64) << 19) | PTE_V | PTE_R | PTE_W | PTE_A | PTE_D;
        }

        // Connect Level 1 table to root entry 0
        root.entries[0] = ((lvl1_paddr >> 12) << 10) | PTE_V;

        // Entry 1: 1 GiB PCIe MMIO window (0x4000_0000 .. 0x7FFF_FFFF) for supervisor access
        let mmio_ppn = 0x40000u64;
        root.entries[1] = (mmio_ppn << 10) | PTE_V | PTE_R | PTE_W | PTE_A | PTE_D;

        // Entry 2: Identity-mapped Kernel DRAM (PPN = 0x80000, 1 GiB)
        let dram_ppn = 0x80000u64;
        root.entries[2] = (dram_ppn << 10) | PTE_V | PTE_R | PTE_W | PTE_X | PTE_A | PTE_D;

        let satp_val = (SATP_MODE_SV39 | ((root_paddr >> 12) & 0x0FFF_FFFF_FFFF)) as usize;

        let mut table_frames = Vec::new();
        table_frames.push(root_paddr);
        table_frames.push(lvl1_paddr);

        Some(Self {
            satp: satp_val,
            root_paddr,
            table_frames,
            mappings: Vec::new(),
            mapped_bytes: 0,
            destroyed: false,
            lifecycle: core::sync::atomic::AtomicU64::new(0),
            executed_harts: core::sync::atomic::AtomicU64::new(0),
            acknowledged_harts: core::sync::atomic::AtomicU64::new(0),
        })
    }

    fn map_leaf_entry(&mut self, vaddr: usize, paddr: u64, is_write: bool, is_exec: bool) -> Result<(), &'static str> {
        let root = unsafe { &mut *(self.root_paddr as *mut PageTable) };
        let vpn2 = (vaddr >> 30) & 0x1FF;
        let vpn1 = (vaddr >> 21) & 0x1FF;
        let vpn0 = (vaddr >> 12) & 0x1FF;

        // Level 2 root entry -> Level 1 table
        let mut newly_allocated_lvl1 = None;
        let lvl1_paddr = if root.entries[vpn2] & PTE_V == 0 {
            let frame = allocate_frame_with_ownership(FrameOwnership::Kernel).ok_or("OOM allocating Level 1 page table")?;
            newly_allocated_lvl1 = Some(frame);
            frame
        } else {
            (root.entries[vpn2] >> 10) << 12
        };

        // Level 1 entry -> Level 0 table
        let lvl1 = unsafe { &mut *(lvl1_paddr as *mut PageTable) };
        let mut newly_allocated_lvl0 = None;
        let lvl0_paddr = if lvl1.entries[vpn1] & PTE_V == 0 {
            let frame = match allocate_frame_with_ownership(FrameOwnership::Kernel) {
                Some(f) => f,
                None => {
                    if let Some(f) = newly_allocated_lvl1 {
                        let _ = recycle_frame(f);
                    }
                    return Err("OOM allocating Level 0 page table");
                }
            };
            newly_allocated_lvl0 = Some(frame);
            frame
        } else {
            if (lvl1.entries[vpn1] & (PTE_R | PTE_X)) != 0 {
                if let Some(f) = newly_allocated_lvl1 {
                    let _ = recycle_frame(f);
                }
                return Err("Virtual address collides with MMIO megapage");
            }
            (lvl1.entries[vpn1] >> 10) << 12
        };

        // Level 0 leaf entry
        let lvl0 = unsafe { &mut *(lvl0_paddr as *mut PageTable) };
        if lvl0.entries[vpn0] & PTE_V != 0 {
            if let Some(f) = newly_allocated_lvl0 {
                let _ = recycle_frame(f);
            }
            if let Some(f) = newly_allocated_lvl1 {
                let _ = recycle_frame(f);
            }
            return Err("Page already mapped");
        }

        // Commit newly allocated tables to hierarchy and table_frames
        if let Some(f) = newly_allocated_lvl1 {
            root.entries[vpn2] = ((f >> 12) << 10) | PTE_V;
            self.table_frames.push(f);
        }
        if let Some(f) = newly_allocated_lvl0 {
            lvl1.entries[vpn1] = ((f >> 12) << 10) | PTE_V;
            self.table_frames.push(f);
        }

        let mut flags = PTE_V | PTE_U | PTE_A | PTE_D | PTE_R;
        if is_write {
            flags |= PTE_W;
        }
        if is_exec {
            flags |= PTE_X;
        }

        lvl0.entries[vpn0] = ((paddr >> 12) << 10) | flags;
        self.mapped_bytes += PAGE_SIZE;
        Ok(())
    }

    pub fn map_private_frame(&mut self, vaddr: usize, paddr: u64, is_write: bool, is_exec: bool) -> Result<(), &'static str> {
        if (paddr & 0xFFF) != 0 || (vaddr & 0xFFF) != 0 {
            return Err("Address must be page-aligned (4KB)");
        }
        if self.destroyed || self.is_closing() {
            return Err("Cannot map into destroyed or closing address space");
        }
        if is_write && is_exec {
            return Err("W^X violation: writable user mapping must be non-executable");
        }
        if vaddr >= USER_SPACE_MAX || vaddr < USER_SPACE_MIN {
            return Err("Virtual address outside canonical user range");
        }
        if frame_ownership(paddr) != Some(FrameOwnership::PrivateOwned) {
            return Err("Frame is not PrivatelyOwned");
        }
        if self.mappings.iter().any(|m| m.vaddr == vaddr) {
            return Err("Virtual address already mapped in address space");
        }
        if self.mappings.iter().any(|m| m.paddr == paddr) {
            return Err("Physical frame already mapped in this address space");
        }

        mark_private_frame_mapped(paddr)?;
        if let Err(e) = self.map_leaf_entry(vaddr, paddr, is_write, is_exec) {
            if let Err(unmap_err) = mark_private_frame_unmapped(paddr) {
                crate::println!("[MMU ERROR] Failed to unmark private frame 0x{:x} on map rollback: {}", paddr, unmap_err);
                quarantine_frame_with_meta(paddr, FrameOwnership::PrivateOwned, "map_private_frame rollback failed");
            }
            return Err(e);
        }
        self.mappings.push(UserMapping {
            vaddr,
            paddr,
            kind: MappingKind::PrivateOwned,
            is_write,
            is_exec,
        });
        Ok(())
    }

    pub fn map_shared_frame(&mut self, vaddr: usize, paddr: u64, is_write: bool, is_exec: bool) -> Result<(), &'static str> {
        if (paddr & 0xFFF) != 0 || (vaddr & 0xFFF) != 0 {
            return Err("Address must be page-aligned (4KB)");
        }
        if self.destroyed || self.is_closing() {
            return Err("Cannot map into destroyed or closing address space");
        }
        if is_write && is_exec {
            return Err("W^X violation: writable user mapping must be non-executable");
        }
        if vaddr >= USER_SPACE_MAX || vaddr < USER_SPACE_MIN {
            return Err("Virtual address outside canonical user range");
        }
        if frame_ownership(paddr) != Some(FrameOwnership::Shared) {
            return Err("Frame is not Shared");
        }
        if self.mappings.iter().any(|m| m.vaddr == vaddr) {
            return Err("Virtual address already mapped in address space");
        }
        if self.mappings.iter().any(|m| m.paddr == paddr) {
            return Err("Physical frame already mapped in this address space");
        }

        retain_shared_frame(paddr)?;
        if let Err(e) = self.map_leaf_entry(vaddr, paddr, is_write, is_exec) {
            if let Err(rel_err) = release_shared_frame(paddr) {
                crate::println!("[MMU ERROR] Failed to release shared frame 0x{:x} on map rollback: {}", paddr, rel_err);
                quarantine_frame_with_meta(paddr, FrameOwnership::Shared, "map_shared_frame rollback failed");
            }
            return Err(e);
        }

        self.mappings.push(UserMapping {
            vaddr,
            paddr,
            kind: MappingKind::Shared,
            is_write,
            is_exec,
        });
        Ok(())
    }

    pub fn map_borrowed_frame(&mut self, vaddr: usize, paddr: u64, is_write: bool, is_exec: bool) -> Result<(), &'static str> {
        if (paddr & 0xFFF) != 0 || (vaddr & 0xFFF) != 0 {
            return Err("Address must be page-aligned (4KB)");
        }
        if self.destroyed || self.is_closing() {
            return Err("Cannot map into destroyed or closing address space");
        }
        if is_write && is_exec {
            return Err("W^X violation: writable user mapping must be non-executable");
        }
        if vaddr >= USER_SPACE_MAX || vaddr < USER_SPACE_MIN {
            return Err("Virtual address outside canonical user range");
        }
        let own = frame_ownership(paddr);
        if paddr >= 0x8000_0000 {
            if own != Some(FrameOwnership::Borrowed) {
                return Err("RAM frame is not Borrowed");
            }
        } else {
            if !is_valid_mmio_peripheral(paddr, 4096) && own != Some(FrameOwnership::Mmio) {
                return Err("Physical address is neither valid RAM nor a registered MMIO peripheral");
            }
        }
        if self.mappings.iter().any(|m| m.vaddr == vaddr) {
            return Err("Virtual address already mapped in address space");
        }
        if self.mappings.iter().any(|m| m.paddr == paddr) {
            return Err("Physical frame already mapped in this address space");
        }

        self.map_leaf_entry(vaddr, paddr, is_write, is_exec)?;
        self.mappings.push(UserMapping {
            vaddr,
            paddr,
            kind: MappingKind::Borrowed,
            is_write,
            is_exec,
        });
        Ok(())
    }

    pub fn map_user_page(&mut self, vaddr: usize, paddr: u64, is_write: bool, is_exec: bool) -> Result<(), &'static str> {
        if (paddr & 0xFFF) != 0 || (vaddr & 0xFFF) != 0 {
            return Err("Address must be page-aligned (4KB)");
        }
        match frame_ownership(paddr) {
            Some(FrameOwnership::PrivateOwned) => self.map_private_frame(vaddr, paddr, is_write, is_exec),
            Some(FrameOwnership::Shared) => self.map_shared_frame(vaddr, paddr, is_write, is_exec),
            Some(FrameOwnership::Borrowed) | Some(FrameOwnership::Mmio) => self.map_borrowed_frame(vaddr, paddr, is_write, is_exec),
            _ => {
                if is_valid_mmio_peripheral(paddr, 4096) {
                    self.map_borrowed_frame(vaddr, paddr, is_write, is_exec)
                } else {
                    Err("Frame must have valid ownership or be a registered MMIO peripheral before mapping")
                }
            }
        }
    }

    pub fn unmap_user_page(&mut self, vaddr: usize, release: bool) -> Result<(), &'static str> {
        let pos = self.mappings.iter().position(|m| m.vaddr == vaddr).ok_or("Address not mapped")?;
        let mapping = self.mappings[pos];

        let root = unsafe { &mut *(self.root_paddr as *mut PageTable) };
        let vpn2 = (vaddr >> 30) & 0x1FF;
        let vpn1 = (vaddr >> 21) & 0x1FF;
        let vpn0 = (vaddr >> 12) & 0x1FF;

        if root.entries[vpn2] & PTE_V != 0 {
            let lvl1_paddr = (root.entries[vpn2] >> 10) << 12;
            let lvl1 = unsafe { &mut *(lvl1_paddr as *mut PageTable) };
            if lvl1.entries[vpn1] & PTE_V != 0 {
                let lvl0_paddr = (lvl1.entries[vpn1] >> 10) << 12;
                let lvl0 = unsafe { &mut *(lvl0_paddr as *mut PageTable) };
                lvl0.entries[vpn0] = 0;
            }
        }

        #[cfg(target_arch = "riscv64")]
        unsafe {
            core::arch::asm!("sfence.vma {0}, zero", in(reg) vaddr, options(nostack));
        }

        let hid = current_hart_id();
        let current_mask = if hid < 62 { 1u64 << hid } else { 0 };
        let executed = self.executed_harts.load(core::sync::atomic::Ordering::SeqCst);
        let online = ONLINE_HARTS.load(core::sync::atomic::Ordering::SeqCst);
        let remote_mask = (executed | online) & !current_mask;
        if remote_mask != 0 {
            if let Err(_) = crate::sbi::remote_sfence_vma(remote_mask as usize, 0, vaddr, PAGE_SIZE) {
                if mapping.kind != MappingKind::Borrowed {
                    quarantine_frame_with_meta(mapping.paddr, mapping.kind.into(), "unmap_user_page sbi_remote_sfence_vma failed");
                }
                return Err("SBI remote_sfence_vma failed during unmap");
            }
        }

        if release {
            match mapping.kind {
                MappingKind::PrivateOwned => {
                    if let Err(reason) = mark_private_frame_unmapped(mapping.paddr) {
                        quarantine_frame_with_meta(mapping.paddr, mapping.kind.into(), reason);
                        return Err(reason);
                    }
                    if let Err(reason) = recycle_frame(mapping.paddr) {
                        quarantine_frame_with_meta(mapping.paddr, mapping.kind.into(), reason);
                        return Err(reason);
                    }
                }
                MappingKind::Shared => {
                    if let Err(reason) = release_shared_frame(mapping.paddr) {
                        quarantine_frame_with_meta(mapping.paddr, mapping.kind.into(), reason);
                        return Err(reason);
                    }
                }
                MappingKind::Borrowed => {}
            }
        }

        self.mappings.remove(pos);
        self.mapped_bytes = self.mapped_bytes.saturating_sub(PAGE_SIZE);
        Ok(())
    }

    pub fn try_activate(&self) -> Result<(), ActivationError> {
        let hid = current_hart_id();
        if hid >= 62 {
            return Err(ActivationError::HartIdOutOfRange(hid));
        }
        let hart_bit = 1u64 << hid;
        loop {
            let cur = self.lifecycle.load(core::sync::atomic::Ordering::Acquire);
            if (cur & (LIFECYCLE_CLOSING_BIT | LIFECYCLE_DESTROYED_BIT)) != 0 {
                return Err(ActivationError::ClosingOrDestroyed);
            }
            let next = cur | hart_bit;
            match self.lifecycle.compare_exchange_weak(
                cur,
                next,
                core::sync::atomic::Ordering::AcqRel,
                core::sync::atomic::Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.executed_harts.fetch_or(hart_bit, core::sync::atomic::Ordering::Release);
                    self.acknowledged_harts.fetch_and(!hart_bit, core::sync::atomic::Ordering::Release);
                    unsafe {
                        switch_satp(self.satp);
                    }
                    return Ok(());
                }
                Err(_) => core::hint::spin_loop(),
            }
        }
    }

    pub fn activate(&self) -> Result<(), ActivationError> {
        self.try_activate()
    }

    pub fn deactivate_after_switch(&self) {
        let hid = current_hart_id();
        if hid >= 62 {
            return;
        }
        let hart_bit = 1u64 << hid;
        // Hardware SATP has already switched away from this address space.
        self.acknowledged_harts.fetch_or(hart_bit, core::sync::atomic::Ordering::Release);
        loop {
            let cur = self.lifecycle.load(core::sync::atomic::Ordering::Acquire);
            let next = cur & !hart_bit;
            match self.lifecycle.compare_exchange_weak(
                cur,
                next,
                core::sync::atomic::Ordering::AcqRel,
                core::sync::atomic::Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(_) => core::hint::spin_loop(),
            }
        }
    }

    pub fn deactivate(&self) {
        let hid = current_hart_id();
        if hid >= 62 {
            return;
        }
        // 1. Switch hardware SATP away to kernel root page table and flush local TLB
        unsafe {
            switch_to_kernel_satp();
        }
        // 2 & 3. Acknowledge and clear this hart from active mask
        self.deactivate_after_switch();
    }

    pub fn destroy_fallible(&mut self) -> Result<(), TeardownError> {
        loop {
            let cur = self.lifecycle.load(core::sync::atomic::Ordering::Acquire);
            if (cur & LIFECYCLE_DESTROYED_BIT) != 0 {
                return Ok(()); // Idempotent
            }
            if (cur & LIFECYCLE_CLOSING_BIT) != 0 {
                break; // Already closing
            }
            let next = cur | LIFECYCLE_CLOSING_BIT;
            match self.lifecycle.compare_exchange_weak(
                cur,
                next,
                core::sync::atomic::Ordering::AcqRel,
                core::sync::atomic::Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(_) => core::hint::spin_loop(),
            }
        }

        let hid = current_hart_id();
        let current_mask = if hid < 62 { 1u64 << hid } else { 0 };

        if (self.lifecycle.load(core::sync::atomic::Ordering::Acquire) & current_mask) != 0 {
            self.deactivate();
        }

        // Bounded loop waiting for remote harts to deactivate and acknowledge
        let mut spins = 0;
        loop {
            let cur = self.lifecycle.load(core::sync::atomic::Ordering::Acquire);
            let active_remote = cur & LIFECYCLE_ACTIVE_HARTS_MASK & !current_mask;
            let executed = self.executed_harts.load(core::sync::atomic::Ordering::Acquire);
            let acknowledged = self.acknowledged_harts.load(core::sync::atomic::Ordering::Acquire);
            let unacknowledged = (executed & !current_mask) & !acknowledged;

            if active_remote == 0 && unacknowledged == 0 {
                break;
            }
            spins += 1;
            if spins > 100_000 {
                if active_remote != 0 {
                    return Err(TeardownError::ActiveOnRemoteHart(active_remote));
                } else {
                    return Err(TeardownError::QuiescenceTimeout);
                }
            }
            core::hint::spin_loop();
        }

        if self.table_frames.contains(&self.root_paddr) {
            // Step 1: Remove user mappings from root table
            let root = unsafe { &mut *(self.root_paddr as *mut PageTable) };
            root.entries[0] = 0; // Detach Level 1 table for user/MMIO space

            // Step 2: Perform local TLB invalidation
            #[cfg(target_arch = "riscv64")]
            unsafe {
                core::arch::asm!("sfence.vma zero, zero", options(nostack));
            }

            // Step 3: Broadcast across remote harts that ever executed this address space via SBI RFNC
            let executed = self.executed_harts.load(core::sync::atomic::Ordering::SeqCst);
            let online = ONLINE_HARTS.load(core::sync::atomic::Ordering::SeqCst);
            let remote_mask = (executed | online) & !current_mask;
            if remote_mask != 0 {
                match crate::sbi::remote_sfence_vma(remote_mask as usize, 0, 0, 0) {
                    Ok(()) => {}
                    Err(err) => {
                        // On remote TLB invalidation failure, quarantine frames to avoid reuse while stale translations exist
                        for m in self.mappings.drain(..) {
                            if m.kind == MappingKind::PrivateOwned || m.kind == MappingKind::Shared {
                                quarantine_frame_with_meta(m.paddr, m.kind.into(), "destroy sbi_remote_sfence_vma failed");
                            }
                        }
                        for t in self.table_frames.drain(..) {
                            quarantine_frame_with_meta(
                                t,
                                frame_ownership(t).unwrap_or(FrameOwnership::Kernel),
                                "destroy sbi_remote_sfence_vma failed table frame",
                            );
                        }
                        self.mapped_bytes = 0;
                        self.lifecycle.fetch_or(LIFECYCLE_DESTROYED_BIT, core::sync::atomic::Ordering::Release);
                        self.destroyed = true;
                        return Err(TeardownError::SbiRemoteFenceFailed(err));
                    }
                }
            }

            #[cfg(target_arch = "riscv64")]
            unsafe {
                core::arch::asm!(
                    "fence rw, rw",
                    "fence.i",
                    options(nostack)
                );
            }
        }

        // Step 4: Recycle private frames, release shared, preserve borrowed
        let mut cleanup_failed = false;
        let mut retained_mappings = Vec::new();
        for m in self.mappings.drain(..) {
            match m.kind {
                MappingKind::PrivateOwned => {
                    if let Err(reason) = mark_private_frame_unmapped(m.paddr) {
                        quarantine_frame_with_meta(m.paddr, m.kind.into(), reason);
                        retained_mappings.push(m);
                        cleanup_failed = true;
                    } else if let Err(_) = recycle_frame(m.paddr) {
                        quarantine_frame_with_meta(m.paddr, m.kind.into(), "destroy recycle private frame failed");
                        retained_mappings.push(m);
                        cleanup_failed = true;
                    }
                }
                MappingKind::Shared => {
                    if let Err(_) = release_shared_frame(m.paddr) {
                        quarantine_frame_with_meta(m.paddr, m.kind.into(), "destroy release shared frame failed");
                        retained_mappings.push(m);
                        cleanup_failed = true;
                    }
                }
                MappingKind::Borrowed => {}
            }
        }
        self.mappings = retained_mappings;
        let mut retained_tables = Vec::new();
        for t in self.table_frames.drain(..) {
            if let Err(_) = recycle_frame(t) {
                quarantine_frame_with_meta(
                    t,
                    frame_ownership(t).unwrap_or(FrameOwnership::Kernel),
                    "destroy recycle table frame failed",
                );
                retained_tables.push(t);
                cleanup_failed = true;
            }
        }
        self.table_frames = retained_tables;
        self.mapped_bytes = self.mappings.len() * PAGE_SIZE;

        if cleanup_failed {
            Err(TeardownError::FrameRecycleFailed)
        } else {
            self.lifecycle.fetch_or(LIFECYCLE_DESTROYED_BIT, core::sync::atomic::Ordering::Release);
            self.destroyed = true;
            Ok(())
        }
    }

    pub fn destroy(&mut self) -> Result<(), TeardownError> {
        self.destroy_fallible()
    }
}

impl Drop for UserAddressSpace {
    fn drop(&mut self) {
        if !self.destroyed {
            if let Err(e) = self.destroy_fallible() {
                crate::println!("[MMU WARNING] UserAddressSpace drop failed: {:?}. Quarantining unreclaimed frames.", e);
                let reason = match e {
                    TeardownError::ActiveOnRemoteHart(_) => "drop failed: address space active on remote hart",
                    TeardownError::SbiRemoteFenceFailed(_) => "drop failed: sbi remote sfence failed",
                    TeardownError::QuiescenceTimeout => "drop failed: quiescence timeout",
                    TeardownError::FrameRecycleFailed => "drop failed: frame recycle failed",
                    TeardownError::AlreadyDestroyed => "drop failed: already destroyed",
                };
                for m in self.mappings.drain(..) {
                    if m.kind == MappingKind::PrivateOwned || m.kind == MappingKind::Shared {
                        quarantine_frame_with_meta(m.paddr, m.kind.into(), reason);
                    }
                }
                for t in self.table_frames.drain(..) {
                    quarantine_frame_with_meta(
                        t,
                        frame_ownership(t).unwrap_or(FrameOwnership::Kernel),
                        reason,
                    );
                }
                self.mapped_bytes = 0;
                self.lifecycle.fetch_or(LIFECYCLE_DESTROYED_BIT, core::sync::atomic::Ordering::Release);
                self.destroyed = true;
            }
        }
    }
}

pub fn test_mmu_ownership_reclamation_errors() {
    // 1. Private ownership validation failure in unmap_user_page:
    //    - returns Err with original reason
    //    - does NOT call recycle_frame / increment recycled_frames
    //    - preserves self.mappings entry and self.mapped_bytes
    //    - quarantines frame with original validation failure reason
    let mut aspace = UserAddressSpace::new().expect("test aspace");
    let f_err = allocate_frame_with_ownership(FrameOwnership::PrivateOwned).expect("alloc private err frame");
    aspace.map_private_frame(0x0040_0000, f_err, true, false).expect("map private err frame");
    assert_eq!(frame_refcount(f_err), 1);

    // Corrupt lifecycle state by marking frame unmapped early (refcount 1 -> 0)
    mark_private_frame_unmapped(f_err).expect("early unmap");
    assert_eq!(frame_refcount(f_err), 0);

    let recycled_before = total_recycled_frames();
    let mapped_before = aspace.mapped_bytes;
    let mappings_len_before = aspace.mappings.len();
    let res = aspace.unmap_user_page(0x0040_0000, true);
    assert!(res.is_err(), "unmap_user_page must fail when private frame validation fails");
    let err_reason = res.unwrap_err();
    assert_eq!(total_recycled_frames(), recycled_before, "Failed unmap must not recycle frame");
    assert_eq!(aspace.mappings.len(), mappings_len_before, "Failed unmap must preserve mapping metadata");
    assert_eq!(aspace.mappings[0].paddr, f_err);
    assert_eq!(aspace.mapped_bytes, mapped_before, "Failed unmap must preserve mapped_bytes");
    assert!(is_quarantined(f_err), "Failed unmap must quarantine frame");
    {
        let q = QUARANTINED_FRAMES.lock();
        let entry = q.iter().find(|e| e.paddr == f_err).expect("quarantine entry for f_err");
        assert_eq!(entry.ownership, FrameOwnership::PrivateOwned);
        assert_eq!(entry.reason, err_reason);
    }

    // 2. Quarantined frame cannot be accidentally recycled via recycle_frame() or release_shared_frame()
    let recycled_before_q = total_recycled_frames();
    assert!(recycle_frame(f_err).is_err(), "Quarantined private frame must not be recycled");
    assert_eq!(total_recycled_frames(), recycled_before_q);

    let f_q_shared = allocate_frame_with_ownership(FrameOwnership::Shared).expect("alloc quarantined shared frame");
    retain_shared_frame(f_q_shared).expect("retain shared");
    quarantine_frame_with_meta(f_q_shared, FrameOwnership::Shared, "test quarantine shared");
    assert!(is_quarantined(f_q_shared));
    let recycled_before_qs = total_recycled_frames();
    assert!(release_shared_frame(f_q_shared).is_err(), "Quarantined shared frame must not be freed at refcount 1");
    assert_eq!(total_recycled_frames(), recycled_before_qs);
    assert_eq!(frame_refcount(f_q_shared), 1);

    // 3. Normal unmap_user_page transitions refcount 1 -> 0 and recycles frame once
    let mut aspace_ok = UserAddressSpace::new().expect("ok aspace");
    let f_ok = allocate_frame_with_ownership(FrameOwnership::PrivateOwned).expect("alloc ok frame");
    aspace_ok.map_private_frame(0x0040_0000, f_ok, true, false).expect("map ok frame");
    assert_eq!(frame_refcount(f_ok), 1);
    let recycled_before_ok = total_recycled_frames();
    assert_eq!(aspace_ok.unmap_user_page(0x0040_0000, true), Ok(()));
    assert_eq!(frame_refcount(f_ok), 0);
    assert!(!is_quarantined(f_ok));
    assert_eq!(total_recycled_frames(), recycled_before_ok + 1);
    assert!(aspace_ok.mappings.is_empty());
    assert_eq!(aspace_ok.mapped_bytes, 0);
    assert!(recycle_frame(f_ok).is_err(), "Already-recycled frame must reject double-free");
    assert_eq!(aspace_ok.destroy_fallible(), Ok(()));

    // 4. destroy_fallible with corrupted ownership retains metadata, avoids recycle_frame, and quarantines with validation reason
    let mut aspace_bad = UserAddressSpace::new().expect("bad aspace");
    let f_bad = allocate_frame_with_ownership(FrameOwnership::PrivateOwned).expect("alloc bad frame");
    aspace_bad.map_private_frame(0x0040_0000, f_bad, true, false).expect("map bad frame");
    {
        let mut alloc = FRAME_ALLOCATOR.lock();
        let idx = ((f_bad - alloc.base_paddr) / 4096) as usize;
        // Corrupt ownership to Kernel so recycle_frame would succeed if validation error were ignored
        alloc.ownership[idx] = FrameOwnership::Kernel as u8;
    }
    let expected_tables = aspace_bad.table_frames.len();
    let recycled_before_bad = total_recycled_frames();
    assert_eq!(aspace_bad.destroy_fallible(), Err(TeardownError::FrameRecycleFailed));
    assert_eq!(
        total_recycled_frames(),
        recycled_before_bad + expected_tables,
        "Only valid table frames may be recycled; corrupted private frame must not be recycled"
    );
    assert_eq!(aspace_bad.mappings.len(), 1);
    assert_eq!(aspace_bad.mappings[0].paddr, f_bad);
    assert_eq!(aspace_bad.mapped_bytes, PAGE_SIZE);
    assert!(is_quarantined(f_bad));
    {
        let q = QUARANTINED_FRAMES.lock();
        let entry = q.iter().find(|e| e.paddr == f_bad).expect("quarantine entry for f_bad");
        assert_eq!(entry.ownership, FrameOwnership::PrivateOwned);
        assert_eq!(entry.reason, "Not a private frame");
    }

    // Verify Drop handles already-attempted teardown cleanly without panic and preserves original quarantine reason
    drop(aspace_bad);
    drop(aspace);
    {
        let q = QUARANTINED_FRAMES.lock();
        let entry = q.iter().find(|e| e.paddr == f_bad).expect("f_bad preserved after drop");
        assert_eq!(entry.reason, "Not a private frame");
        let entry_err = q.iter().find(|e| e.paddr == f_err).expect("f_err preserved after drop");
        assert_eq!(entry_err.reason, err_reason);
    }

    // 5. Shared-frame retain/release continues decrementing properly and frees only at 0
    let f_shared = allocate_frame_with_ownership(FrameOwnership::Shared).expect("alloc shared frame");
    let mut aspace_s1 = UserAddressSpace::new().expect("aspace s1");
    let mut aspace_s2 = UserAddressSpace::new().expect("aspace s2");
    aspace_s1.map_shared_frame(0x0040_0000, f_shared, false, false).expect("map shared s1");
    aspace_s2.map_shared_frame(0x0040_0000, f_shared, false, false).expect("map shared s2");
    assert_eq!(frame_refcount(f_shared), 2);

    let recycled_s1 = total_recycled_frames();
    assert_eq!(aspace_s1.unmap_user_page(0x0040_0000, true), Ok(()));
    assert_eq!(frame_refcount(f_shared), 1);
    assert!(!is_quarantined(f_shared));
    assert_eq!(total_recycled_frames(), recycled_s1, "Shared frame must not be freed while refcount > 0");
    assert_eq!(aspace_s1.destroy_fallible(), Ok(()));

    let recycled_s2 = total_recycled_frames();
    assert_eq!(aspace_s2.unmap_user_page(0x0040_0000, true), Ok(()));
    assert_eq!(frame_refcount(f_shared), 0);
    assert!(!is_quarantined(f_shared));
    assert_eq!(total_recycled_frames(), recycled_s2 + 1, "Shared frame must be freed when refcount reaches 0");
    assert_eq!(aspace_s2.destroy_fallible(), Ok(()));

    crate::println!("[TEST] MMU Ownership Reclamation Errors & Quarantine Protection VERIFIED.");
}
