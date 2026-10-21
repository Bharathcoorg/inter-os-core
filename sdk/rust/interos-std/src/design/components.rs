//! The widgets a third-party app needs to look native.
//!
//! WHY this module exists: the SDK had tokens and a drawing target, and no way
//! to put the two together. An app that wanted a card had to know the platform's
//! corner radius, its hairline colour, its elevation steps and its type scale
//! all at once, and the only way to learn them was to read another app's source.
//! That is not an interface a third party can build against.
//!
//! These are the platform's own treatments, expressed once: a card, a pill, the
//! segmented control the launcher's tab row uses, an app-icon tile and a list
//! row. An app that calls them gets the same pixels the shell draws, and an app
//! that wants something else can still reach the tokens directly.
//!
//! WHY they paint into a [`Surface`] rather than answering "what colour is this
//! pixel": the shell paints per pixel because it repaints a region at a time,
//! but a third-party app paints a whole frame into a buffer it owns, and a
//! widget that only worked one way would be a widget half the apps could not
//! use.
//!
//! WHY nothing here allocates: the SDK is `#![no_std]` with no allocator, so
//! every widget here is a plain value or a function over one the caller already
//! owns.

use super::draw_target::Surface;
use super::font;
use super::outline_text;
use super::primitives::Rect;
use super::theme::Theme;
use super::typeface;

/// How a card sits in the stack of surfaces.
///
/// WHY this is an enum and not a colour: the elevation is a decision about
/// depth, and the colours that express it are the theme's to choose. An app
/// that passed a colour instead would have to know the palette to ask for an
/// elevation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Elevation {
    /// A resting card: the container fill, outlined with the hairline.
    Resting,
    /// A card in front of another: one step up.
    Raised,
    /// A card under a finger: one step down, and no hairline.
    Pressed,
}

/// Fills a rounded rectangle.
///
/// WHY the radius is a parameter and not the theme's card radius: a pill, a
/// sheet and a card are the same shape at different roundings, and a widget
/// that hardcoded one of them could not draw the others.
pub fn fill_rounded_rect(surface: &mut Surface, rect: Rect, radius: usize, color: u32) {
    surface.draw_aa_rounded_rect(
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        radius,
        color,
        None,
        0,
    );
}

/// The platform's card treatment: the fill, the hairline and the corner radius.
///
/// WHY this is data and not only a drawing call: the guest apps paint one
/// pixel at a time into a band, because a surface write is capped at 4,096
/// pixels and a whole frame does not fit in one call. A component that could
/// only paint a whole [`Surface`] would be a component the guest apps could
/// not use at all. The *treatment* is the part that has to be shared -- the
/// fill, the hairline and the radius -- and a per-pixel painter applies it
/// with the platform's own primitive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CardStyle {
    /// The fill the card is painted with.
    pub fill: u32,
    /// The hairline the card is outlined with, if it has one.
    pub border: Option<u32>,
    /// The corner radius the card's corners are drawn at.
    pub radius: usize,
}

/// Resolves the platform's card treatment for an elevation.
///
/// WHY the elevation decides the colours and not the caller: the elevation is
/// a decision about depth, and the colours that express it are the theme's to
/// choose. An app that passed a colour instead would have to know the palette
/// to ask for an elevation.
pub const fn card_style(theme: &Theme, elevation: Elevation) -> CardStyle {
    let (fill, border) = match elevation {
        Elevation::Resting => (theme.surface_container, Some(theme.border)),
        Elevation::Raised => (theme.surface_container_high, Some(theme.border)),
        Elevation::Pressed => (theme.surface_container_low, None),
    };
    CardStyle {
        fill,
        border,
        radius: theme.radii.card,
    }
}

