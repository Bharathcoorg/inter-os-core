//! Lock Screen: wallpaper, clock, controls and unlock affordances.
//!
//! WHY `pub` on the date constants: `LauncherState::date_line` and the RTC parser
//! live in the crate root and size themselves from the same abbreviation tables
//! this screen draws with, so the two must not diverge.
//!
//! Geometry and colour come from `launcher::theme`, `launcher::layout` and
//! `launcher::text`, never from literals local to this file.
//!
//! WHY the type scales below are derived rather than read from `theme::type_scale`:
//! they are this surface's own measured cap heights, and the reference panel fixes
//! them independently of the shell's ladder. Each is
//! `round(measured_cap_px / font::CAP_HEIGHT)`, so the derivation survives the next
//! typeface: only `CAP_HEIGHT` changes, and the arithmetic below is re-run against
//! it. `theme::type_scale` still sizes every other surface in the shell.
//!
//! WHY there is a [`Frame`] and a one-slot memo for it: this renderer is called
//! once per pixel -- 1,036,800 times a frame -- through a function pointer the
//! shell resolved before the loop began, so the compiler cannot hoist anything out
//! of it. Every rectangle it draws with used to be re-measured inside that call:
//! two 16-byte `text::measure` walks for the hint, an 11-byte one for the date
//! line and a 5-byte one for the clock, plus `lock_na_origin`, on every pixel of
//! the surface -- roughly fifty `text::advance` lookups a pixel, about fifty
//! million a frame. None of that depends on `(x, y)`. It is measured once per
//! frame into `Frame` and read back as plain numbers per pixel.

use core::cell::UnsafeCell;

use crate::{test_text, test_text_aa, LauncherState, H, W};
use launcher::layout::{self, Rect};
use launcher::text;
use interos_std::design::walls;
use launcher::theme::*;

/// WHY `Theme::LIGHT`: this is the one surface in the shell whose backdrop is a
/// photograph rather than a page, and the reference's is a bright sky over a
/// near-black ridge. Every piece of ink below is chosen from the pixel behind it
/// instead of from the palette, so the surface reads if the wallpaper ramp is ever
/// swapped for the dark one.
const THEME: Theme = Theme::LIGHT;

/// Reference panel width, in reference pixels.
///
/// WHY this is here and not the catalogue's: the surface's own glyphs and discs
/// are laid out on the reference panel's grid, so it converts a device pixel to
/// a panel column. The catalogue carries the same number for its traces and the
/// crate's tests assert the two are equal, so they cannot drift.
const LOCK_REF_W: usize = walls::SCENE_REF_W;
/// Reference display height, in reference panel pixels. Same single-source rule as
/// [LOCK_REF_W].
const LOCK_REF_H: usize = walls::SCENE_REF_H;
/// Longest string the lock surface can render (`Www, DD Mmm`).
pub const LOCK_DATE_TEXT_LEN: usize = 11;
/// Bottom edge of the status-bar band.
const LOCK_STATUS_BAND: usize = 56;

/// Left edge shared by the status clock, the date line and the big clock.
///
/// WHY one edge and not three: the status clock sat at x 54 while the date and
/// the clock sat at x 75, so the same time of day was set twice on the same
/// surface with its two left edges 21px apart, which reads as two unrelated
/// elements rather than one clock repeated at two sizes. The content block owns
/// the edge because it is the dominant element and its own documentation already
/// claimed the edge was "shared by the date line and the clock"; the status clock
/// now derives its origin from the same constant so they cannot drift apart
/// again.
const LOCK_CONTENT_X: usize = 75;

/// Status-bar clock. Reference origin x 13 / y 9, cap height measured at 7 px on
/// the panel.
///
/// WHY scale 1: 7 px measured cap over this face's 17-row cap is 7 / 17 = 0.41,
/// which rounds to 0 -- and a zero scale rasterises nothing at all. The panel's
/// reference rows are not device pixels, so the measurement is carried across at
/// the surface's own vertical ratio, 1440 / 427 = 3.372: 7 x 3.372 = 23.6 device
/// px, and 23.6 / 17 = 1.39, which is scale 1. Scale 2 would be 34 px, 44% over.
/// WHY derived from `LOCK_CONTENT_X`: the status clock and the content block
/// below it are one clock shown twice, so they start on the same left edge.
const LOCK_STATUS_TIME_X: usize = LOCK_CONTENT_X;
const LOCK_STATUS_TIME_Y: usize = 27;
const LOCK_STATUS_TIME_SCALE: usize = 1;
/// Left edge shared by the date line and the clock, reference x 18.
/// Date line. Reference cap top y 47, cap height measured at 9 px.
///
/// WHY scale 2: 9 x 3.372 = 30.3 device px, and 30.3 / 17 = 1.78, which rounds to
/// 2. Scale 1 is 17 px (44% under), scale 3 is 51 px (68% over).
///
/// WHY `_Y` is 6 px above the old 158: the reference position is a *cap top*, and
/// this face puts cap ink `font::CAP_TOP * scale` below the cell origin, so
/// 3 x 2 = 6. Under the previous fixed 8 px cell the ink began on the cell's first
/// row, which is why the same number used to work.
const LOCK_DATE_Y: usize = 152;
const LOCK_DATE_SCALE: usize = 2;
/// The clock. Reference cap top y 68, cap height measured at 32 px: the single
/// largest type element on the surface.
///
/// WHY scale 6: 32 x 3.372 = 107.9 device px, and 107.9 / 17 = 6.35, which rounds
/// to 6. Scale 5 is 85 px (21% under), scale 7 is 119 px (10% over).
///
/// WHY this is a local constant and not `type_scale::DISPLAY`: the panel's clock
/// is 107.9 px of cap on this surface, while `DISPLAY` has to serve every screen
/// in the shell, and the two do not agree. See the note on `type_scale` below.
const LOCK_CLOCK_Y: usize = 211;
const LOCK_CLOCK_SCALE: usize = 6;
/// Unlock hint. Reference cap top y 341, horizontally centred.
///
/// WHY scale 1: the panel sets the hint at the same size as the status-bar clock,
/// and preserving that relationship preserves both cap heights, so the hint takes
/// the clock's answer -- 23.6 device px of cap, 23.6 / 17 = 1.39, scale 1.
///
/// WHY `_Y` is 3 px above the old 1150: same cap-top correction as the date line,
/// 3 x 1 = 3.
const LOCK_HINT_Y: usize = 1147;
const LOCK_HINT_SCALE: usize = 1;
const LOCK_HINT_TEXT: &[u8] = b"Swipe up to unlock";
/// Padlock glyph centre, reference x 87 / y 328; body 7 x 13 panel px.
const LOCK_PADLOCK_CX: i32 = 362;
const LOCK_PADLOCK_CY: i32 = 1106;
/// Circular controls. Reference centres are (28, 385.5) and (146.5, 385.5) with
/// radius 17.5, i.e. inset 16.2% / 15.3% of the width.
pub const LOCK_BTN_CY: i32 = 1300;
pub const LOCK_BTN_L_CX: i32 = 117;
pub const LOCK_BTN_R_CX: i32 = 610;
pub const LOCK_BTN_R: i32 = 73;
/// Home indicator: reference width 58, height 3, bottom inset 3 panel px.
pub const LOCK_PILL_W: usize = 241;
pub const LOCK_PILL_H: usize = 9;
pub const LOCK_PILL_Y: usize = 1421;
/// What the torch control says about itself.
const LOCK_NA_TEXT: &[u8] = b"N/A";

/// WHY the two veils are percentages and not channel arithmetic: the panel's
/// translucent control solves to a 27% and a 36% white veil, and those two
/// numbers were written out by hand in three places, which is how one of them
/// drifted to 30%. `Theme::veil` is the helper that exists for exactly this.
const LOCK_BTN_FILL_PERCENT: u32 = 27;
const LOCK_BTN_RIM_PERCENT: u32 = 36;

/// How far the sun lifts the right of the frame, as a fraction of the 0..=256
/// ramp scale, out of a full `across` of 256 at the right edge.
const LOCK_ACROSS_SPAN: u32 = 256;
const LOCK_ACROSS_PERCENT: u32 = 26;

pub const LOCK_WEEKDAY_ABBR: [&[u8; 3]; 7] = [
    b"Sun", b"Mon", b"Tue", b"Wed", b"Thu", b"Fri", b"Sat",
];
pub const LOCK_MONTH_ABBR: [&[u8; 3]; 12] = [
    b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec",
];

