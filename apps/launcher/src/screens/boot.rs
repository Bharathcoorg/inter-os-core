//! Guest-owned startup presentation rendered into the native portrait surface.
//!
//! Canonical InterOS boot sequence inspired by the sovereign UI reference design:
//!   * Deep sovereign slate background (0xFF111417)
//!   * Sovereign Emblem: Two-tone squircle tile:
//!       - Left half: Sage Green / Mineral (0xFF546B58)
//!       - Right half: Dark Charcoal / Slate (0xFF202326)
//!       - Centered "IL" Monogram in Warm Ivory (0xFFFAF8F4) with traveling light shimmer
//!   * Sovereign Brand: "InterOS" in crisp Warm Ivory scalable vector outline typeface
//!   * Brand Tagline: "SMARTER • PRIVATE • OPEN" in luminous sage with antialiased typography
//!   * Minimal progress capsule with honest hardware/software verification status
//!   * High-contrast, strictly `#![no_std]` integer math without floating point.

use core::cell::UnsafeCell;

use crate::{LauncherState, H, W};
use launcher::outline_text::OutlineRun;
use launcher::theme::Theme;

/// Theme used for startup presentation.
#[allow(dead_code)]
const THEME: Theme = Theme::DARK;

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
const ICON_R: i32 = 22;

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
const BAR_H: usize = 8;
const BAR_X0: usize = (W - BAR_W) / 2; // (720 - 240) / 2 = 240
const BAR_Y0: usize = 836;
const BAR_R: i32 = 4;

/// System Status Phase subtitle:
const STATUS_Y: usize = 862;

// -----------------------------------------------------------------------------
// Color Palettes
// -----------------------------------------------------------------------------
const COLOR_BG: u32 = 0xFF111417;          // Sovereign Deep Slate
const COLOR_IVORY: u32 = 0xFFFAF8F4;       // Warm Ivory (Letters & Wordmark)
const COLOR_WHITE: u32 = 0xFFFFFFFF;       // Shimmer Peak
const COLOR_SAGE: u32 = 0xFF546B58;        // Sage Green / Mineral (Left Tile Half)
const COLOR_SAGE_LIGHT: u32 = 0xFF7D8F80;  // Tagline Sage
const COLOR_SAGE_BRIGHT: u32 = 0xFF88A88E; // Progress Fill Leading Bead
const COLOR_CHARCOAL: u32 = 0xFF202326;    // Dark Charcoal / Slate (Right Tile Half)
const COLOR_BORDER: u32 = 0xFF3E4744;      // Hairline Border
const COLOR_TRACK_BG: u32 = 0xFF1A1D20;    // Progress Bar Track
const COLOR_TRACK_BORDER: u32 = 0xFF2D3338;// Progress Bar Track Border
const COLOR_MUTED_TEXT: u32 = 0xFF6C7C70;  // Phase Status Text

// -----------------------------------------------------------------------------
// The Main Boot Screen Painter
// -----------------------------------------------------------------------------

/// Computes the colour of one pixel `(x, y)` of the boot presentation.
pub fn get_boot_screen_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // Outside the active band: sovereign deep slate void.
    if y < BAND_TOP || y >= BAND_TOP + BAND_ROWS {
        return COLOR_BG;
    }

    // 1. Sovereign Emblem: Split Monogram Tile (y = 598..718)
    if let Some(c) = emblem_pixel(x, y, state) {
        return c;
    }

    // 2. Sovereign "InterOS" Brand (y ≈ 725..775)
    if let Some(c) = wordmark_pixel(x, y, state) {
        return c;
    }

    // 3. Official Brand Tagline (y ≈ 794..818)
    if let Some(c) = tagline_pixel(x, y) {
        return c;
    }

    // 4. Progress Capsule Bar (y ≈ 836..844)
    if let Some(c) = progress_capsule_pixel(x, y, state) {
        return c;
    }

    // 5. System Status Phase Subtitle (y ≈ 862..886)
    if let Some(c) = status_subtitle_pixel(x, y, state) {
        return c;
    }

    COLOR_BG
}

// -----------------------------------------------------------------------------
// Element Renderers
// -----------------------------------------------------------------------------

