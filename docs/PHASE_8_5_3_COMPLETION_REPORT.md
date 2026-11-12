# InterOS Core — Phase 8.5.3 Completion Report

**Date:** 2026-11-12  
**Branch:** `master`  
**Parent SHA:** `619d4b54d817ce95756bc89e715a7cd30cf4c043`  
**Implementation Commit SHA:** `b56946545e89aaac674c1eab9615f5a2cc89281c`  
**Parent Baseline:** Phase 8.5.2  
**Scope:** Scheduler and Reclamation Correctness (AArch64, RISC-V 64, Workspace Acceptance)  

---

## 1. Executive Summary

Phase 8.5.3 resolves the scheduler-lock deadlock, non-transactional context switching, early metadata loss in `unmap_user_page`, and missing quarantine diagnostics identified during independent review of Phase 8.5.2.

### Resolved Defects
1. **Two-Phase Lock-Safe Reaping (`reap_task_locksafe`):** Eliminated scheduler-lock deadlocks where holding `SCHEDULER.lock()` across `UserAddressSpace::destroy_fallible()` prevented remote CPUs/harts from acquiring the scheduler lock to run `prepare_switch()` and `deactivate()`. Reaping now runs in three guarded phases with `TaskState::Reaping`, `TaskState::ReapFailed`, `ReapHandle { pid, generation, exit_code }`, and identity generation tracking to reject concurrent or stale reaps.
2. **Transactional Context Switching (`prepare_switch`):** Context switches now validate and activate the incoming `UserAddressSpace::try_activate()` prior to mutating `current_task_idx`, `TaskState`, or `cpu_ticks`. Added `UserAddressSpace::deactivate_after_switch()` so switching directly between two user address spaces clears outgoing CPU/hart active tracking without overwriting `TTBR0_EL1` / `satp` back to the kernel page table.
3. **Preserve Mapping Metadata Across Unmap (`unmap_user_page`):** `unmap_user_page` retains `UserMapping` metadata and `mapped_bytes` until PTE clearing, architecture TLB invalidation, and frame recycling/release succeed.
4. **Rich Diagnostic Frame Quarantine (`QuarantinedFrame`):** `QUARANTINED_FRAMES` stores `QuarantinedFrame { paddr, ownership, reason }` via `quarantine_frame_with_meta`. Rollback errors in `map_private_frame` and `map_shared_frame` log errors and quarantine unrecoverable frames cleanly.
5. **Ownership Validation in `mark_private_frame_unmapped`:** Both AArch64 and RISC-V 64 verify bitmap allocation bit, `FrameOwnership::PrivateOwned`, and `ref_counts == 1` prior to clearing mapping count.
6. **Honest Documentation:** Updated [PHASE_8_5_ACCEPTANCE_MATRIX.md](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/docs/PHASE_8_5_ACCEPTANCE_MATRIX.md) with explicit documentation of passive bounded SMP evacuation vs active IPI preemption and structural pixel-geometry visual assertions.

---

## 2. Modified Files Summary

- [kernel-aarch64/src/task.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-aarch64/src/task.rs): Two-phase `begin_reap`/`finish_reap`/`abort_reap`, `reap_task_locksafe`, `ReapHandle`, `TaskState::Reaping`/`ReapFailed`, `generation` field, transactional `prepare_switch` using `try_activate()` + `deactivate_after_switch()`, and unit tests (Test 6 in `test_task_lifecycle`, `test_transactional_switch`).
- [kernel-aarch64/src/mmu.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-aarch64/src/mmu.rs): `QuarantinedFrame` + `quarantine_frame_with_meta`, `From<MappingKind> for FrameOwnership`, bitmap & refcount validation in `mark_private_frame_unmapped`, safe rollback logging/quarantine, ordered `unmap_user_page`, and `deactivate_after_switch`.
- [kernel-aarch64/src/syscall.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-aarch64/src/syscall.rs): Updated `SYS_WAIT_PID` to invoke `reap_task_locksafe`.
- [kernel-aarch64/src/services.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-aarch64/src/services.rs): Updated `start_internal`, `stop`, and `recover_failed` to invoke `reap_task_locksafe`.
- [kernel-aarch64/src/main.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-aarch64/src/main.rs): Added Section 45d quarantine metadata checks and Section 45e `task::test_transactional_switch()` after MMU initialization.
- [kernel-riscv64/src/task.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-riscv64/src/task.rs): Same two-phase lock-safe reaping, transactional `prepare_switch`, `deactivate_after_switch()` invocation, and Tests 6 & 7 in `test_task_lifecycle`.
- [kernel-riscv64/src/mmu.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-riscv64/src/mmu.rs): Same `QuarantinedFrame`, `mark_private_frame_unmapped` bitmap/refcount checks, rollback quarantine, ordered `unmap_user_page`, SBI RFNC quarantine metadata, and `deactivate_after_switch`.
- [kernel-riscv64/src/syscall.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-riscv64/src/syscall.rs): Updated `SYS_WAIT_PID` to invoke `reap_task_locksafe`.
- [kernel-riscv64/src/services.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-riscv64/src/services.rs): Updated `start_internal`, `stop`, and `recover_failed` to invoke `reap_task_locksafe`.
- [kernel-riscv64/src/main.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-riscv64/src/main.rs): Added quarantine metadata assertion in Physical Frame Ownership test.
- [docs/audits/PHASE_8_5_3_INITIAL_AUDIT.md](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/docs/audits/PHASE_8_5_3_INITIAL_AUDIT.md): Initial read-only audit baseline.
- [docs/PHASE_8_5_ACCEPTANCE_MATRIX.md](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/docs/PHASE_8_5_ACCEPTANCE_MATRIX.md): Updated with Phase 8.5.3 rows, observed pass counts, SMP evacuation limitations, and visual geometry methodology.

