//! The monochrome design system the InterOS app showcases describe.
//!
//! WHY this module exists: the SDK had tokens, a type scale and a set of
//! widgets, and every application had grown its own answer to "what colour
//! is a card", "how tall is a row", "what does a status dot look like".
//! Two apps drew the same list and the rows did not line up, and a third
//! app that wanted to look like the others had to read another app's source
//! to find out what the numbers were. That is not a design system; it is a
//! coincidence that has not been written down yet.
//!
//! The showcases in `Ui Samples/apps` all describe the *same* system in the
//! *same* words: a background of `#FAFAFA`, a surface of `#FFFFFF`, a hairline
//! of `#E5E5E5`, text that is `#0A0A0A` or `#6B6B6B` or `#A3A3A3`, and colour
//! reserved for status -- `#22C55E` for good, `#F59E0B` for a warning,
//! `#EF4444` for an error. The type scale is a ladder of cap heights and the
//! spacing is a ladder of four-pixel steps. This module writes that down once
//! so that an application can be *right* rather than merely *consistent*.
//!
//! WHY the widgets are per-pixel and not drawn: the guest applications paint
//! a frame one pixel at a time, because a surface write is capped at 4,096
//! pixels and a whole frame does not fit in one call. A widget that only
//! worked on a whole [`super::draw_target::Surface`] would be a widget that
//! the guest applications could not use. Every helper here answers one
//! question -- "what colour is this pixel?" -- and returns `None` when the
//! pixel is not the widget's, so the caller keeps whatever it had.
//!
//! WHY nothing here allocates: the SDK is `#![no_std]` with no allocator, and
//! every widget here is a value or a function over values the caller owns.

use super::outline_text;
use super::primitives::Rect;
use super::typeface;

// ---------------------------------------------------------------------------
// The palette
// ---------------------------------------------------------------------------

/// The background the whole screen sits on.
pub const BG: u32 = 0xFFFA_FAFA;
/// A card, a sheet, a bar: the white the platform's surfaces are.
pub const SURFACE: u32 = 0xFF_FFFF;
/// A surface that is a step below white: an icon tile, a chip, a field.
pub const SURFACE_MUTED: u32 = 0xFFF2_F2F2;
/// The hairline that separates two surfaces.
pub const BORDER: u32 = 0xFFE5_E5E5;
/// The text a reader reads first.
pub const TEXT_PRIMARY: u32 = 0xFF0A_0A0A;
/// The text a reader reads second.
pub const TEXT_SECONDARY: u32 = 0xFF6B_6B6B;
/// The text that is a whisper: a timestamp, a hint.
pub const TEXT_MUTED: u32 = 0xFFA3_A3A3;
/// A state that is good.
pub const SUCCESS: u32 = 0xFF22_C55E;
/// A state that wants attention.
pub const WARNING: u32 = 0xFFF5_9E0B;
/// A state that is a failure.
pub const ERROR: u32 = 0xFFEF_4444;
/// The text on a filled, dark surface.
pub const ON_PRIMARY: u32 = 0xFF_FFFF;

// ---------------------------------------------------------------------------
// The type scale
// ---------------------------------------------------------------------------

/// The cap height of a caption.
pub const CAP_CAPTION: u32 = super::typography::CAPTION_CAP_PX;
/// The cap height of body text.
pub const CAP_BODY: u32 = super::typography::BODY_CAP_PX;
/// The cap height of a section heading.
pub const CAP_SECTION: u32 = super::typography::SECTION_CAP_PX;
/// The cap height of a title.
pub const CAP_TITLE: u32 = super::typography::TITLE_CAP_PX;
/// The cap height of a page heading.
pub const CAP_HEADING: u32 = super::typography::HEADING_CAP_PX;

/// The line box of a caption.
pub const LINE_CAPTION: usize = super::typography::CAPTION_LINE_PX;
/// The line box of body text.
pub const LINE_BODY: usize = super::typography::BODY_LINE_PX;
/// The line box of a section heading.
pub const LINE_SECTION: usize = super::typography::SECTION_LINE_PX;
/// The line box of a title.
pub const LINE_TITLE: usize = super::typography::TITLE_LINE_PX;
/// The line box of a page heading.
pub const LINE_HEADING: usize = super::typography::HEADING_LINE_PX;

// ---------------------------------------------------------------------------
// The spacing ladder
// ---------------------------------------------------------------------------

/// Four pixels.
pub const SPACE_XS: usize = 4;
/// Eight pixels.
pub const SPACE_SM: usize = 8;
/// Twelve pixels.
pub const SPACE_MD: usize = 12;
/// Sixteen pixels.
pub const SPACE_LG: usize = 16;
/// Twenty-four pixels.
pub const SPACE_XL: usize = 24;
/// Thirty-two pixels.
pub const SPACE_XXL: usize = 32;

// ---------------------------------------------------------------------------
// The radii
// ---------------------------------------------------------------------------

/// The corner radius of a card.
pub const RADIUS_CARD: usize = 16;
/// The corner radius of a field, a tile or a control.
pub const RADIUS_CONTROL: usize = 12;
/// The corner radius of a chip or a button.
pub const RADIUS_PILL: usize = 24;
/// The corner radius of the screen itself.
pub const RADIUS_SCREEN: usize = 28;

// ---------------------------------------------------------------------------
// The theme
// ---------------------------------------------------------------------------

