//! Quick Controls: the shade pulled down from the status bar.
//!
//! WHY this screen is mostly an honest absence: the reference shade offers Wi-Fi,
//! Bluetooth, Mobile, Airplane, brightness, volume and a torch row. This platform
//! binds none of that. There is no radio -- the kernel's `WirelessManager` is
//! constructed with `hardware_present: false` -- no torch hardware, capability or
//! syscall, and no backlight or brightness syscall anywhere in the tree. A
//! per-pixel renderer cannot issue a syscall per pixel to ask, and `LauncherState`
//! carries no reading for any of it. So every control below reports the capability
//! it actually has, and a control with no capability is drawn as an outline rather
//! than a filled tile implying a live connection.
//!
//! WHY that is worth building rather than skipping: a shade is a promise that what
//! you tap is wired to something. Filling a tile and printing "Connected" for
//! hardware that does not exist makes the whole screen untrustworthy, and the one
//! thing this shell can still be trusted for is reporting what it actually did.

#![allow(unused_imports)]

use crate::{test_text, LauncherState};
use launcher::layout::Rect;
use launcher::text;
use launcher::theme::*;

// -----------------------------------------------------------------------------
// Capability states
// -----------------------------------------------------------------------------

/// What this shell can say about a control, truthfully.
///
/// WHY an enum rather than a boolean per tile or a hardcoded string per tile: the
/// previous file answered the same question six different ways in prose, so a
/// reader could not tell which tiles had a reading behind them and which were
/// reporting the absence of one. These are two different claims and they are not
/// interchangeable:
///
/// * [`Cap::Unbound`] -- the platform binds no capability for this control at
///   all. No radio, no torch, no display backlight exists in this build, so
///   nothing could report a state even in principle.
/// * [`Cap::Unread`] -- the platform has the capability, but this surface holds no
///   reading of it: `LauncherState` carries none, and asking would mean a syscall
///   per pixel. Reporting this as `Unbound` would blame the hardware for a gap in
///   the shell, which is the more dangerous of the two mistakes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cap {
    Unbound,
    Unread,
}

impl Cap {
    /// The line a tile prints under its name. WHY it is derived from the state
    /// rather than written per tile: one wording per claim is checkable, and a
    /// tile cannot quietly acquire a caption that overstates it.
    fn caption(self) -> &'static [u8] {
        match self {
            Cap::Unbound => b"Unavailable",
            Cap::Unread => b"Not reported",
        }
    }
}

/// The icons this shade draws. WHY an enum and not a glyph per call site: the icon
/// and the tile are one thing, and a list of tiles that each name their own icon
/// cannot drift out of step with the reference's order.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Glyph {
    Wifi,
    Bluetooth,
    Mobile,
    Airplane,
    Sun,
    Speaker,
    Torch,
    Rotate,
    Moon,
}

/// One control tile.
struct Tile {
    rect: Rect,
    glyph: Glyph,
    name: &'static [u8],
    cap: Cap,
}

// -----------------------------------------------------------------------------
// Geometry
// -----------------------------------------------------------------------------

const INSET: usize = CONTENT_PADDING;
const CONTENT_W: usize = SCREEN_WIDTH - 2 * CONTENT_PADDING;

/// Header line, one step below the status bar.
const HEADER_Y: usize = STATUS_BAR_HEIGHT + space::MD;
/// The dismiss affordance sits where `main.rs` will act on it: a tap at
/// `x >= 600, y >= 70` returns home, so the circle is inside that quadrant rather
/// than straddling its edge.
const CLOSE_Y: usize = 70;
const CLOSE_SIDE: usize = MIN_TOUCH_TARGET;

/// The 2x2 connectivity block.
const BIG_ROW_H: usize = 232;
const BIG_GUTTER: usize = space::LG;
const BIG_W: usize = (CONTENT_W - BIG_GUTTER) / 2;
const BIG_TOP: usize = HEADER_Y + type_scale::BODY * text::GLYPH_H + space::XL;

/// WHY this band is typed: `main.rs` opens the Power menu for a tap inside
/// `(630..740, 375..680)` and nothing else in the shade may be drawn there, or the
/// shade would show one control over another screen's hit region. The control in
/// it is labelled for what that tap actually does.
const POWER_Y: usize = 630;
const POWER_H: usize = 740 - POWER_Y;
const POWER_X: usize = 375;
const POWER_W: usize = CONTENT_W - (POWER_X - INSET);

/// The two level controls. Both are drawn as empty tracks: see `slider_pixel`.
const SLIDER_H: usize = 136;
const SLIDER_Y: usize = POWER_Y + POWER_H + space::XL;
const VOLUME_Y: usize = SLIDER_Y + SLIDER_H + space::LG;

