# InterOS Core — Phase 8.5.4 Initial Source Audit
## Quarantine Integrity, Shared-Frame Hardening & Service Identity Safety

**Date:** 2026-11-12  
**Baseline Commit:** `16b35107db6b991a4a4ba015bd6d7566da5823b7`  
**Parent Baseline:** Phase 8.5.3.2 Completion  
**Branch:** `master`  
**Target Architectures:** `x86_64`, `aarch64`, `riscv64`  

---

## 1. Baseline Verification & Repository State

- **HEAD SHA:** `16b35107db6b991a4a4ba015bd6d7566da5823b7`
- **Branch:** `master` (synchronized with `origin/master`)
- **Working Tree:** Clean
- **Platform Reality & Contract Baseline:** All 1214 framework tests, 27 script contracts, 67 platform reality tests, 593 kernel tests, and 3 QEMU boot suites passing.

---

## 2. Source-Backed Defect Audit & Root Cause Analysis

### Workstream A: Quarantined Shared-Frame Integrity & Rejection of Allocation/Mapping

#### 1. Allocation of Quarantined Frames
- **Source References:**
  - `kernel-aarch64/src/mmu.rs:814-834`
  - `kernel-riscv64/src/mmu.rs:254-274`
- **Mechanism:**
  - `allocate_frame_with_ownership()` scans words in `self.bitmap` looking for a zero bit (`*word != !0u64`).
  - When a zero bit is found, it immediately clears/sets the bit and returns the frame address `paddr = self.base_paddr + (frame_idx as u64) * 4096`.
  - It does NOT check `!is_quarantined(paddr)`.
  - If a frame entered quarantine while unallocated (e.g. from a double-free attempt, speculative error, or corrupted state), or if an administrator quarantined a frame, the frame allocator would happily hand it out to a new caller.
- **Remediation Plan:**
  - In `allocate_frame_with_ownership()`, evaluate each candidate bit; if `is_quarantined(paddr)` is true, skip this frame and continue scanning subsequent bits/words.
  - Reject allocation of any physical frame currently in the `QUARANTINED_FRAMES` list.

#### 2. Shared-Frame Retention on Quarantined Frames
- **Source References:**
  - `kernel-aarch64/src/mmu.rs:886-907`
  - `kernel-riscv64/src/mmu.rs:335-347`
- **Mechanism:**
  - `retain_shared_frame()` checks that the frame is allocated and owned as `FrameOwnership::Shared`, and increments `ref_counts[frame_idx]`.
  - It does NOT check `is_quarantined(paddr)`.
  - In Phase 8.5.3.1, `release_shared_frame()` was hardened to reject recycling a quarantined frame at refcount 1 (`if self.ref_counts[frame_idx] == 1 && is_quarantined(paddr)`). However, if an error occurred during teardown of one address space and the frame was quarantined while still referenced by another address space (`refcount > 1`), a third address space could call `retain_shared_frame()` and map the quarantined frame, propagating dangerous state.
- **Remediation Plan:**
  - In `retain_shared_frame()`, explicitly check `if is_quarantined(paddr) { return Err("Cannot retain quarantined shared frame"); }`.

#### 3. Mapping-Time Quarantine Guards
- **Source References:**
  - `kernel-aarch64/src/mmu.rs:1341-1430`
  - `kernel-riscv64/src/mmu.rs:800-880`
- **Mechanism:**
  - `map_private_frame()`, `map_shared_frame()`, and `map_borrowed_frame()` validate alignments, bounds, and permissions, but do not verify whether `is_quarantined(paddr)` is true prior to writing page table entries.
- **Remediation Plan:**
  - Add explicit `if is_quarantined(paddr) { return Err("Cannot map quarantined frame"); }` to `map_private_frame()`, `map_shared_frame()`, and `map_borrowed_frame()`.
  - Ensure `mark_private_frame_mapped()` also rejects quarantined frames.

#### 4. Metadata and Diagnostic Reason Preservation
- **Source References:**
  - `kernel-aarch64/src/mmu.rs:1068-1073`
  - `kernel-riscv64/src/mmu.rs:497-502`
- **Mechanism:**
  - `quarantine_frame_with_meta()` correctly checks `if !q.iter().any(|entry| entry.paddr == paddr)`.
  - Verify that subsequent rejected operations or teardown attempts preserve the initial fault reason and ownership without overwriting or clearing the entry.

---

### Workstream B: Service Supervisor Identity Safety & Ambient Authority Elimination

#### 1. Hardcoded Caller PID 1 in `services.rs`
- **Source References:**
  - `kernel-aarch64/src/services.rs:157, 163, 251, 256, 334, 339`
  - `kernel-riscv64/src/services.rs:157, 163, 251, 256, 334, 339`
- **Mechanism:**
  - `start_internal()`, `stop()`, and `recover_failed()` call:
    - `sched.terminate_task(1, true, old_pid)`
    - `crate::task::reap_task_locksafe(1, true, old_pid)`
  - Passing `caller_pid = 1` was based on an unverified assumption that PID 1 is always the supervisor process.
  - If a daemon service task is itself assigned PID 1 by the scheduler, `reap_task_locksafe` executes:
    `if target_pid != 0 && target_pid == caller_pid { return Err(-16); }`
    Because `1 == 1`, the service manager cannot stop, restart, or reap its own service task!
  - Furthermore, fabricating a PID 1 caller identity is an ambient authority antipattern.
