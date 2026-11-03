// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Guest-owned startup presentation rendered into the native portrait surface.
//!
//! The presentation is one vertical stack -- emblem, wordmark, tagline,
//! progress capsule, phase line -- whose ink is centred on the surface. Every
//! stage is a function of `boot_progress`, which the shell advances from the
//! tick clock, so the animation cannot drift from the hand-off that ends it and
//! it runs at the same speed whether the shell manages one frame per tick or
//! fewer.
//!
//! WHY the stack is centred as a whole and not the emblem alone: the eye reads
//! the group. Centring the 120 px tile on its own left the brand, capsule and
//! phase line hanging below the optical middle of a 1440 row surface, which is
//! what made the sequence look off-centre even though the tile itself was
//! centred.
//!
//! All metrics here are integer and `#![no_std]`. Text is measured with the
//! same advances the rasteriser steps by (`launcher::text`), so a centred line
//! is centred on the pixels that are actually inked rather than on a guess at
//! the glyph cell.

use core::cell::UnsafeCell;

use crate::{LauncherState, W};
use launcher::outline_text::OutlineRun;
use launcher::theme::Theme;

/// Theme used for startup presentation.
#[allow(dead_code)]
const THEME: Theme = launcher::theme::SHELL_DARK;

// -----------------------------------------------------------------------------
// The band this view owns
// -----------------------------------------------------------------------------

/// Rows `main.rs` repaints once the first (full-surface) boot frame is up.
///
/// WHY the band starts above the emblem: the breathing aura reaches roughly
/// 389, and the closing fade has to cover everything that can differ from the
/// obsidian void, or the fade ends at a visible seam. 384..896 contains the
/// aura, every glyph box, and the capsule.
pub const BAND_TOP: usize = 384;
pub const BAND_ROWS: usize = 512;

/// Ticks the boot presentation owns before the lock screen takes over.
///
/// WHY 3_200 and not 320: the kernel's system tick runs at 1 kHz
/// (`kernel/src/interrupts.rs`, `SYSTEM_TICK_HZ`), so the staged timeline needs
/// 3.2 s worth of ticks. 320 ticks is a third of a second, which the launcher's
/// own start-up work already consumes before the first loop iteration, so the
/// sequence was skipped entirely on real boots.
pub const BOOT_TICKS: u64 = 3_200;

/// Stage boundaries in `boot_progress` permille.
///
/// 0..140 mark, 120..420 wordmark wipe, 380..510 tagline, 560..900 progress,
/// 900..1000 ready, 940..1000 closing fade. The wordmark overlaps the mark on
/// purpose: the brand starts resolving while the emblem is still settling, so
/// the sequence reads as one movement instead of three separate cards.
const MARK_END: u16 = 140;
const WORDMARK_START: u16 = 120;
const WORDMARK_END: u16 = 420;
const TAGLINE_START: u16 = 380;
const TAG_STEP: u16 = 30;
const TAG_FADE: u16 = 70;
const PROGRESS_START: u16 = 560;
const FILL_START: u16 = 600;
const FILL_END: u16 = 900;
const FADE_START: u16 = 940;
/// How far the closing fade travels towards the void, out of 256.
const FADE_DEPTH: u32 = 210;

// -----------------------------------------------------------------------------
// Layout Geometry
// -----------------------------------------------------------------------------
//
// The ink span runs ICON_Y0..STATUS_Y + line_height(1) = 561..886, whose middle
// is row 723 -- three rows below the 720 line, inside the tolerance the eye
// reads as centred. The bottom is the caption run's line box; the run's own ink
// ends two rows above it, at STATUS_Y + 22. Each gap is stated as a gap between
// ink, not between boxes, so a larger wordmark or a descender cannot quietly
// eat the space between it and the tagline.

/// Sovereign Emblem Container (120x120).
const ICON_SIZE: usize = 120;
const ICON_X0: usize = (W - ICON_SIZE) / 2; // (720 - 120) / 2 = 300
const ICON_Y0: usize = 561;
const ICON_R: i32 = 24;
const ICON_CX: usize = 360;
const ICON_CY: usize = ICON_Y0 + ICON_SIZE / 2; // 621

/// 'I' Pill Geometry inside the 120x120 box.
///
/// WHY the letters sit three pixels left of the box centre: the monogram's ink
/// spans I_X0..L_FX1, whose centre was three pixels right of the tile's, and a
/// mark that is geometrically placed but optically right-heavy reads as a
/// rendering error at this size.
const I_X0: usize = 21;
const I_X1: usize = 41;
const I_Y0: usize = 24;
const I_Y1: usize = 96;
const I_R: i32 = 10;

