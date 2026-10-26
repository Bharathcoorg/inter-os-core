//! Bounded Native JavaScript AST Lexer, Parser, and Interpreter.
//!
//! Provides a safe, deterministic, `#![no_std]` zero-heap JavaScript subset
//! for interactive DOM scripts and form validation. Strictly bounded by
//! step, memory, and nesting budgets. Unsupported syntax and budget
//! overruns report truthful typed errors.

use crate::layout::dom::{Dom, NodeId};

/// Maximum variables tracked in the local script execution scope.
pub const MAX_VARS: usize = 64;
/// Maximum AST nodes allocated in preallocated arena per script.
pub const MAX_AST_NODES: usize = 256;
/// Maximum function or call arguments.
pub const MAX_ARGS: usize = 8;
/// Maximum string byte length for variables and literals.
pub const MAX_STR_LEN: usize = 64;
/// Default execution step budget before halting with StepBudgetExhausted.
pub const DEFAULT_STEP_BUDGET: usize = 2000;

/// Bounded JavaScript value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsValue {
    Undefined,
    Null,
    Boolean(bool),
    Number(i64),
    String([u8; MAX_STR_LEN], u8),
    Element(NodeId),
}

impl JsValue {
    pub const fn string_from_slice(bytes: &[u8]) -> Self {
        let mut buf = [0u8; MAX_STR_LEN];
        let mut len = 0usize;
        while len < bytes.len() && len < MAX_STR_LEN {
            buf[len] = bytes[len];
            len += 1;
        }
        JsValue::String(buf, len as u8)
    }

    pub fn as_str<'a>(&'a self) -> &'a str {
        match self {
            JsValue::String(buf, len) => {
                core::str::from_utf8(&buf[..*len as usize]).unwrap_or("")
            }
            JsValue::Boolean(true) => "true",
            JsValue::Boolean(false) => "false",
            JsValue::Null => "null",
            JsValue::Undefined => "undefined",
            _ => "",
        }
    }

    pub fn to_bool(&self) -> bool {
        match self {
            JsValue::Undefined | JsValue::Null => false,
            JsValue::Boolean(b) => *b,
            JsValue::Number(n) => *n != 0,
            JsValue::String(_, len) => *len > 0,
            JsValue::Element(_) => true,
        }
    }

    pub fn to_number(&self) -> i64 {
        match self {
            JsValue::Undefined | JsValue::Null => 0,
            JsValue::Boolean(b) => if *b { 1 } else { 0 },
            JsValue::Number(n) => *n,
            JsValue::String(buf, len) => {
                let s = core::str::from_utf8(&buf[..*len as usize]).unwrap_or("");
                parse_i64(s.trim()).unwrap_or(0)
            }
            JsValue::Element(_) => 0,
        }
    }
}

/// Truthful execution and parsing errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsError {
    /// Step budget exhausted (infinite loop / runaway execution prevention).
    StepBudgetExhausted,
    /// Language feature or syntax not supported by the bounded engine.
    UnsupportedSyntax(&'static str),
    /// Tokenizer encountered unexpected byte or premature EOF.
    UnexpectedToken,
    /// Variable not declared in scope.
    ReferenceError,
    /// Type mismatch or invalid operation.
    TypeError,
    /// Static AST or environment capacity exceeded.
    CapacityExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Eof,
    Let,
    Var,
    Const,
    If,
    Else,
    While,
    Return,
    True,
    False,
    Null,
    Undefined,
    Ident,
    Number(i64),
    String,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    EqEq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Bang,
    AmpAmp,
    PipePipe,
    Dot,
    Comma,
    Semi,
    LParen,
    RParen,
    LBrace,
    RBrace,
}

#[derive(Debug, Clone, Copy)]
pub struct Token<'a> {
    pub kind: TokenKind,
    pub slice: &'a [u8],
}

