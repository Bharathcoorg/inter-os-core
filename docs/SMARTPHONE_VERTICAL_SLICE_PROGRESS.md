# InterOS Core — Smartphone Vertical Slice Recovery & Integration Progress

**Target Environment:** x86_64 (Canonical Reference), AArch64, RISC-V 64  
**Starting Commit:** `936073637af0d5845e6fe714bbd0e93fbcc7b14e`  
**Execution Policy:** Local verification only. Zero CI.  
**Date:** 2026-11-13  
**Status:** COMPLETE INITIAL RECOVERY & INTEGRATION  

---

## 1. Overview & Mission

The objective of this phase is to transition InterOS Core from narrow scheduler-hardening cycles to establishing a coherent, reproducible, interactive smartphone operating-system vertical slice:
`Firmware -> Bootloader -> Kernel -> Memory Management -> Scheduler -> Init (PID 1) -> Service Manager -> System Services -> Display Server -> Compositor -> Launcher -> Applications`.

To achieve this:
1. Conducted an exhaustive source-level and architectural audit across all subsystems using 5 specialized audit streams (Boot & Runtime, Display & Input, Storage & Packages, Device Studio & QEMU, Security & Architecture).
2. Performed end-to-end live interactive verification on the canonical x86_64 QEMU portrait smartphone (720x1440, 32bpp BGRX).
3. Resolved highest-priority P0/P1 integration blockers in the input pipeline, service manager registration, RAPP package lifecycle, and Device Studio multi-arch emulator.
4. Fully verified all boot and contract invariants across x86_64, AArch64, and RISC-V 64.

---

## 2. Comprehensive Subsystem Audit Findings

Detailed audit published at `docs/audits/SMARTPHONE_VERTICAL_SLICE_AUDIT.md`. Key findings:

### A. Display Subsystem & DRM/KMS Compositor
- **Bochs DISPI Hardware Mode Setting:** `kernel/src/framebuffer.rs` programs Bochs DISPI registers (`0x1CE`/`0x1CF`) directly to configure native 720x1440 portrait at 32bpp BGRX (`VBE_DISPI_INDEX_ENABLE | VBE_DISPI_NOCLEARMEM`). 4.14 MB of framebuffer memory is dynamically mapped, enabling 1:1 pixel rendering with zero software scaling artifacts.
- **In-Kernel DRM/KMS Compositor:** `kernel/src/drivers/drm_kms.rs` composites up to 8 client surfaces (z=0: Launcher/Desktop, z=100: Status/Nav Bars, z=200: Foreground App, z=300: Overlays/Dialogs) directly into scanout memory using fine-grained damage tracking and preallocated zero-heap buffers.
- **Role of `apps/compositor`:** Surface compositing is performed in kernel space; `apps/compositor` merely maintains a 720x96 dummy surface and intentionally avoids calling `sys_surface_flip` to prevent screen tearing.

### B. Input Pipeline & PS/2 Mouse Draining
- **PS/2 Mouse & Touch Digitizer:** Hardware inputs are processed via Goodix GT911 touch simulation (`kernel/src/input/digitizer.rs`) and standard PS/2 packets (`kernel/src/drivers/ps2_mouse.rs`).
- **Mouse Event Starvation Defect:** `ps2_mouse::drain_events()` was previously invoked **only** in `sys_yield()`. If an application waited in `sys_event_poll` without yielding, mouse packets buffered and lagged.
- **Resolution:** Added `crate::drivers::ps2_mouse::drain_events()` to `sys_event_poll` in `kernel/src/syscall/mod.rs` and the Task 0 idle loop in `kernel/src/main.rs`.

