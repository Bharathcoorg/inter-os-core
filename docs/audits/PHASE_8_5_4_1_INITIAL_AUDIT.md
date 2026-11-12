# InterOS Core — Phase 8.5.4.1 Initial Source Audit
## Supervisor Identity Reservation, Service-Target Authorization & Lifecycle Regression Hardening

**Date:** 2026-11-12  
**Baseline Commit:** `b86b9f5f9aa6881b1e69988adea3f5153acf6886`  
**Parent Baseline:** Phase 8.5.4 Completion (`4dfdc24c` + `b86b9f5f`)  
**Branch:** `master`  
**Target Architectures:** `aarch64`, `riscv64` (with `x86_64` scope unchanged)  

---

## 1. Baseline Verification & Repository State

- **Branch:** `master`
- **HEAD SHA:** `b86b9f5f9aa6881b1e69988adea3f5153acf6886`
- **Working Tree:** Clean (synchronized with `origin/master`)
- **Parent Commits:**
  - `b86b9f5f`: `docs(sched, mmu): record quarantine integrity and supervisor safety verification`
  - `4dfdc24c`: `fix(mmu, sched): harden shared quarantine and isolate service supervisor identity`
  - `16b35107`: `docs(sched, mmu): finalize caller audit and acceptance evidence`

---

## 2. Source-Backed Defect Audit & Root Cause Analysis

### Defect 1: Unreserved `SUPERVISOR_PID` in PID Allocators
- **Source References:**
  - `kernel-aarch64/src/task.rs:145-161`
  - `kernel-riscv64/src/task.rs:168-184`
- **Mechanism:**
  - `SUPERVISOR_PID` was introduced in Phase 8.5.4 as `0xFFFF_FFFE` (`u32::MAX - 1`).
  - However, `allocate_pid()` in both architectures only checked:
    ```rust
    if candidate == 0 || candidate == INVALID_PID {
        candidate = 1;
    }
    ```
  - When `next_pid` advanced through `0xFFFF_FFFE`, `allocate_pid()` happily assigned `SUPERVISOR_PID` to an ordinary newly-spawned task!
  - In fact, the existing test (`Test 5: PID monotonicity and collision avoidance`) specifically set `sched.next_pid = u32::MAX - 1` and asserted `assert_eq!(pid_a, u32::MAX - 1)`, demonstrating that `SUPERVISOR_PID` was returned to a caller.
- **Remediation Plan:**
  - Implement a centralized, `const fn is_reserved_pid(pid: u32) -> bool` covering `0`, `SUPERVISOR_PID`, and `INVALID_PID`.
  - Refactor `allocate_pid()` to skip all reserved IDs on advance and wraparound.
  - Return `Option<u32>` to represent ID space exhaustion safely without panics or silent collisions.

---

### Defect 2: Unchecked Supervisor Authority in Service Lifecycle Helpers
- **Source References:**
  - `kernel-aarch64/src/task.rs:510-530`
  - `kernel-riscv64/src/task.rs:530-550`
  - `kernel-aarch64/src/services.rs:154-173, 245-265, 330-348`
  - `kernel-riscv64/src/services.rs:154-173, 245-265, 330-348`
- **Mechanism:**
  - `reap_service_task(target_pid: u32)` and `terminate_service_task(sched: &mut Scheduler, target_pid: u32)` took a raw `u32` PID and directly invoked `reap_task_locksafe(SUPERVISOR_PID, true, target_pid)` and `sched.terminate_task(SUPERVISOR_PID, true, target_pid)`.
  - Because `is_supervisor` was passed as `true`, the scheduler bypassed parent ownership checks for *any* task matching `target_pid`.
  - If a caller passed the PID of an ordinary user process, an unrelated system task, or a stale PID that had been recycled, the scheduler terminated or reaped it with supervisor privilege.
- **Remediation Plan:**
  - Introduce an authenticated, strongly-typed `ServiceLifecycleToken` in `services.rs`.
  - Tokens can only be minted for registered services in `SERVICES` and encapsulate `service_id`, `pid`, and `generation`.
  - Enhance `reap_service_task` and `terminate_service_task` to require `&ServiceLifecycleToken`.
  - Validate `token.verify_live()` against the live `SERVICES` array under `SERVICE_LIFECYCLE_LOCK`.
  - In `TaskControlBlock`, record `is_service: bool` and `service_id: Option<u8>`. In `terminate_task` and `begin_reap`, verify that tasks targeted with supervisor authority are genuinely marked as services with matching `service_id`.

---

### Defect 3: Incomplete Service Lifecycle Integration Tests
- **Source References:**
  - `kernel-aarch64/src/main.rs:1680-1705`
  - `kernel-riscv64/src/main.rs:320-345`
- **Mechanism:**
  - Existing Section 46 tests only exercised basic single-service start, heartbeat checks, recovery, and stop.
  - They did not verify:
    1. Complete start-to-reap task table removal.
    2. Stale token rejection when generation advances.
    3. Rejection of supervisor operations on non-service processes.
    4. Service operation when assigned PID 1.
    5. Clean backoff and failure state accounting.
- **Remediation Plan:**
  - Implement a dedicated comprehensive test suite `test_service_supervisor_lifecycle_hardening()` in `services.rs` covering all 7 lifecycle requirements.
  - Wire this suite into the kernel boot sequence across both AArch64 and RISC-V 64 targets.

---

### Defect 4: Quarantine Concurrency Review (Workstream D)
- **Source References:**
  - `kernel-aarch64/src/mmu.rs:812-840, 1066-1086`
  - `kernel-riscv64/src/mmu.rs:254-284, 495-515`
- **Analysis:**
  - `FRAME_ALLOCATOR` and `QUARANTINED_FRAMES` use separate `spin::Mutex` locks.
  - The lock acquisition order is strictly unidirectional: `FRAME_ALLOCATOR -> QUARANTINED_FRAMES`.
  - On single-core/single-hart uniprocessor execution (`-M virt` with 1 vCPU / 1 hart), context switches and interrupts occur at deterministic points, and locks are held non-preemptively, preventing any race.
  - On true multi-core SMP, a potential race window exists:
    If CPU 0 checks `!is_quarantined(paddr)` under `FRAME_ALLOCATOR.lock()`, drops `QUARANTINED_FRAMES.lock()`, and is delayed before setting the bitmap bit, CPU 1 could simultaneously quarantine `paddr`.
  - To be fully SMP-safe in future phases, the quarantine bit should be integrated directly into the frame ownership table / allocation word under a single unified allocator lock.
  - This limitation must be documented as `SOURCE-REVIEWED` in the acceptance matrix.
