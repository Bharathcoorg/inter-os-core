# InterOS Core — Smartphone Vertical Slice Audit & Integration Analysis

**Canonical Reference Platform:** x86_64 QEMU Smartphone (720x1440 portrait, 32bpp BGRX)  
**Target Environments:** x86_64, AArch64, RISC-V 64  
**Date:** 2026-11-13  
**Status:** COMPLETE INITIAL AUDIT  

---

## 1. Executive Summary & Canonical Reference Architecture

InterOS Core has completed the Phase 8.5.4 scheduler and service lifecycle encapsulation. In accordance with the project directives, focus now shifts from narrow scheduler-hardening phases to establishing a coherent, reproducible, interactive smartphone operating system experience.

The canonical execution chain for the InterOS smartphone is:
```mermaid
flowchart TD
    A["Firmware (SeaBIOS / QEMU)"] --> B["Bootloader (Stage 1 MBR + Stage 2 ELF Loader)"]
    B --> C["Kernel Entry (kernel/src/main.rs: _start)"]
    C --> D["Memory Management (Paging, Frame Allocator, GDT/IDT)"]
    D --> E["Storage Subsystem (ATA PIO, InterFS v2, Persistent Journal)"]
    E --> F["Multitasking Scheduler & IPC Router (SCHEDULER, Task 0)"]
    F --> G["Primordial Init (userspace/init: Task 1 / PID 1)"]
    G --> H["Service Manager & System Services (sensord, storaged, netd, aid)"]
    H --> I["Display Subsystem (Bochs DISPI 720x1440, DRM/KMS, Damage Tracking)"]
    I --> J["Input Pipeline (Goodix GT911 Digitizer, PS/2 Mouse Driver, Gestures)"]
    J --> K["System Shell (apps/launcher: Lock Screen, 15-App Grid, Overview, Shades)"]
    K --> L["Applications (12 Verified First-Party Apps + RAPP Sideloaded Apps)"]
```

This audit provides a complete, source-verified assessment of every link in this chain, documenting what is fully operational, what is partial or disconnected, and the required fixes to establish seamless end-to-end integration.

---

## 2. End-to-End Smartphone Execution Chain Analysis

### A. Firmware & Bootloader
- **x86_64 Reference:** Boots via SeaBIOS -> Stage 1 MBR (`bootloader/stage1/boot.asm`) -> Stage 2 (`bootloader/stage2/src/main.rs`). Sets VESA BIOS Extensions (VBE) linear framebuffer 0xE0000000, loads 64-bit kernel ELF from disk into identity-mapped physical memory, and jumps to `_start`.
- **AArch64:** Direct kernel ELF boot via QEMU `-kernel` targeting `0x40080000` with Device Tree (FDT) hardware descriptor.
- **RISC-V 64:** Direct OpenSBI M-mode jump to S-mode kernel entry at `0x80200000`.

### B. Kernel Initialization & Memory Architecture
- **x86_64:** `_start` sets up 4-level paging (`PML4`), heap allocator (16 MiB), IDT interrupt vectors, PIC/PIT timer at 100 Hz (`SYSTEM_TICK_HZ`), and COM1 serial console.
- **MMU Isolation:** Kernel code and data are mapped with `PAGE_PRESENT | PAGE_WRITABLE` (kernel-only). User tasks receive isolated `TaskPageTable` structures with user-accessible pages strictly bounded below `0x0000_7FFF_FFFF_FFFF`. User pointer access is validated via `validate_current_user_buffer` walking the page directory.
- **AArch64 / RISC-V Parity Gap:** AArch64 and RISC-V 64 implement physical frame ownership and quarantine (`QUARANTINED_FRAMES`), but x86_64 lacks frame quarantine and relies on single-core uniprocessor guarantees.

### C. Display Server & Compositor Architecture
- **Hardware Mode Setting:** `kernel/src/framebuffer.rs` programs Bochs DISPI registers (I/O ports `0x1CE` / `0x1CF`) directly to switch video hardware to 720x1440 portrait at 32bpp BGRX (`VBE_DISPI_INDEX_ENABLE | VBE_DISPI_NOCLEARMEM`). Framebuffer memory is mapped dynamically to cover all 4.14 MB.
- **In-Kernel DRM/KMS Compositor:** `kernel/src/drivers/drm_kms.rs` implements hardware scanout compositing:
  - Supports up to 8 z-ordered surfaces (z=0: Launcher/Desktop, z=100: Status/Nav Bars, z=200: Foreground App, z=300: Overlays/Dialogs).
  - Fine-grained damage tracking with bounding box clipping. Dirty scanlines are blitted 1:1 to the hardware framebuffer.
  - Zero heap allocation in compositing: utilizes pre-allocated static scanout buffers.