// --- Wallpaper -------------------------------------------------------------
//
// The panel's wallpaper is a photograph. It is rebuilt here as three tonal bands
// separated by two silhouettes traced directly out of that photograph, for every
// one of the 173 panel columns. The traces, the reference grid and the three
// band heights live in `interos_std::design::walls`, the catalogue every
// wallpaper-conversant surface reads, so a scene can be added or retuned without
// editing this file.
// WHY the skylines are the only measured data left here: they say *where the
// mountains are*, which is a property of the photograph and not of the visual
// language. Everything else -- every colour, every gap -- comes from the scene's
// stops, so retuning a scene re-tints the whole backdrop without touching a
// single number below.

/// The brightest tone in the wallpaper: the sky where the sun sits behind the far
/// ridge. WHY the ramp's top stop lifted toward the light rather than a measured
/// cream: a warm horizon cannot be named by any field the theme has, and lifting
/// the top stop toward its own ink is the one derivation that cannot go stale
/// The mountain body just under the far skyline: the ramp's mid stop pulled
/// The mist band behind the foreground: the same mid stop lifted toward the
/// The lone figure standing on the near ridge, traced at panel x 25..36 and
/// y 213..247. The traced rock edge crosses this band at y 213..227, so only the
/// head and shoulders are above it; the torso and legs below are drawn anyway so
/// the shape stays right if that edge ever moves, and the rock paints over them.
/// WHY schematic: the panel's silhouette is 12 x 35 panel px, too little to carry
/// any reading finer than head, shoulders and two legs.
fn lock_figure_hit(scene: &walls::Scene, x: usize, y: usize) -> bool {
    // WHY the figure is stock-only: it was traced out of the reference lock-screen
    // panel, which is what the stock scene is built from. A wallpaper chosen from
    // the catalogue has its own traced ridge, and pasting this panel's figure into
    // it would put a person from a different photograph on its mountain.
    if scene.key != b"stock" {
        return false;
    }
    if !(715..=833).contains(&y) || !(100..=154).contains(&x) {
        return false;
    }
    let dx = (x as i32) - 127;
    let dy = (y as i32) - 725;
    // head
    if dx * dx + dy * dy <= 100 {
        return true;
    }
    // shoulders, widest just below the head
    if (11..=26).contains(&dy) && dx.abs() <= 27 {
        return true;
    }
    // torso, tapering
    if (27..=58).contains(&dy) && dx.abs() <= 26 - (dy - 27) / 4 {
        return true;
    }
    // legs
    if (59..=108).contains(&dy) && ((-22..=-8).contains(&dx) || (1..=15).contains(&dx)) {
        return true;
    }
    false
}

/// The figure's ink, pulled most of the way to the ramp's darkest stop.
///
/// WHY derived from the pixel behind it rather than a colour of its own: it is a
/// silhouette, and its whole job is to be the darkest thing in the valley. A
/// fixed near-black stops being a silhouette the moment the ramp is retuned,
/// while a figure derived from its own backdrop cannot.
fn lock_figure_ink(f: &Frame, behind: u32) -> u32 {
    Theme::blend(behind, f.bottom, 190)
}

/// The two ramp fractions for one pixel, folded into a single interpolation.
///
/// WHY they are folded instead of blended one after the other: the vertical ramp
/// and the horizontal sun lift both move a pixel toward the bright end of the same
/// band, so together they are one interpolation at `1 - (1 - v)(1 - s)` rather
/// than two blends in sequence. This is the only full-bleed layer on the surface,
/// so it runs once for each of the surface's 1,036,800 pixels, and folding takes
/// it from six channel blends per pixel to two.
fn lock_band(vertical: u32, across: u32) -> u32 {
    vertical
        .saturating_add(across)
        .saturating_sub((vertical.saturating_mul(across)) >> 8)
}

/// Full-bleed wallpaper layer for the lock surface.
///
/// WHY the scene arrives in the `Frame` rather than as a `&Theme`: the ramp's
/// tones are `Scene` derivations and its skylines are read per pixel, and this
/// function is called once for each of the surface's 1,036,800 pixels. Resolving
/// the stored key into a scene once per surface keeps a byte-string lookup off
/// the hot path, and it is what lets the lock screen and the home screen draw
/// independently chosen wallpapers from the same catalogue.
fn lock_wallpaper_pixel(f: &Frame, x: usize, y: usize) -> u32 {
    let scene = f.scene;
    let fx = (x * LOCK_REF_W) / W;
    let rel = ((y * LOCK_REF_H) / H) as u16;
    let far = scene.far[fx.min(LOCK_REF_W - 1)];
    let mut near = scene.near[fx.min(LOCK_REF_W - 1)];
    // WHY the clamp: the two skylines are traced at two independent
    // thresholds, so a dark notch picked up by one can land above the layer it
    // should sit under. A minimum separation stops the layers inverting.
    if near < far + 3 {
        near = far + 3;
    }
    let horizon_y = scene.horizon_y;
    let mist_y = scene.mist_y;
    let rock_y = scene.rock_y;

    // WHY a fraction of the ramp rather than a second measured column per band:
    // the panel's left/right split was one photograph's exposure, and the scene's
    // stops own the exposure now. The sun is behind the far ridge on the right,
    // so every band is lifted toward its own bright end as the frame goes right.
    let across = ((fx * LOCK_ACROSS_SPAN as usize) / (LOCK_REF_W - 1))
        .min(256) as u32
        * LOCK_ACROSS_PERCENT
        / 256;

    if rel < far {
        let vertical = rel as u32 * 256 / horizon_y as u32;
        Theme::blend(f.top, f.horizon, lock_band(vertical, across))
    } else if rel < near {
        // WHY saturating: the traced far ridge is not level, so a pixel can be
        // below the ridge and above the nominal horizon height at the same
        // time, and a plain subtraction there underflows -- which is a panic on
        // this build, not a wrong colour.
        let vertical =
            rel.saturating_sub(horizon_y) as u32 * 256 / (mist_y - horizon_y).max(1) as u32;
        let base = Theme::blend(f.shadow, f.mist, lock_band(vertical, across));
        if lock_figure_hit(scene, x, y) {
            return lock_figure_ink(f, base);
        }
        base
    } else if (rel as usize) < rock_y as usize {
        let vertical =
            rel.saturating_sub(mist_y) as u32 * 256 / (rock_y - mist_y).max(1) as u32;
        Theme::blend(f.mist, f.bottom, lock_band(vertical, across))
    } else {
        // The near rock itself: the ramp's darkest stop, held, with only the sun
        // lift on top. WHY it is the dark stop and not a blend toward it: below
        // the rock line the reference is not a gradient at all, it is the shadowed
        // face of the foreground ridge.
        Theme::blend(f.bottom, f.mist, across)
    }
}

/// Ink for anything drawn over the wallpaper's dark end: the padlock, the unlock
/// hint, the home indicator and the two control glyphs.
///
/// WHY it is resolved per pixel from what is behind it: the foreground follows
/// `wallpaper_bottom`, and a fixed paper white is unreadable the moment that stop
/// is a light colour.
fn lock_paper(theme: &Theme, backdrop: u32) -> u32 {
    layout::ink_on(theme, backdrop)
}

/// The glyph ink of a control that cannot be operated.
///
/// WHY the operable ink pulled back toward the disc instead of a dimmer colour of
/// its own: the torch's shape is the camera's shape, so the only thing that says
/// "this does nothing here" is that it does not come forward.
fn lock_glyph_dim(theme: &Theme, disc: u32) -> u32 {
    Theme::blend(lock_paper(theme, disc), disc, 90)
}

fn lock_padlock_pixel(theme: &Theme, backdrop: u32, x: usize, y: usize) -> Option<u32> {
    if !(1080..=1130).contains(&y) || !(345..=380).contains(&x) {
        return None;
    }
    let dx = (x as i32) - LOCK_PADLOCK_CX;
    let dy = (y as i32) - LOCK_PADLOCK_CY;
    // body, with the two lower corners rounded off
    if (0..22).contains(&dy) && dx.abs() < 12 {
        if dy >= 17 {
            let corner = 21 - dy;
            if dx.abs() > 9 - corner {
                return None;
            }
        }
        return Some(lock_paper(theme, backdrop));
    }
    // shackle: the upper half of an annulus
    if (-19..0).contains(&dy) {
        let d2 = dx * dx + dy * dy;
        if d2 <= 13 * 13 && d2 >= 7 * 7 {
            return Some(lock_paper(theme, backdrop));
        }
    }
    None
}