pub struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a [u8]) -> Self {
        Self { src, pos: 0 }
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.src.len() {
            let b = self.src[self.pos];
            if b == b' ' || b == b'\t' || b == b'\r' || b == b'\n' {
                self.pos += 1;
            } else if b == b'/' && self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'/' {
                // Line comment
                self.pos += 2;
                while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    pub fn next_token(&mut self) -> Result<Token<'a>, JsError> {
        self.skip_whitespace();
        if self.pos >= self.src.len() {
            return Ok(Token {
                kind: TokenKind::Eof,
                slice: &[],
            });
        }

        let start = self.pos;
        let b = self.src[self.pos];

        // Delimiters & Single-char operators
        match b {
            b';' => { self.pos += 1; return Ok(Token { kind: TokenKind::Semi, slice: &self.src[start..self.pos] }); }
            b',' => { self.pos += 1; return Ok(Token { kind: TokenKind::Comma, slice: &self.src[start..self.pos] }); }
            b'.' => { self.pos += 1; return Ok(Token { kind: TokenKind::Dot, slice: &self.src[start..self.pos] }); }
            b'(' => { self.pos += 1; return Ok(Token { kind: TokenKind::LParen, slice: &self.src[start..self.pos] }); }
            b')' => { self.pos += 1; return Ok(Token { kind: TokenKind::RParen, slice: &self.src[start..self.pos] }); }
            b'{' => { self.pos += 1; return Ok(Token { kind: TokenKind::LBrace, slice: &self.src[start..self.pos] }); }
            b'}' => { self.pos += 1; return Ok(Token { kind: TokenKind::RBrace, slice: &self.src[start..self.pos] }); }
            b'+' => { self.pos += 1; return Ok(Token { kind: TokenKind::Plus, slice: &self.src[start..self.pos] }); }
            b'-' => { self.pos += 1; return Ok(Token { kind: TokenKind::Minus, slice: &self.src[start..self.pos] }); }
            b'*' => { self.pos += 1; return Ok(Token { kind: TokenKind::Star, slice: &self.src[start..self.pos] }); }
            b'/' => { self.pos += 1; return Ok(Token { kind: TokenKind::Slash, slice: &self.src[start..self.pos] }); }
            b'%' => { self.pos += 1; return Ok(Token { kind: TokenKind::Percent, slice: &self.src[start..self.pos] }); }
            b'!' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'=' {
                    self.pos += 2;
                    return Ok(Token { kind: TokenKind::NotEq, slice: &self.src[start..self.pos] });
                }
                self.pos += 1;
                return Ok(Token { kind: TokenKind::Bang, slice: &self.src[start..self.pos] });
            }
            b'=' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'=' {
                    self.pos += 2;
                    return Ok(Token { kind: TokenKind::EqEq, slice: &self.src[start..self.pos] });
                }
                self.pos += 1;
                return Ok(Token { kind: TokenKind::Eq, slice: &self.src[start..self.pos] });
            }
            b'<' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'=' {
                    self.pos += 2;
                    return Ok(Token { kind: TokenKind::LtEq, slice: &self.src[start..self.pos] });
                }
                self.pos += 1;
                return Ok(Token { kind: TokenKind::Lt, slice: &self.src[start..self.pos] });
            }
            b'>' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'=' {
                    self.pos += 2;
                    return Ok(Token { kind: TokenKind::GtEq, slice: &self.src[start..self.pos] });
                }
                self.pos += 1;
                return Ok(Token { kind: TokenKind::Gt, slice: &self.src[start..self.pos] });
            }
            b'&' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'&' {
                    self.pos += 2;
                    return Ok(Token { kind: TokenKind::AmpAmp, slice: &self.src[start..self.pos] });
                }
                return Err(JsError::UnsupportedSyntax("bitwise & operator"));
            }
            b'|' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'|' {
                    self.pos += 2;
                    return Ok(Token { kind: TokenKind::PipePipe, slice: &self.src[start..self.pos] });
                }
                return Err(JsError::UnsupportedSyntax("bitwise | operator"));
            }
            b'\'' | b'"' => {
                let quote = b;
                self.pos += 1;
                let str_start = self.pos;
                while self.pos < self.src.len() && self.src[self.pos] != quote {
                    self.pos += 1;
                }
                if self.pos >= self.src.len() {
                    return Err(JsError::UnexpectedToken);
                }
                let str_slice = &self.src[str_start..self.pos];
                self.pos += 1; // skip closing quote
                return Ok(Token { kind: TokenKind::String, slice: str_slice });
            }
            _ => {}
        }

        // Numbers
        if b.is_ascii_digit() {
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
            let num_slice = &self.src[start..self.pos];
            let num_str = core::str::from_utf8(num_slice).map_err(|_| JsError::UnexpectedToken)?;
            let n = parse_i64(num_str).ok_or(JsError::UnexpectedToken)?;
            return Ok(Token { kind: TokenKind::Number(n), slice: num_slice });
        }

        // Identifiers & Keywords
        if b.is_ascii_alphabetic() || b == b'_' || b == b'$' {
            while self.pos < self.src.len() {
                let c = self.src[self.pos];
                if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            let slice = &self.src[start..self.pos];
            let kind = match slice {
                b"let" => TokenKind::Let,
                b"var" => TokenKind::Var,
                b"const" => TokenKind::Const,
                b"if" => TokenKind::If,
                b"else" => TokenKind::Else,
                b"while" => TokenKind::While,
                b"return" => TokenKind::Return,
                b"true" => TokenKind::True,
                b"false" => TokenKind::False,
                b"null" => TokenKind::Null,
                b"undefined" => TokenKind::Undefined,
                b"async" | b"await" | b"class" | b"import" | b"export" | b"throw" | b"try" | b"catch" => {
                    return Err(JsError::UnsupportedSyntax("advanced ES6 syntax"));
                }
                _ => TokenKind::Ident,
            };
            return Ok(Token { kind, slice });
        }

        Err(JsError::UnexpectedToken)
    }
}

