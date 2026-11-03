// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Serial console seam for the architecture-neutral core.
//!
//! WHY this exists rather than a `serial_println!` macro call: the x86 kernel's
//! console writes 16550 registers at fixed COM1 port addresses through
//! `x86_64::instructions::port::Port` (kernel/src/serial.rs:83). That is not
//! linkable from an AArch64 or RISC-V image, so a shared core cannot call it.
//!
//! The core therefore builds its messages as data — the boot guard already does,
//! so that the exact wording can be asserted in a test — and hands the finished
//! line to whichever console the architecture installed. Nothing is printed
//! through a hardcoded port from here.
//!
//! What happens with no console installed is deliberate: the line is dropped
//! rather than panicking, because a kernel that installs no console still has a
//! journal that must commit. The kernel that wants the notice installs one.

use spin::Mutex;

/// A serial console the core can print a finished line to.
pub trait ConsoleSink: Send + Sync {
    fn write_line(&self, line: &str);
}

static SINK: Mutex<Option<&'static (dyn ConsoleSink + Send + Sync)>> = Mutex::new(None);

/// Installs the console the core prints to. Called once, during boot, before any
/// subsystem that can refuse a write runs.
pub fn install_console(sink: &'static (dyn ConsoleSink + Send + Sync)) {
    *SINK.lock() = Some(sink);
}

/// Whether a console has been installed.
pub fn console_installed() -> bool {
    SINK.lock().is_some()
}

/// Prints one finished line, if a console was installed.
///
/// `boot_guard::announce_refusal` is the caller that matters: it explains why a
/// destructive write was refused, which is the difference between a user who is
/// told their writes go nowhere and one who finds out on the next boot.
pub fn print_line(line: &str) {
    if let Some(sink) = *SINK.lock() {
        sink.write_line(line);
    }
}
