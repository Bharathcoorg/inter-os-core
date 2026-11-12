# InterOS Core — Phase 8.5.2 Initial Audit Report
## Independent Safety Audit: Activation Races, Context Switching, TLB Invalidation, Frame Ownership, Teardown & Service Supervision

**Repository:** `https://github.com/Bharathcoorg/inter-os-core`  
**Branch:** `master`  
**Starting HEAD:** `8d27bb36ba3180b1f0dd3c13de2b0229542fe888`  
**Parent Baseline:** `e617c31961c227810a5de5caed62141091244576`  
**Audit Date:** 2026-11-12  
**Auditor:** Antigravity Integration & Architecture Lead  

---

## 1. Repository Gate & Integrity Verification

- **Branch Check:** Confirmed canonical `master`.
- **HEAD Check:** Confirmed exact commit `8d27bb36ba3180b1f0dd3c13de2b0229542fe888`.
- **Working Tree:** Confirmed clean (`nothing to commit, working tree clean`).
- **Parent Baseline:** Confirmed commit parent `e617c31961c227810a5de5caed62141091244576`.
- **Submodule / Worktree Status:** Clean and synchronized.

---

## 2. Identified Safety Defects & Root-Cause Analysis

### A. Activation Wrapper Discarding Errors (`activate()` vs `try_activate()`)
- **Location:** `kernel-aarch64/src/mmu.rs:1435-1437`, `kernel-riscv64/src/mmu.rs:879-881`
- **Defect:** `pub fn activate(&self)` calls `let _ = self.try_activate();`, silently swallowing `ActivationError::ClosingOrDestroyed` or `ActivationError::CpuIdOutOfRange`.
- **Callers Discarding Error:**
  - `kernel-aarch64/src/main.rs:1694`: `l.address_space.activate();` in user process init.
  - `kernel-riscv64/src/trap.rs:211`: `loaded.address_space.activate();` in dynamic ELF spawn.
- **Remediation:** Remove or convert `activate()` to return `Result<(), ActivationError>`, strictly propagating failure up to scheduler dispatch and process spawning.

### B. Hardware Context-Switching Lacks Translation Descheduling (`deactivate()`)
- **Location:** `kernel-aarch64/src/mmu.rs:1439-1458`, `kernel-riscv64/src/mmu.rs:883-902`, `kernel-aarch64/src/task.rs:313-324`, `kernel-riscv64/src/task.rs:355-367`
- **Defect:** `deactivate()` merely clears the CPU/hart bit in `lifecycle: AtomicU32 / AtomicU64`. It does **not** switch the hardware translation context (`TTBR0_EL1` on AArch64, `SATP` on RISC-V) away from the user table to the kernel root table before clearing the active mask.
- **Risk:** Hardware TLB / MMU speculative walkers can continue referencing physical page table frames after `deactivate()` returns, creating a race with `destroy_fallible()`.
- **Remediation:** In `deactivate()`, explicitly switch the hardware translation register to the kernel table (`ArmMmu::switch_to_kernel()` / `switch_to_kernel_satp()`) and execute memory/instruction barriers (`dsb ish; isb` / `sfence.vma`) **before** clearing the CPU/hart bit in `lifecycle`.

### C. Scheduler Does Not Integrate Address-Space Activation in `schedule()`
- **Location:** `kernel-aarch64/src/task.rs:320-324`, `kernel-riscv64/src/task.rs:362-367`
- **Defect:** The scheduler switches raw `page_table: Option<u64>` without calling `aspace.try_activate()` or deactivating the previous task's address space. If an address space was marked `CLOSING` by a supervisor or sibling thread, the scheduler blindly re-activates it into hardware.
- **Remediation:** In `Scheduler::schedule()` / `prepare_switch()`:
  - Deactivate the outgoing task's `address_space` if switching away.
  - Call `try_activate()` on the incoming task's `address_space`.
  - If `try_activate()` fails, abort the context switch to that task, mark it as `Terminated`, revert to kernel translation, and dispatch the next eligible task.