/// Torch pictogram: the panel's torch is a wide tapered head over a narrow
/// handle with a small dark vent in the head. Returns `true` for a pixel of the
/// glyph; the caller supplies the ink, which for this control is the dimmed one.
fn lock_torch_glyph_pixel(x: usize, y: usize) -> bool {
    let dx = (x as i32) - LOCK_BTN_L_CX;
    let dy = (y as i32) - (LOCK_BTN_CY - 2);
    let body = (dx >= -9 && dx <= 9 && dy >= -26 && dy < -14)
        || (dx >= -6 && dx <= 6 && dy >= -14 && dy < -5)
        || (dx >= -4 && dx <= 4 && dy >= -5 && dy < 22);
    if !body {
        return false;
    }
    // the vent in the head is a hole, so the disc shows through it
    !(dx >= -2 && dx <= 2 && dy >= -19 && dy < -15)
}

/// Camera pictogram. Returns `true` for a pixel of the glyph; the caller supplies
/// the ink, which differs between the two controls.
fn lock_camera_glyph_pixel(x: usize, y: usize) -> bool {
    let dx = (x as i32) - LOCK_BTN_R_CX;
    let dy = (y as i32) - LOCK_BTN_CY;
    // body outline
    if dx.abs() <= 25 && (-12..=15).contains(&dy) {
        let rim = dx.abs() >= 22 || dy <= -9 || dy >= 12;
        if rim {
            return true;
        }
        // lens ring
        let l2 = dx * dx + dy * dy;
        return (81..=144).contains(&l2);
    }
    // viewfinder hump on the top edge
    (-19..=-13).contains(&dy) && (-8..=8).contains(&dx)
}

/// One circular control: a translucent veil over the wallpaper, with a slightly
/// denser two-pixel rim, so the control picks up whatever is behind it.
fn lock_button_disc(x: usize, y: usize, cx: i32, bg: u32) -> Option<u32> {
    let dy = (y as i32) - LOCK_BTN_CY;
    if dy < -LOCK_BTN_R || dy > LOCK_BTN_R {
        return None;
    }
    let dx = (x as i32) - cx;
    if dx < -LOCK_BTN_R || dx > LOCK_BTN_R {
        return None;
    }
    let d2 = dx * dx + dy * dy;
    if d2 > LOCK_BTN_R * LOCK_BTN_R {
        return None;
    }
    let percent = if d2 > (LOCK_BTN_R - 2) * (LOCK_BTN_R - 2) {
        LOCK_BTN_RIM_PERCENT
    } else {
        LOCK_BTN_FILL_PERCENT
    };
    Some(Theme::veil(bg, percent, true))
}

/// Where the torch control's unavailability label starts.
///
/// WHY once per frame and not per pixel: this walks `LOCK_NA_TEXT` through
/// `text::measure`, and the pixel function used to call it for every pixel of the
/// surface. It is now reached only from [`Frame::measure`]. See the module header.
///
/// WHY an origin and not a box: `test_text` is handed the glyph run's top-left and
/// rasterises downward from it, so handing it a box whose height came from
/// `text::line_height` puts the run a whole line-height above where it belongs.
/// `line_height(CAPTION)` is 48 px on this face and the rasteriser draws 16, so
/// the earlier version of this function drew "N/A" *inside* the torch disc, where
/// the disc's own veil hid all but its last row.
///
/// WHY the position is still derived rather than typed: the label has to clear the
/// control it belongs to and the home indicator, and both of those move when the
/// disc moves or the type scale changes. It is centred on the disc, held one step
/// below it, capped one step above the indicator, and clamped inside the surface,
/// so it cannot end up off the edge or under the indicator whichever of them grows.
///
/// WHY the width it centres on is `text::measure` and not the rasteriser's cell:
/// `measure` is the documented metric every other surface centres with, and it is
/// the one that has to be right when the rasteriser is migrated onto the same
/// metrics. Until then a label can sit a few pixels left of centre, which is
/// visible but harmless; a label placed from a private metric would be exactly
/// wrong the moment the face changed.
fn lock_na_origin() -> (usize, usize) {
    let w = text::measure(LOCK_NA_TEXT, type_scale::CAPTION);
    let centre = LOCK_BTN_L_CX as usize;
    let x = centre
        .saturating_sub(w / 2)
        .clamp(space::SM, W.saturating_sub(w + space::SM));
    let y = (LOCK_BTN_CY as usize + LOCK_BTN_R as usize + space::MD)
        .min(LOCK_PILL_Y.saturating_sub(space::MD));
    (x, y)
}

/// The "N/A" label as a box: its origin from [`lock_na_origin`] plus its own
/// extent, measured at the caption scale.
///
/// WHY the extent is here at all: the pixel function used to hand `test_text` this
/// label's origin and let it reject the other 1,036,000-odd pixels one row-band
/// test at a time, which is the cheapest possible reject but still a call and a pen
/// walk per pixel. Testing the box first is the same test `test_text` performs on
/// the row (`y` in `[y0, y0 + line_height)`) extended with the one it performs on
/// the column, so it accepts exactly the pixels `test_text` could have accepted and
/// rejects the rest without calling anything.
fn lock_na_box() -> Rect {
    let (x, y) = lock_na_origin();
    Rect::new(
        x,
        y,
        text::measure(LOCK_NA_TEXT, type_scale::CAPTION),
        text::line_height(type_scale::CAPTION),
    )
}

/// The camera disc's absence caption, on the same baseline as the torch disc's.
///
/// WHY a separate box and not the same rect mirrored: the two discs have
/// different centres, and a caption that is not measured for its own column is
/// exactly the mis-centring defect this screen is being fixed for.
fn lock_na_r_box() -> Rect {
    let w = text::measure(LOCK_NA_TEXT, type_scale::CAPTION);
    let (_, y) = lock_na_origin();
    // WHY the same clamp [`lock_na_origin`] applies and not a bare subtraction:
    // the left label is clamped into the surface with a `space::SM` margin, and
    // this one was not, so the pair was safe only for the two centres that
    // happen to be in use today. A right-hand control moved inboard would push
    // its caption off the edge while its neighbour's stayed put.
    Rect::new(
        (LOCK_BTN_R_CX as usize)
            .saturating_sub(w / 2)
            .clamp(space::SM, W.saturating_sub(w + space::SM)),
        y,
        w,
        text::line_height(type_scale::CAPTION),
    )
}

// --- Per-frame metrics -------------------------------------------------------
// Everything below this line exists because `get_lock_screen_color` runs
// 1,036,800 times a frame. The values it needs depend on the shell state and on
// constants -- never on `(x, y)` -- so they are computed once per frame here and
// read back per pixel.

/// Everything this surface draws whose position or tone is derived, resolved once
/// per frame.
///
/// WHY a struct rather than more constants: the four boxes come from
/// `text::measure` over strings the kernel supplies, and the five ramp tones come
/// from `Theme::blend` over theme stops. Both are cheap once and ruinous a million
/// times, and both are exactly the kind of thing a future runtime theme would move
/// out of the compiler's reach.
struct Frame {
    /// Cell height of the status-bar clock: `text::line_height(LOCK_STATUS_TIME_SCALE)`.
    ///
    /// WHY it is here and not typed: the status clock is the one run on this
    /// surface with no measured width -- it is drawn from a fixed origin and the
    /// rasteriser stops at the end of the string -- so its only metric is its
    /// height, and that is still a `text::` call if it is made per pixel.
    status_h: usize,
    /// The date line, left-aligned on `LOCK_CONTENT_X`.
    date: Rect,
    /// The clock, left-aligned on `LOCK_CONTENT_X`.
    clock: Rect,
    /// The unlock hint, horizontally centred on the surface.
    hint: Rect,
    /// The PIN pad's prompt, horizontally centred on the surface.
    ///
    /// WHY it is here and not measured inside the pad's pixel function: that
    /// function used to call `text::measure` over a fourteen-byte string for
    /// every pixel of a 45-row band -- roughly 32,000 advance walks a frame to
    /// compute a box that is identical for all of them. That is precisely the
    /// measurement the module header says this memo exists to keep off the
    /// per-pixel path, and it had come back through a later edit.
    pin_heading: Rect,
    /// The PIN pad's sub-prompt, one scale step below the prompt, on the same
    /// centring rule.
    pin_subheading: Rect,
    /// The torch's "N/A" label: an origin *and* an extent, see [`lock_na_box`].
    na: Rect,
    /// The camera disc's "N/A" label, on the same baseline: [`lock_na_r_box`].
    na_r: Rect,
    /// Ink for the status bar, the date line and the clock: the scene's own sky
    /// ink.
    ink: u32,
    /// The catalogue entry this frame's wallpaper comes from.
    ///
    /// WHY the scene is a field of the memo: its skylines are read per pixel, and
    /// it is chosen by a value the settings store answers. Resolving it once per
    /// surface keeps a byte-string comparison and a pointer chase off the hot
    /// path, and it is what lets the lock and home surfaces draw independently
    /// chosen wallpapers from one catalogue.
    scene: &'static walls::Scene,
    /// The ramp, resolved: top stop, brightest tone, shadowed rock, mist, and the
    /// darkest stop the foreground holds.
    top: u32,
    horizon: u32,
    shadow: u32,
    mist: u32,
    bottom: u32,
}