/// Draws a card: a rounded rectangle with the platform's elevation treatment.
///
/// WHY the hairline is drawn rather than a shadow: the platform's display has
/// no compositor blur, and a shadow drawn by the app is a shape the app has to
/// get right on every surface it draws over. A hairline is one pixel of a
/// colour the theme already names.
pub fn card(surface: &mut Surface, theme: &Theme, rect: Rect, elevation: Elevation) {
    let style = card_style(theme, elevation);
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
}

/// The platform's list-row treatment: the fill, the corner radius, the
/// divider's ink and the row's inner padding.
///
/// WHY it is a struct and not four loose values: a row is one shape, and an
/// app that reached for the padding from one place and the fill from another
/// would draw a row that does not match the platform's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ListRowStyle {
    /// The fill the row is painted with.
    pub fill: u32,
    /// The corner radius the row's corners are drawn at.
    pub radius: usize,
    /// The ink the row's divider is drawn in.
    pub divider: u32,
    /// The row's inner padding, on the text's leading edge.
    pub pad: usize,
}

/// Resolves the platform's list-row treatment.
///
/// WHY a selected row is a raised surface and not the brand colour: the row
/// is still the same row, and a selection that repainted the row's own fill
/// with the accent would read as a different kind of thing.
pub const fn list_row_style(theme: &Theme, selected: bool) -> ListRowStyle {
    ListRowStyle {
        fill: if selected {
            theme.surface_container_high
        } else {
            theme.surface
        },
        radius: theme.radii.card,
        divider: theme.outline_variant,
        pad: theme.spacing.md,
    }
}

/// Draws a pill: a fully rounded shape with a centred label.
///
/// WHY the selected segment is filled with the opposite of the page: the
/// design references the apps are built from put the current segment in
/// near-black, and that is the only way a row of otherwise identical chips
/// says which one is current. The old treatment -- the same raised fill for
/// every segment and a different ink for the selected one -- made the
/// selection a matter of reading rather than of seeing.
///
/// WHY the ink on it is the page's own colour: the control is the opposite of
/// the page, so the page's colour is what reads on it.
pub fn pill(
    surface: &mut Surface,
    theme: &Theme,
    rect: Rect,
    label: &[u8],
    scale: usize,
    selected: bool,
) {
    let (fill, ink) = if selected {
        (theme.inverted_surface(), theme.on_inverted_surface())
    } else {
        (theme.surface_container_high, theme.on_surface)
    };
    fill_rounded_rect(surface, rect, theme.radii.pill, fill);
    let label_h = font::line_height(scale);
    let text_w = font::measure(label, scale);
    let x = if text_w >= rect.w {
        rect.x
    } else {
        rect.x + (rect.w - text_w) / 2
    };
    let y = rect.y + rect.h.saturating_sub(label_h) / 2;
    draw_label(surface, x, y, scale, label, ink);
}

/// The three treatments a button can carry.
///
/// WHY these are named rather than left to the app: the reference designs
/// give a screen exactly one filled control, and an app that is left to
/// choose for itself fills two of them and the screen stops having a
/// primary action.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonKind {
    /// The one control on the screen a person presses. Filled with the
    /// opposite of the page.
    Primary,
    /// A control that sits beside the primary one. A surface and a hairline.
    Secondary,
    /// A control with no fill at all.
    Ghost,
}

/// The platform's button treatment: the fill, the hairline, the radius and
/// the ink the label is drawn in.
///
/// WHY it is a struct and not loose values: a button is one shape, and an app
/// that reached for the fill from one place and the radius from another would
/// draw a button that does not match the platform's. The guest apps paint one
/// pixel at a time, so the *treatment* is the part that has to be shared.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ButtonStyle {
    /// The fill the button is painted with.
    pub fill: u32,
    /// The hairline the button is outlined with, if it has one.
    pub border: Option<u32>,
    /// The corner radius the button's corners are drawn at.
    pub radius: usize,
    /// The ink the label is drawn in.
    pub ink: u32,
}

