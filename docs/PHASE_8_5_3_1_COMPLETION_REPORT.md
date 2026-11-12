# InterOS Core — Phase 8.5.3.1 Completion Report
## Remote Evacuation Race & Frame Reclamation Error Closure

**Date:** 2026-11-12  
**Branch:** `master`  
**Starting Commit SHA:** `96db7c7f6d6a48505960ba2e60c61d086c372d25`  
**Implementation Commit SHA:** `7597e84cd6fd3060d509bd29da2e971d18abe02b`  
**Execution Mode:** Parallel subagents + local QEMU/contract verification (no CI)  

---

## 1. Subagents Used & Assigned Work

Four subagents ran concurrently with strict file-ownership isolation:

1. **Agent 1 — AArch64 Scheduler Specialist (`phase8531_worker`, conversation `3f48c37b-cbd3-4f29-9bb8-9e5bbaa170e7`)**:
   - Owned `kernel-aarch64/src/task.rs`. Implemented the 5-stage lock-safe reaping protocol (`begin_reap`, lock-free bounded evacuation poll, `detach_reaping_aspace`, `abort_reap`, `finish_reap`, updated `reap_task` & `reap_task_locksafe`) and added `test_reap_evacuation_protocol`.
2. **Agent 2 — RISC-V Scheduler Specialist (`phase8531_worker`, conversation `e7e11e24-e7bc-4206-b43a-1e30c2f96719`)**:
   - Owned `kernel-riscv64/src/task.rs`. Implemented the equivalent 5-stage lock-safe reaping protocol for RISC-V (`REAP_EVACUATION_MAX_SPINS`, `aspace_is_evacuated`, `begin_reap`, `detach_reaping_aspace`, `abort_reap`, `reap_task_locksafe`) and added Test 8 evacuation regression tests.
3. **Agent 3 — MMU Ownership & Reclamation Specialist (`phase8531_worker`, conversation `f65beeaa-fa6e-4f23-a358-c71c8632005e`)**:
   - Owned `kernel-aarch64/src/mmu.rs` and `kernel-riscv64/src/mmu.rs`. Eliminated all ignored `mark_private_frame_unmapped()` errors in `unmap_user_page` and `destroy_fallible`, preserved failed mapping metadata and `mapped_bytes`, added `is_quarantined()` protection to `recycle_frame()` and refcount-1 `release_shared_frame()`, fixed table-frame quarantine ownership and `Drop` metadata preservation, added read-only evacuation helpers (`has_unacknowledged_or_active_cpus` / `has_unacknowledged_or_active_harts`, `is_evacuated_for_current`), and added `test_mmu_ownership_reclamation_errors()`.
4. **Agent 4 — Independent Verification Auditor (`research`, read-only, conversation `f5074f95-7599-43b2-8f2d-812803558d0a`)**:
   - Audited `b5694654`, `96db7c7f`, kernel/MMU/scheduler sources, boot/interactive test scripts, and doc accuracy; identified edge cases (pre-`destroy_fallible` bypass when active, missing `dsb ishst`/remote `sfence.vma` in `unmap_user_page`, Section 45b still using locked `reap_task`, boot verifiers omitting regression signal checks, and `kernel/` vs multi-core test script coverage limits).
5. **Lead Agent Integration**:
   - Synchronized `LIFECYCLE_CLOSING_BIT` enforcement on `begin_reap()`, unified `MAX_EVACUATION_POLLS = 1000`, added `dsb ishst` before `tlbi vae1is` (AArch64 `unmap_user_page`) and remote `sbi::remote_sfence_vma` (RISC-V `unmap_user_page`), added end-to-end `reap_task_locksafe` active->`ReapFailed`->`deactivate()`->reap tests, updated `main.rs` Section 45b to call `reap_task_locksafe()` (50 live EL0/U-Mode cycles) and invoke `mmu::test_mmu_ownership_reclamation_errors()`, and added assertions in `tests/verify_aarch64_boot.py` and `tests/verify_riscv_boot.py`.

---

## 2. Files Changed

