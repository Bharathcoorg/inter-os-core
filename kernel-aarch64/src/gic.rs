// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! ARM Generic Interrupt Controller (GICv2) Driver for AArch64
//!
//! Provides bare-metal hardware interrupt routing, CPU core affinity,
//! and priority masking for ARM Cortex-A mobile platforms:
//! - QEMU `virt`: GICD @ `0x08000000`, GICC @ `0x08010000`
//! - PinePhone A64 (Allwinner A64): GICD @ `0x01c81000`, GICC @ `0x01c82000`

use crate::fdt::ArmFdtParser;
use alloc::vec::Vec;

pub const DEFAULT_GICD_BASE: usize = 0x08000000;
pub const DEFAULT_GICC_BASE: usize = 0x08010000;

pub const PINEPHONE_GICD_BASE: usize = 0x01c81000;
pub const PINEPHONE_GICC_BASE: usize = 0x01c82000;

pub const GICD_CTLR: usize = 0x000;
pub const GICD_TYPER: usize = 0x004;
pub const GICD_ISENABLER: usize = 0x100;
pub const GICD_ICENABLER: usize = 0x180;
pub const GICD_IPRIORITYR: usize = 0x400;
pub const GICD_ITARGETSR: usize = 0x800;
pub const GICD_SGIR: usize = 0xF00;

pub const GICC_CTLR: usize = 0x000;
pub const GICC_PMR: usize = 0x004;
pub const GICC_BPR: usize = 0x008;
pub const GICC_IAR: usize = 0x00C;
pub const GICC_EOIR: usize = 0x010;

pub const GIC_SPURIOUS_IRQ: u32 = 1023;

/// What the platform description says the interrupt controller actually is.
///
/// GICv2 exposes the distributor and CPU interface as two separate windows of
/// one node. GICv2m and GICv3 reuse the same conceptual device but have a
/// different register model, so they must not be driven through the GICv2
/// offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntcDescription {
    Gicv2 {
        distributor: usize,
        cpu_interface: usize,
    },
    Gicv3 {
        distributor: usize,
        distributor_size: usize,
        redistributors: usize,
        redistributors_size: usize,
    },
    Unsupported(&'static str),
    Absent,
}

/// Reads the `reg` property of a node as a list of `(address, size)` pairs.
fn reg_windows_with_sizes(parser: &ArmFdtParser, node: &crate::fdt::ArmFdtNode) -> Option<Vec<(usize, usize)>> {
    let reg = parser.get_node_property(node, "reg").ok()?;
    if reg.is_empty() {
        return None;
    }

    let mut address_cells = cell_count(parser, node, "#address-cells").unwrap_or(0);
    let mut size_cells = cell_count(parser, node, "#size-cells").unwrap_or(0);

    // If cell counts are not defined on the node itself, deduce them from the layout.
    // In Devicetree, parent bus nodes define #address-cells and #size-cells.
    if address_cells == 0 || size_cells == 0 {
        if reg.len() >= 32 && reg[0..4] == [0, 0, 0, 0] {
            // 64-bit addresses and sizes (QEMU virt 64-bit bus)
            address_cells = 2;
            size_cells = 2;
        } else if reg.len() >= 16 {
            // 32-bit addresses and sizes (Allwinner A64 32-bit SoC bus)
            address_cells = 1;
            size_cells = 1;
        } else {
            address_cells = 1;
            size_cells = 1;
        }
    }

    if address_cells == 0 || address_cells > 2 || size_cells == 0 || size_cells > 2 {
        return None;
    }
    let stride = (address_cells + size_cells) * 4;
    if stride == 0 {
        return None;
    }

    let mut windows = Vec::new();
    let mut cursor = 0usize;
    while cursor + stride <= reg.len() {
        let base = match address_cells {
            1 => u32::from_be_bytes(reg[cursor..cursor + 4].try_into().ok()?) as usize,
            _ => u64::from_be_bytes(reg[cursor..cursor + 8].try_into().ok()?) as usize,
        };
        let size_offset = cursor + address_cells * 4;
        let size = match size_cells {
            1 => u32::from_be_bytes(reg[size_offset..size_offset + 4].try_into().ok()?) as usize,
            _ => u64::from_be_bytes(reg[size_offset..size_offset + 8].try_into().ok()?) as usize,
        };
        windows.push((base, size));
        cursor += stride;
    }

    // Direct fallback for 32-bit (addr, size) pairs
    if windows.len() >= 2 && windows[1].0 < 0x10000 && reg.len() >= 16 {
        let gicd = u32::from_be_bytes([reg[0], reg[1], reg[2], reg[3]]) as usize;
        let gicc = u32::from_be_bytes([reg[8], reg[9], reg[10], reg[11]]) as usize;
        if gicd >= 0x0010_0000 && gicc >= 0x0010_0000 {
            let gicd_sz = u32::from_be_bytes([reg[4], reg[5], reg[6], reg[7]]) as usize;
            let gicc_sz = u32::from_be_bytes([reg[12], reg[13], reg[14], reg[15]]) as usize;
            return Some(alloc::vec![(gicd, gicd_sz), (gicc, gicc_sz)]);
        }
    }

    if windows.is_empty() {
        None
    } else {
        Some(windows)
    }
}

/// Reads the `reg` property of a node as a list of base addresses.
fn reg_windows(parser: &ArmFdtParser, node: &crate::fdt::ArmFdtNode) -> Option<Vec<usize>> {
    reg_windows_with_sizes(parser, node).map(|windows| windows.into_iter().map(|(base, _)| base).collect())
}

/// Decodes a cell-count property, accepting the 1- and 2-cell encodings.
fn cell_count(parser: &ArmFdtParser, node: &crate::fdt::ArmFdtNode, prop: &str) -> Option<usize> {
    let p = parser.get_node_property(node, prop).ok()?;
    match p.len() {
        4 => Some(u32::from_be_bytes([p[0], p[1], p[2], p[3]]) as usize),
        _ => None,
    }
}

/// Identifies the interrupt controller the platform declares.
///
/// Returns [`IntcDescription::Unsupported`] for a recognised controller whose
/// register model this driver does not implement, so the caller can decline to
/// program it rather than issuing GICv2 writes to incompatible registers.
pub fn describe_interrupt_controller(parser: &ArmFdtParser) -> IntcDescription {
    // The spec identifies an interrupt controller by the presence of the
    // `interrupt-controller` property; the vendor string then says which one.
    let Ok(node) = parser.find_node_by_property("interrupt-controller") else {
        return IntcDescription::Absent;
    };

    // A GICv2m platform describes the CPU interface with a GICv2-compatible
    // string and puts the distributor in a nested `v2m@` frame, so the vendor
    // string alone would misclassify it. The nested frame is authoritative.
    for frame in ["arm,gic-v2m", "arm,gic-v2m-frame"] {
        if parser.find_node_by_compatible(frame).is_ok() {
            return IntcDescription::Unsupported("arm,gic-v2m");
        }
    }
    for frame in ["arm,gic-v3", "arm,gic-v3-frame"] {
        if let Ok(v3_node) = parser.find_node_by_compatible(frame) {
            match reg_windows_with_sizes(parser, &v3_node) {
                Some(windows) if windows.len() >= 2 => {
                    return IntcDescription::Gicv3 {
                        distributor: windows[0].0,
                        distributor_size: windows[0].1,
                        redistributors: windows[1].0,
                        redistributors_size: windows[1].1,
                    };
                }
                _ => return IntcDescription::Unsupported("arm,gic-v3"),
            }
        }
    }

    let mut model = "";
    if let Ok(p) = parser.get_node_property(&node, "compatible") {
        let mut cursor = 0usize;
        while cursor < p.len() {
            let mut end = cursor;
            while end < p.len() && p[end] != 0 {
                end += 1;
            }
            if let Ok(text) = core::str::from_utf8(&p[cursor..end]) {
                if let Some(canonical) = classify(text) {
                    model = canonical;
                    break;
                }
            }
            cursor = end + 1;
        }
    }

    if is_gicv2_compatible(model) {
        match reg_windows(parser, &node) {
            // GICv2 publishes the distributor and the CPU interface as two
            // windows of the same node.
            Some(bases) if bases.len() >= 2 => IntcDescription::Gicv2 {
                distributor: bases[0],
                cpu_interface: bases[1],
            },
            _ => IntcDescription::Unsupported("arm,gic-400 without a CPU interface window"),
        }
    } else if model == "arm,gic-v3" {
        match reg_windows_with_sizes(parser, &node) {
            Some(windows) if windows.len() >= 2 => IntcDescription::Gicv3 {
                distributor: windows[0].0,
                distributor_size: windows[0].1,
                redistributors: windows[1].0,
                redistributors_size: windows[1].1,
            },
            _ => IntcDescription::Unsupported("arm,gic-v3 without redistributor windows"),
        }
    } else if is_other_gic_compatible(model) {
        IntcDescription::Unsupported(model)
    } else {
        IntcDescription::Unsupported("unrecognised interrupt controller")
    }
}

