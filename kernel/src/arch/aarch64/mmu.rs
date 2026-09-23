//! ARMv8-A MMU Page Table Architecture & Virtual Memory (AArch64)
//!
//! Implements 48-bit Virtual Addressing with 4KB translation granules,
//! Level 0-3 Translation Tables, MAIR_EL1 attribute encodings, and TCR_EL1.

pub const PAGE_SIZE: usize = 4096;
pub const ENTRIES_PER_TABLE: usize = 512;

/// Page Table Descriptor Attribute Bits
pub const PTE_VALID: u64 = 1 << 0;
pub const PTE_TABLE: u64 = 1 << 1;
pub const PTE_USER: u64 = 1 << 6;        // AP[1] = 1 (Access from EL0 allowed)
pub const PTE_RO: u64 = 1 << 7;          // AP[2] = 1 (Read-only)
pub const PTE_SH_INNER: u64 = 3 << 8;    // SH[1:0] = 3 (Inner Shareable)
pub const PTE_AF: u64 = 1 << 10;         // Access Flag = 1
pub const PTE_PXN: u64 = 1 << 53;        // Privileged Execute-Never
pub const PTE_UXN: u64 = 1 << 54;        // Unprivileged Execute-Never

/// MAIR_EL1 Memory Attribute Indices
pub const MAIR_ATTR_DEVICE_NGNRE: u64 = 0; // Index 0: 0x04 (Device-nGnRE)
pub const MAIR_ATTR_NORMAL_WB: u64 = 1;    // Index 1: 0xFF (Normal Inner/Outer Write-Back)

/// TCR_EL1 Configuration Constants (48-bit VA, 4KB granule)
/// T0SZ = 16 (48-bit VA for TTBR0), T1SZ = 16 (48-bit VA for TTBR1),
/// TG0 = 0 (4KB for TTBR0), TG1 = 2 (4KB for TTBR1),
/// IPS = 5 (48-bit PA), SH0 = 3 (Inner Shareable), ORGN0/IRGN0 = 1 (Normal WB).
pub const TCR_CONFIG: u64 = (16 << 0)      // T0SZ = 16
    | (1 << 8)                             // IRGN0 = Normal WB
    | (1 << 10)                            // ORGN0 = Normal WB
    | (3 << 12)                            // SH0 = Inner Shareable
    | (0 << 14)                            // TG0 = 4KB
    | (16 << 16)                           // T1SZ = 16
    | (1 << 24)                            // IRGN1 = Normal WB
    | (1 << 26)                            // ORGN1 = Normal WB
    | (3 << 28)                            // SH1 = Inner Shareable
    | (2 << 30)                            // TG1 = 4KB
    | (5 << 32);                           // IPS = 48-bit physical address

/// MAIR_EL1 Register Value:
/// Index 0: 0x04 (Device nGnRE)
/// Index 1: 0xFF (Normal Cacheable Write-Back)
pub const MAIR_CONFIG: u64 = 0x000000000000FF04;

/// 4KB Page Table Entry (64-bit descriptor)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct PageTableEntry(pub u64);

impl PageTableEntry {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn is_valid(&self) -> bool {
        (self.0 & PTE_VALID) != 0
    }

    pub fn is_table(&self) -> bool {
        (self.0 & PTE_TABLE) != 0
    }

    pub fn physical_address(&self) -> u64 {
        self.0 & 0x0000_FFFF_FFFF_F000
    }

    pub fn set_table(&mut self, table_phys_addr: u64) {
        self.0 = (table_phys_addr & 0x0000_FFFF_FFFF_F000) | PTE_VALID | PTE_TABLE;
    }

    pub fn set_page(&mut self, phys_addr: u64, is_device: bool, is_user: bool, is_readonly: bool) {
        let attr_idx = if is_device { MAIR_ATTR_DEVICE_NGNRE } else { MAIR_ATTR_NORMAL_WB };
        let mut flags = PTE_VALID | PTE_TABLE | PTE_AF | PTE_SH_INNER | (attr_idx << 2);

        if is_user {
            flags |= PTE_USER;
        } else {
            flags |= PTE_PXN;
        }

        if is_readonly {
            flags |= PTE_RO;
        }

        if is_device {
            flags |= PTE_UXN | PTE_PXN; // Never execute from device MMIO
        }

        self.0 = (phys_addr & 0x0000_FFFF_FFFF_F000) | flags;
    }
}

/// 4KB Translation Table containing 512 entries
#[derive(Clone, Copy, Debug)]
#[repr(C, align(4096))]
pub struct TranslationTable {
    pub entries: [PageTableEntry; ENTRIES_PER_TABLE],
}

impl TranslationTable {
    pub const fn new() -> Self {
        Self {
            entries: [PageTableEntry::empty(); ENTRIES_PER_TABLE],
        }
    }

    pub fn clear(&mut self) {
        for entry in self.entries.iter_mut() {
            *entry = PageTableEntry::empty();
        }
    }
}
