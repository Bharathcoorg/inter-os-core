# InterOS Core — Phase 8.5 Initial Source-Level Audit
## Critical Correctness Remediation: SMP TLB Shootdown, Physical Frame Ownership, Real Process Lifecycle & Service Supervisor

**Repository:** `https://github.com/Bharathcoorg/inter-os-core`  
**Canonical Branch:** `master`  
**Starting Commit:** `f7eb5038ffca51bc128f33975594c5862232682f`  
**Target Architectures:** `x86_64`, `aarch64`, `riscv64`  
**Date:** 2026-11-12  

---

## 1. Executive Summary

A comprehensive source audit of the Phase 8.4 baseline (`f7eb5038`) was conducted to evaluate multi-core SMP safety, TLB shootdown completion, physical frame ownership tracking, page table construction and rollback, real process execution lifecycle, service supervisor synchronization, and visual calculator verification.

While Phase 8.4 established basic single-core ordering and added data structures, deep inspection revealed critical safety gaps:
1. **RISC-V SBI RFNC Remote Shootdown**: Discards SBI return error codes, ignores failure, passes an unverified hart mask, and recycles frames even if remote fence was rejected or unsupported.
2. **AArch64 SMP Teardown**: Lacks active CPU tracking and quiescence checking; `destroy()` switches only the current core's `TTBR0_EL1`, allowing secondary cores to execute in torn-down address spaces.
3. **Physical Frame Ownership**: `map_user_page` accepts raw physical addresses without verifying allocator ownership or distinguishing `PrivateOwned`, `Shared`, or `Borrowed` mappings. Shared mappings do not use retain/release semantics, leading to premature frame recycling.
4. **Page Table Destruction & Drop**: `destroy()` is infallible and silently frees frames even if TLB shootdown or remote rendezvous failed, rather than quarantining them.
5. **Simulated vs Real Process Lifecycle**: The boot test in `main.rs` does not enter user mode, execute instructions, or trigger real syscalls; it merely spawns a task and sets `t.state = TaskState::Terminated` manually.
6. **Service Supervisor Integrity**: Lacks per-service synchronization for concurrent `start()` calls, and `heartbeat` does not validate service generation, risking PID reuse collisions.
7. **Calculator Visual Verification**: Verifies only digit `2` without checking digit `1`, lacks negative rejection tests for incorrect results (e.g. `13`, `17`, blank).

---

## 2. Finding Breakdown by Workstream

### Finding 1: Discarded SBI Return Code & Blind Frame Reclamation in RISC-V RFENCE
- **Classification:** Confirmed Defect
- **File and Function:** `kernel-riscv64/src/sbi.rs:remote_sfence_vma()`, `kernel-riscv64/src/mmu.rs:UserAddressSpace::destroy()`
- **Relevant Source Lines:**
  - `kernel-riscv64/src/sbi.rs:61-75`
  - `kernel-riscv64/src/mmu.rs:549-565`
- **Root Cause:** `remote_sfence_vma` maps output registers `a0` and `a1` to discarded wildcards (`lateout("a0") _`). It does not return `sbiret.error`. In `mmu.rs:destroy()`, `remote_sfence_vma(0, usize::MAX, 0, 0)` is called without checking whether the SBI firmware returned `SBI_SUCCESS` (0) or an error (`SBI_ERR_NOT_SUPPORTED`, `SBI_ERR_INVALID_PARAM`, `SBI_ERR_FAILED`). Frames are drained and recycled immediately.
- **Impact:** On platforms or hypervisors where SBI RFNC is unsupported or fails, remote harts retain stale translations while physical frames are re-allocated to other processes, resulting in cross-process memory corruption.
- **Reproduction Scenario:** Invoke `destroy()` when SBI RFNC returns error code `-2` (`SBI_ERR_NOT_SUPPORTED`). Observe all frames recycled into free bitmap without quarantine.
- **Proposed Fix:** Define `SbiRet` and `SbiError` in `sbi.rs`. Return `Result<(), SbiError>` from `remote_sfence_vma`. In `UserAddressSpace::destroy()`, if remote invalidation fails or is unsupported on multi-hart systems, do not recycle frames; place them in a quarantined list and return an explicit error.
- **Required Regression Test:** SBI RFNC error injection test verifying frame quarantine upon SBI failure.
- **Target Architecture:** `riscv64`
- **Severity:** CRITICAL

---

