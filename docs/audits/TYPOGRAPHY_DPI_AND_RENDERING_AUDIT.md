# Technical Audit Report: Typography, DPI Scaling, and Graphical Quality across InterOS Core

**Auditor:** Agent C (Typography, DPI, and Rendering Quality Specialist)  
**Target Architecture:** InterOS Core (x86_64 `#![no_std]`, `#![no_main]`, Zero-Heap Userspace)  
**Status:** COMPLETE & SOURCE-VERIFIED  
**Date:** 2026-11-13  

---

## Executive Summary

An exhaustive, independent audit of typography, vector rasterization, DPI scaling, and graphical quality was conducted across `sdk/rust/interos-std`, `kernel/src/drivers/display_kms.rs`, the 17 userspace applications in `apps/`, and the contract test suites in `tests/`.

### Key Findings:
1. **Typography Dual-Engine Architecture**: InterOS possesses **both** a legacy/fallback bitmap font engine (`sdk/rust/interos-std/src/font.rs` and `font_fhd.rs`) and a fully functional in-tree scalable vector outline rasterizer (`sdk/rust/interos-std/src/design/face.rs`, `typeface.rs`, and `outline_text.rs`).
2. **Platform Truth on Antialiasing**: InterOS **does not perform subpixel rendering (RGB stripe filtering)**. All antialiasing is strictly **grayscale coverage AA**: 4-bit packed coverage (16 levels) for bitmap fonts, and 16-subscanline geometric area coverage (0..=256 levels) with perceptual gamma correction and linear-light sRGB blending for vector outlines.
3. **Scanout & Resolution Alignment**: The canonical display scanout is locked at **720x1440 portrait** (320 DPI, 1.0x scale factor). Logical UI space is identical to physical scanout (`LOGICAL_WIDTH == SCANOUT_WIDTH = 720`, `LOGICAL_HEIGHT == SCANOUT_HEIGHT = 1440`). This 1:1 match makes compositor presentation a zero-resample memory copy, completely eliminating the blurry 1.5x scaling defects caused by the retired 1080x2160 scanout.
4. **In-Tree Vector Rasterizer Feasibility**: A lightweight quadratic Bezier vector font rasterizer is **already implemented in-tree** without external dependencies. However, its static run cache consumes **~2.88 MB of static BSS memory** per process (`COVERAGE: [[u8; 640 * 144]; 32]`) and falls back to bitmap fonts when strings exceed 640px or 128 bytes.
5. **Asset & Icon Architecture**: Iconography is almost entirely procedural vector geometry (24-unit normalized grid in `monochrome.rs` and 256-unit supersampled fixed-point vector silhouettes in `launcher/src/icons.rs`), avoiding bitmap scaling artifacts.

---

## 1. Current Typography Implementation

### 1.1 Font Formats & Storage
InterOS Core incorporates three distinct font representations:

1. **Scalable Vector Outline Table (`sdk/rust/interos-std/src/design/face.rs`)**:
   - **Origin**: Generated from `RobotoCondensed-Regular.ttf` using `tools/gen_typeface.py`.
   - **Metrics**: Defined with `UNITS_PER_EM = 2048`, `ASCENT = 2146`, `DESCENT = -555`. Cap height in font units is 1456 (`1456 / 2048 = 71.1%` of em).
   - **Format**: 95 printable ASCII glyphs (`0x20` to `0x7E`).
   - **Storage**: Compact binary curve bytecode occupying only **9,507 bytes** in `OUTLINE_BYTES`, indexed by `OUTLINE_OFFSET: [u16; 96]`.
   - **Structure**: Each glyph encodes `contours: u8`, `points: u8`, 1-byte tags (marking on-curve vs off-curve quadratic Bezier control points), and 16-bit delta-encoded coordinates.
   - **Advances**: `ADVANCE: [u16; 95]` stores exact horizontal advance widths in 2048 units/em space. Digits `0`..=`9` are strictly tabular (each exactly 1011 units wide).