/// Maps a `compatible` entry onto the canonical model name this driver knows.
///
/// Returns a `'static` string so the classification outlives the parsed tree.
fn classify(text: &str) -> Option<&'static str> {
    match text {
        // GICv2: distributor and CPU interface in one node.
        "arm,gic-400" | "arm,cortex-a15-gic" | "arm,cortex-a9-gic" | "arm,arm11mp-gic" => {
            Some("arm,gic-400")
        }
        // Same device family, different register model.
        "arm,gic-v2m" | "arm,gic-v2m-frame" => Some("arm,gic-v2m"),
        "arm,gic-v3" | "arm,gic-v3-frame" => Some("arm,gic-v3"),
        _ => None,
    }
}

/// Distributor and CPU interface in one node, as GICv2 declares them.
fn is_gicv2_compatible(text: &str) -> bool {
    matches!(
        text,
        "arm,gic-400" | "arm,cortex-a15-gic" | "arm,cortex-a9-gic" | "arm,arm11mp-gic"
    )
}

/// GIC variants that share the distributor window but not the register model.
fn is_other_gic_compatible(text: &str) -> bool {
    matches!(
        text,
        "arm,gic-v2m" | "arm,gic-v2m-frame" | "arm,gic-v3" | "arm,gic-v3-frame"
    )
}

#[derive(Debug, Clone, Copy)]
pub struct Gicv2 {
    gicd_base: usize,
    gicc_base: usize,
}

impl Gicv2 {
    pub const fn new(gicd_base: usize, gicc_base: usize) -> Self {
        Self {
            gicd_base,
            gicc_base,
        }
    }

    #[inline(always)]
    unsafe fn gicd_read(&self, offset: usize) -> u32 {
        let ptr = (self.gicd_base + offset) as *const u32;
        core::ptr::read_volatile(ptr)
    }

    #[inline(always)]
    unsafe fn gicd_write(&self, offset: usize, val: u32) {
        let ptr = (self.gicd_base + offset) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }

    #[inline(always)]
    unsafe fn gicc_read(&self, offset: usize) -> u32 {
        let ptr = (self.gicc_base + offset) as *const u32;
        core::ptr::read_volatile(ptr)
    }

    #[inline(always)]
    unsafe fn gicc_write(&self, offset: usize, val: u32) {
        let ptr = (self.gicc_base + offset) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }

    pub unsafe fn init(&self) {
        if self.gicd_base == 0 || self.gicc_base == 0 {
            return;
        }

        self.gicd_write(GICD_CTLR, 0);

        let typer = self.gicd_read(GICD_TYPER);
        let it_lines = (typer & 0x1F) + 1;
        let max_irqs = it_lines * 32;

        for i in (32..max_irqs).step_by(32) {
            let reg_idx = (i / 32) as usize;
            self.gicd_write(GICD_ICENABLER + reg_idx * 4, 0xFFFFFFFF);
        }

        for i in (0..max_irqs).step_by(4) {
            let reg_idx = (i / 4) as usize;
            self.gicd_write(GICD_IPRIORITYR + reg_idx * 4, 0xA0A0A0A0);
        }

        for i in (32..max_irqs).step_by(4) {
            let reg_idx = (i / 4) as usize;
            self.gicd_write(GICD_ITARGETSR + reg_idx * 4, 0x01010101);
        }

        self.gicd_write(GICD_CTLR, 0x03);

        self.init_cpu_interface();
    }

    /// Initializes the banked GICv2 per-CPU distributor PPI/SGI priorities & CPU interface.
    /// Must be called on CPU 0 (via `init()`) and on each secondary CPU upon boot.
    pub unsafe fn init_cpu_interface(&self) {
        if self.gicd_base != 0 {
            // Configure banked SGI (0..15) and PPI (16..31) priorities
            for reg_idx in 0..8usize {
                self.gicd_write(GICD_IPRIORITYR + reg_idx * 4, 0xA0A0A0A0);
            }
            // Enable SGIs (0..15) on this PE's banked GICD_ISENABLER0
            self.gicd_write(GICD_ISENABLER, 0x0000_FFFF);
        }
        if self.gicc_base != 0 {
            self.gicc_write(GICC_PMR, 0xFF);
            self.gicc_write(GICC_BPR, 0x00);
            self.gicc_write(GICC_CTLR, 0x03);
        }
    }

    pub fn enable_irq(&self, irq: u32) {
        if self.gicd_base != 0 {
            let reg_offset = GICD_ISENABLER + ((irq / 32) as usize) * 4;
            let bit = 1 << (irq % 32);
            unsafe {
                self.gicd_write(reg_offset, bit);
            }
        }
    }

    pub fn disable_irq(&self, irq: u32) {
        if self.gicd_base != 0 {
            let reg_offset = GICD_ICENABLER + ((irq / 32) as usize) * 4;
            let bit = 1 << (irq % 32);
            unsafe {
                self.gicd_write(reg_offset, bit);
            }
        }
    }

    pub fn acknowledge_irq(&self) -> u32 {
        if self.gicc_base != 0 {
            unsafe { self.gicc_read(GICC_IAR) & 0x3FF }
        } else {
            GIC_SPURIOUS_IRQ
        }
    }

    pub fn end_of_interrupt(&self, irq: u32) {
        if self.gicc_base != 0 {
            unsafe {
                self.gicc_write(GICC_EOIR, irq & 0x3FF);
            }
        }
    }
}

/// GIC hardware and configuration errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GicError {
    DistributorRwpTimeout,
    RedistributorNotFound,
    RedistributorSleepTimeout,
    InvalidMmioRange,
    TruncatedRedistributorWindow,
    OutOfBoundsMmioAccess,
    MisalignedMmioAccess,
    InvalidParameter,
    RegistrationConflict,
    DuplicateAffinity,
    UnknownPeAffinity,
    PeTableFull,
}

/// Pure MMIO window boundary and alignment validator.
///
/// Enforces:
/// - Non-null base (`base != 0`) and non-zero size (`size != 0`)
/// - Valid access width (4 or 8 bytes)
/// - Strict natural alignment (`offset % width == 0`)
/// - Strict bounds checking: `offset + width <= size` (with overflow check)
/// - Target address overflow prevention: `base + offset` does not overflow
pub fn validate_mmio_window(
    base: usize,
    size: usize,
    offset: usize,
    width: usize,
) -> Result<usize, GicError> {
    if base == 0 || size == 0 {
        return Err(GicError::InvalidMmioRange);
    }
    base.checked_add(size).ok_or(GicError::InvalidMmioRange)?;
    if width != 4 && width != 8 {
        return Err(GicError::InvalidParameter);
    }
    if base % width != 0 || offset % width != 0 {
        return Err(GicError::MisalignedMmioAccess);
    }
    let end = offset.checked_add(width).ok_or(GicError::OutOfBoundsMmioAccess)?;
    if end > size {
        return Err(GicError::OutOfBoundsMmioAccess);
    }
    let target_addr = base.checked_add(offset).ok_or(GicError::OutOfBoundsMmioAccess)?;
    if target_addr % width != 0 {
        return Err(GicError::MisalignedMmioAccess);
    }
    Ok(target_addr)
}

pub const MAX_GIC_PES: usize = 8;
pub const STATE_EMPTY: u8 = 0;
pub const STATE_WRITING: u8 = 1;
pub const STATE_VALID: u8 = 2;

static REGISTRATION_LOCK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// IRQ-safe RAII guard for PE registration serialization.
///
/// Masks local IRQs on AArch64 while holding `REGISTRATION_LOCK` to prevent
/// recursive deadlock if an interrupt occurs during registration. All lookups
/// (`logical_id_for_affinity`, `get_pe_redistributor`, etc.) remain 100%
/// lock-free and wait-free, safe for interrupt handlers.
struct RegistrationLockGuard {
    #[cfg(target_arch = "aarch64")]
    saved_daif: u64,
}

impl RegistrationLockGuard {
    fn acquire() -> Self {
        #[cfg(target_arch = "aarch64")]
        let saved_daif: u64 = unsafe {
            let daif: u64;
            core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
            core::arch::asm!("msr daifset, #2", options(nomem, nostack));
            daif
        };
        while REGISTRATION_LOCK.compare_exchange(
            false,
            true,
            core::sync::atomic::Ordering::Acquire,
            core::sync::atomic::Ordering::Relaxed,
        ).is_err() {
            core::hint::spin_loop();
        }
        Self {
            #[cfg(target_arch = "aarch64")]
            saved_daif,
        }
    }
}

impl Drop for RegistrationLockGuard {
    fn drop(&mut self) {
        REGISTRATION_LOCK.store(false, core::sync::atomic::Ordering::Release);
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("msr daif, {}", in(reg) self.saved_daif, options(nomem, nostack));
        }
    }
}

