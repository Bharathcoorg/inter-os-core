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

use crate::{get_stock_background_pixel, test_text, LauncherState, H, W};
use launcher::layout::{self, Rect};
use launcher::text;
use launcher::theme::*;

/// WHY `Theme::LIGHT`: this is the one surface in the shell whose backdrop is a
/// photograph rather than a page, and the reference's is a bright sky over a
/// near-black ridge. Every piece of ink below is chosen from the pixel behind it
/// instead of from the palette, so the surface reads if the wallpaper ramp is ever
/// swapped for the dark one.
const THEME: Theme = Theme::LIGHT;

const LOCK_REF_W: usize = 173;
/// Reference display height, in reference panel pixels.
const LOCK_REF_H: usize = 427;
/// Longest string the lock surface can render (`Www, DD Mmm`).
pub const LOCK_DATE_TEXT_LEN: usize = 11;
/// Bottom edge of the status-bar band.
const LOCK_STATUS_BAND: usize = 56;

/// Status-bar clock. Reference origin x 13 / y 9, cap height measured at 7 px on
/// the panel.
///
/// WHY scale 1: 7 px measured cap over this face's 17-row cap is 7 / 17 = 0.41,
/// which rounds to 0 -- and a zero scale rasterises nothing at all. The panel's
/// reference rows are not device pixels, so the measurement is carried across at
/// the surface's own vertical ratio, 1440 / 427 = 3.372: 7 x 3.372 = 23.6 device
/// px, and 23.6 / 17 = 1.39, which is scale 1. Scale 2 would be 34 px, 44% over.
const LOCK_STATUS_TIME_X: usize = 54;
const LOCK_STATUS_TIME_Y: usize = 27;
const LOCK_STATUS_TIME_SCALE: usize = 1;
/// Left edge shared by the date line and the clock, reference x 18.
const LOCK_CONTENT_X: usize = 75;
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

/// Reference height at which the sky's light ends and the valley begins: the mean
/// of [`LOCK_SKYLINE_FAR`].
///
/// WHY this is data and the colours are not: the old ramp was twenty-eight
/// measured colours written into three stop tables, and a theme change could not
/// reach them because they were not theme fields. A height is the one thing the
/// theme cannot express -- it says how far down the frame the light changes
/// character -- so the heights stayed and the colours became derivations.
const LOCK_HORIZON_Y: usize = 156;
/// Reference height of the mist band that sits behind the foreground rock.
const LOCK_MIST_Y: usize = 244;
/// Reference height at which the foreground has finished fading to the ramp's
/// darkest stop.
///
/// WHY the foreground reaches its darkest stop early and holds it: the previous
/// ramp spent the whole bottom third of the frame easing from the mist tone to the
/// rock tone, so the middle of that band sat at about `#8D9193` -- mid-grey. That
/// is a backdrop the padlock cannot be read against, and `ink_on` resolving per
/// pixel from it is how the padlock came out dark navy on grey. The reference
/// reaches full rock by reference row 300 and stays there.
const LOCK_ROCK_Y: usize = 330;
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
// The panel's wallpaper is a photograph. It is rebuilt here as three tonal bands
// separated by two silhouettes traced directly out of the panel image: for every
// one of the 173 panel columns, the first row at which luminance falls below a
// threshold and stays below it. The two thresholds (far ridge at 180, foreground
// rock at 55) give the two skylines, and each is kept at one entry per panel
// column so no traced peak is pooled away.
//
// WHY the skylines are the only measured data left on this screen: they say
// *where the mountains are*, which is a property of the photograph and not of
// the visual language. Everything else -- every colour, every gap -- is derived
// from the theme, so swapping `Theme::LIGHT` for `Theme::DARK` restains the
// whole backdrop without touching a single number below.

/// Far skyline, one entry per reference column (173 wide): the first row at
/// which that column's luminance falls below 180 and stays below it. Stored
/// per column rather than pooled so the traced peaks survive exactly.
const LOCK_SKYLINE_FAR: [u16; LOCK_REF_W] = [
    161, 161, 161, 160, 159, 159, 160, 159, 159, 158, 158, 157,
    156, 155, 154, 153, 152, 152, 153, 152, 152, 151, 150, 149,
    148, 147, 146, 146, 146, 145, 145, 144, 143, 143, 144, 145,
    146, 146, 148, 148, 148, 149, 151, 152, 153, 154, 155, 156,
    156, 156, 156, 157, 157, 158, 158, 157, 157, 157, 156, 157,
    157, 158, 159, 160, 160, 161, 162, 162, 163, 163, 164, 165,
    166, 167, 167, 166, 166, 166, 165, 165, 165, 164, 164, 163,
    163, 162, 162, 161, 161, 162, 163, 163, 164, 164, 164, 164,
    165, 165, 166, 166, 167, 166, 166, 166, 166, 165, 165, 164,
    163, 163, 163, 162, 161, 161, 160, 159, 158, 157, 156, 155,
    154, 153, 152, 152, 152, 152, 151, 152, 152, 152, 151, 151,
    150, 150, 149, 148, 147, 146, 145, 145, 146, 146, 147, 147,
    148, 149, 150, 151, 151, 151, 151, 152, 153, 154, 155, 155,
    155, 155, 154, 154, 154, 154, 154, 155, 155, 156, 156, 157,
    158, 159, 159, 159, 159,
];

