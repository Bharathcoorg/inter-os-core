//! The rectangles the dialer draws its controls in and answers taps in.
//!
//! WHY one module holds both: the renderer and the press are two functions
//! over one truth. While each carried its own numbers this app shipped five
//! disagreements a thumb can feel -- the columns beside the back chip's plate
//! (x=72..100) quit the app from header that had never been inked as a button,
//! the home strip answered a two-hundred-and-forty-wide band from row 1400
//! around a pill that is two hundred wide and starts at 1418, the two small
//! discs on the dialed-number card were inked at twenty rows and answered at
//! twenty-four, the card's favourite pill was both painted and answered at
//! thirty-six rows, and the editor's in-field discs were inked at eighteen rows
//! and answered at twenty-two.
//!
//! WHY the geometry lives in the library: the renderer lives in the binary,
//! and a binary is built without a test harness, so a control whose rect lives
//! there cannot be pinned by a test. Both sides read the declaration below.

use interos_std::design::back_button;
use interos_std::design::primitives::Rect;
use interos_std::design::touch::MIN_TARGET;

/// The dialed-number card, so the controls drawn on it can be declared
/// against its own edges instead of beside them.
pub const DISPLAY_X0: usize = 24;
pub const DISPLAY_X1: usize = 696;
pub const DISPLAY_Y0: usize = 170;
pub const DISPLAY_Y1: usize = 380;

/// The radius of the card's two small discs: the leading-plus control and the
/// clear-entry control.
///
/// WHY twenty-two and not the twenty the card drew or the twenty-four the
/// press asked: a disc of radius twenty is forty rows across, four rows under
/// the platform's floor, and a press at twenty-four answered a ring of card
/// the person was not looking at. The radius is the floor, halved, and it is
/// read by both the ink and the answer.
pub const DISC_R: i32 = (MIN_TARGET / 2) as i32;

/// The leading-plus control's centre: the card's own left padding, on the
/// number's row.
pub const PLUS_CX: usize = DISPLAY_X0 + 44;
pub const PLUS_CY: usize = 236;

/// The clear-entry control's centre: the card's right edge, inset by one disc
/// and a hair of air, on the same row.
pub const CLEAR_CX: usize = DISPLAY_X1 - 44;
pub const CLEAR_CY: usize = 236;

/// The card's two action pills, under the number: add-contact and message.
///
/// WHY rects and not the four literals each site spelled: the pills are drawn
/// inside the display card and their plate is what a thumb lands on, so the
/// plate and the press have to be the same pair of numbers -- and the plate
/// has to clear the platform's floor on both of its edges.
pub const ACTION_PILLS: [Rect; 2] = [
    Rect::new(80, 295, 250, MIN_TARGET),
    Rect::new(390, 295, 250, MIN_TARGET),
];

/// The action pill that answers the press, by its index in
/// [`ACTION_PILLS`].
pub fn action_pill_at(x: usize, y: usize) -> Option<usize> {
    ACTION_PILLS.iter().position(|pill| pill.contains(x, y))
}

/// The favourite toggle on the selected-contact card.
///
/// WHY forty-four rows and not the thirty-six the card drew: the pill was
/// painted thirty-six rows tall and answered exactly those thirty-six, so the
/// press and the ink agreed -- and both were eight rows short of the floor the
/// platform names. Growing the plate, not the invisible strip, is what makes
/// the control feel like the ones on the phones this one is being compared to.
pub const FAV_PILL: Rect = Rect::new(DISPLAY_X1 - 130, 186, 110, MIN_TARGET);

/// Whether the press is on the favourite toggle's plate.
#[inline]
pub fn fav_pill_claims(x: usize, y: usize) -> bool {
    FAV_PILL.contains(x, y)
}

/// The favourite star at the trailing edge of every contact row.
///
/// WHY the star's own radius is the disc radius: the row painted a forty-row
/// plate and answered the same forty rows, which is four rows under the floor.
/// The glyph itself is twenty-four rows across, so the plate grows around ink
/// that does not move.
pub const STAR_CX: usize = DISPLAY_X1 - 36;
pub const STAR_R: i32 = DISC_R;
/// The editor's two in-field discs: clear the field, and backspace one char.
///
/// WHY the centres are spread by more than two radii: at the old eighty-eight
/// and forty-four insets the two discs were tangent once each grew to the
/// floor, and a merged pair of circles reads as one control -- the press then
/// had to pick a winner for the row exactly between them.
pub const FIELD_CLEAR_DX: usize = 96;
pub const FIELD_BACKSPACE_DX: usize = 40;
pub const FIELD_DISC_R: i32 = DISC_R;

