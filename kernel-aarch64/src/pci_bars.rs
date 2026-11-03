// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! PCI Base Address Register assignment.
//!
//! A PCI device's BARs read back as zero until something programs them, and
//! with nothing programmed the device's registers sit at no address the CPU
//! can reach. On a PC that something is firmware. QEMU's ARM `virt` machine is
//! booted here with `-kernel` and no firmware, and the machine model does not
//! assign BARs itself, so the kernel has to do it.
//!
//! Every address handed out comes from the host bridge's `ranges` property.
//! That is the platform's own statement of which physical addresses the bridge
//! decodes for its PCI side, so inventing a base address would be a guess;
//! reading one out of the tree is not.
//!
//! Layout of a `ranges` entry, decoded empirically from the tree QEMU's `virt`
//! machine produces (84 bytes, 21 cells, three entries of seven cells):
//!
//! ```text
//!   child address | parent address | size
//!   3 cells        | 2 cells        | 2 cells
//! ```
//!
//! * The child address is `#address-cells` wide (3 here) and its *low* cell
//!   carries the space indicator in bit 0: 0 for memory, 1 for port I/O. The
//!   remaining bits of that cell are the low 32 bits of the device-side
//!   address.
//! * The parent address is two cells, **low word first**, and is the physical
//!   address the bridge decodes — this is where a BAR must be programmed.
//! * The size is `#size-cells` wide, also low word first.
//!
//! So one entry is `(#address-cells + 2 + #size-cells)` cells and the property
//! length must be an exact multiple of that; a trailing partial entry is
//! refused rather than guessed at. Note that the child address is stored with
//! its *low* cell first while the parent address and size are stored with their
//! *high* cell first, so the two halves cannot be decoded with one helper.

use crate::fdt::{ArmFdtNode, ArmFdtParser};
use crate::mmu::ArmMmu;
use crate::pcie::{PcieDevice, PcieEcam};

/// How many `ranges` entries will be kept.
const MAX_WINDOWS: usize = 8;
/// How many assigned or rejected regions are remembered for overlap checks.
const MAX_TRACKED: usize = 48;
/// How many RAM banks are taken from the device tree.
const MAX_RAM_BANKS: usize = 4;

/// WHY this bound: the translation tables use a 39-bit virtual address
/// (`T0SZ = 25`), and this kernel builds an identity mapping, so physical
/// addresses at or above 2^39 = 512 GiB have no virtual address at all. QEMU's
/// `virt` machine publishes a window starting at exactly 0x8000_0000 = 512 GiB
/// for wide BARs; it is real platform state but it cannot be reached from here,
/// so it is reported and refused rather than silently chosen.
pub const MAX_REACHABLE: u64 = 1 << 39;

/// The 2 MiB granule the translation tables can describe below gigabytes.
const BLOCK: u64 = 0x20_0000;

/// One window of host-bridge physical address space that BARs may be placed in.
#[derive(Copy, Clone, Debug)]
pub struct PciWindow {
    /// Physical address the bridge decodes, from the entry's parent address.
    pub base: u64,
    /// Length of the window, from the entry's size.
    pub size: u64,
}

impl PciWindow {
    fn end(&self) -> u64 {
        self.base.saturating_add(self.size)
    }
}

/// Reads the big-endian 32-bit cell at byte `offset` in a cell array.
fn cell(raw: &[u8], offset: usize) -> u64 {
    if offset + 4 > raw.len() {
        return 0;
    }
    u64::from(u32::from_be_bytes([
        raw[offset],
        raw[offset + 1],
        raw[offset + 2],
        raw[offset + 3],
    ]))
}

