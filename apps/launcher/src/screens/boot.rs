//! Guest-owned startup presentation rendered into the native portrait surface.
//! Futuristic InterOS boot sequence inspired by the reference boot design:
//!   * Deep cosmic void background (0xFF050812)
//!   * Glowing celestial / planetary horizon arc in electric cyan & blue
//!   * Two-tone brand wordmark: "Inter" in pure white, "OS" in electric cyan
//!   * Dynamic system verification phase status ("SECURE BOOT ACTIVE", "MICROKERNEL RUNNING")
//!   * Futuristic glowing capsule progress pill with animated scanning pulse
//!   * System component verification badges ([OK] BOOT, [OK] KERNEL, [OK] CRYPTO, [OK] SYSTEM)
//!   * High-contrast, strictly `#![no_std]` integer math without floating point.

use core::cell::UnsafeCell;

use crate::{LauncherState, H, W};
use launcher::outline_text::{self, OutlineRun};
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

/// Horizon Arc apex:
const ARC_APEX_Y: i32 = (BAND_TOP + 26) as i32; // y = 616
const ARC_SPAN_HALF: i32 = 240;

/// Brand Wordmark:
const WORDMARK: &[u8] = b"Inter OS";
const WORDMARK_SCALE: usize = 3;
const WORDMARK_W: usize = 223;
const WORDMARK_X: usize = (W - WORDMARK_W) / 2;
const WORDMARK_TOP: usize = BAND_TOP + 64; // y = 654
const WORDMARK_BASELINE: usize = WORDMARK_TOP + 75; // y = 729
#[allow(dead_code)]
const WORDMARK_LINE_H: usize = 94;

/// Subtitle / Phase status line:
const STATUS_Y: usize = BAND_TOP + 152; // y = 742

/// Progress Capsule Bar:
const BAR_X0: usize = 180;
const BAR_W: usize = 360;
const BAR_Y0: usize = BAND_TOP + 176; // y = 766
const BAR_H: usize = 16;
const BAR_R: i32 = 8;

/// Verification Badges:
const BADGES_Y0: usize = BAND_TOP + 212; // y = 802
const BADGES_H: usize = 22;

/// Tagline:
const TAGLINE_Y: usize = BAND_TOP + 258; // y = 848
const TAGLINE: &[u8] = b"Secure * Fast * Open * For Everyone";

/// Ignition ticks for smooth reveal:
const IGNITE_TICKS: u32 = 36;

// -----------------------------------------------------------------------------
// Color Palettes
// -----------------------------------------------------------------------------
const COLOR_BG: u32 = 0xFF050812;
const COLOR_WHITE: u32 = 0xFFFFFFFF;
const COLOR_CYAN: u32 = 0xFF00E5FF;
const COLOR_CYAN_LIGHT: u32 = 0xFF38BDF8;
const COLOR_BLUE_DEEP: u32 = 0xFF1D4ED8;
const COLOR_BLUE_ELECTRIC: u32 = 0xFF2563EB;
const COLOR_SLATE_DARK: u32 = 0xFF090D1A;
const COLOR_SLATE_BORDER: u32 = 0xFF0284C7;
const COLOR_MUTED_TEXT: u32 = 0xFF64748B;

// -----------------------------------------------------------------------------
// The Main Boot Screen Painter
// -----------------------------------------------------------------------------

/// Computes the colour of one pixel `(x, y)` of the boot presentation.
pub fn get_boot_screen_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // Outside the active band: deep space cosmic void.
    if y < BAND_TOP || y >= BAND_TOP + BAND_ROWS {
        return COLOR_BG;
    }

    let lit = ignite(state.boot_frame);

    // 1. Planetary Horizon Arc / Eclipse curve (y ≈ 606..638)
    if let Some(c) = horizon_arc_pixel(x, y, lit) {
        return c;
    }

    // 2. Futuristic InterOS Wordmark (y ≈ 654..736)
    if let Some(c) = wordmark_pixel(x, y, lit) {
        return c;
    }

    // 3. System Status Phase Subtitle (y ≈ 742..752)
    if let Some(c) = status_subtitle_pixel(x, y, state) {
        return c;
    }

    // 4. Glowing Capsule Progress Bar (y ≈ 766..782)
    if let Some(c) = progress_capsule_pixel(x, y, state) {
        return c;
    }

    // 5. Verification Badges (y ≈ 802..824)
    if let Some(c) = verification_badges_pixel(x, y, state) {
        return c;
    }

    // 6. Tagline (y ≈ 848..858)
    if let Some(c) = tagline_pixel(x, y) {
        return c;
    }

    COLOR_BG
}

