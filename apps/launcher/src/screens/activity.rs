//! Activity Space: the multitasking overview deck.
//!
//! WHY `pub`: `render_view` in the crate root dispatches here by view
//! mode. Geometry and colour come from `crate::theme` and
//! `crate::layout`, never from literals local to this file.

#![allow(unused_imports)]

use crate::{test_text, LauncherState};
// Theme tokens live in the presentation library, not the shell binary.
use launcher::theme::*;

pub fn get_activity_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 {
            return BORDER_SUBTLE;
        }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, &state.clock_time, TEXT_PRIMARY) {
                return c;
            }
        }
        if x >= 590 && x <= 608 && y >= 16 && y <= 32 {
            if x <= 593 && y >= 28 {
                return TEXT_PRIMARY;
            }
            if x >= 595 && x <= 598 && y >= 24 {
                return TEXT_PRIMARY;
            }
            if x >= 600 && x <= 603 && y >= 20 {
                return TEXT_PRIMARY;
            }
            if x >= 605 && x <= 608 && y >= 16 {
                return TEXT_PRIMARY;
            }
        }
        if x >= 626 && x <= 644 && y >= 18 && y <= 32 {
            let dx = (x as i32) - 635;
            let dy = (y as i32) - 30;
            let d2 = dx * dx + dy * dy;
            if (d2 >= 100 && d2 <= 144 && dy <= -2) || (d2 >= 36 && d2 <= 64 && dy <= -1) || d2 <= 9
            {
                return TEXT_PRIMARY;
            }
        }
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 {
                return TEXT_PRIMARY;
            }
            if x >= 685 && x <= 686 && y >= 22 && y <= 28 {
                return TEXT_PRIMARY;
            }
            if x >= 661 && x <= 680 && y >= 21 && y <= 29 {
                return SUCCESS_EMERALD;
            }
        }
        return BG_DARK;
    }

    // 2. Header (y: 48..120)
    if y >= 48 && y < 120 {
        if y >= 64 && y < 88 {
            if let Some(c) = test_text(x, y, 48, 64, 3, b"Activity Space", TEXT_PRIMARY) {
                return c;
            }
        }
        // Search icon circle at (644, 76, r: 18)
        let dx = (x as i32) - 644;
        let dy = (y as i32) - 76;
        let d = dx * dx + dy * dy;
        if (d >= 36 && d <= 64) || (dx >= 6 && dx <= 10 && dx == dy) {
            return TEXT_MUTED;
        }
        if y >= 116 {
            return BORDER_SUBTLE;
        }
        return BG_DARK;
    }

    // 3. Recent applications from the live runtime registry.
    let cards = [(135usize, 340usize, 0usize), (360, 565, 1), (585, 790, 2), (810, 1015, 0)];
    for (y0, y1, recent_idx) in cards {
        if y >= y0 && y < y1 && x >= 48 && x < 672 {
            if y == y0 || y == y1 - 1 || x == 48 || x == 671 { return BORDER_SUBTLE; }
            let accent = match recent_idx {
                0 => ACCENT_SKY,
                1 => SUCCESS_EMERALD,
                2 => ACCENT_AMBER,
                _ => TEXT_MUTED,
            };
            if x < 56 { return accent; }
            if y >= y0 + 18 && y < y0 + 34 {
                if let Some(label) = state.recent_label(recent_idx) {
                    if let Some(c) = test_text(x, y, 76, y0 + 18, 2, label, TEXT_PRIMARY) { return c; }
                } else if let Some(c) = test_text(x, y, 76, y0 + 18, 2, b"No recent app", TEXT_MUTED) {
                    return c;
                }
            }
            if y >= y0 + 48 && y < y0 + 64 {
                let subtitle = if state.recent_task_ids[recent_idx] > 0 {
                    b"Running task" as &[u8]
                } else {
                    b"Available in registry" as &[u8]
                };
                if let Some(c) = test_text(x, y, 76, y0 + 48, 1, subtitle, TEXT_MUTED) { return c; }
            }
            return SURFACE_RAISED;
        }
    }

    // 4. "+ New Activity" Button (x: 200..520, y: 1045..1115)
    if y >= 1045 && y < 1115 && x >= 200 && x < 520 {
        if y == 1045 || y == 1114 || x == 200 || x == 519 {
            return ACCENT_SKY;
        }
        if y >= 1072 && y < 1088 {
            if let Some(c) = test_text(x, y, 260, 1072, 2, b"+ New Activity", TEXT_PRIMARY) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // 5. Bottom Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}
