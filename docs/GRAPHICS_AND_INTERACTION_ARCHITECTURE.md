# Architecture Decision Record: Native Graphics, High-Refresh Rendering, Performance & Smartphone Interaction

**Document ID:** ADR-0024  
**Title:** Native Graphics, High-Refresh Rendering, Performance & Smartphone Interaction Architecture  
**Status:** Accepted / Canonical  
**Target Repository:** InterOS Core (`master`)  
**Architectures:** `x86_64`, `AArch64`, `RISC-V 64`  
**Authors:** InterOS Engineering & Specialist Subagent Panel (Agents A–H)  
**Date:** November 2026  

---

## 1. Context and Problem Statement

InterOS Core has established a working multi-architecture microkernel and a bootable smartphone vertical slice on x86_64 QEMU (`-machine q35`), with verified U-mode/EL0 user execution across AArch64 and RISC-V 64. However, transforming this baseline into a production-grade, highly responsive smartphone operating system requires resolving several key architectural challenges:

1. **Display Hardware Diversity:** x86_64 uses Bochs DISPI over legacy Port I/O (`0x01CE`/`0x01CF`); AArch64 and RISC-V 64 use Bochs Display over PCIe MMIO (`BAR2 + 0x500`) or SimpleFB; VirtIO-GPU provides virtualized 2D acceleration. The codebase lacks a unified, architecture-neutral Display Controller Hardware Abstraction Layer (HAL).
2. **Compositor Placement & Privilege Trade-Off:** The current DRM/KMS compositor runs inside Ring 0 (`kernel/src/drivers/display_kms.rs`). While this initially solved tearing during early boot, it expands the kernel Trusted Computing Base (TCB) by over 3,800 lines of rasterization and alpha-blending code, holds kernel spinlocks across multi-megabyte memory copies, and requires granting `CapabilityType::FrameBufferDirect` to all client applications.
3. **High-Refresh Presentation (60/90/120+ FPS):** 60 FPS is the minimum sustained interactive target ($16.67\text{ ms}$ budget), with modern panels requiring 90 Hz ($11.11\text{ ms}$) and 120+ Hz ($8.33\text{ ms}$). Full-screen repaints at 720×1440 32bpp copy 4.15 MB per pass; with kernel and driver staging, internal memory traffic reaches 20–30 MB per frame (2.5–3.5 GB/s at 120 FPS). Without strict row-granular damage tracking and tile occlusion culling, software rendering under emulation collapses.
4. **Typography & Antialiasing Truth:** High-DPI mobile rendering requires crisp vector typography. The system currently uses an in-tree scalable outline rasterizer, but caches full 640×144 text runs in 32 static slots, consuming 2.88 MB of static BSS memory. Furthermore, the Platform Truth Contract mandates that the system never claim "subpixel RGB stripe antialiasing" when the algorithm is 4-bit (16-subscanline) grayscale area coverage antialiasing.
5. **Input Routing, Pointer Interaction & Latency:** Input arrives via three distinct tiers: Tier A (UART serial injection for test automation), Tier B (emulated i8042 PS/2 pointer for interactive QEMU sessions), and Tier C (I2C Goodix GT911 for physical mobile silicon). Latency jitter from deferred event draining and spinlock contention must be eliminated.
6. **Security & Privilege Boundaries:** The independent security audit revealed vulnerabilities including ambient capability leakage, unvalidated z-orders permitting touch hijacking (clickjacking), and denial-of-service task termination via surface memory exhaustion.

---

## 2. Decision Summary

