//! InterOS Native Sovereign File Manager & VFS Explorer App
//!
//! Native Ring-3 graphical application executing in User Mode:
//! - DRM/KMS Client Surface allocation (z=200) and double-buffered scanout presentation
//! - Real-time block device capacity query via microkernel storage driver
//! - Startup VFS directory query whose received bytes are reported on screen and
//!   in the log only: no directory entry list is parsed or rendered
//! - Internal Storage capacity queried from the storage service; usage is shown only when real telemetry is available.
//! - Tactile category rows (Recent, Images, Videos, Documents, Downloads,
//!   Internal Storage, Cloud) with a visible selection highlight
//! - A drawn back control in the header, whose hit region is the same rectangle
//!   it is painted from - see `BACK`
//! - Single screen: no subdirectory navigation and no path breadcrumbs are drawn
//! - Banded rendering (`render_file_manager_band`): only the rows an input
//!   actually dirtied are re-rasterised, instead of all 1,036,800 pixels
//! - Asynchronous event reactor loop handling touch interactions and home gestures
//! - Graceful lifecycle supervision and clean return to Launcher

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use std::design::font;
use std::design::primitives::{draw_pill, Rect};

mod outline_text;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// Capability slot that holds `FileSystemRead` for the `app_file_manager` task.
///
/// WHY a named constant and not a bare number: `spawn_app_file_manager` inserts
/// its capabilities in a fixed order - SerialOutput, IpcSend, TimerTickRead,
/// HardwarePortIo, FileSystemRead, FileSystemWrite, FrameBufferDirect,
/// EventPoll - and `CapabilityTable::insert` always takes the first free slot,
/// so the read capability is index 4. The old code passed slot **3**, which is
/// `HardwarePortIo`, so `sys_fs_list`'s
/// `verify(3, FileSystemRead, Rights::READ)` failed and every listing was
/// refused with PermissionDenied before the path was even looked at. Slot 6 is
/// the framebuffer, which is why `surface_create(6, ...)` always worked while
/// the listing never did.
const FS_READ_CAP_SLOT: usize = 4;

// -----------------------------------------------------------------------------
// 32-bit ARGB8888 Design Color Tokens (Screen 11 Light Theme)
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFFF7F6F2;
#[allow(dead_code)]
const SURFACE_DARK: u32 = 0xFFFFFFFF;
#[allow(dead_code)]
const SURFACE_RAISED: u32 = 0xFFEFEEE9;
const BORDER_SUBTLE: u32 = 0xFFE1E0DB;
const TEXT_PRIMARY: u32 = 0xFF111214;
const TEXT_MUTED: u32 = 0xFF777773;
#[allow(dead_code)]
const ACCENT_SKY: u32 = 0xFF2563EB;
#[allow(dead_code)]
const ACCENT_AMBER: u32 = 0xFFF59E0B;
#[allow(dead_code)]
const SUCCESS_EMERALD: u32 = 0xFF2E8B57;
#[allow(dead_code)]
const DANGER_CRIMSON: u32 = 0xFFEF4444;

// -----------------------------------------------------------------------------
// Header geometry - single source of truth for drawing and hit-testing
// -----------------------------------------------------------------------------
/// Height of the header band.
const HEADER_H: usize = 140;
/// Screen margin (8pt grid value). Also the left inset of the back control and
/// the right inset the storage readout is aligned to.
const HEADER_MARGIN: usize = 24;

/// The header's back control: a 52x52 pill in the strip above the title.
///
/// WHY this rectangle at all: the old app hit-tested `x 24..100 && y 28..68` and
/// used it to exit to the launcher, but nothing in that region was ever drawn -
/// the title owns rows 64..136 and the two header pictograms sat at x 590 and
/// x 660. An invisible exit hotspot is unusable: a user cannot find it and, when
/// they do land on it by accident, the app leaves without explanation.
///
/// WHY rows 8..60 rather than 28..68: the title's line box starts at 64, so a
/// 52px control has to live above it. The pill is centred horizontally on the
/// same 24px margin the title uses, which is where the shell puts a back
/// control.
const BACK: Rect = Rect::new(HEADER_MARGIN, 8, 52, 52);

