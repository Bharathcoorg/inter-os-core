# InterOS Core — Phase 8.5 Test Evidence

**Date:** 2026-11-12  
**Baseline Commit:** `f7eb5038ffca51bc128f33975594c5862232682f`  
**Execution Environment:** Windows 11 host with QEMU (`qemu-system-x86_64`, `qemu-system-aarch64`, `qemu-system-riscv64`), Python 3.13.1, Rust nightly.  

---

## 1. System Contracts & Platform Reality

### Command: `python tools/interos.py contracts`
```
Running contract: tests\contracts\test_app_contract.py ... OK
Running contract: tests\contracts\test_capability_contract.py ... OK
Running contract: tests\contracts\test_developer_contract.py ... OK
Running contract: tests\contracts\test_diagnostics_contract.py ... OK
Running contract: tests\contracts\test_display_contract.py ... OK
Running contract: tests\contracts\test_documentation_contract.py ... OK
Running contract: tests\contracts\test_driver_contract.py ... OK
Running contract: tests\contracts\test_graphics_contract.py ... OK
Running contract: tests\contracts\test_hardware_contract.py ... OK
Running contract: tests\contracts\test_input_contract.py ... OK
Running contract: tests\contracts\test_ipc_contract.py ... OK
Running contract: tests\contracts\test_kernel_contract.py ... OK
Running contract: tests\contracts\test_launcher_contract.py ... OK
Running contract: tests\contracts\test_lifecycle_contract.py ... OK
Running contract: tests\contracts\test_memory_contract.py ... OK
Running contract: tests\contracts\test_model_contract.py ... OK
Running contract: tests\contracts\test_package_contract.py ... OK
Running contract: tests\contracts\test_power_contract.py ... OK
Running contract: tests\contracts\test_rapp_contract.py ... OK
Running contract: tests\contracts\test_recovery_contract.py ... OK
Running contract: tests\contracts\test_security_contract.py ... OK
Running contract: tests\contracts\test_service_contract.py ... OK
Running contract: tests\contracts\test_storage_contract.py ... OK
Running contract: tests\contracts\test_syscall_pointer_contract.py ... OK
Running contract: tests\contracts\test_window_contract.py ... OK
Running license verification ...
  Checking LICENSE file ... OK (Apache-2.0 detected)
  Checking crate manifests ... OK (11 crates verified)
  Checking Cargo.lock checksums ... OK

All 25 contracts and 3 license checks passed.
```

### Command: `python tests/run_all_contracts.py`
```
======================================================================
  InterOS Core - Complete System Contract Verification Suite
======================================================================
Ran 1214 contract checks across 82 framework suites.
System contract status: 25/25 verified.
License contract status: 3/3 verified.
Script contract status: 27/27 verified.
ALL CONTRACTS PASSED (0 failures, 0 warnings).
======================================================================
```

### Command: `python -m pytest tests/test_platform_reality.py`
```
collected 67 items
tests\test_platform_reality.py ......................................... [ 61%]
..........................                                               [100%]
============================= 67 passed in 0.17s ==============================
```

---

## 2. Kernel Compilation & Unit Tests

### Command: `cargo check --workspace`
```
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.45s
Exit code: 0
```

### Command: `cargo test --manifest-path kernel/Cargo.toml`
```
running 594 tests
test ... ok
test result: ok. 593 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
Exit code: 0
```

---

## 3. Architecture Kernel Builds & QEMU Boot Verification

### Command: `python tools/interos.py build demo`
```
Building InterOS Core x86_64 bootable disk image...
Creating target/inter-os-bios.img (13075456 bytes)...
Image built successfully.
```

### Command: `python tests/verify_x86_boot.py`
```
============================================================
  InterOS Core — x86_64 QEMU Canonical Boot Verification
============================================================
[BOOT TEST] Awaiting Launcher homescreen presentation (timeout=180.0s)...
  [GUEST] [BOOT] Serial COM1 online. Checking relocations...
  [GUEST] [OK] GDT & TSS loaded with dedicated Double Fault IST stack & Ring 3 segments.
  [GUEST] [OK] Page tables and physical frame allocator initialized.
  [GUEST] [OK] Persistent storage encryption root initialized before VFS mount.
  [GUEST] [OK] PID1 userspace service manager spawned (Task 1).
  [GUEST] [TASK launcher (ID 11) LOG] [LAUNCHER] Step 5: Initial presentation complete.

--- [Verifying Kernel & Userspace Boot Invariants] ---
  [OK] GDT & TSS Initialization
  [OK] Page Tables & Memory Setup
  [OK] Physical Frame Allocator
  [OK] Microkernel Memory/Hardware
  [OK] Persistent VFS Storage
  [OK] PID1 Userspace Service Manager
  [OK] Launcher / UI Canvas Online
  [OK] Framebuffer pixels verified: 720x1440, 3666 distinct colours, dominant #F5F5F5 at 8.9%, 91.09% non-dominant

============================================================
  [SUCCESS] x86_64 QEMU boot verification PASSED (100%)
============================================================
```

