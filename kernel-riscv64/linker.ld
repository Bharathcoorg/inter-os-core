/* Inter OS — RISC-V 64 Sovereign Microkernel Linker Script */
OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    /* QEMU virt InterOS RISC-V Firmware default kernel load address */
    . = 0x80200000;

    _kernel_start = .;

    .text : ALIGN(4K) {
        *(.text.entry)
        *(.text .text.*)
    }

    . = ALIGN(4K);

    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    }

    . = ALIGN(4K);

    .data : ALIGN(4K) {
        *(.data .data.*)
        *(.sdata .sdata.*)
    }

    . = ALIGN(4K);

    .bss : ALIGN(4K) {
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(COMMON)
        __bss_end = .;
    }

    . = ALIGN(4K);
    _kernel_end = .;

    /* Kernel boot stack (64KB) */
    . = ALIGN(4K);
    __stack_bottom = .;
    . += 0x10000;
    __stack_top = .;
}
