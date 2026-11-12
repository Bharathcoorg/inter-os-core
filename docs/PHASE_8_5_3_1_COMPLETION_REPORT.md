# InterOS Core — Phase 8.5.3.1 & 8.5.3.2 Completion Report
## Remote Evacuation Race, Frame Reclamation Error Closure & Acceptance Closure

**Date:** 2026-11-12  
**Branch:** `master`  

---

## 1. Phase Objective

Close the two remaining safety blockers after Phase 8.5.3 and finalize cross-architecture acceptance documentation:
1. **Blocker A — Remote Address-Space Evacuation Race:** Prevent `TaskControlBlock::address_space` from being detached before remote CPUs/harts finish switching away and deactivating it cleanly.
2. **Blocker B — Ignored Private-Frame Ownership Validation Errors:** Never ignore `mark_private_frame_unmapped()` errors, never recycle frames on failed ownership transitions, retain mapping metadata and byte counts on failure, and guard against recycling quarantined frames.
3. **Phase 8.5.3.2 Acceptance Closure:** Audit all production and internal task-reaping callers, correct misleading `x86_64` / SMP / polling labels in `docs/PHASE_8_5_ACCEPTANCE_MATRIX.md`, isolate `cargo fmt` baseline vs newly introduced diffs, investigate the intermittent `test_runner_integrity.py` failure, and commit verified documentation without reopening working kernel code.

---

## 2. Starting Baseline SHA

- **Phase 8.5.3.1 Starting Baseline SHA:** `96db7c7f6d6a48505960ba2e60c61d086c372d25` (`master`)
- **Phase 8.5.3.2 Entry SHA:** `e3509c96138ebd0f7e68a32d626b0fec703381e8` (initial Phase 8.5.3.1 completion report commit on top of `7597e84cd6fd3060d509bd29da2e971d18abe02b`)

---

## 3. Implementation SHA

- **Implementation Commit SHA:** `7597e84cd6fd3060d509bd29da2e971d18abe02b` — `fix(sched, mmu): close remote reaping race and preserve frame ownership`

---

## 4. Final Documentation SHA

- **Initial Completion Report Commit SHA:** `e3509c96138ebd0f7e68a32d626b0fec703381e8` — `docs(sched, mmu): add evacuation and reclamation completion report`
- **Phase 8.5.3.2 Final Acceptance Documentation Commit:** Committed directly on top of `e3509c96138ebd0f7e68a32d626b0fec703381e8` as `docs(sched, mmu): finalize phase 8.5.3.1 acceptance evidence` (final commit SHA recorded in the push verification summary, since a commit cannot contain its own hash).

---

## 5. Original Blockers

1. **Blocker A (`task.rs` AArch64 & RISC-V 64):** `Scheduler::begin_reap()` moved `task.address_space.take()` out of the `TaskControlBlock` prior to releasing `SCHEDULER.lock()`, and `reap_task_locksafe()` pre-checked `is_active_on_any_cpu()` before `destroy_fallible()`.
2. **Blocker B (`mmu.rs` AArch64 & RISC-V 64):** Both `unmap_user_page()` and `destroy_fallible()` executed `let _ = mark_private_frame_unmapped(...)`, silently discarding validation failures before calling `recycle_frame(...)`. `recycle_frame()` and `release_shared_frame()` did not check `QUARANTINED_FRAMES`.

---

## 6. Root Causes

- **Blocker A:** Detaching `address_space` in `begin_reap()` left `tasks[curr_idx].address_space == None` while a remote CPU/hart could still be executing in that address space. When that CPU ran `prepare_switch()`, it skipped calling `out_aspace.deactivate()` / `deactivate_after_switch()`, stranding active CPU/hart bits and acknowledgements.
- **Blocker B:** Ignoring `mark_private_frame_unmapped()` allowed corrupted private frames (e.g., `ref_counts == 0` or altered ownership) to fall through into `recycle_frame()`. Calling `recycle_frame()` without checking `is_quarantined()` risked returning quarantined pages to the allocator bitmap.