/// The theme the app showcases describe.
///
/// WHY this is in the design system and not in each application: the
/// showcases all describe the same system in the same words, and an
/// application that carries its own copy of the palette is one more copy
/// to keep in step. An application that wants its own name writes
/// `Theme { name: "notes", ..monochrome::THEME }` and gets the same
/// pixels every other application gets.
pub const THEME: super::theme::Theme = super::theme::Theme {
    name: "monochrome",
    dark: false,

    // The page and the surfaces on it.
    surface: BG,
    surface_container: SURFACE,
    surface_container_high: SURFACE,
    surface_container_low: SURFACE_MUTED,
    surface_frosted: SURFACE,
    overlay: SURFACE,
    overlay_border: BORDER,

    // The ink.
    on_surface: TEXT_PRIMARY,
    on_surface_variant: TEXT_SECONDARY,
    muted: TEXT_MUTED,
    on_primary: ON_PRIMARY,
    on_primary_container: TEXT_PRIMARY,
    ink_on_light: TEXT_PRIMARY,
    ink_on_dark: ON_PRIMARY,

    // The hairlines.
    outline: TEXT_PRIMARY,
    outline_variant: BORDER,
    border: BORDER,

    // The primary is black, because the one filled control on a screen is
    // the one a person presses.
    primary: TEXT_PRIMARY,
    primary_container: SURFACE_MUTED,
    secondary: TEXT_SECONDARY,
    accent: TEXT_PRIMARY,

    // The three status colours, and only these three.
    error: ERROR,
    error_container: 0xFFFDECEC,
    success: SUCCESS,
    success_container: 0xFFEAF9EF,
    on_success_container: 0xFF15803D,
    warning: WARNING,
    warning_container: 0xFFFEF6E7,
    emergency: ERROR,

    // The app draws no wallpaper of its own.
    wallpaper_top: BG,
    wallpaper_mid: SURFACE_MUTED,
    wallpaper_bottom: 0xFFEDEDED,
    wallpaper_ink: TEXT_PRIMARY,

    type_scale: super::theme::TypeScale::STANDARD,
    spacing: super::theme::Spacing::STANDARD,
    radii: super::theme::Radii::STANDARD,
    touch: super::theme::Touch::STANDARD,
};

// ---------------------------------------------------------------------------
// The frame
// ---------------------------------------------------------------------------

/// The width of the guest's design space.
pub const W: usize = 720;
/// The height of the guest's design space.
pub const H: usize = 1440;
/// The height of the status bar's band.
pub const STATUS_H: usize = 84;
/// The margin a screen's content keeps from the edge.
pub const GUTTER: usize = 24;
/// The width of the content column.
pub const CONTENT_W: usize = W - 2 * GUTTER;

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// The colour of `(x, y)` if the run inks it, blended over `bg`.
///
/// WHY the outline face and not the bitmap face: the guest's canvas is
/// 720 by 1440 and the panel is 1080 by 2160, so the compositor magnifies
/// every run by three in width and two in height. A bitmap glyph magnified
/// by three is a soft glyph, and most of the text on these screens is small.
/// The outline face is a real outline at the screen's own resolution, so it
/// stays sharp at any size.
///
/// WHY the bitmap face is still here: a run the outline face refuses -- a
/// character the typeface has no outline for -- has to be drawn somehow, and
/// the bitmap face is what it is drawn with.
pub fn text_px(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    cap_px: u32,
    text: &[u8],
    ink: u32,
    bg: u32,
    bold: bool,
) -> Option<u32> {
    if text.is_empty() || cap_px == 0 {
        return None;
    }
    if let Some(run) = outline_text::place_run_cap(x0, y0, cap_px, text, None, bold) {
        let (bx, by, bw, bh) = run.bounds();
        if x >= bx && x < bx + bw && y >= by && y < by + bh {
            return run.pixel(x, y, ink, bg);
        }
        return None;
    }
    // The bitmap face draws in whole cells, so a run at a cap height it
    // cannot reach is drawn at the nearest cell it has.
    let fallback = (outline_text::em_for_cap_height(cap_px) as usize / crate::font::GLYPH_H).max(1);
    if x < x0 || y < y0 || y >= y0 + crate::font::GLYPH_H * fallback {
        return None;
    }
    crate::font::test_text_aa(x, y, x0, y0, fallback, text, ink, bg, bold)
}

/// The width of `text` as a reader sees it at a cap height.
///
/// WHY the typeface and not the bitmap face: the run is drawn with the
/// outline face, so a caret or a centred label measured with the bitmap's
/// width sits in the wrong place.
pub fn text_width(text: &[u8], cap_px: u32) -> usize {
    if text.is_empty() || cap_px == 0 {
        return 0;
    }
    let em = outline_text::em_for_cap_height(cap_px);
    typeface::text_advance_px(text, em) as usize
}

/// The number of bytes of `text` that fit in `max_px` at a cap height.
///
/// WHY it trims on a byte and not on a character: the face is drawn from
/// bytes, and a run that stops in the middle of a multi-byte character is a
/// run the face refuses.
pub fn fit(text: &[u8], max_px: usize, cap_px: u32) -> usize {
    if text_width(text, cap_px) <= max_px {
        return text.len();
    }
    let mut len = 0usize;
    while len < text.len() && text_width(&text[..len + 1], cap_px) <= max_px {
        len += 1;
    }
    len
}

// ---------------------------------------------------------------------------
// The icons
// ---------------------------------------------------------------------------

