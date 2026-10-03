//! System Space: device and account settings.
//!
//! WHY `pub`: `render_view` in the crate root dispatches here by view
//! mode. Geometry and colour come from `crate::theme` and
//! `crate::layout`, never from literals local to this file.

#![allow(unused_imports)]

use crate::{test_text, LauncherState};
// Theme tokens live in the presentation library, not the shell binary.
use launcher::theme::*;

pub fn get_system_space_color(x: usize, y: usize, _state: &LauncherState) -> u32 {
    // Header
    if y < 100 {
        if y >= 50 && y < 74 {
            if let Some(c) = test_text(x, y, 48, 50, 3, b"System", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 96 {
            return BORDER_SUBTLE;
        }
        return BG_DARK;
    }

    // 3D Glass Ambient Sphere Centerpiece (y: 110..290)
    if y >= 110 && y < 290 {
        let dx = (x as i32) - 360;
        let dy = (y as i32) - 200;
        let d = dx * dx + dy * dy;
        let r_sq = 65 * 65;
        if d <= r_sq {
            let s_dx = dx + 22;
            let s_dy = dy + 22;
            if s_dx * s_dx + s_dy * s_dy <= 120 {
                return 0xFFFFFFFF;
            }
            if s_dx * s_dx + s_dy * s_dy <= 300 {
                return ACCENT_SKY;
            }
            if d >= r_sq - 300 {
                return 0xFF235A7C;
            }
            return 0xFF121B2A;
        }
        if y >= 275 && y < 283 {
            if let Some(c) = test_text(x, y, 260, 275, 1, b"Everything in one place", TEXT_MUTED) {
                return c;
            }
        }
        return BG_DARK;
    }

    // 2-Column Visual Category Grid (y: 310..1180)
    if y >= 310 && y < 430 {
        if x >= 40 && x < 345 {
            if y >= 350 && y < 366 {
                if let Some(c) = test_text(x, y, 70, 350, 2, b"Device", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 380 && y < 388 {
                if let Some(c) =
                    test_text(x, y, 70, 380, 1, b"Display, Sound, Gestures", TEXT_MUTED)
                {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }
        if x >= 375 && x < 680 {
            if y >= 350 && y < 366 {
                if let Some(c) = test_text(x, y, 405, 350, 2, b"Network", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 380 && y < 388 {
                if let Some(c) =
                    test_text(x, y, 405, 380, 1, b"Wi-Fi, Mobile, Bluetooth", TEXT_MUTED)
                {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }
    }
    if y >= 450 && y < 570 {
        if x >= 40 && x < 345 {
            if y >= 490 && y < 506 {
                if let Some(c) = test_text(x, y, 70, 490, 2, b"Personal", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 520 && y < 528 {
                if let Some(c) =
                    test_text(x, y, 70, 520, 1, b"Accounts, Privacy, Language", TEXT_MUTED)
                {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }
        if x >= 375 && x < 680 {
            if y >= 490 && y < 506 {
                if let Some(c) = test_text(x, y, 405, 490, 2, b"Apps", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 520 && y < 528 {
                if let Some(c) = test_text(x, y, 405, 520, 1, b"Permissions, Storage", TEXT_MUTED) {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }
    }
    if y >= 590 && y < 710 {
        if x >= 40 && x < 345 {
            if y >= 630 && y < 646 {
                if let Some(c) = test_text(x, y, 70, 630, 2, b"Power", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 660 && y < 668 {
                if let Some(c) = test_text(x, y, 70, 660, 1, b"Battery, DVFS Governor", TEXT_MUTED)
                {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }
        if x >= 375 && x < 680 {
            if y >= 630 && y < 646 {
                if let Some(c) = test_text(x, y, 405, 630, 2, b"Security", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 660 && y < 668 {
                if let Some(c) = test_text(x, y, 405, 660, 1, b"KeyStore, Encryption", TEXT_MUTED) {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }
    }

    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }
    BG_DARK
}