// -----------------------------------------------------------------------------
// Element Renderers
// -----------------------------------------------------------------------------

/// Planetary Horizon Arc: a brilliant curved celestial light beam
fn horizon_arc_pixel(x: usize, y: usize, lit: i32) -> Option<u32> {
    let dx = x as i32 - (W / 2) as i32;
    if dx.abs() > ARC_SPAN_HALF {
        return None;
    }
    // Parabolic gentle horizon curve
    let y_arc = ARC_APEX_Y + (dx * dx) / 1100;
    let dy = y as i32 - y_arc;
    let dy_abs = dy.abs();

    if dy_abs > 16 {
        return None;
    }

    let distance_sq = (dy_abs * dy_abs) as u32;

    // Intensity scaled by reveal curve
    let reach = (ARC_SPAN_HALF - dx.abs()) as u32 * lit as u32 / (ARC_SPAN_HALF as u32 * 1024);

    if dy_abs <= 2 {
        // Pure luminous white-cyan core
        let core = mix(COLOR_WHITE, COLOR_CYAN, dy_abs as u32 * 100);
        return Some(mix(COLOR_BG, core, reach * 256));
    } else if dy_abs <= 8 {
        // Electric cyan gradient
        let glow = mix(COLOR_CYAN, COLOR_BLUE_ELECTRIC, (dy_abs - 2) as u32 * 42);
        return Some(mix(COLOR_BG, glow, reach * 200));
    } else if distance_sq <= 256 {
        // Deep blue outer halo fading into void
        let fade = mix(COLOR_BLUE_DEEP, COLOR_BG, (dy_abs - 8) as u32 * 32);
        return Some(mix(COLOR_BG, fade, reach * 120));
    }

    None
}

/// Wordmark: "Inter" in pure white, "OS" in electric cyan
fn wordmark_pixel(x: usize, y: usize, lit: i32) -> Option<u32> {
    let mark = wordmark_frame();
    let run = mark.run.as_ref()?;

    if x < mark.box_x || x - mark.box_x >= mark.box_w {
        return None;
    }
    if y < mark.box_y || y - mark.box_y >= mark.box_h {
        return None;
    }

    // Reveal clip: opens outward from center
    let half = (mark.box_w as i32 + 1) / 2;
    if (x as i32 - (W / 2) as i32).abs() > half * lit / 1024 {
        return None;
    }

    // Determine target color: "Inter" is white (x < box_x + 138), "OS" is cyan
    let target_ink = if x < mark.box_x + 138 {
        COLOR_WHITE
    } else {
        COLOR_CYAN
    };

    run.pixel(x, y, target_ink, COLOR_BG)
}

/// Dynamic System Status Phase text
fn status_subtitle_pixel(x: usize, y: usize, state: &LauncherState) -> Option<u32> {
    if y < STATUS_Y || y >= STATUS_Y + 12 {
        return None;
    }

    let phase_text: &[u8] = if state.boot_progress < 250 {
        b"SECURE BOOT ACTIVE * VERIFIED"
    } else if state.boot_progress < 500 {
        b"INTEGRITY CHECKS PASSED * OK"
    } else if state.boot_progress < 750 {
        b"MICROKERNEL RUNNING * PHASE 4"
    } else {
        b"SYSTEM SERVICES ONLINE * READY"
    };

    let text_w = launcher::text::measure(phase_text, 1);
    let x0 = (W.saturating_sub(text_w)) / 2;

    crate::test_text(x, y, x0, STATUS_Y, 1, phase_text, COLOR_CYAN_LIGHT)
}

/// Futuristic Glowing Capsule Progress Bar
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

    // Border of the capsule
    if distance_sq >= ((BAR_R - 2) * (BAR_R - 2)) as u32 {
        return Some(COLOR_SLATE_BORDER);
    }

    // Inner progress fill: 0..BAR_W - 8
    let fill_max = BAR_W - 8;
    let fill_w = (state.boot_progress as usize * fill_max) / 1000;
    let inner_x = x.saturating_sub(BAR_X0 + 4);

    // Dynamic scanning comet bead driven by boot_frame
    let pulse_head = ((state.boot_frame * 3) as usize) % fill_max;
    let p_dist = (inner_x as i32 - pulse_head as i32).abs();

    if inner_x < fill_w {
        // Glowing cyan-to-blue gradient fill
        let t = (inner_x * 256) / fill_max.max(1);
        let base_fill = mix(COLOR_CYAN, COLOR_BLUE_ELECTRIC, t as u32);

        // Leading edge glow bead
        let head_dist = (inner_x as i32 - fill_w as i32).abs();
        if head_dist <= 6 {
            return Some(mix(base_fill, COLOR_WHITE, (6 - head_dist) as u32 * 42));
        }
        if p_dist <= 8 {
            return Some(mix(base_fill, COLOR_WHITE, (8 - p_dist) as u32 * 30));
        }
        return Some(base_fill);
    }

    // Unfilled track: dark glass
    if p_dist <= 6 {
        return Some(mix(COLOR_SLATE_DARK, COLOR_CYAN_LIGHT, (6 - p_dist) as u32 * 35));
    }

    Some(COLOR_SLATE_DARK)
}

