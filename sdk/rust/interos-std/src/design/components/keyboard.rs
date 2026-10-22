//! Native On-Screen Soft Keyboard widget with QWERTY layout, touch hit-testing,
//! and platform theme vector styling.
//!
//! # Platform Truth
//! In accordance with InterOS Core user-interface guidelines, this component operates
//! strictly `#![no_std]` with zero heap allocations during layout, hit-testing, and rendering.
//!
//! # Key Layout Architecture
//! The soft keyboard provides a 5-row portrait QWERTY layout:
//! 1. Number / Symbol row (10 keys: 1-0 or !-))
//! 2. Top letter row (10 keys: Q-P)
//! 3. Home letter row (9 keys: A-L)
//! 4. Bottom letter row (9 keys: Shift, Z-M, Backspace)
//! 5. Action / Space row (5 keys: Hide, Comma, Space bar, Period, Enter)
//!
//! Keys are styled as stadium pills using platform theme tokens, supporting
//! dark/light mode contrast, active shift invert, and pressed tactile highlights.

use crate::design::draw_target::Surface;
use crate::design::font;
use crate::design::primitives::Rect;
use crate::design::theme::Theme;

/// Standard default reference width for the portrait keyboard dock (720px).
pub const DEFAULT_KB_WIDTH: usize = 720;
/// Standard default reference height for the portrait keyboard dock (430px).
pub const DEFAULT_KB_HEIGHT: usize = 430;
/// Number of key rows in the layout.
pub const KEYBOARD_ROW_COUNT: usize = 5;
/// Reference row step height.
pub const DEFAULT_ROW_STEP: usize = 82;
/// Reference key pill height.
pub const DEFAULT_KEY_HEIGHT: usize = 72;

/// What a key of the soft keyboard does when pressed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyAction {
    /// Types a character (ASCII character / letter / symbol / space).
    Char(u8),
    /// Deletes the preceding character before the cursor.
    Backspace,
    /// Commits the input or submits the current line.
    Enter,
    /// Toggles uppercase capitalisation or symbol mode.
    Shift,
    /// Dismisses or hides the on-screen keyboard.
    Hide,
}

impl KeyAction {
    /// Returns the character value if this is a `KeyAction::Char`.
    pub const fn char(self) -> Option<u8> {
        match self {
            KeyAction::Char(c) => Some(c),
            _ => None,
        }
    }

    /// Whether this action inserts a character.
    pub const fn is_char(self) -> bool {
        matches!(self, KeyAction::Char(_))
    }

    /// Whether this action is Backspace.
    pub const fn is_backspace(self) -> bool {
        matches!(self, KeyAction::Backspace)
    }

    /// Whether this action is Enter.
    pub const fn is_enter(self) -> bool {
        matches!(self, KeyAction::Enter)
    }

    /// Whether this action is Shift.
    pub const fn is_shift(self) -> bool {
        matches!(self, KeyAction::Shift)
    }

    /// Whether this action is Hide.
    pub const fn is_hide(self) -> bool {
        matches!(self, KeyAction::Hide)
    }
}

/// Helper converting an action into its typed ASCII byte if it is a character.
pub const fn char_of(action: KeyAction) -> Option<u8> {
    action.char()
}

/// Static description of a key in the reference layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyDef {
    /// Column X position in reference space (0..720).
    pub x: usize,
    /// Row Y position in reference space (0..430).
    pub y: usize,
    /// Key width in reference space.
    pub w: usize,
    /// Key height in reference space.
    pub h: usize,
    /// Action emitted when unshifted.
    pub unshifted: KeyAction,
    /// Action emitted when shifted.
    pub shifted: KeyAction,
    /// Display label when unshifted.
    pub unshifted_label: &'static [u8],
    /// Display label when shifted.
    pub shifted_label: &'static [u8],
}

impl KeyDef {
    /// Returns the action for the given shift state.
    pub const fn action(&self, shift: bool) -> KeyAction {
        if shift {
            self.shifted
        } else {
            self.unshifted
        }
    }

    /// Returns the display label for the given shift state.
    pub const fn label(&self, shift: bool) -> &'static [u8] {
        if shift {
            self.shifted_label
        } else {
            self.unshifted_label
        }
    }
}

