//! InterOS Core Open-Source Reference App: Interactive Touch Calculator
//!
//! Full production Ring-3 graphical application executing in User Mode:
//! - DRM/KMS Client Surface allocation (z=200) and double-buffered scanout presentation
//! - High-contrast dual-line digital expression and evaluation result display
//! - 5x4 rounded mobile keypad matrix with semantic design tokens and minimum 64pt targets
//! - Interactive touch event processing and button hit-testing
//! - Full arithmetic execution engine: Add, Subtract, Multiply, Divide, Clear, Evaluate
//! - History drawer / overlay for past calculations ledger
//! - Banded rendering (`render_calculator_band`): only dirty rows are re-rasterised
//! - Graceful lifecycle supervision and clean return to Launcher

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use calculator::theme::{THEME, radius, spacing};


/// The platform's corner radii, under the short names the renderer uses.
///
/// WHY the aliases exist: the corner test names its radius four times, and a
/// number that appears four times is a number that can drift from the
/// platform. The alias resolves to the platform's step, so the drift is the
/// platform's and not this app's.
const R_CHIP: usize = radius::CHIP;
const R_CONTROL: usize = radius::CONTROL;
const R_MAJOR: usize = radius::MAJOR;
const R_SHEET: usize = radius::SHEET;
const R_SOFT: usize = radius::SOFT_PILL;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

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
#[allow(dead_code)]
#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
) -> Option<u32> {
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, THEME.surface, false)
}

const fn run_rows(scale: usize) -> usize {
    scale * std::design::font::GLYPH_H
}

const HEADER_H: usize = 110;

// Display Card Geometry
const DISPLAY_X0: usize = 24;
const DISPLAY_X1: usize = 696;
const DISPLAY_PAD: usize = spacing::GRID_24;
const VALUE_Y0: usize = 290;

// Button layout constants (Minimum 64pt targets: 144x140px)
const BTN_W: usize = 144;
const BTN_H: usize = 140;
const COL_X: [usize; 4] = [28, 194, 360, 526];
const ROW_Y: [usize; 5] = [440, 610, 780, 950, 1120];

fn draw_calc_operator_glyph(x: usize, y: usize, cx: usize, cy: usize, op: char, color: u32) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    match op {
        '/' => {
            // Division '÷': horizontal bar and top/bottom dots
            if dx.abs() <= 14 && dy.abs() <= 2 {
                return Some(color);
            }
            if dx * dx + (dy + 14) * (dy + 14) <= 12 {
                return Some(color);
            }
            if dx * dx + (dy - 14) * (dy - 14) <= 12 {
                return Some(color);
            }
            None
        }
        '*' => {
            // Multiplication '×': two crossing diagonal bars
            let d1 = (dx - dy).abs();
            let d2 = (dx + dy).abs();
            if dx.abs() <= 12 && dy.abs() <= 12 && (d1 <= 2 || d2 <= 2) {
                return Some(color);
            }
            None
        }
        '-' => {
            // Subtraction '-': clean horizontal bar
            if dx.abs() <= 14 && dy.abs() <= 2 {
                return Some(color);
            }
            None
        }
        '+' => {
            // Addition '+': symmetric cross
            if (dx.abs() <= 14 && dy.abs() <= 2) || (dx.abs() <= 2 && dy.abs() <= 14) {
                return Some(color);
            }
            None
        }
        '=' => {
            // Equals '=': two parallel horizontal bars
            if dx.abs() <= 13 && ((dy + 6).abs() <= 2 || (dy - 6).abs() <= 2) {
                return Some(color);
            }
            None
        }
        _ => None,
    }
}