/// 'L' Glyph Geometry inside the 120x120 box.
const L_SX0: usize = 47;
const L_SX1: usize = 67;
const L_SY0: usize = 24;
const L_FX0: usize = 47;
const L_FX1: usize = 99;
const L_FY0: usize = 76;
const L_FY1: usize = 96;
const L_R: i32 = 10;

/// Sovereign Brand: "InterOS".
const WORDMARK: &[u8] = b"InterOS";
const WORDMARK_EM: u32 = 52;
const WORDMARK_BASELINE: usize = 744;

/// Soft edge of the wordmark's left-to-right wipe, in pixels.
const WIPE_EDGE: i32 = 44;

/// Tagline: "SMARTER • PRIVATE • OPEN", drawn with the outline face at the body
/// rung so it reads above the phase line instead of beside it.
const TAG_SCALE: usize = 2;
const TAGLINE_Y: usize = 758;
const TAGLINE_BAND_H: usize = 40;
const TAG1: &[u8] = b"SMARTER";
const TAG2: &[u8] = b"PRIVATE";
const TAG3: &[u8] = b"OPEN";
const TAG_WORDS: [&[u8]; 3] = [TAG1, TAG2, TAG3];
const TAG_GAP: usize = 26;

/// Progress Capsule Bar.
const BAR_W: usize = 280;
const BAR_H: usize = 8;
const BAR_X0: usize = (W - BAR_W) / 2; // (720 - 280) / 2 = 220
const BAR_Y0: usize = 828;
const BAR_R: i32 = 4;

/// System Status Phase subtitle (the caption rung, drawn with the outline face).
///
/// WHY the band is the ladder's own line box and not a remembered number: the
/// phase line goes through `test_text_aa` at rung 1, which hands it to the
/// outline face at `cap_for_scale(1)` = 15 px -- em 21, ascent 22, descent 5 --
/// so the run reserves `font::line_height(1)` = 27 rows and its coverage buffer
/// is exactly that tall: rows `STATUS_Y..STATUS_Y + 27` are all the run can ever
/// paint, and a row below that is a row nothing can ink. Measured off the three
/// phase strings the solid ink runs from `STATUS_Y + 7` to `STATUS_Y + 21` and
/// the anti-aliased overshoot of the round letters reaches `STATUS_Y + 22`, so
/// the 21 rows this constant used to hard-code -- the retired ladder's caption
/// box, for a 13 px cap at em 18 with ascent 18 and descent 3 -- cut the
/// baseline row off every capital. The bitmap face's 24 px cell would clip a
/// descender (a control string inks as low as `STATUS_Y + 26`), and that cell is
/// a different quantity anyway: it is what the face the outline run falls back to
/// inks from `STATUS_Y`, not what this layout reserves.
const STATUS_SCALE: usize = 1;
const STATUS_Y: usize = 859;
const STATUS_BAND_H: usize = launcher::font::line_height(STATUS_SCALE);
const STATUS_COPY: [&[u8]; 3] = [
    b"STARTING SOVEREIGN LAUNCHER",
    b"PREPARING HOME SURFACE",
    b"SOVEREIGN PLATFORM READY",
];
const STATUS_WINDOW: [(u16, u16); 3] = [(400, 600), (600, 880), (880, 1000)];
const STATUS_FADE: u16 = 44;

/// Aura geometry: a soft pool of light behind the mark.
const AURA_R_MIN: u32 = 224;
const AURA_R_SWING: u32 = 10;

// -----------------------------------------------------------------------------
// Color Palettes
// -----------------------------------------------------------------------------
const COLOR_BG: u32 = 0xFF050608;              // Obsidian True Black (OLED depth)
const COLOR_AURA: u32 = 0xFF101A30;            // Cool sovereign bloom
const COLOR_RING: u32 = 0xFF6EA8C8;            // Entrance pulse ring
const COLOR_IVORY: u32 = 0xFFFAF8F4;           // Warm Ivory (letters & wordmark)
const COLOR_WHITE: u32 = 0xFFF4FBFF;           // Shimmer peak specular crest
const COLOR_GLASS_TOP: u32 = 0xFF161B26;       // Monolith top stop
const COLOR_GLASS_BOTTOM: u32 = 0xFF0A0D14;    // Monolith bottom stop
const COLOR_GLASS_BORDER: u32 = 0xFF263042;    // Frosted beveled border
const COLOR_GLASS_HIGHLIGHT: u32 = 0xFF3A4860; // Top rim light bevel
const COLOR_TAGLINE: u32 = 0xFF8E9BAE;         // Refined silver tagline
const COLOR_CYAN_GLOW: u32 = 0xFF00D2FF;       // Electric cyan leading edge
const COLOR_CYAN_CORE: u32 = 0xFF8AE8FF;       // Brilliant cyan/white head
const COLOR_CYAN_DEEP: u32 = 0xFF0077C8;       // Fill base
const COLOR_TRACK_BG: u32 = 0xFF10141C;        // Progress track
const COLOR_TRACK_BORDER: u32 = 0xFF232C3D;    // Progress track hairline
const COLOR_MUTED_TEXT: u32 = 0xFF8493A8;      // Phase status text

