//! The rectangles the calculator draws its chrome in and answers taps in.
//!
//! WHY one module holds both: the painter and the tap handler are two
//! functions over one truth, and while each carried its own numbers this app
//! shipped three disagreements a thumb can feel. The history drawer's Clear
//! and Close pills were drawn forty rows tall against the platform's
//! forty-four; the home strip answered a two-hundred-and-forty-wide,
//! forty-eight-row box around a pill that was one hundred and forty across;
//! and the columns past the back chip's plate, at x=72..100, quit the app
//! from bare page that had never been inked as a button. Every control named
//! here is read by both sides -- the renderer paints the rect, the press
//! asks the rect -- so a control cannot be drawn in one place and pressed in
//! another, and the const gates below are the platform's floor rather than a
//! hope about it.

use crate::theme::spacing;
use interos_std::design::outline_text;
use interos_std::design::primitives::Rect;
use interos_std::design::touch::MIN_TARGET;
use interos_std::design::typography;

/// The line box of a run drawn at a cap height.
///
/// WHY the typeface's own box and not a number: a line box is the space a
/// run reserves, and a run reserves its ascent and its descent.
pub const fn run_rows(cap_px: u32) -> usize {
    let em = outline_text::em_for_cap_height(cap_px);
    let (ascent, descent) = interos_std::design::typeface::line_metrics(em);
    (ascent + descent) as usize
}

/// The display card: the surface the expression and the result stand on.
pub const DISPLAY_X0: usize = 24;
pub const DISPLAY_X1: usize = 696;
pub const DISPLAY_PAD: usize = spacing::GRID_24;
pub const DISPLAY_Y0: usize = 110;

/// The expression tape.
pub const TAPE_LINES: usize = crate::TAPE_VISIBLE_LINES;
pub const TAPE_LINE_H: usize = crate::TAPE_LINE_H;
pub const TAPE_Y0: usize = DISPLAY_Y0 + DISPLAY_PAD;
pub const TAPE_Y1: usize = TAPE_Y0 + TAPE_LINES * TAPE_LINE_H;

/// The gap between the expression and the result, the result's top, and the
/// bottom of the display band.
pub const VALUE_GAP: usize = 12;
pub const VALUE_Y0: usize = TAPE_Y1 + VALUE_GAP;
pub const DISPLAY_Y1: usize = VALUE_Y0 + interos_std::design::typography::DISPLAY_LINE_PX + DISPLAY_PAD;

/// The right edge of the tape's text.
pub const TAPE_TEXT_X1: usize = DISPLAY_X1 - DISPLAY_PAD;

/// The tape's scroll control.
///
/// WHY it is one plate and not two floating glyphs: the chevrons used to be
/// drawn as nineteen pixels of ink inside a hit box of forty by thirty-six,
/// so the control was smaller than the platform's floor in both directions
/// and the box it answered was bigger than anything the person could see.
/// The plate below is what is painted -- a rounded chip the full height of
/// the tape -- and the tap answers exactly that chip, the upper half
/// scrolling toward the start of the expression and the lower half toward
/// its end.
///
/// WHY the plate is the tape's height and not half of it: two stacked
/// forty-four-row controls need eighty-eight rows, and the tape is two
/// caption lines. A control that cannot be made tall enough has to be made
/// one control, not two that both break the floor.
pub const SCROLL_PLATE_W: usize = MIN_TARGET;
pub const SCROLL_PLATE_H: usize = TAPE_LINES * TAPE_LINE_H;
pub const SCROLL_PLATE_X: usize = TAPE_TEXT_X1 - 12 - SCROLL_PLATE_W / 2;
pub const SCROLL_PLATE: Rect = Rect::new(SCROLL_PLATE_X, TAPE_Y0, SCROLL_PLATE_W, SCROLL_PLATE_H);

/// The centre column of the plate's two chevrons.
pub const SCROLL_CHEVRON_X: usize = SCROLL_PLATE_X + SCROLL_PLATE_W / 2;

/// The band the history drawer's Clear and Close controls are drawn in, and
/// the offset their label starts at inside it.
///
/// WHY the band is one name and not the six numbers it was: the painter, the
/// corner rounding and the press all test the same rows, and the label was
/// centred from the cap rather than the box -- a scale-two run reserves
/// `run_rows(BODY_CAP_PX)` rows, so centring from an 18-row cap stood the
/// box seven rows below the fill's middle and ran its last three rows past
/// the pill's own bottom edge. On a per-pixel blit those rows are not
/// clipped: they are painted on whatever the drawer puts under the pill.
///
/// WHY forty-eight and not the forty it was: the plate is what a thumb
/// lands on, and a forty-row plate is four rows short of the floor the
/// platform names. The drawer's header band grew with it, so the plate is
/// painted at the size the press answers.
pub const PILL_TOP: usize = 128;
pub const PILL_H: usize = 48;
pub const PILL_BOTTOM: usize = PILL_TOP + PILL_H;
pub const PILL_INK_DY: usize = (PILL_H - run_rows(typography::BODY_CAP_PX)) / 2;

/// The drawer's header band: the rows that carry its title and its two pills.
pub const DRAWER_HEADER_Y0: usize = 125;
pub const DRAWER_HEADER_Y1: usize = PILL_BOTTOM + 4;

