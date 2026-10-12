# InterOS QEMU Phone Platform Contract

This document defines the canonical x86_64 developer virtual smartphone platform for InterOS Core.

---

## 1. Purpose & Scope

The QEMU Phone profile is a reproducible virtual development target. It provides a full-featured microkernel and userspace environment for application and system development. It is **not** a claim of physical smartphone hardware equivalence.

---

## 2. Canonical Virtual Machine Specification

| Parameter | Specification | Purpose / Notes |
| :--- | :--- | :--- |
| **Architecture** | x86_64 | Canonical developer CPU target |
| **Rust Target** | `x86_64-unknown-none` | `#![no_std]` bare-metal compilation |
| **QEMU Machine** | `pc-q35` | Modern PC chipset with PCI Express |
| **CPU Model** | `qemu64` | Standard x86_64 CPU features |
| **RAM** | 1024 MiB | System memory allocation |
| **Boot Mode** | BIOS disk image | MBR partition table with embedded kernel |
| **Disk Image** | `target/inter-os-bios.img` | Canonical output of `interos build demo` |
| **Serial TCP (COM1)** | `127.0.0.1:4301` | Microkernel serial UART stream |
| **HMP Monitor** | `127.0.0.1:44445` | QEMU monitor for register/state inspection |
| **VNC Endpoint** | `127.0.0.1:5901` (`:1`) | RFB framebuffer and input channel |
| **WebSocket Bridge** | `127.0.0.1:6081` | noVNC WebSocket bridge for browser access |
| **Device Studio** | `http://127.0.0.1:4300` | Developer control console |
| **Scanout Contract** | `720 × 1440` ARGB8888 | Physical display framebuffer |
| **UI Logical Space** | `720 × 1440` (1:2 aspect) | Mobile portrait layout space |
| **Display Device** | `virtio-vga` with an EDID that pins `xres=720, yres=1440` | The guest KMS modesets exactly this |

> [!NOTE]
> The scanout and the logical space are the same 720×1440. This document used to
> describe a 1280×720 landscape scanout downsampled by 2×2 box filtering into a
> centred 360×720 viewport. That path no longer exists: the kernel now engages
> native 720×1440×32bpp through the Bochs/VBE DISPI registers that `virtio-vga`
> exposes (`kernel/src/framebuffer.rs`, `engage_native_mode`), and the launch
> scripts pin the geometry with `-device virtio-vga,edid=on,xres=720,yres=1440`.
> A machine-checked contract asserts it (`tests/test_qemu_phone_profile_contract.py`),
> and five further contract tests assert the same 720×1440 geometry, so the
> numbers above cannot drift from the launch line without a suite going red.

---

## 3. Canonical Developer Workflow

1. `python tools/interos.py doctor`
2. `python tools/interos.py build demo`
3. `python tools/interos.py run qemu`
4. Access Device Studio at `http://localhost:4300`.
5. Follow serial UART logs: `python tools/interos.py logs -f`.

> [!NOTE]
> `interos build demo` is the authoritative clean-checkout build command. It resolves and compiles all userspace application ELFs specified in `platform/x86_64/userspace-manifest.json` before assembling the bootable disk image.

---

## 4. Input & Display Coordinates

The canonical QEMU input and display coordinate mapping is defined as:

- **Display Scanout**: 720×1440 pixels, origin `(0, 0)` at top-left.
- **Physical Touch Coordinates**: Unsigned display pixels (`X: 0..=719`, `Y: 0..=1439`).
- **Touch Transport**:
  ```text
  INPUT_TOUCH <DOWN|MOVE|UP|CANCEL> <x> <y> <slot>
  ```
- **Keyboard Transport**:
  ```text
  INPUT_KEY <Unicode scalar>
  ```
- All input events outside the virtual display boundaries are rejected. The kernel input subsystem normalizes touch events into multi-contact gesture queues (`kernel/src/drivers/touch_gestures.rs`).

---

## 5. Guest Package Lifecycle

The QEMU developer transport supports live application deployment over the COM1 serial interface:

1. **Trust Provisioning**: `interos trust <package.rapp>` provisions the package's Ed25519 public key into the guest trust registry.
2. **Installation**: `interos install <package.rapp>` validates the package on the host, streams the signed bytes over COM1, verifies the SHA-256 digest and Ed25519 signature inside Ring 0, and stores the verified blob in persistent storage (up to 60 KiB per package).
3. **Execution**: `interos run <package_id>` instructs the guest launcher to spawn a Ring-3 process with dedicated page tables.
4. **Uninstallation**: `interos uninstall <package_id>` removes the package blob and deactivates its registry entry.

---

## 6. Verification Terminology

In accordance with the [Platform Reality Contract](PLATFORM_REALITY_CONTRACT.md):
- **Built**: Artifact was produced successfully by the build graph.
- **QEMU-Verified**: Subsystem was booted and confirmed working inside the guest virtual machine.
- **Hardware-Verified**: Reserved exclusively for physical hardware with verified peripheral drivers. Physical touchscreen or cellular operation is **not** implied by QEMU verification.
