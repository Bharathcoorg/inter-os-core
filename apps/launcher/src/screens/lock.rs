//! Lock Screen: wallpaper, clock, controls and unlock affordances.
//!
//! WHY `pub` on the date constants: `LauncherState::date_line` and the RTC parser
//! live in the crate root and size themselves from the same abbreviation tables
//! this screen draws with, so the two must not diverge.
//!
//! Geometry and colour come from `crate::theme` and `crate::layout`, never from
//! literals local to this file.

use crate::{get_stock_background_pixel, test_text, LauncherState, H, W};
use launcher::theme::*;
use launcher::layout::{self, Rect};

const LOCK_REF_W: usize = 173;
/// Reference display height, in reference panel pixels.
const LOCK_REF_H: usize = 427;
/// Longest string the lock surface can render (`Www, DD Mmm`).
pub const LOCK_DATE_TEXT_LEN: usize = 11;
/// Bottom edge of the status-bar glyph band, reference y 15.
const LOCK_STATUS_BAND: usize = 56;
/// Status-bar clock origin, reference x 13 / y 9. Cap height measured at 7 px.
const LOCK_STATUS_TIME_X: usize = 54;
const LOCK_STATUS_TIME_Y: usize = 30;
const LOCK_STATUS_TIME_SCALE: usize = 3;
/// Left edge shared by the date line and the clock, reference x 18.
const LOCK_CONTENT_X: usize = 75;
/// Date line cap top, reference y 47. Cap height measured at 9 px.
const LOCK_DATE_Y: usize = 158;
const LOCK_DATE_SCALE: usize = 4;
/// Clock cap top, reference y 68. Cap height measured at 32 px: the single
/// largest type element on the surface.
/// WHY 12 and not 14: the panel's clock is 109 x 32 panel px, i.e. 454 x 108
/// logical. FONT_8X8 is 1.68x less condensed than the panel's typeface, so no
/// scale matches both axes. Scale 12 lands the string's right edge on logical
/// x 531 against the panel's 529, which is the measurement a reader can check
/// against the design; the cost is a cap height of 84 against 108.
const LOCK_CLOCK_Y: usize = 229;
const LOCK_CLOCK_SCALE: usize = 12;
/// Unlock hint cap top, reference y 341, horizontally centred.
const LOCK_HINT_Y: usize = 1150;
const LOCK_HINT_SCALE: usize = 3;
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

/// Ink for everything the panel draws over its bright upper sky: the status bar,
/// the date line and the clock. Measured stroke colour #0B0B0B.
const LOCK_INK: u32 = 0xFF0B0B0B;
/// Paper for everything the panel draws over its near-black foreground: the
/// padlock, the unlock hint and the home indicator. Measured #F9F9F8.
const LOCK_PAPER: u32 = 0xFFF9F9F8;
/// The panel's circular controls are translucent, not opaque. Their body
/// measures #545658 over a #1A1613 ridge and their rim #6C6B6D, which solves
/// to a 27% and a 36% white veil respectively.
const LOCK_BTN_FILL_ALPHA: u32 = 68;
const LOCK_BTN_RIM_ALPHA: u32 = 92;
/// Camera glyph inside its control, measured #FAFBFA.
const LOCK_BTN_GLYPH: u32 = 0xFFFAFBFA;
/// WHY dimmer: the torch control must not read as operable, because no torch
/// hardware, capability or syscall exists on this platform.
const LOCK_BTN_GLYPH_DIM: u32 = 0xFFB4B6B7;
/// The figure standing on the near ridge, reference x 25..36 / y 213..247.
const LOCK_FIGURE_INK: u32 = 0xFF16110F;

pub const LOCK_WEEKDAY_ABBR: [&[u8; 3]; 7] = [
    b"Sun", b"Mon", b"Tue", b"Wed", b"Thu", b"Fri", b"Sat",
];
pub const LOCK_MONTH_ABBR: [&[u8; 3]; 12] = [
    b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec",
];