// -----------------------------------------------------------------------------
// The Main Boot Screen Painter
// -----------------------------------------------------------------------------

/// Computes the colour of one pixel `(x, y)` of the boot presentation.
pub fn get_boot_screen_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let bg = boot_background(x, y, state);
    let mut color = bg;

    if let Some(c) = emblem_pixel(x, y, bg, state) {
        color = c;
    } else if let Some(c) = wordmark_pixel(x, y, state, bg) {
        color = c;
    } else if let Some(c) = tagline_pixel(x, y, state, bg) {
        color = c;
    } else if let Some(c) = progress_capsule_pixel(x, y, bg, state) {
        color = c;
    } else if let Some(c) = status_subtitle_pixel(x, y, state, bg) {
        color = c;
    }

    // Closing fade: the boot surface dims to the obsidian void before the lock
    // screen is rendered, so the hand-off does not read as a cut.
    //
    // WHY the fade stops short of pure black: the lock surface is a full repaint
    // and takes a few hundred milliseconds under TCG while the framebuffer still
    // holds the last boot frame. Fading all the way left that window as a dead
    // black screen; a residual 12% keeps the composition visible until the lock
    // pixels land.
    if state.boot_progress >= FADE_START {
        let t = ((state.boot_progress - FADE_START) as u32 * FADE_DEPTH
            / (1000 - FADE_START) as u32)
            .min(FADE_DEPTH);
        color = Theme::blend(color, COLOR_BG, t);
    }

    color
}

/// The obsidian void, its breathing aura and the entrance pulse.
fn boot_background(x: usize, y: usize, state: &LauncherState) -> u32 {
    let cx = x as i32 - ICON_CX as i32;
    let cy = y as i32 - ICON_CY as i32;
    let dist_sq = (cx * cx + cy * cy) as u32;

    let cycle = breath(state.boot_progress, 3);
    let radius = AURA_R_MIN + cycle * AURA_R_SWING / 255;
    let mut bg = COLOR_BG;

    if dist_sq < radius * radius {
        let falloff = (radius * radius - dist_sq) * 256 / (radius * radius);
        let weight = (falloff * falloff) >> 8;
        let breath_alpha = 118 + cycle * 137 / 255;
        let alpha = (weight * breath_alpha) >> 8;
        let alpha = alpha * ramp(state.boot_progress, 0, MARK_END) / 255;
        bg = Theme::blend(COLOR_BG, COLOR_AURA, alpha.min(255));
    }

    let ring = entrance_ring(state.boot_progress, dist_sq);
    if ring > 0 {
        bg = Theme::blend(bg, COLOR_RING, ring);
    }

    bg
}

// -----------------------------------------------------------------------------
// Element Renderers
// -----------------------------------------------------------------------------

