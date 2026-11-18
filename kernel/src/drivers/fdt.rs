// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Flattened Device Tree (FDT / DTB) Hardware Discovery Parser
//!
//! Provides bare-metal device tree parsing for ARM64 and RISC-V 64 platforms:
//! - Parses standard binary Devicetree Blob (DTB) format (Magic `0xd00dfeed`)
//! - Walks node hierarchy (`FDT_BEGIN_NODE`, `FDT_END_NODE`, `FDT_PROP`, `FDT_END`)
//! - Discovers peripheral MMIO addresses, interrupt routes, CPU topology, and RAM ranges
//! - Matches `compatible` strings against known device drivers (`ns16550a`, `arm,gic-400`, `simple-framebuffer`, `goodix,gt911`)

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Standard FDT magic header constant.
pub const FDT_MAGIC: u32 = 0xd00dfeed;

/// FDT Token Identifiers.
pub const FDT_BEGIN_NODE: u32 = 0x01;
pub const FDT_END_NODE: u32 = 0x02;
pub const FDT_PROP: u32 = 0x03;
pub const FDT_NOP: u32 = 0x04;
pub const FDT_END: u32 = 0x09;

/// Parsed device node discovered in the Flattened Device Tree.
#[derive(Debug, Clone)]
pub struct DiscoveredDevice {
    pub name: String,
    pub compatible: Vec<String>,
    pub mmio_base: Option<u64>,
    pub mmio_size: Option<u64>,
    pub irq: Option<u32>,
}

/// FDT Hardware Discovery Tree.
pub struct FdtParser<'a> {
    data: &'a [u8],
    off_struct: usize,
    off_strings: usize,
    totalsize: usize,
}