/// Resolves the platform's button treatment.
///
/// WHY the primary is the opposite of the page and not the theme's accent:
/// the platform's light theme carries a blue accent, and a screen with a
/// blue button on it reads as a marketing page. The design references put
/// the one filled control in near-black, and an app that wanted that had
/// nowhere to ask for it.
pub const fn button_style(theme: &Theme, kind: ButtonKind) -> ButtonStyle {
    match kind {
        ButtonKind::Primary => ButtonStyle {
            fill: theme.inverted_surface(),
            border: None,
            radius: theme.radii.control,
            ink: theme.on_inverted_surface(),
        },
        ButtonKind::Secondary => ButtonStyle {
            fill: theme.surface,
            border: Some(theme.border),
            radius: theme.radii.control,
            ink: theme.on_surface,
        },
        ButtonKind::Ghost => ButtonStyle {
            fill: theme.surface,
            border: None,
            radius: theme.radii.control,
            ink: theme.on_surface,
        },
    }
}

/// Draws a button: a rounded rectangle, a label, and the treatment the
/// button kind calls for.
pub fn button(
    surface: &mut Surface,
    theme: &Theme,
    rect: Rect,
    label: &[u8],
    scale: usize,
    kind: ButtonKind,
) {
    let style = button_style(theme, kind);
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
    let label_h = font::line_height(scale);
    let text_w = font::measure(label, scale);
    let x = if text_w >= rect.w {
        rect.x
    } else {
        rect.x + (rect.w - text_w) / 2
    };
    let y = rect.y + rect.h.saturating_sub(label_h) / 2;
    draw_label(surface, x, y, scale, label, style.ink);
}

/// The platform's segmented-control treatment for one segment.
///
/// WHY a selected segment is the opposite of the page: the design references
/// the apps are built from put the current segment in near-black, and that is
/// the only way a row of otherwise identical chips says which one is
/// current. The old treatment -- the same fill for every segment and a
/// different ink for the selected one -- made the selection a matter of
/// reading rather than of seeing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SegmentStyle {
    /// The fill the segment is painted with.
    pub fill: u32,
    /// The ink the segment's label is drawn in.
    pub ink: u32,
    /// The corner radius the segment's corners are drawn at.
    pub radius: usize,
}

/// Resolves the platform's treatment for one segment of a segmented control.
pub const fn segment_style(theme: &Theme, selected: bool) -> SegmentStyle {
    if selected {
        SegmentStyle {
            fill: theme.inverted_surface(),
            ink: theme.on_inverted_surface(),
            radius: theme.radii.pill,
        }
    } else {
        SegmentStyle {
            fill: theme.surface_container_high,
            ink: theme.on_surface,
            radius: theme.radii.pill,
        }
    }
}

/// Draws a segmented control: a row of pills, one of them selected.
///
/// WHY the segments are separate pills and not one track with a moving
/// selection: a moving selection is an animation, and the platform has no
/// animation clock an app may rely on. Separate pills say the same thing
/// without one.
pub fn segmented_control(
    surface: &mut Surface,
    theme: &Theme,
    bounds: Rect,
    labels: &[&[u8]],
    selected: usize,
    scale: usize,
) {
    if labels.is_empty() {
        return;
    }
    let gap = theme.spacing.sm;
    let count = labels.len();
    let total_gap = gap * (count - 1);
    let segment_w = bounds.w.saturating_sub(total_gap) / count;
    for (index, label) in labels.iter().enumerate() {
        let x = bounds.x + index * (segment_w + gap);
        let rect = Rect::new(x, bounds.y, segment_w, bounds.h);
        pill(surface, theme, rect, label, scale, index == selected);
    }
}