/// The three tiles at the foot.
const SMALL_Y: usize = VOLUME_Y + SLIDER_H + space::XL;
const SMALL_H: usize = SCREEN_HEIGHT - space::XXL * 3 - SMALL_Y;
const SMALL_GUTTER: usize = space::SM;
const SMALL_W: usize = (CONTENT_W - 2 * SMALL_GUTTER) / 3;

/// The icon inside a tile: at most this wide, and never more than half the tile's
/// height, so the name and its caption always have room underneath.
const GLYPH_MAX: usize = space::XXL * 2 + space::XL;

// -----------------------------------------------------------------------------
// Rounded rectangles
// -----------------------------------------------------------------------------

/// Whether a pixel belongs to a rectangle whose corners are rounded to `rad`.
fn rounded_contains(r: Rect, x: usize, y: usize, rad: usize) -> bool {
    if !r.contains(x, y) {
        return false;
    }
    let rad = rad.min(r.w / 2).min(r.h / 2);
    if rad == 0 {
        return true;
    }
    let left = r.x + rad;
    let right = r.x + r.w - 1 - rad;
    let top = r.y + rad;
    let bottom = r.y + r.h - 1 - rad;
    // Outside the corner boxes the rounded rectangle shares its area with the
    // plain one, so only corner pixels need the distance test.
    if (x >= left && x <= right) || (y >= top && y <= bottom) {
        return true;
    }
    let cx = if x < left { left } else { right };
    let cy = if y < top { top } else { bottom };
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    dx * dx + dy * dy <= (rad as i32) * (rad as i32)
}

/// The one-pixel outline of a rounded rectangle.
fn rounded_outline(r: Rect, x: usize, y: usize, rad: usize) -> bool {
    rounded_contains(r, x, y, rad)
        && !rounded_contains(r.inset(1), x, y, rad.saturating_sub(1))
}

// -----------------------------------------------------------------------------
// Text
// -----------------------------------------------------------------------------

// -----------------------------------------------------------------------------
// Text runs: every label, measured once per frame
// -----------------------------------------------------------------------------

/// Capacity of one run's byte buffer.
///
/// WHY 96 and not the longest label here: this is the width `text::ellipsize`
/// returns, so a run built here is byte-for-byte the run the old per-pixel helper
/// built, for any input, including a `date_line` longer than anything typed in
/// this file. A shorter buffer would have been a silent truncation.
const RUN_CAP: usize = 96;

/// One label, placed and measured once per frame.
///
/// WHY this type exists: the old helpers called `text::ellipsize` from inside the
/// per-pixel painter, and this screen has 26 of them, so a full frame did about
/// 26 million calls. Each one initialises a 96-byte buffer and re-measures the
/// string, which is where essentially the whole frame went. Everything that
/// depends on the *string* rather than the *pixel* -- the ellipsis decision, the
/// centring inset, the advance width, the ink -- is settled here, once.
///
/// WHY the box is stored as well as the origin: `test_text` walks the pen of every
/// glyph to decide whether a pixel is on the run, so it rejects a pixel outside the
/// run's row on its first two comparisons. Handing it the run's measured extent
/// first turns those twenty-six calls into twenty-six two-compare rejects for
/// almost every pixel of the shade, without changing which pixels are inked: the
/// stored box is exactly `y .. y + line_height(scale)` by
/// `x .. x + measure(ellipsized)`, which is precisely the region `test_text` can
/// return `Some` for.
struct Run {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    scale: usize,
    len: usize,
    ink: u32,
    bytes: [u8; RUN_CAP],
}

impl Run {
    /// `bytes` with its left edge at `x` and its top row at `y`, ellipsized to
    /// `width` -- the old `label` helper, resolved once instead of per pixel.
    fn new(x: usize, y: usize, scale: usize, width: usize, bytes: &[u8], ink: u32) -> Run {
        let (buf, len) = text::ellipsize(bytes, scale, width);
        Run::placed(x, y, scale, buf, len, ink)
    }

    /// The same, centred across `width` from `x` -- the old `label_centred`.
    fn centred(
        x: usize,
        y: usize,
        scale: usize,
        width: usize,
        bytes: &[u8],
        ink: u32,
    ) -> Run {
        let (buf, len) = text::ellipsize(bytes, scale, width);
        let inset = text::center_x(&buf[..len], scale, width);
        Run::placed(x + inset, y, scale, buf, len, ink)
    }

    /// A run from an already-ellipsized buffer.
    fn placed(x: usize, y: usize, scale: usize, buf: [u8; RUN_CAP], len: usize, ink: u32) -> Run {
        Run {
            x,
            y,
            w: text::measure(&buf[..len], scale),
            h: text::line_height(scale),
            scale,
            len,
            ink,
            bytes: buf,
        }
    }

    /// The colour of the pixel at (`px`, `py`) if it is inked by this run.
    fn pixel(&self, px: usize, py: usize) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if px < self.x || px - self.x >= self.w || py < self.y || py - self.y >= self.h {
            return None;
        }
        test_text(px, py, self.x, self.y, self.scale, &self.bytes[..self.len], self.ink)
    }
}

