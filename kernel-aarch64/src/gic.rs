//! ARM Generic Interrupt Controller (GICv2) Driver for AArch64
//!
//! Provides bare-metal hardware interrupt routing, CPU core affinity,
//! and priority masking for ARM Cortex-A smartphone SoCs and QEMU `virt`.

pub const DEFAULT_GICD_BASE: usize = 0x08000000;
pub const DEFAULT_GICC_BASE: usize = 0x08010000;

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

#[derive(Debug)]
pub struct Gicv2 {
    gicd_base: usize,
    gicc_base: usize,
}

impl Gicv2 {
    pub const fn new(gicd_base: usize, gicc_base: usize) -> Self {
        Self { gicd_base, gicc_base }
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
            unsafe {
                self.gicc_read(GICC_IAR) & 0x3FF
            }
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
