// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Toast / Snackbar notification pill primitive for ephemeral alerts.
//!
//! # Platform Truth
//! SnackBar notifications operate `#![no_std]` with zero heap allocations,
//! providing floating status feedback pills (e.g. "Saved", "Link copied", "Sent").

use crate::design::draw_target::Surface;
use crate::design::font;
use crate::design::primitives::Rect;
use crate::design::theme::Theme;

/// Semantic category for snackbar feedback.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SnackbarKind {
    /// Neutral notification (inverted surface fill).
    Normal,
    /// Confirmation of a successful operation.
    Success,
    /// Alert for an error or failed command.
    Error,
    /// Warning or transient caution notice.
    Warning,
}

/// Hit-testing outcome when interacting with a snackbar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SnackbarHit {
    /// Touch fell outside the snackbar pill.
    None,
    /// Touch hit the main message pill body (dismiss/acknowledge).
    Body,
    /// Touch hit the action button (e.g. "Undo").
    Action,
}

/// Style configuration for a snackbar notification pill.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SnackbarStyle {
    /// Background container fill color.
    pub fill: u32,
    /// Message typography ink.
    pub ink: u32,
    /// Action button label ink.
    pub action_ink: u32,
    /// Corner radius (always pill stadium = 999).
    pub radius: usize,
    /// Border hairline if applicable.
    pub border: Option<u32>,
}

/// Resolves the snackbar appearance based on kind and theme.
pub const fn snackbar_style(theme: &Theme, kind: SnackbarKind) -> SnackbarStyle {
    match kind {
        SnackbarKind::Normal => SnackbarStyle {
            fill: theme.inverted_surface(),
            ink: theme.on_inverted_surface(),
            action_ink: theme.primary,
            radius: theme.radii.pill,
            border: Some(theme.border),
        },
        SnackbarKind::Success => SnackbarStyle {
            fill: theme.success_container,
            ink: theme.on_success_container,
            action_ink: theme.success,
            radius: theme.radii.pill,
            border: None,
        },
        SnackbarKind::Error => SnackbarStyle {
            fill: theme.error_container,
            ink: theme.error,
            action_ink: theme.on_surface,
            radius: theme.radii.pill,
            border: None,
        },
        SnackbarKind::Warning => SnackbarStyle {
            fill: theme.warning_container,
            ink: theme.warning,
            action_ink: theme.on_surface,
            radius: theme.radii.pill,
            border: None,
        },
    }
}

/// A floating ephemeral notification pill.
#[derive(Clone, Copy, Debug)]
pub struct Snackbar<'a> {
    /// Pill bounding rectangle.
    pub bounds: Rect,
    /// Ephemeral message string (ASCII / UTF-8).
    pub message: &'a [u8],
    /// Optional action button label (e.g. b"Undo").
    pub action: Option<&'a [u8]>,
    /// Semantic kind of alert.
    pub kind: SnackbarKind,
    /// Typography scale.
    pub scale: usize,
    /// Sub-rectangle for the interactive action button, if present.
    pub action_rect: Option<Rect>,
}

impl<'a> Snackbar<'a> {
    /// Creates a new snackbar with custom bounds.
    pub const fn new(bounds: Rect, message: &'a [u8]) -> Self {
        Self {
            bounds,
            message,
            action: None,
            kind: SnackbarKind::Normal,
            scale: 1, // Standard compact notification scale
            action_rect: None,
        }
    }

    /// Sets the semantic alert kind.
    pub const fn with_kind(mut self, kind: SnackbarKind) -> Self {
        self.kind = kind;
        self
    }

    /// Attaches an action label to the snackbar.
    pub const fn with_action(mut self, action: &'a [u8]) -> Self {
        self.action = Some(action);
        self
    }

    /// Sets the typography scale.
    pub const fn with_scale(mut self, scale: usize) -> Self {
        self.scale = scale;
        self
    }

    /// Computes centered bottom layout coordinates for a snackbar on a given screen surface.
    pub fn compute_layout(
        screen: Rect,
        message: &'a [u8],
        action: Option<&'a [u8]>,
        kind: SnackbarKind,
        scale: usize,
        theme: &Theme,
    ) -> Self {
        let pad_x = theme.spacing.lg;
        let pad_y = theme.spacing.sm;
        let gap = theme.spacing.md;

        let msg_w = font::measure(message, scale);
        let action_w = action.map_or(0, |a| font::measure(a, scale) + gap);

        let content_w = pad_x * 2 + msg_w + action_w;
        let max_w = screen.w.saturating_sub(theme.spacing.lg * 2);
        let min_w = 140.min(max_w);
        let pill_w = if max_w > 0 {
            content_w.clamp(min_w, max_w)
        } else {
            screen.w
        };
        let pill_h = 44.max(font::line_height(scale) + pad_y * 2);

        let pill_x = screen.x + (screen.w.saturating_sub(pill_w)) / 2;
        // Float 24px above bottom screen margin or navigation bar
        let pill_y = screen.y + screen.h.saturating_sub(pill_h + 24);

        let bounds = Rect::new(pill_x, pill_y, pill_w, pill_h);

        let action_rect = if let Some(_) = action {
            let act_w = action_w;
            let act_x = pill_x + pill_w.saturating_sub(pad_x + act_w);
            Some(Rect::new(act_x, pill_y, act_w, pill_h))
        } else {
            None
        };

        Self {
            bounds,
            message,
            action,
            kind,
            scale,
            action_rect,
        }
    }

