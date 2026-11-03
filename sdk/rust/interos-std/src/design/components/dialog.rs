// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Modal Dialog and Action Sheet native components.
//!
//! # Platform Truth
//! Modal dialogs and action sheets enforce `#![no_std]` and zero heap allocations.
//! Backdrop scrimming, card layout, and stadium action pills are computed directly
//! over fixed-size references and primitive coordinates.

use crate::design::draw_target::Surface;
use crate::design::font;
use crate::design::primitives::Rect;
use crate::design::theme::Theme;

/// Draws a semi-transparent dark backdrop scrim over the given bounds.
pub fn draw_scrim(surface: &mut Surface, _theme: &Theme, bounds: Rect) {
    // 38% opacity scrim (98 / 256) towards black
    for y in bounds.y..bounds.y + bounds.h {
        for x in bounds.x..bounds.x + bounds.w {
            if let Some(existing) = surface.get(x, y) {
                let blended = Theme::blend(existing, 0xFF000000, 98);
                surface.set(x, y, blended);
            }
        }
    }
}

/// The result of hit-testing a touch coordinate against a modal dialog.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DialogAction {
    /// Touch fell outside dialog bounds onto the dimmed backdrop scrim (dismiss intent).
    Backdrop,
    /// Touch hit the primary action stadium pill.
    Primary,
    /// Touch hit the cancel action stadium pill.
    Cancel,
    /// Touch fell inside the dialog card body without triggering an action.
    None,
}

/// A modal dialog card with a title, body message, primary action stadium pill,
/// and optional cancel action stadium pill.
#[derive(Clone, Copy, Debug)]
pub struct ModalDialog<'a> {
    /// Dialog card bounds.
    pub bounds: Rect,
    /// Dialog title.
    pub title: &'a [u8],
    /// Dialog body text.
    pub message: &'a [u8],
    /// Primary action label (e.g. b"Confirm", b"Delete").
    pub primary_label: &'a [u8],
    /// Optional cancel action label (e.g. b"Cancel").
    pub cancel_label: Option<&'a [u8]>,
    /// Whether the primary action is destructive (draws in error status color).
    pub destructive: bool,
    /// Primary action pill rectangle.
    pub primary_rect: Rect,
    /// Cancel action pill rectangle.
    pub cancel_rect: Option<Rect>,
}

impl<'a> ModalDialog<'a> {
    /// Computes centered layout geometry for a modal dialog on a screen surface.
    pub fn layout(
        screen: Rect,
        title: &'a [u8],
        message: &'a [u8],
        primary_label: &'a [u8],
        cancel_label: Option<&'a [u8]>,
        destructive: bool,
        theme: &Theme,
    ) -> Self {
        let pad_x = theme.spacing.lg;
        let pad_y = theme.spacing.lg;
        let gap = theme.spacing.md;

        let max_card_w = (screen.w.saturating_sub(48)).min(320);
        let card_w = max_card_w.max(160);

        let title_scale = theme.type_scale.title.scale;
        let body_scale = theme.type_scale.body.scale;
        let title_h = font::line_height(title_scale);
        let msg_h = font::line_height(body_scale);
        let button_h = 40;

        let content_h = pad_y + title_h + gap + msg_h + gap + button_h + pad_y;
        let card_h = content_h.min(screen.h.saturating_sub(32));

        let card_x = screen.x + (screen.w.saturating_sub(card_w)) / 2;
        let card_y = screen.y + (screen.h.saturating_sub(card_h)) / 2;
        let card_rect = Rect::new(card_x, card_y, card_w, card_h);

        let btn_y = card_y + card_h.saturating_sub(pad_y + button_h);

        let (cancel_rect, primary_rect) = if let Some(_) = cancel_label {
            let total_btn_w = card_w.saturating_sub(pad_x * 2 + gap);
            let single_w = total_btn_w / 2;
            let c_rect = Rect::new(card_x + pad_x, btn_y, single_w, button_h);
            let p_rect = Rect::new(card_x + pad_x + single_w + gap, btn_y, single_w, button_h);
            (Some(c_rect), p_rect)
        } else {
            let p_rect = Rect::new(card_x + pad_x, btn_y, card_w.saturating_sub(pad_x * 2), button_h);
            (None, p_rect)
        };

        Self {
            bounds: card_rect,
            title,
            message,
            primary_label,
            cancel_label,
            destructive,
            primary_rect,
            cancel_rect,
        }
    }