```
+-------------------------------------------------------------------------------------------------------+
|                                    ARCHITECTURE DECISION SUMMARY                                      |
+--------------------------+----------------------------------------------------------------------------+
| Architectural Domain     | Canonical Decision & Architectural Invariant                               |
+--------------------------+----------------------------------------------------------------------------+
| 1. Scanout Geometry      | 720x1440 portrait (9:18, 320 DPI) canonical reference resolution.        |
|                          | Matches UI authoring space 1:1, eliminates 1.5x fractional blur, fits     |
|                          | strictly within the 48 MiB kernel surface memory budget.                   |
+--------------------------+----------------------------------------------------------------------------+
| 2. Display HAL           | Unified #![no_std] `DisplayController` trait. Decouples compositing from   |
|                          | x86 Port I/O, ARM DTB SimpleFB, and RISC-V PCIe ECAM MMIO.                 |
+--------------------------+----------------------------------------------------------------------------+
| 3. Compositor Strategy   | Phased Hybrid Model: Harden and retain the in-kernel DRM/KMS compositor    |
|                          | for the immediate vertical slice (zero heap, row-damage blits, 64x64 tile  |
|                          | occlusion); transition to a zero-copy userspace shared-memory compositor   |
|                          | via the standardized `ICMP` swapchain protocol.                            |
+--------------------------+----------------------------------------------------------------------------+
| 4. High-Refresh Pacing   | Synthetic millisecond tick deadline scheduling (60/90/120 Hz budgets).    |
|                          | Mandatory row-granular dirty banding and `PresentGate` write permissions   |
|                          | to prevent phantom spinloop FPS claims.                                    |
+--------------------------+----------------------------------------------------------------------------+
| 5. Typography Engine     | 4-bit grayscale coverage area antialiasing (never claimed as subpixel AA).|
|                          | Implement an individual glyph alpha tile cache (64x64 tiles, ~109 KB BSS), |
|                          | replacing the 2.88 MB static text-run cache (>90% BSS reduction).          |
+--------------------------+----------------------------------------------------------------------------+
| 6. Three-Tier Input      | Strict separation: Tier A (Serial UART), Tier B (PS/2 IRQ 12), Tier C     |
|                          | (Goodix GT911 I2C). Mandatory telemetry tagging: `serial_injection`,      |
|                          | `hardware_touch: false`. PS/2 queue drained in idle/poll loops.            |
+--------------------------+----------------------------------------------------------------------------+
| 7. Security Hardening    | Partition z-orders: `100..499` for client applications, `500+` for system  |
|                          | overlays (prevents touch hijacking). Surface OOM returns `Err(OutOfMemory)`|
|                          | rather than ambiently terminating background processes.                    |
+--------------------------+----------------------------------------------------------------------------+
```

---

## 3. Display Controller Hardware Abstraction Layer (HAL)

### 3.1 Architectural Principles
The display subsystem is decoupled from platform-specific bus and memory mechanisms. All display hardware implementations conform to `#![no_std]` interfaces:

```rust
// core-display-hal
#![no_std]
use core::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Argb8888,
    Bgrx8888,
    Rgb565,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub stride_bytes: u32,
    pub format: PixelFormat,
    pub refresh_hz: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayCapabilities {
    pub can_reprogram_mode: bool,
    pub hardware_page_flipping: bool,
    pub dirty_rect_flushes: bool,
    pub num_scanout_buffers: u8,
}

pub struct ScanoutCommit<'a> {
    pub buffer_id: u8,
    pub dirty_rows: Range<usize>,
    pub damage_rects: &'a [(u32, u32, u32, u32)],
}

pub trait DisplayController: Send + Sync {
    fn capabilities(&self) -> DisplayCapabilities;
    fn current_mode(&self) -> DisplayMode;
    fn supported_modes(&self) -> &[DisplayMode];
    fn set_mode(&mut self, mode: DisplayMode) -> Result<(), &'static str>;
    fn commit_scanout(&mut self, commit: ScanoutCommit<'_>) -> Result<u64, &'static str>;
}
```

### 3.2 Backend Driver Implementations
1. **`BochsDispiPioDriver` (x86_64):**
   - Accesses registers via Port I/O `0x01CE` (index) and `0x01CF` (data).
   - Linear framebuffer mapped at bootloader-provided physical address.
   - Tail mapping sized to 1013 pages (4,149,248 bytes) for 720×1440×32bpp portrait.
   - Double buffering supported by extending tail mapping to 2025 pages (8,294,400 bytes) and toggling `DISPI_REG_Y_OFFSET` (0 or 1440).