/// Writes `n` in decimal into `buf` and returns the byte count. WHY not `format!`:
/// this is a `no_std` shell with no allocator.
fn decimal(n: usize, buf: &mut [u8]) -> usize {
    if n == 0 {
        if let Some(slot) = buf.first_mut() {
            *slot = b'0';
            return 1;
        }
        return 0;
    }
    let mut digits = [0u8; 8];
    let mut count = 0usize;
    let mut value = n;
    while value > 0 && count < digits.len() {
        digits[count] = b'0' + (value % 10) as u8;
        value /= 10;
        count += 1;
    }
    for (i, slot) in buf.iter_mut().enumerate().take(count) {
        *slot = digits[count - 1 - i];
    }
    count
}

/// `n` followed by `word`, into `buf`.
fn number_with(n: usize, word: &[u8], buf: &mut [u8]) -> usize {
    let count = decimal(n, buf);
    for (i, byte) in word.iter().enumerate() {
        if let Some(slot) = buf.get_mut(count + i) {
            *slot = *byte;
        }
    }
    (count + word.len()).min(buf.len())
}

// -----------------------------------------------------------------------------
// Tiles
// -----------------------------------------------------------------------------

/// The connectivity block. WHY every entry is `Unbound`: the kernel's wireless
/// manager starts with `hardware_present: false`, so Wi-Fi and Bluetooth have no
/// radio to report; no SIM or baseband is attached for Mobile; and Airplane mode
/// has nothing to switch off. A modem service exists and the phone app drives it,
/// which is exactly why a tile here must not claim a registration.
fn big_tiles() -> [Tile; 4] {
    let row0 = Rect::new(INSET, BIG_TOP, BIG_W, BIG_ROW_H);
    let row1 = Rect::new(row0.x, row0.y + BIG_ROW_H + BIG_GUTTER, BIG_W, BIG_ROW_H);
    let col1 = INSET + BIG_W + BIG_GUTTER;
    [
        Tile {
            rect: row0,
            glyph: Glyph::Wifi,
            name: b"Wi-Fi",
            cap: Cap::Unbound,
        },
        Tile {
            rect: Rect::new(col1, row0.y, BIG_W, BIG_ROW_H),
            glyph: Glyph::Bluetooth,
            name: b"Bluetooth",
            cap: Cap::Unbound,
        },
        Tile {
            rect: row1,
            glyph: Glyph::Mobile,
            name: b"Mobile",
            cap: Cap::Unbound,
        },
        Tile {
            rect: Rect::new(col1, row1.y, BIG_W, BIG_ROW_H),
            glyph: Glyph::Airplane,
            name: b"Airplane",
            cap: Cap::Unbound,
        },
    ]
}

/// The foot tiles. Torch is `Unbound` for the reason the lock screen's torch button
/// refuses with a log line: no torch hardware, no torch capability, no torch
/// syscall. Auto-rotate and Dark mode are `Unread`: the platform has an
/// accelerometer and a settings service, but this surface was handed neither a
/// preference nor an orientation reading, so "On" would be a guess.
fn small_tiles() -> [Tile; 3] {
    [
        Tile {
            rect: Rect::new(INSET, SMALL_Y, SMALL_W, SMALL_H),
            glyph: Glyph::Torch,
            name: b"Torch",
            cap: Cap::Unbound,
        },
        Tile {
            rect: Rect::new(INSET + SMALL_W + SMALL_GUTTER, SMALL_Y, SMALL_W, SMALL_H),
            glyph: Glyph::Rotate,
            name: b"Auto rotate",
            cap: Cap::Unread,
        },
        Tile {
            rect: Rect::new(
                INSET + 2 * (SMALL_W + SMALL_GUTTER),
                SMALL_Y,
                SMALL_W,
                SMALL_H,
            ),
            glyph: Glyph::Moon,
            name: b"Dark mode",
            cap: Cap::Unread,
        },
    ]
}

/// The scale a tile's name is drawn at. WHY it is derived rather than fixed: a tile
/// narrower than the reference's connectivity block cannot hold a body-size label
/// without ellipsizing half of it away, and "Auto rot..." is worse than a smaller
/// word.
fn name_scale(tile: &Tile) -> usize {
    if tile.rect.w < BIG_W {
        type_scale::CAPTION
    } else {
        type_scale::BODY
    }
}

// -----------------------------------------------------------------------------
// Glyphs
// -----------------------------------------------------------------------------

/// The pixel's offset from a box's centre, or `None` when it is outside.
fn in_box(box_: Rect, x: usize, y: usize) -> Option<(i32, i32)> {
    if !box_.contains(x, y) {
        return None;
    }
    Some((
        x as i32 - box_.center_x() as i32,
        y as i32 - box_.center_y() as i32,
    ))
}