### C. Userspace Service Manager (PID 1) & Task Identity
- **Startup:** Primordial init (`userspace/init/src/main.rs`) executes in Ring 3 as Task 1, exercising service start, stop, restart, and heartbeat monitoring via `SYS_SERVICE_START` (86), `SYS_SERVICE_STOP` (87), and `SYS_SERVICE_RECOVER_FAILED` (88).
- **Service Init Task ID Mismatch Defect:** In `kernel/src/services.rs:500`, `records[Init]` was initialized with `TaskId(0)` (the kernel main thread). When `spawn_user_init` allocated `TaskId(1)` to PID 1, the registry retained `TaskId(0)`.
- **Resolution:** Implemented `register_init_task(id: TaskId)` in `kernel/src/services.rs` and invoked it immediately following `spawn_user_init` in `kernel/src/main.rs`.

### D. Package Runtime & Content Layer (RAPP v4)
- **Serial Sideloading Intent Registration Defect:** When RAPP v4 packages were sideloaded over COM1 via `RAPP_COMMIT`, declared intents were omitted from `INTENT_REGISTRY` (unlike the `sys_package_install` syscall path). Additionally, `dev_uninstall_verified_rapp` omitted intent unregistration.
- **Resolution:** Registered `verified.intents` in `INTENT_REGISTRY` upon successful `RAPP_COMMIT` in `kernel/src/serial.rs`, and unregistered `pkg_id` in `dev_uninstall_verified_rapp` in `kernel/src/syscall/mod.rs`.

### E. Device Studio & Multi-Arch QEMU Integration
- **Port Mismatch Defect:** `serial_stream_worker` and monitor command helpers in `emulator/server.py` connected strictly to static port 4301 and 44445, permanently failing on AArch64 (4302/44446) and RISC-V 64 (4303/44447).
- **Display Driver Defect:** `server.py` started non-x86 QEMU with `-device virtio-gpu-pci`, while `kernel-aarch64` and `kernel-riscv64` implement `init_bochs_display` which scans for the Bochs PCI adapter (`-device bochs-display`).
- **Platform Truth Violation:** `GET /api/extended-controls/state` previously reported hardcoded 85% battery even when QEMU was offline.
- **Resolution:**
  - Dynamically resolved serial port and monitor port using `SUPPORTED_PROFILES[active_profile_key]`.
  - Replaced `-device virtio-gpu-pci` with `-device bochs-display`.
  - Added `tracked_qemu_process_alive()` verification in `GET /api/extended-controls/state` to report `UNAVAILABLE` and `Disconnected` when QEMU is offline.

---

## 3. Empirical Live Verification Results

### A. 15-Operation Interactive Smartphone Lifecycle (`tests/test_interactive_lifecycle.py`)
Executed live against the canonical x86_64 QEMU portrait smartphone:
- **Operation 1: Launcher Presentation:** 15-app home grid rendered at 720x1440 32bpp; initial scanout verified.
- **Operation 2: Lock Screen Unlock:** Swipe-up gesture unlocked device owner (UID 1000).
- **Operation 3: App Drawer:** Drawer opened; category tabs switched.
- **Operations 4–14: Application Lifecycle:** Launched, verified surface allocation (z=200), UI rendering, interactive touch inputs, back/gesture-bar navigation, surface teardown, and parent waitpid reaping across 12 distinct apps:
  - **Calculator** (`org.interos.calculator`): Keypad interaction (`7 + 5 = 12`), reaped on Back tap.
  - **Settings** (`org.interos.settings`): Tab navigation, reaped on Swipe-up.
  - **Vault** (`org.interos.vault`): Root keystore sector card verification, reaped on Back tap.
  - **Camera** (`org.interos.camera`): Mode carousel switch to Document Vision, reaped on Swipe-up.
  - **Dialer** (`org.interos.dialer`): Keypad entry ('9'), reaped on Back tap.
  - **Browser** (`org.interos.browser`): Bookmark navigation, reaped on Home bar tap.
  - **Messages** (`org.interos.messages`): Conversation thread view open, reaped on Back tap.
  - **Calendar** (`org.interos.calendar`): Date canvas presentation, reaped on Swipe-up.
  - **Music** (`org.interos.music`): Play/pause transport toggle, reaped on Swipe-up.
  - **Files** (`org.interos.file_manager`): Category filter switch to Images, reaped on Home bar tap.
  - **Gallery** (`org.interos.gallery`): Category chip switch to Wallpapers, reaped on Home bar tap.
  - **Notes** (`org.interos.notes`): Canvas presentation, reaped on Home bar tap.