2. **Default 16x24 Condensed Bitmap Font (`sdk/rust/interos-std/src/font.rs`)**:
   - **Metrics**: 16x24 cell size, 10px wide glyph body, 16px tabular digit advance.
   - **1-bit binary tables**: `FONT` (2,280 `u16` words) and `FONT_BOLD` (2,280 `u16` words).
   - **4-bit antialiased coverage tables**: `FONT_COV` and `FONT_COV_BOLD` (95 glyphs * 24 rows * 5 `u8` bytes = 11,400 bytes each). Packs two 4-bit nibbles per byte, providing 16 alpha coverage levels (0..=15).

3. **Reference FHD 24x36 Bitmap Font (`sdk/rust/interos-std/src/font_fhd.rs`)**:
   - Sized for 1080x2160 displays (24x36 cell, 17px tabular digit body).
   - Contains 1-bit `FONT_FHD` (3,420 `u32` words) and 4-bit coverage `FONT_FHD_COV` (95 * 36 * 9 bytes = 30,780 bytes).
   - *Status*: Explicitly marked as non-guest scanout reference data; not linked into active app scanout paths.

---

### 1.2 Glyphs Rasterization & Rendering Pipeline

#### A. Scalable Vector Outline Rasterizer (`sdk/rust/interos-std/src/design/typeface.rs`)
The vector rasterizer operates entirely in `#![no_std]`, zero-heap userspace:
- **Arithmetic**: 16.16 fixed-point signed arithmetic (`i32`).
- **Curve Decomposition**: Quadratic Beziers $B(t) = (1-t)^2 P_0 + 2(1-t)t P_1 + t^2 P_2$ are flattened into straight line segments using forward differencing with a fixed 8-step subdivision (`CURVE_STEPS = 8`).
- **Sub-Scanline Supersampling**: Each horizontal pixel row is evaluated at **16 sub-scanlines** (`SUBSAMPLES = 16`), positioned at $y + \frac{2s + 1}{32}$ for $s \in [0..15]$.
- **Winding & Edge Crossings**: Uses non-zero winding rules. Edge intersections are recorded in a fixed static buffer (`CROSSINGS: [Crossing; 32]`). Segments are sorted by $x$-coordinate, and spans between winding transitions are integrated across the 16 sub-scanlines.
- **Perceptual Alpha Correction**: The accumulated coverage in $0..256$ is transformed via `perceptual_alpha(c)`:
  $$\text{perceptual\_alpha}(c) = \frac{c \cdot (510 - c)}{255}$$
  This quadratic expansion boosts mid-tone coverage to compensate for physical sub-pixel spread and optical thinning on high-density displays.
- **Linear-Light sRGB Blending**: Coverage is blended using `font::blend(fg, bg, alpha)`:
  - Decomposes colors into sRGB channels.
  - Converts sRGB to linear light via 256-entry lookup table `SRGB_TO_LIN`.
  - Performs linear interpolation: $C_{\text{lin}} = \frac{C_{\text{fg,lin}} \cdot \alpha + C_{\text{bg,lin}} \cdot (255 - \alpha)}{255}$.
  - Re-encodes to sRGB via `LIN_TO_SRGB`.
  - Avoids dark fringing and gamma-distortion artifacts.

#### B. Bitmap Rasterizer Fallback (`sdk/rust/interos-std/src/font.rs`)
- When outline rasterization is bypassed or falls back, `test_glyph_aa` extracts the 4-bit coverage nibble (0..=15), scales it to 8-bit alpha via `nibble * 17` (mapping 0..=15 to 0..=255), and blends it into the surface using linear-light blending.

---

### 1.3 Glyph Caching Hierarchy
Vector rasterization of quadratic curves on every scanline is computationally prohibitive for 60 FPS interactive UI. InterOS implements a multi-tiered cache hierarchy in `sdk/rust/interos-std/src/design/outline_text.rs` and `typeface.rs`:

1. **Advance Width Cache (`ADVANCES: [AdvSlot; 32]`)**:
   - Caches precalculated scaled advances for glyphs at active em sizes.
2. **Bounding Box Cache (`BOXES: [BoxSlot; 32]`)**:
   - LRU cache storing bounding dimensions `(width, height)` for text runs up to 128 bytes at specific cap heights.
   - Prevents re-rasterization during UI layout passes.
3. **Run Shape Cache (`SHAPES: [ShapeSlot; 32]`)**:
   - Caches advance width and ink bounding box of text runs.
4. **Last Placed Run Memo (`LAST_PLACED: [LastPlaced; 32]`)**:
   - Fast direct-mapped cache accelerating pixel lookups for consecutive pixels within the same run.