/// Decodes the host bridge's `ranges` property into the windows it publishes.
///
/// Only memory windows are returned. A port I/O entry (space indicator 1) and
/// the two reserved encodings are refused, because a BAR assigned there would
/// decode where the bridge does not forward memory accesses at all.
pub fn parse_ranges(
    parser: &ArmFdtParser,
    node: &ArmFdtNode<'_>,
) -> ([PciWindow; MAX_WINDOWS], usize) {
    let mut windows = [PciWindow { base: 0, size: 0 }; MAX_WINDOWS];
    let mut count = 0usize;

    let raw = match parser.get_node_property(node, "ranges") {
        Ok(raw) => raw,
        Err(_) => return (windows, 0),
    };
    let child_cells = parser
        .get_node_property(node, "#address-cells")
        .ok()
        .and_then(read_cell_count)
        .unwrap_or(0);
    let size_cells = parser
        .get_node_property(node, "#size-cells")
        .ok()
        .and_then(read_cell_count)
        .unwrap_or(0);
    // WHY two: the parent address specifier is sized by the number of address
    // cells at the bridge's *parent*, which is the device tree root. An ARMv8
    // root always uses 2, and the ECAM host bridge relies on that.
    const PARENT_CELLS: usize = 2;
    if child_cells < 2 || size_cells == 0 || size_cells > 2 {
        return (windows, 0);
    }
    let stride = (child_cells + PARENT_CELLS + size_cells) * 4;
    if stride == 0 || raw.len() % stride != 0 {
        // A trailing partial entry means the property is not what it claims to
        // be; decoding a guess would place a BAR somewhere undeclared.
        return (windows, 0);
    }

    let mut cursor = 0usize;
    while cursor + stride <= raw.len() && count < MAX_WINDOWS {
        let space = cell(raw, cursor) & 0x3;
        if space == 0 {
            let parent_at = cursor + child_cells * 4;
            let size_at = parent_at + PARENT_CELLS * 4;
            // WHY the low word comes first here: the parent address and the size
            // are each written least-significant cell first, while the child
            // address is written most-significant cell last. This is not a
            // guess. On QEMU's `virt` machine the 84-byte property decodes as
            // 0x3EFF0000+0x10000, 0x10000000+0x2EFF0000 and
            // 0x8000000000+0x800000000 under this order, which are exactly the
            // PCI configuration, 32-bit MMIO and wide-BAR windows the machine
            // builds; the other order yields a 16 TB size on the first entry.
            // The same tree's `reg` property uses the opposite order, so the two
            // cannot be read with one shared helper.
            let base = if PARENT_CELLS == 2 {
                (cell(raw, parent_at) << 32) | cell(raw, parent_at + 4)
            } else {
                cell(raw, parent_at)
            };
            let size = if size_cells == 2 {
                (cell(raw, size_at) << 32) | cell(raw, size_at + 4)
            } else {
                cell(raw, size_at)
            };
            if size != 0 && base.checked_add(size).is_some() {
                windows[count] = PciWindow { base, size };
                count += 1;
            }
        }
        cursor += stride;
    }

    // Lowest window first, so the first candidate found is the lowest address
    // the platform published.
    for i in 1..count {
        let mut j = i;
        while j > 0 && windows[j - 1].base > windows[j].base {
            windows.swap(j - 1, j);
            j -= 1;
        }
    }
    (windows, count)
}

fn read_cell_count(property: &[u8]) -> Option<usize> {
    if property.len() < 4 {
        return None;
    }
    Some(u32::from_be_bytes([
        property[0],
        property[1],
        property[2],
        property[3],
    ]) as usize)
}

/// Hands out addresses for unassigned memory BARs from the bridge's windows.
pub struct BarAllocator {
    windows: [PciWindow; MAX_WINDOWS],
    window_count: usize,
    /// Regions already handed out or written into a BAR.
    used: [(u64, u64); MAX_TRACKED],
    used_count: usize,
    /// Regions that turned out to be unusable, so they are not offered again.
    rejected: [(u64, u64); MAX_TRACKED],
    rejected_count: usize,
    /// RAM banks from the device tree, which a BAR must never land in.
    ram: [(u64, u64); MAX_RAM_BANKS],
    ram_count: usize,
    /// The ECAM window, which a BAR must never land in: a device decoded there
    /// would answer to configuration-space accesses.
    ecam: (u64, u64),
}