/// Sovereign Emblem: Split Monogram Tile (120x120)
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

    // 1px Border
    if distance_sq >= ((r - 2) * (r - 2)) as u32
        || bx == 0
        || bx == ICON_SIZE - 1
        || by == 0
        || by == ICON_SIZE - 1
    {
        return Some(COLOR_BORDER);
    }

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

    // Monogram Pixels
    if in_letter {
        // Traveling light shimmer pulse
        let shimmer_bx = ((state.boot_frame * 4) % 200) as i32 - 40;
        let dist = (bx as i32 - shimmer_bx).abs();
        let letter_color = if dist < 18 {
            let t = (18 - dist) as u32 * 14;
            mix(COLOR_IVORY, COLOR_WHITE, t)
        } else {
            COLOR_IVORY
        };
        return Some(letter_color);
    }

    // Container Background: Split at bx < 60
    let tile_bg = if bx < 60 {
        COLOR_SAGE
    } else {
        COLOR_CHARCOAL
    };

    Some(tile_bg)
}

/// Sovereign "InterOS" Brand
fn wordmark_pixel(x: usize, y: usize, state: &LauncherState) -> Option<u32> {
    let mark = wordmark_frame();
    let run = mark.run.as_ref()?;

    if x < mark.box_x || x - mark.box_x >= mark.box_w {
        return None;
    }
    if y < mark.box_y || y - mark.box_y >= mark.box_h {
        return None;
    }

    // Subtle specular traveling shimmer across the wordmark
    let shimmer_x = ((state.boot_frame * 5) % (W as u32 + 100)) as i32 - 50;
    let dist = (x as i32 - shimmer_x).abs();
    let ink = if dist < 24 {
        let t = (24 - dist) as u32 * 10;
        mix(COLOR_IVORY, COLOR_WHITE, t)
    } else {
        COLOR_IVORY
    };

    run.pixel(x, y, ink, COLOR_BG)
}

/// Official Brand Tagline: "SMARTER • PRIVATE • OPEN"
fn tagline_pixel(x: usize, y: usize) -> Option<u32> {
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

    // Render words
    if let Some(c) = crate::test_text_aa(x, y, x_tag1, TAGLINE_Y, 1, TAG1, COLOR_SAGE_LIGHT, COLOR_BG) {
        return Some(c);
    }
    if let Some(c) = crate::test_text_aa(x, y, x_tag2, TAGLINE_Y, 1, TAG2, COLOR_SAGE_LIGHT, COLOR_BG) {
        return Some(c);
    }
    if let Some(c) = crate::test_text_aa(x, y, x_tag3, TAGLINE_Y, 1, TAG3, COLOR_SAGE_LIGHT, COLOR_BG) {
        return Some(c);
    }

    // Render smooth circular bullet dots
    let dot_cy = TAGLINE_Y + 7;
    let dy = (y as i32 - dot_cy as i32).abs();
    for &dot_cx in &[dot1_x, dot2_x] {
        let dx = (x as i32 - dot_cx as i32).abs();
        let d2 = dx * dx + dy * dy;
        if d2 <= 2 {
            return Some(COLOR_SAGE_LIGHT);
        } else if d2 <= 4 {
            return Some(Theme::blend(COLOR_BG, COLOR_SAGE_LIGHT, 128));
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
    let pulse_head = ((state.boot_frame * 3) as usize) % fill_max;
    let p_dist = (inner_x as i32 - pulse_head as i32).abs();

    if inner_x < fill_w {
        // Glowing Sage fill
        let base_fill = COLOR_SAGE;

        // Leading edge glow bead
        let head_dist = (inner_x as i32 - fill_w as i32).abs();
        if head_dist <= 3 {
            let bead = mix(base_fill, COLOR_SAGE_BRIGHT, ((3 - head_dist) * 80).min(255) as u32);
            return Some(bead);
        }
        if p_dist <= 6 {
            let shimmer = mix(base_fill, COLOR_WHITE, ((6 - p_dist) * 40).min(255) as u32);
            return Some(shimmer);
        }
        return Some(base_fill);
    }

    // Unfilled track
    if p_dist <= 4 {
        let trace = mix(COLOR_TRACK_BG, COLOR_SAGE_LIGHT, ((4 - p_dist) * 50).min(255) as u32);
        return Some(trace);
    }

    Some(COLOR_TRACK_BG)
}

/// Dynamic System Status Phase text
fn status_subtitle_pixel(x: usize, y: usize, state: &LauncherState) -> Option<u32> {
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

    crate::test_text_aa(x, y, x0, STATUS_Y, 1, phase_text, COLOR_MUTED_TEXT, COLOR_BG)
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
        assert!(STATUS_Y + launcher::font::GLYPH_H <= BAND_TOP + BAND_ROWS);
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
