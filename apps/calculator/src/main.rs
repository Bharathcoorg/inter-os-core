//! InterOS Core Open-Source Reference App: Interactive Touch Calculator
//!
//! Full production Ring-3 graphical application executing in User Mode:
//! - DRM/KMS Client Surface allocation (z=200) and double-buffered scanout presentation
//! - 7-segment digital display rendering for high-contrast mathematical evaluation
//! - 5x4 rounded mobile keypad matrix with semantic design tokens
//! - Interactive touch event processing and button hit-testing
//! - Full arithmetic execution engine: Add, Subtract, Multiply, Divide, Clear, Evaluate
//! - Graceful lifecycle supervision and clean return to Launcher

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalculatorState {
    pub accumulator: i64,
    pub current_operand: i64,
    pub active_operation: Option<char>,
    pub result_mode: bool,
}

impl CalculatorState {
    pub const fn new() -> Self {
        Self {
            accumulator: 0,
            current_operand: 0,
            active_operation: None,
            result_mode: false,
        }
    }

    pub fn push_digit(&mut self, digit: i64) {
        if self.result_mode {
            self.current_operand = 0;
            self.result_mode = false;
        }
        if self.current_operand.abs() < 100_000_000 {
            self.current_operand = self.current_operand * 10 + digit;
        }
    }

    pub fn set_operation(&mut self, op: char) {
        if self.active_operation.is_some() && !self.result_mode {
            self.evaluate();
        } else {
            self.accumulator = self.current_operand;
        }
        self.current_operand = 0;
        self.active_operation = Some(op);
        self.result_mode = false;
    }

    pub fn evaluate(&mut self) -> i64 {
        if let Some(op) = self.active_operation {
            match op {
                '+' => self.accumulator = self.accumulator.saturating_add(self.current_operand),
                '-' => self.accumulator = self.accumulator.saturating_sub(self.current_operand),
                '*' => self.accumulator = self.accumulator.saturating_mul(self.current_operand),
                '/' => {
                    if self.current_operand != 0 {
                        self.accumulator /= self.current_operand;
                    }
                }
                _ => {}
            }
        } else {
            self.accumulator = self.current_operand;
        }
        self.current_operand = self.accumulator;
        self.active_operation = None;
        self.result_mode = true;
        self.accumulator
    }

    pub fn clear(&mut self) {
        self.accumulator = 0;
        self.current_operand = 0;
        self.active_operation = None;
        self.result_mode = false;
    }

    pub fn toggle_sign(&mut self) {
        self.current_operand = -self.current_operand;
    }

    pub fn displayed_value(&self) -> i64 {
        self.current_operand
    }
}

// -----------------------------------------------------------------------------
// 32-bit ARGB8888 Design Color Tokens (Screen 16 Minimal Light Theme)
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFFF7F6F2;
#[allow(dead_code)]
const SURFACE_DARK: u32 = 0xFFFFFFFF;
#[allow(dead_code)]
const SURFACE_RAISED: u32 = 0xFFEFEEE9;
#[allow(dead_code)]
const BORDER_SUBTLE: u32 = 0xFFE1E0DB;
const TEXT_PRIMARY: u32 = 0xFF111214;
const TEXT_MUTED: u32 = 0xFF777773;
const ACCENT_SKY: u32 = 0xFF2563EB;
#[allow(dead_code)]
const SUCCESS_EMERALD: u32 = 0xFF10B981;
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

// Button layout constants
const BTN_W: usize = 144;
const BTN_H: usize = 140;
const COL_X: [usize; 4] = [28, 194, 360, 526];
const ROW_Y: [usize; 5] = [440, 610, 780, 950, 1120];

fn get_button_action(row: usize, col: usize) -> Option<char> {
    match (row, col) {
        (0, 0) => Some('C'),
        (0, 1) => Some('('),
        (0, 2) => Some(')'),
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
        (4, 2) => Some('.'),
        (4, 3) => Some('='),
        _ => None,
    }
}