5. **Static Coverage Buffer Pool (`COVERAGE: [[u8; 640 * 144]; 32]`)**:
   - Allocates 32 static alpha coverage planes, each up to $640 \times 144$ pixels.
   - **Static Footprint**: $32 \times 92,160\text{ bytes} \approx \mathbf{2.88\text{ MB}}$ in process BSS!
   - Whole runs are rasterized once into a coverage buffer slot and timestamped with monotonic `CLOCK`. Subsequent pixel queries in `place_run_cap` and `monochrome::text_px` perform $O(1)$ memory reads from this buffer.
   - **Fallback Gate**: If a run's bounding box exceeds 640px in width, 144px in height, or contains non-ASCII characters, it is rejected and falls back to bitmap rendering.

---

### 1.4 Text Metrics Calculation
Metrics are derived from the cap-height ladder pinned by `tests/test_type_scale_contract.py`:

| Scale Rung | Role | Cap Height (`cap_px`) | Derived Em (`em_px`) | Line Box Height (`line_px`) | Line Box Ratio |
|:---:|:---:|:---:|:---:|:---:|:---:|
| 1 | `CAPTION` | 16 px | 23 px | 24 px | 1.50x |
| 2 | `BODY` | 21 px | 30 px | 32 px | 1.52x |
| 3 | `SECTION` | 27 px | 38 px | 40 px | 1.48x |
| 4 | `TITLE` | 35 px | 49 px | 48 px | 1.37x |
| 5 | `HEADING` | 44 px | 62 px | 60 px | 1.36x |
| 6 | `DISPLAY` | 56 px | 79 px | 76 px | 1.36x |
| 7 | `HERO` (Clock) | 76 px | 107 px | 100 px | 1.32x |

- **Em Conversion**: `em = (cap_px * 2048 + 728) / 1456`.
- **Baseline Ascent**: `ascent = (em * 2146 + 1024) / 2048`.
- **Baseline Descent**: `descent = (em * 555 + 1024) / 2048`.
- **Kerning / Tracking**: Kerning pair tables are omitted to preserve ROM space. Glyphs advance strictly by their proportional advance widths. Tabular numbers share an invariant advance width across all digits.
- **Dynamic Type Scaling**: Accessibility scaling is controlled via `UserTextScale`:
  - `Compact` (0): 80% of canonical cap height (floor bounded to 10px minimum).
  - `Normal` (1): 100% of canonical cap height.
  - `Large` (2): 125% of canonical cap height.
  - `ExtraLarge` (3): 150% of canonical cap height.
  - Symmetrically scales cap height, line height, ascent, and measurement calculations.

---

### 1.5 Antialiasing Evaluation & Platform Truth
- **Platform Truth Contract Compliance**: The platform **never** performs subpixel RGB striping. Claiming "subpixel antialiasing" would violate the Platform Truth Contract, as InterOS makes no assumptions regarding physical LCD subpixel geometries (RGB, BGR, PenTile, or OLED diamond layouts) and performs no colored subpixel fringing.
- **Actual Antialiasing Technique**:
  - Bitmap fonts use **4-bit grayscale coverage AA** (16 uniform alpha levels).
  - Outline vector fonts use **16-subscanline geometric area coverage AA** (0..=256 levels), corrected by non-linear optical weighting and composited via linear-light sRGB LUTs.
  - This delivers pristine edge smoothness on high-density displays without colored chromatic aberration.

---

## 2. Resolution & Coordinate Model

### 2.1 Physical Display vs. Logical Space
- **Reference Scanout Resolution**: 720x1440 portrait (9:18 aspect ratio).
- **Logical Canvas**:
  ```rust
  pub const LOGICAL_WIDTH: usize = 720;
  pub const LOGICAL_HEIGHT: usize = 1440;
  pub const SCANOUT_WIDTH: usize = LOGICAL_WIDTH;
  pub const SCANOUT_HEIGHT: usize = LOGICAL_HEIGHT;
  ```
- **Coordinate Mapping**: Because logical coordinates match physical scanout pixels exactly:
  - Scaling numerator and denominator are both 720 (`SCALE_NUM = 720`, `SCALE_DEN = 720`).
  - Scanout composition in `kernel/src/drivers/display_kms.rs` (`DRM_KMS`) is a **direct 1:1 memory copy**.
  - No fractional coordinate rounding or interpolation occurs during presentation.

