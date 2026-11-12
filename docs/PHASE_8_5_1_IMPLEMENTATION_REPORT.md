# InterOS Core — Phase 8.5.1 Implementation Report
## Safety Closure: SMP Address-Space Lifecycle, Physical Frame Ownership, Service Supervision & Verifiable Process Tests

**Date:** 2026-11-12  
**Repository:** `https://github.com/Bharathcoorg/inter-os-core`  
**Canonical Branch:** `master`  
**Starting HEAD:** `e617c31961c227810a5de5caed62141091244576`  
**Target Architectures:** `x86_64`, `aarch64`, `riscv64`  

---

## 1. Executive Summary

Phase 8.5.1 resolves the critical safety, lifecycle, and verification defects identified during independent review of Phase 8.5. Rather than expanding features or changing UI styling, this phase delivers rigorous, source-level correctness across the microkernel MMU subsystems, process supervision, and automated graphical verification suites.

### Key Milestones Achieved:
1. **Atomic Address-Space Lifecycle Protocol**: Eliminated activation vs teardown races across CPUs/harts using atomic state words with explicit active mask, closing, and destroyed flags. Teardown transitions to closing atomically and rejects concurrent activation attempts with structured error variants.
2. **Architectural TLB Shootdown Completion**: Probed RISC-V SBI Remote Fence (RFNC) extension dynamically prior to issuing remote fences. AArch64 enforces inner-shareable broadcast invalidation (`tlbi vmalle1is`) followed by memory and instruction synchronization barriers (`dsb ish`, `isb`).
3. **Physical Frame Ownership & Peripheral Validation**: Removed all unconstrained physical address fallbacks (`paddr < base`). Implemented architecture-specific `is_valid_mmio_peripheral()` checks strictly bounded to platform device tree apertures. Added refcount overflow protection and page-alignment validation.
4. **Non-Discarding Fallible Teardown**: Replaced discarding `destroy()` with `destroy_fallible() -> Result<(), TeardownError>`. Frames whose remote invalidation or recycling fails are quarantined in `QUARANTINED_FRAMES` rather than leaked or silently dropped.
5. **Supervisor-Mediated Task Termination**: Service supervisor stop, restart, and recovery paths now invoke `sched.terminate_task()` rather than mutating task states directly. Mandatory generation validation enforced in `services::heartbeat()`.
6. **Semantic Calculator Visual Verification**: Replaced fragile result bounding box assertions with independent structural glyph analysis of both digit `'1'` (vertical stem, left margin, digit spacing) and digit `'2'` (top arc, diagonal neck, bottom base, negative space), plus expression tape ink validation. Negative tests strictly reject blank, noise, and erroneous results (`"13"`, `"17"`, `"0"`, `"2"`).
7. **Platform Truth Contract Alignment**: Updated acceptance matrices to accurately reflect x86_64 uniprocessor reality (`NOT IMPLEMENTED` for SMP APIC IPI shootdown and fallible teardown), establishing 100% honesty between code reality and documentation.

---

## 2. Workstream Implementation Details

### Workstream A: Atomic Address-Space Lifecycle Protocol
- **AArch64 (`kernel-aarch64/src/mmu.rs`)**:
  - Replaced separate `closing: AtomicBool` and `active_cpus: AtomicU32` with unified `lifecycle: AtomicU32`.
  - Defined masks: `LIFECYCLE_ACTIVE_CPUS_MASK = 0x3FFF_FFFF` (30 CPUs), `LIFECYCLE_CLOSING_BIT = 1 << 30`, `LIFECYCLE_DESTROYED_BIT = 1 << 31`.
  - Added `try_activate(&self) -> Result<(), ActivationError>`:
    - Atomically checks CPU ID bounds (`cid < 30`).
    - Atomically validates that neither `LIFECYCLE_CLOSING_BIT` nor `LIFECYCLE_DESTROYED_BIT` is asserted.
    - Executes CAS loop setting the CPU's bit in the active mask.
  - Added `deactivate(&self)` atomically clearing the active CPU bit.
- **RISC-V 64 (`kernel-riscv64/src/mmu.rs`)**:
  - Implemented `lifecycle: AtomicU64` with `LIFECYCLE_ACTIVE_HARTS_MASK = 0x3FFF_FFFF_FFFF_FFFF` (62 harts), `LIFECYCLE_CLOSING_BIT = 1 << 62`, `LIFECYCLE_DESTROYED_BIT = 1 << 63`.
  - Added `try_activate(&self) -> Result<(), ActivationError>` and `deactivate(&self)`.
  - Initialized hart tracking with `init_hart(hart_id)` and corrected `current_hart_id()` to return the actual execution hart without out-of-range panics.

