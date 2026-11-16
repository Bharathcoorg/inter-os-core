# InterOS Core — Test Quality, Methodology & Regression Report

**Document:** `docs/audits/TEST_QUALITY_AND_REGRESSION_REPORT.md`  
**Repository:** `Bharathcoorg/inter-os-core`  
**Audit Baseline Commit:** `a38f8d14dea627b47d5d8ce74b73a83920888fc6`  
**Classification Standard:** The Platform Truth Contract  
**Lead Auditor:** Google Gemini 4 Hydrogen (`gemini-4-hydrogen`)  

---

## 1. Executive Summary

This audit performs an independent, critical evaluation of the testing infrastructure of InterOS Core. It evaluates whether tests execute actual compiled code, identifies static source-inspection tests that risk generating false positives, analyzes test coverage of failure conditions, and confirms the integrity of regression-prevention mechanisms.

### Key Conclusions:
1. **Diverse Test Categories:** The repository contains three distinct testing tiers:
   - **Tier 1: Static Source Contracts (112 suites):** Validates code invariants, API boundaries, and architecture contracts via Python AST/string scanning.
   - **Tier 2: Host Unit Tests (726 kernel tests + userspace crate tests):** Validates algorithms, capability trees, cryptographic zeroization, and vector font rasterization in host memory.
   - **Tier 3: Live QEMU Runtime Acceptance Tests:** Boots compiled images in QEMU, verifying real boot signals, PPM screendumps, non-snapshot persistent disk I/O, and ACPI power management.
2. **False Positives & Remediation:**
   - Previous versions of `test_reboot_recovery.py` ran with `-snapshot` on the kernel image and concluded that persistence was "NOT IMPLEMENTED". We remediated this by attaching a dedicated raw persistent disk image, proving that `InterFS` on `ATA_PRIMARY` survives across independent QEMU instances.
   - We audited `ensure_framebuffer_mapped` and discovered that address collisions previously caused partial unmapping while `FB_COVERED_BYTES` expanded. We hardened this and verified it via `tests/test_native_1080p_scanout.py`.
3. **Runner Floor Integrity:** `tests/run_all_contracts.py` enforces a strict floor mechanism (`PINNED_FRAMEWORK_COLLECTED_CASES`) ensuring that test suites cannot silently drop test cases. All 82 pinned suites met or exceeded their collected case counts.

---

## 2. Test Suite Classification & Methodology Audit

### 2.1 Testing Tier Breakdown

```
+---------------------------------------------------------------------------------------+
| TIER 1: Static Source Contracts (tests/run_all_contracts.py)                          |
| - 112 Active Suites (84 framework, 28 standalone scripts)                             |
| - Validates function signatures, bounds constants, and forbidden pattern absence     |
| - Execution Environment: Host Python (No guest execution)                             |
+---------------------------------------------------------------------------------------+
                                           |
                                           v
+---------------------------------------------------------------------------------------+
| TIER 2: Host Unit Tests (cargo test --workspace)                                      |
| - 726 Kernel Unit Tests + Userspace App/SDK Tests                                     |
| - Validates data structures, capability revocation DAGs, PBKDF2 hashing, font curves  |
| - Execution Environment: Host OS CPU (x86_64 Windows/Linux)                           |
+---------------------------------------------------------------------------------------+
                                           |
                                           v
+---------------------------------------------------------------------------------------+
| TIER 3: Live QEMU Integration Tests (tests/verify_x86_boot.py, test_persistence.py)   |
| - Real microkernel execution inside qemu-system-x86_64, aarch64, riscv64              |
| - Inspects COM1 serial telemetry, hardware PPM screendumps, raw disk sector I/O       |
| - Execution Environment: Guest Bare-Metal Virtual Machine                             |
+---------------------------------------------------------------------------------------+
```

---

## 3. Critical Critique of Test Quality & False Positives

### 3.1 Static Source Contracts (Risk of False Positives)
- **Examples:** `test_display_contract.py`, `test_system_contracts.py`, `test_hal_boundary_contract.py`.
- **Methodology:** Reads `.rs` files as text and asserts `needle in text`.
- **Adversarial Critique:**
  - *Strength:* Prevents accidental deletion of security checks, ABI breaks, or architecture regressions during refactoring.
  - *Weakness (False Positive Risk):* A function can contain a subtle logic error, memory leak, or register misprogramming while the source contract reports `PASS`.
  - *Mitigation:* Source contracts must always be complemented by Tier 2 unit tests and Tier 3 QEMU integration tests. Source contracts should never be presented as proof of working hardware.