static PE_STATE: [core::sync::atomic::AtomicU8; MAX_GIC_PES] = [
    core::sync::atomic::AtomicU8::new(STATE_EMPTY),
    core::sync::atomic::AtomicU8::new(STATE_EMPTY),
    core::sync::atomic::AtomicU8::new(STATE_EMPTY),
    core::sync::atomic::AtomicU8::new(STATE_EMPTY),
    core::sync::atomic::AtomicU8::new(STATE_EMPTY),
    core::sync::atomic::AtomicU8::new(STATE_EMPTY),
    core::sync::atomic::AtomicU8::new(STATE_EMPTY),
    core::sync::atomic::AtomicU8::new(STATE_EMPTY),
];
static PE_REDISTRIBUTOR_MAP: [core::sync::atomic::AtomicUsize; MAX_GIC_PES] = [
    core::sync::atomic::AtomicUsize::new(0),
    core::sync::atomic::AtomicUsize::new(0),
    core::sync::atomic::AtomicUsize::new(0),
    core::sync::atomic::AtomicUsize::new(0),
    core::sync::atomic::AtomicUsize::new(0),
    core::sync::atomic::AtomicUsize::new(0),
    core::sync::atomic::AtomicUsize::new(0),
    core::sync::atomic::AtomicUsize::new(0),
];
static PE_MPIDR_MAP: [core::sync::atomic::AtomicU64; MAX_GIC_PES] = [
    core::sync::atomic::AtomicU64::new(u64::MAX),
    core::sync::atomic::AtomicU64::new(u64::MAX),
    core::sync::atomic::AtomicU64::new(u64::MAX),
    core::sync::atomic::AtomicU64::new(u64::MAX),
    core::sync::atomic::AtomicU64::new(u64::MAX),
    core::sync::atomic::AtomicU64::new(u64::MAX),
    core::sync::atomic::AtomicU64::new(u64::MAX),
    core::sync::atomic::AtomicU64::new(u64::MAX),
];
static PE_ONLINE: [core::sync::atomic::AtomicBool; MAX_GIC_PES] = [
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
];

/// Canonicalizes an MPIDR_EL1 value by preserving all 4 architectural affinity levels
/// (Aff3[39:32], Aff2[23:16], Aff1[15:8], Aff0[7:0]) and masking non-affinity bits.
pub const fn canonical_affinity_key(mpidr: u64) -> u64 {
    let (aff3, aff2, aff1, aff0) = decode_mpidr_affinity(mpidr);
    ((aff3 as u64) << 32) | ((aff2 as u64) << 16) | ((aff1 as u64) << 8) | (aff0 as u64)
}

/// Converts a raw MPIDR_EL1 value into the 32-bit GICR_TYPER affinity encoding:
/// Aff3[31:24] | Aff2[23:16] | Aff1[15:8] | Aff0[7:0].
pub const fn mpidr_to_gicr_affinity(mpidr: u64) -> u32 {
    let (aff3, aff2, aff1, aff0) = decode_mpidr_affinity(mpidr);
    ((aff3 as u32) << 24) | ((aff2 as u32) << 16) | ((aff1 as u32) << 8) | (aff0 as u32)
}

/// Converts a 32-bit GICR_TYPER affinity value into canonical 64-bit MPIDR affinity format.
pub const fn gicr_affinity_to_mpidr(gicr_aff: u32) -> u64 {
    let aff3 = ((gicr_aff >> 24) & 0xFF) as u64;
    let aff2 = ((gicr_aff >> 16) & 0xFF) as u64;
    let aff1 = ((gicr_aff >> 8) & 0xFF) as u64;
    let aff0 = (gicr_aff & 0xFF) as u64;
    (aff3 << 32) | (aff2 << 16) | (aff1 << 8) | aff0
}

pub fn reset_pe_redistributor_map_for_test() {
    let _guard = RegistrationLockGuard::acquire();
    for i in 0..MAX_GIC_PES {
        PE_MPIDR_MAP[i].store(u64::MAX, core::sync::atomic::Ordering::Relaxed);
        PE_REDISTRIBUTOR_MAP[i].store(0, core::sync::atomic::Ordering::Relaxed);
        PE_ONLINE[i].store(false, core::sync::atomic::Ordering::Relaxed);
        PE_STATE[i].store(STATE_EMPTY, core::sync::atomic::Ordering::Release);
        PE_TIMER_INTERRUPT_COUNT[i].store(0, core::sync::atomic::Ordering::Relaxed);
        PE_SGI_INTERRUPT_COUNT[i].store(0, core::sync::atomic::Ordering::Relaxed);
    }
    SGI_INTERRUPT_COUNT.store(0, core::sync::atomic::Ordering::Relaxed);
}

/// Registers a Processing Element at an explicit logical ID with its full MPIDR affinity
/// and GICv3 redistributor MMIO base address.
///
/// Rejects duplicate hardware affinities mapped to different logical IDs (`DuplicateAffinity`),
/// conflicting redistributor bases or occupied slots (`RegistrationConflict`), and out-of-range
/// logical IDs (`InvalidParameter`). Guarantees idempotent re-registration when all fields match.
pub fn register_pe(logical_id: usize, mpidr: u64, rd_base: usize) -> Result<(), GicError> {
    if logical_id >= MAX_GIC_PES || rd_base == 0 {
        return Err(GicError::InvalidParameter);
    }
    let key = canonical_affinity_key(mpidr);
    let _guard = RegistrationLockGuard::acquire();

    // 1. Inspect existing valid registrations for duplicate affinity or slot collision
    for i in 0..MAX_GIC_PES {
        if PE_STATE[i].load(core::sync::atomic::Ordering::Acquire) == STATE_VALID {
            let existing_key = PE_MPIDR_MAP[i].load(core::sync::atomic::Ordering::Relaxed);
            let existing_base = PE_REDISTRIBUTOR_MAP[i].load(core::sync::atomic::Ordering::Relaxed);
            if existing_key == key {
                if i != logical_id {
                    return Err(GicError::DuplicateAffinity);
                }
                return if existing_base == rd_base {
                    Ok(())
                } else {
                    Err(GicError::RegistrationConflict)
                };
            }
            if i == logical_id {
                return Err(GicError::RegistrationConflict);
            }
        }
    }

    // 2. Claim the designated logical slot
    if PE_STATE[logical_id].load(core::sync::atomic::Ordering::Relaxed) != STATE_EMPTY {
        return Err(GicError::RegistrationConflict);
    }
    PE_STATE[logical_id].store(STATE_WRITING, core::sync::atomic::Ordering::Relaxed);
    PE_MPIDR_MAP[logical_id].store(key, core::sync::atomic::Ordering::Relaxed);
    PE_REDISTRIBUTOR_MAP[logical_id].store(rd_base, core::sync::atomic::Ordering::Relaxed);
    PE_STATE[logical_id].store(STATE_VALID, core::sync::atomic::Ordering::Release);
    Ok(())
}

/// Registers a PE by 32-bit GICR affinity, automatically allocating the next free logical slot.
pub fn register_pe_redistributor(affinity: u32, rd_base: usize) -> Result<(), GicError> {
    if rd_base == 0 {
        return Err(GicError::InvalidParameter);
    }
    let key = gicr_affinity_to_mpidr(affinity);
    let _guard = RegistrationLockGuard::acquire();

    // 1. Check if this affinity is already registered
    for i in 0..MAX_GIC_PES {
        if PE_STATE[i].load(core::sync::atomic::Ordering::Acquire) == STATE_VALID {
            if PE_MPIDR_MAP[i].load(core::sync::atomic::Ordering::Relaxed) == key {
                if PE_REDISTRIBUTOR_MAP[i].load(core::sync::atomic::Ordering::Relaxed) == rd_base {
                    return Ok(());
                } else {
                    return Err(GicError::RegistrationConflict);
                }
            }
        }
    }

    // 2. Claim first empty slot
    for i in 0..MAX_GIC_PES {
        if PE_STATE[i].load(core::sync::atomic::Ordering::Relaxed) == STATE_EMPTY {
            PE_STATE[i].store(STATE_WRITING, core::sync::atomic::Ordering::Relaxed);
            PE_MPIDR_MAP[i].store(key, core::sync::atomic::Ordering::Relaxed);
            PE_REDISTRIBUTOR_MAP[i].store(rd_base, core::sync::atomic::Ordering::Relaxed);
            PE_STATE[i].store(STATE_VALID, core::sync::atomic::Ordering::Release);
            return Ok(());
        }
    }

    Err(GicError::PeTableFull)
}

/// Lock-free, wait-free lookup resolving full MPIDR affinity (Aff3..Aff0) to its logical CPU ID.
pub fn logical_id_for_affinity(mpidr: u64) -> Result<usize, GicError> {
    let key = canonical_affinity_key(mpidr);
    for i in 0..MAX_GIC_PES {
        if PE_STATE[i].load(core::sync::atomic::Ordering::Acquire) == STATE_VALID {
            if PE_MPIDR_MAP[i].load(core::sync::atomic::Ordering::Relaxed) == key {
                return Ok(i);
            }
        }
    }
    Err(GicError::UnknownPeAffinity)
}