### Command: `python tools/interos.py build --target aarch64`
```
Building InterOS Core for AArch64...
Compiling kernel-aarch64 v0.1.0 (C:\Users\bhara\Desktop\Interlayer\QEMU\inter-os-core\kernel-aarch64)
Finished release [optimized] target(s) in 2.12s
Output: target/aarch64-unknown-none/release/interos-aarch64
```

### Command: `python tests/verify_aarch64_boot.py`
```
============================================================
  InterOS Core — AArch64 QEMU Verification
============================================================
[BOOT] AArch64 kernel booting...
[MMU] MMU enabled with 4KB granule, 48-bit VA space
[TEST] Section 45a User Address-Space Isolation: VERIFIED
[TEST] Section 45b TLB Invalidation Lifecycle: VERIFIED
[TEST] Section 45c Service Supervisor Lifecycle & Recovery: VERIFIED
[TEST] Section 45d Physical Frame Ownership & Shared Mappings: VERIFIED
[TEST] Section 45e Process Execution, Termination & Invariants (50 cycles): VERIFIED
[INIT] PID 1 entering userspace
[LAUNCHER] Homescreen active (720x1440 portrait)
============================================================
  [SUCCESS] AArch64 QEMU boot verification PASSED (100%)
============================================================
```

### Command: `python tests/verify_aarch64_interactive.py`
```
============================================================
  InterOS Core — AArch64 QEMU Interactive Verification
============================================================
[INTERACTIVE] Injecting touch sequence for Calculator: 3 * 4 = 12
[INTERACTIVE] Awaiting frame draw...
[GLYPH] Structural analysis on result card:
  Top Arc Ink Count: 143 (Threshold: >= 50) -> PASS
  Right Neck Ink Count: 90 (Threshold: >= 50) -> PASS
  Bottom Base Ink Count: 141 (Threshold: >= 50) -> PASS
  Negative Space Ink Count: 0 (Threshold: == 0) -> PASS
  Expression Tape Ink Count: 1799 (Threshold: >= 500) -> PASS
[NEGATIVE TEST] Blank frame rejection: PASSED (Detected 0 ink px, rejected as expected)
[NEGATIVE TEST] Noise frame rejection: PASSED (Detected invalid geometry, rejected as expected)
============================================================
  [SUCCESS] AArch64 interactive verification PASSED (100%)
============================================================
```

### Command: `python tools/interos.py build --target riscv64`
```
Building InterOS Core for RISC-V 64...
Compiling kernel-riscv64 v0.1.0 (C:\Users\bhara\Desktop\Interlayer\QEMU\inter-os-core\kernel-riscv64)
Finished release [optimized] target(s) in 1.84s
Output: target/riscv64gc-unknown-none-elf/release/interos-riscv64
```

### Command: `python tests/verify_riscv_boot.py`
```
============================================================
  InterOS Core — RISC-V 64 QEMU Verification
============================================================
[BOOT] RISC-V kernel booting...
[MMU] Sv39 paging enabled
[TEST] Section 45a User Address-Space Isolation: VERIFIED
[TEST] Section 45b TLB Invalidation Lifecycle: VERIFIED
[TEST] Section 45c Service Supervisor Lifecycle & Recovery: VERIFIED
[TEST] Section 45d Physical Frame Ownership & Shared Mappings: VERIFIED
[TEST] Section 45e Process Execution, Termination & Invariants (50 cycles): VERIFIED
[INIT] PID 1 entering userspace
[LAUNCHER] Homescreen active (720x1440 portrait)
============================================================
  [SUCCESS] RISC-V 64 QEMU boot verification PASSED (100%)
============================================================
```

### Command: `python tests/verify_riscv_interactive.py`
```
============================================================
  InterOS Core — RISC-V 64 QEMU Interactive Verification
============================================================
[INTERACTIVE] Injecting touch sequence for Calculator: 3 * 4 = 12
[INTERACTIVE] Awaiting frame draw...
[GLYPH] Structural analysis on result card:
  Top Arc Ink Count: 143 (Threshold: >= 50) -> PASS
  Right Neck Ink Count: 90 (Threshold: >= 50) -> PASS
  Bottom Base Ink Count: 141 (Threshold: >= 50) -> PASS
  Negative Space Ink Count: 0 (Threshold: == 0) -> PASS
  Expression Tape Ink Count: 1799 (Threshold: >= 500) -> PASS
[NEGATIVE TEST] Blank frame rejection: PASSED (Detected 0 ink px, rejected as expected)
[NEGATIVE TEST] Noise frame rejection: PASSED (Detected invalid geometry, rejected as expected)
============================================================
  [SUCCESS] RISC-V 64 interactive verification PASSED (100%)
============================================================
```
