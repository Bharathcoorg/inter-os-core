# InterOS Core — Phase 8.5 / 8.5.3.1 Cross-Architecture Acceptance Matrix

**Date:** 2026-11-12  
**Phase:** 8.5.3.1 Remote Evacuation Race & Frame Reclamation Error Closure  
**Starting HEAD:** `96db7c7f6d6a48505960ba2e60c61d086c372d25`  
**Previous Phase Commit:** `b56946545e89aaac674c1eab9615f5a2cc89281c`  
**Target Architectures:** `x86_64`, `aarch64`, `riscv64`  

---

## 1. Feature & Safety Invariants Matrix

| Category | Invariant / Requirement | x86_64 Status | AArch64 Status | RISC-V 64 Status | Verification Method |
|---|---|---|---|---|---|
| **Memory Isolation** | User/Kernel Separation (Ring 3 / EL0 / U-Mode) | **VERIFIED** | **VERIFIED** | **VERIFIED** | Single-vCPU QEMU Boot & Hardware Exception Handlers |
| **TLB Management** | Local & Broadcast TLB Invalidation (`unmap_user_page` / `destroy_fallible`) | **NOT IMPLEMENTED** (Uniprocessor kernel; no APIC IPI shootdown in `kernel/src`) | **SINGLE-CPU VERIFIED / SMP SOURCE-ONLY** (`dsb ishst` + `tlbi vae1is`/`vmalle1is` + barriers) | **SINGLE-HART VERIFIED / SMP SOURCE-ONLY** (`sfence.vma` + `sbi_remote_sfence_vma` on remote hart mask) | Kernel Unit & Single-Core QEMU Boot Tests |
| **SMP Evacuation Model** | Remote CPU/Hart Deactivation & 5-Stage Reaping | **NOT IMPLEMENTED** | **PASSIVE BOUNDED (SINGLE-CPU TESTED)** (Locked 5-stage reap keeping AS attached during evacuation; no GIC SGI IPI preemption kick) | **PASSIVE BOUNDED (SINGLE-HART TESTED)** (Locked 5-stage reap keeping AS attached during evacuation; no SBI IPI preemption kick) | `test_reap_evacuation_protocol` & `reap_task_locksafe` E2E (Single-vCPU QEMU) |
| **Lock-Safe Reaping** | 5-Stage Lock-Safe Reap (`reap_task_locksafe`, attached AS across Stage 2) | **NOT APPLICABLE** (Single-core kernel) | **VERIFIED (SINGLE-CPU QEMU)** (`begin_reap` keeps AS + sets closing bit -> lock-free Stage 2 poll -> `detach_reaping_aspace` -> `destroy_fallible` -> `finish_reap`/`abort_reap`) | **VERIFIED (SINGLE-HART QEMU)** (`begin_reap` keeps AS + sets closing bit -> lock-free Stage 2 poll -> `detach_reaping_aspace` -> `destroy_fallible` -> `finish_reap`/`abort_reap`) | Kernel Boot (`Section 45b` 50 cycles, `Section 45e`, `Test 8` & `reap_task_locksafe` E2E) |
| **Transactional Switch** | Non-Mutating Switch Failure Rollback (`prepare_switch`) | **VERIFIED** | **VERIFIED** (`try_activate()` checked before mutating state/ticks; `deactivate_after_switch()` clears tracking without reverting TTBR0) | **VERIFIED** (`try_activate()` checked before mutating state/ticks; `deactivate_after_switch()` clears tracking without reverting `satp`) | `test_transactional_switch` & Single-vCPU QEMU Interactive Switch Tests |
| **Ownership Validation** | No Ignored `mark_private_frame_unmapped()` Errors (`unmap_user_page` / `destroy_fallible`) | **VERIFIED** | **VERIFIED** (`mark_private_frame_unmapped` checked; failure skips recycle, preserves mapping, quarantines with original reason) | **VERIFIED** (`mark_private_frame_unmapped` checked; failure skips recycle, preserves mapping, quarantines with original reason) | `test_mmu_ownership_reclamation_errors()` in QEMU Boot |
| **Mapping Metadata Safety** | Retain Mapping & `mapped_bytes` Until Teardown Succeeds | **VERIFIED** | **VERIFIED** (`unmap_user_page` & `destroy_fallible` retain failed mappings and bytes) | **VERIFIED** (`unmap_user_page` & `destroy_fallible` retain failed mappings and bytes) | `test_mmu_ownership_reclamation_errors()` in QEMU Boot |
| **Diagnostic Quarantine** | Rich Quarantine Metadata & Recycle Rejection (`is_quarantined`) | **NOT IMPLEMENTED** | **VERIFIED** (`QuarantinedFrame { paddr, ownership, reason }`; `recycle_frame`/`release_shared_frame` reject quarantined frames; accurate `Kernel` ownership for table frames) | **VERIFIED** (`QuarantinedFrame { paddr, ownership, reason }`; `recycle_frame`/`release_shared_frame` reject quarantined frames; accurate `Kernel` ownership for table frames) | `test_mmu_ownership_reclamation_errors()` in QEMU Boot |
| **TLB Failure Handling** | Fallible Teardown & Quarantining | **NOT IMPLEMENTED** (Single-core teardown in `kernel/src`) | **VERIFIED (SINGLE-CPU QEMU)** (`destroy_fallible()`, `Drop` preserves quarantine reasons and skips `Borrowed`) | **VERIFIED (SINGLE-HART QEMU)** (`destroy_fallible()`, `Drop` preserves quarantine reasons and skips `Borrowed`) | `test_mmu_ownership_reclamation_errors()` in QEMU Boot |
| **Atomic AS Lifecycle** | Race-free Activation & Teardown Protocol | **NOT IMPLEMENTED** | **SINGLE-CPU VERIFIED / SMP SOURCE-ONLY** (`AtomicU32` with Active Mask, Closing & Destroyed Bits) | **SINGLE-HART VERIFIED / SMP SOURCE-ONLY** (`AtomicU64` with Active Mask, Closing & Destroyed Bits) | Sequential CAS & Section 45d/45e Single-Core QEMU Tests |
| **Context Switch Safety** | Register Switch Before Deactivation & Fallible Activation | **NOT IMPLEMENTED** | **VERIFIED** (`deactivate()`/`deactivate_after_switch()` preserves incoming `TTBR0_EL1`) | **VERIFIED** (`deactivate()`/`deactivate_after_switch()` preserves incoming `satp`) | `test_transactional_switch`, `test_reap_evacuation_protocol`, Interactive QEMU UI |
| **Physical Frame Ownership** | Explicit Ownership Classes (`PrivateOwned`, `Shared`, `Borrowed`, `Kernel`, `Mmio`) | **VERIFIED** | **VERIFIED** (Bitmap & refcount checked on `mark_private_frame_unmapped`) | **VERIFIED** (Bitmap & refcount checked on `mark_private_frame_unmapped`) | Section 45d Physical Frame Reclamation & Remapping Tests |
| **Private Frame Mapping** | Single-Owner Enforcement (No Private Double-Mapping) | **VERIFIED** | **VERIFIED** (Double-mapping rejected with `MapError::FrameAlreadyMapped`) | **VERIFIED** (Double-mapping rejected with `MapError::FrameAlreadyMapped`) | In-kernel Physical Frame Ownership Unit Test |
| **Shared Frame Refcounting** | Retain/Release Semantics (`retain_shared_frame`, `release_shared_frame`) | **VERIFIED** | **VERIFIED** (Overflow & Underflow Guarded) | **VERIFIED** (Overflow & Underflow Guarded) | Multi-address space sharing tests |
| **Borrowed/MMIO Immunity** | Device & Borrowed Frames Never Recycled | **VERIFIED** | **VERIFIED** (`is_valid_mmio_peripheral` aperture & overflow checked) | **VERIFIED** (`is_valid_mmio_peripheral` aperture & overflow checked) | Teardown non-reclamation tests |
| **Process Lifecycle** | Genuine User-Mode Execution (50 Cycles) | **VERIFIED** | **VERIFIED** (EL0 ELF SVC dispatch) | **VERIFIED** (U-mode ELF ECALL dispatch) | Section 45e Real Binary Lifecycle Tests |
| **Process Reaping** | Parent `waitpid` Termination & Zero Leaks | **VERIFIED** | **VERIFIED** (Zero frame/task leaks; transactional `destroy_fallible` before reap) | **VERIFIED** (Zero frame/task leaks; transactional `destroy_fallible` before reap) | 50-cycle pre/post counter comparison |
| **Lifecycle Safety** | Quota Exhaustion (`-ENOMEM`) & Active Reap Denial (`-EBUSY`) | **VERIFIED** | **VERIFIED** | **VERIFIED** | Section 45e Safety Invariant Tests |
| **Service Supervisor** | Serialized Lifecycle, Incarnation Generation & Reaping | **VERIFIED** | **VERIFIED** (`SERVICE_LIFECYCLE_LOCK`, `sched.terminate_task()`, `AtomicU32` Incarnation) | **VERIFIED** (`SERVICE_LIFECYCLE_LOCK`, `sched.terminate_task()`, `AtomicU32` Incarnation) | Service restart & supervisor reaping tests |
| **Service Heartbeats** | Heartbeat Generation & PID Validation | **VERIFIED** | **VERIFIED** (Mandatory non-zero generation validation) | **VERIFIED** (Mandatory non-zero generation validation) | Service Supervisor Lifecycle Tests (Section 46) |
| **Display Subsystem** | 720x1440 Portrait Framebuffer Presentation | **VERIFIED** | **VERIFIED** | **VERIFIED** | QEMU Framebuffer Sample Analysis |
| **Visual App Parity** | Calculator Visual Evaluation (`7 + 5 = 12`) | **VERIFIED** | **VERIFIED** (Independent '1' stem and '2' arc/neck/base glyph geometry) | **VERIFIED** (Independent '1' stem and '2' arc/neck/base glyph geometry) | Interactive QEMU UI Test Scripts |
| **Visual Robustness** | Blank, Noise, Synthetic Erroneous ("13", "17", "0", "2", Bridged) & Positive "12" | **VERIFIED** | **VERIFIED** (`run_glyph_negative_tests` image evaluation) | **VERIFIED** (`run_glyph_negative_tests` image evaluation) | Interactive QEMU UI Test Scripts |
| **Platform Truth** | Zero Mock Hardware Telemetry / Zero Ambient Authority | **VERIFIED** | **VERIFIED** | **VERIFIED** | `tests/test_platform_reality.py` (67 passed) |

