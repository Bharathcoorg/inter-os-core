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
        redistributors: usize,
    },
    Unsupported(&'static str),
    Absent,
}

/// Reads the `reg` property of a node as a list of base addresses.
///
/// `reg` is a sequence of `(address, size)` pairs, so the stride depends on both
/// cell counts; skipping only the address cells would misread every size as the
/// next base address.
fn reg_windows(parser: &ArmFdtParser, node: &crate::fdt::ArmFdtNode) -> Option<Vec<usize>> {
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
            size_cells = 0;
        }
    }

    if address_cells == 0 || address_cells > 2 || size_cells > 2 {
        return None;
    }
    let stride = (address_cells + size_cells) * 4;
    if stride == 0 {
        return None;
    }

    let mut bases = Vec::new();
    let mut cursor = 0usize;
    while cursor + address_cells * 4 <= reg.len() {
        let value = match address_cells {
            1 => u32::from_be_bytes(reg[cursor..cursor + 4].try_into().ok()?) as u64,
            _ => u64::from_be_bytes(reg[cursor..cursor + 8].try_into().ok()?),
        };
        bases.push(value as usize);
        cursor += stride;
    }

    // Safety check: if bases[1] looks like a region size (< 64 KiB) instead of a
    // peripheral base address, and we have additional entries, the parser stride
    // may have skipped sizes incorrectly. Fall back to direct (addr, size) parsing.
    if bases.len() >= 2 && bases[1] < 0x10000 && reg.len() >= 16 {
        // Direct parse for 32-bit (addr, size) pairs
        let gicd = u32::from_be_bytes([reg[0], reg[1], reg[2], reg[3]]) as usize;
        let gicc = u32::from_be_bytes([reg[8], reg[9], reg[10], reg[11]]) as usize;
        if gicd >= 0x0010_0000 && gicc >= 0x0010_0000 {
            return Some(alloc::vec![gicd, gicc]);
        }
    }

    if bases.is_empty() {
        None
    } else {
        Some(bases)
    }
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
            match reg_windows(parser, &v3_node) {
                Some(bases) if bases.len() >= 2 => {
                    return IntcDescription::Gicv3 {
                        distributor: bases[0],
                        redistributors: bases[1],
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
        match reg_windows(parser, &node) {
            Some(bases) if bases.len() >= 2 => IntcDescription::Gicv3 {
                distributor: bases[0],
                redistributors: bases[1],
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

        self.gicc_write(GICC_PMR, 0xFF);
        self.gicc_write(GICC_BPR, 0x00);
        self.gicc_write(GICC_CTLR, 0x03);
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

/// GICv3 system-register and memory-mapped driver for modern ARM SoCs (e.g. Qualcomm Snapdragon SM8250).
#[derive(Debug, Clone, Copy)]
pub struct Gicv3 {
    pub distributor_base: usize,
    pub redistributors_base: usize,
}

impl Gicv3 {
    pub const fn new(distributor_base: usize, redistributors_base: usize) -> Self {
        Self {
            distributor_base,
            redistributors_base,
        }
    }

    #[inline(always)]
    unsafe fn gicd_read(&self, offset: usize) -> u32 {
        let ptr = (self.distributor_base + offset) as *const u32;
        core::ptr::read_volatile(ptr)
    }

    #[inline(always)]
    unsafe fn gicd_write(&self, offset: usize, val: u32) {
        let ptr = (self.distributor_base + offset) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }

    #[inline(always)]
    unsafe fn gicr_read(&self, offset: usize) -> u32 {
        let ptr = (self.redistributors_base + offset) as *const u32;
        core::ptr::read_volatile(ptr)
    }

    #[inline(always)]
    unsafe fn gicr_write(&self, offset: usize, val: u32) {
        let ptr = (self.redistributors_base + offset) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }

    /// Full generic GICv3 initialization: Distributor, Redistributor, and CPU Interface.
    pub unsafe fn init(&self) {
        if self.distributor_base == 0 || self.redistributors_base == 0 {
            return;
        }

        // 1. Initialize Distributor:
        // Set ARE_NS (bit 5) and EnableGrp1NS (bit 1) in GICD_CTLR
        let mut ctlr = self.gicd_read(0x0000);
        ctlr |= (1 << 5) | (1 << 1); // ARE_NS | EnableGrp1NS
        self.gicd_write(0x0000, ctlr);

        // Wait for Register Write Pending (RWP, bit 31) to clear
        let mut spins = 0;
        while (self.gicd_read(0x0000) & (1 << 31)) != 0 && spins < 100_000 {
            core::hint::spin_loop();
            spins += 1;
        }

        // Discover number of interrupt lines
        let typer = self.gicd_read(0x0004);
        let it_lines = typer & 0x1F;
        let num_irqs = 32 * (it_lines + 1);

        // Set SPIs (32..num_irqs) to Group 1 Non-Secure and default priority
        for irq_block in 1..(num_irqs / 32) {
            self.gicd_write(0x0080 + (irq_block as usize) * 4, 0xFFFF_FFFF);
        }
        for prio_reg in 8..(num_irqs / 4) {
            self.gicd_write(0x0400 + (prio_reg as usize) * 4, 0xA0A0_A0A0);
        }

        // 2. Initialize Redistributor for current PE:
        // Clear ProcessorSleep (bit 1) in GICR_WAKER to wake up Redistributor
        let mut waker = self.gicr_read(0x0014);
        waker &= !(1 << 1);
        self.gicr_write(0x0014, waker);

        // Poll until ChildrenAsleep (bit 2) is cleared
        spins = 0;
        while (self.gicr_read(0x0014) & (1 << 2)) != 0 && spins < 100_000 {
            core::hint::spin_loop();
            spins += 1;
        }

        // SGI_base is at redistributors_base + 0x10000 (64 KiB frame)
        let sgi_base = 0x10000;
        // Configure SGIs/PPIs (0..31) to Group 1 Non-Secure
        self.gicr_write(sgi_base + 0x0080, 0xFFFF_FFFF);
        // Set default priority for SGIs and PPIs
        for prio in 0..8 {
            self.gicr_write(sgi_base + 0x0400 + prio * 4, 0xA0A0_A0A0);
        }

        // 3. Initialize CPU Interface system registers
        self.init_cpu_interface();
    }

    /// Enables an interrupt line.
    pub fn enable_irq(&self, irq: u32) {
        if irq < 32 {
            // SGI/PPI: programmed in Redistributor SGI frame (offset 0x10000)
            unsafe {
                let sgi_base = 0x10000;
                let bit = 1 << irq;
                self.gicr_write(sgi_base + 0x0100, bit);
            }
        } else if self.distributor_base != 0 {
            // SPI: programmed in Distributor
            unsafe {
                let reg_offset = 0x0100 + ((irq / 32) as usize) * 4;
                let bit = 1 << (irq % 32);
                self.gicd_write(reg_offset, bit);
            }
        }
    }

    /// Disables an interrupt line.
    pub fn disable_irq(&self, irq: u32) {
        if irq < 32 {
            unsafe {
                let sgi_base = 0x10000;
                let bit = 1 << irq;
                self.gicr_write(sgi_base + 0x0180, bit);
            }
        } else if self.distributor_base != 0 {
            unsafe {
                let reg_offset = 0x0180 + ((irq / 32) as usize) * 4;
                let bit = 1 << (irq % 32);
                self.gicd_write(reg_offset, bit);
            }
        }
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
        let mut irq: u64 = 1023;
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("mrs {val}, s3_0_c12_c12_0", val = out(reg) irq, options(nomem, nostack));
        }
        irq as u32
    }

    /// Signals End of Interrupt via ICC_EOIR1_EL1.
    pub fn end_of_interrupt(&self, irq: u32) {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("msr s3_0_c12_c12_1, {val}", val = in(reg) irq as u64, options(nomem, nostack));
            core::arch::asm!("isb");
        }
        #[cfg(not(target_arch = "aarch64"))]
        let _ = irq;
    }
}

/// Global active interrupt controller on the platform.
#[derive(Debug, Clone, Copy)]
pub enum ActiveGic {
    None,
    Gicv2(Gicv2),
    Gicv3(Gicv3),
}

pub static ACTIVE_GIC: spin::Mutex<ActiveGic> = spin::Mutex::new(ActiveGic::None);

pub fn register_active_gicv2(gic: Gicv2) {
    *ACTIVE_GIC.lock() = ActiveGic::Gicv2(gic);
}

pub fn register_active_gicv3(gic: Gicv3) {
    *ACTIVE_GIC.lock() = ActiveGic::Gicv3(gic);
}

pub fn acknowledge_irq() -> u32 {
    let guard = ACTIVE_GIC.lock();
    match *guard {
        ActiveGic::Gicv2(ref g) => g.acknowledge_irq(),
        ActiveGic::Gicv3(ref g) => g.acknowledge_irq(),
        ActiveGic::None => GIC_SPURIOUS_IRQ,
    }
}

pub fn end_of_interrupt(irq: u32) {
    let guard = ACTIVE_GIC.lock();
    match *guard {
        ActiveGic::Gicv2(ref g) => g.end_of_interrupt(irq),
        ActiveGic::Gicv3(ref g) => g.end_of_interrupt(irq),
        ActiveGic::None => {}
    }
}
