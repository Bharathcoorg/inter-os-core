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

// 7-segment bitmask table for digits 0..=9, minus '-', and 'E'
fn get_7segment_mask(val: i64) -> u8 {
    match val {
        0 => 0b0011_1111,
        1 => 0b0000_0110,
        2 => 0b0101_1011,
        3 => 0b0100_1111,
        4 => 0b0110_0110,
        5 => 0b0110_1101,
        6 => 0b0111_1101,
        7 => 0b0000_0111,
        8 => 0b0111_1111,
        9 => 0b0110_1111,
        _ => 0b0100_0000, // '-'
    }
}

fn is_segment_pixel(mask: u8, dx: usize, dy: usize) -> bool {
    if dx >= 36 || dy >= 64 {
        return false;
    }
    // Segment a: top horizontal
    if (mask & 0b0000_0001) != 0 && dy < 6 && dx >= 4 && dx < 32 {
        return true;
    }
    // Segment b: top-right vertical
    if (mask & 0b0000_0010) != 0 && dy >= 4 && dy < 30 && dx >= 30 {
        return true;
    }
    // Segment c: bottom-right vertical
    if (mask & 0b0000_0100) != 0 && dy >= 34 && dy < 60 && dx >= 30 {
        return true;
    }
    // Segment d: bottom horizontal
    if (mask & 0b0000_1000) != 0 && dy >= 58 && dy < 64 && dx >= 4 && dx < 32 {
        return true;
    }
    // Segment e: bottom-left vertical
    if (mask & 0b0001_0000) != 0 && dy >= 34 && dy < 60 && dx < 6 {
        return true;
    }
    // Segment f: top-left vertical
    if (mask & 0b0010_0000) != 0 && dy >= 4 && dy < 30 && dx < 6 {
        return true;
    }
    // Segment g: middle horizontal
    if (mask & 0b0100_0000) != 0 && dy >= 29 && dy < 35 && dx >= 4 && dx < 32 {
        return true;
    }
    false
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

fn render_calculator_ui(surface: u32, calc: &CalculatorState) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;

    // Format display digits
    let mut val = calc.displayed_value();
    let is_neg = val < 0;
    val = val.abs();

    let mut digits = [0u8; 12];
    let mut digit_count = 0usize;
    if val == 0 {
        digits[0] = 0;
        digit_count = 1;
    } else {
        while val > 0 && digit_count < 10 {
            digits[digit_count] = (val % 10) as u8;
            val /= 10;
            digit_count += 1;
        }
    }
    if is_neg && digit_count < 11 {
        digits[digit_count] = 255; // minus indicator
        digit_count += 1;
    }

    // Right-align display digits
    let digit_w = 36usize;
    let digit_gap = 12usize;
    let total_digits_w = digit_count * digit_w + (digit_count.saturating_sub(1)) * digit_gap;
    let display_start_x = (W - 54).saturating_sub(total_digits_w);
    let display_y_start = 240usize;

    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;

        for i in 0..count {
            if y < 96 {
                // Header / Status bar
                if y >= 94 {
                    chunk[i] = std::design::color::ACCENT;
                } else if x >= 24 && x < 100 && y >= 28 && y < 68 {
                    // Back / Exit button
                    chunk[i] = std::design::color::SURFACE_RAISED;
                } else {
                    chunk[i] = std::design::color::SURFACE;
                }
            } else if (110..390).contains(&y) && x >= 24 && x < W - 24 {
                // Display Card
                if y >= 388 {
                    chunk[i] = std::design::color::ACCENT;
                } else {
                    // Check if inside active 7-segment digit
                    let mut is_digit_pixel = false;
                    if (display_y_start..display_y_start + 64).contains(&y) && x >= display_start_x && x < display_start_x + total_digits_w {
                        let rel_x = x - display_start_x;
                        let digit_slot = rel_x / (digit_w + digit_gap);
                        let in_digit_x = rel_x % (digit_w + digit_gap);
                        if digit_slot < digit_count && in_digit_x < digit_w {
                            let d_val = digits[digit_count - 1 - digit_slot];
                            let mask = if d_val == 255 { get_7segment_mask(-1) } else { get_7segment_mask(d_val as i64) };
                            if is_segment_pixel(mask, in_digit_x, y - display_y_start) {
                                is_digit_pixel = true;
                            }
                        }
                    }
                    if is_digit_pixel {
                        chunk[i] = std::design::color::TEXT;
                    } else {
                        chunk[i] = std::design::color::SURFACE_RAISED;
                    }
                }
            } else {
                // Keypad grid hit detection for rendering
                let mut hit_btn = false;
                for row in 0..5 {
                    let by = ROW_Y[row];
                    if y >= by && y < by + BTN_H {
                        for col in 0..4 {
                            let bx = COL_X[col];
                            if x >= bx && x < bx + BTN_W {
                                hit_btn = true;
                                let center_x = bx + BTN_W / 2;
                                let center_y = by + BTN_H / 2;

                                // Determine button base color
                                let btn_color = match (row, col) {
                                    (0, 0) => std::design::color::DANGER,
                                    (4, 3) => std::design::color::SUCCESS,
                                    (_, 3) => std::design::color::ACCENT,
                                    (0, _) => std::design::color::SURFACE,
                                    _ => std::design::color::SURFACE_RAISED,
                                };

                                // Render button glyph icon / symbol at center
                                let mut is_glyph = false;
                                let action = get_button_action(row, col).unwrap_or(' ');
                                if ('0'..='9').contains(&action) {
                                    let d = (action as u8 - b'0') as i64;
                                    let mask = get_7segment_mask(d);
                                    let gx_start = center_x.saturating_sub(18);
                                    let gy_start = center_y.saturating_sub(32);
                                    if x >= gx_start && x < gx_start + 36 && y >= gy_start && y < gy_start + 64 {
                                        is_glyph = is_segment_pixel(mask, x - gx_start, y - gy_start);
                                    }
                                } else if action == '+' {
                                    // Cross +
                                    if (x >= center_x - 16 && x < center_x + 16 && y >= center_y - 3 && y < center_y + 3)
                                        || (x >= center_x - 3 && x < center_x + 3 && y >= center_y - 16 && y < center_y + 16) {
                                        is_glyph = true;
                                    }
                                } else if action == '-' {
                                    // Minus -
                                    if x >= center_x - 16 && x < center_x + 16 && y >= center_y - 3 && y < center_y + 3 {
                                        is_glyph = true;
                                    }
                                } else if action == '=' {
                                    // Equals =
                                    if (x >= center_x - 16 && x < center_x + 16 && y >= center_y - 8 && y < center_y - 2)
                                        || (x >= center_x - 16 && x < center_x + 16 && y >= center_y + 2 && y < center_y + 8) {
                                        is_glyph = true;
                                    }
                                } else if action == 'C' {
                                    // Clear C (arc)
                                    let gx_start = center_x.saturating_sub(18);
                                    let gy_start = center_y.saturating_sub(32);
                                    if x >= gx_start && x < gx_start + 36 && y >= gy_start && y < gy_start + 64 {
                                        let mask = 0b0011_1001; // a, d, e, f
                                        is_glyph = is_segment_pixel(mask, x - gx_start, y - gy_start);
                                    }
                                } else if action == '/' {
                                    // Slash /
                                    let rel_x = x as i32 - center_x as i32;
                                    let rel_y = y as i32 - center_y as i32;
                                    if (-16..=16).contains(&rel_y) && (rel_x + rel_y).abs() <= 2 {
                                        is_glyph = true;
                                    }
                                } else if action == '*' {
                                    // Star *
                                    let rel_x = x as i32 - center_x as i32;
                                    let rel_y = y as i32 - center_y as i32;
                                    if (rel_x.abs() <= 12 && rel_y.abs() <= 2)
                                        || (rel_x.abs() <= 2 && rel_y.abs() <= 12)
                                        || ((rel_x + rel_y).abs() <= 2 && rel_x.abs() <= 10)
                                        || ((rel_x - rel_y).abs() <= 2 && rel_x.abs() <= 10) {
                                        is_glyph = true;
                                    }
                                }

                                if is_glyph {
                                    chunk[i] = std::design::color::TEXT;
                                } else {
                                    chunk[i] = btn_color;
                                }
                                break;
                            }
                        }
                    }
                    if hit_btn {
                        break;
                    }
                }

                if !hit_btn {
                    chunk[i] = std::design::color::BACKGROUND;
                }
            }

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

fn handle_touch_point(calc: &mut CalculatorState, x: u32, y: u32) -> bool {
    // Check if exit / back button pressed
    if x >= 24 && x < 100 && y >= 28 && y < 68 {
        std::log(0, "[CALCULATOR] Exit button tapped. Returning to launcher.").ok();
        return true;
    }

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
                            '=' => {
                                let res = calc.evaluate();
                                std::log(0, "[CALCULATOR] Evaluation completed.").ok();
                                let _ = res;
                            }
                            'C' => {
                                calc.clear();
                                std::log(0, "[CALCULATOR] Clear pressed.").ok();
                            }
                            '~' => {
                                calc.toggle_sign();
                            }
                            _ => {}
                        }
                    }
                    return false;
                }
            }
        }
    }
    false
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[CALCULATOR (Ring 3)] Open-Source Touch Calculator online.").ok();

    let mut calc = CalculatorState::new();

    // 1. Allocate dedicated KMS Client Surface (z=200)
    std::log(0, "[CALCULATOR] Step 1: Allocating application surface (z=200)...").ok();
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(s) => {
            std::log(0, "[CALCULATOR] Step 2: Surface created successfully.").ok();
            s
        }
        Err(_) => {
            std::log(0, "[CALCULATOR] Display surface unavailable; refusing headless demo execution.").ok();
            std::exit_with_code(1);
        }
    };

    // 2. Start with an empty expression; calculations are driven by user input.
    calc.clear();

    // 3. Render initial Calculator UI frame and present
    render_calculator_ui(surface, &calc);
    let _ = std::surface_flip(6);
    std::log(0, "[CALCULATOR] Initial frame presented to hardware scanout.").ok();


    // 4. Interactive Event Reactor Loop
    loop {
        if let Ok(events) = std::event_poll(
            std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK,
            100,
        ) {
            if (events & std::EVENT_TIMER_TICK) != 0 {
                let _ = std::surface_flip(6);
            }

            if (events & std::EVENT_TOUCH_INPUT) != 0 {
                let mut input = std::InputEvent {
                    kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000,
                };
                while std::input_event_poll(&mut input).is_ok() {
                    if input.kind == 1 {
                        if handle_touch_point(&mut calc, input.x, input.y) {
                            let _ = std::surface_destroy(6, surface);
                            std::exit_with_code(0);
                        }
                        render_calculator_ui(surface, &calc);
                        let _ = std::surface_flip(6);
                    }
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
