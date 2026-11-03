// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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

/// The cap a run drawn at the ladder's `base_cap_px` stands at, at `level`.
///
/// WHY beside the ladder and not inside it: [`interos_std::font::cap_for_scale`]
/// is a `const fn` and every `*_CAP_PX` in the design ladder, and the gates
/// under this file, are authored against it -- a const evaluation cannot read
/// the atomic the setting writes at run time. The scaling is therefore applied
/// where the display measures and inks its runs, which is at draw time, by the
/// SDK's own runtime face of the same arithmetic.
///
/// WHY the level is an argument here and read from the published cell beside
/// it: the SDK's `*_user` helpers read one process-wide atomic, and the test
/// harness runs test functions on parallel threads, so a check of these
/// relations that read the cell would race with any other test that publishes
/// a level. The arithmetic therefore takes its level, and the zero-argument
/// faces of the same functions are what the renderer calls.
#[inline]
pub const fn cap_at(base_cap_px: u32, level: u8) -> u32 {
    interos_std::font::scale_cap_height(base_cap_px, level)
}

/// The cap a run at the ladder's `base_cap_px` stands at, at the text-size
/// level this process has published.
#[inline]
pub fn cap_user(base_cap_px: u32) -> u32 {
    cap_at(base_cap_px, interos_std::font::user_text_scale())
}

/// The line box a run at `base_cap_px` reserves at `level`. The box half of the
/// pair with [`cap_at`]: a scaled ink in an unscaled box is clipped text.
#[inline]
pub const fn run_rows_at(base_cap_px: u32, level: u8) -> usize {
    run_rows(cap_at(base_cap_px, level))
}

/// The line box a run at the ladder's `base_cap_px` reserves at the published
/// level.
#[inline]
pub fn run_rows_user(base_cap_px: u32) -> usize {
    run_rows_at(base_cap_px, interos_std::font::user_text_scale())
}

/// The largest cap at `level` whose line box stands inside a plate of
/// `plate_rows` rows.
///
/// WHY this is a run-time guard and not one of the `const _: () = assert!`
/// gates below: those gates hold at the ladder -- the size the platform draws
/// when nobody has touched the setting -- and they stay exactly where they
/// are. A person's text size is not known at compile time, so the relation
/// between a scaled run and a plate that does not move with it can only be
/// checked when the frame is planned. The guard therefore clamps, and the floor
/// it clamps to is the ladder's own cap: a plate that already holds the ladder
/// is never drawn smaller than it is today, so adopting the setting cannot
/// regress a screen that already had no room -- it only stops a larger setting
/// from spilling further past the plate.
#[inline]
pub const fn cap_in_plate_at(base_cap_px: u32, plate_rows: usize, level: u8) -> u32 {
    let want = cap_at(base_cap_px, level);
    if run_rows(want) <= plate_rows {
        return want;
    }
    // Compact is the one level whose own cap is below the ladder's, and it is
    // the size the person asked for, so it is the floor when it is what they
    // chose. Above the ladder the floor is the ladder.
    let floor = if want < base_cap_px { want } else { base_cap_px };
    let mut cap = want;
    while cap > floor && run_rows(cap) > plate_rows {
        cap -= 1;
    }
    cap
}

