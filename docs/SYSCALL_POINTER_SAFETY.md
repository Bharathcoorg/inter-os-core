# Phase 4 — Syscall Pointer Safety

Phase 4 defines the userspace-memory trust boundary for InterOS system calls.

## Required invariants

Every userspace pointer crossing the kernel ABI must be validated before dereference:

1. Null/guard-page rejection — addresses below 0x1000 are rejected.
2. Canonical/range validation — checked addition prevents wraparound and ranges cannot enter the kernel half.
3. Length bounds — individual transfers are capped at MAX_USER_TRANSFER_LEN.
4. Page-crossing validation — every touched 4 KiB page is inspected, not only the first page.
5. Current address-space mapping — the range must be mapped in the calling task's page table.
6. Userspace permission — every page must carry the user-accessible flag; writes additionally require writable mappings.
7. Frame ownership — the resolved physical frame must belong to the calling task's address-space allocation.
8. Typed alignment — typed UserPtr<T> and typed user slices enforce align_of::<T>().
9. Kernel-owned copies — syscall handlers consume kernel buffers rather than long-lived references into userspace.
10. No raw syscall slices — syscall dispatchers must not construct from_raw_parts/from_raw_parts_mut views of untrusted addresses.

## Canonical primitives

kernel/src/syscall/user_ptr.rs is the x86_64 syscall boundary:

- UserPtr<T>
- UserSlice
- UserSliceMut
- copy_from_user
- copy_to_user
- read_user_value
- write_user_value
- read_user_string

The primitives delegate mapping, permission, page-crossing, and frame-ownership checks to TaskPageTable::validate_user_range.

Typed arrays use UserSlice::copy_to_kernel_typed so callers never need to manufacture a Rust slice directly over a userspace virtual address.

## Architecture coverage

The primary x86_64 kernel uses the full per-task page-table ownership model.

The AArch64 and RISC-V source-level syscall stubs use architecture-specific bounded user-copy helpers matching their current static user mappings. These are source-level contracts and are not represented as fresh hardware verification.

## Verification boundary

Static contract tests assert:

- all required validation primitives exist;
- user mapping permissions and ownership are enforced;
- the x86 syscall dispatcher contains no raw userspace slice construction;
- AArch64/RISC-V syscall paths use checked range validation and copy helpers.

This phase does not claim a fresh kernel build, CI result, QEMU boot, or physical-device validation.