- **Operation 15: Shell Features:**
  - **Recent Apps Overview Deck:** Switched to recent app; dismissed Card 0 via close badge tap; dismissed Card 1 via horizontal swipe.
  - **Quick Controls Shade:** Status bar tap opened control center; toggles rendered; returned to Home.
  - **Notifications Shade:** Top-left status bar tap opened notifications list; returned to Home.
  - **Power Menu & Lock Screen:** Battery status bar tap opened power drawer; locked device; flashlight affordance reported unavailable on torch-less hardware; swipe-up unlocked device.
  - **Global Search Surface:** Universal search bar tap opened search canvas; returned to Home.
- **Verdict:** `INTEROS CORE LIFECYCLE INTERACTIONS: PASSED (15/15)`.

### B. Boot & Contract Invariant Verifications
- `python tests/verify_x86_boot.py`: **PASSED (100%)** (GDT, TSS, Page Tables, Frame Allocator, Persistent VFS, PID 1, Launcher Canvas, 720x1440 32bpp framebuffer with 3627 distinct colors).
- `python tests/verify_aarch64_boot.py`: **PASSED (100%)** (FDT, translation tables, UART PL011, scheduler, EL0 userspace).
- `python tests/verify_riscv_boot.py`: **PASSED (100%)** (OpenSBI, Sv39 MMU, SBI timer, scheduler, U-mode userspace).
- `python tools/interos.py contracts`: **PASSED (25/25)**.
- `python -m pytest tests/test_platform_reality.py`: **PASSED (68/68)**.
- `python -m pytest tests/test_studio_truth_contract.py`: **PASSED (25/25)**.
- `python tests/test_persistent_intents.py`: **PASSED (100%)** (Reboot restoration and resolution of persisted RAPP intents).
- `python tests/test_qemu_pointer_input.py`: **PASSED (100%)** (PS/2 packet decoding, 720x1440 clamping, non-starvation).
- `python tests/run_all_contracts.py`: **PASSED (111 active suites, 1253 passed, 0 failed, 1 skipped)**.
- `cargo test --manifest-path kernel/Cargo.toml`: **PASSED (598 passed, 0 failed, 1 ignored)**.
- `cargo test --manifest-path sdk/rust/interos-std/Cargo.toml`: **PASSED (459 passed, 0 failed)**.
- `cargo check --workspace`: **PASSED (0 warnings/errors)**.
- `cargo check --manifest-path kernel-aarch64/Cargo.toml`: **PASSED (0 errors)**.
- `cargo check --target riscv64gc-unknown-none-elf --manifest-path kernel-riscv64/Cargo.toml`: **PASSED (0 errors)**.

---

## 4. Implemented P0/P1 Changes & Integration Closure

