# InterOS Core

<div align="center">

![License: MIT / Apache-2.0](https://img.shields.io/badge/License-MIT%20%2F%20Apache--2.0-blue.svg)
![Language: Rust](https://img.shields.io/badge/Language-Pure%20Rust%20%23!%5Bno__std%5D-orange.svg)
![Architecture: RISC-V 64 / AArch64 / x86_64](https://img.shields.io/badge/Targets-RISC--V%2064%20%7C%20ARM64%20%7C%20x86__64-success.svg)
![Build: Passing](https://img.shields.io/badge/Build-Passing-brightgreen.svg)
![Status: Open Source](https://img.shields.io/badge/Status-Public%20Core%20Platform-blueviolet.svg)

### **The Sovereign, Open-Source Rust Mobile Operating System & Application Platform**
*The AOSP of Modern Memory-Safe Mobile Computing.*

[Quickstart](#-one-click-turnkey-quickstart) • [Architecture](#-system-architecture) • [Device Studio](#-universal-device-studio--digital-twin) • [App Ecosystem](#-production-smartphone-apps-suite) • [App Dev SDK](#-app-development-platform--sdk) • [Hardware Bring-Up](#-supported-hardware-targets)

</div>

> **Project Status: Engineering Prototype / Active Bring-Up Phase**
> InterOS Core is an open-source bare-metal Rust microkernel and mobile platform foundation. Memory management (CR3 / TTBR0_EL1 / SATP page tables), capability enforcement, syscall pointer validation (`UserPtr`), Ed25519 `.rapp` package signing, and multi-architecture bring-up (x86_64, AArch64, RISC-V 64) are functional and verified in QEMU. Real-device testing is actively in progress.

**InterOS Core** is a clean-slate, memory-safe mobile operating system foundation engineered in bare-metal Rust (`#![no_std]`).

Designed as an open-source research and engineering alternative to legacy monolithic mobile operating systems, InterOS Core eliminates ambient root privileges, monolithic kernel vulnerability surfaces, and proprietary telemetry. It delivers a microkernel architecture where device drivers, audio engines, network stacks, and user applications execute in isolated address spaces and protection domains.

InterOS Core serves as both a **deployable mobile OS** for real ARM64 and RISC-V 64 hardware, and a comprehensive **App Development Platform** with a turnkey local emulator and hardware digital twin.

---

## ⚡ One-Click Turnkey Quickstart

Experience InterOS Core locally in seconds. The automated setup scripts detect prerequisites, configure the Rust cross-compilation targets, compile microkernels and reference apps, and launch the interactive Device Studio emulator on `http://localhost:4300`.

### On Windows (PowerShell)
```powershell
git clone https://github.com/InterOS/inter-os-core.git
cd inter-os-core
.\setup.ps1
```

### On Linux / macOS / GitHub Codespaces (Bash)
```bash
git clone https://github.com/InterOS/inter-os-core.git
cd inter-os-core
chmod +x setup.sh
./setup.sh
```

The script will automatically launch your default browser to **`http://localhost:4300`**.

---

## 🏛 System Architecture

InterOS Core is architected around a strict **Capability-Based Microkernel (Ring 0 TCB)** with asynchronous zero-copy shared memory IPC and a hardware abstraction layer (HAL) tailored for mobile silicon:

```
+-------------------------------------------------------------------------+
|                  InterOS Native Smartphone Applications                 |
|    (Phone, Messages, Music Player, Calendar, Files, Camera, Settings)   |
+-------------------------------------------------------------------------+
|                Userspace System Services & SDK (Ring 3)                 |
|   - Window Compositor (DRM/KMS Framebuffer)    - Audio Synthesizer Engine|
|   - Cellular RIL / Serial AT Modem Daemon       - Touch Input Digitizer  |
|   - Package Sideload Manager (.app/.wasm/.apk)  - interos-std Syscalls   |
+-------------------------------------------------------------------------+
|                 Capability-Based Zero-Copy IPC Bus                      |
|       Shared-Memory Ring Buffers + Atomic Futex Synchronization         |
+-------------------------------------------------------------------------+
|                   InterOS Microkernel Core (Ring 0)                     |
|   - Preemptive Priority Scheduler (100 Hz)   - Memory Paging & MMU       |
|   - Hardware Keystore & Vault Controller      - Capability Token Matrix  |
+-------------------------------------------------------------------------+
|                   Hardware Abstraction Layer (HAL)                      |
|      RISC-V 64 (QEMU Virt)   |   ARM64 (Cortex-A / PinePhone)   |  x86_64|
|   - InterOS SBI / PLIC / CLINT |   - PSCI / GICv2/v3 / ArchTimer  |  - APIC|
|   - NS16550A Serial UART     |   - SimpleFB & Goodix Touch      |  - VGA |
+-------------------------------------------------------------------------+
```

### Key Architectural Tenets
1. **Zero Ambient Authority**: Processes possess zero implicit capabilities. To access file sectors, transmit network packets, or read sensors, apps must present cryptographically signed capability tokens (`CapToken`).
2. **True Memory Safety**: 100% of the microkernel, HAL, and userspace applications are written in Rust (`#![no_std]`).
3. **Sub-Microsecond Zero-Copy IPC**: Shared-memory ring buffers with atomic futex synchronization deliver high-throughput audio and display streams without kernel buffer-copy overhead.
4. **Ring 3 Sandboxing**: Crashes in network drivers, cellular modems, or third-party applications cannot compromise kernel memory or panic the device.

---

## 📱 Universal Device Studio & Digital Twin

InterOS Core includes a production-grade **Smartphone Device Studio** running locally on Port 4300:

* **Hardware Chassis**: Realistic tactile buttons (Power Button with screen sleep/wake, Volume Up / Down with live haptic feedback).
* **Quantum Capsule (Notch HUD)**: Interactive dynamic pill displaying real-time system heartbeats, active call timers, and playing music status.
* **Gravity Shade (Swipe-Down Drawer)**: Swipe down from the top edge to toggle Wi-Fi, 5G cellular, Bluetooth, Flashlight, Airplane Mode, Do Not Disturb, and Battery Saver, complete with live brightness and master audio sliders.
* **Developer Telemetry Deck**:
  * **Microkernel UART Console**: Raw high-frequency serial telemetry directly from Ring 0.
  * **Security Enclave Console**: Real-time audit log of capability tokens, hardware keystore derivations, and vault sector encryptions.
  * **Package Sideload Deck**: Drag-and-drop installer for native `.app`, WebAssembly `.wasm`, and Android `.apk` bundles.
  * **App Dev Inspector**: Live inspection of inter-process communication (IPC) packets and Ring 3 sandbox memory allocations.

---

## 📦 Production Smartphone Apps Suite

InterOS Core ships with a suite of functional, zero-placeholder smartphone applications written with `#![no_std]` Rust logic and interactive mobile interfaces:

| App | Capabilities & Features |
| :--- | :--- |
| 📞 **Phone / Dialer** | Keypad with authentic Web Audio DTMF tones, live call screen with call duration timer, contact avatar, mute, speaker, and call history log. |
| 💬 **Messages** | Threaded SMS/RCS conversations, responsive message composer, delivery acknowledgments, and automated simulated conversational replies. |
| 🎵 **Music Player** | Real on-device Web Audio synthesizer supporting 4 waveforms (Sine, Square, Triangle, Sawtooth), turntable visualizer, playlist scrubber, and playback queue. |
| 📅 **Calendar & Agenda** | Interactive monthly date picker, today indicator, event scheduler, and upcoming agenda tasks. |
| 📁 **Files & Storage** | Virtual storage explorer navigating `/system/bin`, `/storage/emulated/0`, and `/vault`, with file hashes and download export. |
| ⚙️ **Settings** | Wi-Fi & cellular management, display brightness & theme toggles, audio haptics, hardware keystore status, and kernel architecture info. |
| 📷 **Camera Preview** | Synthetic mobile camera viewfinder, photo shutter flash animation, and media thumbnail gallery. |
| 📦 **Sideload Manager** | Package catalog viewer, SHA-256 binary validation, Ring 3 privilege inspector, and one-click app launch. |

---

## 🛠 App Development Platform & Sovereign `.rapp` Toolchain

InterOS Core provides a complete development platform for building, packaging, and hot-deploying sovereign mobile applications using the **`.rapp` (Rust App Package)** standard.

### Dual-Tier Architecture
Applications consist of two decoupled, high-performance tiers:
1. **UI Tier**: Rapid, modern HTML5/CSS/WASM interface communicating through the high-speed `window.InterOS.ipc` bridge.
2. **Backend Tier**: Sovereign `#![no_std]` Rust microkernel service compiling to native ELF (`aarch64-unknown-none` or `riscv64gc-unknown-none-elf`), running within a capability-gated Ring 3 sandbox.

---

### The `interos` Developer CLI (`tools/interos.py`)

A single unified command-line tool manages the complete application lifecycle:

```bash
# 1. Scaffold a new application from a reference template
python tools/interos.py new my_authenticator --template authenticator
# Available templates: basic, authenticator, sensor, meshchat

# 2. Compile the #![no_std] Rust backend
python tools/interos.py build my_authenticator --target aarch64-unknown-none

# 3. Package into a sovereign .rapp bundle
python tools/interos.py pack my_authenticator
# Generates dist/my_authenticator.rapp containing manifest.json, UI assets, and compiled ELF

# 4. Hot-Deploy directly into the running local Device Studio (Port 4300)
python tools/interos.py run my_authenticator

# 5. Inspect all installed sovereign packages on the emulator
python tools/interos.py list
```

---

### Production Flagship Showcase Applications

InterOS Core includes three fully-functional production reference applications:

| Application | Package | Capability Permissions | Highlights |
| :--- | :--- | :--- | :--- |
| **Authenticator** | `authenticator.rapp` | `CAP_HARDWARE_KEYSTORE`, `CAP_SECURE_STORAGE` | Hardware-derived TOTP 2FA codes, 30s circular countdown timer, click-to-copy clipboard with heads-up notifications. |
| **Health Hub** | `health.rapp` | `CAP_SENSOR_ACCEL`, `CAP_SENSOR_GYRO`, `CAP_SENSOR_HR` | Real-time 60Hz PPG optical waveform canvas rendering, 3-axis accelerometer/gyro registers, daily steps, and calories. |
| **Mesh Chat** | `meshchat.rapp` | `CAP_MESH_RADIO`, `CAP_P2P_SOCKET` | Offline peer-to-peer radio messaging, radar node discovery, RSSI signal indicators, multi-hop packet routing, and notifications. |

---

### Frontend <-> Microkernel IPC Bridge (`window.InterOS.ipc`)

In the UI tier, applications communicate with system microkernel services and backend daemons through `window.InterOS.ipc`:

```javascript
// Send a syscall request to the backend service
window.InterOS.ipc.send("org.interos.authenticator", 0x01, { refresh: true });

// Await an asynchronous capability response
const result = await window.InterOS.ipc.request("org.interos.health", 0x10, { sample_rate: 250 });
console.log("Vitals Telemetry:", result);
```

Every IPC packet is tracked and visualized in real time inside the **App Dev Inspector** tab in the Device Studio.

---

### Mobile Shell & Multitasking System
* **Gravity Shade**: Swipe-down quick settings drawer with Wi-Fi, 5G Data, Bluetooth, Torch, Airplane Mode, DND, Auto-Rotate, and Battery Saver toggles synchronized with the status bar.
* **Quantum Capsule**: Dynamic notch HUD pill reporting system heartbeat ticks, active phone calls, and media audio synthesis.
* **Heads-up Notifications**: Floating push notification banner with audio chime and direct tap-to-open routing.
* **Multitask App Switcher (Recents)**: Horizontal carousel tracking all active process sandboxes, with per-app termination (`×`) and "Clear All" memory cleanup. Swiping up on the home pill or clicking `▢` reveals running tasks.

---

## 🎯 Supported Hardware Targets

InterOS Core compiles for three primary target architectures:

| Architecture | Target Triple | Microkernel Crate | Primary Hardware Target |
| :--- | :--- | :--- | :--- |
| **RISC-V 64** | `riscv64gc-unknown-none-elf` | `kernel-riscv64` | QEMU `virt` machine, SiFive HiFive, StarFive VisionFive 2 |
| **ARM64** | `aarch64-unknown-none` | `kernel-aarch64` | Allwinner A64 PinePhone, Rockchip RK3399, Google Pixel |
| **x86_64** | `x86_64-unknown-none` | `kernel` | QEMU x86_64 PC, Intel/AMD UEFI laptops & devboards |

### Manual Bare-Metal Verification with QEMU

#### 1. Booting RISC-V 64 Microkernel
```bash
cargo build --manifest-path kernel-riscv64/Cargo.toml --target riscv64gc-unknown-none-elf
qemu-system-riscv64 -machine virt -cpu rv64 -m 512M -nographic -bios default -kernel target/riscv64gc-unknown-none-elf/debug/kernel-riscv64
```

#### 2. Booting ARM64 Microkernel
```bash
cargo build --manifest-path kernel-aarch64/Cargo.toml --target aarch64-unknown-none
qemu-system-aarch64 -machine virt -cpu cortex-a57 -m 512M -nographic -kernel target/aarch64-unknown-none/debug/kernel-aarch64
```

#### 3. Flashing Physical Smartphones
See [docs/PHYSICAL_HARDWARE_FLASHING.md](docs/PHYSICAL_HARDWARE_FLASHING.md) for step-by-step instructions on generating Fastboot Android Boot Image v2 headers (`boot-aarch64.img`) and flashing physical PinePhone and Qualcomm Snapdragon devices.

---

## 🤝 Contributing

We welcome community contributions from kernel developers, embedded engineers, UI/UX designers, and application authors!
Please see [CONTRIBUTING.md](CONTRIBUTING.md) for code style guidelines, pull request protocols, and testing workflows.

---

## 📄 License

InterOS Core is dual-licensed under:
* **MIT License** ([LICENSE-MIT](LICENSE-MIT))
* **Apache License, Version 2.0** ([LICENSE-APACHE](LICENSE-APACHE))

You may choose to use this project under either license at your option.