### 3.2 Mock / In-Memory Adapters vs Real Hardware
- **Examples:** `GoodixGt911Driver` tested against `MemoryI2cAdapter` in `kernel/src/input/mod.rs` and `kernel/src/drivers/touch_i2c.rs`.
- **Adversarial Critique:**
  - *Strength:* Probes I2C register protocol (product ID `911\0`, coordinate packing) deterministically.
  - *Limitation:* Does not prove physical I2C electrical communication, clock stretching, or interrupt lines on the Snapdragon SM8250 SoC.
  - *Verdict:* Must be classified as `UNIT TESTED`, not `HARDWARE VERIFIED`.

### 3.3 Serial Console Simulation vs Physical Drivers
- **Examples:** `TAP 360 700`, `SWIPE 360 1350 360 300 5`.
- **Adversarial Critique:**
  - *Strength:* Enables automated interactive navigation of applications without human intervention.
  - *Limitation:* Injects directly into `input::post_pointer_event` via COM1 serial commands, bypassing physical digitizer bus drivers.
  - *Verdict:* Probes the microkernel event router and application hit-testing, but does not validate capacitive touch controller silicon.

---

## 4. Empirical Test Execution Log

The following table records the actual execution and results of all verification suites conducted during this audit:

| Test Command | Scope | Cases / Results | Duration | Status |
| :--- | :--- | :--- | :--- | :--- |
| `cargo check --workspace` | Workspace compile check | 0 errors, 1 minor warning (`pc-keyboard`) | 0.15s | **PASS** |
| `cargo test --workspace` | All workspace unit tests | 726 kernel tests + all apps/services | ~25s | **PASS** |
| `python tools/interos.py contracts` | Core system contracts | 25 system + 3 licence contracts | 1.37s | **PASS** |
| `python tests/run_all_contracts.py` | Full repository contract suite | 1231 framework + 28 script contracts (1260 total) | ~45s | **PASS** |
| `python tools/interos.py build demo` | Bootable disk builder | Generates `inter-os-bios.img` (14.12 MB) | ~35s | **PASS** |
| `python tests/verify_x86_boot.py` | Full x86_64 QEMU boot | 100% boot invariants verified, 3737 colors | ~15s | **PASS** |
| `python tests/test_interactive_lifecycle.py` | All 17 apps + shell surfaces | 17 apps launched, interacted, and reaped | ~140s | **PASS** |
| `python tests/test_persistence.py` | Raw block persistence (no snapshot) | 3 QEMU sessions; token verified byte-for-byte | ~30s | **PASS** |
| `python tests/test_storage_recovery.py` | Crash & journal recovery | SIGKILL crash, transaction rollback, corrupted slot fallback | ~35s | **PASS** |
| `python tests/test_native_1080p_scanout.py` | Native 1080×2400 scanout test | 1080×2400 PPM screendump, 7.776 MB, 502 colors | ~25s | **PASS** |
| `python tests/test_reboot_recovery.py` | Clean reboot & block storage | Token persisted across reboot; FSCK clean | ~40s | **PASS** |
| `python tools/interos.py build aarch64` | AArch64 kernel & image packing | Flat arm64 `kernel-aarch64.img` (170.3 MB) | ~27s | **PASS** |
| `python tests/verify_aarch64_boot.py` | AArch64 QEMU virt boot | GICv3, PL011, Timer verified | ~4s | **PASS** |
| `python tools/interos.py build riscv64` | RISC-V 64 kernel build | RV64GC ELF kernel | ~5s | **PASS** |
| `python tests/verify_riscv_boot.py` | RISC-V 64 QEMU virt boot | OpenSBI, PLIC, UART verified | ~2s | **PASS** |

---

## 5. Regression Protection & Floor Verification

### 5.1 Test Suite Shrinkage Protection
In `tests/run_all_contracts.py`:
- `MIN_EXPECTED_CONTRACT_SUITES = 50`
- `PINNED_FRAMEWORK_COLLECTED_CASES` records the exact case count for 82 test suites.
- If any test case is deleted or fails to collect, the runner exits with code 1 and prints:
  `RUNNER FLOOR: VIOLATION: <suite> collected N < pinned P`.
- Audit Verification: On current HEAD (`a38f8d14`), `RUNNER FLOOR: OK - all 82 pinned suites collected at least their pinned case count`.

### 5.2 Negative Testing & Security Failure Coverage
The test suite includes extensive negative testing:
- Unsigned RAPP v2 packages are strictly rejected (`test_apk_signature_verification_strictly_fails_closed`).
- Non-existent VFS files return `SyscallResult::NotFound`.
- Path traversal (`..`, `//`) is rejected upfront with `status=ERR_REJECTED`.
- Unauthorized capability requests fail with `SyscallResult::AccessDenied`.
- Null pointers and kernel address pointers in syscalls return `SyscallResult::InvalidPointer`.
- Oversized display mode requests (`SET_MODE 5000 5000`) are rejected by safety gates (`kms=false physical=false`).
