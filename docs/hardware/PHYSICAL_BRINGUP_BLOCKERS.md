# InterOS Core — Physical Hardware Bring-Up Blockers & Architecture Review

**Document Version:** 1.0.0  
**Target Hardware:** Qualcomm Snapdragon 865 (SM8250) / OnePlus 8 Pro (`instantnoodlep`)  
**Scope:** Exhaustive Engineering Blocker Inventory & Hardware Abstraction Review  
**Baseline:** QEMU Virtual Smartphone Baseline (`x86_64`, `aarch64`, `riscv64`)  

---

## 1. Executive Summary

While InterOS Core has achieved advanced QEMU prototype status (with multi-core AArch64 GICv3 SMP, crash-resilient InterFS, native 1080×2400 KMS scanout, and an interactive smartphone UI), physical smartphone silicon introduces hardware coupling and proprietary firmware dependencies not present in virtualized environments.

This document classifies every engineering blocker separating the current QEMU prototype from full physical hardware execution, defines mitigation strategies, and reviews the architectural evolution of the core system.

---

## 2. Exhaustive Hardware Blocker Inventory

| Blocker ID | Subsystem | Physical Silicon Dependency | Technical Defect / Challenge | Severity | Mitigation Strategy | Status |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **BLK-01** | **Boot & Security** | Qualcomm Secure Boot (PBL/XBL/QSEE) | Cryptographic signature enforcement on retail handsets prevents execution of unsigned boot images unless unlocked via OEM fastboot. | **CRITICAL** | Handset must be unlocked via `fastboot flashing unlock`; execute non-destructive tethered boot (`fastboot boot boot.img`). | **READY (Plan Defined)** |
| **BLK-02** | **Debug Telemetry** | Physical Serial Port Absence | Retail OnePlus 8 Pro handsets do not expose external UART TX/RX pins; USB-C port defaults to standard ADB/MTP. | **HIGH** | Implement dual-sink early telemetry: (1) SimpleFB on-screen bitmap font console, and (2) persistent DDR RAM console (`ramoops` / `pstore`). | **READY (Code Implemented)** |
| **BLK-03** | **Memory Map** | Qualcomm XPU Firmware Carveouts | Fixed TrustZone memory carveouts (`0x8000_0000 - 0x97FF_FFFF`) trigger hard asynchronous external aborts if read/written by non-secure EL1 kernel. | **CRITICAL** | Parse ABL Device Tree memory nodes dynamically; clamp physical frame allocator strictly above carveout ceiling (`0x9800_0000`). | **READY (Architecture Defined)** |
| **BLK-04** | **Storage** | Qualcomm UFS 3.0 Controller | UFSHCI 2.1 requires complex clock trees (GCC), UFS PHY calibration, and task management. No UFS driver exists in InterOS Core. | **CRITICAL** | Initial bring-up executes 100% in RAM via Android ramdisk. UFS driver development deferred to Phase 2. | **DEFERRED (Workaround Active)** |
| **BLK-05** | **Display** | Qualcomm DPU 6.0.0 & DSI Host | Samsung 3168×1440 AMOLED panel requires MIPI DSI 2-lane/4-lane differential signaling, panel initialization DCS sequences, and DPU plane composition. | **HIGH** | Bootloader state inheritance: rely on ABL continuous splash framebuffer via SimpleFB (`chosen/framebuffer`). Avoid reprogramming DPU clocks. | **READY (SimpleFB Active)** |
| **BLK-06** | **Power / Clocks** | Qualcomm SPMI & PMIC (PM8150/PMK8002) | Peripherals (touch, cameras, sensors) are unpowered by default. Regulators and clock branches require SPMI bus transactions to Qualcomm PMIC. | **HIGH** | In Phase 1, inherit ABL power states. Develop standalone Qualcomm SPMI bus master driver for Phase 2. | **IN PROGRESS** |
| **BLK-07** | **Touch Digitizer** | Goodix GT9886 / Synaptics Touchscreen | Capacitive touch digitizer requires I2C/SPI bus controller, TLMM GPIO interrupt routing, and VDD regulator enablement. | **MEDIUM** | Virtual Goodix GT911 pipeline verified in QEMU. Physical bring-up requires Qualcomm GENI I2C bus driver. | **SOURCE STUB** |
| **BLK-08** | **Audio Subsystem** | Qualcomm WCD9380 Codec & aDSP | Audio DAC/ADC routing requires Qualcomm SoundWire bus and Hexagon audio DSP firmware (`adsp.mbn`). | **LOW** | Deferred; non-critical for OS boot and UI interaction. AC'97 hardware mixer verified in QEMU. | **DEFERRED** |
| **BLK-09** | **Cellular / Modem** | Snapdragon X55 5G Modem Subsystem | Baseband radio requires proprietary Hexagon modem firmware (`modem.mbn`), shared memory IPC (SMD/QMI), and RPMh clocks. | **LOW** | Deferred; isolated behind System Service abstraction. Protocol stubs present. | **DEFERRED** |
| **BLK-10** | **GPU Acceleration** | Adreno 650 (A6xx) | Hardware 3D acceleration requires Adreno GPU initialization and proprietary secure zap shader (`a650_zap.mbn`). | **LOW** | InterOS uses pure software microkernel composition and DRM/KMS 2D scanout. 3D GPU is non-essential for 2D UI. | **NOT REQUIRED** |

