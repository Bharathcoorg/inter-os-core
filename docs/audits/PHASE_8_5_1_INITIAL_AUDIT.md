# InterOS Core — Phase 8.5.1 Initial Source-Level Audit
## Safety Closure: SMP Address-Space Lifecycle, Physical Frame Ownership, Service Supervision & Verifiable Process Tests

**Repository:** `https://github.com/Bharathcoorg/inter-os-core`  
**Canonical Branch:** `master`  
**Starting Commit:** `e617c31961c227810a5de5caed62141091244576`  
**Target Architectures:** `x86_64`, `aarch64`, `riscv64`  
**Date:** 2026-11-12  

---

## 1. Executive Summary & Audit Baseline

Before modifying any file, the repository environment was audited:
- Current branch: `master`
- Exact HEAD: `e617c31961c227810a5de5caed62141091244576`
- Working tree: clean (`git status --short` returned no changes)

This audit examines the changes introduced in Phase 8.5 (`e617c319`) and previous commits to establish an evidence-based gap analysis across memory management, TLB invalidation, process lifecycle, service supervision, visual verification, and acceptance ledger truthfulness.

---

## 2. Source-Level Defect Analysis & Traceability

### Finding 1: TOCTOU Race Condition between Address-Space Activation and Destruction
- **Classification:** Confirmed Defect
- **Files & Lines:**
  - `kernel-aarch64/src/mmu.rs`: lines 1351–1362 (`activate`), 1372–1384 (`destroy_fallible`)
  - `kernel-riscv64/src/mmu.rs`: lines 777–790 (`activate`), 800–812 (`destroy_fallible`)
- **Trace & Call Sites:**
  - `UserAddressSpace::activate()` is called by `ArmMmu::switch_to_task` or before entering user mode (`main.rs:1687`, `main.rs:1543`).
  - `UserAddressSpace::destroy_fallible()` is called by `destroy()` (`mmu.rs:1434`, `mmu.rs:888`) and `Drop::drop()` (`mmu.rs:1054`, `mmu.rs:430`).
- **Mechanism of Failure:**
  1. In `activate()`, a CPU/hart checks `if self.closing.load(Ordering::SeqCst) { return; }`.
  2. Simultaneously, another CPU calls `destroy_fallible()`, setting `self.closing.store(true, Ordering::SeqCst)`.
  3. The destroying CPU reads `active_cpus` (or `active_harts`), observes `0`, and begins draining `mappings` and recycling table frames.
  4. The activating CPU, having passed the `closing` check, executes `self.active_cpus.fetch_or(...)` and loads `TTBR0_EL1` / `satp` with page tables currently being deallocated.
  5. Furthermore, `activate()` returns `()`, providing no status to the caller that activation failed when `closing` is true, leaving the CPU running with an undefined translation context.
- **Severity:** CRITICAL
- **Required Remediation:**
  - Implement an atomic lifecycle protocol (`Active(u32)`, `Closing`, `Destroyed`).
  - `activate()` must atomically check state and acquire permission to enter, returning `Result<(), ActivationError>`.
  - Closing must transition the state such that subsequent activations fail atomically without touching hardware translation registers.

---

### Finding 2: False Claims of x86_64 SMP TLB Shootdown Parity
- **Classification:** Confirmed Defect & False Acceptance Claim
- **Files & Lines:**
  - `docs/PHASE_8_5_ACCEPTANCE_MATRIX.md`: line 11 (claims `x86_64 Status: VERIFIED (APIC IPI)`)
  - `kernel/src/memory.rs`: lines 621–640
- **Trace & Analysis:**
  - Inspection of `kernel/src` reveals that `memory.rs` flushes TLBs only on the current local CPU via `invlpg` or reloading `CR3`.
  - There is no APIC Inter-Processor Interrupt (IPI) delivery, remote TLB shootdown broadcast, or remote acknowledgment handler implemented in `kernel/src`.
  - Phase 8.5 acceptance matrix falsely claimed `VERIFIED (APIC IPI)`.
- **Severity:** HIGH (Violation of Platform Truth Contract)
- **Required Remediation:**
  - Correct `docs/PHASE_8_5_ACCEPTANCE_MATRIX.md` to truthfully reflect x86_64 SMP TLB shootdown as `NOT IMPLEMENTED`.
  - Document the exact missing architectural components (APIC ICR IPI dispatch, remote invalidation ISR, ACK barrier).

---

### Finding 3: RISC-V SBI RFNC Remote Shootdown Mask & Fallback Deficiencies
- **Classification:** Confirmed Defect
- **Files & Lines:**
  - `kernel-riscv64/src/sbi.rs`: lines 125–134 (`remote_sfence_vma`)
  - `kernel-riscv64/src/mmu.rs`: lines 836–855 (`destroy_fallible`)