    /// Evaluates which part of the dialog or scrim a touch coordinate hits.
    pub fn hit_test(&self, px: usize, py: usize, screen: Rect) -> DialogAction {
        if self.primary_rect.contains(px, py) {
            return DialogAction::Primary;
        }
        if let Some(cancel_rect) = self.cancel_rect {
            if cancel_rect.contains(px, py) {
                return DialogAction::Cancel;
            }
        }
        if self.bounds.contains(px, py) {
            return DialogAction::None;
        }
        if screen.contains(px, py) {
            return DialogAction::Backdrop;
        }
        DialogAction::None
    }

    /// Renders the modal dialog, including scrim, card background, typography, and stadium pills.
    pub fn draw(&self, surface: &mut Surface, theme: &Theme, screen: Rect) {
        // 1. Dim background screen with scrim
        draw_scrim(surface, theme, screen);

        // 2. Draw centered dialog card
        surface.draw_aa_rounded_rect(
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.bounds.h,
            theme.radii.major,
            theme.surface_container_high,
            Some(theme.border),
            1,
        );

        let pad_x = theme.spacing.lg;
        let pad_y = theme.spacing.lg;
        let title_scale = theme.type_scale.title.scale;
        let body_scale = theme.type_scale.body.scale;
        let title_h = font::line_height(title_scale);

        // 3. Draw Title
        super::draw_label(
            surface,
            self.bounds.x + pad_x,
            self.bounds.y + pad_y,
            title_scale,
            self.title,
            theme.on_surface,
        );

        // 4. Draw Message
        let msg_y = self.bounds.y + pad_y + title_h + theme.spacing.sm;
        super::draw_label(
            surface,
            self.bounds.x + pad_x,
            msg_y,
            body_scale,
            self.message,
            theme.on_surface_variant,
        );

        // 5. Draw Cancel Stadium Pill (if present)
        if let Some(cancel_rect) = self.cancel_rect {
            if let Some(cancel_text) = self.cancel_label {
                super::pill(
                    surface,
                    theme,
                    cancel_rect,
                    cancel_text,
                    theme.type_scale.body.scale,
                    false,
                );
            }
        }

        // 6. Draw Primary Action Stadium Pill
        let (p_fill, p_ink) = if self.destructive {
            (theme.error, 0xFFFFFFFF)
        } else {
            (theme.inverted_surface(), theme.on_inverted_surface())
        };

        super::fill_rounded_rect(surface, self.primary_rect, theme.radii.pill, p_fill);

        let btn_scale = theme.type_scale.body.scale;
        let btn_text_w = font::measure(self.primary_label, btn_scale);
        let btn_h = font::line_height(btn_scale);
        let p_text_x = if btn_text_w >= self.primary_rect.w {
            self.primary_rect.x
        } else {
            self.primary_rect.x + (self.primary_rect.w - btn_text_w) / 2
        };
        let p_text_y = self.primary_rect.y + self.primary_rect.h.saturating_sub(btn_h) / 2;

        super::draw_label(
            surface,
            p_text_x,
            p_text_y,
            btn_scale,
            self.primary_label,
            p_ink,
        );
    }
}

/// An action item within an action sheet.
#[derive(Clone, Copy, Debug)]
pub struct ActionSheetItem<'a> {
    /// Action label text.
    pub label: &'a [u8],
    /// Whether this action represents a destructive operation.
    pub destructive: bool,
    /// Whether this action represents the dismiss/cancel choice.
    pub is_cancel: bool,
}

impl<'a> ActionSheetItem<'a> {
    /// Creates a standard action sheet entry.
    pub const fn new(label: &'a [u8]) -> Self {
        Self {
            label,
            destructive: false,
            is_cancel: false,
        }
    }

    /// Marks the action as destructive.
    pub const fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    /// Marks the action as a cancel action.
    pub const fn cancel(mut self) -> Self {
        self.is_cancel = true;
        self
    }
}

/// Hit-test outcome for an action sheet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActionSheetHit {
    /// Touch fell outside the bottom sheet onto the backdrop scrim.
    Backdrop,
    /// Touch hit an action item at index `idx`.
    Item(usize),
    /// Touch hit within the sheet container header/padding.
    None,
}