/// The largest cap at the published level whose line box stands inside a plate
/// of `plate_rows` rows. See [`cap_in_plate_at`].
#[inline]
pub fn cap_in_plate(base_cap_px: u32, plate_rows: usize) -> u32 {
    cap_in_plate_at(base_cap_px, plate_rows, interos_std::font::user_text_scale())
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

/// The first row of the keypad grid: the band the display card may not reach.
///
/// WHY named here: the card's own bottom used to be a `const` derived from the
/// ladder, and the ladder is only the size the platform draws when nobody has
/// touched the text-size setting. A person who picks Extra Large grows the two
/// tape lines and the result run, and the card that holds them has exactly one
/// place to grow into -- the rows between the card and the keypad -- so the
/// limit is named once here and every run-time layout below clamps to it.
pub const KEYPAD_TOP: usize = 420;

/// The drawer's sheet, which the drawer itself and the press both stop at.
pub const DRAWER_SHEET_Y1: usize = 1380;

/// The tape's line box at `level`.
///
/// WHY the box moves with the ink and the card moves with the box: the tape is
/// the whole expression and the only way to read it, and a caption scaled to
/// one hundred and fifty per cent inside a thirty-row stride overwrites the
/// line below it. The stride is the run's own box, so the lines never collide.
#[inline]
pub const fn tape_line_rows_at(level: u8) -> usize {
    run_rows_at(crate::EXPRESSION_CAP_PX, level)
}

/// The tape's line box at the published text-size level.
#[inline]
pub fn tape_line_rows() -> usize {
    tape_line_rows_at(interos_std::font::user_text_scale())
}

/// The bottom of the tape at `level`.
#[inline]
pub const fn tape_y1_at(level: u8) -> usize {
    TAPE_Y0 + TAPE_LINES * tape_line_rows_at(level)
}

/// The bottom of the tape at the published level.
#[inline]
pub fn tape_y1() -> usize {
    tape_y1_at(interos_std::font::user_text_scale())
}

/// The top the result run stands at, at `level`.
#[inline]
pub const fn value_y0_at(level: u8) -> usize {
    tape_y1_at(level) + VALUE_GAP
}

/// The top the result run stands at, at the published level.
#[inline]
pub fn value_y0() -> usize {
    value_y0_at(interos_std::font::user_text_scale())
}

/// The rows a result run may reserve at `level` before the card reaches the
/// keypad.
///
/// WHY a run-time number and not only the `const _: () = assert!(TAPE_Y1 +
/// VALUE_GAP + 40 <= 420)` gate beside it: that gate is the ladder's, and the
/// ladder still holds it -- see the gates at the foot of this file. What a
/// selectable text size does to the card can only be answered when the frame is
/// planned, and the answer below is the clamp the plan draws against.
#[inline]
pub const fn result_rows_room_at(level: u8) -> usize {
    KEYPAD_TOP
        .saturating_sub(DISPLAY_PAD)
        .saturating_sub(value_y0_at(level))
}

/// The rows a result run may reserve at the published level.
#[inline]
pub fn result_rows_room() -> usize {
    result_rows_room_at(interos_std::font::user_text_scale())
}

/// The bottom of the display card at `level`, once the result run it holds is
/// known, clamped so it never reaches the keypad's first row.
#[inline]
pub const fn card_y1_at(result_rows: usize, level: u8) -> usize {
    let grown = value_y0_at(level) + result_rows + DISPLAY_PAD;
    // WHY an if and not `Ord::min`: a `const fn` cannot call the unstable
    // trait face, and the clamp has to be const for the gates beside it.
    if grown > KEYPAD_TOP {
        KEYPAD_TOP
    } else {
        grown
    }
}

/// The bottom of the display card at the published level. See [`card_y1_at`].
#[inline]
pub fn card_y1(result_rows: usize) -> usize {
    card_y1_at(result_rows, interos_std::font::user_text_scale())
}

/// The tape's scroll plate at `level`: the same chip the plan paints and the
/// press answers, the tape's own height.
#[inline]
pub const fn scroll_plate_at(level: u8) -> Rect {
    let tape = TAPE_LINES * tape_line_rows_at(level);
    // WHY floored at the platform's touch target and not at the ladder's plate:
    // the chip is the tape's height because the tape is what it scrolls, and at
    // the smallest text size the tape is shorter than the floor a thumb has to
    // have. The floor is the platform's, so the control keeps it at every level
    // and only grows from there.
    let h = if tape < MIN_TARGET { MIN_TARGET } else { tape };
    Rect::new(SCROLL_PLATE_X, TAPE_Y0, SCROLL_PLATE_W, h)
}

/// The tape's scroll plate at the published level. See [`scroll_plate_at`].
#[inline]
pub fn scroll_plate() -> Rect {
    scroll_plate_at(interos_std::font::user_text_scale())
}

/// The history row's two boxes at `level`: the expression's line box, the
/// result's line box, and the row that holds both.
///
/// WHY the pitch grows with the row instead of the row growing past the pitch:
/// the drawer's rows are painted and pressed from one stride, and a row that
/// overruns its stride paints onto the row below it. Growing the stride keeps
/// both runs readable and asks only that fewer rows stand inside the sheet --
/// which [`history_visible_rows_at`] answers, so the last row a person can
/// reach is never painted past the sheet's bottom edge.
#[inline]
pub const fn history_row_h_at(level: u8) -> usize {
    let expr = run_rows_at(typography::SECTION_CAP_PX, level);
    let result = run_rows_at(typography::TITLE_CAP_PX, level);
    HIST_EXPR_DY + expr + result + HIST_ROW_BOTTOM_PAD
}

/// The history row's height at the published level. See [`history_row_h_at`].
#[inline]
pub fn history_row_h() -> usize {
    history_row_h_at(interos_std::font::user_text_scale())
}

/// The stride between history rows at `level`.
#[inline]
pub const fn history_pitch_at(level: u8) -> usize {
    let grown = history_row_h_at(level);
    if grown > HIST_ROW_PITCH {
        grown
    } else {
        HIST_ROW_PITCH
    }
}

/// The stride between history rows at the published level.
#[inline]
pub fn history_pitch() -> usize {
    history_pitch_at(interos_std::font::user_text_scale())
}

/// How many history rows stand fully inside the drawer's sheet at `level`.
#[inline]
pub const fn history_visible_rows_at(level: u8) -> usize {
    let sheet_rows = DRAWER_SHEET_Y1.saturating_sub(HIST_LIST_Y0);
    let rows = sheet_rows / history_pitch_at(level);
    if rows == 0 {
        1
    } else {
        rows
    }
}

/// How many history rows stand fully inside the drawer's sheet at the published
/// level.
#[inline]
pub fn history_visible_rows() -> usize {
    history_visible_rows_at(interos_std::font::user_text_scale())
}

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
/// The ink and the answering strip used to be authored here, as `260, 1418,
/// 200, 6` over a strip of `240, 1392, 240, 48` -- numbers that had already
/// arrived at the SDK's geometry, so what changes is who owns them. The shell
/// draws the indicator on every screen and this app drew a second one once, a
/// grey line in a different row while its press answered a strip the pill never
/// covered; reading both from `home_band` is what keeps that from happening
/// again.
pub const HOME_PILL: Rect = interos_std::design::home_band::PILL;
pub const HOME_HIT: Rect = interos_std::design::home_band::HIT;

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
const _: () = assert!(TAPE_Y1 + VALUE_GAP + 40 <= KEYPAD_TOP);

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
    interos_std::design::home_band::contains(x, y)
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
    let plate = scroll_plate();
    if !plate.contains(x, y) {
        return None;
    }
    Some(y < TAPE_Y0 + plate.h / 2)
}