- **Trace & Analysis:**
  - `remote_sfence_vma` takes `(hart_mask: usize, hart_mask_base: usize, start_addr: usize, size: usize)`.
  - In `mmu.rs:837-841`, `remote_mask` is computed as `(executed | online) & !current_mask`, and `hart_mask_base` is hardcoded to `0`. If a system has harts numbered >= 64, this bitmask silently truncates or misaddresses harts.
  - According to the SBI specification, passing `hart_mask_base = usize::MAX` directs the firmware to target all other harts automatically.
  - In `sbi.rs`, `probe_extension(SBI_EXT_RFNC)` is provided but never called before issuing `remote_sfence_vma`. If firmware does not implement RFNC, `remote_sfence_vma` returns `Err(SbiError::NotSupported)`, leading to unconditional quarantine on platforms that could otherwise be handled cleanly.
- **Severity:** MEDIUM
- **Required Remediation:**
  - Check SBI RFNC extension availability via `probe_extension(SBI_EXT_RFNC)`.
  - Support `hart_mask_base = usize::MAX` when all other harts must be targeted.
  - Propagate errors accurately and ensure completion before frame recycling.

---

### Finding 4: Permissive MMIO Frame Fallback in Page Allocator
- **Classification:** Confirmed Defect
- **Files & Lines:**
  - `kernel-aarch64/src/mmu.rs`: lines 1296–1303
  - `kernel-riscv64/src/mmu.rs`: lines 728–734
- **Trace & Analysis:**
  - In `kernel-aarch64/src/mmu.rs`:
    ```rust
    _ => {
        let base = ACTIVE_RAM_BASE.load(Ordering::Relaxed);
        if paddr < base {
            self.map_borrowed_frame(vaddr, paddr, is_write, is_exec)
        } else { ... }
    }
    ```
  - In `kernel-riscv64/src/mmu.rs`:
    ```rust
    _ => {
        if paddr < 0x8800_0000 {
            self.map_borrowed_frame(vaddr, paddr, is_write, is_exec)
        } else { ... }
    }
    ```
  - Any arbitrary unregistered physical address is automatically accepted as `Borrowed` merely because it is below a numeric boundary.
  - This allows invalid, unaligned, or unintended physical memory addresses to bypass ownership enforcement and become mapped into user address spaces.
- **Severity:** HIGH
- **Required Remediation:**
  - Eliminate broad numeric fallback logic.
  - Require explicit ownership registration or membership in a documented, validated MMIO peripheral range table (e.g. PL011, GIC, VirtIO apertures).
  - Explicitly reject unaligned physical addresses and out-of-range frames.

---

### Finding 5: Silently Discarded Cleanup Errors during Teardown and `Drop`
- **Classification:** Confirmed Defect
- **Files & Lines:**
  - `kernel-aarch64/src/mmu.rs`: lines 1057, 1336, 1339, 1416, 1419, 1427, 1435
  - `kernel-riscv64/src/mmu.rs`: lines 431, 766, 769, 870, 873, 881, 889
- **Trace & Analysis:**
  - Multiple sites use `let _ = recycle_frame(...)` and `let _ = release_shared_frame(...)`.
  - In `destroy()`, `let _ = self.destroy_fallible()` discards any teardown error.
  - In `Drop::drop()`, `let _ = self.destroy_fallible()` silently drops errors.
  - If a frame cannot be recycled (e.g. wrong ownership or allocator corruption), the failure is silently ignored, causing undetected frame leakage.
- **Severity:** HIGH
- **Required Remediation:**
  - Collect and handle all errors in `destroy_fallible()`.
  - Log and quarantine any frame that fails release or recycling.
  - In `Drop`, make failure observable through global diagnostic counters or explicit quarantine records.

---

### Finding 6: Manufactured Process Termination in Service Supervisor
- **Classification:** Confirmed Defect
- **Files & Lines:**
  - `kernel-aarch64/src/services.rs`: lines 149–153 (`start`), 240–244 (`stop`), 325–329 (`recover_failed`)
  - `kernel-riscv64/src/services.rs`: lines 149–153 (`start`), 240–244 (`stop`), 325–329 (`recover_failed`)
- **Trace & Analysis:**
  - When stopping or recovering a service, the supervisor acquires the scheduler lock and directly overwrites:
    `task.state = TaskState::Terminated`.
  - It does not deliver a genuine termination signal or request through the process lifecycle.
  - If the task was currently executing on another CPU, mutating `task.state` does not stop execution, leading to a race where the task continues executing while its memory is reaped.