---

## 7. AArch64 Implementation

Files modified in commit `7597e84cd6fd3060d509bd29da2e971d18abe02b`:
- [kernel-aarch64/src/task.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-aarch64/src/task.rs):
  - **Stage 1 (`Scheduler::begin_reap`)**: Under `SCHEDULER.lock()`, validates caller/target/state (`Terminated | Exited | ReapFailed`), transitions to `TaskState::Reaping`, increments `generation`, sets `LIFECYCLE_CLOSING_BIT` on `task.address_space` if present, **keeps `task.address_space` attached**, and returns `ReapHandle`.
  - **Stage 2 (`reap_task_locksafe` evacuation loop)**: Performs **bounded polling with the scheduler mutex released between polling attempts** (`MAX_EVACUATION_POLLS = 1000`), inspecting `Scheduler::is_aspace_evacuated()` (`!is_active_on_any_cpu()` and `(executed_cpus & !acknowledged_cpus) == 0`). `prepare_switch()` selects only `TaskState::Ready` while preserving outgoing `Reaping` address-space deactivation (`deactivate()` / `deactivate_after_switch()`). Timeout aborts via `abort_reap(handle, None)` into `TaskState::ReapFailed` (`Err(-16)`).
  - **Stage 3 (`Scheduler::detach_reaping_aspace`)**: Reacquires `SCHEDULER.lock()`, validates PID, `handle.generation`, and `TaskState::Reaping`, rechecks evacuation under lock, and `.take()`s `address_space` only if evacuated.
  - **Stage 4 & 5 (`reap_task_locksafe`)**: Runs `aspace.destroy_fallible()` outside `SCHEDULER.lock()`, then reacquires lock with the original `ReapHandle` to call `finish_reap(handle)` or restore `aspace_opt` via `abort_reap(handle, aspace_opt)`.
- [kernel-aarch64/src/main.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-aarch64/src/main.rs): Updated Section 45b 50-cycle ELF lifecycle loop to invoke `task::reap_task_locksafe()` outside `SCHEDULER.lock()`, and added Section 45f invoking `mmu::test_mmu_ownership_reclamation_errors()`.

---

## 8. RISC-V Implementation

Files modified in commit `7597e84cd6fd3060d509bd29da2e971d18abe02b`:
- [kernel-riscv64/src/task.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-riscv64/src/task.rs): Implemented identical 5-stage `begin_reap` / bounded polling with the scheduler mutex released between polling attempts (`REAP_EVACUATION_MAX_SPINS = 1000`) / `detach_reaping_aspace` / Stage 4 unlocked `destroy_fallible()` / Stage 5 `finish_reap` or `abort_reap` using `aspace_is_evacuated()` (`active_harts() == 0 && (executed_harts & !acknowledged_harts) == 0`).
- [kernel-riscv64/src/main.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-riscv64/src/main.rs): Updated Section 45b 50-cycle U-mode lifecycle loop to invoke `task::reap_task_locksafe()` outside `SCHEDULER.lock()`, and added `mmu::test_mmu_ownership_reclamation_errors()`.

---

## 9. MMU Ownership Changes

Files modified identically across [kernel-aarch64/src/mmu.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-aarch64/src/mmu.rs) and [kernel-riscv64/src/mmu.rs](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-riscv64/src/mmu.rs):
- Added `pub fn is_quarantined(paddr: u64) -> bool` and rejected quarantined frames at the entry of `recycle_frame()` and `release_shared_frame()` (when `ref_counts[frame_idx] == 1`).
- Replaced ignored `let _ = mark_private_frame_unmapped(...)` in `unmap_user_page()` and `destroy_fallible()` with explicit error handling: on failure, `recycle_frame()` is never called, the failed mapping entry and `mapped_bytes` are retained, the frame is quarantined via `quarantine_frame_with_meta()` preserving the original error reason, and an explicit error is returned (`Err(reason)` / `Err(TeardownError::FrameRecycleFailed)`).
- Guarded `destroy_fallible()` root table detachment with `self.table_frames.contains(...)`, kept `self.destroyed == false` and `LIFECYCLE_DESTROYED_BIT` clear when `cleanup_failed` is true, recorded table-frame quarantine ownership using `frame_ownership(paddr).unwrap_or(FrameOwnership::Kernel)`, and ensured `Drop` avoids double-frees, skips `Borrowed` frames, and does not overwrite prior quarantine reasons.
- Added `dsb ishst` before `tlbi vae1is` in AArch64 `unmap_user_page()` and remote `sbi::remote_sfence_vma` on non-zero remote hart masks in RISC-V `unmap_user_page()`, plus aligned RISC-V `SbiRemoteFenceFailed` handling.
- Added read-only helpers `has_unacknowledged_or_active_cpus()` / `has_unacknowledged_or_active_harts()` and `is_evacuated_for_current()`.

