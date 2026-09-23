//! AArch64 Memory Management Unit (MMU) & Virtual Memory Architecture
//!
//! Configures ARMv8-A 4-level translation tables with 4KB page granule:
//! - MAIR_EL1: Memory attribute indirection (Attr 0 = Normal Cacheable, Attr 1 = Device-nGnRE)
//! - TCR_EL1: Translation control register (48-bit VA, 4KB granule, Inner Shareable)
//! - TTBR0_EL1: Translation table base register for lower virtual address space
//! - SCTLR_EL1: Enables MMU (M bit), Data Cache (C bit), and Instruction Cache (I bit)

#[repr(C, align(4096))]
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
pub const DESC_ATTR_NORMAL: u64 = 0 << 2;  // AttrIndx = 0
pub const DESC_ATTR_DEVICE: u64 = 1 << 2;  // AttrIndx = 1
pub const DESC_AP_RW_EL1: u64 = 0 << 6;    // Read/Write at EL1 only
pub const DESC_AP_RW_ALL: u64 = 1 << 6;    // Read/Write at EL1 and EL0
pub const DESC_AP_RW_EL0: u64 = 1 << 6;    // User EL0 accessible
pub const DESC_SH_OUTER: u64 = 2 << 8;     // Outer Shareable
pub const DESC_SH_INNER: u64 = 3 << 8;     // Inner Shareable
pub const DESC_AF: u64 = 1 << 10;          // Access Flag (must be 1 to avoid fault)

// Static primordial page tables
static mut L1_TABLE: PageTable = PageTable::empty();
static mut L2_TABLE: PageTable = PageTable::empty();

pub struct ArmMmu;

impl ArmMmu {
    pub fn init_and_enable() {
        unsafe {
            // 1. Clear tables
            L1_TABLE = PageTable::empty();
            L2_TABLE = PageTable::empty();

            let l2_phys = core::ptr::addr_of!(L2_TABLE) as u64;

            // 2. Level 1 Entry 0: Point to L2 Table for lower 1 GB (0x0000_0000 .. 0x3FFF_FFFF)
            L1_TABLE.entries[0] = l2_phys | DESC_TABLE | DESC_VALID;

            // 3. Level 1 Entry 1: Map 1 GB DRAM (0x4000_0000 .. 0x7FFF_FFFF) as Normal Cacheable Memory
            L1_TABLE.entries[1] = 0x4000_0000
                | DESC_AF
                | DESC_SH_INNER
                | DESC_AP_RW_EL1
                | DESC_ATTR_NORMAL
                | DESC_BLOCK
                | DESC_VALID;

            // 4. Level 2 Table: Populate 2MB blocks for peripheral MMIO & EL0 Userspace
            // Block 2 (0x0040_0000): EL0 Userspace Code Region (2MB)
            L2_TABLE.entries[2] = 0x4400_0000
                | DESC_AF
                | DESC_SH_INNER
                | DESC_AP_RW_ALL
                | DESC_ATTR_NORMAL
                | DESC_BLOCK
                | DESC_VALID;

            // Block 4 (0x0080_0000): EL0 Userspace Stack Region (2MB)
            L2_TABLE.entries[4] = 0x4420_0000
                | DESC_AF
                | DESC_SH_INNER
                | DESC_AP_RW_ALL
                | DESC_ATTR_NORMAL
                | DESC_BLOCK
                | DESC_VALID;

            // Block 64 (0x0800_0000): GICv2 Distributor & CPU Interface (2MB)
            L2_TABLE.entries[64] = 0x0800_0000
                | DESC_AF
                | DESC_SH_OUTER
                | DESC_AP_RW_EL1
                | DESC_ATTR_DEVICE
                | DESC_BLOCK
                | DESC_VALID;

            // Block 72 (0x0900_0000): PrimeCell PL011 UART (2MB)
            L2_TABLE.entries[72] = 0x0900_0000
                | DESC_AF
                | DESC_SH_OUTER
                | DESC_AP_RW_EL1
                | DESC_ATTR_DEVICE
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
                // IPS = 0b001 (36-bit physical addressing, 64 GB)
                let tcr_val: u64 = (25 << 0)          // T0SZ = 25 (39-bit VA, L1 root)
                    | (1 << 8)                        // IRGN0 = 0b01 (Inner WB WA)
                    | (1 << 10)                       // ORGN0 = 0b01 (Outer WB WA)
                    | (3 << 12)                       // SH0 = 0b11 (Inner Shareable)
                    | (0 << 14)                       // TG0 = 0b00 (4KB Granule)
                    | (1u64 << 32);                   // IPS = 0b001 (36-bit PA)
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
        }
    }

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

        // Level 1 Entry 0: points to user L2 table
        user_l1.entries[0] = l2_phys | DESC_TABLE | DESC_VALID;

        // Level 1 Entry 1: Map 1 GB DRAM with DESC_AP_RW_EL1 (Kernel ONLY, EL0 forbidden!)
        user_l1.entries[1] = 0x4000_0000
            | DESC_AF
            | DESC_SH_INNER
            | DESC_AP_RW_EL1
            | DESC_ATTR_NORMAL
            | DESC_BLOCK
            | DESC_VALID;

        // Populate User L2:
        // Block 2 (0x0040_0000): EL0 Userspace Code Region (2MB) with DESC_AP_RW_EL0
        user_l2.entries[2] = 0x4400_0000
            | DESC_AF
            | DESC_SH_INNER
            | DESC_AP_RW_EL0
            | DESC_ATTR_NORMAL
            | DESC_BLOCK
            | DESC_VALID;

        // Block 4 (0x0080_0000): EL0 Userspace Stack Region (2MB) with DESC_AP_RW_EL0
        user_l2.entries[4] = 0x4420_0000
            | DESC_AF
            | DESC_SH_INNER
            | DESC_AP_RW_EL0
            | DESC_ATTR_NORMAL
            | DESC_BLOCK
            | DESC_VALID;

        // Device MMIO (GIC and UART) mapped with DESC_AP_RW_EL1 (Kernel ONLY!)
        user_l2.entries[64] = 0x0800_0000
            | DESC_AF
            | DESC_SH_OUTER
            | DESC_AP_RW_EL1
            | DESC_ATTR_DEVICE
            | DESC_BLOCK
            | DESC_VALID;

        user_l2.entries[72] = 0x0900_0000
            | DESC_AF
            | DESC_SH_OUTER
            | DESC_AP_RW_EL1
            | DESC_ATTR_DEVICE
            | DESC_BLOCK
            | DESC_VALID;

        let root_phys = Box::into_raw(user_l1) as u64;
        let _ = Box::into_raw(user_l2); // prevent deallocation
        root_phys
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