- **The Compositor Daemon (`apps/compositor`):**
  - Does NOT composite client surfaces. Surface compositing is performed entirely in kernel space by DRM/KMS.
  - `apps/compositor` maintains a dummy 720x96 system line surface and explicitly suppresses `sys_surface_flip` to prevent screen tearing.

### D. Input Pipeline & Digitizer Engine
- **Hardware Drivers:**
  - `kernel/src/drivers/ps2_mouse.rs`: 3-byte standard PS/2 packet decoding, clamped to 720x1440 resolution.
  - `kernel/src/drivers/touch.rs` & `kernel/src/input/digitizer.rs`: Goodix GT911 capacitive touch controller simulation over I2C, tracking contact state (down, move, up) and converting taps and swipes into high-level gestures.
- **Routing:** Touch and mouse events are normalized into `InputEvent` structs and delivered directly to the focused task's event queue.
- **Input Daemon (`userspace/system-service/src/lib.rs:run_input`):**
  - **Disconnected:** Consists of an empty heartbeat loop (`loop { sys_service_heartbeat(); sys_yield(); }`). Input routing does not traverse `inputd`.

### E. Userspace Service Manager (PID 1)
- **Startup:** `spawn_user_init(TaskId(0))` launches `userspace/init/src/main.rs` in Ring 3 as Task 1.
- **Capabilities:** Holds `SystemResource | ADMIN | INVOKE`, `IpcSend`, `IpcReceive`, `SerialOutput`.
- **Supervision:** Exercises service startup, stop, restart, and heartbeat monitoring via `SYS_SERVICE_START` (86), `SYS_SERVICE_STOP` (87), and `SYS_SERVICE_RECOVER_FAILED` (88).

### F. System Shell (Launcher) & App Lifecycle
- **Shell (`apps/launcher`):**
  - Allocates desktop surface at z=0 (720x1440).
  - Renders 15-application home grid, lock screen, app drawer, notifications shade, quick controls shade, live widgets, and power menu.
  - Processes touch gestures: swipe-up unlocks device, top-bar tap opens quick controls/notifications, right-edge swipe opens recent apps overview deck.
- **App Launching:**
  - Launcher holds `CapabilityType::ProcessSpawn` (Capability 18) and directly invokes `sys_app_launch` (Syscall 18 / 101).
  - Child application receives foreground surface at z=200, renders UI, handles touch inputs.
  - On exit gesture (back button, home bar tap, swipe up), child destroys its surface, exits via `sys_exit_with_code`, and parent launcher reaps it via `sys_wait_pid`.

---

## 3. Subsystem-by-Subsystem Audit & Status Matrix

| Subsystem | Source Path | Status | Verification Evidence / Key Findings |
| :--- | :--- | :---: | :--- |
| **x86_64 Boot Chain** | `bootloader/`, `kernel/src/main.rs` | **WORKING** | Clean MBR -> Stage 2 -> Kernel jump; Identity page tables; PIT timer 100 Hz. |
| **AArch64 Boot Chain** | `kernel-aarch64/src/main.rs` | **WORKING** | QEMU virt board boots; FDT parsed; UART PL011 online. |
| **RISC-V 64 Boot Chain** | `kernel-riscv64/src/main.rs` | **WORKING** | OpenSBI boots; Sv39 MMU enabled; SBI timer ticks verified. |
| **Framebuffer / DISPI** | `kernel/src/framebuffer.rs` | **WORKING** | Bochs DISPI programmed to 720x1440 32bpp; 4.14 MB mapped; 1:1 pixel blitting. |
| **DRM/KMS Compositor** | `kernel/src/drivers/drm_kms.rs` | **WORKING** | 8 z-ordered surfaces; damage rectangle tracking; tearing-free scanout. |
| **Input Pipeline** | `kernel/src/drivers/ps2_mouse.rs` | **WORKING** | Drained in `sys_event_poll`, `sys_yield`, and Task 0 idle loop; verified by `tests/test_qemu_pointer_input.py`. |
| **Touchscreen Digitizer** | `kernel/src/input/digitizer.rs` | **WORKING** | GT911 protocol; swipe/tap gesture engine; coordinate clamping verified. |
| **PID 1 Userspace Init** | `userspace/init/src/main.rs` | **WORKING** | Boots in Ring 3; verifies service stop/restart; loops on service supervisor; `register_init_task` synchronizes PID 1 TaskId. |
| **Input Daemon (`inputd`)**| `userspace/system-service/` | **DISCONNECTED** | Empty loop in `userspace/system-service/src/lib.rs:run_input`; kernel routes input directly. |
| **Compositor App** | `apps/compositor/src/main.rs` | **PARTIAL** | Non-publishing system line surface; DRM KMS in-kernel compositor owns actual rendering. |
| **Launcher / Shell** | `apps/launcher/src/main.rs` | **WORKING** | Full mobile UI: Home, Drawer, Recents, Notifications, Quick Controls, Power Menu. |
| **12 First-Party Apps** | `apps/` | **WORKING** | Verified via QEMU lifecycle test (Calculator, Settings, Vault, Camera, Dialer, Browser, Messages, Calendar, Music, Files, Gallery, Notes). |
| **Storage / ATA PIO** | `kernel/src/drivers/ata.rs` | **WORKING** | Primary ATA Master (boot) and Slave (6 GiB data); boot guard prevents boot corruption. |
| **InterFS v2 Filesystem** | `kernel/src/storage/interfs.rs`| **WORKING** | Dynamic allocation bitmap; contiguous streaming extents; crash recovery journal. |
| **Metadata Journal** | `kernel/src/storage/persistent.rs`| **WORKING** | 20 slots, 10 dual-slot alternating kinds; HMAC-SHA256 authenticated tags. |
| **RAPP Packaging** | `kernel/src/package/rapp.rs` | **WORKING** | v2/v3/v4 manifests; Ed25519 signature checks; capability tables; dependency checks; reboot intent persistence verified. |
| **Device Studio Server** | `emulator/server.py` | **WORKING** | Dynamic port allocation (4301/4302/4303); `bochs-display` profile config; offline telemetry reports `UNAVAILABLE` and battery `None`. |

