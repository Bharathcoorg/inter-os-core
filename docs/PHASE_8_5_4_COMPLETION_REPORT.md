# InterOS Core — Phase 8.5.4 Completion Report
## Quarantine Integrity, Shared-Frame Hardening & Service Identity Safety

**Date:** 2026-11-12  
**Starting Commit:** `16b35107db6b991a4a4ba015bd6d7566da5823b7`  
**Previous Baseline:** Phase 8.5.3.2 Completion  
**Canonical Branch:** `master`  
**Target Architectures:** `x86_64`, `aarch64`, `riscv64`  
**Execution Policy:** Local verification only. Zero CI used.  

---

## 1. Executive Summary

Phase 8.5.4 achieved the complete closure of the quarantine integrity, shared-frame hardening, and service supervisor identity vulnerabilities identified in the Phase 8.5.3.2 baseline audit. 

All corrections were designed with strict defense-in-depth, preserving `#![no_std]`, `#![no_main]`, and zero heap allocations in interactive rendering and event loops:
1. **Workstream A (Quarantined Shared-Frame Integrity & Allocation Immunity):**
   - Physical frame allocators (`ArmFrameAllocator` and `RiscvFrameAllocator`) now verify `!is_quarantined(paddr)` across all free bitmap bits in `allocate_frame_with_ownership()`, guaranteeing that quarantined frames cannot be re-allocated under any condition.
   - `retain_shared_frame()` now strictly verifies `!is_quarantined(paddr)`, preventing callers from acquiring additional references to a quarantined shared frame (`refcount > 1`).
   - `map_private_frame()`, `map_shared_frame()`, and `map_borrowed_frame()` now reject quarantined frames before modifying page tables.
   - Initial quarantine diagnostic metadata (`paddr`, `ownership`, `reason`) is strictly preserved across all subsequent rejected operations and teardown attempts.
2. **Workstream B (Service Supervisor Identity & Ambient Authority Elimination):**
   - Eliminated the hardcoded `caller_pid = 1` assumption in `services.rs` across service stop, restart, and recovery paths.
   - Defined an authentic internal kernel supervisor identity: `pub const SUPERVISOR_PID: u32 = 0xFFFF_FFFE;`.
   - Implemented dedicated internal supervisor lifecycle APIs: `reap_service_task(target_pid)` and `terminate_service_task(&mut sched, target_pid)`.
   - Prevented self-reap collision when a daemon process is assigned PID 1.
   - Preserved strict lock hierarchy: `SERVICE_LIFECYCLE_LOCK -> SCHEDULER.lock()`, never holding the scheduler mutex across `reap_service_task()`.
3. **Workstream C (Reaping Wildcard & Target Identity Safety):**
   - Corrected the `.position()` candidate search in `begin_reap`: candidate filtering now evaluates `(target_pid != 0 || is_supervisor || t.parent_id == caller_pid)`.
   - When `target_pid == 0` (wait-for-any-child), the scheduler deterministically selects only the caller's own child tasks, skipping foreign child tasks that happen to precede them in the task array.
   - Explicit `target_pid != 0` requests by unauthorized callers continue to fail cleanly with `-EACCES` (-13).

---

## 2. Source Changes by Subsystem

### 2.1 MMU Subsystem (`kernel-aarch64/src/mmu.rs`, `kernel-riscv64/src/mmu.rs`)
- **`allocate_frame_with_ownership()`**:
  - Replaced early trailing-zero extraction with an exhaustive bitwise scan skipping any frame where `is_quarantined(paddr)` is true.
- **`mark_private_frame_mapped()`**:
  - Added explicit check: `if is_quarantined(paddr) { return Err("Cannot map quarantined private frame"); }`.
- **`retain_shared_frame()`**:
  - Added explicit check: `if is_quarantined(paddr) { return Err("Cannot retain quarantined shared frame"); }`.
- **`map_private_frame()`, `map_shared_frame()`, `map_borrowed_frame()`**:
  - Added explicit check: `if is_quarantined(paddr) { return Err("Cannot map quarantined frame"); }`.
- **Unit & In-Kernel Regression Tests**:
  - Extended `test_mmu_ownership_reclamation_errors()` with Parts 6 & 7:
    - Part 6: Verified that freed quarantined frames are skipped by `allocate_frame_with_ownership()`, allocating subsequent clean frames instead, and preserving original diagnostic reasons.
    - Part 7: Verified that quarantined shared frames with `refcount > 1` reject further `retain_shared_frame()` calls and reject new mappings across address spaces.

### 2.2 Task Scheduler & Reaping Protocol (`kernel-aarch64/src/task.rs`, `kernel-riscv64/src/task.rs`)
- **`SUPERVISOR_PID`**:
  - Declared `pub const SUPERVISOR_PID: u32 = 0xFFFF_FFFE;`.
