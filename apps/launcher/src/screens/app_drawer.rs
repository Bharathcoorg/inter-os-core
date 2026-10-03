//! App Drawer: the full catalogue grid.
//!
//! WHY `pub`: `render_view` in the crate root dispatches here by view
//! mode. Geometry and colour come from `crate::theme` and
//! `crate::layout`, never from literals local to this file.

#![allow(unused_imports)]

use crate::{test_text, LauncherState};
// Theme tokens live in the presentation library, not the shell binary.
use launcher::theme::*;

pub fn get_app_drawer_color(x: usize, y: usize, state: &LauncherState) -> u32 {
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

    // 2. Header & Search Command Bar (y: 50..105, x: 40..680)
    if y >= 50 && y < 105 && x >= 40 && x < 680 {
        if y == 50 || y == 104 || x == 40 || x == 679 {
            return BORDER_SUBTLE;
        }
        // Search icon
        let dx_s = (x as i32) - 68;
        let dy_s = (y as i32) - 77;
        let d_s = dx_s * dx_s + dy_s * dy_s;
        if (d_s >= 25 && d_s <= 49) || (dx_s >= 5 && dx_s <= 8 && dx_s == dy_s) {
            return TEXT_MUTED;
        }
        if y >= 70 && y < 86 {
            if let Some(c) = test_text(x, y, 96, 70, 2, b"Search apps...", TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // 3. Installed application filter.
    if y >= 118 && y < 162 && x >= 40 && x < 680 {
        if y == 118 || y == 161 || x == 40 || x == 679 {
            return ACCENT_SKY;
        }
        if y >= 134 && y < 150 {
            if let Some(c) = test_text(x, y, 300, 134, 2, b"Installed", TEXT_DARK) {
                return c;
            }
        }
        return SURFACE_LIGHT;
    }

    // 4. Real application registry grid.
    let row = if (185..395).contains(&y) {
        Some(0)
    } else if (410..620).contains(&y) {
        Some(1)
    } else if (635..845).contains(&y) {
        Some(2)
    } else if (860..1070).contains(&y) {
        Some(3)
    } else {
        None
    };

    if let Some(r_idx) = row {
        let col = if (50..230).contains(&x) {
            Some(0)
        } else if (270..450).contains(&x) {
            Some(1)
        } else if (490..670).contains(&x) {
            Some(2)
        } else {
            None
        };

        if let Some(c_idx) = col {
            let app_idx = state.app_drawer_page * 12 + r_idx * 3 + c_idx;
            let x0 = match c_idx { 0 => 50, 1 => 270, _ => 490 };
            let y0 = match r_idx { 0 => 185, 1 => 410, 2 => 635, _ => 860 };
            if y == y0 || y == y0 + 209 || x == x0 || x == x0 + 179 {
                return BORDER_SUBTLE;
            }
            if app_idx >= state.app_count {
                return SURFACE_RAISED;
            }
            let accent = match app_idx % 6 {
                0 => ACCENT_SKY,
                1 => SUCCESS_EMERALD,
                2 => ACCENT_AMBER,
                3 => ACCENT_VIOLET,
                4 => ACCENT_CYAN,
                _ => TEXT_MUTED,
            };
            let b_dx = (x as i32) - (x0 as i32 + 90);
            let b_dy = (y as i32) - (y0 as i32 + 80);
            let b_d2 = b_dx * b_dx + b_dy * b_dy;
            if b_d2 <= 1600 {
                if b_d2 >= 1369 { return accent; }
                if y >= y0 + 72 && y < y0 + 80 {
                    let icon = &state.app_icons[app_idx][..3];
                    if let Some(c) = test_text(x, y, x0 + 78, y0 + 72, 1, icon, TEXT_DARK) {
                        return c;
                    }
                }
                return SURFACE_RAISED;
            }
            if y >= y0 + 150 && y < y0 + 166 {
                let len = state.app_label_lens[app_idx] as usize;
                if len > 0 {
                    let label = &state.app_labels[app_idx][..len];
                    let ox = x0 + 90 - (len * 16) / 2;
                    if let Some(c) = test_text(x, y, ox, y0 + 150, 2, label, TEXT_PRIMARY) {
                        return c;
                    }
                }
            }
            return SURFACE_RAISED;
        }
    }

    // 5. Registry-backed pagination dots (up to three pages).
    if y >= 1144 && y <= 1152 {
        let pages = ((state.app_count + 11) / 12).max(1).min(3);
        for dot in 0..pages {
            let center = 336 + dot * 24;
            let dx = (x as i32) - center as i32;
            let dy = (y as i32) - 1148;
            if dx * dx + dy * dy <= if dot == state.app_drawer_page { 16 } else { 9 } {
                return if dot == state.app_drawer_page { TEXT_PRIMARY } else { TEXT_MUTED };
            }
        }
    }

    // 6. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}
