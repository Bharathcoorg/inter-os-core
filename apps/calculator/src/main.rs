//! InterOS Core Open-Source Reference App: Interactive Touch Calculator
//!
//! Full production Ring-3 graphical application executing in User Mode:
//! - DRM/KMS Client Surface allocation (z=200) and double-buffered scanout presentation
//! - High-contrast dual-line digital expression and evaluation result display
//! - 6x4 rounded mobile keypad matrix with semantic design tokens and minimum 64pt targets
//! - Distinct clear-all, clear-entry and backspace keys, and a modulo key labelled for what it does
//! - A scrollable expression tape that holds the whole expression, not its last characters
//! - Interactive touch event processing and button hit-testing
//! - Full arithmetic execution engine: Add, Subtract, Multiply, Divide, Modulo, Clear, Evaluate
//! - History drawer / overlay for past calculations ledger
//! - Banded rendering (`render_calculator_band`): only dirty rows are re-rasterised
//! - Graceful lifecycle supervision and clean return to Launcher

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use calculator::theme::{THEME, radius, typography};

use interos_std::design::accessibility::{DynamicType, UserTextScale};
use interos_std::design::components;
use interos_std::design::primitives::Rect;


/// The platform's corner radii, under the short names the renderer uses.
///
/// WHY the aliases exist: the corner test names its radius four times, and a
/// number that appears four times is a number that can drift from the
/// platform. The alias resolves to the platform's step, so the drift is the
/// platform's and not this app's.
const R_CHIP: usize = radius::CHIP;
const R_CONTROL: usize = radius::CONTROL;
#[allow(dead_code)]
const R_MAJOR: usize = radius::MAJOR;
const R_SHEET: usize = radius::SHEET;
const R_SOFT: usize = radius::SOFT_PILL;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

const STATUS_BAR_HEIGHT: usize = interos_std::design::viewport::STATUS_BAR_HEIGHT;

pub fn status_band() -> (usize, usize) {
    (0, STATUS_BAR_HEIGHT)
}

fn refresh_clock(calc: &mut CalculatorState) {
    if let Some(now) = std::rtc::Reading::now() {
        calc.clock = now.clock_text();
        calc.clock_ok = true;
    } else {
        calc.clock_ok = false;
    }
}

/// The text-size level the person has set, handed to the renderer.
///
/// WHY this exists at all: Settings carries a "Text size" control, and the
/// platform's type ladder is read at run time through the `*_user` helpers in
/// `interos_std::font` -- but those helpers answer at a process-wide cell that
/// only an application's own publish can fill. Nothing in this app read the
/// store, so the cell kept its default and the calculator drew one hundred per
/// cent whatever the person had chosen. The display measures and lays its card
/// out from `chrome::cap_user` and the boxes beside it, and those are the faces
/// of the ladder that read the cell: this is the hop that makes them mean
/// anything.
///
/// WHY before the first frame is presented: the alternative is a paint loop
/// that asks the settings store per run, and the store answers with a syscall.
/// The level is read once, here, and the rung it names is what every row of the
/// first frame is already built out of.
///
/// WHY the rung is left alone when the store does not answer or names nothing
/// this app can read: a value the app cannot read is not a level the person
/// chose, and publishing a rung the store never named would resize the screen on
/// a guess. The cell's own default -- one hundred per cent, the ladder -- stands.
///
/// Returns whether the published level differs from the one the last frame was
/// laid out with, which is the caller's signal that the whole screen has to be
/// repainted and not just the rows something else dirtied.
fn publish_text_scale() -> bool {
    let mut buf = [0u8; 32];
    let mut want = None;
    if let Ok(n) = std::settings_get("display.text_scale", &mut buf) {
        // WHY the shared parser and not this app's own chain of compares: the
        // store accepts both the digit and the word for a level, and a reader
        // that understood fewer spellings than the writer produces would read an
        // existing preference as "unset" and quietly reset the rung.
        want = UserTextScale::from_settings_value(&buf[..n]);
    }
    // WHY the level the process already holds when the store did not answer:
    // a frame that fell back to one hundred per cent because the store was busy
    // would resize the screen under a person who never touched the setting.
    let before = DynamicType::current().scale;
    let next = want.unwrap_or(before);
    // WHY the answer is the rung actually stored and not the one asked for: a
    // level above the ladder's largest setting is clamped on the way in, and a
    // caller that repaints the screen because "the level changed" should be
    // wrong about that only when the pixels really did move.
    DynamicType::new(next).publish() != before
}

pub use calculator::{CalcError, CalculatorState, Decimal};
// -----------------------------------------------------------------------------
// Visual Design Tokens
//
// WHY there is no palette here any more: the app used to carry its own
// colours, which is a second design system inside one app. Every colour now
// resolves through the platform's theme, so a third-party app that asks the
// SDK for the same roles draws the same calculator. See `calculator::theme`
// for the mapping from the old names to the platform's roles.
//
// WHY the keypad's buttons are the roles they are: a digit is a control on a
// container, an operator is a control on a warning-tinted container, and a
// destructive key is a control on an error-tinted container. The platform
// names each of those, so the keypad does not have to.
// -----------------------------------------------------------------------------
// -----------------------------------------------------------------------------
// Canonical InterOS Typography
// -----------------------------------------------------------------------------
/// The colour of (`x`, `y`) if the text inks it, blended over `bg`.
///
/// WHY the platform's own text drawing and not a second one: the app used
/// to place the run itself, out of the bitmap face's multiples, which
/// meant the app and the platform could disagree about where a run sits and
/// how wide it is. The platform draws a run at a cap height, and a cap
/// height is the size the design ladder names in the units a reader sees,
/// so that is what the app asks for.
#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    cap_px: u32,
    text: &[u8],
    color: u32,
    bg: u32,
    bold: bool,
) -> Option<u32> {
    if cap_px == 0 || text.is_empty() {
        return None;
    }
    let max_h = (cap_px as usize).saturating_mul(2) + 8;
    if y < y0 || y >= y0 + max_h {
        return None;
    }
    let max_w = text.len().saturating_mul((cap_px as usize).saturating_mul(2) + 4) + 16;
    if x < x0 || x >= x0 + max_w {
        return None;
    }
    interos_std::design::monochrome::text_px(x, y, x0, y0, cap_px, text, color, bg, bold)
}

/// The line box of a run drawn at a cap height, and the rectangles every
/// control of this app is painted in and pressed in.
///
/// WHY they come from the library and not from this file: the painter and the
/// tap handler are two functions, and while each held its own numbers the
/// drawer's Clear pill stood forty rows against the platform's forty-four,
/// the home strip answered a two-hundred-and-forty-wide box around a pill one
/// hundred and forty across, and the columns beside the back chip quit the
/// app from page that had never been inked as a button. One declaration is
/// also the only version of it the app's own tests can reach: this file is a
/// binary, and a binary is built without a test harness.
use calculator::chrome::{
    CLEAR_PILL, CLOSE_PILL, DISPLAY_PAD, DISPLAY_X0, DISPLAY_X1, DISPLAY_Y0, DRAWER_HEADER_Y0,
    DRAWER_HEADER_Y1, HISTORY_PILL, HIST_EXPR_DY, HIST_LIST_Y0, HOME_PILL, KEYPAD_TOP,
    SCROLL_CHEVRON_X, SCROLL_PLATE_W, SCROLL_PLATE_X, TAPE_LINES, TAPE_TEXT_X1, TAPE_Y0, VALUE_GAP,
    run_rows,
};
use calculator::chrome;

const HEADER_H: usize = 110;

/// The row the header's title run starts at.
///
/// WHY named: it used to be the literal `60` in the painter, and the plate the
/// title is clamped into is the distance between that row and the header's
/// bottom -- two numbers that have to be read together to mean anything.
const HEADER_TITLE_Y0: usize = 60;

/// The row the drawer's own title starts at, and the row its empty notice
/// starts at.
///
/// WHY named: both used to be literals inside the painter, and both are now the
/// top of a plate the run is clamped into -- a plate is two numbers, not one.
const DRAWER_TITLE_Y0: usize = 136;
const EMPTY_NOTE_Y0: usize = 320;

/// The cap heights the display draws at.
///
/// WHY the rungs and not the scales: the scale ladder and the cap ladder
/// are not the same ladder -- a rung names a scale, and the cap it draws is
/// a different number -- and these are the sizes the design ladder names in
/// the units a reader sees, taken from `typography` rather than restated.
const CAP_EXPRESSION: u32 = calculator::EXPRESSION_CAP_PX;
const CAP_RESULT: u32 = typography::DISPLAY_CAP_PX;
const CAP_LABEL: u32 = typography::SECTION_CAP_PX;
const CAP_SECTION: u32 = typography::TITLE_CAP_PX;

