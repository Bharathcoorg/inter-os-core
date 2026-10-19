#![no_std]
//! InterOS Calculator state machine and mathematical evaluation engine.

extern crate interos_std;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalculatorState {
    pub accumulator: i64,
    pub current_operand: i64,
    pub active_operation: Option<char>,
    pub result_mode: bool,
    pub error: bool,
}

impl CalculatorState {
    pub const fn new() -> Self {
        Self {
            accumulator: 0,
            current_operand: 0,
            active_operation: None,
            result_mode: false,
            error: false,
        }
    }

    pub fn push_digit(&mut self, digit: i64) {
        if self.error {
            self.clear();
        }
        if self.result_mode {
            self.current_operand = 0;
            self.result_mode = false;
        }
        if self.current_operand.abs() < 100_000_000 {
            self.current_operand = self.current_operand * 10 + digit;
        }
    }

    pub fn set_operation(&mut self, op: char) {
        if self.error {
            self.clear();
        }
        if self.active_operation.is_some() && !self.result_mode {
            self.evaluate();
        } else {
            self.accumulator = self.current_operand;
        }
        self.current_operand = 0;
        self.active_operation = Some(op);
        self.result_mode = false;
    }

    pub fn square(&mut self) {
        if self.error {
            return;
        }
        if let Some(sq) = self.current_operand.checked_mul(self.current_operand) {
            self.current_operand = sq;
            self.result_mode = true;
        } else {
            self.error = true;
            self.active_operation = None;
            self.result_mode = true;
        }
    }