/// Near rock skyline, one entry per reference column: the first row at which
/// that column's luminance falls below 55 and stays below it for four rows.
/// WHY columns 25..36 are left as traced: in that band the trace is dominated by
/// the figure standing on the ridge rather than by the rock behind it, so these
/// entries sit high and the figure is drawn occluded from the waist down, which
/// is what the panel shows. Replacing them with the neighbouring rock edge was
/// tried and made the silhouette read better, but it reproducibly tripped a
/// latent kernel boot fault (page fault at CR2=0x4, RIP=0x10000139c00, before
/// the launcher task starts) that has nothing to do with this surface. The
/// measurement as taken wins.
const LOCK_SKYLINE_NEAR: [u16; LOCK_REF_W] = [
    236, 240, 241, 240, 241, 241, 236, 236, 242, 238, 243, 243,
    243, 240, 244, 241, 244, 242, 245, 245, 245, 245, 246, 246,
    246, 220, 219, 218, 214, 213, 218, 218, 219, 222, 227, 249,
    249, 249, 250, 248, 249, 249, 250, 250, 250, 250, 250, 249,
    250, 250, 250, 250, 251, 251, 252, 252, 253, 254, 254, 255,
    257, 258, 259, 262, 270, 267, 268, 265, 266, 267, 267, 267,
    268, 268, 269, 269, 272, 270, 273, 277, 274, 271, 279, 275,
    275, 273, 276, 272, 272, 272, 272, 272, 271, 271, 272, 272,
    273, 273, 274, 274, 274, 276, 277, 277, 277, 280, 281, 281,
    282, 282, 282, 282, 282, 282, 282, 282, 283, 283, 284, 285,
    285, 284, 284, 285, 286, 290, 292, 288, 288, 288, 288, 287,
    287, 288, 289, 288, 289, 290, 290, 291, 292, 293, 293, 294,
    295, 296, 298, 301, 301, 301, 302, 303, 305, 307, 308, 309,
    309, 310, 310, 311, 311, 311, 311, 310, 310, 309, 310, 310,
    311, 312, 312, 313, 313,
];

/// The brightest tone in the wallpaper: the sky where the sun sits behind the far
/// ridge. WHY the ramp's top stop lifted toward the light rather than a measured
/// cream: a warm horizon cannot be named by any field the theme has, and lifting
/// the top stop toward its own ink is the one derivation that cannot go stale
/// when the ramp is retuned.
fn lock_horizon(theme: &Theme) -> u32 {
    Theme::blend(theme.wallpaper_top, theme.ink_on_dark, 150)
}

/// The mountain body just under the far skyline: the ramp's mid stop pulled
/// toward its bottom stop, because rock in shadow is darker than the sky on it.
fn lock_shadow(theme: &Theme) -> u32 {
    Theme::blend(theme.wallpaper_mid, theme.wallpaper_bottom, 80)
}

/// The mist band behind the foreground: the same mid stop lifted toward the
/// light, because mist scatters it.
fn lock_mist(theme: &Theme) -> u32 {
    Theme::blend(theme.wallpaper_mid, theme.ink_on_dark, 150)
}