/// The monochrome glyphs the showcases use.
///
/// WHY an enum and not a font: a glyph is a shape, and a shape drawn by the
/// platform is the same shape in every application and at every size. A font
/// would make the shape depend on the face, and the face is the one thing
/// the design system is trying to hold still.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    /// A magnifying glass.
    Search,
    /// Three dots in a column.
    More,
    /// Three dots in a row.
    MoreHorizontal,
    /// A plus.
    Plus,
    /// A cross.
    Close,
    /// A tick.
    Check,
    /// A chevron pointing left.
    ChevronLeft,
    /// A chevron pointing right.
    ChevronRight,
    /// A chevron pointing down.
    ChevronDown,
    /// A chevron pointing up.
    ChevronUp,
    /// Three lines: a list.
    List,
    /// Lines with boxes: a checklist.
    Checklist,
    /// A page with a folded corner.
    File,
    /// A folder.
    Folder,
    /// A picture.
    Image,
    /// A camera.
    Camera,
    /// A musical note.
    Music,
    /// A prompt.
    Terminal,
    /// A keypad.
    Calculator,
    /// A calendar.
    Calendar,
    /// A clock.
    Clock,
    /// A globe.
    Globe,
    /// A bell.
    Bell,
    /// A pin.
    Pin,
    /// A padlock.
    Lock,
    /// An open padlock.
    Unlock,
    /// A key.
    Key,
    /// A shield.
    Shield,
    /// An eye.
    Eye,
    /// A share node.
    Share,
    /// Two overlapping pages.
    Copy,
    /// An arrow into a tray.
    Download,
    /// A bin.
    Trash,
    /// A pencil.
    Edit,
    /// A circular arrow.
    Refresh,
    /// A triangle: play.
    Play,
    /// A square: stop.
    Stop,
    /// A filled circle: record.
    Record,
    /// A paper plane: send.
    Send,
    /// A clip.
    Attach,
    /// A bookmark.
    Bookmark,
    /// A pair of people.
    People,
    /// A house.
    Home,
    /// A star.
    Star,
    /// A light bulb.
    Bulb,
    /// A aeroplane.
    Plane,
    /// A box.
    Box,
    /// An archive box.
    Archive,
    /// A microphone.
    Mic,
    /// A gear.
    Settings,
    /// A robot: an agent.
    Agent,
    /// A chip: a model.
    Model,
    /// A pulse: activity.
    Activity,
    /// A grid of squares.
    Grid,
    /// A funnel.
    Filter,
    /// A bookmarked page.
    Note,
    /// A wallet.
    Wallet,
    /// A book.
    Book,
    /// A circle with a bar: a record.
    Circle,
}

/// The size of the square an icon is drawn in.
pub const ICON_BOX: usize = 24;

/// Whether the pixel `(x, y)` is inked by `icon` inside `rect`.
///
/// WHY the geometry is done in a fixed twenty-four unit grid: an icon that
/// was drawn in the pixel grid would be a different shape at every size. The
/// grid is the shape; the rectangle is the size.
pub fn icon_px(x: usize, y: usize, rect: Rect, icon: Icon, ink: u32) -> Option<u32> {
    if rect.w == 0 || rect.h == 0 {
        return None;
    }
    if x < rect.x || x >= rect.x + rect.w || y < rect.y || y >= rect.y + rect.h {
        return None;
    }
    // The pixel's position in the twenty-four unit grid.
    let px = ((x - rect.x) as i32 * 24) / rect.w as i32;
    let py = ((y - rect.y) as i32 * 24) / rect.h as i32;
    if icon_hit(icon, px, py) {
        Some(ink)
    } else {
        None
    }
}

