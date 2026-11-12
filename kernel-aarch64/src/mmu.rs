// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! AArch64 Memory Management Unit (MMU) & Virtual Memory Architecture
//!
//! Configures ARMv8-A 4-level translation tables with 4KB page granule:
//! - MAIR_EL1: Memory attribute indirection (Attr 0 = Normal Cacheable, Attr 1 = Device-nGnRE)
//! - TCR_EL1: Translation control register (48-bit VA, 4KB granule, Inner Shareable)
//! - TTBR0_EL1: Translation table base register for lower virtual address space
//! - SCTLR_EL1: Enables MMU (M bit), Data Cache (C bit), and Instruction Cache (I bit)

#[repr(C, align(4096))]
#[derive(Copy, Clone)]
pub struct PageTable {
    pub entries: [u64; 512],
}

impl PageTable {
    pub const fn empty() -> Self {
        Self { entries: [0; 512] }
    }
}

// Memory attributes for MAIR_EL1
pub const MAIR_ATTR_NORMAL: u64 = 0xFF;    // Normal Memory, Outer/Inner Write-Back Non-transient Read/Write Allocate
pub const MAIR_ATTR_DEVICE: u64 = 0x04;    // Device-nGnRE Memory (Strict ordering for MMIO)
pub const MAIR_VALUE: u64 = (MAIR_ATTR_DEVICE << 8) | MAIR_ATTR_NORMAL;

// Descriptors bitfields
pub const DESC_VALID: u64 = 1 << 0;
pub const DESC_TABLE: u64 = 1 << 1;        // Next-level table (for Level 0, 1, 2)
pub const DESC_BLOCK: u64 = 0 << 1;        // Block entry (for Level 1 or 2)
pub const DESC_PAGE: u64 = 1 << 1;         // Leaf 4KB page entry (for Level 3)
pub const DESC_ATTR_NORMAL: u64 = 0 << 2;  // AttrIndx = 0
pub const DESC_ATTR_DEVICE: u64 = 1 << 2;  // AttrIndx = 1
pub const DESC_AP_RW_EL1: u64 = 0 << 6;    // Read/Write at EL1 only
pub const DESC_AP_RW_ALL: u64 = 1 << 6;    // Read/Write at EL1 and EL0
pub const DESC_AP_RW_EL0: u64 = 1 << 6;    // User EL0 accessible
pub const DESC_AP_RO_EL1: u64 = 2 << 6;    // Read-Only at EL1 only
pub const DESC_AP_RO_ALL: u64 = 3 << 6;    // Read-Only at EL1 and EL0
pub const DESC_SH_OUTER: u64 = 2 << 8;     // Outer Shareable
pub const DESC_SH_INNER: u64 = 3 << 8;     // Inner Shareable
pub const DESC_AF: u64 = 1 << 10;          // Access Flag (must be 1 to avoid fault)
pub const DESC_PXN: u64 = 1 << 53;         // Privileged Execute-Never (EL1 execution forbidden)
pub const DESC_UXN: u64 = 1 << 54;         // Unprivileged Execute-Never (EL0 execution forbidden)
pub const DESC_XN: u64 = DESC_UXN;         // General Execute-Never for EL1 block entries

// Static primordial page tables
static mut L1_TABLE: PageTable = PageTable::empty();
static mut L2_TABLE: PageTable = PageTable::empty();
/// Level 2 table for the high memory window that the level 1 block entries
/// cannot describe exactly, such as the PCIe ECAM window at 0x4010_0000.
static mut L2_HIGH_TABLE: PageTable = PageTable::empty();

/// Extra level 2 tables, so 2 MiB blocks can be described in gigabytes other
/// than the first.
///
/// WHY: a level 1 entry is either one 1 GiB block or a pointer to a level 2
/// table, so a gigabyte that is already a single block (DRAM, the ECAM window)
/// cannot be subdivided, and a gigabyte with no entry has no way to describe a
/// single 2 MiB block. PCI BAR windows on QEMU's `virt` machine start inside
/// the first gigabyte but run to 0x3EFF0000, which spans four of them, so a
/// table per gigabyte is needed rather than the single shared table that the
/// ECAM window uses.
const HIGH_TABLE_SLOTS: usize = 8;
static mut L2_HIGH_TABLES: [PageTable; HIGH_TABLE_SLOTS] =
    [const { PageTable::empty() }; HIGH_TABLE_SLOTS];
/// Gigabyte each pooled table describes, or `usize::MAX` when it is free.
static mut L2_HIGH_OWNER: [usize; HIGH_TABLE_SLOTS] = [usize::MAX; HIGH_TABLE_SLOTS];

/// Discovered physical RAM base address.
pub static ACTIVE_RAM_BASE: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0x4000_0000);
/// Discovered physical RAM size in bytes.
pub static ACTIVE_RAM_SIZE: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(1024 * 1024 * 1024);

pub struct ArmMmu;

