// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Flattened Device Tree (FDT / DTB v17) Binary Parser & Board Topology
//!
//! Zero-mock, real hardware discovery for RISC-V 64 boards (QEMU `virt`,
//! SiFive, StarFive, T-Head). Parses the DTB pointer handed to `_start` in
//! `a1` by the bootloader/firmware and records the physical memory base/size,
//! the 16550 UART aperture, the PLIC, the CLINT, and the timebase frequency.
//!
//! The parser itself is allocation-light (it only materialises the node list
//! on demand via `enumerate_nodes`). The discovered `BoardInfo` is stored in a
//! global so peripheral drivers (PLIC, timer, console) read FDT values instead
//! of hardcoding the QEMU `virt` map. When the DTB is absent or malformed the
//! accessors fall back to the documented QEMU `virt` defaults and `from_fdt`
//! stays `false`, so a missing DTB never fabricates discovery.

extern crate alloc;
use alloc::vec::Vec;
use spin::Mutex;

/// Standard Device Tree binary magic constant (`0xd00dfeed`, big-endian).
pub const FDT_MAGIC: u32 = 0xd00dfeed;

/// Structure block token constants.
pub const FDT_BEGIN_NODE: u32 = 0x0000_0001;
pub const FDT_END_NODE: u32 = 0x0000_0002;
pub const FDT_PROP: u32 = 0x0000_0003;
pub const FDT_NOP: u32 = 0x0000_0004;
pub const FDT_END: u32 = 0x0000_0009;

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

/// A parsed device tree node property (zero-copy borrow into the DTB).
#[derive(Debug, Clone)]
pub struct FdtProperty<'a> {
    pub name: &'a str,
    pub value: &'a [u8],
}

fn read_be_u32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

impl<'a> FdtProperty<'a> {
    /// Interprets the property value as a NUL-terminated UTF-8 string.
    pub fn as_str(&self) -> Option<&'a str> {
        let trimmed = if self.value.ends_with(b"\0") {
            &self.value[..self.value.len() - 1]
        } else {
            self.value
        };
        core::str::from_utf8(trimmed).ok()
    }

    /// Interprets the property value as a big-endian 32-bit unsigned integer.
    pub fn as_u32(&self) -> Option<u32> {
        if self.value.len() >= 4 {
            Some(read_be_u32(&self.value[..4]))
        } else {
            None
        }
    }

    /// Interprets the property value as a single `reg = <base size>` pair of
    /// big-endian 64-bit (or 32-bit) integers.
    pub fn as_reg_u64(&self) -> Option<(u64, u64)> {
        if self.value.len() >= 16 {
            let mut base = [0u8; 8];
            let mut size = [0u8; 8];
            base.copy_from_slice(&self.value[0..8]);
            size.copy_from_slice(&self.value[8..16]);
            Some((u64::from_be_bytes(base), u64::from_be_bytes(size)))
        } else if self.value.len() >= 8 {
            Some((
                read_be_u32(&self.value[0..4]) as u64,
                read_be_u32(&self.value[4..8]) as u64,
            ))
        } else {
            None
        }
    }

    /// Interprets the property value as a list of `<base size>` register pairs
    /// using the parent scope's `#address-cells` / `#size-cells` widths.
    pub fn reg_pairs(&self, addr_cells: u32, size_cells: u32) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        let a = addr_cells as usize;
        let s = size_cells as usize;
        if a == 0 || s == 0 {
            return out;
        }
        let mut off = 0usize;
        while off + (a + s) * 4 <= self.value.len() {
            let mut base = 0u64;
            for i in 0..a {
                base = (base << 32) | read_be_u32(&self.value[off + i * 4..off + i * 4 + 4]) as u64;
            }
            let mut size = 0u64;
            for i in 0..s {
                size = (size << 32)
                    | read_be_u32(&self.value[off + (a + i) * 4..off + (a + i) * 4 + 4]) as u64;
            }
            out.push((base, size));
            off += (a + s) * 4;
        }
        out
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

    /// True if the `compatible` property contains `target`.
    pub fn is_compatible(&self, target: &str) -> bool {
        match self.get_prop("compatible").and_then(|p| p.as_str()) {
            Some(s) => s.split('\0').any(|c| c == target) || s.contains(target),
            None => false,
        }
    }

    /// True if the `device_type` property equals `target`.
    pub fn has_device_type(&self, target: &str) -> bool {
        self.get_prop("device_type").and_then(|p| p.as_str()) == Some(target)
    }
}

