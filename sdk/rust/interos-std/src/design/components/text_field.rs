// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Native Text Field widget primitive with cursor, selection, placeholder, and touch hit-testing.
//!
//! # Platform Truth
//! In accordance with InterOS Core user-interface guidelines, this widget operates
//! strictly `#![no_std]` with zero heap allocations during layout, hit-testing, and rendering.

use crate::design::draw_target::Surface;
use crate::design::font;
use crate::design::primitives::Rect;
use crate::design::theme::Theme;

/// Style configuration for a text field widget.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TextFieldStyle {
    /// Background container fill color (ARGB8888).
    pub fill: u32,
    /// Border stroke color (ARGB8888).
    pub border: u32,
    /// Border thickness in pixels (1px resting, 2px focused).
    pub border_width: usize,
    /// Corner radius in pixels.
    pub radius: usize,
    /// Primary text color (ARGB8888).
    pub text_color: u32,
    /// Placeholder text color (ARGB8888).
    pub placeholder_color: u32,
    /// Blinking caret cursor color (ARGB8888).
    pub cursor_color: u32,
    /// Background fill for selected text bounds.
    pub selection_fill: u32,
    /// Text color inside selected range.
    pub selection_text_color: u32,
    /// Inner horizontal padding in pixels.
    pub pad_x: usize,
    /// Inner vertical padding in pixels.
    pub pad_y: usize,
}

/// Resolves the text field style from the system theme.
pub const fn text_field_style(theme: &Theme, focused: bool) -> TextFieldStyle {
    if focused {
        TextFieldStyle {
            fill: theme.surface,
            border: theme.outline,
            border_width: 2,
            radius: theme.radii.control,
            text_color: theme.on_surface,
            placeholder_color: theme.muted,
            cursor_color: theme.primary,
            selection_fill: theme.primary_container,
            selection_text_color: theme.on_primary_container,
            pad_x: theme.spacing.md,
            pad_y: theme.spacing.sm,
        }
    } else {
        TextFieldStyle {
            fill: theme.surface_container,
            border: theme.border,
            border_width: 1,
            radius: theme.radii.control,
            text_color: theme.on_surface,
            placeholder_color: theme.muted,
            cursor_color: theme.on_surface,
            selection_fill: theme.primary_container,
            selection_text_color: theme.on_primary_container,
            pad_x: theme.spacing.md,
            pad_y: theme.spacing.sm,
        }
    }
}

/// A native text input field component.
#[derive(Clone, Copy, Debug)]
pub struct TextField<'a> {
    /// Bounding rectangle for the text field.
    pub bounds: Rect,
    /// Text content bytes (UTF-8 / ASCII).
    pub text: &'a [u8],
    /// Placeholder label shown when text is empty.
    pub placeholder: &'a [u8],
    /// Cursor position as a byte offset in `text` (0..=text.len()).
    pub cursor_pos: usize,
    /// Active selection range `(start, end)` byte offsets if text is selected.
    pub selection: Option<(usize, usize)>,
    /// Whether the text field currently holds user input focus.
    pub focused: bool,
    /// Caret blinking phase visibility (true when rendered).
    pub cursor_visible: bool,
    /// Typography scale (defaulting to body copy scale).
    pub scale: usize,
}

impl<'a> TextField<'a> {
    /// Creates a new text field widget at the given bounds with empty text.
    pub const fn new(bounds: Rect) -> Self {
        Self {
            bounds,
            text: b"",
            placeholder: b"",
            cursor_pos: 0,
            selection: None,
            focused: false,
            cursor_visible: true,
            scale: 2, // Standard body scale
        }
    }

    /// Sets the text content.
    pub const fn with_text(mut self, text: &'a [u8]) -> Self {
        self.text = text;
        self
    }

    /// Sets the placeholder text.
    pub const fn with_placeholder(mut self, placeholder: &'a [u8]) -> Self {
        self.placeholder = placeholder;
        self
    }

    /// Sets the cursor position (clamped to text length on render).
    pub const fn with_cursor(mut self, cursor_pos: usize) -> Self {
        self.cursor_pos = cursor_pos;
        self
    }

    /// Sets the text selection range.
    pub const fn with_selection(mut self, start: usize, end: usize) -> Self {
        self.selection = Some((start, end));
        self
    }

    /// Sets the focus state.
    pub const fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Sets whether the blinking caret is currently visible.
    pub const fn cursor_visible(mut self, visible: bool) -> Self {
        self.cursor_visible = visible;
        self
    }