/// Draws an app-icon tile: a rounded square with a glyph in it and a label
/// beneath.
///
/// WHY the glyph is a closure and not a bitmap: the platform has no icon
/// format, and an app's icons are its own art. A closure lets the app paint
/// whatever it draws into the tile, at whatever resolution it holds, and still
/// get the platform's tile, its corner radius and its label placement.
///
/// WHY the label is drawn by the tile and not left to the app: a label that
/// sits one pixel lower in one app is a label that reads as a different app.
pub fn app_icon_tile(
    surface: &mut Surface,
    theme: &Theme,
    rect: Rect,
    glyph: &dyn Fn(&mut Surface, usize, usize),
    label: &[u8],
    scale: usize,
) {
    let tile = Rect::new(rect.x, rect.y, rect.w, rect.h);
    surface.draw_aa_rounded_rect(
        tile.x,
        tile.y,
        tile.w,
        tile.h,
        theme.radii.small,
        theme.surface_container,
        Some(theme.border),
        1,
    );
    glyph(surface, tile.x, tile.y);

    let text_w = font::measure(label, scale);
    let x = if text_w >= rect.w {
        rect.x
    } else {
        rect.x + (rect.w - text_w) / 2
    };
    let y = rect.y + rect.h + theme.spacing.xs;
    draw_label(surface, x, y, scale, label, theme.on_surface);
}

/// One row of a list.
#[derive(Clone, Copy)]
pub struct ListRow<'a> {
    /// The row's title, drawn at the body's scale.
    pub title: &'a [u8],
    /// A second line under the title, drawn at the caption's scale.
    pub subtitle: Option<&'a [u8]>,
    /// A value at the trailing edge, drawn at the caption's scale.
    pub trailing: Option<&'a [u8]>,
    /// Whether the row is the selected one.
    pub selected: bool,
}

impl<'a> ListRow<'a> {
    /// A row with a title and nothing else.
    pub const fn new(title: &'a [u8]) -> Self {
        ListRow {
            title,
            subtitle: None,
            trailing: None,
            selected: false,
        }
    }

    /// A row with a second line under the title.
    pub const fn with_subtitle(mut self, subtitle: &'a [u8]) -> Self {
        self.subtitle = Some(subtitle);
        self
    }

    /// A row with a value at its trailing edge.
    pub const fn with_trailing(mut self, trailing: &'a [u8]) -> Self {
        self.trailing = Some(trailing);
        self
    }

    /// A row that is the selected one.
    pub const fn selected(mut self) -> Self {
        self.selected = true;
        self
    }
}

/// Draws a list row: a title, an optional subtitle, an optional trailing value
/// and a hairline divider under the row.
///
/// WHY the divider is drawn by the row and not by the list: a divider that
/// belongs to the list is a divider that the last row also draws, and the last
/// row of a list is the one that shows the edge of the list.
pub fn list_row(surface: &mut Surface, theme: &Theme, rect: Rect, row: &ListRow<'_>) {
    let style = list_row_style(theme, row.selected);
    surface.draw_aa_rounded_rect(
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        style.radius,
        style.fill,
        None,
        0,
    );

    let pad = style.pad;
    let title_scale = theme.type_scale.body.scale;
    let caption_scale = theme.type_scale.caption.scale;
    let title_h = font::line_height(title_scale);
    let caption_h = font::line_height(caption_scale);

    let title_y = if row.subtitle.is_some() {
        rect.y + (rect.h.saturating_sub(title_h + caption_h)) / 2
    } else {
        rect.y + (rect.h.saturating_sub(title_h)) / 2
    };
    draw_label(
        surface,
        rect.x + pad,
        title_y,
        title_scale,
        row.title,
        theme.on_surface,
    );

    if let Some(subtitle) = row.subtitle {
        draw_label(
            surface,
            rect.x + pad,
            title_y + title_h,
            caption_scale,
            subtitle,
            theme.muted,
        );
    }

    if let Some(trailing) = row.trailing {
        let w = font::measure(trailing, caption_scale);
        let x = if w >= rect.w.saturating_sub(pad * 2) {
            rect.x + pad
        } else {
            rect.x + rect.w - pad - w
        };
        let y = rect.y + (rect.h.saturating_sub(caption_h)) / 2;
        draw_label(surface, x, y, caption_scale, trailing, theme.on_surface_variant);
    }

    // The divider sits on the last row of the surface, so a list that ends
    // there shows its edge rather than a line under nothing.
    let divider_y = rect.y + rect.h - 1;
    if divider_y < rect.y {
        return;
    }
    for x in rect.x..rect.x + rect.w {
        if let Some(existing) = surface.get(x, divider_y) {
            let blended = Theme::blend(existing, style.divider, 192);
            surface.set(x, divider_y, blended);
        }
    }
}