---

## 2. Test Execution Evidence & Pass Status

| Target Architecture | Test Script / Suite | Exit Code | Result | Key Indicators |
|---|---|---|---|---|
| **Workspace** | `cargo fmt --all -- --check` | 1 | **PRE-EXISTING DIFFS** | Pre-existing formatting diffs in untouched repository files |
| **Workspace** | `cargo check --workspace` | 0 | **PASSED** | 0 errors (excludes `kernel`, `kernel-aarch64`, `kernel-riscv64`) |
| **Workspace** | `python tools/interos.py contracts` | 0 | **PASSED** | 25/25 system contracts, 3/3 license contracts |
| **Workspace** | `python tests/run_all_contracts.py` | 1 | **1240 PASS / 1 PRE-EXISTING FAIL** | 1213 framework passed + 27 script contracts passed, 1 skipped, 1 pre-existing intermittent failure (`test_runner_integrity.py::test_kernel_net_cases_are_independent_of_execution_order` on `kernel/` x86_64 parallel sweep; 13 runtime/QEMU suites excluded) |
| **Workspace** | `python -m pytest tests/test_platform_reality.py` | 0 | **PASSED** | 67 passed in 0.17s |
| **x86_64** | `cargo test --manifest-path kernel/Cargo.toml` | 0 | **PASSED** | 593 passed, 0 failed, 1 ignored (x86_64 host tests) |
| **x86_64** | `python tools/interos.py build demo` & `verify_x86_boot.py` | 0 | **PASSED (8.5.3)** | Built `target/inter-os-bios.img`; 3680 distinct colours verified (x86_64 code unchanged in 8.5.3.1) |
| **AArch64** | `cargo check --manifest-path kernel-aarch64/Cargo.toml` | 0 | **PASSED** | 0 errors/warnings |
| **AArch64** | `python tools/interos.py build --target aarch64` | 0 | **PASSED** | `target/kernel-aarch64.img` (150,459,888 bytes) |
| **AArch64** | `python tests/verify_aarch64_boot.py` | 0 | **PASSED** | Enforces Transactional Switch, 5-Stage Reap Evacuation & MMU Error signals (single-vCPU QEMU) |
| **AArch64** | `python tests/verify_aarch64_interactive.py` | 0 | **PASSED** | Calculator PID 71, `7 + 5 = 12` glyph + negative tests, reap PID 71, HomeScreen re-presented |
| **RISC-V 64** | `cargo check --target riscv64gc-unknown-none-elf --manifest-path kernel-riscv64/Cargo.toml` | 0 | **PASSED** | 0 errors/warnings |
| **RISC-V 64** | `python tools/interos.py build --target riscv64` | 0 | **PASSED** | Kernel ELF built cleanly |
| **RISC-V 64** | `python tests/verify_riscv_boot.py` | 0 | **PASSED** | Enforces 5-Stage Reap + Transactional Switch & MMU Error signals (single-hart QEMU) |
| **RISC-V 64** | `python tests/verify_riscv_interactive.py` | 0 | **PASSED** | Calculator PID 177, `7 + 5 = 12` glyph + negative tests, reap PID 177, HomeScreen re-presented |