/// Verification Badges: 4 pills across the screen ([OK] BOOT, [OK] KERNEL, [OK] CRYPTO, [OK] SYSTEM)
fn verification_badges_pixel(x: usize, y: usize, state: &LauncherState) -> Option<u32> {
    if y < BADGES_Y0 || y >= BADGES_Y0 + BADGES_H {
        return None;
    }

    const BADGE_W: usize = 110;
    const BADGE_GAP: usize = 18;
    const TOTAL_W: usize = 4 * BADGE_W + 3 * BADGE_GAP; // 440 + 54 = 494
    const START_X: usize = (W - TOTAL_W) / 2; // (720 - 494)/2 = 113

    let labels: [&[u8]; 4] = [
        b"[OK] BOOT",
        b"[OK] KERNEL",
        b"[OK] CRYPTO",
        b"[OK] SYSTEM",
    ];

    for (i, &label) in labels.iter().enumerate() {
        let bx0 = START_X + i * (BADGE_W + BADGE_GAP);
        let bx1 = bx0 + BADGE_W;
        if x >= bx0 && x < bx1 {
            // Is this badge activated?
            let is_active = state.boot_progress >= (i as u16 + 1) * 220;
            let text_color = if is_active { COLOR_CYAN } else { COLOR_MUTED_TEXT };
            let border_color = if is_active { COLOR_SLATE_BORDER } else { 0xFF1E293B };

            // Badge border
            let is_border = x == bx0 || x == bx1 - 1 || y == BADGES_Y0 || y == BADGES_Y0 + BADGES_H - 1;
            if is_border {
                return Some(border_color);
            }

            // Badge text centered inside badge
            let text_w = launcher::text::measure(label, 1);
            let tx0 = bx0 + (BADGE_W.saturating_sub(text_w)) / 2;
            let ty0 = BADGES_Y0 + 7;

            if let Some(c) = crate::test_text(x, y, tx0, ty0, 1, label, text_color) {
                return Some(c);
            }

            return Some(if is_active { 0xFF0B132B } else { COLOR_SLATE_DARK });
        }
    }

    None
}

/// Tagline text: "Secure * Fast * Open * For Everyone"
fn tagline_pixel(x: usize, y: usize) -> Option<u32> {
    if y < TAGLINE_Y || y >= TAGLINE_Y + 12 {
        return None;
    }
    let text_w = launcher::text::measure(TAGLINE, 1);
    let x0 = (W.saturating_sub(text_w)) / 2;
    crate::test_text(x, y, x0, TAGLINE_Y, 1, TAGLINE, COLOR_MUTED_TEXT)
}

// -----------------------------------------------------------------------------
// Utilities
// -----------------------------------------------------------------------------

#[inline]
fn ignite(frame: u32) -> i32 {
    let linear = frame.min(IGNITE_TICKS) as i32 * 1024 / IGNITE_TICKS as i32;
    let rest = 1024 - linear;
    1024 - rest * rest / 1024
}

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
        let em = outline_text::em_for_scale(WORDMARK_SCALE);
        if let Some(run) = OutlineRun::place(WORDMARK_X, WORDMARK_BASELINE, em, WORDMARK) {
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
        assert!(BAR_Y0 >= BAND_TOP);
        assert!(TAGLINE_Y + 12 <= BAND_TOP + BAND_ROWS);
    }

    #[test]
    fn the_wordmark_reveals_once_and_then_holds() {
        let mut state = LauncherState::new();
        state.boot_frame = 0;
        let hidden = inked_in_wordmark(&state);
        state.boot_frame = IGNITE_TICKS;
        let shown = inked_in_wordmark(&state);
        state.boot_frame = IGNITE_TICKS * 2;
        let later = inked_in_wordmark(&state);
        assert_eq!(hidden, 0);
        assert!(shown > 0);
        assert_eq!(shown, later);
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
