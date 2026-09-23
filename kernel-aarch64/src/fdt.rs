//! Zero-Allocation AArch64 Flattened Device Tree (FDT / DTB) Parser
//!
//! Conforms to Devicetree Specification v0.4 with zero heap allocation (`no_std`).
//! Parses DTB passed in CPU register `x0` by physical mobile bootloaders or QEMU.

pub const FDT_MAGIC: u32 = 0xD00D_FEED;

pub const FDT_BEGIN_NODE: u32 = 0x0000_0001;
pub const FDT_END_NODE: u32 = 0x0000_0002;
pub const FDT_PROP: u32 = 0x0000_0003;
pub const FDT_NOP: u32 = 0x0000_0004;
pub const FDT_END: u32 = 0x0000_0009;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdtError {
    BufferTooShort,
    InvalidMagic,
    UnsupportedVersion(u32),
    CorruptedStructure,
    NodeNotFound,
    PropertyNotFound,
}

#[derive(Debug, Clone)]
pub struct ArmFdtNode<'a> {
    pub name: &'a str,
    pub struct_offset: usize,
}

pub struct ArmFdtParser<'a> {
    data: &'a [u8],
    header: FdtHeader,
}

impl<'a> ArmFdtParser<'a> {
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

        if (totalsize as usize) > data.len() {
            return Err(FdtError::BufferTooShort);
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

    pub unsafe fn from_raw_ptr(ptr: *const u8) -> Result<ArmFdtParser<'static>, FdtError> {
        if ptr.is_null() {
            return Err(FdtError::InvalidMagic);
        }
        let p_u8 = ptr as *const [u8; 4];
        let magic = u32::from_be_bytes(*p_u8);
        if magic != FDT_MAGIC {
            return Err(FdtError::InvalidMagic);
        }
        let p_size = ptr.add(4) as *const [u8; 4];
        let totalsize = u32::from_be_bytes(*p_size) as usize;
        if totalsize < 40 || totalsize > 32 * 1024 * 1024 {
            return Err(FdtError::BufferTooShort);
        }
        let slice = core::slice::from_raw_parts(ptr, totalsize);
        ArmFdtParser::new(slice)
    }

    pub fn total_size(&self) -> usize {
        self.header.totalsize as usize
    }

    pub fn get_string(&self, offset: usize) -> Option<&'a str> {
        let base = self.header.off_dt_strings as usize;
        let start = base.checked_add(offset)?;
        if start >= self.data.len() {
            return None;
        }

        let mut end = start;
        while end < self.data.len() && self.data[end] != 0 {
            end += 1;
        }

        core::str::from_utf8(&self.data[start..end]).ok()
    }

    pub fn find_node_by_name(&self, target_name: &str) -> Result<ArmFdtNode<'a>, FdtError> {
        let mut offset = self.header.off_dt_struct as usize;
        let limit = offset + self.header.size_dt_struct as usize;

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
                    offset = (offset + 4) & !3;

                    if name == target_name || name.starts_with(target_name) {
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
                    offset += 8;
                    offset = (offset + len + 3) & !3;
                }
                FDT_END_NODE | FDT_NOP => {}
                FDT_END => break,
                _ => return Err(FdtError::CorruptedStructure),
            }
        }

        Err(FdtError::NodeNotFound)
    }

    pub fn parse_memory_range(&self) -> Result<(u64, u64), FdtError> {
        let mem_node = self.find_node_by_name("memory")?;
        let reg_prop = self.get_node_property(&mem_node, "reg")?;

        if reg_prop.len() >= 16 {
            let base = u64::from_be_bytes([
                reg_prop[0], reg_prop[1], reg_prop[2], reg_prop[3],
                reg_prop[4], reg_prop[5], reg_prop[6], reg_prop[7],
            ]);
            let size = u64::from_be_bytes([
                reg_prop[8], reg_prop[9], reg_prop[10], reg_prop[11],
                reg_prop[12], reg_prop[13], reg_prop[14], reg_prop[15],
            ]);
            Ok((base, size))
        } else if reg_prop.len() >= 8 {
            let base = u32::from_be_bytes([reg_prop[0], reg_prop[1], reg_prop[2], reg_prop[3]]) as u64;
            let size = u32::from_be_bytes([reg_prop[4], reg_prop[5], reg_prop[6], reg_prop[7]]) as u64;
            Ok((base, size))
        } else {
            Err(FdtError::PropertyNotFound)
        }
    }

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

    pub fn get_node_property(&self, node: &ArmFdtNode<'a>, prop_name: &str) -> Result<&'a [u8], FdtError> {
        let mut offset = node.struct_offset;
        let limit = self.header.off_dt_struct as usize + self.header.size_dt_struct as usize;

        while offset < limit && self.data[offset] != 0 {
            offset += 1;
        }
        offset = (offset + 4) & !3;

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
                    offset = (offset + 4) & !3;
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
                    let val_slice = &self.data[offset..offset + val_len];
                    offset = (offset + val_len + 3) & !3;

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