---

## 3. Architectural Limitations & Visual Regression Methodology

### 3.1 SMP Evacuation Model Honesty
- **Global Scheduler & Trap Architecture (`AArch64` & `RISC-V 64`):** Both kernels currently maintain a single global `SCHEDULER` mutex, a single global `current_task_idx`, and global trap save areas rather than per-CPU/per-hart runqueues. Multi-vCPU concurrent execution is therefore not supported today.
- **Single-vCPU QEMU Verification Scope:** All `verify_*_boot.py` and `verify_*_interactive.py` QEMU executions run on `-M virt` without `-smp` (1 CPU / 1 hart). Multi-core evacuation races (`reap_task_locksafe` Stage 2 polling, remote `prepare_switch()`, and RISC-V `sbi::remote_sfence_vma`) are validated via single-CPU simulation and source review, NOT concurrent multi-core QEMU runs.
- **Passive Bounded Protocol:** `reap_task_locksafe()` polls evacuation lock-free up to `1000` iterations (`TaskState::Reaping` keeping `address_space` attached) and `destroy_fallible()` spins up to `100_000` iterations waiting for remote CPUs/harts to transition via `deactivate()`. Neither architecture sends active GIC SGI or SBI IPI preemption kicks to interrupt a spinning remote thread.
- **x86_64:** The `kernel/` x86_64 kernel is compiled and booted as a single-processor (uniprocessor) system without APIC TLB shootdown IPIs.

### 3.2 Calculator Structural Pixel-Geometry Assertion
Whole-frame cryptographic hashes (SHA-256) or exact pixel-buffer comparisons are intentionally avoided for display verification because different host GPU drivers, QEMU VNC color depth conversions (RGB565 vs RGB888/BGR), and grayscale anti-aliasing coverage rounding alter raw pixel values without altering user-visible semantics. Instead, `verify_*_interactive.py` evaluates structural glyph geometry over the rendered result display bounding box (checking foreground pixel separation, independent horizontal/vertical stroke components for `'1'` vs `'2'`, aspect ratios, and negative rejection of `"13"`, `"17"`, `"0"`, `"2"`, noise, and bridged digits).
