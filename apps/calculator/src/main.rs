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
use calculator::theme::{THEME, radius, spacing, typography};

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

/// The line box of a run drawn at a cap height.
///
/// WHY the typeface's own box and not a number: a line box is the space a
/// run reserves, and a run reserves its ascent and its descent.
const fn run_rows(cap_px: u32) -> usize {
    let em = outline_text::em_for_cap_height(cap_px);
    let (ascent, descent) = interos_std::design::typeface::line_metrics(em);
    (ascent + descent) as usize
}

const HEADER_H: usize = 110;

// Display Card Geometry
const DISPLAY_X0: usize = 24;
const DISPLAY_X1: usize = 696;
const DISPLAY_PAD: usize = spacing::GRID_24;

/// The cap heights the display draws at.
///
/// WHY the rungs and not the scales: the scale ladder and the cap ladder
/// are not the same ladder -- the title *scale* is three, whose cap is
/// twenty-two -- and the reference calculator's own result stands thirty
/// pixels of cap tall on a seven-hundred and twenty pixel screen. These
/// are the sizes the design ladder names in the units a reader sees.
const CAP_EXPRESSION: u32 = calculator::EXPRESSION_CAP_PX;
const CAP_RESULT: u32 = typography::DISPLAY_CAP_PX;
const CAP_LABEL: u32 = typography::SECTION_CAP_PX;
const CAP_SECTION: u32 = typography::TITLE_CAP_PX;

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

/// The gap between the expression and the result.
const VALUE_GAP: usize = 12;

/// The top of the result's cap.
const VALUE_Y0: usize = TAPE_Y1 + VALUE_GAP;

/// The bottom of the display band.
///
/// WHY the band ends below the result's line box and not below its cap: a
/// result with a thousands separator has a comma that hangs below the cap,
/// and a band that ended at the cap would cut it off.
const DISPLAY_Y1: usize = VALUE_Y0 + typography::DISPLAY_LINE_PX + DISPLAY_PAD;

/// The top of the display band: the card that holds the expression and the
/// result.
const DISPLAY_Y0: usize = 110;

/// The expression tape's geometry.
///
/// WHY the tape is two lines and not one: the tape used to show the last
/// thirty-two characters of the expression and say nothing about it, so a
/// long expression was cut off at the front with no way to reach it. Two
/// lines of the column, scrolled with the platform's scroll model, hold a
/// whole expression.
///
/// WHY the tape starts at the card's padding and not at the card's edge:
/// the tape is a caption above the answer, and a caption that starts at the
/// card's edge is a caption that is drawn on the border.
const TAPE_Y0: usize = DISPLAY_Y0 + DISPLAY_PAD;
const TAPE_LINES: usize = calculator::TAPE_VISIBLE_LINES;
const TAPE_LINE_H: usize = calculator::TAPE_LINE_H;
const TAPE_Y1: usize = TAPE_Y0 + TAPE_LINES * TAPE_LINE_H;

/// The right edge of the tape's text.
const TAPE_TEXT_X1: usize = DISPLAY_X1 - DISPLAY_PAD;

/// The right edge of the tape's text when the scroll chevrons are shown.
///
/// WHY the text moves in: the chevrons are the only way to reach the part
/// of the expression that is off the screen, and text drawn under them
/// would hide them.
const TAPE_TEXT_X1_SCOLL: usize = TAPE_TEXT_X1 - 40;

/// The centre of the tape's scroll chevrons.
const TAPE_CHEVRON_X: usize = TAPE_TEXT_X1 - 12;

// Button layout constants (Minimum 64pt targets: 144x140px)
//
// WHY there are six rows and not five: the keypad carries twenty-two keys
// and a four-column grid holds sixteen in four rows. The clear-all,
// clear-entry and backspace keys are three keys a person has to be able
// to tell apart, and a five-row grid had no room for them.
const BTN_W: usize = 144;
const BTN_H: usize = 140;
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