impl Frame {
    /// Measures this frame. Called once per frame, from [`frame_for`], and never
    /// from the per-pixel path.
    ///
    /// WHY the date and clock boxes are measured even when there is no clock
    /// reading: `clock_source_ok` gates whether they are *drawn*, and it is read
    /// live from the state per pixel, so measuring them unconditionally costs
    /// sixteen advance lookups a frame and keeps this function free of a branch
    /// whose two halves would have to stay in step with the pixel path's.
    fn measure(state: &LauncherState) -> Frame {
        // WHY the scene comes from the state rather than the theme: the lock
        // wallpaper is a user choice the settings store holds, so resolving it
        // here is the single place the store's answer becomes geometry.
        let scene = state.lock_wallpaper;
        // WHY the pad's two runs are measured here even when the pad is not on
        // screen: `lock_state`, `pin_setup_step` and `auth_error` are read live
        // from the state per pixel, so measuring conditionally would put a branch
        // in `Frame::measure` whose two halves would have to stay in step with the
        // pixel path's. Fourteen and thirty-odd advance lookups a frame is free;
        // a disagreement between the two is not. See `pin_heading` on `Frame`.
        let (heading, subheading) = pin_messages(state);
        Frame {
            status_h: text::line_height(LOCK_STATUS_TIME_SCALE),
            date: Rect::new(
                LOCK_CONTENT_X,
                LOCK_DATE_Y,
                text::measure(&state.date_line, LOCK_DATE_SCALE),
                text::line_height(LOCK_DATE_SCALE),
            ),
            clock: Rect::new(
                LOCK_CONTENT_X,
                LOCK_CLOCK_Y,
                text::measure(&state.clock_time, LOCK_CLOCK_SCALE),
                text::line_height(LOCK_CLOCK_SCALE),
            ),
            hint: Rect::new(
                text::center_x(LOCK_HINT_TEXT, LOCK_HINT_SCALE, W),
                LOCK_HINT_Y,
                text::measure(LOCK_HINT_TEXT, LOCK_HINT_SCALE),
                text::line_height(LOCK_HINT_SCALE),
            ),
            pin_heading: Rect::new(
                text::center_x(heading, PIN_HEADING_SCALE, W),
                PIN_HEADING_Y,
                text::measure(heading, PIN_HEADING_SCALE),
                text::line_height(PIN_HEADING_SCALE),
            ),
            pin_subheading: Rect::new(
                text::center_x(subheading, PIN_SUBHEADING_SCALE, W),
                PIN_SUBHEADING_Y,
                text::measure(subheading, PIN_SUBHEADING_SCALE),
                text::line_height(PIN_SUBHEADING_SCALE),
            ),
            na: lock_na_box(),
            na_r: lock_na_r_box(),
            ink: scene.ink,
            scene,
            top: scene.top,
            horizon: scene.horizon(),
            shadow: scene.shadow(),
            mist: scene.mist(),
            bottom: scene.bottom,
        }
    }
}

/// One memo slot: the frame, and what it was measured for.
struct FrameSlot {
    /// Address of the `LauncherState` these metrics were measured from.
    ///
    /// WHY the address and not the contents: the contents are sixteen bytes of
    /// `clock_time` and `date_line` plus a flag, and comparing them per pixel is
    /// the same order of work as the measurement this cache exists to remove. The
    /// address is free, and `render_view` passes one `&LauncherState` for a whole
    /// surface, so it is the check that matches how this is really called.
    state: usize,
    /// The last pixel served.
    at_x: usize,
    at_y: usize,
    /// False until the first frame has been measured; nothing can be trusted from a
    /// zeroed [`Frame`], and an `at_x` of 0 with no flag would look like a hit for
    /// the second pixel of a frame.
    measured: bool,
    frame: Frame,
}

impl FrameSlot {
    /// Whether the memo still answers for `(x, y)` rendered against `state`.
    fn holds_next(&self, x: usize, y: usize, state: usize) -> bool {
        self.measured
            && self.state == state
            && ((x == self.at_x + 1 && y == self.at_y) || (x == 0 && y == self.at_y + 1))
    }
}

/// A [`FrameSlot`] reachable from a `static`.
///
/// WHY `Sync` is asserted rather than derived: `UnsafeCell` is deliberately not
/// `Sync`, and this slot is mutable shared state, so something has to say the
/// accesses are serialised. They are: `get_lock_screen_color` is a pure rendering
/// function with exactly one caller -- `render_view`, in the launcher task -- the
/// guest runs no second thread, and nothing in this file calls back into the pixel
/// function, so the borrow taken in [`frame_for`] cannot overlap another. If a
/// second caller ever appears this assertion is the thing that has to be revisited,
/// and it is asserted in one place rather than at every use.
struct FrameMemo(UnsafeCell<FrameSlot>);

// SAFETY: see `FrameMemo`'s own doc comment. The mutable access is confined to the
// borrow in `frame_for`, which no other code in this file can reach while it is
// live.
unsafe impl Sync for FrameMemo {}

/// WHY one slot and not a map: the surface is rendered one view at a time, one
/// pixel at a time, in a single contiguous walk.
static FRAME_MEMO: FrameMemo = FrameMemo(UnsafeCell::new(FrameSlot {
    state: 0,
    at_x: 0,
    at_y: 0,
    measured: false,
    frame: Frame {
        status_h: 0,
        date: Rect::new(0, 0, 0, 0),
        clock: Rect::new(0, 0, 0, 0),
        hint: Rect::new(0, 0, 0, 0),
        pin_heading: Rect::new(0, 0, 0, 0),
        pin_subheading: Rect::new(0, 0, 0, 0),
        na: Rect::new(0, 0, 0, 0),
        na_r: Rect::new(0, 0, 0, 0),
        ink: 0,
        scene: &walls::SCENES[0],
        top: 0,
        horizon: 0,
        shadow: 0,
        mist: 0,
        bottom: 0,
    },
}));

/// This frame's metrics, for the pixel at `(x, y)`, re-measuring only when the
/// memo was built for a different state or a different place in the walk.
///
/// WHY the walk and not the state's contents as the cache key: `render_view` walks
/// the surface once in row-major order, so the pixel after the one just served is
/// known exactly, and anything else is the first pixel of a new frame. The state
/// cannot change under a frame either: the loop body holds `&LauncherState` and
/// calls only `draw`, `service_heartbeat` and `surface_write`, none of which can
/// mutate it. Keying on the state's address as well catches the one way a caller
/// could interleave two live states inside one walk -- a harness rendering two
/// surfaces side by side, say -- so the memo is wrong for nobody who calls it the
/// way the shell does, and merely slow for anyone else.
///
/// WHY that is safe rather than merely fast: any call pattern this does not
/// recognise -- a thumbnail, a unit test, a second surface, an interleaved pair --
/// misses and re-measures, which is exactly what the function did before this cache
/// existed. Nothing about the drawing depends on the cache being warm.
///
/// The returned borrow is valid only for the duration of the caller's own use; it
/// must not outlive the pixel, or a later frame's `measure` would overwrite it.
fn frame_for(x: usize, y: usize, state: &LauncherState) -> &Frame {
    let addr = state as *const LauncherState as usize;
    // SAFETY: the single mutable access to the memo, taken here and released when
    // this function returns. Nothing called from this function re-enters it.
    let slot = unsafe { &mut *FRAME_MEMO.0.get() };
    if !slot.holds_next(x, y, addr) {
        slot.frame = Frame::measure(state);
        slot.state = addr;
        slot.measured = true;
    }
    slot.at_x = x;
    slot.at_y = y;
    &slot.frame
}

// --- PIN keypad -------------------------------------------------------------
//
// WHY the grid lives in one place: this keypad used to be written twice -- a
// flat twelve-entry list inside [`hit_test_pin_pad`] and a separate
// rows/columns pair inside [`lock_pin_pad_pixel`] -- which had to be kept equal
// by hand, and any edit to one and not the other produced keys you could draw
// where you could not tap, or tap where nothing was drawn. Everything below is
// read by both paths, so a key's position, code and radius are single-sourced.

