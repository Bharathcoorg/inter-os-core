//! InterOS Core Open-Source Reference App: Touch Calculator
//!
//! Demonstrates:
//! - Arithmetic execution engine
//! - Event loop / touch coordinates simulation
//! - State persistence across cooperative task yields

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

#[derive(Debug, Clone, Copy)]
pub struct CalculatorState {
    pub accumulator: i64,
    pub current_operand: i64,
    pub active_operation: Option<char>,
}

impl CalculatorState {
    pub const fn new() -> Self {
        Self {
            accumulator: 0,
            current_operand: 0,
            active_operation: None,
        }
    }

    pub fn push_digit(&mut self, digit: i64) {
        self.current_operand = self.current_operand * 10 + digit;
    }

    pub fn set_operation(&mut self, op: char) {
        self.accumulator = self.current_operand;
        self.current_operand = 0;
        self.active_operation = Some(op);
    }

    pub fn evaluate(&mut self) -> i64 {
        if let Some(op) = self.active_operation {
            match op {
                '+' => self.accumulator += self.current_operand,
                '-' => self.accumulator -= self.current_operand,
                '*' => self.accumulator *= self.current_operand,
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
        self.current_operand = 0;
        self.active_operation = None;
        self.accumulator
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[CALCULATOR] Starting InterOS Calculator App...").ok();

    let mut calc = CalculatorState::new();
    
    // Simulate user pressing 42 + 58 = 100
    calc.push_digit(4);
    calc.push_digit(2);
    calc.set_operation('+');
    calc.push_digit(5);
    calc.push_digit(8);
    let result = calc.evaluate();
    assert_eq!(result, 100);

    std::yield_now();
    std::log(0, "[CALCULATOR] Arithmetic execution verified (42 + 58 = 100).").ok();
    std::exit();
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[PANIC] Calculator app error!").ok();
    std::exit();
}
