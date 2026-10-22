//! Guest-owned startup presentation rendered into the native portrait surface.
//!
//! Sovereign InterOS boot sequence inspired by modern sovereign mobile design:
//!   * Deep obsidian black background (0xFF050608)
//!   * Fluid, continuous breathing luminescence aura pulsating at 60 Hz
//!   * Sovereign Emblem: Frosted glass obsidian monolith squircle with traveling specular shimmer
//!   * Centered "IL" Monogram in Warm Ivory (0xFFFAF8F4) with radiant light wave
//!   * Sovereign Brand: "InterOS" in crisp Warm Ivory scalable vector outline typeface
//!   * Brand Tagline: "SMARTER • PRIVATE • OPEN" in luminous silver typography
//!   * Minimalist progress capsule with glowing electric cyan gradient fill
//!   * High-contrast, strictly `#![no_std]` integer math without floating point.

use core::cell::UnsafeCell;

use crate::{LauncherState, H, W};
use launcher::outline_text::OutlineRun;
use launcher::theme::Theme;

/// Theme used for startup presentation.
#[allow(dead_code)]
const THEME: Theme = launcher::theme::SHELL_DARK;

// -----------------------------------------------------------------------------
// The band this view owns
// -----------------------------------------------------------------------------

/// Rows `main.rs` repaints while this view is active: rows 590..940 (350 rows).
const BAND_TOP: usize = H / 2 - 130;
const BAND_ROWS: usize = 350;

// -----------------------------------------------------------------------------
// Layout Geometry within the band (y = 590..940)
// -----------------------------------------------------------------------------

/// Sovereign Emblem Container (120x120 centered at X=360, Y=598):
const ICON_SIZE: usize = 120;
const ICON_X0: usize = (W - ICON_SIZE) / 2; // (720 - 120) / 2 = 300
const ICON_Y0: usize = 598;
const ICON_R: i32 = 24;

/// 'I' Pill Geometry inside the 120x120 box:
const I_X0: usize = 24;
const I_X1: usize = 44;
const I_Y0: usize = 24;
const I_Y1: usize = 96;
const I_R: i32 = 10;

/// 'L' Glyph Geometry inside the 120x120 box:
const L_SX0: usize = 50;
const L_SX1: usize = 70;
const L_SY0: usize = 24;
#[allow(dead_code)]
const L_SY1: usize = 96;
const L_FX0: usize = 50;
const L_FX1: usize = 102;
const L_FY0: usize = 76;
const L_FY1: usize = 96;
const L_R: i32 = 10;

/// Sovereign Brand: "InterOS"
const WORDMARK: &[u8] = b"InterOS";
const WORDMARK_EM: u32 = 52;
const WORDMARK_BASELINE: usize = 766;

/// Tagline: "SMARTER • PRIVATE • OPEN"
const TAGLINE_Y: usize = 794;
const TAG1: &[u8] = b"SMARTER";
const TAG2: &[u8] = b"PRIVATE";
const TAG3: &[u8] = b"OPEN";
const TAG_GAP: usize = 24;

/// Progress Capsule Bar:
const BAR_W: usize = 240;
const BAR_H: usize = 6;
const BAR_X0: usize = (W - BAR_W) / 2; // (720 - 240) / 2 = 240
const BAR_Y0: usize = 838;
const BAR_R: i32 = 3;

/// System Status Phase subtitle:
const STATUS_Y: usize = 862;

// -----------------------------------------------------------------------------
// Color Palettes
// -----------------------------------------------------------------------------
const COLOR_BG: u32 = 0xFF050608;              // Obsidian True Black (OLED depth)
const COLOR_IVORY: u32 = 0xFFFAF8F4;           // Warm Ivory (Letters & Wordmark)
const COLOR_WHITE: u32 = launcher::theme::SHELL_DARK.ink_on_dark;           // Shimmer Peak Specular Crest
const COLOR_GLASS_TOP: u32 = 0xFF161B26;       // Monolith Top Stop
const COLOR_GLASS_BOTTOM: u32 = 0xFF0A0D14;    // Monolith Bottom Stop
const COLOR_GLASS_BORDER: u32 = 0xFF263042;    // Frosted Beveled Border
const COLOR_GLASS_HIGHLIGHT: u32 = 0xFF3A4860; // Top Rim Light Bevel
const COLOR_TAGLINE: u32 = 0xFF8E9BAE;         // Refined Silver Tagline
const COLOR_CYAN_GLOW: u32 = 0xFF00D2FF;       // Electric Cyan Progress Leading Edge
const COLOR_CYAN_CORE: u32 = 0xFF8AE8FF;       // Brilliant Cyan/White Progress Head
const COLOR_TRACK_BG: u32 = 0xFF10141C;        // Progress Bar Track
const COLOR_TRACK_BORDER: u32 = 0xFF1C2230;    // Progress Bar Track Border
const COLOR_MUTED_TEXT: u32 = 0xFF728094;      // Minimalist Phase Status Text