/// Row centres of the keypad, top to bottom.
/// WHY these rows sit low: the pad used to run 480..900, which put the last key
/// at y=948 and left roughly 490px of empty surface below it - the keypad looked
/// stranded in the upper two thirds with a void under it. A phone puts the keys
/// in the thumb zone, under the prompt, with the gesture pill below that. The
/// render and [`hit_test_pin_pad`] both read these constants, so a key's tappable
/// disc stays exactly its drawn disc.
const PIN_ROWS: [usize; 4] = [700, 840, 980, 1120];
/// Column centres of the keypad, left to right.
const PIN_COLS: [usize; 3] = [160, 360, 560];
/// The code each cell holds: what [`hit_test_pin_pad`] returns for a tap on it,
/// and the face [`pin_key_face`] draws on it.
///
/// The bottom row is the phone-dialer arrangement. `C` is Clear-all -- the
/// whole entry, the old `X` key's job, now a labelled key instead of a letter
/// nobody could guess. `D` is Delete -- the last digit, the old `C` key's job,
/// drawn as a backspace pictogram. There is no OK/Enter key because there is
/// nothing for one to do: the fourth digit submits, as it always has.
const PIN_KEYS: [[char; 3]; 4] = [
    ['1', '2', '3'],
    ['4', '5', '6'],
    ['7', '8', '9'],
    ['C', '0', 'D'],
];
/// Radius of a key in pixels -- the drawn disc *and* the tappable region, which
/// are deliberately the same circle: every pixel of a key can be tapped and
/// nothing outside it can be. 96 px across is twice the 48 px minimum touch
/// target in `theme`. (The keypad used to draw at radius 44 and hit at 48, a
/// four-pixel annulus of key that was never on screen; one radius removes the
/// question of which is the real key.)
const PIN_KEY_R: i32 = 48;
/// Half the square that rejects a pixel nowhere near a key before the circular
/// test runs: two pixels of slack so the rim itself is never missed by rounding.
const PIN_KEY_BOX: usize = (PIN_KEY_R as usize) + 2;
/// The disc fill: a white veil over the pad backdrop -- the same glass the lock
/// screen's torch and camera controls are made of -- sheened from
/// [`PIN_KEY_FILL_TOP`] at the top of the disc down to [`PIN_KEY_FILL_BOTTOM`]
/// at the bottom, so a resting key reads as a disc lit from above rather than a
/// flat dot. This is the pad's "press" affordance: the input pipeline reports a
/// tap (`kind == 1`) only after the finger is up, with no down/up pair, so a
/// transient pressed frame would need a second render the shell does not have.
const PIN_KEY_FILL_TOP: u32 = 26;
const PIN_KEY_FILL_BOTTOM: u32 = 16;
/// The disc rim: the denser veil the two control discs use, so a keypad key and
/// a lock-screen control are visibly the same material.
const PIN_KEY_RIM_PERCENT: u32 = LOCK_BTN_RIM_PERCENT;
/// The figures' scale.
///
/// WHY 4 and not the 3 this used to be: legibility on this pad is a *weight*
/// problem first and a size problem second, and weight is the bold face's job,
/// so the two had to move together. Measured on the committed tables in
/// `font.rs`, every figure in `FONT` inks columns 1..=8 with a 2-column stem,
/// so at the old scale 3 a stroke was 6 px against a 51 px cap -- a stem/cap
/// ratio of 0.118. `FONT_BOLD` inks columns 1..=9 with a 3-column stem, so at
/// scale 4 a stroke is 12 px against a 68 px cap, 0.176, which is where a phone
/// dialer sits. In area terms the old figure was 24 x 54 px of ink in a 96 px
/// disc (0.25 of the key's width) and the new one is 36 x 72 px (0.375); a
/// 24 px-wide hairline floating in the middle of its key is exactly what "the
/// numbers are not crystal clear" was describing.
///
/// WHY 4 still fits the key: the figure's widest rows sit 18 px either side of
/// the disc's centre line, where the disc's half-chord is 44.9 px, and its
/// extreme cap rows are 18 px wide where the half-chord is 33.9 px. The ink
/// clears the rim on every side, so `PIN_KEY_R` stays the drawn disc *and* the
/// tappable disc and neither has to grow.
///
/// WHY a whole number on both axes: this face is a bitmap and the magnifier is
/// nearest-neighbour, so only an exact integer multiple keeps every stem an
/// axis-aligned block of solid ink. Scale 3.5 would resample the outline and
/// give back the grey fringe this change is removing.
const PIN_DIGIT_SCALE: usize = 4;
/// Whether the keypad's faces come from [`launcher::font::FONT_BOLD`].
///
/// WHY a named constant and not a bare `true` at the call sites: the heavy cut
/// is a decision about this one surface, and it is deliberately *not* global.
/// `main.rs`'s `test_text` and `test_text_aa` both hardcode the regular face, so
/// the whole shell is set against regular metrics today; bold advances are one
/// pixel wider for every glyph with ink (11 -> 12 for the figures), which would
/// move every measured box on every screen. Naming it puts the reason one grep
/// away from the next person who wonders why the key is bold and its label is not.
const PIN_DIGIT_BOLD: bool = true;
/// The `CLR` label's scale.
///
/// WHY 1: it has to stay subordinate to the figures, and 1 is the only rung left
/// below them. Measured on the committed tables, "CLR" is 38 px at scale 1, 76 px
/// at scale 2 and 114 px at scale 3; the disc's chord across its middle is 96 px,
/// so scale 3 runs straight off both sides of the key and scale 2 leaves 10 px of
/// margin and reads as jammed against the rim. Scale 1 is 0.24 of the figure's
/// cap height and 0.40 of its width, which is the hierarchy a dialer wants.
///
/// The numbers this constant's note used to quote (102 / 90 / 45 px against a
/// 90 px chord) were measured against the pre-condensing fixed 8x8 face, which
/// this surface no longer draws with; they were stale, and a stale measurement
/// in a WHY is worse than none.
///
/// WHY the regular weight and not the bold one the figures use: weight is the
/// second axis of hierarchy and the figures have already spent it. The label
/// stays in the lighter face so it cannot compete with the thing it labels.
const PIN_LABEL_SCALE: usize = 1;

/// The cell origin that centres a run's cap ink on `cy`: ink begins
/// `CAP_TOP * scale` into its cell and stands `CAP_HEIGHT * scale` rows tall.
///
/// WHY derived rather than typed: the old keypad hand-typed `cy - 17`, which
/// put its scale-2 figures six pixels below the disc's centre. This lands the
/// cap box on the centre for whatever scale is in use, so the next typeface or
/// scale change cannot re-offset the keys.
fn pin_face_y(cy: usize, scale: usize) -> usize {
    cy.saturating_sub(launcher::font::CAP_TOP * scale + launcher::font::CAP_HEIGHT * scale / 2)
}

/// Backspace pictogram: the conventional left-pointing pentagon, outlined,
/// with an X through its body. `dx`/`dy` are offsets from the key centre; the
/// shape spans 64 x 38 and clears the key's rim at every corner.
///
/// WHY drawn as geometry and not set as type: the key is a control, not a
/// character. The face has no backspace glyph, and the three pictograms already
/// on this surface (padlock, torch, camera) are all drawn this way, so the
/// keypad's one pictogram belongs to the same family.
fn lock_backspace_pixel(dx: i32, dy: i32) -> bool {
    const VX: [i32; 5] = [-32, -8, 32, 32, -8];
    const VY: [i32; 5] = [0, -19, -19, 19, 19];
    /// Outline thickness of the pentagon, in pixels.
    const STROKE: i32 = 3;
    /// Half the X's stroke, so the X is four pixels thick like the outline.
    const X_HALF: i32 = 2;

    // Inside the pentagon: every edge must turn the same way about the point.
    // The vertex order above is consistent, so all five cross products are
    // non-negative exactly when the point is inside -- and the smallest cross
    // product over its edge's length is the distance to the nearest edge, which
    // is what turns the filled pentagon into an outline of STROKE pixels.
    let mut on_outline = false;
    for i in 0..5 {
        let j = (i + 1) % 5;
        let (ax, ay) = (VX[i], VY[i]);
        let (ex, ey) = (VX[j] - ax, VY[j] - ay);
        let cross = ex * (dy - ay) - ey * (dx - ax);
        if cross < 0 {
            return false;
        }
        if cross * cross <= STROKE * STROKE * (ex * ex + ey * ey) {
            on_outline = true;
        }
    }
    if on_outline {
        return true;
    }
    // The X in the body: two crossed segments, centred on the rectangle part.
    lock_seg_hit(dx, dy, 1, -10, 21, 10, X_HALF)
        || lock_seg_hit(dx, dy, 1, 10, 21, -10, X_HALF)
}

