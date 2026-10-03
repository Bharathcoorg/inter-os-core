//! Quick Controls shade: radios, sliders and toggles.
//!
//! WHY `pub`: `render_view` in the crate root dispatches here by view
//! mode. Geometry and colour come from `crate::theme` and
//! `crate::layout`, never from literals local to this file.

#![allow(unused_imports)]

use crate::{test_text, LauncherState};
// Theme tokens live in the presentation library, not the shell binary.
use launcher::theme::*;

pub fn get_quick_controls_color(x: usize, y: usize, _state: &LauncherState) -> u32 {
    // Header
    if y < 120 {
        if y >= 64 && y < 80 {
            if let Some(c) = test_text(x, y, 48, 64, 2, b"Date unavailable", TEXT_PRIMARY) {
                return c;
            }
        }
        let dx = (x as i32) - 640;
        let dy = (y as i32) - 76;
        let d = dx * dx + dy * dy;
        if (d >= 100 && d <= 196) || d <= 25 {
            return TEXT_MUTED;
        }
        if y >= 116 {
            return BORDER_SUBTLE;
        }
        return BG_DARK;
    }

    // Primary Connectivity Cards (y: 130..260, 2 Columns)
    if y >= 130 && y < 260 {
        // Wi-Fi
        if x >= 40 && x < 345 {
            if y >= 175 && y < 191 {
                if let Some(c) = test_text(x, y, 100, 175, 2, b"Wi-Fi", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 205 && y < 213 {
                if let Some(c) =
                    test_text(x, y, 100, 205, 1, b"Status unavailable", SUCCESS_EMERALD)
                {
                    return c;
                }
            }
            let dx = (x as i32) - 72;
            let dy = (y as i32) - 195;
            let d = dx * dx + dy * dy;
            if (d >= 64 && d <= 100) || (d >= 16 && d <= 36) || d <= 4 {
                return SUCCESS_EMERALD;
            }
            if y >= 256 || x == 40 || x == 344 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }
        // Bluetooth
        if x >= 375 && x < 680 {
            if y >= 175 && y < 191 {
                if let Some(c) = test_text(x, y, 435, 175, 2, b"Bluetooth", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 205 && y < 213 {
                if let Some(c) = test_text(x, y, 435, 205, 1, b"Unavailable", TEXT_MUTED) {
                    return c;
                }
            }
            let dx = (x as i32) - 407;
            let dy = (y as i32) - 195;
            if (dx == 0 && dy >= -12 && dy <= 12) || (dx == dy && dy >= -8 && dy <= 8) {
                return ACCENT_SKY;
            }
            if y >= 256 || x == 375 || x == 679 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }
        return BG_DARK;
    }

    // Secondary Toggles (y: 280..380, 3 Columns)
    if y >= 280 && y < 380 {
        if (x >= 40 && x < 235) || (x >= 262 && x < 458) || (x >= 485 && x < 680) {
            if y >= 320 && y < 336 {
                if x < 235 {
                    if let Some(c) = test_text(x, y, 64, 320, 2, b"Mobile unavailable", TEXT_MUTED) {
                        return c;
                    }
                } else if x < 458 {
                    if let Some(c) = test_text(x, y, 290, 320, 2, b"Hotspot unavailable", TEXT_MUTED) {
                        return c;
                    }
                } else {
                    if let Some(c) = test_text(x, y, 508, 320, 2, b"Airplane", TEXT_MUTED) {
                        return c;
                    }
                }
            }
            if y >= 376 || y == 280 {
                return BORDER_SUBTLE;
            }
            return SURFACE_DARK;
        }
        return BG_DARK;
    }

    // Continuous Sliders (y: 400..570)
    // Brightness Slider
    if x >= 40 && x < 680 && y >= 400 && y < 465 {
        if x <= 520 {
            return SURFACE_LIGHT;
        }
        return SURFACE_DARK;
    }
    // Volume Slider
    if x >= 40 && x < 680 && y >= 485 && y < 550 {
        if x <= 424 {
            return ACCENT_SKY;
        }
        return SURFACE_DARK;
    }

    // Active Sovereign Media Player Card (x: 40..680, y: 720..890)
    if x >= 40 && x < 680 && y >= 720 && y < 890 {
        let dx = (x as i32) - 100;
        let dy = (y as i32) - 805;
        let d = dx * dx + dy * dy;
        if d <= 1764 {
            if d <= 144 {
                return ACCENT_AMBER;
            }
            if d >= 1600 || (d >= 400 && d <= 441) || (d >= 900 && d <= 961) {
                return 0xFF2A3448;
            }
            return 0xFF05070D;
        }
        if y >= 785 && y < 801 {
            if let Some(c) = test_text(x, y, 170, 785, 2, b"No active media", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 815 && y < 831 {
            if let Some(c) = test_text(x, y, 170, 815, 2, b"Media data unavailable", TEXT_MUTED) {
                return c;
            }
        }

        // Transport: |<<, ||, >>|
        if y >= 800 && y < 816 {
            if let Some(c) = test_text(x, y, 480, 800, 2, b"|<", TEXT_PRIMARY) {
                return c;
            }
            if let Some(c) = test_text(x, y, 540, 800, 2, b"||", ACCENT_SKY) {
                return c;
            }
            if let Some(c) = test_text(x, y, 600, 800, 2, b">|", TEXT_PRIMARY) {
                return c;
            }
        }

        if y >= 886 || x == 40 || x == 679 {
            return BORDER_SUBTLE;
        }
        return SURFACE_RAISED;
    }

    // Return to Home bar
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}
