#![no_std]
//! InterOS Calculator state machine and mathematical evaluation engine.

extern crate interos_std;

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
                '%' => {
                    if self.current_operand != 0 {
                        self.accumulator %= self.current_operand;
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
    }

    pub fn toggle_sign(&mut self) {
        self.current_operand = -self.current_operand;
    }

    pub fn displayed_value(&self) -> i64 {
        self.current_operand
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
}