/// Error conditions encountered during FDT parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdtError {
    BufferTooShort,
    InvalidMagic,
    MalformedStructure,
    OutOfBoundsOffset,
}

/// Zero-copy Flattened Device Tree viewer borrowing the live DTB in memory.
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
        let magic = read_be_u32(&data[0..4]);
        if magic != FDT_MAGIC {
            return Err(FdtError::InvalidMagic);
        }
        let totalsize = read_be_u32(&data[4..8]);
        if (totalsize as usize) < 40 || (totalsize as usize) > data.len() {
            return Err(FdtError::BufferTooShort);
        }
        let header = FdtHeader {
            magic,
            totalsize,
            off_dt_struct: read_be_u32(&data[8..12]),
            off_dt_strings: read_be_u32(&data[12..16]),
            off_mem_rsvmap: read_be_u32(&data[16..20]),
            version: read_be_u32(&data[20..24]),
            last_comp_version: read_be_u32(&data[24..28]),
            boot_cpuid_phys: read_be_u32(&data[28..32]),
            size_dt_strings: read_be_u32(&data[32..36]),
            size_dt_struct: read_be_u32(&data[36..40]),
        };
        let data = &data[..totalsize as usize];
        Ok(Self { data, header })
    }

    /// Builds a parser over the live physical DTB at `addr`.
    ///
    /// # Safety
    /// `addr` must point to at least `max_len` readable bytes of identity-mapped
    /// memory containing a valid DTB. The caller guarantees the mapping is live.
    pub unsafe fn from_raw(addr: usize, max_len: usize) -> Result<FdtParser<'static>, FdtError> {
        if addr == 0 || max_len < 40 {
            return Err(FdtError::BufferTooShort);
        }
        let data: &'static [u8] = core::slice::from_raw_parts(addr as *const u8, max_len);
        FdtParser::from_slice(data)
    }

    /// Returns the parsed FDT header.
    pub fn header(&self) -> &FdtHeader {
        &self.header
    }

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

    /// Scans the structure block and enumerates every declared node.
    pub fn enumerate_nodes(&self) -> Result<Vec<FdtNode<'a>>, FdtError> {
        let mut nodes = Vec::new();
        let mut offset = self.header.off_dt_struct as usize;
        let struct_end = match offset.checked_add(self.header.size_dt_struct as usize) {
            Some(e) => e,
            None => return Err(FdtError::OutOfBoundsOffset),
        };
        if struct_end > self.data.len() {
            return Err(FdtError::OutOfBoundsOffset);
        }

        let read_u32_at = |pos: usize| -> Result<u32, FdtError> {
            if pos + 4 > struct_end {
                return Err(FdtError::MalformedStructure);
            }
            Ok(read_be_u32(&self.data[pos..pos + 4]))
        };

        let mut node_stack: Vec<(&'a str, Vec<FdtProperty<'a>>)> = Vec::new();

        while offset < struct_end {
            let token = read_u32_at(offset)?;
            offset += 4;
            match token {
                FDT_BEGIN_NODE => {
                    let name_start = offset;
                    while offset < struct_end && self.data[offset] != 0 {
                        offset += 1;
                    }
                    let name_raw = &self.data[name_start..offset];
                    let node_name = core::str::from_utf8(name_raw).unwrap_or("unnamed");
                    offset += 1; // skip NUL
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
                        nodes.push(FdtNode { name, properties });
                    }
                }
                FDT_NOP => {}
                FDT_END => break,
                _ => return Err(FdtError::MalformedStructure),
            }
        }
        Ok(nodes)
    }

    /// Finds the first node whose `compatible` property contains `compatible`.
    pub fn find_compatible_node(&self, compatible: &str) -> Result<Option<FdtNode<'a>>, FdtError> {
        for node in self.enumerate_nodes()? {
            if node.is_compatible(compatible) {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }

    /// Root node `#address-cells` / `#size-cells`, defaulting to 2/2 (RV64).
    pub fn root_cells(&self) -> (u32, u32) {
        if let Ok(nodes) = self.enumerate_nodes() {
            if let Some(root) = nodes.iter().find(|n| n.name == "/") {
                let ac = root.get_prop("#address-cells").and_then(|p| p.as_u32()).unwrap_or(2);
                let sc = root.get_prop("#size-cells").and_then(|p| p.as_u32()).unwrap_or(2);
                return (ac, sc);
            }
        }
        (2, 2)
    }
}