/// Whether the pixel is inked, as a boolean.
pub fn icon_hit(icon: Icon, px: i32, py: i32) -> bool {
    match icon {
        Icon::Search => {
            circle(px, py, 10, 10, 6, false, 2) || seg(px, py, 14, 14, 21, 21, 2)
        }
        Icon::More => dot(px, py, 12, 5, 2) || dot(px, py, 12, 12, 2) || dot(px, py, 12, 19, 2),
        Icon::MoreHorizontal => dot(px, py, 5, 12, 2) || dot(px, py, 12, 12, 2) || dot(px, py, 19, 12, 2),
        Icon::Plus => seg(px, py, 12, 4, 12, 20, 2) || seg(px, py, 4, 12, 20, 12, 2),
        Icon::Close => seg(px, py, 5, 5, 19, 19, 2) || seg(px, py, 19, 5, 5, 19, 2),
        Icon::Check => seg(px, py, 4, 13, 10, 19, 2) || seg(px, py, 10, 19, 20, 6, 2),
        Icon::ChevronLeft => seg(px, py, 15, 5, 8, 12, 2) || seg(px, py, 8, 12, 15, 19, 2),
        Icon::ChevronRight => seg(px, py, 9, 5, 16, 12, 2) || seg(px, py, 16, 12, 9, 19, 2),
        Icon::ChevronDown => seg(px, py, 5, 9, 12, 16, 2) || seg(px, py, 12, 16, 19, 9, 2),
        Icon::ChevronUp => seg(px, py, 5, 15, 12, 8, 2) || seg(px, py, 12, 8, 19, 15, 2),
        Icon::List => {
            dot(px, py, 4, 6, 2)
                || dot(px, py, 4, 12, 2)
                || dot(px, py, 4, 18, 2)
                || seg(px, py, 9, 6, 21, 6, 2)
                || seg(px, py, 9, 12, 21, 12, 2)
                || seg(px, py, 9, 18, 21, 18, 2)
        }
        Icon::Checklist => {
            (square(px, py, 2, 3, 8, 8, 2) && (seg(px, py, 3, 7, 5, 9, 1) || seg(px, py, 5, 9, 9, 4, 1)))
                || (square(px, py, 2, 13, 8, 8, 2))
                || seg(px, py, 13, 7, 22, 7, 2)
                || seg(px, py, 13, 17, 22, 17, 2)
        }
        Icon::File | Icon::Note => {
            seg(px, py, 5, 3, 15, 3, 2)
                || seg(px, py, 15, 3, 19, 7, 2)
                || seg(px, py, 19, 7, 19, 21, 2)
                || seg(px, py, 19, 21, 5, 21, 2)
                || seg(px, py, 5, 21, 5, 3, 2)
                || seg(px, py, 15, 3, 15, 8, 2)
                || seg(px, py, 15, 8, 19, 8, 2)
                || seg(px, py, 8, 13, 16, 13, 2)
                || seg(px, py, 8, 17, 13, 17, 2)
        }
        Icon::Folder => {
            seg(px, py, 3, 6, 10, 6, 2)
                || seg(px, py, 10, 6, 12, 9, 2)
                || seg(px, py, 12, 9, 21, 9, 2)
                || seg(px, py, 21, 9, 21, 19, 2)
                || seg(px, py, 21, 19, 3, 19, 2)
                || seg(px, py, 3, 19, 3, 6, 2)
        }
        Icon::Image => {
            square(px, py, 3, 5, 18, 14, 2)
                || circle(px, py, 8, 9, 2, false, 1)
                || seg(px, py, 4, 18, 10, 11, 2)
                || seg(px, py, 10, 11, 20, 18, 2)
        }
        Icon::Camera => {
            square(px, py, 2, 7, 20, 13, 2)
                || seg(px, py, 8, 7, 9, 4, 2)
                || seg(px, py, 9, 4, 15, 4, 2)
                || seg(px, py, 15, 4, 16, 7, 2)
                || circle(px, py, 12, 13, 4, false, 2)
        }
        Icon::Music => {
            circle(px, py, 7, 18, 3, false, 2)
                || circle(px, py, 17, 16, 3, false, 2)
                || seg(px, py, 10, 18, 10, 5, 2)
                || seg(px, py, 10, 5, 20, 3, 2)
                || seg(px, py, 20, 3, 20, 16, 2)
        }
        Icon::Terminal => {
            square(px, py, 2, 4, 20, 16, 2)
                || seg(px, py, 6, 9, 11, 12, 2)
                || seg(px, py, 11, 12, 6, 15, 2)
                || seg(px, py, 13, 16, 19, 16, 2)
        }
        Icon::Calculator => {
            square(px, py, 4, 2, 16, 20, 2)
                || seg(px, py, 8, 6, 16, 6, 2)
                || square(px, py, 7, 10, 3, 3, 1)
                || square(px, py, 13, 10, 3, 3, 1)
                || square(px, py, 7, 15, 3, 3, 1)
                || square(px, py, 13, 15, 3, 3, 1)
        }
        Icon::Calendar => {
            square(px, py, 3, 5, 18, 16, 2)
                || seg(px, py, 3, 10, 21, 10, 2)
                || seg(px, py, 8, 2, 8, 7, 2)
                || seg(px, py, 16, 2, 16, 7, 2)
                || dot(px, py, 8, 14, 1)
                || dot(px, py, 12, 14, 1)
                || dot(px, py, 16, 14, 1)
                || dot(px, py, 8, 18, 1)
        }
        Icon::Clock => {
            circle(px, py, 12, 12, 9, false, 2)
                || seg(px, py, 12, 6, 12, 12, 2)
                || seg(px, py, 12, 12, 16, 14, 2)
        }
        Icon::Globe => {
            circle(px, py, 12, 12, 9, false, 2)
                || seg(px, py, 3, 12, 21, 12, 2)
                || seg(px, py, 6, 6, 18, 6, 1)
                || seg(px, py, 6, 18, 18, 18, 1)
        }
        Icon::Bell => {
            seg(px, py, 6, 17, 6, 10, 2)
                || seg(px, py, 6, 10, 9, 5, 2)
                || seg(px, py, 9, 5, 15, 5, 2)
                || seg(px, py, 15, 5, 18, 10, 2)
                || seg(px, py, 18, 10, 18, 17, 2)
                || seg(px, py, 4, 17, 20, 17, 2)
                || dot(px, py, 12, 20, 2)
        }
        Icon::Pin => {
            seg(px, py, 9, 3, 15, 3, 2)
                || seg(px, py, 15, 3, 15, 9, 2)
                || seg(px, py, 15, 9, 12, 15, 2)
                || seg(px, py, 12, 15, 12, 21, 2)
                || seg(px, py, 12, 21, 9, 15, 2)
                || seg(px, py, 9, 15, 6, 9, 2)
                || seg(px, py, 6, 9, 6, 3, 2)
                || seg(px, py, 6, 9, 15, 9, 2)
        }
        Icon::Lock => {
            square(px, py, 5, 11, 14, 10, 2)
                || seg(px, py, 8, 11, 8, 7, 2)
                || seg(px, py, 8, 7, 12, 4, 2)
                || seg(px, py, 12, 4, 16, 7, 2)
                || seg(px, py, 16, 7, 16, 11, 2)
                || dot(px, py, 12, 16, 2)
        }
        Icon::Unlock => {
            square(px, py, 5, 11, 14, 10, 2)
                || seg(px, py, 8, 11, 8, 7, 2)
                || seg(px, py, 8, 7, 12, 4, 2)
                || seg(px, py, 12, 4, 16, 7, 2)
                || seg(px, py, 16, 7, 16, 10, 2)
                || dot(px, py, 12, 16, 2)
        }
        Icon::Key => {
            circle(px, py, 8, 9, 5, false, 2)
                || seg(px, py, 12, 13, 21, 21, 2)
                || seg(px, py, 18, 18, 15, 21, 2)
        }
        Icon::Shield => {
            seg(px, py, 12, 2, 20, 5, 2)
                || seg(px, py, 20, 5, 20, 12, 2)
                || seg(px, py, 20, 12, 12, 22, 2)
                || seg(px, py, 12, 22, 4, 12, 2)
                || seg(px, py, 4, 12, 4, 5, 2)
                || seg(px, py, 4, 5, 12, 2, 2)
        }
        Icon::Eye => {
            seg(px, py, 2, 12, 8, 6, 2)
                || seg(px, py, 8, 6, 16, 6, 2)
                || seg(px, py, 16, 6, 22, 12, 2)
                || seg(px, py, 22, 12, 16, 18, 2)
                || seg(px, py, 16, 18, 8, 18, 2)
                || seg(px, py, 8, 18, 2, 12, 2)
                || circle(px, py, 12, 12, 4, false, 2)
        }
        Icon::Share => {
            circle(px, py, 17, 5, 3, false, 2)
                || circle(px, py, 6, 12, 3, false, 2)
                || circle(px, py, 17, 19, 3, false, 2)
                || seg(px, py, 9, 11, 14, 6, 2)
                || seg(px, py, 9, 13, 14, 18, 2)
        }
        Icon::Copy => {
            square(px, py, 8, 3, 13, 14, 2)
                || seg(px, py, 5, 8, 3, 8, 2)
                || seg(px, py, 3, 8, 3, 21, 2)
                || seg(px, py, 3, 21, 16, 21, 2)
        }
        Icon::Download => {
            seg(px, py, 12, 3, 12, 15, 2)
                || seg(px, py, 7, 10, 12, 15, 2)
                || seg(px, py, 17, 10, 12, 15, 2)
                || seg(px, py, 4, 20, 20, 20, 2)
        }
        Icon::Trash => {
            seg(px, py, 4, 6, 20, 6, 2)
                || seg(px, py, 9, 3, 15, 3, 2)
                || seg(px, py, 6, 6, 7, 21, 2)
                || seg(px, py, 7, 21, 17, 21, 2)
                || seg(px, py, 17, 21, 18, 6, 2)
                || seg(px, py, 10, 10, 10, 17, 1)
                || seg(px, py, 14, 10, 14, 17, 1)
        }
        Icon::Edit => {
            seg(px, py, 4, 20, 4, 16, 2)
                || seg(px, py, 4, 20, 8, 20, 2)
                || seg(px, py, 8, 20, 19, 9, 2)
                || seg(px, py, 19, 9, 15, 5, 2)
                || seg(px, py, 15, 5, 4, 16, 2)
        }
        Icon::Refresh => {
            circle(px, py, 12, 12, 8, false, 2)
                || seg(px, py, 12, 1, 17, 6, 2)
                || seg(px, py, 17, 6, 12, 6, 2)
        }
        Icon::Play => {
            seg(px, py, 8, 5, 8, 19, 2)
                || seg(px, py, 8, 5, 18, 12, 2)
                || seg(px, py, 18, 12, 8, 19, 2)
        }
        Icon::Stop => square(px, py, 7, 7, 10, 10, 2),
        Icon::Record => circle(px, py, 12, 12, 7, true, 0),
        Icon::Send => {
            // WHY the dart and not the chevron: a chevron pointing left
            // reads as "back", and a control that sends reads as "back" if
            // the shape is the one the platform already uses for going
            // back.
            seg(px, py, 3, 11, 21, 4, 2)
                || seg(px, py, 21, 4, 10, 20, 2)
                || seg(px, py, 10, 20, 13, 12, 2)
                || seg(px, py, 13, 12, 3, 11, 2)
        }
        Icon::Attach => {
            circle(px, py, 7, 16, 3, false, 2)
                || seg(px, py, 9, 14, 17, 6, 2)
                || seg(px, py, 17, 6, 19, 8, 2)
                || seg(px, py, 19, 8, 11, 16, 2)
                || seg(px, py, 11, 16, 4, 9, 2)
                || seg(px, py, 4, 9, 6, 7, 2)
                || seg(px, py, 6, 7, 8, 9, 2)
        }
        Icon::Bookmark => {
            seg(px, py, 6, 3, 18, 3, 2)
                || seg(px, py, 6, 3, 6, 21, 2)
                || seg(px, py, 18, 3, 18, 21, 2)
                || seg(px, py, 6, 21, 12, 15, 2)
                || seg(px, py, 12, 15, 18, 21, 2)
        }
        Icon::People => {
            circle(px, py, 9, 8, 4, false, 2)
                || seg(px, py, 2, 21, 2, 18, 2)
                || seg(px, py, 2, 18, 7, 14, 2)
                || seg(px, py, 7, 14, 11, 14, 2)
                || seg(px, py, 11, 14, 16, 18, 2)
                || seg(px, py, 16, 18, 16, 21, 2)
                || circle(px, py, 17, 9, 3, false, 2)
        }
        Icon::Home => {
            seg(px, py, 3, 11, 12, 3, 2)
                || seg(px, py, 12, 3, 21, 11, 2)
                || seg(px, py, 6, 10, 6, 21, 2)
                || seg(px, py, 6, 21, 18, 21, 2)
                || seg(px, py, 18, 21, 18, 10, 2)
                || seg(px, py, 10, 21, 10, 15, 2)
                || seg(px, py, 10, 15, 14, 15, 2)
                || seg(px, py, 14, 15, 14, 21, 2)
        }
        Icon::Star => {
            seg(px, py, 12, 3, 15, 9, 2)
                || seg(px, py, 15, 9, 21, 10, 2)
                || seg(px, py, 21, 10, 17, 15, 2)
                || seg(px, py, 17, 15, 18, 21, 2)
                || seg(px, py, 18, 21, 12, 18, 2)
                || seg(px, py, 12, 18, 6, 21, 2)
                || seg(px, py, 6, 21, 7, 15, 2)
                || seg(px, py, 7, 15, 3, 10, 2)
                || seg(px, py, 3, 10, 9, 9, 2)
                || seg(px, py, 9, 9, 12, 3, 2)
        }
        Icon::Bulb => {
            circle(px, py, 12, 10, 6, false, 2)
                || seg(px, py, 9, 17, 15, 17, 2)
                || seg(px, py, 10, 20, 14, 20, 2)
        }
        Icon::Plane => {
            seg(px, py, 2, 13, 22, 11, 2)
                || seg(px, py, 9, 8, 11, 2, 2)
                || seg(px, py, 11, 2, 13, 8, 2)
                || seg(px, py, 10, 13, 8, 21, 2)
                || seg(px, py, 14, 13, 16, 21, 2)
        }
        Icon::Box => {
            seg(px, py, 12, 3, 21, 8, 2)
                || seg(px, py, 21, 8, 21, 17, 2)
                || seg(px, py, 21, 17, 12, 21, 2)
                || seg(px, py, 12, 21, 3, 17, 2)
                || seg(px, py, 3, 17, 3, 8, 2)
                || seg(px, py, 3, 8, 12, 3, 2)
                || seg(px, py, 12, 3, 12, 21, 1)
                || seg(px, py, 3, 8, 21, 8, 1)
        }
        Icon::Archive => {
            square(px, py, 3, 4, 18, 4, 2)
                || seg(px, py, 4, 8, 4, 20, 2)
                || seg(px, py, 4, 20, 20, 20, 2)
                || seg(px, py, 20, 20, 20, 8, 2)
                || seg(px, py, 4, 8, 20, 8, 2)
                || seg(px, py, 9, 12, 15, 12, 2)
        }
        Icon::Mic => {
            square(px, py, 9, 3, 6, 11, 2)
                || seg(px, py, 6, 11, 6, 14, 2)
                || seg(px, py, 6, 14, 9, 16, 2)
                || seg(px, py, 9, 16, 12, 16, 2)
                || seg(px, py, 12, 16, 15, 14, 2)
                || seg(px, py, 15, 11, 15, 14, 2)
                || seg(px, py, 12, 16, 12, 21, 2)
                || seg(px, py, 8, 21, 16, 21, 2)
        }
        Icon::Settings => {
            circle(px, py, 12, 12, 4, false, 2)
                || seg(px, py, 12, 2, 12, 5, 2)
                || seg(px, py, 12, 19, 12, 22, 2)
                || seg(px, py, 2, 12, 5, 12, 2)
                || seg(px, py, 19, 12, 22, 12, 2)
                || seg(px, py, 5, 5, 7, 7, 2)
                || seg(px, py, 17, 17, 19, 19, 2)
                || seg(px, py, 19, 5, 17, 7, 2)
                || seg(px, py, 7, 17, 5, 19, 2)
        }
        Icon::Agent => {
            square(px, py, 4, 7, 16, 13, 2)
                || dot(px, py, 9, 13, 2)
                || dot(px, py, 15, 13, 2)
                || seg(px, py, 12, 2, 12, 7, 2)
                || dot(px, py, 12, 2, 2)
                || seg(px, py, 2, 12, 4, 12, 2)
                || seg(px, py, 20, 12, 22, 12, 2)
        }
        Icon::Model => {
            square(px, py, 6, 6, 12, 12, 2)
                || seg(px, py, 9, 3, 9, 6, 2)
                || seg(px, py, 15, 3, 15, 6, 2)
                || seg(px, py, 9, 18, 9, 21, 2)
                || seg(px, py, 15, 18, 15, 21, 2)
                || seg(px, py, 3, 9, 6, 9, 2)
                || seg(px, py, 3, 15, 6, 15, 2)
                || seg(px, py, 18, 9, 21, 9, 2)
                || seg(px, py, 18, 15, 21, 15, 2)
        }
        Icon::Activity => {
            seg(px, py, 2, 12, 8, 12, 2)
                || seg(px, py, 8, 12, 11, 5, 2)
                || seg(px, py, 11, 5, 14, 19, 2)
                || seg(px, py, 14, 19, 17, 12, 2)
                || seg(px, py, 17, 12, 22, 12, 2)
        }
        Icon::Grid => {
            square(px, py, 3, 3, 8, 8, 2)
                || square(px, py, 13, 3, 8, 8, 2)
                || square(px, py, 3, 13, 8, 8, 2)
                || square(px, py, 13, 13, 8, 8, 2)
        }
        Icon::Filter => {
            seg(px, py, 3, 5, 21, 5, 2)
                || seg(px, py, 21, 5, 13, 13, 2)
                || seg(px, py, 13, 13, 13, 21, 2)
                || seg(px, py, 3, 5, 11, 13, 2)
                || seg(px, py, 11, 13, 11, 19, 2)
        }
        Icon::Wallet => {
            square(px, py, 3, 6, 18, 14, 2)
                || seg(px, py, 3, 11, 21, 11, 2)
                || dot(px, py, 16, 15, 2)
        }
        Icon::Book => {
            seg(px, py, 4, 4, 4, 20, 2)
                || seg(px, py, 4, 4, 12, 4, 2)
                || seg(px, py, 12, 4, 12, 20, 2)
                || seg(px, py, 12, 20, 4, 20, 2)
                || seg(px, py, 6, 8, 10, 8, 1)
                || seg(px, py, 6, 12, 10, 12, 1)
        }
        Icon::Circle => circle(px, py, 12, 12, 8, false, 2),
    }
}