impl<'a> FdtParser<'a> {
    /// Validates the DTB header and initializes the parser.
    pub fn new(data: &'a [u8]) -> Result<Self, &'static str> {
        if data.len() < 32 {
            return Err("FDT: Buffer too small for header");
        }

        let magic = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        if magic != FDT_MAGIC {
            return Err("FDT: Invalid magic header (expected 0xd00dfeed)");
        }

        let totalsize = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let off_struct = u32::from_be_bytes([data[8], data[9], data[10], data[11]]) as usize;
        let off_strings = u32::from_be_bytes([data[12], data[13], data[14], data[15]]) as usize;

        if totalsize > data.len() || off_struct >= totalsize || off_strings >= totalsize {
            return Err("FDT: Corrupt header offsets");
        }

        Ok(Self {
            data,
            off_struct,
            off_strings,
            totalsize,
        })
    }

    /// Reads a null-terminated string from the strings block.
    fn read_string(&self, offset: usize) -> Option<String> {
        // WHY checked_add + totalsize bound: `offset` is an attacker-controlled
        // u32 from a FDT_PROP nameoff field. The sum must not wrap, and the
        // NUL scan must stay inside the declared blob, never past it.
        let abs_off = self.off_strings.checked_add(offset)?;
        if abs_off >= self.totalsize {
            return None;
        }
        let end = self.data[abs_off..self.totalsize].iter().position(|&b| b == 0)?;
        core::str::from_utf8(&self.data[abs_off..abs_off + end]).ok().map(|s| s.to_string())
    }

    /// Walks the device tree and extracts all peripheral devices.
    pub fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>, &'static str> {
        let mut devices = Vec::new();
        let mut offset = self.off_struct;
        let mut node_stack: Vec<DiscoveredDevice> = Vec::new();

        while offset + 4 <= self.totalsize {
            let token = u32::from_be_bytes([
                self.data[offset],
                self.data[offset + 1],
                self.data[offset + 2],
                self.data[offset + 3],
            ]);
            offset += 4;

            match token {
                FDT_BEGIN_NODE => {
                    // Extract node name null-terminated
                    let start = offset;
                    while offset < self.totalsize && self.data[offset] != 0 {
                        offset += 1;
                    }
                    let name = core::str::from_utf8(&self.data[start..offset])
                        .unwrap_or("unknown")
                        .to_string();
                    offset += 1; // skip null byte
                    // Align to 4 bytes
                    while offset % 4 != 0 {
                        offset += 1;
                    }

                    node_stack.push(DiscoveredDevice {
                        name,
                        compatible: Vec::new(),
                        mmio_base: None,
                        mmio_size: None,
                        irq: None,
                    });
                }
                FDT_END_NODE => {
                    if let Some(dev) = node_stack.pop() {
                        if !dev.compatible.is_empty() || dev.mmio_base.is_some() {
                            devices.push(dev);
                        }
                    }
                }
                FDT_PROP => {
                    if offset + 8 > self.totalsize {
                        break;
                    }
                    let len = u32::from_be_bytes([
                        self.data[offset],
                        self.data[offset + 1],
                        self.data[offset + 2],
                        self.data[offset + 3],
                    ]) as usize;
                    let nameoff = u32::from_be_bytes([
                        self.data[offset + 4],
                        self.data[offset + 5],
                        self.data[offset + 6],
                        self.data[offset + 7],
                    ]) as usize;
                    offset += 8;

                    // WHY the bound is checked before slicing: `len` is an
                    // attacker-controlled u32 straight out of the blob. The
                    // old code sliced data[offset..offset + len] with no
                    // bound at all, so a forged length panicked (or read out
                    // of bounds). The property must fit inside totalsize,
                    // which `new` already proved is <= data.len().
                    let prop_end = offset
                        .checked_add(len)
                        .ok_or("FDT: Property length overflows address space")?;
                    if prop_end > self.totalsize {
                        return Err("FDT: Property extends past declared totalsize");
                    }
                    let prop_name = self.read_string(nameoff).unwrap_or_default();
                    let prop_val = &self.data[offset..prop_end];
                    offset = prop_end;
                    while offset % 4 != 0 {
                        offset += 1;
                    }

                    if let Some(current_node) = node_stack.last_mut() {
                        if prop_name == "compatible" {
                            // Multiple null-separated strings
                            for slice in prop_val.split(|&b| b == 0) {
                                if !slice.is_empty() {
                                    if let Ok(s) = core::str::from_utf8(slice) {
                                        current_node.compatible.push(s.to_string());
                                    }
                                }
                            }
                        } else if prop_name == "reg" && len >= 8 {
                            if len >= 16 {
                                // 64-bit base + 64-bit size
                                let base = u64::from_be_bytes([
                                    prop_val[0], prop_val[1], prop_val[2], prop_val[3],
                                    prop_val[4], prop_val[5], prop_val[6], prop_val[7],
                                ]);
                                let size = u64::from_be_bytes([
                                    prop_val[8], prop_val[9], prop_val[10], prop_val[11],
                                    prop_val[12], prop_val[13], prop_val[14], prop_val[15],
                                ]);
                                current_node.mmio_base = Some(base);
                                current_node.mmio_size = Some(size);
                            } else {
                                // 32-bit base + 32-bit size
                                let base = u32::from_be_bytes([prop_val[0], prop_val[1], prop_val[2], prop_val[3]]) as u64;
                                let size = u32::from_be_bytes([prop_val[4], prop_val[5], prop_val[6], prop_val[7]]) as u64;
                                current_node.mmio_base = Some(base);
                                current_node.mmio_size = Some(size);
                            }
                        } else if prop_name == "interrupts" && len >= 4 {
                            current_node.irq = Some(u32::from_be_bytes([
                                prop_val[0], prop_val[1], prop_val[2], prop_val[3],
                            ]));
                        }
                    }
                }
                FDT_NOP => {}
                FDT_END => break,
                _ => break,
            }
        }

        Ok(devices)
    }
}