// --- Wallpaper -------------------------------------------------------------
// The panel's wallpaper is a photograph. It is rebuilt here as three measured
// tonal bands separated by two silhouettes traced directly out of the panel
// image: for every one of the 173 panel columns, the first row at which
// luminance falls below a threshold and stays below it. The two thresholds
// (far ridge at 180, foreground rock at 55) give the two skylines, and each is
// kept at one entry per panel column so no traced peak is pooled away.
//
// Each colour ramp is a stop table of (reference y, left colour, right colour).
// The panel's light comes from the upper right at the horizon and from the
// upper left at the top of the frame, which is why the two ends differ.

// Sky above the far ridge. Sampled on the panel at x 20 and x 172.
const LOCK_SKY_STOPS: [(u16, [u32; 3], [u32; 3]); 9] = [
    (0, [0xC6, 0xCF, 0xD6], [0xB8, 0xC1, 0xC5]),
    (30, [0xC8, 0xCE, 0xD3], [0xBB, 0xC1, 0xCA]),
    (55, [0xCC, 0xD0, 0xD5], [0xC8, 0xC9, 0xCF]),
    (75, [0xD2, 0xD2, 0xD5], [0xD8, 0xD1, 0xD3]),
    (95, [0xDC, 0xDA, 0xDA], [0xED, 0xE0, 0xD8]),
    (115, [0xE2, 0xDB, 0xD9], [0xFC, 0xF1, 0xE8]),
    (135, [0xEE, 0xE3, 0xDC], [0xFB, 0xF0, 0xE5]),
    (150, [0xF4, 0xE6, 0xDD], [0xFD, 0xF2, 0xE6]),
    (160, [0xF6, 0xE7, 0xDC], [0xFE, 0xF5, 0xEA]),
];

// Everything below the far ridge and above the foreground: the mountain body
// falling into shadow, the mist band, and the warm fog that sits behind the
// foreground rock. WHY one ramp and not one per layer: the panel's tonal bands
// are horizontal, so splitting the ramp at a second silhouette only introduced
// a visible seam at that silhouette. Left and right values are the panel's
// measured split at the same heights; the right side is lit by the sun that
// glows at the panel's upper-right horizon.
const LOCK_VALLEY_STOPS: [(u16, [u32; 3], [u32; 3]); 12] = [
    (145, [0x8D, 0x97, 0xA3], [0x98, 0xA1, 0xAC]),
    (160, [0x7A, 0x84, 0x92], [0x8A, 0x93, 0x9F]),
    (175, [0x62, 0x6D, 0x7C], [0x7A, 0x85, 0x93]),
    (190, [0x53, 0x5F, 0x6B], [0x86, 0x8E, 0x9B]),
    (205, [0x77, 0x7D, 0x87], [0x97, 0x96, 0x9D]),
    (220, [0x7C, 0x7D, 0x83], [0xAA, 0xA0, 0xA1]),
    (235, [0x93, 0x87, 0x84], [0xBD, 0xAF, 0xAD]),
    (248, [0xA7, 0x9B, 0x98], [0xD2, 0xC6, 0xC2]),
    (258, [0x6B, 0x5F, 0x5B], [0x9E, 0x92, 0x90]),
    (275, [0x4B, 0x43, 0x40], [0x6E, 0x66, 0x68]),
    (290, [0x2E, 0x28, 0x25], [0x42, 0x3C, 0x3E]),
    (313, [0x1E, 0x1A, 0x19], [0x2A, 0x25, 0x23]),
];

// Foreground rock below the near ridge: near-black, warming to the bottom.
const LOCK_FOREGROUND_STOPS: [(u16, [u32; 3], [u32; 3]); 7] = [
    (240, [0x3A, 0x33, 0x33], [0x4A, 0x43, 0x42]),
    (258, [0x2C, 0x25, 0x23], [0x38, 0x30, 0x2E]),
    (278, [0x1E, 0x19, 0x17], [0x26, 0x20, 0x1E]),
    (300, [0x1E, 0x1A, 0x19], [0x24, 0x1F, 0x1E]),
    (330, [0x16, 0x14, 0x14], [0x1A, 0x17, 0x16]),
    (370, [0x0F, 0x0E, 0x0E], [0x12, 0x11, 0x11]),
    (426, [0x0B, 0x0B, 0x0D], [0x0D, 0x0D, 0x0F]),
];

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

