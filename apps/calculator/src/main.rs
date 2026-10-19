//! InterOS Core Open-Source Reference App: Interactive Touch Calculator
//!
//! Full production Ring-3 graphical application executing in User Mode:
//! - DRM/KMS Client Surface allocation (z=200) and double-buffered scanout presentation
//! - 7-segment digital display rendering for high-contrast mathematical evaluation
//! - 5x4 rounded mobile keypad matrix with semantic design tokens
//! - Interactive touch event processing and button hit-testing
//! - Full arithmetic execution engine: Add, Subtract, Multiply, Divide, Clear, Evaluate
//! - Banded rendering (`render_calculator_band`): only the rows an input
//!   actually dirtied are re-rasterised, instead of all 1,036,800 pixels on
//!   every event
//! - Graceful lifecycle supervision and clean return to Launcher

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

pub use calculator::CalculatorState;

// -----------------------------------------------------------------------------
// 32-bit ARGB8888 Design Color Tokens (Screen 16 Minimal Light Theme)
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFF000000;
#[allow(dead_code)]
const SURFACE_DARK: u32 = 0xFF0E1117;
#[allow(dead_code)]
const SURFACE_RAISED: u32 = 0xFF181C22;
#[allow(dead_code)]
const BORDER_SUBTLE: u32 = 0xFF262C36;
const TEXT_PRIMARY: u32 = 0xFFFFFFFF;
const TEXT_MUTED: u32 = 0xFF8B949E;
#[allow(dead_code)]
const ACCENT_SKY: u32 = 0xFFFFFFFF;

#[allow(dead_code)]
const SUCCESS_EMERALD: u32 = 0xFFEF4444; // Electric Red Nothing OS accent
#[allow(dead_code)]
const DANGER_CRIMSON: u32 = 0xFFEF4444;

// -----------------------------------------------------------------------------
// Canonical InterOS Typography
// -----------------------------------------------------------------------------
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

/// Rows a `scale`-high text run occupies on the shared bitmap face.
/// WHY a local `const fn`: this is exactly `font::line_height(scale)`, but that
/// helper is not `const`, and every run box in this file is derived from a
/// compile-time constant. Gating a run on anything shorter than this silently
/// slices the glyphs, so the width is computed once here and reused.
const fn run_rows(scale: usize) -> usize {
    scale * std::design::font::GLYPH_H
}

/// Header band height (y: 0..HEADER_H). Rows at or below it belong to the
/// display card, so a header run must be centred inside 0..HEADER_H.
const HEADER_H: usize = 110;
/// Top of the operation badge's scale-3 line box inside the display card.
#[allow(dead_code)]
const OP_BADGE_Y0: usize = 140;

/// Display card box (x: 24..696, y: 110..410).
const DISPLAY_X0: usize = 24;
const DISPLAY_X1: usize = 696;
/// Inset from the card's edge to the displayed value's right-hand column. WHY a
/// named inset: the value is right-aligned to it, so the number's right edge is
/// the same distance from the card rim as the card's left edge is.
const DISPLAY_PAD: usize = 24;
/// Top of the displayed value's line box. At scale 4 that box is 96 rows, so
/// 280..376 sits between the operation badge (which ends at 212) and the card's
/// bottom rim.
const VALUE_Y0: usize = 280;

// Button layout constants
const BTN_W: usize = 144;
const BTN_H: usize = 140;
const COL_X: [usize; 4] = [28, 194, 360, 526];
const ROW_Y: [usize; 5] = [440, 610, 780, 950, 1120];