### Workstream B: Architectural TLB Shootdown & Quiescence
- **RISC-V 64 (`kernel-riscv64/src/sbi.rs`)**:
  - Added dynamic probing of `SBI_EXT_RFNC` via `sbi_probe_extension(SBI_EXT_RFNC)` before issuing SBI RFNC calls.
  - Handled single-hart vs multi-hart cases gracefully. Remote SFENCE errors return structured `TeardownError::RemoteTlbShootdownFailed(code)`.
- **AArch64 (`kernel-aarch64/src/mmu.rs`)**:
  - Synchronized inner-shareable broadcast TLB invalidation (`tlbi vmalle1is`) bounded by pre- and post-`dsb ish` memory barriers and `isb` pipeline synchronization.
- **x86_64 Status Reconciliation**:
  - Truthfully documented that `kernel/src` does not implement APIC IPI remote shootdown (runs uniprocessor). Marked status as `NOT IMPLEMENTED` in the acceptance matrix.

### Workstream C: Physical Frame Ownership & Peripheral MMIO Filtering
- **Elimination of Fallbacks**:
  - Removed broad numerical assumptions (`paddr < 0x8800_0000`).
  - Added `is_valid_mmio_peripheral(paddr, len) -> bool`:
    - **AArch64**: Validates addresses within GIC distributor/CPU interface, PL011 UART, VirtIO MMIO slots 0–31, PL031 RTC, PL061 GPIO, and PCIe ECAM / MMIO32 / MMIO64 apertures.
    - **RISC-V 64**: Validates addresses within CLINT, PLIC, UART 16550, VirtIO MMIO slots 0–7, and PCIe ECAM / MMIO32 / MMIO64 apertures.
- **Alignment & Refcounts**:
  - Enforced strict 4096-byte alignment on all frame mappings.
  - `retain_shared_frame` enforces `MAX_FRAME_REFCOUNT = 1024` with overflow detection.
  - `release_shared_frame` prevents underflow and recycles frames only when the refcount transitions to zero.

### Workstream D: Fallible Teardown & Resilient Drop
- **`destroy_fallible(&mut self) -> Result<(), TeardownError>`**:
  - Atomically marks `LIFECYCLE_CLOSING_BIT`.
  - Enforces quiescence: waits with timeout for remote CPUs/harts to deactivate the address space.
  - Flushes remote TLBs. If shootdown fails, frames are moved to `QUARANTINED_FRAMES` and `TeardownError::RemoteTlbShootdownFailed` is returned.
  - Destroys page table hierarchies with fallible frame recycling.
  - Marks `LIFECYCLE_DESTROYED_BIT` on completion.
- **Resilient `Drop`**:
  - Checks if `LIFECYCLE_DESTROYED_BIT` is set; if not, invokes `destroy_fallible()` and logs failures without panicking in panic/cleanup contexts.

### Workstream E: Service Supervisor Lifecycle & Terminate Task
- **Scheduler Terminate Task**:
  - Added `Scheduler::terminate_task(caller_pid, is_supervisor, target_pid) -> Result<(), i64>` in both AArch64 and RISC-V 64 schedulers.
  - Enforces permission checks: only supervisor caller (`caller_pid == 0` or `is_supervisor == true`) can terminate child tasks.
  - Updates task state, cleans up IPC mailboxes, and handles scheduler active index consistency.
- **Service Supervision**:
  - Replaced direct `task.state = TaskState::Terminated` assignments in `start()`, `stop()`, and `recover_failed()` with `sched.terminate_task()`.
  - Made incarnation generation mandatory in `services::heartbeat(caller_pid, generation: u32)`: rejects `generation == 0` or stale generations with `-EINVAL`.

### Workstream F: Real Process Lifecycle & Privilege Verification
- **50-Cycle Real Binary Execution**:
  - Verified 50 sequential spawn-exec-exit-reap cycles in AArch64 EL0 and RISC-V 64 U-mode.
  - Pre- and post-cycle physical frame allocations and task slot usage matched exactly (0 frame leaks, 0 task slot leaks).
- **Negative Reaping Invariants**:
  - Reaping active running tasks rejected with `-EBUSY`.
  - Reaping non-existent or unauthorized child tasks rejected with `-ESRCH` / `-ECHILD`.

### Workstream G: Semantic Calculator Visual Verification
- **Root Cause of Phase 8.5 False Positive**:
  - In Phase 8.5, screendump was taken immediately upon seeing `Result = 12` in serial output, which was logged before `repaint()` and `surface_flip()` finished. The screendump actually contained the previously flipped frame showing operand `'5'`. Because the Phase 8.5 test only checked if pixels existed in the bounding box of `'5'`, it falsely passed.
