//! Power Menu: lock, restart and power off.
//!
//! WHY `pub`: `render_view` in the crate root dispatches here by view
//! mode. Geometry and colour come from `crate::theme` and
//! `crate::layout`, never from literals local to this file.

#![allow(unused_imports)]

use crate::{test_text, LauncherState};
// Theme tokens live in the presentation library, not the shell binary.
use launcher::theme::*;

pub fn get_power_menu_color(x: usize, y: usize, _state: &LauncherState) -> u32 {
    // Backdrop blur / dim on left
    if x < 340 {
        return 0xFF05070A;
    }

    // Drawer Container (x >= 340)
    if x == 340 {
        return BORDER_SUBTLE;
    }

    // Header (y: 80..150)
    if y >= 95 && y < 111 {
        if let Some(c) = test_text(x, y, 370, 95, 2, b"Power & Safety", TEXT_PRIMARY) {
            return c;
        }
    }
    if y >= 120 && y < 128 {
        if let Some(c) = test_text(x, y, 370, 120, 1, b"System actions & control", TEXT_MUTED) {
            return c;
        }
    }

    // Action Card 1: Lock device (y: 190..265, x: 365..695)
    if y >= 190 && y < 265 && x >= 365 && x < 695 {
        if y == 190 || y == 264 || x == 365 || x == 694 {
            return BORDER_SUBTLE;
        }
        if y >= 212 && y < 228 {
            if let Some(c) = test_text(x, y, 395, 212, 2, b"Lock Device", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 236 && y < 244 {
            if let Some(c) = test_text(x, y, 395, 236, 1, b"Secure current session", TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // Action Card 2: Restart (y: 285..360, x: 365..695)
    if y >= 285 && y < 360 && x >= 365 && x < 695 {
        if y == 285 || y == 359 || x == 365 || x == 694 {
            return BORDER_SUBTLE;
        }
        if y >= 307 && y < 323 {
            if let Some(c) = test_text(x, y, 395, 307, 2, b"Restart", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 331 && y < 339 {
            if let Some(c) = test_text(x, y, 395, 331, 1, b"Reboot InterOS kernel", TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // Action Card 3: Power off (y: 380..455, x: 365..695)
    if y >= 380 && y < 455 && x >= 365 && x < 695 {
        if y == 380 || y == 454 || x == 365 || x == 694 {
            return BORDER_SUBTLE;
        }
        if y >= 402 && y < 418 {
            if let Some(c) = test_text(x, y, 395, 402, 2, b"Power Off", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 426 && y < 434 {
            if let Some(c) = test_text(x, y, 395, 426, 1, b"Shut down device power", TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // Action Card 4: Emergency (y: 475..550, x: 365..695)
    if y >= 475 && y < 550 && x >= 365 && x < 695 {
        if y == 475 || y == 549 || x == 365 || x == 694 {
            return DANGER_CRIMSON;
        }
        if y >= 497 && y < 513 {
            if let Some(c) = test_text(x, y, 395, 497, 2, b"Emergency", DANGER_CRIMSON) {
                return c;
            }
        }
        if y >= 521 && y < 529 {
            if let Some(c) = test_text(
                x,
                y,
                395,
                521,
                1,
                b"Direct dial emergency lines",
                TEXT_MUTED,
            ) {
                return c;
            }
        }
        return 0xFF221115;
    }

    // Dismiss Button (y: 580..635, x: 420..640)
    if y >= 580 && y < 635 && x >= 420 && x < 640 {
        if y == 580 || y == 634 || x == 420 || x == 639 {
            return BORDER_SUBTLE;
        }
        if y >= 600 && y < 616 {
            if let Some(c) = test_text(x, y, 490, 600, 2, b"Cancel", TEXT_PRIMARY) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    SURFACE_DARK
}