| File | Scope |
|---|---|
| [kernel-aarch64/src/task.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-aarch64/src/task.rs) | 5-stage lock-safe reaping + evacuation & `reap_task_locksafe` E2E tests (`+255 / -42` net lines) |
| [kernel-aarch64/src/mmu.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-aarch64/src/mmu.rs) | Quarantine protection, non-ignored `mark_private_frame_unmapped`, metadata preservation, `dsb ishst` barrier, `test_mmu_ownership_reclamation_errors()` |
| [kernel-aarch64/src/main.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-aarch64/src/main.rs) | Section 45b calls `reap_task_locksafe`; Section 45f invokes `mmu::test_mmu_ownership_reclamation_errors()` |
| [kernel-riscv64/src/task.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-riscv64/src/task.rs) | 5-stage lock-safe reaping + Test 8 & `reap_task_locksafe` E2E regression tests |
| [kernel-riscv64/src/mmu.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-riscv64/src/mmu.rs) | Quarantine protection, non-ignored `mark_private_frame_unmapped`, metadata preservation, remote `sfence.vma` in `unmap_user_page`, `SbiRemoteFenceFailed` cleanup alignment, `test_mmu_ownership_reclamation_errors()` |
| [kernel-riscv64/src/main.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-riscv64/src/main.rs) | Section 45b calls `reap_task_locksafe`; invokes `mmu::test_mmu_ownership_reclamation_errors()` |
| [tests/verify_aarch64_boot.py](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/tests/verify_aarch64_boot.py) | Enforces `Transactional Switch Rollback`, `5-Stage Reap Evacuation Protocol`, and `MMU Ownership Reclamation Errors` signals |
| [tests/verify_riscv_boot.py](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/tests/verify_riscv_boot.py) | Enforces `5-Stage Reap & Transactional Switch` and `MMU Ownership Reclamation Errors` signals |
| [docs/PHASE_8_5_ACCEPTANCE_MATRIX.md](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/docs/PHASE_8_5_ACCEPTANCE_MATRIX.md) | Reconciled matrix, honest SMP single-vCPU notes, exact test outputs |
| [docs/PHASE_8_5_3_1_COMPLETION_REPORT.md](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/docs/PHASE_8_5_3_1_COMPLETION_REPORT.md) | This completion report |

---

## 3. Root Causes of Blockers

### Blocker A — Remote Address-Space Evacuation Race
- `Scheduler::begin_reap()` previously executed `let aspace = task.address_space.take();` during Stage 1 before dropping `SCHEDULER.lock()`.
- Once detached from `TaskControlBlock`, any remote CPU/hart switching out of that task via `prepare_switch()` observed `self.tasks[curr_idx].address_space == None` and skipped calling `deactivate()` / `deactivate_after_switch()`, stranding the remote CPU bit in `lifecycle` and omitting `acknowledged_cpus`/`acknowledged_harts`.
- Additionally, `reap_task_locksafe()` pre-checked `is_active_on_any_cpu()` before calling `destroy_fallible()`, bypassing both `LIFECYCLE_CLOSING_BIT` activation blocking and bounded evacuation waiting.

### Blocker B — Frame Ownership Validation Errors Ignored
- Both `UserAddressSpace::unmap_user_page()` and `UserAddressSpace::destroy_fallible()` executed `let _ = mark_private_frame_unmapped(...);` before calling `recycle_frame(...)`.
- If private-frame validation failed (e.g., corrupted ownership or `ref_counts != 1`), `recycle_frame()` could still free the frame (if `ref_counts` was 0) instead of refusing recycling, retaining mapping metadata, and quarantining the frame with the original validation reason.
- Neither `recycle_frame()` nor refcount-1 `release_shared_frame()` consulted `is_quarantined(paddr)`, leaving quarantined frames vulnerable to accidental free-list re-insertion through another path.

---

## 4. Exact Implementation Changes

1. **5-Stage Lock-Safe Reaping (`task.rs` AArch64 & RISC-V 64):**
   - **Stage 1 (`begin_reap`)**: Under `SCHEDULER.lock()`, validates caller/target/state (`Terminated | Exited | ReapFailed`), transitions to `TaskState::Reaping`, increments `generation`, sets `LIFECYCLE_CLOSING_BIT` on `task.address_space` if present, **keeps `task.address_space` attached**, and returns `ReapHandle`.
   - **Stage 2 (`reap_task_locksafe` evacuation loop)**: `prepare_switch()` selects only `TaskState::Ready` so `Reaping` tasks cannot be scheduled. Outside `SCHEDULER.lock()`, loops up to `1000` polls taking/dropping `SCHEDULER.lock()` briefly to inspect `is_aspace_evacuated()` / `aspace_is_evacuated()`. On timeout while active/unacknowledged, calls `abort_reap(handle, None)` (`TaskState::ReapFailed`, bumps `generation`) without detaching or clearing active CPU bits, returning `Err(-16)`.
   - **Stage 3 (`detach_reaping_aspace`)**: Reacquires `SCHEDULER.lock()`, validates PID, exact `handle.generation`, and `TaskState::Reaping`, rechecks full evacuation under lock, transitions to `TaskState::ReapFailed` (`Err(-16)`) without detaching if busy, or `.take()`s `address_space` while keeping `handle.generation` intact.
   - **Stage 4**: Executes `aspace.destroy_fallible()` outside `SCHEDULER.lock()`.
   - **Stage 5**: Validates original `ReapHandle` under `SCHEDULER.lock()`: removes task + cleans IPC/surfaces/input via `finish_reap(handle)`, or restores `aspace_opt` into `TaskState::ReapFailed` via `abort_reap(handle, aspace_opt)` with incremented `generation`.
