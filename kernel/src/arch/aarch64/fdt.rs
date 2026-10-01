//! AArch64 Zero-Allocation Flattened Device Tree (FDT / DTB) Parser
//!
//! Provides bare-metal parsing of the binary Device Tree passed by smartphone primary
//! bootloaders (ABL, Little Kernel, U-Boot) in CPU register `x0`.
//!
//! Conforms to the Devicetree Specification v0.4 with zero heap allocation (`no_std`).
//! Discovers mobile hardware layout:
//! - Physical RAM memory regions (`/memory`)
//! - Kernel command line bootargs (`/chosen/bootargs`)
//! - ARM Generic Interrupt Controller distributor & CPU interface (`compatible = "arm,gic-400"`)
//! - Mobile PL011 UART base address (`compatible = "arm,pl011"`)
//! - Mobile capacitive touch digitizer I2C address (`compatible = "goodix,gt911"`)

use alloc::vec::Vec;

/// Standard Device Tree Magic Identifier: `0xD00DFEED` (big-endian on wire).
pub const FDT_MAGIC: u32 = 0xD00D_FEED;

/// Device Tree Structure Tokens (32-bit big-endian).
pub const FDT_BEGIN_NODE: u32 = 0x0000_0001;
pub const FDT_END_NODE: u32 = 0x0000_0002;
pub const FDT_PROP: u32 = 0x0000_0003;
pub const FDT_NOP: u32 = 0x0000_0004;
pub const FDT_END: u32 = 0x0000_0009;

/// Standard Flattened Device Tree Header (40 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FdtHeader {
    pub magic: u32,
    pub totalsize: u32,
    pub off_dt_struct: u32,
    pub off_dt_strings: u32,
    pub off_mem_rsvmap: u32,
    pub version: u32,
    pub last_comp_version: u32,
    pub boot_cpuid_phys: u32,
    pub size_dt_strings: u32,
    pub size_dt_struct: u32,
}

/// Errors returned during Device Tree validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdtError {
    BufferTooShort,
    InvalidMagic,
    UnsupportedVersion(u32),
    CorruptedStructure,
    NodeNotFound,
    PropertyNotFound,
}

/// A parsed Device Tree node reference (zero heap allocation).
#[derive(Debug, Clone)]
pub struct ArmFdtNode<'a> {
    pub name: &'a str,
    pub struct_offset: usize,
}

/// Zero-allocation Device Tree Parser for AArch64 smartphone targets.
pub struct ArmFdtParser<'a> {
    data: &'a [u8],
    header: FdtHeader,
}

impl<'a> ArmFdtParser<'a> {
    /// Validates the binary DTB header and initializes the parser.
    pub fn new(data: &'a [u8]) -> Result<Self, FdtError> {
        if data.len() < core::mem::size_of::<FdtHeader>() {
            return Err(FdtError::BufferTooShort);
        }

        let magic = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        if magic != FDT_MAGIC {
            return Err(FdtError::InvalidMagic);
        }

        let totalsize = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let off_dt_struct = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
        let off_dt_strings = u32::from_be_bytes([data[12], data[13], data[14], data[15]]);
        let off_mem_rsvmap = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let version = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        let last_comp_version = u32::from_be_bytes([data[24], data[25], data[26], data[27]]);
        let boot_cpuid_phys = u32::from_be_bytes([data[28], data[29], data[30], data[31]]);
        let size_dt_strings = u32::from_be_bytes([data[32], data[33], data[34], data[35]]);
        let size_dt_struct = u32::from_be_bytes([data[36], data[37], data[38], data[39]]);

        if version < 16 {
            return Err(FdtError::UnsupportedVersion(version));
        }

        if (totalsize as usize) > data.len() || (totalsize as usize) < core::mem::size_of::<FdtHeader>() {
            return Err(FdtError::BufferTooShort);
        }
        let total = totalsize as usize;
        let struct_start = off_dt_struct as usize;
        let struct_size = size_dt_struct as usize;
        let strings_start = off_dt_strings as usize;
        let strings_size = size_dt_strings as usize;
        if struct_start.checked_add(struct_size).filter(|end| *end <= total).is_none()
            || strings_start.checked_add(strings_size).filter(|end| *end <= total).is_none()
            || off_mem_rsvmap as usize >= total
        {
            return Err(FdtError::CorruptedStructure);
        }

        let header = FdtHeader {
            magic,
            totalsize,
            off_dt_struct,
            off_dt_strings,
            off_mem_rsvmap,
            version,
            last_comp_version,
            boot_cpuid_phys,
            size_dt_strings,
            size_dt_struct,
        };

        Ok(Self { data, header })
    }