impl ArmMmu {
    pub fn init_and_enable(ram_base: u64, ram_size: u64) {
        let base = if ram_base == 0 { 0x4000_0000 } else { ram_base };
        let size = if ram_size == 0 { 1024 * 1024 * 1024 } else { ram_size };
        ACTIVE_RAM_BASE.store(base, core::sync::atomic::Ordering::Relaxed);
        ACTIVE_RAM_SIZE.store(size, core::sync::atomic::Ordering::Relaxed);

        unsafe {
            // 1. Clear tables
            L1_TABLE = PageTable::empty();
            L2_TABLE = PageTable::empty();

            let l2_phys = core::ptr::addr_of!(L2_TABLE) as u64;

            // 2. Level 1 Entry 0: Point to L2 Table for lower 1 GB (0x0000_0000 .. 0x3FFF_FFFF)
            L1_TABLE.entries[0] = l2_phys | DESC_TABLE | DESC_VALID;

            // 3. Dynamically map discovered RAM gigapages
            const GIGABYTE: u64 = 0x4000_0000;
            let start_gb = (base / GIGABYTE) as usize;
            let num_gbs = ((size + GIGABYTE - 1) / GIGABYTE).max(1) as usize;

            for gb_idx in start_gb..(start_gb + num_gbs).min(512) {
                if gb_idx == 0 {
                    continue;
                }
                L1_TABLE.entries[gb_idx] = ((gb_idx as u64) * GIGABYTE)
                    | DESC_AF
                    | DESC_SH_INNER
                    | DESC_AP_RW_EL1
                    | DESC_ATTR_NORMAL
                    | DESC_UXN
                    | DESC_BLOCK
                    | DESC_VALID;
            }

            // Fallback: always ensure 0x4000_0000 (QEMU virt default DRAM base) is mapped
            if L1_TABLE.entries[1] == 0 {
                L1_TABLE.entries[1] = 0x4000_0000
                    | DESC_AF
                    | DESC_SH_INNER
                    | DESC_AP_RW_EL1
                    | DESC_ATTR_NORMAL
                    | DESC_UXN
                    | DESC_BLOCK
                    | DESC_VALID;
            }

            // 4. Level 2 Table: Populate 2MB blocks for peripheral MMIO & EL0 Userspace
            // User code region (2MB) - mapped from discovered RAM offset + 64MB (0x0400_0000)
            let user_code_phys = base + 0x0400_0000;
            let user_stack_phys = base + 0x0420_0000;

            L2_TABLE.entries[2] = user_code_phys
                | DESC_AF
                | DESC_SH_INNER
                | DESC_AP_RW_ALL
                | DESC_ATTR_NORMAL
                | DESC_PXN
                | DESC_BLOCK
                | DESC_VALID;

            // User stack region (2MB) - Non-executable (PXN + UXN)
            L2_TABLE.entries[4] = user_stack_phys
                | DESC_AF
                | DESC_SH_INNER
                | DESC_AP_RW_ALL
                | DESC_ATTR_NORMAL
                | DESC_PXN
                | DESC_UXN
                | DESC_BLOCK
                | DESC_VALID;

            // Block 8 (0x0100_0000): Allwinner DE2 Display Engine & DSI (2MB) - Device MMIO (PXN + UXN)
            L2_TABLE.entries[8] = 0x0100_0000
                | DESC_AF
                | DESC_SH_OUTER
                | DESC_AP_RW_EL1
                | DESC_ATTR_DEVICE
                | DESC_PXN
                | DESC_UXN
                | DESC_BLOCK
                | DESC_VALID;

            // Block 14 (0x01c0_0000): Allwinner UART0 (0x01c28000), TWI0/I2C0 (0x01c2ac00),
            // GICD (0x01c81000), GICC (0x01c82000) (2MB) - Device MMIO (PXN + UXN)
            L2_TABLE.entries[14] = 0x01c0_0000
                | DESC_AF
                | DESC_SH_OUTER
                | DESC_AP_RW_EL1
                | DESC_ATTR_DEVICE
                | DESC_PXN
                | DESC_UXN
                | DESC_BLOCK
                | DESC_VALID;

            // Block 15 (0x01e0_0000): Allwinner Clock, Reset, and SysCon Registers (2MB) - Device MMIO (PXN + UXN)
            L2_TABLE.entries[15] = 0x01e0_0000
                | DESC_AF
                | DESC_SH_OUTER
                | DESC_AP_RW_EL1
                | DESC_ATTR_DEVICE
                | DESC_PXN
                | DESC_UXN
                | DESC_BLOCK
                | DESC_VALID;

            // Block 64 (0x0800_0000): GICv2 Distributor & CPU Interface (2MB) - Device MMIO (PXN + UXN)
            L2_TABLE.entries[64] = 0x0800_0000
                | DESC_AF
                | DESC_SH_OUTER
                | DESC_AP_RW_EL1
                | DESC_ATTR_DEVICE
                | DESC_PXN
                | DESC_UXN
                | DESC_BLOCK
                | DESC_VALID;

            // Block 72 (0x0900_0000): PrimeCell PL011 UART (2MB) - Device MMIO (PXN + UXN)
            L2_TABLE.entries[72] = 0x0900_0000
                | DESC_AF
                | DESC_SH_OUTER
                | DESC_AP_RW_EL1
                | DESC_ATTR_DEVICE
                | DESC_PXN
                | DESC_UXN
                | DESC_BLOCK
                | DESC_VALID;

            // 5. Configure MAIR_EL1
            #[cfg(target_arch = "aarch64")]
            {
                core::arch::asm!("msr mair_el1, {}", in(reg) MAIR_VALUE);

                // 6. Configure TCR_EL1:
                // T0SZ = 25 (39-bit VA space, Level 1 is root table)
                // TG0 = 0b00 (4KB granule)
                // IRGN0 = 0b01, ORGN0 = 0b01 (Write-Back)
                // SH0 = 0b11 (Inner Shareable)
                // EPD1 = 1 (Disable TTBR1 walks to isolate upper VA space)
                // IPS = 0b100 (48-bit physical addressing).
                let tcr_val: u64 = (25 << 0)          // T0SZ = 25 (39-bit VA, L1 root)
                    | (1 << 8)                        // IRGN0 = 0b01 (Inner WB WA)
                    | (1 << 10)                       // ORGN0 = 0b01 (Outer WB WA)
                    | (3 << 12)                       // SH0 = 0b11 (Inner Shareable)
                    | (0 << 14)                       // TG0 = 0b00 (4KB Granule)
                    | (1 << 23)                       // EPD1 = 1 (Disable unconfigured TTBR1 table walks)
                    | (4u64 << 32);                   // IPS = 0b100 (48-bit PA)
                core::arch::asm!("msr tcr_el1, {}", in(reg) tcr_val);


                // 7. Install TTBR0_EL1
                let l1_phys = core::ptr::addr_of!(L1_TABLE) as u64;
                core::arch::asm!("msr ttbr0_el1, {}", in(reg) l1_phys);
                core::arch::asm!("isb");

                // 8. Invalidate unified TLB
                core::arch::asm!("tlbi vmalle1");
                core::arch::asm!("dsb ish");
                core::arch::asm!("isb");

                // 9. Enable MMU, D-Cache, and I-Cache in SCTLR_EL1:
                // Bit 0 (M) = 1 (MMU enable)
                // Bit 2 (C) = 1 (Data Cache enable)
                // Bit 12 (I) = 1 (Instruction Cache enable)
                let mut sctlr: u64;
                core::arch::asm!("mrs {}, sctlr_el1", out(reg) sctlr);
                sctlr |= (1 << 0) | (1 << 2) | (1 << 12);
                core::arch::asm!("msr sctlr_el1, {}", in(reg) sctlr);
                core::arch::asm!("isb");
            }

            let alloc_base = base + 0x0800_0000;
            let alloc_frames = ((size.saturating_sub(0x0800_0000)) / 4096).min(65536) as usize;
            init_frame_allocator(alloc_base, alloc_frames);
        }
    }