### D. Multi-Core Rendezvous & Acknowledged Invalidation
- **Location:** `kernel-aarch64/src/mmu.rs:1480-1498`, `kernel-riscv64/src/mmu.rs:955-972`
- **Defect:** AArch64 uses `tlbi vmalle1is` which broadcasts across the inner-shareable domain, but does not track explicit core-by-core rendezvous acknowledgements for all cores that previously executed the address space. RISC-V `sbi::remote_sfence_vma` call only passed a single machine-word mask with base `0`, failing to handle multi-word hart clusters or hart IDs >= 64.
- **Remediation:**
  - AArch64: Maintain `acknowledged_cpus: AtomicU32`. When cores deactivate, they acknowledge context evacuation. `destroy_fallible()` enforces that all cores in `executed_cpus` have acknowledged prior to reclaiming frames.
  - RISC-V 64: Correctly handle arbitrary hart IDs and multi-word hart masks via chunked RFNC calls or `hart_mask_base = usize::MAX`. Verify SBI return statuses on all invocations.

### E. Physical Frame Ownership & Allocator Bypass
- **Location:** `kernel-aarch64/src/mmu.rs:825, 913-924`, `kernel-riscv64/src/mmu.rs:265, 353-364`
- **Defect:**
  1. `recycle_frame()` accepted `FrameOwnership::Shared` and manipulated refcounts, bypassing `release_shared_frame()`.
  2. `PrivateOwned` frames did not track whether they were already mapped, allowing potential multi-mapping of supposed private frames.
  3. `is_valid_mmio_peripheral()` on RISC-V allowed a broad 1.25 GB PCIe window (`0x3000_0000..0x8000_0000`).
- **Remediation:**
  - `recycle_frame()` strictly rejects `Kernel`, `Mmio`, `Borrowed`, and `Shared` frames (only `PrivateOwned` with `ref_count == 0` can be recycled).
  - Explicit transfer of ownership on `map_private_frame`: transitions from unmapped (`ref_count == 0`) to mapped (`ref_count == 1`). Duplicate mapping attempts are rejected.
  - Narrow MMIO aperture validation with overflow-checked `paddr.checked_add(len)` bounds.

### F. Teardown Failure Propagation & Scheduler Reaping Discard
- **Location:** `kernel-aarch64/src/task.rs:286-289`, `kernel-riscv64/src/task.rs:231-234`
- **Defect:** `reap_task` used `let _ = aspace.destroy_fallible();` and emitted success logs `[REAP] Reclaimed user address space...` even if teardown failed. Furthermore, it removed the task before confirming destruction.
- **Remediation:** Run `aspace.destroy_fallible()` before removing the task. If teardown fails, return `Err(-16)` (`-EBUSY`), preserve the task in the scheduler, and quarantine frames without emitting false success logs.

### G. Service Supervisor Concurrency & Heartbeat Validation
- **Location:** `kernel-aarch64/src/services.rs:148-158`, `kernel-riscv64/src/services.rs:148-158`
- **Defect:** `let _ = sched.terminate_task(...)` discarded errors. If termination failed, it proceeded to call `reap_task`.
- **Remediation:** Check `sched.terminate_task(...)`. If it fails, abort recovery/start and preserve `svc.pid`. Serialize all service lifecycle operations under a dedicated lifecycle mutex.

### H. Calculator Visual Negative Tests Evaluated Argument Strings Rather Than Images
- **Location:** `tests/verify_aarch64_interactive.py:291-297`, `tests/verify_riscv_interactive.py:291-297`
- **Defect:** `verify_calculator_result_glyphs(tmp_ppm, bad_res)` tested `expected_result != "12"`, failing on function assertion rather than analyzing visual image content.
- **Remediation:** Generate synthetic images containing erroneous glyphs (`"13"`, `"17"`, `"0"`, `"2"`, missing digit, incorrect spacing) and pass them to `verify_calculator_result_glyphs(tmp_ppm, "12")` to strictly verify that the image visual analyzer rejects them.

---

## 3. Work Package Action Plan

1. **WP-1: Activation, Context Switch & Deactivation Hardening** (`mmu.rs`, `task.rs`)
2. **WP-2: Remote Rendezvous & Hart Invalidation** (`mmu.rs`, `sbi.rs`)
3. **WP-3: Physical Frame Ownership & Allocator Invariants** (`mmu.rs`)
4. **WP-4: Transactional Teardown & Reaping Safety** (`mmu.rs`, `task.rs`)
5. **WP-5: Service Supervisor Serialization & Monotonic Backoff** (`services.rs`)
6. **WP-6: Calculator Synthetic Image Rejection Suite** (`tests/verify_*_interactive.py`)
7. **WP-7: Truthful Acceptance Matrix Reconciliation** (`docs/PHASE_8_5_ACCEPTANCE_MATRIX.md`)
8. **WP-8: Full Suite Execution & Remote Delivery** (Local QEMU & contracts verification)