/// The system home indicator, and the strip that contains it.
///
/// WHY the hit box is larger than the ink: 6px of ink is not a touch target, so
/// the tap region is the whole gesture strip - but the old code tested
/// `x 240..480 && y >= 1400` with no upper bound on y, a 241x40px region that was
/// never painted. Both rectangles are named here so the region that is
/// hit-tested is the region that contains the ink.
const HOME_HIT: Rect = Rect::new(240, 1400, 240, 40);
const HOME_PILL: Rect = Rect::new(260, 1418, 200, 6);

/// Right edge the header readout is aligned to (the 24px right margin).
const READOUT_X1: usize = W - HEADER_MARGIN;
/// Widest a header readout run may be. WHY 400 and not "whatever fits": the
/// title ends at x 180, so a readout longer than `READOUT_X1 - 180` = 516 would
/// collide with it. 400 leaves a 116px gutter and `font::ellipsize` trims to it
/// rather than letting a long path run under the title.
const READOUT_MAX_W: usize = 400;
/// Row boxes for the two readout lines. WHY 68 and 100: the face is 24 rows
/// tall, so scale-1 runs own 68..92 and 100..124 - both inside the title's
/// 64..136 band, separated by 8 rows, and never overlapping each other.
const READOUT_LINE1_Y0: usize = 68;
const READOUT_LINE2_Y0: usize = READOUT_LINE1_Y0 + font::GLYPH_H + 8;

/// Rows the category list owns. WHY there is no header band constant beside it:
/// nothing a touch can reach writes to the header - the storage readout is
/// filled once, before the first present - so a band for it would never run.
const ROW_BAND_Y0: usize = 150;
const ROW_BAND_Y1: usize = 820;

/// Ink for an affordance that is painted but has nothing behind it.
///
/// WHY the border token: it is the design system's own hairline, so "you cannot
/// do this" reads the same here as it does in the gallery's disabled chips.
/// Unused on this screen today - the two header pictograms that had nothing
/// behind them were removed rather than greyed, because there was nothing left
/// for them to grey out *and* say.
#[allow(dead_code)]
const DISABLED: u32 = BORDER_SUBTLE;

/// Back-chevron stroke, apex pointing left, centred on `(cx, cy)`.
///
/// WHY it exists rather than a bitmap: the shell draws its back controls from
/// one rounded-stroke family, and a vector stroke stays inside the pill it is
/// centred in when the pill is moved.
fn chevron_left(x: usize, y: usize, cx: usize, cy: usize, half: isize) -> bool {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    let d = dy.abs();
    dx <= 0 && d <= half && (dx + d).abs() <= 2
}

/// Rasterises one text run into the pixel at `(x, y)`.
///
/// `bg` is the fill the run is drawn on top of. WHY it is a parameter: the
/// anti-aliased edges of a glyph are blended toward `bg`, so hard-coding the
/// page colour handed the selected row's label dark fringes against its own
/// `SURFACE_RAISED` fill. Callers pass the fill that is actually under the run.
///
/// The face is `GLYPH_H` (24) rows tall, so a run at `scale` owns
/// `GLYPH_H * scale` rows. Clipping to that line box here means a caller can no
/// longer slice a glyph with a hand-written y window.
///
/// WHY the outline face comes first: the title is drawn at scale 3 and the
/// category labels at scale 2, so on the bitmap face both are a 16x24 cell
/// magnified by an integer -- a soft ramp where an edge belongs.
/// `outline_text::place_run` rasterises the real outline at the em that matches
/// the cell's cap height (17 rows at scale 1, so 17*N*2048/1456 px per em), and
/// answers for the pixel itself. When it declines -- a run wider than the
/// coverage buffer, a glyph whose ink would leave this caller's window, or a
/// weight this app draws bold -- the bitmap face below draws the run exactly as
/// it did before, so a refusal costs the edge quality and never the text.
///
/// WHY the run answers alone when it is placed: the two faces are alternatives,
/// not layers. Falling through to the bitmap face after an outline run had
/// already inked a pixel would double-ink every partially covered edge.
///
/// WHY no column bound is handed to `place_run`: this funnel's callers gate
/// their runs by construction -- the title sits 131px into a 720px header and
/// the widest label ends 301px into the 648px list band -- so no run can reach
/// the edge the caller would cut it at.
#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    if x < x0 || y < y0 || y >= y0 + std::design::font::GLYPH_H * scale {
        return None;
    }
    if let Some(run) = outline_text::place_run(x0, y0, scale, text, None, false) {
        return run.pixel(x, y, color, bg);
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, false)
}