// WHY this gate sits beside the run-time clamp: the card's bottom is derived
// from the tape's line box, and the line box is the person's text size, so the
// only compile-time answer left here is the ladder's own -- that at one hundred
// per cent the card still stands clear of the keypad. `result_rows_room` is the
// run-time half of the same relation, for every level the setting offers.
const _: () = assert!(DISPLAY_Y1 <= KEYPAD_TOP);

#[cfg(test)]
mod tests {
    use super::*;
    use interos_std::font;

    /// Every selectable text size keeps its ink inside the plate it is drawn on.
    ///
    /// WHY this is a run-time test and not a `const _: () = assert!`: the
    /// person's level is one atomic the settings store fills at run time, so a
    /// compile-time gate can only ever speak for the ladder. What the gates at
    /// the foot of this file say about the ladder, this says about all four
    /// levels -- including the one the app has no compile-time number for.
    ///
    /// WHY every call below takes its level as an argument: the published cell
    /// is one per process and the test harness runs test functions on parallel
    /// threads, so a check that wrote or read the cell would race with the
    /// tape's own tests. The pure arithmetic answers the same question and
    /// cannot be moved by another thread.
    #[test]
    fn no_selectable_level_overflows_its_plate() {
        for level in font::TEXT_SCALE_COMPACT..=font::TEXT_SCALE_EXTRA_LARGE {
            // The display card stops short of the keypad at every level.
            assert!(
                card_y1_at(run_rows_at(typography::DISPLAY_CAP_PX, level), level) <= KEYPAD_TOP,
                "level {level}: the card reaches the keypad",
            );
            assert!(
                result_rows_room_at(level) >= run_rows_at(typography::SECTION_CAP_PX, level),
                "level {level}: the card has no room left for the smallest result rung",
            );

            // A pill that does not move never has its label painted past its
            // own bottom edge, and is never drawn smaller than the ladder.
            for (base, plate) in [
                (typography::BODY_CAP_PX, PILL_H),
                (typography::BODY_CAP_PX, HISTORY_PILL.h as usize),
                (typography::TITLE_CAP_PX, crate::KEYPAD_ROW_H),
            ] {
                let cap = cap_in_plate_at(base, plate, level);
                if run_rows(base) <= plate {
                    assert!(
                        run_rows(cap) <= plate,
                        "level {level}: a {base} cap in a {plate}-row plate overflows it",
                    );
                } else {
                    assert_eq!(
                        cap, base,
                        "level {level}: a plate the ladder itself does not fit was drawn smaller",
                    );
                }
                assert!(cap <= cap_at(base, level));
                assert!(
                    cap >= font::scale_cap_height(base, font::TEXT_SCALE_COMPACT),
                    "level {level}: the clamp went below the ladder's smallest offering",
                );
            }

            // The drawer's rows stay inside the sheet, and one row holds both
            // of its runs.
            let pitch = history_pitch_at(level);
            assert!(HIST_LIST_Y0 + history_visible_rows_at(level) * pitch <= DRAWER_SHEET_Y1);
            assert!(history_row_h_at(level) <= pitch);
            assert!(
                HIST_EXPR_DY + run_rows_at(typography::TITLE_CAP_PX, level) <= history_row_h_at(level)
            );

            // The tape's painted plate and the plate the press answers are one
            // plate, and it never leaves the card.
            //
            // WHY the floor is the platform's touch target and not the ladder's
            // plate height: at the smallest text size the tape really is shorter
            // than the ladder's two caption lines, and the chip that scrolls it
            // shortens with it -- what may not shorten is the thumb's floor, so
            // that is what is checked here beside the card's own bottom edge.
            let plate = scroll_plate_at(level);
            assert!(plate.h >= MIN_TARGET);
            assert!(plate.w >= MIN_TARGET);
            assert!(plate.y + plate.h <= card_y1_at(result_rows_room_at(level), level));
        }
    }