    /// Hit-tests a coordinate against the snackbar and its optional action.
    pub fn hit_test(&self, px: usize, py: usize) -> SnackbarHit {
        if !self.bounds.contains(px, py) {
            return SnackbarHit::None;
        }

        if let Some(act_rect) = self.action_rect {
            if act_rect.contains(px, py) {
                return SnackbarHit::Action;
            }
        }

        SnackbarHit::Body
    }

    /// Renders the snackbar notification pill.
    pub fn draw(&self, surface: &mut Surface, theme: &Theme) {
        let style = snackbar_style(theme, self.kind);

        // 1. Draw stadium pill background
        surface.draw_aa_rounded_rect(
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.bounds.h,
            style.radius,
            style.fill,
            style.border,
            1,
        );

        let pad_x = theme.spacing.lg;
        let line_h = font::line_height(self.scale);
        let text_y = self.bounds.y + (self.bounds.h.saturating_sub(line_h)) / 2;

        // 2. Draw message
        super::draw_label(
            surface,
            self.bounds.x + pad_x,
            text_y,
            self.scale,
            self.message,
            style.ink,
        );

        // 3. Draw action button if present
        if let Some(act_text) = self.action {
            if let Some(act_rect) = self.action_rect {
                let act_w = font::measure(act_text, self.scale);
                let act_x = act_rect.x + (act_rect.w.saturating_sub(act_w)) / 2;
                super::draw_label(
                    surface,
                    act_x,
                    text_y,
                    self.scale,
                    act_text,
                    style.action_ink,
                );
            }
        }
    }
}

/// Convenience standalone function to paint a snackbar toast.
pub fn snackbar(
    surface: &mut Surface,
    theme: &Theme,
    screen: Rect,
    message: &[u8],
    action: Option<&[u8]>,
    kind: SnackbarKind,
) {
    let sb = Snackbar::compute_layout(screen, message, action, kind, 1, theme);
    sb.draw(surface, theme);
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
    fn snackbar_style_resolution() {
        let theme = Theme::LIGHT;
        let normal = snackbar_style(&theme, SnackbarKind::Normal);
        assert_eq!(normal.fill, theme.inverted_surface());
        assert_eq!(normal.ink, theme.on_inverted_surface());
        assert_eq!(normal.radius, theme.radii.pill);

        let success = snackbar_style(&theme, SnackbarKind::Success);
        assert_eq!(success.fill, theme.success_container);
        assert_eq!(success.ink, theme.on_success_container);
    }

    #[test]
    fn snackbar_layout_and_hit_testing() {
        let theme = Theme::LIGHT;
        let screen = Rect::new(0, 0, 320, 240);
        let sb = Snackbar::compute_layout(
            screen,
            b"Message sent",
            Some(b"Undo"),
            SnackbarKind::Normal,
            1,
            &theme,
        );

        // Centered horizontally
        assert!(sb.bounds.x > 0);
        assert!(sb.bounds.x + sb.bounds.w < 320);
        // Positioned near bottom
        assert!(sb.bounds.y > 150);

        // Outside hits None
        assert_eq!(sb.hit_test(10, 10), SnackbarHit::None);

        // Body hit
        assert_eq!(sb.hit_test(sb.bounds.x + 10, sb.bounds.y + 10), SnackbarHit::Body);

        // Action hit
        let act = sb.action_rect.expect("action rect exists");
        assert_eq!(
            sb.hit_test(act.x + act.w / 2, act.y + act.h / 2),
            SnackbarHit::Action
        );
    }

    #[test]
    fn snackbar_renders_pill_and_ink() {
        let theme = Theme::LIGHT;
        let screen = Rect::new(0, 0, 120, 60);
        let sb = Snackbar::compute_layout(
            screen,
            b"Saved",
            None,
            SnackbarKind::Normal,
            1,
            &theme,
        );

        let pixels = paint(120, 60, |surface| {
            sb.draw(surface, &theme);
        });

        let style = snackbar_style(&theme, SnackbarKind::Normal);
        assert!(pixels.contains(&style.fill), "pill container fill missing");
    }
}