pub fn get_pe_redistributor_by_logical_id(logical_id: usize) -> Option<usize> {
    if logical_id >= MAX_GIC_PES {
        return None;
    }
    if PE_STATE[logical_id].load(core::sync::atomic::Ordering::Acquire) == STATE_VALID {
        let base = PE_REDISTRIBUTOR_MAP[logical_id].load(core::sync::atomic::Ordering::Relaxed);
        if base != 0 {
            return Some(base);
        }
    }
    None
}

pub fn get_pe_mpidr_by_logical_id(logical_id: usize) -> Option<u64> {
    if logical_id >= MAX_GIC_PES {
        return None;
    }
    if PE_STATE[logical_id].load(core::sync::atomic::Ordering::Acquire) == STATE_VALID {
        let key = PE_MPIDR_MAP[logical_id].load(core::sync::atomic::Ordering::Relaxed);
        if key != u64::MAX {
            return Some(key);
        }
    }
    None
}

pub fn get_pe_redistributor(affinity: u32) -> Option<usize> {
    let key = gicr_affinity_to_mpidr(affinity);
    if let Ok(logical_id) = logical_id_for_affinity(key) {
        return get_pe_redistributor_by_logical_id(logical_id);
    }
    None
}

pub fn set_pe_online(logical_id: usize, online: bool) {
    if logical_id < MAX_GIC_PES {
        PE_ONLINE[logical_id].store(online, core::sync::atomic::Ordering::Release);
    }
}

pub fn is_pe_online(logical_id: usize) -> bool {
    if logical_id < MAX_GIC_PES {
        PE_ONLINE[logical_id].load(core::sync::atomic::Ordering::Acquire)
    } else {
        false
    }
}

/// GICv3 system-register and memory-mapped driver for modern ARM SoCs (e.g. Qualcomm Snapdragon SM8250).
#[derive(Debug, Clone, Copy)]
pub struct Gicv3 {
    pub distributor_base: usize,
    pub distributor_size: usize,
    pub redistributors_base: usize,
    pub redistributors_size: usize,
    pub current_rd_base: usize,
}

impl Gicv3 {
    pub const fn new(distributor_base: usize, redistributors_base: usize) -> Self {
        Self {
            distributor_base,
            distributor_size: 0x10000,
            redistributors_base,
            redistributors_size: 0x20000, // Default 128 KiB minimum redistributor frame
            current_rd_base: 0,
        }
    }

    pub const fn with_size(distributor_base: usize, redistributors_base: usize, redistributors_size: usize) -> Self {
        Self {
            distributor_base,
            distributor_size: 0x10000,
            redistributors_base,
            redistributors_size,
            current_rd_base: 0,
        }
    }

    pub const fn with_sizes(
        distributor_base: usize,
        distributor_size: usize,
        redistributors_base: usize,
        redistributors_size: usize,
    ) -> Self {
        Self {
            distributor_base,
            distributor_size,
            redistributors_base,
            redistributors_size,
            current_rd_base: 0,
        }
    }

    #[inline(always)]
    pub unsafe fn gicd_read_checked(&self, offset: usize) -> Result<u32, GicError> {
        let target = validate_mmio_window(self.distributor_base, self.distributor_size, offset, 4)?;
        let ptr = target as *const u32;
        Ok(core::ptr::read_volatile(ptr))
    }

    #[inline(always)]
    pub unsafe fn gicd_write_checked(&self, offset: usize, val: u32) -> Result<(), GicError> {
        let target = validate_mmio_window(self.distributor_base, self.distributor_size, offset, 4)?;
        let ptr = target as *mut u32;
        core::ptr::write_volatile(ptr, val);
        #[cfg(target_arch = "aarch64")]
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
        Ok(())
    }

    #[inline(always)]
    pub unsafe fn gicd_read_64_checked(&self, offset: usize) -> Result<u64, GicError> {
        let target = validate_mmio_window(self.distributor_base, self.distributor_size, offset, 8)?;
        let ptr = target as *const u64;
        Ok(core::ptr::read_volatile(ptr))
    }

    #[inline(always)]
    pub unsafe fn gicd_write_64_checked(&self, offset: usize, val: u64) -> Result<(), GicError> {
        let target = validate_mmio_window(self.distributor_base, self.distributor_size, offset, 8)?;
        let ptr = target as *mut u64;
        core::ptr::write_volatile(ptr, val);
        #[cfg(target_arch = "aarch64")]
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
        Ok(())
    }

    #[inline(always)]
    pub unsafe fn gicr_read_checked(&self, offset: usize) -> Result<u32, GicError> {
        let target = validate_mmio_window(self.redistributors_base, self.redistributors_size, offset, 4)?;
        let ptr = target as *const u32;
        Ok(core::ptr::read_volatile(ptr))
    }

    #[inline(always)]
    pub unsafe fn gicr_write_checked(&self, offset: usize, val: u32) -> Result<(), GicError> {
        let target = validate_mmio_window(self.redistributors_base, self.redistributors_size, offset, 4)?;
        let ptr = target as *mut u32;
        core::ptr::write_volatile(ptr, val);
        #[cfg(target_arch = "aarch64")]
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
        Ok(())
    }

    #[inline(always)]
    pub unsafe fn gicr_read_64_checked(&self, offset: usize) -> Result<u64, GicError> {
        let target = validate_mmio_window(self.redistributors_base, self.redistributors_size, offset, 8)?;
        let ptr = target as *const u64;
        Ok(core::ptr::read_volatile(ptr))
    }