macro_rules! key {
    ($x:expr, $y:expr, $w:expr, $h:expr, $un_act:expr, $sh_act:expr, $un_lbl:expr, $sh_lbl:expr) => {
        KeyDef {
            x: $x,
            y: $y,
            w: $w,
            h: $h,
            unshifted: $un_act,
            shifted: $sh_act,
            unshifted_label: $un_lbl,
            shifted_label: $sh_lbl,
        }
    };
}

/// Static definition of all 43 keys in the standard reference layout.
pub const KEYS: &[KeyDef] = &[
    // --- Row 0: Numbers & Symbols (y: 14, h: 72) ---
    key!(15, 14, 60, 72, KeyAction::Char(b'1'), KeyAction::Char(b'!'), b"1", b"!"),
    key!(85, 14, 60, 72, KeyAction::Char(b'2'), KeyAction::Char(b'@'), b"2", b"@"),
    key!(155, 14, 60, 72, KeyAction::Char(b'3'), KeyAction::Char(b'#'), b"3", b"#"),
    key!(225, 14, 60, 72, KeyAction::Char(b'4'), KeyAction::Char(b'$'), b"4", b"$"),
    key!(295, 14, 60, 72, KeyAction::Char(b'5'), KeyAction::Char(b'%'), b"5", b"%"),
    key!(365, 14, 60, 72, KeyAction::Char(b'6'), KeyAction::Char(b'^'), b"6", b"^"),
    key!(435, 14, 60, 72, KeyAction::Char(b'7'), KeyAction::Char(b'&'), b"7", b"&"),
    key!(505, 14, 60, 72, KeyAction::Char(b'8'), KeyAction::Char(b'*'), b"8", b"*"),
    key!(575, 14, 60, 72, KeyAction::Char(b'9'), KeyAction::Char(b'('), b"9", b"("),
    key!(645, 14, 60, 72, KeyAction::Char(b'0'), KeyAction::Char(b')'), b"0", b")"),

    // --- Row 1: Top Letter Row Q-P (y: 96, h: 72) ---
    key!(15, 96, 60, 72, KeyAction::Char(b'q'), KeyAction::Char(b'Q'), b"q", b"Q"),
    key!(85, 96, 60, 72, KeyAction::Char(b'w'), KeyAction::Char(b'W'), b"w", b"W"),
    key!(155, 96, 60, 72, KeyAction::Char(b'e'), KeyAction::Char(b'E'), b"e", b"E"),
    key!(225, 96, 60, 72, KeyAction::Char(b'r'), KeyAction::Char(b'R'), b"r", b"R"),
    key!(295, 96, 60, 72, KeyAction::Char(b't'), KeyAction::Char(b'T'), b"t", b"T"),
    key!(365, 96, 60, 72, KeyAction::Char(b'y'), KeyAction::Char(b'Y'), b"y", b"Y"),
    key!(435, 96, 60, 72, KeyAction::Char(b'u'), KeyAction::Char(b'U'), b"u", b"U"),
    key!(505, 96, 60, 72, KeyAction::Char(b'i'), KeyAction::Char(b'I'), b"i", b"I"),
    key!(575, 96, 60, 72, KeyAction::Char(b'o'), KeyAction::Char(b'O'), b"o", b"O"),
    key!(645, 96, 60, 72, KeyAction::Char(b'p'), KeyAction::Char(b'P'), b"p", b"P"),

    // --- Row 2: Home Letter Row A-L (y: 178, h: 72) ---
    key!(50, 178, 60, 72, KeyAction::Char(b'a'), KeyAction::Char(b'A'), b"a", b"A"),
    key!(120, 178, 60, 72, KeyAction::Char(b's'), KeyAction::Char(b'S'), b"s", b"S"),
    key!(190, 178, 60, 72, KeyAction::Char(b'd'), KeyAction::Char(b'D'), b"d", b"D"),
    key!(260, 178, 60, 72, KeyAction::Char(b'f'), KeyAction::Char(b'F'), b"f", b"F"),
    key!(330, 178, 60, 72, KeyAction::Char(b'g'), KeyAction::Char(b'G'), b"g", b"G"),
    key!(400, 178, 60, 72, KeyAction::Char(b'h'), KeyAction::Char(b'H'), b"h", b"H"),
    key!(470, 178, 60, 72, KeyAction::Char(b'j'), KeyAction::Char(b'J'), b"j", b"J"),
    key!(540, 178, 60, 72, KeyAction::Char(b'k'), KeyAction::Char(b'K'), b"k", b"K"),
    key!(610, 178, 60, 72, KeyAction::Char(b'l'), KeyAction::Char(b'L'), b"l", b"L"),

    // --- Row 3: Shift, Z-M, Backspace (y: 260, h: 72) ---
    key!(15, 260, 80, 72, KeyAction::Shift, KeyAction::Shift, b"^", b"^"),
    key!(105, 260, 60, 72, KeyAction::Char(b'z'), KeyAction::Char(b'Z'), b"z", b"Z"),
    key!(175, 260, 60, 72, KeyAction::Char(b'x'), KeyAction::Char(b'X'), b"x", b"X"),
    key!(245, 260, 60, 72, KeyAction::Char(b'c'), KeyAction::Char(b'C'), b"c", b"C"),
    key!(315, 260, 60, 72, KeyAction::Char(b'v'), KeyAction::Char(b'V'), b"v", b"V"),
    key!(385, 260, 60, 72, KeyAction::Char(b'b'), KeyAction::Char(b'B'), b"b", b"B"),
    key!(455, 260, 60, 72, KeyAction::Char(b'n'), KeyAction::Char(b'N'), b"n", b"N"),
    key!(525, 260, 60, 72, KeyAction::Char(b'm'), KeyAction::Char(b'M'), b"m", b"M"),
    key!(595, 260, 110, 72, KeyAction::Backspace, KeyAction::Backspace, b"Del", b"Del"),

    // --- Row 4: Controls, Space, Enter (y: 342, h: 72) ---
    key!(15, 342, 80, 72, KeyAction::Hide, KeyAction::Hide, b"Hide", b"Hide"),
    key!(105, 342, 60, 72, KeyAction::Char(b','), KeyAction::Char(b'?'), b",", b"?"),
    key!(175, 342, 330, 72, KeyAction::Char(b' '), KeyAction::Char(b' '), b"Space", b"Space"),
    key!(515, 342, 60, 72, KeyAction::Char(b'.'), KeyAction::Char(b'!'), b".", b"!"),
    key!(585, 342, 120, 72, KeyAction::Enter, KeyAction::Enter, b"Enter", b"Enter"),
];