/// Whether an offset lies on either diagonal through the centre, `weight` pixels
/// thick. Used for the ray pattern of the sun glyph.
fn on_cross(dx: i32, dy: i32, weight: i32) -> bool {
    (dx - dy).abs() <= weight || (dx + dy).abs() <= weight
}

/// Whether a squared distance lies in a ring of `thick` pixels either side of `r`.
///
/// WHY squared distances and no square root: this runs once per pixel, and the
/// band `(r-1)^2 .. (r+1)^2` is already about two pixels wide at any radius this
/// shade uses, which is the weight the reference's strokes have. A zero-thickness
/// band picks single pixels and reads as a dotted line, so nothing here uses one.
fn in_ring(d2: i32, r: i32, thick: i32) -> bool {
    let inner = (r - thick).max(0);
    let outer = r + thick;
    d2 >= inner * inner && d2 <= outer * outer
}

/// Whether an offset lies on the segment from `(x0,y0)` to `(x1,y1)`, `weight`
/// pixels thick.
///
/// WHY a segment test and not a hand-written line equation per glyph: the
/// reference's icons are strokes between two points -- a Bluetooth rune, a plane's
/// wings, a torch's beam -- and one correct thickness-aware test draws all of them
/// at any size. The cross product is the distance from the infinite line times the
/// segment's length, so comparing it against `weight * length` gives a stroke of
/// the requested width.
fn on_segment(dx: i32, dy: i32, x0: i32, y0: i32, x1: i32, y1: i32, weight: i32) -> bool {
    let vx = x1 - x0;
    let vy = y1 - y0;
    let px = dx - x0;
    let py = dy - y0;
    let cross = vx * py - vy * px;
    let length = vx.abs() + vy.abs();
    if cross.abs() * 2 > weight * length.max(1) {
        return false;
    }
    dx >= x0.min(x1) - weight && dx <= x0.max(x1) + weight
        && dy >= y0.min(y1) - weight
        && dy <= y0.max(y1) + weight
}