- **Remediation Plan:**
  - Define an authentic, dedicated internal kernel supervisor identity:
    `pub const SUPERVISOR_PID: u32 = 0xFFFF_FFFE;`
  - Expose dedicated internal supervisor helpers in `task.rs`:
    - `pub fn reap_service_task(target_pid: u32) -> Result<i32, i64>`
    - `pub fn terminate_service_task(sched: &mut Scheduler, target_pid: u32) -> Result<(), i64>`
  - Ordinary userspace tasks cannot impersonate `SUPERVISOR_PID` because task IDs are sequentially allocated (0..255), and syscall `get_caller_identity_and_caps()` strictly derives the caller PID from the executing task.
  - In `services.rs`, replace all hardcoded `(1, true, pid)` invocations with `terminate_service_task` and `reap_service_task`.

#### 2. Lock Hierarchy Preservation
- **Source References:**
  - `kernel-aarch64/src/services.rs:244-256`
  - `kernel-riscv64/src/services.rs:244-256`
- **Mechanism:**
  - `SERVICE_LIFECYCLE_LOCK` is acquired before service lifecycle transitions.
  - `reap_task_locksafe()` internally manages `SCHEDULER.lock()` across its 5 stages, dropping the scheduler lock during bounded evacuation wait and page table destruction.
  - Verify that `SCHEDULER.lock()` is never held across calls to `reap_service_task()`.
  - The lock order `SERVICE_LIFECYCLE_LOCK -> SCHEDULER.lock()` must be strictly maintained.

---

### Workstream C: Reaping Wildcard and Target Identity Safety

#### 1. Wildcard PID 0 Task Selection Failure
- **Source References:**
  - `kernel-aarch64/src/task.rs:301-310`
  - `kernel-riscv64/src/task.rs:234-243`
- **Mechanism:**
  - In `begin_reap(caller_pid, is_supervisor, target_pid)`:
    ```rust
    let pos = self.tasks.iter().position(|t| {
        (target_pid == 0 || t.id == target_pid)
            && (target_pid != 0 || t.id != caller_pid)
            && (t.state == TaskState::Terminated || t.state == TaskState::Exited || t.state == TaskState::ReapFailed)
    }).ok_or(-30i64)?;

    let task = &mut self.tasks[pos];
    if !is_supervisor && task.parent_id != caller_pid {
        return Err(-13); // -EACCES
    }
    ```
  - When `target_pid == 0` (standard `waitpid(0)` / wait-for-any-child), `.position()` finds the *first* terminated/exited task in `self.tasks`.
  - If another process's child happens to be earlier in the task array than the caller's child, `.position()` selects that foreign task.
  - The subsequent check `if !is_supervisor && task.parent_id != caller_pid` rejects with `-EACCES (-13)`.
  - As a result, the caller's own terminated children are never inspected or reaped!
- **Remediation Plan:**
  - Move the ownership filter `(is_supervisor || t.parent_id == caller_pid)` *inside* the `.position()` predicate.
  - Ensure that when `target_pid == 0`:
    - The candidate must belong to the caller (or caller must be supervisor).
    - The candidate cannot be the caller itself (`t.id != caller_pid`).
    - The candidate cannot be task 0 (kernel / idle task).
    - If `target_pid != 0`, target-specific identity checks remain intact.

---

## 3. Subagent Workstream Assignments

1. **Workstream A (MMU Quarantine & Shared-Frame Hardening):**
   - Files: `kernel-aarch64/src/mmu.rs`, `kernel-riscv64/src/mmu.rs`
   - Actions: Allocator quarantine skip, `retain_shared_frame` quarantine rejection, `map_*_frame` quarantine check, regression unit tests in both kernels.

2. **Workstream B (Service Supervisor Identity & Lock Safety):**
   - Files: `kernel-aarch64/src/task.rs`, `kernel-riscv64/src/task.rs`, `kernel-aarch64/src/services.rs`, `kernel-riscv64/src/services.rs`
   - Actions: Define `SUPERVISOR_PID`, add `reap_service_task` / `terminate_service_task`, replace hardcoded PID 1 calls in `services.rs`.

3. **Workstream C (Wildcard Reaping Target Selection):**
   - Files: `kernel-aarch64/src/task.rs`, `kernel-riscv64/src/task.rs`
   - Actions: Fix `.position()` predicate in `begin_reap` to check `(is_supervisor || t.parent_id == caller_pid)`, add regression tests covering wildcard reaping with multiple interleaved child tasks.

4. **Integration & Full Platform Verification:**
   - Full test pipeline across `x86_64`, `aarch64`, `riscv64`.
   - Update `docs/PHASE_8_5_ACCEPTANCE_MATRIX.md` and generate `docs/PHASE_8_5_4_COMPLETION_REPORT.md`.