/// Sovereign Emblem: frosted obsidian monolith tile (120x120).
fn emblem_pixel(x: usize, y: usize, bg: u32, state: &LauncherState) -> Option<u32> {
    if x < ICON_X0 || x >= ICON_X0 + ICON_SIZE || y < ICON_Y0 || y >= ICON_Y0 + ICON_SIZE {
        return None;
    }

    // Mark stage: the tile fades up and eases from 90% to its resting size.
    // Sampling the 120 px artwork in source space keeps the letters, bevel and
    // corner radius coherent instead of scaling each shape independently.
    let reveal = ease_out(ramp(state.boot_progress, 0, MARK_END));
    let alpha = 140 + reveal * 115 / 255;
    let scale = (900 + reveal * 100 / 255) as i32;
    let half = ICON_SIZE as i32 / 2;
    let bx_src = half + ((x - ICON_X0) as i32 - half) * 1000 / scale;
    let by_src = half + ((y - ICON_Y0) as i32 - half) * 1000 / scale;
    if bx_src < 0
        || by_src < 0
        || bx_src >= ICON_SIZE as i32
        || by_src >= ICON_SIZE as i32
    {
        return None;
    }
    let bx = bx_src as usize;
    let by = by_src as usize;
    let r = ICON_R;

    let cx = if bx < r as usize {
        r - bx as i32
    } else if bx > ICON_SIZE - 1 - r as usize {
        bx as i32 - (ICON_SIZE - 1 - r as usize) as i32
    } else {
        0
    };
    let cy = if by < r as usize {
        r - by as i32
    } else if by > ICON_SIZE - 1 - r as usize {
        by as i32 - (ICON_SIZE - 1 - r as usize) as i32
    } else {
        0
    };
    let distance_sq = (cx * cx + cy * cy) as u32;
    if distance_sq > (r * r) as u32 {
        return None;
    }

    let sweep_phase = (motion(state.boot_progress, 2) as i32) * 360 / 1000 - 60;
    let sweep_dist = (bx as i32 + by as i32 - sweep_phase).abs();

    let color = if distance_sq >= ((r - 2) * (r - 2)) as u32
        || bx == 0
        || bx == ICON_SIZE - 1
        || by == 0
        || by == ICON_SIZE - 1
    {
        // 1 px border with a top specular highlight.
        if by <= 2 {
            COLOR_GLASS_HIGHLIGHT
        } else {
            COLOR_GLASS_BORDER
        }
    } else if let Some(letter) = monogram_pixel(bx, by, sweep_dist) {
        letter
    } else {
        // Glass surface: a top-to-bottom depth gradient plus the travelling
        // specular band. The band is wider than the letters' so the highlight
        // reads as light crossing the tile, not as the glyphs changing colour.
        let t_y = (by * 256 / ICON_SIZE) as u32;
        let mut tile = Theme::blend(COLOR_GLASS_TOP, COLOR_GLASS_BOTTOM, t_y);
        if sweep_dist < 34 {
            let s_alpha = ((34 - sweep_dist) * 92 / 34) as u32;
            tile = Theme::blend(tile, 0xFF354664, s_alpha);
        }
        tile
    };

    Some(Theme::blend(bg, color, alpha))
}

/// The "IL" monogram inside the tile, or `None` when `(bx, by)` misses it.
fn monogram_pixel(bx: usize, by: usize, sweep_dist: i32) -> Option<u32> {
    let in_i = {
        let mut inside = false;
        if bx >= I_X0 && bx < I_X1 && by >= I_Y0 && by < I_Y1 {
            let icx = if bx < I_X0 + I_R as usize {
                (I_X0 + I_R as usize - 1) as i32 - bx as i32
            } else if bx >= I_X1 - I_R as usize {
                bx as i32 - (I_X1 - I_R as usize) as i32
            } else {
                0
            };
            let icy = if by < I_Y0 + I_R as usize {
                (I_Y0 + I_R as usize - 1) as i32 - by as i32
            } else if by >= I_Y1 - I_R as usize {
                by as i32 - (I_Y1 - I_R as usize) as i32
            } else {
                0
            };
            inside = (icx * icx + icy * icy) as u32 <= (I_R * I_R) as u32;
        }
        inside
    };

    let in_l = {
        let mut inside = false;
        if bx >= L_SX0 && bx < L_SX1 && by >= L_SY0 && by < L_FY0 {
            let scx = if bx < L_SX0 + L_R as usize {
                (L_SX0 + L_R as usize - 1) as i32 - bx as i32
            } else if bx >= L_SX1 - L_R as usize {
                bx as i32 - (L_SX1 - L_R as usize) as i32
            } else {
                0
            };
            let scy = if by < L_SY0 + L_R as usize {
                (L_SY0 + L_R as usize - 1) as i32 - by as i32
            } else {
                0
            };
            inside = (scx * scx + scy * scy) as u32 <= (L_R * L_R) as u32;
        } else if bx >= L_FX0 && bx < L_FX1 && by >= L_FY0 && by < L_FY1 {
            let fcx = if bx < L_FX0 + L_R as usize {
                (L_FX0 + L_R as usize - 1) as i32 - bx as i32
            } else if bx >= L_FX1 - L_R as usize {
                bx as i32 - (L_FX1 - L_R as usize) as i32
            } else {
                0
            };
            let fcy = if by >= L_FY1 - L_R as usize {
                by as i32 - (L_FY1 - L_R as usize) as i32
            } else {
                0
            };
            inside = (fcx * fcx + fcy * fcy) as u32 <= (L_R * L_R) as u32;
        }
        inside
    };

    if !in_i && !in_l {
        return None;
    }

    Some(if sweep_dist < 24 {
        let t = ((24 - sweep_dist) * 255 / 24) as u32;
        mix(COLOR_IVORY, COLOR_WHITE, t)
    } else {
        COLOR_IVORY
    })
}

