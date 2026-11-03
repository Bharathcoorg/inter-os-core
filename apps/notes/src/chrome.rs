//! The header row's partition: the platform's back control, and the runs that
//! have to clear it.
//!
//! WHY this is its own module and not two literals in the renderer: the painter
//! and the tap handler have to agree on which pixels the control owns, and the
//! device showed what happens when they do not -- Notes painted no control at
//! the canonical rect, so a tap at (48, 80) fell through to the search pill,
//! answered nothing and left the person on a screen with no way back. One
//! statement of the partition is also the only version of it a host test can
//! reach: the renderer lives in the binary, which is built without a test
//! harness.

use crate::theme::{spacing, THEME};
use interos_std::design::back_button;
/// The platform's rectangle, re-exported so the renderer and the tests name
/// the same type the controls below are declared in.
pub use interos_std::design::primitives::Rect;
use interos_std::design::touch::MIN_TARGET;

/// The left edge a header run may start at: one grid step clear of the control.
///
/// WHY it is derived from the control's own geometry: the rect belongs to the
/// SDK, and a run authored against a hand-copied number slides under the chip
/// the moment the platform moves it.
pub const LABEL_X: usize = back_button::X + back_button::SIZE + spacing::GRID_16;

/// The control's pixel, in the theme this app draws in.
#[inline]
pub fn back_pixel(x: usize, y: usize) -> Option<u32> {
    back_button::pixel_for_theme(x, y, &THEME)
}

/// Whether the control claims the tap -- answered before any row that shares
/// its rect.
#[inline]
pub fn back_claims(x: usize, y: usize) -> bool {
    back_button::contains(x, y)
}

// ---------------------------------------------------------------------------
// The controls this app draws outside the header row.
//
// WHY they are here and not beside the renderer: the painter and the press
// are two functions, and while each held its own radius every circular tool
// in this app was drawn smaller than the plate it answered -- the editor's
// toolbar stood twenty rows to the press's twenty-two, the reading dock's
// tools twenty to twenty-four, the list's floating action button twenty-eight
// to thirty-four, and the swatch picker twenty-two to twenty-six. A thumb
// lands on the ink, so the ink is what has to meet the platform's floor: the
// radius below is read by both sides, and the const gate is the floor.
// ---------------------------------------------------------------------------

/// The radius of every circular tool the app draws, and of its press.
pub const TOOL_R: usize = MIN_TARGET / 2;

/// The floating action button's own radius: a primary action, so the plate
/// the platform names for one.
pub const FAB_R: usize = 28;

/// The editor toolbar's three tools, on the header row's centre line.
pub const EDITOR_TOOL_CXS: [usize; 3] = [520, 580, 640];
pub const EDITOR_TOOL_CY: usize = back_button::Y + back_button::SIZE / 2;

/// The reading dock's four tools and the row they stand on.
pub const READING_TOOL_CXS: [usize; 4] = [240, 340, 440, 630];
pub const READING_TOOL_CY: usize = 1358;

/// The reading dock's plate, and the category tag drawn at its head.
pub const READING_DOCK: Rect = Rect::new(36, 1324, 648, 68);
pub const READING_TAG: Rect = Rect::new(54, READING_TOOL_CY - MIN_TARGET / 2, 100, MIN_TARGET);

/// The list's floating pill, the action button beside it, and the three tools
/// the pill carries at its trailing end.
///
/// WHY the tools are bands of one width centred on their own glyphs: the pill
/// ran to x=608 while its three glyph centres were 440, 500 and 560, and the
/// press used 415..465, 475..525 and 535..608 -- so the middle of each glyph
/// sat left of the middle of the band that answered it, the ten rows between
/// the palette and the keyboard answered nothing, and the last column of the
/// pill was a dead edge.
pub const DOCK_PILL: Rect = Rect::new(28, 1320, 580, 64);
pub const DOCK_FAB_CX: usize = 654;
pub const DOCK_FAB_CY: usize = 1352;
pub const DOCK_TOOL_CXS: [usize; 3] = [440, 500, 560];
pub const DOCK_TOOL_W: usize = 60;

