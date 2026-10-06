# Contributing to InterOS Core

Thank you for your interest in contributing to **InterOS Core**! InterOS Core is an open-source, community-driven sovereign mobile operating system foundation.

---

## 🧭 Core Principles

1. **Memory Safety First**: All microkernel and userspace components must be written in bare-metal Rust (`#![no_std]`). The use of `unsafe` is restricted strictly to hardware MMIO access, page table operations, and low-level context switches.
2. **Zero Ambient Authority**: Processes start with empty capability tables. Ambient root or supervisor privileges are strictly prohibited.
3. **Multi-Architecture Parity**: Interfaces and HAL abstractions should maintain design parity across x86_64, AArch64, and RISC-V 64 targets.
4. **Reproducible Builds**: All components must compile deterministically and be verifiable in the canonical QEMU environment.
5. **Truthful Reality Model**: Never represent emulated or demo telemetry as physical hardware evidence. Follow the [Platform Reality Contract](docs/PLATFORM_REALITY_CONTRACT.md).

---

## 🛠 Development Environment Setup

### Prerequisites

- **Rust Toolchain**: Rust Nightly with the `rust-src` component:
  ```bash
  rustup toolchain install nightly
  rustup default nightly
  rustup component add rust-src
  ```
- **Compilation Target Triples**:
  ```bash
  rustup target add x86_64-unknown-none
  rustup target add aarch64-unknown-none
  rustup target add riscv64gc-unknown-none-elf
  ```
- **QEMU Emulators**:
  - `qemu-system-x86_64` (canonical development target)
  - `qemu-system-aarch64`
  - `qemu-system-riscv64`
- **Python**: Python 3.8+ with `cryptography` package (`pip install cryptography`).

### Diagnostic Environment Check
Verify your local setup using the unified developer CLI:
```bash
python tools/interos.py doctor
```

---

## 🌿 Branch Policy

- **`master` is the sole canonical development branch.**
- All pull requests, feature branches, and agent workflows must branch from and target `master`.
- Do not create milestone branches (e.g. `M1`, `M2`, etc.) in the canonical repository.
- Do not squash, reset, or rewrite canonical Git history.
- Ensure all commits adhere to Conventional Commits:
  - `feat(scope): ...`
  - `fix(scope): ...`
  - `test(scope): ...`
  - `docs: ...`

---

## 🧪 Testing & Validation Workflow

Before submitting a Pull Request, verify all changes locally:

1. **Workspace Compilation**:
   ```bash
   cargo check --workspace
   ```
2. **Unit Tests**:
   ```bash
   cargo test --workspace
   ```
3. **System Contract Suite**:
   ```bash
   python tools/interos.py contracts
   ```
4. **Full Contracts Suite**:
   ```bash
   python tests/run_all_contracts.py
   ```
5. **Canonical QEMU Boot Verification**:
   ```bash
   python tools/interos.py build demo
   python tests/verify_x86_boot.py
   ```

---

## 🔒 Security Reporting

Security is central to InterOS Core. If you discover a vulnerability, capability escalation, or memory safety violation:
- Please do **not** open a public issue.
- Report security vulnerabilities privately through GitHub Private Vulnerability Reporting or contact the repository maintainers directly.