/// Sovereign "InterOS" brand, wiped in from the left.
fn wordmark_pixel(x: usize, y: usize, state: &LauncherState, bg: u32) -> Option<u32> {
    let staged = state
        .boot_progress
        .saturating_sub(WORDMARK_START)
        .min(WORDMARK_END - WORDMARK_START);
    if staged == 0 {
        return None;
    }

    let mark = wordmark_frame();
    let run = mark.run.as_ref()?;

    if x < mark.box_x || x - mark.box_x >= mark.box_w {
        return None;
    }
    if y < mark.box_y || y - mark.box_y >= mark.box_h {
        return None;
    }

    // Left-to-right wipe with a soft edge. The wipe position travels past the
    // box by WIPE_EDGE so the trailing soft edge clears the final glyph instead
    // of stopping halfway through it.
    let span = (WORDMARK_END - WORDMARK_START) as i32;
    let reveal_px = staged as i32 * (mark.box_w as i32 + WIPE_EDGE) / span;
    let cover = (reveal_px - (x - mark.box_x) as i32).clamp(0, WIPE_EDGE);
    if cover == 0 {
        return None;
    }
    let alpha = (cover * 255 / WIPE_EDGE) as u32;

    // A specular band crosses the brand as it resolves, then keeps travelling.
    let shimmer_x =
        -70 + (motion(state.boot_progress, 3) as i32) * ((W as i32) + 140) / 1000;
    let dist = (x as i32 - shimmer_x).abs();
    let ink = if dist < 26 {
        let t = ((26 - dist) * 120 / 26) as u32;
        mix(COLOR_IVORY, COLOR_WHITE, t)
    } else {
        COLOR_IVORY
    };
    let ink = Theme::blend(bg, ink, alpha);

    run.pixel(x, y, ink, bg)
}

/// Official brand tagline: "SMARTER • PRIVATE • OPEN".
fn tagline_pixel(x: usize, y: usize, state: &LauncherState, bg: u32) -> Option<u32> {
    if y < TAGLINE_Y || y >= TAGLINE_Y + TAGLINE_BAND_H {
        return None;
    }

    let layout = tagline_layout();

    for (i, word) in TAG_WORDS.iter().enumerate() {
        let alpha = word_alpha(state.boot_progress, TAGLINE_START + i as u16 * TAG_STEP);
        if alpha == 0 || x < layout.x[i] || x - layout.x[i] >= layout.w[i] {
            continue;
        }
        let ink = tagline_ink(bg, state, alpha);
        if let Some(c) = crate::test_text_aa(
            x,
            y,
            layout.x[i],
            TAGLINE_Y,
            TAG_SCALE,
            word,
            ink,
            bg,
        ) {
            return Some(c);
        }
    }

    // Separator dots share the reveal of the word that precedes them, so the
    // line assembles left to right instead of flashing in as a block.
    let dot_cy = TAGLINE_Y + 17;
    let dy = y as i32 - dot_cy as i32;
    if dy.abs() > 2 {
        return None;
    }
    for (i, dot_x) in layout.dot.iter().enumerate() {
        let alpha = word_alpha(state.boot_progress, TAGLINE_START + i as u16 * TAG_STEP);
        if alpha == 0 {
            continue;
        }
        let dx = x as i32 - *dot_x as i32;
        let d2 = dx * dx + dy * dy;
        if d2 <= 2 {
            return Some(tagline_ink(bg, state, alpha));
        }
        if d2 <= 5 {
            return Some(Theme::blend(bg, tagline_ink(bg, state, alpha), 110));
        }
    }

    None
}

/// Silver ink for one tagline element, briefly lifted to white as it settles.
fn tagline_ink(bg: u32, state: &LauncherState, alpha: u32) -> u32 {
    let settle = (255 - alpha) * 90 / 255;
    let breath = breath(state.boot_progress, 3);
    let lift = (settle + 18 + breath * 24 / 255).min(255);
    let base = mix(COLOR_TAGLINE, COLOR_WHITE, lift);
    Theme::blend(bg, base, alpha)
}

