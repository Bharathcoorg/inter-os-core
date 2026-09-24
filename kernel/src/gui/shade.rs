//! Mobile Touch Notification Shade & Quick Settings Panel
//!
//! Provides a smartphone notification shade and quick control center.
//! Responds to downward pull-down touch gestures from the top screen notch.

use crate::framebuffer::{PHONE_W, PHONE_X, PHONE_Y};

/// State of the notification shade drawer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadeState {
    Closed,
    Dragging { current_y: u32 },
    Open,
}

/// A quick-setting toggle switch (WiFi, 5G, Bluetooth, Battery, DVFS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuickToggle {
    pub name: &'static str,
    pub enabled: bool,
    pub action_intent: &'static str,
}

/// Mobile Notification Shade and Quick Settings.
pub struct NotificationShade {
    pub state: ShadeState,
    pub toggles: [QuickToggle; 5],
    pub height: u32,
}

impl NotificationShade {
    pub const fn new() -> Self {
        Self {
            state: ShadeState::Closed,
            toggles: [
                QuickToggle { name: "WiFi", enabled: true, action_intent: "TOGGLE_WIFI" },
                QuickToggle { name: "5G Cellular", enabled: true, action_intent: "TOGGLE_CELLULAR" },
                QuickToggle { name: "Bluetooth", enabled: false, action_intent: "TOGGLE_BT" },
                QuickToggle { name: "Battery Saver", enabled: false, action_intent: "TOGGLE_BATTERY_SAVER" },
                QuickToggle { name: "Interactive DVFS", enabled: true, action_intent: "TOGGLE_DVFS" },
            ],
            height: 320,
        }
    }

    /// Handles a drag gesture from the top edge to expand the shade.
    pub fn handle_drag(&mut self, start_x: u32, start_y: u32, _current_x: u32, current_y: u32) -> bool {
        let phone_top = PHONE_Y as u32;
        let phone_left = PHONE_X as u32;
        let phone_right = (PHONE_X + PHONE_W) as u32;

        // Check if drag originated in the top notch zone (0..40 px from top)
        if start_x >= phone_left && start_x <= phone_right && start_y >= phone_top && start_y <= (phone_top + 40) {
            if current_y > start_y + 80 {
                self.state = ShadeState::Open;
                return true;
            } else if current_y > start_y {
                self.state = ShadeState::Dragging { current_y };
                return true;
            }
        } else if self.state == ShadeState::Open {
            // Drag upward to close
            if current_y + 80 < start_y {
                self.state = ShadeState::Closed;
                return true;
            }
        }
        false
    }

    /// Handles tap events when the shade is open to toggle quick settings.
    pub fn handle_tap(&mut self, x: u32, y: u32) -> Option<&'static str> {
        if self.state != ShadeState::Open {
            return None;
        }

        let phone_top = PHONE_Y as u32;
        let phone_left = PHONE_X as u32;

        // 5 toggle pills arranged horizontally in the shade header (Y: phone_top + 60 .. phone_top + 110)
        if y >= (phone_top + 60) && y <= (phone_top + 110) {
            let relative_x = x.saturating_sub(phone_left);
            let pill_width = (PHONE_W as u32) / 5;
            let index = (relative_x / pill_width) as usize;
            if index < self.toggles.len() {
                self.toggles[index].enabled = !self.toggles[index].enabled;
                return Some(self.toggles[index].action_intent);
            }
        }

        // Tapping outside the shade closes it
        if y > (phone_top + self.height) {
            self.state = ShadeState::Closed;
        }

        None
    }
}