---

## 3. Observed Validation Results (Post-Final Edit)

| Command | Result | Observed Details |
|---|---|---|
| `cargo check --workspace` | **PASSED** (0) | 0 compilation errors/warnings |
| `cargo check --manifest-path kernel-aarch64/Cargo.toml` | **PASSED** (0) | 0 errors/warnings |
| `cargo check --target riscv64gc-unknown-none-elf --manifest-path kernel-riscv64/Cargo.toml` | **PASSED** (0) | 0 errors/warnings |
| `cargo test --manifest-path kernel/Cargo.toml` | **PASSED** (0) | 593 passed, 0 failed, 1 ignored |
| `python tools/interos.py contracts` | **PASSED** (0) | 25 system contracts OK, 3 license contracts OK |
| `python tests/run_all_contracts.py` | **PASSED** (0) | 1214 framework passed across 82 suites + 27 script contracts = 1241 passed (1 skipped; 13 runtime/QEMU suites excluded per runner) |
| `python -m pytest tests/test_platform_reality.py` | **PASSED** (0) | 67 passed in 0.17s |
| `python tools/interos.py build demo` | **PASSED** (0) | `target\inter-os-bios.img` (13,075,456 bytes) |
| `python tools/interos.py build --target aarch64` | **PASSED** (0) | `target\kernel-aarch64.img` (150,431,216 bytes) |
| `python tools/interos.py build --target riscv64` | **PASSED** (0) | RISC-V ELF built cleanly |
| `python tests/verify_x86_boot.py` | **PASSED** (0) | 100% pass; framebuffer 720x1440, 3680 distinct colours verified |
| `python tests/verify_aarch64_boot.py` | **PASSED** (0) | 100% contract compliance |
| `python tests/verify_aarch64_interactive.py` | **PASSED** (0) | LockScreen -> HomeScreen -> AppDrawer -> Calculator PID 73 (`7 + 5 = 12` glyph verified: `1_stem=408, 2_top=227, 2_neck=187, 2_base=143`) -> Exit & Reap PID 73 -> HomeScreen |
| `python tests/verify_riscv_boot.py` | **PASSED** (0) | 100% contract compliance |
| `python tests/verify_riscv_interactive.py` | **PASSED** (0) | LockScreen -> HomeScreen -> AppDrawer -> Calculator PID 151 (`7 + 5 = 12`) -> Exit & Reap PID 151 -> HomeScreen |

---

## 4. Remaining Known Limitations

1. **Passive SMP Evacuation:** Neither `kernel-aarch64` nor `kernel-riscv64` issues active inter-processor interrupts (GIC SGI / SBI IPI preemption kick) to interrupt a user thread spinning on a remote CPU/hart during `destroy_fallible()`. Remote eviction relies on passive bounded spinning until the remote scheduler transitions away and calls `deactivate()`.
2. **Uniprocessor x86_64 Kernel:** The `kernel/` x86_64 codebase operates as a single-processor kernel and lacks APIC TLB shootdown IPIs.
3. **Unit Tests Isolated from Multiprocessor Execution:** True multi-core parallel preemptive teardown tests across concurrent QEMU vCPUs require active GIC/PLIC IRQ + IPI preemption handlers beyond single-hart/single-CPU test harness execution.
