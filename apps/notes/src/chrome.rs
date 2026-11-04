// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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

// ---------------------------------------------------------------------------
// The keyboard's accessory strip.
//
// WHY one array of plates and not seven pairs of literals: the strip drew its
// seven buttons as rounded rects at `32,920,54,48` and pressed them as
// `x >= 32 && x <= 86`. The inclusive bound made the press one column wider
// than the plate -- and for the two cursor steps, three (`350..406` inked,
// `350..408` answered) -- while the strip's own band ran `916..974` against a
// container that ends its ink at row 973. So the last column of every button
// and the last row of the whole strip were page the press took as a key and
// the painter had already let go of. The plates below are the shape, the
// painter inks them and the press answers them, and no third site restates
// the numbers.
// ---------------------------------------------------------------------------

/// The accessory strip's container plate, above the keyboard's first row.
///
/// WHY derived from the keyboard: the strip belongs to the dock it sits on, and
/// the dock's own top edge is `keyboard::KB_Y0`. Authored as a bare `916` it
/// would have to be re-measured every time the dock moved.
pub const STRIP: Rect = Rect::new(16, crate::keyboard::KB_Y0 - 64, 688, 58);

/// The height of every plate the strip carries, and the row its plates start
/// on. Four rows of air above the plate and six below are the container's own
/// inset, kept exactly as the strip has always drawn it.
pub const STRIP_BTN_H: usize = 48;
pub const STRIP_BTN_Y: usize = STRIP.y + 4;

/// The seven plates the strip paints and answers, in the order the press asks
/// them: checklist, bullet, palette, category cycle, cursor left, cursor right,
/// hide keyboard.
pub const STRIP_PLATES: [Rect; 7] = [
    Rect::new(32, STRIP_BTN_Y, 54, STRIP_BTN_H),
    Rect::new(94, STRIP_BTN_Y, 54, STRIP_BTN_H),
    Rect::new(156, STRIP_BTN_Y, 54, STRIP_BTN_H),
    Rect::new(218, STRIP_BTN_Y, 116, STRIP_BTN_H),
    Rect::new(350, STRIP_BTN_Y, 56, STRIP_BTN_H),
    Rect::new(418, STRIP_BTN_Y, 56, STRIP_BTN_H),
    Rect::new(626, STRIP_BTN_Y, 62, STRIP_BTN_H),
];

/// The strip plate that holds the press, by its index in [`STRIP_PLATES`].
pub fn strip_plate_at(x: usize, y: usize) -> Option<usize> {
    STRIP_PLATES.iter().position(|plate| plate.contains(x, y))
}

// ---------------------------------------------------------------------------
// The tone picker's popover.
//
// WHY a function here and not two identical `if` expressions in the renderer
// and the press: both sites wrote `let swatch_y0 = if keyboard_visible { 852 }
// else { 1248 }; let swatch_y1 = swatch_y0 + 62;`. Two copies of one band, in
// two files' worth of reasoning, is the pattern this module exists to end --
// and the day the keyboard moves, one of the two is updated and the picker
// answers rows it does not ink.
// ---------------------------------------------------------------------------

/// The tone picker's band while the keyboard is up, and while it is not.
///
/// WHY the two rows are named separately: with the keyboard up the picker sits
/// on the free rows above the accessory strip; without it, above the reading
/// dock. Neither is the keyboard's own rows, and the press must not reach into
/// them.
pub const SWATCH_Y0_DOCKED: usize = 1248;
pub const SWATCH_Y0_KEYBOARD: usize = STRIP.y - 64;
/// The picker's container: its width, height and the swatch pitch inside it.
pub const SWATCH_H: usize = 62;
pub const SWATCH_PITCH: usize = 82;
pub const SWATCH_X0: usize = 58;
/// The swatch's press radius: the same floor-sized disc every other circular
/// tool in this app answers, and the ring the active swatch inks.
pub const SWATCH_R: usize = TOOL_R;

/// The picker's band for the keyboard state the screen is in.
#[inline]
pub const fn swatch_band_at(keyboard_visible: bool) -> Rect {
    let y0 = if keyboard_visible {
        SWATCH_Y0_KEYBOARD
    } else {
        SWATCH_Y0_DOCKED
    };
    Rect::new(20, y0, 680, SWATCH_H)
}

/// The centre row of the picker's swatches, read off the band itself.
#[inline]
pub const fn swatch_cy(band: Rect) -> usize {
    band.y + band.h / 2
}

/// The swatch the press lands on, or `None` when the press is on the picker's
/// container rather than on one of its plates.
pub fn swatch_at(x: usize, y: usize, keyboard_visible: bool) -> Option<usize> {
    let band = swatch_band_at(keyboard_visible);
    if !band.contains(x, y) {
        return None;
    }
    let cy = swatch_cy(band);
    (0..crate::NOTE_TONE_COUNT).find(|&i| {
        let dx = x as i32 - (SWATCH_X0 + i * SWATCH_PITCH) as i32;
        let dy = y as i32 - cy as i32;
        dx * dx + dy * dy <= (SWATCH_R * SWATCH_R) as i32
    })
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

// The accessory strip's floor, and the proof that its plates stand inside the
// container the strip inks. WHY a sweep and not seven lines: a plate the strip
// grows tomorrow has to clear the same floor as the seven it has today.
const _: () = {
    let mut i = 0;
    while i < STRIP_PLATES.len() {
        let p = STRIP_PLATES[i];
        assert!(p.w >= MIN_TARGET && p.h >= MIN_TARGET);
        assert!(p.x >= STRIP.x && p.y >= STRIP.y);
        assert!(p.x + p.w <= STRIP.x + STRIP.w);
        assert!(p.y + p.h <= STRIP.y + STRIP.h);
        if i + 1 < STRIP_PLATES.len() {
            assert!(p.x + p.w <= STRIP_PLATES[i + 1].x);
        }
        i += 1;
    }
    assert!(STRIP.y + STRIP.h <= crate::keyboard::KB_Y0);
    assert!(STRIP_BTN_Y + STRIP_BTN_H <= STRIP.y + STRIP.h);
};

// The tone picker's floor: a band tall enough for a thumb, a press disc at the
// platform's floor, and swatches that never merge into one another.
const _: () = assert!(SWATCH_H >= MIN_TARGET);
const _: () = assert!(SWATCH_R * 2 >= MIN_TARGET);
const _: () = assert!(SWATCH_PITCH >= 2 * SWATCH_R);
const _: () = assert!(
    SWATCH_X0 + (crate::NOTE_TONE_COUNT - 1) * SWATCH_PITCH + SWATCH_R
        <= swatch_band_at(false).x + swatch_band_at(false).w
);
const _: () = assert!(swatch_band_at(true).y + SWATCH_H <= STRIP.y);
const _: () = assert!(swatch_band_at(false).y + SWATCH_H <= READING_DOCK.y);
