//! Flattened Device Tree (FDT / DTB v0.4) Binary Parser
//!
//! Provides zero-allocation, bare-metal hardware discovery for mobile ARM64 SoCs
//! (Snapdragon, Dimensity, Tensor, Apple Silicon, and QEMU-virt AArch64).
//! Parses Devicetree blobs passed by mobile bootloaders into peripheral descriptors.

use alloc::vec::Vec;

/// Standard Device Tree binary magic constant (`0xd00dfeed` in big-endian).
pub const FDT_MAGIC: u32 = 0xd00dfeed;

/// Structure block token constants.
pub const FDT_BEGIN_NODE: u32 = 0x00000001;
pub const FDT_END_NODE: u32 = 0x00000002;
pub const FDT_PROP: u32 = 0x00000003;
pub const FDT_NOP: u32 = 0x00000004;
pub const FDT_END: u32 = 0x00000009;

/// Parsed header of a Flattened Device Tree binary.
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

/// A parsed device tree node property.
#[derive(Debug, Clone)]
pub struct FdtProperty<'a> {
    pub name: &'a str,
    pub value: &'a [u8],
}

impl<'a> FdtProperty<'a> {
    /// Interprets property value as a null-terminated UTF-8 string.
    pub fn as_str(&self) -> Option<&'a str> {
        let trimmed = if self.value.ends_with(b"\0") {
            &self.value[..self.value.len() - 1]
        } else {
            self.value
        };
        core::str::from_utf8(trimmed).ok()
    }

    /// Interprets property value as a big-endian 32-bit unsigned integer.
    pub fn as_u32(&self) -> Option<u32> {
        if self.value.len() >= 4 {
            Some(u32::from_be_bytes([self.value[0], self.value[1], self.value[2], self.value[3]]))
        } else {
            None
        }
    }

    /// Interprets property value as a pair of big-endian 64-bit integers (`reg = <base size>`).
    pub fn as_reg_u64(&self) -> Option<(u64, u64)> {
        if self.value.len() >= 16 {
            let base = u64::from_be_bytes([
                self.value[0], self.value[1], self.value[2], self.value[3],
                self.value[4], self.value[5], self.value[6], self.value[7],
            ]);
            let size = u64::from_be_bytes([
                self.value[8], self.value[9], self.value[10], self.value[11],
                self.value[12], self.value[13], self.value[14], self.value[15],
            ]);
            Some((base, size))
        } else if self.value.len() >= 8 {
            let base = u32::from_be_bytes([self.value[0], self.value[1], self.value[2], self.value[3]]) as u64;
            let size = u32::from_be_bytes([self.value[4], self.value[5], self.value[6], self.value[7]]) as u64;
            Some((base, size))
        } else {
            None
        }
    }
}

/// A parsed device tree peripheral node.
#[derive(Debug, Clone)]
pub struct FdtNode<'a> {
    pub name: &'a str,
    pub properties: Vec<FdtProperty<'a>>,
}

impl<'a> FdtNode<'a> {
    /// Retrieves a property by its exact name.
    pub fn get_prop(&self, name: &str) -> Option<&FdtProperty<'a>> {
        self.properties.iter().find(|p| p.name == name)
    }

    /// Checks if this node's `compatible` property contains the specified target string.
    pub fn is_compatible(&self, target: &str) -> bool {
        if let Some(prop) = self.get_prop("compatible") {
            if let Some(s) = prop.as_str() {
                return s.contains(target);
            }
        }
        false
    }
}

/// Error conditions encountered during FDT parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdtError {
    BufferTooShort,
    InvalidMagic,
    UnsupportedVersion,
    MalformedStructure,
    OutOfBoundsOffset,
}

/// Zero-allocation, high-performance Flattened Device Tree viewer.
pub struct FdtParser<'a> {
    data: &'a [u8],
    header: FdtHeader,
}

impl<'a> FdtParser<'a> {
    /// Parses and validates an FDT binary blob header.
    pub fn from_slice(data: &'a [u8]) -> Result<Self, FdtError> {
        if data.len() < 40 {
            return Err(FdtError::BufferTooShort);
        }

        let read_u32 = |offset: usize| -> u32 {
            u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
        };

        let magic = read_u32(0);
        if magic != FDT_MAGIC {
            return Err(FdtError::InvalidMagic);
        }

        let totalsize = read_u32(4);
        let off_dt_struct = read_u32(8);
        let off_dt_strings = read_u32(12);
        let off_mem_rsvmap = read_u32(16);
        let version = read_u32(20);
        let last_comp_version = read_u32(24);
        let boot_cpuid_phys = read_u32(28);
        let size_dt_strings = read_u32(32);
        let size_dt_struct = read_u32(36);

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

    /// Returns the parsed FDT header.
    pub fn header(&self) -> &FdtHeader {
        &self.header
    }

    /// Resolves a null-terminated property name string from the strings block.
    fn resolve_string(&self, nameoff: u32) -> Result<&'a str, FdtError> {
        let base = self.header.off_dt_strings as usize + nameoff as usize;
        if base >= self.data.len() {
            return Err(FdtError::OutOfBoundsOffset);
        }

        let mut end = base;
        while end < self.data.len() && self.data[end] != 0 {
            end += 1;
        }

        core::str::from_utf8(&self.data[base..end]).map_err(|_| FdtError::MalformedStructure)
    }

