//! Live Widgets dashboard.
//!
//! WHY `pub`: `render_view` in the crate root dispatches here by view
//! mode. Geometry and colour come from `crate::theme` and
//! `crate::layout`, never from literals local to this file.

#![allow(unused_imports)]

use crate::{test_text, LauncherState};
// Theme tokens live in the presentation library, not the shell binary.
use launcher::theme::*;

pub fn get_live_widgets_color(x: usize, y: usize, _state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 {
            return BORDER_SUBTLE;
        }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, b"--:--", TEXT_PRIMARY) {
                return c;
            }
        }
        // Cellular
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
        // Wi-Fi
        if x >= 626 && x <= 644 && y >= 18 && y <= 32 {
            let dx = (x as i32) - 635;
            let dy = (y as i32) - 30;
            let d2 = dx * dx + dy * dy;
            if (d2 >= 100 && d2 <= 144 && dy <= -2) || (d2 >= 36 && d2 <= 64 && dy <= -1) || d2 <= 9
            {
                return TEXT_PRIMARY;
            }
        }
        // Battery
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 {
                return TEXT_PRIMARY;
            }
            if x >= 685 && x <= 686 && y >= 22 && y <= 28 {
                return TEXT_PRIMARY;
            }
            // Synthetic battery fill removed; hardware telemetry must supply the level.
        }
        return BG_DARK;
    }

    // 2. Header (y: 50..125)
    if y >= 50 && y < 125 {
        if y >= 64 && y < 88 {
            if let Some(c) = test_text(x, y, 48, 64, 3, b"Live Widgets", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 96 && y < 104 {
            if let Some(c) = test_text(
                x,
                y,
                48,
                96,
                1,
                b"Real-time glanceable telemetry & stats",
                TEXT_MUTED,
            ) {
                return c;
            }
        }
        return BG_DARK;
    }

    // 3. 2-Column Top Widgets (y: 135..315)
    // Left: Weather (x: 40..345)
    if y >= 135 && y < 315 && x >= 40 && x < 345 {
        if y == 135 || y == 314 || x == 40 || x == 344 {
            return BORDER_SUBTLE;
        }
        if y >= 155 && y < 171 {
            if let Some(c) = test_text(x, y, 65, 155, 2, b"Location unavailable", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 190 && y < 214 {
            if let Some(c) = test_text(x, y, 65, 190, 3, b"--", ACCENT_AMBER) {
                return c;
            }
        }
        let s_dx = (x as i32) - 300;
        let s_dy = (y as i32) - 180;
        let s_d2 = s_dx * s_dx + s_dy * s_dy;
        if s_d2 <= 324 {
            if s_d2 <= 144 {
                return ACCENT_AMBER;
            }
            if (s_dx == 0 || s_dy == 0 || s_dx == s_dy || s_dx == -s_dy) && s_d2 >= 196 {
                return ACCENT_AMBER;
            }
        }
        if y >= 250 && y < 258 {
            if let Some(c) = test_text(x, y, 65, 250, 1, b"Weather data unavailable", TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // Right: Build Progress (x: 375..680)
    if y >= 135 && y < 315 && x >= 375 && x < 680 {
        if y == 135 || y == 314 || x == 375 || x == 679 {
            return BORDER_SUBTLE;
        }
        if y >= 155 && y < 171 {
            if let Some(c) = test_text(x, y, 400, 155, 2, b"Build Status", TEXT_PRIMARY) {
                return c;
            }
            if let Some(c) = test_text(x, y, 610, 155, 2, b"--", ACCENT_SKY) {
                return c;
            }
        }
        if y >= 200 && y < 216 && x >= 400 && x < 655 {
            if x <= 609 {
                return ACCENT_SKY;
            }
            return 0xFF0E1A2C;
        }
        if y >= 250 && y < 258 {
            if let Some(c) = test_text(x, y, 400, 250, 1, b"Build progress unavailable", TEXT_MUTED)
            {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // 4. Now Playing Mini Player (x: 40..680, y: 335..485)
    if y >= 335 && y < 485 && x >= 40 && x < 680 {
        if y == 335 || y == 484 || x == 40 || x == 679 {
            return BORDER_SUBTLE;
        }
        let v_dx = (x as i32) - 95;
        let v_dy = (y as i32) - 410;
        let v_d2 = v_dx * v_dx + v_dy * v_dy;
        if v_d2 <= 1225 {
            if v_d2 <= 100 {
                return ACCENT_AMBER;
            }
            if v_d2 >= 1100 || (v_d2 >= 300 && v_d2 <= 340) || (v_d2 >= 650 && v_d2 <= 700) {
                return 0xFF2A3448;
            }
            return 0xFF05070D;
        }
        if y >= 380 && y < 396 {
            if let Some(c) = test_text(x, y, 155, 380, 2, b"No active media", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 406 && y < 414 {
            if let Some(c) = test_text(x, y, 155, 406, 1, b"Media data unavailable", TEXT_MUTED) {
                return c;
            }
        }
        if y >= 430 && y < 436 && x >= 155 && x < 450 {
            if x <= 265 {
                return ACCENT_SKY;
            }
            return 0xFF142032;
        }
        if y >= 445 && y < 453 {
            if let Some(c) = test_text(x, y, 155, 445, 1, b"--:-- / --:--", TEXT_MUTED) {
                return c;
            }
        }
        if y >= 400 && y < 416 {
            if let Some(c) = test_text(x, y, 500, 400, 2, b"|<", TEXT_PRIMARY) {
                return c;
            }
            if let Some(c) = test_text(x, y, 555, 400, 2, b"||", ACCENT_SKY) {
                return c;
            }
            if let Some(c) = test_text(x, y, 610, 400, 2, b">|", TEXT_PRIMARY) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // 5. 2-Column Bottom Widgets (y: 505..685)
    // Left: Meeting Widget (x: 40..345)
    if y >= 505 && y < 685 && x >= 40 && x < 345 {
        if y == 505 || y == 684 || x == 40 || x == 344 {
            return BORDER_SUBTLE;
        }
        if y >= 525 && y < 541 {
            if let Some(c) = test_text(x, y, 65, 525, 2, b"Next Meeting", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 560 && y < 576 {
            if let Some(c) = test_text(x, y, 65, 560, 2, b"--:--", ACCENT_SKY) {
                return c;
            }
        }
        if y >= 595 && y < 603 {
            if let Some(c) = test_text(x, y, 65, 595, 1, b"No calendar data", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 620 && y < 628 {
            if let Some(c) = test_text(x, y, 65, 620, 1, b"Schedule data unavailable", TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // Right: Daily Activity & Steps (x: 375..680)
    if y >= 505 && y < 685 && x >= 375 && x < 680 {
        if y == 505 || y == 684 || x == 375 || x == 679 {
            return BORDER_SUBTLE;
        }
        if y >= 525 && y < 541 {
            if let Some(c) = test_text(x, y, 400, 525, 2, b"Activity Rings", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 560 && y < 584 {
            if let Some(c) = test_text(x, y, 400, 560, 3, b"--", SUCCESS_EMERALD) {
                return c;
            }
        }
        if y >= 605 && y < 613 {
            if let Some(c) = test_text(x, y, 400, 605, 1, b"Activity data unavailable", TEXT_MUTED)
            {
                return c;
            }
        }
        if y >= 630 && y < 638 && x >= 400 && x < 655 {
            if x <= 490 {
                return SUCCESS_EMERALD;
            }
            return 0xFF0E2218;
        }
        return SURFACE_RAISED;
    }

    // 6. System Telemetry Gauge Card (x: 40..680, y: 705..845)
    if y >= 705 && y < 845 && x >= 40 && x < 680 {
        if y == 705 || y == 844 || x == 40 || x == 679 {
            return BORDER_SUBTLE;
        }
        if y >= 725 && y < 741 {
            if let Some(c) = test_text(x, y, 65, 725, 2, b"System Hardware Telemetry", TEXT_PRIMARY)
            {
                return c;
            }
        }
        if y >= 755 && y < 763 {
            if let Some(c) = test_text(
                x,
                y,
                65,
                755,
                1,
                b"Hardware telemetry unavailable",
                TEXT_MUTED,
            ) {
                return c;
            }
        }
        if y >= 785 && y < 797 && x >= 65 && x < 655 {
            if x <= 220 {
                return ACCENT_SKY;
            }
            if x <= 410 {
                return ACCENT_AMBER;
            }
            return 0xFF142032;
        }
        return SURFACE_RAISED;
    }

    // 7. Appearance: the stock system theme is immutable.
    if y >= 865 && y < 1015 && x >= 40 && x < 680 {
        if y == 865 || y == 1014 || x == 40 || x == 679 { return BORDER_SUBTLE; }
        if y >= 885 && y < 901 { if let Some(c) = test_text(x, y, 65, 885, 2, b"Appearance", ACCENT_CYAN) { return c; } }
        if y >= 915 && y < 923 { if let Some(c) = test_text(x, y, 65, 915, 1, b"InterOS Stock", TEXT_PRIMARY) { return c; } }
        if y >= 940 && y < 948 { if let Some(c) = test_text(x, y, 65, 940, 1, b"System theme is fixed for this device build", TEXT_MUTED) { return c; } }
        if y >= 970 && y < 978 { if let Some(c) = test_text(x, y, 65, 970, 1, b"Personal themes are unavailable", TEXT_MUTED) { return c; } }
        return SURFACE_RAISED;
    }

    // 8. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}
