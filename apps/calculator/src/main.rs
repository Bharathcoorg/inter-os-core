// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
//! - Text laid out once per repaint: one frame plan places every run, and the
//!   pixel path only samples the box it was placed into
//! - Graceful lifecycle supervision and clean return to Launcher

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use calculator::theme::{THEME, radius, typography};

use interos_std::design::accessibility::{DynamicType, UserTextScale};
use interos_std::design::components;
use interos_std::design::outline_text;
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

/// The panel this app paints: the platform's own authoring size, so a surface
/// that ever moves is one number moving in one place.
const W: usize = interos_std::design::viewport::LOGICAL_WIDTH;
const H: usize = interos_std::design::viewport::LOGICAL_HEIGHT;
const CHUNK: usize = 4096;

const STATUS_BAR_HEIGHT: usize = interos_std::design::viewport::STATUS_BAR_HEIGHT;

/// The status band's geometry for this panel, resolved once for the process.
///
/// WHY a `static` and not a `StatusBand::new(W)` in the pixel path: the band's
/// box and its instrument strip are a function of the panel's width alone, and
/// the pixel callback is asked for every pixel of the band's rows -- a million
/// questions a frame, of which thirty-four thousand of them used to be "lay the
/// instruments out again". A `static` is the allocation-free cache: it is built
/// at compile time here, and the callback keeps being the arithmetic it is.
static STATUS_BAND: components::StatusBand = components::StatusBand::new(W);

/// The instruments this app draws, which it has never measured.
///
/// WHY the same treatment: `unmeasured()` is the band's third argument, and an
/// app that asks the radio and the fuel gauge for nothing has one answer for
/// every pixel of the band's rows. The `const fn` costs nothing to build, but it
/// was still being built a million times a frame to say so.
static STATUS_INSTRUMENTS: components::Instruments = components::Instruments::unmeasured();

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
    SCROLL_CHEVRON_X, SCROLL_PLATE_W, SCROLL_PLATE_X, TAPE_LINES, TAPE_TEXT_X1, TAPE_Y0,
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
// WHY there are six rows and not five: the keypad carries twenty-four keys
// and a four-column grid holds sixteen in four rows. The clear-all,
// clear-entry and backspace keys are three keys a person has to be able
// to tell apart, and a five-row grid had no room for them.
//
// WHY one key per cell and no width but `BTN_W`: the last row used to hold
// the modulo and the equals drawn across two columns each, and those four
// cells were the room the two parentheses and the percentage key needed. A
// grid where every cell holds one key is what makes a wide plate unnecessary
// rather than hidden -- the shape a thumb reads is one plate, twenty-four
// times.
//
// WHY the grid is read from `calculator::chrome` and not written here: the
// painter inks these rects, the press answers them, and a repaint now has to
// name ONE of them as the region it re-inks. Three readers, one owner -- see
// `chrome`'s own header for what the two-reader version cost.
use chrome::{BTN_W, BTN_H, COL_X, ROW_Y};

/// The columns the drawer's ledger lives in: the divider at the sheet's head,
/// the row plates below it and the band a row answers from are all this width.
///
/// WHY named: they were the literals `40` and `680`, written five times across
/// the painter, the plan and the press. A row that inked two columns wider than
/// it answered is the disagreement a thumb feels as "the history entry did not
/// open", and no gate could have caught it because there was nothing to compare.
const HIST_ROW_X0: usize = 40;
const HIST_ROW_X1: usize = 680;

#[inline(always)]
fn in_hist_row_columns(x: usize) -> bool {
    x >= HIST_ROW_X0 && x < HIST_ROW_X1
}

/// The drawer's sheet: the band the painter rounds and fills, and the band the
/// press dismisses from.
///
/// WHY named: they were the literals `110` and `1380`, written in the pixel path
/// and again in the press, with `1379` as a third copy for the corner rounding.
/// The sheet's bottom is already the chrome's own `DRAWER_SHEET_Y1` -- the number
/// the ledger's visible rows are measured against -- so the dismiss band and the
/// fill now move together.
const SHEET_Y0: usize = HEADER_H;
const SHEET_Y1: usize = chrome::DRAWER_SHEET_Y1;