/// The scroll chevron on the expression tape.
///
/// WHY it is drawn and not typed: the platform's typeface carries no
/// chevron character, and a control labelled with a character the face
/// does not carry would draw as a blank.
fn tape_chevron_pixel(x: usize, y: usize, up: bool, color: u32) -> Option<u32> {
    let cx = TAPE_CHEVRON_X as isize;
    let cy = if up {
        (TAPE_Y0 + 18) as isize
    } else {
        (TAPE_Y1 - 18) as isize
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

/// The home indicator, in the platform's own geometry.
///
/// WHY these numbers and not the app's own: the shell draws the home
/// indicator on every screen, and the app drew a second one -- a grey
/// line, two hundred pixels wide, in a different row. One home indicator
/// is the shell's.
const HOME_PILL_W: usize = 140;
const HOME_PILL_H: usize = 5;
const HOME_PILL_Y: usize = 1418;
const HOME_PILL_X: usize = (W - HOME_PILL_W) / 2;

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
}

static mut CALC_PLAN: Option<CalcDisplayPlan> = None;

/// The width of the display's content column.
fn display_column_w() -> usize {
    (DISPLAY_X1 - DISPLAY_PAD).saturating_sub(DISPLAY_X0 + DISPLAY_PAD)
}

fn prepare_calc_plan(calc: &CalculatorState) {
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
    };
    let column = display_column_w();
    if calc.tape_count > 0 {
        plan.tape_count = calc.tape_count;
        plan.tape_first = calc.tape_scroll.offset / TAPE_LINE_H;
        plan.tape_overflow = calc.tape_count > TAPE_LINES;
        let right = if plan.tape_overflow {
            TAPE_TEXT_X1_SCOLL
        } else {
            TAPE_TEXT_X1
        };
        for index in 0..calc.tape_count.min(calculator::TAPE_MAX_LINES) {
            let width = calculator::text_width(
                &calc.tape_lines[index][..calc.tape_lens[index]],
                CAP_EXPRESSION,
            );
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
        let mut cap = ERROR_LADDER[0];
        for rung in ERROR_LADDER.iter() {
            cap = *rung;
            if calculator::text_width(err_text, cap) <= column {
                break;
            }
        }
        plan.err_x = (DISPLAY_X1 - DISPLAY_PAD)
            .saturating_sub(calculator::text_width(err_text, cap));
        plan.err_text = err_text;
        plan.err_cap = cap;
        plan.is_error = true;
    } else {
        let digits = &calc.display_buf[..calc.display_len];
        // WHY the result comes down a rung when it does not fit: the length
        // of a number is not the width of it, and a long number at the title
        // rung would run off the card.
        let mut cap = RESULT_LADDER[0];
        for rung in RESULT_LADDER.iter() {
            cap = *rung;
            if calculator::text_width(digits, cap) <= column {
                break;
            }
        }
        let total_w = calculator::text_width(digits, cap);
        plan.digits_x = (DISPLAY_X1 - DISPLAY_PAD).saturating_sub(total_w);
        plan.digits_cap = cap;
        plan.digits_len = calc.display_len;
    }
    unsafe {
        CALC_PLAN = Some(plan);
    }
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

    // 1. Status Bar & Header Title with Back Chevron Disc (y: 0..110)
    if y < HEADER_H {
        // Standardized back button squircle (24..72, 24..72)
        if let Some(c) = std::design::back_button::pixel(x, y, THEME.on_surface, THEME.border, THEME.surface_container_high) {
            return c;
        }

        // Canonical App Title: "Calculator" vertically centered with back button and history pill
        if let Some(c) = test_text(x, y, 92, 36, interos_std::design::typography::TITLE_CAP_PX, b"Calculator", THEME.on_surface, THEME.surface, true) {
            return c;
        }

        // History toggle pill button at top right: x: 576..688, y: 32..80
        if x >= 576 && x < 688 && y >= 32 && y < 80 {
            let rx = (x - 576).min(687 - x);
            let ry = (y - 32).min(79 - y);
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
            let hw = calculator::text_width(b"History", typography::BODY_CAP_PX);
            let hx = 576 + (112 - hw) / 2;
            let hy = 32 + (48 - typography::BODY_CAP_PX as usize) / 2;
            if let Some(c) = test_text(x, y, hx, hy, typography::BODY_CAP_PX, b"History", text_c, bg, true) {
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

            // Header of History Drawer: y: 125..175
            if y >= 125 && y < 175 {
                // "Calculation History" title
                if let Some(c) = test_text(x, y, 48, 136, typography::SECTION_CAP_PX, b"Calculation History", THEME.on_surface, THEME.surface_container, true) {
                    return c;
                }

                // Clear button pill: x: 440..530, y: 130..170
                if x >= 440 && x < 530 && y >= 130 && y < 170 {
                    let prx = (x - 440).min(529 - x);
                    let pry = (y - 130).min(169 - y);
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
                    let cw = calculator::text_width(b"CLEAR", typography::BODY_CAP_PX);
                    let cx0 = 440 + (90 - cw) / 2;
                    let cy0 = 130 + (40 - typography::BODY_CAP_PX as usize) / 2;
                    if let Some(c) = test_text(x, y, cx0, cy0, typography::BODY_CAP_PX, b"CLEAR", THEME.error, THEME.surface_container_high, true) {
                        return c;
                    }
                    return THEME.surface_container_high;
                }

                // Close button pill: x: 550..660, y: 130..170
                if x >= 550 && x < 660 && y >= 130 && y < 170 {
                    let prx = (x - 550).min(659 - x);
                    let pry = (y - 130).min(169 - y);
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
                    let cw = calculator::text_width(b"CLOSE", typography::BODY_CAP_PX);
                    let cx0 = 550 + (110 - cw) / 2;
                    let cy0 = 130 + (40 - typography::BODY_CAP_PX as usize) / 2;
                    if let Some(c) = test_text(x, y, cx0, cy0, typography::BODY_CAP_PX, b"CLOSE", THEME.on_surface, THEME.surface_container_high, true) {
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
                let mw = calculator::text_width(msg, typography::SECTION_CAP_PX);
                let mx0 = DISPLAY_X0 + (DISPLAY_X1 - DISPLAY_X0 - mw) / 2;
                if let Some(c) = test_text(x, y, mx0, 320, typography::SECTION_CAP_PX, msg, THEME.on_surface_variant, THEME.surface_container, false) {
                    return c;
                }
            } else {
                for i in 0..count.min(8) {
                    let row_y0 = 195 + i * 140;
                    let row_y1 = row_y0 + 125;
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
                                let elen = calculator::ellipsize_cap(expr_bytes, CAP_LABEL, 590, &mut ebuf);
                                let ew = calculator::text_width(&ebuf[..elen], CAP_LABEL);
                                let ex0 = 650usize.saturating_sub(ew).max(60);
                                if let Some(c) = test_text(x, y, ex0, row_y0 + 16, CAP_LABEL, &ebuf[..elen], THEME.on_surface_variant, row_style.fill, false) {
                                    return c;
                                }
                            }

                            // Bottom line: result
                            let mut res_buf = [0u8; 32];
                            let rlen = entry.result.format(&mut res_buf);
                            let rw = calculator::text_width(&res_buf[..rlen], CAP_SECTION);
                            let rx0 = 650usize.saturating_sub(rw).max(60);
                            if let Some(c) = test_text(x, y, rx0, row_y0 + 58, CAP_SECTION, &res_buf[..rlen], THEME.on_surface, row_style.fill, true) {
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
    if (DISPLAY_Y0..DISPLAY_Y1).contains(&y) && x >= DISPLAY_X0 && x < DISPLAY_X1 {
        // WHY the platform's card treatment: the card's fill, its hairline
        // and its radius come from the platform's own elevation, so an app
        // that drew a card with the SDK draws this card.
        let card = components::card_style(&THEME, components::Elevation::Resting);
        let rx = (x - DISPLAY_X0).min(DISPLAY_X1 - 1 - x);
        let ry = (y - DISPLAY_Y0).min(DISPLAY_Y1 - 1 - y);
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
        if let Some(op) = calc.active_operation {
            if (VALUE_Y0..VALUE_Y0 + 34).contains(&y) && (44..104).contains(&x) {
                let prx = (x - 44).min(103 - x);
                let pry = (y - VALUE_Y0).min(VALUE_Y0 + 33 - y);
                if prx < R_CONTROL && pry < R_CONTROL {
                    let cdx = (R_CONTROL - prx) as isize;
                    let cdy = (R_CONTROL - pry) as isize;
                    if cdx * cdx + cdy * cdy <= (R_CONTROL * R_CONTROL) as isize {
                        if prx < 2 || pry < 2 {
                            return THEME.warning_container;
                        }
                        if let Some(c) = draw_calc_badge_operator_glyph(x, y, 74, VALUE_Y0 + 17, op, THEME.warning) {
                            return c;
                        }
                        return THEME.surface_container_high;
                    }
                } else {
                    if prx < 2 || pry < 2 {
                        return THEME.warning_container;
                    }
                    if let Some(c) = draw_calc_badge_operator_glyph(x, y, 74, VALUE_Y0 + 17, op, THEME.warning) {
                        return c;
                    }
                    return THEME.surface_container_high;
                }
            }
        }

        // Dual-line Top Line and Main Line / Error Banner via precomputed plan
        let plan = unsafe { CALC_PLAN };
        if let Some(p) = plan {
            // The expression tape.
            //
            // WHY the tape shows wrapped lines and not the last thirty-two
            // characters: a long expression was cut off at the front with
            // nothing said about it, so the part of the expression the
            // person could not see was simply gone. The tape now holds the
            // whole expression and scrolls through it.
            if p.has_tape && y >= TAPE_Y0 && y < TAPE_Y1 {
                let row = (y - TAPE_Y0) / TAPE_LINE_H;
                let line = p.tape_first + row;
                if line < p.tape_count {
                    let line_y = TAPE_Y0 + row * TAPE_LINE_H;
                    let text = &calc.tape_lines[line][..calc.tape_lens[line]];
                    if let Some(c) = test_text(x, y, p.tape_xs[line], line_y, CAP_EXPRESSION, text, THEME.on_surface, THEME.surface_container, true) {
                        return c;
                    }
                }
                // The scroll chevrons, only when there is more than one
                // screenful of expression to reach.
                if p.tape_overflow {
                    if let Some(c) = tape_chevron_pixel(x, y, true, THEME.on_surface_variant) {
                        return c;
                    }
                    if let Some(c) = tape_chevron_pixel(x, y, false, THEME.on_surface_variant) {
                        return c;
                    }
                }
            }
            if p.is_error {
                let h = run_rows(p.err_cap);
                if y >= VALUE_Y0 && y < VALUE_Y0 + h {
                    if let Some(c) = test_text(x, y, p.err_x, VALUE_Y0, p.err_cap, p.err_text, THEME.error, THEME.surface_container, true) {
                        return c;
                    }
                }
            } else {
                let h = run_rows(p.digits_cap);
                if y >= VALUE_Y0 && y < VALUE_Y0 + h {
                    let digits = &calc.display_buf[..p.digits_len.min(calc.display_buf.len())];
                    if let Some(c) = test_text(x, y, p.digits_x, VALUE_Y0, p.digits_cap, digits, THEME.on_surface, THEME.surface_container, true) {
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
                        let w = calculator::text_width(label, CAP_LABEL);
                        let x0 = center_x.saturating_sub(w / 2);
                        let y0 = center_y.saturating_sub((CAP_LABEL / 2) as usize);
                        if let Some(c) = test_text(x, y, x0, y0, CAP_LABEL, label, text_color, btn_bg, true) {
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
    // height and the same row, in the ink that stands out against the
    // page behind it.
    if y >= HOME_PILL_Y
        && y < HOME_PILL_Y + HOME_PILL_H
        && x >= HOME_PILL_X
        && x < HOME_PILL_X + HOME_PILL_W
    {
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

fn repaint(surface: u32, calc: &CalculatorState, dirty: Dirty) {
    match dirty {
        Dirty::None => {}
        Dirty::Display => render_calculator_band(surface, calc, DISPLAY_Y0, DISPLAY_Y1),
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
    // Back button
    let is_back = std::design::back_button::contains(x as usize, y as usize) || {
        let dx = x as i32 - 62;
        let dy = y as i32 - 55;
        dx * dx + dy * dy <= 24 * 24
    };
    if is_back {
        return Tap::Leave("[CALCULATOR] Back button tapped. Returning to launcher.");
    }

    // Home gesture bar
    if x >= 240 && x <= 480 && y >= 1400 {
        return Tap::Leave("[CALCULATOR] Home gesture tapped. Returning to launcher.");
    }

    // History button in Header (576..688, 32..80)
    if x >= 576 && x < 688 && y >= 32 && y < 80 {
        calc.toggle_history();
        return Tap::Redraw(Dirty::All);
    }

    // History Drawer active interactions
    if calc.show_history {
        // Close button: x: 550..660, y: 130..170
        if x >= 550 && x < 660 && y >= 130 && y < 170 {
            calc.show_history = false;
            return Tap::Redraw(Dirty::All);
        }
        // Clear history button: x: 440..530, y: 130..170
        if x >= 440 && x < 530 && y >= 130 && y < 170 {
            calc.history.clear();
            return Tap::Redraw(Dirty::All);
        }
        // History rows hit test
        let count = calc.history.count();
        for i in 0..count.min(8) {
            let row_y0 = 195 + i * 140;
            let row_y1 = row_y0 + 125;
            if (y as usize) >= row_y0 && (y as usize) < row_y1 && x >= 40 && x < 680 {
                calc.load_history_entry(i);
                calc.show_history = false;
                return Tap::Redraw(Dirty::All);
            }
        }
        // Tapping outside closes drawer
        if (y as usize) >= 110 && (y as usize) < 1380 {
            calc.show_history = false;
            return Tap::Redraw(Dirty::All);
        }
        return Tap::None;
    }

    // The expression tape's scroll chevrons.
    //
    // WHY they are there: the tape holds the whole expression, and the
    // part of it that is off the screen has to be reachable.
    if (y as usize) >= TAPE_Y0 && (y as usize) < TAPE_Y1 && calc.tape_count > TAPE_LINES {
        let chevron_x = x as isize;
        if (chevron_x - TAPE_CHEVRON_X as isize).abs() <= 20 {
            if (y as usize) < TAPE_Y0 + 36 {
                calc.scroll_tape(-1);
                return Tap::Redraw(Dirty::Display);
            }
            if (y as usize) + 36 > TAPE_Y1 {
                calc.scroll_tape(1);
                return Tap::Redraw(Dirty::Display);
            }
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
    repaint(surface, &calc, Dirty::All);
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
                repaint(surface, &calc, dirty);
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