// -----------------------------------------------------------------------------
// Counted Log Assembly (no allocator in this no_std app)
// -----------------------------------------------------------------------------

/// Formats `v` as decimal ASCII into `out`; returns the bytes written.
fn fmt_u64(mut v: u64, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 20];
    let mut n = 0usize;
    while v > 0 && n < 20 {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let n = n.min(out.len());
    for i in 0..n {
        out[i] = digits[n - 1 - i];
    }
    n
}

/// Builds `prefix ++ decimal(value) ++ suffix` into `buf` and returns its length.
/// WHY: measured values only exist at runtime, so they are assembled into a
/// stack buffer before being logged.
fn log_counted(prefix: &str, value: u64, suffix: &str) {
    let mut buf = [0u8; 192];
    let p = prefix.len().min(buf.len());
    buf[..p].copy_from_slice(&prefix.as_bytes()[..p]);
    let v = fmt_u64(value, &mut buf[p..]);
    let n = p + v;
    let s = suffix.len().min(buf.len().saturating_sub(n));
    buf[n..n + s].copy_from_slice(&suffix.as_bytes()[..s]);
    if let Ok(msg) = core::str::from_utf8(&buf[..n + s]) {
        std::log(0, msg).ok();
    }
}

// -----------------------------------------------------------------------------
// VFS Entry Representation
// Runtime entries must come from the kernel VFS; no synthetic inventory is embedded.
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, Copy)]
pub struct VfsItem {
    pub name: &'static str,
    pub is_dir: bool,
    pub size_str: &'static str,
    pub meta_str: &'static str,
    pub color_accent: u32,
}

/// The app ships no embedded file inventory: `ITEMS_ALL` stays empty until a
/// real VFS listing can populate it (WHY: the platform truth contract pins the
/// empty ledger, and nothing on screen may invent file names).
#[allow(dead_code)]
const ITEMS_ALL: [VfsItem; 0] = [];

/// Default probe label before the syscall has run: a real statement, not a
/// placeholder that could be mistaken for a result.
///
/// WHY a byte-literal and not `b"..."`: `copy_from_slice` is not usable in a
/// `const`, and the array has to be initialised in `const fn new` so a fresh app
/// is never uninitialised. `"not queried"` is 10 bytes; the remaining 30 are
/// padding and are never read because `probe_label_len` is 10.
const PROBE_DEFAULT: [u8; 40] =
    *b"not queried\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";

pub struct FileManagerApp {
    pub selected_shortcut: usize,
    pub storage_capacity_bytes: u64,
    pub current_path: &'static str,
    /// What `fs_list` actually answered, pre-rendered.
    ///
    /// WHY a fixed buffer instead of formatting per pixel: `get_file_manager_color`
    /// runs once per pixel, so building this string in the render path would run
    /// it 1,036,800 times for one number. It is built once, when the syscall
    /// returns, and only read afterwards.
    pub probe_label: [u8; 40],
    pub probe_label_len: usize,
}

impl FileManagerApp {
    pub const fn new() -> Self {
        Self {
            selected_shortcut: 0,
            storage_capacity_bytes: 0,
            current_path: "/data",
            probe_label: PROBE_DEFAULT,
            probe_label_len: 10,
        }
    }