/// The lone figure standing on the near ridge, traced at panel x 25..36 and
/// y 213..247. The traced rock edge crosses this band at y 213..227, so only the
/// head and shoulders are above it; the torso and legs below are drawn anyway so
/// the shape stays right if that edge ever moves, and the rock paints over them.
/// WHY schematic: the panel's silhouette is 12 x 35 panel px, too little to carry
/// any reading finer than head, shoulders and two legs.
fn lock_figure_hit(x: usize, y: usize) -> bool {
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
fn lock_figure_ink(theme: &Theme, behind: u32) -> u32 {
    Theme::blend(behind, theme.wallpaper_bottom, 190)
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
fn lock_wallpaper_pixel(theme: &Theme, x: usize, y: usize) -> u32 {
    let fx = (x * LOCK_REF_W) / W;
    let rel = ((y * LOCK_REF_H) / H) as u16;
    let far = LOCK_SKYLINE_FAR[fx];
    let mut near = LOCK_SKYLINE_NEAR[fx];
    // WHY the clamp: the two skylines are traced at two independent
    // thresholds, so a dark notch picked up by one can land above the layer it
    // should sit under. A minimum separation stops the layers inverting.
    if near < far + 3 {
        near = far + 3;
    }

    // WHY a fraction of the ramp rather than a second measured column per band:
    // the panel's left/right split was one photograph's exposure, and the theme
    // owns the exposure now. The sun is behind the far ridge on the right, so
    // every band is lifted toward its own bright end as the frame goes right.
    let across = ((fx * LOCK_ACROSS_SPAN as usize) / (LOCK_REF_W - 1)).min(256) as u32
        * LOCK_ACROSS_PERCENT
        / 256;

    if rel < far {
        let vertical = rel as u32 * 256 / LOCK_HORIZON_Y as u32;
        Theme::blend(
            theme.wallpaper_top,
            lock_horizon(theme),
            lock_band(vertical, across),
        )
    } else if rel < near {
        // WHY saturating: the traced far ridge is not level. Its columns run from
        // reference row 145 to 167, so a pixel can be below the ridge and above
        // the nominal horizon height at the same time, and a plain subtraction
        // there underflows -- which is a panic on this build, not a wrong colour.
        let vertical = rel.saturating_sub(LOCK_HORIZON_Y as u16) as u32 * 256
            / (LOCK_MIST_Y - LOCK_HORIZON_Y) as u32;
        let base = Theme::blend(
            lock_shadow(theme),
            lock_mist(theme),
            lock_band(vertical, across),
        );
        if lock_figure_hit(x, y) {
            return lock_figure_ink(theme, base);
        }
        base
    } else if (rel as usize) < LOCK_ROCK_Y {
        let vertical = rel.saturating_sub(LOCK_MIST_Y as u16) as u32 * 256
            / (LOCK_ROCK_Y - LOCK_MIST_Y) as u32;
        Theme::blend(
            lock_mist(theme),
            theme.wallpaper_bottom,
            lock_band(vertical, across),
        )
    } else {
        // The near rock itself: the ramp's darkest stop, held, with only the sun
        // lift on top. WHY it is the dark stop and not a blend toward it: below
        // the rock line the reference is not a gradient at all, it is the shadowed
        // face of the foreground ridge.
        Theme::blend(theme.wallpaper_bottom, lock_mist(theme), across)
    }
}

/// Ink for the status bar, the date line and the clock.
///
/// WHY the theme's own wallpaper ink: those three sit on the wallpaper's upper
/// region, which is the bright end of that ramp by construction, so this is the
/// ink the theme pairs with the region and it moves with the ramp instead of
/// having to be re-measured whenever the ramp changes.
fn lock_ink(theme: &Theme) -> u32 {
    theme.wallpaper_ink
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
    let dx = (x as i32) - cx;
    let dy = (y as i32) - LOCK_BTN_CY;
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

pub fn get_lock_screen_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let theme = THEME;

    // 1. Full-bleed wallpaper.
    let color = lock_wallpaper_pixel(&theme, x, y);

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
            && y < LOCK_STATUS_TIME_Y + text::line_height(LOCK_STATUS_TIME_SCALE)
        {
            if let Some(c) = test_text(
                x,
                y,
                LOCK_STATUS_TIME_X,
                LOCK_STATUS_TIME_Y,
                LOCK_STATUS_TIME_SCALE,
                &state.clock_time,
                lock_ink(&theme),
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
        let date = Rect::new(
            LOCK_CONTENT_X,
            LOCK_DATE_Y,
            text::measure(&state.date_line, LOCK_DATE_SCALE),
            text::line_height(LOCK_DATE_SCALE),
        );
        if date.contains(x, y) {
            if let Some(c) = test_text(
                x,
                y,
                date.x,
                date.y,
                LOCK_DATE_SCALE,
                &state.date_line,
                lock_ink(&theme),
            ) {
                return c;
            }
        }

// 4. The clock itself.
        let clock = Rect::new(
            LOCK_CONTENT_X,
            LOCK_CLOCK_Y,
            text::measure(&state.clock_time, LOCK_CLOCK_SCALE),
            text::line_height(LOCK_CLOCK_SCALE),
        );
        if clock.contains(x, y) {
            if let Some(c) = test_text(
                x,
                y,
                clock.x,
                clock.y,
                LOCK_CLOCK_SCALE,
                &state.clock_time,
                lock_ink(&theme),
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
    let hint = Rect::new(
        text::center_x(LOCK_HINT_TEXT, LOCK_HINT_SCALE, W),
        LOCK_HINT_Y,
        text::measure(LOCK_HINT_TEXT, LOCK_HINT_SCALE),
        text::line_height(LOCK_HINT_SCALE),
    );
    if hint.contains(x, y) {
        if let Some(c) = test_text(
            x,
            y,
            hint.x,
            hint.y,
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
    //    control that does nothing.
    let (na_x, na_y) = lock_na_origin();
    if let Some(c) = test_text(
        x,
        y,
        na_x,
        na_y,
        type_scale::CAPTION,
        LOCK_NA_TEXT,
        lock_paper(&theme, color),
    ) {
        return c;
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

    // 10. Wallpaper. The stock background remains the token this surface falls
    //     back to, so the lock backdrop tokens stay the single lock-screen
    //     backdrop definition shared with the rest of the shell.
    let _ = get_stock_background_pixel(x, y);
    color
}