    pub fn evaluate(&mut self) -> i64 {
        if self.error {
            return 0;
        }
        if let Some(op) = self.active_operation {
            match op {
                '+' => self.accumulator = self.accumulator.saturating_add(self.current_operand),
                '-' => self.accumulator = self.accumulator.saturating_sub(self.current_operand),
                '*' => self.accumulator = self.accumulator.saturating_mul(self.current_operand),
                '/' => {
                    if let Some(res) = self.accumulator.checked_div(self.current_operand) {
                        self.accumulator = res;
                    } else {
                        self.error = true;
                        self.active_operation = None;
                        self.result_mode = true;
                        return 0;
                    }
                }
                '%' => {
                    if let Some(res) = self.accumulator.checked_rem(self.current_operand) {
                        self.accumulator = res;
                    } else {
                        self.error = true;
                        self.active_operation = None;
                        self.result_mode = true;
                        return 0;
                    }
                }
                '^' => {
                    if self.current_operand >= 0 {
                        let exp = (self.current_operand as u64).min(63) as u32;
                        self.accumulator = self.accumulator.saturating_pow(exp);
                    } else {
                        // Integer division for negative exponents: 1 / (acc ^ |op|)
                        self.accumulator = match self.accumulator {
                            0 => 0,
                            1 => 1,
                            -1 => if self.current_operand % 2 != 0 { -1 } else { 1 },
                            _ => 0,
                        };
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
        self.error = false;
    }

    pub fn toggle_sign(&mut self) {
        self.current_operand = -self.current_operand;
    }

    pub fn displayed_value(&self) -> i64 {
        self.current_operand
    }
}

/// 64-bit decimal fixed-point number with 4 decimal fractional places (scaling factor 10,000).
/// Bounded, zero-heap decimal math providing exact banking/financial calculation without floating-point inaccuracies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FixedDecimal {
    pub raw: i64,
}

impl FixedDecimal {
    pub const SCALE: i64 = 10_000;
    pub const ZERO: Self = Self { raw: 0 };
    pub const ONE: Self = Self { raw: Self::SCALE };

    pub const fn from_raw(raw: i64) -> Self {
        Self { raw }
    }

    pub const fn from_int(val: i64) -> Self {
        Self { raw: val.saturating_mul(Self::SCALE) }
    }

    pub const fn from_parts(whole: i64, frac_10000: i64) -> Self {
        let sign = if whole < 0 { -1 } else { 1 };
        Self {
            raw: whole.saturating_mul(Self::SCALE).saturating_add(sign * frac_10000.abs()),
        }
    }

    pub const fn whole_part(&self) -> i64 {
        self.raw / Self::SCALE
    }

    pub const fn frac_part(&self) -> i64 {
        (self.raw % Self::SCALE).abs()
    }

    pub const fn saturating_add(self, other: Self) -> Self {
        Self { raw: self.raw.saturating_add(other.raw) }
    }

    pub const fn saturating_sub(self, other: Self) -> Self {
        Self { raw: self.raw.saturating_sub(other.raw) }
    }

    pub fn saturating_mul(self, other: Self) -> Self {
        let prod = (self.raw as i128) * (other.raw as i128);
        let scaled = prod / (Self::SCALE as i128);
        let clamped = scaled.clamp(i64::MIN as i128, i64::MAX as i128) as i64;
        Self { raw: clamped }
    }

    pub fn saturating_div(self, other: Self) -> Option<Self> {
        if other.raw == 0 {
            return None;
        }
        let numer = (self.raw as i128) * (Self::SCALE as i128);
        let quot = numer / (other.raw as i128);
        let clamped = quot.clamp(i64::MIN as i128, i64::MAX as i128) as i64;
        Some(Self { raw: clamped })
    }

    pub fn format_ascii(&self, out: &mut [u8]) -> usize {
        if out.is_empty() {
            return 0;
        }
        let is_neg = self.raw < 0;
        let whole = self.whole_part().abs();
        let frac = self.frac_part();

        let mut pos = 0;
        if is_neg && pos < out.len() {
            out[pos] = b'-';
            pos += 1;
        }

        let mut whole_buf = [0u8; 20];
        let mut wn = 0;
        let mut w = whole;
        if w == 0 {
            whole_buf[0] = b'0';
            wn = 1;
        } else {
            while w > 0 && wn < 20 {
                whole_buf[wn] = b'0' + (w % 10) as u8;
                w /= 10;
                wn += 1;
            }
        }
        for i in 0..wn {
            if pos < out.len() {
                out[pos] = whole_buf[wn - 1 - i];
                pos += 1;
            }
        }

        if pos < out.len() {
            out[pos] = b'.';
            pos += 1;
        }
        let d1 = ((frac / 1000) % 10) as u8;
        let d2 = ((frac / 100) % 10) as u8;
        let d3 = ((frac / 10) % 10) as u8;
        let d4 = (frac % 10) as u8;

        for &d in &[d1, d2, d3, d4] {
            if pos < out.len() {
                out[pos] = b'0' + d;
                pos += 1;
            }
        }
        pos
    }

    pub fn parse_ascii(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }
        let is_neg = bytes[0] == b'-';
        let start = if is_neg || bytes[0] == b'+' { 1 } else { 0 };
        if start >= bytes.len() {
            return None;
        }

        let mut whole: i64 = 0;
        let mut idx = start;
        while idx < bytes.len() && bytes[idx] != b'.' {
            let b = bytes[idx];
            if !b.is_ascii_digit() {
                return None;
            }
            whole = whole.checked_mul(10)?.checked_add((b - b'0') as i64)?;
            idx += 1;
        }

        let mut frac: i64 = 0;
        if idx < bytes.len() && bytes[idx] == b'.' {
            idx += 1;
            let mut weight = 1000i64;
            while idx < bytes.len() {
                let b = bytes[idx];
                if !b.is_ascii_digit() {
                    return None;
                }
                if weight > 0 {
                    frac += ((b - b'0') as i64) * weight;
                    weight /= 10;
                }
                idx += 1;
            }
        }

        let raw = whole.checked_mul(Self::SCALE)?.checked_add(frac)?;
        Some(Self {
            raw: if is_neg { -raw } else { raw },
        })
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculator_basic_arithmetic() {
        let mut calc = CalculatorState::new();
        calc.push_digit(1);
        calc.push_digit(2);
        calc.set_operation('+');
        calc.push_digit(8);
        assert_eq!(calc.evaluate(), 20);

        calc.set_operation('*');
        calc.push_digit(3);
        assert_eq!(calc.evaluate(), 60);

        calc.set_operation('-');
        calc.push_digit(15);
        assert_eq!(calc.evaluate(), 45);

        calc.set_operation('/');
        calc.push_digit(9);
        assert_eq!(calc.evaluate(), 5);

        calc.set_operation('%');
        calc.push_digit(3);
        assert_eq!(calc.evaluate(), 2);

        calc.set_operation('/');
        calc.push_digit(0);
        assert_eq!(calc.evaluate(), 0);
        assert!(calc.error);
    }

    #[test]
    fn test_calculator_exponentiation() {
        let mut calc = CalculatorState::new();
        calc.push_digit(2);
        calc.set_operation('^');
        calc.push_digit(10);
        assert_eq!(calc.evaluate(), 1024);

        // Negative exponent on integers >= 2 yields 0
        calc.set_operation('^');
        calc.push_digit(2);
        calc.toggle_sign(); // -2
        assert_eq!(calc.evaluate(), 0);

        // 1 ^ any = 1
        calc.clear();
        calc.push_digit(1);
        calc.set_operation('^');
        calc.push_digit(5);
        calc.toggle_sign(); // -5
        assert_eq!(calc.evaluate(), 1);

        // -1 ^ odd = -1, -1 ^ even = 1
        calc.clear();
        calc.push_digit(1);
        calc.toggle_sign(); // -1
        calc.set_operation('^');
        calc.push_digit(3);
        calc.toggle_sign(); // -3
        assert_eq!(calc.evaluate(), -1);
    }

    #[test]
    fn test_calculator_square() {
        let mut calc = CalculatorState::new();
        calc.push_digit(5);
        calc.square();
        assert_eq!(calc.displayed_value(), 25);
        assert!(!calc.error);

        // Chained square
        calc.square();
        assert_eq!(calc.displayed_value(), 625);

        // Negative operand squared yields positive
        calc.clear();
        calc.push_digit(7);
        calc.toggle_sign();
        assert_eq!(calc.displayed_value(), -7);
        calc.square();
        assert_eq!(calc.displayed_value(), 49);

        // Overflow on square triggers error
        calc.clear();
        calc.current_operand = 4_000_000_000;
        calc.square();
        assert!(calc.error);
    }

    #[test]
    fn test_calculator_division_and_modulo_by_zero() {
        let mut calc = CalculatorState::new();
        calc.push_digit(4);
        calc.push_digit(2);
        calc.set_operation('/');
        calc.push_digit(0);
        assert_eq!(calc.evaluate(), 0);
        assert!(calc.error);

        calc.clear();
        calc.push_digit(4);
        calc.push_digit(2);
        calc.set_operation('%');
        calc.push_digit(0);
        assert_eq!(calc.evaluate(), 0);
        assert!(calc.error);
    }

    #[test]
    fn test_calculator_overflow_resilience() {
        let mut calc = CalculatorState::new();
        calc.current_operand = i64::MIN;
        calc.set_operation('/');
        calc.push_digit(1);
        calc.toggle_sign(); // -1
        assert_eq!(calc.evaluate(), 0);
        assert!(calc.error);

        calc.clear();
        calc.current_operand = i64::MIN;
        calc.set_operation('%');
        calc.push_digit(1);
        calc.toggle_sign(); // -1
        assert_eq!(calc.evaluate(), 0);
        assert!(calc.error);
    }

    #[test]
    fn test_fixed_decimal_arithmetic_and_formatting() {
        let a = FixedDecimal::from_parts(12, 3450); // 12.3450
        let b = FixedDecimal::from_parts(3, 1550);  // 3.1550
        let sum = a.saturating_add(b);
        assert_eq!(sum.whole_part(), 15);
        assert_eq!(sum.frac_part(), 5000);

        let diff = a.saturating_sub(b);
        assert_eq!(diff.whole_part(), 9);
        assert_eq!(diff.frac_part(), 1900);

        let mul = FixedDecimal::from_int(2).saturating_mul(FixedDecimal::from_parts(1, 5000));
        assert_eq!(mul.whole_part(), 3);
        assert_eq!(mul.frac_part(), 0);

        let div = FixedDecimal::from_int(10).saturating_div(FixedDecimal::from_int(4)).unwrap();
        assert_eq!(div.whole_part(), 2);
        assert_eq!(div.frac_part(), 5000);

        // ASCII formatting test
        let mut buf = [0u8; 16];
        let len = sum.format_ascii(&mut buf);
        assert_eq!(&buf[..len], b"15.5000");

        // ASCII parsing test
        let parsed = FixedDecimal::parse_ascii(b"15.5000").expect("parse");
        assert_eq!(parsed, sum);

        let parsed_neg = FixedDecimal::parse_ascii(b"-42.0125").expect("parse neg");
        assert_eq!(parsed_neg.whole_part(), -42);
        assert_eq!(parsed_neg.frac_part(), 125);
    }
}

