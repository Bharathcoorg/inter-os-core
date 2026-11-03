// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Native Graphical Smartphone Shell & Touch Compositor
//!
//! Manages interactive application cards, capacitive touch hit-testing,
//! and event-driven intent routing.

use super::component::{PermissionModal, ShareSheetDrawer, UiCard};
use super::shade::{NotificationShade, ShadeState};
use crate::framebuffer::{COLOR_AMBER, COLOR_CYAN, COLOR_GREEN, COLOR_PURPLE, PHONE_W, PHONE_X, PHONE_Y};
use crate::input::{InputEvent, TouchGesture, TouchPhase};

/// Interactive smartphone home screen and compositor shell.
pub struct MobileShell {
    pub cards: [UiCard; 12],
    pub shade: NotificationShade,
    pub active_modal: Option<PermissionModal>,
    pub active_share_drawer: Option<ShareSheetDrawer>,
    pub last_dispatched_intent: &'static str,
    pub touch_count: u64,
}

impl MobileShell {
    /// Creates a new MobileShell with the default interactive application cards and notification shade.
    pub const fn new() -> Self {
        let card_x = PHONE_X + 20;
        let card_w = PHONE_W - 40;

        let cards = [
            UiCard {
                id: 1,
                title: "InterOS System Diagnostics",
                subtitle: "Ring 3 Microkernel Core - All Tests Passed",
                badge: "ONLINE",
                x: card_x,
                y: PHONE_Y + 144,
                w: card_w,
                h: 80,
                accent: COLOR_GREEN,
                action_intent: "SYSTEM_DIAGNOSTICS_OK",
            },
            UiCard {
                id: 2,
                title: "Android ACR Container",
                subtitle: "Zone 4 Isolation - Binder IPC Guarded",
                badge: "ISOLATED",
                x: card_x,
                y: PHONE_Y + 236,
                w: card_w,
                h: 80,
                accent: COLOR_CYAN,
                action_intent: "ACR_AUDIT_CONTAINER",
            },
            UiCard {
                id: 3,
                title: "Storage & File System",
                subtitle: "/packages/ Storage Sandbox Active",
                badge: "MOUNTED",
                x: card_x,
                y: PHONE_Y + 328,
                w: card_w,
                h: 80,
                accent: COLOR_AMBER,
                action_intent: "VFS_AUDIT",
            },
            UiCard {
                id: 4,
                title: "Settings & System",
                subtitle: "Wi-Fi, Bluetooth, Display, Audio",
                badge: "NATIVE",
                x: card_x,
                y: PHONE_Y + 420,
                w: card_w,
                h: 80,
                accent: COLOR_CYAN,
                action_intent: "APP_LAUNCH_SETTINGS",
            },
            UiCard {
                id: 6,
                title: "Phone & Dialer",
                subtitle: "Voice Calls & Cellular Management",
                badge: "NATIVE",
                x: card_x,
                y: PHONE_Y + 604,
                w: card_w,
                h: 80,
                accent: COLOR_GREEN,
                action_intent: "APP_LAUNCH_DIALER",
            },
            UiCard {
                id: 7,
                title: "Vault & Encrypted Storage",
                subtitle: "Hardware Keystore & Secure Vault",
                badge: "ENCRYPTED",
                x: card_x,
                y: PHONE_Y + 696,
                w: card_w,
                h: 80,
                accent: COLOR_AMBER,
                action_intent: "APP_LAUNCH_VAULT",
            },
            UiCard {
                id: 8,
                title: "Camera & Photos",
                subtitle: "High Definition Camera & Image Processing",
                badge: "LIVE",
                x: card_x,
                y: PHONE_Y + 788,
                w: card_w,
                h: 80,
                accent: COLOR_CYAN,
                action_intent: "APP_LAUNCH_CAMERA",
            },
            UiCard {
                id: 9,
                title: "Calculator & Utilities",
                subtitle: "Mathematical & Scientific Engine",
                badge: "READY",
                x: card_x,
                y: PHONE_Y + 880,
                w: card_w,
                h: 80,
                accent: COLOR_PURPLE,
                action_intent: "APP_LAUNCH_CALCULATOR",
            },
            UiCard {
                id: 10,
                title: "Web Browser",
                subtitle: "Secure Web Navigation & Client",
                badge: "ONLINE",
                x: card_x,
                y: PHONE_Y + 972,
                w: card_w,
                h: 80,
                accent: COLOR_CYAN,
                action_intent: "APP_LAUNCH_BROWSER",
            },
            UiCard {
                id: 11,
                title: "Messages & Chats",
                subtitle: "Secure End-to-End Messaging",
                badge: "ONLINE",
                x: card_x,
                y: PHONE_Y + 1064,
                w: card_w,
                h: 80,
                accent: COLOR_GREEN,
                action_intent: "APP_LAUNCH_MESSAGES",
            },
            UiCard {
                id: 12,
                title: "Calendar & Events",
                subtitle: "Schedule & Daily Agenda",
                badge: "ONLINE",
                x: card_x,
                y: PHONE_Y + 1156,
                w: card_w,
                h: 80,
                accent: COLOR_PURPLE,
                action_intent: "APP_LAUNCH_CALENDAR",
            },
            UiCard {
                id: 13,
                title: "Music & Media Player",
                subtitle: "Audio Playback & Library",
                badge: "ONLINE",
                x: card_x,
                y: PHONE_Y + 1248,
                w: card_w,
                h: 80,
                accent: COLOR_CYAN,
                action_intent: "APP_LAUNCH_MUSIC",
            },
        ];

        Self {
            cards,
            shade: NotificationShade::new(),
            active_modal: None,
            active_share_drawer: None,
            last_dispatched_intent: "READY: Awaiting Capacitive Touch / Intent",
            touch_count: 0,
        }
    }