/// Visual treatment for a keyboard key pill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyStyle {
    /// Container fill color.
    pub fill: u32,
    /// Typography ink color.
    pub ink: u32,
    /// Corner radius (stadium pill).
    pub radius: usize,
    /// Hairline border color if stroked.
    pub border: Option<u32>,
}

/// Resolves key styling based on system theme, key action, shift state, and pressed state.
pub fn resolve_key_style(
    theme: &Theme,
    action: KeyAction,
    shift: bool,
    is_pressed: bool,
) -> KeyStyle {
    if is_pressed {
        return KeyStyle {
            fill: theme.primary_container,
            ink: theme.on_primary_container,
            radius: theme.radii.pill,
            border: Some(theme.outline),
        };
    }

    match action {
        KeyAction::Enter => KeyStyle {
            fill: theme.primary,
            ink: theme.on_primary,
            radius: theme.radii.pill,
            border: None,
        },
        KeyAction::Shift if shift => KeyStyle {
            fill: theme.inverted_surface(),
            ink: theme.on_inverted_surface(),
            radius: theme.radii.pill,
            border: None,
        },
        KeyAction::Shift | KeyAction::Backspace | KeyAction::Hide => KeyStyle {
            fill: theme.surface_container_low,
            ink: theme.on_surface,
            radius: theme.radii.pill,
            border: Some(theme.border),
        },
        KeyAction::Char(b' ') => KeyStyle {
            fill: theme.surface_container_high,
            ink: theme.on_surface_variant,
            radius: theme.radii.pill,
            border: Some(theme.border),
        },
        KeyAction::Char(_) => KeyStyle {
            fill: theme.surface_container_high,
            ink: theme.on_surface,
            radius: theme.radii.pill,
            border: Some(theme.border),
        },
    }
}

