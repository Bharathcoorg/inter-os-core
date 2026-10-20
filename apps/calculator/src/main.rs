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

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

pub use calculator::{CalcError, CalculatorState, Decimal};

// -----------------------------------------------------------------------------
// 32-bit ARGB8888 Design Color Tokens (Screen 16 Minimal Dark Theme)
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFF000000;
const SURFACE_DARK: u32 = 0xFF0E1117;
const SURFACE_RAISED: u32 = 0xFF181C22;
const BORDER_SUBTLE: u32 = 0xFF262C36;
const TEXT_PRIMARY: u32 = 0xFFFFFFFF;
const TEXT_MUTED: u32 = 0xFF8B949E;

// Accent Tokens
const ACCENT_AMBER: u32 = 0xFFF59E0B;
const ACCENT_AMBER_BORDER: u32 = 0xFFD97706;
const ERROR_RED: u32 = 0xFFEF4444;

// Keypad Semantic Theme Tokens
const BTN_DIGIT_BG: u32 = 0xFF141820;
const BTN_DIGIT_BORDER: u32 = 0xFF222936;
const BTN_FN_BG: u32 = 0xFF1E2430;
const BTN_FN_BORDER: u32 = 0xFF2E3848;
const BTN_FN_TEXT: u32 = 0xFF94A3B8;
const BTN_OP_BG: u32 = 0xFF262117;
const BTN_OP_BORDER: u32 = 0xFF4D3818;
const BTN_OP_TEXT: u32 = 0xFFF59E0B;
const BTN_CLEAR_BG: u32 = 0xFF2A1518;
const BTN_CLEAR_BORDER: u32 = 0xFF522026;
const BTN_CLEAR_TEXT: u32 = 0xFFF85149;

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
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, BG_DARK, false)
}

const fn run_rows(scale: usize) -> usize {
    scale * std::design::font::GLYPH_H
}

const HEADER_H: usize = 110;