/// The cap a key label is drawn at.
///
/// WHY it comes from the engine and not the section rung: `lib.rs` declares
/// `KEY_LABEL_CAP_PX` as the body rung, and the reference calculator's key
/// faces stand a body's height. A label drawn at the section rung is a third
/// larger than the size the engine names for it, so the two ladders drift.
/// The keypad honours the engine's declaration.
const CAP_KEY_LABEL: u32 = calculator::KEY_LABEL_CAP_PX;

// WHY these gates exist: the two runs above share the row's column and the
// row's own height, so the relations that keep them off each other and off
// the row's bottom edge are checked in `chrome`, where the numbers are set.
// What is checked here is the one relation that belongs to this file: the key
// label inside the keycap it is drawn on.
const _: () = assert!(run_rows(CAP_KEY_LABEL) <= BTN_H);

/// The rungs the result comes down when it does not fit the card.
const RESULT_LADDER: [u32; 4] = [
    CAP_RESULT,
    typography::HEADING_CAP_PX,
    typography::TITLE_CAP_PX,
    typography::SECTION_CAP_PX,
];

/// The rungs the error line comes down when it does not fit the card.
const ERROR_LADDER: [u32; 3] = [
    typography::TITLE_CAP_PX,
    typography::SECTION_CAP_PX,
    typography::BODY_CAP_PX,
];

/// The right edge of the tape's text when the scroll plate is shown.
///
/// WHY the text moves in: the plate is the only way to reach the part of the
/// expression that is off the screen, and text drawn under it would hide the
/// control the person is looking for.
const TAPE_TEXT_X1_SCOLL: usize = SCROLL_PLATE_X.saturating_sub(6);

// Button layout constants (Minimum 64pt targets: 144x140px)
//
// WHY there are six rows and not five: the keypad carries twenty-two keys
// and a four-column grid holds sixteen in four rows. The clear-all,
// clear-entry and backspace keys are three keys a person has to be able
// to tell apart, and a five-row grid had no room for them.
const BTN_W: usize = 144;
const BTN_H: usize = calculator::KEYPAD_ROW_H;
const COL_X: [usize; 4] = [28, 194, 360, 526];
const ROW_Y: [usize; 6] = [420, 580, 740, 900, 1060, 1220];

/// The width of a key that spans two columns of the grid.
const WIDE_BTN_W: usize = BTN_W * 2 + (COL_X[1] - COL_X[0] - BTN_W);

/// The x of the wide key in the last row's right half.
const WIDE_BTN_X1: usize = COL_X[2];

fn draw_calc_badge_operator_glyph(x: usize, y: usize, cx: usize, cy: usize, op: char, color: u32) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    match op {
        '/' => {
            if dx.abs() <= 9 && dy.abs() <= 2 {
                return Some(color);
            }
            if dx * dx + (dy + 8) * (dy + 8) <= 6 {
                return Some(color);
            }
            if dx * dx + (dy - 8) * (dy - 8) <= 6 {
                return Some(color);
            }
            None
        }
        '*' => {
            let d1 = (dx - dy).abs();
            let d2 = (dx + dy).abs();
            if dx.abs() <= 8 && dy.abs() <= 8 && (d1 <= 2 || d2 <= 2) {
                return Some(color);
            }
            None
        }
        '-' => {
            if dx.abs() <= 9 && dy.abs() <= 2 {
                return Some(color);
            }
            None
        }
        '+' => {
            if (dx.abs() <= 9 && dy.abs() <= 2) || (dx.abs() <= 2 && dy.abs() <= 9) {
                return Some(color);
            }
            None
        }
        _ => None,
    }
}

fn draw_calc_operator_glyph(x: usize, y: usize, cx: usize, cy: usize, op: char, color: u32) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    match op {
        '/' => {
            // Division '÷': horizontal bar and top/bottom dots
            if dx.abs() <= 20 && dy.abs() <= 3 {
                return Some(color);
            }
            if dx * dx + (dy + 18) * (dy + 18) <= 18 {
                return Some(color);
            }
            if dx * dx + (dy - 18) * (dy - 18) <= 18 {
                return Some(color);
            }
            None
        }
        '*' => {
            // Multiplication '×': two crossing diagonal bars
            let d1 = (dx - dy).abs();
            let d2 = (dx + dy).abs();
            if dx.abs() <= 18 && dy.abs() <= 18 && (d1 <= 3 || d2 <= 3) {
                return Some(color);
            }
            None
        }
        '-' => {
            // Subtraction '−': clean horizontal bar
            if dx.abs() <= 20 && dy.abs() <= 3 {
                return Some(color);
            }
            None
        }
        '+' => {
            // Addition '+': symmetric cross
            if (dx.abs() <= 20 && dy.abs() <= 3) || (dx.abs() <= 3 && dy.abs() <= 20) {
                return Some(color);
            }
            None
        }
        '=' => {
            // Equals '=': two parallel horizontal bars
            if dx.abs() <= 20 && ((dy + 8).abs() <= 3 || (dy - 8).abs() <= 3) {
                return Some(color);
            }
            None
        }
        _ => None,
    }
}


/// The scroll plate's chevron.
///
/// WHY it is drawn and not typed: the platform's typeface carries no
/// chevron character, and a control labelled with a character the face
/// does not carry would draw as a blank.
///
/// WHY its centre comes from the plate: the plate is what the person sees
/// and what the press answers, and a glyph that stood eighteen rows into a
/// band of its own was a glyph the plate's halves did not agree with.
fn tape_chevron_pixel(x: usize, y: usize, up: bool, color: u32, plate_h: usize) -> Option<u32> {
    let cx = SCROLL_CHEVRON_X as isize;
    // WHY the plate's own height and not the ladder's: the two chevrons mark
    // the plate's two halves, and the plate is the tape at this frame's text
    // size. Marking a grown chip at the ladder's quarters puts one chevron on
    // the line the person is already reading.
    let cy = if up {
        (TAPE_Y0 + plate_h / 4) as isize
    } else {
        (TAPE_Y0 + plate_h * 3 / 4) as isize
    };
    let dx = (x as isize) - cx;
    let dy = (y as isize) - cy;
    if dx.abs() > 9 || dy.abs() > 7 {
        return None;
    }
    // The two arms run from the point at the centre out to the corners.
    let along = if up { dy + 6 } else { 6 - dy };
    if !(0..=10).contains(&along) {
        return None;
    }
    let reach = along * 7 / 10;
    if (dx.abs() - reach).abs() <= 1 {
        return Some(color);
    }
    None
}

/// The rectangle of the key at (row, col), if the grid has one there.
///
/// WHY the last row is two wide keys: the keypad carries twenty-two keys
/// and a four-column grid holds sixteen in four rows, so the bottom row
/// holds the two keys that are wider than one cell.
fn key_rect(row: usize, col: usize) -> Option<Rect> {
    match (row, col) {
        (5, 0) => Some(Rect::new(COL_X[0], ROW_Y[5], WIDE_BTN_W, BTN_H)),
        (5, 2) => Some(Rect::new(WIDE_BTN_X1, ROW_Y[5], WIDE_BTN_W, BTN_H)),
        (5, _) => None,
        (r, c) if r < 5 && c < 4 => Some(Rect::new(COL_X[c], ROW_Y[r], BTN_W, BTN_H)),
        _ => None,
    }
}

/// The action of the key at (row, col), if there is one.
fn get_button_action(row: usize, col: usize) -> Option<char> {
    calculator::button_action(row, col)
}

/// The distances from a pixel to the edges of the key at (row, col).
///
/// WHY the distances and not a boolean: the renderer draws the key's own
/// hairline from them and the hit test answers from the same numbers, so
/// the key that is drawn and the key that is pressed are the same shape.
///
/// WHY the shape is the keycap and not the grid cell: the key that is
/// drawn used to be four pixels smaller than the cell it was hit in, so
/// the gap between two keys belonged to one of them and a press in that gap
/// answered a key that was not there.
fn key_edges(row: usize, col: usize, x: usize, y: usize) -> Option<(usize, usize)> {
    let rect = key_rect(row, col)?;
    if x < rect.x || x >= rect.x + rect.w || y < rect.y || y >= rect.y + rect.h {
        return None;
    }
    let rx = (x - rect.x).min(rect.x + rect.w - 1 - x);
    let ry = (y - rect.y).min(rect.y + rect.h - 1 - y);
    // Full stadium pill shape (radius = height / 2)
    let r = rect.h / 2;
    if rx < r && ry < r {
        let cdx = (r - rx) as isize;
        let cdy = (r - ry) as isize;
        if cdx * cdx + cdy * cdy > (r * r) as isize {
            return None;
        }
    }
    Some((rx, ry))
}