fn get_button_action(row: usize, col: usize) -> Option<char> {
    match (row, col) {
        (0, 0) => Some('C'),
        (0, 1) => Some('B'),
        (0, 2) => Some('%'),
        (0, 3) => Some('/'),
        (1, 0) => Some('7'),
        (1, 1) => Some('8'),
        (1, 2) => Some('9'),
        (1, 3) => Some('*'),
        (2, 0) => Some('4'),
        (2, 1) => Some('5'),
        (2, 2) => Some('6'),
        (2, 3) => Some('-'),
        (3, 0) => Some('1'),
        (3, 1) => Some('2'),
        (3, 2) => Some('3'),
        (3, 3) => Some('+'),
        (4, 0) => Some('~'), // +/- toggle
        (4, 1) => Some('0'),
        (4, 2) => Some('^'),
        (4, 3) => Some('='),
        _ => None,
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

        // Canonical App Title: "Calculator" at display weight
        let title_rows = run_rows(3);
        let title_y0 = (HEADER_H - title_rows) / 2;
        if y >= title_y0 && y < title_y0 + title_rows {
            if let Some(c) = test_text(x, y, 116, title_y0, 3, b"Calculator", TEXT_PRIMARY) {
                return c;
            }
        }

        return BG_DARK;
    }

    // 2. Numerical Display Area Container Card (y: 110..410, x: 24..696)
    if (110..410).contains(&y) && x >= DISPLAY_X0 && x < DISPLAY_X1 {
        // Display card squircle corner test (radius: 24)
        let rx = (x - DISPLAY_X0).min(DISPLAY_X1 - 1 - x);
        let ry = (y - 110).min(409 - y);
        if rx < 24 && ry < 24 {
            let cdx = (24 - rx) as isize;
            let cdy = (24 - ry) as isize;
            if cdx * cdx + cdy * cdy > 576 {
                return BG_DARK;
            }
        }

        // Nothing OS subtle Electric Red signature accent dot at (52, 142)
        let dot_dx = x as i32 - 52;
        let dot_dy = y as i32 - 142;
        if dot_dx * dot_dx + dot_dy * dot_dy <= 16 {
            return 0xFFEF4444;
        }

        // Formula Tape (Line 1: Accumulator & active operation if set)
        if let Some(op) = calc.active_operation {
            let mut tape = [0u8; 32];
            let mut tlen = 0usize;
            let acc_val = calc.accumulator;
            let is_neg_acc = acc_val < 0;
            let mut uacc = acc_val.unsigned_abs();
            if is_neg_acc {
                tape[0] = b'-';
                tlen = 1;
            }
            if uacc == 0 {
                tape[tlen] = b'0';
                tlen += 1;
            } else {
                let mut rev_acc = [0u8; 20];
                let mut r_acc_count = 0usize;
                while uacc > 0 && r_acc_count < 18 {
                    rev_acc[r_acc_count] = b'0' + (uacc % 10) as u8;
                    uacc /= 10;
                    r_acc_count += 1;
                }
                for j in 0..r_acc_count {
                    tape[tlen + j] = rev_acc[r_acc_count - 1 - j];
                }
                tlen += r_acc_count;
            }
            tape[tlen] = b' ';
            tlen += 1;
            tape[tlen] = op as u8;
            tlen += 1;

            let tape_w = std::design::font::measure_bold(&tape[..tlen], 2);
            let tape_x = (DISPLAY_X1 - DISPLAY_PAD).saturating_sub(tape_w);
            if let Some(c) = std::design::font::test_text_aa(x, y, tape_x, 190, 2, &tape[..tlen], TEXT_MUTED, SURFACE_DARK, false) {
                return c;
            }
        }

        // Active operation badge at top-left
        if let Some(op) = calc.active_operation {
            let op_sym: &[u8] = match op {
                '/' => b"/",
                '*' => b"*",
                '-' => b"-",
                '+' => b"+",
                '%' => b"%",
                '^' => b"^",
                _ => b"?",
            };
            // Badge pill: x: 72..124, y: 128..156 (radius 12)
            if (128..156).contains(&y) && (72..124).contains(&x) {
                let prx = (x - 72).min(123 - x);
                let pry = (y - 128).min(155 - y);
                if prx < 12 && pry < 12 {
                    let cdx = (12 - prx) as isize;
                    let cdy = (12 - pry) as isize;
                    if cdx * cdx + cdy * cdy <= 144 {
                        if prx < 2 || pry < 2 {
                            return BORDER_SUBTLE;
                        }
                        if let Some(c) = std::design::font::test_text_aa(x, y, 92, 130, 1, op_sym, TEXT_PRIMARY, SURFACE_RAISED, true) {
                            return c;
                        }
                        return SURFACE_RAISED;
                    }
                } else {
                    if prx < 2 || pry < 2 {
                        return BORDER_SUBTLE;
                    }
                    if let Some(c) = std::design::font::test_text_aa(x, y, 92, 130, 1, op_sym, TEXT_PRIMARY, SURFACE_RAISED, true) {
                        return c;
                    }
                    return SURFACE_RAISED;
                }
            }
        }

        if calc.error {
            let err_text = b"Error";
            let err_w = std::design::font::measure_bold(err_text, 4);
            let x0 = (DISPLAY_X1 - DISPLAY_PAD).saturating_sub(err_w);
            if let Some(c) = std::design::font::test_text_aa(x, y, x0, VALUE_Y0, 4, err_text, 0xFFEF4444, SURFACE_DARK, true) {
                return c;
            }
        } else {
            // Format displayed value
            let val = calc.displayed_value();
            let is_neg = val < 0;
            let mut uval = val.unsigned_abs();
            let mut digits = [0u8; 20];
            let mut count = 0usize;
            if uval == 0 {
                digits[0] = b'0';
                count = 1;
            } else {
                let mut rev = [0u8; 20];
                let mut rcount = 0usize;
                while uval > 0 && rcount < 18 {
                    rev[rcount] = b'0' + (uval % 10) as u8;
                    uval /= 10;
                    rcount += 1;
                }
                if is_neg {
                    digits[0] = b'-';
                    count = 1;
                }
                for j in 0..rcount {
                    digits[count + j] = rev[rcount - 1 - j];
                }
                count += rcount;
            }

            let scale: usize = if count <= 7 { 4 } else if count <= 10 { 3 } else { 2 };
            let total_w: usize = std::design::font::measure_bold(&digits[..count], scale);
            let x0: usize = (DISPLAY_X1 - DISPLAY_PAD).saturating_sub(total_w);
            let y0: usize = VALUE_Y0;

            if let Some(c) = std::design::font::test_text_aa(x, y, x0, y0, scale, &digits[..count], TEXT_PRIMARY, SURFACE_DARK, true) {
                return c;
            }
        }

        // Subtle card border
        if rx < 2 || ry < 2 {
            return BORDER_SUBTLE;
        }

        // Clean dark card background
        return SURFACE_DARK;
    }

    // 3. Keypad Matrix (5 rows, 4 cols)
    for row in 0..5 {
        let by = ROW_Y[row];
        if y >= by && y < by + BTN_H {
            for col in 0..4 {
                let bx = COL_X[col];
                if x >= bx && x < bx + BTN_W {
                    // Tactile keycap squircle inset (4px padding for clean visual breathing room)
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
                    let is_fn = matches!(action, Some('B') | Some('%') | Some('~') | Some('^') | Some('S'));

                    // Minimal Bauhaus / Nothing OS Keycap Palette
                    let (btn_bg, btn_border, text_color) = if is_active_op {
                        // Active operator glows in Electric Red
                        (0xFFEF4444, 0xFFDC2626, 0xFFFFFFFF)
                    } else if is_op {
                        // Arithmetic operators: Stark Milk White with deep dark text
                        (0xFFFFFFFF, 0xFFE2E8F0, 0xFF000000)
                    } else if is_equals {
                        // Equals: Electric Red signature Nothing OS accent with pure white text
                        (0xFFEF4444, 0xFFDC2626, 0xFFFFFFFF)
                    } else if is_clear {
                        // Clear: Slate surface with semantic Electric Red accent text
                        (SURFACE_RAISED, BORDER_SUBTLE, 0xFFEF4444)
                    } else if is_fn {
                        // Functions: Slate surface with clean light text
                        (SURFACE_RAISED, BORDER_SUBTLE, 0xFFE6EDF3)
                    } else {
                        // Digits 0..9: Slate surface with crisp white text
                        (SURFACE_RAISED, BORDER_SUBTLE, TEXT_PRIMARY)
                    };

                    // Tactile 2px border outline
                    if rx < 2 || ry < 2 {
                        return btn_border;
                    }

                    // Button symbol at center
                    let center_x = bx + BTN_W / 2;
                    let center_y = by + BTN_H / 2;

                    if let Some(act) = action {
                        if act == '~' {
                            let sym = b"+/-";
                            let w = std::design::font::measure_bold(sym, 1);
                            let x0 = center_x.saturating_sub(w / 2);
                            let y0 = center_y.saturating_sub(12);
                            if let Some(c) = std::design::font::test_text_aa(x, y, x0, y0, 1, sym, text_color, btn_bg, true) {
                                return c;
                            }
                        } else if act == '^' || act == 'S' {
                            let sym = b"x^2";
                            let w = std::design::font::measure_bold(sym, 1);
                            let x0 = center_x.saturating_sub(w / 2);
                            let y0 = center_y.saturating_sub(12);
                            if let Some(c) = std::design::font::test_text_aa(x, y, x0, y0, 1, sym, text_color, btn_bg, true) {
                                return c;
                            }
                        } else {
                            let act_byte = [act as u8];
                            let w = std::design::font::measure_bold(&act_byte, 2);
                            let x0 = center_x.saturating_sub(w / 2);
                            let y0 = center_y.saturating_sub(24);
                            if let Some(c) = std::design::font::test_text_aa(x, y, x0, y0, 2, &act_byte, text_color, btn_bg, true) {
                                return c;
                            }
                        }
                    }

                    return btn_bg;
                }
            }
        }
    }

    // 4. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn render_calculator_ui(surface: u32, calc: &CalculatorState) {
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

// -----------------------------------------------------------------------------
// Screen band - the single source of truth for the `Dirty` -> band mapping
// used by the input paths below.
// -----------------------------------------------------------------------------

/// Rows the display card owns (y: 110..410, x: 24..696).
///
/// WHY the card and not just the two runs inside it: the operation badge and
/// the displayed value are the only pixels on this surface that are a function
/// of state. The header, the keypad and the home gesture pill are drawn from
/// compile-time constants, so no input can move a pixel outside this band.
const DISPLAY_Y0: usize = 110;
const DISPLAY_Y1: usize = 410;

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a whole band with no stride
/// bookkeeping. Every band starts on a row boundary, so the running `x`/`y`
/// cursor below is exact.
///
/// WHY this is correct rather than a shortcut: the band is named after what the
/// input actually moved (see `Dirty`), and every row outside it is correct
/// because nothing about it changed. The old renderer re-rasterised all
/// 1,036,800 pixels on every event, including taps that hit no key at all.
fn render_calculator_band(surface: u32, calc: &CalculatorState, y0: usize, y1: usize) {
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
// Input handling
// -----------------------------------------------------------------------------

/// Which rows of the surface an input actually changed.
///
/// WHY this exists: the old loop called `render_calculator_ui` after every
/// touch and every keypress, so a single digit re-rasterised all 1,036,800
/// pixels through `get_calculator_pixel` - the same cost as a full screen
/// transition, for a change confined to one card.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing moved; the surface is already correct, so do not render at all.
    None,
    /// A digit, operator, evaluate, clear, sign, backspace, percent or square
    /// moved the entry or the active-operation badge: the display card only.
    Display,
    /// More than one band moved, or the first frame: the whole surface.
    All,
}

/// Grow a dirty region so one rendered frame covers everything a poll batch
/// changed.
///
/// WHY: `event_poll` can deliver several inputs before the app is scheduled
/// again; rendering only the last region would leave the earlier one stale.
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
    /// Nothing on screen moved.
    None,
    /// The model moved; render the named region.
    Redraw(Dirty),
    /// Leave to the launcher, logging this exact reason first.
    Leave(&'static str),
}

/// Maps a `Dirty` onto the bands it re-rasterises.
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
        48..=57 => calc.push_digit((code - 48) as i64),
        43 => calc.set_operation('+'),
        45 => calc.set_operation('-'),
        42 => calc.set_operation('*'),
        47 => calc.set_operation('/'),
        61 | input_key_code::ENTER => { calc.evaluate(); }
        99 | input_key_code::ESCAPE => calc.clear(),
        input_key_code::BACKSPACE => {
            calc.current_operand /= 10;
            calc.result_mode = false;
        }
        37 => {
            calc.set_operation('%');
        }
        94 | 115 | 83 => {
            calc.square();
        }
        95 | 126 => calc.toggle_sign(),
        _ => return false,
    }
    true
}

