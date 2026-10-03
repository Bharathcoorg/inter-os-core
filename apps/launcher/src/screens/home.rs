//! Home surface: launcher deck, dock and status card.
//!
//! WHY `pub`: `render_view` in the crate root dispatches here by view
//! mode. Geometry and colour come from `crate::theme` and
//! `crate::layout`, never from literals local to this file.

#![allow(unused_imports)]

use crate::{test_text, LauncherState};
// Theme tokens live in the presentation library, not the shell binary.
use launcher::theme::*;

pub fn get_home_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 {
            return 0xFFE2E8F0;
        }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, &state.clock_time, 0xFF0F172A) {
                return c;
            }
        }
        // Status indicators.
        //
        // No radio, Wi-Fi radio or fuel gauge is bound on this platform, so these
        // draw empty outlines rather than a filled level. A full battery glyph
        // or four signal bars with no measurement behind them is a fabricated
        // reading, and the hardware query syscalls report unavailable.
        const STATUS_UNAVAILABLE: u32 = 0xFFB6BFCC;
        // Cellular: outline of the bar stack only, no filled level.
        if x >= 590 && x <= 608 && y >= 16 && y <= 32 {
            if y == 32 || x == 590 || (x == 605 && y == 16) {
                return STATUS_UNAVAILABLE;
            }
        }
        // Wi-Fi: outline dot only, no arcs, indicating no association.
        if x >= 633 && x <= 637 && y >= 28 && y <= 32 {
            return STATUS_UNAVAILABLE;
        }
        // Battery: shell and terminal only, with no charge fill.
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 {
                return STATUS_UNAVAILABLE;
            }
            if x >= 685 && x <= 686 && y >= 22 && y <= 28 {
                return STATUS_UNAVAILABLE;
            }
        }
        return 0xFFF0F2F5;
    }

    // 2. Canonical stock header with truthful unavailable telemetry.
    if y < 235 {
        if y >= 64 && y < 80 {
            if let Some(c) = test_text(x, y, 48, 64, 2, b"InterOS", TEXT_MUTED) { return c; }
        }
        if y >= 88 && y < 120 {
            if let Some(c) = test_text(x, y, 48, 88, 4, b"Device", TEXT_PRIMARY) { return c; }
        }
        if y >= 132 && y < 148 {
            if let Some(c) = test_text(x, y, 48, 132, 2, b"System status", TEXT_MUTED) { return c; }
        }
        if y >= 80 && y < 112 && x >= 480 {
            if let Some(c) = test_text(x, y, 480, 80, 2, b"Weather unavailable", TEXT_MUTED) { return c; }
        }
        return BG_DARK;
    }

    // 3. System Status Card (x: 40..680, y: 240..470)
    if x >= 40 && x < 680 && y >= 240 && y < 470 {
        if y == 240 || y == 469 || x == 40 || x == 679 { return BORDER_SUBTLE; }
        if y >= 256 && y < 272 {
            if let Some(c) = test_text(x, y, 68, 256, 2, b"System status", TEXT_PRIMARY) { return c; }
        }
        if y >= 305 && y < 321 {
            if let Some(c) = test_text(x, y, 68, 305, 2, b"Launcher", TEXT_PRIMARY) { return c; }
        }
        if y >= 330 && y < 346 {
            if let Some(c) = test_text(x, y, 68, 330, 1, b"Running from the system package registry", TEXT_MUTED) { return c; }
        }
        if y >= 370 && y < 386 {
            if let Some(c) = test_text(x, y, 68, 370, 2, b"Input", TEXT_PRIMARY) { return c; }
        }
        if y >= 395 && y < 411 {
            if let Some(c) = test_text(x, y, 68, 395, 1, b"Touch and keyboard events are live", TEXT_MUTED) { return c; }
        }
        return SURFACE_DARK;
    }

    // 4. Reference 6-Tile Quick Launch Dock (x: 40..680, y: 500..810)
    // 2 Rows x 3 Columns:
    // Row 0: Phone, Messages, Camera
    // Row 1: Files, Browser, Apps
    if y >= 500 && y < 810 {
        let is_row0 = (500..640).contains(&y);
        let is_row1 = (660..800).contains(&y);

        if is_row0 || is_row1 {
            let col = if (48..238).contains(&x) {
                Some(0)
            } else if (265..455).contains(&x) {
                Some(1)
            } else if (482..672).contains(&x) {
                Some(2)
            } else {
                None
            };

            if let Some(c_idx) = col {
                let card_idx = if is_row0 { c_idx } else { 3 + c_idx };
                let (x0, y0) = match (is_row0, c_idx) {
                    (true, 0) => (48, 500),
                    (true, 1) => (265, 500),
                    (true, 2) => (482, 500),
                    (false, 0) => (48, 660),
                    (false, 1) => (265, 660),
                    (false, 2) => (482, 660),
                    _ => (0, 0),
                };

                // Border
                if y == y0 || y == y0 + 139 || x == x0 || x == x0 + 189 {
                    return 0xFFE2E8F0;
                }

                // Tile Icon Badge (Center: x0 + 95, y0 + 55, r = 26)
                let b_dx = (x as i32) - (x0 as i32 + 95);
                let b_dy = (y as i32) - (y0 as i32 + 55);
                let b_d2 = b_dx * b_dx + b_dy * b_dy;
                let (icon_text, icon_color, label) = match card_idx {
                    0 => (&b"TEL"[..], 0xFF10B981, &b"Phone"[..]),
                    1 => (&b"MSG"[..], 0xFF0284C7, &b"Messages"[..]),
                    2 => (&b"CAM"[..], 0xFFF59E0B, &b"Camera"[..]),
                    3 => (&b"DIR"[..], 0xFF6366F1, &b"Files"[..]),
                    4 => (&b"WWW"[..], 0xFF8B5CF6, &b"Browser"[..]),
                    _ => (&b"APP"[..], 0xFFF43F5E, &b"Apps"[..]),
                };

                if b_d2 <= 676 {
                    if y >= y0 + 51 && y < y0 + 59 {
                        let ox = x0 + 95 - 12;
                        if let Some(c) = test_text(x, y, ox, y0 + 51, 1, icon_text, 0xFFFFFFFF) {
                            return c;
                        }
                    }
                    return icon_color;
                }

                // Label below icon (y0 + 100..y0 + 116)
                if y >= y0 + 100 && y < y0 + 116 {
                    let ox = x0 + 95 - (label.len() * 16) / 2;
                    if let Some(c) = test_text(x, y, ox, y0 + 100, 2, label, 0xFF0F172A) {
                        return c;
                    }
                }

                return 0xFFFFFFFF;
            }
        }
        return 0xFFF0F2F5;
    }

    // 5. Universal Search Command Bar (x: 40..680, y: 840..915)
    if x >= 40 && x < 680 && y >= 840 && y < 915 {
        if y == 840 || y == 914 || x == 40 || x == 679 {
            return 0xFFE2E8F0;
        }

        // Search magnifying glass icon
        let dx_s = (x as i32) - 68;
        let dy_s = (y as i32) - 877;
        let d_s = dx_s * dx_s + dy_s * dy_s;
        if (d_s >= 36 && d_s <= 64) || (dx_s >= 6 && dx_s <= 10 && dx_s == dy_s) {
            return 0xFF64748B;
        }

        // Search placeholder text
        if y >= 870 && y < 886 {
            if let Some(c) = test_text(x, y, 96, 870, 2, b"Search apps and files...", 0xFF94A3B8) {
                return c;
            }
        }

        // Right arrow icon
        if y >= 870 && y < 886 {
            if let Some(c) = test_text(x, y, 644, 870, 2, b">", 0xFF64748B) {
                return c;
            }
        }
        return 0xFFFFFFFF;
    }

    // 6. Bottom Navigation Bar (x: 0..720, y: 1380..1440)
    if y >= 1380 {
        if y == 1380 {
            return 0xFFE2E8F0;
        }
        // Back chevron (<) at x: 140, y: 1404
        if y >= 1402 && y < 1418 {
            if let Some(c) = test_text(x, y, 140, 1402, 2, b"<", 0xFF64748B) {
                return c;
            }
        }
        // Home pill at (320..400, 1408..1414)
        if x >= 320 && x <= 400 && y >= 1408 && y <= 1414 {
            return 0xFF0F172A;
        }
        // Recents ([]) at x: 580, y: 1402
        if y >= 1402 && y < 1418 {
            if let Some(c) = test_text(x, y, 580, 1402, 2, b"[]", 0xFF64748B) {
                return c;
            }
        }
        return 0xFFFFFFFF;
    }

    0xFFF0F2F5
}
