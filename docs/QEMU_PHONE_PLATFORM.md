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
| **VNC Endpoint** | `127.0.0.1:5901` (`:1`, default) | RFB framebuffer and input channel; the display is configurable with `INTEROS_VNC_DISPLAY`, and a launch whose display is already taken moves to a free one and reports the display it used |
| **WebSocket Bridge** | `127.0.0.1:6081` | noVNC WebSocket bridge for browser access |
| **Device Studio** | `http://127.0.0.1:4300` | Developer control console |
| **Scanout Contract** | `1080 × 2160` FHD+ (720 × 1440 logical, 3:2 bilinear hardware upscaled) | Physical display scanout plane |
| **UI Logical Space** | `720 × 1440` (1:2 aspect) | Mobile portrait layout space |
| **Display Device** | `virtio-vga` / stdvga with 1080×2160 native scanout | The guest KMS modesets the physical scanout |

> [!NOTE]
> InterOS operates on a physical `1080 × 2160` FHD+ scanout buffer with a `720 × 1440`
> logical authoring space, utilizing hardware-accelerated 3:2 bilinear upscaling
> within the DRM/KMS pipeline (`kernel/src/drivers/display_kms.rs`). Applications
> render `#![no_std]` UI into the 720×1440 authoring coordinate space, which the
> compositor upsamples to retina Full HD+ during hardware presentation flips.
> Physical and logical display parameters are pinned in `platform/x86_64/qemu-phone.json`
> and verified by machine-checked contract tests (`tests/test_qemu_phone_profile_contract.py`).

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

- **Display Scanout**: 1080×2160 pixels (physical FHD+ scanout), 720×1440 logical layout space, origin `(0, 0)` at top-left.
- **Physical Touch Coordinates**: Logical display pixels (`X: 0..=719`, `Y: 0..=1439`).
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

1. **Trust Provisioning**: Publisher trust roots are anchored in guest firmware / persistent keystore. Unauthenticated COM1 serial commands cannot inject arbitrary publisher keys (`RAPP_TRUST` is rejected by design).
2. **Installation**: `interos install <package.rapp>` validates the package on the host, streams the signed bytes over COM1, verifies the SHA-256 digest and Ed25519 signature inside Ring 0 against the trust store, and stores the verified blob in persistent storage (up to 60 KiB per package).
3. **Execution**: `interos run <package_id>` instructs the guest launcher to spawn a Ring-3 process with dedicated page tables.
4. **Uninstallation**: `interos uninstall <package_id>` removes the package blob and deactivates its registry entry.

---

## 6. Device State Reporting

Device Studio reports the state of the device it can prove, and nothing else.

| Field | Meaning |
| :--- | :--- |
| `device_state: running` | The QEMU process this Studio launched is alive **and** its monitor answers. |
| `device_state: not_running` | No such process exists. A port that answers is reported as somebody else's listener, never as the device. |
| `device_state: unknown` | Something answers on the host (a monitor, a port) that the Studio cannot identify as the device it launched. |

The Studio tracks the QEMU process it launched by PID **and** the operating system's process creation time, so a recycled PID can never be mistaken for the guest. The VNC port is probed as a secondary signal only: it can never make the Studio report a running device.

The VNC display is a setting rather than a constant:

- `INTEROS_VNC_DISPLAY` selects the display index (`N` listens on `127.0.0.1:(5900+N)`); the default is `1`, i.e. `127.0.0.1:5901`.
- `VNC_PORT` is still honoured when it names a display range; the display index is derived from it so QEMU and the Studio's own client cannot disagree about the port.
- When the selected display is already served by another process, the Studio picks a free display, reports it in the launch response (`vnc_display`, `vnc_port`), and writes it to the console log.

`POST /api/qemu/launch` starts a device after a stale state, and `POST /api/qemu/stop` clears one: stopping a device that is not running succeeds and reports `QEMU is not running` instead of an error about a guest that never existed.

---

## 7. Verification Terminology

In accordance with the [Platform Reality Contract](PLATFORM_REALITY_CONTRACT.md):
- **Built**: Artifact was produced successfully by the build graph.
- **QEMU-Verified**: Subsystem was booted and confirmed working inside the guest virtual machine.
- **Hardware-Verified**: Reserved exclusively for physical hardware with verified peripheral drivers. Physical touchscreen or cellular operation is **not** implied by QEMU verification.