fn get_button_action(row: usize, col: usize) -> Option<char> {
    match (row, col) {
        (0, 0) => Some('C'), // AC (All Clear)
        (0, 1) => Some('P'), // () Parentheses
        (0, 2) => Some('%'), // % Percent
        (0, 3) => Some('/'), // ÷ Divide
        (1, 0) => Some('7'),
        (1, 1) => Some('8'),
        (1, 2) => Some('9'),
        (1, 3) => Some('*'), // × Multiply
        (2, 0) => Some('4'),
        (2, 1) => Some('5'),
        (2, 2) => Some('6'),
        (2, 3) => Some('-'), // - Subtract
        (3, 0) => Some('1'),
        (3, 1) => Some('2'),
        (3, 2) => Some('3'),
        (3, 3) => Some('+'), // + Add
        (4, 0) => Some('~'), // +/- Sign flip
        (4, 1) => Some('0'),
        (4, 2) => Some('.'), // . Decimal point
        (4, 3) => Some('='), // = Evaluate
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct CalcDisplayPlan {
    has_tape: bool,
    tape_x: usize,
    is_error: bool,
    err_text: &'static [u8],
    err_x: usize,
    digits_len: usize,
    digits_scale: usize,
    digits_x: usize,
}

static mut CALC_PLAN: Option<CalcDisplayPlan> = None;

fn prepare_calc_plan(calc: &CalculatorState) {
    let mut plan = CalcDisplayPlan {
        has_tape: false,
        tape_x: 0,
        is_error: false,
        err_text: b"",
        err_x: 0,
        digits_len: 0,
        digits_scale: 3,
        digits_x: 0,
    };
    let expr_slice = if calc.expr_len > 0 {
        let start = calc.expr_len.saturating_sub(32);
        &calc.expr_buf[start..calc.expr_len]
    } else if let Some(last_entry) = calc.history.get_recent(0) {
        &last_entry.expr[..last_entry.expr_len]
    } else {
        b""
    };
    if !expr_slice.is_empty() {
        let tape_w = std::design::font::measure_bold(expr_slice, 2);
        plan.tape_x = (DISPLAY_X1 - DISPLAY_PAD).saturating_sub(tape_w);
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
        let err_w = std::design::font::measure_bold(err_text, 3);
        plan.err_x = (DISPLAY_X1 - DISPLAY_PAD).saturating_sub(err_w);
        plan.err_text = err_text;
        plan.is_error = true;
    } else {
        let digits = &calc.display_buf[..calc.display_len];
        let scale: usize = if digits.len() <= 7 {
            3
        } else if digits.len() <= 10 {
            2
        } else {
            1
        };
        let total_w = std::design::font::measure_bold(digits, scale);
        plan.digits_x = (DISPLAY_X1 - DISPLAY_PAD).saturating_sub(total_w);
        plan.digits_scale = scale;
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
    // 1. Status Bar & Header Title with Back Chevron Disc (y: 0..110)
    if y < HEADER_H {
        // Standardized back button squircle (24..72, 24..72)
        if let Some(c) = std::design::back_button::pixel(x, y, THEME.on_surface, THEME.border, THEME.surface_container_high) {
            return c;
        }

        // Canonical App Title: "Calculator" at clean scale 1 bold with subtitle
        if let Some(c) = std::design::font::test_text_aa(x, y, 92, 34, 1, b"Calculator", THEME.on_surface, THEME.surface, true) {
            return c;
        }
        if let Some(c) = std::design::font::test_text_aa(x, y, 92, 58, 1, b"Precision Arithmetic", THEME.on_surface_variant, THEME.surface, false) {
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
            if let Some(c) = std::design::font::test_text_aa(x, y, 604, 46, 1, b"History", text_c, bg, true) {
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
                if let Some(c) = std::design::font::test_text_aa(x, y, 48, 140, 1, b"Calculation History", THEME.on_surface, THEME.surface_container, true) {
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
                    if let Some(c) = std::design::font::test_text_aa(x, y, 452, 138, 1, b"CLEAR", THEME.error, THEME.surface_container_high, true) {
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
                    if let Some(c) = std::design::font::test_text_aa(x, y, 566, 138, 1, b"CLOSE", THEME.on_surface, THEME.surface_container_high, true) {
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
            if count == 0 {
                if let Some(c) = std::design::font::test_text_aa(x, y, 180, 320, 2, b"No recent calculations", THEME.on_surface_variant, THEME.surface_container, false) {
                    return c;
                }
            } else {
                for i in 0..count.min(8) {
                    let row_y0 = 195 + i * 140;
                    let row_y1 = row_y0 + 125;
                    if y >= row_y0 && y < row_y1 && x >= 40 && x < 680 {
                        let erx = (x - 40).min(679 - x);
                        let ery = (y - row_y0).min(row_y1 - 1 - y);
                        if erx < R_SOFT && ery < R_SOFT {
                            let cdx = (R_SOFT - erx) as isize;
                            let cdy = (R_SOFT - ery) as isize;
                            if cdx * cdx + cdy * cdy > (R_SOFT * R_SOFT) as isize {
                                return THEME.surface_container;
                            }
                        }

                        if let Some(entry) = calc.history.get_recent(i) {
                            // Top line: expression
                            let expr_bytes = &entry.expr[..entry.expr_len];
                            if !expr_bytes.is_empty() {
                                let ew = std::design::font::measure_bold(expr_bytes, 2);
                                let ex0 = 650usize.saturating_sub(ew);
                                if let Some(c) = std::design::font::test_text_aa(x, y, ex0, row_y0 + 16, 2, expr_bytes, THEME.on_surface_variant, THEME.surface_container_high, false) {
                                    return c;
                                }
                            }

                            // Bottom line: result
                            let mut res_buf = [0u8; 32];
                            let rlen = entry.result.format(&mut res_buf);
                            let rw = std::design::font::measure_bold(&res_buf[..rlen], 3);
                            let rx0 = 650usize.saturating_sub(rw);
                            if let Some(c) = std::design::font::test_text_aa(x, y, rx0, row_y0 + 58, 3, &res_buf[..rlen], THEME.on_surface, THEME.surface_container_high, true) {
                                return c;
                            }
                        }

                        if erx < 2 || ery < 2 {
                            return THEME.border;
                        }
                        return THEME.surface_container_high;
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

    // 3. Numerical Display Area Container Card (Dual-Line: y: 110..410, x: 24..696)
    if (110..410).contains(&y) && x >= DISPLAY_X0 && x < DISPLAY_X1 {
        let rx = (x - DISPLAY_X0).min(DISPLAY_X1 - 1 - x);
        let ry = (y - 110).min(409 - y);
        if rx < R_SHEET && ry < R_SHEET {
            let cdx = (R_SHEET - rx) as isize;
            let cdy = (R_SHEET - ry) as isize;
            if cdx * cdx + cdy * cdy > (R_SHEET * R_SHEET) as isize {
                return THEME.surface;
            }
        }

        // Active operation badge if set: x: 44..104, y: 130..164
        if let Some(op) = calc.active_operation {
            if (130..164).contains(&y) && (44..104).contains(&x) {
                let prx = (x - 44).min(103 - x);
                let pry = (y - 130).min(163 - y);
                if prx < R_CONTROL && pry < R_CONTROL {
                    let cdx = (R_CONTROL - prx) as isize;
                    let cdy = (R_CONTROL - pry) as isize;
                    if cdx * cdx + cdy * cdy <= (R_CONTROL * R_CONTROL) as isize {
                        if prx < 2 || pry < 2 {
                            return THEME.warning_container;
                        }
                        if let Some(c) = draw_calc_operator_glyph(x, y, 74, 147, op, THEME.warning) {
                            return c;
                        }
                        return THEME.surface_container_high;
                    }
                } else {
                    if prx < 2 || pry < 2 {
                        return THEME.warning_container;
                    }
                    if let Some(c) = draw_calc_operator_glyph(x, y, 74, 147, op, THEME.warning) {
                        return c;
                    }
                    return THEME.surface_container_high;
                }
            }
        }

        // Dual-line Top Line and Main Line / Error Banner via precomputed plan
        let plan = unsafe { CALC_PLAN };
        if let Some(p) = plan {
            if p.has_tape && y >= 185 && y < 233 {
                let expr_slice = if calc.expr_len > 0 {
                    let start = calc.expr_len.saturating_sub(32);
                    &calc.expr_buf[start..calc.expr_len]
                } else if let Some(last_entry) = calc.history.get_recent(0) {
                    &last_entry.expr[..last_entry.expr_len]
                } else {
                    b""
                };
                if let Some(c) = std::design::font::test_text_aa(x, y, p.tape_x, 185, 2, expr_slice, THEME.on_surface_variant, THEME.surface_container, false) {
                    return c;
                }
            }
            if p.is_error {
                let h = run_rows(3);
                if y >= VALUE_Y0 && y < VALUE_Y0 + h {
                    if let Some(c) = std::design::font::test_text_aa(x, y, p.err_x, VALUE_Y0, 3, p.err_text, THEME.error, THEME.surface_container, true) {
                        return c;
                    }
                }
            } else {
                let h = run_rows(p.digits_scale);
                if y >= VALUE_Y0 && y < VALUE_Y0 + h {
                    let digits = &calc.display_buf[..p.digits_len.min(calc.display_buf.len())];
                    if let Some(c) = std::design::font::test_text_aa(x, y, p.digits_x, VALUE_Y0, p.digits_scale, digits, THEME.on_surface, THEME.surface_container, true) {
                        return c;
                    }
                }
            }
        }

        if rx < 2 || ry < 2 {
            return THEME.border;
        }

        return THEME.surface_container;
    }

    // 4. Keypad Matrix (5 rows, 4 cols)
    for row in 0..5 {
        let by = ROW_Y[row];
        if y >= by && y < by + BTN_H {
            for col in 0..4 {
                let bx = COL_X[col];
                if x >= bx && x < bx + BTN_W {
                    let kx0 = bx + 4;
                    let kx1 = bx + BTN_W - 4;
                    let ky0 = by + 4;
                    let ky1 = by + BTN_H - 4;

                    if x < kx0 || x >= kx1 || y < ky0 || y >= ky1 {
                        return THEME.surface;
                    }

                    // Keycap squircle corner test (radius: 34)
                    let rx = (x - kx0).min(kx1 - 1 - x);
                    let ry = (y - ky0).min(ky1 - 1 - y);
                    if rx < R_MAJOR && ry < R_MAJOR {
                        let cdx = (R_MAJOR - rx) as isize;
                        let cdy = (R_MAJOR - ry) as isize;
                        if cdx * cdx + cdy * cdy > (R_MAJOR * R_MAJOR) as isize {
                            return THEME.surface;
                        }
                    }

                    let action = get_button_action(row, col);
                    let is_active_op = action.is_some() && action == calc.active_operation;
                    let is_op = matches!(action, Some('/') | Some('*') | Some('-') | Some('+'));
                    let is_equals = action == Some('=');
                    let is_clear = action == Some('C');
                    let is_fn = matches!(action, Some('P') | Some('%') | Some('~'));

                    let (btn_bg, btn_border, text_color) = if is_active_op {
                        (THEME.warning, THEME.warning_container, THEME.on_primary)
                    } else if is_equals {
                        // Equals: Warm Amber accent token with black glyph
                        (THEME.warning, THEME.warning_container, THEME.on_primary)
                    } else if is_op {
                        // Arithmetic operators: Deep amber bronze with warm amber glyphs
                        (THEME.warning_container, THEME.warning_container, THEME.warning)
                    } else if is_clear {
                        // Clear: Crimson wine with semantic red text
                        (THEME.error_container, THEME.error_container, THEME.error)
                    } else if is_fn {
                        // Functions: Soft slate with cool light text
                        (THEME.surface_container_high, THEME.border, THEME.on_surface_variant)
                    } else {
                        // Digits 0..9 and Decimal point: Obsidian slate with crisp white text
                        (THEME.surface_container, THEME.border, THEME.on_surface)
                    };

                    if rx < 2 || ry < 2 {
                        return btn_border;
                    }

                    let center_x = bx + BTN_W / 2;
                    let center_y = by + BTN_H / 2;

                    if let Some(act) = action {
                        if matches!(act, '/' | '*' | '-' | '+' | '=') {
                            if let Some(c) = draw_calc_operator_glyph(x, y, center_x, center_y, act, text_color) {
                                return c;
                            }
                        } else {
                            let label: &[u8] = match act {
                                'C' => b"AC",
                                'P' => b"()",
                                '~' => b"+/-",
                                '.' => b".",
                                _ => &[act as u8],
                            };
                            let scale = if label.len() > 1 { 1 } else { 2 };
                            let w = std::design::font::measure_bold(label, scale);
                            let x0 = center_x.saturating_sub(w / 2);
                            let y_offset = if scale == 1 { 12 } else { 24 };
                            let y0 = center_y.saturating_sub(y_offset);
                            if let Some(c) = std::design::font::test_text_aa(x, y, x0, y0, scale, label, text_color, btn_bg, true) {
                                return c;
                            }
                        }
                    }

                    return btn_bg;
                }
            }
        }
    }

    // 5. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return THEME.on_surface_variant;
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

const DISPLAY_Y0: usize = 110;
const DISPLAY_Y1: usize = 410;

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
    match code {
        48..=57 => {
            calc.push_digit((code - 48) as i64);
            std::log("[CALCULATOR] Keypad digit pressed.").ok();
        }
        43 => {
            calc.set_operation('+');
            std::log("[CALCULATOR] Keypad operator selected.").ok();
        }
        45 => {
            calc.set_operation('-');
            std::log("[CALCULATOR] Keypad operator selected.").ok();
        }
        42 => {
            calc.set_operation('*');
            std::log("[CALCULATOR] Keypad operator selected.").ok();
        }
        47 => {
            calc.set_operation('/');
            std::log("[CALCULATOR] Keypad operator selected.").ok();
        }
        40 | 41 => {
            calc.push_parenthesis();
            std::log("[CALCULATOR] Keypad operator selected.").ok();
        }
        46 => {
            calc.push_char('.');
        }
        61 | input_key_code::ENTER => {
            calc.evaluate();
            std::log("[CALCULATOR] Evaluation completed.").ok();
        }
        99 | 67 | input_key_code::ESCAPE => {
            calc.clear();
            std::log("[CALCULATOR] Clear pressed.").ok();
        }
        input_key_code::BACKSPACE => {
            calc.backspace();
        }
        37 => {
            calc.set_operation('%');
            std::log("[CALCULATOR] Modulo operation queued.").ok();
        }
        94 | 115 | 83 => {
            calc.square();
            std::log("[CALCULATOR] Square evaluated.").ok();
        }
        95 | 126 => {
            calc.toggle_sign();
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

    // Keypad Matrix hit test
    for row in 0..5 {
        let by = ROW_Y[row] as u32;
        if y >= by && y < by + (BTN_H as u32) {
            for col in 0..4 {
                let bx = COL_X[col] as u32;
                if x >= bx && x < bx + (BTN_W as u32) {
                    if let Some(action) = get_button_action(row, col) {
                        match action {
                            '0'..='9' => {
                                let digit = (action as u8 - b'0') as i64;
                                calc.push_digit(digit);
                                std::log("[CALCULATOR] Keypad digit pressed.").ok();
                            }
                            '+' | '-' | '*' | '/' => {
                                calc.set_operation(action);
                                std::log("[CALCULATOR] Keypad operator selected.").ok();
                            }
                            'P' => {
                                calc.push_parenthesis();
                                std::log("[CALCULATOR] Keypad operator selected.").ok();
                            }
                            '%' => {
                                calc.set_operation('%');
                                std::log("[CALCULATOR] Modulo operation queued.").ok();
                            }
                            '=' => {
                                let res = calc.evaluate();
                                let _ = res;
                                std::log("[CALCULATOR] Evaluation completed.").ok();
                            }
                            'C' => {
                                calc.clear();
                                std::log("[CALCULATOR] Clear pressed.").ok();
                            }
                            '~' => {
                                calc.toggle_sign();
                                std::log("[CALCULATOR] Sign toggled.").ok();
                            }
                            '.' => {
                                calc.push_char('.');
                            }
                            _ => {}
                        }
                        return Tap::Redraw(Dirty::Display);
                    }
                    return Tap::None;
                }
            }
        }
    }

    Tap::None
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[CALCULATOR (Ring 3)] Open-Source Touch Calculator online.",
    )
    .ok();

    let mut calc = CalculatorState::new();

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

    // 3. Render initial Calculator UI frame and present
    repaint(surface, &calc, Dirty::All);
    let _ = std::surface_flip();
    std::log("[CALCULATOR] Initial frame presented to hardware scanout.",
    )
    .ok();

    // 4. Interactive Event Reactor Loop
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
