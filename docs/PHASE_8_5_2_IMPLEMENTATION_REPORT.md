# InterOS Core — Phase 8.5.2 Implementation Report

**Date:** 2026-11-12  
**Baseline Starting Commit:** `8d27bb36ba3180b1f0dd3c13de2b0229542fe888`  
**Parent Commit:** `e617c31961c227810a5de5caed62141091244576`  
**Target Architectures:** `x86_64`, `aarch64`, `riscv64`  

---

## 1. Executive Summary

Phase 8.5.2 closes the remaining correctness and memory-safety invariants discovered during independent review of Phase 8.5.1 across the AArch64 and RISC-V 64 microkernels and cross-platform verification test suites:

1. **Hardware Translation Deactivation Before Mask Clearance**: Prevented translation races by requiring `switch_to_kernel()` / `switch_to_kernel_satp()` and full pipeline barriers prior to clearing active CPU/hart bitmasks.
2. **Scheduler Address-Space Lifecycle Integration**: `prepare_switch()` now explicitly deactivates outgoing address spaces and performs fallible `try_activate()` on incoming address spaces, safely aborting to kernel tables on failure.
3. **Remote Core Quiescence & Evacuation Verification**: `acknowledged_cpus` and `acknowledged_harts` ensure all remote cores that ever executed in an address space have evacuated and switched translation registers before physical frames can be reclaimed.
4. **Physical Frame Ownership Invariants**:
   - Added single-owner mapping tracking (`mark_private_frame_mapped` / `mark_private_frame_unmapped`) rejecting private frame double-mapping.
   - `recycle_frame()` strictly rejects `Mmio`, `Borrowed`, `Shared` (requiring `release_shared_frame`), and actively mapped private frames.
   - Narrowed explicit MMIO apertures with length overflow checks.
5. **Transactional Process Reaping & Safe Drop**:
   - `reap_task` executes `aspace.destroy_fallible()` before removing task metadata; on error, preserves the task, returns `-EBUSY`, and quarantines frames.
   - Safe `Drop` for `UserAddressSpace` logs warnings and quarantines frames rather than panicking or corrupting memory.
6. **Service Supervisor Concurrency**: Serialized all lifecycle operations under `SERVICE_LIFECYCLE_LOCK` and checked `sched.terminate_task(...)` results on all paths.
7. **Calculator Visual Negative & Positive Synthetic Suite**: Replaced parameter-level assertions with genuine synthetic PPM images representing erroneous glyphs (`"13"`, `"17"`, `"0"`, `"2"`, bridged spacing, noise, blank) and verified that geometric glyph analysis strictly rejects them, while validating synthetic `"12"` positive control.

---

## 2. Key Code Changes by Component

### AArch64 Microkernel (`kernel-aarch64`)
- `src/mmu.rs`:
  - Added `mark_private_frame_mapped` and `mark_private_frame_unmapped` to `ArmFrameAllocator`.
  - Enforced that `recycle_frame()` rejects `Mmio`, `Borrowed`, `Shared`, and actively mapped private frames (`ref_counts != 0`), while permitting unmapped `PrivateOwned` and dynamically allocated page table (`Kernel`) frames.
  - Added `acknowledged_cpus: AtomicU32` to track core evacuation.
  - Changed `activate(&self) -> Result<(), ActivationError>`.
  - Updated `deactivate(&self)` to switch hardware translation (`ArmMmu::switch_to_kernel()`) before clearing active bitmask.
  - Updated `destroy_fallible()` to bounded-wait for remote core deactivation and acknowledgment, unmap private frames, release shared frames, and return typed `TeardownError`.
  - Implemented non-panicking `Drop` for `UserAddressSpace` with frame quarantining.
- `src/task.rs`:
  - `reap_task()` executes `aspace.destroy_fallible()` transactionally before task removal.
  - `prepare_switch()` deactivates outgoing task address space and fallibly activates incoming task address space.
- `src/services.rs`:
  - Added `SERVICE_LIFECYCLE_LOCK` mutex to serialize service operations.
  - Handled `sched.terminate_task(...)` errors across `start_internal`, `stop`, and `recover_failed`.
- `src/main.rs`:
  - Added unit test asserting private frame double-mapping rejection.
  - Verified `aspace.destroy().is_ok()` in 50-cycle memory stability test.