// -----------------------------------------------------------------------------
// The Main Boot Screen Painter
// -----------------------------------------------------------------------------

/// Computes the colour of one pixel `(x, y)` of the boot presentation.
pub fn get_boot_screen_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // Outside the active band: sovereign obsidian black void.
    if y < BAND_TOP || y >= BAND_TOP + BAND_ROWS {
        return COLOR_BG;
    }

    // Dynamic aura background calculation:
    // Continuous breathing luminescence pulse centered around the emblem (360, 658)
    let cx = x as i32 - 360;
    let cy = y as i32 - 658;
    let dist_sq = (cx * cx + cy * cy) as u32;

    let cycle = (state.boot_frame % 120) as i32;
    let phase = if cycle < 60 { cycle } else { 120 - cycle };
    let pulse = (phase * 255 / 60) as u32; // 0..=255 smooth breathing wave

    let mut bg = COLOR_BG;
    const AURA_R_SQ: u32 = 220 * 220;
    if dist_sq < AURA_R_SQ {
        let falloff = ((AURA_R_SQ - dist_sq) * 256 / AURA_R_SQ) as u32;
        let weight = (falloff * falloff) >> 8;
        let breath = 110 + (pulse * 145 / 255);
        let alpha = (weight * breath) >> 8;
        let aura_color = 0xFF121E36;
        bg = Theme::blend(COLOR_BG, aura_color, alpha.min(255));
    }

    // 1. Sovereign Emblem: Frosted Obsidian Monolith (y = 598..718)
    if let Some(c) = emblem_pixel(x, y, state) {
        return c;
    }

    // 2. Sovereign "InterOS" Brand (y ≈ 725..775)
    if let Some(c) = wordmark_pixel(x, y, state, bg) {
        return c;
    }

    // 3. Official Brand Tagline (y ≈ 794..818)
    if let Some(c) = tagline_pixel(x, y, bg, pulse) {
        return c;
    }

    // 4. Progress Capsule Bar (y ≈ 838..844)
    if let Some(c) = progress_capsule_pixel(x, y, state) {
        return c;
    }

    // 5. System Status Phase Subtitle (y ≈ 862..886)
    if let Some(c) = status_subtitle_pixel(x, y, state, bg) {
        return c;
    }

    bg
}

// -----------------------------------------------------------------------------
// Element Renderers
// -----------------------------------------------------------------------------

/// Sovereign Emblem: Frosted Obsidian Monolith Tile (120x120)
fn emblem_pixel(x: usize, y: usize, state: &LauncherState) -> Option<u32> {
    if x < ICON_X0 || x >= ICON_X0 + ICON_SIZE || y < ICON_Y0 || y >= ICON_Y0 + ICON_SIZE {
        return None;
    }

    let bx = x - ICON_X0;
    let by = y - ICON_Y0;
    let r = ICON_R;

    // Corner mask
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

    // 1px Border with top specular highlight
    if distance_sq >= ((r - 2) * (r - 2)) as u32
        || bx == 0
        || bx == ICON_SIZE - 1
        || by == 0
        || by == ICON_SIZE - 1
    {
        if by <= 2 {
            return Some(COLOR_GLASS_HIGHLIGHT);
        }
        return Some(COLOR_GLASS_BORDER);
    }

    // Traveling diagonal specular light wave across the monolith
    let diag = bx as i32 + by as i32;
    let sweep_phase = ((state.boot_frame * 5) % 360) as i32 - 60;
    let sweep_dist = (diag - sweep_phase).abs();

    // Check 'I' Pill
    let mut in_letter = false;
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
        if (icx * icx + icy * icy) as u32 <= (I_R * I_R) as u32 {
            in_letter = true;
        }
    }

    // Check 'L' Glyph
    if !in_letter {
        // Stem upper part
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
            if (scx * scx + scy * scy) as u32 <= (L_R * L_R) as u32 {
                in_letter = true;
            }
        }
        // Foot and base
        else if bx >= L_FX0 && bx < L_FX1 && by >= L_FY0 && by < L_FY1 {
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
            if (fcx * fcx + fcy * fcy) as u32 <= (L_R * L_R) as u32 {
                in_letter = true;
            }
        }
    }

    // Monogram Pixels: Warm Ivory with radiant specular wave
    if in_letter {
        let letter_color = if sweep_dist < 24 {
            let t = ((24 - sweep_dist) * 255 / 24) as u32;
            mix(COLOR_IVORY, COLOR_WHITE, t)
        } else {
            COLOR_IVORY
        };
        return Some(letter_color);
    }

    // Monolith Glass Surface: subtle top-to-bottom depth gradient + specular reflection
    let t_y = (by * 256 / ICON_SIZE) as u32;
    let mut tile_bg = Theme::blend(COLOR_GLASS_TOP, COLOR_GLASS_BOTTOM, t_y);

    if sweep_dist < 32 {
        let s_alpha = ((32 - sweep_dist) * 95 / 32) as u32;
        tile_bg = Theme::blend(tile_bg, 0xFF354664, s_alpha);
    }

    Some(tile_bg)
}