    /// Maps a small device region by covering it with 2 MiB blocks.
    ///
    /// A device BAR is typically far smaller than the mapping granularity and
    /// is rarely aligned to it, so it cannot be described by a single level 2
    /// block. Every 2 MiB block that overlaps the region is mapped instead.
    ///
    /// `guard_lo..guard_hi` is a range the caller holds for normal memory; any
    /// block that would map inside it is refused, so a coarse mapping can never
    /// shadow RAM.
    pub fn map_device_region_guarded(
        start_paddr: u64,
        size: u64,
        guard_lo: u64,
        guard_hi: u64,
    ) -> bool {
        const L2_BLOCK: u64 = 0x20_0000;

        if size == 0 || start_paddr == 0 {
            return false;
        }
        let first = start_paddr / L2_BLOCK;
        let last = (start_paddr + size - 1) / L2_BLOCK;
        for index in first..=last {
            let block_lo = index * L2_BLOCK;
            let block_hi = block_lo + L2_BLOCK;
            if block_lo < guard_hi && block_hi > guard_lo {
                // The block would shadow normal memory; refuse it.
                return false;
            }
        }
        // Only gigabytes already covered by the primordial level 2 table are
        // usable here; anything else needs its own table and must go through
        // map_device_region_high.
        if unsafe { L1_TABLE.entries[((first >> 9) & 0x1FF) as usize] } & DESC_TABLE == 0 {
            return false;
        }
        for index in first..=last {
            unsafe {
                L2_TABLE.entries[(index & 0x1FF) as usize] = (index * L2_BLOCK)
                    | DESC_AF
                    | DESC_SH_OUTER
                    | DESC_AP_RW_EL1
                    | DESC_ATTR_DEVICE
                    | DESC_PXN
                    | DESC_UXN
                    | DESC_BLOCK
                    | DESC_VALID;
            }
        }
        unsafe {
            core::arch::asm!("dsb ish", options(nostack, preserves_flags));
            core::arch::asm!("tlbi vmalle1", options(nostack, preserves_flags));
            core::arch::asm!("dsb ish", options(nostack, preserves_flags));
            core::arch::asm!("isb", options(nostack, preserves_flags));
        }
        true
    }

    /// Maps a device MMIO region as uncached, strongly ordered memory.
    ///
    /// Uses the coarsest level that can represent the region exactly: a 1 GiB
    /// level 1 block when the range is 1 GiB aligned, otherwise 2 MiB level 2
    /// blocks inside the first gigabyte. Returns false without touching the
    /// tables when the range is not representable, so a caller never proceeds on
    /// a window that was silently left unmapped.
    ///
    /// This matters for the PCIe ECAM window, which sits at 0x4010000000 on
    /// QEMU's `virt` machine and is far outside the first gigabyte.
    pub fn map_device_region(start_paddr: u64, size: u64) -> bool {
        const L1_BLOCK: u64 = 0x4000_0000; // 1 GiB
        const L2_BLOCK: u64 = 0x20_0000; // 2 MiB

        if size == 0 {
            return false;
        }

        if start_paddr % L1_BLOCK == 0 && size % L1_BLOCK == 0 {
            let first = start_paddr / L1_BLOCK;
            let last = first + (size / L1_BLOCK);
            if last > 512 {
                return false;
            }
            unsafe {
                for index in first..last {
                    L1_TABLE.entries[index as usize] = (index * L1_BLOCK)
                        | DESC_AF
                        | DESC_SH_OUTER
                        | DESC_AP_RW_EL1
                        | DESC_ATTR_DEVICE
                        | DESC_PXN
                        | DESC_UXN
                        | DESC_BLOCK
                        | DESC_VALID;
                }
            }
            unsafe {
                core::arch::asm!("dsb ish", options(nostack, preserves_flags));
                core::arch::asm!("tlbi vmalle1", options(nostack, preserves_flags));
                core::arch::asm!("dsb ish", options(nostack, preserves_flags));
                core::arch::asm!("isb", options(nostack, preserves_flags));
            }
            return true;
        }

        if start_paddr % L2_BLOCK != 0 || size % L2_BLOCK != 0 {
            return false;
        }
        let first = start_paddr / L2_BLOCK;
        let last = first + (size / L2_BLOCK);
        // Only the first gigabyte is reachable through the level 2 table.
        if last > 512 {
            return false;
        }
        unsafe {
            for index in first..last {
                L2_TABLE.entries[index as usize] = (index * L2_BLOCK)
                    | DESC_AF
                    | DESC_SH_OUTER
                    | DESC_AP_RW_EL1
                    | DESC_ATTR_DEVICE
                    | DESC_PXN
                    | DESC_UXN
                    | DESC_BLOCK
                    | DESC_VALID;
            }
            core::arch::asm!("dsb ish", options(nostack, preserves_flags));
            core::arch::asm!("tlbi vmalle1", options(nostack, preserves_flags));
            core::arch::asm!("dsb ish", options(nostack, preserves_flags));
            core::arch::asm!("isb", options(nostack, preserves_flags));
        }
        true
    }

    /// Maps a device region that is neither 1 GiB aligned nor inside the first
    /// gigabyte, by giving it its own level 2 table.
    ///
    /// A level 1 entry can only point at a level 2 table, so the region is
    /// mapped relative to the gigabyte its start address falls in. This is the
    /// path the PCIe ECAM window takes on QEMU's `virt` machine, which sits at
    /// 256 GiB + 256 MiB and is therefore 2 MiB but not 1 GiB aligned.
    pub fn map_device_region_high(start_paddr: u64, size: u64) -> bool {
        const L1_BLOCK: u64 = 0x4000_0000; // 1 GiB
        const L2_BLOCK: u64 = 0x20_0000; // 2 MiB

        if size == 0 || start_paddr % L2_BLOCK != 0 || size % L2_BLOCK != 0 {
            return false;
        }
        let l1_index = start_paddr / L1_BLOCK;
        // The window must sit inside a single gigabyte.
        if (start_paddr + size - 1) / L1_BLOCK != l1_index {
            return false;
        }
        let first_in_gib = (start_paddr % L1_BLOCK) / L2_BLOCK;
        let count = size / L2_BLOCK;
        if first_in_gib + count > 512 {
            return false;
        }

        unsafe {
            let l2_phys = core::ptr::addr_of!(L2_HIGH_TABLE) as u64;
            L1_TABLE.entries[l1_index as usize] = l2_phys | DESC_TABLE | DESC_VALID;
            // The level 1 entry is a table pointer, not a block, so the leaf
            // descriptor must carry the FULL physical address of each 2 MiB
            // block. Writing the offset within the gigabyte here pointed every
            // leaf at physical zero, which made the mapped window read back
            // whatever happened to be there.
            for step in 0..count {
                L2_HIGH_TABLE.entries[(first_in_gib + step) as usize] =
                    (start_paddr + step * L2_BLOCK)
                        | DESC_AF
                        | DESC_SH_OUTER
                        | DESC_AP_RW_EL1
                        | DESC_ATTR_DEVICE
                        | DESC_PXN
                        | DESC_UXN
                        | DESC_BLOCK
                        | DESC_VALID;
            }
            core::arch::asm!("dsb ish", options(nostack, preserves_flags));
            core::arch::asm!("tlbi vmalle1", options(nostack, preserves_flags));
            core::arch::asm!("dsb ish", options(nostack, preserves_flags));
            core::arch::asm!("isb", options(nostack, preserves_flags));
        }
        true
    }

