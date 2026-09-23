/* Inter OS — AArch64 Sovereign Smartphone Microkernel Linker Script */
OUTPUT_ARCH(aarch64)
ENTRY(_start)

SECTIONS
{
    /* Physical load address for mobile ARM64 platforms (RAM Base 0x40000000 + 512KB offset) */
    . = 0x40080000;

    _kernel_start = .;

    /* Standard ARM64 Linux / Android Boot Header (64 bytes) */
    .head : {
        /* code0: Branch to _start (b 0x40) */
        LONG(0x14000010);
        /* code1: NOP */
        LONG(0xd503201f);
        /* text_offset: 0x80000 (512 KiB) */
        QUAD(0x00080000);
        /* image_size: Total kernel image size */
        QUAD(_kernel_end - _kernel_start);
        /* flags: 4KB page size, little-endian, anywhere in RAM */
        QUAD(0x02);
        /* res2, res3, res4 */
        QUAD(0);
        QUAD(0);
        QUAD(0);
        /* magic: "ARM\x64" = 0x644d5241 */
        LONG(0x644d5241);
        /* res5 */
        LONG(0);
    }

    . = ALIGN(4K);

    .text : ALIGN(4K) {
        *(.text.boot)
        *(.text .text.*)
    }

    . = ALIGN(4K);

    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    }

    . = ALIGN(4K);

    .data : ALIGN(4K) {
        *(.data .data.*)
    }

    . = ALIGN(4K);

    .bss : ALIGN(4K) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    . = ALIGN(16);

    .stack : ALIGN(16) {
        . += 0x10000; /* 64 KiB Primordial Boot Stack */
        __stack_top = .;
    }

    . = ALIGN(4K);
    _kernel_end = .;
}
