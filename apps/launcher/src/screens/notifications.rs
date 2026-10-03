//! Notifications shade.
//!
//! WHY `pub`: `render_view` in the crate root dispatches here by view
//! mode. Geometry and colour come from `crate::theme` and
//! `crate::layout`, never from literals local to this file.

#![allow(unused_imports)]

use crate::{test_text, LauncherState};
// Theme tokens live in the presentation library, not the shell binary.
use launcher::theme::*;

pub fn get_notifications_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    if y < 48 {
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, &state.clock_time, TEXT_PRIMARY) {
                return c;
            }
        }
        return BG_DARK;
    }

    if y >= 48 && y < 110 {
        if y >= 64 && y < 88 {
            if let Some(c) = test_text(x, y, 48, 64, 3, b"Notifications", TEXT_PRIMARY) {
                return c;
            }
        }
        return BG_DARK;
    }

    if y >= 125 && y < 260 && x >= 40 && x < 680 {
        if y == 125 || y == 259 || x == 40 || x == 679 {
            return BORDER_SUBTLE;
        }
        if y >= 150 && y < 166 {
            let label = if state.notification_count == 0 {
                &b"No active notifications"[..]
            } else {
                &b"Active notifications"[..]
            };
            if let Some(c) = test_text(x, y, 68, 150, 2, label, TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 182 && y < 198 {
            let sub = if state.notification_count == 0 {
                &b"The notification center is clear"[..]
            } else {
                &b"Details are provided by system services"[..]
            };
            if let Some(c) = test_text(x, y, 68, 182, 1, sub, TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    if y >= 300 && y < 355 && x >= 260 && x < 460 {
        if y == 300 || y == 354 || x == 260 || x == 459 {
            return BORDER_SUBTLE;
        }
        if y >= 320 && y < 336 {
            if let Some(c) = test_text(x, y, 315, 320, 2, b"Clear All", TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}
