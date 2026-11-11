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
}

pub struct ArmFrameAllocator {
    pub base_paddr: u64,
    pub total_frames: usize,
    pub allocated_frames: usize,
    pub recycled_frames: usize,
    pub bitmap: [u64; 1024], // 65,536 frames = 256 MiB
}

impl ArmFrameAllocator {
    pub const fn empty() -> Self {
        Self {
            base_paddr: 0,
            total_frames: 0,
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

pub static FRAME_ALLOCATOR: spin::Mutex<ArmFrameAllocator> = spin::Mutex::new(ArmFrameAllocator::empty());

pub fn init_frame_allocator(base_paddr: u64, total_frames: usize) {
    let mut alloc = FRAME_ALLOCATOR.lock();
    alloc.base_paddr = base_paddr;
    alloc.total_frames = total_frames.min(65536);
    alloc.allocated_frames = 0;
    alloc.recycled_frames = 0;
    alloc.bitmap = [0; 1024];
}

pub fn allocate_frame() -> Option<u64> {
    FRAME_ALLOCATOR.lock().allocate_frame()
}

pub fn recycle_frame(paddr: u64) {
    FRAME_ALLOCATOR.lock().recycle_frame(paddr);
}

pub fn total_recycled_frames() -> usize {
    FRAME_ALLOCATOR.lock().recycled_frames
}

extern crate alloc;
use alloc::vec::Vec;

pub struct UserAddressSpace {
    pub ttbr0: u64,
    pub table_frames: Vec<u64>,
    pub mapped_frames: Vec<u64>,
    pub mapped_bytes: usize,
}

impl UserAddressSpace {
    pub fn new() -> Option<Self> {
        let root_l1_phys = allocate_frame()?;
        let l2_phys = allocate_frame()?;

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

        let mut table_frames = Vec::new();
        table_frames.push(root_l1_phys);
        table_frames.push(l2_phys);

        Some(Self {
            ttbr0: root_l1_phys,
            table_frames,
            mapped_frames: Vec::new(),
            mapped_bytes: 0,
        })
    }

    pub fn map_user_page(&mut self, vaddr: u64, paddr: u64, is_write: bool, is_exec: bool) -> Result<(), &'static str> {
        if is_write && is_exec {
            return Err("W^X violation: writable user mapping must be non-executable");
        }
        if vaddr >= 0x0000_8000_0000_0000 || vaddr < 0x1000 {
            return Err("Virtual address outside canonical user range");
        }

        let l1 = unsafe { &mut *(self.ttbr0 as *mut PageTable) };
        let l1_entry = l1.entries[0];
        if l1_entry & DESC_VALID == 0 {
            return Err("L1 root table corrupt");
        }
        let l2_phys = l1_entry & 0x0000_FFFF_FFFF_F000;
        let l2 = unsafe { &mut *(l2_phys as *mut PageTable) };

        let l2_idx = ((vaddr >> 21) & 0x1FF) as usize;
        let l3_phys = if l2.entries[l2_idx] & DESC_VALID == 0 {
            let frame = allocate_frame().ok_or("OOM allocating L3 page table")?;
            l2.entries[l2_idx] = frame | DESC_TABLE | DESC_VALID;
            self.table_frames.push(frame);
            frame
        } else {
            l2.entries[l2_idx] & 0x0000_FFFF_FFFF_F000
        };

        let l3 = unsafe { &mut *(l3_phys as *mut PageTable) };
        let l3_idx = ((vaddr >> 12) & 0x1FF) as usize;
        if l3.entries[l3_idx] & DESC_VALID != 0 {
            return Err("physical frame already owned by address space");
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
        self.mapped_frames.push(paddr);
        self.mapped_bytes += 4096;
        Ok(())
    }

    pub fn user_mapped_bytes(&self) -> usize {
        self.mapped_bytes
    }

    pub fn destroy(&mut self) {
        for paddr in self.mapped_frames.drain(..) {
            recycle_frame(paddr);
        }
        for paddr in self.table_frames.drain(..) {
            recycle_frame(paddr);
        }
        self.mapped_bytes = 0;
    }

    pub fn activate(&self) {
        ArmMmu::switch_to_task(self.ttbr0);
    }
}