/// Draws a run of text into the surface, blended over whatever is already
/// there.
///
/// WHY the outline face is preferred at scale 2 and above: the bitmap face is
/// the fallback for the smallest step, and the platform's own screens draw
/// their body copy with the outline face, so a label drawn here has to be the
/// same letterforms or it reads as a different app.
fn draw_label(surface: &mut Surface, x: usize, y: usize, scale: usize, text: &[u8], ink: u32) {
    if text.is_empty() || scale == 0 {
        return;
    }
    let w = font::measure(text, scale);
    let h = font::line_height(scale);
    if w == 0 || h == 0 {
        return;
    }
    for py in y..y + h {
        for px in x..x + w {
            let Some(bg) = surface.get(px, py) else {
                continue;
            };
            if let Some(color) = label_pixel(px, py, x, y, scale, text, ink, bg) {
                surface.set(px, py, color);
            }
        }
    }
}

/// The colour of (`px`, `py`) if the label inks it, blended over `bg`.
fn label_pixel(
    px: usize,
    py: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    ink: u32,
    bg: u32,
) -> Option<u32> {
    if scale >= 2 {
        let em = outline_text::em_for_scale(scale);
        let (ascent, _) = typeface::line_metrics(em);
        let baseline = (y0 + font::BASELINE * scale).max(ascent as usize);
        if let Some(run) = outline_text::OutlineRun::place(x0, baseline, em, text) {
            let (bx, by, bw, bh) = run.bounds();
            if px >= bx && px < bx + bw && py >= by && py < by + bh {
                return run.pixel(px, py, ink, bg);
            }
            return None;
        }
    }
    font::test_text_aa(px, py, x0, y0, scale, text, ink, bg, false)
}

#[cfg(test)]
mod tests {
    use super::{
        app_icon_tile, card, card_style, list_row, list_row_style, pill, segmented_control,
        Elevation, ListRow,
    };
    use crate::design::draw_target::Surface;
    use crate::design::primitives::Rect;
    use crate::design::theme::Theme;

    #[test]
    fn a_card_style_is_the_treatment_the_card_is_drawn_with() {
        let theme = Theme::LIGHT;
        // WHY this matters: a guest app paints one pixel at a time, so the
        // treatment is the only thing it can share with the whole-surface
        // widget. The two must be the same numbers or the app's card and the
        // widget's card are different cards.
        let resting = card_style(&theme, Elevation::Resting);
        assert_eq!(resting.fill, theme.surface_container);
        assert_eq!(resting.border, Some(theme.border));
        assert_eq!(resting.radius, theme.radii.card);

        let raised = card_style(&theme, Elevation::Raised);
        assert_eq!(raised.fill, theme.surface_container_high);
        let pressed = card_style(&theme, Elevation::Pressed);
        assert_eq!(pressed.fill, theme.surface_container_low);
        assert_eq!(pressed.border, None);
    }

    #[test]
    fn a_list_row_style_is_the_treatment_the_row_is_drawn_with() {
        let theme = Theme::LIGHT;
        let resting = list_row_style(&theme, false);
        assert_eq!(resting.fill, theme.surface);
        assert_eq!(resting.divider, theme.outline_variant);
        assert_eq!(resting.pad, theme.spacing.md);
        assert_eq!(resting.radius, theme.radii.card);

        let selected = list_row_style(&theme, true);
        assert_eq!(selected.fill, theme.surface_container_high);
    }

