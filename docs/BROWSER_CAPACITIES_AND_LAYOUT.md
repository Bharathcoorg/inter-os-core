# Browser Capacities & CSS Layout Invariants

## 1. Engine Architecture & Memory Capacities

The InterOS native browser (`apps/browser/`) is a `#![no_std]` and `#![no_main]` userspace application executing with zero dynamic heap allocations in its interactive render and event loops. Memory is bounded by pre-allocated static arenas in `.bss`:

| Buffer / Limit | Current Size | File Location | Rationale & Invariant |
|---|---|---|---|
| `MAX_TOKENS` | 832 | `apps/browser/src/css.rs:63` | Stores `--var` CSS custom property definitions within `MAX_CSS_BYTES` (64 KiB). Bounds CSS token storage to prevent memory exhaustion on stylesheets with extensive variable cascades. |
| `MAX_RESPONSE_BYTES` | 256 KiB | `apps/browser/src/main.rs:68` | Static `RESPONSE_BUF` accumulating raw HTTP(S) responses from the network layer before extraction. |
| `MAX_HTML_BYTES` | 128 KiB | `apps/browser/src/lib.rs:2258` | Destination buffer for extracted HTML markup before DOM tokenization. |
| `MAX_READER_BYTES` | 64 KiB | `apps/browser/src/lib.rs:2267` | Formatted reader-mode document buffer. |
| `MAX_INLINE_IMAGES` | 8 | `apps/browser/src/images.rs:45` | Maximum number of concurrent inline decoded images per page within the shared static `IMAGE_ARENA`. |
| `MAX_IMAGE_BODY_BYTES`| 224 KiB | `apps/browser/src/images.rs:31` | Maximum raw byte size of an admitted image file. Oversized images are refused before allocation. |

## 2. CSS Selectors & Specificity
- **Supported Selectors**: Type selectors (`tag`), Class selectors (`.class`), ID selectors (`#id`), and descendant combinations (`div p`, `#main .card`).
- **Specificity Calculation**:
  - ID selector: 100 points
  - Class selector: 10 points
  - Type selector: 1 point
- **Compound Constraints**:
  - In `apps/browser/src/css.rs:806`, compound selectors are currently bounded to a single class per compound (`tag.class#id`). Selectors with multiple consecutive class constraints (e.g. `.btn.btn-primary`) are rejected by `parse_compound`.

## 3. CSS Positioning & Layout Flow
- **`Position::Static`**: Normal document flow.
- **`Position::Relative`**: Offsets visual bounding box by `(pos_offset_x, pos_offset_y)` while preserving the element's original space in vertical flow.
- **`Position::Absolute`**: Positioned relative to containing block; removed from normal flow calculation.

## 4. Platform Truth Contract on Layout Capabilities
- As verified by `apps/browser/src/engine/mod.rs:native_flow_capabilities_match_contract`, `NativeFlowEngine` explicitly declares:
  - `supports_css_flexbox: false`
  - `supports_css_grid: false`
  - `supports_full_js: false`
  - `supports_cookies: true`
  - `supports_streaming_render: true`
- In accordance with the Platform Truth Contract, native flow does not pretend to support full modern flexbox or CSS grid without an upstream layout engine (such as Servo in `interos-apps`).