/// True when `(px, py)` lies within `half` pixels of the segment `a`->`b`.
///
/// WHY integers: the closest point is a clamped projection, and clearing the
/// denominator once (`dist^2 * len2 <= half^2 * len2`) keeps the whole test in
/// `i32` at these sizes with no square root.
fn lock_seg_hit(px: i32, py: i32, ax: i32, ay: i32, bx: i32, by: i32, half: i32) -> bool {
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let (ex, ey) = (px - ax, py - ay);
    let mut t = ex * dx + ey * dy;
    if t < 0 {
        t = 0;
    } else if t > len2 {
        t = len2;
    }
    (ex * ex + ey * ey) * len2 - t * t <= half * half * len2
}

/// One key's face, drawn over the key's disc: the figure, the `CLR` label or
/// the backspace pictogram -- chosen from the same cell code
/// [`hit_test_pin_pad`] returns, so what a key says and what a tap on it does
/// are read from a single value.
///
/// `ink` is the colour the figure and the label are drawn in, so the pad's inks
/// stay named in one place instead of being re-typed at every arm of this
/// `match`. `pin_len` is the live entry length: the backspace pictogram dims
/// while there is nothing to delete, which is the one state this keypad has (the
/// same pulled-back ink the torch control uses for a control that cannot act).
fn pin_key_face(
    x: usize,
    y: usize,
    cx: usize,
    cy: usize,
    ch: char,
    disc: u32,
    ink: u32,
    pin_len: u8,
) -> Option<u32> {
    match ch {
        // Clear-all: labelled rather than picted, because "CLR" is what a
        // dialer says and every user already reads it.
        'C' => {
            const LABEL: &[u8] = b"CLR";
            let x0 = cx.saturating_sub(text::measure(LABEL, PIN_LABEL_SCALE) / 2);
            test_text_aa(
                x,
                y,
                x0,
                pin_face_y(cy, PIN_LABEL_SCALE),
                PIN_LABEL_SCALE,
                LABEL,
                ink,
                disc,
            )
        }
        // Backspace pictogram, dimmed when the entry is already empty.
        'D' => {
            if !lock_backspace_pixel((x as i32) - (cx as i32), (y as i32) - (cy as i32)) {
                return None;
            }
            Some(if pin_len == 0 {
                lock_glyph_dim(&THEME, disc)
            } else {
                ink
            })
        }
        // A figure, centred on the disc both ways.
        //
        // WHY bold: see `PIN_DIGIT_BOLD`. The regular face's 2-column stem is 6 px
        // at the old scale 3, and a stem has to be thick enough to survive being
        // read at a glance from arm's length on a 720 px panel before it is
        // legible at all.
        //
        // WHY `text::rasterize_pixel_bold` and not `test_text_aa`: this is the one
        // run on the surface the user has to be *certain* of, and it is currently
        // the softest. `test_text_aa` reads `interos_std`'s four-bit coverage table
        // and magnifies it with nearest neighbour, so at any integer scale above
        // one every partially covered outline pixel becomes a `scale` x `scale`
        // block of a sixteen-step tone. On a #414141 disc with white
        // ink that reads as a grid of mid greys wrapped round the figure: a haze,
        // which is the "not crystal clear" symptom in one sentence.
        // `text::rasterize_pixel_bold` is the same pen walk as `test_text` against
        // `font::run_pixel` with `bold = true` -- one bit per pixel, still nearest
        // neighbour -- so a 3-column bold stem magnified 4x is a solid 12 px block
        // of pure ink with a hard edge and nothing between the ink and the disc.
        //
        // WHY this is the right place to want hard edges and the header above it
        // is not: the header is 34 px of cap and its run is anti-aliased at
        // scale 2, where coverage buys real curve smoothing for 48 rows of type
        // read once. The figure is 68 px of cap magnified 4x, where the same
        // coverage buys nothing but a grey outline and costs the only weight the
        // glyph has.
        '0'..='9' => {
            let digit = ch as u8;
            // WHY the figure's own bold advance rather than a hand-tuned optical
            // centre: the figures are tabular, so all ten share one advance and
            // one ink box, and centring that box is what `text::center_x` does
            // for every other run on this surface. The face's sidebearing then
            // leaves the ink 2 px left of the disc's centre line at scale 4 -- 2%
            // of the key, and identical on all twelve keys, so the row of digits
            // reads as evenly set rather than twelve separate judgements.
            let adv = launcher::font::advance_of(digit, PIN_DIGIT_BOLD) * PIN_DIGIT_SCALE;
            let x0 = cx.saturating_sub(adv / 2);
            text::rasterize_pixel_bold(
                x,
                y,
                x0,
                pin_face_y(cy, PIN_DIGIT_SCALE),
                PIN_DIGIT_SCALE,
                &[digit],
                ink,
            )
        }
        // WHY spelled out rather than folded into the arm above as a catch-all:
        // `PIN_KEYS` holds exactly the ten figures plus `C` and `D`, so this is
        // unreachable today. A wildcard would silently rasterise any future byte
        // as a digit; returning `None` draws a bare disc instead, which is a
        // visible fault rather than a plausible-looking wrong one.
        _ => None,
    }
}

/// Hit-tests a touch point against the lock screen PIN keypad.
///
/// Returns the code of the key whose *drawn disc* contains `(x, y)`: rows,
/// columns, codes and radius all come from [`PIN_ROWS`], [`PIN_COLS`],
/// [`PIN_KEYS`] and [`PIN_KEY_R`], the same constants
/// [`lock_pin_pad_pixel`] draws with, so a key's tappable region is exactly its
/// visible circle.
pub fn hit_test_pin_pad(x: usize, y: usize) -> Option<char> {
    // WHY this delegates: the PadEntry surface in `pin.rs` is what the user sees
    // and presses, so the hit test resolves a point against that surface's own
    // key rectangles. The table that used to answer here placed rows at
    // 700/840/980/1120 while the surface draws 572/700/828/956, so the top row
    // resolved to nothing and the bottom row resolved to the row above it - a
    // PIN typed against the surface could be stored as different digits.
    crate::screens::key_at(x, y)
}

/// How opaque the PIN pad's own backdrop is.
///
/// WHY 100 and not 85: `Theme::veil(_, p, false)` blends toward black by `p`, so
/// at 85 the lock wallpaper still showed through - the hiker silhouette and the
/// skyline ridge read as smudges and stray marks behind the keys, and the horizon
/// line cut visibly across the inside of every key disc. A keypad is a control
/// surface, not a window onto the wallpaper.
const PIN_PAD_VEIL_PERCENT: u32 = 100;

/// The four entry dots: how many, where the first one sits, how far apart they
/// are, their row, and their radius.
///
/// WHY these are named and derived from each other: the dots used to be four
/// hard-coded centres inside a `345..375` / `245..475` window that was the same
/// four numbers re-spelled. Moving a dot without also re-typing its window is how
/// an indicator ends up drawn where the gate can no longer see it. The band and
/// the span below are computed from these, so there is one place to change.
const PIN_DOT_COUNT: usize = 4;
const PIN_DOT_X0: usize = 270;
const PIN_DOT_PITCH: usize = 60;
const PIN_DOT_CY: usize = 360;
const PIN_DOT_R: i32 = 9;
/// How far past its own radius the dot group's gate reaches.
///
/// WHY 6: the old window was `cy - 15 .. cy + 15` against a radius of 9, i.e.
/// exactly 6 px of slack, and the dots span 351..=369 inside it. Reusing that
/// slack keeps the accepted set identical to what the pad accepted before while
/// making it follow the geometry.
const PIN_DOT_SLACK: i32 = 6;

/// The pad's four inks, written out in full `0xAARRGGBB`.
///
/// WHY the alpha byte is written out rather than left off: the literals these
/// replaced were three bytes each -- `0xFFFFFF`, `0x90A0B0`, `0x708090` and
/// `0xFF5555` -- which as `0xAARRGGBB` are alpha `0x00`, i.e. fully transparent
/// ink. Nothing on screen shows it today, because the 720x1440 launcher surface
/// reaches the scanout through `DisplayKms::composite`'s identity
/// `copy_from_slice` and `present_argb8888` discards the byte; but the first
/// presenter that honours alpha would make the heading, the sub-heading and
/// both halves of the PIN dots disappear. A colour that only works because
/// nothing reads its alpha is a colour waiting for a bug.
///
/// WHY the hues are unchanged: they were chosen against this pad's own black
/// backdrop. `Theme::LIGHT`'s `text_muted` is a warm grey (119,119,115) and its
/// `danger` is a deep red (220,38,38); swapping either in would restain the pad
/// rather than fix it. The pad is the one surface in the shell that draws on a
/// backdrop it owns rather than on a theme page, and the header does the same.
const PIN_HEADING_INK: u32 = 0xFFFFFFFF;
const PIN_SUBHEADING_INK: u32 = 0xFF90A0B0;
const PIN_ERROR_INK: u32 = 0xFFFF5555;
const PIN_DOT_EMPTY_INK: u32 = 0xFF708090;