- **Severity:** CRITICAL
- **Required Remediation:**
  - Implement a cooperative/preemptive termination request mechanism (e.g. `request_termination` or task detach).
  - Reaping must verify that the process is detached from every CPU/hart before freeing memory.
  - Never manufacture termination by overwriting `task.state`.

---

### Finding 7: Bypassed Generation Validation in Service Heartbeats
- **Classification:** Confirmed Defect
- **Files & Lines:**
  - `kernel-aarch64/src/services.rs`: lines 276–282, 293–295
  - `kernel-riscv64/src/services.rs`: lines 276–282, 293–295
- **Trace & Analysis:**
  - `services::heartbeat(caller_pid)` calls `heartbeat_with_generation(caller_pid, None)`.
  - In `heartbeat_with_generation`:
    `if let Some(expected_gen) = generation { ... }`
  - When `generation` is `None`, generation validation is completely skipped.
  - Any task knowing the PID can update the heartbeat of a stale or dead service incarnation.
- **Severity:** MEDIUM
- **Required Remediation:**
  - Make generation validation mandatory: `pub fn heartbeat(caller_pid: u32, generation: u32) -> Result<(), ()>`.
  - Deprecate/remove the unvalidated `None` generation path.

---

### Finding 8: Superficial Calculator Visual Verification & Missing Digit 1 Check
- **Classification:** Confirmed Defect
- **Files & Lines:**
  - `tests/verify_aarch64_interactive.py`: lines 212–230
  - `tests/verify_riscv_interactive.py`: lines 212–230
- **Trace & Analysis:**
  - The function `verify_calculator_result_glyphs(path_ppm, expected_result="12")` takes `expected_result="12"` as an argument, but completely ignores it in its evaluation logic!
  - It only inspects coordinates `x: 640..665`, which corresponds exclusively to digit `'2'`.
  - Digit `'1'` (at `x: ~615..635`) is never inspected or validated.
  - Relative spacing between `'1'` and `'2'` is not checked.
  - Passing `expected_result="02"` or `expected_result="92"` would erroneously pass because only the `'2'` at the right edge is sampled.
- **Severity:** MEDIUM
- **Required Remediation:**
  - Verify both digit `'1'` and digit `'2'` independently with distinct structural bounding boxes.
  - Verify relative spacing between digits.
  - Verify tape expression ink for `"3 * 4"`.
  - Ensure negative tests for incorrect results (e.g. `"13"`, `"17"`, `"0"`, `"2"`) fail as intended.

---

## 3. Workstream Execution Roadmap for Phase 8.5.1

| Workstream | Subsystem | Planned Implementation | Target Files |
|---|---|---|---|
| **A** | Address-Space Lifecycle | Atomic state protocol (`Active(mask)`, `Closing`, `Destroyed`), race-safe `activate()` returning `Result` | `kernel-aarch64/src/mmu.rs`, `kernel-riscv64/src/mmu.rs` |
| **B** | SMP TLB Shootdown | RISC-V SBI RFNC hart mask handling & probe; AArch64 rendezvous; update acceptance ledger for x86_64 | `kernel-riscv64/src/sbi.rs`, `kernel-riscv64/src/mmu.rs`, `docs/PHASE_8_5_ACCEPTANCE_MATRIX.md` |
| **C** | Frame Ownership | Eliminate broad numeric fallbacks; validated MMIO registry; refcount bounds | `kernel-aarch64/src/mmu.rs`, `kernel-riscv64/src/mmu.rs` |
| **D** | Fallible Teardown | Non-discarding teardown; quarantine diagnostic tracking; resilient `Drop` | `kernel-aarch64/src/mmu.rs`, `kernel-riscv64/src/mmu.rs` |
| **E** | Service Supervision | Genuine task termination request; mandatory generation check; backoff | `kernel-aarch64/src/services.rs`, `kernel-riscv64/src/services.rs`, `syscall.rs` |
| **F** | Process Lifecycle | Truthful naming of EL0/U-mode tests; rigorous negative assertions | `kernel-aarch64/src/main.rs`, `kernel-riscv64/src/main.rs` |
| **G** | Calculator Verification | Multi-digit structural verification (`1` and `2`), expression tape check, negative test suite | `tests/verify_aarch64_interactive.py`, `tests/verify_riscv_interactive.py` |
| **H** | Acceptance Ledger | Cross-architecture status audit; cite path, symbol, test, evidence | `docs/PHASE_8_5_ACCEPTANCE_MATRIX.md` |