### RISC-V 64 Microkernel (`kernel-riscv64`)
- `src/mmu.rs`:
  - Added `mark_private_frame_mapped` and `mark_private_frame_unmapped` to `RiscvFrameAllocator`.
  - Enforced that `recycle_frame()` rejects `Mmio`, `Borrowed`, `Shared`, and actively mapped private frames.
  - Added `switch_to_kernel_satp()`.
  - Narrowed MMIO apertures to PLIC (`0x0c00_0000..0x0c20_0000`) and PCIe Display BAR (`0x6000_0000..0x6100_0000`) with length overflow validation.
  - Added `acknowledged_harts: AtomicU64` to track hart evacuation.
  - Changed `activate(&self) -> Result<(), ActivationError>`.
  - Updated `deactivate(&self)` to switch SATP to kernel root table before clearing active bitmask.
  - Updated `destroy_fallible()` to bounded-wait for remote hart acknowledgment, unmap private frames, release shared frames, and quarantine frames on failure.
  - Implemented non-panicking `Drop` for `UserAddressSpace`.
- `src/task.rs`:
  - Transactional `reap_task()` destroying address space before removing task.
  - `prepare_switch()` deactivates outgoing and activates incoming address space.
- `src/services.rs`:
  - Added `SERVICE_LIFECYCLE_LOCK` mutex and checked `terminate_task` return codes.
- `src/trap.rs`:
  - Handled fallible `try_activate()` on PID 1 init entry.
- `src/main.rs`:
  - Added private frame double-mapping rejection assertion.
  - Verified `aspace.destroy().is_ok()` in 50-cycle memory stability test.

### Interactive Test Suites (`tests/verify_*_interactive.py`)
- Upgraded `run_glyph_negative_tests()` to generate genuine PPM images with synthetic geometry for:
  - Blank frame (all white).
  - Solid black noise frame.
  - Synthetic `"13"` (erroneous ink in negative space of digit '2').
  - Synthetic `"17"` (missing base of digit '2').
  - Synthetic `"0"` (missing stem of digit '1').
  - Synthetic lone `"2"` (missing digit '1').
  - Synthetic bridged `"12"` (excess ink between digits).
  - Synthetic valid `"12"` positive control (passes analysis).

---

## 3. Test Verification Results

All required verification suites were executed locally and passed with 100% compliance:

| Target | Test Command | Result | Details |
|---|---|---|---|
| **Workspace** | `cargo check --workspace` | **PASSED** | 0 errors, 0 warnings |
| **Workspace** | `cargo test --manifest-path kernel/Cargo.toml` | **PASSED** | 593 passed, 0 failed, 1 ignored |
| **Workspace** | `python tools/interos.py contracts` | **PASSED** | 25/25 system contracts, 3/3 licence contracts |
| **Workspace** | `python tests/run_all_contracts.py` | **PASSED** | 1214 framework tests + 27 script contracts |
| **Workspace** | `python -m pytest tests/test_platform_reality.py` | **PASSED** | 67 passed in 0.16s |
| **x86_64** | `python tools/interos.py build demo` | **PASSED** | Built `target/inter-os-bios.img` (13,075,456 bytes) |
| **x86_64** | `python tests/verify_x86_boot.py` | **PASSED** | 100% compliance, 3679 distinct colours |
| **AArch64** | `python tools/interos.py build --target aarch64` | **PASSED** | Built flat image `target/kernel-aarch64.img` |
| **AArch64** | `python tests/verify_aarch64_boot.py` | **PASSED** | 100% compliance, all required signals observed |
| **AArch64** | `python tests/verify_aarch64_interactive.py` | **PASSED** | 100% compliance, synthetic glyph negative tests & full UI loop |
| **RISC-V 64** | `python tools/interos.py build --target riscv64` | **PASSED** | Built `target/riscv64gc-unknown-none-elf/debug/kernel-riscv64` |
| **RISC-V 64** | `python tests/verify_riscv_boot.py` | **PASSED** | 100% compliance, all required signals observed |
| **RISC-V 64** | `python tests/verify_riscv_interactive.py` | **PASSED** | 100% compliance, synthetic glyph negative tests & full UI loop |