---

## 10. Scheduler Caller Audit

Audited every call site across the entire repository for `reap_task(`, `reap_task_locksafe(`, `begin_reap(`, `detach_reaping_aspace(`, `finish_reap(`, and `abort_reap(` (Agent 1, `401168ce`). **`x86_64` (`kernel/`)** does not use these APIs (it uses uniprocessor `reap_task_now_or_defer`), and neither architecture implements a `stop_all()` service function.

| Caller | Architecture | Context | API Used | Locks Held at Call Site | Remote Active AS Possible? | Safe? |
|---|---|---|---|---|---|---|
| `sys_dispatch` (`SYS_WAIT_PID`, [kernel-aarch64/src/syscall.rs:680](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-aarch64/src/syscall.rs#L680)) | AArch64 | Production syscall: process wait/reap | `task::reap_task_locksafe` | None (`SCHEDULER.lock()` dropped before call) | Yes | **Yes** — all 7 invariants (I1–I7) satisfied |
| `dispatch_syscall` (`SYS_WAIT_PID`, [kernel-riscv64/src/syscall.rs:812](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-riscv64/src/syscall.rs#L812)) | RISC-V 64 | Production syscall: process wait/reap | `task::reap_task_locksafe` | None (`SCHEDULER.lock()` dropped before call) | Yes | **Yes** — all 7 invariants satisfied |
| `start_internal` / `stop` / `recover_failed` ([kernel-aarch64/src/services.rs:163,256,339](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-aarch64/src/services.rs#L163)) | AArch64 | Production supervisor: service lifecycle | `task::reap_task_locksafe` | `SERVICE_LIFECYCLE_LOCK` only (`SCHEDULER.lock()` dropped prior to call; no path acquires `SERVICE_LIFECYCLE_LOCK` under `SCHEDULER`) | Yes | **Yes** — all 7 invariants satisfied |
| `start_internal` / `stop` / `recover_failed` ([kernel-riscv64/src/services.rs:163,256,339](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-riscv64/src/services.rs#L163)) | RISC-V 64 | Production supervisor: service lifecycle | `task::reap_task_locksafe` | `SERVICE_LIFECYCLE_LOCK` only (`SCHEDULER.lock()` dropped prior to call) | Yes | **Yes** — all 7 invariants satisfied |
| Boot Section 45b ([kernel-aarch64/src/main.rs:1568](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-aarch64/src/main.rs#L1568), [kernel-riscv64/src/main.rs:209](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-riscv64/src/main.rs#L209)) | Both | Kernel boot self-test (50 ELF cycles) | `task::reap_task_locksafe` | None (`SCHEDULER.lock()` dropped) | No (1 core at boot, kernel page table active) | **Yes** — all 7 invariants satisfied |
| Boot Section 45c ([kernel-aarch64/src/main.rs:1600,1605](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-aarch64/src/main.rs#L1600), [kernel-riscv64/src/main.rs:242,247](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/kernel-riscv64/src/main.rs#L242)) | Both | Kernel boot test (active process reap denial on `active_proc` with `address_space: None`) | `sched.reap_task` | `SCHEDULER.lock()` held | No (single-core boot, `address_space: None`) | **Safe for single-core no-AS test** (not used in production; `Scheduler::reap_task` does not poll Stage 2 and holds `&mut self` across destroy) |
| Unit tests in `task.rs` (`test_task_lifecycle`, `test_reap_evacuation_protocol`, Test 8) | Both | Isolated `Scheduler::new()` & E2E `SCHEDULER` tests | `begin_reap`, `detach_reaping_aspace`, `abort_reap`, `finish_reap`, `reap_task_locksafe`, local `sched.reap_task` | None held across `reap_task_locksafe`; local `Scheduler::new()` for unit stages | Simulated on 1 core | **Yes** — directly unit-tests each protocol stage and E2E `reap_task_locksafe` |

**Audit Conclusion:** Zero production callers bypass `reap_task_locksafe()`, no recursive `SCHEDULER.lock()` acquisition exists, and no production caller edits were required.

---

## 11. Regression Tests

- **Scheduler (`kernel-aarch64/src/task.rs` `test_reap_evacuation_protocol` & `kernel-riscv64/src/task.rs` Test 8):**
  - Verifies `TaskState::Reaping` is unschedulable in `prepare_switch()`.
  - Verifies `task.address_space` remains `Some(...)` during `TaskState::Reaping`.
  - Verifies outgoing `prepare_switch()` from a `Reaping` task accesses `curr.address_space` and deactivates it while preserving `TaskState::Reaping`.
  - Verifies `detach_reaping_aspace()` rejects with `Err(-16)` while active CPU/hart bits remain, preserving `address_space` and active bits without clearing them, and succeeds after evacuation (`prepare_switch` or `deactivate()`).
  - Verifies `abort_reap()` restores detached `address_space` into `TaskState::ReapFailed`, bumps `generation`, and rejects stale `ReapHandle` with `Err(-3)`.
  - Verifies end-to-end `reap_task_locksafe()` on global `SCHEDULER`: active AS returns `Err(-16)` (`ReapFailed`, AS attached) -> `deactivate()` -> retry succeeds `Ok(0)`.
- **MMU (`test_mmu_ownership_reclamation_errors()` in both `mmu.rs` files):**
  - Verifies ownership validation failure in `unmap_user_page` skips `recycle_frame()`, retains `self.mappings` and `mapped_bytes`, quarantines with the original failure reason, and returns error.
  - Verifies `recycle_frame()` and refcount-1 `release_shared_frame()` reject quarantined frames.
  - Verifies normal `unmap_user_page` transitions `refcount` `1 -> 0` and recycles once (rejecting double-free).
  - Verifies `destroy_fallible()` with corrupted ownership retains failed mapping and `mapped_bytes`, skips `recycle_frame()`, quarantines with `"Not a private frame"`, returns `Err(TeardownError::FrameRecycleFailed)`, and `Drop` preserves original quarantine reasons.
  - Verifies shared-frame retain/release decrements properly and frees only at `refcount == 0`.

---

## 12. Exact Local Test Commands & 13. Actual Exit Codes

| Check / Command | Exit Code | Executed Phase | Actual Outcome |
|---|---|---|---|
| `cargo fmt --all -- --check` | 1 | 8.5.3.1 / 8.5.3.2 | Fails on pre-existing unformatted lines across workspace crates (`apps/*`, `sdk/*`, `userspace/*`) |
| `cargo fmt --manifest-path kernel/Cargo.toml -- --check` | 1 | 8.5.3.2 | Fails on pre-existing unformatted lines in `kernel/build.rs` and `kernel/src/*` |
| `cargo fmt --manifest-path kernel-aarch64/Cargo.toml -- --check` | 1 | 8.5.3.2 | Fails across 48 files in `kernel-aarch64/src/` (pre-existing + newly added diffs detailed below) |
| `cargo fmt --manifest-path kernel-riscv64/Cargo.toml -- --check` | 1 | 8.5.3.2 | Fails across 14 files in `kernel-riscv64/src/` |
| `cargo check --workspace` | 0 | 8.5.3.1 / 8.5.3.2 | Passed (excludes `kernel`, `kernel-aarch64`, `kernel-riscv64`) |
| `cargo check --manifest-path kernel-aarch64/Cargo.toml` | 0 | 8.5.3.1 / 8.5.3.2 | Passed (0 errors, 0 warnings) |
| `cargo check --target riscv64gc-unknown-none-elf --manifest-path kernel-riscv64/Cargo.toml` | 0 | 8.5.3.1 / 8.5.3.2 | Passed (0 errors, 0 warnings) |
| `cargo test --manifest-path kernel/Cargo.toml` | 0 | 8.5.3.1 / 8.5.3.2 | Passed: `593 passed; 0 failed; 1 ignored` (`x86_64` host tests) |
| `cargo test --manifest-path kernel/Cargo.toml -- --test-threads=1` | 0 | 8.5.3.2 | Passed: `593 passed; 0 failed; 1 ignored` (sequential execution) |
| `python tools/interos.py contracts` | 0 | 8.5.3.1 / 8.5.3.2 | Passed: 25/25 system contracts, 3/3 license contracts |
| `python -m pytest tests/test_platform_reality.py` | 0 | 8.5.3.1 / 8.5.3.2 | Passed: 67 passed |
| `python -m pytest tests/test_runner_integrity.py -k test_kernel_net_cases_are_independent_of_execution_order -v` | 0 | 8.5.3.2 | Passed standalone (`1 passed, 14 deselected in 51.87s`) |
| `cargo test --manifest-path kernel/Cargo.toml --no-fail-fast -- --format terse --test-threads=8 --skip task::` (30 sweeps) | 101 (2/30) | 8.5.3.2 | 28/30 passed; 2/30 failed (Run 11 & Run 26) on `serial::tests::proc_list_answers_with_the_process_table_on_demand` |
| `python tests/run_all_contracts.py` | 1 | 8.5.3.1 | 1213 passed + 27 script contracts passed, 1 skipped, 1 intermittent fail (`test_runner_integrity.py::test_kernel_net_cases_are_independent_of_execution_order`), 13 QEMU/runtime suites excluded |
| `python tools/interos.py build --target aarch64` | 0 | 8.5.3.1 | Built `target\kernel-aarch64.img` (150,459,888 bytes) |
| `python tools/interos.py build --target riscv64` | 0 | 8.5.3.1 | Built RISC-V 64 kernel ELF |
| `python tests/verify_aarch64_boot.py` | 0 | 8.5.3.1 | Passed (1-vCPU QEMU); enforced Transactional Switch, 5-Stage Reap Evacuation, and MMU Error signals |
| `python tests/verify_riscv_boot.py` | 0 | 8.5.3.1 | Passed (1-hart QEMU); enforced 5-Stage Reap + Transactional Switch and MMU Error signals |
| `python tests/verify_aarch64_interactive.py` | 0 | 8.5.3.1 | Passed (1-vCPU QEMU); Calculator PID 71 (`7 + 5 = 12`), reap PID 71, HomeScreen |
| `python tests/verify_riscv_interactive.py` | 0 | 8.5.3.1 | Passed (1-hart QEMU); Calculator PID 177 (`7 + 5 = 12`), reap PID 177, HomeScreen |
| `verify_x86_boot.py` | — | 8.5.3 baseline | Not re-run in 8.5.3.1/8.5.3.2 (`kernel/` untouched; ran and passed in Phase 8.5.3) |

---

## 14. Formatting Status

Investigated by Agent 3 (`a7d0b251`) using `rustfmt --edition 2021 --check` vs `git diff -U0 619d4b54..7597e84c` (and `b5694654..7597e84c`):
- Pre-existing repository files in both crates and workspace are unformatted baseline-wide (`cargo fmt --all -- --check` fails across `apps/`, `sdk/`, `userspace/`; `kernel-aarch64` fails on 48 files; `kernel-riscv64` fails on 14 files).
- Breakdown across the 6 Rust files changed in `7597e84c` (total diffs vs newly touched `619d4b54..7597e84c` diffs):
  - `kernel-aarch64/src/main.rs`: 720 total diff lines (4 pre-existing/new touched in 2 hunks; 716 pre-existing).
  - `kernel-aarch64/src/mmu.rs`: 106 total diff lines (40 touched; 66 pre-existing).
  - `kernel-aarch64/src/task.rs`: 78 total diff lines (59 touched; 19 pre-existing).
  - `kernel-riscv64/src/main.rs`: 400 total diff lines (5 touched; 395 pre-existing).
  - `kernel-riscv64/src/mmu.rs`: 78 total diff lines (51 touched; 27 pre-existing).
  - `kernel-riscv64/src/task.rs`: 79 total diff lines (64 touched; 15 pre-existing).
- Per project discipline ("Do not create unrelated repository-wide formatting churn" and "finish the documentation promptly instead of reopening working kernel code"), untouched formatting diffs were not rewritten.

---

## 15. Contract-Suite Status

Investigated by Agent 3 (`a7d0b251`):
- `tests/test_runner_integrity.py::test_kernel_net_cases_are_independent_of_execution_order` failed on Half 3 (`--test-threads=8 --skip task::` across 3 parallel sweeps) during the Phase 8.5.3.1 `run_all_contracts.py` run.
- Standalone reproduction: `pytest tests/test_runner_integrity.py -k test_kernel_net_cases_are_independent_of_execution_order -v` PASSED (`1/1`), `cargo test --manifest-path kernel/Cargo.toml -- --test-threads=1` PASSED (`593/593`), and in 30 repeated `--test-threads=8 --skip task::` sweeps, **2/30 failed** (Run 11 and Run 26) identically in **`serial::tests::proc_list_answers_with_the_process_table_on_demand`** (`kernel/src/serial.rs:2469`).
- Root cause: `handle_serial_command(b"PROC_LIST")` calls `crate::interrupts::process_table_dump_now()` (`kernel/src/interrupts.rs:869`), which calls `SCHEDULER.try_lock()?`. Concurrent non-`task::` tests (e.g., `serial::tests`, `syscall::tests`, `fs::vfs`, `net::caller_task_id`) lock `SCHEDULER.lock()`, causing `PROC_LIST` to print `[PROC_LIST] UNAVAILABLE scheduler_busy` (additionally `CAPTURED` in `serial.rs:149` lacks a test synchronization mutex).
- `git diff --stat 619d4b54..HEAD -- kernel/ tests/test_runner_integrity.py` is **0 bytes**, confirming this is a pre-existing `kernel/` test-harness parallel race unrelated to Phase 8.5.3 / 8.5.3.1.

---

## 16. QEMU Evidence

- AArch64 boot (`verify_aarch64_boot.py`) and interactive UI (`verify_aarch64_interactive.py`) ran on QEMU `virt` (`cortex-a53`) with **one vCPU** (no `-smp`).
- RISC-V 64 boot (`verify_riscv_boot.py`) and interactive UI (`verify_riscv_interactive.py`) ran on QEMU `virt` (`rv64`, OpenSBI default) with **one hart** (no `-smp`).
- Both boots executed real `UserAddressSpace`, `SCHEDULER`, `FRAME_ALLOCATOR`, and `QUARANTINED_FRAMES` invariants and passed interactive Launcher -> Calculator (`7 + 5 = 12`) -> Exit/Reap (`PID 71` on AArch64, `PID 177` on RISC-V 64) -> HomeScreen checks.

---

## 17. SMP Limitations

- **No genuine concurrent multi-core evacuation test was executed.** Remote CPU/hart behavior has **`SOURCE-REVIEWED`** + sequential single-core test coverage only.
- **Global Scheduler & Trap State:** Both architectures still use a single global `SCHEDULER` mutex, a single global `current_task_idx`, and global trap save areas rather than per-CPU/per-hart runqueues; multi-core simultaneous task execution is not supported.
- **Passive Bounded Protocol:** `reap_task_locksafe()` uses **bounded polling with the scheduler mutex released between polling attempts** (1000 iterations) and `destroy_fallible()` spins up to 100k iterations waiting for `deactivate()`. Active GIC SGI or SBI IPI preemption rendezvous is **`NOT IMPLEMENTED`**.
- **Untested Multi-Core Paths in Single-Core QEMU:** On 1 hart in RISC-V 64, `remote_mask == 0`, so `sbi::remote_sfence_vma` and `SbiRemoteFenceFailed` are not triggered at runtime; AArch64 `tlbi vae1is` / `tlbi vmalle1is` cross-core broadcast is similarly only single-CPU executed.
- **x86_64:** Remains uniprocessor; none of the Phase 8.5.3.1 AArch64/RISC-V ownership, quarantine, or lock-safe reaping features exist in `kernel/`.

---

## 18. Remaining Known Issues

1. **Quarantined Frame Re-Allocation / Re-Mapping Hardening (Minor):** `allocate_frame_with_ownership()` checks the allocation bitmap rather than `is_quarantined()`, and `retain_shared_frame()` does not reject a frame quarantined while `refcount >= 1` (frames quarantined with bitmap bit still 1 cannot be freed via `recycle_frame()` or refcount-1 `release_shared_frame()`, preventing re-allocation in normal teardown failures).
2. **`SYS_SERVICE_*` Error Collapsing & `caller_pid == 1` Assumption (`services.rs`/`syscall.rs`):** `services.rs` passes `caller_pid = 1` when calling `reap_task_locksafe(1, true, pid)`, assuming no managed daemon holds PID `1`; `SYS_SERVICE_STOP/START/RECOVER_FAILED` map supervisor `Err(())` to generic `-1` / `-22`.
3. **Wildcard `begin_reap(..., 0)` / 50-Cycle Synthetic MMU Activation:** Calling `begin_reap(sup, true, 0)` from a non-self ID could select a terminated `curr_idx` still on CPU and wait out the 1000-poll bound (`-EBUSY`); the 50-cycle ELF boot self-tests in `main.rs` switch registers via `switch_to_task` / `switch_satp` rather than `try_activate()`.
4. **Pre-existing Upstream Issues:** `cargo fmt` baseline diffs across untouched files and the pre-existing `serial::tests::proc_list_answers_with_the_process_table_on_demand` parallel `--test-threads=8` race in `kernel/`.

---

## 19. Final Acceptance Verdict

**ACCEPTANCE PASS — CLOSURE VERIFIED (within Single-CPU / Single-Hart QEMU Scope):**
- Both Phase 8.5.3.1 technical blockers (Blocker A 5-stage lock-safe evacuation and Blocker B private-frame ownership validation + quarantine protection) are verified present in published commit `7597e84cd6fd3060d509bd29da2e971d18abe02b` and preserved unmodified on `master`.
- All production reaping callers (`syscall.rs` `SYS_WAIT_PID` and `services.rs` `start_internal`, `stop`, `recover_failed`) use `reap_task_locksafe()` without holding `SCHEDULER.lock()` and enforce all 7 reaping invariants.
- [PHASE_8_5_ACCEPTANCE_MATRIX.md](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/docs/PHASE_8_5_ACCEPTANCE_MATRIX.md) and [PHASE_8_5_3_1_COMPLETION_REPORT.md](file:///c:/Users/bhara/Desktop/Interlayer/QEMU/inter-os-core/docs/PHASE_8_5_3_1_COMPLETION_REPORT.md) now accurately document `x86_64` baseline vs AArch64/RISC-V 64 implementation scope, exact evidence labels, bounded polling with released scheduler mutex terminology, and SMP single-core QEMU limitations.
- Full local checks (`cargo check`, `kernel` tests, `interos.py contracts`, `test_platform_reality.py`), single-core AArch64/RISC-V 64 QEMU boot & interactive runs passed; the single `run_all_contracts.py` failure is independently reproduced and confirmed to be a pre-existing `kernel/src/serial.rs` parallel-scheduling race unrelated to Phase 8.5.3 / 8.5.3.1.