    /// Opens an interactive permission consent modal.
    pub fn open_permission_modal(&mut self, modal: PermissionModal) {
        self.active_modal = Some(modal);
    }

    /// Closes the active permission consent modal.
    pub fn close_permission_modal(&mut self) {
        self.active_modal = None;
    }

    /// Opens an interactive cross-application sovereign share drawer.
    pub fn open_share_drawer(&mut self, drawer: ShareSheetDrawer) {
        self.active_share_drawer = Some(drawer);
    }

    /// Closes the active sovereign share drawer.
    pub fn close_share_drawer(&mut self) {
        self.active_share_drawer = None;
    }

    /// Handles a unified touch event, performing hit testing on touch contact points and shade.
    pub fn handle_event(&mut self, event: InputEvent) -> Option<&'static str> {
        // Priority Interception: Active Permission Modal blocks background navigation
        if let Some(modal) = self.active_modal {
            match event {
                InputEvent::Gesture(TouchGesture::Tap { x, y }) => {
                    self.touch_count = self.touch_count.saturating_add(1);
                    if modal.hit_test_allow(x, y) {
                        self.active_modal = None;
                        let _ = crate::syscall::sys_consent_resolve(modal.request_id, 1);
                        self.last_dispatched_intent = "PERMISSION_GRANTED";
                        return Some("PERMISSION_GRANTED");
                    } else if modal.hit_test_deny(x, y) {
                        self.active_modal = None;
                        let _ = crate::syscall::sys_consent_resolve(modal.request_id, 2);
                        self.last_dispatched_intent = "PERMISSION_DENIED";
                        return Some("PERMISSION_DENIED");
                    } else if modal.hit_test_modal(x, y) {
                        return Some("PERMISSION_MODAL_ACTIVE");
                    }
                    return None;
                }
                InputEvent::Touch(touch) if touch.phase == TouchPhase::Down => {
                    self.touch_count = self.touch_count.saturating_add(1);
                    if modal.hit_test_allow(touch.x, touch.y) {
                        self.active_modal = None;
                        let _ = crate::syscall::sys_consent_resolve(modal.request_id, 1);
                        self.last_dispatched_intent = "PERMISSION_GRANTED";
                        return Some("PERMISSION_GRANTED");
                    } else if modal.hit_test_deny(touch.x, touch.y) {
                        self.active_modal = None;
                        let _ = crate::syscall::sys_consent_resolve(modal.request_id, 2);
                        self.last_dispatched_intent = "PERMISSION_DENIED";
                        return Some("PERMISSION_DENIED");
                    } else if modal.hit_test_modal(touch.x, touch.y) {
                        return Some("PERMISSION_MODAL_ACTIVE");
                    }
                    return None;
                }
                _ => return None,
            }
        }