/// Whether the pixel is within `t` of the segment.
fn seg(px: i32, py: i32, x0: i32, y0: i32, x1: i32, y1: i32, t: i32) -> bool {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len2 = dx * dx + dy * dy;
    if len2 == 0 {
        let ddx = px - x0;
        let ddy = py - y0;
        return ddx * ddx + ddy * ddy <= t * t;
    }
    let mut u = ((px - x0) * dx + (py - y0) * dy) as f64 / len2 as f64;
    if u < 0.0 {
        u = 0.0;
    }
    if u > 1.0 {
        u = 1.0;
    }
    let cx = x0 as f64 + u * dx as f64;
    let cy = y0 as f64 + u * dy as f64;
    let ddx = px as f64 - cx;
    let ddy = py as f64 - cy;
    ddx * ddx + ddy * ddy <= (t * t) as f64
}

/// Whether the pixel is on the outline of a circle, or inside a filled one.
fn circle(px: i32, py: i32, cx: i32, cy: i32, r: i32, filled: bool, t: i32) -> bool {
    let dx = px - cx;
    let dy = py - cy;
    let d2 = dx * dx + dy * dy;
    if filled {
        d2 <= r * r
    } else {
        let outer = (r + t / 2) * (r + t / 2);
        let inner = (r - t / 2).max(0);
        d2 <= outer && d2 >= inner * inner
    }
}