### 2.2 Device-Independent Scaling (dp/sp)
In `sdk/rust/interos-std/src/design/mod.rs`, `DisplayMetrics` establishes the density model:
- **Baseline Metrics**:
  - `density_dpi`: 320 DPI (XHDPI density class).
  - `scale_factor_permille`: 1000 (1.0x baseline).
  - Conversion formulae:
    $$\text{px} = \frac{\text{dp} \cdot \text{scale\_factor\_permille} + 500}{1000}$$
    $$\text{dp} = \frac{\text{px} \cdot 1000 + 500}{\text{scale\_factor\_permille}}$$
- **Hairline Guarantee**:
  - `DisplayMetrics::hairline_px()` strictly returns `1`. A hairline is guaranteed to be exactly 1 physical scanout pixel, preventing stroke dropout on high-density displays.
- **Targeting Higher Resolutions (1080p / 1440p)**:
  - On a 1080x2160 panel (480 DPI, XXHDPI), `scale_factor_permille` becomes 1500 (1.5x).
  - On a 1440x2880 panel (640 DPI, XXXHDPI), `scale_factor_permille` becomes 2000 (2.0x).

### 2.3 Touch Coordinate Normalization & Hit Targets
- **Input Pipeline**: Touch events arriving via VirtIO Tablet/Input are handled by `viewport::touch_to_logical(px, py)`:
  1. Clamps coordinates to $[0, \text{SCANOUT\_WIDTH} - 1]$ and $[0, \text{SCANOUT\_HEIGHT} - 1]$.
  2. Applies `surface_to_logical` scaling. Under 1:1 scanout, this is an exact identity mapping with zero latency and zero floating-point overhead.
- **Hit Target Contracts (`tests/test_ui_design_goal_contract.py`)**:
  - `MIN_TARGET`: 44 dp / 44 px (strict minimum for touch interaction across all apps).
  - `PREFERRED_TARGET`: 48 dp / 48 px.
  - `PRIMARY_ACTION`: 56 dp / 56 px (e.g. Floating Action Buttons).
  - Contract verification confirms `UNDERSIZED_TARGETS` is empty across all 17 applications in `apps/`.

---

## 3. Graphics & Asset Quality

### 3.1 Icon Rendering Architecture
InterOS strictly prohibits embedded raster bitmaps (PNGs/JPEGs) for system icons, ensuring sharp rendering at any scale:
1. **Design System Monochrome Kit (`interos-std/src/design/monochrome.rs`)**:
   - Implements `icon_px` and `icon_hit` over a normalized **24x24 coordinate grid**.
   - Primitive shapes (lines, circles, dots, rounded caps) are rendered with anti-aliasing directly into target rectangles.
   - Covers core navigation and status glyphs: Back, Forward, Menu, Search, Close, Check, Wifi, Battery, Volume, Mic, Audio, Play, Pause, Send, etc.
2. **Launcher High-Definition Vector Pictograms (`apps/launcher/src/icons.rs`)**:
   - Procedural vector engine evaluating shapes inside normalized fixed-point space `[-256..=256]`.
   - Implements **9x subpixel supersampling** (`rasterize_icon_pixel_scaled_aa`):
     - Uses a $3 \times 3$ grid with offsets $[-1, 0, 1] / 3$.
     - Evaluates analytical curve functions (`eval_icon_subpixel`).
     - Averages hits and blends with the background using linear-light color blending.

### 3.2 Image Scaling and Filtering
- **Wallpapers & Image Buffers (`interos-std/src/design/walls.rs`)**:
  - `raw_wallpaper_pixel_scaled` uses 16.16 fixed-point nearest-neighbor sampling with edge antialiasing for procedural gradient bands.
- **Display List Blits (`interos-std/src/design/display_list.rs`)**:
  - Surfaces and blit commands currently rely on integer decimation/replication.
- **SVG Subset Vector Rendering (`interos-std/src/image/svg.rs`)**:
  - In-tree bounded SVG renderer (`#![no_std]`, no `alloc`, zero heap).
  - Rasterizes lines, cubic/quadratic Beziers, arcs, fills, and strokes with 16 sub-scanlines.
  - Rejects external links, JavaScript, CSS filters, masks, and text elements to prevent security vulnerabilities and out-of-memory panics.

