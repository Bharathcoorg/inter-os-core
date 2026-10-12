//! PL011 console as the shared core's serial console seam.
//!
//! The architecture-neutral core cannot print anything: the x86 kernel's console
//! is 16550 registers behind `x86_64::instructions::port::Port`
//! (kernel/src/serial.rs:83), which is not linkable from an AArch64 image. So the
//! core declares a sink (`interos_kernel_core::console::ConsoleSink`) and this
//! file is the AArch64 implementation of it, over the PL011 already driven here.
//!
//! What gets printed through it: the boot guard's refusal notice, which is the
//! difference between a user who is told their persistent writes were refused and
//! a user who finds out on the next boot.

use interos_kernel_core::console::{install_console, ConsoleSink};

/// The PL011 UART this kernel already owns, presented as a line console.
pub struct Pl011Console;

impl ConsoleSink for Pl011Console {
    fn write_line(&self, line: &str) {
        let uart = crate::uart::UART.lock();
        uart.write_string(line);
        uart.write_string("\r\n");
    }
}

static PL011_CONSOLE: Pl011Console = Pl011Console;

/// Installs the PL011 console for the shared core. Called once, early, before any
/// subsystem that can refuse a write runs.
pub fn install() {
    install_console(&PL011_CONSOLE);
}