/// Draws a glyph inside `box_`, or returns `None` when the pixel is outside it.
///
/// WHY geometric and not the shell's font: that font is ASCII and has no glyph for
/// a Wi-Fi fan, a sun or a crescent. Every shape here is either a filled region, a
/// ring or a stroked segment, which is all a per-pixel renderer can afford, and at
/// the one size this shade uses they read the way the reference's do.
///
/// The reference's icons are outlined; a filled one reads as a different state,
/// which is the one thing these tiles must never imply.
fn glyph_pixel(g: Glyph, box_: Rect, x: usize, y: usize, ink: u32) -> Option<u32> {
    let (dx, dy) = in_box(box_, x, y)?;
    let arm = (box_.w.min(box_.h) / 2).max(2) as i32;
    let d2 = dx * dx + dy * dy;
    let hit = match g {
        // A fan of three arcs over a dot, opening upward. The arcs are rings
        // centred on the dot, so only their upper halves are drawn.
        Glyph::Wifi => {
            let dot_r = (arm / 8).max(1);
            let dot = d2 <= dot_r * dot_r;
            let arc = dy <= 0
                && [0i32, 1, 2].iter().any(|i| {
                    let r = dot_r + (i + 1) * (arm - dot_r) / 3;
                    in_ring(d2, r, 1)
                });
            dot || arc
        }
        // The rune: a stem with two triangles sharing its middle, which is what
        // makes it read as Bluetooth rather than as a letter K.
        Glyph::Bluetooth => {
            let tip = arm * 3 / 5;
            let stem = dx.abs() <= 1 && dy.abs() <= arm;
            stem
                || on_segment(dx, dy, 0, -arm, tip, -arm / 2, 1)
                || on_segment(dx, dy, tip, -arm / 2, 0, 0, 1)
                || on_segment(dx, dy, 0, 0, tip, arm / 2, 1)
                || on_segment(dx, dy, tip, arm / 2, 0, arm, 1)
        }
        // Three columns rising from a common base: the reading every phone draws
        // for a radio.
        Glyph::Mobile => {
            let base = arm;
            let half = (arm / 4).max(1);
            (0..3i32).any(|k| {
                let cx = -arm * 2 / 3 + k * (arm * 2 / 3);
                let top = base - (k + 1) * (2 * arm / 3);
                (dx - cx).abs() <= half && dy >= top && dy <= base
            })
        }
        // A fuselage, two swept wings and a tail fin.
        Glyph::Airplane => {
            let body = dx.abs() <= (arm / 5).max(2) && dy >= -arm && dy <= arm / 2;
            let wing = on_segment(dx, dy, 0, -arm / 4, -arm * 9 / 10, arm / 5, 1)
                || on_segment(dx, dy, 0, -arm / 4, arm * 9 / 10, arm / 5, 1);
            let tail = on_segment(dx, dy, 0, arm / 4, -arm / 2, arm * 4 / 5, 1)
                || on_segment(dx, dy, 0, arm / 4, arm / 2, arm * 4 / 5, 1);
            body || wing || tail
        }
        // An outlined sun: a ring with eight rays, which is the reference's shape.
        Glyph::Sun => {
            let ring = in_ring(d2, arm / 2, 1);
            let cardinal = dx.abs() <= 1 || dy.abs() <= 1;
            let diagonal = on_cross(dx, dy, 1);
            (cardinal || diagonal) && d2 > (arm / 2 + 1) * (arm / 2 + 1)
                || ring
        }
        // A body block, a cone opening to the right, and one arc for the sound.
        // WHY the body's half-height is half the cone's: matched, the two merge
        // into a single triangle and the glyph reads as a play button.
        Glyph::Speaker => {
            let body = dx >= -arm && dx <= -arm / 2 && dy.abs() <= arm / 6;
            // The cone's half-height grows from `arm/6` at the body to `arm/2` at its
            // mouth. Written as one inequality so the wedge and the body meet at
            // exactly the same height, which is what keeps the step visible.
            let cone = dx > -arm / 2 && dx <= arm / 3 && dy.abs() * 30 <= arm * 5 + 6 * (dx + arm / 2);
            let wave = dx > arm / 4 && in_ring(d2, arm * 3 / 4, 1);
            body || cone || wave
        }
        // A torch: a wide head over a body that narrows downward, with three
    // strokes of light above the head.
        Glyph::Torch => {
            let head = dy < 0 && dy >= -arm / 3 && dx.abs() <= arm / 2 - dy / 6;
            let body = dy >= 0 && dy <= arm && dx.abs() * 5 <= arm * 3 + dy;
            let beam = on_segment(dx, dy, 0, -arm / 3, 0, -arm, 1)
                || on_segment(dx, dy, 0, -arm / 3, -arm * 3 / 5, -arm * 4 / 5, 1)
                || on_segment(dx, dy, 0, -arm / 3, arm * 3 / 5, -arm * 4 / 5, 1);
            head || body || beam
        }
        // A ring open at the top right with an arrowhead on the open end.
        Glyph::Rotate => {
            let r = arm * 3 / 5;
            let ring = in_ring(d2, r, 1) && !(dx > r / 3 && dy < -r / 3);
            let head = on_segment(dx, dy, r / 3, -r * 6 / 5, r * 5 / 4, -r * 6 / 5, 1)
                || on_segment(dx, dy, r / 3, -r * 6 / 5, r * 5 / 4, -r / 4, 1);
            ring || head
        }
        // A crescent. WHY filled and not an outline: an outlined crescent reads as
        // a letter C, which is what the first attempt looked like.
        Glyph::Moon => {
            let bx = dx - arm / 2;
            let by = dy + arm / 3;
            d2 <= (arm - 1) * (arm - 1) && bx * bx + by * by > (arm * 3 / 4) * (arm * 3 / 4)
        }
    };
    if hit {
        Some(ink)
    } else {
        None
    }
}

// -----------------------------------------------------------------------------
// Renderer
// -----------------------------------------------------------------------------

// -----------------------------------------------------------------------------
// Frame: everything this shade draws that does not depend on the pixel
// -----------------------------------------------------------------------------

/// One control tile with its icon box and both of its labels already measured.
struct TilePlan {
    rect: Rect,
    icon: Rect,
    glyph: Glyph,
    name: Run,
    caption: Run,
}

/// One level control with its two end-cap boxes and all three of its labels.
struct SliderPlan {
    track: Rect,
    caps: [Rect; 2],
    cap_ink: [u32; 2],
    glyph: Glyph,
    name: Run,
    caption: Run,
    reason: Run,
}

/// The whole shade, minus the arithmetic of deciding which pixel is where.
///
/// WHY this is resolved once per frame and not per pixel: this screen is the
/// densest in the shell -- 26 labels -- and the painter used to call
/// `text::ellipsize` once per label per pixel, so a single frame performed about 27
/// million of them, each initialising a 96-byte buffer and re-measuring a string.
/// Nothing in this struct depends on `x` or `y`. It is all either a theme constant
/// or something read out of `LauncherState`, which leaves the painter asking only
/// "is this pixel on it", which is two comparisons per label.
///
/// WHY the notification readout lives here rather than being formatted per pixel:
/// `state.notification_count` decides whether the line reads "None" or "N pending",
/// and formatting it a million times was part of the same cost.
struct Frame {
    /// `None` when the kernel refused the RTC read. The old painter tested
    /// `state.clock_source_ok` per pixel for these two runs and drew nothing.
    clock: Option<Run>,
    date: Option<Run>,
    close: Rect,
    close_arm: i32,
    big: [TilePlan; 4],
    power: Rect,
    power_name: Run,
    power_sub: Run,
    readout_name: Run,
    readout_value: Run,
    sliders: [SliderPlan; 2],
    small: [TilePlan; 3],
    home: Rect,
    home_ink: u32,
}