    /// Reads the raw hardware `MPIDR_EL1` register of the executing PE.
    pub fn current_raw_mpidr() -> u64 {
        #[allow(unused_mut)]
        let mut mpidr: u64 = 0;
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack));
        }
        mpidr
    }

    /// Resolves the executing PE's full `MPIDR_EL1` affinity to its registered logical CPU ID.
    pub fn current_logical_id() -> Result<usize, GicError> {
        logical_id_for_affinity(Self::current_raw_mpidr())
    }

    /// Reads the current processing element's 32-bit GICR affinity from `MPIDR_EL1`.
    pub fn current_pe_affinity() -> u32 {
        mpidr_to_gicr_affinity(Self::current_raw_mpidr())
    }

    /// Walks the Redistributor frames in MMIO space to find the Redistributor
    /// belonging to the PE with the target affinity.
    ///
    /// Validates MMIO window bounds before every read and rejects zero-sized,
    /// truncated, or misaligned windows. Never falls back to affinity zero.
    pub fn discover_redistributor_for_affinity(&self, target_aff: u32) -> Result<usize, GicError> {
        if self.redistributors_base == 0 || self.redistributors_size < 0x20000 {
            return Err(GicError::InvalidMmioRange);
        }

        let mut cursor = self.redistributors_base;
        loop {
            let offset_from_base = cursor.checked_sub(self.redistributors_base)
                .ok_or(GicError::InvalidMmioRange)?;

            // Verify that GICR_TYPER (8 bytes at offset 0x0008) is inside the declared window
            if offset_from_base.checked_add(0x0010).map_or(true, |end| end > self.redistributors_size) {
                return Err(GicError::TruncatedRedistributorWindow);
            }

            // Read 64-bit GICR_TYPER at offset 0x0008 of current RD_base
            let typer = unsafe { self.gicr_read_64_checked(offset_from_base + 0x0008)? };
            let rdist_aff = (typer >> 32) as u32;

            let has_vlpis = (typer & (1 << 1)) != 0; // Bit 1: VLPIS
            let stride = if has_vlpis { 0x40000 } else { 0x20000 }; // 256 KiB vs 128 KiB
            let is_last = (typer & (1 << 4)) != 0; // Bit 4: Last

            // Validate that the entire frame (RD + SGI [+ VLPI]) fits in the declared window
            if offset_from_base.checked_add(stride).map_or(true, |end| end > self.redistributors_size) {
                return Err(GicError::TruncatedRedistributorWindow);
            }

            if rdist_aff == target_aff {
                return Ok(cursor);
            }

            if is_last {
                return Err(GicError::RedistributorNotFound);
            }

            cursor = cursor.checked_add(stride).ok_or(GicError::InvalidMmioRange)?;
        }
    }

    /// Initializes the Distributor.
    pub unsafe fn init_distributor(&self) -> Result<(), GicError> {
        if self.distributor_base == 0 {
            return Err(GicError::InvalidMmioRange);
        }

        // Set ARE_NS (bit 5), ARE (bit 4), and EnableGrp1NS (bit 1) in GICD_CTLR
        let mut ctlr = self.gicd_read_checked(0x0000)?;
        ctlr |= (1 << 5) | (1 << 4) | (1 << 1);
        self.gicd_write_checked(0x0000, ctlr)?;

        #[cfg(target_arch = "aarch64")]
        // Ensure GICD_CTLR write is visible to GIC before polling RWP
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));

        // Wait for Register Write Pending (RWP, bit 31) to clear
        let mut spins = 0;
        while (self.gicd_read_checked(0x0000)? & (1 << 31)) != 0 {
            core::hint::spin_loop();
            spins += 1;
            if spins >= 100_000 {
                return Err(GicError::DistributorRwpTimeout);
            }
        }

        // Discover number of interrupt lines
        let typer = self.gicd_read_checked(0x0004)?;
        let it_lines = typer & 0x1F;
        let num_irqs = (32 * (it_lines + 1)).min(1020);

        // Set SPIs (32..num_irqs) to Group 1 Non-Secure and default priority
        let num_blocks = (num_irqs + 31) / 32;
        for irq_block in 1..num_blocks {
            self.gicd_write_checked(0x0080 + (irq_block as usize) * 4, 0xFFFF_FFFF)?;
        }
        let num_prio_regs = (num_irqs + 3) / 4;
        for prio_reg in 8..num_prio_regs {
            self.gicd_write_checked(0x0400 + (prio_reg as usize) * 4, 0xA0A0_A0A0)?;
        }

        #[cfg(target_arch = "aarch64")]
        // Order SPI group/priority writes before CPU interface enablement
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));

        Ok(())
    }

    /// Initializes the Redistributor for the executing PE under a specific logical CPU ID.
    pub unsafe fn init_redistributor_for_logical_id(&mut self, logical_id: usize) -> Result<(), GicError> {
        let pe_mpidr = Self::current_raw_mpidr();
        let pe_aff = mpidr_to_gicr_affinity(pe_mpidr);
        let rd_base = self.discover_redistributor_for_affinity(pe_aff)?;
        self.current_rd_base = rd_base;
        register_pe(logical_id, pe_mpidr, rd_base)?;

        self.wake_and_configure_redistributor(rd_base)
    }

    /// Initializes the Redistributor for the current PE.
    pub unsafe fn init_redistributor(&mut self) -> Result<(), GicError> {
        let pe_aff = Self::current_pe_affinity();
        let rd_base = self.discover_redistributor_for_affinity(pe_aff)?;
        self.current_rd_base = rd_base;
        register_pe_redistributor(pe_aff, rd_base)?;

        self.wake_and_configure_redistributor(rd_base)
    }

    unsafe fn wake_and_configure_redistributor(&self, rd_base: usize) -> Result<(), GicError> {
        let rd_offset = rd_base.checked_sub(self.redistributors_base)
            .ok_or(GicError::InvalidMmioRange)?;

        // Clear ProcessorSleep (bit 1) in GICR_WAKER to wake up Redistributor
        let mut waker = self.gicr_read_checked(rd_offset + 0x0014)?;
        waker &= !(1 << 1);
        self.gicr_write_checked(rd_offset + 0x0014, waker)?;

        #[cfg(target_arch = "aarch64")]
        // Ensure GICR_WAKER write reaches the Redistributor before polling ChildrenAsleep
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));

        // Poll until ChildrenAsleep (bit 2) is cleared
        let mut spins = 0;
        while (self.gicr_read_checked(rd_offset + 0x0014)? & (1 << 2)) != 0 {
            core::hint::spin_loop();
            spins += 1;
            if spins >= 100_000 {
                return Err(GicError::RedistributorSleepTimeout);
            }
        }

        // SGI_base is at rd_base + 0x10000 (64 KiB frame)
        let sgi_offset = rd_offset.checked_add(0x10000).ok_or(GicError::InvalidMmioRange)?;
        // Configure SGIs/PPIs (0..31) to Group 1 Non-Secure
        self.gicr_write_checked(sgi_offset + 0x0080, 0xFFFF_FFFF)?;
        // Set default priority for SGIs and PPIs
        for prio in 0..8 {
            self.gicr_write_checked(sgi_offset + 0x0400 + prio * 4, 0xA0A0_A0A0)?;
        }

        #[cfg(target_arch = "aarch64")]
        // Ensure SGI/PPI configuration is visible before CPU interface activation
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));

        Ok(())
    }

    /// Full generic GICv3 initialization: Distributor, Redistributor, and CPU Interface.
    pub unsafe fn init(&mut self) -> Result<(), GicError> {
        if self.distributor_base == 0 || self.redistributors_base == 0 || self.redistributors_size < 0x20000 {
            return Err(GicError::InvalidMmioRange);
        }

        self.init_distributor()?;
        self.init_redistributor_for_logical_id(0)?;

        #[cfg(target_arch = "aarch64")]
        {
            // Order distributor/redistributor MMIO setup before system-register CPU interface writes
            core::arch::asm!("dsb ish", options(nostack, preserves_flags));
            core::arch::asm!("isb");
        }

        self.init_cpu_interface();
        Ok(())
    }

    /// Enables an interrupt line.
    /// SGI/PPI (0..31) are routed to the calling PE's own Redistributor.
    /// SPI (32..1019) are routed to the shared Distributor.
    pub fn enable_irq(&self, irq: u32) -> Result<(), GicError> {
        if irq >= 1020 {
            return Err(GicError::InvalidParameter);
        }
        if irq < 32 {
            let pe_mpidr = Self::current_raw_mpidr();
            let logical_id = logical_id_for_affinity(pe_mpidr)?;
            let rd_base = get_pe_redistributor_by_logical_id(logical_id)
                .ok_or(GicError::RedistributorNotFound)?;
            if rd_base < self.redistributors_base {
                return Err(GicError::InvalidMmioRange);
            }
            let rd_offset = rd_base - self.redistributors_base;
            let sgi_offset = rd_offset.checked_add(0x10000).ok_or(GicError::InvalidMmioRange)?;
            let bit = 1 << irq;
            unsafe {
                self.gicr_write_checked(sgi_offset + 0x0100, bit)?;
            }
            Ok(())
        } else if self.distributor_base != 0 {
            let reg_offset = 0x0100 + ((irq / 32) as usize) * 4;
            let bit = 1 << (irq % 32);
            unsafe {
                self.gicd_write_checked(reg_offset, bit)?;
            }
            Ok(())
        } else {
            Err(GicError::InvalidMmioRange)
        }
    }

    /// Disables an interrupt line.
    pub fn disable_irq(&self, irq: u32) -> Result<(), GicError> {
        if irq >= 1020 {
            return Err(GicError::InvalidParameter);
        }
        if irq < 32 {
            let pe_mpidr = Self::current_raw_mpidr();
            let logical_id = logical_id_for_affinity(pe_mpidr)?;
            let rd_base = get_pe_redistributor_by_logical_id(logical_id)
                .ok_or(GicError::RedistributorNotFound)?;
            if rd_base < self.redistributors_base {
                return Err(GicError::InvalidMmioRange);
            }
            let rd_offset = rd_base - self.redistributors_base;
            let sgi_offset = rd_offset.checked_add(0x10000).ok_or(GicError::InvalidMmioRange)?;
            let bit = 1 << irq;
            unsafe {
                self.gicr_write_checked(sgi_offset + 0x0180, bit)?;
            }
            Ok(())
        } else if self.distributor_base != 0 {
            let reg_offset = 0x0180 + ((irq / 32) as usize) * 4;
            let bit = 1 << (irq % 32);
            unsafe {
                self.gicd_write_checked(reg_offset, bit)?;
            }
            Ok(())
        } else {
            Err(GicError::InvalidMmioRange)
        }
    }

    /// Configures affinity routing for an SPI (IRQ 32..1019) via GICD_IROUTER<n>.
    pub fn set_spi_routing(&self, irq: u32, affinity: u64, any_pe: bool) -> Result<(), GicError> {
        if self.distributor_base == 0 {
            return Err(GicError::InvalidMmioRange);
        }
        if irq < 32 || irq >= 1020 {
            return Err(GicError::InvalidParameter);
        }
        let (aff3, aff2, aff1, aff0) = decode_mpidr_affinity(affinity);
        let val = encode_irouter(aff3, aff2, aff1, aff0, any_pe);
        let offset = 0x6000usize
            .checked_add((irq as usize).checked_mul(8).ok_or(GicError::InvalidParameter)?)
            .ok_or(GicError::InvalidParameter)?;
        unsafe {
            self.gicd_write_64_checked(offset, val)?;
        }
        Ok(())
    }

    /// Initializes the CPU interface via system registers.
    pub fn init_cpu_interface(&self) {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            // 1. Enable System Register access in ICC_SRE_EL1 (bit 0 = SRE)
            core::arch::asm!("msr s3_0_c12_c12_5, {val}", val = in(reg) 1u64, options(nomem, nostack));
            core::arch::asm!("isb");

            // 2. Set Priority Mask Register ICC_PMR_EL1 to 0xFF (allow all priority levels)
            core::arch::asm!("msr s3_0_c4_c6_0, {val}", val = in(reg) 0xFFu64, options(nomem, nostack));

            // 3. Set Binary Point Register ICC_BPR1_EL1 to 0
            core::arch::asm!("msr s3_0_c12_c12_3, {val}", val = in(reg) 0u64, options(nomem, nostack));

            // 4. Enable Group 1 interrupts in ICC_IGRPEN1_EL1 (bit 0 = Enable)
            core::arch::asm!("msr s3_0_c12_c12_7, {val}", val = in(reg) 1u64, options(nomem, nostack));
            core::arch::asm!("isb");
        }
    }

    /// Acknowledges pending interrupt via ICC_IAR1_EL1.
    pub fn acknowledge_irq(&self) -> u32 {
        #[allow(unused_mut)]
        let mut irq: u64 = 1023;
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("mrs {val}, s3_0_c12_c12_0", val = out(reg) irq, options(nomem, nostack));
        }
        irq as u32
    }

    /// Signals End of Interrupt via ICC_EOIR1_EL1.
    pub fn end_of_interrupt(&self, irq: u32) {
        if irq >= 1020 {
            return;
        }
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("msr s3_0_c12_c12_1, {val}", val = in(reg) irq as u64, options(nomem, nostack));
            core::arch::asm!("isb");
        }
        #[cfg(not(target_arch = "aarch64"))]
        let _ = irq;
    }
}