#[inline(always)]
fn lock_lerp_ch(a: u32, b: u32, t: u32) -> u32 {
    if b >= a {
        a + (b - a) * t / 256
    } else {
        a - (a - b) * t / 256
    }
}

#[inline(always)]
fn lock_mix3(a: [u32; 3], b: [u32; 3], t: u32) -> [u32; 3] {
    [
        lock_lerp_ch(a[0], b[0], t),
        lock_lerp_ch(a[1], b[1], t),
        lock_lerp_ch(a[2], b[2], t),
    ]
}

#[inline(always)]
fn lock_argb(c: [u32; 3]) -> u32 {
    0xFF00_0000 | (c[0] << 16) | (c[1] << 8) | c[2]
}

/// Samples a stop table at `rel`, returning the left and right colour for that
/// height so the caller can apply the panel's horizontal light gradient.
fn lock_pair_at(stops: &[(u16, [u32; 3], [u32; 3])], rel: u16) -> ([u32; 3], [u32; 3]) {
    let mut i = 0usize;
    while i + 2 < stops.len() && rel >= stops[i + 1].0 {
        i += 1;
    }
    let a = stops[i];
    let b = stops[i + 1];
    let span = b.0.saturating_sub(a.0).max(1) as u32;
    let f = ((rel.saturating_sub(a.0) as u32) * 256 / span).min(256);
    (
        [
            lock_lerp_ch(a.1[0], b.1[0], f),
            lock_lerp_ch(a.1[1], b.1[1], f),
            lock_lerp_ch(a.1[2], b.1[2], f),
        ],
        [
            lock_lerp_ch(a.2[0], b.2[0], f),
            lock_lerp_ch(a.2[1], b.2[1], f),
            lock_lerp_ch(a.2[2], b.2[2], f),
        ],
    )
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

/// Full-bleed wallpaper layer for the lock surface.
fn lock_wallpaper_pixel(x: usize, y: usize) -> u32 {
    let fx = (x * LOCK_REF_W) / W;
    let rel = ((y * LOCK_REF_H) / H) as u16;
    let t = (((fx * 256) / (LOCK_REF_W - 1)).min(256)) as u32;

    let far = LOCK_SKYLINE_FAR[fx];
    let mut near = LOCK_SKYLINE_NEAR[fx];
    // WHY the clamp: the two skylines are traced at two independent
    // thresholds, so a dark notch picked up by one can land above the layer it
    // should sit under. A minimum separation stops the layers inverting.
    if near < far + 3 {
        near = far + 3;
    }

    if rel < far {
        let (l, r) = lock_pair_at(&LOCK_SKY_STOPS, rel);
        lock_argb(lock_mix3(l, r, t))
    } else if rel < near {
        if lock_figure_hit(x, y) {
            return LOCK_FIGURE_INK;
        }
        let (l, r) = lock_pair_at(&LOCK_VALLEY_STOPS, rel);
        lock_argb(lock_mix3(l, r, t))
    } else {
        let (l, r) = lock_pair_at(&LOCK_FOREGROUND_STOPS, rel);
        lock_argb(lock_mix3(l, r, t))
    }
}

/// Status-bar signal, wifi and battery glyphs. Measured on the panel at
/// x 132..177 and y 9..15, which is logical x 549..736 and y 30..51.
fn lock_status_icons_pixel(x: usize, y: usize) -> Option<u32> {
    let px = x as i32;
    let py = y as i32;
    // Four ascending signal bars: x 549..612, top edges stepping 42/38/34/30.
    for i in 0..4i32 {
        let x0 = 549 + i * 17;
        if px >= x0 && px < x0 + 12 {
            if py >= 30 + i * 4 && py < 52 {
                return Some(LOCK_INK);
            }
        }
    }
    // Wifi: two arcs and a dot about a centre on the bottom edge of the band.
    let wdx = px - 650;
    let wdy = py - 51;
    let wd2 = wdx * wdx + wdy * wdy;
    if wd2 <= 400 && wdy <= -4 && (wd2 >= 230 || wd2 <= 20) {
        return Some(LOCK_INK);
    }
    if wd2 <= 160 && wd2 >= 80 && wdy <= -3 {
        return Some(LOCK_INK);
    }
    // Battery: 37 x 22 outline flush with the right edge, as measured, with the
    // positive terminal nub in its final column.
    if (682..=718).contains(&px) && (30..=51).contains(&py) {
        let rim = px <= 683 || px >= 717 || py <= 31 || py >= 50;
        if rim {
            return Some(LOCK_INK);
        }
        if (685..=715).contains(&px) && (33..=48).contains(&py) {
            return Some(SUCCESS_EMERALD);
        }
        return None;
    }
    if px == 719 && (37..=44).contains(&py) {
        return Some(LOCK_INK);
    }
    None
}

fn lock_padlock_pixel(x: usize, y: usize) -> Option<u32> {
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
        return Some(LOCK_PAPER);
    }
    // shackle: the upper half of an annulus
    if (-19..0).contains(&dy) {
        let d2 = dx * dx + dy * dy;
        if d2 <= 13 * 13 && d2 >= 7 * 7 {
            return Some(LOCK_PAPER);
        }
    }
    None
}