/// Discovered board topology. Populated from the DTB when present; otherwise
/// the documented QEMU `virt` defaults are reported with `from_fdt == false`.
#[derive(Debug, Clone, Copy)]
pub struct BoardInfo {
    pub memory_base: u64,
    pub memory_size: u64,
    pub uart_base: u64,
    pub plic_base: u64,
    pub clint_base: u64,
    pub timebase_freq: u64,
    pub from_fdt: bool,
}

impl BoardInfo {
    /// Documented QEMU `virt` RISC-V memory map (fallback, not fabrication).
    pub const fn virt_default() -> Self {
        Self {
            memory_base: 0x8000_0000,
            memory_size: 0,
            uart_base: 0x1000_0000,
            plic_base: 0x0c00_0000,
            clint_base: 0x0200_0000,
            timebase_freq: 10_000_000,
            from_fdt: false,
        }
    }
}

static BOARD: Mutex<BoardInfo> = Mutex::new(BoardInfo::virt_default());

/// Parses `parser` and records the discovered board topology. Returns the
/// stored `BoardInfo`. Missing nodes keep their QEMU `virt` default value.
pub fn init_from_fdt(parser: &FdtParser) -> BoardInfo {
    let mut info = BoardInfo::virt_default();
    info.from_fdt = true;
    let (ac, sc) = parser.root_cells();

    if let Ok(nodes) = parser.enumerate_nodes() {
        for node in &nodes {
            let is_mem = node.has_device_type("memory") || node.name.starts_with("memory");
            if is_mem {
                if let Some(reg) = node.get_prop("reg") {
                    if let Some((base, size)) = reg.reg_pairs(ac, sc).into_iter().next() {
                        info.memory_base = base;
                        info.memory_size = size;
                    }
                }
            }
            if node.is_compatible("ns16550a") {
                if let Some((base, _)) = node.get_prop("reg").and_then(|r| r.as_reg_u64()) {
                    info.uart_base = base;
                }
            }
            if node.is_compatible("sifive,plic-1.0") || node.is_compatible("riscv,plic0") {
                if let Some((base, _)) = node.get_prop("reg").and_then(|r| r.as_reg_u64()) {
                    info.plic_base = base;
                }
            }
            if node.is_compatible("sifive,clint0") || node.is_compatible("riscv,clint0") {
                if let Some((base, _)) = node.get_prop("reg").and_then(|r| r.as_reg_u64()) {
                    info.clint_base = base;
                }
            }
            if node.name == "/" {
                if let Some(freq) = node.get_prop("timebase-frequency").and_then(|p| p.as_u32()) {
                    if freq != 0 {
                        info.timebase_freq = freq as u64;
                    }
                }
            }
        }
    }

    *BOARD.lock() = info;
    info
}

/// Returns the most recently discovered board topology.
pub fn board_info() -> BoardInfo {
    *BOARD.lock()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_magic() {
        let junk = [0u8; 64];
        assert!(matches!(
            FdtParser::from_slice(&junk),
            Err(FdtError::InvalidMagic)
        ));
    }
}