2. **`BochsDispiMmioDriver` (AArch64 & RISC-V 64):**
   - Probes PCI configuration space for vendor `0x1234`, device `0x1111`.
   - Accesses DISPI registers via 16-bit MMIO at `BAR2 + 0x500`.
   - Maps 16 MiB BAR0 framebuffer via architecture MMU:
     - AArch64: `DESC_ATTR_DEVICE` (MAIR Attr 1 = `Device-nGnRE`).
     - RISC-V 64: Sv39 2 MiB megapages governed by hardware Physical Memory Attributes (PMA).
3. **`VirtioGpuDriver` (Virtual Hardware 2D Acceleration):**
   - Implements VirtIO 1.0 GPU device specification over PCI/MMIO.
   - Supports 2D resource creation, backing store attachment, and dirty-band submission (`transfer_to_host_2d` + `resource_flush`).
   - Delegates scanout presentation to host GPU without MMIO trap overhead.
4. **`SimpleFbDriver` (Embedded / Physical Boards):**
   - Discovered from Device Tree (`/chosen/framebuffer`).
   - Static scanout geometry with software write-combining copy.

---

## 4. Compositor & Surface Architecture

### 4.1 Evaluation & Strategy: In-Kernel vs. Userspace System Service
The specialist audits evaluated two distinct models:
- **Model A (In-Kernel DRM/KMS Compositor):** Existing implementation. Fastest initial path, zero IPC overhead, but expands kernel TCB by ~3,800 lines and holds spinlocks during rendering.
- **Model B (Userspace Compositor Daemon):** Microkernel standard. Enforces least privilege, isolates faults to Ring 3, but introduces IPC synchronization requirements.

**Strategic Decision: Phased Hybrid Transition.**
1. **Phase 1 (Immediate Hardened Vertical Slice):** Retain the in-kernel DRM/KMS compositor in `kernel/src/drivers/display_kms.rs`. Harden all arithmetic against integer overflow, eliminate blocking spinlock copies, clamp client z-orders, and enforce strict capability checks.
2. **Phase 2 (Userspace Shared-Memory Migration):** Implement zero-copy userspace composition using the standardized `ICMP` protocol (`COMPOSITOR_MAGIC = *b"ICMP"`). Kernel transitions to a minimal DRM sync and page-flip broker, while `displayd` / `compositor` executes in Ring 3.

### 4.2 Surface Management & Memory Ceilings
- **Maximum Active Surfaces:** `MAX_CLIENT_SURFACES = 32`.
- **Surface Memory Ceiling:** `MAX_SURFACE_BYTES = 48 MiB` (out of 64 MiB total kernel heap).
- **Canonical Buffer Size:** $720 \times 1440 \times 4 = 4,147,200\text{ bytes}$ (~3.95 MiB).
- **Memory Safety & OOM Handling:**
  - If a surface allocation exceeds `MAX_SURFACE_BYTES`, the kernel **must fail closed** and return `Err(SyscallResult::OutOfMemory)`.
  - The ambient process termination loop in `sys_surface_create` is **strictly deprecated**. The kernel will not terminate background processes without an explicit, capability-governed request from an attested system supervisor.

### 4.3 Occlusion Culling & Damage Blitting
1. **64×64 Tile Occlusion Grid:**
   - Scanout is partitioned into a $12 \times 23$ grid of 64×64 pixel tiles (`MAX_TILES = 2048`).
   - Prior to rasterization, visible surfaces are traversed front-to-back (reverse z-order).
   - Fully opaque surfaces commit their covered tiles to `TileOcclusionMap`.
   - Subordinate surfaces fully occluded by committed tiles are flagged as `occluded = true` and skipped completely during composition.
2. **Row-Granular Damage Blitting:**
   - Surfaces track invalidated regions as half-open row intervals $[r_0, r_1)$.
   - `write_surface_pixels` computes exact modified rows: $r_0 = \frac{\text{offset}}{\text{width}}, r_1 = \frac{\text{offset} + \text{count} - 1}{\text{width}} + 1$.
   - Partial repaints (e.g. clock update: 64 rows, ~184 KB) reduce memory bus traffic by **90% to 97%** compared to full-frame repaints (1440 rows, 4.15 MB).
   - `diff_present_rows` executes vectorized 64-bit comparisons between front and back buffers to ensure only genuinely modified rows are committed to physical scanout.

---