/// Sovereign "InterOS" Brand
fn wordmark_pixel(x: usize, y: usize, state: &LauncherState, bg: u32) -> Option<u32> {
    let mark = wordmark_frame();
    let run = mark.run.as_ref()?;

    if x < mark.box_x || x - mark.box_x >= mark.box_w {
        return None;
    }
    if y < mark.box_y || y - mark.box_y >= mark.box_h {
        return None;
    }

    // Subtle specular traveling shimmer across the wordmark
    let shimmer_x = ((state.boot_frame * 6) % ((W + 160) as u32)) as i32 - 80;
    let dist = (x as i32 - shimmer_x).abs();
    let ink = if dist < 28 {
        let t = ((28 - dist) * 255 / 28) as u32;
        mix(COLOR_IVORY, COLOR_WHITE, t)
    } else {
        COLOR_IVORY
    };

    run.pixel(x, y, ink, bg)
}

/// Official Brand Tagline: "SMARTER • PRIVATE • OPEN"
fn tagline_pixel(x: usize, y: usize, bg: u32, pulse: u32) -> Option<u32> {
    if y < TAGLINE_Y.saturating_sub(8) || y >= TAGLINE_Y + 24 {
        return None;
    }

    let w1 = interos_std::design::typeface::text_advance_px(TAG1, 24) as usize;
    let w2 = interos_std::design::typeface::text_advance_px(TAG2, 24) as usize;
    let w3 = interos_std::design::typeface::text_advance_px(TAG3, 24) as usize;
    let total_tag_w = w1 + TAG_GAP + w2 + TAG_GAP + w3;
    let tag_x0 = (W.saturating_sub(total_tag_w)) / 2;

    let x_tag1 = tag_x0;
    let dot1_x = x_tag1 + w1 + TAG_GAP / 2;
    let x_tag2 = x_tag1 + w1 + TAG_GAP;
    let dot2_x = x_tag2 + w2 + TAG_GAP / 2;
    let x_tag3 = x_tag2 + w2 + TAG_GAP;

    let tag_color = Theme::blend(COLOR_TAGLINE, COLOR_WHITE, pulse / 6);

    // Render words
    if let Some(c) = crate::test_text_aa(x, y, x_tag1, TAGLINE_Y, 1, TAG1, tag_color, bg) {
        return Some(c);
    }
    if let Some(c) = crate::test_text_aa(x, y, x_tag2, TAGLINE_Y, 1, TAG2, tag_color, bg) {
        return Some(c);
    }
    if let Some(c) = crate::test_text_aa(x, y, x_tag3, TAGLINE_Y, 1, TAG3, tag_color, bg) {
        return Some(c);
    }

    // Render smooth circular bullet dots
    let dot_cy = TAGLINE_Y + 7;
    let dy = (y as i32 - dot_cy as i32).abs();
    for &dot_cx in &[dot1_x, dot2_x] {
        let dx = (x as i32 - dot_cx as i32).abs();
        let d2 = dx * dx + dy * dy;
        if d2 <= 2 {
            return Some(tag_color);
        } else if d2 <= 4 {
            return Some(Theme::blend(bg, tag_color, 128));
        }
    }

    None
}