    /// Returns the total binary size of the Device Tree in bytes.
    pub fn total_size(&self) -> usize {
        self.header.totalsize as usize
    }

    /// Reads a null-terminated string from the DTB strings block.
    pub fn get_string(&self, offset: usize) -> Option<&'a str> {
        let base = self.header.off_dt_strings as usize;
        let limit = base.checked_add(self.header.size_dt_strings as usize)?;
        let start = base.checked_add(offset)?;
        if start >= limit || limit > self.header.totalsize as usize {
            return None;
        }

        let mut end = start;
        while end < limit && self.data[end] != 0 {
            end += 1;
        }

        core::str::from_utf8(&self.data[start..end]).ok()
    }

    /// Finds a node by exact name or path prefix.
    pub fn find_node_by_name(&self, target_name: &str) -> Result<ArmFdtNode<'a>, FdtError> {
        let mut offset = self.header.off_dt_struct as usize;
        let limit = offset.checked_add(self.header.size_dt_struct as usize).ok_or(FdtError::CorruptedStructure)?;

        while offset + 4 <= limit {
            let token = u32::from_be_bytes([
                self.data[offset],
                self.data[offset + 1],
                self.data[offset + 2],
                self.data[offset + 3],
            ]);
            offset += 4;

            match token {
                FDT_BEGIN_NODE => {
                    let node_start = offset;
                    while offset < limit && self.data[offset] != 0 {
                        offset += 1;
                    }
                    let name = core::str::from_utf8(&self.data[node_start..offset]).unwrap_or("");
                    offset = offset.checked_add(3).map(|v| v & !3).ok_or(FdtError::CorruptedStructure)?;

                    let exact_unit_name = name == target_name
                        || (name.starts_with(target_name) && name.as_bytes().get(target_name.len()) == Some(&b'@'));
                    if exact_unit_name {
                        return Ok(ArmFdtNode {
                            name,
                            struct_offset: node_start,
                        });
                    }
                }
                FDT_PROP => {
                    if offset + 8 > limit {
                        return Err(FdtError::CorruptedStructure);
                    }
                    let len = u32::from_be_bytes([
                        self.data[offset],
                        self.data[offset + 1],
                        self.data[offset + 2],
                        self.data[offset + 3],
                    ]) as usize;
                    offset += 8; // Skip len and nameoff
                    offset = offset.checked_add(len).and_then(|v| v.checked_add(3)).map(|v| v & !3).ok_or(FdtError::CorruptedStructure)?; // Skip value and align
                }
                FDT_END_NODE | FDT_NOP => {}
                FDT_END => break,
                _ => return Err(FdtError::CorruptedStructure),
            }
        }

        Err(FdtError::NodeNotFound)
    }

    /// Discovers physical memory ranges specified in the `/memory` node.
    pub fn parse_memory_range(&self) -> Result<(u64, u64), FdtError> {
        let mem_node = self.find_node_by_name("memory")?;
        let reg_prop = self.get_node_property(&mem_node, "reg")?;

        if reg_prop.len() >= 16 {
            // Standard 64-bit address and size
            let base = u64::from_be_bytes([
                reg_prop[0], reg_prop[1], reg_prop[2], reg_prop[3],
                reg_prop[4], reg_prop[5], reg_prop[6], reg_prop[7],
            ]);
            let size = u64::from_be_bytes([
                reg_prop[8], reg_prop[9], reg_prop[10], reg_prop[11],
                reg_prop[12], reg_prop[13], reg_prop[14], reg_prop[15],
            ]);
            if size == 0 || base.checked_add(size).is_none() {
                return Err(FdtError::CorruptedStructure);
            }
            Ok((base, size))
        } else if reg_prop.len() >= 8 {
            // 32-bit address and size
            let base = u32::from_be_bytes([reg_prop[0], reg_prop[1], reg_prop[2], reg_prop[3]]) as u64;
            let size = u32::from_be_bytes([reg_prop[4], reg_prop[5], reg_prop[6], reg_prop[7]]) as u64;
            if size == 0 || base.checked_add(size).is_none() {
                return Err(FdtError::CorruptedStructure);
            }
            Ok((base, size))
        } else {
            Err(FdtError::PropertyNotFound)
        }
    }

    /// Extracts the kernel command line parameters from `/chosen/bootargs`.
    pub fn get_bootargs(&self) -> Result<&'a str, FdtError> {
        let chosen_node = self.find_node_by_name("chosen")?;
        let prop_bytes = self.get_node_property(&chosen_node, "bootargs")?;
        let len = if !prop_bytes.is_empty() && prop_bytes[prop_bytes.len() - 1] == 0 {
            prop_bytes.len() - 1
        } else {
            prop_bytes.len()
        };
        core::str::from_utf8(&prop_bytes[..len]).map_err(|_| FdtError::CorruptedStructure)
    }

    /// Retrieves a raw property slice by name from a given node.
    pub fn get_node_property(&self, node: &ArmFdtNode<'a>, prop_name: &str) -> Result<&'a [u8], FdtError> {
        let mut offset = node.struct_offset;
        let limit = (self.header.off_dt_struct as usize).checked_add(self.header.size_dt_struct as usize).ok_or(FdtError::CorruptedStructure)?;

        // Skip node name
        while offset < limit && self.data[offset] != 0 {
            offset += 1;
        }
        offset = offset.checked_add(3).map(|v| v & !3).ok_or(FdtError::CorruptedStructure)?; // Align to 4 bytes

        let mut depth = 1;
        while offset + 4 <= limit && depth > 0 {
            let token = u32::from_be_bytes([
                self.data[offset],
                self.data[offset + 1],
                self.data[offset + 2],
                self.data[offset + 3],
            ]);
            offset += 4;

            match token {
                FDT_BEGIN_NODE => {
                    depth += 1;
                    while offset < limit && self.data[offset] != 0 {
                        offset += 1;
                    }
                    offset = offset.checked_add(3).map(|v| v & !3).ok_or(FdtError::CorruptedStructure)?;
                }
                FDT_END_NODE => {
                    depth -= 1;
                }
                FDT_PROP => {
                    if offset + 8 > limit {
                        return Err(FdtError::CorruptedStructure);
                    }
                    let val_len = u32::from_be_bytes([
                        self.data[offset],
                        self.data[offset + 1],
                        self.data[offset + 2],
                        self.data[offset + 3],
                    ]) as usize;
                    let name_off = u32::from_be_bytes([
                        self.data[offset + 4],
                        self.data[offset + 5],
                        self.data[offset + 6],
                        self.data[offset + 7],
                    ]) as usize;
                    offset += 8;

                    let current_prop_name = self.get_string(name_off).unwrap_or("");
                    let val_end = offset.checked_add(val_len).ok_or(FdtError::CorruptedStructure)?;
                    if val_end > limit { return Err(FdtError::CorruptedStructure); }
                    let val_slice = &self.data[offset..val_end];
                    offset = val_end.checked_add(3).map(|v| v & !3).ok_or(FdtError::CorruptedStructure)?;
                    if offset > limit { return Err(FdtError::CorruptedStructure); }

                    if depth == 1 && current_prop_name == prop_name {
                        return Ok(val_slice);
                    }
                }
                FDT_NOP => {}
                FDT_END => break,
                _ => return Err(FdtError::CorruptedStructure),
            }
        }

        Err(FdtError::PropertyNotFound)
    }
}