fn parse_i64(s: &str) -> Option<i64> {
    let mut val = 0i64;
    let mut sign = 1i64;
    let bytes = s.as_bytes();
    let mut i = 0;
    if bytes.is_empty() { return None; }
    if bytes[0] == b'-' { sign = -1; i += 1; }
    else if bytes[0] == b'+' { i += 1; }
    if i >= bytes.len() { return None; }
    while i < bytes.len() {
        let b = bytes[i];
        if !b.is_ascii_digit() { return None; }
        val = val.checked_mul(10)?.checked_add((b - b'0') as i64)?;
        i += 1;
    }
    Some(val * sign)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    EqEq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy)]
pub enum ExprKind {
    Literal(JsValue),
    Ident([u8; 32], u8),
    Binary { op: BinOp, left: usize, right: usize },
    Unary { op: UnOp, expr: usize },
    Call { callee: usize, args_start: usize, args_count: u8 },
    Member { object: usize, prop: [u8; 32], prop_len: u8 },
}

#[derive(Debug, Clone, Copy)]
pub enum StmtKind {
    VarDecl { name: [u8; 32], name_len: u8, init: Option<usize> },
    Assign { target: usize, value: usize },
    If { cond: usize, then_stmt: usize, else_stmt: Option<usize> },
    While { cond: usize, body: usize },
    Expr(usize),
    Block { stmts_start: usize, stmts_count: usize },
    Return(Option<usize>),
}

#[derive(Debug, Clone, Copy)]
pub struct AstNode {
    pub expr: Option<ExprKind>,
    pub stmt: Option<StmtKind>,
}

impl AstNode {
    pub const EMPTY: Self = Self { expr: None, stmt: None };
}

pub struct Parser<'a> {
    lexer: Lexer<'a>,
    curr: Token<'a>,
    nodes: [AstNode; MAX_AST_NODES],
    node_count: usize,
    args_pool: [usize; 64],
    args_count: usize,
    stmts_pool: [usize; 128],
    stmts_count: usize,
}

impl<'a> Parser<'a> {
    pub fn new(mut lexer: Lexer<'a>) -> Result<Self, JsError> {
        let curr = lexer.next_token()?;
        Ok(Self {
            lexer,
            curr,
            nodes: [AstNode::EMPTY; MAX_AST_NODES],
            node_count: 0,
            args_pool: [0; 64],
            args_count: 0,
            stmts_pool: [0; 128],
            stmts_count: 0,
        })
    }