/// The drawer's two controls, as one rect each.
pub const CLEAR_PILL: Rect = Rect::new(440, PILL_TOP, 96, PILL_H);
pub const CLOSE_PILL: Rect = Rect::new(550, PILL_TOP, 110, PILL_H);

/// The drawer's ledger: the row's two boxes, their stride, and the row.
pub const HIST_LIST_Y0: usize = 195;
pub const HIST_ROW_PITCH: usize = 140;
pub const HIST_EXPR_DY: usize = 16;
pub const HIST_LINE_STRIDE: usize = run_rows(typography::SECTION_CAP_PX);
pub const HIST_RES_DY: usize = HIST_EXPR_DY + HIST_LINE_STRIDE;
pub const HIST_ROW_BOTTOM_PAD: usize = 2;
pub const HIST_ROW_H: usize =
    HIST_RES_DY + run_rows(typography::TITLE_CAP_PX) + HIST_ROW_BOTTOM_PAD;

/// The home indicator, in the platform's own geometry.
///
/// WHY these numbers and not the app's own: the shell draws the home
/// indicator on every screen, and the app drew a second one -- a grey line,
/// two hundred pixels wide, in a different row, while its press answered a
/// strip from x=240 to x=480 that the pill never covered. The pill and the
/// strip are now the same declaration: the pill is the ink, the strip is
/// the gesture, and the strip is the width the launcher's own tests tap.
pub const HOME_PILL: Rect = Rect::new(260, 1418, 200, 6);
pub const HOME_HIT: Rect = Rect::new(240, 1392, 240, 48);

/// The header's history toggle: the pill beside the back chip that opens the
/// drawer. Declared once because the header row draws it and the press asks
/// it, and the two used to be copies of the same four literals.
pub const HISTORY_PILL: Rect = Rect::new(
    576,
    interos_std::design::back_button::Y,
    112,
    interos_std::design::back_button::SIZE,
);

/// The pending operation's badge is not a control: it is the operation the
/// calculator is waiting on, and it answers nothing. It stands beside the
/// result at `VALUE_Y0`, thirty-four rows tall.

// WHY these gates sit next to the numbers: every one of them is a control's
// own floor, and a floor that is only written down in a comment is a floor
// the next edit can step over.
const _: () = assert!(SCROLL_PLATE_W >= MIN_TARGET);
const _: () = assert!(SCROLL_PLATE_H >= MIN_TARGET);
const _: () = assert!(PILL_H >= MIN_TARGET);
const _: () = assert!(HISTORY_PILL.w >= MIN_TARGET && HISTORY_PILL.h >= MIN_TARGET);
const _: () = assert!(CLEAR_PILL.w >= MIN_TARGET && CLEAR_PILL.h >= MIN_TARGET);
const _: () = assert!(CLOSE_PILL.w >= MIN_TARGET && CLOSE_PILL.h >= MIN_TARGET);
const _: () = assert!(HOME_PILL.x >= HOME_HIT.x);
const _: () = assert!(HOME_PILL.x + HOME_PILL.w <= HOME_HIT.x + HOME_HIT.w);
const _: () = assert!(HOME_PILL.y >= HOME_HIT.y);
const _: () = assert!(HOME_PILL.y + HOME_PILL.h <= HOME_HIT.y + HOME_HIT.h);
const _: () = assert!(HIST_RES_DY >= HIST_EXPR_DY + HIST_LINE_STRIDE);
const _: () = assert!(HIST_RES_DY + run_rows(typography::TITLE_CAP_PX) <= HIST_ROW_H);
const _: () = assert!(run_rows(typography::BODY_CAP_PX) <= PILL_H);
const _: () = assert!(TAPE_Y1 + VALUE_GAP + 40 <= 420);

/// Whether the press is on the back chip's own plate.
///
/// WHY this is a function and not the SDK call at the two sites: the header
/// row used to union the chip's rect with a 24..100 band, so the columns
/// beside the plate quit the app from bare page. There is now one answer,
/// and it is the painted plate.
#[inline]
pub fn back_claims(x: usize, y: usize) -> bool {
    interos_std::design::back_button::contains(x, y)
}

/// Whether the press is on the home strip.
#[inline]
pub fn home_claims(x: usize, y: usize) -> bool {
    HOME_HIT.contains(x, y)
}

/// Whether the press is on the header's history toggle.
#[inline]
pub fn history_claims(x: usize, y: usize) -> bool {
    HISTORY_PILL.contains(x, y)
}

/// Whether the press is on the drawer's Clear pill.
#[inline]
pub fn clear_claims(x: usize, y: usize) -> bool {
    CLEAR_PILL.contains(x, y)
}

/// Whether the press is on the drawer's Close pill.
#[inline]
pub fn close_claims(x: usize, y: usize) -> bool {
    CLOSE_PILL.contains(x, y)
}

/// Whether the press is on the tape's scroll plate, and which half took it.
///
/// WHY the plate is asked for as a whole: the plate is the control the
/// person sees, so the answer has to be yes or no about that chip before it
/// is a direction about its halves.
#[inline]
pub fn scroll_claims(x: usize, y: usize) -> Option<bool> {
    if !SCROLL_PLATE.contains(x, y) {
        return None;
    }
    Some(y < TAPE_Y0 + SCROLL_PLATE_H / 2)
}
