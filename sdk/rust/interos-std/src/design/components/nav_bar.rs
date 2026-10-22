//! Standard Bottom Navigation Bar primitive with active indicator pill.
//!
//! # Platform Truth
//! In accordance with InterOS Core user-interface guidelines, this widget operates
//! strictly `#![no_std]` with zero heap allocations during layout, hit-testing, and rendering.

use crate::design::draw_target::Surface;
use crate::design::font;
use crate::design::primitives::Rect;
use crate::design::theme::Theme;

/// A single tab destination item in the navigation bar.
#[derive(Clone, Copy)]
pub struct NavItem<'a> {
    /// Item text label.
    pub label: &'a [u8],
    /// Optional badge text (e.g. b"3" or b"99+").
    pub badge: Option<&'a [u8]>,
    /// Optional custom glyph painter closure `(surface, center_x, center_y, ink)`.
    pub glyph: Option<&'a dyn Fn(&mut Surface, usize, usize, u32)>,
}

impl<'a> NavItem<'a> {
    /// Creates a new navigation bar item.
    pub const fn new(label: &'a [u8]) -> Self {
        Self {
            label,
            badge: None,
            glyph: None,
        }
    }

    /// Attaches an unread / notification badge.
    pub const fn with_badge(mut self, badge: &'a [u8]) -> Self {
        self.badge = Some(badge);
        self
    }

    /// Attaches an icon renderer closure.
    pub const fn with_glyph(
        mut self,
        glyph: &'a dyn Fn(&mut Surface, usize, usize, u32),
    ) -> Self {
        self.glyph = Some(glyph);
        self
    }
}

/// A bottom navigation bar component with active indicator pills.
#[derive(Clone, Copy)]
pub struct BottomNavBar<'a> {
    /// Bar boundary rectangle.
    pub bounds: Rect,
    /// Destination items.
    pub items: &'a [NavItem<'a>],
    /// Currently selected tab index.
    pub selected_index: usize,
    /// Typography scale for tab labels.
    pub scale: usize,
}

impl<'a> BottomNavBar<'a> {
    /// Creates a bottom navigation bar widget.
    pub const fn new(
        bounds: Rect,
        items: &'a [NavItem<'a>],
        selected_index: usize,
    ) -> Self {
        Self {
            bounds,
            items,
            selected_index,
            scale: 1, // Compact caption scale
        }
    }

    /// Helper that calculates layout anchored to the bottom edge of a screen.
    pub fn layout(
        screen: Rect,
        items: &'a [NavItem<'a>],
        selected_index: usize,
        height: usize,
    ) -> Self {
        let bar_h = height.max(48);
        let bar_y = screen.y + screen.h.saturating_sub(bar_h);
        let bounds = Rect::new(screen.x, bar_y, screen.w, bar_h);
        Self::new(bounds, items, selected_index)
    }

    /// Tests touch coordinates to resolve the selected tab destination.
    pub fn hit_test(&self, px: usize, py: usize) -> Option<usize> {
        if !self.bounds.contains(px, py) || self.items.is_empty() {
            return None;
        }

        let count = self.items.len();
        let slot_w = self.bounds.w / count;
        if slot_w == 0 {
            return None;
        }

        let idx = (px - self.bounds.x) / slot_w;
        Some(idx.min(count - 1))
    }

    /// Renders the bottom navigation bar and active indicator pill.
    pub fn draw(&self, surface: &mut Surface, theme: &Theme) {
        if self.items.is_empty() {
            return;
        }

        // 1. Draw bar background surface
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

        let count = self.items.len();
        let slot_w = self.bounds.w / count;
        let line_h = font::line_height(self.scale);

        // 3. Render items
        for (i, item) in self.items.iter().enumerate() {
            let slot_x = self.bounds.x + i * slot_w;
            let is_selected = i == self.selected_index;

            let cx = slot_x + slot_w / 2;

            let (label_ink, pill_fill) = if is_selected {
                (theme.on_surface, theme.primary_container)
            } else {
                (theme.on_surface_variant, theme.surface_container)
            };

            // Active indicator pill
            if is_selected {
                let pill_w = 64.min(slot_w.saturating_sub(12)).max(32);
                let pill_h = 32.min(self.bounds.h.saturating_sub(8));
                let pill_x = cx.saturating_sub(pill_w / 2);
                let pill_y = self.bounds.y + (self.bounds.h.saturating_sub(pill_h)) / 2;
                let pill_rect = Rect::new(pill_x, pill_y, pill_w, pill_h);
                super::fill_rounded_rect(surface, pill_rect, theme.radii.pill, pill_fill);
            }

            // Draw glyph or label
            if let Some(glyph_painter) = item.glyph {
                let glyph_cy = self.bounds.y + self.bounds.h / 2;
                glyph_painter(surface, cx, glyph_cy, label_ink);
            } else {
                let label_w = font::measure(item.label, self.scale);
                let lx = cx.saturating_sub(label_w / 2);
                let ly = self.bounds.y + (self.bounds.h.saturating_sub(line_h)) / 2;
                super::draw_label(surface, lx, ly, self.scale, item.label, label_ink);
            }

            // Draw badge if present
            if let Some(badge_text) = item.badge {
                let badge_scale = 1;
                let bw = font::measure(badge_text, badge_scale) + 8;
                let bh = 14;
                let bx = cx + 8;
                let by = self.bounds.y + 4;
                let badge_rect = Rect::new(bx, by, bw, bh);

                super::fill_rounded_rect(surface, badge_rect, theme.radii.pill, theme.error);
                let blx = bx + (bw.saturating_sub(font::measure(badge_text, badge_scale))) / 2;
                let bly = by + (bh.saturating_sub(font::line_height(badge_scale))) / 2;
                super::draw_label(
                    surface,
                    blx,
                    bly,
                    badge_scale,
                    badge_text,
                    0xFFFFFFFF,
                );
            }
        }
    }
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
    fn nav_bar_layout_and_hit_testing() {
        let screen = Rect::new(0, 0, 300, 200);
        let items = [
            NavItem::new(b"Home"),
            NavItem::new(b"Search"),
            NavItem::new(b"Profile"),
        ];

        let bar = BottomNavBar::layout(screen, &items, 1, 56);
        assert_eq!(bar.bounds.y, 200 - 56);
        assert_eq!(bar.bounds.w, 300);

        // Outside hits None
        assert_eq!(bar.hit_test(10, 50), None);

        // Slot 0 (Home)
        assert_eq!(bar.hit_test(50, 160), Some(0));
        // Slot 1 (Search)
        assert_eq!(bar.hit_test(150, 160), Some(1));
        // Slot 2 (Profile)
        assert_eq!(bar.hit_test(250, 160), Some(2));
    }

    #[test]
    fn nav_bar_renders_active_indicator_and_labels() {
        let theme = Theme::LIGHT;
        let screen = Rect::new(0, 0, 120, 48);
        let items = [
            NavItem::new(b"App1"),
            NavItem::new(b"App2").with_badge(b"1"),
        ];

        let bar = BottomNavBar::layout(screen, &items, 0, 48);
        let pixels = paint(120, 48, |surface| {
            bar.draw(surface, &theme);
        });

        // Bar container fill
        assert!(pixels.contains(&theme.surface_container));
        // Active indicator pill on slot 0
        assert!(pixels.contains(&theme.primary_container));
        // Error badge pill on slot 1
        assert!(pixels.contains(&theme.error));
    }
}
