// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! AArch64 Serial Console Driver Subsystem
//!
//! Provides bare-metal serial console support for:
//! 1. ARM PrimeCell PL011 UART (MMIO base: `0x09000000` on QEMU `virt`).
//! 2. Allwinner 8250 / Synopsys DesignWare 16550A UART (MMIO base: `0x01c28000` on PinePhone A64).

use core::fmt;
use spin::Mutex;

// ============================================================================
// ARM PrimeCell PL011 Register Definitions (QEMU virt)
// ============================================================================

pub const DEFAULT_PL011_BASE: usize = 0x09000000;

pub const PL011_DR: usize = 0x00;
pub const PL011_FR: usize = 0x18;
pub const PL011_IBRD: usize = 0x24;
pub const PL011_FBRD: usize = 0x28;
pub const PL011_LCR_H: usize = 0x2C;
pub const PL011_CR: usize = 0x30;
pub const PL011_IMSC: usize = 0x38;
pub const PL011_ICR: usize = 0x44;

pub const PL011_FR_TXFF: u32 = 1 << 5;
pub const PL011_FR_RXFE: u32 = 1 << 4;

pub const PL011_LCR_H_FEN: u32 = 1 << 4;
pub const PL011_LCR_H_WLEN_8: u32 = 3 << 5;

pub const PL011_CR_UARTEN: u32 = 1 << 0;
pub const PL011_CR_TXE: u32 = 1 << 8;
pub const PL011_CR_RXE: u32 = 1 << 9;

// Backwards-compatible aliases for PL011 registers
pub const UART_DR: usize = PL011_DR;
pub const UART_FR: usize = PL011_FR;
pub const UART_IBRD: usize = PL011_IBRD;
pub const UART_FBRD: usize = PL011_FBRD;
pub const UART_LCR_H: usize = PL011_LCR_H;
pub const UART_CR: usize = PL011_CR;
pub const UART_IMSC: usize = PL011_IMSC;
pub const UART_ICR: usize = PL011_ICR;
pub const FR_TXFF: u32 = PL011_FR_TXFF;
pub const FR_RXFE: u32 = PL011_FR_RXFE;
pub const LCR_H_FEN: u32 = PL011_LCR_H_FEN;
pub const LCR_H_WLEN_8: u32 = PL011_LCR_H_WLEN_8;
pub const CR_UARTEN: u32 = PL011_CR_UARTEN;
pub const CR_TXE: u32 = PL011_CR_TXE;
pub const CR_RXE: u32 = PL011_CR_RXE;

#[derive(Debug, Clone, Copy)]
pub struct Pl011Uart {
    pub base_address: usize,
}

impl Pl011Uart {
    pub const fn new(base_address: usize) -> Self {
        Self { base_address }
    }

    #[inline(always)]
    pub unsafe fn read_reg(&self, offset: usize) -> u32 {
        let ptr = (self.base_address + offset) as *const u32;
        core::ptr::read_volatile(ptr)
    }

    #[inline(always)]
    pub unsafe fn write_reg(&self, offset: usize, val: u32) {
        let ptr = (self.base_address + offset) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }

    pub unsafe fn init(&self, clock_hz: u32, baud: u32) {
        if self.base_address == 0 {
            return;
        }
        self.write_reg(PL011_CR, 0);
        self.write_reg(PL011_ICR, 0x7FF);

        let div = (4 * clock_hz) / baud;
        let ibrd = div / 64;
        let fbrd = div % 64;

        self.write_reg(PL011_IBRD, ibrd);
        self.write_reg(PL011_FBRD, fbrd);
        self.write_reg(PL011_LCR_H, PL011_LCR_H_WLEN_8 | PL011_LCR_H_FEN);
        self.write_reg(PL011_IMSC, 0);
        self.write_reg(PL011_CR, PL011_CR_UARTEN | PL011_CR_TXE | PL011_CR_RXE);
    }

    pub fn write_byte(&self, byte: u8) {
        if self.base_address != 0 {
            unsafe {
                let mut spins = 0;
                while (self.read_reg(PL011_FR) & PL011_FR_TXFF) != 0 && spins < 100_000 {
                    core::hint::spin_loop();
                    spins += 1;
                }
                self.write_reg(PL011_DR, byte as u32);
            }
        }
    }

    pub fn write_string(&self, s: &str) {
        for b in s.bytes() {
            if b == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(b);
        }
    }

    pub fn read_byte(&self) -> Option<u8> {
        if self.base_address != 0 {
            unsafe {
                if (self.read_reg(PL011_FR) & PL011_FR_RXFE) != 0 {
                    None
                } else {
                    Some((self.read_reg(PL011_DR) & 0xFF) as u8)
                }
            }
        } else {
            None
        }
    }
}

impl fmt::Write for Pl011Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