/// Evaluates whether coordinate (x, y) falls inside a rounded rectangle / stadium pill.
pub fn in_rounded_rect(
    x: usize,
    y: usize,
    rx: usize,
    ry: usize,
    rw: usize,
    rh: usize,
    r: usize,
) -> bool {
    if x < rx || x >= rx + rw || y < ry || y >= ry + rh {
        return false;
    }
    let r = r.min(rw / 2).min(rh / 2);
    let dx = if x < rx + r {
        rx + r - x
    } else if x >= rx + rw - r {
        x - (rx + rw - r - 1)
    } else {
        0
    };
    let dy = if y < ry + r {
        ry + r - y
    } else if y >= ry + rh - r {
        y - (ry + rh - r - 1)
    } else {
        0
    };
    dx * dx + dy * dy <= r * r
}

/// An on-screen soft keyboard widget for touchscreen text entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoftKeyboard {
    /// Bounding rectangle for the keyboard dock.
    pub bounds: Rect,
    /// Whether the shift (uppercase / symbol) mode is active.
    pub shift: bool,
    /// Currently pressed key action for tactile touch feedback highlighting.
    pub pressed_key: Option<KeyAction>,
}

impl SoftKeyboard {
    /// Creates a soft keyboard at the specified bounds.
    pub const fn new(bounds: Rect) -> Self {
        Self {
            bounds,
            shift: false,
            pressed_key: None,
        }
    }

    /// Computes layout anchored to the bottom edge of a screen.
    pub fn layout(screen: Rect) -> Self {
        let kb_h = DEFAULT_KB_HEIGHT.min(screen.h);
        let kb_y = screen.y + screen.h.saturating_sub(kb_h);
        let bounds = Rect::new(screen.x, kb_y, screen.w, kb_h);
        Self::new(bounds)
    }

    /// Sets the shift state.
    pub const fn with_shift(mut self, shift: bool) -> Self {
        self.shift = shift;
        self
    }

    /// Sets the active pressed key for touch highlight.
    pub const fn with_pressed(mut self, pressed: Option<KeyAction>) -> Self {
        self.pressed_key = pressed;
        self
    }

    /// Toggles the shift state between lowercase and uppercase / symbols.
    pub fn toggle_shift(&mut self) {
        self.shift = !self.shift;
    }

    /// Clears any active pressed state highlight.
    pub fn clear_pressed(&mut self) {
        self.pressed_key = None;
    }

    /// Resolves the key under a touch point and sets it as the active pressed key.
    pub fn set_pressed_at(&mut self, px: usize, py: usize) -> Option<KeyAction> {
        let action = self.hit_test(px, py);
        self.pressed_key = action;
        action
    }

    /// Maps a reference key definition to screen coordinates within this keyboard's bounds.
    pub fn key_rect(&self, key: &KeyDef) -> Rect {
        if self.bounds.w == DEFAULT_KB_WIDTH && self.bounds.h == DEFAULT_KB_HEIGHT {
            Rect::new(
                self.bounds.x + key.x,
                self.bounds.y + key.y,
                key.w,
                key.h,
            )
        } else if self.bounds.w == 0 || self.bounds.h == 0 {
            Rect::new(self.bounds.x, self.bounds.y, 0, 0)
        } else {
            let actual_x = self.bounds.x + (key.x * self.bounds.w) / DEFAULT_KB_WIDTH;
            let actual_y = self.bounds.y + (key.y * self.bounds.h) / DEFAULT_KB_HEIGHT;
            let actual_w = (key.w * self.bounds.w) / DEFAULT_KB_WIDTH;
            let actual_h = (key.h * self.bounds.h) / DEFAULT_KB_HEIGHT;
            Rect::new(actual_x, actual_y, actual_w, actual_h)
        }
    }