    /// Sets typography scale.
    pub const fn with_scale(mut self, scale: usize) -> Self {
        self.scale = scale;
        self
    }

    /// Tests whether a screen coordinate (px, py) falls inside this field's bounds.
    pub fn hit_test(&self, px: usize, py: usize) -> bool {
        self.bounds.contains(px, py)
    }

    /// Maps a touch coordinate (px, py) to the nearest cursor byte index in `text`.
    pub fn cursor_at_touch(&self, px: usize, _py: usize, theme: &Theme) -> usize {
        let style = text_field_style(theme, self.focused);
        let text_start_x = self.bounds.x + style.pad_x;

        if px <= text_start_x || self.text.is_empty() {
            return 0;
        }

        let mut prev_x = text_start_x;
        for i in 1..=self.text.len() {
            let curr_w = font::measure(&self.text[..i], self.scale);
            let curr_x = text_start_x + curr_w;
            let mid_x = prev_x + (curr_x.saturating_sub(prev_x)) / 2;
            if px < mid_x {
                return i - 1;
            }
            prev_x = curr_x;
        }

        self.text.len()
    }

    /// Draws the text field onto the specified target surface.
    pub fn draw(&self, surface: &mut Surface, theme: &Theme) {
        let style = text_field_style(theme, self.focused);

        // 1. Draw background rounded rect with border
        surface.draw_aa_rounded_rect(
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.bounds.h,
            style.radius,
            style.fill,
            Some(style.border),
            style.border_width,
        );

        let line_h = font::line_height(self.scale);
        let text_x = self.bounds.x + style.pad_x;
        let text_y = self.bounds.y + (self.bounds.h.saturating_sub(line_h)) / 2;
        let inner_max_w = self.bounds.w.saturating_sub(style.pad_x * 2);

        // 2. Render Text / Placeholder / Selection
        if self.text.is_empty() {
            if !self.placeholder.is_empty() {
                super::draw_label(
                    surface,
                    text_x,
                    text_y,
                    self.scale,
                    self.placeholder,
                    style.placeholder_color,
                );
            }
        } else {
            // Check for valid selection bounds
            if let Some((raw_start, raw_end)) = self.selection {
                let start = raw_start.min(raw_end).min(self.text.len());
                let end = raw_start.max(raw_end).min(self.text.len());

                if start < end {
                    let sel_start_px = text_x + font::measure(&self.text[..start], self.scale);
                    let sel_w = font::measure(&self.text[start..end], self.scale);
                    let sel_rect = Rect::new(
                        sel_start_px,
                        text_y.saturating_sub(2),
                        sel_w.min(inner_max_w.saturating_sub(sel_start_px.saturating_sub(text_x))),
                        line_h + 4,
                    );

                    // Fill selection backdrop
                    super::fill_rounded_rect(surface, sel_rect, 3, style.selection_fill);

                    // Draw unselected prefix
                    if start > 0 {
                        super::draw_label(
                            surface,
                            text_x,
                            text_y,
                            self.scale,
                            &self.text[..start],
                            style.text_color,
                        );
                    }
                    // Draw selected text
                    super::draw_label(
                        surface,
                        sel_start_px,
                        text_y,
                        self.scale,
                        &self.text[start..end],
                        style.selection_text_color,
                    );
                    // Draw unselected suffix
                    if end < self.text.len() {
                        let sel_end_px = sel_start_px + sel_w;
                        super::draw_label(
                            surface,
                            sel_end_px,
                            text_y,
                            self.scale,
                            &self.text[end..],
                            style.text_color,
                        );
                    }
                } else {
                    // Start == end: render normal text
                    super::draw_label(
                        surface,
                        text_x,
                        text_y,
                        self.scale,
                        self.text,
                        style.text_color,
                    );
                }
            } else {
                super::draw_label(
                    surface,
                    text_x,
                    text_y,
                    self.scale,
                    self.text,
                    style.text_color,
                );
            }
        }

        // 3. Draw blinking caret indicator
        if self.focused && self.cursor_visible {
            let cursor_idx = self.cursor_pos.min(self.text.len());
            let cursor_x = text_x + font::measure(&self.text[..cursor_idx], self.scale);

            if cursor_x < self.bounds.x + self.bounds.w.saturating_sub(style.pad_x) {
                let caret_top = text_y;
                let caret_h = line_h;
                let caret_w = 2;

                for cy in caret_top..caret_top + caret_h {
                    for cx in cursor_x..cursor_x + caret_w {
                        surface.set(cx, cy, style.cursor_color);
                    }
                }
            }
        }
    }
}