        // Priority Interception: Active Sovereign Share Sheet Drawer
        if let Some(drawer) = self.active_share_drawer {
            match event {
                InputEvent::Gesture(TouchGesture::Tap { x, y }) => {
                    self.touch_count = self.touch_count.saturating_add(1);
                    if drawer.hit_test_target1(x, y) {
                        self.active_share_drawer = None;
                        self.last_dispatched_intent = "SHARE_INTENT_MESSAGES";
                        return Some("SHARE_INTENT_MESSAGES");
                    } else if drawer.hit_test_target3(x, y) {
                        self.active_share_drawer = None;
                        self.last_dispatched_intent = "SHARE_INTENT_VAULT";
                        return Some("SHARE_INTENT_VAULT");
                    } else if drawer.hit_test_target4(x, y) {
                        self.active_share_drawer = None;
                        self.last_dispatched_intent = "SHARE_INTENT_BROWSER";
                        return Some("SHARE_INTENT_BROWSER");
                    } else if drawer.hit_test_drawer(x, y) {
                        return Some("SHARE_DRAWER_ACTIVE");
                    }
                    self.active_share_drawer = None;
                    return Some("SHARE_DRAWER_DISMISSED");
                }
                InputEvent::Touch(touch) if touch.phase == TouchPhase::Down => {
                    self.touch_count = self.touch_count.saturating_add(1);
                    if drawer.hit_test_target1(touch.x, touch.y) {
                        self.active_share_drawer = None;
                        self.last_dispatched_intent = "SHARE_INTENT_MESSAGES";
                        return Some("SHARE_INTENT_MESSAGES");
                    } else if drawer.hit_test_target3(touch.x, touch.y) {
                        self.active_share_drawer = None;
                        self.last_dispatched_intent = "SHARE_INTENT_VAULT";
                        return Some("SHARE_INTENT_VAULT");
                    } else if drawer.hit_test_target4(touch.x, touch.y) {
                        self.active_share_drawer = None;
                        self.last_dispatched_intent = "SHARE_INTENT_BROWSER";
                        return Some("SHARE_INTENT_BROWSER");
                    } else if drawer.hit_test_drawer(touch.x, touch.y) {
                        return Some("SHARE_DRAWER_ACTIVE");
                    }
                    self.active_share_drawer = None;
                    return Some("SHARE_DRAWER_DISMISSED");
                }
                _ => return None,
            }
        }

        match event {
            InputEvent::Gesture(TouchGesture::Drag { start_x, start_y, current_x, current_y }) => {
                self.touch_count = self.touch_count.saturating_add(1);
                if self.shade.handle_drag(start_x, start_y, current_x, current_y) {
                    return Some(if self.shade.state == ShadeState::Open {
                        "NOTIFICATION_SHADE_OPEN"
                    } else {
                        "NOTIFICATION_SHADE_DRAG"
                    });
                }
                None
            }
            InputEvent::Gesture(TouchGesture::Tap { x, y }) => {
                self.touch_count = self.touch_count.saturating_add(1);
                if self.shade.state == ShadeState::Open {
                    if let Some(intent) = self.shade.handle_tap(x, y) {
                        self.last_dispatched_intent = intent;
                        return Some(intent);
                    }
                }
                for card in &self.cards {
                    if card.hit_test(x, y) {
                        self.last_dispatched_intent = card.action_intent;
                        return Some(card.action_intent);
                    }
                }
                None
            }
            InputEvent::Touch(touch) => {
                if touch.phase == TouchPhase::Down {
                    self.touch_count = self.touch_count.saturating_add(1);
                    if self.shade.state == ShadeState::Open {
                        if let Some(intent) = self.shade.handle_tap(touch.x, touch.y) {
                            self.last_dispatched_intent = intent;
                            return Some(intent);
                        }
                    }
                    for card in &self.cards {
                        if card.hit_test(touch.x, touch.y) {
                            self.last_dispatched_intent = card.action_intent;
                            return Some(card.action_intent);
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }
}
