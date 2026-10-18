# InterOS Core

<div align="center">

[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPLv3-blue.svg)](LICENSE)
![Language: Pure Rust](https://img.shields.io/badge/Language-Pure%20Rust%20%23!%5Bno__std%5D-orange.svg)
![Architectures: x86_64 / AArch64 / RISC-V 64](https://img.shields.io/badge/Architectures-x86__64%20%7C%20AArch64%20%7C%20RISC--V%2064-success.svg)
![Build: Reproducible](https://img.shields.io/badge/Build-Reproducible-blue.svg)
![Verification: Local Audit Required](https://img.shields.io/badge/Verification-Local%20Audit%20Required-yellow.svg)

### **The Sovereign, Open-Source Rust Mobile Operating System Foundation**
*A memory-safe, capability-based microkernel mobile platform for x86_64, AArch64, and RISC-V 64.*

[Quickstart](#-quickstart) • [Architecture](#-system-architecture) • [Reality Contract](#-platform-reality-contract) • [Security & Isolation](#-security-invariants--isolation-model) • [UI & Compositor](#-ui-rendering--compositor) • [Device Studio](#-universal-device-studio) • [Developer CLI](#-developer-cli-interos) • [Experimental Bring-Up](#-hardware-bring-up--experimental-status)

</div>

---

> **Status Notice: Engineering Reality & Verification Status**  
> InterOS Core is an open-source, bare-metal Rust microkernel and mobile operating system foundation. The intended platform enforces per-process virtual address spaces (CR3 / TTBR0_EL1 / SATP), capability-based access control, and safe syscall memory copies (`UserPtr`). Native `.rapp` packages have an Ed25519 attestation path; this does not mean Android APK signatures are verified.  
> The repository includes automated source-contract and subsystem tests for the canonical x86_64 QEMU smartphone target and QEMU virt bring-up targets for AArch64 and RISC-V 64. The latest Device Studio/display/input lifecycle changes still require a fresh local QEMU run before they can be described as runtime-verified. Physical smartphone deployment remains experimental bring-up.

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
| **Native Smartphone Applications** | Emulated / in development | **Unit-tested / partial QEMU coverage** | 12 first-party app targets; end-to-end usability and lifecycle acceptance still require the local Device Studio audit |
| **Android Compatibility Boundary (ACR)** | Experimental / Sandboxed | **Source-only / Unit-tested** | Fail-closed APK signature boundary, APK structure/manifest parsing, bounded DEX opcode subset, sandbox VFS and framework bridge; full Android app launch is not supported yet |
| **Universal Device Studio** | Emulated | **Source-only / Unit-tested** | `emulator/server.py`; QEMU manager, VNC scanout, serial input bridge and live HMP/QMP queries; latest end-to-end guest interaction requires local QEMU verification |
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
- **Focus Mode (Default)**: Clean, immersive mobile testing with instant 1:1 touch response.
- **Developer Telemetry Deck**: Press **`D`** or click **`Deck`** in the header to inspect real QEMU CPU registers, block I/O, IPC pipelines, and packages. Press **`F`** or **`Esc`** to return to Focus Mode.
- See [DEVELOPERS.md](DEVELOPERS.md) for full developer instructions.

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
  - **QEMU Display Request**: The canonical launchers request a `720 × 1440` VirtIO-VGA mode and expose an absolute USB tablet. The guest's actual negotiated scanout and direct touch path must still be confirmed by the local QEMU audit; a `1280 × 720` fallback path remains supported.
  - **Framebuffer Presentation**: The kernel uses a software KMS compositor and framebuffer presentation adapter. Anti-aliasing and native portrait mode are implementation goals; final sharpness, frame pacing, and negotiated QEMU resolution require runtime measurement.
- **Typography Engine (`interos-std::design::font`)**:
  - Proportional 16×24 condensed typeface (`FONT`, `FONT_BOLD`).
  - Tabular digits (11px fixed advance) ensuring clock text never jitters horizontally.
  - 4-bit grayscale coverage anti-aliasing (`test_text_aa`) with analytical corner feathering and early-exit fast paths.
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
- **Signature Verification Boundary**: The current kernel parser deliberately rejects APKs because it does not yet cryptographically verify APK Signature Scheme v2/v3 or JAR/X.509 signatures. Signature metadata is not treated as proof of authenticity; signed APK installation remains disabled until a trusted verifier and signer trust policy are implemented.
- **Bounded DEX Runtime**: A small opcode subset is interpreted with register, instruction-step, and call-depth limits. This is not ART and does not yet run arbitrary Android applications; unsupported opcodes/framework APIs fail closed.
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
| **x86_64** | `kernel` | `x86_64-unknown-none` | QEMU PC (`q35`), BIOS | Primary emulation contract; runtime validation pending |
| **AArch64** | `kernel-aarch64` | `aarch64-unknown-none` | QEMU `virt` (Cortex-A53), PinePhone | Source-level architecture target; runtime validation pending |
| **RISC-V 64** | `kernel-riscv64` | `riscv64gc-unknown-none-elf` | QEMU `virt` (RV64), VisionFive 2 | Source-level architecture target; runtime validation pending |

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

InterOS Core is free and open-source software licensed under the **GNU Affero General Public License v3.0 (AGPLv3)**. See [LICENSE](LICENSE) for the full license text.

### 💼 Commercial & OEM Dual-Licensing
For hardware manufacturers, enterprise smartphone deployments, or defense contractors requiring proprietary closed-source driver integration, custom board support packages (BSPs), or compliance terms exempt from AGPLv3 copyleft requirements, commercial licensing is available. Contact the maintainers for commercial terms.