/// Whether the pixel is on the key at (row, col). Uses full rectangular cell for generous touch target.
fn key_hit(row: usize, col: usize, x: usize, y: usize) -> bool {
    let Some(rect) = key_rect(row, col) else { return false; };
    x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
}

// The home indicator's ink (`HOME_PILL`) and the strip that answers it
// (`HOME_HIT`) are both declared in `calculator::chrome`, beside the numbers
// the press asks.

#[derive(Clone, Copy)]
struct CalcDisplayPlan {
    has_tape: bool,
    tape_xs: [usize; calculator::TAPE_MAX_LINES],
    tape_count: usize,
    tape_first: usize,
    tape_overflow: bool,
    is_error: bool,
    err_text: &'static [u8],
    err_x: usize,
    err_cap: u32,
    digits_len: usize,
    digits_cap: u32,
    digits_x: usize,
    /// The cap the tape is drawn at, the tape's own line box, and the three
    /// rows the card is built out of -- all of them at the text-size level this
    /// process publishes.
    ///
    /// WHY they ride in the plan and are not read where they are drawn: the
    /// pixel engine runs once per pixel of the band, and a box that the display
    /// re-derived per pixel would answer seven hundred and twenty thousand
    /// times the question the frame asks once. The plan is built once per
    /// repaint, and both the painter and the press read what it settled on, so
    /// the card cannot be painted at one size and pressed at another.
    expr_cap: u32,
    tape_rows: usize,
    tape_y1: usize,
    value_y0: usize,
    card_y1: usize,
}

static mut CALC_PLAN: Option<CalcDisplayPlan> = None;

/// The plan a display band with nothing typed in it draws.
///
/// WHY it exists: the painter reads the plan for its geometry, and the first
/// frames of the app -- and any band repaint that runs before the plan is
/// built -- must not answer with a card of no rows. This is the ladder's own
/// card, which is exactly what the app drew before it had a plan.
const DEFAULT_PLAN: CalcDisplayPlan = CalcDisplayPlan {
    has_tape: false,
    tape_xs: [0; calculator::TAPE_MAX_LINES],
    tape_count: 0,
    tape_first: 0,
    tape_overflow: false,
    is_error: false,
    err_text: b"",
    err_x: 0,
    err_cap: ERROR_LADDER[0],
    digits_len: 0,
    digits_cap: RESULT_LADDER[0],
    digits_x: 0,
    expr_cap: CAP_EXPRESSION,
    tape_rows: calculator::chrome::TAPE_LINE_H,
    tape_y1: calculator::chrome::TAPE_Y1,
    value_y0: calculator::chrome::VALUE_Y0,
    card_y1: calculator::chrome::DISPLAY_Y1,
};

/// The width of the display's content column.
fn display_column_w() -> usize {
    (DISPLAY_X1 - DISPLAY_PAD).saturating_sub(DISPLAY_X0 + DISPLAY_PAD)
}

fn prepare_calc_plan(calc: &CalculatorState) {
    // The card's geometry at the level this process publishes. The tape grows
    // its own line box, the result stands below it, and the whole card is
    // clamped clear of the keypad -- see `chrome::KEYPAD_TOP`.
    let expr_cap = chrome::cap_user(CAP_EXPRESSION);
    let tape_rows = chrome::run_rows(expr_cap);
    let tape_y1 = TAPE_Y0 + TAPE_LINES * tape_rows;
    let value_y0 = tape_y1 + VALUE_GAP;
    let room = KEYPAD_TOP.saturating_sub(DISPLAY_PAD).saturating_sub(value_y0);
    let mut plan = CalcDisplayPlan {
        has_tape: false,
        tape_xs: [0; calculator::TAPE_MAX_LINES],
        tape_count: 0,
        tape_first: 0,
        tape_overflow: false,
        is_error: false,
        err_text: b"",
        err_x: 0,
        err_cap: ERROR_LADDER[0],
        digits_len: 0,
        digits_cap: RESULT_LADDER[0],
        digits_x: 0,
        expr_cap,
        tape_rows,
        tape_y1,
        value_y0,
        card_y1: chrome::DISPLAY_Y1,
    };
    let column = display_column_w();
    if calc.tape_count > 0 {
        plan.tape_count = calc.tape_count;
        // WHY the scroll model stays in the ladder's rows: the engine counts
        // the expression in lines and the offset in the ladder's own pixels, and
        // a scroll whose units changed when the person changed their text size
        // would land the tape on a different line than the one it was scrolled
        // to. The line *index* is what carries over; the pixels it draws in are
        // this frame's.
        plan.tape_first = calc.tape_scroll.offset / chrome::TAPE_LINE_H;
        plan.tape_overflow = calc.tape_count > TAPE_LINES;
        let right = if plan.tape_overflow {
            TAPE_TEXT_X1_SCOLL
        } else {
            TAPE_TEXT_X1
        };
        for index in 0..calc.tape_count.min(calculator::TAPE_MAX_LINES) {
            let width =
                calculator::text_width(&calc.tape_lines[index][..calc.tape_lens[index]], expr_cap);
            plan.tape_xs[index] = right.saturating_sub(width);
        }
        plan.has_tape = true;
    }
    if calc.error || calc.calc_error.is_some() {
        let err_text: &'static [u8] = match calc.calc_error {
            Some(CalcError::DivisionByZero) => b"Error: Div by zero",
            Some(CalcError::Overflow) => b"Error: Overflow",
            Some(CalcError::UnbalancedParentheses) => b"Error: Unbalanced ()",
            Some(CalcError::EmptyExpression) => b"Error: Empty expr",
            _ => b"Error",
        };
        // WHY the error is fitted to the column: the error line used to be
        // drawn too large and ran past the card's right edge, so the end of
        // the message was outside the card it belongs to.
        //
        // WHY the rungs are read at the published level and then fitted to the
        // rows the card has left: the person's text size makes the message
        // wider *and* taller, and the card is bounded below by the keypad. A
        // rung that fits the column but not the card would be painted past the
        // card's own bottom edge, so both bounds are answered together by
        // `fit_display_cap`.
        plan.err_cap = fit_display_cap(&ERROR_LADDER, err_text, column, room);
        plan.err_x = (DISPLAY_X1 - DISPLAY_PAD)
            .saturating_sub(calculator::text_width(err_text, plan.err_cap));
        plan.err_text = err_text;
        plan.is_error = true;
        plan.card_y1 = chrome::card_y1(run_rows(plan.err_cap));
    } else {
        let digits = &calc.display_buf[..calc.display_len];
        // WHY the result comes down a rung when it does not fit: the length
        // of a number is not the width of it, and a long number at the title
        // rung would run off the card.
        plan.digits_cap = fit_display_cap(&RESULT_LADDER, digits, column, room);
        let total_w = calculator::text_width(digits, plan.digits_cap);
        plan.digits_x = (DISPLAY_X1 - DISPLAY_PAD).saturating_sub(total_w);
        plan.digits_len = calc.display_len;
        plan.card_y1 = chrome::card_y1(run_rows(plan.digits_cap));
    }
    unsafe {
        CALC_PLAN = Some(plan);
    }
}

/// The cap a display run is drawn at, out of the rungs `ladder` offers, at the
/// text-size level this process publishes.
///
/// WHY the width and the height are answered together: the person's setting
/// scales both, and the card the run stands in is bounded on the right by its
/// own column and below by the keypad. A rung chosen for its width alone is a
/// rung that can clear the column and still be painted past the card.
///
/// WHY the fallback clamps instead of shrinking: if no rung on the ladder fits
/// the rows the card has left at the level the person asked for, the run is
/// drawn at the smallest rung clamped into those rows -- whose floor is that
/// rung's own ladder size, so the number is never made smaller than the size
/// the platform names for it. What a person loses at that extreme is the rung
/// they asked for, and it is stated here rather than painted over the keypad.
fn fit_display_cap(ladder: &[u32], text: &[u8], column: usize, room: usize) -> u32 {
    let level = chrome_level();
    let smallest = ladder[ladder.len() - 1];
    for rung in ladder.iter() {
        let cap = chrome::cap_at(*rung, level);
        if calculator::text_width(text, cap) <= column && chrome::run_rows(cap) <= room {
            return cap;
        }
    }
    chrome::cap_in_plate(smallest, room)
}