// ============================================================================
// Allwinner 8250 / Synopsys DesignWare 16550A Register Definitions (PinePhone A64)
// ============================================================================

/// PinePhone A64 physical UART0 MMIO address.
/// Connected to APB2 bus; accessible via 3.5mm headphone jack debug cable.
pub const ALLWINNER_A64_UART0_BASE: usize = 0x01c28000;

/// Allwinner A64 APB2 UART source clock (24 MHz OSC24M oscillator).
pub const ALLWINNER_A64_UART_CLOCK: u32 = 24_000_000;

// Allwinner 16550 registers are 32-bit spaced (reg-shift = 2, 4-byte stride).
pub const NS16550_RBR: usize = 0x00; // Receive Buffer Register (Read, DLAB=0)
pub const NS16550_THR: usize = 0x00; // Transmitter Holding Register (Write, DLAB=0)
pub const NS16550_DLL: usize = 0x00; // Divisor Latch Low (Read/Write, DLAB=1)
pub const NS16550_DLH: usize = 0x04; // Divisor Latch High (Read/Write, DLAB=1)
pub const NS16550_IER: usize = 0x04; // Interrupt Enable Register
pub const NS16550_IIR: usize = 0x08; // Interrupt Identification Register (Read)
pub const NS16550_FCR: usize = 0x08; // FIFO Control Register (Write)
pub const NS16550_LCR: usize = 0x0C; // Line Control Register
pub const NS16550_MCR: usize = 0x10; // Modem Control Register
pub const NS16550_LSR: usize = 0x14; // Line Status Register
pub const NS16550_MSR: usize = 0x18; // Modem Status Register
pub const NS16550_SCR: usize = 0x1C; // Scratch Register
pub const NS16550_USR: usize = 0x7C; // Synopsys DesignWare UART Status Register

// LCR Bitfields
pub const NS16550_LCR_WLEN8: u32 = 0x03; // 8 data bits
pub const NS16550_LCR_STOP1: u32 = 0x00; // 1 stop bit
pub const NS16550_LCR_PARITY_NONE: u32 = 0x00; // No parity
pub const NS16550_LCR_DLAB: u32 = 1 << 7; // Divisor Latch Access Bit

// LSR Bitfields
pub const NS16550_LSR_DR: u32 = 1 << 0;   // Data Ready (RX FIFO has byte)
pub const NS16550_LSR_THRE: u32 = 1 << 5; // Transmitter Holding Register Empty
pub const NS16550_LSR_TEMT: u32 = 1 << 6; // Transmitter Empty

// FCR Bitfields
pub const NS16550_FCR_FIFO_EN: u32 = 1 << 0;
pub const NS16550_FCR_RX_FIFO_RST: u32 = 1 << 1;
pub const NS16550_FCR_TX_FIFO_RST: u32 = 1 << 2;

// MCR Bitfields
pub const NS16550_MCR_DTR: u32 = 1 << 0;
pub const NS16550_MCR_RTS: u32 = 1 << 1;

#[derive(Debug, Clone, Copy)]
pub struct Ns16550Uart {
    pub base_address: usize,
}

impl Ns16550Uart {
    pub const fn new(base_address: usize) -> Self {
        Self { base_address }
    }

    #[inline(always)]
    pub unsafe fn read_reg(&self, offset: usize) -> u32 {
        let ptr = (self.base_address + offset) as *const u32;
        core::ptr::read_volatile(ptr)
    }

    #[inline(always)]
    pub unsafe fn write_reg(&self, offset: usize, val: u32) {
        let ptr = (self.base_address + offset) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }

    /// Initializes the 16550 / Allwinner 8250 UART to 8N1 at the requested baud.
    pub unsafe fn init(&self, clock_hz: u32, baud: u32) {
        if self.base_address == 0 {
            return;
        }

        // 1. Disable all interrupts during configuration
        self.write_reg(NS16550_IER, 0x00);

        // 2. Compute 16x oversampled baud rate divisor: divisor = clock / (16 * baud)
        let divisor = if baud > 0 {
            (clock_hz + (8 * baud)) / (16 * baud)
        } else {
            13 // Default 115200 on 24 MHz clock
        };

        // 3. Set DLAB = 1 to program baud divisor registers
        self.write_reg(NS16550_LCR, NS16550_LCR_DLAB);
        self.write_reg(NS16550_DLL, (divisor & 0xFF) as u32);
        self.write_reg(NS16550_DLH, ((divisor >> 8) & 0xFF) as u32);

        // 4. Clear DLAB and configure 8 data bits, 1 stop bit, no parity (8N1)
        self.write_reg(NS16550_LCR, NS16550_LCR_WLEN8 | NS16550_LCR_STOP1 | NS16550_LCR_PARITY_NONE);

        // 5. Reset and enable 16-byte FIFO (FIFOE | RFIFOR | XFIFOR)
        self.write_reg(
            NS16550_FCR,
            NS16550_FCR_FIFO_EN | NS16550_FCR_RX_FIFO_RST | NS16550_FCR_TX_FIFO_RST,
        );

        // 6. Assert RTS/DTR modem lines
        self.write_reg(NS16550_MCR, NS16550_MCR_DTR | NS16550_MCR_RTS);
    }