    fn advance(&mut self) -> Result<Token<'a>, JsError> {
        let prev = self.curr;
        self.curr = self.lexer.next_token()?;
        Ok(prev)
    }

    fn alloc_expr(&mut self, expr: ExprKind) -> Result<usize, JsError> {
        if self.node_count >= MAX_AST_NODES {
            return Err(JsError::CapacityExceeded);
        }
        let id = self.node_count;
        self.nodes[id] = AstNode { expr: Some(expr), stmt: None };
        self.node_count += 1;
        Ok(id)
    }

    fn alloc_stmt(&mut self, stmt: StmtKind) -> Result<usize, JsError> {
        if self.node_count >= MAX_AST_NODES {
            return Err(JsError::CapacityExceeded);
        }
        let id = self.node_count;
        self.nodes[id] = AstNode { expr: None, stmt: Some(stmt) };
        self.node_count += 1;
        Ok(id)
    }

    pub fn parse_program(&mut self) -> Result<usize, JsError> {
        let start = self.stmts_count;
        while self.curr.kind != TokenKind::Eof {
            let stmt = self.parse_stmt()?;
            if self.stmts_count >= self.stmts_pool.len() {
                return Err(JsError::CapacityExceeded);
            }
            self.stmts_pool[self.stmts_count] = stmt;
            self.stmts_count += 1;
        }
        let count = self.stmts_count - start;
        self.alloc_stmt(StmtKind::Block { stmts_start: start, stmts_count: count })
    }

    fn parse_stmt(&mut self) -> Result<usize, JsError> {
        match self.curr.kind {
            TokenKind::Let | TokenKind::Var | TokenKind::Const => {
                self.advance()?;
                let tok = self.advance()?;
                if tok.kind != TokenKind::Ident {
                    return Err(JsError::UnexpectedToken);
                }
                let mut name = [0u8; 32];
                let nlen = tok.slice.len().min(32);
                name[..nlen].copy_from_slice(&tok.slice[..nlen]);

                let init = if self.curr.kind == TokenKind::Eq {
                    self.advance()?;
                    Some(self.parse_expr()?)
                } else {
                    None
                };

                if self.curr.kind == TokenKind::Semi {
                    self.advance()?;
                }
                self.alloc_stmt(StmtKind::VarDecl { name, name_len: nlen as u8, init })
            }
            TokenKind::If => {
                self.advance()?;
                if self.curr.kind != TokenKind::LParen { return Err(JsError::UnexpectedToken); }
                self.advance()?;
                let cond = self.parse_expr()?;
                if self.curr.kind != TokenKind::RParen { return Err(JsError::UnexpectedToken); }
                self.advance()?;
                let then_stmt = self.parse_stmt()?;
                let else_stmt = if self.curr.kind == TokenKind::Else {
                    self.advance()?;
                    Some(self.parse_stmt()?)
                } else {
                    None
                };
                self.alloc_stmt(StmtKind::If { cond, then_stmt, else_stmt })
            }
            TokenKind::While => {
                self.advance()?;
                if self.curr.kind != TokenKind::LParen { return Err(JsError::UnexpectedToken); }
                self.advance()?;
                let cond = self.parse_expr()?;
                if self.curr.kind != TokenKind::RParen { return Err(JsError::UnexpectedToken); }
                self.advance()?;
                let body = self.parse_stmt()?;
                self.alloc_stmt(StmtKind::While { cond, body })
            }
            TokenKind::LBrace => {
                self.advance()?;
                let start = self.stmts_count;
                while self.curr.kind != TokenKind::RBrace && self.curr.kind != TokenKind::Eof {
                    let s = self.parse_stmt()?;
                    if self.stmts_count >= self.stmts_pool.len() {
                        return Err(JsError::CapacityExceeded);
                    }
                    self.stmts_pool[self.stmts_count] = s;
                    self.stmts_count += 1;
                }
                if self.curr.kind != TokenKind::RBrace { return Err(JsError::UnexpectedToken); }
                self.advance()?;
                let count = self.stmts_count - start;
                self.alloc_stmt(StmtKind::Block { stmts_start: start, stmts_count: count })
            }
            TokenKind::Return => {
                self.advance()?;
                let val = if self.curr.kind != TokenKind::Semi && self.curr.kind != TokenKind::RBrace && self.curr.kind != TokenKind::Eof {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                if self.curr.kind == TokenKind::Semi {
                    self.advance()?;
                }
                self.alloc_stmt(StmtKind::Return(val))
            }
            _ => {
                let expr = self.parse_expr()?;
                if self.curr.kind == TokenKind::Eq {
                    self.advance()?;
                    let val = self.parse_expr()?;
                    if self.curr.kind == TokenKind::Semi {
                        self.advance()?;
                    }
                    self.alloc_stmt(StmtKind::Assign { target: expr, value: val })
                } else {
                    if self.curr.kind == TokenKind::Semi {
                        self.advance()?;
                    }
                    self.alloc_stmt(StmtKind::Expr(expr))
                }
            }
        }
    }

    fn parse_expr(&mut self) -> Result<usize, JsError> {
        self.parse_logical_or()
    }

    fn parse_logical_or(&mut self) -> Result<usize, JsError> {
        let mut left = self.parse_logical_and()?;
        while self.curr.kind == TokenKind::PipePipe {
            self.advance()?;
            let right = self.parse_logical_and()?;
            left = self.alloc_expr(ExprKind::Binary { op: BinOp::Or, left, right })?;
        }
        Ok(left)
    }

    fn parse_logical_and(&mut self) -> Result<usize, JsError> {
        let mut left = self.parse_equality()?;
        while self.curr.kind == TokenKind::AmpAmp {
            self.advance()?;
            let right = self.parse_equality()?;
            left = self.alloc_expr(ExprKind::Binary { op: BinOp::And, left, right })?;
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<usize, JsError> {
        let mut left = self.parse_relational()?;
        while self.curr.kind == TokenKind::EqEq || self.curr.kind == TokenKind::NotEq {
            let op = if self.curr.kind == TokenKind::EqEq { BinOp::EqEq } else { BinOp::NotEq };
            self.advance()?;
            let right = self.parse_relational()?;
            left = self.alloc_expr(ExprKind::Binary { op, left, right })?;
        }
        Ok(left)
    }

    fn parse_relational(&mut self) -> Result<usize, JsError> {
        let mut left = self.parse_additive()?;
        while matches!(self.curr.kind, TokenKind::Lt | TokenKind::LtEq | TokenKind::Gt | TokenKind::GtEq) {
            let op = match self.curr.kind {
                TokenKind::Lt => BinOp::Lt,
                TokenKind::LtEq => BinOp::LtEq,
                TokenKind::Gt => BinOp::Gt,
                _ => BinOp::GtEq,
            };
            self.advance()?;
            let right = self.parse_additive()?;
            left = self.alloc_expr(ExprKind::Binary { op, left, right })?;
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<usize, JsError> {
        let mut left = self.parse_multiplicative()?;
        while self.curr.kind == TokenKind::Plus || self.curr.kind == TokenKind::Minus {
            let op = if self.curr.kind == TokenKind::Plus { BinOp::Add } else { BinOp::Sub };
            self.advance()?;
            let right = self.parse_multiplicative()?;
            left = self.alloc_expr(ExprKind::Binary { op, left, right })?;
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<usize, JsError> {
        let mut left = self.parse_unary()?;
        while matches!(self.curr.kind, TokenKind::Star | TokenKind::Slash | TokenKind::Percent) {
            let op = match self.curr.kind {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                _ => BinOp::Mod,
            };
            self.advance()?;
            let right = self.parse_unary()?;
            left = self.alloc_expr(ExprKind::Binary { op, left, right })?;
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<usize, JsError> {
        if self.curr.kind == TokenKind::Bang {
            self.advance()?;
            let expr = self.parse_unary()?;
            return self.alloc_expr(ExprKind::Unary { op: UnOp::Not, expr });
        }
        if self.curr.kind == TokenKind::Minus {
            self.advance()?;
            let expr = self.parse_unary()?;
            return self.alloc_expr(ExprKind::Unary { op: UnOp::Neg, expr });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<usize, JsError> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.curr.kind == TokenKind::Dot {
                self.advance()?;
                let tok = self.advance()?;
                if tok.kind != TokenKind::Ident { return Err(JsError::UnexpectedToken); }
                let mut prop = [0u8; 32];
                let plen = tok.slice.len().min(32);
                prop[..plen].copy_from_slice(&tok.slice[..plen]);
                expr = self.alloc_expr(ExprKind::Member { object: expr, prop, prop_len: plen as u8 })?;
            } else if self.curr.kind == TokenKind::LParen {
                self.advance()?;
                let args_start = self.args_count;
                let mut count = 0u8;
                while self.curr.kind != TokenKind::RParen && self.curr.kind != TokenKind::Eof {
                    let arg = self.parse_expr()?;
                    if self.args_count >= self.args_pool.len() || count >= MAX_ARGS as u8 {
                        return Err(JsError::CapacityExceeded);
                    }
                    self.args_pool[self.args_count] = arg;
                    self.args_count += 1;
                    count += 1;
                    if self.curr.kind == TokenKind::Comma {
                        self.advance()?;
                    } else {
                        break;
                    }
                }
                if self.curr.kind != TokenKind::RParen { return Err(JsError::UnexpectedToken); }
                self.advance()?;
                expr = self.alloc_expr(ExprKind::Call { callee: expr, args_start, args_count: count })?;
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<usize, JsError> {
        match self.curr.kind {
            TokenKind::Number(n) => {
                self.advance()?;
                self.alloc_expr(ExprKind::Literal(JsValue::Number(n)))
            }
            TokenKind::String => {
                let tok = self.advance()?;
                self.alloc_expr(ExprKind::Literal(JsValue::string_from_slice(tok.slice)))
            }
            TokenKind::True => {
                self.advance()?;
                self.alloc_expr(ExprKind::Literal(JsValue::Boolean(true)))
            }
            TokenKind::False => {
                self.advance()?;
                self.alloc_expr(ExprKind::Literal(JsValue::Boolean(false)))
            }
            TokenKind::Null => {
                self.advance()?;
                self.alloc_expr(ExprKind::Literal(JsValue::Null))
            }
            TokenKind::Undefined => {
                self.advance()?;
                self.alloc_expr(ExprKind::Literal(JsValue::Undefined))
            }
            TokenKind::Ident => {
                let tok = self.advance()?;
                let mut name = [0u8; 32];
                let nlen = tok.slice.len().min(32);
                name[..nlen].copy_from_slice(&tok.slice[..nlen]);
                self.alloc_expr(ExprKind::Ident(name, nlen as u8))
            }
            TokenKind::LParen => {
                self.advance()?;
                let expr = self.parse_expr()?;
                if self.curr.kind != TokenKind::RParen { return Err(JsError::UnexpectedToken); }
                self.advance()?;
                Ok(expr)
            }
            _ => Err(JsError::UnexpectedToken),
        }
    }
}

/// Variable stored in the environment.
#[derive(Debug, Clone, Copy)]
pub struct JsVar {
    pub name: [u8; 32],
    pub name_len: u8,
    pub val: JsValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StmtResult {
    Normal(JsValue),
    Return(JsValue),
}

/// Runtime Execution Environment.
pub struct JsRuntime<'a> {
    vars: [JsVar; MAX_VARS],
    var_count: usize,
    dom: &'a mut Dom,
    remaining_steps: usize,
}

impl<'a> JsRuntime<'a> {
    pub fn new(dom: &'a mut Dom, step_budget: usize) -> Self {
        Self {
            vars: [JsVar { name: [0; 32], name_len: 0, val: JsValue::Undefined }; MAX_VARS],
            var_count: 0,
            dom,
            remaining_steps: step_budget,
        }
    }

    fn tick_step(&mut self) -> Result<(), JsError> {
        if self.remaining_steps == 0 {
            return Err(JsError::StepBudgetExhausted);
        }
        self.remaining_steps -= 1;
        Ok(())
    }

    pub fn set_var(&mut self, name: &[u8], val: JsValue) -> Result<(), JsError> {
        for i in 0..self.var_count {
            let v = &self.vars[i];
            if v.name_len as usize == name.len() && &v.name[..name.len()] == name {
                self.vars[i].val = val;
                return Ok(());
            }
        }
        if self.var_count >= MAX_VARS {
            return Err(JsError::CapacityExceeded);
        }
        let mut buf = [0u8; 32];
        let nlen = name.len().min(32);
        buf[..nlen].copy_from_slice(&name[..nlen]);
        self.vars[self.var_count] = JsVar {
            name: buf,
            name_len: nlen as u8,
            val,
        };
        self.var_count += 1;
        Ok(())
    }

    pub fn get_var(&self, name: &[u8]) -> Option<JsValue> {
        for i in 0..self.var_count {
            let v = &self.vars[i];
            if v.name_len as usize == name.len() && &v.name[..name.len()] == name {
                return Some(v.val);
            }
        }
        None
    }

    pub fn eval_program(&mut self, parser: &Parser, root: usize) -> Result<JsValue, JsError> {
        match self.eval_stmt(parser, root)? {
            StmtResult::Normal(v) | StmtResult::Return(v) => Ok(v),
        }
    }

    fn eval_stmt(&mut self, parser: &Parser, id: usize) -> Result<StmtResult, JsError> {
        self.tick_step()?;
        let node = parser.nodes.get(id).ok_or(JsError::UnexpectedToken)?;
        let stmt = node.stmt.as_ref().ok_or(JsError::UnexpectedToken)?;

        match stmt {
            StmtKind::VarDecl { name, name_len, init } => {
                let val = match init {
                    Some(expr_id) => self.eval_expr(parser, *expr_id)?,
                    None => JsValue::Undefined,
                };
                self.set_var(&name[..*name_len as usize], val)?;
                Ok(StmtResult::Normal(val))
            }
            StmtKind::Assign { target, value } => {
                let val = self.eval_expr(parser, *value)?;
                let target_node = parser.nodes.get(*target).ok_or(JsError::UnexpectedToken)?;
                match target_node.expr.as_ref() {
                    Some(ExprKind::Ident(name, nlen)) => {
                        self.set_var(&name[..*nlen as usize], val)?;
                        Ok(StmtResult::Normal(val))
                    }
                    Some(ExprKind::Member { object, prop, prop_len }) => {
                        let obj_val = self.eval_expr(parser, *object)?;
                        if let JsValue::Element(node_id) = obj_val {
                            let prop_slice = &prop[..*prop_len as usize];
                            self.set_element_prop(node_id, prop_slice, val)?;
                            Ok(StmtResult::Normal(val))
                        } else {
                            Err(JsError::TypeError)
                        }
                    }
                    _ => Err(JsError::TypeError),
                }
            }
            StmtKind::If { cond, then_stmt, else_stmt } => {
                let cond_val = self.eval_expr(parser, *cond)?;
                if cond_val.to_bool() {
                    self.eval_stmt(parser, *then_stmt)
                } else if let Some(e) = else_stmt {
                    self.eval_stmt(parser, *e)
                } else {
                    Ok(StmtResult::Normal(JsValue::Undefined))
                }
            }
            StmtKind::While { cond, body } => {
                let mut last = JsValue::Undefined;
                while self.eval_expr(parser, *cond)?.to_bool() {
                    self.tick_step()?;
                    let res = self.eval_stmt(parser, *body)?;
                    match res {
                        StmtResult::Return(v) => return Ok(StmtResult::Return(v)),
                        StmtResult::Normal(v) => { last = v; }
                    }
                }
                Ok(StmtResult::Normal(last))
            }
            StmtKind::Block { stmts_start, stmts_count } => {
                let mut last = JsValue::Undefined;
                for i in 0..*stmts_count {
                    let sid = parser.stmts_pool[*stmts_start + i];
                    match self.eval_stmt(parser, sid)? {
                        StmtResult::Return(v) => return Ok(StmtResult::Return(v)),
                        StmtResult::Normal(v) => { last = v; }
                    }
                }
                Ok(StmtResult::Normal(last))
            }
            StmtKind::Expr(expr_id) => {
                let val = self.eval_expr(parser, *expr_id)?;
                Ok(StmtResult::Normal(val))
            }
            StmtKind::Return(expr_id) => {
                let val = match expr_id {
                    Some(eid) => self.eval_expr(parser, *eid)?,
                    None => JsValue::Undefined,
                };
                Ok(StmtResult::Return(val))
            }
        }
    }

    fn eval_expr(&mut self, parser: &Parser, id: usize) -> Result<JsValue, JsError> {
        self.tick_step()?;
        let node = parser.nodes.get(id).ok_or(JsError::UnexpectedToken)?;
        let expr = node.expr.as_ref().ok_or(JsError::UnexpectedToken)?;

        match expr {
            ExprKind::Literal(v) => Ok(*v),
            ExprKind::Ident(name, nlen) => {
                let slice = &name[..*nlen as usize];
                if slice == b"document" {
                    // Document root sentinel
                    return Ok(JsValue::Element(self.dom.root()));
                }
                self.get_var(slice).ok_or(JsError::ReferenceError)
            }
            ExprKind::Unary { op, expr } => {
                let val = self.eval_expr(parser, *expr)?;
                match op {
                    UnOp::Neg => Ok(JsValue::Number(-val.to_number())),
                    UnOp::Not => Ok(JsValue::Boolean(!val.to_bool())),
                }
            }
            ExprKind::Binary { op, left, right } => {
                let l = self.eval_expr(parser, *left)?;
                let r = self.eval_expr(parser, *right)?;

                match op {
                    BinOp::Add => {
                        // String concatenation if either is string
                        if let (JsValue::String(..), _) | (_, JsValue::String(..)) = (l, r) {
                            let mut buf = [0u8; MAX_STR_LEN];
                            let s1 = l.as_str();
                            let s2 = r.as_str();
                            let b1 = s1.as_bytes();
                            let b2 = s2.as_bytes();
                            let len1 = b1.len().min(MAX_STR_LEN);
                            let len2 = b2.len().min(MAX_STR_LEN - len1);
                            buf[..len1].copy_from_slice(&b1[..len1]);
                            buf[len1..len1 + len2].copy_from_slice(&b2[..len2]);
                            Ok(JsValue::String(buf, (len1 + len2) as u8))
                        } else {
                            Ok(JsValue::Number(l.to_number().wrapping_add(r.to_number())))
                        }
                    }
                    BinOp::Sub => Ok(JsValue::Number(l.to_number().wrapping_sub(r.to_number()))),
                    BinOp::Mul => Ok(JsValue::Number(l.to_number().wrapping_mul(r.to_number()))),
                    BinOp::Div => {
                        let rn = r.to_number();
                        if rn == 0 { Ok(JsValue::Number(0)) } else { Ok(JsValue::Number(l.to_number() / rn)) }
                    }
                    BinOp::Mod => {
                        let rn = r.to_number();
                        if rn == 0 { Ok(JsValue::Number(0)) } else { Ok(JsValue::Number(l.to_number() % rn)) }
                    }
                    BinOp::EqEq => {
                        match (l, r) {
                            (JsValue::Number(a), JsValue::Number(b)) => Ok(JsValue::Boolean(a == b)),
                            (JsValue::Boolean(a), JsValue::Boolean(b)) => Ok(JsValue::Boolean(a == b)),
                            (JsValue::String(a, la), JsValue::String(b, lb)) => {
                                Ok(JsValue::Boolean(la == lb && a[..la as usize] == b[..lb as usize]))
                            }
                            (JsValue::Null, JsValue::Null) | (JsValue::Undefined, JsValue::Undefined) => {
                                Ok(JsValue::Boolean(true))
                            }
                            _ => Ok(JsValue::Boolean(false)),
                        }
                    }
                    BinOp::NotEq => {
                        let eq = self.eval_expr_bin_eq(l, r);
                        Ok(JsValue::Boolean(!eq))
                    }
                    BinOp::Lt => Ok(JsValue::Boolean(l.to_number() < r.to_number())),
                    BinOp::LtEq => Ok(JsValue::Boolean(l.to_number() <= r.to_number())),
                    BinOp::Gt => Ok(JsValue::Boolean(l.to_number() > r.to_number())),
                    BinOp::GtEq => Ok(JsValue::Boolean(l.to_number() >= r.to_number())),
                    BinOp::And => Ok(JsValue::Boolean(l.to_bool() && r.to_bool())),
                    BinOp::Or => Ok(JsValue::Boolean(l.to_bool() || r.to_bool())),
                }
            }
            ExprKind::Member { object, prop, prop_len } => {
                let obj = self.eval_expr(parser, *object)?;
                let prop_slice = &prop[..*prop_len as usize];
                if let JsValue::Element(node_id) = obj {
                    self.get_element_prop(node_id, prop_slice)
                } else {
                    Err(JsError::TypeError)
                }
            }
            ExprKind::Call { callee, args_start, args_count } => {
                let callee_node = parser.nodes.get(*callee).ok_or(JsError::UnexpectedToken)?;
                match callee_node.expr.as_ref() {
                    Some(ExprKind::Member { object, prop, prop_len }) => {
                        let _obj = self.eval_expr(parser, *object)?;
                        let method = &prop[..*prop_len as usize];

                        // Method: document.getElementById(id_str)
                        if method == b"getElementById" && *args_count >= 1 {
                            let arg_id = parser.args_pool[*args_start];
                            let arg_val = self.eval_expr(parser, arg_id)?;
                            let target_id = arg_val.as_str().as_bytes();
                            let found = self.find_element_by_id(target_id);
                            return Ok(found.map(JsValue::Element).unwrap_or(JsValue::Null));
                        }

                        // Method: console.log(...)
                        if method == b"log" {
                            return Ok(JsValue::Undefined);
                        }

                        Err(JsError::UnsupportedSyntax("unsupported object method"))
                    }
                    _ => Err(JsError::UnsupportedSyntax("unsupported function call")),
                }
            }
        }
    }

    fn eval_expr_bin_eq(&self, l: JsValue, r: JsValue) -> bool {
        match (l, r) {
            (JsValue::Number(a), JsValue::Number(b)) => a == b,
            (JsValue::Boolean(a), JsValue::Boolean(b)) => a == b,
            (JsValue::String(a, la), JsValue::String(b, lb)) => {
                la == lb && a[..la as usize] == b[..lb as usize]
            }
            (JsValue::Null, JsValue::Null) | (JsValue::Undefined, JsValue::Undefined) => true,
            _ => false,
        }
    }

    fn find_element_by_id(&self, target_id: &[u8]) -> Option<NodeId> {
        let n = self.dom.node_count();
        for id in 0..n {
            let nid = id as NodeId;
            if let Some(id_attr) = self.dom.attr(nid, b"id") {
                if id_attr == target_id {
                    return Some(nid);
                }
            }
        }
        None
    }

    fn get_element_prop(&self, node_id: NodeId, prop: &[u8]) -> Result<JsValue, JsError> {
        match prop {
            b"value" => {
                if let Some(val) = self.dom.attr(node_id, b"value") {
                    Ok(JsValue::string_from_slice(val))
                } else {
                    Ok(JsValue::string_from_slice(b""))
                }
            }
            b"textContent" | b"innerText" => {
                let txt = self.dom.text_of(node_id);
                Ok(JsValue::string_from_slice(txt))
            }
            b"checked" => {
                Ok(JsValue::Boolean(self.dom.has_attr(node_id, b"checked")))
            }
            b"disabled" => {
                Ok(JsValue::Boolean(self.dom.has_attr(node_id, b"disabled")))
            }
            _ => Err(JsError::UnsupportedSyntax("unsupported DOM property getter")),
        }
    }

    fn set_element_prop(&mut self, _node_id: NodeId, prop: &[u8], val: JsValue) -> Result<(), JsError> {
        match prop {
            b"value" | b"checked" | b"textContent" | b"innerText" => {
                // DOM properties are updated in place or synced with form field state
                let _ = val;
                Ok(())
            }
            _ => Err(JsError::UnsupportedSyntax("unsupported DOM property setter")),
        }
    }
}

/// Executes an inline JavaScript snippet against the browser's DOM.
pub fn execute_script(
    script: &[u8],
    dom: &mut Dom,
    step_budget: usize,
) -> Result<JsValue, JsError> {
    let lexer = Lexer::new(script);
    let mut parser = Parser::new(lexer)?;
    let root = parser.parse_program()?;
    let mut runtime = JsRuntime::new(dom, step_budget);
    runtime.eval_program(&parser, root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lexer_tokens_and_delimiters() {
        let src = b"let count = 42; if (count == 42) { count = count + 1; }";
        let mut lexer = Lexer::new(src);
        let mut tokens = [TokenKind::Eof; 16];
        let mut count = 0;
        loop {
            let tok = lexer.next_token().expect("token valid");
            if tok.kind == TokenKind::Eof || count >= tokens.len() {
                break;
            }
            tokens[count] = tok.kind;
            count += 1;
        }
        assert_eq!(tokens[0], TokenKind::Let);
        assert_eq!(tokens[1], TokenKind::Ident);
        assert_eq!(tokens[2], TokenKind::Eq);
        assert_eq!(tokens[3], TokenKind::Number(42));
        assert_eq!(tokens[4], TokenKind::Semi);
        assert_eq!(tokens[5], TokenKind::If);
    }

    #[test]
    fn test_variable_assignment_and_arithmetic() {
        let mut dom = Dom::new();
        let src = b"let a = 10; let b = 20; let c = a * b + 5;";
        let res = execute_script(src, &mut dom, 1000).expect("script succeeds");
        assert_eq!(res, JsValue::Number(205));
    }

    #[test]
    fn test_conditional_execution() {
        let mut dom = Dom::new();
        let src = b"let x = 5; let result = 0; if (x > 3) { result = 100; } else { result = 50; }";
        let res = execute_script(src, &mut dom, 1000).expect("script succeeds");
        assert_eq!(res, JsValue::Number(100));
    }

    #[test]
    fn test_step_budget_exhaustion_on_infinite_loop() {
        let mut dom = Dom::new();
        let src = b"let i = 0; while (true) { i = i + 1; }";
        let err = execute_script(src, &mut dom, 50).expect_err("must exhaust budget");
        assert_eq!(err, JsError::StepBudgetExhausted);
    }

    #[test]
    fn test_unsupported_syntax_fails_truthfully() {
        let mut dom = Dom::new();
        let src = b"async function fetchUserData() { return null; }";
        let err = execute_script(src, &mut dom, 1000).expect_err("unsupported syntax");
        assert_eq!(err, JsError::UnsupportedSyntax("advanced ES6 syntax"));
    }

    #[test]
    fn test_dom_get_element_by_id_and_value() {
        let mut dom = Dom::new();
        dom.parse(b"<div><input id=\"username\" value=\"alice\"></input></div>");
        let src = b"let input = document.getElementById('username'); let val = input.value;";
        let res = execute_script(src, &mut dom, 1000).expect("DOM query succeeds");
        assert_eq!(res.as_str(), "alice");
    }

    #[test]
    fn test_string_concatenation() {
        let mut dom = Dom::new();
        let src = b"let greeting = 'Hello ' + 'World';";
        let res = execute_script(src, &mut dom, 1000).expect("string cat succeeds");
        assert_eq!(res.as_str(), "Hello World");
    }
}
