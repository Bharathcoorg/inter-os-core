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

pub struct RiscvFrameAllocator {
    pub base_paddr: u64,
    pub total_frames: usize,
    pub allocated_frames: usize,
    pub recycled_frames: usize,
    pub bitmap: [u64; 1024], // 65,536 frames = 256 MiB
}

impl RiscvFrameAllocator {
    pub const fn empty() -> Self {
        Self {
            base_paddr: 0x8800_0000,
            total_frames: 65536,
            allocated_frames: 0,
            recycled_frames: 0,
            bitmap: [0; 1024],
        }
    }

    pub fn allocate_frame(&mut self) -> Option<u64> {
        for (word_idx, word) in self.bitmap.iter_mut().enumerate() {
            if *word != !0u64 {
                let bit_idx = (!*word).trailing_zeros() as usize;
                let frame_idx = word_idx * 64 + bit_idx;
                if frame_idx >= self.total_frames {
                    return None;
                }
                *word |= 1u64 << bit_idx;
                self.allocated_frames += 1;
                let paddr = self.base_paddr + (frame_idx as u64) * 4096;
                unsafe {
                    core::ptr::write_bytes(paddr as *mut u8, 0, 4096);
                }
                return Some(paddr);
            }
        }
        None
    }

    pub fn recycle_frame(&mut self, paddr: u64) {
        if paddr < self.base_paddr {
            return;
        }
        let frame_idx = ((paddr - self.base_paddr) / 4096) as usize;
        if frame_idx < self.total_frames {
            let word_idx = frame_idx / 64;
            let bit_idx = frame_idx % 64;
            if (self.bitmap[word_idx] & (1u64 << bit_idx)) != 0 {
                self.bitmap[word_idx] &= !(1u64 << bit_idx);
                self.recycled_frames += 1;
            }
        }
    }
}

pub static FRAME_ALLOCATOR: spin::Mutex<RiscvFrameAllocator> = spin::Mutex::new(RiscvFrameAllocator::empty());

pub fn allocate_frame() -> Option<u64> {
    FRAME_ALLOCATOR.lock().allocate_frame()
}

pub fn recycle_frame(paddr: u64) {
    FRAME_ALLOCATOR.lock().recycle_frame(paddr);
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

pub struct UserAddressSpace {
    pub satp: usize,
    pub root_paddr: u64,
    pub mapped_bytes: usize,
}

impl UserAddressSpace {
    pub fn new() -> Option<Self> {
        let root_paddr = allocate_frame()?;
        let lvl1_paddr = match allocate_frame() {
            Some(f) => f,
            None => {
                recycle_frame(root_paddr);
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

        Some(Self {
            satp: satp_val,
            root_paddr,
            mapped_bytes: 0,
        })
    }

    pub fn map_user_page(&mut self, vaddr: usize, paddr: u64, is_write: bool, is_exec: bool) -> Result<(), &'static str> {
        if is_write && is_exec {
            return Err("W^X violation: writable user mapping must be non-executable");
        }
        if vaddr >= USER_SPACE_MAX || vaddr < USER_SPACE_MIN {
            return Err("Virtual address outside canonical user range");
        }

        let root = unsafe { &mut *(self.root_paddr as *mut PageTable) };
        let vpn2 = (vaddr >> 30) & 0x1FF;
        let vpn1 = (vaddr >> 21) & 0x1FF;
        let vpn0 = (vaddr >> 12) & 0x1FF;

        // Level 2 root entry -> Level 1 table
        let lvl1_paddr = if root.entries[vpn2] & PTE_V == 0 {
            let frame = allocate_frame().ok_or("OOM allocating Level 1 page table")?;
            root.entries[vpn2] = ((frame >> 12) << 10) | PTE_V;
            frame
        } else {
            (root.entries[vpn2] >> 10) << 12
        };

        // Level 1 entry -> Level 0 table
        let lvl1 = unsafe { &mut *(lvl1_paddr as *mut PageTable) };
        let lvl0_paddr = if lvl1.entries[vpn1] & PTE_V == 0 {
            let frame = allocate_frame().ok_or("OOM allocating Level 0 page table")?;
            lvl1.entries[vpn1] = ((frame >> 12) << 10) | PTE_V;
            frame
        } else {
            if (lvl1.entries[vpn1] & (PTE_R | PTE_X)) != 0 {
                return Err("Virtual address collides with MMIO megapage");
            }
            (lvl1.entries[vpn1] >> 10) << 12
        };

        // Level 0 leaf entry
        let lvl0 = unsafe { &mut *(lvl0_paddr as *mut PageTable) };
        if lvl0.entries[vpn0] & PTE_V != 0 {
            return Err("Page already mapped");
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

    pub fn activate(&self) {
        unsafe {
            switch_satp(self.satp);
        }
    }

    pub fn destroy(&mut self) {
        let root = unsafe { &mut *(self.root_paddr as *mut PageTable) };
        for i in 0..512 {
            if i == 1 || i == 2 {
                continue; // Skip cloned kernel MMIO and DRAM gigapage entries
            }
            let pte2 = root.entries[i];
            if pte2 & PTE_V != 0 && (pte2 & (PTE_R | PTE_X)) == 0 {
                let lvl1_paddr = (pte2 >> 10) << 12;
                let lvl1 = unsafe { &mut *(lvl1_paddr as *mut PageTable) };
                for j in 0..512 {
                    let pte1 = lvl1.entries[j];
                    if pte1 & PTE_V != 0 {
                        if (pte1 & (PTE_R | PTE_X)) == 0 {
                            let lvl0_paddr = (pte1 >> 10) << 12;
                            let lvl0 = unsafe { &mut *(lvl0_paddr as *mut PageTable) };
                            for k in 0..512 {
                                let pte0 = lvl0.entries[k];
                                if (pte0 & (PTE_V | PTE_U)) == (PTE_V | PTE_U) {
                                    recycle_frame((pte0 >> 10) << 12);
                                }
                            }
                            recycle_frame(lvl0_paddr);
                        }
                    }
                }
                recycle_frame(lvl1_paddr);
            }
        }
        recycle_frame(self.root_paddr);
        self.mapped_bytes = 0;
        #[cfg(target_arch = "riscv64")]
        unsafe {
            core::arch::asm!(
                "sfence.vma zero, zero",
                "fence rw, rw",
                "fence.i",
                options(nostack)
            );
        }
    }
}