/// Resolves one [`Tile`] into everything the painter needs for it.
fn tile_plan(tile: &Tile, theme: &Theme) -> TilePlan {
    let side = GLYPH_MAX.min(tile.rect.h / 2);
    let icon = Rect::new(
        tile.rect.x + (tile.rect.w - side) / 2,
        tile.rect.y + space::LG,
        side,
        side,
    );
    let scale = name_scale(tile);
    let name_y = icon.y + icon.h + space::MD;
    TilePlan {
        rect: tile.rect,
        icon,
        glyph: tile.glyph,
        name: Run::centred(
            tile.rect.x,
            name_y,
            scale,
            tile.rect.w,
            tile.name,
            theme.text,
        ),
        caption: Run::centred(
            tile.rect.x,
            name_y + scale * text::GLYPH_H + space::SM,
            type_scale::CAPTION,
            tile.rect.w,
            tile.cap.caption(),
            theme.text_muted,
        ),
    }
}

/// Resolves one level control into everything the painter needs for it.
fn slider_plan(
    track: Rect,
    glyph: Glyph,
    name: &'static [u8],
    cap: Cap,
    reason: &'static [u8],
    theme: &Theme,
) -> SliderPlan {
    let side = space::XXL * 2;
    let text_x = track.x + space::XL + space::XXL * 2;
    let text_w = track.w - 2 * (space::XL + space::XXL * 2);
    SliderPlan {
        track,
        caps: [
            Rect::new(
                track.x + space::LG,
                track.center_y() - side / 2,
                side,
                side,
            ),
            Rect::new(
                track.x + track.w - space::LG - side,
                track.center_y() - side / 2,
                side,
                side,
            ),
        ],
        cap_ink: [
            theme.text_muted,
            Theme::veil(theme.text_muted, 60, true),
        ],
        glyph,
        name: Run::new(
            text_x,
            track.center_y() - type_scale::BODY * text::GLYPH_H - space::SM,
            type_scale::BODY,
            text_w,
            name,
            theme.text,
        ),
        caption: Run::new(
            text_x,
            track.center_y() + space::SM,
            type_scale::CAPTION,
            text_w,
            cap.caption(),
            theme.text_muted,
        ),
        reason: Run::new(
            text_x,
            track.center_y() + space::SM + type_scale::CAPTION * text::GLYPH_H + space::XS,
            type_scale::CAPTION,
            text_w,
            reason,
            Theme::veil(theme.text_muted, 70, true),
        ),
    }
}

/// Builds this frame's plan from the state and the theme.
fn build(state: &LauncherState) -> Frame {
    let theme = Theme::DARK;

    // WHY the two status-bar runs are `Option`: the old painter asked
    // `state.clock_source_ok` once per pixel and drew nothing when it was false.
    // Answering it here keeps that gate without re-reading the state per pixel, and
    // keeps the refusal visible: a shell with no clock shows an empty status bar.
    let clock = if state.clock_source_ok {
        let row = (STATUS_BAR_HEIGHT - type_scale::CAPTION * text::GLYPH_H) / 2;
        Some(Run::new(
            INSET,
            row,
            type_scale::CAPTION,
            space::XXL * 6,
            &state.clock_time,
            theme.text_muted,
        ))
    } else {
        None
    };
    let date = if state.clock_source_ok {
        Some(Run::new(
            INSET,
            HEADER_Y,
            type_scale::BODY,
            CONTENT_W,
            &state.date_line,
            theme.text,
        ))
    } else {
        None
    };

    let big_tiles = big_tiles();
    let small = small_tiles();
    let power = Rect::new(POWER_X, POWER_Y, POWER_W, POWER_H);
    let power_w = power.w - 2 * space::MD;
    let readout_w = POWER_X - INSET - space::MD;
    let readout_top = POWER_Y + space::MD;

    // WHY the count is formatted into a frame-owned buffer: "None" versus "N
    // pending" is the one label on this screen whose text is not a constant, and
    // deciding it per pixel meant rebuilding the decimal for every pixel of the
    // surface.
    let mut readout = [b' '; 32];
    let count = state.notification_count;
    let readout_len = if count == 0 {
        let mut n = 0;
        for byte in b"None" {
            readout[n] = *byte;
            n += 1;
        }
        n
    } else {
        number_with(count, b" pending", &mut readout)
    };

    Frame {
        clock,
        date,
        close: Rect::new(
            SCREEN_WIDTH - INSET - CLOSE_SIDE,
            CLOSE_Y,
            CLOSE_SIDE,
            CLOSE_SIDE,
        ),
        close_arm: (CLOSE_SIDE / 2 - 3) as i32,
        big: core::array::from_fn(|i| tile_plan(&big_tiles[i], &theme)),
        power,
        power_name: Run::new(
            power.x + space::MD,
            power.y + space::MD,
            type_scale::BODY,
            power_w,
            b"Power",
            theme.text,
        ),
        power_sub: Run::new(
            power.x + space::MD,
            power.y + space::MD + type_scale::BODY * text::GLYPH_H + space::SM,
            type_scale::CAPTION,
            power_w,
            b"Off, restart",
            theme.text_muted,
        ),
        readout_name: Run::new(
            INSET,
            readout_top,
            type_scale::BODY,
            readout_w,
            b"Notifications",
            theme.text,
        ),
        readout_value: Run::new(
            INSET,
            readout_top + type_scale::BODY * text::GLYPH_H + space::SM,
            type_scale::CAPTION,
            readout_w,
            &readout[..readout_len],
            theme.text_muted,
        ),
        sliders: [
            slider_plan(
                Rect::new(INSET, SLIDER_Y, CONTENT_W, SLIDER_H),
                Glyph::Sun,
                b"Brightness",
                Cap::Unbound,
                b"No backlight device",
                &theme,
            ),
            slider_plan(
                Rect::new(INSET, VOLUME_Y, CONTENT_W, SLIDER_H),
                Glyph::Speaker,
                b"Volume",
                Cap::Unread,
                b"No volume reading here",
                &theme,
            ),
        ],
        small: core::array::from_fn(|i| tile_plan(&small[i], &theme)),
        home: Rect::new(
            SCREEN_WIDTH / 2 - space::XXL * 2,
            SCREEN_HEIGHT - space::XXL,
            space::XXL * 4,
            space::SM,
        ),
        // WHY the veil is resolved here: it is channel arithmetic, and it ran once
        // per pixel for a bar that covers a fixed few thousand of them.
        home_ink: Theme::veil(theme.text, 90, true),
    }
}