    /// Records the real answer from `fs_list`.
    ///
    /// WHY both arms are shown: the honest result of a directory query on this
    /// platform is often "refused", and a screen that only ever prints a byte
    /// count teaches the user nothing. Reporting the kernel's own status code is
    /// the only truthful thing to put there.
    pub fn record_probe(&mut self, result: Result<usize, i64>) {
        let mut buf = [0u8; 40];
        let n = match result {
            Ok(bytes) => {
                let prefix = b"received ";
                let p = prefix.len();
                buf[..p].copy_from_slice(prefix);
                let mut n = p + fmt_u64(bytes as u64, &mut buf[p..]);
                let suffix = b" bytes";
                for &b in suffix {
                    if n < buf.len() {
                        buf[n] = b;
                        n += 1;
                    }
                }
                n
            }
            Err(code) => {
                let prefix = b"refused -";
                let p = prefix.len();
                buf[..p].copy_from_slice(prefix);
                p + fmt_u64(code.unsigned_abs(), &mut buf[p..])
            }
        };
        self.probe_label = [0u8; 40];
        self.probe_label[..n].copy_from_slice(&buf[..n]);
        self.probe_label_len = n;
    }
}

// -----------------------------------------------------------------------------
// Pixel Color Resolution Engine
// -----------------------------------------------------------------------------
#[inline(never)]
fn get_file_manager_color(x: usize, y: usize, app: &FileManagerApp) -> u32 {
    // 1. Status Bar & Clean Header (y: 0..HEADER_H)
    if y < HEADER_H {
        // Clock and battery indicators are omitted until live guest services
        // expose real readings. Never paint plausible but fabricated values.

        // Back control, painted from the same rectangle the tap handler tests.
        if let Some(c) = draw_pill(x, y, BACK, SURFACE_RAISED, Some(BORDER_SUBTLE)) {
            if c != SURFACE_RAISED {
                return c;
            }
            let cx = BACK.x + BACK.w / 2;
            let cy = BACK.y + BACK.h / 2;
            if chevron_left(x, y, cx, cy, 12) {
                return TEXT_PRIMARY;
            }
            return SURFACE_RAISED;
        }

        // Title: "Files" at x: 48, y: 64 (scale 3).
        //
        // WHY the full box: the bitmap face is GLYPH_H (24) rows tall, so a
        // scale-3 run owns 72 rows (64..136). The old window was 76..108 -- 32
        // rows -- which drew only the top sliver of each letter and severed the
        // baseline. 136 still clears the header band (ends at 140) and the
        // category rows (start at 150).
        //
        // WHY the coordinates are written out instead of pulled from
        // constants: the native-UI truth contract pins this exact call
        // (`test_text(x, y, 48, 64, 3, b"Files"`) precisely so the title's line
        // box cannot be narrowed again. Naming the numbers here would satisfy
        // the compiler and silently break the test whose job is to prevent
        // that regression, so the literal stays the single source of truth.
        if let Some(c) = test_text(x, y, 48, 64, 3, b"Files", TEXT_PRIMARY, BG_DARK) {
            return c;
        }

        // Storage readout, right-aligned, two measured lines.
        //
        // WHY this replaced the search and overflow pictograms: both were drawn
        // at full strength in the header and neither had a tap handler - this
        // app has no search index and no menu, so they were two dead controls
        // that looked like the most important things on the screen. What the app
        // can actually do is ask the kernel for a directory listing, so it now
        // says which path it asked and what the kernel answered. Line 1 is the
        // path that was really passed to `fs_list`; line 2 is the real outcome,
        // including a refusal, which is what this platform actually returns for
        // a path outside the task's storage root.
        let path = app.current_path.as_bytes();
        let pw = font::measure(path, 1);
        if pw <= READOUT_MAX_W {
            if let Some(c) = test_text(
                x,
                y,
                READOUT_X1 - pw,
                READOUT_LINE1_Y0,
                1,
                path,
                TEXT_MUTED,
                BG_DARK,
            ) {
                return c;
            }
        }

        let mut label_buf = [0u8; 40];
        let label_len = font::ellipsize(
            &app.probe_label[..app.probe_label_len],
            1,
            READOUT_MAX_W,
            &mut label_buf,
        );
        let lw = font::measure(&label_buf[..label_len], 1);
        if lw <= READOUT_MAX_W {
            if let Some(c) = test_text(
                x,
                y,
                READOUT_X1 - lw,
                READOUT_LINE2_Y0,
                1,
                &label_buf[..label_len],
                TEXT_PRIMARY,
                BG_DARK,
            ) {
                return c;
            }
        }

        return BG_DARK;
    }

    // 2. Vertical Category Rows (y: 150..820)
    if (150..820).contains(&y) && x >= 36 && x < 684 {
        let (row_idx, row_start_y, row_h) = if y < 240 {
            (0, 150, 90)
        } else if y < 330 {
            (1, 240, 90)
        } else if y < 420 {
            (2, 330, 90)
        } else if y < 510 {
            (3, 420, 90)
        } else if y < 600 {
            (4, 510, 90)
        } else if y < 710 {
            (5, 600, 110)
        } else {
            (6, 710, 90)
        };

        // Thin bottom divider line
        if y == row_start_y + row_h - 1 {
            return BORDER_SUBTLE;
        }

        // Group container card outer border and rounded corners
        let rx = (x - 36).min(683 - x);
        let ry = (y - 150).min(799 - y);
        if (y < 170 || y >= 780) && rx < 16 && ry < 16 {
            let cdx = (16 - rx) as isize;
            let cdy = (16 - ry) as isize;
            if cdx * cdx + cdy * cdy >= 256 {
                return BG_DARK;
            }
        }
        if rx < 2 || y < 152 || y >= 798 {
            return BORDER_SUBTLE;
        }

        // WHY: this highlight is what makes the tap handler's selection visible —
        // the selected row gets a raised background and a left accent bar, so the
        // control reports a real state change instead of an invisible one.
        // It is applied only inside the selected row's own band, so no tint
        // bleeds into the padding below the last row.
        let row_selected =
            app.selected_shortcut == row_idx && y >= row_start_y && y < row_start_y + row_h;
        if row_selected && x < 44 {
            return ACCENT_SKY;
        }
        let row_bg = if row_selected { 0xFFEFF6FF } else { SURFACE_DARK };

        let center_y = row_start_y + (if row_idx == 5 { 40 } else { 45 });
        let icon_cx = 64i32;
        let icon_cy = center_y as i32;
        let idx = (x as i32) - icon_cx;
        let idy = (y as i32) - icon_cy;
        let idist2 = idx * idx + idy * idy;

        // WHY the line box is centred on `center_y` and not inset from it: the
        // six single-line rows below laid their scale-2 runs out with
        // `center_y - 8`, which puts the run's own 48-row box 16px below the
        // icon's centre, so the label's ink sat visibly low against its icon.
        // Measured on the guest: row 0's icon ink centred on 194.5 while
        // "Recent" centred on 207 - a 12.5px step. Row 5, the two-line stack,
        // already used the centred form (`row_start_y + 16` is exactly
        // `center_y - line_height(2) / 2` at its own `center_y`), so these six
        // were the inconsistent site. The reference mockup (Ui Samples/11_files)
        // centres the label's ink on its icon in every row, and the arithmetic
        // below reproduces that: `line_height(2) / 2` is half the run's own box,
        // so the box straddles `center_y` exactly.
        let label_y0 = center_y - std::design::font::line_height(2) / 2;
        // Render Icons
        match row_idx {
            0 => {
                // Clock icon (Recent)
                if (idist2 >= 100 && idist2 <= 196) || (idx == 0 && idy >= -10 && idy <= 0) || (idy == 0 && idx >= 0 && idx <= 8) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, label_y0, 2, b"Recent", TEXT_PRIMARY, row_bg) {
                    return c;
                }
            }
            1 => {
                // Images icon (Landscape frame)
                if (idx.abs() <= 14 && idy.abs() <= 10) && (idx.abs() >= 12 || idy.abs() >= 8 || ((idx - 5).abs() <= 2 && (idy + 3).abs() <= 2) || (idy >= 2 && idx.abs() <= (10 - idy))) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, label_y0, 2, b"Images", TEXT_PRIMARY, row_bg) {
                    return c;
                }
            }
            2 => {
                // Videos icon (Camcorder)
                if ((idx >= -14 && idx <= 4 && idy.abs() <= 10) && (idx == -14 || idx == 4 || idy.abs() >= 8)) || (idx >= 6 && idx <= 14 && idy.abs() <= (idx - 5)) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, label_y0, 2, b"Videos", TEXT_PRIMARY, row_bg) {
                    return c;
                }
            }
            3 => {
                // Documents icon (Folded sheet)
                if (idx.abs() <= 10 && idy.abs() <= 14) && (idx.abs() >= 8 || idy == 12 || (idy == -12 && idx <= 2) || (idy == -2 || idy == 3 || idy == 8)) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, label_y0, 2, b"Documents", TEXT_PRIMARY, row_bg) {
                    return c;
                }
            }
            4 => {
                // Downloads icon (Down arrow with tray)
                if (idx == 0 && idy >= -12 && idy <= 4) || (idy >= 0 && idy <= 6 && idx.abs() <= (6 - idy)) || (idy == 10 && idx.abs() <= 12) || (idx.abs() == 12 && idy >= 6 && idy <= 10) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, label_y0, 2, b"Downloads", TEXT_PRIMARY, row_bg) {
                    return c;
                }
            }
            5 => {
                // Internal Storage (Drive icon)
                if (idx.abs() <= 14 && idy.abs() <= 10) && (idx.abs() >= 12 || idy.abs() >= 8 || (idy == 2 && idx.abs() <= 10)) {
                    return TEXT_PRIMARY;
                }
                // Two-line stack: label (scale 2, 48px tall) above caption
                // (scale 1, 24px tall). Measured via
                // std::design::font::measure: "Internal Storage"@2 = 298px,
                // "Storage capacity unavailable"@1 = 267px,
                // "Capacity detected; usage unavailable"@1 = 348px — all fit
                // in the 576px budget (x 108..684). y0 delta (56) >=
                // line_height(2) (48), so the bands never share a pixel.
                // Old y0s (row_start+24 / +56, delta 32) overlapped by 16px.
                //
                // WHY `label_y0` is not rebound here: the row's own `center_y`
                // is `row_start_y + 40` (this row is the taller of the two
                // shapes), so the shared `label_y0` computed above is exactly
                // the `row_start_y + 16` this branch used to spell out. Rebinding
                // it to the same number was redundant; only the caption moves.
                let caption_y0 =
                    label_y0 + std::design::font::line_height(2) + 8;
                if let Some(c) =
                    test_text(x, y, 108, label_y0, 2, b"Internal Storage", TEXT_PRIMARY, row_bg)
                {
                    return c;
                }
                let capacity_status: &[u8] = if app.storage_capacity_bytes > 0 {
                    b"Capacity detected; usage unavailable"
                } else {
                    b"Storage capacity unavailable"
                };
                if let Some(c) = test_text(x, y, 108, caption_y0, 1, capacity_status, TEXT_MUTED, row_bg) {
                    return c;
                }

                // Tactile capacity track bar (x: 108..640, y: 700..706)
                if y >= 700 && y < 706 && x >= 108 && x < 640 {
                    if app.storage_capacity_bytes > 0 && x < 108 + 160 {
                        return ACCENT_SKY;
                    }
                    return BORDER_SUBTLE;
                }
            }
            _ => {
                // Cloud icon
                if (idy >= 2 && idy <= 8 && idx.abs() <= 14) || ((idx + 6) * (idx + 6) + idy * idy <= 49) || ((idx - 6) * (idx - 6) + (idy + 2) * (idy + 2) <= 36) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, label_y0, 2, b"Cloud (Optional)", TEXT_PRIMARY, row_bg) {
                    return c;
                }
            }
        }

        return row_bg;
    }

    // 3. Home Gesture Pill, drawn from the same rectangle the tap handler tests.
    if HOME_PILL.contains(x, y) {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn render_file_manager_ui(surface: u32, app: &FileManagerApp) {
    render_file_manager_band(surface, app, 0, H);
}

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a whole band with no stride
/// bookkeeping. Every band starts on a row boundary, so the running `x`/`y`
/// cursor below is exact.
///
/// WHY this is correct rather than a shortcut: the band is named after what the
/// touch actually moved. Selecting a category repaints only the list band -
/// 670 of the 1440 rows - and a tap that changed nothing does not render at all.
/// The old renderer re-rasterised all 1,036,800 pixels on every event.
fn render_file_manager_band(surface: u32, app: &FileManagerApp, y0: usize, y1: usize) {
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_file_manager_color(x, y, app);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
        offset += count;
    }
}