    /// Performs touch hit-testing against the keyboard dock.
    ///
    /// Returns `Some(KeyAction)` if the point `(px, py)` lands on a key pill,
    /// or `None` if the touch falls outside keyboard bounds or in the gutters between keys.
    pub fn hit_test(&self, px: usize, py: usize) -> Option<KeyAction> {
        if !self.bounds.contains(px, py) {
            return None;
        }

        let rx = if self.bounds.w == DEFAULT_KB_WIDTH {
            px - self.bounds.x
        } else if self.bounds.w == 0 {
            return None;
        } else {
            ((px - self.bounds.x) * DEFAULT_KB_WIDTH) / self.bounds.w
        };

        let ry = if self.bounds.h == DEFAULT_KB_HEIGHT {
            py - self.bounds.y
        } else if self.bounds.h == 0 {
            return None;
        } else {
            ((py - self.bounds.y) * DEFAULT_KB_HEIGHT) / self.bounds.h
        };

        for key in KEYS {
            if rx >= key.x && rx < key.x + key.w && ry >= key.y && ry < key.y + key.h {
                return Some(key.action(self.shift));
            }
        }

        None
    }

    /// Alias for `hit_test`.
    pub fn key_at(&self, px: usize, py: usize) -> Option<KeyAction> {
        self.hit_test(px, py)
    }