/// Minimal sovereign capsule progress bar.
fn progress_capsule_pixel(
    x: usize,
    y: usize,
    bg: u32,
    state: &LauncherState,
) -> Option<u32> {
    if state.boot_progress < PROGRESS_START {
        return None;
    }
    if y < BAR_Y0 || y >= BAR_Y0 + BAR_H {
        return None;
    }
    if x < BAR_X0 || x >= BAR_X0 + BAR_W {
        return None;
    }

    let cy = (BAR_Y0 + BAR_H / 2) as i32;
    let dy = y as i32 - cy;
    let x_i = x as i32;
    let left = BAR_X0 as i32 + BAR_R;
    let right = (BAR_X0 + BAR_W - 1) as i32 - BAR_R;
    let dx = if x_i < left {
        left - x_i
    } else if x_i > right {
        x_i - right
    } else {
        0
    };

    // Quarter-pixel thresholds keep the hairline one pixel wide on a capsule
    // whose radius is half its height; comparing squared distances alone would
    // make the top and bottom rows two pixels of border.
    let dist4 = (dx * dx + dy * dy) * 4;
    if dist4 > 4 * BAR_R * BAR_R {
        return None;
    }

    let color = if dist4 >= (2 * BAR_R - 1) * (2 * BAR_R - 1) {
        COLOR_TRACK_BORDER
    } else if dist4 <= (2 * BAR_R - 3) * (2 * BAR_R - 3) {
        let content_x0 = BAR_X0 + 2;
        let fill_max = BAR_W - 4;
        let staged = state
            .boot_progress
            .saturating_sub(FILL_START)
            .min(FILL_END - FILL_START);
        let eased = ease_in_out(staged as u32 * 255 / (FILL_END - FILL_START) as u32);
        let fill_w = eased * fill_max as u32 / 255;
        let rel = x.saturating_sub(content_x0) as u32;
        if rel < fill_w {
            let t = (rel * 256 / fill_max.max(1) as u32).min(256);
            let mut fill = Theme::blend(COLOR_CYAN_DEEP, COLOR_CYAN_GLOW, t);
            // The leading edge is the only bright bead: a second scanning bead
            // detached from the fill read as a glitch, not as loading.
            let remaining = fill_w - rel;
            if remaining <= 1 {
                fill = mix(fill, COLOR_WHITE, 235);
            } else if remaining <= 12 {
                fill = mix(fill, COLOR_CYAN_CORE, (12 - remaining) * 16);
            }
            fill
        } else {
            COLOR_TRACK_BG
        }
    } else {
        COLOR_TRACK_BG
    };

    let fade = ramp(state.boot_progress, PROGRESS_START, PROGRESS_START + 50);
    Some(Theme::blend(bg, color, fade))
}

/// Dynamic system phase line.
fn status_subtitle_pixel(x: usize, y: usize, state: &LauncherState, bg: u32) -> Option<u32> {
    if y < STATUS_Y || y >= STATUS_Y + STATUS_BAND_H {
        return None;
    }

    let layout = tagline_layout();
    for i in 0..STATUS_COPY.len() {
        let (start, end) = STATUS_WINDOW[i];
        let alpha = window_alpha(state.boot_progress, start, end, STATUS_FADE);
        if alpha == 0 {
            continue;
        }
        let x0 = layout.status_x[i];
        if x < x0 || x - x0 >= layout.status_w[i] {
            continue;
        }
        let ink = Theme::blend(bg, COLOR_MUTED_TEXT, alpha);
        if let Some(c) =
            crate::test_text_aa(x, y, x0, STATUS_Y, STATUS_SCALE, STATUS_COPY[i], ink, bg)
        {
            return Some(c);
        }
    }

    None
}

// -----------------------------------------------------------------------------
// Timing helpers
// -----------------------------------------------------------------------------

/// Linear stage ramp, 0..=255.
#[inline]
fn ramp(progress: u16, start: u16, end: u16) -> u32 {
    if progress <= start {
        return 0;
    }
    if progress >= end {
        return 255;
    }
    (progress - start) as u32 * 255 / (end - start) as u32
}

/// Ease-out quad over 0..=255, used for entrances that settle rather than stop.
#[inline]
fn ease_out(t: u32) -> u32 {
    let inv = 255 - t.min(255);
    255 - inv * inv / 255
}

/// Ease-in-out quad over 0..=255, used for the progress fill.
#[inline]
fn ease_in_out(t: u32) -> u32 {
    let t = t.min(255);
    if t < 128 {
        2 * t * t / 255
    } else {
        let inv = 255 - t;
        255 - 2 * inv * inv / 255
    }
}

/// Triangle wave in 0..=255 with `cycles` full breaths across the boot.
#[inline]
fn breath(progress: u16, cycles: u32) -> u32 {
    let p = (progress as u32 * cycles) % 1000;
    let tri = if p < 500 { p } else { 1000 - p };
    tri * 255 / 500
}

/// Linear motion phase in 0..=999 with `cycles` passes across the boot.
#[inline]
fn motion(progress: u16, cycles: u32) -> u32 {
    (progress as u32 * cycles) % 1000
}

/// Alpha of a copy window: fades in, holds, fades out, and is zero outside.
fn window_alpha(progress: u16, start: u16, end: u16, fade: u16) -> u32 {
    if progress < start || progress >= end {
        return 0;
    }
    let a_in = (progress - start).min(fade) as u32 * 255 / fade as u32;
    let a_out = (end - progress).min(fade) as u32 * 255 / fade as u32;
    a_in.min(a_out)
}

/// Alpha of one tagline word after `start`.
fn word_alpha(progress: u16, start: u16) -> u32 {
    let staged = progress.saturating_sub(start).min(TAG_FADE);
    if staged == 0 {
        return 0;
    }
    ease_out(staged as u32 * 255 / TAG_FADE as u32)
}