/// Whether the pixel is on the outline of a rectangle.
fn box_outline(px: i32, py: i32, x: i32, y: i32, w: i32, h: i32, t: i32) -> bool {
    if px < x || px >= x + w || py < y || py >= y + h {
        return false;
    }
    let in_left = px < x + t;
    let in_right = px >= x + w - t;
    let in_top = py < y + t;
    let in_bottom = py >= y + h - t;
    in_left || in_right || in_top || in_bottom
}

/// Whether the pixel is a filled dot.
fn dot(px: i32, py: i32, cx: i32, cy: i32, r: i32) -> bool {
    let dx = px - cx;
    let dy = py - cy;
    dx * dx + dy * dy <= r * r
}

/// Whether the pixel is on the outline of a rectangle.
#[allow(non_snake_case)]
fn square(px: i32, py: i32, x: i32, y: i32, w: i32, h: i32, t: i32) -> bool {
    box_outline(px, py, x, y, w, h, t)
}

// ---------------------------------------------------------------------------
// The widgets
// ---------------------------------------------------------------------------

/// The colour of `(x, y)` if it is the card's fill or its hairline.
pub fn card_px(x: usize, y: usize, rect: Rect, fill: u32, border: Option<u32>) -> Option<u32> {
    super::primitives::draw_rounded_rect(x, y, rect, RADIUS_CARD, fill, border, 1)
}

