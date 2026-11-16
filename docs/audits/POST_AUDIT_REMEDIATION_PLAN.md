# InterOS Core — Post-Audit Remediation Plan

**Document:** `docs/audits/POST_AUDIT_REMEDIATION_PLAN.md`  
**Repository:** `Bharathcoorg/inter-os-core`  
**Branch:** `master` ONLY  
**Baseline Git Commit:** `47748a4298346df08746be3b84764e8b35ee4973`  
**Starting Audit Commit:** `02af1b661f886084c1de9ab5008a07ba36ee53ec`  
**Current Classification:** `ADVANCED QEMU PROTOTYPE`  
**Adherence Standard:** The Platform Truth Contract  

---

## 1. Executive Remediation Mission

This document defines the actionable engineering plan to remediate findings identified in the Independent Platform Audit (`docs/audits/INDEPENDENT_PLATFORM_AUDIT.md` through `TEST_QUALITY_AND_REGRESSION_REPORT.md`).

The objective is **not** to inflate platform readiness or artificially alter verdicts, but to systematically harden the microkernel, verify end-to-end subsystem reality in QEMU, eliminate false positives, and ensure empirical repeatability.

---

## 2. Prioritized Remediation Matrix

The matrix organizes findings into 7 priority tiers in accordance with project instructions:

| Finding ID | Track / Subsystem | Original Document | Severity | Affected Source Files | Status |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **REM-SEC-01** | Kernel Security: Syscall Memory | `SECURITY_AND_KERNEL_RELIABILITY.md` | **HIGH** | `kernel/src/syscall/mod.rs` | **IN PROGRESS** |
| **REM-SEC-02** | Kernel Security: DMA & Page Bounds | `SECURITY_AND_KERNEL_RELIABILITY.md` | **HIGH** | `kernel/src/memory.rs`, `kernel/src/drivers/audio.rs` | **IN PROGRESS** |
| **REM-SEC-03** | Kernel Security: Secret Zeroization | `SECURITY_AND_KERNEL_RELIABILITY.md` | **MEDIUM** | `kernel/src/security/`, `kernel/src/task/mod.rs` | **IN PROGRESS** |
| **REM-DISP-01**| Display: Native 1080×2400 vs 720×1440 | `DISPLAY_AND_INPUT_VERIFICATION.md` | **MEDIUM** | `kernel/src/drivers/display_kms.rs`, `kernel/src/framebuffer.rs` | **IN PROGRESS** |
| **REM-DISP-02**| Display: Dirty Region & Damage Tracking| `DISPLAY_AND_INPUT_VERIFICATION.md` | **LOW** | `kernel/src/drivers/display_kms.rs` | **IN PROGRESS** |
| **REM-INP-01** | Input: PS/2 vs Serial Test Bridge | `DISPLAY_AND_INPUT_VERIFICATION.md` | **MEDIUM** | `kernel/src/drivers/ps2_mouse.rs`, `kernel/src/serial.rs` | **IN PROGRESS** |
| **REM-INP-02** | Input: Event Processing Latency | `DISPLAY_AND_INPUT_VERIFICATION.md` | **LOW** | `kernel/src/input/queue.rs`, `kernel/src/drivers/touch_gestures.rs`| **IN PROGRESS** |
| **REM-STOR-01**| Storage: In-Memory VFS vs InterFS Sync| `STORAGE_AND_RECOVERY_VERIFICATION.md` | **HIGH** | `kernel/src/fs/vfs.rs`, `kernel/src/storage/interfs.rs` | **IN PROGRESS** |
| **REM-STOR-02**| Storage: Non-Snapshot Crash Injection | `STORAGE_AND_RECOVERY_VERIFICATION.md` | **MEDIUM** | `kernel/src/storage/interfs.rs`, `tests/test_storage_recovery.py` | **IN PROGRESS** |
| **REM-AUD-01** | Audio: Real QEMU AC'97 Verification | `INDEPENDENT_PLATFORM_AUDIT.md` | **MEDIUM** | `kernel/src/drivers/audio.rs`, `kernel/src/audio/mod.rs` | **IN PROGRESS** |
| **REM-AUD-02** | Audio: Mixer Volume Hardware Mapping | `INDEPENDENT_PLATFORM_AUDIT.md` | **LOW** | `kernel/src/audio/mod.rs`, `kernel/src/drivers/audio.rs` | **IN PROGRESS** |
| **REM-ARCH-01**| Architecture: Multi-Arch Capability Parity| `HARDWARE_READINESS_MATRIX.md` | **MEDIUM** | `kernel-aarch64/`, `kernel-riscv64/` | **IN PROGRESS** |
| **REM-PERF-01**| Performance: Measured Latency & Memory | `TEST_QUALITY_AND_REGRESSION_REPORT.md` | **LOW** | `tools/interos.py`, `tests/` | **IN PROGRESS** |
| **REM-HW-01**  | Hardware: SM8250 Bring-Up Prerequisites | `HARDWARE_READINESS_MATRIX.md` | **CRITICAL** (HW) | `docs/hardware/`, `kernel-aarch64/` | **IN PROGRESS** |