- **`begin_reap()`**:
  - Updated candidate search predicate:
    ```rust
    let pos = self.tasks.iter().position(|t| {
        (target_pid == 0 || t.id == target_pid)
            && (target_pid != 0 || t.id != caller_pid)
            && (target_pid != 0 || t.id != 0)
            && (target_pid != 0 || is_supervisor || t.parent_id == caller_pid)
            && (t.state == TaskState::Terminated || t.state == TaskState::Exited || t.state == TaskState::ReapFailed)
    }).ok_or(-30i64)?;
    ```
- **`reap_service_task()` and `terminate_service_task()`**:
  - Exposing internal supervisor lifecycle helpers parameterized by `SUPERVISOR_PID` with validation rejecting `0`, `INVALID_PID`, and `SUPERVISOR_PID`.
- **Unit & In-Kernel Regression Tests**:
  - Added interleaved parent/child wildcard tests verifying that `begin_reap(parent_b, false, 0)` skips `child_a` and reaps `child_b`.
  - Added supervisor service reaping API validation tests.

### 2.3 Service Supervision Subsystem (`kernel-aarch64/src/services.rs`, `kernel-riscv64/src/services.rs`)
- **`start_internal()`**:
  - Replaced `sched.terminate_task(1, true, old_pid)` with `crate::task::terminate_service_task(&mut sched, old_pid)`.
  - Replaced `crate::task::reap_task_locksafe(1, true, old_pid)` with `crate::task::reap_service_task(old_pid)`.
- **`stop()`**:
  - Replaced `sched.terminate_task(1, true, pid)` with `crate::task::terminate_service_task(&mut sched, pid)`.
  - Replaced `crate::task::reap_task_locksafe(1, true, pid)` with `crate::task::reap_service_task(pid)`.
- **`recover_failed()`**:
  - Replaced `sched.terminate_task(1, true, old_pid)` with `crate::task::terminate_service_task(&mut sched, old_pid)`.
  - Replaced `crate::task::reap_task_locksafe(1, true, old_pid)` with `crate::task::reap_service_task(old_pid)`.

---

## 3. Comprehensive Local Test Evidence

All verification was conducted strictly locally with zero CI usage:

| Command | Exit Code | Observed Output / Evidence |
|---|---|---|
| `cargo check --workspace` | 0 | 0 errors across all workspace crates |
| `python tools/interos.py contracts` | 0 | 25 system contracts + 3 license contracts passing |
| `python -m pytest tests/test_platform_reality.py` | 0 | 67 platform reality tests passed in 0.28s |
| `cargo test --manifest-path kernel/Cargo.toml` | 0 | 593 passed, 0 failed, 1 ignored |
| `cargo check --manifest-path kernel-aarch64/Cargo.toml` | 0 | 0 warnings, 0 errors |
| `python tools/interos.py build --target aarch64` | 0 | `target/kernel-aarch64.img` (150,472,176 bytes) |
| `python tests/verify_aarch64_boot.py` | 0 | `[SUCCESS] AArch64 QEMU Boot Verification PASSED (100% contract compliance)` |
| `python tests/verify_aarch64_interactive.py` | 0 | `[SUCCESS] AArch64 Interactive Smartphone Parity Verified!` (Calculator PID 61, Result = 12, P6 PPM confirmed, Back button reap confirmed) |
| `cargo check --target riscv64gc-unknown-none-elf --manifest-path kernel-riscv64/Cargo.toml` | 0 | 0 warnings, 0 errors |
| `python tools/interos.py build --target riscv64` | 0 | `kernel-riscv64` ELF built cleanly |
| `python tests/verify_riscv_boot.py` | 0 | `[SUCCESS] RISC-V 64 QEMU Boot Verification PASSED (100% contract compliance)` |
| `python tests/verify_riscv_interactive.py` | 0 | `[SUCCESS] RISC-V 64 Interactive Smartphone Parity Verified!` (Calculator PID 149, Result = 12, P6 PPM confirmed, Back button reap confirmed) |

---

## 4. Architectural Limitations & Non-Claims

1. **Uniprocessor x86_64 Scope:** `kernel/` was untouched in Phase 8.5.4. APIC IPI TLB shootdown, `reap_task_locksafe()`, `destroy_fallible()`, and frame quarantine remain not implemented in `kernel/src`.
2. **Single-Core QEMU Verification:** QEMU execution across both AArch64 and RISC-V 64 targets ran on single vCPU / single hart (`-M virt` without `-smp`). Multi-core concurrent SMP execution is marked `SOURCE-REVIEWED` only.
3. **Passive Bounded Evacuation:** Remote evacuation is bounded passive polling with the scheduler mutex released between polling attempts; active GIC SGI / SBI IPI preemption rendezvous remains not implemented.