## 5. High-Refresh (60/90/120+ FPS) Frame Scheduling & Latency Optimization

### 5.1 Refresh Budgets & Deadlines
Frame pacing is enforced against strict mathematical deadlines:

$$T_{\text{budget}} = \frac{1000}{f_{\text{target}}} \text{ ms}$$

- **60 Hz:** $16.667\text{ ms}$ budget (composition target $< 12.0\text{ ms}$).
- **90 Hz:** $11.111\text{ ms}$ budget (composition target $< 8.0\text{ ms}$).
- **120 Hz:** $8.333\text{ ms}$ budget (composition target $< 5.5\text{ ms}$).

### 5.2 Eliminating "Fake FPS" and Phantom Spinloops
To uphold the Platform Truth Contract, frame credits are strictly governed:
1. **`PresentGate` Authorization:** An application must record actual pixel modifications via `PresentGate::note_write(pixels)`. If an application loops without modifying pixels (`writes == 0`), `take_permission()` returns false and `SurfaceFlip` is rejected.
2. **Monotonic Hardware Flip Accounting:** `LAST_FLIP_NUMBER` in `kernel/src/performance.rs` increments **only** when `composite_and_flip()` successfully commits pixels to physical hardware scanout.
3. **Decoupling Guest Cycles from Host Browser Rates:** Telemetry explicitly reports guest execution cycles separately from host transport frame rates and browser canvas presentation FPS.

### 5.3 Input-to-Scanout Latency Minimization
1. **Non-Starvation Event Draining:** PS/2 mouse packets are queued into a 64-slot lock-free atomic circular buffer during hardware IRQ 12. Events are drained in non-interrupt context during `sys_event_poll`, `sys_yield`, and the Task 0 idle loop. Button Down and Up events are prioritized via compare-and-swap (CAS) loops to prevent touch dropouts.
2. **Directed Scheduler Handoff:** When an input event targets an application surface, the kernel transfers the remainder of the current scheduler quantum directly to the target task, reducing wake-up latency from milliseconds to microseconds.

---

## 6. Typography, Scalable Vector Outlines & DPI Scaling

### 6.1 Antialiasing Reality & Platform Truth
- **Algorithm:** 16-subscanline geometric area coverage antialiasing with linear-light sRGB gamma blending.
- **Platform Truth Mandate:** The typography engine operates as **4-bit grayscale coverage antialiasing**. It must **never** be claimed or documented as "subpixel antialiasing". Standard QEMU virtual displays have no fixed physical subpixel geometry, making RGB subpixel filtering inappropriate.

### 6.2 Glyph Alpha Tile Cache Architecture
The existing vector text engine allocates 32 static slots of 640×144 pixels, consuming 2.88 MB of static BSS. This architecture is replaced by an **Individual Glyph Alpha Tile Cache**:
- **Tile Geometry:** Individual glyph tiles bounded at $64 \times 64$ bytes (4,096 bytes per glyph).
- **Capacity:** 64 LRU glyph slots per type size.
- **Memory Consumption:**
  $$64 \text{ slots} \times 4,096 \text{ bytes} \approx 256 \text{ KiB static BSS}$$
- **Architectural Benefits:**
  1. Reduces static kernel/std BSS footprint by **>90%** (saving ~2.6 MiB).
  2. Eliminates the arbitrary 640-pixel text-run width limit, allowing arbitrary-length strings.
  3. Caches reusable letterforms across all text layouts, improving text rendering performance.

---

## 7. Security Model & Privilege Invariants

### 7.1 Strict Z-Order Partitioning (Clickjacking Prevention)
To prevent untrusted applications from stealing touch input or spoofing system dialogs, the window manager enforces strict z-order partitioning:
- **`0..99`:** System Desktop & Wallpaper.
- **`100..499`:** Client Application Surfaces. Application requests for arbitrary z-indices are automatically clamped within this band.
- **`500..899`:** Privileged System Overlays (Volume HUD, Dynamic Island, Notification Shade). Requires system authorization.
- **`900..1000`:** Security Dialogs, Lockscreen & System Alerts.