/// The colour of `(x, y)` if it is a control's fill or its hairline.
pub fn control_px(x: usize, y: usize, rect: Rect, fill: u32, border: Option<u32>) -> Option<u32> {
    super::primitives::draw_rounded_rect(x, y, rect, RADIUS_CONTROL, fill, border, 1)
}

/// The colour of `(x, y)` if it is a chip's fill or its hairline.
pub fn chip_px(x: usize, y: usize, rect: Rect, fill: u32, border: Option<u32>) -> Option<u32> {
    super::primitives::draw_pill(x, y, rect, fill, border)
}

/// The colour of `(x, y)` if it is a circle's fill or its hairline.
pub fn circle_px(
    x: usize,
    y: usize,
    cx: usize,
    cy: usize,
    radius: usize,
    fill: u32,
    border: Option<u32>,
) -> Option<u32> {
    super::primitives::draw_circle(x, y, cx, cy, radius, fill, border)
}

/// The colour of `(x, y)` if it is a one-pixel divider.
pub fn divider_px(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    len: usize,
    horizontal: bool,
    colour: u32,
) -> Option<u32> {
    super::primitives::draw_divider(x, y, x0, y0, len, horizontal, colour)
}

/// The colour of `(x, y)` if it is a checkbox's box, its tick or its fill.
///
/// WHY the tick is drawn rather than a glyph: a tick is a shape, and the
/// shape has to be the same in every application.
pub fn check_box_px(
    x: usize,
    y: usize,
    rect: Rect,
    checked: bool,
    ink: u32,
    surface: u32,
    border: u32,
) -> Option<u32> {
    if let Some(c) = super::primitives::draw_rounded_rect(x, y, rect, 6, surface, Some(border), 2) {
        if checked {
            // The tick is two strokes inside the box.
            let t = (rect.w.min(rect.h) as i32 / 8).max(1);
            let rx = rect.x as i32;
            let ry = rect.y as i32;
            let rw = rect.w as i32;
            let rh = rect.h as i32;
            if seg(x as i32, y as i32, rx + rw / 4, ry + rh / 2, rx + rw * 2 / 5, ry + rh * 3 / 4, t)
                || seg(x as i32, y as i32, rx + rw * 2 / 5, ry + rh * 3 / 4, rx + rw * 3 / 4, ry + rh / 4, t)
            {
                return Some(ink);
            }
        }
        return Some(c);
    }
    None
}