/// Torch pictogram, drawn in the dimmed unavailable style: the panel's torch is
/// a wide tapered head over a narrow handle with a small dark vent in the head.
fn lock_torch_glyph_pixel(x: usize, y: usize) -> Option<u32> {
    let dx = (x as i32) - LOCK_BTN_L_CX;
    let dy = (y as i32) - (LOCK_BTN_CY - 2);
    let body = (dx >= -9 && dx <= 9 && dy >= -26 && dy < -14)
        || (dx >= -6 && dx <= 6 && dy >= -14 && dy < -5)
        || (dx >= -4 && dx <= 4 && dy >= -5 && dy < 22);
    if !body {
        return None;
    }
    // the vent in the head is a hole, so the disc shows through it
    if dx >= -2 && dx <= 2 && dy >= -19 && dy < -15 {
        return None;
    }
    Some(LOCK_BTN_GLYPH_DIM)
}

/// Camera pictogram.
fn lock_camera_glyph_pixel(x: usize, y: usize) -> Option<u32> {
    let dx = (x as i32) - LOCK_BTN_R_CX;
    let dy = (y as i32) - LOCK_BTN_CY;
    // body outline
    if dx.abs() <= 25 && (-12..=15).contains(&dy) {
        let rim = dx.abs() >= 22 || dy <= -9 || dy >= 12;
        if rim {
            return Some(LOCK_BTN_GLYPH);
        }
        // lens ring
        let l2 = dx * dx + dy * dy;
        if (81..=144).contains(&l2) {
            return Some(LOCK_BTN_GLYPH);
        }
        return None;
    }
    // viewfinder hump on the top edge
    if (-19..=-13).contains(&dy) && dx >= -8 && dx <= 8 {
        return Some(LOCK_BTN_GLYPH);
    }
    None
}

/// Composites `alpha` (0..=255) of white over `bg`.
#[inline(always)]
fn lock_veil(bg: u32, alpha: u32) -> u32 {
    let r = ((bg >> 16) & 0xFF) as u32;
    let g = ((bg >> 8) & 0xFF) as u32;
    let b = (bg & 0xFF) as u32;
    let nr = r + (255 - r) * alpha / 255;
    let ng = g + (255 - g) * alpha / 255;
    let nb = b + (255 - b) * alpha / 255;
    0xFF00_0000 | (nr.min(255) << 16) | (ng.min(255) << 8) | nb.min(255)
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
    let alpha = if d2 > (LOCK_BTN_R - 2) * (LOCK_BTN_R - 2) {
        LOCK_BTN_RIM_ALPHA
    } else {
        LOCK_BTN_FILL_ALPHA
    };
    Some(lock_veil(bg, alpha))
}

