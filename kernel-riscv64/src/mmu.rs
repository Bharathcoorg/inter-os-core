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

// Static kernel root page table and secondary/tertiary tables for bare-metal boot
static mut KERNEL_ROOT_PT: PageTable = PageTable::empty();
static mut USER_LVL1_PT: PageTable = PageTable::empty();
static mut USER_LVL0_PT: PageTable = PageTable::empty();
static mut USER_PAGE_FRAME: [u8; PAGE_SIZE] = [0; PAGE_SIZE];
static mut USER_STACK_FRAME: [u8; PAGE_SIZE] = [0; PAGE_SIZE];

pub const USER_CODE_VADDR: usize = 0x4000_0000;
pub const USER_STACK_TOP: usize = 0x4000_2000;

/// Copies userspace code payload into the dedicated physical code frame.
pub unsafe fn load_user_code(code: &[u8]) {
    assert!(code.len() <= PAGE_SIZE, "User code exceeds 4KiB page size");
    let dest = (&raw mut USER_PAGE_FRAME) as *mut u8;
    core::ptr::copy_nonoverlapping(code.as_ptr(), dest, code.len());
}

/// Initializes Sv39 virtual memory, maps kernel RAM and MMIO, and activates SATP.
pub unsafe fn init_sv39() {
    let root = unsafe { &mut *(&raw mut KERNEL_ROOT_PT) };
    root.clear();

    // 1. Identity map Gigapage 0 (0x0000_0000 - 0x3FFF_FFFF) for MMIO (UART, CLINT, PLIC)
    // PPN = 0, Flags: Valid + Read + Write + Accessed + Dirty (Kernel only: NO PTE_U)
    root.entries[0] = (0 << 10) | PTE_V | PTE_R | PTE_W | PTE_A | PTE_D;

    // 2. Identity map Gigapage 2 (0x8000_0000 - 0xBFFF_FFFF) for Kernel DRAM (1 GiB)
    // PPN = 0x80000 (0x8000_0000 >> 12), Flags: Valid + Read + Write + Exec + Accessed + Dirty (Kernel only: NO PTE_U)
    let dram_ppn = 0x80000u64;
    root.entries[2] = (dram_ppn << 10) | PTE_V | PTE_R | PTE_W | PTE_X | PTE_A | PTE_D;

    // 3. User lower-half mapping (Gigapage 1: 0x4000_0000 .. 0x7FFF_FFFF)
    let user_lvl1 = unsafe { &mut *(&raw mut USER_LVL1_PT) };
    user_lvl1.clear();
    let user_lvl0 = unsafe { &mut *(&raw mut USER_LVL0_PT) };
    user_lvl0.clear();

    // Leaf 0 (0x4000_0000): User Code Region (RX only: Read + Exec + User, NO Write -> W^X enforced!)
    let user_frame_paddr = (&raw const USER_PAGE_FRAME) as *const u8 as u64;
    let user_frame_ppn = user_frame_paddr >> 12;
    user_lvl0.entries[0] = (user_frame_ppn << 10) | PTE_V | PTE_R | PTE_X | PTE_U | PTE_A | PTE_D;

    // Leaf 1 (0x4000_1000): User Stack Region (RW only: Read + Write + User, NO Exec -> W^X enforced!)
    let stack_frame_paddr = (&raw const USER_STACK_FRAME) as *const u8 as u64;
    let stack_frame_ppn = stack_frame_paddr >> 12;
    user_lvl0.entries[1] = (stack_frame_ppn << 10) | PTE_V | PTE_R | PTE_W | PTE_U | PTE_A | PTE_D;

    // Point user_lvl1 entry 0 to user_lvl0 page table
    let user_lvl0_ppn = (user_lvl0 as *const PageTable as u64) >> 12;
    user_lvl1.entries[0] = (user_lvl0_ppn << 10) | PTE_V; // Branch pointer (no R/W/X flags)

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