/// Builds a verified synthetic FDT blob for automated discovery self-tests.
pub fn build_test_fdt() -> Vec<u8> {
    let mut fdt = Vec::new();
    let mut str_block = Vec::new();

    // Helper to intern strings
    let mut add_str = |s: &str| -> usize {
        let off = str_block.len();
        str_block.extend_from_slice(s.as_bytes());
        str_block.push(0);
        off
    };

    let comp_off = add_str("compatible");
    let reg_off = add_str("reg");
    let irq_off = add_str("interrupts");

    let mut struct_block = Vec::new();

    // 1. Root node
    struct_block.extend_from_slice(&FDT_BEGIN_NODE.to_be_bytes());
    struct_block.push(0); // root name ""
    while struct_block.len() % 4 != 0 { struct_block.push(0); }

    // 2. Serial UART node: "serial@09000000"
    struct_block.extend_from_slice(&FDT_BEGIN_NODE.to_be_bytes());
    struct_block.extend_from_slice(b"serial@09000000\0");
    while struct_block.len() % 4 != 0 { struct_block.push(0); }

    // Prop: compatible = "ns16550a"
    struct_block.extend_from_slice(&FDT_PROP.to_be_bytes());
    let comp_val = b"ns16550a\0";
    struct_block.extend_from_slice(&(comp_val.len() as u32).to_be_bytes());
    struct_block.extend_from_slice(&(comp_off as u32).to_be_bytes());
    struct_block.extend_from_slice(comp_val);
    while struct_block.len() % 4 != 0 { struct_block.push(0); }

    // Prop: reg = 0x09000000, size = 0x1000
    struct_block.extend_from_slice(&FDT_PROP.to_be_bytes());
    struct_block.extend_from_slice(&8u32.to_be_bytes());
    struct_block.extend_from_slice(&(reg_off as u32).to_be_bytes());
    struct_block.extend_from_slice(&0x09000000u32.to_be_bytes());
    struct_block.extend_from_slice(&0x00001000u32.to_be_bytes());

    // Prop: interrupts = 10
    struct_block.extend_from_slice(&FDT_PROP.to_be_bytes());
    struct_block.extend_from_slice(&4u32.to_be_bytes());
    struct_block.extend_from_slice(&(irq_off as u32).to_be_bytes());
    struct_block.extend_from_slice(&10u32.to_be_bytes());

    struct_block.extend_from_slice(&FDT_END_NODE.to_be_bytes());

    // 3. Touch Controller node: "touch@5d"
    struct_block.extend_from_slice(&FDT_BEGIN_NODE.to_be_bytes());
    struct_block.extend_from_slice(b"touch@5d\0");
    while struct_block.len() % 4 != 0 { struct_block.push(0); }

    // Prop: compatible = "goodix,gt911"
    struct_block.extend_from_slice(&FDT_PROP.to_be_bytes());
    let touch_comp = b"goodix,gt911\0";
    struct_block.extend_from_slice(&(touch_comp.len() as u32).to_be_bytes());
    struct_block.extend_from_slice(&(comp_off as u32).to_be_bytes());
    struct_block.extend_from_slice(touch_comp);
    while struct_block.len() % 4 != 0 { struct_block.push(0); }

    struct_block.extend_from_slice(&FDT_END_NODE.to_be_bytes());

    // Close Root node
    struct_block.extend_from_slice(&FDT_END_NODE.to_be_bytes());
    struct_block.extend_from_slice(&FDT_END.to_be_bytes());

    // Assemble Header
    let header_size = 40;
    let off_struct = header_size;
    let off_strings = off_struct + struct_block.len();
    let totalsize = off_strings + str_block.len();

    fdt.extend_from_slice(&FDT_MAGIC.to_be_bytes());
    fdt.extend_from_slice(&(totalsize as u32).to_be_bytes());
    fdt.extend_from_slice(&(off_struct as u32).to_be_bytes());
    fdt.extend_from_slice(&(off_strings as u32).to_be_bytes());
    fdt.extend_from_slice(&32u32.to_be_bytes()); // off_mem_rsvmap
    fdt.extend_from_slice(&17u32.to_be_bytes()); // version
    fdt.extend_from_slice(&16u32.to_be_bytes()); // last_comp_version
    fdt.extend_from_slice(&0u32.to_be_bytes());  // boot_cpuid_phys
    fdt.extend_from_slice(&(str_block.len() as u32).to_be_bytes()); // size_dt_strings
    fdt.extend_from_slice(&(struct_block.len() as u32).to_be_bytes()); // size_dt_struct

    fdt.extend_from_slice(&struct_block);
    fdt.extend_from_slice(&str_block);
    fdt
}

