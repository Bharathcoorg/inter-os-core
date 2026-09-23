//! RISC-V 64 Assembly Entry Point

use core::arch::asm;

#[no_mangle]
#[link_section = ".text.entry"]
pub unsafe extern "C" fn _start() -> ! {
    asm!(
        // Disable interrupts
        "csrw sie, zero",
        // Setup stack pointer
        "la sp, __stack_top",
        // Clear BSS section
        "la t0, __bss_start",
        "la t1, __bss_end",
        "1:",
        "bgeu t0, t1, 2f",
        "sd zero, 0(t0)",
        "addi t0, t0, 8",
        "j 1b",
        "2:",
        // Jump to Rust kernel main: a0 = hartid, a1 = dtb
        "tail kmain",
        options(noreturn)
    );
}
