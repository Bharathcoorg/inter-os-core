//! ARM Generic Interrupt Controller (GICv2 / GICv3) Driver (AArch64)
//!
//! Provides bare-metal hardware interrupt routing, CPU core affinity,
//! and priority masking for ARM Cortex-A smartphone SoCs and QEMU `virt`.

/// Standard GICv2 MMIO base addresses on QEMU `virt` and mobile devboards
pub const DEFAULT_GICD_BASE: usize = 0x08000000;
pub const DEFAULT_GICC_BASE: usize = 0x08010000;

/// GIC Distributor Register Offsets
pub const GICD_CTLR: usize = 0x000;
pub const GICD_TYPER: usize = 0x004;
pub const GICD_IIDR: usize = 0x008;
pub const GICD_IGROUPR: usize = 0x080;
pub const GICD_ISENABLER: usize = 0x100;
pub const GICD_ICENABLER: usize = 0x180;
pub const GICD_ISPENDR: usize = 0x200;
pub const GICD_ICPENDR: usize = 0x280;
pub const GICD_IPRIORITYR: usize = 0x400;
pub const GICD_ITARGETSR: usize = 0x800;
pub const GICD_ICFGR: usize = 0xC00;

/// GIC CPU Interface Register Offsets
pub const GICC_CTLR: usize = 0x000;
pub const GICC_PMR: usize = 0x004;
pub const GICC_BPR: usize = 0x008;
pub const GICC_IAR: usize = 0x00C;
pub const GICC_EOIR: usize = 0x010;
pub const GICC_RPR: usize = 0x014;
pub const GICC_HPPIR: usize = 0x018;

/// Spurious Interrupt ID
pub const GIC_SPURIOUS_IRQ: u32 = 1023;

/// ARM GICv2 Controller Instance
#[derive(Debug)]
pub struct Gicv2 {
    gicd_base: usize,
    gicc_base: usize,
}

impl Gicv2 {
    pub const fn new(gicd_base: usize, gicc_base: usize) -> Self {
        Self { gicd_base, gicc_base }
    }

    #[inline]
    unsafe fn gicd_read(&self, offset: usize) -> u32 {
        let Some(addr) = self.gicd_base.checked_add(offset) else { return 0; };
        core::ptr::read_volatile(addr as *const u32)
    }

    #[inline]
    unsafe fn gicd_write(&self, offset: usize, val: u32) {
        let Some(addr) = self.gicd_base.checked_add(offset) else { return; };
        core::ptr::write_volatile(addr as *mut u32, val);
    }

    #[inline]
    unsafe fn gicc_read(&self, offset: usize) -> u32 {
        let Some(addr) = self.gicc_base.checked_add(offset) else { return 0; };
        core::ptr::read_volatile(addr as *const u32)
    }

    #[inline]
    unsafe fn gicc_write(&self, offset: usize, val: u32) {
        let Some(addr) = self.gicc_base.checked_add(offset) else { return; };
        core::ptr::write_volatile(addr as *mut u32, val);
    }

    /// Initializes the Distributor and CPU Interface.
    pub unsafe fn init(&self) {
        if self.gicd_base == 0 || self.gicc_base == 0 {
            return;
        }

        // 1. Disable distributor while configuring
        self.gicd_write(GICD_CTLR, 0);

        // Read total interrupt lines supported
        let typer = self.gicd_read(GICD_TYPER);
        let it_lines = (typer & 0x1F) + 1;
        let max_irqs = it_lines * 32;

        // Disable all SPI interrupts (32 and above)
        for i in (32..max_irqs).step_by(32) {
            let reg_idx = (i / 32) as usize;
            self.gicd_write(GICD_ICENABLER + reg_idx * 4, 0xFFFFFFFF);
        }

        // Set all interrupts to priority 0xA0 (mid priority)
        for i in (0..max_irqs).step_by(4) {
            let reg_idx = (i / 4) as usize;
            self.gicd_write(GICD_IPRIORITYR + reg_idx * 4, 0xA0A0A0A0);
        }

        // Route all SPI interrupts to CPU 0 (target bitmask 0x01)
        for i in (32..max_irqs).step_by(4) {
            let reg_idx = (i / 4) as usize;
            self.gicd_write(GICD_ITARGETSR + reg_idx * 4, 0x01010101);
        }

        // Enable distributor (Group 0 & Group 1)
        self.gicd_write(GICD_CTLR, 0x03);

        // 2. Initialize CPU Interface
        // Set Priority Mask to accept all priorities (0xFF)
        self.gicc_write(GICC_PMR, 0xFF);
        // Set Binary Point Register
        self.gicc_write(GICC_BPR, 0x00);
        // Enable CPU interface (Group 0 & Group 1 signaling)
        self.gicc_write(GICC_CTLR, 0x03);
    }

    /// Enables a specific hardware interrupt IRQ number.
    pub fn enable_irq(&self, irq: u32) {
        if self.gicd_base != 0 {
            let Some(reg_offset) = (irq / 32).checked_mul(4).and_then(|n| GICD_ISENABLER.checked_add(n as usize)) else { return; };
            let bit = 1 << (irq % 32);
            unsafe {
                self.gicd_write(reg_offset, bit);
            }
        }
    }

    /// Disables a specific hardware interrupt IRQ number.
    pub fn disable_irq(&self, irq: u32) {
        if self.gicd_base != 0 {
            let Some(reg_offset) = (irq / 32).checked_mul(4).and_then(|n| GICD_ICENABLER.checked_add(n as usize)) else { return; };
            let bit = 1 << (irq % 32);
            unsafe {
                self.gicd_write(reg_offset, bit);
            }
        }
    }

    /// Acknowledges an incoming interrupt, returning the IRQ ID.
    pub fn acknowledge_irq(&self) -> u32 {
        if self.gicc_base != 0 {
            unsafe {
                self.gicc_read(GICC_IAR) & 0x3FF
            }
        } else {
            GIC_SPURIOUS_IRQ
        }
    }

    /// Signals End of Interrupt (EOI) to allow lower or equal priority IRQs to fire.
    pub fn end_of_interrupt(&self, irq: u32) {
        if self.gicc_base != 0 {
            unsafe {
                self.gicc_write(GICC_EOIR, irq & 0x3FF);
            }
        }
    }
}