/// A bottom action sheet modal component.
#[derive(Clone, Copy, Debug)]
pub struct ActionSheet<'a> {
    /// Sheet container bounds.
    pub bounds: Rect,
    /// Optional header title.
    pub title: Option<&'a [u8]>,
    /// Action entries.
    pub items: &'a [ActionSheetItem<'a>],
}

impl<'a> ActionSheet<'a> {
    /// Layout computation for an action sheet anchored to the bottom of the screen.
    pub fn layout(
        screen: Rect,
        title: Option<&'a [u8]>,
        items: &'a [ActionSheetItem<'a>],
        theme: &Theme,
    ) -> Self {
        let pad_y = theme.spacing.md;
        let item_h = 44;
        let gap = theme.spacing.xs;

        let title_h = if title.is_some() {
            font::line_height(theme.type_scale.title.scale) + theme.spacing.sm
        } else {
            0
        };

        let items_h = items.len() * item_h + items.len().saturating_sub(1) * gap;
        let sheet_h = (pad_y * 2 + title_h + items_h).min(screen.h);

        let sheet_y = screen.y + screen.h.saturating_sub(sheet_h);
        let bounds = Rect::new(screen.x, sheet_y, screen.w, sheet_h);

        Self {
            bounds,
            title,
            items,
        }
    }

    /// Evaluates touch coordinates against the action sheet and backdrop scrim.
    pub fn hit_test(&self, px: usize, py: usize, screen: Rect, theme: &Theme) -> ActionSheetHit {
        if !self.bounds.contains(px, py) {
            if screen.contains(px, py) {
                return ActionSheetHit::Backdrop;
            }
            return ActionSheetHit::None;
        }

        let pad_x = theme.spacing.md;
        let pad_y = theme.spacing.md;
        let item_h = 44;
        let gap = theme.spacing.xs;

        let title_h = if self.title.is_some() {
            font::line_height(theme.type_scale.title.scale) + theme.spacing.sm
        } else {
            0
        };

        let mut curr_y = self.bounds.y + pad_y + title_h;
        for (i, _) in self.items.iter().enumerate() {
            let item_rect = Rect::new(
                self.bounds.x + pad_x,
                curr_y,
                self.bounds.w.saturating_sub(pad_x * 2),
                item_h,
            );
            if item_rect.contains(px, py) {
                return ActionSheetHit::Item(i);
            }
            curr_y += item_h + gap;
        }

        ActionSheetHit::None
    }

    /// Renders the action sheet onto the target surface.
    pub fn draw(&self, surface: &mut Surface, theme: &Theme, screen: Rect) {
        // 1. Scrim backdrop
        draw_scrim(surface, theme, screen);

        // 2. Bottom sheet container with sheet radius
        surface.draw_aa_rounded_rect(
            self.bounds.x,
            self.bounds.y,
            self.bounds.w,
            self.bounds.h,
            theme.radii.sheet,
            theme.surface_container_high,
            Some(theme.border),
            1,
        );

        let pad_x = theme.spacing.md;
        let pad_y = theme.spacing.md;
        let item_h = 44;
        let gap = theme.spacing.xs;

        let mut curr_y = self.bounds.y + pad_y;

        // 3. Title if present
        if let Some(title_text) = self.title {
            let scale = theme.type_scale.title.scale;
            let title_w = font::measure(title_text, scale);
            let tx = if title_w >= self.bounds.w {
                self.bounds.x + pad_x
            } else {
                self.bounds.x + (self.bounds.w - title_w) / 2
            };
            super::draw_label(surface, tx, curr_y, scale, title_text, theme.on_surface);
            curr_y += font::line_height(scale) + theme.spacing.sm;
        }

        // 4. Render item stadium pills
        for item in self.items.iter() {
            let item_rect = Rect::new(
                self.bounds.x + pad_x,
                curr_y,
                self.bounds.w.saturating_sub(pad_x * 2),
                item_h,
            );

            let (fill, ink) = if item.destructive {
                (theme.error_container, theme.error)
            } else if item.is_cancel {
                (theme.surface_container, theme.on_surface)
            } else {
                (theme.surface_container_low, theme.on_surface)
            };

            super::fill_rounded_rect(surface, item_rect, theme.radii.pill, fill);

            let scale = theme.type_scale.body.scale;
            let label_w = font::measure(item.label, scale);
            let lh = font::line_height(scale);
            let lx = if label_w >= item_rect.w {
                item_rect.x
            } else {
                item_rect.x + (item_rect.w - label_w) / 2
            };
            let ly = item_rect.y + item_rect.h.saturating_sub(lh) / 2;

            super::draw_label(surface, lx, ly, scale, item.label, ink);

            curr_y += item_h + gap;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::theme::Theme;

    fn paint(w: usize, h: usize, f: impl FnOnce(&mut Surface<'_>)) -> [u32; 16384] {
        let mut pixels = [0u32; 16384];
        {
            let mut surface = Surface::new(&mut pixels[..w * h], w, h).expect("canvas fits");
            f(&mut surface);
        }
        pixels
    }

    #[test]
    fn modal_dialog_layout_and_hit_testing() {
        let theme = Theme::LIGHT;
        let screen = Rect::new(0, 0, 320, 240);
        let dialog = ModalDialog::layout(
            screen,
            b"Delete Item?",
            b"This action cannot be undone.",
            b"Delete",
            Some(b"Cancel"),
            true,
            &theme,
        );

        // Outside dialog hits backdrop
        assert_eq!(dialog.hit_test(10, 10, screen), DialogAction::Backdrop);

        // Inside primary button hits Primary
        let px = dialog.primary_rect.x + dialog.primary_rect.w / 2;
        let py = dialog.primary_rect.y + dialog.primary_rect.h / 2;
        assert_eq!(dialog.hit_test(px, py, screen), DialogAction::Primary);

        // Inside cancel button hits Cancel
        let cancel = dialog.cancel_rect.expect("cancel rect present");
        let cx = cancel.x + cancel.w / 2;
        let cy = cancel.y + cancel.h / 2;
        assert_eq!(dialog.hit_test(cx, cy, screen), DialogAction::Cancel);

        // Inside card but outside buttons hits None
        let card_mid_x = dialog.bounds.x + dialog.bounds.w / 2;
        let card_top_y = dialog.bounds.y + 12;
        assert_eq!(dialog.hit_test(card_mid_x, card_top_y, screen), DialogAction::None);
    }

    #[test]
    fn modal_dialog_renders_scrim_and_buttons() {
        let theme = Theme::LIGHT;
        let screen = Rect::new(0, 0, 100, 100);
        let dialog = ModalDialog::layout(
            screen,
            b"Save",
            b"Save work?",
            b"OK",
            None,
            false,
            &theme,
        );

        let pixels = paint(100, 100, |surface| {
            // Fill initial white backdrop
            for p in surface.pixels_mut() {
                *p = 0xFFFFFFFF;
            }
            dialog.draw(surface, &theme, screen);
        });

        // Top-left pixel (outside dialog) was dimmed by scrim
        assert_ne!(pixels[0], 0xFFFFFFFF, "scrim did not dim backdrop");
        // Dialog card was rendered
        assert!(pixels.contains(&theme.surface_container_high));
    }

    #[test]
    fn action_sheet_layout_and_hit_testing() {
        let theme = Theme::LIGHT;
        let screen = Rect::new(0, 0, 200, 400);
        let items = [
            ActionSheetItem::new(b"Share"),
            ActionSheetItem::new(b"Delete").destructive(),
            ActionSheetItem::new(b"Cancel").cancel(),
        ];

        let sheet = ActionSheet::layout(screen, Some(b"Actions"), &items, &theme);
        assert_eq!(sheet.bounds.x, 0);
        assert_eq!(sheet.bounds.w, 200);
        assert!(sheet.bounds.y > 0);
        assert_eq!(sheet.bounds.y + sheet.bounds.h, 400);

        // Backdrop hit
        assert_eq!(sheet.hit_test(50, 10, screen, &theme), ActionSheetHit::Backdrop);

        // Item 0 hit
        let mid_x = sheet.bounds.x + sheet.bounds.w / 2;
        let item0_y = sheet.bounds.y + theme.spacing.md + font::line_height(theme.type_scale.title.scale) + theme.spacing.sm + 10;
        assert_eq!(sheet.hit_test(mid_x, item0_y, screen, &theme), ActionSheetHit::Item(0));
    }
}