/// The home indicator's ink, and the strip that answers it.
///
/// WHY the strip is declared beside the pill: the pill is two hundred rows
/// wide from x=260 and starts at row 1418, and the press answered x=240..480
/// from row 1400 -- two hundred rows of page that had been inked as nothing.
/// The strip keeps the gesture's own generous width, and the pill is the ink
/// inside it, so what a person sees and what answers are one declaration.
pub const HOME_PILL: Rect = Rect::new(260, 1418, 200, 6);
pub const HOME_HIT: Rect = Rect::new(240, 1392, 240, 48);

/// Whether the press is on the home strip.
#[inline]
pub fn home_claims(x: usize, y: usize) -> bool {
    HOME_HIT.contains(x, y)
}

/// Whether the press is on the back chip's own plate.
///
/// WHY a function and not the union this file used to carry: the header row
/// answered the chip's rect plus a band to x=100, and the title run starts at
/// x=92 -- so a press on the app's own name quit the app.
#[inline]
pub fn back_claims(x: usize, y: usize) -> bool {
    back_button::contains(x, y)
}

/// Whether the press is inside the columns the app's cards are painted in.
///
/// WHY the list rows need it: the settings rows, the ringtone rows and the
/// recents ledger each chose their row from `y` alone, so the twenty-four
/// columns of page at either edge answered a row that had never been inked
/// there -- and a press in the margin that does nothing looks, to the person
/// holding the phone, exactly like a press that was dropped.
#[inline]
pub fn card_columns(x: usize) -> bool {
    x >= DISPLAY_X0 && x < DISPLAY_X1
}

/// Whether the press is on the card's leading-plus disc.
#[inline]
pub fn plus_claims(x: usize, y: usize) -> bool {
    disc_claims(PLUS_CX, PLUS_CY, x, y)
}

/// Whether the press is on the card's clear-entry disc.
#[inline]
pub fn clear_claims(x: usize, y: usize) -> bool {
    disc_claims(CLEAR_CX, CLEAR_CY, x, y)
}

/// Whether the press is inside a disc of [`DISC_R`] at the given centre.
#[inline]
pub fn disc_claims(cx: usize, cy: usize, x: usize, y: usize) -> bool {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    dx * dx + dy * dy <= DISC_R * DISC_R
}

// WHY these gates sit beside the numbers: each one is a control's own floor,
// and a floor written only in prose is a floor the next edit steps over.
const _: () = assert!(DISC_R as usize * 2 >= MIN_TARGET);
const _: () = assert!(back_button::SIZE >= MIN_TARGET);
const _: () = assert!(HOME_PILL.x >= HOME_HIT.x);
const _: () = assert!(HOME_PILL.x + HOME_PILL.w <= HOME_HIT.x + HOME_HIT.w);
const _: () = assert!(HOME_PILL.y >= HOME_HIT.y);
const _: () = assert!(HOME_PILL.y + HOME_PILL.h <= HOME_HIT.y + HOME_HIT.h);
const _: () = assert!(PLUS_CX + DISC_R as usize <= CLEAR_CX);
const _: () = assert!(ACTION_PILLS[0].w >= MIN_TARGET && ACTION_PILLS[0].h >= MIN_TARGET);
const _: () = assert!(ACTION_PILLS[1].w >= MIN_TARGET && ACTION_PILLS[1].h >= MIN_TARGET);
const _: () = assert!(ACTION_PILLS[0].x < ACTION_PILLS[1].x);
const _: () = assert!(ACTION_PILLS[1].x + ACTION_PILLS[1].w <= DISPLAY_X1);
const _: () = assert!(FAV_PILL.h >= MIN_TARGET);
const _: () = assert!(FAV_PILL.x + FAV_PILL.w <= DISPLAY_X1);
const _: () = assert!(FAV_PILL.y + FAV_PILL.h <= DISPLAY_Y1);
const _: () = assert!(STAR_CX as i32 - DISC_R >= 0);
const _: () = assert!(STAR_CX + DISC_R as usize <= DISPLAY_X1);
const _: () = assert!(FIELD_DISC_R * 2 >= MIN_TARGET as i32);
const _: () = assert!(FIELD_CLEAR_DX - FIELD_BACKSPACE_DX >= 2 * FIELD_DISC_R as usize);
const _: () = assert!(DISPLAY_X1 - FIELD_BACKSPACE_DX + FIELD_DISC_R as usize <= DISPLAY_X1);