### Finding 2: Missing Multi-Core Inactivity Verification in AArch64 Teardown
- **Classification:** Confirmed Defect
- **File and Function:** `kernel-aarch64/src/mmu.rs:UserAddressSpace::destroy()`
- **Relevant Source Lines:**
  - `kernel-aarch64/src/mmu.rs:1078-1085`
- **Root Cause:** `destroy()` reads `mrs ttbr0_el1` of the *current CPU* only. It does not track which CPUs are actively running tasks in the address space, does not mark the address space as closing, and does not request or verify that secondary cores have switched away from `ttbr0`.
- **Impact:** In an SMP environment, if a thread is running on CPU 1 while CPU 0 destroys the address space, CPU 1 continues translating via `ttbr0`. Invalidation via `tlbi vmalle1is` triggers a page table walk on CPU 1 for deallocated tables, causing a kernel crash or data corruption.
- **Reproduction Scenario:** On a multi-core guest, execute a thread on CPU 1 while calling `destroy()` on CPU 0.
- **Proposed Fix:** Add `active_cpus: AtomicU32` and `closing: AtomicBool` to `UserAddressSpace`. Require `closing.store(true)` to prevent new activations. Verify `active_cpus == 0` (or rendezvous secondary cores to primordial `L1_TABLE`) before unmapping. If quiescence cannot be established, quarantine frames and fail teardown.
- **Required Regression Test:** Unit test asserting rejection of destruction while `active_cpus > 0` and transition to `closing`.
- **Target Architecture:** `aarch64`
- **Severity:** HIGH

---

### Finding 3: Lack of Frame Ownership Enforcement at Mapping Time
- **Classification:** Confirmed Defect
- **File and Function:** `kernel-aarch64/src/mmu.rs:map_user_page()`, `kernel-riscv64/src/mmu.rs:map_user_page()`
- **Relevant Source Lines:**
  - `kernel-aarch64/src/mmu.rs:1000-1065`
  - `kernel-riscv64/src/mmu.rs:429-514`
- **Root Cause:** `map_user_page` accepts an arbitrary `paddr: u64` without checking its ownership class in `ArmFrameAllocator`/`RiscvFrameAllocator`. Furthermore, `mapped_frames: Vec<u64>` only stores raw physical addresses with no mapping metadata (`PrivateOwned`, `Shared`, `Borrowed`).
- **Impact:**
  1. A caller can map arbitrary kernel or MMIO frames into userspace.
  2. In `destroy()`, `recycle_frame` is called for every frame in `mapped_frames`. If a `Borrowed` or `Shared` frame was mapped, `destroy()` attempts to recycle it. For shared frames, because `map_user_page` did not increment the refcount with retain semantics, the frame is freed while still in use by other address spaces.
- **Reproduction Scenario:** Allocate a `Shared` frame, map it into AddressSpace A and AddressSpace B. Destroy AddressSpace A. Observe the frame freed in the allocator while AddressSpace B retains an active mapping.
- **Proposed Fix:** Introduce explicit mapping APIs: `map_private_frame`, `map_shared_frame`, and `map_borrowed_frame`. Store `MappingRecord { paddr, kind: MappingKind }`. Enforce that `map_private_frame` requires `FrameOwnership::PrivateOwned`, `map_shared_frame` calls `retain_shared_frame(paddr)`, and `destroy()` only recycles `PrivateOwned` or decrements refcount on `Shared`, leaving `Borrowed` completely untouched.
- **Required Regression Test:** Tests for `Shared` retain/release across multiple address spaces, double-map rejection, and `Borrowed` frame immunity from recycling.
- **Target Architecture:** `aarch64`, `riscv64`
- **Severity:** CRITICAL

---

### Finding 4: Infallible Page Table Destruction & Drop Hiding Failures
- **Classification:** Confirmed Defect
- **File and Function:** `kernel-aarch64/src/mmu.rs:destroy()`, `kernel-riscv64/src/mmu.rs:destroy()`, `Drop` implementations
- **Relevant Source Lines:**
  - `kernel-aarch64/src/mmu.rs:915-921, 1071-1111`
  - `kernel-riscv64/src/mmu.rs:429-432, 522-567`