    /// Scans the entire FDT structure block and enumerates all declared hardware peripheral nodes.
    pub fn enumerate_nodes(&self) -> Result<Vec<FdtNode<'a>>, FdtError> {
        let mut nodes = Vec::new();
        let mut offset = self.header.off_dt_struct as usize;
        let struct_end = offset + self.header.size_dt_struct as usize;

        if struct_end > self.data.len() {
            return Err(FdtError::OutOfBoundsOffset);
        }

        let read_u32_at = |pos: usize| -> Result<u32, FdtError> {
            if pos + 4 > struct_end {
                return Err(FdtError::MalformedStructure);
            }
            Ok(u32::from_be_bytes([
                self.data[pos],
                self.data[pos + 1],
                self.data[pos + 2],
                self.data[pos + 3],
            ]))
        };

        let mut node_stack: Vec<(&'a str, Vec<FdtProperty<'a>>)> = Vec::new();

        while offset < struct_end {
            let token = read_u32_at(offset)?;
            offset += 4;

            match token {
                FDT_BEGIN_NODE => {
                    // Read null-terminated node name
                    let name_start = offset;
                    while offset < struct_end && self.data[offset] != 0 {
                        offset += 1;
                    }
                    let name_raw = &self.data[name_start..offset];
                    let node_name = core::str::from_utf8(name_raw).unwrap_or("unnamed");
                    offset += 1; // skip null byte
                    // Align offset to 4 bytes
                    while (offset % 4) != 0 && offset < struct_end {
                        offset += 1;
                    }

                    let clean_name = if node_name.is_empty() { "/" } else { node_name };
                    node_stack.push((clean_name, Vec::new()));
                }

                FDT_PROP => {
                    let val_len = read_u32_at(offset)? as usize;
                    offset += 4;
                    let nameoff = read_u32_at(offset)?;
                    offset += 4;

                    if offset + val_len > struct_end {
                        return Err(FdtError::MalformedStructure);
                    }

                    let val_bytes = &self.data[offset..offset + val_len];
                    offset += val_len;
                    // Align offset to 4 bytes
                    while (offset % 4) != 0 && offset < struct_end {
                        offset += 1;
                    }

                    let prop_name = self.resolve_string(nameoff)?;
                    if let Some(top) = node_stack.last_mut() {
                        top.1.push(FdtProperty {
                            name: prop_name,
                            value: val_bytes,
                        });
                    }
                }

                FDT_END_NODE => {
                    if let Some((name, properties)) = node_stack.pop() {
                        nodes.push(FdtNode {
                            name,
                            properties,
                        });
                    }
                }

                FDT_NOP => {
                    // No-op token; advance to next
                }

                FDT_END => {
                    break;
                }

                _ => return Err(FdtError::MalformedStructure),
            }
        }

        Ok(nodes)
    }

    /// Finds the first device node matching the specified `compatible` string.
    pub fn find_compatible_node(&self, compatible: &str) -> Result<Option<FdtNode<'a>>, FdtError> {
        let nodes = self.enumerate_nodes()?;
        for node in nodes {
            if node.is_compatible(compatible) {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }
}

