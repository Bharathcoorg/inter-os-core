// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Primordial Assembly Entry Point for AArch64 Microkernel
//!
//! Handles CPU execution level negotiation (EL2 -> EL1 drop),
//! primordial stack initialization, BSS clearing, and FDT pointer handoff.

use core::arch::global_asm;

global_asm!(
    r#"
    .section .text.boot, "ax"
    .global _start
    _start:
        // 1. Inspect current execution level (CurrentEL[3:2])
        mrs x1, CurrentEL
        lsr x1, x1, #2
        cmp x1, #2
        b.ne .Lel1_entry

        // 2. We are in EL2 (Hypervisor). Prepare clean transition to EL1 (OS).
        // Enable EL1 access to ARM generic timers
        mov x2, #3
        msr cnthctl_el2, x2
        msr cntvoff_el2, xzr

        // Enable Floating-Point and Advanced SIMD in EL2 (disable traps)
        msr cptr_el2, xzr

        // If GIC system registers are present (ID_AA64PFR0_EL1[27:24] != 0), enable ICC_SRE_EL2
        mrs x2, id_aa64pfr0_el1
        ubfx x2, x2, #24, #4
        cbz x2, 1f
        mov x2, #0x09
        msr s3_4_c12_c9_5, x2
        isb
1:

        // Configure HCR_EL2 (Hypervisor Configuration Register):
        // Bit 31 (RW) = 1 (AArch64 execution state for EL1)
        mov x2, #(1 << 31)
        msr hcr_el2, x2

        // Configure SPSR_EL2 (Saved Program Status Register):
        // M[3:0] = 0b0101 (EL1h dedicated stack pointer)
        // DAIF[9:6] = 0b1111 (Debug, SError, IRQ, FIQ masked during early boot)
        mov x2, #0x3c5
        msr spsr_el2, x2

        // Set return address to EL1 entrypoint
        adr x2, .Lel1_entry
        msr elr_el2, x2
        eret

    .Lel1_entry:
        // Ensure memory visibility before cache/TLB maintenance
        dsb ish
        ic iallu
        dsb ish
        isb

        tlbi vmalle1
        dsb ish
        isb

        // Sanitize SCTLR_EL1: clear MMU (M=0, bit 0), Data Cache (C=0, bit 2), Instruction Cache (I=0, bit 12),
        // and ensure stack alignment checks (SA=1, bit 3; SA0=1, bit 4) are active, while preserving
        // implementation-defined and architecture-reserved flags configured by firmware.
        mrs x2, sctlr_el1
        mov x3, #0x1005
        bic x2, x2, x3
        mov x3, #(3 << 3)
        orr x2, x2, x3
        msr sctlr_el1, x2
        isb

        // Enable Floating-Point and Advanced SIMD (NEON) execution in EL1 (CPACR_EL1[21:20] = 0b11)
        mov x2, #(3 << 20)
        msr cpacr_el1, x2
        isb

        // Save Device Tree Blob physical address (passed in x0 by bootloader)
        mov x19, x0

        // 3. Set primordial 64 KiB boot stack pointer
        adrp x1, __stack_top
        add sp, x1, :lo12:__stack_top

        // 4. Zero the BSS section
        adrp x1, __bss_start
        add x1, x1, :lo12:__bss_start
        adrp x2, __bss_end
        add x2, x2, :lo12:__bss_end

    .Lbss_clear_loop:
        cmp x1, x2
        b.ge .Lbss_cleared
        str xzr, [x1], #8
        b .Lbss_clear_loop

    .Lbss_cleared:
        // 5. Restore FDT physical pointer in x0 and jump to Rust microkernel main
        mov x0, x19
        bl aarch64_kernel_main

    .Lhalt:
        wfe
        b .Lhalt
    "#
);