/// The colour of `(x, y)` if it is the search field's clear button.
///
/// WHY the clear button is a filled circle: it is a target, and a target
/// that is the same colour as the field it sits in is a target nobody finds.
pub fn clear_button_px(
    x: usize,
    y: usize,
    cx: usize,
    cy: usize,
    radius: usize,
    fill: u32,
    ink: u32,
) -> Option<u32> {
    if let Some(c) = super::primitives::draw_circle(x, y, cx, cy, radius, fill, None) {
        let t = (radius as i32 / 3).max(1);
        if seg(
            x as i32,
            y as i32,
            cx as i32 - radius as i32 / 2,
            cy as i32 - radius as i32 / 2,
            cx as i32 + radius as i32 / 2,
            cy as i32 + radius as i32 / 2,
            t,
        ) || seg(
            x as i32,
            y as i32,
            cx as i32 + radius as i32 / 2,
            cy as i32 - radius as i32 / 2,
            cx as i32 - radius as i32 / 2,
            cy as i32 + radius as i32 / 2,
            t,
        ) {
            return Some(ink);
        }
        return Some(c);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::primitives::Rect;

    /// Every icon in the set, so the test below reaches all of them.
    const ALL: [Icon; 59] = [
        Icon::Search,
        Icon::More,
        Icon::MoreHorizontal,
        Icon::Plus,
        Icon::Close,
        Icon::Check,
        Icon::ChevronLeft,
        Icon::ChevronRight,
        Icon::ChevronDown,
        Icon::ChevronUp,
        Icon::List,
        Icon::Checklist,
        Icon::File,
        Icon::Folder,
        Icon::Image,
        Icon::Camera,
        Icon::Music,
        Icon::Terminal,
        Icon::Calculator,
        Icon::Calendar,
        Icon::Clock,
        Icon::Globe,
        Icon::Bell,
        Icon::Pin,
        Icon::Lock,
        Icon::Unlock,
        Icon::Key,
        Icon::Shield,
        Icon::Eye,
        Icon::Share,
        Icon::Copy,
        Icon::Download,
        Icon::Trash,
        Icon::Edit,
        Icon::Refresh,
        Icon::Play,
        Icon::Stop,
        Icon::Record,
        Icon::Send,
        Icon::Attach,
        Icon::Bookmark,
        Icon::People,
        Icon::Home,
        Icon::Star,
        Icon::Bulb,
        Icon::Plane,
        Icon::Box,
        Icon::Archive,
        Icon::Mic,
        Icon::Settings,
        Icon::Agent,
        Icon::Model,
        Icon::Activity,
        Icon::Grid,
        Icon::Filter,
        Icon::Wallet,
        Icon::Book,
        Icon::Note,
        Icon::Circle,
    ];

    #[test]
    fn every_icon_inks_something() {
        let rect = Rect::new(0, 0, 24, 24);
        for icon in ALL.iter() {
            let mut inked = 0usize;
            for y in 0..24usize {
                for x in 0..24usize {
                    if icon_px(x, y, rect, *icon, 0xFF000000).is_some() {
                        inked += 1;
                    }
                }
            }
            assert!(
                inked >= 6,
                "the {:?} icon inks {inked} pixels, which is too few to read",
                icon
            );
        }
    }

    #[test]
    fn an_icon_outside_its_box_is_not_inked() {
        let rect = Rect::new(10, 20, 24, 24);
        assert!(icon_px(5, 5, rect, Icon::Search, 0xFF000000).is_none());
        assert!(icon_px(10, 20, rect, Icon::Search, 0xFF000000).is_none());
    }

    #[test]
    fn the_palette_is_the_one_the_showcase_names() {
        assert_eq!(BG, 0xFFFA_FAFA);
        assert_eq!(SURFACE, 0xFF_FFFF);
        assert_eq!(BORDER, 0xFFE5_E5E5);
        assert_eq!(TEXT_PRIMARY, 0xFF0A_0A0A);
        assert_eq!(TEXT_SECONDARY, 0xFF6B_6B6B);
        assert_eq!(TEXT_MUTED, 0xFFA3_A3A3);
        assert_eq!(SUCCESS, 0xFF22_C55E);
        assert_eq!(WARNING, 0xFFF5_9E0B);
        assert_eq!(ERROR, 0xFFEF_4444);
    }

    #[test]
    fn a_wider_run_is_never_narrower() {
        let short = text_width(b"Agent", CAP_BODY);
        let long = text_width(b"Agent Manager", CAP_BODY);
        assert!(long > short);
        assert!(short > 0);
    }

    #[test]
    fn the_fit_trims_to_the_width() {
        let text = b"General purpose agent for everyday tasks";
        let len = fit(text, 200, CAP_CAPTION);
        assert!(len > 0);
        assert!(len < text.len());
        assert!(text_width(&text[..len], CAP_CAPTION) <= 200);
    }

    #[test]
    fn a_card_draws_its_fill_and_its_hairline() {
        let rect = Rect::new(10, 10, 100, 60);
        let middle = card_px(60, 40, rect, SURFACE, Some(BORDER));
        assert_eq!(middle, Some(SURFACE));
        let edge = card_px(10, 40, rect, SURFACE, Some(BORDER));
        assert_eq!(edge, Some(BORDER));
        assert!(card_px(0, 0, rect, SURFACE, Some(BORDER)).is_none());
    }
}