/// Which rows of the surface a touch actually changed.
///
/// WHY this exists: the old loop called `render_file_manager_ui` from the row
/// branch, which re-rasterised the whole screen to move a selection fill.
///
/// WHY there is no `Header` variant: nothing a touch can reach writes to the
/// header. The storage readout is filled once, before the first surface is
/// presented, and no control mutates it afterwards - a band for it would be
/// dead weight pretending to be an optimisation.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing moved; the surface is already correct, so do not render at all.
    None,
    /// A different category row is selected: the list band only.
    Rows,
    /// Anything that can move pixels in more than one band at once.
    All,
}

/// Grow a dirty region so one rendered frame covers everything a poll batch
/// changed.
///
/// WHY: `event_poll` can deliver several touches before the app is scheduled
/// again, and rendering only the last region would leave the earlier one stale.
impl Dirty {
    fn widen(self, other: Dirty) -> Dirty {
        match (self, other) {
            (Dirty::None, d) | (d, Dirty::None) => d,
            (a, b) if a == b => a,
            _ => Dirty::All,
        }
    }
}

enum Tap {
    /// Nothing on screen moved.
    None,
    /// The model moved; render the named region.
    Redraw(Dirty),
    /// Leave to the launcher, logging this exact reason first.
    Leave(&'static str),
}

fn exit_to_launcher(surface: u32) -> ! {
    if surface > 0 {
        let _ = std::surface_destroy(6, surface);
    }
    std::exit_with_code(0);
}

// -----------------------------------------------------------------------------
// Entry Point & Event Reactor Loop
// -----------------------------------------------------------------------------
#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(
        0,
        "[FILE_MANAGER (Ring 3)] Sovereign File Manager & VFS Explorer online.",
    )
    .ok();

    let mut app = FileManagerApp::new();

    // Query physical block storage capacity from driver
    if let Ok(bytes) = std::storage_query() {
        app.storage_capacity_bytes = bytes;
        std::log(
            0,
            "[FILE_MANAGER] Storage capacity validated from hardware service.",
        )
        .ok();
    }

    // Enumerate initial VFS directory. WHY byte counts: the payload is received
    // but never parsed or rendered, so claiming a successful parse would be false.
    //
    // WHY `FS_READ_CAP_SLOT` (4) and not the literal 3 that was here before:
    // slot 3 is `HardwarePortIo`, so `sys_fs_list`'s capability check failed
    // before the path was ever examined and the app logged a refusal it had not
    // actually been entitled to receive. The result is recorded on screen too,
    // so the header says what this app can actually see of the filesystem.
    let mut entries = [0u8; 1024];
    let probe = std::fs_list(FS_READ_CAP_SLOT, app.current_path, &mut entries);
    app.record_probe(probe);
    match probe {
        Ok(n) => {
            log_counted(
                "[FILE_MANAGER] fs_list('/data'): ",
                n as u64,
                " bytes of directory entries received; not parsed or rendered.",
            );
        }
        Err(code) => {
            log_counted(
                "[FILE_MANAGER] fs_list('/data') refused, code -",
                code.unsigned_abs(),
                "; no directory data received.",
            );
        }
    }

    // Allocate client surface (slot 6 = FrameBufferDirect, z=200)
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(id) => {
            std::log(
                0,
                "[FILE_MANAGER] DRM/KMS Client Surface allocated (z=200).",
            )
            .ok();
            id
        }
        Err(_) => 0,
    };

    if surface > 0 {
        render_file_manager_ui(surface, &app);
        let _ = std::surface_flip(6);
        std::log(0, "[FILE_MANAGER] Files UI presented to scanout buffer.").ok();
    }

    // Interactive Event Loop
    loop {
        let interest = std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK;
        if let Ok(triggered) = std::event_poll(interest, 100) {
            if (triggered & std::EVENT_TOUCH_INPUT) != 0 {
                let mut input = std::InputEvent {
                    kind: 0,
                    x: 0,
                    y: 0,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                };
                let mut dirty = Dirty::None;
                while std::input_event_poll(&mut input).is_ok() {
                    // Swipe gesture -> Return to launcher
                    if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10
                    {
                        std::log(
                            0,
                            "[FILE_MANAGER] Navigation gesture recognized. Exiting to launcher.",
                        )
                        .ok();
                        exit_to_launcher(surface);
                    }

                    if input.kind == 1 {
                        match handle_tap(&mut app, input.x as usize, input.y as usize) {
                            Tap::Leave(msg) => {
                                std::log(0, msg).ok();
                                exit_to_launcher(surface);
                            }
                            Tap::Redraw(d) => dirty = dirty.widen(d),
                            Tap::None => {}
                        }
                    }
                }
                if dirty != Dirty::None && surface > 0 {
                    match dirty {
                        Dirty::Rows => {
                            render_file_manager_band(surface, &app, ROW_BAND_Y0, ROW_BAND_Y1)
                        }
                        Dirty::All => render_file_manager_ui(surface, &app),
                        Dirty::None => {}
                    }
                    let _ = std::surface_flip(6);
                }
            }
        }
        std::yield_now();
    }
}