/// Standalone convenience drawing function for a text field.
pub fn text_field(
    surface: &mut Surface,
    theme: &Theme,
    bounds: Rect,
    text: &[u8],
    placeholder: &[u8],
    focused: bool,
) {
    TextField::new(bounds)
        .with_text(text)
        .with_placeholder(placeholder)
        .focused(focused)
        .with_cursor(text.len())
        .draw(surface, theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::theme::Theme;

    fn paint(w: usize, h: usize, f: impl FnOnce(&mut Surface<'_>)) -> [u32; 8192] {
        let mut pixels = [0u32; 8192];
        {
            let mut surface = Surface::new(&mut pixels[..w * h], w, h).expect("canvas fits");
            f(&mut surface);
        }
        pixels
    }

    #[test]
    fn test_field_resting_and_focused_style() {
        let theme = Theme::LIGHT;
        let resting = text_field_style(&theme, false);
        assert_eq!(resting.fill, theme.surface_container);
        assert_eq!(resting.border, theme.border);
        assert_eq!(resting.border_width, 1);
        assert_eq!(resting.radius, theme.radii.control);

        let focused = text_field_style(&theme, true);
        assert_eq!(focused.fill, theme.surface);
        assert_eq!(focused.border, theme.outline);
        assert_eq!(focused.border_width, 2);
    }

    #[test]
    fn test_field_hit_test_and_touch_caret_mapping() {
        let theme = Theme::LIGHT;
        let rect = Rect::new(10, 10, 200, 44);
        let field = TextField::new(rect)
            .with_text(b"Hello")
            .focused(true)
            .with_scale(1);

        // Hit testing
        assert!(field.hit_test(15, 15));
        assert!(field.hit_test(209, 53));
        assert!(!field.hit_test(5, 15));
        assert!(!field.hit_test(215, 15));

        // Touch to caret position
        let pad_x = text_field_style(&theme, true).pad_x;
        let left_edge = rect.x + pad_x;
        assert_eq!(field.cursor_at_touch(left_edge - 5, 20, &theme), 0);
        assert_eq!(field.cursor_at_touch(left_edge, 20, &theme), 0);

        // Way to the right returns text.len()
        assert_eq!(field.cursor_at_touch(rect.x + rect.w, 20, &theme), 5);
    }

    #[test]
    fn test_field_draws_placeholder_when_empty() {
        let theme = Theme::LIGHT;
        let rect = Rect::new(4, 4, 100, 32);
        let pixels = paint(120, 40, |surface| {
            TextField::new(rect)
                .with_placeholder(b"Search")
                .focused(false)
                .with_scale(1)
                .draw(surface, &theme);
        });

        let style = text_field_style(&theme, false);
        // Container fill is drawn
        assert!(pixels.contains(&style.fill));
        // Placeholder ink is drawn
        assert!(pixels.contains(&style.placeholder_color));
    }

    #[test]
    fn test_field_draws_cursor_indicator_when_focused() {
        let theme = Theme::LIGHT;
        let rect = Rect::new(0, 0, 80, 28);
        let pixels_with_cursor = paint(80, 28, |surface| {
            TextField::new(rect)
                .with_text(b"Hi")
                .focused(true)
                .cursor_visible(true)
                .with_scale(1)
                .draw(surface, &theme);
        });

        let pixels_without_cursor = paint(80, 28, |surface| {
            TextField::new(rect)
                .with_text(b"Hi")
                .focused(true)
                .cursor_visible(false)
                .with_scale(1)
                .draw(surface, &theme);
        });

        let style = text_field_style(&theme, true);
        assert!(
            pixels_with_cursor.contains(&style.cursor_color),
            "focused visible caret was not drawn"
        );
        let cursor_count_with = pixels_with_cursor
            .iter()
            .filter(|p| **p == style.cursor_color)
            .count();
        let cursor_count_without = pixels_without_cursor
            .iter()
            .filter(|p| **p == style.cursor_color)
            .count();
        assert!(cursor_count_with > cursor_count_without);
    }

    #[test]
    fn test_field_selection_bounds() {
        let theme = Theme::LIGHT;
        let rect = Rect::new(0, 0, 100, 30);
        let pixels = paint(100, 30, |surface| {
            TextField::new(rect)
                .with_text(b"SelectMe")
                .with_selection(0, 6)
                .focused(true)
                .with_scale(1)
                .draw(surface, &theme);
        });

        let style = text_field_style(&theme, true);
        assert!(
            pixels.contains(&style.selection_fill),
            "selection fill background not rendered"
        );
    }
}