/// This frame's plan.
///
/// WHY `(0, 0)` is the frame boundary and not a counter: `render_view` walks the
/// surface in raster order starting at the top-left pixel, so `(0, 0)` is the first
/// pixel of every frame and the only pixel of a frame that repeats it.
/// `LauncherState` is borrowed immutably for the whole of a frame, so rebuilding at
/// `(0, 0)` is exactly one rebuild per frame, and the cached plan can never describe
/// a frame that is not the one being drawn.
///
/// WHY the slot is a `static`: the shell's renderer contract is
/// `fn(x, y, &LauncherState) -> u32`, so there is no place to hand a precomputed
/// plan in from `render_view` without changing `main.rs`, which is not this change's
/// to make. The launcher is one bare-metal task with no preemption inside the paint
/// loop, so a single slot is enough and needs no locking.
static mut FRAME: Option<Frame> = None;

fn frame(x: usize, y: usize, state: &LauncherState) -> &'static Frame {
    let slot: *mut Option<Frame> = core::ptr::addr_of_mut!(FRAME);
    // SAFETY: the only writer is this function, called from the single-threaded
    // paint loop, and no reference outlives the call that made it.
    let empty = unsafe { (*slot).is_none() };
    if empty || (x == 0 && y == 0) {
        // SAFETY: as above; the write is the one mutation of the slot.
        unsafe { *slot = Some(build(state)) };
    }
    // SAFETY: the slot is `Some` here -- it was just filled when it was empty --
    // and nothing mutates it while a reference to it is alive.
    unsafe { (*slot).as_ref() }.expect("WHY: the slot is filled when it is empty")
}
#[inline(never)]
pub fn get_quick_controls_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // WHY `Theme::DARK` here rather than one inherited default: the reference
    // splits its palette by context, and the shade is one of the near-black
    // surfaces. A shade that lightened the wallpaper would stop reading as an
    // overlay, and a shade's whole job is to sit above the screen it came from.
    // Binding the palette in the screen that draws it keeps that split legible in
    // the source, and means no other screen has to agree with this one about what
    // "current" means.
    let theme = Theme::DARK;

    // WHY the plan is fetched before anything is drawn: it carries every label,
    // every rect and every derived ink for this frame, and fetching it is the only
    // place in this function where anything other than a pixel test happens.
    let f = frame(x, y, state);

    // WHY a flat backdrop: the previous file filled only its content rectangles and
    // left the rest showing whatever the view underneath had drawn, so pulling the
    // shade down over a light app tore the surface. A shade owns the whole surface.
    let mut colour = theme.bg;

    // Status bar: the clock, and only the clock. WHY no signal or battery glyphs:
    // `LauncherState` carries no reading for either. Both runs are absent when the
    // kernel refused the RTC read, which is the same condition the old code gated
    // them on inside the pixel function.
    if let Some(c) = f.clock.as_ref().and_then(|r| r.pixel(x, y)) {
        colour = c;
    }

    // Header: the real date line. WHY not the old "Date unavailable" literal:
    // `state.date_line` is already the answer -- spaces when the kernel refused the
    // RTC read -- so the shell says nothing rather than printing a sentence about
    // itself.
    if let Some(c) = f.date.as_ref().and_then(|r| r.pixel(x, y)) {
        colour = c;
    }

    // The shade's dismiss affordance, drawn inside the quadrant `main.rs` acts on.
    if let Some((dx, dy)) = in_box(f.close, x, y) {
        if in_ring(dx * dx + dy * dy, f.close_arm, 1) || (dx.abs() <= 5 && on_cross(dx, dy, 1)) {
            colour = theme.text_muted;
        }
    }

    // Connectivity block.
    for tile in f.big.iter() {
        if let Some(c) = tile_pixel(tile, x, y, &theme) {
            colour = c;
        }
    }

    // The Power band. `main.rs` routes a tap here to the Power menu, so the label
    // names the actions that live there rather than pretending to be one of them.
    if rounded_contains(f.power, x, y, radius::CARD) {
        colour = theme.surface_raised;
        if rounded_outline(f.power, x, y, radius::CARD) {
            colour = theme.border;
        }
        if let Some(c) = f.power_name.pixel(x, y) {
            colour = c;
        }
        if let Some(c) = f.power_sub.pixel(x, y) {
            colour = c;
        }
    }

    // Beside the Power band, and deliberately not a tile: the notification count is
    // the one thing in this shade the shell can state without inventing anything.
    // It is a readout, so it carries no border a finger could mistake for a button,
    // and it stays clear of the Power tile's rectangle on both sides.
    if let Some(c) = f.readout_name.pixel(x, y) {
        colour = c;
    }
    if let Some(c) = f.readout_value.pixel(x, y) {
        colour = c;
    }

    // Level controls. WHY neither has a filled portion: the old file filled both
    // tracks to two thirds, which reads as a measured level. There is no brightness
    // reading and no volume reading on this surface, so both tracks are empty.
    for slider in f.sliders.iter() {
        if let Some(c) = slider_pixel(slider, x, y, &theme) {
            colour = c;
        }
    }

    // Foot tiles.
    for tile in f.small.iter() {
        if let Some(c) = tile_pixel(tile, x, y, &theme) {
            colour = c;
        }
    }

    // Home indicator.
    if rounded_contains(f.home, x, y, radius::CHIP) {
        colour = f.home_ink;
    }

    colour
}