    fn paint(w: usize, h: usize, f: impl FnOnce(&mut Surface<'_>)) -> [u32; 16384] {
        let mut pixels = [0u32; 16384];
        {
            let mut surface = Surface::new(&mut pixels[..w * h], w, h).expect("canvas fits");
            f(&mut surface);
        }
        pixels
    }

    #[test]
    fn a_card_is_filled_and_outlined_with_the_theme_hairline() {
        let theme = Theme::LIGHT;
        // WHY the card is 60x40 and not square: the radius is clamped to half
        // the shape's shorter side, so a square card at the card radius is a
        // circle and has no straight edge to check the hairline on.
        let pixels = paint(80, 60, |surface| {
            card(
                surface,
                &theme,
                Rect::new(10, 10, 60, 40),
                Elevation::Resting,
            );
        });
        // The middle of the card is the container fill.
        assert_eq!(pixels[30 * 80 + 40], theme.surface_container);
        // The hairline is on the straight part of the edge.
        assert_eq!(pixels[10 * 80 + 40], theme.border);
        // The corner is rounded away, and the card did not spill outside itself.
        assert_eq!(pixels[10 * 80 + 10], 0);
        assert_eq!(pixels[0], 0);
    }

    #[test]
    fn a_pressed_card_sits_below_a_resting_one() {
        let theme = Theme::LIGHT;
        let pixels = paint(80, 60, |surface| {
            card(
                surface,
                &theme,
                Rect::new(10, 10, 60, 40),
                Elevation::Pressed,
            );
        });
        assert_eq!(pixels[30 * 80 + 40], theme.surface_container_low);
    }

    #[test]
    fn a_pill_inks_its_label() {
        let theme = Theme::LIGHT;
        let pixels = paint(64, 24, |surface| {
            pill(
                surface,
                &theme,
                Rect::new(2, 2, 60, 20),
                b"Work",
                1,
                false,
            );
        });
        let inked = pixels
            .iter()
            .filter(|p| **p != theme.surface_container_high)
            .count();
        assert!(inked > 0, "the pill drew no label");
    }

    #[test]
    fn a_segmented_control_draws_one_pill_per_label() {
        let theme = Theme::LIGHT;
        let pixels = paint(120, 24, |surface| {
            segmented_control(
                surface,
                &theme,
                Rect::new(0, 0, 120, 24),
                &[b"All", b"Work"],
                1,
                1,
            );
        });
        // The first segment is not selected, so it is inked with the body ink.
        let first_ink = pixels.iter().filter(|p| **p == theme.on_surface).count();
        assert!(first_ink > 0, "the unselected segment drew no label");
    }

    #[test]
    fn an_icon_tile_paints_the_glyph_and_the_label() {
        let theme = Theme::LIGHT;
        let pixels = paint(48, 72, |surface| {
            app_icon_tile(
                surface,
                &theme,
                Rect::new(4, 4, 40, 40),
                &|surface, x, y| {
                    surface.set(x + 4, y + 4, theme.primary);
                },
                b"Files",
                1,
            );
        });
        assert!(
            pixels.contains(&theme.primary),
            "the tile did not paint the glyph"
        );
        assert!(
            pixels.contains(&theme.surface_container),
            "the tile has no fill"
        );
    }

    #[test]
    fn a_list_row_draws_its_title_and_its_divider() {
        let theme = Theme::LIGHT;
        let pixels = paint(80, 40, |surface| {
            let row = ListRow::new(b"Storage").with_trailing(b"42 GB");
            list_row(surface, &theme, Rect::new(0, 0, 80, 40), &row);
        });
        assert!(
            pixels.iter().any(|p| *p == theme.on_surface),
            "the row drew no title"
        );
        assert!(
            pixels.iter().any(|p| *p != theme.surface && *p != theme.on_surface),
            "the row drew no divider"
        );
    }
}