/// One ring expanding off the tile as it settles, then gone.
fn entrance_ring(progress: u16, dist_sq: u32) -> u32 {
    const END: u32 = 300;
    let t = (progress as u32).min(END);
    if t >= END {
        return 0;
    }
    let radius = 76 + t * 64 / END;
    let inner = radius.saturating_sub(1);
    let outer = radius + 2;
    if dist_sq < inner * inner || dist_sq > outer * outer {
        return 0;
    }
    (END - t) * 90 / END
}

// -----------------------------------------------------------------------------
// Utilities
// -----------------------------------------------------------------------------

#[inline]
fn mix(a: u32, b: u32, t: u32) -> u32 {
    Theme::blend(a, b, t.min(256))
}

// -----------------------------------------------------------------------------
// Layout memos
// -----------------------------------------------------------------------------
//
// Both memos exist because the pixel painter runs once per row pixel in the
// band: measuring a run inside it would walk the advance tables hundreds of
// thousands of times per frame. The values are constants of the design, so they
// are resolved on first use and never change.

struct Layout {
    resolved: bool,
    x: [usize; 3],
    w: [usize; 3],
    dot: [usize; 2],
    status_x: [usize; 3],
    status_w: [usize; 3],
}

struct LayoutMemo(UnsafeCell<Layout>);
unsafe impl Sync for LayoutMemo {}

static LAYOUT: LayoutMemo = LayoutMemo(UnsafeCell::new(Layout {
    resolved: false,
    x: [0; 3],
    w: [0; 3],
    dot: [0; 2],
    status_x: [0; 3],
    status_w: [0; 3],
}));

fn tagline_layout() -> &'static Layout {
    let slot = unsafe { &mut *LAYOUT.0.get() };
    if !slot.resolved {
        slot.resolved = true;

        let mut widths = [0usize; 3];
        for i in 0..3 {
            widths[i] = launcher::text::measure(TAG_WORDS[i], TAG_SCALE);
        }
        let total = widths[0] + TAG_GAP + widths[1] + TAG_GAP + widths[2];
        let mut x = (W.saturating_sub(total)) / 2;
        for i in 0..3 {
            slot.x[i] = x;
            slot.w[i] = widths[i];
            x += widths[i] + TAG_GAP;
        }
        slot.dot[0] = slot.x[0] + widths[0] + TAG_GAP / 2;
        slot.dot[1] = slot.x[1] + widths[1] + TAG_GAP / 2;

        for i in 0..STATUS_COPY.len() {
            // The caption rung is rasterised through the outline face at its
            // 15 px cap, so its width is the outline advance at that cap -- the
            // bitmap cell advance is wider and would hang the line left of
            // centre by the difference.
            let w = interos_std::design::monochrome::text_width(
                STATUS_COPY[i],
                interos_std::font::cap_for_scale(STATUS_SCALE),
            );
            slot.status_w[i] = w;
            slot.status_x[i] = (W.saturating_sub(w)) / 2;
        }
    }
    slot
}

struct Wordmark {
    resolved: bool,
    run: Option<OutlineRun>,
    box_x: usize,
    box_y: usize,
    box_w: usize,
    box_h: usize,
}

struct WordmarkMemo(UnsafeCell<Wordmark>);
unsafe impl Sync for WordmarkMemo {}

static WORDMARK_RUN: WordmarkMemo = WordmarkMemo(UnsafeCell::new(Wordmark {
    resolved: false,
    run: None,
    box_x: 0,
    box_y: 0,
    box_w: 0,
    box_h: 0,
}));