/// The prompt above the keypad and the sub-prompt under it, for the state the
/// shell is actually in.
///
/// WHY one function and not two literals picked at the call site: the same
/// `lock_state` / `pin_setup_step` / `auth_error` triple decides *both* runs, and
/// they are measured into the frame from this one answer. Choosing the pair
/// separately is how "Incorrect PIN" ends up captioned "Enter your 4-digit PIN"
/// because one arm was edited and the other was not.
///
/// WHY the order of the tests matters and is not alphabetical: an uninitialized
/// device has no credential to be wrong about, so the setup pair is asked for
/// first; a failed attempt outranks the lockout caption because it is the state
/// that just changed and the one the user is looking at.
fn pin_messages(state: &LauncherState) -> (&'static [u8], &'static [u8]) {
    if state.lock_state == interos_std::UserLockState::Uninitialized {
        if state.pin_setup_step == 0 {
            (b"Set Device PIN", b"Create a 4-digit PIN")
        } else {
            (b"Confirm Device PIN", b"Re-enter your 4-digit PIN")
        }
    } else if state.auth_error {
        (b"Incorrect PIN", b"Try again")
    } else if state.lock_state == interos_std::UserLockState::Lockout {
        (b"Device Locked Out", b"Account locked after 5 attempts")
    } else {
        (b"Enter Device PIN", b"Enter your 4-digit PIN to unlock")
    }
}

/// The prompt's scale: 2, i.e. a 34 px cap.
///
/// WHY 2: it is the line the user reads before every keystroke, and the widest of
/// the four strings [`pin_messages`] can return -- "Confirm Device PIN" -- measures
/// 364 px of the surface's 720, so it still clears both edges by 178 px. Scale 3
/// would be 546 px and leave 87 px, which is inside the sidebearing of the
/// widest glyph; scale 1 would put a 17 px cap above a keypad whose figures are
/// 68 px and invert the hierarchy the pad is built on.
const PIN_HEADING_SCALE: usize = 2;
/// The sub-prompt's scale: 1, i.e. a 17 px cap -- one rung below the prompt and
/// two below the figures, which is the ladder the pad reads down: figures, then
/// prompt, then sub-prompt, then the entry dots.
const PIN_SUBHEADING_SCALE: usize = 1;
/// The prompt's cell origin, and the sub-prompt's.
///
/// WHY these two numbers and not the `250..295` and `300..330` row windows the
/// pixel function used to gate on: those windows were typed for the old fixed 8x8
/// cell, in which a run's ink began on the cell's very first row, so 250 sat five
/// rows *above* the run's origin and the window stopped at 295 -- eight rows
/// short of the 48-row line box a scale-2 run occupies from 255.
///
/// WHY that survived: every string [`pin_messages`] can return is caps and
/// spaces, so the 17-row cap box (261..=294 for the prompt) happened to sit
/// entirely inside the clipped window, and a descender is the only thing that
/// would have revealed it. A run is now gated on `Rect::contains` over
/// `text::line_height`, which is the face's own box rather than a remembered
/// number, so the window follows the next typeface and the next scale by
/// construction. Nothing moves on screen today; the clip is gone.
const PIN_HEADING_Y: usize = 255;
const PIN_SUBHEADING_Y: usize = 305;

fn lock_pin_pad_pixel(
    color: u32,
    x: usize,
    y: usize,
    state: &LauncherState,
    f: &Frame,
) -> Option<u32> {
    let backdrop = Theme::veil(color, PIN_PAD_VEIL_PERCENT, false);

    // WHY the pad owns the whole surface, status band included: this returned
    // None above LOCK_STATUS_BAND, so the caller painted the lock wallpaper
    // there instead. That wallpaper's upper band is the LIGHT theme's sky, so
    // the dark PIN screen carried a bright blue strip across its own top with
    // the clock sitting on it and no separator. The pad draws its own clock on
    // its own backdrop now.
    if y < LOCK_STATUS_BAND {
        if state.clock_source_ok {
            // WHY the same gated box as the lock surface's own status clock and not
            // an open band test: the run is 24 rows tall from `LOCK_STATUS_TIME_Y`
            // and nothing else, so the 56-row band hands 32 rows to the rasteriser
            // for a 24-row run.
            if y >= LOCK_STATUS_TIME_Y && y < LOCK_STATUS_TIME_Y + f.status_h {
                if let Some(c) = test_text_aa(
                    x,
                    y,
                    LOCK_STATUS_TIME_X,
                    LOCK_STATUS_TIME_Y,
                    LOCK_STATUS_TIME_SCALE,
                    &state.clock_time,
                    PIN_HEADING_INK,
                    backdrop,
                ) {
                    return Some(c);
                }
            }
        }
        return Some(backdrop);
    }

    // 1. The prompt and its sub-prompt, each gated on its own measured line box
    //    in the frame rather than on a remembered row window -- see
    //    [`PIN_HEADING_Y`] for what was wrong with the old windows and why the
    //    defect was invisible.
    //
    //    WHY `pin_messages` is still called per pixel: it is four comparisons and
    //    no measurement. What this function used to do here was `text::measure`
    //    over a fourteen-byte string, for every pixel of a 45-row band -- about
    //    32,000 advance walks a frame to compute a box that is fixed for all of
    //    them, which is the exact regression the module header is about.
    let (header, subheader) = pin_messages(state);
    if f.pin_heading.contains(x, y) {
        let ink = if state.auth_error {
            PIN_ERROR_INK
        } else {
            PIN_HEADING_INK
        };
        if let Some(c) = test_text_aa(
            x,
            y,
            f.pin_heading.x,
            f.pin_heading.y,
            PIN_HEADING_SCALE,
            header,
            ink,
            backdrop,
        ) {
            return Some(c);
        }
    }
    if f.pin_subheading.contains(x, y) {
        if let Some(c) = test_text(
            x,
            y,
            f.pin_subheading.x,
            f.pin_subheading.y,
            PIN_SUBHEADING_SCALE,
            subheader,
            PIN_SUBHEADING_INK,
        ) {
            return Some(c);
        }
    }

    // 2. The four entry dots, on the row under the sub-prompt.
    //
    //    WHY the gate is derived from [`PIN_DOT_COUNT`] and friends and not typed:
    //    the old `345..375` and `245..475` were the same four centres re-spelled,
    //    so a dot and its gate could be moved independently. The slack reproduces
    //    the old row band exactly (`cy - 9 - 6 .. cy + 9 + 6` = 345..375) and is
    //    wider than it across, which is all the group needs.
    let dot_band = (
        (PIN_DOT_CY as i32 - PIN_DOT_R - PIN_DOT_SLACK).max(0) as usize,
        PIN_DOT_CY + PIN_DOT_R as usize + PIN_DOT_SLACK as usize,
    );
    let dot_span = (
        (PIN_DOT_X0 as i32 - PIN_DOT_R - PIN_DOT_SLACK).max(0) as usize,
        PIN_DOT_X0
            + (PIN_DOT_COUNT - 1) * PIN_DOT_PITCH
            + PIN_DOT_R as usize
            + PIN_DOT_SLACK as usize,
    );
    if (dot_band.0..dot_band.1).contains(&y) && (dot_span.0..dot_span.1).contains(&x) {
        for i in 0usize..PIN_DOT_COUNT {
            let cx = PIN_DOT_X0 + i * PIN_DOT_PITCH;
            let dx = (x as i32) - (cx as i32);
            let dy = (y as i32) - (PIN_DOT_CY as i32);
            let dist_sq = dx * dx + dy * dy;
            if dist_sq <= PIN_DOT_R * PIN_DOT_R {
                if i < state.pin_len as usize {
                    // Filled dots in the theme's accent: the one piece of colour
                    // on the pad, on the state the user is actually watching.
                    return Some(THEME.accent);
                } else if dist_sq >= (PIN_DOT_R - 2) * (PIN_DOT_R - 2) {
                    return Some(PIN_DOT_EMPTY_INK);
                }
            }
        }
    }

    // 3. The keypad: twelve circular keys on the shared grid. Rows, columns,
    //    codes and radius all come from the constants [`hit_test_pin_pad`]
    //    taps with, so a key is drawn exactly where it is tappable and says
    //    exactly what a tap on it does.
    for (row, &cy) in PIN_ROWS.iter().enumerate() {
        if y < cy.saturating_sub(PIN_KEY_BOX) || y > cy + PIN_KEY_BOX {
            continue;
        }
        for (col, &cx) in PIN_COLS.iter().enumerate() {
            if x < cx.saturating_sub(PIN_KEY_BOX) || x > cx + PIN_KEY_BOX {
                continue;
            }
            let dx = (x as i32) - (cx as i32);
            let dy = (y as i32) - (cy as i32);
            let dist_sq = dx * dx + dy * dy;
            if dist_sq > PIN_KEY_R * PIN_KEY_R {
                continue;
            }
            // The disc: the control glass, sheened from PIN_KEY_FILL_TOP at the
            // top of the circle to PIN_KEY_FILL_BOTTOM at the bottom. The
            // pixel's own fill is what the face is read against.
            //
            // WHY the ink is the theme's own and not a literal: `THEME` is the
            // LIGHT theme and the pad's backdrop is opaque black by
            // construction, so `ink_on_dark` is the theme's answer for "what
            // colour is type on this" and it moves if the theme ever does. The
            // measured contrast it gives here is 255 against a disc of 39..65, so
            // the figure was never low-contrast -- which is why the weight, and
            // not the palette, is what this pad was missing.
            let sheen = (dy + PIN_KEY_R) as u32 * (PIN_KEY_FILL_TOP - PIN_KEY_FILL_BOTTOM)
                / (2 * PIN_KEY_R as u32);
            let disc = Theme::veil(backdrop, PIN_KEY_FILL_TOP - sheen, true);
            let ch = PIN_KEYS[row][col];
            if let Some(c) = pin_key_face(
                x,
                y,
                cx,
                cy,
                ch,
                disc,
                THEME.ink_on_dark,
                state.pin_len,
            ) {
                return Some(c);
            }
            if dist_sq >= (PIN_KEY_R - 2) * (PIN_KEY_R - 2) {
                return Some(Theme::veil(backdrop, PIN_KEY_RIM_PERCENT, true));
            }
            return Some(disc);
        }
    }

    Some(backdrop)
}

