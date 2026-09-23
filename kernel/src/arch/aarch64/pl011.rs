//! ARM PrimeCell PL011 UART MMIO Driver (AArch64)
//!
//! Standard serial console peripheral found on ARM Cortex-A mobile SoCs
//! and QEMU `virt` platform at base address `0x09000000`.

use crate::hal::traits::SerialConsole;

/// PL011 Register Offsets
pub const UART_DR: usize = 0x00;
pub const UART_RSR: usize = 0x04;
pub const UART_FR: usize = 0x18;
pub const UART_IBRD: usize = 0x24;
pub const UART_FBRD: usize = 0x28;
pub const UART_LCR_H: usize = 0x2C;
pub const UART_CR: usize = 0x30;
pub const UART_IFLS: usize = 0x34;
pub const UART_IMSC: usize = 0x38;
pub const UART_RIS: usize = 0x3C;
pub const UART_MIS: usize = 0x40;
pub const UART_ICR: usize = 0x44;

/// Flag Register (FR) Bits
pub const FR_BUSY: u32 = 1 << 3;
pub const FR_RXFE: u32 = 1 << 4;
pub const FR_TXFF: u32 = 1 << 5;

/// Line Control Register (LCR_H) Bits
pub const LCR_H_FEN: u32 = 1 << 4;   // Enable FIFOs
pub const LCR_H_WLEN_8: u32 = 3 << 5; // 8 bit word length

/// Control Register (CR) Bits
pub const CR_UARTEN: u32 = 1 << 0;  // UART Enable
pub const CR_TXE: u32 = 1 << 8;     // Transmit Enable
pub const CR_RXE: u32 = 1 << 9;     // Receive Enable

/// ARM PrimeCell PL011 Memory-Mapped UART
#[derive(Debug)]
pub struct Pl011Uart {
    base_address: usize,
}

impl Pl011Uart {
    /// Creates a PL011 UART driver bound to a physical MMIO base address.
    pub const fn new(base_address: usize) -> Self {
        Self { base_address }
    }

    /// Read a 32-bit register from the PL011 peripheral.
    #[inline]
    unsafe fn read_reg(&self, offset: usize) -> u32 {
        let ptr = (self.base_address + offset) as *const u32;
        core::ptr::read_volatile(ptr)
    }

    /// Write a 32-bit register to the PL011 peripheral.
    #[inline]
    unsafe fn write_reg(&self, offset: usize, val: u32) {
        let ptr = (self.base_address + offset) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }

    /// Initializes the PL011 UART for 115200 baud, 8N1, FIFO enabled.
    pub unsafe fn init(&self, clock_hz: u32, baud: u32) {
        // Disable UART first
        self.write_reg(UART_CR, 0);

        // Clear all interrupts
        self.write_reg(UART_ICR, 0x7FF);

        // Compute baud rate divisor
        // Divisor = clock / (16 * baud)
        let div = (4 * clock_hz) / baud;
        let ibrd = div / 64;
        let fbrd = div % 64;

        self.write_reg(UART_IBRD, ibrd);
        self.write_reg(UART_FBRD, fbrd);

        // 8-bit word length, FIFOs enabled
        self.write_reg(UART_LCR_H, LCR_H_WLEN_8 | LCR_H_FEN);

        // Mask all interrupts initially
        self.write_reg(UART_IMSC, 0);

        // Enable UART, TX, and RX
        self.write_reg(UART_CR, CR_UARTEN | CR_TXE | CR_RXE);
    }

    /// Sends a single byte over the serial wire, blocking until transmit FIFO has space.
    pub fn write_byte(&self, byte: u8) {
        // In QEMU x86_64 host emulation or bare-metal AArch64:
        // We write to memory if base_address is valid
        if self.base_address != 0 {
            unsafe {
                // Wait while TX FIFO is full (TXFF)
                // Note: We cap loop to avoid hanging if MMIO isn't mapped
                let mut spins = 0;
                while (self.read_reg(UART_FR) & FR_TXFF) != 0 && spins < 100_000 {
                    core::hint::spin_loop();
                    spins += 1;
                }
                self.write_reg(UART_DR, byte as u32);
            }
        }
    }

    /// Reads a single byte if available from the receive FIFO.
    pub fn read_byte(&self) -> Option<u8> {
        if self.base_address != 0 {
            unsafe {
                // If RX FIFO is empty (RXFE), return None
                if (self.read_reg(UART_FR) & FR_RXFE) != 0 {
                    None
                } else {
                    Some((self.read_reg(UART_DR) & 0xFF) as u8)
                }
            }
        } else {
            None
        }
    }
}

impl SerialConsole for Pl011Uart {
    fn write_byte(&self, byte: u8) {
        Pl011Uart::write_byte(self, byte);
    }

    fn read_byte(&self) -> Option<u8> {
        Pl011Uart::read_byte(self)
    }
}