fn wordmark_frame() -> &'static Wordmark {
    let slot = unsafe { &mut *WORDMARK_RUN.0.get() };
    if !slot.resolved {
        slot.resolved = true;
        let adv = interos_std::design::typeface::text_advance_px(WORDMARK, WORDMARK_EM) as usize;
        let x0 = (W.saturating_sub(adv)) / 2;
        if let Some(run) = OutlineRun::place(x0, WORDMARK_BASELINE, WORDMARK_EM, WORDMARK) {
            let (bx, by, bw, bh) = run.bounds();
            slot.box_x = bx;
            slot.box_y = by;
            slot.box_w = bw;
            slot.box_h = bh;
            slot.run = Some(run);
        }
    }
    slot
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::H;

    #[test]
    fn the_surface_stays_backdrop_outside_the_stack() {
        let state = LauncherState::new();
        assert_eq!(get_boot_screen_color(0, 0, &state), COLOR_BG);
        assert_eq!(get_boot_screen_color(W - 1, H - 1, &state), COLOR_BG);
    }

    #[test]
    fn the_stack_stays_inside_the_rows_main_rs_repaints() {
        assert!(ICON_Y0 >= BAND_TOP);
        assert!(STATUS_Y + STATUS_BAND_H <= BAND_TOP + BAND_ROWS);
        // The band is the rung's own line box rather than a remembered number,
        // so the phase line can never be clipped by a box narrower than the run
        // drawn in it.
        assert_eq!(STATUS_BAND_H, launcher::font::line_height(STATUS_SCALE));
    }

    #[test]
    fn the_phase_line_band_holds_the_run_and_its_neighbour() {
        // The outline run paints rows STATUS_Y..STATUS_Y + line_height(1) and
        // the bitmap face it can fall back to inks that face's 24 px cell from
        // the same top, so the band has to contain both.
        assert!(STATUS_BAND_H >= launcher::font::line_height(STATUS_SCALE));
        assert!(STATUS_BAND_H >= launcher::text::line_height(STATUS_SCALE));
        // Growing the band downward must not walk it into the tagline above or
        // the capsule that sits between them.
        assert!(TAGLINE_Y + TAGLINE_BAND_H <= STATUS_Y);
        assert!(BAR_Y0 + BAR_H <= STATUS_Y);
    }

    #[test]
    fn the_stack_is_optically_centred_on_the_surface() {
        let top = ICON_Y0;
        // The stack's bottom is the caption run's line box, which is the space
        // the surface reserves for it; its inked rows land two inside that.
        let bottom = STATUS_Y + STATUS_BAND_H;
        let centre = (top + bottom) / 2;
        assert!(
            (centre as i32 - (H / 2) as i32).abs() <= 4,
            "stack centre {centre} is not within four rows of {}",
            H / 2
        );
    }

    #[test]
    fn every_line_is_centred_and_fits() {
        let layout = tagline_layout();
        let last = layout.x[2] + layout.w[2];
        let first = layout.x[0];
        assert!(
            (first as i32 - (W - last) as i32).abs() <= 1,
            "tagline margins differ: {first} vs {}",
            W - last
        );
        assert!(first >= 32, "tagline must keep a real margin");
        for i in 0..STATUS_COPY.len() {
            assert!(layout.status_x[i] >= 32);
            assert!(layout.status_x[i] + layout.status_w[i] <= W - 32);
        }
    }

    #[test]
    fn the_boot_surface_reveals_in_stages() {
        let mut state = LauncherState::new();
        state.boot_frame = 0;
        state.boot_progress = 0;

        // Mark stage: the emblem is present, the brand and capsule are not.
        assert!(
            ink_in(
                &state,
                ICON_X0,
                ICON_X0 + ICON_SIZE,
                ICON_Y0,
                ICON_Y0 + ICON_SIZE
            ) > 0,
            "the emblem must be visible from the first frame"
        );
        let mark = wordmark_frame();
        assert_eq!(
            ink_in(
                &state,
                mark.box_x,
                mark.box_x + mark.box_w,
                mark.box_y,
                mark.box_y + mark.box_h
            ),
            0,
            "the wordmark must not be present before its stage"
        );
        assert_eq!(
            ink_in(&state, BAR_X0, BAR_X0 + BAR_W, BAR_Y0, BAR_Y0 + BAR_H),
            0,
            "the progress capsule must not be present before its stage"
        );

        // Wordmark stage.
        state.boot_progress = 300;
        assert!(
            ink_in(
                &state,
                mark.box_x,
                mark.box_x + mark.box_w,
                mark.box_y,
                mark.box_y + mark.box_h
            ) > 0,
            "the wordmark must be inked once its stage has begun"
        );

        // Progress stage.
        state.boot_progress = 620;
        assert!(
            ink_in(&state, BAR_X0, BAR_X0 + BAR_W, BAR_Y0, BAR_Y0 + BAR_H) > 0,
            "the capsule must be inked once loading has begun"
        );

        // Ready stage: the surface is dimmed almost to the void by the last
        // frame so the lock screen can take over without a visible cut, while a
        // residual glimmer keeps the hand-off frame from being dead black.
        state.boot_progress = 1000;
        let faded = get_boot_screen_color(ICON_CX, ICON_CY, &state);
        assert!(
            ((faded >> 16) & 0xFF) < 64,
            "the closing frame must be dim, got #{faded:06X}"
        );
    }

    fn ink_in(state: &LauncherState, x0: usize, x1: usize, y0: usize, y1: usize) -> usize {
        let mut ink = 0usize;
        for y in y0..y1 {
            for x in x0..x1 {
                if get_boot_screen_color(x, y, state) != boot_background(x, y, state) {
                    ink += 1;
                }
            }
        }
        ink
    }
}
