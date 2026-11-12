# InterOS Core — Phase 8.5 Cross-Architecture Acceptance Matrix

**Date:** 2026-11-12  
**Starting Commit:** `f7eb5038ffca51bc128f33975594c5862232682f`  
**Target Architectures:** `x86_64`, `aarch64`, `riscv64`  

---

## 1. Feature & Safety Invariants Matrix

| Category | Invariant / Requirement | x86_64 Status | AArch64 Status | RISC-V 64 Status | Verification Method |
|---|---|---|---|---|---|
| **Memory Isolation** | User/Kernel Separation (Ring 3 / EL0 / U-Mode) | **VERIFIED** | **VERIFIED** | **VERIFIED** | QEMU Boot & Hardware Exception Handlers |
| **TLB Management** | Remote TLB Invalidation / Shootdown | **VERIFIED** (APIC IPI) | **VERIFIED** (`tlbi vmalle1is` + Quiescence) | **VERIFIED** (SBI RFNC + Status Code Check) | Kernel Unit & Boot Invariant Tests |
| **TLB Failure Handling** | Fallible Teardown & Quarantining | **VERIFIED** | **VERIFIED** (`QUARANTINED_FRAMES`) | **VERIFIED** (`QUARANTINED_FRAMES`) | `destroy_fallible()` tests |
| **Physical Frame Ownership** | Explicit Ownership Classes (`PrivateOwned`, `Shared`, `Borrowed`) | **VERIFIED** | **VERIFIED** | **VERIFIED** | Section 45d Boot Contract Tests |
| **Shared Frame Refcounting** | Retain/Release Semantics (`retain_shared_frame`, `release_shared_frame`) | **VERIFIED** | **VERIFIED** | **VERIFIED** | Multi-address space sharing tests |
| **Borrowed/MMIO Immunity** | Device & Borrowed Frames Never Recycled | **VERIFIED** | **VERIFIED** | **VERIFIED** | Teardown non-reclamation tests |
| **Process Lifecycle** | Genuine User-Mode Execution (50 Cycles) | **VERIFIED** | **VERIFIED** (EL0 ELF SVC dispatch) | **VERIFIED** (U-mode ELF ECALL dispatch) | Section 45e Real Binary Lifecycle Tests |
| **Process Reaping** | Parent `waitpid` Termination & Zero Leaks | **VERIFIED** | **VERIFIED** (Zero frame/task leaks) | **VERIFIED** (Zero frame/task leaks) | 50-cycle pre/post counter comparison |
| **Lifecycle Safety** | Quota Exhaustion (`-ENOMEM`) & Active Reap Denial (`-EBUSY`) | **VERIFIED** | **VERIFIED** | **VERIFIED** | Section 45e Safety Invariant Tests |
| **Service Supervisor** | Incarnation Generation Tracking | **VERIFIED** | **VERIFIED** (`AtomicU32`) | **VERIFIED** (`AtomicU32`) | Generation increment on restart |
| **Service Heartbeats** | Heartbeat Generation & PID Validation | **VERIFIED** | **VERIFIED** (Syscall 90 ABI verified) | **VERIFIED** (Syscall 90 ABI verified) | Service Supervisor Lifecycle Tests |
| **Display Subsystem** | 720x1440 Portrait Framebuffer Presentation | **VERIFIED** | **VERIFIED** | **VERIFIED** | QEMU Framebuffer Sample Analysis |
| **Visual App Parity** | Calculator Visual Evaluation (`3 * 4 = 12`) | **VERIFIED** | **VERIFIED** (Geometric glyph ink analysis) | **VERIFIED** (Geometric glyph ink analysis) | Interactive QEMU UI Test Scripts |
| **Visual Robustness** | Blank / Noise Rejection Negative Tests | **VERIFIED** | **VERIFIED** (`run_glyph_negative_tests`) | **VERIFIED** (`run_glyph_negative_tests`) | Interactive QEMU UI Test Scripts |
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
