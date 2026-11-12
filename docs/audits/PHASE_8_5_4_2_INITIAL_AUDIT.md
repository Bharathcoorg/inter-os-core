# InterOS Core — Phase 8.5.4.2 Initial Source Audit
## Lifecycle Capability Encapsulation, Reserved-ID Registration Audit & Lifecycle Consistency

**Date:** 2026-11-13  
**Baseline Commit:** `5636f19ea7513bdb6ddadffa44c4f2f2ded5b7db`  
**Branch:** `master`  
**Targets Audited:** `kernel-aarch64`, `kernel-riscv64`, `kernel` (x86_64 scope verification)  

---

## 1. Audit Objective

This audit investigates the live source code across AArch64 and RISC-V 64 kernel trees to identify:
1. Unrestricted public visibility or ambient issuance paths for `ServiceLifecycleToken`.
2. Gaps in lifecycle transition consistency, error handling, and state preservation in `services.rs`.
3. All task registration and insertion paths across both architectures to ensure reserved PIDs (`0`, `SUPERVISOR_PID`, `INVALID_PID`) cannot be registered explicitly or inadvertently.
4. Completeness of supervisor lifecycle regression tests against the 10 mandatory evaluation criteria.

---

## 2. Findings by Workstream

### 2.1 Workstream A: Lifecycle Token Encapsulation & Visibility
- **Current Source State:**
  - `kernel-aarch64/src/services.rs` and `kernel-riscv64/src/services.rs` define `ServiceLifecycleToken` with `pub fn for_service()` and `pub const fn new()`.
  - `pub static SERVICES: [ServiceEntry; 12]` exposes raw atomic PID, generation, and state fields to any module in the kernel.
  - Any external kernel module can call `ServiceLifecycleToken::new(...)` or `ServiceLifecycleToken::for_service(&SERVICES[i])` and invoke `reap_service_task` or `terminate_service_task`.
- **Remediation Plan:**
  - Remove `pub const fn new()` and `pub fn for_service()`.
  - Provide an internal issuance helper `fn issue_lifecycle_token(entry: &ServiceEntry) -> Option<ServiceLifecycleToken>` restricted to `services.rs`.
  - Keep `ServiceLifecycleToken` fields strictly private.
  - Make `reap_service_task` and `terminate_service_task` `pub(crate)` instead of `pub`.
  - Make `SERVICES` private to `services.rs` (or restrict atomic field mutations), providing safe query helpers `service_pid(id: u8) -> Option<u32>` and `service_generation(id: u8) -> Option<u32>` for `main.rs`.
  - For negative test cases in `services.rs`, use an internal `forge_test_token()` helper not exposed outside `services.rs`.

### 2.2 Workstream B: Consistency of Authorization & Lifecycle Transitions
- **Current Source State:**
  - In `start_internal()`, `stop()`, and `recover_failed()`, if `issue_token` returns `None` or if a check fails, the fallback `else` branch executes `svc.pid.store(0, Ordering::Release)`. This silently clears the registered PID while leaving an un-reaped task in the scheduler table!
  - `spawn_service_process()` sets `t.generation = 1` on the `TaskControlBlock`, ignoring the actual service incarnation generation. Consequently, `reap_service_task` and `terminate_service_task` only verify `is_service` and `service_id`, but cannot verify that `t.generation == token.generation()`.
- **Remediation Plan:**
  - Pass `generation: u32` to `spawn_service_process()` and store it on `TaskControlBlock`.
  - In `terminate_service_task` and `reap_service_task`, verify `t.generation == token.generation()`.
  - Eliminate all paths that silently clear `svc.pid` to 0 on failure. On termination or reap failure, keep `svc.pid` recorded in the registry and transition `svc.state` to `Crashed` so recovery mechanisms know a process is still pending cleanup.
  - Maintain strict lock ordering: `SERVICE_LIFECYCLE_LOCK -> SCHEDULER.lock()`. Ensure `SCHEDULER.lock()` is dropped before `reap_service_task` enters lock-safe Stage 2 bounded evacuation polling.

### 2.3 Workstream C: Task Registration Paths for Reserved PIDs
- **Audit of Insertion Sites:**
  1. `kernel-riscv64/src/task.rs`: `add_task(_id: TaskId, ...)`
     - Currently allows explicit `_id.0 != 0` without validating `is_reserved_pid(_id.0 as u32)`.
     - Explicit registration with `0`, `SUPERVISOR_PID`, or `INVALID_PID` must return `false`.
     - Duplicate PID check `self.tasks.iter().any(|t| t.id == task_id)` must reject collisions.
  2. `kernel-aarch64/src/task.rs`:
     - Does not currently have an explicit `add_task()` method. Add `pub fn add_task(&mut self, id: u32, ...)` with symmetric validation rejecting `is_reserved_pid(id)`, duplicate IDs, and `MAX_TASKS` exhaustion.
  3. Dynamic allocators (`allocate_pid`):
     - Both architectures skip `0`, `SUPERVISOR_PID`, `INVALID_PID`, wrap at `SUPERVISOR_PID - 1` to `1`, and return `None` on exhaustion.
  4. `spawn_user_process`, `spawn_service_process`, `spawn_with_caps`:
     - Allocate via `allocate_pid()`, guaranteeing no reserved PIDs are assigned dynamically.

### 2.4 Workstream D: Lifecycle Regression Suite
- Expand `test_service_supervisor_lifecycle_hardening()` from 7 to 10 comprehensive tests:
  1. Real service start through service manager.
  2. Stop through authenticated lifecycle path.
  3. Restart with old task removal and generation advancement.
  4. Crash recovery with verified termination and reap.
  5. Ordinary user process rejection (`-EPERM`).
  6. Stale PID / generation token rejection (`-EPERM`).
  7. Service registration mismatch rejection (`-EPERM`).
  8. Explicit reserved PID registration rejection (`0`, `SUPERVISOR_PID`, `INVALID_PID`).
  9. Failure paths preserving service PID without silent clearing.
  10. Verification of process absence and replacement integrity.

---

## 3. Scope & Execution Plan

- Modify only `kernel-aarch64/src/services.rs`, `kernel-aarch64/src/task.rs`, `kernel-aarch64/src/main.rs`, `kernel-riscv64/src/services.rs`, `kernel-riscv64/src/task.rs`, and `kernel-riscv64/src/main.rs`.
- Do not modify `kernel/` (`x86_64`).
- Verify all builds, host tests, contracts, boot tests, and interactive tests locally without CI.
