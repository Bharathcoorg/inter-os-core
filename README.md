# InterOS Core

<div align="center">

![License: MIT / Apache-2.0](https://img.shields.io/badge/License-MIT%20%2F%20Apache--2.0-blue.svg)
![Language: Pure Rust](https://img.shields.io/badge/Language-Pure%20Rust%20%23!%5Bno__std%5D-orange.svg)
![Architectures: x86_64 / AArch64 / RISC-V 64](https://img.shields.io/badge/Architectures-x86__64%20%7C%20AArch64%20%7C%20RISC--V%2064-success.svg)
![Build: Reproducible](https://img.shields.io/badge/Build-Reproducible-blue.svg)
![Verification: QEMU-Gated](https://img.shields.io/badge/Verification-QEMU--Gated-brightgreen.svg)

### **The Sovereign, Open-Source Rust Mobile Operating System Foundation**
*A memory-safe, capability-based microkernel mobile platform for x86_64, AArch64, and RISC-V 64.*

[Quickstart](#-quickstart) • [Architecture](#-system-architecture) • [Reality Contract](#-platform-reality-contract) • [Security & Isolation](#-security-invariants--isolation-model) • [UI & Compositor](#-ui-rendering--compositor) • [Device Studio](#-universal-device-studio) • [Developer CLI](#-developer-cli-interos) • [Experimental Bring-Up](#-hardware-bring-up--experimental-status)

</div>

---

> **Status Notice: Engineering Reality & Verification Status**  
> InterOS Core is an open-source, bare-metal Rust microkernel and mobile operating system foundation. All user processes execute in isolated virtual address spaces with hardware MMU enforcement (CR3 / TTBR0_EL1 / SATP), capability-based access control, safe syscall memory copies (`UserPtr`), and mandatory Ed25519 cryptographic package attestation.  
> The current repository includes runtime verification coverage for the canonical x86_64 QEMU smartphone platform and QEMU virt targets for AArch64 and RISC-V 64. Physical smartphone deployment remains experimental bring-up.

---

## 🏛 Platform Reality Contract

InterOS strictly distinguishes implementation from verification evidence:

- **Implementation States**:
  - `Unavailable`: No backend attached; telemetry and drivers report absence rather than fabricating values.
  - `Emulated`: Virtual platform (QEMU) provides the backend implementation.
  - `Present`: Genuine physical hardware backend is attached and verified.
- **Verification States**:
  - `Source-only`: Code, types, and build graph compile.
  - `Unit-tested`: Function-level logic covered by automated unit tests.
  - `QEMU-verified`: Subsystem exercised and verified inside guest virtual machine.
  - `Hardware-verified`: Verified on physical silicon target with empirical evidence.

> [!IMPORTANT]
> **QEMU verification does not equal physical hardware verification.**  
> The canonical developer platform is a virtual smartphone target in QEMU. Physical smartphone flashing (e.g. PinePhone Allwinner A64) is an experimental research track and is not production-ready.

See [Platform Reality Contract](docs/PLATFORM_REALITY_CONTRACT.md) and run `python tools/interos.py reality` for live state inspection.

---

## 📊 Subsystem Status & Verification Matrix

| Subsystem / Feature | Implementation State | Verification State | Verification Evidence / Mechanism |
| :--- | :--- | :--- | :--- |
| **x86_64 Virtual Address Isolation** | Present | **QEMU-verified** | `tests/verify_x86_boot.py`; PML4 CR3 per-process paging, W^X, Ring 3 isolation |
| **AArch64 Virtual Address Isolation** | Present | **Unit-tested / QEMU virt** | `kernel-aarch64`; TTBR0_EL1 split tables, EL1/EL0 exception levels, PSCI |
| **RISC-V 64 MMU & U-Mode** | Present | **Unit-tested / QEMU virt** | `kernel-riscv64`; Sv39 3-level page tables, SATP Mode 8, U-mode trap ABI |
| **Syscall Pointer Validation** | Present | **Unit-tested** | `kernel/src/syscall/user_ptr.rs`; `UserPtr<T>`, null (<0x1000) exclusion, range checks |
| **Capability Security Model** | Present | **Unit-tested / QEMU-verified** | Per-process capability token table, rights bitmask (`READ`, `WRITE`, `EXECUTE`, `GRANT`) |
| **Shared-Memory IPC** | Present | **Unit-tested / QEMU-verified** | Bounded ring buffers, atomic futex-like synchronization, capability endpoints |
| **Cryptographic `.rapp` Packages** | Present | **Unit-tested / QEMU-verified** | `kernel/src/package/rapp.rs`; Ed25519 signatures, SHA-256 digests, RAPP v2/v3 parser |
| **Process Lifecycle & Multitasking** | Present | **Unit-tested / QEMU-verified** | Ring-3 preemptive scheduler, PID tracking, `WaitPid` & `ExitWithCode` syscalls |
| **DRM/KMS Multi-Surface Compositor** | Present | **QEMU-verified** | Double-buffered scanout, z-order multi-plane composition, 2×2 SSAA downsampling |
| **Native Typography & UI Primitives** | Present | **Unit-tested / QEMU-verified** | Proportional 16×24 anti-aliased font (`test_text_aa`), tabular digits, squircle cards |
| **Native Smartphone Applications** | Present | **QEMU-verified** | 12 first-party apps (Calculator, Dialer, Messages, Settings, Music, Gallery, etc.) |
| **Android Compatibility Boundary (ACR)**| Emulated / Sandboxed | **Source-only / Unit-tested** | `interos-android-shared`; APK v2/v3 signature check, VFS sandbox, Dalvik bytecode auditor |
| **Universal Device Studio** | Emulated | **Unit-tested** | `emulator/server.py`; QEMU manager, live HMP/QMP register queries, serial UART stream |
| **Unified Developer CLI (`interos`)** | Present | **Unit-tested** | `tools/interos.py`; `doctor`, `build`, `test`, `package`, `sign`, `verify`, `install`, `run` |
| **A/B System Updates & Rollback** | Present | **Unit-tested** | `kernel/src/update.rs`, `kernel/src/recovery.rs`; monotonic versions, boot health counter |
| **Physical Hardware Deployment** | Experimental | **Source-only** | `docs/PHYSICAL_HARDWARE_FLASHING.md`; Fastboot v2 `boot-aarch64.img` header format |

---

## ⚡ Quickstart

### Prerequisites
- **Rust Toolchain**: Rust Nightly (with `rust-src` component):
  ```bash
  rustup toolchain install nightly
  rustup default nightly
  rustup component add rust-src
  ```
- **Cross-Compilation Targets**:
  ```bash
  rustup target add x86_64-unknown-none
  rustup target add aarch64-unknown-none
  rustup target add riscv64gc-unknown-none-elf
  ```
- **Emulators**: `qemu-system-x86_64`, `qemu-system-aarch64`, `qemu-system-riscv64`
- **Python**: Python 3.8+ with `cryptography` package (`pip install cryptography`)

### 1. Run Environment Diagnostics
Verify toolchains, targets, and manifest dependencies:
```bash
python tools/interos.py doctor
```

### 2. Build the Canonical QEMU Phone Demo
Build the complete clean-checkout image (compiles userspace ELFs before embedding the x86_64 BIOS disk image):
```bash
python tools/interos.py build demo
```

### 3. Launch in QEMU & Device Studio
Launch the canonical QEMU smartphone backend and inspect serial UART:
```bash
python tools/interos.py run qemu
python tools/interos.py logs -f
```
Device Studio web console is accessible at `http://localhost:4300`.

### 4. Execute Test Suites
```bash
python tools/interos.py test --target workspace     # Run workspace unit tests
python tools/interos.py contracts                  # Run static system contract tests
python tools/interos.py test --target security      # Run adversarial security suite
```

---

## 🏛 System Architecture

InterOS Core implements a strict **Capability-Based Microkernel** architecture where device policy, drivers, window management, and applications execute in userspace:

```text
                    INTEROS CORE
                         │
                         ▼
             MICROKERNEL (Ring 0 / EL1 / S-Mode)
       ┌─────────────────┼─────────────────┐
     Memory             IPC           Capabilities
       │                 │                 │
    Scheduler        Endpoints          Objects
    Interrupts       Ring Buffers       Rights
    PML4 / Sv39      Atomic Futex       Revocation
       │
       ▼
           USERSPACE SERVICES (Ring 3 / EL0 / U-Mode)
       ┌────────┬────────┬────────┬────────┬────────┐
     init    displayd  inputd   storaged  netd   modemd
       │        │        │        │        │        │
       └────────┴────────┴────────┴────────┴────────┘
                         │
                     compositor
                         │
                   Launcher Shell
       ┌─────────────────┼─────────────────┐
    Lock Screen     Home Surface      App Drawer
    Quick Controls  Activity Space    12 First-Party Apps
                         │
                 Applications (.rapp)
```

---

## 🛡 Security Invariants & Isolation Model

1. **Hardware MMU Isolation**:
   - **x86_64**: Per-process PML4 page tables. Context switches reload `CR3`. Kernel space (`U/S = 0`) is inaccessible from user code. User text pages enforce `W^X` (No-Execute `NX = 1` on data/heap/stack).
   - **AArch64**: Split translation tables (`TTBR0_EL1` for user virtual space, `TTBR1_EL1` for kernel). User pages enforce `AP = 0b01` (EL0 Read/Write) and `PXN = 1` (Privileged Execute-Never).
   - **RISC-V 64**: Sv39 3-level paging via `satp` Mode 8. User pages require `PTE_U`. Supervisor mode hardware prevents execution of user mappings.
2. **Safe Syscall Memory Access (`UserPtr<T>`)**:
   - Kernel code eliminates raw pointer dereferencing (`&*ptr`).
   - Copy-in/copy-out semantics via `copy_from_user` and `copy_to_user`.
   - Pointers enforce null guard page exclusion (`< 0x1000`), canonical user address bounds, and integer overflow prevention.
3. **Capability Token Model**:
   - Tasks start with zero ambient authority.
   - Capabilities are kernel-managed table indices guarded by rights bitmasks (`READ`, `WRITE`, `EXECUTE`, `GRANT`) and generation tags.
   - Stale or forged handles return `-EPERM`.
4. **Mandatory Ed25519 Package Attestation**:
   - Applications distributed as `.rapp` bundles require valid Ed25519 signatures and SHA-256 payload digests.
   - Kernel verifies publisher keys against the trust store before allocating address spaces.

---

## 🎨 UI Rendering & Compositor

InterOS Core features a native smartphone UI pipeline engineered for `#![no_std]` environments:

- **Logical vs. Physical Coordinate Space**:
  - **Logical Portrait Resolution**: `720 × 1440` (1:2 aspect ratio) with defined safe area insets (Status bar: 48px, Nav pill: 64px).
  - **Physical QEMU Scanout**: `1280 × 720` display with centered 360×720 viewport and black letterboxing.
  - **Kernel Downsampler**: Upgraded to **2×2 SSAA (Super-Sample Anti-Aliasing) box filtering** in `kernel/src/framebuffer.rs`, replacing point sampling to eliminate decimation artifacts.
- **Typography Engine (`interos-std::design::font`)**:
  - Proportional 16×24 condensed typeface (`FONT`, `FONT_BOLD`).
  - Tabular digits (11px fixed advance) ensuring clock text never jitters horizontally.
  - Subpixel edge anti-aliasing (`test_text_aa`) with analytical corner feathering and early-exit fast paths.
  - Text measurement (`measure`), optical centering (`center_x`), wrapping, and ellipsization (`ellipsize`).
- **Reusable Primitives (`interos-std::design::primitives`)**:
  - Squircle cards (`draw_rounded_rect`), capsules/pills (`draw_pill`), circles (`draw_circle`), and dividers (`draw_divider`).
- **Vector Icons**: Procedural 24×24 rounded vector icon system in `launcher::icons`.

The native UI architecture and layout tokens are implemented in `sdk/rust/interos-std/src/design.rs` and `apps/launcher`.

---

## 🤖 Android Compatibility Boundary

InterOS Core maintains an isolated, bounded subsystem for Android compatibility:

- **Isolated Shared Library**: `#![no_std]` `interos-android-shared` crate.
- **Sandbox Filesystem Bounds**: Paths normalized and restricted strictly to `/data/data/<pkg>` and `/sdcard/Android/data/<pkg>`; path traversals rejected.
- **Signature Parsing**: APK Signature Scheme v2/v3 block inspection (`APK Sig Block 42`) with fallback JAR manifest verification.
- **Dalvik Bytecode Auditor**: Header checksum verification and instruction boundary validation.
- **Boundary Reality**: The Android compatibility subsystem is an isolated compatibility layer, **not** a claim of full Android application ecosystem parity. Native `.rapp` applications remain the primary platform target.

---

## 📱 Universal Device Studio

Device Studio (`http://localhost:4300`) provides a local development twin and emulator control plane:

- **Real QEMU Process Management**: Launch, pause, reset, or stop QEMU backends (`x86_64`, `aarch64`, `riscv64`).
- **Honest Hardware Telemetry**: Live registers (`CR0`, `CR3`, `RIP`, `EAX`) queried directly from the QEMU monitor. When QEMU is offline, registers reset to dashes (`—`) and RIP reports `Offline`.
- **Microkernel Serial UART**: Streaming logs directly from Ring 0 / EL1 / S-Mode.
- **Package Sideloading**: Direct deployer for signed `.rapp` packages with cryptographic validation.

---

## 🛠 Developer CLI (`interos`)

The developer CLI (`tools/interos.py`, with root wrappers `interos.ps1` and `interos.cmd`) exposes complete workflow automation:

```bash
# Environment & Diagnostic Verification
python tools/interos.py doctor
python tools/interos.py reality

# Build Targets
python tools/interos.py build demo              # Canonical QEMU demo
python tools/interos.py build --target x86_64   # Build x86_64 microkernel
python tools/interos.py build --target aarch64  # Build AArch64 microkernel
python tools/interos.py build --target riscv64  # Build RISC-V 64 microkernel

# Testing & Contracts
python tools/interos.py test --target workspace
python tools/interos.py contracts
python tools/interos.py test --target security

# Package Management (.rapp)
python tools/interos.py new my_app --template authenticator
python tools/interos.py package my_app
python tools/interos.py sign dist/my_app.rapp --key dev.key
python tools/interos.py verify dist/my_app.rapp
python tools/interos.py inspect dist/my_app.rapp

# Runtime & QEMU Operations
python tools/interos.py run qemu
python tools/interos.py logs -f
python tools/interos.py trust dist/my_app.rapp
python tools/interos.py install dist/my_app.rapp
python tools/interos.py list
python tools/interos.py run my_app
python tools/interos.py stop
```

---

## 🎯 Hardware Bring-Up & Experimental Status

| Architecture | Microkernel Crate | Target Triple | Target Platform | Status |
| :--- | :--- | :--- | :--- | :--- |
| **x86_64** | `kernel` | `x86_64-unknown-none` | QEMU PC (`q35`), BIOS | Canonical QEMU development platform; fully verified |
| **AArch64** | `kernel-aarch64` | `aarch64-unknown-none` | QEMU `virt` (Cortex-A53), PinePhone | Source-level target; experimental bring-up |
| **RISC-V 64** | `kernel-riscv64` | `riscv64gc-unknown-none-elf` | QEMU `virt` (RV64), VisionFive 2 | Source-level target; experimental bring-up |

For experimental hardware bring-up documentation, see [docs/PHYSICAL_HARDWARE_FLASHING.md](docs/PHYSICAL_HARDWARE_FLASHING.md).

### Reliability & Platform Specifications
- [Platform Reality Contract](docs/PLATFORM_REALITY_CONTRACT.md) — canonical implementation vs. verification terminology.
- [System Architecture](docs/ARCHITECTURE.md) — deep-dive into microkernel isolation, IPC, and security.
- [QEMU Phone Platform](docs/QEMU_PHONE_PLATFORM.md) — virtual hardware and transport specification.
- [RAPP Package Format](docs/RAPP_FORMAT.md) — deterministic binary package specification.
- [Syscall Pointer Safety](docs/SYSCALL_POINTER_SAFETY.md) — memory trust boundary invariants.
- [System Services](docs/SYSTEM_SERVICES.md) — Ring-3 isolated service lifecycle and contracts.
- [Recovery and Secure Updates](docs/RECOVERY_AND_UPDATES.md) — authenticated A/B transactions and rollback.
- [Diagnostics and Performance](docs/DIAGNOSTICS_AND_PERFORMANCE.md) — bounded diagnostics and telemetry.
- [Developer Guide](docs/DEVELOPER_GUIDE.md) — comprehensive CLI and workflow manual.

---

## 📄 License

InterOS Core is dual-licensed under:
* **MIT License** ([LICENSE-MIT](LICENSE-MIT))
* **Apache License, Version 2.0** ([LICENSE-APACHE](LICENSE-APACHE))