---

## 4. Empirical Live Verification Results

The interactive lifecycle test suite (`tests/test_interactive_lifecycle.py`) was executed against the canonical x86_64 QEMU smartphone build. All 15 interactive operations passed without regressions:

```
============================================================
  INTEROS CORE LIFECYCLE INTERACTIONS: PASSED (15/15)
============================================================
1.  Launcher Home Screen Presentation: PASSED (15 packages loaded, canvas rendered)
2.  Lock Screen Unlock: PASSED (Gesture swipe-up unlocked UID 1000)
3.  App Drawer Presentation: PASSED (Switched category tabs)
4.  Calculator Launch & Interaction: PASSED (7 + 5 = 12 calculated, reaped via Back button)
5.  Settings Launch & Navigation: PASSED (Tabs switched, reaped via Swipe-up)
6.  Vault Launch & Interaction: PASSED (Sector card 0 re-verified, reaped via Back button)
7.  Camera Launch & Mode Switch: PASSED (Switched to Document Vision, reaped via Swipe-up)
8.  Dialer Launch & Keypad Entry: PASSED (Digit '9' entered, reaped via Back button)
9.  Browser Launch & Bookmark Tap: PASSED (Navigated bookmark, reaped via Home bar)
10. Messages Launch & Thread Open: PASSED (Opened conversation thread, reaped via Back button)
11. Calendar Launch & Interaction: PASSED (Reaped via Swipe-up)
12. Music Player & Backgrounding: PASSED (Transport toggled play/pause, reaped via Swipe-up)
13. Files Manager & Category Filter: PASSED (Switched Images shortcut, reaped via Home bar)
14. Gallery & Notes: PASSED (Filter chips switched, reaped via Home bar)
15. Shell Features:
    - Recent Apps Overview Deck: PASSED (Switched to app, dismissed via [X] and swipe)
    - Quick Controls Shade: PASSED (Opened, toggles rendered, returned home)
    - Notifications Shade: PASSED (Opened, records rendered, returned home)
    - Power Menu & Lock Screen: PASSED (Locked device, flashlight reported unavailable)
    - Global Search Surface: PASSED (Opened, search canvas rendered, returned home)
```

---

## 5. Architectural Disparities & Platform Truth Audit

### A. Capability Rights Bitmask Disparity
In `kernel/src/capability/mod.rs` (x86_64):
- `Rights::REVOKE = 1 << 4` (0x10)
- `Rights::INVOKE = 1 << 5` (0x20)
In `kernel-aarch64/src/capability.rs` and `kernel-riscv64/src/capability.rs`:
- `Rights::INVOKE = 1 << 4` (0x10)
- `Rights::ADMIN = 1 << 5` (0x20)
*Impact:* SDK syscall wrappers passing `Rights::INVOKE` (0x20) fail authorization on AArch64/RISC-V 64.