    /// Maps an arbitrary device region with 2 MiB blocks, allocating a level 2
    /// table for every gigabyte it covers.
    ///
    /// This is the path a PCI BAR takes. A BAR is usually a few kilobytes and
    /// almost never 2 MiB aligned, so it can only be described by the blocks
    /// that cover it, and those blocks may live in any gigabyte.
    ///
    /// Blocks that are already mapped are left alone, so calling this twice for
    /// the same region succeeds and does not disturb anything.
    ///
    /// `guard_lo..guard_hi` is a range held for normal memory. A block that
    /// would *newly* shadow it is refused, but a block that is already mapped is
    /// accepted, so re-mapping a region that another stage deliberately placed
    /// cannot fail spuriously. Returns false without touching any table when a
    /// gigabyte cannot be subdivided, so a caller never proceeds on a window
    /// that was silently left unmapped.
    pub fn map_device_blocks(
        start_paddr: u64,
        size: u64,
        guard_lo: u64,
        guard_hi: u64,
    ) -> bool {
        const L1_BLOCK: u64 = 0x4000_0000;
        const L2_BLOCK: u64 = 0x20_0000;

        if size == 0 || start_paddr == 0 {
            return false;
        }
        let first = start_paddr / L2_BLOCK;
        let last = (start_paddr + size - 1) / L2_BLOCK;
        unsafe {
            if last / (L1_BLOCK / L2_BLOCK) >= 512 {
                // Outside the 39-bit virtual address space the tables describe.
                return false;
            }

            // Everything must be doable before anything is written, so a refusal
            // leaves the tables exactly as they were.
            for index in first..=last {
                if Self::block_is_mapped(index) {
                    continue;
                }
                let block_lo = index * L2_BLOCK;
                if block_lo < guard_hi && block_lo + L2_BLOCK > guard_lo {
                    // A new block would shadow normal memory; refuse it.
                    return false;
                }
                if Self::table_for_gigabyte(index / (L1_BLOCK / L2_BLOCK)).is_none() {
                    return false;
                }
            }

            for index in first..=last {
                if Self::block_is_mapped(index) {
                    continue;
                }
                let gib = index / (L1_BLOCK / L2_BLOCK);
                if let Some(table) = Self::table_for_gigabyte(gib) {
                    table.entries[(index % (L1_BLOCK / L2_BLOCK)) as usize] = (index * L2_BLOCK)
                        | DESC_AF
                        | DESC_SH_OUTER
                        | DESC_AP_RW_EL1
                        | DESC_ATTR_DEVICE
                        | DESC_PXN
                        | DESC_UXN
                        | DESC_BLOCK
                        | DESC_VALID;
                }
            }
            core::arch::asm!("dsb ish", options(nostack, preserves_flags));
            core::arch::asm!("tlbi vmalle1", options(nostack, preserves_flags));
            core::arch::asm!("dsb ish", options(nostack, preserves_flags));
            core::arch::asm!("isb", options(nostack, preserves_flags));
        }
        true
    }