- **Root Cause:** `destroy()` returns `()`. If TLB shootdown or remote quiescence fails, `destroy()` cannot report error and recycles frames anyway. `Drop` blindly calls `self.destroy()`.
- **Impact:** Live page-table frames can be returned to the free allocator pool under error conditions, causing memory aliasing.
- **Reproduction Scenario:** Trigger a teardown failure scenario (e.g., active task or SBI error). Observe that `destroy()` returns without error and recycles memory.
- **Proposed Fix:** Implement `pub fn destroy_fallible(&mut self) -> Result<(), TeardownError>`. If teardown fails, quarantine all table and user frames in `QUARANTINED_FRAMES` list. Ensure `Drop` calls `destroy_fallible()` and safely quarantines frames without leaking them into the active allocation pool.
- **Required Regression Test:** Test teardown failure and verify frames are quarantined rather than recycled into the allocator pool.
- **Target Architecture:** `aarch64`, `riscv64`
- **Severity:** HIGH

---

### Finding 5: Simulated Process Execution in 50-Cycle Lifecycle Test
- **Classification:** Unsupported Claim in Earlier Reports / Confirmed Defect
- **File and Function:** `kernel-aarch64/src/main.rs:1507-1536`, `kernel-riscv64/src/main.rs:144-173`
- **Relevant Source Lines:**
  - `kernel-aarch64/src/main.rs:1518-1524`
  - `kernel-riscv64/src/main.rs:155-161`
- **Root Cause:** The test loads the ELF and creates the process, but then immediately loops through `sched.tasks` and manually writes:
  ```rust
  t.state = task::TaskState::Terminated;
  t.exit_code = Some(0);
  ```
  The process is never dispatched by the scheduler, never switches `TTBR0_EL1` or `satp`, never drops to Ring 3 / U-mode, never executes machine instructions, and never invokes `SYS_EXIT`.
- **Impact:** Real userspace execution, U-mode trap entry, syscall dispatching, address-space switching under execution, and the true kernel exit path are completely unexercised by the 50-cycle test.
- **Reproduction Scenario:** Inspect lines 1518-1524 of `kernel-aarch64/src/main.rs`. Notice that no context switch or CPU execution occurs.
- **Proposed Fix:** Implement genuine execution: dispatch the process into user mode, execute genuine U-mode instructions that invoke `SYS_GET_TASK_ID` and `SYS_EXIT`, handle trap return, perform real scheduler context switch away to parent, and then have parent reap the reaped child via `reap_task`.
- **Required Regression Test:** Real 50-cycle user execution test verifying actual instruction execution and clean return to baseline.
- **Target Architecture:** `aarch64`, `riscv64`
- **Severity:** CRITICAL

---

### Finding 6: Missing Concurrency Locks and Generation Verification in Service Supervisor
- **Classification:** Confirmed Defect
- **File and Function:** `kernel-aarch64/src/services.rs`, `kernel-riscv64/src/services.rs`
- **Relevant Source Lines:**
  - `kernel-aarch64/src/services.rs:125-155, 251-273`
  - `kernel-riscv64/src/services.rs:125-155, 251-273`
- **Root Cause:**
  1. `start(id)` reads `cur_state` and checks `Starting`/`Running`, but does not perform an atomic compare-exchange (`compare_exchange(Stopped, Starting)`). Two concurrent callers can both enter `start(id)` simultaneously.
  2. `heartbeat(caller_pid)` validates `svc.pid == caller_pid`, but ignores `svc.generation`. If a service process is terminated and its PID is quickly recycled before the supervisor processes cleanup, a stale process can heartbeat on behalf of the new instance.
- **Impact:** Duplicate process spawning for a single service; stale heartbeat spoofing across service restarts.
- **Reproduction Scenario:** Call `services::heartbeat(pid)` with a stale generation or simulate concurrent `start(id)` invocations.
- **Proposed Fix:**
  1. Use atomic compare-exchange on `state` or a per-service lock to serialize `start`, `stop`, and `recover_failed`.
  2. Update `heartbeat` signature and syscall to accept `(caller_pid, generation)` and assert `svc.generation.load() == generation`.
- **Required Regression Test:** Tests for concurrent start serialization and generation-mismatched heartbeat rejection.
- **Target Architecture:** `aarch64`, `riscv64`
- **Severity:** MEDIUM

---

### Finding 7: Incomplete Visual Glyph Validation & Missing Negative Cases
- **Classification:** Confirmed Defect
- **File and Function:** `tests/verify_aarch64_interactive.py:verify_calculator_result_glyphs()`, `tests/verify_riscv_interactive.py:verify_calculator_result_glyphs()`
- **Relevant Source Lines:**
  - `tests/verify_aarch64_interactive.py:212-242`
  - `tests/verify_riscv_interactive.py:212-242`
