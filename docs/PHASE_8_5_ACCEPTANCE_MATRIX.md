# InterOS Core — Phase 8.5 / 8.5.1 Cross-Architecture Acceptance Matrix

**Date:** 2026-11-12  
**Baseline Commit:** `e617c31961c227810a5de5caed62141091244576`  
**Target Architectures:** `x86_64`, `aarch64`, `riscv64`  

---

## 1. Feature & Safety Invariants Matrix

| Category | Invariant / Requirement | x86_64 Status | AArch64 Status | RISC-V 64 Status | Verification Method |
|---|---|---|---|---|---|
| **Memory Isolation** | User/Kernel Separation (Ring 3 / EL0 / U-Mode) | **VERIFIED** | **VERIFIED** | **VERIFIED** | QEMU Boot & Hardware Exception Handlers |
| **TLB Management** | Remote TLB Invalidation / Shootdown | **NOT IMPLEMENTED** (Uniprocessor kernel; no APIC IPI shootdown in `kernel/src`) | **VERIFIED** (`tlbi vmalle1is` Inner-Shareable Broadcast + Quiescence) | **VERIFIED** (SBI RFNC Probing + `sbi_remote_sfence_vma` Status Code Validation) | Kernel Unit & Boot Invariant Tests (Section 45d) |
| **TLB Failure Handling** | Fallible Teardown & Quarantining | **NOT IMPLEMENTED** (Single-core teardown in `kernel/src`) | **VERIFIED** (`destroy_fallible()`, `QUARANTINED_FRAMES`) | **VERIFIED** (`destroy_fallible()`, `QUARANTINED_FRAMES`) | Section 45d Teardown & Drop Invariant Tests |
| **Atomic AS Lifecycle** | Race-free Activation & Teardown Protocol | **NOT IMPLEMENTED** | **VERIFIED** (`AtomicU32` with Active Mask, Closing & Destroyed Bits) | **VERIFIED** (`AtomicU64` with Active Mask, Closing & Destroyed Bits) | Multi-core CAS & Section 45d Activation Race Tests |
| **Physical Frame Ownership** | Explicit Ownership Classes (`PrivateOwned`, `Shared`, `Borrowed`) | **VERIFIED** | **VERIFIED** (Explicit Peripheral MMIO Aperture Filtering) | **VERIFIED** (Explicit Peripheral MMIO Aperture Filtering) | Section 45d Physical Frame Reclamation Tests |
| **Shared Frame Refcounting** | Retain/Release Semantics (`retain_shared_frame`, `release_shared_frame`) | **VERIFIED** | **VERIFIED** (Overflow & Underflow Guarded) | **VERIFIED** (Overflow & Underflow Guarded) | Multi-address space sharing tests |
| **Borrowed/MMIO Immunity** | Device & Borrowed Frames Never Recycled | **VERIFIED** | **VERIFIED** (`is_valid_mmio_peripheral` Verified) | **VERIFIED** (`is_valid_mmio_peripheral` Verified) | Teardown non-reclamation tests |
| **Process Lifecycle** | Genuine User-Mode Execution (50 Cycles) | **VERIFIED** | **VERIFIED** (EL0 ELF SVC dispatch) | **VERIFIED** (U-mode ELF ECALL dispatch) | Section 45e Real Binary Lifecycle Tests |
| **Process Reaping** | Parent `waitpid` Termination & Zero Leaks | **VERIFIED** | **VERIFIED** (Zero frame/task leaks) | **VERIFIED** (Zero frame/task leaks) | 50-cycle pre/post counter comparison |
| **Lifecycle Safety** | Quota Exhaustion (`-ENOMEM`) & Active Reap Denial (`-EBUSY`) | **VERIFIED** | **VERIFIED** | **VERIFIED** | Section 45e Safety Invariant Tests |
| **Service Supervisor** | Incarnation Generation & Non-Mutating Reaping | **VERIFIED** | **VERIFIED** (`sched.terminate_task()`, `AtomicU32` Incarnation) | **VERIFIED** (`sched.terminate_task()`, `AtomicU32` Incarnation) | Service restart & supervisor reaping tests |
| **Service Heartbeats** | Heartbeat Generation & PID Validation | **VERIFIED** | **VERIFIED** (Mandatory non-zero generation validation) | **VERIFIED** (Mandatory non-zero generation validation) | Service Supervisor Lifecycle Tests (Section 46) |
| **Display Subsystem** | 720x1440 Portrait Framebuffer Presentation | **VERIFIED** | **VERIFIED** | **VERIFIED** | QEMU Framebuffer Sample Analysis |
| **Visual App Parity** | Calculator Visual Evaluation (`7 + 5 = 12`) | **VERIFIED** | **VERIFIED** (Independent '1' stem and '2' arc/neck/base glyph geometry) | **VERIFIED** (Independent '1' stem and '2' arc/neck/base glyph geometry) | Interactive QEMU UI Test Scripts |
| **Visual Robustness** | Blank, Noise, and Erroneous ("13", "17", "0", "2") Negative Tests | **VERIFIED** | **VERIFIED** (`run_glyph_negative_tests`) | **VERIFIED** (`run_glyph_negative_tests`) | Interactive QEMU UI Test Scripts |
| **Platform Truth** | Zero Mock Hardware Telemetry / Zero Ambient Authority | **VERIFIED** | **VERIFIED** | **VERIFIED** | `tests/test_platform_reality.py` (67 passed) |

---

## 2. Test Execution Evidence & Pass Status

| Target Architecture | Test Script / Suite | Exit Code | Result | Key Indicators |
|---|---|---|---|---|
| **Workspace** | `cargo check --workspace` | 0 | **PASSED** | 0 compilation errors or warnings |
| **Workspace** | `python tools/interos.py contracts` | 0 | **PASSED** | 25/25 system contracts, 3/3 license contracts |
| **Workspace** | `python tests/run_all_contracts.py` | 0 | **PASSED** | 1214 passed across 82 suites + 27 script contracts |
| **Workspace** | `python -m pytest tests/test_platform_reality.py` | 0 | **PASSED** | 67 passed in 0.17s |
| **x86_64** | `cargo test --manifest-path kernel/Cargo.toml` | 0 | **PASSED** | 593 passed, 0 failed, 1 ignored |
| **x86_64** | `python tools/interos.py build demo` | 0 | **PASSED** | Built `target/inter-os-bios.img` (13,075,456 bytes) |
| **x86_64** | `python tests/verify_x86_boot.py` | 0 | **PASSED** | 100% compliance, 3666 distinct colours verified |
| **AArch64** | `python tools/interos.py build --target aarch64` | 0 | **PASSED** | Kernel ELF built |
| **AArch64** | `python tests/verify_aarch64_boot.py` | 0 | **PASSED** | 100% compliance, all sections 45a–45e OK |
| **AArch64** | `python tests/verify_aarch64_interactive.py` | 0 | **PASSED** | 100% compliance, Calculator glyph + negative tests OK |
| **RISC-V 64** | `python tools/interos.py build --target riscv64` | 0 | **PASSED** | Kernel ELF built |
| **RISC-V 64** | `python tests/verify_riscv_boot.py` | 0 | **PASSED** | 100% compliance, all sections 45a–45e OK |
| **RISC-V 64** | `python tests/verify_riscv_interactive.py` | 0 | **PASSED** | 100% compliance, Calculator glyph + negative tests OK |