---

## 3. Finding Deep-Dives & Remediation Specifications

### 3.1 REM-SEC-01: Syscall User-Memory Bounds Validation
- **Original Document:** `docs/audits/SECURITY_AND_KERNEL_RELIABILITY.md` §3.2
- **Severity:** High
- **Affected Files:** `kernel/src/syscall/mod.rs`
- **Technical Limitation:** `verify_user_slice` checks `ptr != null` and `ptr + len <= USER_SPACE_END`, but lacks explicit tests for integer overflow edge-cases (e.g. `u64::MAX - 4`) and cross-boundary allocations where `ptr < USER_SPACE_END` but `ptr + len > USER_SPACE_END`.
- **Reproduction Procedure:** Invoke syscall with `ptr = 0x0000_7FFF_FFFF_FF00` and `len = 0x1000`.
- **Proposed Correction:** Ensure `checked_add` and non-wrapping arithmetic strictly gate every user slice check. Expand negative test suite.
- **Required Regression Test:** Unit test in `kernel/src/syscall/mod.rs` and live QEMU syscall injection verifying immediate rejection with `SyscallResult::InvalidPointer`.
- **Evidence for Closure:** 100% pass on negative syscall pointer test cases.

### 3.2 REM-SEC-02: DMA Frame Continuity & Bounds Safety
- **Original Document:** `docs/audits/SECURITY_AND_KERNEL_RELIABILITY.md` §3.1 & `docs/audits/INDEPENDENT_PLATFORM_AUDIT.md` Track D
- **Severity:** High
- **Affected Files:** `kernel/src/drivers/audio.rs`, `kernel/src/memory.rs`
- **Technical Limitation:** In `kernel/src/drivers/audio.rs:429-434`, `Ac97Controller::from_pci` verifies `bdl_phys <= u32::MAX` and `buffer_end <= u32::MAX`, but assumes that `buffer_frames` (multi-page allocation) is physically contiguous without explicitly auditing `frames[i].start_address() + 4096 == frames[i+1].start_address()`.
- **Reproduction Procedure:** Under memory fragmentation, non-contiguous multi-frame allocation would result in corrupted audio DMA transfers.
- **Proposed Correction:** Explicitly verify physical continuity across all allocated DMA frames in `from_pci` before programming buffer descriptors.
- **Required Regression Test:** Unit test checking contiguous validation and failure handling.
- **Evidence for Closure:** Zero DMA discontinuity vulnerability; clean validation pass.

### 3.3 REM-DISP-01: Display Scanout Scaling & Viewport Modeset
- **Original Document:** `docs/audits/DISPLAY_AND_INPUT_VERIFICATION.md` §2.2
- **Severity:** Medium
- **Affected Files:** `kernel/src/drivers/display_kms.rs`, `kernel/src/framebuffer.rs`
- **Technical Limitation:** Standard applications author surfaces at 720×1440. On 1080×2400 scanout, KMS scales 1.5× with 120px letterboxes. True 1080×2400 client surfaces must be blitted 1:1, but client application surfaces authored at 1080×2400 must not exceed the 48 MiB surface budget.
- **Reproduction Procedure:** Test both 720×1440 letterboxed scaling and true 1080×2400 full-canvas rendering in live QEMU.
- **Proposed Correction:** Maintain explicit scanout geometry modes, enforce letterbox clipping, and verify 1080×2400 full-surface composition.
- **Required Regression Test:** `tests/test_native_1080p_scanout.py` and screendump verification.
- **Evidence for Closure:** Authentic PPM screendumps at both 720×1440 and 1080×2400.