pub const ACTIVE_GIC_KIND_NONE: u8 = 0;
pub const ACTIVE_GIC_KIND_GICV2: u8 = 1;
pub const ACTIVE_GIC_KIND_GICV3: u8 = 2;

pub static ACTIVE_GIC_KIND: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(ACTIVE_GIC_KIND_NONE);
static ACTIVE_GICC_BASE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static ACTIVE_GICD_BASE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Global active interrupt controller on the platform.
#[derive(Debug, Clone, Copy)]
pub enum ActiveGic {
    None,
    Gicv2(Gicv2),
    Gicv3(Gicv3),
}

pub static ACTIVE_GIC: spin::Mutex<ActiveGic> = spin::Mutex::new(ActiveGic::None);

pub fn register_active_gicv2(gic: Gicv2) {
    // Store the full controller descriptor BEFORE publishing ACTIVE_GIC_KIND with Release ordering.
    ACTIVE_GICD_BASE.store(gic.gicd_base, core::sync::atomic::Ordering::Release);
    ACTIVE_GICC_BASE.store(gic.gicc_base, core::sync::atomic::Ordering::Release);
    *ACTIVE_GIC.lock() = ActiveGic::Gicv2(gic);
    ACTIVE_GIC_KIND.store(ACTIVE_GIC_KIND_GICV2, core::sync::atomic::Ordering::Release);
}

pub fn register_active_gicv3(gic: Gicv3) {
    // Store the full controller descriptor BEFORE publishing ACTIVE_GIC_KIND with Release ordering.
    *ACTIVE_GIC.lock() = ActiveGic::Gicv3(gic);
    ACTIVE_GIC_KIND.store(ACTIVE_GIC_KIND_GICV3, core::sync::atomic::Ordering::Release);
}

/// Acknowledges pending interrupt. This function is lock-free and wait-free,
/// ensuring zero possibility of interrupt-context deadlock.
#[inline(always)]
pub fn acknowledge_irq() -> u32 {
    match ACTIVE_GIC_KIND.load(core::sync::atomic::Ordering::Acquire) {
        ACTIVE_GIC_KIND_GICV3 => {
            #[allow(unused_mut)]
            let mut irq: u64 = GIC_SPURIOUS_IRQ as u64;
            #[cfg(target_arch = "aarch64")]
            unsafe {
                core::arch::asm!("mrs {val}, s3_0_c12_c12_0", val = out(reg) irq, options(nomem, nostack));
            }
            irq as u32
        }
        ACTIVE_GIC_KIND_GICV2 => {
            let gicc = ACTIVE_GICC_BASE.load(core::sync::atomic::Ordering::Relaxed);
            if gicc != 0 {
                unsafe {
                    core::ptr::read_volatile((gicc + GICC_IAR) as *const u32) & 0x3FF
                }
            } else {
                GIC_SPURIOUS_IRQ
            }
        }
        _ => GIC_SPURIOUS_IRQ,
    }
}

/// Signals End of Interrupt. This function is lock-free and wait-free.
/// Spurious interrupts (irq >= 1020) are automatically dropped without writing to EOIR.
#[inline(always)]
pub fn end_of_interrupt(irq: u32) {
    if irq >= 1020 {
        return; // Spurious interrupt: architectural requirement to omit EOI write
    }
    match ACTIVE_GIC_KIND.load(core::sync::atomic::Ordering::Acquire) {
        ACTIVE_GIC_KIND_GICV3 => {
            #[cfg(target_arch = "aarch64")]
            unsafe {
                core::arch::asm!("dsb ish", options(nostack, preserves_flags));
                core::arch::asm!("msr s3_0_c12_c12_1, {val}", val = in(reg) irq as u64, options(nomem, nostack));
                core::arch::asm!("isb");
            }
            #[cfg(not(target_arch = "aarch64"))]
            let _ = irq;
        }
        ACTIVE_GIC_KIND_GICV2 => {
            let gicc = ACTIVE_GICC_BASE.load(core::sync::atomic::Ordering::Relaxed);
            if gicc != 0 {
                unsafe {
                    core::ptr::write_volatile((gicc + GICC_EOIR) as *mut u32, irq & 0x3FF);
                }
            }
        }
        _ => {}
    }
}

/// Encodes affinity fields into GICv3 GICD_IROUTER<n> format (ARM GICv3 architecture).
///
/// Bit layout in GICD_IROUTER<n>:
/// - Bits [39:32]: Aff3
/// - Bit 31: IRM (1 = any participating PE, 0 = specific PE defined by Aff3.Aff2.Aff1.Aff0)
/// - Bits [23:16]: Aff2
/// - Bits [15:8]: Aff1
/// - Bits [7:0]: Aff0
/// - All other bits: 0 (RES0)
pub const fn encode_irouter(aff3: u8, aff2: u8, aff1: u8, aff0: u8, any_pe: bool) -> u64 {
    if any_pe {
        1u64 << 31
    } else {
        ((aff3 as u64) << 32)
            | ((aff2 as u64) << 16)
            | ((aff1 as u64) << 8)
            | (aff0 as u64)
    }
}

/// Decodes an MPIDR_EL1 value into its architectural affinity levels (Aff3, Aff2, Aff1, Aff0).
///
/// In MPIDR_EL1:
/// - Bits [39:32]: Aff3
/// - Bits [23:16]: Aff2
/// - Bits [15:8]: Aff1
/// - Bits [7:0]: Aff0
pub const fn decode_mpidr_affinity(mpidr: u64) -> (u8, u8, u8, u8) {
    let aff3 = ((mpidr >> 32) & 0xFF) as u8;
    let aff2 = ((mpidr >> 16) & 0xFF) as u8;
    let aff1 = ((mpidr >> 8) & 0xFF) as u8;
    let aff0 = (mpidr & 0xFF) as u8;
    (aff3, aff2, aff1, aff0)
}

/// Encodes fields for the ICC_SGI1R_EL1 system register.
///
/// Architectural layout of ICC_SGI1R_EL1:
/// - Bits [55:48]: Aff3
/// - Bit [40]: IRM (0 = route to PEs specified in TargetList, 1 = route to all PEs except the sender)
/// - Bits [39:32]: Aff2
/// - Bits [27:24]: INTID (SGI interrupt ID, 0..15)
/// - Bits [23:16]: Aff1
/// - Bits [15:0]: TargetList (bitmask of target PEs with Aff0 = bit index)
pub const fn encode_sgi1r(
    aff3: u8,
    aff2: u8,
    aff1: u8,
    target_list: u16,
    intid: u8,
    irm: bool,
) -> u64 {
    let mut reg = 0u64;
    reg |= ((aff3 as u64) & 0xFF) << 48;
    if irm {
        reg |= 1u64 << 40;
    }
    reg |= ((aff2 as u64) & 0xFF) << 32;
    reg |= ((intid as u64) & 0x0F) << 24;
    reg |= ((aff1 as u64) & 0xFF) << 16;
    reg |= (target_list as u64) & 0xFFFF;
    reg
}

/// Sends a Software Generated Interrupt (SGI) via ICC_SGI1R_EL1 system register.
pub fn send_sgi(val: u64) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
        core::arch::asm!("msr s3_0_c12_c11_5, {val}", val = in(reg) val, options(nomem, nostack));
        core::arch::asm!("isb");
    }
    #[cfg(not(target_arch = "aarch64"))]
    let _ = val;
}