/// Resolves one touch against the keys that are actually drawn.
///
/// Every branch below mutates state that is drawn inside the display card (see
/// `Dirty::Display`), so the returned region is the same for all of them; a
/// touch that lands on no key at all changes nothing and reports `Tap::None`,
/// which is what stops an inert tap from costing a full repaint.
fn handle_touch_point(calc: &mut CalculatorState, x: u32, y: u32) -> Tap {
    // Back button: canonical back button bounds (24..72, 24..72) or legacy circular disc (62, 55, r: 24)
    let is_back = std::design::back_button::contains(x as usize, y as usize) || {
        let dx = x as i32 - 62;
        let dy = y as i32 - 55;
        dx * dx + dy * dy <= 24 * 24
    };
    if is_back {
        return Tap::Leave("[CALCULATOR] Back button tapped. Returning to launcher.");
    }

    // Home gesture bar exits to the launcher.
    if x >= 240 && x <= 480 && y >= 1400 {
        return Tap::Leave("[CALCULATOR] Home gesture tapped. Returning to launcher.");
    }

    for row in 0..5 {
        let by = ROW_Y[row] as u32;
        if y >= by && y < by + (BTN_H as u32) {
            for col in 0..4 {
                let bx = COL_X[col] as u32;
                if x >= bx && x < bx + (BTN_W as u32) {
                    if let Some(action) = get_button_action(row, col) {
                        // WHY the messages are logged here rather than returned:
                        // the keypad arms are the only thing between the press
                        // and the log line, and carrying each string back out
                        // would make the caller reassemble a decision it cannot
                        // see. The wording is unchanged.
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
                            '^' | 'S' => {
                                calc.square();
                                std::log(0, "[CALCULATOR] Square evaluated.").ok();
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
                            'B' => {
                                calc.current_operand /= 10;
                                calc.result_mode = false;
                                std::log(0, "[CALCULATOR] Backspace applied.").ok();
                            }
                            '%' => {
                                calc.set_operation('%');
                                std::log(0, "[CALCULATOR] Modulo operation queued.").ok();
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
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
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
    let _ = std::surface_flip(6);
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
                        // WHY the display band: every key this handler accepts
                        // moves the entry or the active-operation badge, and
                        // nothing else on the surface is a function of state.
                        dirty = dirty.widen(Dirty::Display);
                    }
                    continue;
                }
                if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                    std::log(
                        0,
                        "[CALCULATOR] Navigation gesture recognized. Returning to launcher.",
                    )
                    .ok();
                    let _ = std::surface_destroy(6, surface);
                    std::exit_with_code(0);
                }
                // WHY: kind 1 is Tap and kind 2 is DoubleTap. The kernel emits
                // exactly one event per physical release (router.rs classifies
                // a second press within 300 ticks and TAP_SLOP of the last tap
                // as DoubleTap), so both kinds mean "a key was pressed here".
                // Handling only Tap would silently swallow fast repeats of the
                // same key (e.g. typing 55 in under 300 ms), while firing on
                // both keeps press-consistent semantics: two presses, two
                // actions.
                if input.kind == 1 || input.kind == 2 {
                    match handle_touch_point(&mut calc, input.x, input.y) {
                        Tap::Leave(msg) => {
                            std::log(0, msg).ok();
                            let _ = std::surface_destroy(6, surface);
                            std::exit_with_code(0);
                        }
                        Tap::Redraw(d) => dirty = dirty.widen(d),
                        Tap::None => {}
                    }
                }
            }
            // WHY once per batch: the drain above can carry several inputs
            // before this task is scheduled again, and each one only names the
            // rows it moved, so the merged region is repainted and presented
            // exactly once instead of once per event.
            if dirty != Dirty::None {
                repaint(surface, &calc, dirty);
                let _ = std::surface_flip(6);
                // WHY this marker exists: everything logged above happens before
                // the redraw, so nothing in this file could show that the frame
                // carrying the new state was actually handed to the display path.
                // It sits inside the `dirty != Dirty::None` branch because that
                // branch is the only place a repaint runs: an inert tap leaves
                // `dirty` at None and emits nothing, so this line cannot be
                // produced by a tap that changed nothing, and the app has no
                // periodic redraw that could emit it either.
                //
                // WHAT IT PROVES: `surface_flip` has returned, which means the
                // surfaces were committed, composited, the front/back buffers
                // swapped and the composited image written into the guest boot
                // framebuffer. It does NOT prove that a display controller
                // fetched those pixels or that anything was emitted as light.
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