### 3.3 Root Cause Analysis: The Retired 1080x2160 Blur Defect
During earlier development iterations, the DRM/KMS scanout was configured for 1080x2160 while platform applications authored UI surfaces at 720x1440.

**Why this caused severe blur**:
1. **Fractional Resampling (1.5x Ratio)**:
   $$\frac{1080}{720} = 1.5, \quad \frac{2160}{1440} = 1.5$$
   Every single logical pixel was mapped to 1.5 physical scanout pixels.
2. **Nearest-Neighbor Failure**: Alternated between 1-pixel and 2-pixel physical widths, causing severe harmonic aliasing, jagged diagonal lines, and phase shimmer during scrolling.
3. **Bilinear Interpolation Failure**: A sharp 1-pixel high-contrast black line on white ($0 \to 255$) was filtered across 2 physical pixels with weights (e.g., 50%/50% or 33%/67%), transforming crisp edges into soft 2-pixel gray ramps. Contrast dropped by up to 50%, producing noticeable blur across all text, borders, and icons.
4. **Resolution**: By pinning `DRM_KMS` scanout to 720x1440 to match `LOGICAL_WIDTH` and `LOGICAL_HEIGHT`, composition became a direct 1:1 memory blit, restoring crisp rendering.

---

## 4. Feasibility of In-Tree Lightweight Font Rasterizer

### 4.1 Evaluation of the Current Engine
The objective of having an in-tree, lightweight, `#![no_std]` vector font rasterizer without large third-party crates is **already achieved in InterOS Core**.
- `sdk/rust/interos-std/src/design/face.rs` + `typeface.rs` + `outline_text.rs` provides full quadratic Bezier curve evaluation, 16-subscanline antialiasing, and caching with zero heap allocations.

### 4.2 Limitations & Critical Bottlenecks

| Feature / Limit | Current Status | Impact / Bottleneck |
|:---|:---|:---|
| **Static Memory Usage** | `COVERAGE: [[u8; 640*144]; 32]` | **2.88 MB BSS overhead** per process; severe for embedded memory budgets. |
| **Max Text Run Width** | Capped at 640 px | Text runs wider than 640px fall back to bitmap rendering. |
| **Max Text Run Length** | Capped at 128 bytes | Long strings fall back to bitmap rendering. |
| **Codepoint Coverage** | 95 ASCII glyphs (32..=126) | Non-ASCII, accented Latin, CJK, and emoji trigger bitmap fallback. |
| **Kerning Support** | None (advance widths only) | Display headers lack optical kerning pairs (e.g. "To", "Wa", "AV"). |
| **Small Size Grid Fitting**| No stem hinting / grid alignment | At small cap heights (16px), vertical stems can appear unevenly weighted depending on sub-pixel phase. |

---

## 5. Architectural Proposals for Multi-Density Crisp Rendering

To achieve crisp, native-resolution rendering across multiple densities (720p, 1080p, 1440p) while resolving memory overhead, the following architecture is proposed:

### Proposal 1: Dynamic Native Density Architecture (Zero Compositor Resampling)
- **Principle**: The DRM/KMS scanout matches physical hardware native resolution, and applications author UI directly at the native density.
- **Mechanism**:
  - At startup, the app receives `DisplayMetrics` from the display service via IPC:
    - 720x1440 $\to$ `scale_factor_permille = 1000` (1.0x)
    - 1080x2160 $\to$ `scale_factor_permille = 1500` (1.5x)
    - 1440x2880 $\to$ `scale_factor_permille = 2000` (2.0x)
  - UI layouts continue using `dp` tokens. During layout resolution, `dp` values are converted to integer physical pixels via `dp_to_px()`.
  - Fonts scale smoothly: `cap_px = (LADDER_CAPS[rung] * scale_factor_permille) / 1000`. Vector outlines are rasterized directly at target pixel sizes, completely avoiding compositor resampling blur.

