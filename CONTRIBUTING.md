# Contributing to InterOS Core

Thank you for your interest in contributing to **InterOS Core**! InterOS Core is an open-source, community-driven sovereign mobile operating system and developer platform.

---

## 🧭 Code of Conduct & Principles

1. **Memory Safety First**: All microkernel and userspace components must be written in safe Rust (`#![no_std]`), utilizing `unsafe` only where strictly required for bare-metal hardware registers and MMU paging.
2. **Zero Ambient Authority**: Every system service must operate via capability tokens. Ambient root privileges are strictly prohibited.
3. **Multi-Architecture Parity**: Changes to the HAL or syscall interfaces should maintain parity across RISC-V 64, AArch64, and x86_64 targets.
4. **Reproducible Builds**: All builds should be deterministic and testable under QEMU.

---

## 🛠 Setting Up Your Development Environment

### Prerequisites
- **Rust Toolchain**: `rustup default nightly` (or stable 1.80+)
- **Cross-Compilation Targets**:
  ```bash
  rustup target add riscv64gc-unknown-none-elf
  rustup target add aarch64-unknown-none
  ```
- **QEMU Emulators**:
  - `qemu-system-riscv64`
  - `qemu-system-aarch64`
  - `qemu-system-x86_64`
- **Python 3.10+** (for Device Studio emulator server)

### 1-Click Launch
Run `setup.ps1` (Windows) or `setup.sh` (Linux/macOS) to compile and launch the local Device Studio on `http://localhost:4300`.

---

## 📝 Contribution Areas

We actively welcome contributions in the following domains:
- **Microkernel Core**: Memory allocators, preemptive scheduler optimizations, page table management.
- **Hardware Abstraction Layer (HAL)**: Drivers for touch digitizers (Goodix, FocalTech), power management ICs (PMIC), and cellular modems (Quectel, SIMCom).
- **Userspace SDK (`interos-std`)**: Additional POSIX-lite syscalls, async event loops, graphics and font rendering primitives.
- **Native Applications**: New sovereign mobile apps (notes, calculator, media tools).
- **Device Studio**: Emulator UX enhancements, developer tools, and sensor mock pipelines.
- **Documentation**: Tutorials, architecture deep-dives, hardware bring-up guides.

---

## 🧪 Testing Guidelines

Before submitting a Pull Request, ensure:
1. `cargo check --target riscv64gc-unknown-none-elf` passes with zero errors.
2. `cargo check --target aarch64-unknown-none` passes with zero errors.
3. The Device Studio launches and serves all endpoints on port 4300.
4. App interactions and IPC messages stream cleanly without panic.

---

## 🔒 Security Vulnerability Reporting

Security is paramount in InterOS Core. If you discover a security vulnerability or capability leak, please do not open a public issue. Email security reports to `security@interos.org` or contact the core maintainers privately.