// -----------------------------------------------------------------------------
// Pixel Color Resolution Engine
// -----------------------------------------------------------------------------
#[inline(never)]
fn get_calculator_pixel(x: usize, y: usize, calc: &CalculatorState) -> u32 {
    // 1. Status Bar & Header Title (y: 0..110)
    if y < 110 {
        // Clock "09:20" at x: 48, y: 20
        if y >= 20 && y < 36 {
            if let Some(c) = test_text(x, y, 48, 20, 2, b"09:20", TEXT_PRIMARY) {
                return c;
            }
        }

        // Wi-Fi / Battery indicator at top right
        if y >= 20 && y < 36 {
            if let Some(c) = test_text(x, y, 620, 20, 2, b"100%", TEXT_MUTED) {
                return c;
            }
        }

        // Canonical App Title: "Calculator" (bold display font at x: 48, y: 64)
        if y >= 64 && y < 96 {
            if let Some(c) = test_text(x, y, 48, 64, 3, b"Calculator", TEXT_PRIMARY) {
                return c;
            }
        }

        return BG_DARK;
    }

    // 2. Numerical Display Area (y: 110..410, x: 24..696)
    if (110..410).contains(&y) && x >= 24 && x < 696 {
        // Active operation badge at top-left
        if let Some(op) = calc.active_operation {
            let op_sym: &[u8] = match op {
                '/' => b"/",
                '*' => b"*",
                '-' => b"-",
                '+' => b"+",
                _ => b"?",
            };
            if y >= 140 && y < 164 {
                if let Some(c) = test_text(x, y, 48, 140, 3, op_sym, ACCENT_SKY) {
                    return c;
                }
            }
        }

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
        let x0: usize = 660usize.saturating_sub(total_w);
        let y0: usize = 280;

        if let Some(c) = std::design::font::test_text_aa(x, y, x0, y0, scale, &digits[..count], TEXT_PRIMARY, BG_DARK, true) {
            return c;
        }

        return BG_DARK;
    }

    // 3. Keypad Matrix (5 rows, 4 cols)
    for row in 0..5 {
        let by = ROW_Y[row];
        if y >= by && y < by + BTN_H {
            for col in 0..4 {
                let bx = COL_X[col];
                if x >= bx && x < bx + BTN_W {
                    // Rounded squircle corner test (radius: 28)
                    let rx = (x - bx).min(bx + BTN_W - 1 - x);
                    let ry = (y - by).min(by + BTN_H - 1 - y);
                    if rx < 28 && ry < 28 {
                        let cdx = (28 - rx) as isize;
                        let cdy = (28 - ry) as isize;
                        if cdx * cdx + cdy * cdy > 784 {
                            return BG_DARK;
                        }
                    }

                    // Canonical InterOS Calculator button styling:
                    // Equals is solid dark (#111214) with white text (#FFFFFF);
                    // all other keys use soft neutral surface (#EFEEE9) with subtle border (#E1E0DB).
                    let is_equals = row == 4 && col == 3;
                    let (btn_bg, btn_border, text_color) = if is_equals {
                        (0xFF111214, 0xFF111214, 0xFFFFFFFF)
                    } else {
                        (0xFFEFEEE9, 0xFFE1E0DB, 0xFF111214)
                    };

                    // Subtle border rim for light buttons
                    if !is_equals && (rx < 1 || ry < 1) {
                        return btn_border;
                    }

                    // Button symbol at center
                    let center_x = bx + BTN_W / 2;
                    let center_y = by + BTN_H / 2;

                    if let Some(action) = get_button_action(row, col) {
                        if action == '~' {
                            let sym = b"+/-";
                            let w = std::design::font::measure_bold(sym, 1);
                            let x0 = center_x.saturating_sub(w / 2);
                            let y0 = center_y.saturating_sub(12);
                            if let Some(c) = std::design::font::test_text_aa(x, y, x0, y0, 1, sym, text_color, btn_bg, true) {
                                return c;
                            }
                        } else {
                            let act_byte = [action as u8];
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

fn handle_touch_point(calc: &mut CalculatorState, x: u32, y: u32) -> (bool, Option<&'static str>) {
    // Check if exit / back button or home gesture bar pressed
    if (x >= 24 && x < 100 && y >= 28 && y < 68) || (x >= 240 && x <= 480 && y >= 1400) {
        std::log(
            0,
            "[CALCULATOR] Exit button or home gesture tapped. Returning to launcher.",
        )
        .ok();
        return (true, None);
    }

    for row in 0..5 {
        let by = ROW_Y[row] as u32;
        if y >= by && y < by + (BTN_H as u32) {
            for col in 0..4 {
                let bx = COL_X[col] as u32;
                if x >= bx && x < bx + (BTN_W as u32) {
                    if let Some(action) = get_button_action(row, col) {
                        let msg = match action {
                            '0'..='9' => {
                                let digit = (action as u8 - b'0') as i64;
                                calc.push_digit(digit);
                                Some("[CALCULATOR] Keypad digit pressed.")
                            }
                            '+' | '-' | '*' | '/' => {
                                calc.set_operation(action);
                                Some("[CALCULATOR] Keypad operator selected.")
                            }
                            '=' => {
                                let res = calc.evaluate();
                                let _ = res;
                                Some("[CALCULATOR] Evaluation completed.")
                            }
                            'C' => {
                                calc.clear();
                                Some("[CALCULATOR] Clear pressed.")
                            }
                            '~' => {
                                calc.toggle_sign();
                                None
                            }
                            _ => None,
                        };
                        return (false, msg);
                    }
                    return (false, None);
                }
            }
        }
    }
    (false, None)
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
    render_calculator_ui(surface, &calc);
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
            while std::input_event_poll(&mut input).is_ok() {
                if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                    std::log(
                        0,
                        "[CALCULATOR] Navigation gesture recognized. Returning to launcher.",
                    )
                    .ok();
                    let _ = std::surface_destroy(6, surface);
                    std::exit_with_code(0);
                }
                if input.kind == 1 || input.kind == 2 {
                    let (should_exit, action_logged) =
                        handle_touch_point(&mut calc, input.x, input.y);
                    if let Some(msg) = action_logged {
                        std::log(0, msg).ok();
                    }
                    if should_exit {
                        let _ = std::surface_destroy(6, surface);
                        std::exit_with_code(0);
                    }
                    render_calculator_ui(surface, &calc);
                    let _ = std::surface_flip(6);
                }
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
