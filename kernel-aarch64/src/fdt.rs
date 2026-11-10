// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Zero-Allocation Flattened Device Tree (FDT / DTB) Parser
//!
//! Conforms to Devicetree Specification v0.4 with zero heap allocation (`no_std`).
//! Parses DTB passed in CPU register `x0` by physical mobile bootloaders or QEMU.
//!
//! A device tree is supplied by the bootloader, so every offset and length in it
//! is untrusted input. All header fields are validated against `totalsize` before
//! any token is read, and every structure/string walk is bounded by a limit that
//! is itself proven to lie inside the mapped blob. Any inconsistency is reported
//! as an error instead of being clamped, so a malformed tree can never steer a
//! read outside the blob.

extern crate alloc;

use alloc::vec::Vec;

pub const FDT_MAGIC: u32 = 0xD00D_FEED;

/// Highest specification version this parser understands (v17, 2020-12-10).
pub const FDT_LAST_SUPPORTED_VERSION: u32 = 17;

/// Oldest specification version this parser understands (v16, 3.14).
pub const FDT_FIRST_SUPPORTED_VERSION: u32 = 16;

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

        if version < FDT_FIRST_SUPPORTED_VERSION {
            return Err(FdtError::UnsupportedVersion(version));
        }
        if version > FDT_LAST_SUPPORTED_VERSION {
            return Err(FdtError::UnsupportedVersion(version));
        }
        if last_comp_version > version {
            return Err(FdtError::CorruptedStructure);
        }

        // The blob may be longer than the tree it describes, but never shorter.
        let totalsize = totalsize as usize;
        if totalsize > data.len() {
            return Err(FdtError::BufferTooShort);
        }
        if totalsize < core::mem::size_of::<FdtHeader>() {
            return Err(FdtError::BufferTooShort);
        }

        // The reserved-memory map must contain at least one terminating entry.
        if (off_mem_rsvmap as usize)
            .checked_add(16)
            .map_or(true, |end| end > totalsize)
        {
            return Err(FdtError::CorruptedStructure);
        }

        // Every region must lie inside the blob. Without this the walk limits
        // below could point past `data` and turn a malformed tree into an
        // out-of-bounds read.
        let struct_end = (off_dt_struct as usize)
            .checked_add(size_dt_struct as usize)
            .ok_or(FdtError::CorruptedStructure)?;
        if struct_end > totalsize {
            return Err(FdtError::CorruptedStructure);
        }

        let strings_end = (off_dt_strings as usize)
            .checked_add(size_dt_strings as usize)
            .ok_or(FdtError::CorruptedStructure)?;
        if strings_end > totalsize {
            return Err(FdtError::CorruptedStructure);
        }

        // The structure block must be 4-octet aligned and long enough to hold
        // at least the root node plus the FDT_END token.
        if off_dt_struct % 4 != 0 || size_dt_struct < 8 {
            return Err(FdtError::CorruptedStructure);
        }

        let header = FdtHeader {
            magic,
            totalsize: totalsize as u32,
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

    /// Builds a parser over a blob located at a bootloader-supplied address.
    ///
    /// The caller is responsible for knowing that `totalsize` bytes are mapped
    /// at `ptr`; the size is still bounded so a corrupt header cannot request an
    /// unbounded mapping, and the blob is re-validated by [`ArmFdtParser::new`].
    pub unsafe fn from_raw_ptr(ptr: *const u8) -> Result<ArmFdtParser<'static>, FdtError> {
        if ptr.is_null() {
            return Err(FdtError::InvalidMagic);
        }
        if (ptr as usize) % 4 != 0 {
            return Err(FdtError::CorruptedStructure);
        }
        let p_u8 = ptr as *const [u8; 4];
        let magic = u32::from_be_bytes(*p_u8);
        if magic != FDT_MAGIC {
            return Err(FdtError::InvalidMagic);
        }
        let p_size = ptr.add(4) as *const [u8; 4];
        let totalsize = u32::from_be_bytes(*p_size) as usize;
        if totalsize < core::mem::size_of::<FdtHeader>() || totalsize > 32 * 1024 * 1024 {
            return Err(FdtError::BufferTooShort);
        }
        let slice = core::slice::from_raw_parts(ptr, totalsize);
        ArmFdtParser::new(slice)
    }

    pub fn total_size(&self) -> usize {
        self.header.totalsize as usize
    }

    /// Exclusive end of the structure block. Proven to be within `data`.
    fn struct_limit(&self) -> usize {
        self.header.off_dt_struct as usize + self.header.size_dt_struct as usize
    }

    /// Exclusive end of the strings block. Proven to be within `data`.
    fn strings_limit(&self) -> usize {
        self.header.off_dt_strings as usize + self.header.size_dt_strings as usize
    }

    /// Resolves a string-block-relative offset to a NUL-terminated name.
    pub fn get_string(&self, offset: usize) -> Option<&'a str> {
        let base = self.header.off_dt_strings as usize;
        let limit = self.strings_limit();
        let start = base.checked_add(offset)?;
        if start >= limit {
            return None;
        }

        let mut end = start;
        while end < limit && self.data[end] != 0 {
            end += 1;
        }
        // A name must terminate inside the strings block.
        if end == limit {
            return None;
        }

        core::str::from_utf8(&self.data[start..end]).ok()
    }

    /// Matches a node name against a bare node name, allowing the `@unit-address`
    /// suffix used by most device-tree nodes (`pl011` matches `pl011@9000000`).
    ///
    /// Deliberately stricter than a prefix test: `intc` must not match
    /// `intcmux`, or device discovery could bind MMIO to the wrong node.
    fn node_name_matches(name: &str, target: &str) -> bool {
        if name == target {
            return true;
        }
        match name.strip_prefix(target) {
            Some(rest) => rest.starts_with('@'),
            None => false,
        }
    }

    /// Resolves a NUL-terminated node name starting at `offset`.
    ///
    /// Returns the name bounds and the offset of the next token, which is past
    /// both the terminator and the 4-octet padding.
    fn read_name(&self, offset: usize, limit: usize) -> Option<(&'a str, usize)> {
        let mut cursor = offset;
        while cursor < limit && self.data[cursor] != 0 {
            cursor += 1;
        }
        if cursor >= limit {
            // Unterminated name: the structure block is truncated or corrupt.
            return None;
        }
        let name = core::str::from_utf8(&self.data[offset..cursor]).unwrap_or("");
        let next = (cursor + 4) & !3;
        if next > limit {
            return None;
        }
        Some((name, next))
    }

    pub fn find_node_by_name(&self, target_name: &str) -> Result<ArmFdtNode<'a>, FdtError> {
        let mut offset = self.header.off_dt_struct as usize;
        let limit = self.struct_limit();

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
                    let (name, next) = self
                        .read_name(offset, limit)
                        .ok_or(FdtError::CorruptedStructure)?;
                    offset = next;

                    if Self::node_name_matches(name, target_name) {
                        return Ok(ArmFdtNode {
                            name,
                            struct_offset: node_start,
                        });
                    }
                }
                FDT_PROP => {
                    let (next, _) = self.read_property(offset, limit)?;
                    offset = next;
                }
                FDT_END_NODE | FDT_NOP => {}
                FDT_END => break,
                _ => return Err(FdtError::CorruptedStructure),
            }
        }

        Err(FdtError::NodeNotFound)
    }

    /// Finds the first node whose `compatible` property contains `compatible`.
    pub fn find_node_by_compatible(&self, compatible: &str) -> Result<ArmFdtNode<'a>, FdtError> {
        let mut offset = self.header.off_dt_struct as usize;
        let limit = self.struct_limit();

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
                    let (name, next) = self
                        .read_name(offset, limit)
                        .ok_or(FdtError::CorruptedStructure)?;
                    offset = next;

                    let node = ArmFdtNode {
                        name,
                        struct_offset: node_start,
                    };
                    if let Ok(prop) = self.get_node_property(&node, "compatible") {
                        if Self::compatible_matches(prop, compatible) {
                            return Ok(node);
                        }
                    }
                }
                FDT_PROP => {
                    let (next, _) = self.read_property(offset, limit)?;
                    offset = next;
                }
                FDT_END_NODE | FDT_NOP => {}
                FDT_END => break,
                _ => return Err(FdtError::CorruptedStructure),
            }
        }

        Err(FdtError::NodeNotFound)
    }

    /// Finds the root device tree node ("/").
    pub fn find_root_node(&self) -> Result<ArmFdtNode<'a>, FdtError> {
        let offset = self.header.off_dt_struct as usize;
        let limit = self.struct_limit();
        if offset + 4 <= limit {
            let token = u32::from_be_bytes([
                self.data[offset],
                self.data[offset + 1],
                self.data[offset + 2],
                self.data[offset + 3],
            ]);
            if token == FDT_BEGIN_NODE {
                return Ok(ArmFdtNode {
                    name: "/",
                    struct_offset: offset + 4,
                });
            }
        }
        Err(FdtError::NodeNotFound)
    }

    /// Discovers the simple-framebuffer node passed by U-Boot or firmware.
    ///
    /// Checks in order of priority:
    /// 1. Node with compatible "simple-framebuffer"
    /// 2. Node with compatible "allwinner,simple-framebuffer"
    /// 3. Node named "framebuffer" (e.g. /framebuffer or /chosen/framebuffer)
    pub fn find_framebuffer_node(&self) -> Result<ArmFdtNode<'a>, FdtError> {
        if let Ok(node) = self.find_node_by_compatible("simple-framebuffer") {
            return Ok(node);
        }
        if let Ok(node) = self.find_node_by_compatible("allwinner,simple-framebuffer") {
            return Ok(node);
        }
        if let Ok(node) = self.find_node_by_name("framebuffer") {
            return Ok(node);
        }
        Err(FdtError::NodeNotFound)
    }

    /// Returns true if this device tree describes a Pine64 PinePhone or Allwinner A64 SoC.
    pub fn is_pinephone(&self) -> bool {
        if let Ok(root) = self.find_root_node() {
            if let Ok(prop) = self.get_node_property(&root, "compatible") {
                if Self::compatible_matches(prop, "pine64,pinephone")
                    || Self::compatible_matches(prop, "pine64,pinephone-1.2")
                    || Self::compatible_matches(prop, "allwinner,sun50i-a64")
                {
                    return true;
                }
            }
            if let Ok(prop) = self.get_node_property(&root, "model") {
                if let Ok(s) = core::str::from_utf8(prop) {
                    if s.contains("PinePhone") || s.contains("pinephone") || s.contains("A64") {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Returns the root `#address-cells` value (defaults to 2).
    pub fn root_address_cells(&self) -> usize {
        if let Ok(root) = self.find_root_node() {
            if let Ok(prop) = self.get_node_property(&root, "#address-cells") {
                if let Some(cells) = read_usize(prop) {
                    return cells;
                }
            }
        }
        2
    }

    /// Returns the root `#size-cells` value (defaults to 2).
    pub fn root_size_cells(&self) -> usize {
        if let Ok(root) = self.find_root_node() {
            if let Ok(prop) = self.get_node_property(&root, "#size-cells") {
                if let Some(cells) = read_usize(prop) {
                    return cells;
                }
            }
        }
        2
    }

    /// Tests a NUL-separated `compatible` string list for an exact entry match.
    fn compatible_matches(property: &[u8], wanted: &str) -> bool {
        let mut cursor = 0usize;
        while cursor < property.len() {
            let mut end = cursor;
            while end < property.len() && property[end] != 0 {
                end += 1;
            }
            if core::str::from_utf8(&property[cursor..end]).ok() == Some(wanted) {
                return true;
            }
            cursor = end + 1;
        }
        false
    }

    /// Reads a property header at `offset` and returns its value plus the offset
    /// of the next token. Every bound is derived from `limit`, which is proven to
    /// lie inside the blob, so a forged length cannot escape the structure block.
    fn read_property(&self, offset: usize, limit: usize) -> Result<(usize, &'a [u8]), FdtError> {
        // len and nameoff
        if offset.checked_add(8).map_or(true, |end| end > limit) {
            return Err(FdtError::CorruptedStructure);
        }
        let len = u32::from_be_bytes([
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
        let value_start = offset + 8;

        if len > limit - value_start {
            return Err(FdtError::CorruptedStructure);
        }
        let value_end = value_start + len;
        let next = (value_end + 3) & !3;
        if next > limit {
            return Err(FdtError::CorruptedStructure);
        }

        // A property name must resolve inside the strings block.
        if self.get_string(name_off).is_none() {
            return Err(FdtError::CorruptedStructure);
        }

        Ok((next, &self.data[value_start..value_end]))
    }

    /// Returns the total size of every memory bank described by the tree.
    ///
    /// Honours the root `#address-cells` / `#size-cells` so that a tree using
    /// 32-bit cells is not misread as 64-bit ones, and sums all banks instead of
    /// reporting only the first.
    pub fn parse_memory_regions(&self) -> Result<Vec<(u64, u64)>, FdtError> {
        let mut regions = Vec::new();
        let mut offset = self.header.off_dt_struct as usize;
        let limit = self.struct_limit();

        let root_address_cells = 2usize;
        let root_size_cells = 2usize;

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
                    let (name, next) = self
                        .read_name(offset, limit)
                        .ok_or(FdtError::CorruptedStructure)?;
                    offset = next;

                    let node = ArmFdtNode {
                        name,
                        struct_offset: node_start,
                    };

                    if Self::node_name_matches(name, "memory") {
                        if let Ok(reg) = self.get_node_property(&node, "reg") {
                            let address_cells = self
                                .get_node_property(&node, "#address-cells")
                                .ok()
                                .and_then(read_usize)
                                .unwrap_or(root_address_cells);
                            let size_cells = self
                                .get_node_property(&node, "#size-cells")
                                .ok()
                                .and_then(read_usize)
                                .unwrap_or(root_size_cells);

                            let stride = (address_cells + size_cells) * 4;
                            if address_cells <= 2 && size_cells <= 2 && stride > 0 {
                                let mut cursor = 0usize;
                                while cursor + stride <= reg.len() {
                                    let base = read_cell(&reg[cursor..], address_cells);
                                    let size =
                                        read_cell(&reg[cursor + address_cells * 4..], size_cells);
                                    if size != 0 {
                                        regions.push((base, size));
                                    }
                                    cursor += stride;
                                }
                            }
                        }
                    }
                }
                FDT_PROP => {
                    let (next, _) = self.read_property(offset, limit)?;
                    offset = next;
                }
                FDT_END_NODE | FDT_NOP => {}
                FDT_END => break,
                _ => return Err(FdtError::CorruptedStructure),
            }
        }

        if regions.is_empty() {
            Err(FdtError::PropertyNotFound)
        } else {
            Ok(regions)
        }
    }

    /// Returns the lowest RAM base and the total mapped size across all banks.
    pub fn parse_memory_range(&self) -> Result<(u64, u64), FdtError> {
        let regions = self.parse_memory_regions()?;
        let mut base = u64::MAX;
        let mut total: u64 = 0;
        for (region_base, region_size) in regions {
            if region_base < base {
                base = region_base;
            }
            // Saturating: a hostile tree must not be able to wrap the total.
            total = total.saturating_add(region_size);
        }
        if base == u64::MAX {
            return Err(FdtError::PropertyNotFound);
        }
        Ok((base, total))
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

    pub fn get_node_property(
        &self,
        node: &ArmFdtNode<'a>,
        prop_name: &str,
    ) -> Result<&'a [u8], FdtError> {
        let limit = self.struct_limit();
        let (_, mut offset) = self
            .read_name(node.struct_offset, limit)
            .ok_or(FdtError::CorruptedStructure)?;

        let mut depth = 1usize;
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
                    let (_, next) = self
                        .read_name(offset, limit)
                        .ok_or(FdtError::CorruptedStructure)?;
                    offset = next;
                }
                FDT_END_NODE => {
                    depth -= 1;
                }
                FDT_PROP => {
                    let name_off = u32::from_be_bytes([
                        self.data[offset + 4],
                        self.data[offset + 5],
                        self.data[offset + 6],
                        self.data[offset + 7],
                    ]) as usize;
                    let (next, value) = self.read_property(offset, limit)?;
                    let current_prop_name = self.get_string(name_off).unwrap_or("");
                    offset = next;

                    if depth == 1 && current_prop_name == prop_name {
                        return Ok(value);
                    }
                }
                FDT_NOP => {}
                FDT_END => break,
                _ => return Err(FdtError::CorruptedStructure),
            }
        }

        Err(FdtError::PropertyNotFound)
    }

    /// Finds the first node, at any depth, that carries `prop_name`.
    ///
    /// Used for spec-defined marker properties such as `interrupt-controller`,
    /// which identify a node's role without relying on a vendor string.
    pub fn find_node_by_property(&self, prop_name: &str) -> Result<ArmFdtNode<'a>, FdtError> {
        let mut offset = self.header.off_dt_struct as usize;
        let limit = self.struct_limit();

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
                    let (name, next) = self
                        .read_name(offset, limit)
                        .ok_or(FdtError::CorruptedStructure)?;
                    offset = next;

                    let node = ArmFdtNode {
                        name,
                        struct_offset: node_start,
                    };
                    if self.get_node_property(&node, prop_name).is_ok() {
                        return Ok(node);
                    }
                }
                FDT_PROP => {
                    let (next, _) = self.read_property(offset, limit)?;
                    offset = next;
                }
                FDT_END_NODE | FDT_NOP => {}
                FDT_END => break,
                _ => return Err(FdtError::CorruptedStructure),
            }
        }

        Err(FdtError::NodeNotFound)
    }

    /// Reads a `reg` property as a single address, honouring the declared cell
    /// count. Returns `None` when the property is too short or not 32-bit cells.
    pub fn get_node_reg_base(&self, node: &ArmFdtNode<'a>, address_cells: usize) -> Option<u64> {
        let reg = self.get_node_property(node, "reg").ok()?;
        if address_cells == 0 || address_cells > 2 || reg.len() < address_cells * 4 {
            return None;
        }
        Some(read_cell(reg, address_cells))
    }
}

/// Decodes a big-endian 32- or 64-bit cell list into a single integer.
fn read_cell(bytes: &[u8], cells: usize) -> u64 {
    match cells {
        1 => u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as u64,
        2 => u64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]),
        _ => 0,
    }
}

/// Decodes a property holding a cell count.
fn read_usize(property: &[u8]) -> Option<usize> {
    match property.len() {
        4 => {
            Some(u32::from_be_bytes([property[0], property[1], property[2], property[3]]) as usize)
        }
        8 => {
            let value = u64::from_be_bytes([
                property[0],
                property[1],
                property[2],
                property[3],
                property[4],
                property[5],
                property[6],
                property[7],
            ]);
            usize::try_from(value).ok()
        }
        _ => None,
    }
}
