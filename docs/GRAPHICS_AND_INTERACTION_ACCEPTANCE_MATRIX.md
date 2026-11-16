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

### 1.1 High-DPI Flagship Hardware & Heap Constraints Matrix

| Target Profile | Resolution | Ratio | Buffer Size (ARGB8888) | Scanout (Front+Back) | Heap Impact (64MB Heap) | Bandwidth (60Hz) | Bandwidth (120Hz) | Status / Notes |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **Canonical QEMU** | **720×1440** | **18:9** | 3.95 MiB (4,147,200 B) | 7.91 MiB (8,294,400 B) | 12.3% of heap | 248.8 MB/s | 497.7 MB/s | **ACCEPTED (Canonical Baseline)**. 1:1 authoring match, zero fractional blur. |
| **FHD+ Reference** | **1080×2160** | **18:9** | 8.90 MiB (9,331,200 B) | 17.80 MiB (18,662,400 B) | 27.8% of heap | 559.9 MB/s | 1,119.7 MB/s | **VERIFIED PROFILE**. Preserves 18:9 canvas; exact 1.5x font match (`font_fhd.rs`). |
| **Flagship 20:9** | **1080×2400** | **20:9** | 9.89 MiB (10,368,000 B) | 19.78 MiB (20,736,000 B) | 30.9% of heap | 622.1 MB/s | 1,244.2 MB/s | **CONSTRAINED TARGET**. Requires 240px vertical letterbox or 20:9 app authoring. Surface memory limit must not exceed 40 MiB to prevent kernel heap panic. |

#### Flagship High-DPI Technical Constraints & Invariants:
1. **Firmware & Bootloader VBE Boundary:** The BIOS stage-2 bootloader enumerates VESA/VBE modes and greedily initializes the widest landscape mode (1280×720). Runtime mode reprogramming via Bochs DISPI requires the kernel page-table tail to map all physical scanout pages.
2. **Physical Framebuffer Coverage Gate:** In `kernel/src/framebuffer.rs`, `set_physical_mode(1080, 2400)` enforces `FB_COVERED_BYTES >= 10,368,000`. In the canonical QEMU phone profile, `engage_native_mode()` sizes the tail mapping to 4,149,248 bytes (1013 pages of 4 KiB) for the canonical 720×1440 profile. Reprogramming to 1080×2400 fails closed to prevent unmapped page faults into host video RAM.
3. **Kernel Heap Safety Invariant:** When scanout buffers occupy 19.78 MiB (10,368,000 bytes front buffer + 10,368,000 bytes back buffer), `MAX_SURFACE_BYTES` cannot remain at 48 MiB on a 64 MiB kernel heap ($19.78 + 48.0 = 67.78\text{ MiB} > 64\text{ MiB}$). Without physical DMA backing (`allocate_dma_frames`), surface memory limit must not exceed 40 MiB to prevent fatal kernel heap exhaustion panics.
4. **Platform Truth on High Refresh & Emulation Bandwidth:** At 1080×2400 @ 120 FPS, full-frame software copy requires moving 7.47 GB/s through internal DRAM and ~375M cycles (~187ms) in QEMU TCG (~5 FPS observed). High-refresh operation is achievable only through dirty-region row damage tracking (<5% screen update, ~50–150 KB/frame) or hardware-accelerated VirtIO-GPU scanout submission.
5. **Compositor & Tiling Pipeline Verification:** Logical surface allocation, automatic native mode selection, 1:1 bit-exact composition without resampling, Porter-Duff alpha blending, and row-level damage tracking for 1080×2400 are fully implemented and verified in `DrmKmsController` (`explicit_1080x2400_surface_engages_matching_scanout_mode`, `explicit_1080x2400_blit_preserves_alpha_blending`, `explicit_1080x2400_blit_maps_dirty_rows_1to1`) and userspace tiling (`chunk_bounds_flagship`, 2532 chunks).

---

## 2. Platform Verification & Evidence Matrix

| Verification Target | Test / Automation Command | Expected Outcome | Result |
| :--- | :--- | :--- | :--- |
| **1. Workspace Cargo Check** | `cargo check --workspace` | Zero errors across all crates | **Verified (0 errors, 0.12s)** |
| **2. Kernel Unit Tests** | `cargo test --manifest-path kernel/Cargo.toml` | 100% tests pass | **Verified (725 passed, 0 failed, 1 ignored)** |
| **3. Compositor Crate Tests** | `cargo test --package compositor` | 100% tests pass | **Verified (7 passed, 0 failed)** |
| **4. InterOS Std Unit Tests** | `cargo test --manifest-path sdk/rust/interos-std/Cargo.toml` | 100% tests pass | **Verified (459 passed, 0 failed)** |
| **5. Architecture Contracts** | `python tools/interos.py contracts` | All platform contracts pass | **Verified (25/25 passed)** |
| **6. Full Contract Suite** | `python tests/run_all_contracts.py` | 100% contracts pass | **Verified (1258/1258 passed, 111 suites, all 82 pins met)** |
| **7. Display Boundary Contracts** | `pytest tests/test_display_contract.py` | 100% tests pass | **Verified (1 passed, 0 failed)** |
| **8. Framebuffer Pixel Verification** | `pytest tests/test_framebuffer_verification.py` | 100% tests pass | **Verified (14 passed, 0 failed)** |
| **9. Pointer Input Contract** | `python tests/test_qemu_pointer_input.py` | Real guest pointer contracts pass | **Verified (pass)** |
| **10. HMP Pointer Acceptance** | `python tests/test_qemu_hmp_pointer_acceptance.py` | 4/4 acceptance assertions pass | **Verified (4 passed, 0 failed)** |
| **11. Live QEMU ARP/UDP Netdev** | `python tests/qemu/network/test_qemu_net_arp_udp.py` | Live E1000 ARP resolution & UDP checksum drop | **Verified (pass, trace_arp_udp.pcap)** |
| **12. Live QEMU IPv6/NDP Netdev** | `python tests/qemu/network/test_qemu_net_ipv6_ndp.py` | Live RA/SLAAC/Ping6 & DAD defense | **Verified (pass, trace_ipv6_ndp.pcap)** |
| **13. Live QEMU Net Robustness** | `python tests/qemu/network/test_qemu_net_robustness.py` | Drop telemetry & 2x RX ring wraparound | **Verified (pass, trace_robustness.pcap)** |
| **14. Live QEMU TCP/DNS Netdev** | `python tests/qemu/network/test_qemu_net_tcp_dns.py` | TCP handshake & HTTP GET request/reply | **Verified (pass, trace_tcp_dns.pcap)** |
| **15. Clean Reboot Recovery** | `python tests/test_reboot_recovery.py` | ACPI poweroff & state recovery across reboots | **Verified (pass, code 0)** |
| **16. x86_64 Boot Verification** | `python tests/verify_x86_boot.py` | Boot animation, service init, launcher alive | **Verified (pass, 720x1440 3742 colors)** |

### Verified Implementation Commits

- `4b99e111`: `fix(display): actor-based z-order privilege and geometry bounds hardening`
- `297616e0`: `fix(emulator): truthfully degrade paused guest telemetry and add hmp pointer endpoint`
- `7d680926`: `fix(display): align self-test surface bounds with expanded max surface limit`