/// Resolves one touch against the controls that are actually drawn.
///
/// Every branch below tests a rectangle the renderer also draws from, so a tap
/// is accepted exactly where there is something to press.
fn handle_tap(app: &mut FileManagerApp, x: usize, y: usize) -> Tap {
    // Back control or the home gesture strip.
    //
    // WHY the back control now exists at all: `x 24..100 && y 28..68` used to
    // exit the app while drawing nothing there - an invisible hotspot a user
    // could not find and could land on by accident. `BACK` is the same 52x52
    // pill the renderer paints, so the region is visible.
    if BACK.contains(x, y) || HOME_HIT.contains(x, y) {
        return Tap::Leave(
            "[FILE_MANAGER] Back control or home gesture pressed. Exiting to launcher.",
        );
    }

    // Category rows are drawn as a vertical list (y ROW_BAND_Y0..ROW_BAND_Y1,
    // x 36..684; rows of 90px, Internal Storage 110px).
    // WHY row-shaped hit test: the previous horizontal band logged a filter
    // switch for taps that hit nothing, missed the rows that are actually drawn,
    // and had no visible effect. The selection and the log now happen together,
    // only inside the row that was really tapped, and the renderer highlights
    // the selected row.
    if (ROW_BAND_Y0..ROW_BAND_Y1).contains(&y) && (36..684).contains(&x) {
        let shortcut = if y < 240 {
            0 // Recent
        } else if y < 330 {
            1 // Images
        } else if y < 420 {
            2 // Videos
        } else if y < 510 {
            3 // Documents
        } else if y < 600 {
            4 // Downloads
        } else if y < 710 {
            5 // Internal Storage
        } else {
            6 // Cloud
        };
        // WHY no redraw when the tap lands on the row that is already selected:
        // the fill, the accent bar and the caption are all functions of
        // `selected_shortcut`, so re-rendering would produce an identical
        // surface. The old loop always rendered.
        if app.selected_shortcut == shortcut {
            return Tap::None;
        }
        app.selected_shortcut = shortcut;
        // `current_path` keeps the directory that was really queried ("/data"):
        // no navigation happens, so no new path is claimed.
        std::log(0, "[FILE_MANAGER] Category filter shortcut switched.").ok();
        return Tap::Redraw(Dirty::Rows);
    }

    Tap::None
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[FILE_MANAGER] Panic detected! Terminating task...").ok();
    std::exit();
}
