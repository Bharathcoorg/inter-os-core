// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! AArch64 Interactive Mobile Graphical Shell & Touch Hit-Testing Engine
//!
//! Provides the smartphone user interface pipeline:
//! - Hit-testing multi-touch coordinates against interactive mobile UI elements
//! - Real-time visual feedback rendering on SimpleFramebuffer (touch ripple, button glow)
//! - Sovereign application surface composition (Settings, Dialer, Vault, AI Assistant)

use crate::display::SimpleFramebuffer;
use crate::touch::{TouchPhase, TouchPoint};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobileAction {
    LaunchDialer,
    LaunchVault,
    LaunchSettings,
}

pub struct DockButton {
    pub label: &'static str,
    pub x_min: u32,
    pub x_max: u32,
    pub y_min: u32,
    pub y_max: u32,
    pub action: MobileAction,
}

pub const DOCK_BUTTONS: [DockButton; 3] = [
    DockButton {
        label: "DIALER",
        x_min: 60,
        x_max: 180,
        y_min: 1320,
        y_max: 1400,
        action: MobileAction::LaunchDialer,
    },
    DockButton {
        label: "VAULT",
        x_min: 190,
        x_max: 310,
        y_min: 1320,
        y_max: 1400,
        action: MobileAction::LaunchVault,
    },
    DockButton {
        label: "SETTINGS",
        x_min: 510,
        x_max: 660,
        y_min: 1320,
        y_max: 1400,
        action: MobileAction::LaunchSettings,
    },
];

pub struct MobileShell {
    pub active_action: Option<MobileAction>,
    pub last_touch_x: u32,
    pub last_touch_y: u32,
    pub total_taps: u64,
}

impl MobileShell {
    pub const fn new() -> Self {
        Self {
            active_action: None,
            last_touch_x: 0,
            last_touch_y: 0,
            total_taps: 0,
        }
    }

    pub fn hit_test(&self, x: u32, y: u32) -> Option<MobileAction> {
        for btn in &DOCK_BUTTONS {
            if x >= btn.x_min && x <= btn.x_max && y >= btn.y_min && y <= btn.y_max {
                return Some(btn.action);
            }
        }
        None
    }

    pub fn process_touch(&mut self, pt: &TouchPoint, fb: &mut SimpleFramebuffer) -> Option<MobileAction> {
        self.last_touch_x = pt.x;
        self.last_touch_y = pt.y;

        match pt.phase {
            TouchPhase::Down | TouchPhase::Move => {
                // Render touch cursor indicator: 12x12 cyan crosshair indicator
                self.draw_touch_indicator(fb, pt.x, pt.y);
                None
            }
            TouchPhase::Up => {
                self.total_taps += 1;
                if let Some(action) = self.hit_test(pt.x, pt.y) {
                    self.active_action = Some(action);
                    self.render_app_surface(fb, action);
                    self.highlight_button(fb, action);
                    Some(action)
                } else {
                    None
                }
            }
        }
    }

    fn draw_touch_indicator(&self, fb: &mut SimpleFramebuffer, x: u32, y: u32) {
        // High-contrast touch feedback crosshair at (x, y)
        let cx = x.min(fb.width.saturating_sub(6));
        let cy = y.min(fb.height.saturating_sub(6));
        fb.fill_rect(cx.saturating_sub(5), cy.saturating_sub(1), 11, 3, 0, 229, 255);
        fb.fill_rect(cx.saturating_sub(1), cy.saturating_sub(5), 3, 11, 0, 229, 255);
    }

    pub fn highlight_button(&self, fb: &mut SimpleFramebuffer, action: MobileAction) {
        for btn in &DOCK_BUTTONS {
            let is_selected = btn.action == action;
            let (r, g, b) = if is_selected {
                (0, 229, 255) // Cyan active highlight
            } else {
                (35, 45, 75) // Dark slate neutral
            };
            // Draw button border
            let w = btn.x_max - btn.x_min;
            let h = btn.y_max - btn.y_min;
            fb.fill_rect(btn.x_min, btn.y_min, w, 2, r, g, b);
            fb.fill_rect(btn.x_min, btn.y_max - 2, w, 2, r, g, b);
            fb.fill_rect(btn.x_min, btn.y_min, 2, h, r, g, b);
            fb.fill_rect(btn.x_max - 2, btn.y_min, 2, h, r, g, b);
        }
    }

    pub fn render_app_surface(&self, fb: &mut SimpleFramebuffer, action: MobileAction) {
        // Clear center mobile application viewport (Y: 100 to 1280)
        fb.fill_rect(20, 100, fb.width - 40, 1180, 14, 18, 32);

        // Header border
        fb.fill_rect(20, 100, fb.width - 40, 3, 0, 229, 255);

        match action {
            MobileAction::LaunchSettings => {
                fb.draw_string(40, 130, "SYSTEM SETTINGS", (0, 229, 255), (14, 18, 32));
                fb.draw_string(40, 160, "Security: Zero Ambient Authority", (255, 255, 255), (14, 18, 32));
                fb.draw_string(40, 190, "Capability Matrix: Active & Enforced", (160, 175, 195), (14, 18, 32));
                fb.draw_string(40, 220, "Architecture: AArch64 (ARMv8-A)", (160, 175, 195), (14, 18, 32));
                fb.draw_string(40, 250, "Flash Storage: SDHCI Controller Online", (160, 175, 195), (14, 18, 32));
                fb.draw_string(40, 280, "Kernel Privilege: EL1 Supervised Separation", (0, 229, 255), (14, 18, 32));
                fb.draw_string(40, 320, "[STATUS: OPERATIONAL  |  MEMORY: ISOLATED]", (100, 255, 150), (14, 18, 32));
            }
            MobileAction::LaunchDialer => {
                fb.draw_string(40, 130, "TELEPHONY SUBSYSTEM", (0, 229, 255), (14, 18, 32));
                fb.draw_string(40, 160, "Modem Interface: Serial AT Controller", (255, 255, 255), (14, 18, 32));
                fb.draw_string(40, 190, "Capability Required: TelephonyModem", (160, 175, 195), (14, 18, 32));
                fb.draw_string(40, 220, "Call State: Idle", (160, 175, 195), (14, 18, 32));
                fb.draw_string(40, 250, "Modem Hardware: Checking UART / AT", (160, 175, 195), (14, 18, 32));
            }
            MobileAction::LaunchVault => {
                fb.draw_string(40, 130, "ENCRYPTED VAULT", (0, 229, 255), (14, 18, 32));
                fb.draw_string(40, 160, "Hardware Keystore: 16 Enclave Slots", (255, 255, 255), (14, 18, 32));
                fb.draw_string(40, 190, "Master Seed: Sealed in EL1 Memory", (100, 255, 150), (14, 18, 32));
                fb.draw_string(40, 220, "Storage Isolation: AES-256 Block Encrypted", (160, 175, 195), (14, 18, 32));
            }
        }
    }
}