    /// At the platform's default level nothing about this file's numbers moved.
    ///
    /// WHY this matters: every layout constant in the tree, and the truth
    /// contracts that read them, are authored against the ladder. A surface
    /// that adopted the setting and resized the page at one hundred per cent
    /// would be resizing screens that never asked for it.
    #[test]
    fn the_default_level_draws_the_ladder() {
        let normal = font::TEXT_SCALE_NORMAL;
        assert_eq!(tape_line_rows_at(normal), TAPE_LINE_H);
        assert_eq!(tape_y1_at(normal), TAPE_Y1);
        assert_eq!(value_y0_at(normal), VALUE_Y0);
        assert_eq!(history_pitch_at(normal), HIST_ROW_PITCH);
        assert_eq!(history_row_h_at(normal), HIST_ROW_H);
        assert_eq!(history_visible_rows_at(normal), crate::HISTORY_MAX_ROWS);
        assert_eq!(scroll_plate_at(normal), SCROLL_PLATE);
        assert_eq!(card_y1_at(run_rows_at(typography::DISPLAY_CAP_PX, normal), normal), DISPLAY_Y1);
        assert_eq!(cap_at(typography::BODY_CAP_PX, normal), typography::BODY_CAP_PX);
        assert_eq!(
            cap_in_plate_at(typography::BODY_CAP_PX, PILL_H, normal),
            typography::BODY_CAP_PX
        );
    }

    /// A larger setting really does move the ink and the box together.
    ///
    /// WHY this is checked here: the defect this file exists to fix is a page
    /// that reads the setting and then draws the ladder anyway. The pair below
    /// is the proof that the run and the space it reserves both follow the
    /// level, which is what the app's own renderer is built out of.
    #[test]
    fn the_box_follows_the_ink_at_every_level() {
        for level in font::TEXT_SCALE_COMPACT..=font::TEXT_SCALE_EXTRA_LARGE {
            for base in [
                typography::CAPTION_CAP_PX,
                typography::BODY_CAP_PX,
                typography::SECTION_CAP_PX,
                typography::TITLE_CAP_PX,
                typography::DISPLAY_CAP_PX,
            ] {
                assert_eq!(run_rows_at(base, level), run_rows(cap_at(base, level)));
            }
            assert!(
                tape_line_rows_at(font::TEXT_SCALE_EXTRA_LARGE)
                    > tape_line_rows_at(font::TEXT_SCALE_NORMAL),
                "the tape's stride must grow with the caption it draws",
            );
            assert!(
                history_row_h_at(font::TEXT_SCALE_EXTRA_LARGE) > history_row_h_at(font::TEXT_SCALE_NORMAL),
                "the ledger's row must grow with the two runs it holds",
            );
        }
    }
}