2. **Ownership & Quarantine Protection (`mmu.rs` AArch64 & RISC-V 64):**
   - Added `is_quarantined(paddr: u64) -> bool` and rejected quarantined frames at the entry of `recycle_frame()` and `release_shared_frame()` (when `ref_counts == 1`).
   - Replaced all `let _ = mark_private_frame_unmapped(...)` in `unmap_user_page()` and `destroy_fallible()`: on `Err(reason)`, skips `recycle_frame()`, retains the `UserMapping` entry and `mapped_bytes`, quarantines via `quarantine_frame_with_meta(paddr, kind.into(), reason)`, and returns error (`Err(reason)` / `Err(TeardownError::FrameRecycleFailed)`).
   - Guarded `destroy_fallible()` root table detachment with `self.table_frames.contains(...)`, omitted `LIFECYCLE_DESTROYED_BIT` / `self.destroyed = true` when `cleanup_failed` is true so `Drop` and retries remain consistent, quarantined table frames using `frame_ownership(paddr).unwrap_or(FrameOwnership::Kernel)`, and ensured `Drop` preserves earlier quarantine reasons without touching `Borrowed` mappings.
   - Added `dsb ishst` prior to `tlbi vae1is` in AArch64 `unmap_user_page()` and `sbi::remote_sfence_vma` on non-zero remote hart masks in RISC-V `unmap_user_page()`.

---

## 5. Regression Tests Added

- **Scheduler (`kernel-aarch64/src/task.rs`, `kernel-riscv64/src/task.rs`):**
  - `TaskState::Reaping` skipped by `prepare_switch()`.
  - `task.address_space` stays `Some` during `TaskState::Reaping`.
  - Outgoing `prepare_switch()` from a `Reaping` task still invokes `deactivate()`/`deactivate_after_switch()` cleanly while preserving `TaskState::Reaping`.
  - `detach_reaping_aspace()` rejects with `Err(-16)` while active CPU/hart bit != 0 (preserving `address_space` & active bit), and succeeds after confirmed evacuation.
  - `abort_reap()` restores `address_space` and transitions to `ReapFailed`; stale `ReapHandle` is rejected with `Err(-3)`.
  - Global `reap_task_locksafe()` end-to-end test: active AS returns `Err(-16)` (`ReapFailed`, AS intact) -> `deactivate()` -> retry succeeds `Ok(0)`.
  - Section 45b 50-cycle live EL0/U-Mode ELF tests updated to call `task::reap_task_locksafe()` outside `SCHEDULER.lock()`.
- **MMU (`kernel-aarch64/src/mmu.rs`, `kernel-riscv64/src/mmu.rs` -> `test_mmu_ownership_reclamation_errors()`):**
  - Ownership validation error in `unmap_user_page` skips `recycle_frame`, preserves `self.mappings` & `mapped_bytes`, and quarantines with original failure reason.
  - Quarantined private & refcount-1 shared frames cannot be recycled via `recycle_frame()` / `release_shared_frame()`.
  - Valid `unmap_user_page` transitions `refcount` `1 -> 0` and recycles exactly once (double-free rejected).
  - `destroy_fallible` with corrupted ownership retains failed mapping & `mapped_bytes`, skips `recycle_frame`, returns `Err(TeardownError::FrameRecycleFailed)`, and `Drop` preserves original quarantine reasons.
  - Shared-frame retain/release frees only at `refcount == 0`.

---

## 6. Executed Commands & Observed Results