/// Computes coalesced `ICC_SGI1R_EL1` values (grouped by `(Aff3, Aff2, Aff1)`) for a bitmask
/// of logical PE IDs (`0..MAX_GIC_PES`) registered in `PE_MPIDR_MAP`.
///
/// Stack-allocated and lock-free (safe to invoke from IRQ or TLB-shootdown paths).
pub fn compute_sgi1r_for_mask(sgi_id: u32, cpu_mask: u32) -> ([u64; MAX_GIC_PES], usize) {
    if cpu_mask == 0 || sgi_id >= 16 {
        return ([0u64; MAX_GIC_PES], 0);
    }
    let mut keys: [(u8, u8, u8, u16); MAX_GIC_PES] = [(0, 0, 0, 0); MAX_GIC_PES];
    let mut count = 0usize;
    let intid = sgi_id as u8;

    for slot in 0..MAX_GIC_PES {
        if (cpu_mask & (1u32 << slot)) == 0 {
            continue;
        }
        if let Some(mpidr) = get_pe_mpidr_by_logical_id(slot) {
            let (aff3, aff2, aff1, aff0) = decode_mpidr_affinity(mpidr);
            if aff0 >= 16 {
                continue;
            }
            let target_bit = 1u16 << aff0;
            let mut merged = false;
            for k in &mut keys[..count] {
                if k.0 == aff3 && k.1 == aff2 && k.2 == aff1 {
                    k.3 |= target_bit;
                    merged = true;
                    break;
                }
            }
            if !merged && count < MAX_GIC_PES {
                keys[count] = (aff3, aff2, aff1, target_bit);
                count += 1;
            }
        }
    }

    let mut sgi_vals = [0u64; MAX_GIC_PES];
    for i in 0..count {
        let (aff3, aff2, aff1, target_list) = keys[i];
        sgi_vals[i] = encode_sgi1r(aff3, aff2, aff1, target_list, intid, false);
    }
    (sgi_vals, count)
}

/// Broadcasts a Software Generated Interrupt (`0..15`) to the logical CPUs selected by `cpu_mask`.
/// Supports both GICv3 (`ICC_SGI1R_EL1` with multi-cluster MPIDR affinity coalescing) and
/// GICv2 (`GICD_SGIR`), using lock-free atomic controller state.
pub fn broadcast_sgi_to_mask(sgi_id: u32, cpu_mask: u32) -> usize {
    if cpu_mask == 0 || sgi_id >= 16 {
        return 0;
    }
    match ACTIVE_GIC_KIND.load(core::sync::atomic::Ordering::Acquire) {
        ACTIVE_GIC_KIND_GICV3 => {
            let (vals, count) = compute_sgi1r_for_mask(sgi_id, cpu_mask);
            for i in 0..count {
                send_sgi(vals[i]);
            }
            count
        }
        ACTIVE_GIC_KIND_GICV2 => {
            let gicd = ACTIVE_GICD_BASE.load(core::sync::atomic::Ordering::Acquire);
            if gicd == 0 {
                return 0;
            }
            let target_list = (cpu_mask & 0xFF) << 16;
            let sgir_val = target_list | (sgi_id & 0xF);
            unsafe {
                core::ptr::write_volatile((gicd + GICD_SGIR) as *mut u32, sgir_val);
                #[cfg(target_arch = "aarch64")]
                core::arch::asm!("dsb ish", options(nostack, preserves_flags));
            }
            1
        }
        _ => 0,
    }
}

/// Initializes GICv2 per-CPU interface and banked distributor registers when GICv2 is active.
pub fn init_active_gicv2_secondary() {
    if ACTIVE_GIC_KIND.load(core::sync::atomic::Ordering::Acquire) != ACTIVE_GIC_KIND_GICV2 {
        return;
    }
    let gicd = ACTIVE_GICD_BASE.load(core::sync::atomic::Ordering::Acquire);
    let gicc = ACTIVE_GICC_BASE.load(core::sync::atomic::Ordering::Acquire);
    if gicd != 0 && gicc != 0 {
        let gic = Gicv2::new(gicd, gicc);
        unsafe {
            gic.init_cpu_interface();
        }
        // Enable timer PPI 27 on this secondary PE's banked GICD_ISENABLER0
        gic.enable_irq(27);
    }
}

pub static SGI_INTERRUPT_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub static PE_TIMER_INTERRUPT_COUNT: [core::sync::atomic::AtomicU64; MAX_GIC_PES] = [
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
];

pub static PE_SGI_INTERRUPT_COUNT: [core::sync::atomic::AtomicU64; MAX_GIC_PES] = [
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
];

