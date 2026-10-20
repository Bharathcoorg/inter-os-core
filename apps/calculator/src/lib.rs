#![no_std]
//! InterOS Calculator state machine and mathematical evaluation engine.

extern crate interos_std;

/// Error types encountered during mathematical evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalcError {
    DivisionByZero,
    Overflow,
    InvalidSyntax,
    UnbalancedParentheses,
    EmptyExpression,
}

impl CalcError {
    pub const fn as_str(&self) -> &'static str {
        match self {
            CalcError::DivisionByZero => "Error: Div by zero",
            CalcError::Overflow => "Error: Overflow",
            CalcError::InvalidSyntax => "Error: Syntax error",
            CalcError::UnbalancedParentheses => "Error: Unbalanced ()",
            CalcError::EmptyExpression => "Error: Empty expr",
        }
    }
}

/// 64-bit decimal fixed-point number with 4 decimal fractional places (scaling factor 10,000).
/// Bounded, zero-heap decimal math providing exact banking/financial calculation without floating-point inaccuracies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Decimal {
    pub raw: i64,
}

pub type FixedDecimal = Decimal;

impl Decimal {
    pub const SCALE: i64 = 10_000;
    pub const ZERO: Self = Self { raw: 0 };
    pub const ONE: Self = Self { raw: Self::SCALE };

    pub const fn from_raw(raw: i64) -> Self {
        Self { raw }
    }

    pub const fn from_i64(val: i64) -> Option<Self> {
        match val.checked_mul(Self::SCALE) {
            Some(raw) => Some(Self { raw }),
            None => None,
        }
    }

    pub const fn from_int(val: i64) -> Self {
        Self {
            raw: val.saturating_mul(Self::SCALE),
        }
    }

    pub const fn from_parts(whole: i64, frac_10000: i64) -> Option<Self> {
        let sign = if whole < 0 { -1 } else { 1 };
        let whole_scaled = match whole.checked_mul(Self::SCALE) {
            Some(v) => v,
            None => return None,
        };
        let frac_val = sign * frac_10000.abs();
        match whole_scaled.checked_add(frac_val) {
            Some(raw) => Some(Self { raw }),
            None => None,
        }
    }

    pub const fn whole_part(&self) -> i64 {
        self.raw / Self::SCALE
    }

    pub const fn frac_part(&self) -> i64 {
        (self.raw % Self::SCALE).abs()
    }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.raw.checked_add(other.raw).map(Self::from_raw)
    }

    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.raw.checked_sub(other.raw).map(Self::from_raw)
    }

    pub fn checked_mul(self, other: Self) -> Option<Self> {
        let prod = (self.raw as i128).checked_mul(other.raw as i128)?;
        let scaled = prod / (Self::SCALE as i128);
        if scaled < (i64::MIN as i128) || scaled > (i64::MAX as i128) {
            None
        } else {
            Some(Self { raw: scaled as i64 })
        }
    }

    pub fn checked_div(self, other: Self) -> Option<Self> {
        if other.raw == 0 {
            return None;
        }
        let numer = (self.raw as i128).checked_mul(Self::SCALE as i128)?;
        let quot = numer / (other.raw as i128);
        if quot < (i64::MIN as i128) || quot > (i64::MAX as i128) {
            None
        } else {
            Some(Self { raw: quot as i64 })
        }
    }

    pub fn abs(self) -> Option<Self> {
        self.raw.checked_abs().map(Self::from_raw)
    }

    pub fn neg(self) -> Option<Self> {
        self.raw.checked_neg().map(Self::from_raw)
    }

    pub fn reciprocal(self) -> Option<Self> {
        Self::ONE.checked_div(self)
    }

    pub fn percentage(self) -> Option<Self> {
        self.raw.checked_div(100).map(Self::from_raw)
    }

    pub fn isqrt(self) -> Option<Self> {
        if self.raw < 0 {
            return None;
        }
        if self.raw == 0 {
            return Some(Self::ZERO);
        }
        let target = (self.raw as u128).checked_mul(Self::SCALE as u128)?;
        let mut x0 = 1u128 << ((128 - target.leading_zeros() + 1) / 2);
        loop {
            let x1 = (x0 + target / x0) >> 1;
            if x1 >= x0 {
                break;
            }
            x0 = x1;
        }
        while x0.saturating_mul(x0) > target {
            x0 -= 1;
        }
        if x0 <= i64::MAX as u128 {
            Some(Self { raw: x0 as i64 })
        } else {
            None
        }
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

    pub fn format(&self, out: &mut [u8]) -> usize {
        if out.is_empty() {
            return 0;
        }
        if self.raw == 0 {
            out[0] = b'0';
            return 1;
        }

        let is_neg = self.raw < 0;
        let u = self.raw.unsigned_abs();
        let whole = u / (Self::SCALE as u64);
        let frac = (u % (Self::SCALE as u64)) as u16;

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

        if frac > 0 {
            if pos < out.len() {
                out[pos] = b'.';
                pos += 1;
            }
            let d1 = ((frac / 1000) % 10) as u8;
            let d2 = ((frac / 100) % 10) as u8;
            let d3 = ((frac / 10) % 10) as u8;
            let d4 = (frac % 10) as u8;
            let frac_digits = [d1, d2, d3, d4];
            let mut num_frac = 4;
            while num_frac > 0 && frac_digits[num_frac - 1] == 0 {
                num_frac -= 1;
            }
            for i in 0..num_frac {
                if pos < out.len() {
                    out[pos] = b'0' + frac_digits[i];
                    pos += 1;
                }
            }
        }
        pos
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
        let mut s = 0;
        while s < bytes.len() && (bytes[s] == b' ' || bytes[s] == b'\t') {
            s += 1;
        }
        let mut e = bytes.len();
        while e > s && (bytes[e - 1] == b' ' || bytes[e - 1] == b'\t') {
            e -= 1;
        }
        let b = &bytes[s..e];
        if b.is_empty() {
            return None;
        }

        let is_neg = b[0] == b'-';
        let start = if is_neg || b[0] == b'+' { 1 } else { 0 };
        if start >= b.len() {
            return None;
        }

        let mut whole: i64 = 0;
        let mut idx = start;
        let mut has_digits = false;
        while idx < b.len() && b[idx] != b'.' {
            let byte = b[idx];
            if !byte.is_ascii_digit() {
                return None;
            }
            has_digits = true;
            whole = whole.checked_mul(10)?.checked_add((byte - b'0') as i64)?;
            idx += 1;
        }

        let mut frac: i64 = 0;
        if idx < b.len() && b[idx] == b'.' {
            idx += 1;
            let mut weight = 1000i64;
            while idx < b.len() {
                let byte = b[idx];
                if !byte.is_ascii_digit() {
                    return None;
                }
                has_digits = true;
                if weight > 0 {
                    frac += ((byte - b'0') as i64) * weight;
                    weight /= 10;
                }
                idx += 1;
            }
        }

        if !has_digits {
            return None;
        }

        let whole_scaled = whole.checked_mul(Self::SCALE)?;
        let raw = whole_scaled.checked_add(frac)?;
        Some(Self {
            raw: if is_neg { -raw } else { raw },
        })
    }
}