| Check / Command | Exit | Outcome & Exact Classification |
|---|---|---|
| `cargo fmt --all -- --check` | 1 | **EXECUTED** — exited 1 on pre-existing formatting differences in untouched workspace/kernel files |
| `cargo check --workspace` | 0 | **EXECUTED / PASSED** — 0 errors (excludes `kernel`, `kernel-aarch64`, `kernel-riscv64`) |
| `cargo check --manifest-path kernel-aarch64/Cargo.toml` | 0 | **EXECUTED / PASSED** — 0 errors, 0 warnings |
| `cargo check --target riscv64gc-unknown-none-elf --manifest-path kernel-riscv64/Cargo.toml` | 0 | **EXECUTED / PASSED** — 0 errors, 0 warnings |
| `cargo test --manifest-path kernel/Cargo.toml` | 0 | **EXECUTED / PASSED** — 593 passed, 0 failed, 1 ignored (`x86_64` host tests only) |
| `python tools/interos.py build --target aarch64` | 0 | **EXECUTED / PASSED** — `target\kernel-aarch64.img` (150,459,888 bytes) |
| `python tools/interos.py build --target riscv64` | 0 | **EXECUTED / PASSED** — RISC-V ELF built |
| `python tests/verify_aarch64_boot.py` | 0 | **EXECUTED / PASSED** — single-vCPU QEMU (`cortex-a53`); verified transactional switch, 5-stage evacuation, and MMU error signals |
| `python tests/verify_riscv_boot.py` | 0 | **EXECUTED / PASSED** — single-hart QEMU (`rv64`); verified 5-stage reap + transactional switch & MMU error signals |
| `python tests/verify_aarch64_interactive.py` | 0 | **EXECUTED / PASSED** — Launcher -> Calculator PID 71 (`7 + 5 = 12` glyph verified) -> Reap PID 71 -> HomeScreen |
| `python tests/verify_riscv_interactive.py` | 0 | **EXECUTED / PASSED** — Launcher -> Calculator PID 177 (`7 + 5 = 12` glyph verified) -> Reap PID 177 -> HomeScreen |
| `python tools/interos.py contracts` | 0 | **EXECUTED / PASSED** — 25/25 system contracts, 3/3 license contracts |
| `python -m pytest tests/test_platform_reality.py` | 0 | **EXECUTED / PASSED** — 67 passed in 0.17s |
| `python tests/run_all_contracts.py` | 1 | **EXECUTED** — 1213 passed + 27 script contracts passed, 1 skipped, **1 pre-existing intermittent fail** (`tests/test_runner_integrity.py::test_kernel_net_cases_are_independent_of_execution_order` on `kernel/` parallel sweep attempt 3; standalone `cargo test --manifest-path kernel/Cargo.toml -- --test-threads=8 --skip task::` passed `573 passed; 0 failed`); 13 runtime/QEMU suites intentionally skipped by runner |
| `verify_x86_boot.py` | — | **NOT RE-RUN IN 8.5.3.1** (no `kernel/` x86_64 files were modified; passed in Phase 8.5.3) |
| Multi-core `-smp > 1` QEMU stress | — | **UNAVAILABLE / SOURCE-INFERRED ONLY** (kernels use global `SCHEDULER` lock / single `current_task_idx`; QEMU harness boots 1 vCPU/1 hart) |

---

## 7. QEMU / SMP Evidence & Remaining Known Issues

1. **QEMU Verification Scope**: AArch64 and RISC-V 64 boot and interactive QEMU runs execute on **single-CPU / single-hart virtual machines (`-M virt` without `-smp`)**. All 5-stage reaping, evacuation rejection/retry, `prepare_switch` outgoing deactivation, and MMU ownership/quarantine tests were genuinely compiled and executed inside QEMU, while *simultaneous remote multi-core preemption* across separate CPUs/harts is verified by source inspection and single-core simulation rather than concurrent SMP hardware/QEMU execution.
2. **Global Scheduler & Trap Architecture**: `kernel-aarch64` and `kernel-riscv64` still use a single global `SCHEDULER` lock, single global `current_task_idx`, and global trap save slots rather than per-CPU/per-hart runqueues and no GIC SGI / SBI IPI preemption kicks exist for passive evacuation.
3. **Pre-Existing Upstream Workspace Findings**:
   - `cargo fmt --all -- --check` exits 1 due to pre-existing unformatted files outside Phase 8.5.3.1 edits.
   - `tests/test_runner_integrity.py::test_kernel_net_cases_are_independent_of_execution_order` failed intermittently (attempt 3 of 3) on `kernel/` x86_64 parallel `--test-threads=8` sweeps (`kernel/` was untouched in Phase 8.5.3.1).

---

## 8. Explicit Acceptance Verdict

**PASS (with documented SMP single-core harness scope and pre-existing `kernel/`/`rustfmt` exclusions):**
- Blocker A (remote address-space evacuation race across `begin_reap`, Stage 2 bounded evacuation polling, `detach_reaping_aspace`, `abort_reap`, `finish_reap`, and `prepare_switch`) is resolved in both AArch64 and RISC-V 64 with focused regression tests enforced in `verify_aarch64_boot.py` and `verify_riscv_boot.py`.
- Blocker B (ignored `mark_private_frame_unmapped()` errors, metadata loss on failed unmap/destroy, and quarantine bypass in `recycle_frame`/`release_shared_frame`) is resolved across both architectures with focused lifecycle regression tests executing in QEMU boot.
