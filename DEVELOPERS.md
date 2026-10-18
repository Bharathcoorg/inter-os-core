# InterOS Core — Developer & Studio Guide

Welcome to the **InterOS** developer guide. This document details our development workflow, Device Studio modes, telemetry inspection, and UI/input guidelines.

---

## 📱 Device Studio: Focus Mode vs. Telemetry Deck

Device Studio (`http://localhost:4300`) is the primary developer interface for InterOS running in QEMU. It provides two operational modes:

### 1. Immersive Smartphone Focus Mode (Default)
By default, Device Studio launches in **Focus Mode**:
- **Distraction-Free**: Centers the native virtual smartphone with an ambient frosted glass backdrop.
- **Flagship Aesthetic**: Optimized for responsive interactive testing, UI/UX evaluation, app navigation, and presentations.
- **Real-Time Input**: Mouse and touchpad gestures map directly with zero-latency 1:1 coordinate scaling to the guest microkernel.

### 2. Developer Telemetry Deck
The **Developer Telemetry Deck** provides deep, transparent hardware and microkernel inspection tools:
- **CPU Registers & State**: Live registers (RAX, RBX, RCX, RDX, RSI, RDI, RIP, RSP), CPU execution state, and active Task ID.
- **Memory & VFS Block I/O**: Real QEMU RAM allocation, active block storage drive name, read ops, and sector I/O accounting.
- **Live Process Supervisor**: Task list, Ring privilege level, thread states (`Running`, `Blocked`, `Sleeping`), and PID accounting.
- **IPC Inspector**: Real-time shared-memory IPC ring buffer endpoints, futex events, and message router telemetry.
- **Universal Package Sideloading**: Drag-and-drop installer supporting `.app` (Rust native), `.rap` (Cryptographic sovereign packages), and `.apk` (Android compatibility boundary).

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
| **Hardware Lock / Power**| Click **Power Key** / `#hw-btn-power` | Simulates pressing the physical smartphone power button |

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
   - **Hardware Button**: Click the right-side physical power button on the chassis.
   - **Quick Controls**: Swipe down from top-right status bar, tap **Screen lock**.
   - **Power Menu**: Tap battery status area or click hardware power button, tap **Lock device**.

---

## 🛠 Testing & Verification Directives

Before committing changes, always verify all verification suites pass:
```bash
# 1. Run full contract test suite (551/551 contracts)
python tests/run_all_contracts.py

# 2. Run kernel unit & subsystem tests
cargo test --manifest-path kernel/Cargo.toml

# 3. Run launcher and system apps tests
cargo test --manifest-path apps/launcher/Cargo.toml
cargo test --manifest-path apps/settings/Cargo.toml
```