### 3.4 REM-STOR-01: InterFS Transactional Durability & Non-Snapshot Flush
- **Original Document:** `docs/audits/STORAGE_AND_RECOVERY_VERIFICATION.md` §2 & §3
- **Severity:** High
- **Affected Files:** `kernel/src/fs/vfs.rs`, `kernel/src/storage/interfs.rs`
- **Technical Limitation:** Root VFS is in-memory. Only `/data` is backed by IDE Primary Slave (`ATA_PRIMARY`). Applications writing outside `/data` lose data on reboot.
- **Reproduction Procedure:** Boot QEMU without snapshot, write `/data/file`, sync, reboot, verify; write `/root/file`, reboot, verify absence.
- **Proposed Correction:** Ensure all persistent userspace state is strictly routed to `/data/` and flushed via `sys_sync`. Add explicit documentation and path checks.
- **Required Regression Test:** `tests/test_persistence.py` and `tests/test_reboot_recovery.py`.
- **Evidence for Closure:** Cross-process data survival on raw disk images without QEMU `-snapshot`.

### 3.5 REM-AUD-01: Real AC'97 Audio Output in QEMU
- **Original Document:** `docs/audits/INDEPENDENT_PLATFORM_AUDIT.md` Track D
- **Severity:** Medium
- **Affected Files:** `kernel/src/drivers/audio.rs`, `kernel/src/audio/mod.rs`
- **Technical Limitation:** Audio tests were previously Tier 1 source checks (`tests/test_audio_contract.py`). Real QEMU execution with `-device AC97` had not been empirically demonstrated with audio capture.
- **Reproduction Procedure:** Launch QEMU with `-audiodev wav,id=aud0,path=target/audio_out.wav -device AC97,audiodev=aud0`, trigger audio tone or PCM stream, verify non-zero WAV file generation and PCI probe log.
- **Proposed Correction:** Create automated acceptance test `tests/test_qemu_audio_playback.py` verifying real AC'97 hardware binding in QEMU.
- **Required Regression Test:** `tests/test_qemu_audio_playback.py`.
- **Evidence for Closure:** QEMU serial log `[OK] AC'97 PCM output initialized: PCI ...` and valid WAV capture.

### 3.6 REM-ARCH-01: Cross-Architecture Parity & Honest Limits
- **Original Document:** `docs/audits/HARDWARE_READINESS_MATRIX.md` §2
- **Severity:** Medium
- **Affected Files:** `kernel-aarch64/`, `kernel-riscv64/`
- **Technical Limitation:** AArch64 and RISC-V 64 boot in QEMU virt, but do not provide equivalent userspace smartphone applications or KMS compositor as x86_64.
- **Reproduction Procedure:** Inspect boot logs and kernel source trees across all 3 targets.
- **Proposed Correction:** Document precise architecture-by-architecture parity table distinguishing microkernel boot vs full userspace graphical stack.
- **Required Regression Test:** `tests/verify_aarch64_boot.py` and `tests/verify_riscv_boot.py`.
- **Evidence for Closure:** Fully verified boot logs and clearly defined architecture limits.

### 3.7 REM-HW-01: OnePlus 8 Pro / SM8250 Bring-Up Prerequisites
- **Original Document:** `docs/audits/HARDWARE_READINESS_MATRIX.md` §3 & §4
- **Severity:** Critical (for hardware bring-up)
- **Affected Files:** `docs/hardware/SM8250_ONEPLUS8PRO_BRINGUP_FEASIBILITY.md`
- **Technical Limitation:** Physical hardware execution is deferred. No physical drivers exist for Qualcomm UFS, DPU, or SPMI PMIC.
- **Proposed Correction:** Maintain classification `NOT IMPLEMENTED / DEFERRED`. Deliver an authoritative dependency map showing exact steps required before physical silicon execution can be attempted safely.
- **Evidence for Closure:** Clear dependency roadmap avoiding premature claims.
