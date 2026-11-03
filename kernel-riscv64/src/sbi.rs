// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V Supervisor Binary Interface (SBI) Wrappers

use core::arch::asm;

#[inline(always)]
pub fn console_putchar(c: usize) {
    unsafe {
        asm!(
            "ecall",
            in("a7") 1usize, // SBI_CONSOLE_PUTCHAR
            in("a0") c,
            lateout("a0") _,
            lateout("a1") _,
        );
    }
}

#[inline(always)]
pub fn shutdown() -> ! {
    unsafe {
        asm!(
            "ecall",
            in("a7") 8usize, // SBI_SHUTDOWN
            lateout("a0") _,
            lateout("a1") _,
        );
    }
    loop {}
}

pub struct SbiConsole;

impl core::fmt::Write for SbiConsole {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                console_putchar(b'\r' as usize);
            }
            console_putchar(b as usize);
        }
        Ok(())
    }
}
