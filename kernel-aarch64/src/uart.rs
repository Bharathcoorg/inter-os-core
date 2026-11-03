// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! ARM PrimeCell PL011 UART MMIO Driver & Thread-Safe Logging
//!
//! Provides bare-metal serial console on ARM Cortex-A mobile platforms
//! and QEMU `virt` machine (MMIO base: `0x09000000`).

use core::fmt;
use spin::Mutex;

pub const DEFAULT_PL011_BASE: usize = 0x09000000;

pub const UART_DR: usize = 0x00;
pub const UART_FR: usize = 0x18;
pub const UART_IBRD: usize = 0x24;
pub const UART_FBRD: usize = 0x28;
pub const UART_LCR_H: usize = 0x2C;
pub const UART_CR: usize = 0x30;
pub const UART_IMSC: usize = 0x38;
pub const UART_ICR: usize = 0x44;

pub const FR_TXFF: u32 = 1 << 5;
pub const FR_RXFE: u32 = 1 << 4;

pub const LCR_H_FEN: u32 = 1 << 4;
pub const LCR_H_WLEN_8: u32 = 3 << 5;

pub const CR_UARTEN: u32 = 1 << 0;
pub const CR_TXE: u32 = 1 << 8;
pub const CR_RXE: u32 = 1 << 9;

pub struct Pl011Uart {
    base_address: usize,
}

impl Pl011Uart {
    pub const fn new(base_address: usize) -> Self {
        Self { base_address }
    }

    #[inline(always)]
    unsafe fn read_reg(&self, offset: usize) -> u32 {
        let ptr = (self.base_address + offset) as *const u32;
        core::ptr::read_volatile(ptr)
    }

    #[inline(always)]
    unsafe fn write_reg(&self, offset: usize, val: u32) {
        let ptr = (self.base_address + offset) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }

    pub unsafe fn init(&self, clock_hz: u32, baud: u32) {
        self.write_reg(UART_CR, 0);
        self.write_reg(UART_ICR, 0x7FF);

        let div = (4 * clock_hz) / baud;
        let ibrd = div / 64;
        let fbrd = div % 64;

        self.write_reg(UART_IBRD, ibrd);
        self.write_reg(UART_FBRD, fbrd);
        self.write_reg(UART_LCR_H, LCR_H_WLEN_8 | LCR_H_FEN);
        self.write_reg(UART_IMSC, 0);
        self.write_reg(UART_CR, CR_UARTEN | CR_TXE | CR_RXE);
    }

    pub fn write_byte(&self, byte: u8) {
        if self.base_address != 0 {
            unsafe {
                let mut spins = 0;
                while (self.read_reg(UART_FR) & FR_TXFF) != 0 && spins < 100_000 {
                    core::hint::spin_loop();
                    spins += 1;
                }
                self.write_reg(UART_DR, byte as u32);
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

impl fmt::Write for Pl011Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

pub static UART: Mutex<Pl011Uart> = Mutex::new(Pl011Uart::new(DEFAULT_PL011_BASE));

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