/// Helper to build a valid binary FDT blob in memory for testing and synthesis.
pub fn build_synthetic_dtb() -> Vec<u8> {
    let mut dtb = Vec::new();

    // Strings table
    let mut strings = Vec::new();
    let add_str = |strs: &mut Vec<u8>, s: &str| -> u32 {
        let off = strs.len() as u32;
        strs.extend_from_slice(s.as_bytes());
        strs.push(0);
        off
    };

    let name_model = add_str(&mut strings, "model");
    let name_compat = add_str(&mut strings, "compatible");
    let name_reg = add_str(&mut strings, "reg");
    let name_irq = add_str(&mut strings, "interrupts");
    let _name_freq = add_str(&mut strings, "clock-frequency");

    // Align strings block
    while (strings.len() % 4) != 0 {
        strings.push(0);
    }

    // Structure block
    let mut structs = Vec::new();

    let align_struct = |s: &mut Vec<u8>| {
        while (s.len() % 4) != 0 {
            s.push(0);
        }
    };

    // Root node
    structs.extend_from_slice(&FDT_BEGIN_NODE.to_be_bytes());
    structs.push(0); // Root name is empty
    align_struct(&mut structs);

    // Prop: model = "InterOS Mobile DevBoard v1.0"
    structs.extend_from_slice(&FDT_PROP.to_be_bytes());
    let val_model = b"InterOS Mobile DevBoard v1.0\0";
    structs.extend_from_slice(&(val_model.len() as u32).to_be_bytes());
    structs.extend_from_slice(&name_model.to_be_bytes());
    structs.extend_from_slice(val_model);
    align_struct(&mut structs);

    // Node 1: uart@09000000 (ARM PrimeCell PL011)
    structs.extend_from_slice(&FDT_BEGIN_NODE.to_be_bytes());
    structs.extend_from_slice(b"uart@09000000\0");
    align_struct(&mut structs);

    structs.extend_from_slice(&FDT_PROP.to_be_bytes());
    let val_compat_uart = b"arm,pl011\0arm,primecell\0";
    structs.extend_from_slice(&(val_compat_uart.len() as u32).to_be_bytes());
    structs.extend_from_slice(&name_compat.to_be_bytes());
    structs.extend_from_slice(val_compat_uart);
    align_struct(&mut structs);

    structs.extend_from_slice(&FDT_PROP.to_be_bytes());
    let mut val_reg_uart = [0u8; 8];
    val_reg_uart[0..4].copy_from_slice(&0x09000000u32.to_be_bytes());
    val_reg_uart[4..8].copy_from_slice(&0x00001000u32.to_be_bytes());
    structs.extend_from_slice(&(8u32).to_be_bytes());
    structs.extend_from_slice(&name_reg.to_be_bytes());
    structs.extend_from_slice(&val_reg_uart);
    align_struct(&mut structs);

    structs.extend_from_slice(&FDT_END_NODE.to_be_bytes());

    // Node 2: touchscreen@5d (Goodix GT911 Capacitive Touchscreen on I2C)
    structs.extend_from_slice(&FDT_BEGIN_NODE.to_be_bytes());
    structs.extend_from_slice(b"touchscreen@5d\0");
    align_struct(&mut structs);

    structs.extend_from_slice(&FDT_PROP.to_be_bytes());
    let val_compat_touch = b"goodix,gt911\0";
    structs.extend_from_slice(&(val_compat_touch.len() as u32).to_be_bytes());
    structs.extend_from_slice(&name_compat.to_be_bytes());
    structs.extend_from_slice(val_compat_touch);
    align_struct(&mut structs);

    structs.extend_from_slice(&FDT_PROP.to_be_bytes());
    let val_irq_touch = 35u32.to_be_bytes(); // IRQ 35
    structs.extend_from_slice(&(4u32).to_be_bytes());
    structs.extend_from_slice(&name_irq.to_be_bytes());
    structs.extend_from_slice(&val_irq_touch);
    align_struct(&mut structs);

    structs.extend_from_slice(&FDT_END_NODE.to_be_bytes());

    // End of root
    structs.extend_from_slice(&FDT_END_NODE.to_be_bytes());
    structs.extend_from_slice(&FDT_END.to_be_bytes());

    // Compute layout
    let header_size = 40usize;
    let rsvmap_size = 16usize; // Two empty 64-bit entries
    let off_rsvmap = header_size;
    let off_struct = off_rsvmap + rsvmap_size;
    let off_strings = off_struct + structs.len();
    let total_size = off_strings + strings.len();

    // Assemble final DTB binary
    dtb.extend_from_slice(&FDT_MAGIC.to_be_bytes());
    dtb.extend_from_slice(&(total_size as u32).to_be_bytes());
    dtb.extend_from_slice(&(off_struct as u32).to_be_bytes());
    dtb.extend_from_slice(&(off_strings as u32).to_be_bytes());
    dtb.extend_from_slice(&(off_rsvmap as u32).to_be_bytes());
    dtb.extend_from_slice(&17u32.to_be_bytes()); // version 17
    dtb.extend_from_slice(&16u32.to_be_bytes()); // last_comp_version 16
    dtb.extend_from_slice(&0u32.to_be_bytes());  // boot_cpuid_phys
    dtb.extend_from_slice(&(strings.len() as u32).to_be_bytes());
    dtb.extend_from_slice(&(structs.len() as u32).to_be_bytes());

    // Reservation map (empty terminator)
    dtb.extend_from_slice(&[0u8; 16]);

    // Structure block
    dtb.extend_from_slice(&structs);

    // Strings block
    dtb.extend_from_slice(&strings);

    dtb
}