/// The text-size level this process publishes, for the plan's arithmetic.
///
/// WHY read once per frame and passed down: the plan is built once per repaint
/// and the ladder below it is answered at that one level, so a frame cannot
/// paint the tape at one rung and the result at another.
#[inline]
fn chrome_level() -> u8 {
    interos_std::font::user_text_scale()
}

// -----------------------------------------------------------------------------
// Pixel Color Resolution Engine
// -----------------------------------------------------------------------------
#[inline(never)]
fn get_calculator_pixel(x: usize, y: usize, calc: &CalculatorState) -> u32 {
    // 0. Status Band (y < STATUS_BAR_HEIGHT)
    if y < STATUS_BAR_HEIGHT {
        let band = components::StatusBand::new(W);
        let clock = if calc.clock_ok { Some(&calc.clock[..]) } else { None };
        return band
            .pixel(x, y, clock, &components::Instruments::unmeasured(), &THEME)
            .unwrap_or(THEME.surface);
    }

    // 1. Header row: title, back chevron and history toggle (y: 48..110)
    if y < HEADER_H {
        // Standardized back button squircle (24..72, HEADER_TOP..HEADER_TOP+48)
        if let Some(c) = std::design::back_button::pixel_for_theme(x, y, &THEME) {
            return c;
        }

        // Canonical App Title: "Calculator" vertically centered with back button and history pill
        //
        // WHY the cap comes from `cap_in_plate`: the header is one hundred and
        // ten rows and the display card owns the row below it, so this plate
        // cannot grow with the person's text size -- the run's own box is already
        // taller than the band it stands in at the ladder's title rung. The
        // guard therefore clamps the run into the plate it has and never below
        // the size the ladder draws there today, which keeps the title on the
        // row it is on now and stops a larger setting from painting further past
        // the band. Growing it is a layout change the card below does not have
        // room for, not a font change.
        let title_cap = chrome::cap_in_plate(
            interos_std::design::typography::TITLE_CAP_PX,
            HEADER_H - HEADER_TITLE_Y0,
        );
        if let Some(c) = test_text(x, y, 92, HEADER_TITLE_Y0, title_cap, b"Calculator", THEME.on_surface, THEME.surface, true) {
            return c;
        }

        // History toggle pill: the rect the press answers, drawn.
        if HISTORY_PILL.contains(x, y) {
            let rx = (x - HISTORY_PILL.x).min(HISTORY_PILL.x + HISTORY_PILL.w - 1 - x);
            let ry = (y - HISTORY_PILL.y).min(HISTORY_PILL.y + HISTORY_PILL.h - 1 - y);
            if rx < R_SOFT && ry < R_SOFT {
                let cdx = (R_SOFT - rx) as isize;
                let cdy = (R_SOFT - ry) as isize;
                if cdx * cdx + cdy * cdy > (R_SOFT * R_SOFT) as isize {
                    return THEME.surface;
                }
            }
            let (bg, border, text_c) = if calc.show_history {
                (THEME.warning, THEME.warning_container, THEME.on_primary)
            } else {
                (THEME.surface_container_high, THEME.border, THEME.on_surface)
            };
            if rx < 2 || ry < 2 {
                return border;
            }
            let hw_cap = chrome::cap_in_plate(typography::BODY_CAP_PX, HISTORY_PILL.h);
            let hw = calculator::text_width(b"History", hw_cap);
            let hx = HISTORY_PILL.x + (HISTORY_PILL.w - hw) / 2;
            // WHY the line box and not the cap: a run reserves its ascent and
            // its descent, and centring the cap inside a forty-eight-row pill
            // left the box's last two rows painted on the page under it.
            let hy = HISTORY_PILL.y + (HISTORY_PILL.h - run_rows(hw_cap)) / 2;
            if let Some(c) = test_text(x, y, hx, hy, hw_cap, b"History", text_c, bg, true) {
                return c;
            }
            return bg;
        }

        return THEME.surface;
    }

    // 2. History Drawer / Overlay (Active when calc.show_history is true)
    if calc.show_history {
        if y >= 110 && y < 1380 && x >= DISPLAY_X0 && x < DISPLAY_X1 {
            let rx = (x - DISPLAY_X0).min(DISPLAY_X1 - 1 - x);
            let ry = (y - 110).min(1379 - y);
            if rx < R_SHEET && ry < R_SHEET {
                let cdx = (R_SHEET - rx) as isize;
                let cdy = (R_SHEET - ry) as isize;
                if cdx * cdx + cdy * cdy > (R_SHEET * R_SHEET) as isize {
                    return THEME.surface;
                }
            }

            // Header of History Drawer: y: DRAWER_HEADER_Y0..DRAWER_HEADER_Y1
            if y >= DRAWER_HEADER_Y0 && y < DRAWER_HEADER_Y1 {
                // "Calculation History" title
                //
                // WHY clamped to the drawer's own header band: the band is the
                // pill's height plus four rows, the pills are the platform's
                // minimum target and do not move, and a run scaled past the band
                // is painted over the separator and the first ledger row. The
                // clamp keeps the title inside the header it belongs to.
                let drawer_cap = chrome::cap_in_plate(
                    typography::SECTION_CAP_PX,
                    DRAWER_HEADER_Y1 - DRAWER_TITLE_Y0,
                );
                if let Some(c) = test_text(x, y, 48, DRAWER_TITLE_Y0, drawer_cap, b"Calculation History", THEME.on_surface, THEME.surface_container, true) {
                    return c;
                }

                // Clear button pill: the rect the press answers, drawn.
                if CLEAR_PILL.contains(x, y) {
                    let prx = (x - CLEAR_PILL.x).min(CLEAR_PILL.x + CLEAR_PILL.w - 1 - x);
                    let pry = (y - CLEAR_PILL.y).min(CLEAR_PILL.y + CLEAR_PILL.h - 1 - y);
                    if prx < R_CHIP && pry < R_CHIP {
                        let cdx = (R_CHIP - prx) as isize;
                        let cdy = (R_CHIP - pry) as isize;
                        if cdx * cdx + cdy * cdy > (R_CHIP * R_CHIP) as isize {
                            return THEME.surface_container;
                        }
                    }
                    if prx < 2 || pry < 2 {
                        return THEME.border;
                    }
                    let clear_cap = chrome::cap_in_plate(typography::BODY_CAP_PX, chrome::PILL_H);
                    let cw = calculator::text_width(b"CLEAR", clear_cap);
                    let cx0 = CLEAR_PILL.x + (CLEAR_PILL.w - cw) / 2;
                    let cy0 = chrome::PILL_TOP + (chrome::PILL_H - run_rows(clear_cap)) / 2;
                    if let Some(c) = test_text(x, y, cx0, cy0, clear_cap, b"CLEAR", THEME.error, THEME.surface_container_high, true) {
                        return c;
                    }
                    return THEME.surface_container_high;
                }

                // Close button pill: the rect the press answers, drawn.
                if CLOSE_PILL.contains(x, y) {
                    let prx = (x - CLOSE_PILL.x).min(CLOSE_PILL.x + CLOSE_PILL.w - 1 - x);
                    let pry = (y - CLOSE_PILL.y).min(CLOSE_PILL.y + CLOSE_PILL.h - 1 - y);
                    if prx < R_CHIP && pry < R_CHIP {
                        let cdx = (R_CHIP - prx) as isize;
                        let cdy = (R_CHIP - pry) as isize;
                        if cdx * cdx + cdy * cdy > (R_CHIP * R_CHIP) as isize {
                            return THEME.surface_container;
                        }
                    }
                    if prx < 2 || pry < 2 {
                        return THEME.border;
                    }
                    let close_cap = chrome::cap_in_plate(typography::BODY_CAP_PX, chrome::PILL_H);
                    let cw = calculator::text_width(b"CLOSE", close_cap);
                    let cx0 = CLOSE_PILL.x + (CLOSE_PILL.w - cw) / 2;
                    let cy0 = chrome::PILL_TOP + (chrome::PILL_H - run_rows(close_cap)) / 2;
                    if let Some(c) = test_text(x, y, cx0, cy0, close_cap, b"CLOSE", THEME.on_surface, THEME.surface_container_high, true) {
                        return c;
                    }
                    return THEME.surface_container_high;
                }
            }

            // Separator line
            if y == 182 && x >= 40 && x < 680 {
                return THEME.border;
            }

            // History Entries
            let count = calc.history.count();
            // WHY the platform's list-row treatment: the row's fill, its
            // radius and its divider come from the platform, so a history
            // row here is the same row an app that lists things would
            // draw.
            let row_style = components::list_row_style(&THEME, false);
            if count == 0 {
                let msg = b"No recent calculations";
                let msg_cap = chrome::cap_in_plate(
                    typography::SECTION_CAP_PX,
                    chrome::DRAWER_SHEET_Y1 - EMPTY_NOTE_Y0,
                );
                let mw = calculator::text_width(msg, msg_cap);
                let mx0 = DISPLAY_X0 + (DISPLAY_X1 - DISPLAY_X0 - mw) / 2;
                if let Some(c) = test_text(x, y, mx0, EMPTY_NOTE_Y0, msg_cap, msg, THEME.on_surface_variant, THEME.surface_container, false) {
                    return c;
                }
            } else {
                // WHY the row's geometry is read from the chrome and not from
                // the ladder's constants: the row holds two runs, and both grow
                // with the person's text size. The stride grows with them, so a
                // row never paints on the row below it, and the number of rows
                // the drawer shows is the number that stands inside the sheet --
                // a larger setting shows fewer entries at once rather than
                // entries running off the sheet's bottom edge.
                let pitch = chrome::history_pitch();
                let row_h = chrome::history_row_h();
                let expr_cap = chrome::cap_in_plate(CAP_LABEL, row_h - HIST_EXPR_DY);
                let result_dy = HIST_EXPR_DY + run_rows(expr_cap);
                let result_cap = chrome::cap_in_plate(CAP_SECTION, row_h - result_dy);
                for i in 0..count.min(chrome::history_visible_rows()) {
                    let row_y0 = HIST_LIST_Y0 + i * pitch;
                    let row_y1 = row_y0 + row_h;
                    if y >= row_y0 && y < row_y1 && x >= 40 && x < 680 {
                        let erx = (x - 40).min(679 - x);
                        let ery = (y - row_y0).min(row_y1 - 1 - y);
                        if erx < row_style.radius && ery < row_style.radius {
                            let cdx = (row_style.radius - erx) as isize;
                            let cdy = (row_style.radius - ery) as isize;
                            if cdx * cdx + cdy * cdy > (row_style.radius * row_style.radius) as isize {
                                return THEME.surface_container;
                            }
                        }

                        if let Some(entry) = calc.history.get_recent(i) {
                            // Top line: expression
                            let expr_bytes = &entry.expr[..entry.expr_len];
                            if !expr_bytes.is_empty() {
                                let mut ebuf = [0u8; 64];
                                // WHY the ellipsis is measured at the size the
                                // run is drawn at: the ledger's column is five
                                // hundred and ninety rows wide and the scaled
                                // expression is not, so the line that stops
                                // fitting gets its `...` cut at the same rung the
                                // ink is painted at. Measuring the truncation at
                                // the ladder's rung and drawing at the person's is
                                // how a scaled row ends up running past the column
                                // it was "fitted" to.
                                let elen = calculator::ellipsize_cap(expr_bytes, expr_cap, 590, &mut ebuf);
                                let ew = calculator::text_width(&ebuf[..elen], expr_cap);
                                let ex0 = 650usize.saturating_sub(ew).max(60);
                                if let Some(c) = test_text(x, y, ex0, row_y0 + HIST_EXPR_DY, expr_cap, &ebuf[..elen], THEME.on_surface_variant, row_style.fill, false) {
                                    return c;
                                }
                            }

                            // Bottom line: result
                            let mut res_buf = [0u8; 32];
                            let rlen = entry.result.format(&mut res_buf);
                            let rw = calculator::text_width(&res_buf[..rlen], result_cap);
                            let rx0 = 650usize.saturating_sub(rw).max(60);
                            if let Some(c) = test_text(x, y, rx0, row_y0 + result_dy, result_cap, &res_buf[..rlen], THEME.on_surface, row_style.fill, true) {
                                return c;
                            }
                        }

                        if erx < 2 || ery < 2 {
                            return row_style.divider;
                        }
                        return row_style.fill;
                    }
                }
            }

            if rx < 2 || ry < 2 {
                return THEME.border;
            }
            return THEME.surface_container;
        }

        return THEME.surface;
    }

    // 3. The display band: the expression above, the result below, on one
    //    card that holds the two runs and nothing else.
    //
    // WHY the card's rows come from the plan and not from the ladder's
    // constants: the tape's line box and the result's line box are both the
    // person's text size, and the card is the box that holds them. The plan is
    // built once per repaint from the level this process publishes, and the
    // card painted here and the card the press answers are the same rows.
    let plan = unsafe { CALC_PLAN }.unwrap_or(DEFAULT_PLAN);
    if (DISPLAY_Y0..plan.card_y1).contains(&y) && x >= DISPLAY_X0 && x < DISPLAY_X1 {
        // WHY the platform's card treatment: the card's fill, its hairline
        // and its radius come from the platform's own elevation, so an app
        // that drew a card with the SDK draws this card.
        let card = components::card_style(&THEME, components::Elevation::Resting);
        let rx = (x - DISPLAY_X0).min(DISPLAY_X1 - 1 - x);
        let ry = (y - DISPLAY_Y0).min(plan.card_y1 - 1 - y);
        if rx < card.radius && ry < card.radius {
            let cdx = (card.radius - rx) as isize;
            let cdy = (card.radius - ry) as isize;
            if cdx * cdx + cdy * cdy > (card.radius * card.radius) as isize {
                return THEME.surface;
            }
        }

        // The pending operation's badge, level with the result.
        //
        // WHY it sits beside the result and not beside the expression: the
        // expression is what was typed and the result is the number the
        // pending operation is waiting on, so the badge belongs to the
        // number.
        //
        // WHY the badge grows by the result's rows gained, not by its box:
        // it is the operation the result is waiting on, drawn in the result's
        // own rows, so a plate left at thirty-four while the number beside it
        // doubles is a badge the reader can no longer line up with anything.
        // But the plate's shipped thirty-four is not a line box of any rung --
        // it is the designer's number -- so the growth is measured against the
        // tallest rung the result stands at, the way Messages measures a raised
        // rung against the ladder's own box: the saturated difference answers
        // nought at the default level, which keeps the shipped plate pixel for
        // pixel, and adds exactly the rows the published level gives the run.
        if let Some(op) = calc.active_operation {
            let badge_rows = 34
                + run_rows(plan.digits_cap.max(plan.err_cap))
                    .saturating_sub(run_rows(RESULT_LADDER[0]));
            let badge_cy = plan.value_y0 + badge_rows / 2;
            if (plan.value_y0..plan.value_y0 + badge_rows).contains(&y) && (44..104).contains(&x) {
                let prx = (x - 44).min(103 - x);
                let pry = (y - plan.value_y0).min(plan.value_y0 + badge_rows - 1 - y);
                if prx < R_CONTROL && pry < R_CONTROL {
                    let cdx = (R_CONTROL - prx) as isize;
                    let cdy = (R_CONTROL - pry) as isize;
                    if cdx * cdx + cdy * cdy <= (R_CONTROL * R_CONTROL) as isize {
                        if prx < 2 || pry < 2 {
                            return THEME.warning_container;
                        }
                        if let Some(c) = draw_calc_badge_operator_glyph(x, y, 74, badge_cy, op, THEME.warning) {
                            return c;
                        }
                        return THEME.surface_container_high;
                    }
                } else {
                    if prx < 2 || pry < 2 {
                        return THEME.warning_container;
                    }
                    if let Some(c) = draw_calc_badge_operator_glyph(x, y, 74, badge_cy, op, THEME.warning) {
                        return c;
                    }
                    return THEME.surface_container_high;
                }
            }
        }

        // Dual-line Top Line and Main Line / Error Banner via precomputed plan
        //
        // WHY the plate is rebuilt from the plan instead of re-derived: the plan
        // settled this frame's tape geometry once, and a chip measured again here
        // would be a second answer to one question.
        {
            let p = plan;
            // The expression tape.
            //
            // WHY the tape shows wrapped lines and not the last thirty-two
            // characters: a long expression was cut off at the front with
            // nothing said about it, so the part of the expression the
            // person could not see was simply gone. The tape now holds the
            // whole expression and scrolls through it.
            if p.has_tape && y >= TAPE_Y0 && y < p.tape_y1 {
                let row = (y - TAPE_Y0) / p.tape_rows;
                let line = p.tape_first + row;
                if line < p.tape_count {
                    let line_y = TAPE_Y0 + row * p.tape_rows;
                    let text = &calc.tape_lines[line][..calc.tape_lens[line]];
                    if let Some(c) = test_text(x, y, p.tape_xs[line], line_y, p.expr_cap, text, THEME.on_surface, THEME.surface_container, true) {
                        return c;
                    }
                }
                // The scroll plate, only when there is more than one
                // screenful of expression to reach.
                //
                // WHY the plate is inked and not just the two chevrons: the
                // press answers the whole chip, and a chip whose visible
                // mark is nineteen pixels across inside a box forty across
                // is a person tapping page they cannot see and calling it a
                // broken screen. What is drawn is what is answered.
                //
                // WHY the chip is the tape's own height at this frame's text
                // size: the plate is the tape, and the tape grew with the
                // caption it carries. A chip left at the ladder's height would
                // be painted over the last tape line and would answer less than
                // the control the person can see.
                if p.tape_overflow {
                    let plate_h = p.tape_y1 - TAPE_Y0;
                    if x >= SCROLL_PLATE_X && x < SCROLL_PLATE_X + SCROLL_PLATE_W && y >= TAPE_Y0
                        && y < TAPE_Y0 + plate_h
                    {
                        let prx = (x - SCROLL_PLATE_X).min(SCROLL_PLATE_X + SCROLL_PLATE_W - 1 - x);
                        let pry = (y - TAPE_Y0).min(TAPE_Y0 + plate_h - 1 - y);
                        if prx < R_CHIP && pry < R_CHIP {
                            let cdx = (R_CHIP - prx) as isize;
                            let cdy = (R_CHIP - pry) as isize;
                            if cdx * cdx + cdy * cdy > (R_CHIP * R_CHIP) as isize {
                                return THEME.surface_container;
                            }
                        }
                        if prx < 2 || pry < 2 {
                            return THEME.border;
                        }
                        if let Some(c) = tape_chevron_pixel(x, y, true, THEME.on_surface_variant, plate_h) {
                            return c;
                        }
                        if let Some(c) = tape_chevron_pixel(x, y, false, THEME.on_surface_variant, plate_h) {
                            return c;
                        }
                        return THEME.surface_container_high;
                    }
                }
            }
            if p.is_error {
                let h = run_rows(p.err_cap);
                if y >= p.value_y0 && y < p.value_y0 + h {
                    if let Some(c) = test_text(x, y, p.err_x, p.value_y0, p.err_cap, p.err_text, THEME.error, THEME.surface_container, true) {
                        return c;
                    }
                }
            } else {
                let h = run_rows(p.digits_cap);
                if y >= p.value_y0 && y < p.value_y0 + h {
                    let digits = &calc.display_buf[..p.digits_len.min(calc.display_buf.len())];
                    if let Some(c) = test_text(x, y, p.digits_x, p.value_y0, p.digits_cap, digits, THEME.on_surface, THEME.surface_container, true) {
                        return c;
                    }
                }
            }
        }

        if rx < 2 || ry < 2 {
            return card.border.unwrap_or(THEME.border);
        }

        return card.fill;
    }

    // 4. Keypad Matrix (6 rows, 4 cols, the last row two wide keys)
    for row in 0..6 {
        for col in 0..4 {
            let Some(rect) = key_rect(row, col) else {
                continue;
            };
            let Some((rx, ry)) = key_edges(row, col, x, y) else {
                continue;
            };
            {
                let r = rect.h / 2;
                if rx < r && ry < r {
                    let cdx = (r - rx) as isize;
                    let cdy = (r - ry) as isize;
                    if cdx * cdx + cdy * cdy > (r * r) as isize {
                        return THEME.surface;
                    }
                }

                let action = get_button_action(row, col);
                let is_active_op = action.is_some() && action == calc.active_operation;
                let is_op = matches!(action, Some('/') | Some('*') | Some('-') | Some('+'));
                let is_equals = action == Some('=');
                let is_clear = action == Some('C');
                let is_clear_entry = action == Some('E');
                let is_backspace = action == Some('B');
                let is_fn = matches!(action, Some('P') | Some('%') | Some('~'));

                // WHY the operators and the equals are the opposite of the page
                // and not a colour of their own: the design the app is built
                // from puts the whole operator column in near-black, and the
                // platform's tokens already carry that colour. What the app
                // had instead was the *warning* role on an operator key and
                // the *error* role on the clear key, and a status colour that
                // means "operator" and "clear" is a status colour that means
                // nothing -- in this app and in every app that reads the
                // platform's warning and error roles.
                //
                // WHY the armed operator is a slightly lighter black and not
                // a hue: a person has to see which operator is waiting, and
                // a shade of the same colour says that without spending the
                // screen's one accent.
                let (btn_bg, btn_border, text_color) = if is_equals {
                    (THEME.inverted_surface(), THEME.inverted_surface(), THEME.on_inverted_surface())
                } else if is_active_op {
                    let armed = interos_std::design::theme::Theme::blend(
                        THEME.surface_container_high,
                        THEME.on_surface,
                        48,
                    );
                    (armed, armed, THEME.on_surface)
                } else if is_op {
                    (THEME.surface_container_high, THEME.border, THEME.on_surface)
                } else if is_clear {
                    (THEME.surface_container_high, THEME.border, THEME.on_surface)
                } else if is_clear_entry {
                    (THEME.surface_container_high, THEME.border, THEME.on_surface_variant)
                } else if is_backspace {
                    (THEME.surface_container_high, THEME.border, THEME.on_surface)
                } else if is_fn {
                    (THEME.surface_container_high, THEME.border, THEME.on_surface_variant)
                } else {
                    (THEME.surface_container_high, THEME.border, THEME.on_surface)
                };

                let r = rect.h / 2;
                let is_pill_border = if rx < r && ry < r {
                    let cdx = (r - rx) as isize;
                    let cdy = (r - ry) as isize;
                    let d2 = cdx * cdx + cdy * cdy;
                    d2 >= ((r - 2) * (r - 2)) as isize
                } else {
                    rx < 2 || ry < 2
                };

                if is_pill_border {
                    return btn_border;
                }

                let center_x = rect.x + rect.w / 2;
                let center_y = rect.y + rect.h / 2;

                if let Some(act) = action {
                    if matches!(act, '/' | '*' | '-' | '+' | '=') {
                        if let Some(c) = draw_calc_operator_glyph(x, y, center_x, center_y, act, text_color) {
                            return c;
                        }
                    } else {
                        let label: &[u8] = match act {
                            'C' | 'E' | 'B' | 'P' | '~' | '.' | '%' => calculator::button_label(act),
                            _ => &[act as u8],
                        };
                        // WHY the key's own height is the plate: the keypad grid
                        // is what a thumb lands on and the app's gate says a key
                        // label's box must stand inside its keycap. The clamp
                        // keeps that true at every text size, and centring the
                        // *box* -- not the cap -- on the keycap is what keeps the
                        // label where the person is aiming.
                        let key_cap = chrome::cap_in_plate(CAP_KEY_LABEL, rect.h);
                        let w = calculator::text_width(label, key_cap);
                        let x0 = center_x.saturating_sub(w / 2);
                        // WHY the box and not the cap: a run reserves its line
                        // box, so centring the box puts the run's middle on the
                        // keycap's middle. Centring from the cap left a scale-two
                        // box hanging ten rows below the key it labels.
                        let y0 = center_y.saturating_sub(run_rows(key_cap) / 2);
                        if let Some(c) = test_text(x, y, x0, y0, key_cap, label, text_color, btn_bg, true) {
                            return c;
                        }
                    }
                }

                return btn_bg;
            }
        }
    }

    // 5. Home Gesture Pill
    //
    // WHY the platform's own pill and not the app's own bar: the shell
    // draws the home indicator on every screen, and the app's bar was a
    // grey line of a different size in a different place, so the screen
    // carried two home indicators and the one that was drawn was the one
    // nobody could see. The pill is the shell's: the same width, the same
    // height and the same row as the strip the press answers, in the ink
    // that stands out against the page behind it.
    if HOME_PILL.contains(x, y) {
        return THEME.on_surface;
    }

    THEME.surface
}