/// Automated verification self-test for FDT parser.
pub fn test_fdt_parser() {
    let dtb = build_test_fdt();
    let parser = FdtParser::new(&dtb).expect("FDT parser initialization failed");
    let devices = parser.discover_devices().expect("Device discovery failed");

    assert!(devices.iter().any(|d| d.compatible.contains(&"ns16550a".to_string())));
    assert!(devices.iter().any(|d| d.compatible.contains(&"goodix,gt911".to_string())));

    let serial = devices.iter().find(|d| d.compatible.contains(&"ns16550a".to_string())).unwrap();
    assert_eq!(serial.mmio_base, Some(0x09000000));
    assert_eq!(serial.irq, Some(10));

    crate::serial_println!("[TEST] FDT: Device Tree parser discovered 2 hardware peripherals (UART, Goodix Touch).");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locate the first FDT_PROP token inside the struct block and return the
    /// byte index of its 4-byte big-endian length field.
    fn first_prop_len_offset(dtb: &[u8]) -> usize {
        let off_struct = u32::from_be_bytes([dtb[8], dtb[9], dtb[10], dtb[11]]) as usize;
        let mut i = off_struct;
        while i + 4 <= dtb.len() {
            let token = u32::from_be_bytes([dtb[i], dtb[i + 1], dtb[i + 2], dtb[i + 3]]);
            if token == FDT_PROP {
                return i + 4;
            }
            i += 4;
        }
        panic!("fixture must contain at least one FDT_PROP");
    }

    #[test]
    fn fdt_bounds_oversized_property_refused_without_slice_panic() {
        let mut dtb = build_test_fdt();
        let len_at = first_prop_len_offset(&dtb);
        // Forged u32 length far beyond the blob: must be a typed refusal,
        // never an out-of-bounds slice panic.
        dtb[len_at..len_at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        let parser = FdtParser::new(&dtb).expect("header still valid");
        let err = parser.discover_devices().unwrap_err();
        assert!(err.contains("totalsize"), "unexpected refusal: {err}");
        std::println!("[FDT-BOUNDS] forged len=0xFFFFFFFF refused: {err}");
    }

    #[test]
    fn fdt_bounds_property_len_just_past_totalsize_refused() {
        let mut dtb = build_test_fdt();
        let len_at = first_prop_len_offset(&dtb);
        let totalsize = u32::from_be_bytes([dtb[4], dtb[5], dtb[6], dtb[7]]) as usize;
        // Smallest length that pushes the property one byte past totalsize.
        let forged = (totalsize - (len_at + 4) + 1) as u32;
        dtb[len_at..len_at + 4].copy_from_slice(&forged.to_be_bytes());
        let parser = FdtParser::new(&dtb).expect("header still valid");
        assert!(parser.discover_devices().is_err());
    }

    #[test]
    fn fdt_bounds_forged_nameoff_and_unterminated_strings_do_not_panic() {
        let mut dtb = build_test_fdt();
        let len_at = first_prop_len_offset(&dtb);
        // nameoff far past the strings block: read_string must return None.
        dtb[len_at + 4..len_at + 8].copy_from_slice(&u32::MAX.to_be_bytes());
        let parser = FdtParser::new(&dtb).expect("header still valid");
        let devices = parser.discover_devices().expect("walk must not fail on bad nameoff");
        // reg property still yields the UART mmio even with no compatible name.
        assert!(devices.iter().any(|d| d.mmio_base == Some(0x09000000)));

        // Remove every NUL from the strings block: bounded scan must stop at
        // totalsize instead of running off the buffer.
        let mut dtb2 = build_test_fdt();
        let off_strings = u32::from_be_bytes([dtb2[12], dtb2[13], dtb2[14], dtb2[15]]) as usize;
        for b in &mut dtb2[off_strings..] {
            *b = b'A';
        }
        let parser2 = FdtParser::new(&dtb2).expect("header still valid");
        let devices2 = parser2.discover_devices().expect("walk must not fail on unterminated strings");
        assert!(devices2.iter().all(|d| d.compatible.is_empty()));
    }

    #[test]
    fn fdt_bounds_valid_fixture_still_discovers_both_devices() {
        let dtb = build_test_fdt();
        let parser = FdtParser::new(&dtb).expect("valid fixture");
        let devices = parser.discover_devices().expect("valid fixture discovers");
        assert_eq!(devices.len(), 2);
        let serial = devices.iter().find(|d| d.compatible.contains(&"ns16550a".to_string())).unwrap();
        assert_eq!(serial.mmio_base, Some(0x09000000));
        assert_eq!(serial.mmio_size, Some(0x1000));
        assert_eq!(serial.irq, Some(10));
        assert!(devices.iter().any(|d| d.compatible.contains(&"goodix,gt911".to_string())));
    }
}