### Proposal 2: Glyph-Level Alpha Mask Tile Cache (Slashing 2.88 MB BSS to < 256 KB)
- **Current Problem**: Caching full 640x144 placed text runs wastes massive amounts of memory on whitespace and duplicate text runs.
- **Solution**:
  - Replace run-level caching with an individual **Glyph Alpha Tile Cache**.
  - For 95 ASCII glyphs at active type rungs (e.g. Caption 16px, Body 21px, Title 35px), cache individual glyph coverage bitmaps (e.g., $24 \times 32$ or $36 \times 48$ bounding boxes).
  - 95 glyphs $\times$ 1,152 bytes $\approx$ **109 KB** per active rung.
  - Text placement simply blits pre-rasterized glyph alpha masks onto the surface with linear-light blending.
  - **Benefits**:
    - Reduces BSS memory consumption from **2.88 MB down to ~256 KB** (>90% savings).
    - Completely removes the 640px run width and 128-byte string length limits.

### Proposal 3: Lightweight Integer Stem Darkening & Grid Snapping
- **Problem**: At small sizes (16px and 21px cap heights), vector outlines without hinting can suffer from thin stems or asymmetric character strokes due to fractional phase alignment.
- **Solution**:
  - Incorporate a lightweight vertical-stem snapping rule during curve decomposition: snap vertical segment $x$-coordinates to the nearest $\frac{1}{4}$ pixel.
  - Apply 1-subscanline stem darkening for small text on dark backgrounds to ensure legibility and prevent hairline stroke fading.

### Proposal 4: Retirement of Separate Bitmap Fonts in UI Paths
- Relegate `font.rs` (16x24 bitmap) strictly to kernel panic, crash recovery, and early boot screens where memory and complex state are inaccessible.
- Remove `font_fhd.rs` from userspace linking to reclaim ROM space.
- Standardize all app typography on the scalable vector engine.

### Proposal 5: Verified Contrast Enforcement
- Maintain the WCAG AA contrast contract verified in `tests/test_ui_design_goal_contract.py`:
  - Primary text on surface ($4.5:1$ minimum, currently $5.11:1$ for `#6B6B6B` and $>15:1$ for `#1A1A1A`).
  - Strict contract tests ensuring no app requests unscaled rungs or non-ladder cap heights.

---

## Summary Matrix of Audited Files

| File | Component | Architecture & Status |
|:---|:---|:---|
| `sdk/rust/interos-std/src/font.rs` | Default Bitmap Font | 16x24 bitmap; 4-bit packed coverage (16 alpha levels); delegates AA text to `monochrome::text_px`. |
| `sdk/rust/interos-std/src/font_fhd.rs` | Reference FHD Font | 24x36 bitmap; 4-bit coverage; marked non-guest reference. |
| `sdk/rust/interos-std/src/design/face.rs` | Scalable Vector Outline Table | 9,507 bytes quadratic Bezier curve table; 2048 upem; 95 ASCII glyphs; tabular digits. |
| `sdk/rust/interos-std/src/design/typeface.rs` | In-Tree Vector Rasterizer | 16.16 fixed point; 16 sub-scanlines; 8-step Bezier decomposition; non-zero winding; zero heap. |
| `sdk/rust/interos-std/src/design/outline_text.rs` | Run Caching System | Multi-tiered LRU caching; 32 static coverage slots (2.88 MB BSS); 640px width limit. |
| `sdk/rust/interos-std/src/design/monochrome.rs` | Design System Kit | 24-unit normalized vector icons; rounded rects; linear-light sRGB blending; cap ladder mapping. |
| `sdk/rust/interos-std/src/image/svg.rs` | Vector Asset Engine | In-tree `#![no_std]` SVG subset rasterizer; 16 sub-scanlines; safe bounds enforcement. |
| `apps/launcher/src/icons.rs` | Launcher Icon Engine | Procedural vector silhouettes in 256-space; 9x subpixel supersampling AA. |
| `kernel/src/drivers/display_kms.rs` | DRM/KMS Controller | Pinned 720x1440 portrait scanout; 1:1 scanout composition copy avoiding 1.5x blur. |
| `tests/test_type_scale_contract.py` | Type Scale Contract | Pinned cap heights (16, 21, 27, 35, 44, 56, 76); guarantees both faces match ladder. |
| `tests/test_ui_design_goal_contract.py` | UI Design System Contract | Touch targets $\ge 44\text{ px}$; canonical $720 \times 1440$ coordinate bounds. |