/// The list's search pill and the clear control at its trailing end.
///
/// WHY the clear is a rect and not a column test: the pill is six hundred and
/// sixty-four rows wide and the clear's ink is a fourteen-row cross, so the
/// cross has to sit in the middle of the plate the press answers rather than
/// at a column someone measured once.
pub const SEARCH_PILL: Rect = Rect::new(28, back_button::Y, 664, 54);
pub const SEARCH_CLEAR: Rect = Rect::new(618, SEARCH_PILL.y, 68, SEARCH_PILL.h);

/// The home indicator's ink, and the strip that answers it.
///
/// The ink and the answering box used to be authored here -- `260, 1418, 200, 6`
/// over a strip this file built by hand from the keyboard's own bottom edge.
/// `home_band` names both now: the pill is the shell's, and `hit_from` is the
/// SDK's answer to an app whose own control holds the bottom of the screen.
///
/// WHY the box starts at the keyboard's bottom rather than at the band's: the
/// soft keyboard owns `keyboard::KB_Y0..keyboard::KB_Y1`, and the home band used
/// to begin at row 1400 inside it -- a finger that pressed a key in the
/// keyboard's last row, between x=240 and x=480, quit the app instead of typing.
/// The box begins where the keyboard's own hit test stops, still reaches the
/// panel's last row, and still covers the pill it is drawn for.
///
/// WHY the pill is drawn at all: the app answered a strip on a page it had never
/// inked, so the one way out of the app that a person could see was nothing at
/// all.
pub const HOME_PILL: Rect = interos_std::design::home_band::PILL;
pub const HOME_HIT: Rect = interos_std::design::home_band::hit_from(crate::keyboard::KB_Y1);

/// Whether the press is on the home strip.
#[inline]
pub fn home_claims(x: usize, y: usize) -> bool {
    HOME_HIT.contains(x, y)
}

/// Whether the press is on the clear control of the search pill.
#[inline]
pub fn search_clear_claims(x: usize, y: usize) -> bool {
    SEARCH_CLEAR.contains(x, y)
}

/// The dock tool whose band holds the press, by its glyph index.
///
/// WHY index and not character: the dock's three tools are checklist, palette
/// and keyboard in that order in both the renderer and the press, and an index
/// is the one name that cannot be typed differently in the two places.
pub fn dock_tool_at(x: usize, y: usize) -> Option<usize> {
    if !DOCK_PILL.contains(x, y) {
        return None;
    }
    DOCK_TOOL_CXS
        .iter()
        .position(|&cx| x >= cx - DOCK_TOOL_W / 2 && x < cx + DOCK_TOOL_W / 2)
}

const _: () = assert!(TOOL_R * 2 >= MIN_TARGET);
const _: () = assert!(FAB_R * 2 >= MIN_TARGET);
const _: () = assert!(SEARCH_CLEAR.w >= MIN_TARGET && SEARCH_CLEAR.h >= MIN_TARGET);
const _: () = assert!(READING_TAG.w >= MIN_TARGET && READING_TAG.h >= MIN_TARGET);
const _: () = assert!(DOCK_TOOL_W >= MIN_TARGET);
const _: () = assert!(HOME_PILL.y >= HOME_HIT.y);
const _: () = assert!(HOME_PILL.y + HOME_PILL.h <= HOME_HIT.y + HOME_HIT.h);
const _: () = assert!(HOME_PILL.x >= HOME_HIT.x);
const _: () = assert!(HOME_PILL.x + HOME_PILL.w <= HOME_HIT.x + HOME_HIT.w);
const _: () = assert!(SEARCH_CLEAR.x + SEARCH_CLEAR.w <= SEARCH_PILL.x + SEARCH_PILL.w);
const _: () = assert!(READING_TAG.y >= READING_DOCK.y);
const _: () = assert!(READING_TAG.y + READING_TAG.h <= READING_DOCK.y + READING_DOCK.h);