impl BarAllocator {
    pub const fn new() -> Self {
        Self {
            windows: [PciWindow { base: 0, size: 0 }; MAX_WINDOWS],
            window_count: 0,
            used: [(0, 0); MAX_TRACKED],
            used_count: 0,
            rejected: [(0, 0); MAX_TRACKED],
            rejected_count: 0,
            ram: [(0, 0); MAX_RAM_BANKS],
            ram_count: 0,
            ecam: (0, 0),
        }
    }

    /// Records the memory windows the host bridge publishes.
    pub fn set_windows(&mut self, windows: [PciWindow; MAX_WINDOWS], count: usize) {
        self.windows = windows;
        self.window_count = count.min(MAX_WINDOWS);
    }

    /// Marks a RAM bank as untouchable.
    pub fn reserve_ram(&mut self, base: u64, size: u64) {
        if self.ram_count < MAX_RAM_BANKS && size != 0 {
            self.ram[self.ram_count] = (base, base.saturating_add(size));
            self.ram_count += 1;
        }
    }

    /// Marks the ECAM window as untouchable.
    pub fn reserve_ecam(&mut self, base: u64, size: u64) {
        self.ecam = (base, base.saturating_add(size));
    }

    /// True when the device tree published at least one usable window.
    pub fn has_window(&self) -> bool {
        self.window_count > 0
    }

    fn overlaps(list: &[(u64, u64)], count: usize, base: u64, size: u64) -> bool {
        let end = base.saturating_add(size);
        for index in 0..count {
            let (low, high) = list[index];
            if base < high && low < end {
                return true;
            }
        }
        false
    }

    fn is_usable(&self, base: u64, size: u64) -> bool {
        if Self::overlaps(&self.ram, self.ram_count, base, size) {
            return false;
        }
        if self.ecam.1 != 0 && Self::overlaps(&[self.ecam], 1, base, size) {
            return false;
        }
        if Self::overlaps(&self.used, self.used_count, base, size) {
            return false;
        }
        if Self::overlaps(&self.rejected, self.rejected_count, base, size) {
            return false;
        }
        true
    }

    /// The lowest address of `size` bytes inside a published window that
    /// collides with nothing.
    ///
    /// WHY the lowest: it keeps the assignment deterministic and re-running it
    /// produces the same layout, and it leaves the rest of the window free for
    /// later BARs. WHY only power-of-two sizes: the size mask of a BAR can only
    /// express one, so any other value is already a decoding mistake.
    pub fn allocate(&self, size: u64) -> Option<u64> {
        if size == 0 || !size.is_power_of_two() || size > MAX_REACHABLE {
            return None;
        }
        for index in 0..self.window_count {
            let window = self.windows[index];
            if window.size < size {
                continue;
            }
            let limit = window.end();
            // A BAR must be aligned to its own size or the device decodes at an
            // offset the mask says does not exist.
            let mut candidate = window.base.saturating_add(size - 1) & !(size - 1);
            while candidate.saturating_add(size) <= limit {
                let end = candidate.saturating_add(size);
                if end > MAX_REACHABLE {
                    break;
                }
                if self.is_usable(candidate, size) {
                    return Some(candidate);
                }
                candidate = candidate.saturating_add(size);
            }
        }
        None
    }

    /// Records a region that was handed out, so nothing else is given the same
    /// address.
    pub fn commit(&mut self, base: u64, size: u64) {
        if self.used_count < MAX_TRACKED {
            self.used[self.used_count] = (base, base.saturating_add(size));
            self.used_count += 1;
        }
    }

    /// Records a region that could not be used, so it is not offered again.
    pub fn reject(&mut self, base: u64, size: u64) {
        if self.rejected_count < MAX_TRACKED {
            self.rejected[self.rejected_count] = (base, base.saturating_add(size));
            self.rejected_count += 1;
        }
    }