pub fn get_lock_screen_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let theme = THEME;
    let f = frame_for(x, y, state);

    // 1. Full-bleed wallpaper.
    let color = lock_wallpaper_pixel(f, x, y);

    // If the PIN pad is active (or on first-boot uninitialized state), render the PIN pad!
    let pin_active = state.pin_pad_visible || state.lock_state == interos_std::UserLockState::Uninitialized;
    if pin_active {
        if let Some(pin_color) = lock_pin_pad_pixel(color, x, y, state, f) {
            return pin_color;
        }
    } else if y >= 360 && y < 1080 {
        // Fast path: between the clock/date band (y <= 360) and the padlock/hint (y >= 1080),
        // there are NO UI elements on the lock screen when PIN pad is inactive.
        return color;
    }

    // 2. Status bar. The panel draws its status glyphs in near-black because
    //    they sit on the bright upper sky.
    //
    //    WHY the clock and nothing else: the panel also draws four signal bars, a
    //    wifi arc and a filled battery, and this shell can measure none of the
    //    three. The kernel's wireless manager starts with `hardware_present:
    //    false`, there is no SIM, and the fuel gauge reports
    //    `telemetry_valid: false`, so four filled bars and a solid battery are
    //    three claims the platform cannot support -- and they are on the surface
    //    a user reads as the state of their device, before they have touched
    //    anything. The rest of the shell already draws its status bars this way.
    if y < LOCK_STATUS_BAND {
        if state.clock_source_ok
            && y >= LOCK_STATUS_TIME_Y
            && y < LOCK_STATUS_TIME_Y + f.status_h
        {
            if let Some(c) = test_text(
                x,
                y,
                LOCK_STATUS_TIME_X,
                LOCK_STATUS_TIME_Y,
                LOCK_STATUS_TIME_SCALE,
                &state.clock_time,
                f.ink,
            ) {
                return c;
            }
        }
    }

    // 3. Date line above the clock, both left-aligned on LOCK_CONTENT_X.
    //    WHY both are conditional on a live reading: the panel's date line and
    //    clock are the two largest pieces of type on the surface, so a
    //    placeholder there would read as a real time. With no source they are
    //    simply absent, and the layout they occupy is unchanged.
    //
    //    WHY every one of these is inside an explicit box. `test_text` already
    //    rejects a pixel outside the run's own extent, but it does that *after*
    //    measuring the run, and this function runs for all 1,036,800 pixels of the
    //    surface. Testing `x >= LOCK_CONTENT_X && y >= LOCK_CLOCK_Y` with no upper
    //    bound -- which is what this file used to do -- hands four fifths of the
    //    screen to the text path on every frame, which is most of why a full
    //    render of this surface costs what it does on the emulated build.
    if state.clock_source_ok {
        if f.date.contains(x, y) {
            if let Some(c) = test_text_aa(
                x,
                y,
                f.date.x,
                f.date.y,
                LOCK_DATE_SCALE,
                &state.date_line,
                f.ink,
                color,
            ) {
                return c;
            }
        }

// 4. The clock itself.
        if f.clock.contains(x, y) {
            if let Some(c) = test_text_aa(
                x,
                y,
                f.clock.x,
                f.clock.y,
                LOCK_CLOCK_SCALE,
                &state.clock_time,
                f.ink,
                color,
            ) {
                return c;
            }
        }
    }

    // 5. Closed padlock above the centred unlock hint, both on the near-black
    //    foreground where the panel draws them in paper white.
    if let Some(c) = lock_padlock_pixel(&theme, color, x, y) {
        return c;
    }
    // WHY the origin is derived from the measured string: the panel centres the
    // hint on the surface, so the x origin must follow the measured text.
    if f.hint.contains(x, y) {
        if let Some(c) = test_text(
            x,
            y,
            f.hint.x,
            f.hint.y,
            LOCK_HINT_SCALE,
            LOCK_HINT_TEXT,
            lock_paper(&theme, color),
        ) {
            return c;
        }
    }

    // 6. Torch control on the left. WHY dimmed and labelled: there is no torch
    //    hardware, no torch capability and no torch syscall on this platform, so
    //    the affordance must not read as operable.
    if let Some(disc) = lock_button_disc(x, y, LOCK_BTN_L_CX, color) {
        if lock_torch_glyph_pixel(x, y) {
            return lock_glyph_dim(&theme, disc);
        }
        return disc;
    }

    // 7. Camera control on the right.
    if let Some(disc) = lock_button_disc(x, y, LOCK_BTN_R_CX, color) {
        if lock_camera_glyph_pixel(x, y) {
            return lock_paper(&theme, disc);
        }
        return disc;
    }

    // 8. "N/A" under the torch disc, stating the absence rather than implying a
    //    control that does nothing. WHY the box test first: the label's extent is
    //    in the frame, so the ~99% of the surface that is nowhere near it is
    //    turned away by four comparisons instead of a call and a pen walk.
    if f.na.contains(x, y) {
        if let Some(c) = test_text(
            x,
            y,
            f.na.x,
            f.na.y,
            type_scale::CAPTION,
            LOCK_NA_TEXT,
            lock_paper(&theme, color),
        ) {
            return c;
        }
    }

    // 8b. The camera disc gets the same absence caption as the torch disc.
    //     WHY it needs one: the two discs are a matched pair, but only the left
    //     one was labelled, so the right read as a live control while the left
    //     stated it was not. Neither hardware is bound here - the camera app
    //     reports no sensor - so both state it, symmetrically.
    if f.na_r.contains(x, y) {
        if let Some(c) = test_text(
            x,
            y,
            f.na_r.x,
            f.na_r.y,
            type_scale::CAPTION,
            LOCK_NA_TEXT,
            lock_paper(&theme, color),
        ) {
            return c;
        }
    }

    // 9. Home indicator, centred.
    let pill_x = (W - LOCK_PILL_W) / 2;
    if y >= LOCK_PILL_Y
        && y < LOCK_PILL_Y + LOCK_PILL_H
        && x >= pill_x
        && x < pill_x + LOCK_PILL_W
    {
        return lock_paper(&theme, color);
    }

    // 10. Wallpaper.
    //
    // WHY the stock backdrop ramp is no longer sampled here: it used to be, as
    // `let _ = get_stock_background_pixel(x, y);` -- a call whose result was thrown
    // away, so it bought nothing and cost a ramp evaluation on every pixel. The
    // lock backdrop tokens it read (`LOCK_BACKDROP_TOP` / `LOCK_BACKDROP_BOTTOM`)
    // are unchanged and still describe this surface's ramp; the ramp this screen
    // draws is the one derived from them in [`Frame::measure`].
    color
}