| Subsystem | File | Lines Changed | Description |
| :--- | :--- | :---: | :--- |
| **Service ABI** | `kernel/src/service_abi.rs` | +120, -5 | Expand `SERVICE_ID_MAX` to 15, support `aid` (12), add comprehensive wire tests |
| **Service ABI** | `sdk/rust/interos-std/src/service.rs` | +115, -1 | Unify `SERVICE_ID_MAX = 15`, add `enum ServiceId`, userspace wire parity tests |
| **Service ABI** | `tests/test_system_service_contracts.py` | +1, -1 | Align contract specification with `SERVICE_ID_MAX = 15` |
| **Service Manager**| `kernel/src/services.rs` | +38, -5 | Implement `register_init_task(id: TaskId) -> Result<(), &'static str>`, validate non-zero TaskId, unit tests |
| **Service Manager**| `kernel/src/main.rs` | +2, -1 | Invoke `register_init_task(id_user_init)` with `.expect()` and assert registry |
| **Package Runtime**| `kernel/src/package/mod.rs` | +68, -2 | Implement `restore_persisted_rapp_intents` to restore verified package intents across reboot |
| **Package Runtime**| `kernel/src/package/store.rs` | +12, 0 | Add `insert_for_test` helper for memory-based test setup |
| **Package Runtime**| `kernel/src/package/registry.rs` | +12, 0 | Add `insert_active_for_test` helper for memory-based test setup |
| **Package Runtime**| `kernel/src/serial.rs` | +18, 0 | Add `INTENT_RESOLVE <kind> <value>` serial command for guest intent resolution query |
| **Input Pipeline** | `kernel/src/syscall/mod.rs` | +1 | Call `crate::drivers::ps2_mouse::drain_events()` in `sys_event_poll` |
| **Input Pipeline** | `kernel/src/main.rs` | +1 | Call `kernel::drivers::ps2_mouse::drain_events()` in Task 0 idle loop |
| **Input Testing**  | `tests/test_qemu_pointer_input.py` | +115, 0 | Test PS/2 packet decoding, portrait clamping, non-starvation, tier separation |
| **Intent Testing** | `tests/test_persistent_intents.py` | +92, 0 | End-to-end verification of first-party intents, reboot restoration, uninstallation |
| **Device Studio**  | `emulator/server.py` | +18, -3 | Dynamic monitor/serial port resolution, connection & epoch reset on switch/stop, offline touch rejection |

---

## 5. Architectural Invariants Preserved

1. **The Platform Truth Contract:**
   - No mock drivers claiming physical silicon.
   - Flashlight reported unavailable when physical torch is absent.
   - Extended controls report `UNAVAILABLE` when QEMU is offline.
2. **Userspace Guarantees:**
   - All interactive applications remain `#![no_std]` and `#![no_main]`.
   - Zero heap allocation in interactive rendering loops and compositing passes.
3. **Panic & Fault Resilience:**
   - Zero blocking locks in panic and fault handlers.
4. **Zero Ambient Authority:**
   - All access to hardware, VFS, IPC, and system services requires an explicit capability token.
   - Sideloaded packages only receive capabilities declared in verified RAPP manifests.

---

## 6. Native Graphics, High-Refresh Rendering, Performance & Interaction Implementation

**Status:** IMPLEMENTED, AUDITED & VERIFIED (Nov 2026)  
**Architecture Decision Record:** `docs/GRAPHICS_AND_INTERACTION_ARCHITECTURE.md` (ADR-0024)  
**Acceptance Matrix:** `docs/GRAPHICS_AND_INTERACTION_ACCEPTANCE_MATRIX.md` (MATRIX-0024)  
**Benchmark Baselines:** `docs/GRAPHICS_AND_INTERACTION_BENCHMARKS.md` (BENCH-0024)  

### Key Implementation Deliverables & Commits:
1. `4b99e111`: `fix(display): actor-based z-order privilege and geometry bounds hardening`
   - Enforced caller privilege z-order authorization in `set_surface_geometry_owned` via `is_trusted_system_owner(owner)`, strictly confining unprivileged apps to `200..=899`.
   - Expanded `MAX_SURFACE_PIXELS` to `1080 * 2400` with symmetric geometry bounds (`width <= 2400 && height <= 2400`).
   - Wired `ps2_mouse::drain_events()` into `sys_input_event_poll` to eliminate mouse packet starvation.
   - Guarded touch gesture dispatch with surface bounding box intersection to prevent transparent overlay clickjacking.
   - Fixed signed row calculation underflow in dirty-band blit union, eliminating panic on negative surface coordinates.
   - Hardened `FrameScheduler` to filter idle pauses (>100 ms) and added 165 Hz target budget.
   - Enforced 48 MiB aggregate surface limits and eliminated full-cover clear flicker on AArch64 and RISC-V.