- **Root Cause:**
  1. The function verifies features of digit `2` only (`top_arc`, `right_neck`, `bottom_base`), but completely ignores digit `1` (which occupies `x=610..635, y=232..288`).
  2. There are no negative test assertions verifying that incorrect results (e.g. `13`, `17`, blank, or malformed display) are strictly rejected.
- **Impact:** If the calculator evaluated `7 + 5` to `2` (single digit) or `92`, the test would erroneously pass because digit `2`'s geometry matches.
- **Reproduction Scenario:** Pass a synthetic framebuffer with digit `2` alone or `13` to `verify_calculator_result_glyphs`.
- **Proposed Fix:**
  1. Add structural verification for digit `1`: vertical stem (`x=622..628, y=235..285`), left serif (`x=614..622, y=235..245`), bottom baseline (`x=612..634, y=280..288`), and negative space surrounding the stem.
  2. Add unit tests with negative sample frames (`13`, `17`, blank) asserting that `verify_calculator_result_glyphs` raises `AssertionError`.
- **Required Regression Test:** Negative case test suite in test runners.
- **Target Architecture:** Test infrastructure (`aarch64`, `riscv64`)
- **Severity:** MEDIUM

---

### Finding 8: Cross-Architecture Memory Reclamation Disparities in x86_64
- **Classification:** Potential Defect / Architectural Disparity
- **File and Function:** `kernel/src/task/process.rs`, `kernel/src/memory/mod.rs`
- **Root Cause:** x86_64 uses a comprehensive `ProcessTable` with `ReapError` and resource limits, but relies on CR3 reloading without SBI/IPI broadcast for SMP shootdown when tearing down user address spaces.
- **Impact:** SMP shootdown on multi-core x86_64 requires APIC IPI broadcasts (`INVVPID` or `INVLPG` broadcast).
- **Proposed Fix:** Document x86_64 SMP status accurately in the acceptance matrix, verifying single-core invariants while reporting multi-core shootdown as architecture-specific.
- **Target Architecture:** `x86_64`
- **Severity:** LOW

---

## 3. Defect Classification Summary

| ID | Title | Architecture | Severity | Classification |
|---|---|---|---|---|
| F-01 | Discarded SBI return error code in RISC-V RFENCE | `riscv64` | CRITICAL | Confirmed Defect |
| F-02 | Unverified multi-core inactivity in AArch64 teardown | `aarch64` | HIGH | Confirmed Defect |
| F-03 | Unenforced frame ownership and shared retain/release | `aarch64`, `riscv64` | CRITICAL | Confirmed Defect |
| F-04 | Infallible teardown & Drop hiding shootdown failures | `aarch64`, `riscv64` | HIGH | Confirmed Defect |
| F-05 | Simulated process execution in 50-cycle test | `aarch64`, `riscv64` | CRITICAL | Unsupported Claim / Defect |
| F-06 | Missing concurrency lock & generation check in services | `aarch64`, `riscv64` | MEDIUM | Confirmed Defect |
| F-07 | Incomplete glyph verification (missing digit 1 & negative tests) | Test Infra | MEDIUM | Confirmed Defect |
| F-08 | Cross-architecture SMP shootdown disparity in x86_64 | `x86_64` | LOW | Architectural Disparity |

---

## 4. Remediation Plan

The remediation will follow the strict phases outlined in Phase 8.5 specification:
1. **Phase 8.5A:** Correct RISC-V SBI RFENCE ABI, error propagation, and frame quarantine on shootdown failure.
2. **Phase 8.5B:** Implement address-space closing state, active CPU/hart tracking, and quiescence rendezvous on AArch64 and RISC-V.
3. **Phase 8.5C:** Enforce physical frame ownership at mapping time with explicit `PrivateOwned`, `Shared` (retain/release), and `Borrowed` semantics.
4. **Phase 8.5D:** Implement `destroy_fallible()` with quarantine fallback on teardown failure and safe Drop semantics.
5. **Phase 8.5E:** Implement real user-mode process lifecycle tests executing genuine U-mode instructions, syscalls, and exits across 50 cycles.
6. **Phase 8.5F:** Harden service supervisor state machine with atomic concurrency guards, generation verification on heartbeats, and bounded backoff.
7. **Phase 8.5G:** Enhance calculator visual validation with digit `1` structural geometry and negative test suites.
8. **Phase 8.5H:** Execute cross-architecture acceptance testing and author required documentation.