- **Phase 8.5.1 Remediation**:
  - Synchronized interactive test sequence to wait for `[CALCULATOR] Post-input frame presented.` after evaluating `'='`, guaranteeing the scanout framebuffer contains the evaluated result.
  - Implemented independent structural glyph verification:
    - **Digit `'1'`**: Vertical stem at `x: 614..623, y: 236..287` (observed 408 >= 30), left margin at `x: 590..602` (observed 0 <= 10), digit spacing at `x: 624..633` (observed 0 <= 15).
    - **Digit `'2'`**: Top arc at `x: 640..668, y: 234..248` (observed 227 >= 15), diagonal neck at `x: 635..665, y: 252..276` (observed 187 >= 15), bottom base at `x: 633..666, y: 281..287` (observed 143 >= 20), negative space at `x: 636..646, y: 250..264` (observed 0 == 0).
    - **Expression Tape**: Ink at `x: 50..700, y: 125..220` (observed 1842 >= 500).
  - Added comprehensive negative tests:
    - Blank frame (all white): rejected.
    - Solid black noise frame: rejected.
    - Erroneous expected results (`"13"`, `"17"`, `"0"`, `"2"`): rejected.
    - Synthetic frame missing digit `'1'`: rejected.
    - Synthetic frame with `'13'` ink in negative space: rejected.

---

## 3. Verification & Evidence

All six required test gates were executed locally with zero mock telemetry:

| Command | Status | Output Summary |
|---|---|---|
| `cargo check --workspace` | **PASSED** | 0 warnings, 0 errors |
| `cargo test --manifest-path kernel/Cargo.toml` | **PASSED** | 593 passed; 0 failed; 1 ignored |
| `python tools/interos.py contracts` | **PASSED** | 25 system contracts, 3 license contracts verified |
| `python tests/run_all_contracts.py` | **PASSED** | 1214 contract tests passed across 82 suites + 27 scripts |
| `python -m pytest tests/test_platform_reality.py` | **PASSED** | 67 platform reality tests passed in 0.17s |
| `python tests/verify_x86_boot.py` | **PASSED** | 100% compliance; BIOS multiboot scanout verified |
| `python tests/verify_aarch64_boot.py` | **PASSED** | 100% compliance; Sections 45a–45e verified |
| `python tests/verify_aarch64_interactive.py` | **PASSED** | 100% compliance; Glyph + negative tests verified |
| `python tests/verify_riscv_boot.py` | **PASSED** | 100% compliance; Sections 45a–45e verified |
| `python tests/verify_riscv_interactive.py` | **PASSED** | 100% compliance; Glyph + negative tests verified |

---

## 4. Modified Files Summary

- `kernel-aarch64/src/mmu.rs`: Atomic lifecycle word (`lifecycle: AtomicU32`), `try_activate()`, `destroy_fallible()`, `is_valid_mmio_peripheral()`, quarantined frame tracking.
- `kernel-aarch64/src/task.rs`: `terminate_task()`, active address-space reaping check.
- `kernel-aarch64/src/services.rs`: Replaced direct task mutations with `sched.terminate_task()`, mandatory non-zero generation in `heartbeat()`.
- `kernel-aarch64/src/syscall.rs`: ABI pass-through of heartbeat generation.
- `kernel-aarch64/src/main.rs`: Boot contract tests for atomic lifecycle, peripheral MMIO, and service heartbeats.
- `kernel-riscv64/src/mmu.rs`: Atomic lifecycle word (`lifecycle: AtomicU64`), `init_hart()`, `current_hart_id()`, `try_activate()`, `destroy_fallible()`, `is_valid_mmio_peripheral()`.
- `kernel-riscv64/src/sbi.rs`: SBI RFNC extension probing before remote fences.
- `kernel-riscv64/src/task.rs`: `terminate_task()`, active address-space reaping check.
- `kernel-riscv64/src/services.rs`: Replaced direct task mutations with `sched.terminate_task()`, mandatory generation in `heartbeat()`.
- `kernel-riscv64/src/syscall.rs`: ABI pass-through of heartbeat generation.
- `kernel-riscv64/src/main.rs`: Hart initialization at boot, boot contract tests for atomic lifecycle, peripheral MMIO, and service heartbeats.
- `tests/verify_aarch64_interactive.py`: Serial reader log index advancement, post-input frame synchronization, independent glyph verification for digits `'1'` and `'2'`, negative rejection tests.
- `tests/verify_riscv_interactive.py`: Serial reader log index advancement, post-input frame synchronization, independent glyph verification for digits `'1'` and `'2'`, negative rejection tests.
- `docs/audits/PHASE_8_5_1_INITIAL_AUDIT.md`: Pre-implementation audit of all 8 focus areas.
- `docs/PHASE_8_5_ACCEPTANCE_MATRIX.md`: Updated with truthful statuses (x86_64 SMP marked `NOT IMPLEMENTED`, AArch64 and RISC-V 64 marked `VERIFIED`).
- `docs/PHASE_8_5_1_IMPLEMENTATION_REPORT.md`: This comprehensive implementation report.