### B. Service ABI Wire Ceiling Truncation (RESOLVED)
`kernel/src/service_abi.rs` and `sdk/rust/interos-std/src/service.rs` previously pinned `SERVICE_ID_MAX = 11`, which caused `ServiceId::Inference = 12` (`aid`) and higher services to be rejected by wire message decoders.
*Resolution:* Unified `SERVICE_ID_MIN = 1` and `SERVICE_ID_MAX = 15` across kernel, SDK, and contract tests. Validated IDs 1..=15 (`Display` through `Voice`), with explicit rejection of ID 0, ID > 15, truncated buffers, bad magic, bad version, unknown opcodes, and oversized payloads.

### C. `ServiceId::Init` Registration TaskId Mismatch (RESOLVED)
In `kernel/src/services.rs:500`, `records[Init]` was initialized with `TaskId(0)` (kernel main thread).
*Resolution:* Implemented `register_init_task(id: TaskId) -> Result<(), &'static str>`. Rejects `TaskId(0)` and updates the Init record in `SERVICE_RECORDS` under lock upon successful spawn of `user_init`.

### D. Platform Truth Telemetry Violations in Device Studio (RESOLVED)
1. `GET /api/extended-controls/state` now verifies `tracked_qemu_process_alive()`; when offline, battery level is `None`, health is `'UNAVAILABLE'`, and telemetry status is `'UNAVAILABLE'`.
2. `server.py` serial stream worker and QEMU monitor dynamically bind to active profile ports (`4301`/`44445` for x86_64, `4302`/`44446` for AArch64, `4303`/`44447` for RISC-V 64), resetting monitor connections and serial epochs upon profile switch or stop.
3. `server.py` profiles configure `-device bochs-display` instead of `virtio-gpu-pci`.
4. `/api/touch/event` explicitly returns `'delivery_method': 'serial_injection'` and `'hardware_touch': False`, rejecting synthetic touch with HTTP 503 when the emulator is offline.

---

## 6. Input Tier Separation & Hardware Reality

InterOS Core strictly delineates three tiers of input handling:

```mermaid
flowchart LR
    subgraph TierA["Tier A: Serial Injection"]
        direction TB
        A1["Host Test Runner"] --> A2["UART COM1 (I/O 0x3F8)"]
        A2 --> A3["kernel/src/serial.rs: TAP/SWIPE"]
        A3 --> A4["Digitizer Simulation (Contact Down/Move/Up)"]
    end
    subgraph TierB["Tier B: Emulated Pointer Interaction"]
        direction TB
        B1["QEMU HMP Monitor"] --> B2["i8042 Controller (IRQ 12, Port 0x60/0x64)"]
        B2 --> B3["kernel/src/drivers/ps2_mouse.rs"]
        B3 --> B4["Drained in sys_event_poll, sys_yield & Idle Loop"]
    end
    subgraph TierC["Tier C: Capacitive Touch Controller"]
        direction TB
        C1["Goodix GT911 Digitizer"] --> C2["I2C Bus / Memory Adapter Driver"]
        C2 --> C3["kernel/src/input/digitizer.rs"]
        C3 --> C4["Coordinate Clamping to 720x1440 Portrait"]
    end
    A4 --> F["Focused Window Event Queue (InputEvent)"]
    B4 --> F
    C4 --> F
```

- **Tier A (Serial Test Injection):** Direct command injection over UART COM1 via `TAP <x> <y>` and `SWIPE <x1> <y1> <x2> <y2> <steps>`. Intended exclusively for automated regression testing and headless test harnesses. Marked with `hardware_touch: false` in Device Studio.
- **Tier B (Emulated PS/2 Pointer Interaction):** Standard 3-byte PS/2 mouse packets delivered via QEMU's emulated i8042 PS/2 controller (IRQ 12, port 0x60/0x64). Clamped to 720x1440 portrait bounds. Drained without starvation in `sys_event_poll`, `sys_yield`, and the Task 0 idle loop. Verified by `tests/test_qemu_pointer_input.py`.
- **Tier C (Capacitive Multi-Touch Digitizer):** Goodix GT911 driver interfacing over I2C memory adapter. Evaluates touch contact tracking (Down, Move, Up) and coordinates. On standard QEMU PC hardware, physical touch hardware is absent; the GT911 driver functions via in-kernel memory-mapped adapter simulation.

---

## 7. Cross-Architecture Interactive Parity & Reality