// Display Card Geometry
const DISPLAY_X0: usize = 24;
const DISPLAY_X1: usize = 696;
const DISPLAY_PAD: usize = 24;
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
        if let Some(c) = std::design::back_button::pixel(x, y, TEXT_PRIMARY, BORDER_SUBTLE, SURFACE_RAISED) {
            return c;
        }

        // Canonical App Title: "Calculator" at clean scale 1 bold with subtitle
        if let Some(c) = std::design::font::test_text_aa(x, y, 92, 34, 1, b"Calculator", TEXT_PRIMARY, BG_DARK, true) {
            return c;
        }
        if let Some(c) = std::design::font::test_text_aa(x, y, 92, 58, 1, b"Precision Arithmetic", TEXT_MUTED, BG_DARK, false) {
            return c;
        }

        // History toggle pill button at top right: x: 576..688, y: 32..80
        if x >= 576 && x < 688 && y >= 32 && y < 80 {
            let rx = (x - 576).min(687 - x);
            let ry = (y - 32).min(79 - y);
            if rx < 14 && ry < 14 {
                let cdx = (14 - rx) as isize;
                let cdy = (14 - ry) as isize;
                if cdx * cdx + cdy * cdy > 196 {
                    return BG_DARK;
                }
            }
            let (bg, border, text_c) = if calc.show_history {
                (ACCENT_AMBER, ACCENT_AMBER_BORDER, 0xFF000000)
            } else {
                (SURFACE_RAISED, BORDER_SUBTLE, TEXT_PRIMARY)
            };
            if rx < 2 || ry < 2 {
                return border;
            }
            if let Some(c) = std::design::font::test_text_aa(x, y, 604, 46, 1, b"History", text_c, bg, true) {
                return c;
            }
            return bg;
        }

        return BG_DARK;
    }

    // 2. History Drawer / Overlay (Active when calc.show_history is true)
    if calc.show_history {
        if y >= 110 && y < 1380 && x >= DISPLAY_X0 && x < DISPLAY_X1 {
            let rx = (x - DISPLAY_X0).min(DISPLAY_X1 - 1 - x);
            let ry = (y - 110).min(1379 - y);
            if rx < 24 && ry < 24 {
                let cdx = (24 - rx) as isize;
                let cdy = (24 - ry) as isize;
                if cdx * cdx + cdy * cdy > 576 {
                    return BG_DARK;
                }
            }

            // Header of History Drawer: y: 125..175
            if y >= 125 && y < 175 {
                // "Calculation History" title
                if let Some(c) = std::design::font::test_text_aa(x, y, 48, 140, 1, b"Calculation History", TEXT_PRIMARY, SURFACE_DARK, true) {
                    return c;
                }

                // Clear button pill: x: 440..530, y: 130..170
                if x >= 440 && x < 530 && y >= 130 && y < 170 {
                    let prx = (x - 440).min(529 - x);
                    let pry = (y - 130).min(169 - y);
                    if prx < 10 && pry < 10 {
                        let cdx = (10 - prx) as isize;
                        let cdy = (10 - pry) as isize;
                        if cdx * cdx + cdy * cdy > 100 {
                            return SURFACE_DARK;
                        }
                    }
                    if prx < 2 || pry < 2 {
                        return BORDER_SUBTLE;
                    }
                    if let Some(c) = std::design::font::test_text_aa(x, y, 452, 138, 1, b"CLEAR", ERROR_RED, SURFACE_RAISED, true) {
                        return c;
                    }
                    return SURFACE_RAISED;
                }

                // Close button pill: x: 550..660, y: 130..170
                if x >= 550 && x < 660 && y >= 130 && y < 170 {
                    let prx = (x - 550).min(659 - x);
                    let pry = (y - 130).min(169 - y);
                    if prx < 10 && pry < 10 {
                        let cdx = (10 - prx) as isize;
                        let cdy = (10 - pry) as isize;
                        if cdx * cdx + cdy * cdy > 100 {
                            return SURFACE_DARK;
                        }
                    }
                    if prx < 2 || pry < 2 {
                        return BORDER_SUBTLE;
                    }
                    if let Some(c) = std::design::font::test_text_aa(x, y, 566, 138, 1, b"CLOSE", TEXT_PRIMARY, SURFACE_RAISED, true) {
                        return c;
                    }
                    return SURFACE_RAISED;
                }
            }

            // Separator line
            if y == 182 && x >= 40 && x < 680 {
                return BORDER_SUBTLE;
            }

            // History Entries
            let count = calc.history.count();
            if count == 0 {
                if let Some(c) = std::design::font::test_text_aa(x, y, 180, 320, 2, b"No recent calculations", TEXT_MUTED, SURFACE_DARK, false) {
                    return c;
                }
            } else {
                for i in 0..count.min(8) {
                    let row_y0 = 195 + i * 140;
                    let row_y1 = row_y0 + 125;
                    if y >= row_y0 && y < row_y1 && x >= 40 && x < 680 {
                        let erx = (x - 40).min(679 - x);
                        let ery = (y - row_y0).min(row_y1 - 1 - y);
                        if erx < 16 && ery < 16 {
                            let cdx = (16 - erx) as isize;
                            let cdy = (16 - ery) as isize;
                            if cdx * cdx + cdy * cdy > 256 {
                                return SURFACE_DARK;
                            }
                        }

                        if let Some(entry) = calc.history.get_recent(i) {
                            // Top line: expression
                            let expr_bytes = &entry.expr[..entry.expr_len];
                            if !expr_bytes.is_empty() {
                                let ew = std::design::font::measure_bold(expr_bytes, 2);
                                let ex0 = 650usize.saturating_sub(ew);
                                if let Some(c) = std::design::font::test_text_aa(x, y, ex0, row_y0 + 16, 2, expr_bytes, TEXT_MUTED, SURFACE_RAISED, false) {
                                    return c;
                                }
                            }

                            // Bottom line: result
                            let mut res_buf = [0u8; 32];
                            let rlen = entry.result.format(&mut res_buf);
                            let rw = std::design::font::measure_bold(&res_buf[..rlen], 3);
                            let rx0 = 650usize.saturating_sub(rw);
                            if let Some(c) = std::design::font::test_text_aa(x, y, rx0, row_y0 + 58, 3, &res_buf[..rlen], TEXT_PRIMARY, SURFACE_RAISED, true) {
                                return c;
                            }
                        }

                        if erx < 2 || ery < 2 {
                            return BORDER_SUBTLE;
                        }
                        return SURFACE_RAISED;
                    }
                }
            }

            if rx < 2 || ry < 2 {
                return BORDER_SUBTLE;
            }
            return SURFACE_DARK;
        }

        return BG_DARK;
    }

    // 3. Numerical Display Area Container Card (Dual-Line: y: 110..410, x: 24..696)
    if (110..410).contains(&y) && x >= DISPLAY_X0 && x < DISPLAY_X1 {
        let rx = (x - DISPLAY_X0).min(DISPLAY_X1 - 1 - x);
        let ry = (y - 110).min(409 - y);
        if rx < 24 && ry < 24 {
            let cdx = (24 - rx) as isize;
            let cdy = (24 - ry) as isize;
            if cdx * cdx + cdy * cdy > 576 {
                return BG_DARK;
            }
        }

        // Active operation badge if set: x: 44..104, y: 130..164
        if let Some(op) = calc.active_operation {
            if (130..164).contains(&y) && (44..104).contains(&x) {
                let prx = (x - 44).min(103 - x);
                let pry = (y - 130).min(163 - y);
                if prx < 12 && pry < 12 {
                    let cdx = (12 - prx) as isize;
                    let cdy = (12 - pry) as isize;
                    if cdx * cdx + cdy * cdy <= 144 {
                        if prx < 2 || pry < 2 {
                            return ACCENT_AMBER_BORDER;
                        }
                        if let Some(c) = draw_calc_operator_glyph(x, y, 74, 147, op, ACCENT_AMBER) {
                            return c;
                        }
                        return SURFACE_RAISED;
                    }
                } else {
                    if prx < 2 || pry < 2 {
                        return ACCENT_AMBER_BORDER;
                    }
                    if let Some(c) = draw_calc_operator_glyph(x, y, 74, 147, op, ACCENT_AMBER) {
                        return c;
                    }
                    return SURFACE_RAISED;
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
                if let Some(c) = std::design::font::test_text_aa(x, y, p.tape_x, 185, 2, expr_slice, TEXT_MUTED, SURFACE_DARK, false) {
                    return c;
                }
            }
            if p.is_error {
                let h = run_rows(3);
                if y >= VALUE_Y0 && y < VALUE_Y0 + h {
                    if let Some(c) = std::design::font::test_text_aa(x, y, p.err_x, VALUE_Y0, 3, p.err_text, ERROR_RED, SURFACE_DARK, true) {
                        return c;
                    }
                }
            } else {
                let h = run_rows(p.digits_scale);
                if y >= VALUE_Y0 && y < VALUE_Y0 + h {
                    let digits = &calc.display_buf[..p.digits_len.min(calc.display_buf.len())];
                    if let Some(c) = std::design::font::test_text_aa(x, y, p.digits_x, VALUE_Y0, p.digits_scale, digits, TEXT_PRIMARY, SURFACE_DARK, true) {
                        return c;
                    }
                }
            }
        }

        if rx < 2 || ry < 2 {
            return BORDER_SUBTLE;
        }

        return SURFACE_DARK;
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
                        return BG_DARK;
                    }

                    // Keycap squircle corner test (radius: 34)
                    let rx = (x - kx0).min(kx1 - 1 - x);
                    let ry = (y - ky0).min(ky1 - 1 - y);
                    if rx < 34 && ry < 34 {
                        let cdx = (34 - rx) as isize;
                        let cdy = (34 - ry) as isize;
                        if cdx * cdx + cdy * cdy > 1156 {
                            return BG_DARK;
                        }
                    }

                    let action = get_button_action(row, col);
                    let is_active_op = action.is_some() && action == calc.active_operation;
                    let is_op = matches!(action, Some('/') | Some('*') | Some('-') | Some('+'));
                    let is_equals = action == Some('=');
                    let is_clear = action == Some('C');
                    let is_fn = matches!(action, Some('P') | Some('%') | Some('~'));

                    let (btn_bg, btn_border, text_color) = if is_active_op {
                        (ACCENT_AMBER, ACCENT_AMBER_BORDER, 0xFF000000)
                    } else if is_equals {
                        // Equals: Warm Amber accent token with black glyph
                        (ACCENT_AMBER, ACCENT_AMBER_BORDER, 0xFF000000)
                    } else if is_op {
                        // Arithmetic operators: Deep amber bronze with warm amber glyphs
                        (BTN_OP_BG, BTN_OP_BORDER, BTN_OP_TEXT)
                    } else if is_clear {
                        // Clear: Crimson wine with semantic red text
                        (BTN_CLEAR_BG, BTN_CLEAR_BORDER, BTN_CLEAR_TEXT)
                    } else if is_fn {
                        // Functions: Soft slate with cool light text
                        (BTN_FN_BG, BTN_FN_BORDER, BTN_FN_TEXT)
                    } else {
                        // Digits 0..9 and Decimal point: Obsidian slate with crisp white text
                        (BTN_DIGIT_BG, BTN_DIGIT_BORDER, TEXT_PRIMARY)
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
        return TEXT_MUTED;
    }

    BG_DARK
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
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
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
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
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
            std::log(0, "[CALCULATOR] Keypad digit pressed.").ok();
        }
        43 => {
            calc.set_operation('+');
            std::log(0, "[CALCULATOR] Keypad operator selected.").ok();
        }
        45 => {
            calc.set_operation('-');
            std::log(0, "[CALCULATOR] Keypad operator selected.").ok();
        }
        42 => {
            calc.set_operation('*');
            std::log(0, "[CALCULATOR] Keypad operator selected.").ok();
        }
        47 => {
            calc.set_operation('/');
            std::log(0, "[CALCULATOR] Keypad operator selected.").ok();
        }
        40 | 41 => {
            calc.push_parenthesis();
            std::log(0, "[CALCULATOR] Keypad operator selected.").ok();
        }
        46 => {
            calc.push_char('.');
        }
        61 | input_key_code::ENTER => {
            calc.evaluate();
            std::log(0, "[CALCULATOR] Evaluation completed.").ok();
        }
        99 | 67 | input_key_code::ESCAPE => {
            calc.clear();
            std::log(0, "[CALCULATOR] Clear pressed.").ok();
        }
        input_key_code::BACKSPACE => {
            calc.backspace();
        }
        37 => {
            calc.set_operation('%');
            std::log(0, "[CALCULATOR] Modulo operation queued.").ok();
        }
        94 | 115 | 83 => {
            calc.square();
            std::log(0, "[CALCULATOR] Square evaluated.").ok();
        }
        95 | 126 => {
            calc.toggle_sign();
            std::log(0, "[CALCULATOR] Sign toggled.").ok();
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
                                std::log(0, "[CALCULATOR] Keypad digit pressed.").ok();
                            }
                            '+' | '-' | '*' | '/' => {
                                calc.set_operation(action);
                                std::log(0, "[CALCULATOR] Keypad operator selected.").ok();
                            }
                            'P' => {
                                calc.push_parenthesis();
                                std::log(0, "[CALCULATOR] Keypad operator selected.").ok();
                            }
                            '%' => {
                                calc.set_operation('%');
                                std::log(0, "[CALCULATOR] Modulo operation queued.").ok();
                            }
                            '=' => {
                                let res = calc.evaluate();
                                let _ = res;
                                std::log(0, "[CALCULATOR] Evaluation completed.").ok();
                            }
                            'C' => {
                                calc.clear();
                                std::log(0, "[CALCULATOR] Clear pressed.").ok();
                            }
                            '~' => {
                                calc.toggle_sign();
                                std::log(0, "[CALCULATOR] Sign toggled.").ok();
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
    std::log(
        0,
        "[CALCULATOR (Ring 3)] Open-Source Touch Calculator online.",
    )
    .ok();

    let mut calc = CalculatorState::new();

    // 1. Allocate dedicated KMS Client Surface (z=200)
    std::log(
        0,
        "[CALCULATOR] Step 1: Allocating application surface (z=200)...",
    )
    .ok();
    // WHY the slot is resolved by type: a slot number is a position in a
    // table, and the position depends on the order this task's
    // capabilities were inserted in. Six happened to hold the framebuffer
    // here, and would not in an app with one more capability.
    let Some(fb_slot) = std::cap::framebuffer_slot() else {
        std::log(
            0,
            "[CALCULATOR] This app holds no framebuffer capability, so it cannot draw.",
        )
        .ok();
        std::exit_with_code(0);
    };
    let surface = match std::surface_create(fb_slot, W as u32, H as u32, 200) {
        Ok(s) => {
            std::log(0, "[CALCULATOR] Step 2: Surface created successfully.").ok();
            s
        }
        Err(_) => {
            std::log(
                0,
                "[CALCULATOR] Display surface unavailable; refusing headless execution.",
            )
            .ok();
            std::exit_with_code(1);
        }
    };

    // 2. Start with an empty expression; calculations are driven by user input.
    calc.clear();

    // 3. Render initial Calculator UI frame and present
    repaint(surface, &calc, Dirty::All);
    let _ = std::cap::framebuffer_slot().map(|slot| std::surface_flip(slot));
    std::log(
        0,
        "[CALCULATOR] Initial frame presented to hardware scanout.",
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
                    std::log(
                        0,
                        "[CALCULATOR] Navigation gesture recognized. Returning to launcher.",
                    )
                    .ok();
                    let _ = std::cap::framebuffer_slot().map(|slot| std::surface_destroy(slot, surface));
                    std::exit_with_code(0);
                }
                if input.kind == 1 || input.kind == 2 {
                    match handle_touch_point(&mut calc, input.x, input.y) {
                        Tap::Leave(msg) => {
                            std::log(0, msg).ok();
                            let _ = std::cap::framebuffer_slot().map(|slot| std::surface_destroy(slot, surface));
                            std::exit_with_code(0);
                        }
                        Tap::Redraw(d) => dirty = dirty.widen(d),
                        Tap::None => {}
                    }
                }
            }
            if dirty != Dirty::None {
                repaint(surface, &calc, dirty);
                let _ = std::cap::framebuffer_slot().map(|slot| std::surface_flip(slot));
                std::log(0, "[CALCULATOR] Post-input frame presented.").ok();
            }
        }

        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[PANIC] Calculator app error!").ok();
    std::exit_with_code(1);
}
