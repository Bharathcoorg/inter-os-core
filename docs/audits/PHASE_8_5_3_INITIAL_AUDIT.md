# InterOS Core — Phase 8.5.3 Initial Source Audit
## Scheduler Concurrency, Lock-Safe Reaping, Transactional Context Switching & Memory Reclamation

**Date:** 2026-11-12  
**Baseline Commit:** `619d4b54d817ce95756bc89e715a7cd30cf4c043`  
**Parent Baseline:** Phase 8.5.2  
**Branch:** `master`  
**Target Architectures:** `x86_64`, `aarch64`, `riscv64`  

---

## 1. Baseline Verification & Repository State

- **HEAD SHA:** `619d4b54d817ce95756bc89e715a7cd30cf4c043`
- **Branch:** `master`
- **Working Tree:** Clean (all Phase 8.5.2 changes pushed to remote `origin/master`).
- **Existing Contracts & Reality Tests:** 1214 framework tests, 27 script contracts, 67 platform reality tests, 593 kernel tests, and 3 QEMU boot suites passing.

---

## 2. Source-Backed Defect Audit & Root Cause Analysis

### Defect 1: Deadlock in `Scheduler::reap_task()` during Remote Evacuation
- **Source References:**
  - `kernel-aarch64/src/task.rs:270-295`
  - `kernel-riscv64/src/task.rs:215-240`
- **Mechanism:**
  - In `sys_waitpid`, the reaper calls `SCHEDULER.lock()`, then `sched.reap_task()`.
  - Inside `reap_task()`, while holding the global `SCHEDULER` lock, it invokes `aspace.destroy_fallible()`.
  - `destroy_fallible()` waits in a bounded loop for remote CPUs/harts that have executed in this address space to evacuate and acknowledge (`active_remote == 0 && unacknowledged == 0`).
  - However, for a remote CPU/hart to evacuate, it must execute `schedule()` / `prepare_switch()` and call `aspace.deactivate()`.
  - But `schedule()` and `prepare_switch()` require the same global `SCHEDULER` lock!
  - Therefore, the remote CPU cannot acquire the scheduler lock, cannot switch away, and cannot acknowledge evacuation until `destroy_fallible()` times out after 100,000 spins.
- **Remediation Plan:**
  - Implement a two-phase lock-safe reaping protocol:
    1. Under `SCHEDULER.lock()`: find candidate by PID, verify ownership, transition `state` to `TaskState::Reaping` (preventing any scheduler dispatch or concurrent reap), and extract (`.take()`) the `address_space: Option<UserAddressSpace>`.
    2. Drop `SCHEDULER.lock()`.
    3. Perform remote evacuation and address-space teardown (`destroy_fallible()`) *without holding the scheduler mutex*.
    4. Reacquire `SCHEDULER.lock()`:
       - If teardown succeeded: verify the task is still in `TaskState::Reaping`, remove task record from `self.tasks`, clean up IPC/display/input queues, and return exit code.
       - If teardown failed or timed out: restore `address_space` into the task, transition state to `TaskState::ReapFailed` (or keep `Terminated`), preserve task metadata, and return `-EBUSY` without dropping the task.

### Defect 2: Non-Transactional Context Switching in `prepare_switch()`
- **Source References:**
  - `kernel-aarch64/src/task.rs:314-338`
  - `kernel-riscv64/src/task.rs:356-384`
- **Mechanism:**
  - `prepare_switch()` changes `curr_idx` to `Ready`, `next_idx` to `Running`, increments `cpu_ticks`, and sets `current_task_idx = next_idx` *before* attempting `aspace.try_activate()`.
  - If `try_activate()` fails, it marks `next_idx` as `TaskState::Terminated` and returns `(None, None)`.
  - The CPU continues running the current thread, but `current_task_idx` now points to the failed candidate, which is now falsely marked `Terminated`, while `curr_idx` is marked `Ready` and tick accounting is corrupted.