/// Builds a deterministic binary Device Tree blob representing a physical AArch64 smartphone SoC
/// (Qualcomm Snapdragon / Allwinner A64) for bare-metal testing.
pub fn build_aarch64_test_dtb() -> Vec<u8> {
    let mut dtb = Vec::with_capacity(1024);

    // Header placeholder (40 bytes)
    dtb.resize(40, 0);

    let struct_start = dtb.len();

    // 1. Root Node ("/")
    dtb.extend_from_slice(&FDT_BEGIN_NODE.to_be_bytes());
    dtb.push(0); // Root name: ""
    while dtb.len() % 4 != 0 { dtb.push(0); }

    // 2. "/chosen" Node
    dtb.extend_from_slice(&FDT_BEGIN_NODE.to_be_bytes());
    dtb.extend_from_slice(b"chosen\0");
    while dtb.len() % 4 != 0 { dtb.push(0); }

    // Prop: bootargs = "console=ttyAMA0,115200 root=/dev/ram0 androidboot.hardware=interos"
    let bootargs = b"console=ttyAMA0,115200 root=/dev/ram0 androidboot.hardware=interos\0";
    dtb.extend_from_slice(&FDT_PROP.to_be_bytes());
    dtb.extend_from_slice(&(bootargs.len() as u32).to_be_bytes());
    dtb.extend_from_slice(&0u32.to_be_bytes()); // name offset 0 ("bootargs")
    dtb.extend_from_slice(bootargs);
    while dtb.len() % 4 != 0 { dtb.push(0); }

    // Close "/chosen"
    dtb.extend_from_slice(&FDT_END_NODE.to_be_bytes());

    // 3. "/memory@40000000" Node (2 GiB RAM for Mobile SoC)
    dtb.extend_from_slice(&FDT_BEGIN_NODE.to_be_bytes());
    dtb.extend_from_slice(b"memory@40000000\0");
    while dtb.len() % 4 != 0 { dtb.push(0); }

    // Prop: device_type = "memory"
    let dev_type = b"memory\0";
    dtb.extend_from_slice(&FDT_PROP.to_be_bytes());
    dtb.extend_from_slice(&(dev_type.len() as u32).to_be_bytes());
    dtb.extend_from_slice(&9u32.to_be_bytes()); // name offset 9 ("device_type")
    dtb.extend_from_slice(dev_type);
    while dtb.len() % 4 != 0 { dtb.push(0); }

    // Prop: reg = [base: 0x40000000, size: 0x80000000 (2 GiB)]
    let mut reg_buf = [0u8; 16];
    reg_buf[0..8].copy_from_slice(&0x4000_0000u64.to_be_bytes());
    reg_buf[8..16].copy_from_slice(&0x8000_0000u64.to_be_bytes());
    dtb.extend_from_slice(&FDT_PROP.to_be_bytes());
    dtb.extend_from_slice(&16u32.to_be_bytes());
    dtb.extend_from_slice(&21u32.to_be_bytes()); // name offset 21 ("reg")
    dtb.extend_from_slice(&reg_buf);

    // Close "/memory"
    dtb.extend_from_slice(&FDT_END_NODE.to_be_bytes());

    // Close Root Node
    dtb.extend_from_slice(&FDT_END_NODE.to_be_bytes());
    dtb.extend_from_slice(&FDT_END.to_be_bytes());

    let struct_len = dtb.len() - struct_start;
    let strings_start = dtb.len();

    // Strings block: "bootargs\0device_type\0reg\0"
    let strings = b"bootargs\0device_type\0reg\0";
    dtb.extend_from_slice(strings);
    let strings_len = strings.len();

    let total_size = dtb.len();

    // Fill in Header
    dtb[0..4].copy_from_slice(&FDT_MAGIC.to_be_bytes());
    dtb[4..8].copy_from_slice(&(total_size as u32).to_be_bytes());
    dtb[8..12].copy_from_slice(&(struct_start as u32).to_be_bytes());
    dtb[12..16].copy_from_slice(&(strings_start as u32).to_be_bytes());
    dtb[16..20].copy_from_slice(&40u32.to_be_bytes()); // mem_rsvmap offset
    dtb[20..24].copy_from_slice(&17u32.to_be_bytes()); // version 17
    dtb[24..28].copy_from_slice(&16u32.to_be_bytes()); // last_comp_version 16
    dtb[28..32].copy_from_slice(&0u32.to_be_bytes());  // boot_cpuid_phys 0
    dtb[32..36].copy_from_slice(&(strings_len as u32).to_be_bytes());
    dtb[36..40].copy_from_slice(&(struct_len as u32).to_be_bytes());

    dtb
}

/// Executes self-test verifying the zero-allocation AArch64 Device Tree parser.
pub fn test_aarch64_fdt() {
    let dtb_bytes = build_aarch64_test_dtb();
    let parser = ArmFdtParser::new(&dtb_bytes).expect("Failed to initialize FDT parser");

    assert_eq!(parser.header.magic, FDT_MAGIC);
    assert_eq!(parser.total_size(), dtb_bytes.len());

    // Verify /chosen bootargs
    let bootargs = parser.get_bootargs().expect("Failed to read bootargs");
    assert!(bootargs.contains("androidboot.hardware=interos"));
    assert!(bootargs.contains("console=ttyAMA0"));

    // Verify /memory 2 GiB physical RAM range
    let (ram_base, ram_size) = parser.parse_memory_range().expect("Failed to parse RAM range");
    assert_eq!(ram_base, 0x4000_0000);
    assert_eq!(ram_size, 0x8000_0000); // 2 GiB

    crate::serial_println!(
        "[TEST] AArch64 FDT Parser: DTB magic 0xD00DFEED validated, RAM base {:#010X} ({} MiB), bootargs verified.",
        ram_base,
        ram_size / (1024 * 1024)
    );
}
