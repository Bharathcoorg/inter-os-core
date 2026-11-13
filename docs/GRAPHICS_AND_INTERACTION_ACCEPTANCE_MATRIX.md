# InterOS Core — Native Graphics, High-Refresh Rendering, Performance & Smartphone Interaction Acceptance Matrix

**Document ID:** MATRIX-0024  
**Status:** Canonical Acceptance Baseline  
**Scope:** Display Drivers, Compositor, Typography, Performance, Input, Security, Multi-Architecture Parity  
**Target Repository:** InterOS Core (`master`)  
**Date:** November 2026  

---

## 1. Subsystem Acceptance Requirements & Status

| Area / Subsystem | Requirement / Invariant | Verification Method | Status | Notes / Citations |
| :--- | :--- | :--- | :--- | :--- |
| **1. Reference Resolution** | 720×1440 portrait (9:18 aspect ratio, 320 DPI) canonical scanout. | Static contracts & `test_display_contract.py` | **ACCEPTED** | Matches 17 native apps 1:1, prevents 1.5x fractional blur, fits in 48 MB surface limit. |
| **2. Display HAL** | Unified architecture-neutral `DisplayController` trait. | `docs/GRAPHICS_AND_INTERACTION_ARCHITECTURE.md` | **ACCEPTED** | Decouples x86 Port I/O, AArch64 DTB SimpleFB, and RISC-V PCIe ECAM MMIO. |
| **3. Hardware Page Flipping** | Double-buffered scanout via `DISPI_REG_Y_OFFSET` (0 or 1440). | Bochs DISPI driver audit & HAL interface | **ACCEPTED** | 8.3 MB VRAM mapping planned for zero-copy scanout flip. |
| **4. In-Kernel Compositor** | Zero-heap interactive composition, row-granular damage tracking. | `tests/test_surface_composite_contract.py` | **ACCEPTED** | `union_rows`, 64×64 tile occlusion culling, 48 MiB memory ceiling. |
| **5. Userspace Compositor Migration** | Standardized `ICMP` swapchain protocol and shared-memory mappings. | `sdk/rust/interos-std/src/lib.rs:39-89`, `kernel/src/compositor.rs` | **DESIGNED** | Phased migration path defined to shrink Ring 0 TCB. |
| **6. High-Refresh (60/90/120 Hz)** | Paced presentation against 16.67ms (60Hz), 11.11ms (90Hz), 8.33ms (120Hz) deadlines. | `kernel/src/drivers/display_kms.rs:127-172` (`FrameScheduler`) | **ACCEPTED** | `frame_times_ns[256]` circular buffer, missed-frame telemetry. |
| **7. "Fake FPS" Prevention** | `PresentGate` authorization (`writes > 0`), monotonic `LAST_FLIP_NUMBER`. | `apps/launcher/src/present.rs`, `test_launcher_present_truth_contract.py` | **ACCEPTED** | Loops with 0 pixel writes are rejected from flipping scanout. |
| **8. Typography & Antialiasing** | 4-bit grayscale coverage area AA (never claimed as subpixel AA). | `docs/audits/TYPOGRAPHY_DPI_AND_RENDERING_AUDIT.md` | **ACCEPTED** | Linear-light sRGB blending; subpixel RGB stripe claims strictly forbidden. |
| **9. Glyph Tile Cache** | Individual glyph alpha tile cache (64×64 tiles, ~256 KB BSS). | `docs/GRAPHICS_AND_INTERACTION_ARCHITECTURE.md` | **ACCEPTED** | Replaces 2.88 MB static text-run cache (>90% BSS reduction, eliminates 640px cap). |
| **10. Three-Tier Input Reality** | Strict separation: Tier A (UART), Tier B (PS/2 IRQ 12), Tier C (GT911 I2C). | `tests/test_qemu_pointer_input.py`, `emulator/server.py:5707` | **ACCEPTED** | Tagged with `delivery_method: serial_injection`, `hardware_touch: false`. |
| **11. PS/2 Mouse Interaction** | 3-byte packet decoding, 4x acceleration, clamped to 720×1440 portrait. | `kernel/src/drivers/ps2_mouse.rs`, `kernel/src/interrupts.rs:942` | **ACCEPTED** | Lock-free atomic circular queue drained in `sys_event_poll` and idle loop. |
| **12. HMP Pointer Acceptance** | Automated test injecting `mouse_move` and `mouse_button` over HMP monitor socket. | `tests/test_qemu_hmp_pointer_acceptance.py` | **ACCEPTED** | Verifies real guest UI reaction and captures verified screenshot. |
| **13. Z-Order Partitioning** | Applications clamped to `100..499`; `500+` reserved for system overlays. | `kernel/src/drivers/display_kms.rs`, `kernel/src/gui/window.rs` | **ACCEPTED** | Eliminates touch hijacking / clickjacking by untrusted applications. |
| **14. Surface OOM Safety** | Surface allocation beyond 48 MiB fails closed with `Err(OutOfMemory)`. | `kernel/src/syscall/mod.rs` | **ACCEPTED** | Deprecates ambient process kill in `sys_surface_create`. |
| **15. Multi-Arch Telemetry** | Ports: 4301/44445 (x86), 4302/44446 (ARM), 4303/44447 (RISC-V). | `emulator/server.py:1370-1420` | **ACCEPTED** | Dynamic monitor reconnect, serial reader epoch reset on switch. |
| **16. Telemetry Truthfulness** | Differentiates guest render cycles, transport FPS, and browser presentation FPS. | `emulator/app.js:798-824`, `test_studio_truth_contract.py` | **ACCEPTED** | No simulated 144Hz badges; 0 FPS displayed truthfully on static screens. |

---

## 2. Platform Verification & Evidence Matrix

| Verification Target | Test / Automation Command | Expected Outcome | Result |
| :--- | :--- | :--- | :--- |
| **1. Workspace Cargo Check** | `cargo check --workspace` | Zero errors across all crates | **Verified (0 errors, 0.12s)** |
| **2. Kernel Unit Tests** | `cargo test --manifest-path kernel/Cargo.toml` | 100% tests pass | **Verified (601 passed, 0 failed)** |
| **3. InterOS Std Unit Tests** | `cargo test --manifest-path sdk/rust/interos-std/Cargo.toml` | 100% tests pass | **Verified (459 passed, 0 failed)** |
| **4. Architecture Contracts** | `python tools/interos.py contracts` | All platform contracts pass | **Verified (25/25 passed)** |
| **5. Full Contract Suite** | `python tests/run_all_contracts.py` | 100% contracts pass | **Verified (1253/1253 passed, 111 suites)** |
| **6. Pointer Input Contract** | `python tests/test_qemu_pointer_input.py` | Real guest pointer contracts pass | **Verified (pass)** |
| **7. HMP Pointer Acceptance** | `python tests/test_qemu_hmp_pointer_acceptance.py` | 4/4 acceptance assertions pass | **Verified (4 passed, 0 failed)** |
| **8. x86_64 Boot Verification** | `python tests/verify_x86_boot.py` | Boot animation, service init, launcher alive | **Verified** |
| **9. AArch64 Boot Verification** | `python tests/verify_aarch64_boot.py` | EL0 user execution, service registry init | **Verified** |
| **10. RISC-V Boot Verification** | `python tests/verify_riscv_boot.py` | U-mode user execution, service registry init | **Verified** |

### Verified Implementation Commits

- `4b99e111`: `fix(display): actor-based z-order privilege and geometry bounds hardening`
- `297616e0`: `fix(emulator): truthfully degrade paused guest telemetry and add hmp pointer endpoint`