- **Remediation Plan:**
  - Make context switching strictly transactional:
    1. Inspect candidate `next_idx`.
    2. Attempt `aspace.try_activate()` *first*.
    3. If activation fails: abort transaction immediately. Do NOT modify `current_task_idx`, do NOT modify `curr_idx` or `next_idx` state, do NOT increment ticks, and do NOT mark candidate `Terminated`. Return `(None, None)`.
    4. If activation succeeds: deactivate outgoing address space, then commit `curr_idx` -> `Ready`, `next_idx` -> `Running`, `cpu_ticks += 1`, `current_task_idx = next_idx`.

### Defect 3: Passive Acknowledgement vs Active Evacuation Protocol
- **Source References:**
  - `kernel-aarch64/src/mmu.rs:1541-1572`
  - `kernel-riscv64/src/mmu.rs:1005-1036`
- **Mechanism:**
  - Acknowledgement bits (`acknowledged_cpus`, `acknowledged_harts`) are updated when cores deactivate during scheduler context switch.
  - If a core is spinning in user space without an interrupt or timer tick, it depends on a timer interrupt triggering a context switch.
  - On RISC-V 64, SBI RFENCE (`sbi_remote_sfence_vma`) performs TLB shootdown via SBI, but hart mask base and mask range must be handled accurately.
- **Remediation Plan:**
  - Explicitly document the passive acknowledgement protocol and its bounded nature.
  - Ensure timeouts preserve address-space metadata and quarantine frames without treating timeouts as success.
  - On RISC-V, properly check and propagate SBI error codes and hart mask boundaries.

### Defect 4: Frame Ownership Rollback & Metadata Preservation
- **Source References:**
  - `kernel-aarch64/src/mmu.rs:859-872, 1306-1309, 1344-1347, 1421-1458`
  - `kernel-riscv64/src/mmu.rs:315-328, 775-778, 800-803, 874-910`
- **Mechanism:**
  - `mark_private_frame_unmapped` unconditionally reset `ref_counts` to 0 without asserting that the frame was allocated and mapped (`ref_counts == 1`).
  - `map_private_frame` and `map_shared_frame` rollback on `map_leaf_entry` failure ignored errors from `mark_private_frame_unmapped` and `release_shared_frame`.
  - In `unmap_user_page`, `self.mappings.remove(pos)` was called *before* leaf entry unmapping and frame recycling, losing the mapping record if unmap or recycling failed.
- **Remediation Plan:**
  - In `mark_private_frame_unmapped`, strictly verify `self.bitmap != 0`, `ownership == PrivateOwned`, and `ref_counts == 1`.
  - In `map_private_frame` / `map_shared_frame`, propagate and log rollback errors.
  - In `unmap_user_page`, only remove the mapping record after PTE invalidation, TLB shootdown, and frame recycling/release have succeeded. If release fails, retain diagnostic quarantine metadata.
  - Enhance `QUARANTINED_FRAMES` to store diagnostic metadata (`paddr`, `ownership`, `reason`).

---

## 3. Subagent Execution & Ownership Plan

To guarantee isolation without merge conflicts or overlapping edits:
- **Subagent A & B (Scheduler & Transactional Context Switch):**
  - Files: `kernel-aarch64/src/task.rs`, `kernel-riscv64/src/task.rs`
- **Subagent C & D (MMU, Evacuation & Ownership Correctness):**
  - Files: `kernel-aarch64/src/mmu.rs`, `kernel-riscv64/src/mmu.rs`, `kernel-riscv64/src/sbi.rs`
- **Subagent E (Tests, Diagnostics & Acceptance Matrix):**
  - Files: `kernel-aarch64/src/main.rs`, `kernel-riscv64/src/main.rs`, `tests/verify_*_interactive.py`, `docs/PHASE_8_5_ACCEPTANCE_MATRIX.md`

Integration will be performed sequentially and verified with the full test suite.
