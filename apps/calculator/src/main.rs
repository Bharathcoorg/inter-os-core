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
// Embedded 8x8 ASCII Font (Characters 32 ' ' through 126 '~')
// -----------------------------------------------------------------------------
const FONT_8X8: [[u8; 8]; 95] = [
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // 32 ' '
    [0x18, 0x18, 0x18, 0x18, 0x00, 0x00, 0x18, 0x00], // 33 '!'
    [0x66, 0x66, 0x24, 0x00, 0x00, 0x00, 0x00, 0x00], // 34 '"'
    [0x6c, 0x6c, 0xfe, 0x6c, 0xfe, 0x6c, 0x6c, 0x00], // 35 '#'
    [0x18, 0x3e, 0x60, 0x3c, 0x06, 0x7c, 0x18, 0x00], // 36 '$'
    [0x00, 0xc6, 0xcc, 0x18, 0x30, 0x66, 0xc6, 0x00], // 37 '%'
    [0x38, 0x6c, 0x38, 0x76, 0xdc, 0xcc, 0x76, 0x00], // 38 '&'
    [0x18, 0x18, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00], // 39 '''
    [0x0c, 0x18, 0x30, 0x30, 0x30, 0x18, 0x0c, 0x00], // 40 '('
    [0x30, 0x18, 0x0c, 0x0c, 0x0c, 0x18, 0x30, 0x00], // 41 ')'
    [0x00, 0x66, 0x3c, 0xff, 0x3c, 0x66, 0x00, 0x00], // 42 '*'
    [0x00, 0x18, 0x18, 0x7e, 0x18, 0x18, 0x00, 0x00], // 43 '+'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x30], // 44 ','
    [0x00, 0x00, 0x00, 0x7e, 0x00, 0x00, 0x00, 0x00], // 45 '-'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00], // 46 '.'
    [0x06, 0x0c, 0x18, 0x30, 0x60, 0xc0, 0x80, 0x00], // 47 '/'
    [0x7c, 0xc6, 0xce, 0xd6, 0xe6, 0xc6, 0x7c, 0x00], // 48 '0'
    [0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x7e, 0x00], // 49 '1'
    [0x7c, 0xc6, 0x06, 0x1c, 0x30, 0x66, 0xfe, 0x00], // 50 '2'
    [0x7c, 0xc6, 0x06, 0x3c, 0x06, 0xc6, 0x7c, 0x00], // 51 '3'
    [0x1c, 0x3c, 0x6c, 0xcc, 0xfe, 0x0c, 0x1e, 0x00], // 52 '4'
    [0xfe, 0xc0, 0xfc, 0x06, 0x06, 0xc6, 0x7c, 0x00], // 53 '5'
    [0x78, 0x0c, 0xc0, 0xfc, 0xc6, 0xc6, 0x7c, 0x00], // 54 '6'
    [0xfe, 0xc6, 0x0c, 0x18, 0x30, 0x30, 0x30, 0x00], // 55 '7'
    [0x7c, 0xc6, 0xc6, 0x7c, 0xc6, 0xc6, 0x7c, 0x00], // 56 '8'
    [0x7c, 0xc6, 0xc6, 0x7e, 0x06, 0x0c, 0x78, 0x00], // 57 '9'
    [0x00, 0x18, 0x18, 0x00, 0x18, 0x18, 0x00, 0x00], // 58 ':'
    [0x00, 0x18, 0x18, 0x00, 0x18, 0x18, 0x30, 0x00], // 59 ';'
    [0x06, 0x0c, 0x18, 0x30, 0x18, 0x0c, 0x06, 0x00], // 60 '<'
    [0x00, 0x7e, 0x00, 0x7e, 0x00, 0x00, 0x00, 0x00], // 61 '='
    [0x60, 0x30, 0x18, 0x0c, 0x18, 0x30, 0x60, 0x00], // 62 '>'
    [0x7c, 0xc6, 0x0c, 0x18, 0x18, 0x00, 0x18, 0x00], // 63 '?'
    [0x7c, 0xc6, 0xde, 0xde, 0xdc, 0xc0, 0x7c, 0x00], // 64 '@'
    [0x38, 0x6c, 0xc6, 0xfe, 0xc6, 0xc6, 0xc6, 0x00], // 65 'A'
    [0xfc, 0x66, 0x66, 0x7c, 0x66, 0x66, 0xfc, 0x00], // 66 'B'
    [0x3c, 0x66, 0xc0, 0xc0, 0xc0, 0x66, 0x3c, 0x00], // 67 'C'
    [0xf8, 0x6c, 0x66, 0x66, 0x66, 0x6c, 0xf8, 0x00], // 68 'D'
    [0xfe, 0x62, 0x68, 0x78, 0x68, 0x62, 0xfe, 0x00], // 69 'E'
    [0xfe, 0x62, 0x68, 0x78, 0x68, 0x60, 0xf0, 0x00], // 70 'F'
    [0x3c, 0x66, 0xc0, 0xc0, 0xce, 0x66, 0x3e, 0x00], // 71 'G'
    [0xc6, 0xc6, 0xc6, 0xfe, 0xc6, 0xc6, 0xc6, 0x00], // 72 'H'
    [0x7e, 0x18, 0x18, 0x18, 0x18, 0x18, 0x7e, 0x00], // 73 'I'
    [0x1e, 0x0c, 0x0c, 0x0c, 0xcc, 0xcc, 0x78, 0x00], // 74 'J'
    [0xe6, 0x66, 0x6c, 0x78, 0x6c, 0x66, 0xe6, 0x00], // 75 'K'
    [0xf0, 0x60, 0x60, 0x60, 0x62, 0x66, 0xfe, 0x00], // 76 'L'
    [0xc6, 0xee, 0xfe, 0xfe, 0xd6, 0xc6, 0xc6, 0x00], // 77 'M'
    [0xc6, 0xe6, 0xf6, 0xde, 0xce, 0xc6, 0xc6, 0x00], // 78 'N'
    [0x7c, 0xc6, 0xc6, 0xc6, 0xc6, 0xc6, 0x7c, 0x00], // 79 'O'
    [0xfc, 0x66, 0x66, 0x7c, 0x60, 0x60, 0xf0, 0x00], // 80 'P'
    [0x7c, 0xc6, 0xc6, 0xc6, 0xd6, 0xcc, 0x76, 0x00], // 81 'Q'
    [0xfc, 0x66, 0x66, 0x7c, 0x6c, 0x66, 0xe6, 0x00], // 82 'R'
    [0x7c, 0xc6, 0x60, 0x38, 0x0c, 0xc6, 0x7c, 0x00], // 83 'S'
    [0x7e, 0x7e, 0x5a, 0x18, 0x18, 0x18, 0x3c, 0x00], // 84 'T'
    [0xc6, 0xc6, 0xc6, 0xc6, 0xc6, 0xc6, 0x7c, 0x00], // 85 'U'
    [0xc6, 0xc6, 0xc6, 0xc6, 0x6c, 0x38, 0x10, 0x00], // 86 'V'
    [0xc6, 0xc6, 0xd6, 0xfe, 0xfe, 0xee, 0xc6, 0x00], // 87 'W'
    [0xc6, 0x6c, 0x38, 0x38, 0x6c, 0xc6, 0xc6, 0x00], // 88 'X'
    [0x66, 0x66, 0x66, 0x3c, 0x18, 0x18, 0x3c, 0x00], // 89 'Y'
    [0xfe, 0xc6, 0x0c, 0x18, 0x30, 0x62, 0xfe, 0x00], // 90 'Z'
    [0x3c, 0x30, 0x30, 0x30, 0x30, 0x30, 0x3c, 0x00], // 91 '['
    [0xc0, 0x60, 0x30, 0x18, 0x0c, 0x06, 0x02, 0x00], // 92 '\'
    [0x3c, 0x0c, 0x0c, 0x0c, 0x0c, 0x0c, 0x3c, 0x00], // 93 ']'
    [0x10, 0x38, 0x6c, 0xc6, 0x00, 0x00, 0x00, 0x00], // 94 '^'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff], // 95 '_'
    [0x30, 0x18, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00], // 96 '`'
    [0x00, 0x00, 0x7c, 0x06, 0x7e, 0xc6, 0x7e, 0x00], // 97 'a'
    [0xe0, 0x60, 0x7c, 0x66, 0x66, 0x66, 0xdc, 0x00], // 98 'b'
    [0x00, 0x00, 0x7c, 0xc6, 0xc0, 0xc6, 0x7c, 0x00], // 99 'c'
    [0x1c, 0x0c, 0x7c, 0xcc, 0xcc, 0xcc, 0x76, 0x00], // 100 'd'
    [0x00, 0x00, 0x7c, 0xc6, 0xfe, 0xc0, 0x7c, 0x00], // 101 'e'
    [0x38, 0x6c, 0x60, 0xf0, 0x60, 0x60, 0xf0, 0x00], // 102 'f'
    [0x00, 0x00, 0x76, 0xcc, 0xcc, 0x7c, 0x0c, 0xf8], // 103 'g'
    [0xe0, 0x60, 0x6c, 0x76, 0x66, 0x66, 0xe6, 0x00], // 104 'h'
    [0x18, 0x00, 0x38, 0x18, 0x18, 0x18, 0x3c, 0x00], // 105 'i'
    [0x06, 0x00, 0x0e, 0x06, 0x06, 0x66, 0x66, 0x3c], // 106 'j'
    [0xe0, 0x60, 0x66, 0x6c, 0x78, 0x6c, 0xe6, 0x00], // 107 'k'
    [0x38, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3c, 0x00], // 108 'l'
    [0x00, 0x00, 0xe6, 0xff, 0xdb, 0xc9, 0xc9, 0x00], // 109 'm'
    [0x00, 0x00, 0xdc, 0x66, 0x66, 0x66, 0x66, 0x00], // 110 'n'
    [0x00, 0x00, 0x7c, 0xc6, 0xc6, 0xc6, 0x7c, 0x00], // 111 'o'
    [0x00, 0x00, 0xdc, 0x66, 0x66, 0x7c, 0x60, 0xf0], // 112 'p'
    [0x00, 0x00, 0x76, 0xcc, 0xcc, 0x7c, 0x0c, 0x1e], // 113 'q'
    [0x00, 0x00, 0xdc, 0x66, 0x60, 0x60, 0xf0, 0x00], // 114 'r'
    [0x00, 0x00, 0x7c, 0xc0, 0x7c, 0x06, 0xfc, 0x00], // 115 's'
    [0x30, 0x30, 0xfc, 0x30, 0x30, 0x34, 0x18, 0x00], // 116 't'
    [0x00, 0x00, 0xcc, 0xcc, 0xcc, 0xcc, 0x76, 0x00], // 117 'u'
    [0x00, 0x00, 0xc6, 0xc6, 0xc6, 0x6c, 0x38, 0x00], // 118 'v'
    [0x00, 0x00, 0xc6, 0xd6, 0xfe, 0xfe, 0x6c, 0x00], // 119 'w'
    [0x00, 0x00, 0xc6, 0x6c, 0x38, 0x6c, 0xc6, 0x00], // 120 'x'
    [0x00, 0x00, 0xc6, 0xc6, 0xc6, 0x7e, 0x06, 0xfc], // 121 'y'
    [0x00, 0x00, 0x7e, 0x4c, 0x18, 0x32, 0x7e, 0x00], // 122 'z'
    [0x0e, 0x18, 0x18, 0x70, 0x18, 0x18, 0x0e, 0x00], // 123 '{'
    [0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00], // 124 '|'
    [0x70, 0x18, 0x18, 0x0e, 0x18, 0x18, 0x70, 0x00], // 125 '}'
    [0x76, 0xdc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // 126 '~'
];

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
    let char_w = 8 * scale;
    let char_h = 8 * scale;
    let total_w = text.len() * char_w;
    if y >= y0 && y < y0 + char_h && x >= x0 && x < x0 + total_w {
        let char_idx = (x - x0) / char_w;
        let c = text[char_idx];
        if (32..=126).contains(&c) {
            let glyph_idx = (c - 32) as usize;
            let col = ((x - x0) % char_w) / scale;
            let row = (y - y0) / scale;
            let row_byte = FONT_8X8[glyph_idx][row];
            if ((row_byte >> (7 - col)) & 1) == 1 {
                return Some(color);
            }
        }
    }
    None
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

        let scale: usize = if count <= 7 { 6 } else if count <= 10 { 5 } else { 4 };
        let char_w: usize = 8 * scale;
        let total_w: usize = count * char_w;
        let x0: usize = 660usize.saturating_sub(total_w);
        let y0: usize = 280;

        if let Some(c) = test_text(x, y, x0, y0, scale, &digits[..count], TEXT_PRIMARY) {
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
                            if let Some(c) = test_text(x, y, center_x - 16, center_y - 8, 2, b"+/-", text_color) {
                                return c;
                            }
                        } else {
                            let act_byte = [action as u8];
                            if let Some(c) = test_text(x, y, center_x - 12, center_y - 12, 3, &act_byte, text_color) {
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
        if let Ok(events) = std::event_poll(std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK, 100) {
            if (events & std::EVENT_TIMER_TICK) != 0 {
                let _ = std::surface_flip(6);
            }

            if (events & std::EVENT_TOUCH_INPUT) != 0 {
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
                    if input.kind == 1 {
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
        }

        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[PANIC] Calculator app error!").ok();
    std::exit_with_code(1);
}