fn render_calculator_ui(surface: u32, calc: &CalculatorState) {
    prepare_calc_plan(calc);
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_calculator_pixel(x, y, calc);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(surface, offset, &chunk[..count]);
        offset += count;
    }
}

fn render_calculator_band(surface: u32, calc: &CalculatorState, y0: usize, y1: usize) {
    prepare_calc_plan(calc);
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_calculator_pixel(x, y, calc);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(surface, offset, &chunk[..count]);
        offset += count;
    }
}

// -----------------------------------------------------------------------------
// Input Handling and Event Dispatch
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    None,
    Display,
    All,
}

impl Dirty {
    fn widen(self, other: Dirty) -> Dirty {
        match (self, other) {
            (Dirty::None, d) | (d, Dirty::None) => d,
            (a, b) if a == b => a,
            _ => Dirty::All,
        }
    }
}

enum Tap {
    None,
    Redraw(Dirty),
    Leave(&'static str),
}

fn repaint(surface: u32, calc: &mut CalculatorState, dirty: Dirty) {
    // WHY the level is re-adopted here and not only at launch: Settings writes
    // the store and nothing tells this app that it did, so a person who makes
    // the text larger while the calculator is open would otherwise see the next
    // frame drawn at the size the app started with -- the same defect the row
    // below closes. One settings query per repaint, and never inside the paint
    // loop itself.
    //
    // WHY the whole screen when it changed: the level moves the display card,
    // the tape's stride, the drawer's rows and the keypad's labels, and a band
    // repaint that only covers the card would leave every other region at the
    // rung the previous frame used.
    //
    // WHY the tape is re-wrapped here too: the engine wrapped the expression
    // into lanes sized at the level the last repaint published, and a raised
    // rung draws those lines wider than the lane that justified them. The
    // wrap reads the same published cap the painter does (see
    // `CalculatorState::sync_tape`), so this is the one point where a moved
    // level and a stale wrap can be reconciled before a single row is painted.
    if publish_text_scale() {
        calc.sync_tape();
        render_calculator_ui(surface, calc);
        return;
    }
    match dirty {
        Dirty::None => {}
        // WHY the band runs to the keypad's first row instead of to the ladder's
        // card bottom: the card grows with the person's text size, and a band that
        // repaints only the rows the ladder reserved leaves the last rows of a
        // scaled card showing the previous frame.
        Dirty::Display => render_calculator_band(surface, calc, DISPLAY_Y0, KEYPAD_TOP),
        Dirty::All => render_calculator_ui(surface, calc),
    }
}

fn handle_keyboard_key(calc: &mut CalculatorState, code: u32) -> bool {
    use std::input_key_code;
    // WHY the keypad's own names: the keyboard names a key by the character
    // it types and the keypad names it by what it does, and the state
    // change goes through the same function either way, so the number on
    // the display is the number that was typed.
    match code {
        48..=57 => {
            calc.press_key((code as u8) as char);
            std::log("[CALCULATOR] Keypad digit pressed.").ok();
        }
        43 => {
            calc.press_key('+');
            std::log("[CALCULATOR] Keypad operator selected.").ok();
        }
        45 => {
            calc.press_key('-');
            std::log("[CALCULATOR] Keypad operator selected.").ok();
        }
        42 => {
            calc.press_key('*');
            std::log("[CALCULATOR] Keypad operator selected.").ok();
        }
        47 => {
            calc.press_key('/');
            std::log("[CALCULATOR] Keypad operator selected.").ok();
        }
        40 | 41 => {
            calc.press_key('P');
            std::log("[CALCULATOR] Keypad operator selected.").ok();
        }
        46 => {
            calc.press_key('.');
        }
        61 | input_key_code::ENTER => {
            calc.press_key('=');
            std::log("[CALCULATOR] Evaluation completed.").ok();
        }
        99 | 67 | input_key_code::ESCAPE => {
            calc.press_key('C');
            std::log("[CALCULATOR] Clear pressed.").ok();
        }
        input_key_code::BACKSPACE => {
            calc.press_key('B');
        }
        // WHY the tape scrolls with the arrow and page keys: the tape
        // holds the whole expression, and a person with a keyboard has to
        // be able to reach the part of it that is off the screen.
        input_key_code::ARROW_UP => {
            calc.scroll_tape(-1);
        }
        input_key_code::ARROW_DOWN => {
            calc.scroll_tape(1);
        }
        input_key_code::PAGE_UP => {
            calc.scroll_tape(-(TAPE_LINES as isize));
        }
        input_key_code::PAGE_DOWN => {
            calc.scroll_tape(TAPE_LINES as isize);
        }
        37 => {
            calc.press_key('%');
            std::log("[CALCULATOR] Modulo operation queued.").ok();
        }
        94 | 115 | 83 => {
            calc.square();
            std::log("[CALCULATOR] Square evaluated.").ok();
        }
        95 | 126 => {
            calc.press_key('~');
            std::log("[CALCULATOR] Sign toggled.").ok();
        }
        104 | 72 => {
            calc.toggle_history();
        }
        _ => return false,
    }
    true
}

fn handle_touch_point(calc: &mut CalculatorState, x: u32, y: u32) -> Tap {
    let x = x as usize;
    let y = y as usize;

    // The back chip.
    //
    // WHY there is no second clause: this used to be the chip's rect unioned
    // with a band from x=24 to x=100, and the columns past the chip's
    // forty-eight-row plate are bare header -- the title run starts at x=92.
    // A press there quit the app from page that had never been inked as a
    // button, which is the defect the platform's own chip exists to prevent.
    if chrome::back_claims(x, y) {
        return Tap::Leave("[CALCULATOR] Back button tapped. Returning to launcher.");
    }

    // The home strip: the rows below the pill's ink, at the pill's own
    // width, and no wider than the plate the shell draws.
    if chrome::home_claims(x, y) {
        return Tap::Leave("[CALCULATOR] Home gesture tapped. Returning to launcher.");
    }

    // History toggle: the pill the header row paints.
    if chrome::history_claims(x, y) {
        calc.toggle_history();
        return Tap::Redraw(Dirty::All);
    }

    // History Drawer active interactions
    if calc.show_history {
        // Close pill: the rect the drawer draws.
        if chrome::close_claims(x, y) {
            calc.show_history = false;
            return Tap::Redraw(Dirty::All);
        }
        // Clear pill: the rect the drawer draws.
        if chrome::clear_claims(x, y) {
            calc.history.clear();
            return Tap::Redraw(Dirty::All);
        }
        // History rows hit test
        //
        // WHY the ceiling is the chrome's and not the eight the painter used to
        // hard-code: the rows a person can reach are the rows that stand inside
        // the sheet at this text size, and a press that answered a row the
        // painter never drew is the disagreement a thumb can feel.
        let count = calc.history.count();
        for i in 0..count.min(chrome::history_visible_rows()) {
            let row_y0 = HIST_LIST_Y0 + i * chrome::history_pitch();
            let row_y1 = row_y0 + chrome::history_row_h();
            if y >= row_y0 && y < row_y1 && x >= 40 && x < 680 {
                calc.load_history_entry(i);
                calc.show_history = false;
                return Tap::Redraw(Dirty::All);
            }
        }
        // Tapping outside closes drawer
        if y >= 110 && y < 1380 {
            calc.show_history = false;
            return Tap::Redraw(Dirty::All);
        }
        return Tap::None;
    }

    // The expression tape's scroll plate.
    //
    // WHY it is there: the tape holds the whole expression, and the part of
    // it that is off the screen has to be reachable -- through the chip the
    // person can see, at the size the platform names.
    if calc.tape_count > TAPE_LINES {
        if let Some(toward_the_start) = chrome::scroll_claims(x, y) {
            if toward_the_start {
                calc.scroll_tape(-1);
            } else {
                calc.scroll_tape(1);
            }
            return Tap::Redraw(Dirty::Display);
        }
    }

    // Keypad Matrix hit test
    //
    // WHY the same shape the renderer draws: a key that answers a touch it
    // does not show is a key that is not where it looks, and the gap
    // between two keys has to belong to neither of them.
    for row in 0..6 {
        for col in 0..4 {
            if !key_hit(row, col, x as usize, y as usize) {
                continue;
            }
            if let Some(action) = get_button_action(row, col) {
                match action {
                    '0'..='9' => {
                        std::log("[CALCULATOR] Keypad digit pressed.").ok();
                    }
                    '+' | '-' | '*' | '/' => {
                        std::log("[CALCULATOR] Keypad operator selected.").ok();
                    }
                    'P' => {
                        std::log("[CALCULATOR] Keypad operator selected.").ok();
                    }
                    '%' => {
                        std::log("[CALCULATOR] Modulo operation queued.").ok();
                    }
                    '=' => {
                        std::log("[CALCULATOR] Evaluation completed.").ok();
                    }
                    'C' => {
                        std::log("[CALCULATOR] Clear-all pressed.").ok();
                    }
                    'E' => {
                        std::log("[CALCULATOR] Clear-entry pressed.").ok();
                    }
                    'B' => {
                        std::log("[CALCULATOR] Backspace pressed.").ok();
                    }
                    '~' => {
                        std::log("[CALCULATOR] Sign toggled.").ok();
                    }
                    '.' => {}
                    _ => {}
                }
                // WHY the state change goes through one function: the value
                // on the display has to be the value the key produced, and
                // the keypad and the keyboard are two ways of pressing the
                // same key.
                calc.press_key(action);
                let is_op = matches!(action, '/' | '*' | '-' | '+' | '=');
                if is_op {
                    return Tap::Redraw(Dirty::All);
                } else {
                    return Tap::Redraw(Dirty::Display);
                }
            }
            return Tap::None;
        }
    }

    Tap::None
}

// -----------------------------------------------------------------------------
// The raw contact channel
// -----------------------------------------------------------------------------
//
// WHY the app reads the platform's input device node: the normalized
// pipeline publishes a gesture when the finger lifts, so a key that
// answers on the gesture cannot answer until the finger is up, and a
// person who presses "7" and sees nothing until they lift their finger
// cannot tell whether the key is broken or they are. The device node
// carries the contact itself -- its phase, its slot and its position -- so
// the key under the finger answers while the finger is still on it.
//
// WHY the gesture path stays: the device node is one reader of one queue,
// and a guest that does not publish it leaves the gesture as the only way
// in. The app falls back to it and says so on the log.

/// The platform's input device node.
const INPUT_DEVICE: &str = "/dev/input/event0";

/// The length of one record the device node publishes.
const CONTACT_RECORD_LEN: usize = 16;

/// The record type of a contact.
const CONTACT_TYPE_TOUCH: u8 = 2;

/// The phase of a contact: the finger landing on the glass.
///
/// WHY the value and not a name: the device node publishes the phase as
/// the platform's own contact phase, and the phases are down, move, up and
/// cancel in that order.
const CONTACT_PHASE_DOWN: u8 = 0;

/// One record from the platform's input device node.
enum DeviceRecord {
    /// The queue is empty.
    Empty,
    /// A contact: where it is and what phase it is in.
    Contact { x: u16, y: u16, phase: u8 },
    /// A record that is not a contact: a key, or a gesture.
    Other,
}

/// Reads one record from the platform's input device node.
fn read_device_record(fd: u32) -> DeviceRecord {
    let mut buf = [0u8; CONTACT_RECORD_LEN];
    match std::fs_read(fd, &mut buf) {
        Ok(n) if n >= CONTACT_RECORD_LEN => {
            if buf[0] == CONTACT_TYPE_TOUCH {
                DeviceRecord::Contact {
                    x: u16::from_le_bytes([buf[2], buf[3]]),
                    y: u16::from_le_bytes([buf[4], buf[5]]),
                    phase: buf[8],
                }
            } else {
                DeviceRecord::Other
            }
        }
        _ => DeviceRecord::Empty,
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[CALCULATOR (Ring 3)] Open-Source Touch Calculator online.",
    )
    .ok();

    let mut calc = CalculatorState::new();
    refresh_clock(&mut calc);
    // WHY before the surface exists and before a single row is painted: the
    // first frame is laid out from the level this publishes, and a rung handed
    // to the renderer after the card is drawn is a card drawn twice.
    publish_text_scale();

    // 1. Allocate dedicated KMS Client Surface (z=200)
    std::log("[CALCULATOR] Step 1: Allocating application surface (z=200)...",
    )
    .ok();
    // WHY the slot is resolved by type: a slot number is a position in a
    // table, and the position depends on the order this task's
    // capabilities were inserted in. Six happened to hold the framebuffer
    // here, and would not in an app with one more capability.
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(s) => {
            std::log("[CALCULATOR] Step 2: Surface created successfully.").ok();
            s
        }
        Err(_) => {
            std::log("[CALCULATOR] Display surface unavailable; refusing headless execution.",
            )
            .ok();
            std::exit_with_code(1);
        }
    };