- **x86_64 (Canonical Reference Platform):** Full interactive smartphone vertical slice verified. Bochs DISPI mode setting to 720x1440 at 32bpp BGRX, in-kernel DRM/KMS compositor with 8 surfaces, PS/2 mouse and touch digitizer input, PID 1 userspace init, system services, and 15 interactive application lifecycle operations verified live.
- **AArch64 (`-M virt`):** Headless and serial-interactive baseline verified. Bochs PCI display adapter (`-device bochs-display`), PL011 UART console, EL0 userspace execution, 10-contract supervisor lifecycle verified. No physical touchscreen hardware claimed on standard QEMU `virt` machine.
- **RISC-V 64 (`-M virt`):** Headless and serial-interactive baseline verified. Bochs PCI display adapter (`-device bochs-display`), 16550 UART console, OpenSBI Sv39 MMU, U-mode userspace execution, 10-contract supervisor lifecycle verified. No physical touchscreen hardware claimed on standard QEMU `virt` machine.

---

## 8. Detailed Bugs & Gaps Resolution Catalog

All priority integration blockers identified in the vertical slice audit are resolved:

1. **[P0-INPUT] PS/2 Mouse Event Drain Starvation (RESOLVED):**
   - Added `crate::drivers::ps2_mouse::drain_events()` to `sys_event_poll` and the Task 0 idle loop. Tested under continuous packet injection and idle polling.
2. **[P0-SERVICES] Service ABI Limit Truncation (RESOLVED):**
   - Expanded `SERVICE_ID_MAX` from 11 to 15 in `kernel/src/service_abi.rs` and `sdk/rust/interos-std/src/service.rs`. Added service IDs 12 (`Inference`), 13 (`Search`), 14 (`Agent`), 15 (`Voice`). Updated contracts in `tests/test_system_service_contracts.py`.
3. **[P0-STUDIO] Device Studio Port Mismatch & Non-x86 Display Driver (RESOLVED):**
   - Dynamic port resolution (`4301`, `4302`, `4303`) implemented. Added `reset_monitor_connection()` and `reset_serial_epoch()` on profile switches and server shutdown. Corrected QEMU display parameter to `bochs-display`.
4. **[P0-PACKAGES] Serial Sideloading Drops RAPP v4 Intent Declarations (RESOLVED):**
   - Registered `verified.intents` in `INTENT_REGISTRY` upon `RAPP_COMMIT`. Unregistered in `dev_uninstall_verified_rapp`. Added `INTENT_RESOLVE` serial command.
5. **[P1-INIT] Service Manager PID 1 Task ID Registration (RESOLVED):**
   - Added `register_init_task(id: TaskId) -> Result<(), &'static str>` in `kernel/src/services.rs` and invoked it with assertion in `kernel/src/main.rs`.
6. **[P1-PLATFORM] Device Studio Telemetry Platform Truth (RESOLVED):**
   - `GET /api/extended-controls/state` returns `level: None`, `health: 'UNAVAILABLE'`, `telemetry_status: 'UNAVAILABLE'` when QEMU is offline. `POST /api/touch/event` returns 503 when offline.
7. **[P1-STORAGE] Third-Party Intent Reboot Persistence (RESOLVED):**
   - Implemented `restore_persisted_rapp_intents()` in `kernel/src/package/mod.rs`. Reads active packages from `PACKAGE_REGISTRY`, retrieves blobs from `PACKAGE_BLOB_STORE`, cryptographically validates via `parse_rapp`, and re-registers declared intents during boot. Verified via `tests/test_persistent_intents.py`.

---

## 9. Verification Battery & Acceptance Criteria

The full contract and verification battery executed locally confirms system integrity:
1. `cargo check --workspace`: PASSED (0 warnings/errors)
2. `cargo check --manifest-path kernel-aarch64/Cargo.toml`: PASSED (0 errors)
3. `cargo check --target riscv64gc-unknown-none-elf --manifest-path kernel-riscv64/Cargo.toml`: PASSED (0 errors)
4. `cargo test --manifest-path kernel/Cargo.toml`: PASSED (598 passed, 0 failed, 1 ignored)
5. `cargo test --manifest-path sdk/rust/interos-std/Cargo.toml`: PASSED (459 passed, 0 failed)
6. `python tools/interos.py contracts`: PASSED (25/25)
7. `python tests/run_all_contracts.py`: PASSED (111 active suites, 1253 passed, 0 failed, 1 skipped)
8. `python tests/verify_x86_boot.py`: PASSED (100%)
9. `python tests/verify_aarch64_boot.py`: PASSED (100%)
10. `python tests/verify_riscv_boot.py`: PASSED (100%)
11. `python tests/test_persistent_intents.py`: PASSED (100%)
12. `python tests/test_qemu_pointer_input.py`: PASSED (100%)
13. `python -m pytest tests/test_platform_reality.py`: PASSED (68/68)
14. `python -m pytest tests/test_studio_truth_contract.py`: PASSED (25/25)