// -----------------------------------------------------------------------------
// The touch floor, stated once against the plates this file inks.
//
// WHY here and not only in `calculator::chrome`: the chrome owns the header's
// chips, the tape's scroll plate, the drawer's two pills and the keypad's grid,
// and it already holds each of them to the floor. What it cannot check from
// inside itself is what the plate costs against the panel this file paints --
// `W`, `H` and the home strip the caps may not reach -- nor the ledger column
// the drawer paints and the press answers. The bin target is built `test =
// false`, so a `#[test]` written beside these numbers would never run: these are
// build-time gates, and a number that steps over the floor fails the build.
// -----------------------------------------------------------------------------
const _: () = {
    use interos_std::design::touch::MIN_TARGET;

    // A key is a `BTN_W` by `BTN_H` cap, and every one of the twenty-four
    // keys is that cap: there is no second shape to check.
    assert!(BTN_W >= MIN_TARGET);
    assert!(BTN_H >= MIN_TARGET);

    // The grid: the rect `key_rect` hands the painter is the rect `key_hit`
    // hands the press, so what is checked is that rect -- on screen, floor-sized,
    // and never touching its neighbour's.
    let mut r = 0;
    while r < ROW_Y.len() {
        assert!(ROW_Y[r] >= KEYPAD_TOP);
        assert!(ROW_Y[r] + BTN_H <= H - chrome::HOME_HIT.h);
        if r + 1 < ROW_Y.len() {
            assert!(ROW_Y[r] + BTN_H <= ROW_Y[r + 1]);
        }
        let mut c = 0;
        while c < COL_X.len() {
            assert!(COL_X[c] >= 16 && COL_X[c] + BTN_W <= W - 16);
            if c + 1 < COL_X.len() {
                assert!(COL_X[c] + BTN_W <= COL_X[c + 1]);
            }
            c += 1;
        }
        r += 1;
    }

    // The ledger's column, and a row at the platform's own text size.
    assert!(HIST_ROW_X0 >= DISPLAY_X0 && HIST_ROW_X1 <= DISPLAY_X1);
    assert!(HIST_ROW_X1 - HIST_ROW_X0 >= MIN_TARGET);
    assert!(chrome::history_row_h_at(std::design::font::TEXT_SCALE_NORMAL) >= MIN_TARGET);

    // The sheet the press dismisses from is the sheet the painter fills, and it
    // stops above the strip the home gesture answers from.
    assert!(SHEET_Y1 > SHEET_Y0);
    assert!(SHEET_Y1 - SHEET_Y0 >= MIN_TARGET);
    assert!(HIST_LIST_Y0 >= SHEET_Y0);
    assert!(SHEET_Y1 <= H);

    // The chips the header answers with are the platform's and the chrome's own.
    assert!(std::design::back_button::SIZE >= MIN_TARGET);
    assert!(chrome::HOME_HIT.w >= MIN_TARGET && chrome::HOME_HIT.h >= MIN_TARGET);
    assert!(chrome::HISTORY_PILL.w >= MIN_TARGET && chrome::HISTORY_PILL.h >= MIN_TARGET);
    assert!(chrome::CLEAR_PILL.w >= MIN_TARGET && chrome::CLEAR_PILL.h >= MIN_TARGET);
    assert!(chrome::CLOSE_PILL.w >= MIN_TARGET && chrome::CLOSE_PILL.h >= MIN_TARGET);
    assert!(chrome::PILL_H >= MIN_TARGET);
};

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
/// WHY one shape for all twenty-four keys: the last row used to be two keys
/// drawn across two columns each, which is a plate of a different shape in a
/// grid of identical ones -- and the cells they covered were where the two
/// parentheses belong. The grid has twenty-four cells and the keypad has
/// twenty-four keys, so every key is one cell and one `BTN_W` by `BTN_H` cap.
///
/// WHY the chrome owns it now: the repaint has to name a single keycap as the
/// region it re-inks, and a region the press cannot reach is a region the
/// painter would be guessing at.
#[inline]
fn key_rect(row: usize, col: usize) -> Option<Rect> {
    chrome::key_cap(row, col)
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

// -----------------------------------------------------------------------------
// The per-repaint frame plan
// -----------------------------------------------------------------------------

/// One run the frame paints, placed once for a whole repaint.
///
/// WHY the run itself is the plan entry: every run below used to be placed where
/// it is drawn, and this surface is drawn one pixel at a time -- a repaint asks a
/// little over a million questions, and a hundred thousand of them were "where
/// does this run sit, how wide is it, how many rows does it reserve". An
/// [`outline_text::OutlineRun`] is a plain `Copy` value that carries its own ink
/// box, so the plan can hold that answer and the pixel path can ask the box the
/// one question that does change per pixel.
///
/// WHY the bytes travel with the run: a run the outline face refuses is drawn by
/// the bitmap face, which still needs them -- see [`PlannedRun::paint`].
///
/// WHY `N` is a parameter and not a generous constant: every slot is sized from
/// the state that fills it (the `*_RUN_MAX` constants below). A run longer than
/// its buffer would be painted truncated, and a truncated run is a pixel change,
/// which is exactly what this refactor is not allowed to make.
#[derive(Clone, Copy)]
struct PlannedRun<const N: usize> {
    x: usize,
    y: usize,
    cap: u32,
    bold: bool,
    len: usize,
    buf: [u8; N],
    run: Option<outline_text::OutlineRun>,
}

/// The longest run each planned slot can hold, read off what paints into it.
///
/// WHY they name the state and not the strings: the strings below are fixed and
/// the state is not -- a tape line, a result number and a wrapped ledger
/// expression are all bounded by the buffers `lib.rs` declares, so that is what
/// sizes the slots. The fixed strings are shorter than the slot that carries
/// them: `Calculation History` is nineteen bytes and `No recent calculations`
/// twenty-one, in a twenty-four-byte header slot; `History`, `CLEAR` and `CLOSE`
/// are at most seven, in an eight-byte pill slot; the longest key label is
/// `DEL`, `+/-` or `mod` at three, in a four-byte key slot.
const TAPE_RUN_MAX: usize = 128;
const RESULT_RUN_MAX: usize = 32;
const ERROR_RUN_MAX: usize = 24;
const KEY_RUN_MAX: usize = 4;
const PILL_RUN_MAX: usize = 8;
const HEADER_RUN_MAX: usize = 24;
const HIST_EXPR_RUN_MAX: usize = 64;
const HIST_RESULT_RUN_MAX: usize = 32;

/// Every cell of the keypad grid, including the two holes in its last row: the
/// painter indexes its key by `(row, col)`, so the plan answers the same index.
const KEY_CELLS: usize = calculator::KEY_ROWS * calculator::KEY_COLS;

impl<const N: usize> PlannedRun<N> {
    /// A run with no bytes in it, which inks nothing anywhere.
    fn blank() -> PlannedRun<N> {
        PlannedRun {
            x: 0,
            y: 0,
            cap: 0,
            bold: false,
            len: 0,
            buf: [0; N],
            run: None,
        }
    }

    /// The colour of (`px`, `py`) if this run inks it, blended over `bg`.
    ///
    /// WHY the two size gates are still applied here: they are the gates the
    /// painter's own [`test_text`] applied before it reached either face, and a
    /// pixel outside them was answered `None` whatever the run's box said. A
    /// run's box is taller than a clamped cap's gate can be, so dropping them
    /// would ink rows the shipped frame leaves as fill.
    ///
    /// WHY the bitmap fallback is still here: a run the outline face refuses --
    /// a character the typeface has no outline for, a run wider than its scratch
    /// buffer -- is drawn by the bitmap face, and that answer is not something
    /// the plan can carry. What the plan saves is the placement question, not
    /// the ink: a pixel that falls back is painted exactly as it was.
    #[inline(always)]
    fn paint(&self, px: usize, py: usize, ink: u32, bg: u32) -> Option<u32> {
        if self.cap == 0 || self.len == 0 {
            return None;
        }
        let cap = self.cap as usize;
        if py < self.y || py >= self.y + cap.saturating_mul(2) + 8 {
            return None;
        }
        let max_w = self.len.saturating_mul(cap.saturating_mul(2) + 4) + 16;
        if px < self.x || px >= self.x + max_w {
            return None;
        }
        if let Some(ref run) = self.run {
            return run.pixel(px, py, ink, bg);
        }
        test_text(
            px,
            py,
            self.x,
            self.y,
            self.cap,
            &self.buf[..self.len],
            ink,
            bg,
            self.bold,
        )
    }
}

/// The run as the frame will draw it: `text` placed at (`x`, `y`), at `cap`.
///
/// WHY this mirrors [`test_text`] instead of asking the face something cleverer:
/// the painter's helper hands the outline face exactly these arguments -- the
/// run's own bytes, no width cap, the bold flag -- so only a placement made the
/// same way can stand in for its answer. `test_text` reaches the face through
/// `monochrome::text_px`, which tries `place_run_cap` first and falls back to the
/// bitmap face, so the run resolved here is the run that path would have resolved
/// per pixel. Where none resolves, the plan keeps the bytes and
/// [`PlannedRun::paint`] takes the original path.
fn planned_run<const N: usize>(
    x: usize,
    y: usize,
    cap: u32,
    text: &[u8],
    bold: bool,
) -> PlannedRun<N> {
    debug_assert!(
        text.len() <= N,
        "planned run of {} bytes does not fit its {}-byte slot",
        text.len(),
        N
    );
    let mut run = PlannedRun::<N>::blank();
    let n = text.len().min(N);
    run.buf[..n].copy_from_slice(&text[..n]);
    run.x = x;
    run.y = y;
    run.cap = cap;
    run.bold = bold;
    run.len = n;
    if n != 0 && cap != 0 {
        run.run = outline_text::place_run_cap(x, y, cap, &run.buf[..n], None, bold);
    }
    run
}

/// One ledger row of the history drawer: the band it is painted in, and the two
/// runs it holds.
///
/// WHY the band rides along: the row's stride and its height both grow with the
/// person's text size, and the painter tests a pixel against them before it asks
/// anything else. They were derived per pixel; now the row is measured once and
/// every pixel of the drawer reads the same two numbers it was painted with.
#[derive(Clone, Copy)]
struct PlannedRow {
    y0: usize,
    y1: usize,
    expr: PlannedRun<HIST_EXPR_RUN_MAX>,
    result: PlannedRun<HIST_RESULT_RUN_MAX>,
}

impl PlannedRow {
    fn blank() -> PlannedRow {
        PlannedRow {
            y0: 0,
            y1: 0,
            expr: PlannedRun::blank(),
            result: PlannedRun::blank(),
        }
    }
}

/// The history drawer: its header, its two pills, and the ledger it lists.
#[derive(Clone, Copy)]
struct DrawerPlan {
    title: PlannedRun<HEADER_RUN_MAX>,
    clear: PlannedRun<PILL_RUN_MAX>,
    close: PlannedRun<PILL_RUN_MAX>,
    notice: PlannedRun<HEADER_RUN_MAX>,
    rows: [PlannedRow; calculator::HISTORY_MAX_ROWS],
    row_count: usize,
}

impl DrawerPlan {
    fn blank() -> DrawerPlan {
        DrawerPlan {
            title: PlannedRun::blank(),
            clear: PlannedRun::blank(),
            close: PlannedRun::blank(),
            notice: PlannedRun::blank(),
            rows: [PlannedRow::blank(); calculator::HISTORY_MAX_ROWS],
            row_count: 0,
        }
    }
}

/// What the whole surface settles before it paints a single pixel.
///
/// WHY a plan at all: the app asks its pixel function for every pixel of a
/// 720x1440 surface, and until now that function re-derived the ink of every run
/// it owns from the published text size -- a clamped cap, a measured width, a
/// line box and a placement -- once per pixel. The plan answers those once per
/// repaint from the same state the painter is handed.
///
/// WHY the card's geometry rides in it too: the tape's line box, the card's
/// bottom and the badge's height are all derived from the same run rows, and the
/// card painted here and the card the press answers have to be one answer.
#[derive(Clone, Copy)]
struct CalcFramePlan {
    /// The header row, painted in every frame, drawer open or not.
    title: PlannedRun<HEADER_RUN_MAX>,
    history_pill: PlannedRun<PILL_RUN_MAX>,
    /// The display card. `badge_rows` is the plate the pending operation stands
    /// on, grown by the rows the published level gives the result run.
    badge_rows: usize,
    value_y0: usize,
    card_y1: usize,
    tape_rows: usize,
    tape_y1: usize,
    tape_count: usize,
    tape_first: usize,
    tape_overflow: bool,
    has_tape: bool,
    is_error: bool,
    result_rows: usize,
    error_rows: usize,
    tape: [PlannedRun<TAPE_RUN_MAX>; chrome::TAPE_LINES],
    result: PlannedRun<RESULT_RUN_MAX>,
    error: PlannedRun<ERROR_RUN_MAX>,
    /// The keypad's labels, indexed the way the painter indexes its grid.
    keys: [PlannedRun<KEY_RUN_MAX>; KEY_CELLS],
    /// The drawer, planned only for a frame that opens it.
    drawer: DrawerPlan,
}

impl CalcFramePlan {
    /// The frame the app draws with nothing typed in it and the drawer shut.
    ///
    /// WHY it exists: every field has an answer before the builder looks at the
    /// state, and the builder fills in only the half of the screen the frame
    /// actually paints -- so a section that was never planned inks nothing.
    fn blank() -> CalcFramePlan {
        CalcFramePlan {
            title: PlannedRun::blank(),
            history_pill: PlannedRun::blank(),
            badge_rows: 34,
            value_y0: chrome::VALUE_Y0,
            card_y1: chrome::DISPLAY_Y1,
            tape_rows: chrome::TAPE_LINE_H,
            tape_y1: chrome::TAPE_Y1,
            tape_count: 0,
            tape_first: 0,
            tape_overflow: false,
            has_tape: false,
            is_error: false,
            result_rows: 0,
            error_rows: 0,
            tape: [PlannedRun::blank(); chrome::TAPE_LINES],
            result: PlannedRun::blank(),
            error: PlannedRun::blank(),
            keys: [PlannedRun::blank(); KEY_CELLS],
            drawer: DrawerPlan::blank(),
        }
    }
}

/// The width of the display's content column.
fn display_column_w() -> usize {
    (DISPLAY_X1 - DISPLAY_PAD).saturating_sub(DISPLAY_X0 + DISPLAY_PAD)
}

/// The answers in [`CalcFramePlan`], from the state the repaint is about to draw.
///
/// WHY every number here is the one the painter used to take: each line below is
/// a derivation lifted out of the pixel path unchanged -- the same clamped caps,
/// the same measured widths, the same centring, the same plate floors -- so the
/// colour a pixel gets is the colour the shipped frame gave it. What moved is
/// where the question is asked, not what it asks.
fn build_calc_plan(calc: &CalculatorState) -> CalcFramePlan {
    let mut plan = CalcFramePlan::blank();

    // The header is painted in every frame, drawer open or not: the pixel engine
    // answers its rows before it ever looks at the sheet.
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
    plan.title = planned_run(92, HEADER_TITLE_Y0, title_cap, b"Calculator", true);

    let hw_cap = chrome::cap_in_plate(typography::BODY_CAP_PX, HISTORY_PILL.h);
    let hw = calculator::text_width(b"History", hw_cap);
    let hx = HISTORY_PILL.x + (HISTORY_PILL.w - hw) / 2;
    // WHY the line box and not the cap: a run reserves its ascent and its
    // descent, and centring the cap inside a forty-eight-row pill left the box's
    // last two rows painted on the page under it.
    let hy = HISTORY_PILL.y + (HISTORY_PILL.h - run_rows(hw_cap)) / 2;
    plan.history_pill = planned_run(hx, hy, hw_cap, b"History", true);

    // WHY the two halves of the screen are planned one at a time: the pixel
    // engine returns inside its drawer branch and never reaches the display card
    // or the keypad while the sheet is open, so a frame paints one half or the
    // other. The outline face keeps a fixed number of runs resident and evicts
    // its own earliest placement when a frame asks for more than fit, and an
    // evicted run re-rasterises itself for every pixel that still wants it --
    // slower than placing it ever was. Planning only the half this frame paints
    // keeps every run it inks resident for the whole walk.
    if calc.show_history {
        build_drawer_plan(&mut plan, calc);
        return plan;
    }
    build_display_plan(&mut plan, calc);
    build_keypad_plan(&mut plan);
    plan
}

/// The display card and the keypad, which are what a frame paints with the
/// drawer shut.
fn build_display_plan(plan: &mut CalcFramePlan, calc: &CalculatorState) {
    // The card's geometry at the level this process publishes. The tape grows
    // its own line box, the result stands below it, and the whole card is
    // clamped clear of the keypad -- see `chrome::KEYPAD_TOP`.
    //
    // WHY the tape's lines are counted and not assumed: the card is the box
    // that holds the runs this frame draws, and the tape draws nothing until a
    // person types an expression. Reserving its two lines anyway is what made
    // the screen a person opens a two-hundred-row plate with one number in its
    // lower corner and a field of nothing above it.
    let tape_lines = if calc.tape_count > 0 { TAPE_LINES } else { 0 };
    let expr_cap = chrome::cap_user(CAP_EXPRESSION);
    let tape_rows = chrome::run_rows(expr_cap);
    let tape_y1 = TAPE_Y0 + tape_lines * tape_rows;
    let value_y0 = chrome::value_y0_for_tape(tape_lines);
    let room = KEYPAD_TOP.saturating_sub(DISPLAY_PAD).saturating_sub(value_y0);
    let column = display_column_w();

    // The caps the badge's height is measured against. WHY they keep the
    // ladder's rungs when the frame has no result or no error to size: the
    // badge grows from whichever of the two the card drew, and a card that drew
    // the other one measured against the ladder's rung before too -- see the
    // note beside the badge below.
    let mut digits_cap = RESULT_LADDER[0];
    let mut err_cap = ERROR_LADDER[0];

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
        // WHY the runs are planned by row and not by line: the painter turns a
        // pixel's `y` into a row and a row into a line, and only the rows this
        // frame can land on are ever inked. A row whose line does not exist gets
        // a blank run, which inks nothing -- exactly the answer the painter's
        // `line < tape_count` test gave.
        let line_count = calc.tape_count.min(calculator::TAPE_MAX_LINES);
        for row in 0..TAPE_LINES {
            let line = plan.tape_first + row;
            if line < line_count {
                let text = &calc.tape_lines[line][..calc.tape_lens[line]];
                let width = calculator::text_width(text, expr_cap);
                plan.tape[row] = planned_run(
                    right.saturating_sub(width),
                    TAPE_Y0 + row * tape_rows,
                    expr_cap,
                    text,
                    true,
                );
            }
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
        err_cap = fit_display_cap(&ERROR_LADDER, err_text, column, room);
        let err_x =
            (DISPLAY_X1 - DISPLAY_PAD).saturating_sub(calculator::text_width(err_text, err_cap));
        plan.error = planned_run(err_x, value_y0, err_cap, err_text, true);
        plan.is_error = true;
        plan.error_rows = run_rows(err_cap);
        plan.card_y1 = chrome::card_y1_for_tape(tape_lines, plan.error_rows);
    } else {
        let digits = &calc.display_buf[..calc.display_len];
        // WHY the result comes down a rung when it does not fit: the length
        // of a number is not the width of it, and a long number at the title
        // rung would run off the card.
        digits_cap = fit_display_cap(&RESULT_LADDER, digits, column, room);
        let total_w = calculator::text_width(digits, digits_cap);
        let digits_x = (DISPLAY_X1 - DISPLAY_PAD).saturating_sub(total_w);
        plan.result = planned_run(digits_x, value_y0, digits_cap, digits, true);
        plan.result_rows = run_rows(digits_cap);
        plan.card_y1 = chrome::card_y1_for_tape(tape_lines, plan.result_rows);
    }

    plan.tape_rows = tape_rows;
    plan.tape_y1 = tape_y1;
    plan.value_y0 = value_y0;

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
    // WHY the saturated difference answers nought below the ladder: at a level
    // under one hundred per cent the result's rung is smaller than the ladder's,
    // and the plate does not shrink past the thirty-four the design shipped --
    // only grows.
    plan.badge_rows =
        34 + run_rows(digits_cap.max(err_cap)).saturating_sub(run_rows(RESULT_LADDER[0]));
}

/// The keypad's twenty-four key labels, placed once for the repaint.
///
/// WHY this is the dominant saving: the keypad is a little over half of the
/// surface, and every pixel of it used to clamp a cap, measure a label, take a
/// line box and place a run before it answered a colour.
///
/// WHY every cell is visited and most are left blank: the painter indexes its key
/// by `(row, col)`, so the plan answers the same index. The keys that draw a
/// glyph rather than a run -- the operators and the equals -- place none, because
/// their ink was never the text face's to give.
fn build_keypad_plan(plan: &mut CalcFramePlan) {
    // WHY the key's own height is the plate: the keypad grid is what a thumb
    // lands on and the app's gate says a key label's box must stand inside its
    // keycap. The clamp keeps that true at every text size, and centring the
    // *box* -- not the cap -- on the keycap is what keeps the label where the
    // person is aiming. Every keycap is `BTN_H` rows tall, so the clamp is
    // answered once for the grid rather than twenty-four times a frame.
    let key_cap = chrome::cap_in_plate(CAP_KEY_LABEL, BTN_H);
    let key_rows = run_rows(key_cap);
    for row in 0..calculator::KEY_ROWS {
        for col in 0..calculator::KEY_COLS {
            let Some(rect) = key_rect(row, col) else {
                continue;
            };
            let Some(act) = get_button_action(row, col) else {
                continue;
            };
            if matches!(act, '/' | '*' | '-' | '+' | '=') {
                continue;
            }
            let label: &[u8] = match act {
                'C' | 'E' | 'B' | 'p' | '~' | '.' | '%' | '(' | ')' => {
                    calculator::button_label(act)
                }
                _ => &[act as u8],
            };
            let w = calculator::text_width(label, key_cap);
            let x0 = (rect.x + rect.w / 2).saturating_sub(w / 2);
            // WHY the box and not the cap: a run reserves its line box, so
            // centring the box puts the run's middle on the keycap's middle.
            // Centring from the cap left a scale-two box hanging ten rows below
            // the key it labels.
            let y0 = (rect.y + rect.h / 2).saturating_sub(key_rows / 2);
            plan.keys[row * calculator::KEY_COLS + col] =
                planned_run(x0, y0, key_cap, label, true);
        }
    }
}

/// The history drawer: its header, its two pills and the ledger it lists.
fn build_drawer_plan(plan: &mut CalcFramePlan, calc: &CalculatorState) {
    // WHY clamped to the drawer's own header band: the band is the pill's height
    // plus four rows, the pills are the platform's minimum target and do not
    // move, and a run scaled past the band is painted over the separator and the
    // first ledger row. The clamp keeps the title inside the header it belongs
    // to.
    let drawer_cap = chrome::cap_in_plate(
        typography::SECTION_CAP_PX,
        DRAWER_HEADER_Y1 - DRAWER_TITLE_Y0,
    );
    plan.drawer.title = planned_run(48, DRAWER_TITLE_Y0, drawer_cap, b"Calculation History", true);

    // The two pills: the rects the press answers, drawn from the same plate.
    let clear_cap = chrome::cap_in_plate(typography::BODY_CAP_PX, chrome::PILL_H);
    let cw = calculator::text_width(b"CLEAR", clear_cap);
    let clear_x0 = CLEAR_PILL.x + (CLEAR_PILL.w - cw) / 2;
    let clear_y0 = chrome::PILL_TOP + (chrome::PILL_H - run_rows(clear_cap)) / 2;
    plan.drawer.clear = planned_run(clear_x0, clear_y0, clear_cap, b"CLEAR", true);

    let close_cap = chrome::cap_in_plate(typography::BODY_CAP_PX, chrome::PILL_H);
    let cw = calculator::text_width(b"CLOSE", close_cap);
    let close_x0 = CLOSE_PILL.x + (CLOSE_PILL.w - cw) / 2;
    let close_y0 = chrome::PILL_TOP + (chrome::PILL_H - run_rows(close_cap)) / 2;
    plan.drawer.close = planned_run(close_x0, close_y0, close_cap, b"CLOSE", true);

    let count = calc.history.count();
    if count == 0 {
        let msg = b"No recent calculations";
        let msg_cap = chrome::cap_in_plate(
            typography::SECTION_CAP_PX,
            chrome::DRAWER_SHEET_Y1 - EMPTY_NOTE_Y0,
        );
        let mw = calculator::text_width(msg, msg_cap);
        let mx0 = DISPLAY_X0 + (DISPLAY_X1 - DISPLAY_X0 - mw) / 2;
        plan.drawer.notice = planned_run(mx0, EMPTY_NOTE_Y0, msg_cap, msg, false);
        return;
    }

    // WHY the row's geometry is read from the chrome and not from the ladder's
    // constants: the row holds two runs, and both grow with the person's text
    // size. The stride grows with them, so a row never paints on the row below
    // it, and the number of rows the drawer shows is the number that stands
    // inside the sheet -- a larger setting shows fewer entries at once rather
    // than entries running off the sheet's bottom edge.
    let pitch = chrome::history_pitch();
    let row_h = chrome::history_row_h();
    let expr_cap = chrome::cap_in_plate(CAP_LABEL, row_h - HIST_EXPR_DY);
    let result_dy = HIST_EXPR_DY + run_rows(expr_cap);
    let result_cap = chrome::cap_in_plate(CAP_SECTION, row_h - result_dy);
    for i in 0..count.min(chrome::history_visible_rows()) {
        if i >= plan.drawer.rows.len() {
            break;
        }
        let row_y0 = HIST_LIST_Y0 + i * pitch;
        let mut row = PlannedRow::blank();
        row.y0 = row_y0;
        row.y1 = row_y0 + row_h;
        if let Some(entry) = calc.history.get_recent(i) {
            let expr_bytes = &entry.expr[..entry.expr_len];
            if !expr_bytes.is_empty() {
                let mut ebuf = [0u8; HIST_EXPR_RUN_MAX];
                // WHY the ellipsis is measured at the size the run is drawn at:
                // the ledger's column is five hundred and ninety rows wide and
                // the scaled expression is not, so the line that stops fitting
                // gets its `...` cut at the same rung the ink is painted at.
                // Measuring the truncation at the ladder's rung and drawing at
                // the person's is how a scaled row ends up running past the
                // column it was "fitted" to.
                let elen = calculator::ellipsize_cap(expr_bytes, expr_cap, 590, &mut ebuf);
                let ew = calculator::text_width(&ebuf[..elen], expr_cap);
                let ex0 = 650usize.saturating_sub(ew).max(60);
                row.expr = planned_run(ex0, row_y0 + HIST_EXPR_DY, expr_cap, &ebuf[..elen], false);
            }

            let mut res_buf = [0u8; HIST_RESULT_RUN_MAX];
            let rlen = entry.result.format(&mut res_buf);
            let rw = calculator::text_width(&res_buf[..rlen], result_cap);
            let rx0 = 650usize.saturating_sub(rw).max(60);
            row.result = planned_run(rx0, row_y0 + result_dy, result_cap, &res_buf[..rlen], true);
        }
        plan.drawer.rows[i] = row;
        if plan.drawer.row_count < i + 1 {
            plan.drawer.row_count = i + 1;
        }
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
/// The colour of (`x`, `y`) of the frame `plan` laid out.
///
/// WHY the plan is an argument and not something this function reads out of a
/// global: a repaint carries a little over twenty runs, and the cheapest way to
/// hand them over is the one reference the walk already holds. Nothing here
/// measures, clamps or places: every text question this function used to answer
/// per pixel is answered in [`build_calc_plan`] and sampled from the run's own
/// box here.
#[inline(never)]
fn get_calculator_pixel(x: usize, y: usize, calc: &CalculatorState, plan: &CalcFramePlan) -> u32 {
    // 0. Status Band (y < STATUS_BAR_HEIGHT)
    if y < STATUS_BAR_HEIGHT {
        let clock = if calc.clock_ok { Some(&calc.clock[..]) } else { None };
        return STATUS_BAND
            .pixel(x, y, clock, &STATUS_INSTRUMENTS, &THEME)
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
        // WHY the run is sampled here and not laid out: its clamped cap and its
        // placement are the header's one answer for the repaint -- the note on
        // why the plate cannot grow rides with the clamp in [`build_calc_plan`].
        if let Some(c) = plan.title.paint(x, y, THEME.on_surface, THEME.surface) {
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
            // WHY the run is sampled from the plan: the pill's label is centred
            // from its measured width and its line box, both of which the frame
            // settles once -- see [`build_calc_plan`].
            if let Some(c) = plan.history_pill.paint(x, y, text_c, bg) {
                return c;
            }
            return bg;
        }

        return THEME.surface;
    }

    // 2. History Drawer / Overlay (Active when calc.show_history is true)
    if calc.show_history {
        if y >= SHEET_Y0 && y < SHEET_Y1 && x >= DISPLAY_X0 && x < DISPLAY_X1 {
            let rx = (x - DISPLAY_X0).min(DISPLAY_X1 - 1 - x);
            let ry = (y - SHEET_Y0).min(SHEET_Y1 - 1 - y);
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
                // WHY the run is sampled here: the clamp that keeps it inside the
                // drawer's own header band is the frame's answer, not the
                // pixel's -- see [`build_drawer_plan`].
                if let Some(c) = plan
                    .drawer
                    .title
                    .paint(x, y, THEME.on_surface, THEME.surface_container)
                {
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
                    if let Some(c) = plan.drawer.clear.paint(x, y, THEME.error, THEME.surface_container_high) {
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
                    if let Some(c) = plan.drawer.close.paint(x, y, THEME.on_surface, THEME.surface_container_high) {
                        return c;
                    }
                    return THEME.surface_container_high;
                }
            }

            // Separator line
            if y == 182 && in_hist_row_columns(x) {
                return THEME.border;
            }

            // History Entries
            //
            // WHY the drawer's rows come from the plan: a row's band, its two
            // clamped caps, its ellipsised expression and its formatted result
            // are one answer per row per repaint, and the sheet is over eight
            // hundred thousand pixels of this surface. The painter's own branch
            // is what built them, so a row the plan did not place inks nothing
            // and the sheet paints its fill exactly as it did.
            //
            // WHY the platform's list-row treatment: the row's fill, its
            // radius and its divider come from the platform, so a history
            // row here is the same row an app that lists things would
            // draw.
            let row_style = components::list_row_style(&THEME, false);
            if plan.drawer.row_count == 0 {
                if let Some(c) = plan
                    .drawer
                    .notice
                    .paint(x, y, THEME.on_surface_variant, THEME.surface_container)
                {
                    return c;
                }
            } else {
                for i in 0..plan.drawer.row_count {
                    let row = &plan.drawer.rows[i];
                    if y >= row.y0 && y < row.y1 && in_hist_row_columns(x) {
                        let erx = (x - HIST_ROW_X0).min(HIST_ROW_X1 - 1 - x);
                        let ery = (y - row.y0).min(row.y1 - 1 - y);
                        if erx < row_style.radius && ery < row_style.radius {
                            let cdx = (row_style.radius - erx) as isize;
                            let cdy = (row_style.radius - ery) as isize;
                            if cdx * cdx + cdy * cdy > (row_style.radius * row_style.radius) as isize {
                                return THEME.surface_container;
                            }
                        }

                        // Top line: expression, then bottom line: result. Both
                        // were cut and placed by the plan; the note on why the
                        // ellipsis is measured at the rung the ink is painted at
                        // rides with the cutting in [`build_drawer_plan`].
                        if let Some(c) = row
                            .expr
                            .paint(x, y, THEME.on_surface_variant, row_style.fill)
                        {
                            return c;
                        }
                        if let Some(c) = row.result.paint(x, y, THEME.on_surface, row_style.fill) {
                            return c;
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
        // WHY its height is read from the plan: the plate grows by the rows the
        // published level gives the result run, and that is the card's one answer
        // for the repaint -- the reasoning rides with the arithmetic in
        // [`build_display_plan`].
        if let Some(op) = calc.active_operation {
            let badge_rows = plan.badge_rows;
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

        // Dual-line Top Line and Main Line / Error Banner, sampled from the plan.
        //
        // WHY the plate is read from the plan instead of re-derived: the plan
        // settled this frame's tape geometry and placed this frame's runs once,
        // and a chip measured or a run placed again here would be a second answer
        // to one question.
        {
            // The expression tape.
            //
            // WHY the tape shows wrapped lines and not the last thirty-two
            // characters: a long expression was cut off at the front with
            // nothing said about it, so the part of the expression the
            // person could not see was simply gone. The tape now holds the
            // whole expression and scrolls through it.
            if plan.has_tape && y >= TAPE_Y0 && y < plan.tape_y1 {
                let row = (y - TAPE_Y0) / plan.tape_rows;
                let line = plan.tape_first + row;
                if line < plan.tape_count {
                    if let Some(c) = plan.tape[row].paint(x, y, THEME.on_surface, THEME.surface_container) {
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
                if plan.tape_overflow {
                    let plate_h = plan.tape_y1 - TAPE_Y0;
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
            if plan.is_error {
                if y >= plan.value_y0 && y < plan.value_y0 + plan.error_rows {
                    if let Some(c) = plan.error.paint(x, y, THEME.error, THEME.surface_container) {
                        return c;
                    }
                }
            } else {
                if y >= plan.value_y0 && y < plan.value_y0 + plan.result_rows {
                    if let Some(c) = plan.result.paint(x, y, THEME.on_surface, THEME.surface_container) {
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

    // 4. Keypad Matrix (6 rows, 4 cols, every cell one key)
    for row in 0..calculator::KEY_ROWS {
        for col in 0..calculator::KEY_COLS {
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
                let is_fn = matches!(
                    action,
                    Some('p') | Some('%') | Some('~') | Some('(') | Some(')')
                );

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
                        // WHY the label is sampled from the plan: its clamped cap,
                        // its measured width and its line box are the keypad's one
                        // answer for the repaint, and this grid is over half of the
                        // surface -- the note on why the keycap is the plate and the
                        // box is centred rather than the cap rides with the clamp in
                        // [`build_keypad_plan`].
                        if let Some(c) = plan.keys[row * calculator::KEY_COLS + col]
                            .paint(x, y, text_color, btn_bg)
                        {
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

/// Re-inks the regions `dirty` names, and only those.
///
/// WHY one walker for every repaint, the panel included: the app used to own two
/// -- one that crossed the whole surface and one that crossed a band of rows --
/// and a caller could hand a small region to the walker that ignored it, so the
/// classifier's arithmetic stopped meaning anything. There is now one walk, and
/// what it costs is what `dirty` says.
///
/// WHY the plan is built here and not where it is read: a plan built inside the
/// pixel function places its runs again for every pixel that wants them, and one
/// build per repaint is the cost this removes. It is one build for the whole
/// repaint, not one per region: every region of this frame is answered out of
/// the same plan, which is what keeps the tape, the card and the key labels at
/// one text size.
///
/// WHY walking a sub-rectangle is the same picture and not an approximation:
/// `get_calculator_pixel` is a function of (`x`, `y`, the state, the plan), so
/// the pixels it answers for a region are the pixels a full walk would have
/// written there. A smaller region is less work, never a different frame.
fn render_dirty(surface: u32, calc: &CalculatorState, dirty: Dirty) {
    if dirty.is_none() {
        return;
    }
    let plan = build_calc_plan(calc);
    let mut chunk = [0u32; CHUNK];
    for region in dirty.regions() {
        let x1 = (region.x + region.w).min(W);
        let y1 = (region.y + region.h).min(H);
        let mut y = region.y;
        while y < y1 {
            let row_start = y * W + region.x;
            let width = x1 - region.x;
            let mut done = 0;
            while done < width {
                let count = (width - done).min(CHUNK);
                for i in 0..count {
                    chunk[i] = get_calculator_pixel(region.x + done + i, y, calc, &plan);
                }
                let _ = std::surface_write(surface, row_start + done, &chunk[..count]);
                done += count;
            }
            y += 1;
        }
    }
}

// -----------------------------------------------------------------------------
// Input Handling and Event Dispatch
// -----------------------------------------------------------------------------

/// The regions a repaint re-inks. Owned by `calculator::chrome`, which holds the
/// rects in it, so the press that names a region and the painter that walks it
/// read the same geometry. See [`chrome::Dirty`].
use chrome::Dirty;

enum Tap {
    None,
    Redraw(Dirty),
    Leave(&'static str),
}

fn repaint(surface: u32, calc: &mut CalculatorState, dirty: Dirty) {
    // WHY nothing is asked of the settings store here: the text size belongs to
    // the wake that brought this repaint about, and `repaint` is called for
    // every presented frame -- a digit press, a scroll, a headless tool call.
    // Reading it here was one blocking IPC per repaint and a whole-panel walk
    // whenever the answer differed, so an unrelated tap resized the screen under
    // somebody who had not touched Settings. The event loop reads the rung once
    // per wake and hands the whole panel down when it moved; see `_start`.
    //
    // WHY the card's rows go as far as the keypad's first row rather than to the
    // ladder's card bottom: the card grows with the person's text size, and a
    // region that repaints only the rows the ladder reserved would leave the last
    // rows of a scaled card showing the previous frame.
    render_dirty(surface, calc, dirty);
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
        // WHY two codes and not one arm: the keyboard has a key for each
        // bracket, so a press of `(` has to type `(`. The one arm that read
        // `40 | 41` handed both to a plate that guessed which bracket the
        // person meant, which is the merged key wearing a different name.
        40 => {
            calc.press_key('(');
            std::log("[CALCULATOR] Keypad parenthesis typed.").ok();
        }
        41 => {
            calc.press_key(')');
            std::log("[CALCULATOR] Keypad parenthesis typed.").ok();
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
        // WHY this types the percentage and not the modulo: the keyboard
        // names a key by the character it types, and the plate that carries
        // this character is the percentage key. The modulo is the operation
        // the plate reads `mod`, which no character spells, so it stays on
        // the plate. Handing the character to the other operation would
        // leave one glyph doing two things, which is the defect the keypad
        // just stopped having.
        37 => {
            calc.press_key('p');
            std::log("[CALCULATOR] Keypad percentage typed.").ok();
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
    //
    // WHY the whole panel: the drawer's sheet runs from the header's bottom edge
    // to row 1380, so opening it covers the card, the keypad and the rows between
    // them, and the header pill's own state moves with it. There is no smaller set
    // of rectangles that is also the truth, and this is the case the region model
    // is not trying to shrink.
    if chrome::history_claims(x, y) {
        calc.toggle_history();
        return Tap::Redraw(Dirty::all());
    }

    // History Drawer active interactions
    if calc.show_history {
        // Close pill: the rect the drawer draws.
        if chrome::close_claims(x, y) {
            calc.show_history = false;
            return Tap::Redraw(Dirty::all());
        }
        // Clear pill: the rect the drawer draws.
        //
        // WHY the sheet's rows and not the panel: clearing the ledger moves the
        // ledger's rows and the empty notice inside the sheet, and the drawer
        // stays open -- the sheet's own chrome, the header pill and everything the
        // sheet is drawn over are painted from state this press did not touch.
        if chrome::clear_claims(x, y) {
            calc.history.clear();
            return Tap::Redraw(Dirty::rows(DISPLAY_Y0, SHEET_Y1));
        }
        // History rows hit test
        //
        // WHY the ceiling is the chrome's and not the eight the painter used to
        // hard-code: the rows a person can reach are the rows that stand inside
        // the sheet at this text size, and a press that answered a row the
        // painter never drew is the disagreement a thumb can feel.
        //
        // WHY the same two chrome answers the plan builds its rows from: the band
        // a row is painted in is `build_drawer_plan`'s `y0`/`y1`, and those are
        // these same `history_pitch()` and `history_row_h()` at the level this
        // process publishes -- one derivation, read by both sides, so the row the
        // person lands on is the row they can see.
        let count = calc.history.count();
        for i in 0..count.min(chrome::history_visible_rows()) {
            let row_y0 = HIST_LIST_Y0 + i * chrome::history_pitch();
            let row_y1 = row_y0 + chrome::history_row_h();
            if y >= row_y0 && y < row_y1 && in_hist_row_columns(x) {
                calc.load_history_entry(i);
                calc.show_history = false;
                return Tap::Redraw(Dirty::all());
            }
        }
        // Tapping outside closes drawer
        if y >= SHEET_Y0 && y < SHEET_Y1 {
            calc.show_history = false;
            return Tap::Redraw(Dirty::all());
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
            return Tap::Redraw(Dirty::display());
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
                    '(' | ')' => {
                        std::log("[CALCULATOR] Keypad parenthesis typed.").ok();
                    }
                    'p' => {
                        std::log("[CALCULATOR] Keypad percentage typed.").ok();
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
                //
                // WHY the regions come from what the press MOVED and not from
                // the name on the key: the keypad's only state-dependent ink is
                // the armed operation's cap, so a key that neither armed nor
                // disarmed one has moved the card and nothing else -- and a key
                // that did has that cap and the one it replaced. Sorting by
                // `is_op` got both halves wrong: it walked the panel for an
                // operator, rows the press never touched included, and it walked
                // the card alone for `C` and `=`, which disarm -- leaving the
                // armed key shaded until something else repainted the row.
                let op_before = calc.active_operation;
                calc.press_key(action);
                return Tap::Redraw(chrome::press_dirty(op_before, calc.active_operation));
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

fn handle_appmcp_request(calc: &mut CalculatorState, raw: &[u8], sender: u64, msg_type: u32) -> bool {
    if msg_type != std::appmcp::APPMCP_MSG_INVOKE {
        return false;
    }
    let Some(req) = std::appmcp::AppMcpRequest::from_bytes(raw) else {
        return false;
    };

    // Zero ambient authority check: caller must present a non-zero capability token
    if req.cap_handle == 0 {
        let resp = std::appmcp::AppMcpResponse::error(
            std::appmcp::APPMCP_STATUS_PERMISSION_DENIED,
            req.tool_id,
            req.token,
        );
        let _ = std::ipc_send(sender, std::appmcp::APPMCP_MSG_RESPONSE, &resp.to_bytes());
        return true;
    }

    match req.tool_id {
        std::appmcp::TOOL_ID_CALCULATOR_EVAL => {
            let expr = req.payload();
            match calculator::evaluate_ascii(expr) {
                Ok(dec) => {
                    let mut out = [0u8; 32];
                    let len = dec.format(&mut out);
                    if let Ok(expr_str) = core::str::from_utf8(expr) {
                        calc.history.push(expr_str, dec);
                    }
                    let resp = std::appmcp::AppMcpResponse::success(req.tool_id, req.token)
                        .with_payload(&out[..len]);
                    let _ = std::ipc_send(sender, std::appmcp::APPMCP_MSG_RESPONSE, &resp.to_bytes());
                    true
                }
                Err(_) => {
                    let resp = std::appmcp::AppMcpResponse::error(
                        std::appmcp::APPMCP_STATUS_INVALID_ARGUMENTS,
                        req.tool_id,
                        req.token,
                    );
                    let _ = std::ipc_send(sender, std::appmcp::APPMCP_MSG_RESPONSE, &resp.to_bytes());
                    true
                }
            }
        }
        _ => {
            let resp = std::appmcp::AppMcpResponse::error(
                std::appmcp::APPMCP_STATUS_TOOL_NOT_FOUND,
                req.tool_id,
                req.token,
            );
            let _ = std::ipc_send(sender, std::appmcp::APPMCP_MSG_RESPONSE, &resp.to_bytes());
            true
        }
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
            std::log("[CALCULATOR] Display surface unavailable; checking for headless AppMCP invocation.").ok();
            let mut rx_buf = [0u8; 64];
            if let Ok((sender, msg_type, len)) = std::ipc::receive_any(&mut rx_buf) {
                if handle_appmcp_request(&mut calc, &rx_buf[..len], sender, msg_type) {
                    std::log("[CALCULATOR] Headless AppMCP tool executed successfully.").ok();
                    std::exit_with_code(0);
                }
            }
            std::log("[CALCULATOR] Display surface unavailable; refusing headless execution.").ok();
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
    repaint(surface, &mut calc, Dirty::all());
    let _ = std::surface_flip();
    std::log("[CALCULATOR] Initial frame presented to hardware scanout.",
    )
    .ok();

    // 5. Interactive Event Reactor Loop
    loop {
        if let Ok(events) = std::event_poll(std::EVENT_TOUCH_INPUT | std::EVENT_IPC_MSG, 100) {
            // WHY here, on the wake and not in `repaint`: Settings writes the
            // store and nothing tells this app that it did, so the rung has to be
            // re-adopted while the screen is alive -- that behaviour is delivered
            // and stays. What it must not be is a per-repaint debt: `repaint` runs
            // for every presented frame, so reading the store there cost one
            // blocking IPC plus a whole-panel walk per tap, whatever the tap was.
            // This is one query per wake that brought work here, the same shape
            // `agent_manager` and `messages` use, and a poll that timed out asks
            // the store nothing.
            //
            // WHY a moved rung is the whole panel: the level lays out the card,
            // the tape's stride, the drawer's rows and the keypad's labels, so no
            // smaller set of regions is the truth. WHY the tape is re-wrapped with
            // it: the engine wrapped the expression into lanes sized at the level
            // the last frame published, and the plan below measures at the new one
            // (see `CalculatorState::sync_tape`).
            let rung_moved = events != 0 && publish_text_scale();
            let mut input = std::InputEvent {
                kind: 0,
                x: 0,
                y: 0,
                dx: 0,
                dy: 0,
                scale_milli: 1000,
            };
            let mut dirty = Dirty::none();
            if rung_moved {
                calc.sync_tape();
                dirty = dirty.widen(Dirty::all());
            }

            if events & std::EVENT_IPC_MSG != 0 {
                let mut rx_buf = [0u8; 64];
                while let Ok((sender, msg_type, len)) = std::ipc::receive_any(&mut rx_buf) {
                    if handle_appmcp_request(&mut calc, &rx_buf[..len], sender, msg_type) {
                        // WHY the panel: a tool call types into the expression,
                        // evaluates it and adds to the ledger from outside the
                        // press paths, and this file cannot say which rows it
                        // moved without restating what the tool did -- a second
                        // answer to one question, and the expensive kind of guess.
                        dirty = dirty.widen(Dirty::all());
                    }
                }
            }

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
                    // WHY the state is taken before the key goes in: the regions
                    // a key moves are what the key CHANGED, and after the call
                    // there is nothing left to compare -- the same reading the
                    // finger's press is given. The drawer is the one difference:
                    // a key that opened or closed it covered the card and the
                    // keypad at once, whatever it typed while it was up.
                    let op_before = calc.active_operation;
                    let sheet_before = calc.show_history;
                    if key.pressed && handle_keyboard_key(&mut calc, key.code) {
                        dirty = dirty.widen(chrome::keys_dirty(
                            op_before,
                            calc.active_operation,
                            sheet_before,
                            calc.show_history,
                        ));
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
            if !dirty.is_none() {
                // WHY the tape is re-wrapped here: the expression is what
                // the tape shows, and the expression is what the input
                // changed.
                //
                // WHY a flip only when something moved: a wake that answered no
                // key re-inks no pixel and presents no frame, so it costs the
                // panel nothing.
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
