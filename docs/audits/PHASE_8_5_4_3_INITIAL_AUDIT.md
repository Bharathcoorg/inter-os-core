# InterOS Core — Phase 8.5.4.3 Initial Audit
## Final Internal API Boundary Cleanup

**Repository:** `Desktop/interlayer/QEMU/inter-os-core`  
**Canonical branch:** `master`  
**Starting HEAD:** `55e19034f9baa7e0ebef853c28cca495f29587f2`  
**Date:** 2026-11-13  
**Mode:** Implementation-first, local verification only, no CI.

---

## 1. Audit Scope & Objectives

Following Phase 8.5.4.2 (which decoupled incarnation generation, eliminated silent PID clearing, and introduced query accessors), this phase completes the internal API boundary encapsulation across `kernel-aarch64` and `kernel-riscv64`:

1. **Audit Production Test-Only Token Construction:**
   - Audit `ServiceLifecycleToken::forge_for_test()`.
   - Remove `forge_for_test` from production builds; reserve it strictly behind `#[cfg(test)]`.
   - Ensure normal production modules cannot construct arbitrary lifecycle tokens.
   - Use private internal constructor `issue()` as the sole production token issuance path.
   - Disallow `#[doc(hidden)]` as an access control bypass.

2. **Audit Service Registry Mutation & Visibility:**
   - Audit `ServiceEntry` and `SERVICES`.
   - Confirm whether any external module outside `services.rs` directly references `SERVICES` or its fields.
   - Restrict `ServiceEntry` and `SERVICES` to private visibility within `services.rs`.
   - Ensure all read-only access uses `service_pid()`, `service_generation()`, `service_state()`, or `snapshot()`.
   - Provide `is_valid_service_id(id: u8) -> bool` to check registered services without exposing registry internals.

3. **Audit Service Process Creation (`Scheduler::spawn_service_process`):**
   - Restrict `spawn_service_process` from `pub` to `pub(crate)`.
   - Validate that `service_id` is a registered service (`is_valid_service_id(service_id)`).
   - Validate that `generation` is nonzero (`generation > 0`).
   - Reject invalid service processes immediately before PID allocation.

---

## 2. Findings from Source Inspection

### 2.1 `ServiceLifecycleToken::forge_for_test()`
- **Current Declaration:**
  `#[doc(hidden)] pub(crate) fn forge_for_test(service_id: u8, pid: u32, generation: u32) -> Self` in both `kernel-aarch64/src/services.rs` and `kernel-riscv64/src/services.rs`.
- **Call Sites:**
  Called only within `test_service_supervisor_lifecycle_hardening()` inside `services.rs`.
- **Finding:**
  Because `forge_for_test` had `pub(crate)` visibility, any kernel module could invoke it despite `#[doc(hidden)]`.
- **Resolution:**
  Gate `forge_for_test` behind `#[cfg(test)]`. In production builds, `forge_for_test` is completely absent. The in-kernel self-test in `services.rs` will construct invalid test tokens via a local closure within `test_service_supervisor_lifecycle_hardening()`, fully contained within the module.

### 2.2 `ServiceEntry` and `SERVICES`
- **Current Visibility:**
  `pub struct ServiceEntry { pub ... }` and `pub static SERVICES: [ServiceEntry; 12] = [ ... ];`.
- **External Consumers:**
  A full grep across `kernel-aarch64` and `kernel-riscv64` confirms **zero** external modules access `SERVICES` directly (`main.rs` was already migrated to `service_generation` in Phase 8.5.4.2).
- **Finding:**
  Both `ServiceEntry` and `SERVICES` can be downgraded to private (`struct ServiceEntry` and `static SERVICES`).
- **Resolution:**
  Make `ServiceEntry` private with private fields. Make `SERVICES` private. Add `pub fn is_valid_service_id(id: u8) -> bool`. Replace all remaining direct reads in `services.rs` tests with `service_state()`, `service_pid()`, and `service_generation()`.

### 2.3 `Scheduler::spawn_service_process`
- **Current Visibility:**
  `pub fn spawn_service_process(...) -> u32` in both `kernel-aarch64/src/task.rs` and `kernel-riscv64/src/task.rs`.
- **Call Sites:**
  Called only by `services::start_internal` (line 307) and `task::test_task_lifecycle` (test only).
- **Finding:**
  It lacked parameter validation: accepting `generation == 0` or arbitrary out-of-range `service_id`.
- **Resolution:**
  Downgrade visibility to `pub(crate)`. Prepend validation:
  ```rust
  if generation == 0 || !crate::services::is_valid_service_id(service_id) {
      return 0;
  }
  ```