    pub fn write_byte(&self, byte: u8) {
        if self.base_address != 0 {
            unsafe {
                let mut spins = 0;
                // Wait for Transmitter Holding Register Empty (LSR bit 5)
                while (self.read_reg(NS16550_LSR) & NS16550_LSR_THRE) == 0 && spins < 100_000 {
                    core::hint::spin_loop();
                    spins += 1;
                }
                self.write_reg(NS16550_THR, byte as u32);
            }
        }
    }

    pub fn write_string(&self, s: &str) {
        for b in s.bytes() {
            if b == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(b);
        }
    }

    pub fn read_byte(&self) -> Option<u8> {
        if self.base_address != 0 {
            unsafe {
                if (self.read_reg(NS16550_LSR) & NS16550_LSR_DR) != 0 {
                    Some((self.read_reg(NS16550_RBR) & 0xFF) as u8)
                } else {
                    None
                }
            }
        } else {
            None
        }
    }
}

impl fmt::Write for Ns16550Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

// ============================================================================
// Unified Microkernel Serial Console
// ============================================================================

#[derive(Debug, Clone, Copy)]
pub enum UartDriver {
    Pl011(Pl011Uart),
    Ns16550(Ns16550Uart),
}

#[derive(Debug, Clone, Copy)]
pub struct KernelUart {
    pub driver: UartDriver,
}

impl KernelUart {
    pub const fn new_pl011(base_address: usize) -> Self {
        Self {
            driver: UartDriver::Pl011(Pl011Uart::new(base_address)),
        }
    }

    pub const fn new_ns16550(base_address: usize) -> Self {
        Self {
            driver: UartDriver::Ns16550(Ns16550Uart::new(base_address)),
        }
    }

    pub unsafe fn init_pl011(&mut self, base_address: usize, clock_hz: u32, baud: u32) {
        let uart = Pl011Uart::new(base_address);
        uart.init(clock_hz, baud);
        self.driver = UartDriver::Pl011(uart);
    }

    pub unsafe fn init_ns16550(&mut self, base_address: usize, clock_hz: u32, baud: u32) {
        let uart = Ns16550Uart::new(base_address);
        uart.init(clock_hz, baud);
        self.driver = UartDriver::Ns16550(uart);
    }

    pub fn write_byte(&self, byte: u8) {
        match self.driver {
            UartDriver::Pl011(ref uart) => uart.write_byte(byte),
            UartDriver::Ns16550(ref uart) => uart.write_byte(byte),
        }
    }

    pub fn write_string(&self, s: &str) {
        match self.driver {
            UartDriver::Pl011(ref uart) => uart.write_string(s),
            UartDriver::Ns16550(ref uart) => uart.write_string(s),
        }
    }

    pub fn read_byte(&self) -> Option<u8> {
        match self.driver {
            UartDriver::Pl011(ref uart) => uart.read_byte(),
            UartDriver::Ns16550(ref uart) => uart.read_byte(),
        }
    }

    pub fn debug_status(&self) -> (u32, u32) {
        match self.driver {
            UartDriver::Pl011(ref uart) => unsafe {
                (uart.read_reg(PL011_FR), uart.read_reg(PL011_CR))
            },
            UartDriver::Ns16550(ref uart) => unsafe {
                (uart.read_reg(NS16550_LSR), 0)
            },
        }
    }

    pub fn base_address(&self) -> usize {
        match self.driver {
            UartDriver::Pl011(ref uart) => uart.base_address,
            UartDriver::Ns16550(ref uart) => uart.base_address,
        }
    }
}

impl fmt::Write for KernelUart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

impl From<Pl011Uart> for KernelUart {
    fn from(uart: Pl011Uart) -> Self {
        Self {
            driver: UartDriver::Pl011(uart),
        }
    }
}

impl From<Ns16550Uart> for KernelUart {
    fn from(uart: Ns16550Uart) -> Self {
        Self {
            driver: UartDriver::Ns16550(uart),
        }
    }
}

pub static UART: Mutex<KernelUart> = Mutex::new(KernelUart::new_pl011(DEFAULT_PL011_BASE));

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    let mut uart = UART.lock();
    let _ = uart.write_fmt(args);
}

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => {
        $crate::uart::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! kprintln {
    () => ($crate::kprint!("\n"));
    ($($arg:tt)*) => ($crate::kprint!("{}\n", format_args!($($arg)*)));
}