pub const HISTORY_CAPACITY: usize = 8;
pub const MAX_EXPR_LEN: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub expr: [u8; MAX_EXPR_LEN],
    pub expr_len: usize,
    pub result: Decimal,
}

impl HistoryEntry {
    pub fn expr_str(&self) -> &str {
        core::str::from_utf8(&self.expr[..self.expr_len]).unwrap_or("")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CalculationHistory {
    entries: [HistoryEntry; HISTORY_CAPACITY],
    head: usize,
    count: usize,
}

impl CalculationHistory {
    pub const fn new() -> Self {
        const EMPTY_ENTRY: HistoryEntry = HistoryEntry {
            expr: [0u8; MAX_EXPR_LEN],
            expr_len: 0,
            result: Decimal::ZERO,
        };
        Self {
            entries: [EMPTY_ENTRY; HISTORY_CAPACITY],
            head: 0,
            count: 0,
        }
    }

    pub fn push(&mut self, expr: &str, result: Decimal) {
        let mut entry = HistoryEntry {
            expr: [0u8; MAX_EXPR_LEN],
            expr_len: 0,
            result,
        };
        let b = expr.as_bytes();
        let len = b.len().min(MAX_EXPR_LEN);
        entry.expr[..len].copy_from_slice(&b[..len]);
        entry.expr_len = len;

        self.entries[self.head] = entry;
        self.head = (self.head + 1) % HISTORY_CAPACITY;
        if self.count < HISTORY_CAPACITY {
            self.count += 1;
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.count = 0;
    }

    pub fn get(&self, index: usize) -> Option<&HistoryEntry> {
        if index >= self.count {
            return None;
        }
        let oldest_idx = (self.head + HISTORY_CAPACITY - self.count) % HISTORY_CAPACITY;
        let actual_idx = (oldest_idx + index) % HISTORY_CAPACITY;
        Some(&self.entries[actual_idx])
    }

    pub fn get_recent(&self, index: usize) -> Option<&HistoryEntry> {
        if index >= self.count {
            return None;
        }
        let newest_idx = (self.head + HISTORY_CAPACITY - 1) % HISTORY_CAPACITY;
        let actual_idx = (newest_idx + HISTORY_CAPACITY - (index % HISTORY_CAPACITY)) % HISTORY_CAPACITY;
        Some(&self.entries[actual_idx])
    }
}

// -----------------------------------------------------------------------------
// Bounded Expression Evaluator
// -----------------------------------------------------------------------------

pub fn evaluate(expr: &str) -> Result<Decimal, CalcError> {
    evaluate_ascii(expr.as_bytes())
}

pub fn evaluate_ascii(bytes: &[u8]) -> Result<Decimal, CalcError> {
    if bytes.is_empty() {
        return Err(CalcError::EmptyExpression);
    }
    let mut all_space = true;
    for &b in bytes {
        if b != b' ' && b != b'\t' && b != b'\r' && b != b'\n' {
            all_space = false;
            break;
        }
    }
    if all_space {
        return Err(CalcError::EmptyExpression);
    }

    let mut parser = Parser {
        bytes,
        pos: 0,
        paren_depth: 0,
    };
    let res = parser.parse_expr()?;
    parser.skip_whitespace();
    if parser.pos < parser.bytes.len() {
        if parser.bytes[parser.pos] == b')' {
            return Err(CalcError::UnbalancedParentheses);
        }
        return Err(CalcError::InvalidSyntax);
    }
    if parser.paren_depth != 0 {
        return Err(CalcError::UnbalancedParentheses);
    }
    Ok(res)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    paren_depth: usize,
}

impl<'a> Parser<'a> {
    fn skip_whitespace(&mut self) {
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b' ' || b == b'\t' || b == b'\r' || b == b'\n' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek_non_space(&self, from: usize) -> Option<u8> {
        let mut idx = from;
        while idx < self.bytes.len() {
            let b = self.bytes[idx];
            if b != b' ' && b != b'\t' && b != b'\r' && b != b'\n' {
                return Some(b);
            }
            idx += 1;
        }
        None
    }

    fn parse_expr(&mut self) -> Result<Decimal, CalcError> {
        self.parse_add()
    }

    fn parse_add(&mut self) -> Result<Decimal, CalcError> {
        let mut left = self.parse_mul()?;
        loop {
            self.skip_whitespace();
            if self.pos >= self.bytes.len() {
                break;
            }
            let b = self.bytes[self.pos];
            if b == b'+' {
                self.pos += 1;
                let right = self.parse_mul()?;
                left = left.checked_add(right).ok_or(CalcError::Overflow)?;
            } else if b == b'-' {
                self.pos += 1;
                let right = self.parse_mul()?;
                left = left.checked_sub(right).ok_or(CalcError::Overflow)?;
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn match_mul_op(&mut self) -> bool {
        self.skip_whitespace();
        if self.pos < self.bytes.len() {
            if self.bytes[self.pos] == b'*' {
                self.pos += 1;
                return true;
            }
            // UTF-8 multiplication sign '×' (U+00D7): 0xC3, 0x97
            if self.pos + 1 < self.bytes.len() && self.bytes[self.pos] == 0xC3 && self.bytes[self.pos + 1] == 0x97 {
                self.pos += 2;
                return true;
            }
        }
        false
    }

    fn match_div_op(&mut self) -> bool {
        self.skip_whitespace();
        if self.pos < self.bytes.len() {
            if self.bytes[self.pos] == b'/' {
                self.pos += 1;
                return true;
            }
            // UTF-8 division sign '÷' (U+00F7): 0xC3, 0xB7
            if self.pos + 1 < self.bytes.len() && self.bytes[self.pos] == 0xC3 && self.bytes[self.pos + 1] == 0xB7 {
                self.pos += 2;
                return true;
            }
        }
        false
    }

    fn parse_mul(&mut self) -> Result<Decimal, CalcError> {
        let mut left = self.parse_pow()?;
        loop {
            self.skip_whitespace();
            if self.pos >= self.bytes.len() {
                break;
            }
            if self.match_mul_op() {
                let right = self.parse_pow()?;
                left = left.checked_mul(right).ok_or(CalcError::Overflow)?;
            } else if self.match_div_op() {
                let right = self.parse_pow()?;
                if right.raw == 0 {
                    return Err(CalcError::DivisionByZero);
                }
                left = left.checked_div(right).ok_or(CalcError::Overflow)?;
            } else if self.bytes[self.pos] == b'%' {
                // Modulo check: only binary modulo if followed by operand
                let next_char = self.peek_non_space(self.pos + 1);
                let is_operand_next = match next_char {
                    Some(c) => c.is_ascii_digit() || c == b'.' || c == b'(' || c.is_ascii_alphabetic(),
                    None => false,
                };
                if is_operand_next {
                    self.pos += 1;
                    let right = self.parse_pow()?;
                    if right.raw == 0 {
                        return Err(CalcError::DivisionByZero);
                    }
                    let rem = left.raw % right.raw;
                    left = Decimal::from_raw(rem);
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_pow(&mut self) -> Result<Decimal, CalcError> {
        let mut left = self.parse_postfix()?;
        self.skip_whitespace();
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b'^' {
            self.pos += 1;
            let right = self.parse_pow()?;
            if right.frac_part() != 0 {
                return Err(CalcError::InvalidSyntax);
            }
            let exp = right.whole_part();
            left = self.eval_pow(left, exp)?;
        }
        Ok(left)
    }

    fn eval_pow(&self, base: Decimal, exp: i64) -> Result<Decimal, CalcError> {
        if exp == 0 {
            return Ok(Decimal::ONE);
        }
        if exp > 0 {
            let mut res = Decimal::ONE;
            let mut b = base;
            let mut e = exp;
            while e > 0 {
                if e % 2 == 1 {
                    res = res.checked_mul(b).ok_or(CalcError::Overflow)?;
                }
                e /= 2;
                if e > 0 {
                    b = b.checked_mul(b).ok_or(CalcError::Overflow)?;
                }
            }
            Ok(res)
        } else {
            let pos_exp = exp.checked_abs().ok_or(CalcError::Overflow)?;
            let denom = self.eval_pow(base, pos_exp)?;
            if denom.raw == 0 {
                return Err(CalcError::DivisionByZero);
            }
            Decimal::ONE.checked_div(denom).ok_or(CalcError::Overflow)
        }
    }

    fn parse_postfix(&mut self) -> Result<Decimal, CalcError> {
        let mut val = self.parse_primary()?;
        loop {
            self.skip_whitespace();
            if self.pos < self.bytes.len() && self.bytes[self.pos] == b'%' {
                let next_char = self.peek_non_space(self.pos + 1);
                let is_operand_next = match next_char {
                    Some(c) => c.is_ascii_digit() || c == b'.' || c == b'(' || c.is_ascii_alphabetic(),
                    None => false,
                };
                if is_operand_next {
                    break;
                } else {
                    self.pos += 1;
                    val = val.percentage().ok_or(CalcError::Overflow)?;
                }
            } else {
                break;
            }
        }
        Ok(val)
    }

    fn parse_primary(&mut self) -> Result<Decimal, CalcError> {
        self.skip_whitespace();
        if self.pos >= self.bytes.len() {
            return Err(CalcError::InvalidSyntax);
        }
        let b = self.bytes[self.pos];
        if b == b'+' {
            self.pos += 1;
            return self.parse_primary();
        }
        if b == b'-' {
            self.pos += 1;
            let val = self.parse_primary()?;
            return val.neg().ok_or(CalcError::Overflow);
        }
        if b == b'(' {
            self.pos += 1;
            self.paren_depth += 1;
            if self.paren_depth > 16 {
                return Err(CalcError::UnbalancedParentheses);
            }
            let val = self.parse_expr()?;
            self.skip_whitespace();
            if self.pos < self.bytes.len() && self.bytes[self.pos] == b')' {
                self.pos += 1;
                self.paren_depth -= 1;
                return Ok(val);
            } else {
                return Err(CalcError::UnbalancedParentheses);
            }
        }

        // Functions: sqrt(...) or reciprocal(...)
        if self.bytes[self.pos].is_ascii_alphabetic() {
            let start = self.pos;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_alphabetic() {
                self.pos += 1;
            }
            let ident = &self.bytes[start..self.pos];
            if ident == b"sqrt" || ident == b"isqrt" {
                self.skip_whitespace();
                if self.pos >= self.bytes.len() || self.bytes[self.pos] != b'(' {
                    return Err(CalcError::InvalidSyntax);
                }
                self.pos += 1;
                self.paren_depth += 1;
                if self.paren_depth > 16 {
                    return Err(CalcError::UnbalancedParentheses);
                }
                let arg = self.parse_expr()?;
                self.skip_whitespace();
                if self.pos < self.bytes.len() && self.bytes[self.pos] == b')' {
                    self.pos += 1;
                    self.paren_depth -= 1;
                    return arg.isqrt().ok_or(CalcError::InvalidSyntax);
                } else {
                    return Err(CalcError::UnbalancedParentheses);
                }
            } else if ident == b"reciprocal" || ident == b"recip" {
                self.skip_whitespace();
                if self.pos >= self.bytes.len() || self.bytes[self.pos] != b'(' {
                    return Err(CalcError::InvalidSyntax);
                }
                self.pos += 1;
                self.paren_depth += 1;
                if self.paren_depth > 16 {
                    return Err(CalcError::UnbalancedParentheses);
                }
                let arg = self.parse_expr()?;
                self.skip_whitespace();
                if self.pos < self.bytes.len() && self.bytes[self.pos] == b')' {
                    self.pos += 1;
                    self.paren_depth -= 1;
                    if arg.raw == 0 {
                        return Err(CalcError::DivisionByZero);
                    }
                    return arg.reciprocal().ok_or(CalcError::Overflow);
                } else {
                    return Err(CalcError::UnbalancedParentheses);
                }
            } else {
                return Err(CalcError::InvalidSyntax);
            }
        }

        // Numbers
        if b.is_ascii_digit() || b == b'.' {
            let start = self.pos;
            let mut has_dot = false;
            while self.pos < self.bytes.len() {
                let c = self.bytes[self.pos];
                if c == b'.' {
                    if has_dot {
                        break;
                    }
                    has_dot = true;
                    self.pos += 1;
                } else if c.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            let num_bytes = &self.bytes[start..self.pos];
            return Decimal::parse_ascii(num_bytes).ok_or(CalcError::InvalidSyntax);
        }

        Err(CalcError::InvalidSyntax)
    }
}

// -----------------------------------------------------------------------------
// Interactive Calculator State Machine
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalculatorState {
    pub accumulator: i64,
    pub current_operand: i64,
    pub active_operation: Option<char>,
    pub result_mode: bool,
    pub error: bool,
    pub calc_error: Option<CalcError>,
    pub expr_buf: [u8; 128],
    pub expr_len: usize,
    pub display_buf: [u8; 32],
    pub display_len: usize,
    pub history: CalculationHistory,
    pub show_history: bool,
}

impl CalculatorState {
    pub const fn new() -> Self {
        let mut display = [0u8; 32];
        display[0] = b'0';
        Self {
            accumulator: 0,
            current_operand: 0,
            active_operation: None,
            result_mode: false,
            error: false,
            calc_error: None,
            expr_buf: [0u8; 128],
            expr_len: 0,
            display_buf: display,
            display_len: 1,
            history: CalculationHistory::new(),
            show_history: false,
        }
    }

    pub fn expr_str(&self) -> &str {
        core::str::from_utf8(&self.expr_buf[..self.expr_len]).unwrap_or("")
    }

    pub fn display_str(&self) -> &str {
        core::str::from_utf8(&self.display_buf[..self.display_len]).unwrap_or("0")
    }

    pub fn push_char(&mut self, c: char) {
        if self.expr_len < self.expr_buf.len() {
            self.expr_buf[self.expr_len] = c as u8;
            self.expr_len += 1;
        }
    }

    pub fn push_str(&mut self, s: &str) {
        for b in s.as_bytes() {
            if self.expr_len < self.expr_buf.len() {
                self.expr_buf[self.expr_len] = *b;
                self.expr_len += 1;
            }
        }
    }

    pub fn push_digit(&mut self, digit: i64) {
        if self.error {
            self.clear();
        }
        if self.result_mode {
            self.expr_len = 0;
            self.current_operand = 0;
            self.result_mode = false;
        }

        // Update legacy operand
        if self.current_operand.abs() < 100_000_000 {
            self.current_operand = self.current_operand * 10 + digit;
        }

        // Append to expression
        let digit_char = (b'0' + (digit as u8 % 10)) as char;
        self.push_char(digit_char);

        // Update display buffer
        self.update_display_from_operand();
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

        // Append operator with spaces to expression if not trailing operator
        if self.expr_len > 0 && self.expr_buf[self.expr_len - 1] != b' ' {
            self.push_char(' ');
        }
        self.push_char(op);
        self.push_char(' ');
    }

    pub fn square(&mut self) {
        if self.error {
            return;
        }
        if let Some(sq) = self.current_operand.checked_mul(self.current_operand) {
            self.current_operand = sq;
            self.result_mode = true;
            self.update_display_from_operand();
        } else {
            self.error = true;
            self.calc_error = Some(CalcError::Overflow);
            self.active_operation = None;
            self.result_mode = true;
        }
    }

    pub fn push_parenthesis(&mut self) {
        if self.error {
            self.clear();
        }
        let mut open_count = 0usize;
        let mut close_count = 0usize;
        for &b in &self.expr_buf[..self.expr_len] {
            if b == b'(' {
                open_count += 1;
            } else if b == b')' {
                close_count += 1;
            }
        }

        let last_byte = if self.expr_len > 0 {
            Some(self.expr_buf[self.expr_len - 1])
        } else {
            None
        };

        if open_count > close_count && matches!(last_byte, Some(b'0'..=b'9' | b')' | b'%')) {
            self.push_char(')');
        } else {
            if matches!(last_byte, Some(b'0'..=b'9' | b')' | b'%')) {
                self.push_char(' ');
                self.push_char('*');
                self.push_char(' ');
            }
            self.push_char('(');
        }
    }

    pub fn backspace(&mut self) {
        if self.error {
            self.clear();
            return;
        }
        if self.expr_len > 0 {
            // If trailing space, remove trailing space too
            if self.expr_buf[self.expr_len - 1] == b' ' {
                self.expr_len -= 1;
            }
            if self.expr_len > 0 {
                self.expr_len -= 1;
            }
            if self.expr_len > 0 && self.expr_buf[self.expr_len - 1] == b' ' {
                self.expr_len -= 1;
            }
        }
        self.current_operand /= 10;
        self.result_mode = false;
        if self.expr_len == 0 {
            self.display_buf[0] = b'0';
            self.display_len = 1;
        } else {
            self.update_display_from_operand();
        }
    }

    pub fn evaluate_decimal(&mut self) -> Result<Decimal, CalcError> {
        let res = evaluate_ascii(&self.expr_buf[..self.expr_len]);
        match res {
            Ok(dec) => {
                self.display_len = dec.format(&mut self.display_buf);
                let expr_str = core::str::from_utf8(&self.expr_buf[..self.expr_len]).unwrap_or("");
                self.history.push(expr_str, dec);
                self.accumulator = dec.whole_part();
                self.current_operand = dec.whole_part();
                self.active_operation = None;
                self.result_mode = true;
                self.error = false;
                self.calc_error = None;
                Ok(dec)
            }
            Err(err) => {
                self.error = true;
                self.calc_error = Some(err);
                self.result_mode = true;
                Err(err)
            }
        }
    }

    pub fn evaluate(&mut self) -> i64 {
        if self.error {
            return 0;
        }

        if let Some(op) = self.active_operation {
            let mut expr_scratch = [0u8; 64];
            let mut elen = 0usize;
            if let Some(acc_dec) = Decimal::from_i64(self.accumulator) {
                elen += acc_dec.format(&mut expr_scratch[elen..]);
            }
            if elen < expr_scratch.len() {
                expr_scratch[elen] = b' ';
                elen += 1;
            }
            if elen < expr_scratch.len() {
                expr_scratch[elen] = op as u8;
                elen += 1;
            }
            if elen < expr_scratch.len() {
                expr_scratch[elen] = b' ';
                elen += 1;
            }
            if let Some(op_dec) = Decimal::from_i64(self.current_operand) {
                elen += op_dec.format(&mut expr_scratch[elen..]);
            }

            match op {
                '+' => {
                    if let Some(res) = self.accumulator.checked_add(self.current_operand) {
                        self.accumulator = res;
                    } else {
                        self.error = true;
                        self.calc_error = Some(CalcError::Overflow);
                        self.active_operation = None;
                        self.result_mode = true;
                        return 0;
                    }
                }
                '-' => {
                    if let Some(res) = self.accumulator.checked_sub(self.current_operand) {
                        self.accumulator = res;
                    } else {
                        self.error = true;
                        self.calc_error = Some(CalcError::Overflow);
                        self.active_operation = None;
                        self.result_mode = true;
                        return 0;
                    }
                }
                '*' => {
                    if let Some(res) = self.accumulator.checked_mul(self.current_operand) {
                        self.accumulator = res;
                    } else {
                        self.error = true;
                        self.calc_error = Some(CalcError::Overflow);
                        self.active_operation = None;
                        self.result_mode = true;
                        return 0;
                    }
                }
                '/' => {
                    if self.current_operand == 0 {
                        self.error = true;
                        self.calc_error = Some(CalcError::DivisionByZero);
                        self.active_operation = None;
                        self.result_mode = true;
                        return 0;
                    }
                    if let Some(res) = self.accumulator.checked_div(self.current_operand) {
                        self.accumulator = res;
                    } else {
                        self.error = true;
                        self.calc_error = Some(CalcError::Overflow);
                        self.active_operation = None;
                        self.result_mode = true;
                        return 0;
                    }
                }
                '%' => {
                    if self.current_operand == 0 {
                        self.error = true;
                        self.calc_error = Some(CalcError::DivisionByZero);
                        self.active_operation = None;
                        self.result_mode = true;
                        return 0;
                    }
                    if let Some(res) = self.accumulator.checked_rem(self.current_operand) {
                        self.accumulator = res;
                    } else {
                        self.error = true;
                        self.calc_error = Some(CalcError::Overflow);
                        self.active_operation = None;
                        self.result_mode = true;
                        return 0;
                    }
                }
                '^' => {
                    if self.current_operand >= 0 {
                        let exp = (self.current_operand as u64).min(63) as u32;
                        if let Some(res) = self.accumulator.checked_pow(exp) {
                            self.accumulator = res;
                        } else {
                            self.error = true;
                            self.calc_error = Some(CalcError::Overflow);
                            self.active_operation = None;
                            self.result_mode = true;
                            return 0;
                        }
                    } else {
                        self.accumulator = match self.accumulator {
                            0 => 0,
                            1 => 1,
                            -1 => {
                                if self.current_operand % 2 != 0 {
                                    -1
                                } else {
                                    1
                                }
                            }
                            _ => 0,
                        };
                    }
                }
                _ => {}
            }
            if let Some(res_dec) = Decimal::from_i64(self.accumulator) {
                let expr_str = core::str::from_utf8(&expr_scratch[..elen]).unwrap_or("");
                self.history.push(expr_str, res_dec);
            }
        } else if self.expr_len > 0 {
            if let Ok(dec) = self.evaluate_decimal() {
                return dec.whole_part();
            } else {
                return 0;
            }
        } else {
            self.accumulator = self.current_operand;
        }
        self.current_operand = self.accumulator;
        self.active_operation = None;
        self.result_mode = true;
        self.update_display_from_operand();
        self.accumulator
    }

    pub fn clear(&mut self) {
        self.accumulator = 0;
        self.current_operand = 0;
        self.active_operation = None;
        self.result_mode = false;
        self.error = false;
        self.calc_error = None;
        self.expr_len = 0;
        self.display_buf[0] = b'0';
        self.display_len = 1;
    }

    pub fn toggle_sign(&mut self) {
        if self.error {
            self.clear();
            return;
        }
        self.current_operand = -self.current_operand;
        if self.result_mode && self.display_len > 0 {
            if self.display_buf[0] == b'-' {
                // remove '-'
                for i in 1..self.display_len {
                    self.display_buf[i - 1] = self.display_buf[i];
                }
                self.display_len -= 1;
            } else if self.display_len < self.display_buf.len() {
                for i in (0..self.display_len).rev() {
                    self.display_buf[i + 1] = self.display_buf[i];
                }
                self.display_buf[0] = b'-';
                self.display_len += 1;
            }
            return;
        }

        // In expression mode, wrap or toggle sign of last operand
        if self.expr_len > 0 {
            // Find start of last number
            let mut idx = self.expr_len;
            while idx > 0 && self.expr_buf[idx - 1] != b' ' {
                idx -= 1;
            }
            if idx < self.expr_len {
                if self.expr_buf[idx] == b'-' {
                    for i in (idx + 1)..self.expr_len {
                        self.expr_buf[i - 1] = self.expr_buf[i];
                    }
                    self.expr_len -= 1;
                } else if self.expr_len < self.expr_buf.len() {
                    for i in (idx..self.expr_len).rev() {
                        self.expr_buf[i + 1] = self.expr_buf[i];
                    }
                    self.expr_buf[idx] = b'-';
                    self.expr_len += 1;
                }
            }
        } else {
            self.push_char('-');
        }
        self.update_display_from_operand();
    }

    pub fn toggle_history(&mut self) {
        self.show_history = !self.show_history;
    }

    pub fn load_history_entry(&mut self, recent_index: usize) {
        if let Some(entry) = self.history.get_recent(recent_index) {
            let res = entry.result;
            self.clear();
            self.display_len = res.format(&mut self.display_buf);
            self.expr_len = res.format(&mut self.expr_buf);
            self.accumulator = res.whole_part();
            self.current_operand = res.whole_part();
            self.result_mode = true;
        }
    }

    pub fn displayed_value(&self) -> i64 {
        self.current_operand
    }

    fn update_display_from_operand(&mut self) {
        let val = self.current_operand;
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
        let copy_len = count.min(self.display_buf.len());
        self.display_buf[..copy_len].copy_from_slice(&digits[..copy_len]);
        self.display_len = copy_len;
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
        let a = Decimal::from_parts(12, 3450).unwrap(); // 12.3450
        let b = Decimal::from_parts(3, 1550).unwrap();  // 3.1550
        let sum = a.saturating_add(b);
        assert_eq!(sum.whole_part(), 15);
        assert_eq!(sum.frac_part(), 5000);

        let diff = a.saturating_sub(b);
        assert_eq!(diff.whole_part(), 9);
        assert_eq!(diff.frac_part(), 1900);

        let mul = Decimal::from_int(2).saturating_mul(Decimal::from_parts(1, 5000).unwrap());
        assert_eq!(mul.whole_part(), 3);
        assert_eq!(mul.frac_part(), 0);

        let div = Decimal::from_int(10).saturating_div(Decimal::from_int(4)).unwrap();
        assert_eq!(div.whole_part(), 2);
        assert_eq!(div.frac_part(), 5000);

        // ASCII formatting test
        let mut buf = [0u8; 16];
        let len = sum.format_ascii(&mut buf);
        assert_eq!(&buf[..len], b"15.5000");

        // ASCII parsing test
        let parsed = Decimal::parse_ascii(b"15.5000").expect("parse");
        assert_eq!(parsed, sum);

        let parsed_neg = Decimal::parse_ascii(b"-42.0125").expect("parse neg");
        assert_eq!(parsed_neg.whole_part(), -42);
        assert_eq!(parsed_neg.frac_part(), 125);
    }

    #[test]
    fn test_expression_precedence_and_bodmas() {
        assert_eq!(evaluate("2 + 3 * 4").unwrap(), Decimal::from_i64(14).unwrap());
        assert_eq!(evaluate("(2 + 3) * 4").unwrap(), Decimal::from_i64(20).unwrap());
        assert_eq!(evaluate("10 - 2 * 3 + 4 / 2").unwrap(), Decimal::from_i64(6).unwrap());
    }

    #[test]
    fn test_expression_unary_operators() {
        assert_eq!(evaluate("-5 + 8").unwrap(), Decimal::from_i64(3).unwrap());
        assert_eq!(evaluate("-(2 + 3)").unwrap(), Decimal::from_i64(-5).unwrap());
        assert_eq!(evaluate("+7 - -3").unwrap(), Decimal::from_i64(10).unwrap());
        assert_eq!(evaluate("4 * -2").unwrap(), Decimal::from_i64(-8).unwrap());
    }

    #[test]
    fn test_expression_decimal_arithmetic() {
        let res = evaluate("0.1 + 0.2").unwrap();
        assert_eq!(res, Decimal::parse_ascii(b"0.3").unwrap());
        let mut buf = [0u8; 16];
        let len = res.format(&mut buf);
        assert_eq!(&buf[..len], b"0.3");

        let mul = evaluate("1.25 * 4").unwrap();
        assert_eq!(mul, Decimal::from_i64(5).unwrap());

        let div = evaluate("1 / 4").unwrap();
        assert_eq!(div, Decimal::parse_ascii(b"0.25").unwrap());
    }

    #[test]
    fn test_expression_functions() {
        assert_eq!(evaluate("sqrt(16)").unwrap(), Decimal::from_i64(4).unwrap());
        assert_eq!(evaluate("isqrt(25)").unwrap(), Decimal::from_i64(5).unwrap());
        assert_eq!(evaluate("reciprocal(4)").unwrap(), Decimal::parse_ascii(b"0.25").unwrap());
        assert_eq!(evaluate("50 %").unwrap(), Decimal::parse_ascii(b"0.5").unwrap());
        assert_eq!(evaluate("100 + 50 %").unwrap(), Decimal::parse_ascii(b"100.5").unwrap());
    }

    #[test]
    fn test_expression_errors() {
        assert_eq!(evaluate("10 / 0"), Err(CalcError::DivisionByZero));
        assert_eq!(evaluate("reciprocal(0)"), Err(CalcError::DivisionByZero));
        assert_eq!(evaluate("(2 + 3"), Err(CalcError::UnbalancedParentheses));
        assert_eq!(evaluate("2 + 3)"), Err(CalcError::UnbalancedParentheses));
        assert_eq!(evaluate("((1)"), Err(CalcError::UnbalancedParentheses));
        assert_eq!(evaluate("9999999999999 * 9999999999999"), Err(CalcError::Overflow));
        assert_eq!(evaluate(""), Err(CalcError::EmptyExpression));
        assert_eq!(evaluate("   "), Err(CalcError::EmptyExpression));
        assert_eq!(evaluate("2 + * 3"), Err(CalcError::InvalidSyntax));
    }

    #[test]
    fn test_history_tape_circular_overwrite() {
        let mut history = CalculationHistory::new();
        assert_eq!(history.count(), 0);

        for i in 1..=10 {
            let mut expr_buf = [0u8; 16];
            expr_buf[0] = b'e';
            expr_buf[1] = b'x';
            expr_buf[2] = b'p';
            expr_buf[3] = b'r';
            let dec = Decimal::from_i64(i).unwrap();
            history.push(core::str::from_utf8(&expr_buf[..4]).unwrap(), dec);
        }

        assert_eq!(history.count(), 8);
        assert_eq!(history.get(0).unwrap().result, Decimal::from_i64(3).unwrap());
        assert_eq!(history.get(7).unwrap().result, Decimal::from_i64(10).unwrap());
        assert_eq!(history.get_recent(0).unwrap().result, Decimal::from_i64(10).unwrap());

        history.clear();
        assert_eq!(history.count(), 0);
        assert!(history.get(0).is_none());
    }

    #[test]
    fn test_decimal_clean_formatting() {
        let mut buf = [0u8; 32];

        let d1 = Decimal::parse_ascii(b"12.5000").unwrap();
        let l1 = d1.format(&mut buf);
        assert_eq!(&buf[..l1], b"12.5");

        let d2 = Decimal::parse_ascii(b"-0.0075").unwrap();
        let l2 = d2.format(&mut buf);
        assert_eq!(&buf[..l2], b"-0.0075");

        let d3 = Decimal::from_i64(14).unwrap();
        let l3 = d3.format(&mut buf);
        assert_eq!(&buf[..l3], b"14");

        let d4 = Decimal::ZERO;
        let l4 = d4.format(&mut buf);
        assert_eq!(&buf[..l4], b"0");
    }
}