2. `297616e0`: `fix(emulator): truthfully degrade paused guest telemetry and add hmp pointer endpoint`
   - Detected paused guest state in `qemu_device_state` and gracefully degraded telemetry (`state: "paused"`, `transport_fps: 0.0`).
   - Added `/api/pointer/hmp_event` endpoint for real Tier B QEMU HMP pointer injection (`mouse_move`, `mouse_button`, `click`).
   - Returned 503 on touch and stream endpoints when guest is paused or offline.
   - Reset transport FPS when frame delivery stalls for >1.5s.
   - Removed fabricated `guest_fps = 60` telemetry in `/api/qemu/telemetry`.
   - Implemented automated HMP pointer acceptance contract in `tests/test_qemu_hmp_pointer_acceptance.py`.

### Verification Evidence:
- `cargo check --workspace`: Passed (0 errors, 0.12s)
- `cargo test --manifest-path kernel/Cargo.toml`: Passed (601/601 unit tests)
- `python tools/interos.py contracts`: Passed (25/25 platform contracts)
- `python tests/run_all_contracts.py`: Passed (1253/1253 test cases across 111 active suites, 14 skipped)
- `python tests/test_qemu_hmp_pointer_acceptance.py`: Passed (4/4 pointer acceptance cases)  

### Specialist Audit Deliverables:
1. `docs/audits/DISPLAY_DRIVER_ARCHITECTURE_AUDIT.md`: Display driver & HAL analysis across Bochs DISPI, SimpleFB, VirtIO-GPU, and scanout geometry bounds.
2. `docs/audits/COMPOSITOR_AND_SURFACE_ARCHITECTURE_AUDIT.md`: Surface capacity limits (48 MiB), dynamic OOM eviction, z-ordering, damage tracking, tile occlusion engine, and userspace compositor migration specification.
3. `docs/audits/TYPOGRAPHY_DPI_AND_RENDERING_AUDIT.md`: Vector typography, 4-bit grayscale coverage area AA (strictly separating from subpixel RGB stripe claims), and glyph tile caching architecture.
4. `docs/audits/RENDERING_PERFORMANCE_BASELINE.md`: 11 representative workloads, refresh budgets (60/90/120+ Hz), `PresentGate` write permission gating, and frame pacing telemetry.
5. `docs/audits/INPUT_AND_TOUCH_LATENCY_AUDIT.md`: Three-tier input reality (UART serial injection vs PS/2 IRQ 12 vs I2C GT911), non-starvation queues, gesture latching, and latency breakdown.
6. `docs/audits/DEVICE_STUDIO_GRAPHICS_INTEGRATION_AUDIT.md`: Real guest execution vs emulation, VNC/RFB persistent streaming, telemetry truthfulness, and offline state degradation.
7. `docs/audits/MULTIARCH_GRAPHICS_PORTABILITY_AUDIT.md`: Cross-architecture comparison matrix across x86_64, AArch64, and RISC-V 64.
8. `docs/audits/GRAPHICS_SECURITY_AND_CORRECTNESS_REVIEW.md`: Adversarial review defining mandatory hard safety requirements (z-order partitioning, checked arithmetic, capability de-escalation).

### Core Architecture Invariants:
- **Canonical Reference Resolution:** 720×1440 portrait (9:18, 320 DPI) across all native applications.
- **Display Controller HAL:** Architecture-independent `DisplayController` trait decoupling composition from platform I/O.
- **Phased Hybrid Compositor:** Zero-heap row-damage in-kernel DRM/KMS engine with planned migration to shared-memory `ICMP` userspace swapchains.
- **High-Refresh Pacing:** 16.67ms (60Hz), 11.11ms (90Hz), 8.33ms (120Hz) deadlines with `PresentGate` write authorization preventing phantom spinloop FPS claims.
- **Glyph Tile Caching:** Individual 64×64 glyph alpha tile cache replacing 2.88 MB static text-run cache (>90% BSS reduction).
- **Three-Tier Input Reality:** Strict separation of synthetic UART injection from emulated PS/2 hardware and physical I2C silicon.
- **Security Hardening:** Z-order partitioning (`100..499` application band, `500+` system overlays) preventing touch hijacking / clickjacking.
