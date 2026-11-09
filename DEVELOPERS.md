# InterOS Core — Developer & Studio Guide

InterOS Core is an open-source research prototype of a smartphone operating system, running in QEMU only. It is not a commercial product. External contributions are not accepted at this stage and will open after the first stable release.

This document details the development workflow, Device Studio modes, virtual machine inspection, and UI/input guidelines.

---

## 📱 Device Studio: Focus Mode vs. Telemetry Deck

Device Studio (`http://localhost:4300`) is the primary developer interface for InterOS running in QEMU. It provides two operational modes:

### 1. Immersive Smartphone Focus Mode (Default)
By default, Device Studio launches in **Focus Mode**:
- **Distraction-Free**: Centers the native virtual smartphone with an ambient frosted glass backdrop.
- **Flagship Aesthetic**: Optimized for responsive interactive testing, UI/UX evaluation, app navigation, and presentations.
- **Real-Time Input**: Mouse and touchpad gestures map directly with responsive 1:1 coordinate scaling to the guest microkernel.

### 2. Developer Telemetry Deck
The **Developer Telemetry Deck** provides deep, transparent virtual machine and microkernel inspection tools:
- **CPU Registers & State (x86_64 Target)**: Live registers (RAX, RBX, RCX, RDX, RSI, RDI, RIP, RSP), CPU execution state, and active Task ID.
- **Memory & VFS Block I/O**: Real QEMU RAM allocation, active block storage drive name, read ops, and sector I/O accounting.
- **Live Process Supervisor**: Task list, hardware privilege level (Ring 0/3 on x86_64, EL1/0 on AArch64, S/U-Mode on RISC-V), thread states (`Running`, `Blocked`, `Sleeping`), and PID accounting.
- **IPC Inspector**: Real-time shared-memory IPC ring buffer endpoints, futex events, and message router telemetry.
- **Package Ingest & Sideloading**: Drag-and-drop classifies `.app` (ELF), `.wasm`, `.apk` and `.rapp` payloads on the host and answers `Analyzed; guest installation required` — an `.apk` drop reports DEX and signature *metadata* and explicitly performs no cryptographic verification. The signed install path is separate (`/api/rapp/install`), verifies the RAPP v2 Ed25519 signature and accepts x86_64 `.rapp` packages only. The guest package registry remains the only installed-app authority.

---

## ⌨️ How to Use and Switch to Deck Mode

Developers can switch between **Focus Mode** and the **Telemetry Deck** at any time using simple controls:

| Action | Shortcut / Trigger | Description |
| :--- | :--- | :--- |
| **Open Deck** | Click **`Deck`** button | Top header pane switcher (right side) |
| **Hot Switch to Deck** | Press **`D`** | Toggles from Focus Mode into full Telemetry Deck |
| **Exit Focus Mode** | Press **`Escape`** | Collapses phone focus and expands the Telemetry Deck |
| **Toggle Focus Mode** | Press **`F`** | Toggles between immersive Focus Mode and standard view |
| **Fullscreen WebApp** | Click **`Fullscreen`** | Hides browser address bar and tabs for native mobile feel |
| **Virtual Lock / Power**| Click **Power Key** / `#hw-btn-power` | Simulates pressing the virtual power key |

> [!TIP]
> **Developer Quick Workflow**: Press **`D`** while testing any app to inspect its CPU register allocation and IPC packets in real time. Press **`F`** to return to immersive smartphone testing.

---

## 🔒 Lock Screen & Gestures Policy

InterOS enforces a strict mobile touch and authentication contract:
1. **Swipe Up to Unlock**:
   - Just like iOS and Android, tapping the lock screen does **not** dismiss it. An upward swipe (drag distance $\ge 28\text{px}$) is required to unlock.
   - This prevents accidental wakeups, pocket-taps, or click-release bypasses.
2. **PIN Protection**:
   - If an owner PIN has been provisioned (via Settings > Security), swiping up transitions to `ViewMode::PinEntry`.
   - On an unprovisioned system, swiping up directly unlocks onto the Home Screen.
3. **Locking the Screen**:
   - **Chassis Button**: Click the right-side virtual power button on the studio chassis.
   - **Quick Controls**: Swipe down from top-right status bar, tap **Screen lock**.
   - **Power Menu**: Tap battery status area or click virtual power button, tap **Lock device**.

---

## 🛠 Testing & Verification Directives

Before committing changes, always verify all verification suites pass:
```bash
# 1. Run full contract test suite
python tests/run_all_contracts.py

# 2. Run kernel unit & subsystem tests
cargo test --manifest-path kernel/Cargo.toml

# 3. Run launcher and system apps tests
cargo test --manifest-path apps/launcher/Cargo.toml
cargo test --manifest-path apps/settings/Cargo.toml
```