/// One control tile: outline, icon, name, and the caption its capability earns.
///
/// WHY an outline and never a fill: the fill is the reference's signal for "this is
/// live". Filling a tile whose control has no capability is the single most
/// misleading thing this screen could do, so every tile takes the outline path and
/// the caption says which of the two absences it is.
///
/// WHY it still returns the tile's fill rather than only its ink: the caller paints
/// a flat backdrop first and this tile's background is opaque, so a pixel inside the
/// tile has to be told what the tile looks like even when no label covers it.
fn tile_pixel(plan: &TilePlan, x: usize, y: usize, theme: &Theme) -> Option<u32> {
    if !rounded_contains(plan.rect, x, y, radius::CARD) {
        return None;
    }
    let mut colour = theme.bg;
    if rounded_outline(plan.rect, x, y, radius::CARD) {
        colour = theme.border;
    }
    if let Some(c) = glyph_pixel(plan.glyph, plan.icon, x, y, theme.text_muted) {
        colour = c;
    }
    if let Some(c) = plan.name.pixel(x, y) {
        colour = c;
    }
    if let Some(c) = plan.caption.pixel(x, y) {
        colour = c;
    }
    Some(colour)
}

/// A level control with no level. WHY the track is a well and not a slider: a
/// slider drawn at zero reads as "muted", which is a reading.
fn slider_pixel(plan: &SliderPlan, x: usize, y: usize, theme: &Theme) -> Option<u32> {
    if !rounded_contains(plan.track, x, y, radius::CONTROL) {
        return None;
    }
    let mut colour = theme.surface_sunken;
    if rounded_outline(plan.track, x, y, radius::CONTROL) {
        colour = theme.border;
    }

    // The end caps the reference puts at each end of the track: the control's icon
    // at the low end, and the same icon dimmed at the high end.
    for (index, cap_box) in plan.caps.iter().enumerate() {
        if let Some(c) = glyph_pixel(plan.glyph, *cap_box, x, y, plan.cap_ink[index]) {
            colour = c;
        }
    }

    if let Some(c) = plan.name.pixel(x, y) {
        colour = c;
    }
    if let Some(c) = plan.caption.pixel(x, y) {
        colour = c;
    }
    if let Some(c) = plan.reason.pixel(x, y) {
        colour = c;
    }
    Some(colour)
}
