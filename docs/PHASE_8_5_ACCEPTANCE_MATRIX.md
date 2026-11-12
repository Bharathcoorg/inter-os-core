# InterOS Core — Phase 8.5 / 8.5.3 Cross-Architecture Acceptance Matrix

**Date:** 2026-11-12  
**Phase:** 8.5.3 Scheduler and Reclamation Correctness  
**Starting Baseline Commit:** `619d4b54d817ce95756bc89e715a7cd30cf4c043`  
**Parent Commit:** `8d27bb36ba3180b1f0dd3c13de2b0229542fe888`  
**Target Architectures:** `x86_64`, `aarch64`, `riscv64`  

---

## 1. Feature & Safety Invariants Matrix

| Category | Invariant / Requirement | x86_64 Status | AArch64 Status | RISC-V 64 Status | Verification Method |
|---|---|---|---|---|---|
| **Memory Isolation** | User/Kernel Separation (Ring 3 / EL0 / U-Mode) | **VERIFIED** | **VERIFIED** | **VERIFIED** | QEMU Boot & Hardware Exception Handlers |
| **TLB Management** | Remote TLB Invalidation / Shootdown | **NOT IMPLEMENTED** (Uniprocessor kernel; no APIC IPI shootdown in `kernel/src`) | **VERIFIED** (`tlbi vmalle1is` Inner-Shareable Broadcast + Passive Quiescence) | **VERIFIED** (SBI RFNC Probing + `sbi_remote_sfence_vma` Error Handling + Passive Quiescence) | Kernel Unit & Boot Invariant Tests (Section 45d) |
| **SMP Evacuation Model** | Remote CPU/Hart Deactivation | **NOT IMPLEMENTED** | **PASSIVE BOUNDED** (Bounded 100k spin wait for `deactivate()` without active IPI preemption) | **PASSIVE BOUNDED** (Bounded 100k spin wait for `deactivate()` + SBI RFNC broadcast) | Architecture Audit & `destroy_fallible()` timeout handling |
| **Lock-Safe Reaping** | Two-Phase Lock-Free Teardown (`reap_task_locksafe`) | **NOT APPLICABLE** (Single-core scheduler lock does not block remote evacuation) | **VERIFIED** (`TaskState::Reaping`/`ReapFailed`, generation token, lock dropped during `destroy_fallible`) | **VERIFIED** (`TaskState::Reaping`/`ReapFailed`, generation token, lock dropped during `destroy_fallible`) | Task Lifecycle Tests 6 & Generation Verification |
| **Transactional Switch** | Non-Mutating Switch Failure Rollback (`prepare_switch`) | **VERIFIED** | **VERIFIED** (`try_activate()` checked before mutating state/ticks; aborts cleanly) | **VERIFIED** (`try_activate()` checked before mutating state/ticks; aborts cleanly) | Task Lifecycle Test 7 (Closing Address Space Rollback) |
| **Mapping Metadata Safety** | Retain Mapping Until Teardown Succeeds | **VERIFIED** | **VERIFIED** (`unmap_user_page` keeps mapping until PTE clear, TLBI, & recycle succeed) | **VERIFIED** (`unmap_user_page` keeps mapping until PTE clear, `sfence.vma`, & recycle succeed) | MMU Unmap & Frame Reclamation Verification |
| **Diagnostic Quarantine** | Rich Quarantined Frame Metadata & Rollback Handling | **NOT IMPLEMENTED** | **VERIFIED** (`QuarantinedFrame { paddr, ownership, reason }` + map rollback quarantine) | **VERIFIED** (`QuarantinedFrame { paddr, ownership, reason }` + map rollback quarantine) | Section 45d Quarantine Metadata Assertion |
| **TLB Failure Handling** | Fallible Teardown & Quarantining | **NOT IMPLEMENTED** (Single-core teardown in `kernel/src`) | **VERIFIED** (`destroy_fallible()`, `QUARANTINED_FRAMES`) | **VERIFIED** (`destroy_fallible()`, `QUARANTINED_FRAMES`) | Section 45d Teardown & Drop Invariant Tests |
| **Atomic AS Lifecycle** | Race-free Activation & Teardown Protocol | **NOT IMPLEMENTED** | **VERIFIED** (`AtomicU32` with Active Mask, Closing & Destroyed Bits) | **VERIFIED** (`AtomicU64` with Active Mask, Closing & Destroyed Bits) | Multi-core CAS & Section 45d Activation Race Tests |
| **Context Switch Safety** | Register Switch Before Deactivation & Fallible Activation | **NOT IMPLEMENTED** | **VERIFIED** (`switch_to_kernel()` before clearing active mask; `try_activate()` in `prepare_switch`) | **VERIFIED** (`switch_to_kernel_satp()` before clearing active mask; `try_activate()` in `prepare_switch`) | Scheduler context-switch and deactivation test paths |
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
| **Workspace** | `cargo check --workspace` | 0 | **PASSED** | 0 compilation errors or warnings |
| **Workspace** | `python tools/interos.py contracts` | 0 | **PASSED** | 25/25 system contracts, 3/3 license contracts |
| **Workspace** | `python tests/run_all_contracts.py` | 0 | **PASSED** | 1214 framework passed + 27 script contracts = 1241 passed (1 framework skipped) |
| **Workspace** | `python -m pytest tests/test_platform_reality.py` | 0 | **PASSED** | 67 passed in 0.47s |
| **x86_64** | `cargo test --manifest-path kernel/Cargo.toml` | 0 | **PASSED** | 593 passed, 0 failed, 1 ignored |
| **x86_64** | `python tools/interos.py build demo` | 0 | **PASSED** | Built `target/inter-os-bios.img` (13,075,456 bytes) |
| **x86_64** | `python tests/verify_x86_boot.py` | 0 | **PASSED** | 100% compliance, 3680 distinct colours verified |
| **AArch64** | `python tools/interos.py build --target aarch64` | 0 | **PASSED** | `target/kernel-aarch64.img` (150,431,216 bytes) |
| **AArch64** | `python tests/verify_aarch64_boot.py` | 0 | **PASSED** | 100% compliance, sections 45a–45e OK |
| **AArch64** | `python tests/verify_aarch64_interactive.py` | 0 | **PASSED** | 100% compliance, Calculator glyph + negative tests + reap PID 63 OK |
| **RISC-V 64** | `python tools/interos.py build --target riscv64` | 0 | **PASSED** | Kernel ELF built |
| **RISC-V 64** | `python tests/verify_riscv_boot.py` | 0 | **PASSED** | 100% compliance, sections 45a–45e OK |
| **RISC-V 64** | `python tests/verify_riscv_interactive.py` | 0 | **PASSED** | 100% compliance, Calculator glyph + negative tests + reap PID 151 OK |