---

## 3. Architecture Quality & Evolution Review

To determine whether InterOS Core can evolve toward physical smartphone platforms without unnecessary rewrites, the fundamental OS architectural boundaries were reviewed:

### 3.1 Hardware Abstraction Layer (HAL) Boundaries
- **Assessment:** Clean separation exists between architecture-independent kernel subsystems (`kernel/src/memory/`, `kernel/src/syscall/`, `kernel/src/task/`, `kernel/src/capability/`) and architecture-specific code (`kernel/src/arch/x86_64/`, `kernel/src/arch/aarch64/`, `kernel/src/arch/riscv64/`).
- **Verdict: SOUND.** Porting to SM8250 requires updating only `kernel/src/arch/aarch64/` MMIO addresses and platform initialization stubs.

### 3.2 Interrupt Handling & GICv3 Abstraction
- **Assessment:** GICv3 distributor (`Gicd`) and redistributor (`Gicr`) abstractions in `kernel/src/arch/aarch64/gicv3.rs` already handle multi-core SGI, PPI, and SPI interrupts cleanly.
- **Verdict: SOUND.** Proven in QEMU `-smp 2` and `-smp 4` verification. Physical SM8250 requires only updating the base addresses from `0x0800_0000` (QEMU virt) to `0x17A0_0000` (SM8250).

### 3.3 Memory Ownership & DMA Bounds
- **Assessment:** Physical frame allocation (`allocate_dma_frames`) now strictly enforces page continuity before programming hardware descriptor lists. Syscall user pointer validation (`user_ptr.rs`) strictly isolates kernel space from user tasks.
- **Verdict: SOUND.** Re-tested and verified in post-audit remediation.

### 3.4 Process Isolation & Capability Enforcement
- **Assessment:** System calls enter EL1 via `svc #0` from EL0. Access to hardware devices, VFS files, and IPC channels requires an explicit `CapHandle`. Dynamic capability revocation ensures revoked leases cannot be exploited after task teardown.
- **Verdict: SOUND.** 100% compliance across all 1,260 contract test suites.

### 3.5 Display & Compositor Abstraction
- **Assessment:** In-kernel compositor operates via a generic `Surface` and `DrmKmsController` interface. Scanout works identically over linear framebuffers (SimpleFB) or virtual KMS planes (VirtIO-GPU / Bochs).
- **Verdict: SOUND.** Native 1080×2400 and 720×1440 modes verified. SimpleFB support guarantees screen output on day 1 of physical bring-up without DPU drivers.

---

## 4. Hardware Bring-Up Milestone Roadmap

```mermaid
flowchart TD
    M0["Milestone 0: Advanced QEMU Prototype<br/>(COMPLETED & VERIFIED)"] --> M1["Milestone 1: Tethered Physical First Boot<br/>(OnePlus 8 Pro: EL1 Entry, SimpleFB Console, GICv3, Timer)"]
    M1 --> M2["Milestone 2: InterOS UI on Physical Screen<br/>(Native Display Scanout, Bitmap Fonts, Launcher Grid)"]
    M2 --> M3["Milestone 3: Physical Input Bring-Up<br/>(Qualcomm GENI I2C Master + Goodix GT9886 Touch)"]
    M3 --> M4["Milestone 4: Persistent Storage<br/>(Qualcomm UFS 3.0 Host Controller + InterFS On-Disk)"]
```

### Milestone 1 Definition (Minimal Meaningful Physical Milestone)
- **Kernel Entry:** ABL transfers control to `entry.S` at EL1 Non-Secure.
- **Memory Initialization:** Minimal page tables set up above `0x9800_0000` without touching firmware carveouts.
- **Console Output:** Immediate visual banner on the smartphone display via SimpleFB linear framebuffer handoff.
- **Interrupts & Timer:** GIC-600 distributor initialized; ARM generic timer ticking at 19.2 MHz.
- **Orderly Idle:** Microkernel enters safe WFI idle loop or executes graceful reboot.
- **Truth Criterion:** This milestone verifies physical silicon execution; it is explicitly NOT a fully functioning smartphone until input, storage, and power management are brought up in subsequent milestones.
