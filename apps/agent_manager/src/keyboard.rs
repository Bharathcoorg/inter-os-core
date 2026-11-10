// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The on-screen soft keyboard for Agent Manager text entry.
//!
//! Provides the geometry, touch hit-testing, and key dispatch for typing
//! prompts into the Chat surface on touchscreen portrait devices.

use interos_std::design::components::{KeyAction, SoftKeyboard};
use interos_std::design::primitives::Rect;

/// The width of the portrait screen.
pub const SCREEN_W: usize = 720;

/// The keyboard dock's reference height.
pub const KB_HEIGHT: usize = 430;

/// The keyboard dock's top edge when visible.
pub const KB_Y0: usize = 1010;

/// The keyboard dock's bottom edge (screen height).
pub const KB_Y1: usize = KB_Y0 + KB_HEIGHT;

/// Fixed composer height in pixels.
pub const COMPOSER_H: usize = 72;

/// Where the composer sits when the keyboard is visible: directly above it.
pub const COMPOSER_TOP_KEYBOARD: usize = KB_Y0 - COMPOSER_H; // 938

/// Where the composer sits when the keyboard is idle: above the bottom action bar (ACTION_TOP = 1284).
pub const COMPOSER_TOP_IDLE: usize = 1284 - COMPOSER_H; // 1212

/// State of the on-screen virtual keyboard for Agent Manager.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentKeyboard {
    /// Whether the virtual keyboard is currently visible on the screen.
    pub visible: bool,
    /// Whether the shift mode (uppercase / symbol) is active.
    pub shift: bool,
    /// Currently pressed key for tactile visual touch feedback.
    pub pressed: Option<KeyAction>,
}

impl AgentKeyboard {
    /// Creates a new hidden keyboard state.
    pub const fn new() -> Self {
        Self {
            visible: false,
            shift: false,
            pressed: None,
        }
    }

    /// Returns whether the keyboard is currently shown.
    #[inline(always)]
    pub const fn is_visible(&self) -> bool {
        self.visible
    }

    /// Shows the keyboard.
    pub fn show(&mut self) {
        self.visible = true;
    }

    /// Hides the keyboard and clears pressed state.
    pub fn hide(&mut self) {
        self.visible = false;
        self.pressed = None;
    }

    /// Toggles the shift mode.
    pub fn toggle_shift(&mut self) {
        self.shift = !self.shift;
    }

    /// Sets the pressed key action for touch highlighting.
    pub fn set_pressed(&mut self, action: Option<KeyAction>) {
        self.pressed = action;
    }

    /// Clears any active pressed key highlight.
    pub fn clear_pressed(&mut self) {
        self.pressed = None;
    }

    /// Instantiates the underlying platform `SoftKeyboard` widget.
    pub fn widget(&self) -> SoftKeyboard {
        SoftKeyboard::new(Rect::new(0, KB_Y0, SCREEN_W, KB_HEIGHT))
            .with_shift(self.shift)
            .with_pressed(self.pressed)
    }

    /// Performs hit testing against the soft keyboard dock.
    pub fn hit_test(&self, x: usize, y: usize) -> Option<KeyAction> {
        if !self.visible || y < KB_Y0 || y >= KB_Y1 || x >= SCREEN_W {
            return None;
        }
        self.widget().hit_test(x, y)
    }

    /// Computes the top coordinate of the composer given keyboard visibility.
    #[inline(always)]
    pub const fn composer_top(&self) -> usize {
        if self.visible {
            COMPOSER_TOP_KEYBOARD
        } else {
            COMPOSER_TOP_IDLE
        }
    }

    /// Computes the bottom coordinate of the scrollable chat body.
    #[inline(always)]
    pub const fn body_bottom(&self) -> usize {
        self.composer_top()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatSession;

    #[test]
    fn keyboard_geometry_invariants() {
        assert_eq!(KB_Y0 + KB_HEIGHT, KB_Y1);
        assert_eq!(KB_Y1, 1440);
        assert_eq!(COMPOSER_TOP_KEYBOARD + COMPOSER_H, KB_Y0);
        assert_eq!(COMPOSER_TOP_KEYBOARD, 938);
        assert_eq!(COMPOSER_TOP_IDLE, 1212);
    }

    #[test]
    fn keyboard_visibility_and_toggles() {
        let mut kb = AgentKeyboard::new();
        assert!(!kb.is_visible());
        assert_eq!(kb.composer_top(), COMPOSER_TOP_IDLE);

        kb.show();
        assert!(kb.is_visible());
        assert_eq!(kb.composer_top(), COMPOSER_TOP_KEYBOARD);

        kb.toggle_shift();
        assert!(kb.shift);
        kb.toggle_shift();
        assert!(!kb.shift);

        kb.hide();
        assert!(!kb.is_visible());
        assert_eq!(kb.composer_top(), COMPOSER_TOP_IDLE);
    }

    #[test]
    fn keyboard_hit_test_and_chat_prompt_entry() {
        let mut kb = AgentKeyboard::new();
        let mut chat = ChatSession::new();

        // When hidden, hit test returns None even within bounds
        assert!(kb.hit_test(360, KB_Y0 + 200).is_none());

        kb.show();
        // A touch in keyboard dock resolves to a key
        let key_action = kb.hit_test(360, KB_Y0 + 200);
        assert!(key_action.is_some());

        // Simulate typing characters
        assert!(chat.push_prompt_char(b'h'));
        assert!(chat.push_prompt_char(b'i'));
        assert_eq!(chat.prompt_text(), b"hi");

        // Simulate backspace
        chat.pop_prompt_char();
        assert_eq!(chat.prompt_text(), b"h");
    }
}