---

## 3. Architectural Limitations & Visual Regression Methodology

### 3.1 SMP Evacuation Model Honesty
- **AArch64 & RISC-V 64:** `UserAddressSpace::destroy_fallible()` currently uses a **passive bounded spin protocol** (`100_000` iterations) waiting for remote cores/harts running the target address space to transition through `deactivate()` on scheduler preemption or task exit. No active GIC SGI or SBI IPI preemption kick is sent to interrupt a spinning remote CPU in user mode. On timeout, `destroy_fallible()` cleanly returns `Err(TeardownError::ActiveOnRemoteCpu)` or `Err(TeardownError::RemoteRendezvousFailed)` without releasing any frames, and `reap_task_locksafe()` preserves the task in `TaskState::ReapFailed` for safe deferred recovery.
- **x86_64:** The `kernel/` x86_64 kernel is compiled and booted as a single-processor (uniprocessor) system without APIC TLB shootdown IPIs.

### 3.2 Calculator Structural Pixel-Geometry Assertion
Whole-frame cryptographic hashes (SHA-256) or exact pixel-buffer comparisons are intentionally avoided for display verification because different host GPU drivers, QEMU VNC color depth conversions (RGB565 vs RGB888/BGR), and grayscale anti-aliasing coverage rounding alter raw pixel values without altering user-visible semantics. Instead, `verify_*_interactive.py` evaluates structural glyph geometry over the rendered result display bounding box (checking foreground pixel separation, independent horizontal/vertical stroke components for `'1'` vs `'2'`, aspect ratios, and negative rejection of `"13"`, `"17"`, `"0"`, `"2"`, noise, and bridged digits).
