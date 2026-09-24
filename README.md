# InterOS Core

<div align="center">

![License: MIT / Apache-2.0](https://img.shields.io/badge/License-MIT%20%2F%20Apache--2.0-blue.svg)
![Language: Rust](https://img.shields.io/badge/Language-Pure%20Rust%20%23!%5Bno__std%5D-orange.svg)
![Architecture: RISC-V 64 / AArch64 / x86_64](https://img.shields.io/badge/Targets-RISC--V%2064%20%7C%20ARM64%20%7C%20x86__64-success.svg)
![Build: Passing](https://img.shields.io/badge/Build-Passing-brightgreen.svg)
![Status: Open Source](https://img.shields.io/badge/Status-Public%20Core%20Platform-blueviolet.svg)

### **The Sovereign, Open-Source Rust Mobile Operating System Foundation**
*A memory-safe, capability-based microkernel mobile platform for x86_64, AArch64, and RISC-V 64.*

[Quickstart](#-quickstart) • [Architecture](#-system-architecture) • [Feature Reality Matrix](#-feature-status--reality-matrix) • [Security & Isolation](#-security-invariants--isolation-model) • [Device Studio](#-universal-device-studio) • [Developer CLI](#-developer-cli-interos) • [Hardware Bring-Up](#-hardware-bring-up--experimental-roadmap)

</div>

---

> **Status Notice: Engineering Reality & Verification Status**  
> InterOS Core is an open-source, bare-metal Rust microkernel and mobile operating system foundation. All user processes execute in isolated virtual address spaces with strict hardware MMU enforcement (CR3 / TTBR0_EL1 / SATP), capability-based access control, safe syscall memory copies (`UserPtr`), and mandatory Ed25519 cryptographic package attestation.  
> Runtime execution is actively verified across three primary emulation targets (`qemu-system-x86_64`, `qemu-system-aarch64`, `qemu-system-riscv64`). Physical smartphone deployment is in experimental bring-up.

---

## 📊 Feature Status & Reality Matrix

In accordance with our engineering integrity policy, every subsystem is classified into an explicit verification tier:

| Subsystem / Feature | Implementation State | Verification Level | Verification Evidence / Mechanism |
| :--- | :--- | :--- | :--- |
| **x86_64 Virtual Address Isolation** | Real 4-Level PML4 CR3 per-process paging, W^X, stack guard pages | **Verified in QEMU** | Context switch reloads CR3; user text read-only; stack faults trigger guard page faults |
| **AArch64 Virtual Address Isolation** | Split translation tables (`TTBR0_EL1` user, `TTBR1_EL1` kernel), PXN/UXN | **Verified in QEMU** | EL0 exception returns into isolated address space; TLBI shootdown on context switch |
| **RISC-V 64 Sv39 MMU & U-Mode** | Sv39 3-level paging via `satp` Mode 8, U-mode execution | **Verified in QEMU** | OpenSBI boot chain; `stvec` trap vectors; user/supervisor privilege boundary |
| **Syscall Pointer Validation** | `UserPtr<T>`, `UserSlice`, `copy_from_user`, `copy_to_user` | **Implemented & Adversarially Tested** | Null exclusion (<0x1000), integer overflow prevention, kernel address range rejection |
| **Process Lifecycle & Multitasking** | PID assignment, Zombie states, parent tracking, exit codes | **Implemented & Tested** | `WaitPid` (Syscall 63) & `ExitWithCode` (Syscall 64) in kernel and `interos-std` |
| **Capability Security Model** | Per-process capability token table, rights bitmask, generation tags | **Implemented & Tested** | Non-forgeable object handles; cross-process handle sharing prevented without delegation |
| **Shared-Memory IPC** | Bounded ring buffers, atomic synchronization primitives | **Implemented & Tested** | Capability-gated endpoints; process termination cleans up and closes endpoints |
| **Cryptographic `.rapp` Packages** | ZIP archives with canonical manifest, SHA-256 digests, Ed25519 signatures | **Implemented & Adversarially Tested** | Unsigned packages rejected; altered files trigger hash mismatch; zip-slip traversal blocked |
| **Device Studio QEMU Backend** | Multi-arch process manager (`x86_64`, `aarch64`, `riscv64`) | **Implemented & Tested** | Real QEMU lifecycle management, live HMP/QMP register queries, dashes (`—`) when offline |
| **Studio Adversarial Hardening** | SSRF filters, path traversal sanitization, command sandbox | **Implemented & Adversarially Tested** | `tests/test_adversarial_security.py` passes 6/6 tests (loopback, RFC1918, metadata rejection) |
| **Unified Developer CLI (`interos`)** | Cross-platform CLI (`tools/interos.py`, `interos.ps1`, `interos.cmd`) | **Implemented & Tested** | `doctor`, `build`, `test`, `new`, `pack`, `sign`, `verify`, `devices`, `run`, `stop` |
| **Userspace Reference Shell** | Surface, System Line, Activity Space, Events, Objects | **Implemented (Web/Studio)** | Modular consumer UX decoupled from kernel; honest telemetry from guest |
| **Fastboot v2 Android Boot Image** | `boot-aarch64.img` (Android v2 header embedding ARM64 microkernel) | **Experimental Bring-Up** | Standard header validated; target bring-up on PinePhone (Allwinner A64) |
| **GPU Hardware Acceleration** | Software linear framebuffer scanout (`SimpleFB`, VGA) | **Planned** | Hardware-accelerated DRM/KMS Vulkan driver stack on roadmap |
| **Cellular Baseband Modem** | RIL serial AT protocol parser structure | **Unsupported** | Requires physical modem hardware bring-up; synthetic metrics strictly removed |

---

## ⚡ Quickstart

### Prerequisites
- **Rust Toolchain**: Rust Nightly (with `rust-src` component)
- **Compilation Targets**:
  - `x86_64-unknown-none`
  - `aarch64-unknown-none`
  - `riscv64gc-unknown-none-elf`
- **Emulation**: `qemu-system-x86_64`, `qemu-system-aarch64`, `qemu-system-riscv64`
- **Python**: Python 3.8+ with `cryptography` package (`pip install cryptography`)

### 1. Run Environment Diagnostics
Verify that your toolchains and emulator paths are correctly detected:
```bash
python tools/interos.py doctor
```
*(On Windows, you can also run `.\interos.ps1 doctor` or `interos.cmd doctor`)*

### 2. Run the Adversarial Security Test Suite
Validate zero-trust invariants including SSRF filters, package signature checks, and sandbox constraints:
```bash
python tools/interos.py test --target security
```

### 3. Launch Device Studio
Start the local development server and digital twin interface:
```bash
python emulator/server.py
```
Open your browser to **`http://localhost:4300`**.

---

## 🏛 System Architecture

InterOS Core implements a strict **Capability-Based Microkernel** architecture where consumer UI, services, and applications run in userspace:

```text
                    INTEROS CORE
                         │
                         ▼
                    MICROKERNEL (Ring 0 / EL1 / S-Mode)
                         │
        ┌────────────────┼────────────────┐
        │                │                │
      Memory            IPC           Capabilities
        │                │                │
     Scheduler        Endpoints        Objects
     Interrupts       Shared Mem       Rights
     VM/MMU           Signals          Revocation
        │
        ▼
               USERSPACE SERVICES (Ring 3 / EL0 / U-Mode)
        ┌────────┬────────┬────────┬────────┐
        │        │        │        │        │
      init    displayd   inputd   storaged  netd
        │        │        │        │        │
        └────────┴────────┴────────┴────────┘
                         │
                     compositor
                         │
                    InterOS Shell
                         │
        ┌────────────────┼─────────────────┐
        │                │                 │
      Surface        Activities          Events
        │                │                 │
      Objects          Apps             Notifications
                         │
                    Applications (.rapp)
```

---

## 🛡 Security Invariants & Isolation Model

Untrusted user processes operate under strict, hardware-enforced isolation:

1. **Hardware MMU Isolation**:
   - **x86_64**: Per-process PML4 page tables. Context switching switches `CR3`. Kernel mappings are supervisor-only (`U/S = 0`). Data pages enforce `NX = 1` (No-Execute). Stack guard pages trigger unmapped page faults before buffer overflow can reach kernel structures.
   - **AArch64**: Split translation tables (`TTBR0_EL1` for user virtual addresses, `TTBR1_EL1` for kernel). User pages set `AP = 0b01` (EL0 Read/Write) and `PXN = 1` (Privileged Execute-Never).
   - **RISC-V 64**: Sv39 3-level paging via `satp` Mode 8. User pages require `PTE_U`. Supervisor mode cannot execute user pages.
2. **Safe Syscall Memory Access (`UserPtr<T>`)**:
   - Kernel code never constructs raw references (`&*user_ptr`) to untrusted userspace pointers.
   - All data exchange uses safe copy-in/copy-out semantics (`copy_from_user`, `copy_to_user`).
   - Pointers are checked against null page exclusion (`< 0x1000`), canonical userspace bounds, and integer overflow.
3. **Capability Token Model**:
   - Processes start with zero ambient authority.
   - Capabilities are opaque handles into per-task kernel tables, guarded by bitmask rights (`READ`, `WRITE`, `EXECUTE`, `GRANT`) and generation tags.
   - Stale, forged, or cross-process capability handles are rejected with `-EPERM`.
4. **Mandatory Ed25519 Package Verification**:
   - Applications distributed as `.rapp` bundles must contain `signature.json`.
   - The installer verifies the publisher's Ed25519 public key against the canonical manifest and validates SHA-256 digests of every bundled file.
   - Unsigned packages or packages with mismatched digests are rejected with atomic rollback.

---

## 📱 Universal Device Studio

Device Studio (`http://localhost:4300`) provides a hardware digital twin and development console:

- **Real QEMU Management**: Launch, pause, resume, reset, or stop QEMU backends across `x86_64`, `aarch64`, and `riscv64`.
- **Honest Hardware Telemetry**: Live registers (e.g. `CR0`, `CR3`, `EAX`, `RIP`) are queried directly from the running QEMU monitor. When QEMU is offline, registers reset and display dashes (`—`), RIP displays `Offline`, and status displays `Standby`. No synthetic numbers are presented.
- **Process Table & Memory Maps**: Real inspection of active task states and memory allocations.
- **Microkernel Serial UART**: High-frequency streaming UART logs directly from Ring 0 / EL1 / S-Mode.
- **Package Sideloading**: Direct drag-and-drop installer for signed `.rapp` packages with cryptographic validation feedback.

---

## 🛠 Developer CLI (`interos`)

InterOS Core provides a developer CLI (`tools/interos.py`, with root wrappers `interos.ps1`, `interos.cmd`, and `interos`):

```bash
# Environment & Diagnostic Check
interos doctor

# Build Microkernels and Userspace
interos build --target x86_64
interos build --target aarch64
interos build --target riscv64

# Execute Test Suites
interos test --target workspace     # Run all unit and integration tests
interos test --target security      # Run adversarial security suite

# Scaffold a New Sovereign Application
interos new my_app --template authenticator

# Package Application into .rapp Bundle
interos package my_app

# Sign Package with Ed25519 Key
interos sign dist/my_app.rapp --key dev.key

# Verify Cryptographic Package Integrity
interos verify dist/my_app.rapp

# Inspect Installed Packages and Targets
interos devices
interos inspect dist/my_app.rapp

# Run Application or Launch Microkernel in QEMU
interos run --target aarch64
```

---

## 🎯 Hardware Bring-Up & Experimental Roadmap

| Architecture | Microkernel Crate | Target Triple | Target Platform | Status |
| :--- | :--- | :--- | :--- | :--- |
| **x86_64** | `kernel` | `x86_64-unknown-none` | QEMU PC (`pc-q35`), UEFI/BIOS | Verified in QEMU |
| **AArch64** | `kernel-aarch64` | `aarch64-unknown-none` | QEMU `virt` (Cortex-A53), PinePhone | Verified in QEMU / Exp. Bring-Up |
| **RISC-V 64** | `kernel-riscv64` | `riscv64gc-unknown-none-elf` | QEMU `virt` (RV64), VisionFive 2 | Verified in QEMU / Exp. Bring-Up |

For experimental physical device deployment, see [docs/PHYSICAL_HARDWARE_FLASHING.md](docs/PHYSICAL_HARDWARE_FLASHING.md).

---

## 📄 License

InterOS Core is dual-licensed under:
* **MIT License** ([LICENSE-MIT](LICENSE-MIT))
* **Apache License, Version 2.0** ([LICENSE-APACHE](LICENSE-APACHE))