    /// Returns the level 2 table describing `gib`, if it can describe one.
    ///
    /// The first gigabyte has the primordial table. Any other gigabyte must
    /// have either no level 1 entry at all or one that is already a table
    /// pointer; a gigabyte that is a single 1 GiB block (DRAM, or the ECAM
    /// window) holds state that subdividing it would destroy.
    unsafe fn table_for_gigabyte(gib: u64) -> Option<&'static mut PageTable> {
        if gib == 0 {
            return Some(&mut *(&raw mut L2_TABLE));
        }
        if gib >= 512 {
            return None;
        }
        let entry = L1_TABLE.entries[gib as usize];
        if entry & DESC_VALID == 0 {
            // Unused gigabyte: claim a pooled table for it.
            for slot in 0..HIGH_TABLE_SLOTS {
                if L2_HIGH_OWNER[slot] == usize::MAX {
                    L2_HIGH_OWNER[slot] = gib as usize;
                    L1_TABLE.entries[gib as usize] =
                        core::ptr::addr_of!(L2_HIGH_TABLES[slot]) as u64 | DESC_TABLE | DESC_VALID;
                    return Some(&mut *(&raw mut L2_HIGH_TABLES[slot]));
                }
            }
            return None;
        }
        if entry & DESC_TABLE == 0 {
            // A 1 GiB block already describes this gigabyte.
            return None;
        }
        for slot in 0..HIGH_TABLE_SLOTS {
            if L2_HIGH_OWNER[slot] == gib as usize {
                return Some(&mut *(&raw mut L2_HIGH_TABLES[slot]));
            }
        }
        // Some other table owns the gigabyte (the ECAM window's).
        None
    }

    /// True when the 2 MiB block is already described in the tables.
    fn block_is_mapped(index: u64) -> bool {
        const PER_GIB: u64 = 512;
        let gib = index / PER_GIB;
        let entry = unsafe { L1_TABLE.entries[gib as usize] };
        if entry & DESC_VALID == 0 {
            return false;
        }
        if entry & DESC_TABLE == 0 {
            return true;
        }
        let slot = (index % PER_GIB) as usize;
        if gib == 0 {
            return unsafe { L2_TABLE.entries[slot] } & DESC_VALID != 0;
        }
        for slot_index in 0..HIGH_TABLE_SLOTS {
            if unsafe { L2_HIGH_OWNER[slot_index] } == gib as usize {
                return unsafe { L2_HIGH_TABLES[slot_index].entries[slot] } & DESC_VALID != 0;
            }
        }
        true
    }

    /// Returns true when the MMU is currently enabled.
    pub fn is_enabled() -> bool {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            let sctlr: u64;
            core::arch::asm!("mrs {}, sctlr_el1", out(reg) sctlr);
            (sctlr & 1) != 0
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            true
        }
    }

    /// Creates an isolated per-task translation table tree for an EL0 task.
    ///
    /// User code and stack regions are marked with `DESC_AP_RW_EL0`, while
    /// physical DRAM kernel regions and MMIO are strictly `DESC_AP_RW_EL1`.
    pub fn create_user_page_table() -> u64 {
        extern crate alloc;
        use alloc::boxed::Box;

        let mut user_l1 = Box::new(PageTable::empty());
        let mut user_l2 = Box::new(PageTable::empty());

        let l2_phys = user_l2.as_ref() as *const PageTable as u64;

        let r_base = ACTIVE_RAM_BASE.load(core::sync::atomic::Ordering::Relaxed);
        let r_size = ACTIVE_RAM_SIZE.load(core::sync::atomic::Ordering::Relaxed);
        const GIGABYTE: u64 = 0x4000_0000;
        let start_gb = (r_base / GIGABYTE) as usize;
        let num_gbs = ((r_size + GIGABYTE - 1) / GIGABYTE).max(1) as usize;

        // Level 1 Entry 0: points to user L2 table
        user_l1.entries[0] = l2_phys | DESC_TABLE | DESC_VALID;

        // Map all discovered RAM gigapages as Kernel-only (DESC_AP_RW_EL1)
        for gb_idx in start_gb..(start_gb + num_gbs).min(512) {
            if gb_idx == 0 {
                continue;
            }
            user_l1.entries[gb_idx] = ((gb_idx as u64) * GIGABYTE)
                | DESC_AF
                | DESC_SH_INNER
                | DESC_AP_RW_EL1
                | DESC_ATTR_NORMAL
                | DESC_BLOCK
                | DESC_VALID;
        }

        if user_l1.entries[1] == 0 {
            user_l1.entries[1] = 0x4000_0000
                | DESC_AF
                | DESC_SH_INNER
                | DESC_AP_RW_EL1
                | DESC_ATTR_NORMAL
                | DESC_BLOCK
                | DESC_VALID;
        }

        let user_code_phys = r_base + 0x0400_0000;
        let user_stack_phys = r_base + 0x0420_0000;

        // Populate User L2:
        // Block 2 (0x0040_0000): EL0 Userspace Code Region (2MB) with DESC_AP_RW_EL0 + PXN
        // PXN prevents EL1 supervisor from executing code placed in user space (ret2usr / PAN protection).
        user_l2.entries[2] = user_code_phys
            | DESC_AF
            | DESC_SH_INNER
            | DESC_AP_RW_EL0
            | DESC_ATTR_NORMAL
            | DESC_PXN
            | DESC_BLOCK
            | DESC_VALID;

        // Block 4 (0x0080_0000): EL0 Userspace Stack Region (2MB) - Strictly Non-Executable (PXN + UXN)
        // Enforces W^X / Data Execution Prevention (DEP) on user stack.
        user_l2.entries[4] = user_stack_phys
            | DESC_AF
            | DESC_SH_INNER
            | DESC_AP_RW_EL0
            | DESC_ATTR_NORMAL
            | DESC_PXN
            | DESC_UXN
            | DESC_BLOCK
            | DESC_VALID;

        // PinePhone A64 MMIO: Block 8 (DE2 Display Engine) (Kernel ONLY, PXN + UXN)
        user_l2.entries[8] = 0x0100_0000
            | DESC_AF
            | DESC_SH_OUTER
            | DESC_AP_RW_EL1
            | DESC_ATTR_DEVICE
            | DESC_PXN
            | DESC_UXN
            | DESC_BLOCK
            | DESC_VALID;

        // PinePhone A64 MMIO: Block 14 (UART0, TWI0, GIC-400) (Kernel ONLY, PXN + UXN)
        user_l2.entries[14] = 0x01c0_0000
            | DESC_AF
            | DESC_SH_OUTER
            | DESC_AP_RW_EL1
            | DESC_ATTR_DEVICE
            | DESC_PXN
            | DESC_UXN
            | DESC_BLOCK
            | DESC_VALID;

        // PinePhone A64 MMIO: Block 15 (Clock/SysCon) (Kernel ONLY, PXN + UXN)
        user_l2.entries[15] = 0x01e0_0000
            | DESC_AF
            | DESC_SH_OUTER
            | DESC_AP_RW_EL1
            | DESC_ATTR_DEVICE
            | DESC_PXN
            | DESC_UXN
            | DESC_BLOCK
            | DESC_VALID;

        // QEMU virt MMIO: Block 64 (GICv2) (Kernel ONLY, PXN + UXN)
        user_l2.entries[64] = 0x0800_0000
            | DESC_AF
            | DESC_SH_OUTER
            | DESC_AP_RW_EL1
            | DESC_ATTR_DEVICE
            | DESC_PXN
            | DESC_UXN
            | DESC_BLOCK
            | DESC_VALID;

        // QEMU virt MMIO: Block 72 (PL011 UART) (Kernel ONLY, PXN + UXN)
        user_l2.entries[72] = 0x0900_0000
            | DESC_AF
            | DESC_SH_OUTER
            | DESC_AP_RW_EL1
            | DESC_ATTR_DEVICE
            | DESC_PXN
            | DESC_UXN
            | DESC_BLOCK
            | DESC_VALID;

        let root_phys = Box::into_raw(user_l1) as u64;
        let _ = Box::into_raw(user_l2); // prevent deallocation
        root_phys
    }

    /// Ensures that physical address range `paddr..paddr+size` is mapped in the
    /// active translation tables. If it is already mapped, returns true immediately.
    /// Otherwise, maps 2 MiB blocks covering the range as device memory.
    pub fn ensure_mapped(paddr: u64, size: u64) -> bool {
        const L2_BLOCK: u64 = 0x20_0000;
        if size == 0 || paddr == 0 {
            return false;
        }
        let first = paddr / L2_BLOCK;
        let last = (paddr + size - 1) / L2_BLOCK;
        let mut all_mapped = true;
        for idx in first..=last {
            if !Self::block_is_mapped(idx) {
                all_mapped = false;
                break;
            }
        }
        if all_mapped {
            return true;
        }
        Self::map_device_blocks(paddr, size, 0, 0)
    }

    /// Switches the active TTBR0_EL1 register to the given task's page table.
    pub fn switch_to_task(ttbr0: u64) {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!(
                "msr ttbr0_el1, {0}",
                "isb",
                "tlbi vmalle1is",
                "dsb ish",
                "isb",
                in(reg) ttbr0
            );
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            let _ = ttbr0;
        }
    }

    pub fn kernel_ttbr0() -> u64 {
        (&raw const L1_TABLE) as usize as u64
    }

    pub fn switch_to_kernel() {
        Self::switch_to_task(Self::kernel_ttbr0());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameOwnership {
    Kernel = 0,
    Mmio = 1,
    Borrowed = 2,
    Shared = 3,
    PrivateOwned = 4,
}

pub struct ArmFrameAllocator {
    pub base_paddr: u64,
    pub total_frames: usize,
    pub allocated_frames: usize,
    pub recycled_frames: usize,
    pub bitmap: [u64; 1024], // 65,536 frames = 256 MiB
    pub ownership: [u8; 65536],
    pub ref_counts: [u16; 65536],
}

impl ArmFrameAllocator {
    pub const fn empty() -> Self {
        Self {
            base_paddr: 0,
            total_frames: 0,
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
        let word_idx = frame_idx / 64;
        let bit_idx = frame_idx % 64;
        if (self.bitmap[word_idx] & (1u64 << bit_idx)) == 0 {
            return Err("Cannot unmap unallocated frame");
        }
        if self.ownership[frame_idx] != FrameOwnership::PrivateOwned as u8 {
            return Err("Not a private frame");
        }
        if self.ref_counts[frame_idx] != 1 {
            return Err("Private frame is not currently mapped (refcount != 1)");
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
            Ok(true)
        } else {
            Ok(false)
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

pub static FRAME_ALLOCATOR: spin::Mutex<ArmFrameAllocator> = spin::Mutex::new(ArmFrameAllocator::empty());

pub fn init_frame_allocator(base_paddr: u64, total_frames: usize) {
    let mut alloc = FRAME_ALLOCATOR.lock();
    alloc.base_paddr = base_paddr;
    alloc.total_frames = total_frames.min(65536);
    alloc.allocated_frames = 0;
    alloc.recycled_frames = 0;
    alloc.bitmap = [0; 1024];
    alloc.ownership = [0; 65536];
    alloc.ref_counts = [0; 65536];
}

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

pub fn current_cpu_id() -> u8 {
    let mpidr: u64;
    unsafe {
        core::arch::asm!("mrs {0}, mpidr_el1", out(reg) mpidr);
    }
    (mpidr & 0xFF) as u8
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
    pub vaddr: u64,
    pub paddr: u64,
    pub kind: MappingKind,
    pub is_write: bool,
    pub is_exec: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeardownError {
    AlreadyDestroyed,
    ActiveOnRemoteCpu(u32),
    RemoteRendezvousFailed(u32),
    QuiescenceTimeout,
    FrameRecycleFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationError {
    ClosingOrDestroyed,
    CpuIdOutOfRange(u32),
}

pub const LIFECYCLE_ACTIVE_CPUS_MASK: u32 = 0x0000_FFFF; // bits 0..15: bitmask of active CPUs
pub const LIFECYCLE_CLOSING_BIT: u32 = 1 << 31;
pub const LIFECYCLE_DESTROYED_BIT: u32 = 1 << 30;

/// Validates that a physical address belongs to a documented QEMU virt peripheral MMIO aperture.
pub fn is_valid_mmio_peripheral(paddr: u64, len: usize) -> bool {
    if (paddr & 0xFFF) != 0 || len == 0 || (len & 0xFFF) != 0 {
        return false;
    }
    let end = match paddr.checked_add(len as u64) {
        Some(e) => e,
        None => return false,
    };
    (paddr >= 0x0800_0000 && end <= 0x0802_0000) // GIC Distributor & CPU interface
        || (paddr >= 0x0900_0000 && end <= 0x0901_0000) // PL011 UART0
        || (paddr >= 0x0a00_0000 && end <= 0x0a01_0000) // VirtIO MMIO slots 0..31
        || (paddr >= 0x1000_0000 && end <= 0x1200_0000) // VirtIO GPU Display BAR aperture
}

extern crate alloc;
use alloc::vec::Vec;

pub struct UserAddressSpace {
    pub ttbr0: u64,
    pub table_frames: Vec<u64>,
    pub mappings: Vec<UserMapping>,
    pub mapped_bytes: usize,
    pub destroyed: bool,
    pub lifecycle: core::sync::atomic::AtomicU32,
    pub executed_cpus: core::sync::atomic::AtomicU32,
    pub acknowledged_cpus: core::sync::atomic::AtomicU32,
}

impl UserAddressSpace {
    pub fn is_closing(&self) -> bool {
        (self.lifecycle.load(core::sync::atomic::Ordering::Acquire) & LIFECYCLE_CLOSING_BIT) != 0
    }

    pub fn is_destroyed(&self) -> bool {
        self.destroyed || (self.lifecycle.load(core::sync::atomic::Ordering::Acquire) & LIFECYCLE_DESTROYED_BIT) != 0
    }

    pub fn active_cpus(&self) -> u32 {
        self.lifecycle.load(core::sync::atomic::Ordering::Acquire) & LIFECYCLE_ACTIVE_CPUS_MASK
    }

    pub fn is_active_on_any_cpu(&self) -> bool {
        self.active_cpus() != 0
    }

    pub fn has_unacknowledged_or_active_cpus(&self) -> bool {
        let active = self.active_cpus();
        let executed = self.executed_cpus.load(core::sync::atomic::Ordering::Acquire);
        let acknowledged = self.acknowledged_cpus.load(core::sync::atomic::Ordering::Acquire);
        active != 0 || (executed & !acknowledged) != 0
    }

    pub fn is_evacuated_for_current(&self) -> bool {
        let cid = current_cpu_id() as u32;
        if cid >= 16 {
            return true;
        }
        let cpu_bit = 1u32 << cid;
        let active = self.active_cpus();
        let executed = self.executed_cpus.load(core::sync::atomic::Ordering::Acquire);
        let acknowledged = self.acknowledged_cpus.load(core::sync::atomic::Ordering::Acquire);
        (active & cpu_bit) == 0 && (executed & !acknowledged & cpu_bit) == 0
    }

    pub fn new() -> Option<Self> {
        let root_l1_phys = allocate_frame_with_ownership(FrameOwnership::Kernel)?;
        let l2_phys = match allocate_frame_with_ownership(FrameOwnership::Kernel) {
            Some(f) => f,
            None => {
                let _ = recycle_frame(root_l1_phys);
                return None;
            }
        };

        let user_l1 = unsafe { &mut *(root_l1_phys as *mut PageTable) };
        let user_l2 = unsafe { &mut *(l2_phys as *mut PageTable) };

        user_l1.entries[0] = l2_phys | DESC_TABLE | DESC_VALID;

        let r_base = ACTIVE_RAM_BASE.load(core::sync::atomic::Ordering::Relaxed);
        let r_size = ACTIVE_RAM_SIZE.load(core::sync::atomic::Ordering::Relaxed);
        const GIGABYTE: u64 = 0x4000_0000;
        let start_gb = (r_base / GIGABYTE) as usize;
        let num_gbs = ((r_size + GIGABYTE - 1) / GIGABYTE).max(1) as usize;

        for gb_idx in start_gb..(start_gb + num_gbs).min(512) {
            if gb_idx == 0 {
                continue;
            }
            user_l1.entries[gb_idx] = ((gb_idx as u64) * GIGABYTE)
                | DESC_AF
                | DESC_SH_INNER
                | DESC_AP_RW_EL1
                | DESC_ATTR_NORMAL
                | DESC_BLOCK
                | DESC_VALID;
        }

        if user_l1.entries[1] == 0 {
            user_l1.entries[1] = 0x4000_0000
                | DESC_AF
                | DESC_SH_INNER
                | DESC_AP_RW_EL1
                | DESC_ATTR_NORMAL
                | DESC_BLOCK
                | DESC_VALID;
        }

        user_l2.entries[8] = 0x0100_0000 | DESC_AF | DESC_SH_OUTER | DESC_AP_RW_EL1 | DESC_ATTR_DEVICE | DESC_PXN | DESC_UXN | DESC_BLOCK | DESC_VALID;
        user_l2.entries[14] = 0x01c0_0000 | DESC_AF | DESC_SH_OUTER | DESC_AP_RW_EL1 | DESC_ATTR_DEVICE | DESC_PXN | DESC_UXN | DESC_BLOCK | DESC_VALID;
        user_l2.entries[15] = 0x01e0_0000 | DESC_AF | DESC_SH_OUTER | DESC_AP_RW_EL1 | DESC_ATTR_DEVICE | DESC_PXN | DESC_UXN | DESC_BLOCK | DESC_VALID;
        user_l2.entries[64] = 0x0800_0000 | DESC_AF | DESC_SH_OUTER | DESC_AP_RW_EL1 | DESC_ATTR_DEVICE | DESC_PXN | DESC_UXN | DESC_BLOCK | DESC_VALID;
        user_l2.entries[72] = 0x0900_0000 | DESC_AF | DESC_SH_OUTER | DESC_AP_RW_EL1 | DESC_ATTR_DEVICE | DESC_PXN | DESC_UXN | DESC_BLOCK | DESC_VALID;

        // Map PCI MMIO aperture (0x1000_0000 .. 0x1200_0000: 32 MiB covering BAR 0 & BAR 2) for kernel EL1 display
        for i in 128..144 {
            user_l2.entries[i] = ((i as u64) << 21)
                | DESC_AF
                | DESC_SH_OUTER
                | DESC_AP_RW_EL1
                | DESC_ATTR_DEVICE
                | DESC_PXN
                | DESC_UXN
                | DESC_BLOCK
                | DESC_VALID;
        }

        let mut table_frames = Vec::new();
        table_frames.push(root_l1_phys);
        table_frames.push(l2_phys);

        Some(Self {
            ttbr0: root_l1_phys,
            table_frames,
            mappings: Vec::new(),
            mapped_bytes: 0,
            destroyed: false,
            lifecycle: core::sync::atomic::AtomicU32::new(0),
            executed_cpus: core::sync::atomic::AtomicU32::new(0),
            acknowledged_cpus: core::sync::atomic::AtomicU32::new(0),
        })
    }

    fn map_leaf_entry(&mut self, vaddr: u64, paddr: u64, is_write: bool, is_exec: bool) -> Result<(), &'static str> {
        let l1 = unsafe { &mut *(self.ttbr0 as *mut PageTable) };
        let l1_idx = ((vaddr >> 30) & 0x1FF) as usize;
        if l1_idx != 0 {
            return Err("Virtual address outside L1 user slot (0..1GB)");
        }
        let l1_entry = l1.entries[0];
        if l1_entry & DESC_VALID == 0 {
            return Err("L1 root table corrupt");
        }
        let l2_phys = l1_entry & 0x0000_FFFF_FFFF_F000;
        let l2 = unsafe { &mut *(l2_phys as *mut PageTable) };

        let l2_idx = ((vaddr >> 21) & 0x1FF) as usize;
        let l2_entry = l2.entries[l2_idx];
        if l2_entry & DESC_VALID != 0 && (l2_entry & DESC_TABLE == 0) {
            return Err("Cannot map user page over device block entry");
        }
        let mut newly_allocated_l3 = None;
        let l3_phys = if l2_entry & DESC_VALID == 0 {
            let frame = allocate_frame_with_ownership(FrameOwnership::Kernel).ok_or("OOM allocating L3 page table")?;
            newly_allocated_l3 = Some(frame);
            frame
        } else {
            l2_entry & 0x0000_FFFF_FFFF_F000
        };

        let l3 = unsafe { &mut *(l3_phys as *mut PageTable) };
        let l3_idx = ((vaddr >> 12) & 0x1FF) as usize;
        if l3.entries[l3_idx] & DESC_VALID != 0 {
            if let Some(new_frame) = newly_allocated_l3 {
                let _ = recycle_frame(new_frame);
            }
            return Err("physical frame already owned by address space");
        }

        if let Some(new_frame) = newly_allocated_l3 {
            l2.entries[l2_idx] = new_frame | DESC_TABLE | DESC_VALID;
            self.table_frames.push(new_frame);
        }

        let mut flags = DESC_PAGE | DESC_AF | DESC_SH_INNER | DESC_ATTR_NORMAL | DESC_VALID | DESC_PXN;
        if is_write {
            flags |= DESC_AP_RW_ALL;
        } else {
            flags |= DESC_AP_RO_ALL;
        }
        if !is_exec {
            flags |= DESC_UXN;
        }

        l3.entries[l3_idx] = (paddr & !0xFFF) | flags;
        self.mapped_bytes += 4096;
        Ok(())
    }

    pub fn map_private_frame(&mut self, vaddr: u64, paddr: u64, is_write: bool, is_exec: bool) -> Result<(), &'static str> {
        if (paddr & 0xFFF) != 0 || (vaddr & 0xFFF) != 0 {
            return Err("Address must be page-aligned (4KB)");
        }
        if self.destroyed || self.is_closing() {
            return Err("Cannot map into destroyed or closing address space");
        }
        if is_write && is_exec {
            return Err("W^X violation: writable user mapping must be non-executable");
        }
        if vaddr >= 0x4000_0000 || vaddr < 0x1000 {
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
                crate::kprintln!("[MMU ERROR] Failed to unmap private frame 0x{:x} during rollback: {}", paddr, unmap_err);
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

    pub fn map_shared_frame(&mut self, vaddr: u64, paddr: u64, is_write: bool, is_exec: bool) -> Result<(), &'static str> {
        if (paddr & 0xFFF) != 0 || (vaddr & 0xFFF) != 0 {
            return Err("Address must be page-aligned (4KB)");
        }
        if self.destroyed || self.is_closing() {
            return Err("Cannot map into destroyed or closing address space");
        }
        if is_write && is_exec {
            return Err("W^X violation: writable user mapping must be non-executable");
        }
        if vaddr >= 0x4000_0000 || vaddr < 0x1000 {
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
                crate::kprintln!("[MMU ERROR] Failed to release shared frame 0x{:x} during rollback: {}", paddr, rel_err);
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

    pub fn map_borrowed_frame(&mut self, vaddr: u64, paddr: u64, is_write: bool, is_exec: bool) -> Result<(), &'static str> {
        if (paddr & 0xFFF) != 0 || (vaddr & 0xFFF) != 0 {
            return Err("Address must be page-aligned (4KB)");
        }
        if self.destroyed || self.is_closing() {
            return Err("Cannot map into destroyed or closing address space");
        }
        if is_write && is_exec {
            return Err("W^X violation: writable user mapping must be non-executable");
        }
        if vaddr >= 0x4000_0000 || vaddr < 0x1000 {
            return Err("Virtual address outside canonical user range");
        }
        let own = frame_ownership(paddr);
        let base = ACTIVE_RAM_BASE.load(core::sync::atomic::Ordering::Relaxed);
        let size = ACTIVE_RAM_SIZE.load(core::sync::atomic::Ordering::Relaxed);
        let in_ram = paddr >= base && (size == 0 || paddr < base.saturating_add(size));
        if in_ram {
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

    pub fn map_user_page(&mut self, vaddr: u64, paddr: u64, is_write: bool, is_exec: bool) -> Result<(), &'static str> {
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

    pub fn unmap_user_page(&mut self, vaddr: u64, release: bool) -> Result<(), &'static str> {
        let pos = self.mappings.iter().position(|m| m.vaddr == vaddr).ok_or("Address not mapped")?;
        let mapping = self.mappings[pos];

        let l1 = unsafe { &mut *(self.ttbr0 as *mut PageTable) };
        let l1_idx = ((vaddr >> 30) & 0x1FF) as usize;
        if l1_idx == 0 && l1.entries[0] & DESC_VALID != 0 {
            let l2_phys = l1.entries[0] & 0x0000_FFFF_FFFF_F000;
            let l2 = unsafe { &mut *(l2_phys as *mut PageTable) };
            let l2_idx = ((vaddr >> 21) & 0x1FF) as usize;
            if l2.entries[l2_idx] & DESC_VALID != 0 && (l2.entries[l2_idx] & DESC_TABLE != 0) {
                let l3_phys = l2.entries[l2_idx] & 0x0000_FFFF_FFFF_F000;
                let l3 = unsafe { &mut *(l3_phys as *mut PageTable) };
                let l3_idx = ((vaddr >> 12) & 0x1FF) as usize;
                l3.entries[l3_idx] = 0;
            }
        }

        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("dsb ishst", options(nostack));
            core::arch::asm!("tlbi vae1is, {0}", in(reg) (vaddr >> 12), options(nostack));
            core::arch::asm!("dsb ish", options(nostack));
            core::arch::asm!("isb", options(nostack));
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
        self.mapped_bytes = self.mapped_bytes.saturating_sub(4096);
        Ok(())
    }

    pub fn user_mapped_bytes(&self) -> usize {
        self.mapped_bytes
    }

    pub fn try_activate(&self) -> Result<(), ActivationError> {
        let cid = current_cpu_id() as u32;
        if cid >= 16 {
            return Err(ActivationError::CpuIdOutOfRange(cid));
        }
        let cpu_bit = 1u32 << cid;
        loop {
            let cur = self.lifecycle.load(core::sync::atomic::Ordering::Acquire);
            if (cur & (LIFECYCLE_CLOSING_BIT | LIFECYCLE_DESTROYED_BIT)) != 0 {
                return Err(ActivationError::ClosingOrDestroyed);
            }
            let next = cur | cpu_bit;
            match self.lifecycle.compare_exchange_weak(
                cur,
                next,
                core::sync::atomic::Ordering::AcqRel,
                core::sync::atomic::Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.executed_cpus.fetch_or(cpu_bit, core::sync::atomic::Ordering::Release);
                    self.acknowledged_cpus.fetch_and(!cpu_bit, core::sync::atomic::Ordering::Release);
                    ArmMmu::switch_to_task(self.ttbr0);
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
        let cid = current_cpu_id() as u32;
        if cid >= 16 {
            return;
        }
        let cpu_bit = 1u32 << cid;
        // Hardware register has already switched away from this address space.
        self.acknowledged_cpus.fetch_or(cpu_bit, core::sync::atomic::Ordering::Release);
        loop {
            let cur = self.lifecycle.load(core::sync::atomic::Ordering::Acquire);
            let next = cur & !cpu_bit;
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
        let cid = current_cpu_id() as u32;
        if cid >= 16 {
            return;
        }
        // 1. Switch hardware register away from user table to kernel root table and flush pipeline
        ArmMmu::switch_to_kernel();
        // 2 & 3. Acknowledge and clear CPU from active bitmask
        self.deactivate_after_switch();
    }

    pub fn destroy_fallible(&mut self) -> Result<(), TeardownError> {
        loop {
            let cur = self.lifecycle.load(core::sync::atomic::Ordering::Acquire);
            if (cur & LIFECYCLE_DESTROYED_BIT) != 0 {
                return Ok(()); // Idempotent
            }
            if (cur & LIFECYCLE_CLOSING_BIT) != 0 {
                break; // Already marked closing
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

        let cid = current_cpu_id() as u32;
        let current_mask = if cid < 16 { 1u32 << cid } else { 0 };

        // If currently active on this CPU, switch away to kernel and deactivate
        if (self.lifecycle.load(core::sync::atomic::Ordering::Acquire) & current_mask) != 0 {
            self.deactivate();
        }

        // Bounded loop waiting for remote CPUs to deactivate and acknowledge
        let mut spins = 0;
        loop {
            let cur = self.lifecycle.load(core::sync::atomic::Ordering::Acquire);
            let active_remote = cur & LIFECYCLE_ACTIVE_CPUS_MASK & !current_mask;
            let executed = self.executed_cpus.load(core::sync::atomic::Ordering::Acquire);
            let acknowledged = self.acknowledged_cpus.load(core::sync::atomic::Ordering::Acquire);
            let unacknowledged = (executed & !current_mask) & !acknowledged;

            if active_remote == 0 && unacknowledged == 0 {
                break;
            }
            spins += 1;
            if spins > 100_000 {
                if active_remote != 0 {
                    return Err(TeardownError::ActiveOnRemoteCpu(active_remote));
                } else {
                    return Err(TeardownError::RemoteRendezvousFailed(unacknowledged));
                }
            }
            core::hint::spin_loop();
        }

        // Step 1: Detach root translation entry (only if root table frame has not already been recycled)
        if self.table_frames.contains(&self.ttbr0) {
            let l1 = unsafe { &mut *(self.ttbr0 as *mut PageTable) };
            l1.entries[0] = 0; // Detach L2 table for user 0..1GB

            // Step 2: Inner-shareable broadcast TLB invalidation and memory barriers
            #[cfg(target_arch = "aarch64")]
            unsafe {
                core::arch::asm!(
                    "dsb ishst",
                    "tlbi vmalle1is",
                    "dsb ish",
                    "isb",
                    options(nostack)
                );
            }
        }

        // Step 3: Safely recycle exclusively owned frames, release shared, and keep borrowed
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
                MappingKind::Borrowed => {
                    // Borrowed: never returned to allocator pools
                }
            }
        }
        self.mappings = retained_mappings;
        let mut retained_tables = Vec::new();
        for paddr in self.table_frames.drain(..) {
            if let Err(_) = recycle_frame(paddr) {
                quarantine_frame_with_meta(
                    paddr,
                    frame_ownership(paddr).unwrap_or(FrameOwnership::Kernel),
                    "destroy recycle table frame failed",
                );
                retained_tables.push(paddr);
                cleanup_failed = true;
            }
        }
        self.table_frames = retained_tables;
        self.mapped_bytes = self.mappings.len() * 4096;

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
                crate::kprintln!("[MMU WARNING] UserAddressSpace drop failed: {:?}. Quarantining unreclaimed frames.", e);
                let reason = match e {
                    TeardownError::ActiveOnRemoteCpu(_) => "drop failed: address space active on remote cpu",
                    TeardownError::RemoteRendezvousFailed(_) => "drop failed: remote cpu rendezvous failed",
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
    assert_eq!(aspace_bad.mapped_bytes, 4096);
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

    crate::kprintln!("[TEST] MMU Ownership Reclamation Errors & Quarantine Protection VERIFIED.");
}

