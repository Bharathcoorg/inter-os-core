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

    .global secondary_cpu_entry
    secondary_cpu_entry:
        // Save context_id (logical core ID) in callee-saved register x19
        mov x19, x0

        // 1. Inspect execution level (CurrentEL[3:2])
        mrs x1, CurrentEL
        lsr x1, x1, #2
        cmp x1, #2
        b.ne .Lsecondary_el1_entry

        // In EL2: Prepare clean transition to EL1
        mov x2, #3
        msr cnthctl_el2, x2
        msr cntvoff_el2, xzr
        msr cptr_el2, xzr

        mrs x2, id_aa64pfr0_el1
        ubfx x2, x2, #24, #4
        cbz x2, 1f
        mov x2, #0x09
        msr s3_4_c12_c9_5, x2
        isb
1:
        mov x2, #(1 << 31)
        msr hcr_el2, x2
        mov x2, #0x3c5
        msr spsr_el2, x2
        adr x2, .Lsecondary_el1_entry
        msr elr_el2, x2
        eret

    .Lsecondary_el1_entry:
        dsb ish
        ic iallu
        dsb ish
        isb

        tlbi vmalle1
        dsb ish
        isb

        mrs x2, sctlr_el1
        mov x3, #0x1005
        bic x2, x2, x3
        mov x3, #(3 << 3)
        orr x2, x2, x3
        msr sctlr_el1, x2
        isb

        mov x2, #(3 << 20)
        msr cpacr_el1, x2
        isb

        // Validate PSCI context_id (x19) strictly within secondary logical slots 1..7.
        // Never derive logical CPU identity from MPIDR_EL1 Aff0 (which collides across clusters).
        cmp x19, #1
        b.lo .Lsecondary_halt
        cmp x19, #7
        b.hi .Lsecondary_halt

        // Set dedicated per-core 64 KiB secondary stack pointer and validate full range + alignment:
        // sp = __secondary_stacks_start + (logical_id << 16), must be in (__secondary_stacks_start..=__secondary_stacks_end]
        adrp x1, __secondary_stacks_start
        add x1, x1, :lo12:__secondary_stacks_start
        adrp x3, __secondary_stacks_end
        add x3, x3, :lo12:__secondary_stacks_end
        lsl x2, x19, #16
        add x1, x1, x2
        cmp x1, x3
        b.hi .Lsecondary_halt
        tst x1, #0xF
        b.ne .Lsecondary_halt
        mov sp, x1

        mov x0, x19
        // Enter Rust secondary CPU main with validated logical core ID in x0
        bl secondary_cpu_main

    .Lsecondary_halt:
        wfe
        b .Lsecondary_halt
    "#
);

extern "C" {
    pub fn secondary_cpu_entry();
    static __secondary_stacks_start: u8;
    static __secondary_stacks_end: u8;
}

pub const SECONDARY_STACK_STRIDE: usize = 0x10000;
pub const MAX_SECONDARY_LOGICAL_ID: usize = 7;

/// Validates a secondary logical core ID and computes its isolated 64 KiB stack bounds `(bottom, top)`.
/// Fails closed (`None`) on CPU 0, out-of-range IDs, unaligned regions, or out-of-bounds slots.
pub const fn validate_secondary_stack_slot(
    logical_id: usize,
    stacks_start: usize,
    stacks_end: usize,
) -> Option<(usize, usize)> {
    if logical_id == 0 || logical_id > MAX_SECONDARY_LOGICAL_ID {
        return None;
    }
    if stacks_start == 0 || stacks_end <= stacks_start || (stacks_start & 0xF) != 0 {
        return None;
    }
    let Some(offset_end) = logical_id.checked_mul(SECONDARY_STACK_STRIDE) else {
        return None;
    };
    let Some(stack_top) = stacks_start.checked_add(offset_end) else {
        return None;
    };
    let Some(stack_bottom) = stack_top.checked_sub(SECONDARY_STACK_STRIDE) else {
        return None;
    };
    if stack_bottom < stacks_start || stack_top > stacks_end || (stack_top & 0xF) != 0 {
        return None;
    }
    Some((stack_bottom, stack_top))
}

/// Validates a logical secondary CPU slot against the linked `.stack` secondary arena.
pub fn validate_linked_secondary_stack_slot(logical_id: usize) -> Option<(usize, usize)> {
    let start = core::ptr::addr_of!(__secondary_stacks_start) as usize;
    let end = core::ptr::addr_of!(__secondary_stacks_end) as usize;
    validate_secondary_stack_slot(logical_id, start, end)
}