pub fn handle_sgi(_irq: u32) {
    SGI_INTERRUPT_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

pub fn sgi_interrupt_count() -> u64 {
    SGI_INTERRUPT_COUNT.load(core::sync::atomic::Ordering::Relaxed)
}

pub fn record_pe_timer_tick(pe_idx: usize) {
    if pe_idx < MAX_GIC_PES {
        PE_TIMER_INTERRUPT_COUNT[pe_idx].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}

pub fn record_pe_sgi(pe_idx: usize, _irq: u32) {
    if pe_idx < MAX_GIC_PES {
        PE_SGI_INTERRUPT_COUNT[pe_idx].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    SGI_INTERRUPT_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

pub fn pe_timer_interrupt_count(pe_idx: usize) -> u64 {
    if pe_idx < MAX_GIC_PES {
        PE_TIMER_INTERRUPT_COUNT[pe_idx].load(core::sync::atomic::Ordering::Relaxed)
    } else {
        0
    }
}

pub fn pe_sgi_interrupt_count(pe_idx: usize) -> u64 {
    if pe_idx < MAX_GIC_PES {
        PE_SGI_INTERRUPT_COUNT[pe_idx].load(core::sync::atomic::Ordering::Relaxed)
    } else {
        0
    }
}

#[cfg(test)]
static GLOBAL_PE_TEST_LOCK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
pub struct GlobalPeTestGuard;

#[cfg(test)]
impl GlobalPeTestGuard {
    pub fn acquire() -> Self {
        while GLOBAL_PE_TEST_LOCK.compare_exchange(
            false,
            true,
            core::sync::atomic::Ordering::Acquire,
            core::sync::atomic::Ordering::Relaxed,
        ).is_err() {
            core::hint::spin_loop();
        }
        Self
    }
}

#[cfg(test)]
impl Drop for GlobalPeTestGuard {
    fn drop(&mut self) {
        GLOBAL_PE_TEST_LOCK.store(false, core::sync::atomic::Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_mmio_window_valid_reads_and_writes() {
        let base = 0x080A_0000;
        let size = 0x20000; // 128 KiB
        // Start of window
        assert_eq!(validate_mmio_window(base, size, 0, 4), Ok(0x080A_0000));
        assert_eq!(validate_mmio_window(base, size, 0, 8), Ok(0x080A_0000));

        // Arbitrary aligned interior offset
        assert_eq!(validate_mmio_window(base, size, 0x1000, 4), Ok(0x080A_1000));
        assert_eq!(validate_mmio_window(base, size, 0x1000, 8), Ok(0x080A_1000));

        // Exact end of window boundary
        assert_eq!(validate_mmio_window(base, size, size - 4, 4), Ok(base + size - 4));
        assert_eq!(validate_mmio_window(base, size, size - 8, 8), Ok(base + size - 8));
    }

    #[test]
    fn test_validate_mmio_window_null_base_or_zero_size() {
        assert_eq!(validate_mmio_window(0, 0x1000, 0, 4), Err(GicError::InvalidMmioRange));
        assert_eq!(validate_mmio_window(0x0800_0000, 0, 0, 4), Err(GicError::InvalidMmioRange));
        assert_eq!(validate_mmio_window(0, 0, 0, 4), Err(GicError::InvalidMmioRange));
    }

    #[test]
    fn test_validate_mmio_window_out_of_bounds() {
        let base = 0x0800_0000;
        let size = 0x1000;
        // Exactly at size (0-byte remaining) for 4-byte read
        assert_eq!(validate_mmio_window(base, size, size, 4), Err(GicError::OutOfBoundsMmioAccess));
        // Exactly at size (0-byte remaining) for 8-byte read
        assert_eq!(validate_mmio_window(base, size, size, 8), Err(GicError::OutOfBoundsMmioAccess));
        // Past size for 4-byte read
        assert_eq!(validate_mmio_window(base, size, size + 4, 4), Err(GicError::OutOfBoundsMmioAccess));
        // Past size for 8-byte read
        assert_eq!(validate_mmio_window(base, size, size + 8, 8), Err(GicError::OutOfBoundsMmioAccess));
    }

    #[test]
    fn test_validate_mmio_window_unaligned_base() {
        let size = 0x1000;
        // 4-byte read with unaligned base (1, 2, 3)
        assert_eq!(validate_mmio_window(0x0800_0001, size, 0, 4), Err(GicError::MisalignedMmioAccess));
        assert_eq!(validate_mmio_window(0x0800_0002, size, 0, 4), Err(GicError::MisalignedMmioAccess));
        assert_eq!(validate_mmio_window(0x0800_0003, size, 0, 4), Err(GicError::MisalignedMmioAccess));

        // 8-byte read with 4-byte aligned base (not 8-byte aligned)
        assert_eq!(validate_mmio_window(0x0800_0004, size, 0, 8), Err(GicError::MisalignedMmioAccess));
    }

    #[test]
    fn test_validate_mmio_window_misaligned_offset() {
        let base = 0x0800_0000;
        let size = 0x1000;
        // 4-byte read at offsets 1, 2, 3
        assert_eq!(validate_mmio_window(base, size, 1, 4), Err(GicError::MisalignedMmioAccess));
        assert_eq!(validate_mmio_window(base, size, 2, 4), Err(GicError::MisalignedMmioAccess));
        assert_eq!(validate_mmio_window(base, size, 3, 4), Err(GicError::MisalignedMmioAccess));

        // 8-byte read at offset 4 (multiple of 4, but not 8)
        assert_eq!(validate_mmio_window(base, size, 4, 8), Err(GicError::MisalignedMmioAccess));
    }

    #[test]
    fn test_validate_mmio_window_invalid_width() {
        let base = 0x0800_0000;
        let size = 0x1000;
        assert_eq!(validate_mmio_window(base, size, 0, 0), Err(GicError::InvalidParameter));
        assert_eq!(validate_mmio_window(base, size, 0, 1), Err(GicError::InvalidParameter));
        assert_eq!(validate_mmio_window(base, size, 0, 2), Err(GicError::InvalidParameter));
        assert_eq!(validate_mmio_window(base, size, 0, 16), Err(GicError::InvalidParameter));
    }

    #[test]
    fn test_validate_mmio_window_overflow() {
        let base = usize::MAX - 0x100;
        let size = 0x1000;
        assert_eq!(validate_mmio_window(base, size, 0, 4), Err(GicError::InvalidMmioRange));
        assert_eq!(validate_mmio_window(0x1000, 0x10000, usize::MAX - 3, 4), Err(GicError::OutOfBoundsMmioAccess));
    }

    #[test]
    fn test_pe_redistributor_state_machine_and_idempotency() {
        let _guard = GlobalPeTestGuard::acquire();
        reset_pe_redistributor_map_for_test();

        // 1. Initial lookup returns None
        assert_eq!(get_pe_redistributor(0x0000_0000), None);
        assert_eq!(get_pe_redistributor(0x0000_0001), None);

        // 2. Register PE 0
        assert_eq!(register_pe_redistributor(0x0000_0000, 0x080A_0000), Ok(()));
        assert_eq!(get_pe_redistributor(0x0000_0000), Some(0x080A_0000));

        // 3. Register PE 1
        assert_eq!(register_pe_redistributor(0x0000_0001, 0x080C_0000), Ok(()));
        assert_eq!(get_pe_redistributor(0x0000_0001), Some(0x080C_0000));

        // 4. Idempotent re-registration of PE 0 with same base succeeds
        assert_eq!(register_pe_redistributor(0x0000_0000, 0x080A_0000), Ok(()));

        // 5. Conflicting re-registration of PE 0 with different base fails
        assert_eq!(register_pe_redistributor(0x0000_0000, 0x080E_0000), Err(GicError::RegistrationConflict));

        // 6. Unknown PE still returns None
        assert_eq!(get_pe_redistributor(0x0000_0002), None);

        reset_pe_redistributor_map_for_test();
    }

    #[test]
    fn test_pe_redistributor_table_full_exhaustion() {
        let _guard = GlobalPeTestGuard::acquire();
        reset_pe_redistributor_map_for_test();

        // Register 8 distinct PEs (0..7)
        for i in 0..MAX_GIC_PES {
            let aff = i as u32;
            let rd_base = 0x080A_0000 + (i * 0x20000);
            assert_eq!(register_pe_redistributor(aff, rd_base), Ok(()));
            assert_eq!(get_pe_redistributor(aff), Some(rd_base));
        }

        // 9th PE registration must fail closed with PeTableFull
        assert_eq!(register_pe_redistributor(0x0000_0008, 0x081A_0000), Err(GicError::PeTableFull));

        reset_pe_redistributor_map_for_test();
    }

    #[test]
    fn test_encode_irouter_values() {
        // Specific PE: Aff3=0x01, Aff2=0x02, Aff1=0x03, Aff0=0x04, any_pe=false
        let irouter = encode_irouter(0x01, 0x02, 0x03, 0x04, false);
        // Aff3 is [39:32], Aff2 is [23:16], Aff1 is [15:8], Aff0 is [7:0]
        let expected: u64 = (0x01u64 << 32) | (0x02u64 << 16) | (0x03u64 << 8) | 0x04u64;
        assert_eq!(irouter, expected);
        assert_eq!(irouter & (1 << 31), 0); // IRM bit must be 0

        // Non-zero Aff3, Aff2, Aff1 complex coordinate: Aff3=0xFF, Aff2=0xAA, Aff1=0x55, Aff0=0x10
        let complex_irouter = encode_irouter(0xFF, 0xAA, 0x55, 0x10, false);
        let expected_complex: u64 = (0xFFu64 << 32) | (0xAAu64 << 16) | (0x55u64 << 8) | 0x10u64;
        assert_eq!(complex_irouter, expected_complex);
        assert_eq!(complex_irouter & (1 << 31), 0);

        // Any participating PE: any_pe=true
        let any_irouter = encode_irouter(0x01, 0x02, 0x03, 0x04, true);
        assert_eq!(any_irouter, 1u64 << 31); // IRM bit must be 1, rest zeroed
    }

    #[test]
    fn test_decode_mpidr_affinity_fields() {
        // MPIDR with Aff3=0xAB, Aff2=0xCD, Aff1=0xEF, Aff0=0x12
        let mpidr: u64 = (0xABu64 << 32) | (0xCDu64 << 16) | (0xEFu64 << 8) | 0x12u64;
        let (aff3, aff2, aff1, aff0) = decode_mpidr_affinity(mpidr);
        assert_eq!(aff3, 0xAB);
        assert_eq!(aff2, 0xCD);
        assert_eq!(aff1, 0xEF);
        assert_eq!(aff0, 0x12);
    }

    #[test]
    fn test_encode_sgi1r_layouts() {
        // Target list mode: TargetList=0b0010 (CPU 1), INTID=1, Aff1=0, Aff2=0, Aff3=0, IRM=0
        let sgi = encode_sgi1r(0, 0, 0, 0b0000_0010, 1, false);
        // Bit [40] = 0, INTID [27:24] = 1, TargetList [15:0] = 2
        let expected: u64 = (1u64 << 24) | 0x0002;
        assert_eq!(sgi, expected);

        // Complex non-zero Aff3, Aff2, Aff1 with target list: Aff3=0xFE, Aff2=0xDC, Aff1=0xBA, TargetList=0x8001, INTID=14
        let complex_sgi = encode_sgi1r(0xFE, 0xDC, 0xBA, 0x8001, 14, false);
        let expected_complex: u64 = ((0xFEu64) << 48) | ((0xDCu64) << 32) | ((14u64) << 24) | ((0xBAu64) << 16) | 0x8001u64;
        assert_eq!(complex_sgi, expected_complex);

        // Broadcast to all except self: IRM=1
        let bcast_sgi = encode_sgi1r(0, 0, 0, 0, 5, true);
        let expected_bcast: u64 = (1u64 << 40) | (5u64 << 24);
        assert_eq!(bcast_sgi, expected_bcast);
    }

    #[test]
    fn test_enable_disable_irq_out_of_bounds() {
        let gic = Gicv3::with_sizes(0x0800_0000, 0x10000, 0x080A_0000, 0x40000);
        assert_eq!(gic.enable_irq(1020), Err(GicError::InvalidParameter));
        assert_eq!(gic.enable_irq(1024), Err(GicError::InvalidParameter));
        assert_eq!(gic.enable_irq(8192), Err(GicError::InvalidParameter));

        assert_eq!(gic.disable_irq(1020), Err(GicError::InvalidParameter));
        assert_eq!(gic.disable_irq(1024), Err(GicError::InvalidParameter));
        assert_eq!(gic.disable_irq(8192), Err(GicError::InvalidParameter));
    }

    #[test]
    fn test_per_pe_interrupt_counters() {
        let _guard = GlobalPeTestGuard::acquire();
        reset_pe_redistributor_map_for_test();

        assert_eq!(pe_timer_interrupt_count(0), 0);
        assert_eq!(pe_timer_interrupt_count(1), 0);
        assert_eq!(pe_sgi_interrupt_count(0), 0);
        assert_eq!(pe_sgi_interrupt_count(1), 0);
        assert_eq!(sgi_interrupt_count(), 0);

        // Record timer tick on PE 0
        record_pe_timer_tick(0);
        assert_eq!(pe_timer_interrupt_count(0), 1);
        assert_eq!(pe_timer_interrupt_count(1), 0);

        // Record timer tick on PE 1
        record_pe_timer_tick(1);
        assert_eq!(pe_timer_interrupt_count(0), 1);
        assert_eq!(pe_timer_interrupt_count(1), 1);

        // Record SGI 1 on PE 1
        record_pe_sgi(1, 1);
        assert_eq!(pe_sgi_interrupt_count(0), 0);
        assert_eq!(pe_sgi_interrupt_count(1), 1);
        assert_eq!(sgi_interrupt_count(), 1);

        reset_pe_redistributor_map_for_test();
    }
}