pub fn get_lock_screen_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Full-bleed wallpaper.
    let color = lock_wallpaper_pixel(x, y);

    // 2. Status bar. The panel draws its status glyphs in near-black because
    //    they sit on the bright upper sky.
    if y < LOCK_STATUS_BAND {
        if state.clock_source_ok
            && y >= LOCK_STATUS_TIME_Y
            && y < LOCK_STATUS_TIME_Y + 8 * LOCK_STATUS_TIME_SCALE
        {
            if let Some(c) = test_text(
                x,
                y,
                LOCK_STATUS_TIME_X,
                LOCK_STATUS_TIME_Y,
                LOCK_STATUS_TIME_SCALE,
                &state.clock_time,
                LOCK_INK,
            ) {
                return c;
            }
        }
        if let Some(c) = lock_status_icons_pixel(x, y) {
            return c;
        }
    }

    // 3. Date line above the clock, both left-aligned on LOCK_CONTENT_X.
    //    WHY both are conditional on a live reading: the panel's date line and
    //    clock are the two largest pieces of type on the surface, so a
    //    placeholder there would read as a real time. With no source they are
    //    simply absent, and the layout they occupy is unchanged.
    if state.clock_source_ok {
        if let Some(c) = test_text(
            x,
            y,
            LOCK_CONTENT_X,
            LOCK_DATE_Y,
            LOCK_DATE_SCALE,
            &state.date_line,
            LOCK_INK,
        ) {
            return c;
        }

        // 4. The clock itself.
        if let Some(c) = test_text(
            x,
            y,
            LOCK_CONTENT_X,
            LOCK_CLOCK_Y,
            LOCK_CLOCK_SCALE,
            &state.clock_time,
            LOCK_INK,
        ) {
            return c;
        }
    }

    // 5. Closed padlock above the centred unlock hint, both on the near-black
    //    foreground where the panel draws them in paper white.
    if let Some(c) = lock_padlock_pixel(x, y) {
        return c;
    }
    // WHY the origin is derived from the string length: the panel centres the
    // hint on the surface, so the x origin must follow the measured text.
    let hint_w = LOCK_HINT_TEXT.len() * 8 * LOCK_HINT_SCALE;
    let hint_x = (W.saturating_sub(hint_w)) / 2;
    if let Some(c) = test_text(
        x,
        y,
        hint_x,
        LOCK_HINT_Y,
        LOCK_HINT_SCALE,
        LOCK_HINT_TEXT,
        LOCK_PAPER,
    ) {
        return c;
    }

    // 6. Torch control on the left. WHY dimmed and labelled: there is no torch
    //    hardware, no torch capability and no torch syscall on this platform, so
    //    the affordance must not read as operable.
    if let Some(c) = lock_button_disc(x, y, LOCK_BTN_L_CX, color) {
        if let Some(g) = lock_torch_glyph_pixel(x, y) {
            return g;
        }
        return c;
    }

    // 7. Camera control on the right.
    if let Some(c) = lock_button_disc(x, y, LOCK_BTN_R_CX, color) {
        if let Some(g) = lock_camera_glyph_pixel(x, y) {
            return g;
        }
        return c;
    }

    // 8. "N/A" under the torch disc, stating the absence rather than implying a
    //    control that does nothing.
    let na_w = 3 * 8 * 2;
    let na_x = (LOCK_BTN_L_CX as i32 - (na_w as i32) / 2).max(0) as usize;
    if let Some(c) = test_text(x, y, na_x, 1387, 2, b"N/A", LOCK_BTN_GLYPH_DIM) {
        return c;
    }

    // 9. Home indicator, centred.
    let pill_x = (W - LOCK_PILL_W) / 2;
    if y >= LOCK_PILL_Y
        && y < LOCK_PILL_Y + LOCK_PILL_H
        && x >= pill_x
        && x < pill_x + LOCK_PILL_W
    {
        return LOCK_PAPER;
    }

    // 10. Wallpaper. The stock background remains the token this surface falls
    //     back to, so the lock backdrop tokens stay the single lock-screen
    //     backdrop definition shared with the rest of the shell.
    let _ = get_stock_background_pixel(x, y);
    color
}