### 7.2 Capability De-Escalation
1. **Revoke Ambient `FrameBufferDirect`:** Client applications authoring surfaces are granted scoped `SurfaceHandle` tokens. Only attested display services (`displayd`, `compositor`) may hold `CapabilityType::FrameBufferDirect`.
2. **Gated Mode Setting:** Syscalls `sys_display_set_mode` and `sys_display_set_orientation` verify `CapabilityType::FrameBufferDirect` with `Rights::WRITE` and reject unprivileged client callers.
3. **Safe Memory Arithmetic:** All coordinate and offset calculations in `display_kms.rs` and `framebuffer.rs` use checked or saturating arithmetic. Negative coordinate offsets are strictly guarded against signed underflow before casting to `usize`.

---

## 8. Multi-Architecture Parity & Verification Standards

| Architectural Standard | x86_64 (`kernel/`) | AArch64 (`kernel-aarch64/`) | RISC-V 64 (`kernel-riscv64/`) |
| :--- | :--- | :--- | :--- |
| **Reference Resolution** | 720x1440 32bpp Portrait | 720x1440 32bpp Portrait | 720x1440 32bpp Portrait |
| **Hardware Driver** | Bochs DISPI Port I/O + VGA | Bochs Display PCIe MMIO | Bochs Display PCIe MMIO |
| **Compositor Engine** | DRM/KMS Multi-surface | Unified HAL SurfaceManager | Unified HAL SurfaceManager |
| **Hardware Page Flipping** | DISPI Y-Offset Reprogramming | Single-buffer / SHM blit | Single-buffer / SHM blit |
| **Interactive Test Source** | Tier A (UART) + Tier B (PS/2) | Tier A (UART Serial) | Tier A (UART Serial) |
| **Verification Gate** | Behavioural QEMU Integration | Behavioural QEMU Boot/EL0 | Behavioural QEMU Boot/U-Mode |

All performance benchmarks, contracts, and acceptance tests must execute against real virtual hardware in QEMU. String-scraping tests are deprecated in favor of verified behavioral integration suites.

---

## 9. Contradictions Resolution & Platform Truth Implementation

During the Native Graphics Implementation phase, four core architectural contradictions were systematically addressed and resolved:

1. **Surface Geometry Bounds (`MAX_SURFACE_PIXELS`):**  
   - *Previous state:* `MAX_SURFACE_PIXELS = 1280 * 720` (921,600 px), which was smaller than a full-screen 720×1440 portrait surface (1,036,800 px).  
   - *Resolution:* Expanded `MAX_SURFACE_PIXELS` to `1080 * 2400` (2,592,000 px) with symmetric aspect bounds (`width <= 2400 && height <= 2400`). Safely supports 720×1440, 1080×2160, and 1080×2400 mobile displays without integer truncation or allocation failures.
2. **Memory Ceiling & Jetsam Coexistence:**  
   - *Previous state:* 48 MiB aggregate surface allocation limit (`MAX_SURFACE_BYTES`) lacked explicit multi-arch parity.  
   - *Resolution:* Enforced the 48 MiB aggregate ceiling across all three architecture trees (x86_64, AArch64, RISC-V 64). Under memory pressure, the kernel activates jetsam eviction based on background dwell time, preventing kernel heap exhaustion.
3. **Userspace vs. Kernel Heap Truth Invariants:**  
   - *Previous state:* Ambiguity between zero-heap userspace guarantees and kernel surface buffer allocations.  
   - *Resolution:* Platform invariants explicitly codified: userspace interactive rendering and event loops in all 17 `#![no_std]` applications remain strictly zero-heap; the kernel KMS compositor manages bounded kernel heap surfaces protected by the 48 MiB ceiling and proactive memory reclamation.
4. **Caller Privilege Z-Order Authorization:**  
   - *Previous state:* In `set_surface_geometry_owned`, privilege clamping checked the surface's *current* `z_order` rather than the caller's authenticated identity (`owner`).  
   - *Resolution:* Implemented actor-based privilege verification (`DrmKmsController::is_trusted_system_owner(owner)`). Unprivileged applications are strictly constrained to the application band (`200..=899`), preventing clickjacking overlays and privilege escalation. System overlays (`>= 1000`) and desktop surfaces (`0..=199`) require authenticated system services.