/// Minimal Sovereign Capsule Progress Bar
fn progress_capsule_pixel(x: usize, y: usize, state: &LauncherState) -> Option<u32> {
    if y < BAR_Y0 || y >= BAR_Y0 + BAR_H {
        return None;
    }
    if x < BAR_X0 || x >= BAR_X0 + BAR_W {
        return None;
    }

    let cy = (BAR_Y0 + BAR_H / 2) as i32;
    let dy = y as i32 - cy;

    // Rounded capsule pill ends
    let dx = if x < BAR_X0 + BAR_R as usize {
        (BAR_X0 + BAR_R as usize) as i32 - x as i32
    } else if x > BAR_X0 + BAR_W - BAR_R as usize {
        x as i32 - (BAR_X0 + BAR_W - BAR_R as usize) as i32
    } else {
        0
    };

    let distance_sq = (dx * dx + dy * dy) as u32;
    if distance_sq > (BAR_R * BAR_R) as u32 {
        return None;
    }

    // Capsule border
    if distance_sq >= ((BAR_R - 1) * (BAR_R - 1)) as u32 {
        return Some(COLOR_TRACK_BORDER);
    }

    // Inner progress fill: 0..BAR_W - 4
    let fill_max = BAR_W - 4;
    let fill_w = (state.boot_progress as usize * fill_max) / 1000;
    let inner_x = x.saturating_sub(BAR_X0 + 2);

    // Dynamic scanning comet bead driven by boot_frame
    let pulse_head = ((state.boot_frame * 4) as usize) % fill_max;
    let p_dist = (inner_x as i32 - pulse_head as i32).abs();

    if inner_x < fill_w {
        // Vibrant electric cyan gradient along the fill
        let fill_t = (inner_x * 256 / fill_max.max(1)) as u32;
        let base_fill = Theme::blend(0xFF007ACC, COLOR_CYAN_GLOW, fill_t);

        // Leading edge glow bead
        let head_dist = (inner_x as i32 - fill_w as i32).abs();
        if head_dist <= 3 {
            let bead = mix(base_fill, COLOR_WHITE, ((3 - head_dist) * 80).min(255) as u32);
            return Some(bead);
        }
        if p_dist <= 6 {
            let shimmer = mix(base_fill, COLOR_CYAN_CORE, ((6 - p_dist) * 40).min(255) as u32);
            return Some(shimmer);
        }
        return Some(base_fill);
    }

    // Unfilled track with scanning comet trace
    if p_dist <= 5 {
        let trace = mix(COLOR_TRACK_BG, COLOR_CYAN_GLOW, ((5 - p_dist) * 35).min(255) as u32);
        return Some(trace);
    }

    Some(COLOR_TRACK_BG)
}

/// Dynamic System Status Phase text
fn status_subtitle_pixel(x: usize, y: usize, state: &LauncherState, bg: u32) -> Option<u32> {
    if y < STATUS_Y.saturating_sub(8) || y >= STATUS_Y + 24 {
        return None;
    }

    let phase_text: &[u8] = if state.boot_progress < 250 {
        b"STARTING SECURE MICROKERNEL"
    } else if state.boot_progress < 500 {
        b"VERIFYING SYSTEM INTEGRITY"
    } else if state.boot_progress < 750 {
        b"MOUNTING ENCRYPTED VFS"
    } else {
        b"SOVEREIGN PLATFORM READY"
    };

    let text_w = interos_std::design::typeface::text_advance_px(phase_text, 24) as usize;
    let x0 = (W.saturating_sub(text_w)) / 2;

    crate::test_text_aa(x, y, x0, STATUS_Y, 1, phase_text, COLOR_MUTED_TEXT, bg)
}

// -----------------------------------------------------------------------------
// Utilities
// -----------------------------------------------------------------------------

#[inline]
fn mix(a: u32, b: u32, t: u32) -> u32 {
    Theme::blend(a, b, t.min(256))
}

// -----------------------------------------------------------------------------
// Wordmark Memo
// -----------------------------------------------------------------------------

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

    #[test]
    fn the_surface_stays_backdrop_outside_the_stack() {
        let state = LauncherState::new();
        assert_eq!(get_boot_screen_color(0, 0, &state), COLOR_BG);
        assert_eq!(get_boot_screen_color(W - 1, H - 1, &state), COLOR_BG);
    }

    #[test]
    fn the_stack_stays_inside_the_rows_main_rs_repaints() {
        assert!(ICON_Y0 >= BAND_TOP);
        assert!(STATUS_Y + launcher::font::line_height(1) <= BAND_TOP + BAND_ROWS);
    }

    #[test]
    fn the_wordmark_is_rendered_from_frame_zero() {
        let mut state = LauncherState::new();
        state.boot_frame = 0;
        let frame0 = inked_in_wordmark(&state);
        state.boot_frame = 36;
        let frame36 = inked_in_wordmark(&state);
        assert!(frame0 > 0, "wordmark must be visible and inked at frame 0");
        assert!(frame36 > 0, "wordmark must remain visible at frame 36");
    }

    fn inked_in_wordmark(state: &LauncherState) -> usize {
        let mark = wordmark_frame();
        let mut ink = 0usize;
        for y in mark.box_y..mark.box_y + mark.box_h {
            for x in mark.box_x..mark.box_x + mark.box_w {
                if get_boot_screen_color(x, y, state) != COLOR_BG {
                    ink += 1;
                }
            }
        }
        ink
    }
}