    /// Returns the static slice of all key definitions in the layout.
    pub const fn keys() -> &'static [KeyDef] {
        KEYS
    }

    /// Returns the total count of keys in the layout.
    pub const fn key_count() -> usize {
        KEYS.len()
    }

    /// Renders the soft keyboard dock and vector stadium pill keys onto the target surface.
    pub fn draw(&self, surface: &mut Surface, theme: &Theme) {
        if self.bounds.w == 0 || self.bounds.h == 0 {
            return;
        }

        // 1. Draw keyboard dock container background
        surface.draw_aa_rounded_rect(
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.bounds.h,
            0,
            theme.surface_container,
            None,
            0,
        );

        // 2. Draw top hairline divider
        let divider_y = self.bounds.y;
        for x in self.bounds.x..self.bounds.x + self.bounds.w {
            if let Some(existing) = surface.get(x, divider_y) {
                let blended = Theme::blend(existing, theme.outline_variant, 210);
                surface.set(x, divider_y, blended);
            }
        }

        // 3. Render each key pill and label
        for key in KEYS {
            let rect = self.key_rect(key);
            let action = key.action(self.shift);
            let label = key.label(self.shift);
            let is_pressed = self.pressed_key == Some(action);
            let style = resolve_key_style(theme, action, self.shift, is_pressed);

            // Vector stadium pill key
            surface.draw_aa_rounded_rect(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                style.radius,
                style.fill,
                style.border,
                1,
            );

            // Text label
            let scale = if label.len() <= 1 {
                2
            } else {
                1
            };
            let label_w = font::measure(label, scale);
            let label_h = font::line_height(scale);

            let lx = rect.x + rect.w.saturating_sub(label_w) / 2;
            let ly = rect.y + rect.h.saturating_sub(label_h) / 2;

            super::draw_label(surface, lx, ly, scale, label, style.ink);
        }
    }

    /// Evaluates the pixel color at (px, py) for per-pixel shader rendering pipelines.
    ///
    /// Returns `Some(argb8888)` if (px, py) lies within the keyboard dock, or `None` otherwise.
    pub fn render_pixel(&self, px: usize, py: usize, theme: &Theme) -> Option<u32> {
        if !self.bounds.contains(px, py) {
            return None;
        }

        // Top hairline divider
        if py == self.bounds.y || (self.bounds.h > 1 && py == self.bounds.y + 1) {
            return Some(theme.outline_variant);
        }

        // Map to reference space
        let rx = if self.bounds.w == DEFAULT_KB_WIDTH {
            px - self.bounds.x
        } else if self.bounds.w == 0 {
            return Some(theme.surface_container);
        } else {
            ((px - self.bounds.x) * DEFAULT_KB_WIDTH) / self.bounds.w
        };

        let ry = if self.bounds.h == DEFAULT_KB_HEIGHT {
            py - self.bounds.y
        } else if self.bounds.h == 0 {
            return Some(theme.surface_container);
        } else {
            ((py - self.bounds.y) * DEFAULT_KB_HEIGHT) / self.bounds.h
        };

        // Check if inside any key pill
        for key in KEYS {
            if rx >= key.x && rx < key.x + key.w && ry >= key.y && ry < key.y + key.h {
                let rect = self.key_rect(key);
                let radius = theme.radii.pill;
                if in_rounded_rect(px, py, rect.x, rect.y, rect.w, rect.h, radius) {
                    let action = key.action(self.shift);
                    let label = key.label(self.shift);
                    let is_pressed = self.pressed_key == Some(action);
                    let style = resolve_key_style(theme, action, self.shift, is_pressed);

                    let scale = if label.len() <= 1 {
                        2
                    } else {
                        1
                    };
                    let label_w = font::measure(label, scale);
                    let label_h = font::line_height(scale);

                    let lx = rect.x + rect.w.saturating_sub(label_w) / 2;
                    let ly = rect.y + rect.h.saturating_sub(label_h) / 2;

                    if let Some(c) = super::label_pixel(px, py, lx, ly, scale, label, style.ink, style.fill) {
                        return Some(c);
                    }
                    return Some(style.fill);
                }
            }
        }

        Some(theme.surface_container)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::draw_target::Surface;
    use crate::design::theme::Theme;

    #[test]
    fn keyboard_layout_geometry_and_invariants() {
        assert_eq!(KEYS.len(), 43);
        assert_eq!(SoftKeyboard::key_count(), 43);

        for (i, key) in KEYS.iter().enumerate() {
            // Must reside within 720 x 430 reference box
            assert!(
                key.x + key.w <= DEFAULT_KB_WIDTH,
                "key {} out of horizontal bounds: {} + {} > {}",
                i,
                key.x,
                key.w,
                DEFAULT_KB_WIDTH
            );
            assert!(
                key.y + key.h <= DEFAULT_KB_HEIGHT,
                "key {} out of vertical bounds: {} + {} > {}",
                i,
                key.y,
                key.h,
                DEFAULT_KB_HEIGHT
            );

            // Verify non-zero dimensions
            assert!(key.w > 0);
            assert!(key.h > 0);

            // Check no other key overlaps with this key
            for (j, other) in KEYS.iter().enumerate() {
                if i != j {
                    let no_overlap = key.x + key.w <= other.x
                        || other.x + other.w <= key.x
                        || key.y + key.h <= other.y
                        || other.y + other.h <= key.y;
                    assert!(
                        no_overlap,
                        "key {} ({:?}) overlaps with key {} ({:?})",
                        i,
                        key.unshifted,
                        j,
                        other.unshifted
                    );
                }
            }
        }
    }

    #[test]
    fn keyboard_hit_testing_unshifted() {
        let kb = SoftKeyboard::new(Rect::new(0, 0, 720, 430));

        // Row 0: 1, 2, 0
        assert_eq!(kb.hit_test(15 + 10, 14 + 10), Some(KeyAction::Char(b'1')));
        assert_eq!(kb.hit_test(85 + 10, 14 + 10), Some(KeyAction::Char(b'2')));
        assert_eq!(kb.hit_test(645 + 10, 14 + 10), Some(KeyAction::Char(b'0')));

        // Row 1: q, w, p
        assert_eq!(kb.hit_test(15 + 10, 96 + 10), Some(KeyAction::Char(b'q')));
        assert_eq!(kb.hit_test(85 + 10, 96 + 10), Some(KeyAction::Char(b'w')));
        assert_eq!(kb.hit_test(645 + 10, 96 + 10), Some(KeyAction::Char(b'p')));

        // Row 2: a, s, l
        assert_eq!(kb.hit_test(50 + 10, 178 + 10), Some(KeyAction::Char(b'a')));
        assert_eq!(kb.hit_test(120 + 10, 178 + 10), Some(KeyAction::Char(b's')));
        assert_eq!(kb.hit_test(610 + 10, 178 + 10), Some(KeyAction::Char(b'l')));

        // Row 3: Shift, z, m, Backspace
        assert_eq!(kb.hit_test(15 + 10, 260 + 10), Some(KeyAction::Shift));
        assert_eq!(kb.hit_test(105 + 10, 260 + 10), Some(KeyAction::Char(b'z')));
        assert_eq!(kb.hit_test(525 + 10, 260 + 10), Some(KeyAction::Char(b'm')));
        assert_eq!(kb.hit_test(595 + 10, 260 + 10), Some(KeyAction::Backspace));

        // Row 4: Hide, Comma, Space, Period, Enter
        assert_eq!(kb.hit_test(15 + 10, 342 + 10), Some(KeyAction::Hide));
        assert_eq!(kb.hit_test(105 + 10, 342 + 10), Some(KeyAction::Char(b',')));
        assert_eq!(kb.hit_test(200, 342 + 10), Some(KeyAction::Char(b' ')));
        assert_eq!(kb.hit_test(515 + 10, 342 + 10), Some(KeyAction::Char(b'.')));
        assert_eq!(kb.hit_test(600, 342 + 10), Some(KeyAction::Enter));

        // Bounds checks: outside returns None
        assert_eq!(kb.hit_test(720, 100), None);
        assert_eq!(kb.hit_test(100, 430), None);
        assert_eq!(kb.hit_test(0, 0), None); // top-left margin
        assert_eq!(kb.hit_test(75, 14 + 10), None); // gap between key 1 and key 2
    }

    #[test]
    fn keyboard_hit_testing_shifted() {
        let kb = SoftKeyboard::new(Rect::new(0, 0, 720, 430)).with_shift(true);

        // Row 0 shifted: ! and )
        assert_eq!(kb.hit_test(15 + 10, 14 + 10), Some(KeyAction::Char(b'!')));
        assert_eq!(kb.hit_test(645 + 10, 14 + 10), Some(KeyAction::Char(b')')));

        // Row 1 shifted: Q and P
        assert_eq!(kb.hit_test(15 + 10, 96 + 10), Some(KeyAction::Char(b'Q')));
        assert_eq!(kb.hit_test(645 + 10, 96 + 10), Some(KeyAction::Char(b'P')));

        // Row 2 shifted: A and L
        assert_eq!(kb.hit_test(50 + 10, 178 + 10), Some(KeyAction::Char(b'A')));
        assert_eq!(kb.hit_test(610 + 10, 178 + 10), Some(KeyAction::Char(b'L')));

        // Row 3 shifted: Shift, Z, Backspace
        assert_eq!(kb.hit_test(15 + 10, 260 + 10), Some(KeyAction::Shift));
        assert_eq!(kb.hit_test(105 + 10, 260 + 10), Some(KeyAction::Char(b'Z')));
        assert_eq!(kb.hit_test(595 + 10, 260 + 10), Some(KeyAction::Backspace));

        // Row 4 shifted: ?, Space, !, Enter
        assert_eq!(kb.hit_test(105 + 10, 342 + 10), Some(KeyAction::Char(b'?')));
        assert_eq!(kb.hit_test(200, 342 + 10), Some(KeyAction::Char(b' ')));
        assert_eq!(kb.hit_test(515 + 10, 342 + 10), Some(KeyAction::Char(b'!')));
        assert_eq!(kb.hit_test(600, 342 + 10), Some(KeyAction::Enter));
    }

    #[test]
    fn shift_state_toggle_and_pressed_touch() {
        let mut kb = SoftKeyboard::new(Rect::new(0, 0, 720, 430));
        assert!(!kb.shift);
        kb.toggle_shift();
        assert!(kb.shift);
        kb.toggle_shift();
        assert!(!kb.shift);

        assert_eq!(kb.pressed_key, None);
        let action = kb.set_pressed_at(15 + 10, 14 + 10);
        assert_eq!(action, Some(KeyAction::Char(b'1')));
        assert_eq!(kb.pressed_key, Some(KeyAction::Char(b'1')));

        kb.clear_pressed();
        assert_eq!(kb.pressed_key, None);
    }

    #[test]
    fn keyboard_layout_anchoring() {
        let screen = Rect::new(0, 0, 720, 1440);
        let kb = SoftKeyboard::layout(screen);
        assert_eq!(kb.bounds.x, 0);
        assert_eq!(kb.bounds.y, 1440 - 430);
        assert_eq!(kb.bounds.w, 720);
        assert_eq!(kb.bounds.h, 430);

        // Hit testing at bottom anchored offset
        let hit = kb.hit_test(15 + 10, 1440 - 430 + 14 + 10);
        assert_eq!(hit, Some(KeyAction::Char(b'1')));
    }

    #[test]
    fn scaled_keyboard_hit_testing() {
        // Half resolution bounds: 360 x 215
        let kb = SoftKeyboard::new(Rect::new(10, 20, 360, 215));
        // Key 0 reference is at (15, 14, 60, 72).
        // In half scale, x is roughly 10 + (15*360)/720 = 10 + 7 = 17, w = 30.
        // y is roughly 20 + (14*215)/430 = 20 + 7 = 27, h = 36.
        let hit = kb.hit_test(17 + 10, 27 + 10);
        assert_eq!(hit, Some(KeyAction::Char(b'1')));
    }

    #[test]
    fn key_action_predicates() {
        let ch = KeyAction::Char(b'a');
        assert!(ch.is_char());
        assert_eq!(ch.char(), Some(b'a'));
        assert_eq!(char_of(ch), Some(b'a'));
        assert!(!ch.is_backspace());
        assert!(!ch.is_enter());
        assert!(!ch.is_shift());
        assert!(!ch.is_hide());

        let bs = KeyAction::Backspace;
        assert!(bs.is_backspace());
        assert_eq!(bs.char(), None);

        let ent = KeyAction::Enter;
        assert!(ent.is_enter());

        let sh = KeyAction::Shift;
        assert!(sh.is_shift());

        let hi = KeyAction::Hide;
        assert!(hi.is_hide());
    }

    #[test]
    fn resolve_key_style_theming_and_contrast() {
        let theme = Theme::LIGHT;

        let char_style = resolve_key_style(&theme, KeyAction::Char(b'A'), false, false);
        assert_eq!(char_style.fill, theme.surface_container_high);
        assert_eq!(char_style.ink, theme.on_surface);

        let enter_style = resolve_key_style(&theme, KeyAction::Enter, false, false);
        assert_eq!(enter_style.fill, theme.primary);
        assert_eq!(enter_style.ink, theme.on_primary);

        let shift_active = resolve_key_style(&theme, KeyAction::Shift, true, false);
        assert_eq!(shift_active.fill, theme.inverted_surface());
        assert_eq!(shift_active.ink, theme.on_inverted_surface());

        let pressed_style = resolve_key_style(&theme, KeyAction::Char(b'A'), false, true);
        assert_eq!(pressed_style.fill, theme.primary_container);
        assert_eq!(pressed_style.ink, theme.on_primary_container);
    }

    #[test]
    fn keyboard_vector_draw_renders_to_surface() {
        let theme = Theme::LIGHT;
        let mut pixels = [0u32; 80 * 50];
        let mut surface = Surface::new(&mut pixels, 80, 50).expect("fits");

        let kb = SoftKeyboard::new(Rect::new(0, 0, 80, 50));
        kb.draw(&mut surface, &theme);

        // Check dock background was drawn
        assert!(pixels.iter().any(|&p| p == theme.surface_container));
        // Check keys were drawn
        assert!(pixels.iter().any(|&p| p == theme.surface_container_high));
        // Check primary enter key was drawn
        assert!(pixels.iter().any(|&p| p == theme.primary));
    }

    #[test]
    fn keyboard_render_pixel_shader_sampling() {
        let theme = Theme::LIGHT;
        let kb = SoftKeyboard::new(Rect::new(0, 0, 720, 430));

        // Outside dock returns None
        assert_eq!(kb.render_pixel(750, 100, &theme), None);
        assert_eq!(kb.render_pixel(100, 450, &theme), None);

        // Hairline divider
        assert_eq!(kb.render_pixel(100, 0, &theme), Some(theme.outline_variant));

        // Center of space bar key pill
        let space_pixel = kb.render_pixel(340, 342 + 36, &theme);
        assert!(space_pixel.is_some());
    }
}