    /// Maps a BAR so its registers are reachable, refusing any block that would
    /// newly shadow RAM.
    pub fn map_region(&self, base: u64, size: u64) -> bool {
        if size == 0 {
            return false;
        }
        // WHY round outwards to the granule: a BAR is a few kilobytes and is
        // almost never 2 MiB aligned, so the tables can only describe the blocks
        // that cover it.
        let map_base = base & !(BLOCK - 1);
        let map_end = base
            .saturating_add(size)
            .saturating_add(BLOCK - 1)
            & !(BLOCK - 1);
        let (ram_lo, ram_hi) = self.ram_span();
        ArmMmu::map_device_blocks(map_base, map_end - map_base, ram_lo, ram_hi)
    }

    /// Lowest RAM base and highest RAM end, as one span.
    fn ram_span(&self) -> (u64, u64) {
        let mut low = u64::MAX;
        let mut high = 0u64;
        for index in 0..self.ram_count {
            let (base, end) = self.ram[index];
            if base < low {
                low = base;
            }
            if end > high {
                high = end;
            }
        }
        if low == u64::MAX {
            (0, 0)
        } else {
            (low, high)
        }
    }
}

/// Assigns every unassigned memory BAR of one device.
///
/// Returns the number of BARs that were given an address. A BAR that cannot be
/// sized, or for which no window has room, is reported rather than skipped in
/// silence.
pub unsafe fn assign_device_bars(
    ecam: &PcieEcam,
    device: &PcieDevice,
    allocator: &mut BarAllocator,
) -> u32 {
    let (bus, dev, func) = device.address();
    let mut assigned = 0u32;

    for bar in 0..6usize {
        let probe = ecam.probe_bar(bus, dev, func, bar);
        if !probe.is_memory || probe.size == 0 {
            // Not implemented, or a port I/O BAR. Neither can be placed in a
            // memory window, and this platform publishes no I/O window.
            continue;
        }
        if probe.address != 0 {
            // Something already programmed it; leave it alone.
            continue;
        }

        let mut placed = None;
        // Each attempt either maps cleanly or marks the candidate unusable, so
        // the loop terminates after at most one pass over the window.
        loop {
            let candidate = match allocator.allocate(probe.size) {
                Some(candidate) => candidate,
                None => break,
            };
            if allocator.map_region(candidate, probe.size) {
                placed = Some(candidate);
                break;
            }
            crate::kprintln!(
                "[PCI] bar {} of {:04x}:{:04x} could not be mapped at 0x{:X}; trying the next address.",
                bar,
                device.vendor_id,
                device.device_id,
                candidate
            );
            allocator.reject(candidate, probe.size);
        }

        let address = match placed {
            Some(address) => address,
            None => {
                crate::kprintln!(
                    "[PCI] bar {} of {:04x}:{:04x} ({} bytes) left unassigned: no published window has room for it.",
                    bar,
                    device.vendor_id,
                    device.device_id,
                    probe.size
                );
                continue;
            }
        };

        allocator.commit(address, probe.size);
        let read_back = ecam.write_bar_address(bus, dev, func, bar, address, probe.is_64bit);
        if read_back != address {
            crate::kprintln!(
                "[PCI] bar {} of {:04x}:{:04x} did not accept 0x{:X}; it reads back 0x{:X}.",
                bar,
                device.vendor_id,
                device.device_id,
                address,
                read_back
            );
            allocator.reject(address, probe.size);
            continue;
        }
        // Memory space and bus mastering must be on before the device's own
        // registers answer at the address just programmed.
        ecam.enable_memory_and_bus_master(bus, dev, func);
        assigned += 1;
        crate::kprintln!(
            "[PCI] {:04x}:{:04x} bar{} assigned 0x{:X} ({} bytes) from the host bridge ranges.",
            device.vendor_id,
            device.device_id,
            bar,
            address,
            probe.size
        );
    }
    assigned
}