    // 2. Start with an empty expression; calculations are driven by user input.
    calc.clear();

    // 3. The raw contact channel.
    //
    // WHY the queue is drained before the loop starts: the contacts that
    // are already queued are the ones that brought the app here, and a key
    // that answers a contact that ended before the app was up is a key
    // that answers a press nobody made.
    let contact_device = match std::fs_open(INPUT_DEVICE, std::FS_READ) {
        Ok(fd) => {
            while !matches!(read_device_record(fd), DeviceRecord::Empty) {}
            Some(fd)
        }
        Err(_) => {
            std::log("[CALCULATOR] The raw contact channel is unavailable; a key answers when the finger lifts.").ok();
            None
        }
    };
    // Whether the contact that is on the glass has already answered the key
    // under it.
    let mut contact_answered = false;

    // 4. Render initial Calculator UI frame and present
    repaint(surface, &mut calc, Dirty::All);
    let _ = std::surface_flip();
    std::log("[CALCULATOR] Initial frame presented to hardware scanout.",
    )
    .ok();

    // 5. Interactive Event Reactor Loop
    loop {
        if let Ok(_events) = std::event_poll(std::EVENT_TOUCH_INPUT, 100) {
            let mut input = std::InputEvent {
                kind: 0,
                x: 0,
                y: 0,
                dx: 0,
                dy: 0,
                scale_milli: 1000,
            };
            let mut dirty = Dirty::None;

            // The contacts the platform publishes while the finger is down.
            //
            // WHY they are read before the gestures: the contact is the
            // finger landing, and the gesture that follows is the same
            // touch arriving again at the end of it. A key answers the
            // first of the two, and the frame that carries the answer is
            // drawn before the second of them is looked at.
            if let Some(fd) = contact_device {
                loop {
                    match read_device_record(fd) {
                        DeviceRecord::Empty => break,
                        DeviceRecord::Other => continue,
                        DeviceRecord::Contact { x, y, phase } => {
                            if phase != CONTACT_PHASE_DOWN {
                                continue;
                            }
                            match handle_touch_point(&mut calc, x as u32, y as u32) {
                                Tap::Leave(msg) => {
                                    std::log(msg).ok();
                                    if surface > 0 {
                                        let _ = std::surface_destroy(surface);
                                    }
                                    std::exit_with_code(0);
                                }
                                Tap::Redraw(d) => {
                                    contact_answered = true;
                                    dirty = dirty.widen(d);
                                }
                                Tap::None => {}
                            }
                        }
                    }
                }
            }

            while std::input_event_poll(&mut input).is_ok() {
                if let Some(key) = input.key_event() {
                    if key.pressed && handle_keyboard_key(&mut calc, key.code) {
                        dirty = dirty.widen(if calc.show_history { Dirty::All } else { Dirty::Display });
                    }
                    continue;
                }
                if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                    std::log("[CALCULATOR] Navigation gesture recognized. Returning to launcher.",
                    )
                    .ok();
                    if surface > 0 {
                        // WHY no slot argument: a slot is a position in this
                        // task's capability table, and the kernel authorises
                        // the release by the capability's type. The surface
                        // must be handed back or it stays a leaked KMS
                        // surface for the life of the device.
                        let _ = std::surface_destroy(surface);
                    }
                    std::exit_with_code(0);
                }
                if input.kind == 1 || input.kind == 2 {
                    // WHY the gesture is ignored when the contact already
                    // answered: the contact that landed on the key is the
                    // same touch, and answering it twice would type two
                    // digits for one press.
                    if contact_answered {
                        contact_answered = false;
                        continue;
                    }
                    match handle_touch_point(&mut calc, input.x, input.y) {
                        Tap::Leave(msg) => {
                            std::log(msg).ok();
                            if surface > 0 {
                                let _ = std::surface_destroy(surface);
                            }
                            std::exit_with_code(0);
                        }
                        Tap::Redraw(d) => dirty = dirty.widen(d),
                        Tap::None => {}
                    }
                }
            }
            if dirty != Dirty::None {
                // WHY the tape is re-wrapped here: the expression is what
                // the tape shows, and the expression is what the input
                // changed.
                calc.sync_tape();
                repaint(surface, &mut calc, dirty);
                let _ = std::surface_flip();
                std::log("[CALCULATOR] Post-input frame presented.").ok();
            }
        }

        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log("[PANIC] Calculator app error!").ok();
    std::exit_with_code(1);
}
