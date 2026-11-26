// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Bounded Native JavaScript AST Lexer, Parser, and Interpreter.
//!
//! Provides a safe, deterministic, `#![no_std]` zero-heap JavaScript subset
//! for interactive DOM scripts and form validation. Strictly bounded by
//! step, memory, and nesting budgets. Unsupported syntax and budget
//! overruns report truthful typed errors.

use crate::layout::dom::{Dom, NodeId, NO_NODE};

/// Maximum variables tracked in the local script execution scope.
pub const MAX_VARS: usize = 64;
/// Maximum AST nodes allocated in preallocated arena per script.
pub const MAX_AST_NODES: usize = 256;
/// Maximum function or call arguments.
pub const MAX_ARGS: usize = 8;
/// Maximum string byte length for variables and literals.
pub const MAX_STR_LEN: usize = 64;
/// Maximum arrays allocated per script execution.
pub const MAX_ARRAYS: usize = 16;
/// Maximum elements per array.
pub const MAX_ARRAY_ELEMS: usize = 16;
/// Maximum functions declared in script scope.
pub const MAX_FUNCTIONS: usize = 16;
/// Default execution step budget before halting with StepBudgetExhausted.
pub const DEFAULT_STEP_BUDGET: usize = 2000;

/// Maximum event listeners registered through `addEventListener` in one run.
///
/// WHY bounded: the runtime is a fixed arena with no heap, so the listener
/// table is a fixed array. A page that registers more than this keeps the
/// first `MAX_LISTENERS` and the rest are refused with `CapacityExceeded`.
pub const MAX_LISTENERS: usize = 16;

/// Bounded JavaScript value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsValue {
    Undefined,
    Null,
    Boolean(bool),
    Number(i64),
    String([u8; MAX_STR_LEN], u8),
    Element(NodeId),
    Array(u8),
    Function(u8),
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
            JsValue::String(buf, len) => core::str::from_utf8(&buf[..*len as usize]).unwrap_or(""),
            JsValue::Boolean(true) => "true",
            JsValue::Boolean(false) => "false",
            JsValue::Null => "null",
            JsValue::Undefined => "undefined",
            JsValue::Array(_) => "[array]",
            JsValue::Function(_) => "[function]",
            _ => "",
        }
    }

    pub fn to_bool(&self) -> bool {
        match self {
            JsValue::Undefined | JsValue::Null => false,
            JsValue::Boolean(b) => *b,
            JsValue::Number(n) => *n != 0,
            JsValue::String(_, len) => *len > 0,
            JsValue::Element(_) | JsValue::Array(_) | JsValue::Function(_) => true,
        }
    }

    pub fn to_number(&self) -> i64 {
        match self {
            JsValue::Undefined | JsValue::Null => 0,
            JsValue::Boolean(b) => {
                if *b {
                    1
                } else {
                    0
                }
            }
            JsValue::Number(n) => *n,
            JsValue::String(buf, len) => {
                let s = core::str::from_utf8(&buf[..*len as usize]).unwrap_or("");
                parse_i64(s.trim()).unwrap_or(0)
            }
            JsValue::Element(_) | JsValue::Array(_) | JsValue::Function(_) => 0,
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
    LBracket,
    RBracket,
    Function,
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
            b';' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::Semi,
                    slice: &self.src[start..self.pos],
                });
            }
            b',' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::Comma,
                    slice: &self.src[start..self.pos],
                });
            }
            b'.' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::Dot,
                    slice: &self.src[start..self.pos],
                });
            }
            b'(' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::LParen,
                    slice: &self.src[start..self.pos],
                });
            }
            b')' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::RParen,
                    slice: &self.src[start..self.pos],
                });
            }
            b'{' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::LBrace,
                    slice: &self.src[start..self.pos],
                });
            }
            b'}' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::RBrace,
                    slice: &self.src[start..self.pos],
                });
            }
            b'[' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::LBracket,
                    slice: &self.src[start..self.pos],
                });
            }
            b']' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::RBracket,
                    slice: &self.src[start..self.pos],
                });
            }
            b'+' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::Plus,
                    slice: &self.src[start..self.pos],
                });
            }
            b'-' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::Minus,
                    slice: &self.src[start..self.pos],
                });
            }
            b'*' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::Star,
                    slice: &self.src[start..self.pos],
                });
            }
            b'/' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::Slash,
                    slice: &self.src[start..self.pos],
                });
            }
            b'%' => {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::Percent,
                    slice: &self.src[start..self.pos],
                });
            }
            b'!' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'=' {
                    self.pos += 2;
                    return Ok(Token {
                        kind: TokenKind::NotEq,
                        slice: &self.src[start..self.pos],
                    });
                }
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::Bang,
                    slice: &self.src[start..self.pos],
                });
            }
            b'=' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'=' {
                    self.pos += 2;
                    return Ok(Token {
                        kind: TokenKind::EqEq,
                        slice: &self.src[start..self.pos],
                    });
                }
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::Eq,
                    slice: &self.src[start..self.pos],
                });
            }
            b'<' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'=' {
                    self.pos += 2;
                    return Ok(Token {
                        kind: TokenKind::LtEq,
                        slice: &self.src[start..self.pos],
                    });
                }
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::Lt,
                    slice: &self.src[start..self.pos],
                });
            }
            b'>' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'=' {
                    self.pos += 2;
                    return Ok(Token {
                        kind: TokenKind::GtEq,
                        slice: &self.src[start..self.pos],
                    });
                }
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::Gt,
                    slice: &self.src[start..self.pos],
                });
            }
            b'&' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'&' {
                    self.pos += 2;
                    return Ok(Token {
                        kind: TokenKind::AmpAmp,
                        slice: &self.src[start..self.pos],
                    });
                }
                return Err(JsError::UnsupportedSyntax("bitwise & operator"));
            }
            b'|' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'|' {
                    self.pos += 2;
                    return Ok(Token {
                        kind: TokenKind::PipePipe,
                        slice: &self.src[start..self.pos],
                    });
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
                return Ok(Token {
                    kind: TokenKind::String,
                    slice: str_slice,
                });
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
            return Ok(Token {
                kind: TokenKind::Number(n),
                slice: num_slice,
            });
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
                b"function" => TokenKind::Function,
                b"true" => TokenKind::True,
                b"false" => TokenKind::False,
                b"null" => TokenKind::Null,
                b"undefined" => TokenKind::Undefined,
                b"async" | b"await" | b"class" | b"import" | b"export" | b"throw" | b"try"
                | b"catch" => {
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
    if bytes.is_empty() {
        return None;
    }
    if bytes[0] == b'-' {
        sign = -1;
        i += 1;
    } else if bytes[0] == b'+' {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    while i < bytes.len() {
        let b = bytes[i];
        if !b.is_ascii_digit() {
            return None;
        }
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
    Binary {
        op: BinOp,
        left: usize,
        right: usize,
    },
    Unary {
        op: UnOp,
        expr: usize,
    },
    Call {
        callee: usize,
        args_start: usize,
        args_count: u8,
    },
    Member {
        object: usize,
        prop: [u8; 32],
        prop_len: u8,
    },
    Index {
        object: usize,
        index: usize,
    },
    Array {
        elements_start: usize,
        elements_count: u8,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum StmtKind {
    VarDecl {
        name: [u8; 32],
        name_len: u8,
        init: Option<usize>,
    },
    Assign {
        target: usize,
        value: usize,
    },
    If {
        cond: usize,
        then_stmt: usize,
        else_stmt: Option<usize>,
    },
    While {
        cond: usize,
        body: usize,
    },
    Expr(usize),
    Block {
        stmts_start: usize,
        stmts_count: usize,
    },
    Return(Option<usize>),
    FunctionDecl {
        name: [u8; 32],
        name_len: u8,
        params_start: usize,
        params_count: u8,
        body: usize,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct AstNode {
    pub expr: Option<ExprKind>,
    pub stmt: Option<StmtKind>,
}

impl AstNode {
    pub const EMPTY: Self = Self {
        expr: None,
        stmt: None,
    };
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
    params_pool: [[u8; 32]; 32],
    params_lens: [u8; 32],
    params_count: usize,
    depth: usize,
}

/// Nesting limit for expression parsing. Each paren/bracket level recurses
/// through `parse_expr`, so nesting depth is bounded well before the fixed
/// node pool could drive the stack past its budget.
const MAX_PARSE_DEPTH: usize = 128;

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
            params_pool: [[0; 32]; 32],
            params_lens: [0; 32],
            params_count: 0,
            depth: 0,
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
        self.nodes[id] = AstNode {
            expr: Some(expr),
            stmt: None,
        };
        self.node_count += 1;
        Ok(id)
    }

    fn alloc_stmt(&mut self, stmt: StmtKind) -> Result<usize, JsError> {
        if self.node_count >= MAX_AST_NODES {
            return Err(JsError::CapacityExceeded);
        }
        let id = self.node_count;
        self.nodes[id] = AstNode {
            expr: None,
            stmt: Some(stmt),
        };
        self.node_count += 1;
        Ok(id)
    }

    pub fn parse_program(&mut self) -> Result<usize, JsError> {
        let mut local = [0usize; 64];
        let mut lcount = 0;
        while self.curr.kind != TokenKind::Eof {
            let stmt = self.parse_stmt()?;
            if lcount >= local.len() {
                return Err(JsError::CapacityExceeded);
            }
            local[lcount] = stmt;
            lcount += 1;
        }
        let start = self.stmts_count;
        if self.stmts_count + lcount > self.stmts_pool.len() {
            return Err(JsError::CapacityExceeded);
        }
        self.stmts_pool[start..start + lcount].copy_from_slice(&local[..lcount]);
        self.stmts_count += lcount;
        self.alloc_stmt(StmtKind::Block {
            stmts_start: start,
            stmts_count: lcount,
        })
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
                self.alloc_stmt(StmtKind::VarDecl {
                    name,
                    name_len: nlen as u8,
                    init,
                })
            }
            TokenKind::If => {
                self.advance()?;
                if self.curr.kind != TokenKind::LParen {
                    return Err(JsError::UnexpectedToken);
                }
                self.advance()?;
                let cond = self.parse_expr()?;
                if self.curr.kind != TokenKind::RParen {
                    return Err(JsError::UnexpectedToken);
                }
                self.advance()?;
                let then_stmt = self.parse_stmt()?;
                let else_stmt = if self.curr.kind == TokenKind::Else {
                    self.advance()?;
                    Some(self.parse_stmt()?)
                } else {
                    None
                };
                self.alloc_stmt(StmtKind::If {
                    cond,
                    then_stmt,
                    else_stmt,
                })
            }
            TokenKind::While => {
                self.advance()?;
                if self.curr.kind != TokenKind::LParen {
                    return Err(JsError::UnexpectedToken);
                }
                self.advance()?;
                let cond = self.parse_expr()?;
                if self.curr.kind != TokenKind::RParen {
                    return Err(JsError::UnexpectedToken);
                }
                self.advance()?;
                let body = self.parse_stmt()?;
                self.alloc_stmt(StmtKind::While { cond, body })
            }
            TokenKind::LBrace => {
                self.advance()?;
                let mut local = [0usize; 32];
                let mut lcount = 0;
                while self.curr.kind != TokenKind::RBrace && self.curr.kind != TokenKind::Eof {
                    let s = self.parse_stmt()?;
                    if lcount >= local.len() {
                        return Err(JsError::CapacityExceeded);
                    }
                    local[lcount] = s;
                    lcount += 1;
                }
                if self.curr.kind != TokenKind::RBrace {
                    return Err(JsError::UnexpectedToken);
                }
                self.advance()?;
                let start = self.stmts_count;
                if self.stmts_count + lcount > self.stmts_pool.len() {
                    return Err(JsError::CapacityExceeded);
                }
                self.stmts_pool[start..start + lcount].copy_from_slice(&local[..lcount]);
                self.stmts_count += lcount;
                self.alloc_stmt(StmtKind::Block {
                    stmts_start: start,
                    stmts_count: lcount,
                })
            }
            TokenKind::Return => {
                self.advance()?;
                let val = if self.curr.kind != TokenKind::Semi
                    && self.curr.kind != TokenKind::RBrace
                    && self.curr.kind != TokenKind::Eof
                {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                if self.curr.kind == TokenKind::Semi {
                    self.advance()?;
                }
                self.alloc_stmt(StmtKind::Return(val))
            }
            TokenKind::Function => {
                self.advance()?;
                let tok = self.advance()?;
                if tok.kind != TokenKind::Ident {
                    return Err(JsError::UnexpectedToken);
                }
                let mut name = [0u8; 32];
                let nlen = tok.slice.len().min(32);
                name[..nlen].copy_from_slice(&tok.slice[..nlen]);

                if self.curr.kind != TokenKind::LParen {
                    return Err(JsError::UnexpectedToken);
                }
                self.advance()?;
                let params_start = self.params_count;
                let mut pcount = 0u8;
                while self.curr.kind != TokenKind::RParen && self.curr.kind != TokenKind::Eof {
                    let ptok = self.advance()?;
                    if ptok.kind != TokenKind::Ident {
                        return Err(JsError::UnexpectedToken);
                    }
                    if self.params_count >= self.params_pool.len() {
                        return Err(JsError::CapacityExceeded);
                    }
                    let mut pname = [0u8; 32];
                    let plen = ptok.slice.len().min(32);
                    pname[..plen].copy_from_slice(&ptok.slice[..plen]);
                    self.params_pool[self.params_count] = pname;
                    self.params_lens[self.params_count] = plen as u8;
                    self.params_count += 1;
                    pcount += 1;
                    if self.curr.kind == TokenKind::Comma {
                        self.advance()?;
                    } else {
                        break;
                    }
                }
                if self.curr.kind != TokenKind::RParen {
                    return Err(JsError::UnexpectedToken);
                }
                self.advance()?;
                if self.curr.kind != TokenKind::LBrace {
                    return Err(JsError::UnexpectedToken);
                }
                let body = self.parse_stmt()?;
                self.alloc_stmt(StmtKind::FunctionDecl {
                    name,
                    name_len: nlen as u8,
                    params_start,
                    params_count: pcount,
                    body,
                })
            }
            _ => {
                let expr = self.parse_expr()?;
                if self.curr.kind == TokenKind::Eq {
                    self.advance()?;
                    let val = self.parse_expr()?;
                    if self.curr.kind == TokenKind::Semi {
                        self.advance()?;
                    }
                    self.alloc_stmt(StmtKind::Assign {
                        target: expr,
                        value: val,
                    })
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
            left = self.alloc_expr(ExprKind::Binary {
                op: BinOp::Or,
                left,
                right,
            })?;
        }
        Ok(left)
    }

    fn parse_logical_and(&mut self) -> Result<usize, JsError> {
        let mut left = self.parse_equality()?;
        while self.curr.kind == TokenKind::AmpAmp {
            self.advance()?;
            let right = self.parse_equality()?;
            left = self.alloc_expr(ExprKind::Binary {
                op: BinOp::And,
                left,
                right,
            })?;
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<usize, JsError> {
        let mut left = self.parse_relational()?;
        while self.curr.kind == TokenKind::EqEq || self.curr.kind == TokenKind::NotEq {
            let op = if self.curr.kind == TokenKind::EqEq {
                BinOp::EqEq
            } else {
                BinOp::NotEq
            };
            self.advance()?;
            let right = self.parse_relational()?;
            left = self.alloc_expr(ExprKind::Binary { op, left, right })?;
        }
        Ok(left)
    }

    fn parse_relational(&mut self) -> Result<usize, JsError> {
        let mut left = self.parse_additive()?;
        while matches!(
            self.curr.kind,
            TokenKind::Lt | TokenKind::LtEq | TokenKind::Gt | TokenKind::GtEq
        ) {
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
            let op = if self.curr.kind == TokenKind::Plus {
                BinOp::Add
            } else {
                BinOp::Sub
            };
            self.advance()?;
            let right = self.parse_multiplicative()?;
            left = self.alloc_expr(ExprKind::Binary { op, left, right })?;
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<usize, JsError> {
        let mut left = self.parse_unary()?;
        while matches!(
            self.curr.kind,
            TokenKind::Star | TokenKind::Slash | TokenKind::Percent
        ) {
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
            self.enter_nested()?;
            self.advance()?;
            let expr = self.parse_unary()?;
            self.depth -= 1;
            return self.alloc_expr(ExprKind::Unary {
                op: UnOp::Not,
                expr,
            });
        }
        if self.curr.kind == TokenKind::Minus {
            self.enter_nested()?;
            self.advance()?;
            let expr = self.parse_unary()?;
            self.depth -= 1;
            return self.alloc_expr(ExprKind::Unary {
                op: UnOp::Neg,
                expr,
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<usize, JsError> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.curr.kind == TokenKind::Dot {
                self.advance()?;
                let tok = self.advance()?;
                if tok.kind != TokenKind::Ident {
                    return Err(JsError::UnexpectedToken);
                }
                let mut prop = [0u8; 32];
                let plen = tok.slice.len().min(32);
                prop[..plen].copy_from_slice(&tok.slice[..plen]);
                expr = self.alloc_expr(ExprKind::Member {
                    object: expr,
                    prop,
                    prop_len: plen as u8,
                })?;
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
                if self.curr.kind != TokenKind::RParen {
                    return Err(JsError::UnexpectedToken);
                }
                self.advance()?;
                expr = self.alloc_expr(ExprKind::Call {
                    callee: expr,
                    args_start,
                    args_count: count,
                })?;
            } else if self.curr.kind == TokenKind::LBracket {
                self.advance()?;
                let index = self.parse_expr()?;
                if self.curr.kind != TokenKind::RBracket {
                    return Err(JsError::UnexpectedToken);
                }
                self.advance()?;
                expr = self.alloc_expr(ExprKind::Index {
                    object: expr,
                    index,
                })?;
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
                self.enter_nested()?;
                self.advance()?;
                let expr = self.parse_expr()?;
                if self.curr.kind != TokenKind::RParen {
                    self.depth -= 1;
                    return Err(JsError::UnexpectedToken);
                }
                self.advance()?;
                self.depth -= 1;
                Ok(expr)
            }
            TokenKind::LBracket => {
                self.enter_nested()?;
                self.advance()?;
                let start = self.args_count;
                let mut count = 0u8;
                while self.curr.kind != TokenKind::RBracket && self.curr.kind != TokenKind::Eof {
                    let elem = self.parse_expr()?;
                    if self.args_count >= self.args_pool.len() || count >= MAX_ARGS as u8 {
                        return Err(JsError::CapacityExceeded);
                    }
                    self.args_pool[self.args_count] = elem;
                    self.args_count += 1;
                    count += 1;
                    if self.curr.kind == TokenKind::Comma {
                        self.advance()?;
                    } else {
                        break;
                    }
                }
                if self.curr.kind != TokenKind::RBracket {
                    self.depth -= 1;
                    return Err(JsError::UnexpectedToken);
                }
                self.advance()?;
                self.depth -= 1;
                self.alloc_expr(ExprKind::Array {
                    elements_start: start,
                    elements_count: count,
                })
            }
            _ => Err(JsError::UnexpectedToken),
        }
    }

    fn enter_nested(&mut self) -> Result<(), JsError> {
        self.depth += 1;
        if self.depth > MAX_PARSE_DEPTH {
            return Err(JsError::CapacityExceeded);
        }
        Ok(())
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

#[derive(Debug, Clone, Copy)]
pub struct JsFunctionDef {
    pub name: [u8; 32],
    pub name_len: u8,
    pub params_start: usize,
    pub params_count: u8,
    pub body: usize,
}

/// One `addEventListener` registration made during a script run.
///
/// WHY the handler is a function id and not source: the interpreter keeps
/// function bodies in the per-run arena, so a listener is a reference into
/// that arena. Listeners live for the run in which they were registered and
/// are dispatched by [`JsRuntime::dispatch_event`] before that run ends; they
/// are not persisted to a later user gesture (inline `on*` attributes cover
/// that path).
#[derive(Debug, Clone, Copy)]
struct JsListener {
    node: NodeId,
    event: [u8; 16],
    event_len: u8,
    fn_id: u8,
}

/// Runtime Execution Environment.
pub struct JsRuntime<'a> {
    vars: [JsVar; MAX_VARS],
    var_count: usize,
    arrays: [[JsValue; MAX_ARRAY_ELEMS]; MAX_ARRAYS],
    array_lens: [u8; MAX_ARRAYS],
    array_count: usize,
    functions: [JsFunctionDef; MAX_FUNCTIONS],
    function_count: usize,
    listeners: [JsListener; MAX_LISTENERS],
    listener_count: usize,
    call_depth: usize,
    dom: &'a mut Dom,
    remaining_steps: usize,
    mutated: bool,
}

impl<'a> JsRuntime<'a> {
    pub fn new(dom: &'a mut Dom, step_budget: usize) -> Self {
        Self {
            vars: [JsVar {
                name: [0; 32],
                name_len: 0,
                val: JsValue::Undefined,
            }; MAX_VARS],
            var_count: 0,
            arrays: [[JsValue::Undefined; MAX_ARRAY_ELEMS]; MAX_ARRAYS],
            array_lens: [0; MAX_ARRAYS],
            array_count: 0,
            functions: [JsFunctionDef {
                name: [0; 32],
                name_len: 0,
                params_start: 0,
                params_count: 0,
                body: 0,
            }; MAX_FUNCTIONS],
            function_count: 0,
            listeners: [JsListener {
                node: NO_NODE,
                event: [0; 16],
                event_len: 0,
                fn_id: 0,
            }; MAX_LISTENERS],
            listener_count: 0,
            call_depth: 0,
            dom,
            remaining_steps: step_budget,
            mutated: false,
        }
    }

    pub fn dom_mutated(&self) -> bool {
        self.mutated
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
            StmtKind::VarDecl {
                name,
                name_len,
                init,
            } => {
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
                    Some(ExprKind::Member {
                        object,
                        prop,
                        prop_len,
                    }) => {
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
            StmtKind::If {
                cond,
                then_stmt,
                else_stmt,
            } => {
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
                        StmtResult::Normal(v) => {
                            last = v;
                        }
                    }
                }
                Ok(StmtResult::Normal(last))
            }
            StmtKind::Block {
                stmts_start,
                stmts_count,
            } => {
                let mut last = JsValue::Undefined;
                for i in 0..*stmts_count {
                    let sid = parser.stmts_pool[*stmts_start + i];
                    match self.eval_stmt(parser, sid)? {
                        StmtResult::Return(v) => return Ok(StmtResult::Return(v)),
                        StmtResult::Normal(v) => {
                            last = v;
                        }
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
            StmtKind::FunctionDecl {
                name,
                name_len,
                params_start,
                params_count,
                body,
            } => {
                if self.function_count >= MAX_FUNCTIONS {
                    return Err(JsError::CapacityExceeded);
                }
                let fn_id = self.function_count;
                self.functions[fn_id] = JsFunctionDef {
                    name: *name,
                    name_len: *name_len,
                    params_start: *params_start,
                    params_count: *params_count,
                    body: *body,
                };
                self.function_count += 1;
                self.set_var(&name[..*name_len as usize], JsValue::Function(fn_id as u8))?;
                Ok(StmtResult::Normal(JsValue::Function(fn_id as u8)))
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
                        if rn == 0 {
                            Ok(JsValue::Number(0))
                        } else {
                            Ok(JsValue::Number(l.to_number() / rn))
                        }
                    }
                    BinOp::Mod => {
                        let rn = r.to_number();
                        if rn == 0 {
                            Ok(JsValue::Number(0))
                        } else {
                            Ok(JsValue::Number(l.to_number() % rn))
                        }
                    }
                    BinOp::EqEq => match (l, r) {
                        (JsValue::Number(a), JsValue::Number(b)) => Ok(JsValue::Boolean(a == b)),
                        (JsValue::Boolean(a), JsValue::Boolean(b)) => Ok(JsValue::Boolean(a == b)),
                        (JsValue::String(a, la), JsValue::String(b, lb)) => Ok(JsValue::Boolean(
                            la == lb && a[..la as usize] == b[..lb as usize],
                        )),
                        (JsValue::Null, JsValue::Null)
                        | (JsValue::Undefined, JsValue::Undefined) => Ok(JsValue::Boolean(true)),
                        _ => Ok(JsValue::Boolean(false)),
                    },
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
            ExprKind::Member {
                object,
                prop,
                prop_len,
            } => {
                let obj = self.eval_expr(parser, *object)?;
                let prop_slice = &prop[..*prop_len as usize];
                match obj {
                    JsValue::Element(node_id) => self.get_element_prop(node_id, prop_slice),
                    JsValue::Array(aid) => {
                        if prop_slice == b"length" {
                            let aid_usize = aid as usize;
                            if aid_usize < self.array_count {
                                Ok(JsValue::Number(self.array_lens[aid_usize] as i64))
                            } else {
                                Ok(JsValue::Number(0))
                            }
                        } else {
                            Err(JsError::UnsupportedSyntax("unsupported array property"))
                        }
                    }
                    JsValue::String(_, len) => {
                        if prop_slice == b"length" {
                            Ok(JsValue::Number(len as i64))
                        } else {
                            Err(JsError::UnsupportedSyntax("unsupported string property"))
                        }
                    }
                    _ => Err(JsError::TypeError),
                }
            }
            ExprKind::Call {
                callee,
                args_start,
                args_count,
            } => {
                let callee_node = parser.nodes.get(*callee).ok_or(JsError::UnexpectedToken)?;
                match callee_node.expr.as_ref() {
                    Some(ExprKind::Member {
                        object,
                        prop,
                        prop_len,
                    }) => {
                        let obj = self.eval_expr(parser, *object)?;
                        let method = &prop[..*prop_len as usize];

                        // Method: document.getElementById(id_str)
                        if method == b"getElementById" && *args_count >= 1 {
                            let arg_id = parser.args_pool[*args_start];
                            let arg_val = self.eval_expr(parser, arg_id)?;
                            let target_id = arg_val.as_str().as_bytes();
                            let found = self.find_element_by_id(target_id);
                            return Ok(found.map(JsValue::Element).unwrap_or(JsValue::Null));
                        }

                        // Method: document.getElementsByTagName(tag_str)
                        // Returns an array of matching element nodes (empty
                        // when none match), bounded by the array arena.
                        if method == b"getElementsByTagName" && *args_count >= 1 {
                            let arg_id = parser.args_pool[*args_start];
                            let arg_val = self.eval_expr(parser, arg_id)?;
                            let tag = arg_val.as_str().as_bytes();
                            if self.array_count >= MAX_ARRAYS {
                                return Err(JsError::CapacityExceeded);
                            }
                            let aid = self.array_count;
                            let mut count = 0usize;
                            let n = self.dom.node_count();
                            for id in 0..n {
                                let nid = id as NodeId;
                                if self.dom.node(nid).is_element()
                                    && self.dom.tag(nid).eq_ignore_ascii_case(tag)
                                {
                                    if count >= MAX_ARRAY_ELEMS {
                                        break;
                                    }
                                    self.arrays[aid][count] = JsValue::Element(nid);
                                    count += 1;
                                }
                            }
                            self.array_lens[aid] = count as u8;
                            self.array_count += 1;
                            return Ok(JsValue::Array(aid as u8));
                        }

                        // Method: document.createElement(tag_str)
                        if method == b"createElement" && *args_count >= 1 {
                            if !matches!(obj, JsValue::Element(_)) {
                                return Err(JsError::TypeError);
                            }
                            let arg_id = parser.args_pool[*args_start];
                            let arg_val = self.eval_expr(parser, arg_id)?;
                            let tag = arg_val.as_str().as_bytes();
                            return match self.dom.create_element(tag) {
                                Some(node) => {
                                    self.mutated = true;
                                    Ok(JsValue::Element(node))
                                }
                                None => Err(JsError::CapacityExceeded),
                            };
                        }

                        // Method: element.appendChild(child_element)
                        if method == b"appendChild" && *args_count >= 1 {
                            let parent = match obj {
                                JsValue::Element(n) => n,
                                _ => return Err(JsError::TypeError),
                            };
                            let arg_id = parser.args_pool[*args_start];
                            let arg_val = self.eval_expr(parser, arg_id)?;
                            let child = match arg_val {
                                JsValue::Element(n) => n,
                                _ => return Err(JsError::TypeError),
                            };
                            if self.dom.append_child(parent, child) {
                                self.mutated = true;
                                return Ok(JsValue::Element(child));
                            } else {
                                return Err(JsError::TypeError);
                            }
                        }

                        // Method: element.removeChild(child_element)
                        if method == b"removeChild" && *args_count >= 1 {
                            let parent = match obj {
                                JsValue::Element(n) => n,
                                _ => return Err(JsError::TypeError),
                            };
                            let arg_id = parser.args_pool[*args_start];
                            let arg_val = self.eval_expr(parser, arg_id)?;
                            let child = match arg_val {
                                JsValue::Element(n) => n,
                                _ => return Err(JsError::TypeError),
                            };
                            if self.dom.remove_child(parent, child) {
                                self.mutated = true;
                                return Ok(JsValue::Element(child));
                            } else {
                                return Err(JsError::TypeError);
                            }
                        }

                        // Method: element.addEventListener(event, handler)
                        // Registers a function-valued handler for a named event
                        // on this element. The handler runs when the runtime
                        // dispatches that event (see `dispatch_event`).
                        if method == b"addEventListener" && *args_count >= 2 {
                            let node = match obj {
                                JsValue::Element(n) => n,
                                _ => return Err(JsError::TypeError),
                            };
                            let ev_id = parser.args_pool[*args_start];
                            let ev_val = self.eval_expr(parser, ev_id)?;
                            let ev_bytes = ev_val.as_str().as_bytes();
                            let fn_id = parser.args_pool[*args_start + 1];
                            let fn_val = self.eval_expr(parser, fn_id)?;
                            let handler = match fn_val {
                                JsValue::Function(f) => f,
                                _ => return Err(JsError::TypeError),
                            };
                            if self.listener_count >= MAX_LISTENERS {
                                return Err(JsError::CapacityExceeded);
                            }
                            let mut event = [0u8; 16];
                            let elen = ev_bytes.len().min(event.len());
                            event[..elen].copy_from_slice(&ev_bytes[..elen]);
                            self.listeners[self.listener_count] = JsListener {
                                node,
                                event,
                                event_len: elen as u8,
                                fn_id: handler,
                            };
                            self.listener_count += 1;
                            return Ok(JsValue::Undefined);
                        }

                        // Method: console.log(...)
                        if method == b"log" {
                            return Ok(JsValue::Undefined);
                        }

                        Err(JsError::UnsupportedSyntax("unsupported object method"))
                    }
                    _ => {
                        let callee_val = self.eval_expr(parser, *callee)?;
                        if let JsValue::Function(fn_id) = callee_val {
                            let mut evaled_args = [JsValue::Undefined; MAX_ARGS];
                            let arg_take = (*args_count as usize).min(MAX_ARGS);
                            for i in 0..arg_take {
                                let arg_expr = parser.args_pool[*args_start + i];
                                evaled_args[i] = self.eval_expr(parser, arg_expr)?;
                            }
                            self.invoke_function(parser, fn_id, &evaled_args[..arg_take])
                        } else {
                            Err(JsError::UnsupportedSyntax("unsupported function call"))
                        }
                    }
                }
            }
            ExprKind::Index { object, index } => {
                let obj = self.eval_expr(parser, *object)?;
                let idx_val = self.eval_expr(parser, *index)?;
                let i = idx_val.to_number();
                match obj {
                    JsValue::Array(aid) => {
                        let aid_usize = aid as usize;
                        if aid_usize < self.array_count
                            && i >= 0
                            && (i as usize) < self.array_lens[aid_usize] as usize
                        {
                            Ok(self.arrays[aid_usize][i as usize])
                        } else {
                            Ok(JsValue::Undefined)
                        }
                    }
                    JsValue::String(buf, len) => {
                        if i >= 0 && (i as usize) < len as usize {
                            let pos = i as usize;
                            Ok(JsValue::string_from_slice(&buf[pos..pos + 1]))
                        } else {
                            Ok(JsValue::Undefined)
                        }
                    }
                    _ => Err(JsError::TypeError),
                }
            }
            ExprKind::Array {
                elements_start,
                elements_count,
            } => {
                if self.array_count >= MAX_ARRAYS {
                    return Err(JsError::CapacityExceeded);
                }
                let aid = self.array_count;
                let count = (*elements_count as usize).min(MAX_ARRAY_ELEMS);
                for i in 0..count {
                    let elem_id = parser.args_pool[*elements_start + i];
                    let val = self.eval_expr(parser, elem_id)?;
                    self.arrays[aid][i] = val;
                }
                self.array_lens[aid] = count as u8;
                self.array_count += 1;
                Ok(JsValue::Array(aid as u8))
            }
        }
    }

    /// Invokes a user function by id with already-evaluated arguments.
    ///
    /// WHY a helper: both a call expression and an event dispatch need to
    /// enter a function body, and the body sets and restores the variable
    /// scope and the call-depth guard. Keeping one copy means a listener
    /// runs under exactly the same budgets as an ordinary call.
    fn invoke_function(
        &mut self,
        parser: &Parser,
        fn_id: u8,
        evaled_args: &[JsValue],
    ) -> Result<JsValue, JsError> {
        let fn_idx = fn_id as usize;
        if fn_idx >= self.function_count {
            return Err(JsError::ReferenceError);
        }
        if self.call_depth >= 8 {
            return Err(JsError::CapacityExceeded);
        }
        self.call_depth += 1;
        let fndef = self.functions[fn_idx];
        let saved_var_count = self.var_count;
        let arg_take = evaled_args.len().min(MAX_ARGS);

        for i in 0..fndef.params_count as usize {
            let pidx = fndef.params_start + i;
            let pname = &parser.params_pool[pidx][..parser.params_lens[pidx] as usize];
            let pval = if i < arg_take {
                evaled_args[i]
            } else {
                JsValue::Undefined
            };
            self.set_var(pname, pval)?;
        }

        let res = self.eval_stmt(parser, fndef.body);
        self.var_count = saved_var_count;
        self.call_depth -= 1;

        match res? {
            StmtResult::Return(v) | StmtResult::Normal(v) => Ok(v),
        }
    }

    /// Dispatches `event` to every listener registered for that name.
    ///
    /// The matching function ids are gathered first so the mutable borrow
    /// taken by [`JsRuntime::invoke_function`] does not overlap the listener
    /// table read. A handler that overruns its budget aborts the dispatch
    /// with the same error an ordinary call would report.
    fn dispatch_event(&mut self, parser: &Parser, event: &[u8]) -> Result<(), JsError> {
        let mut ids = [0u8; MAX_LISTENERS];
        let mut count = 0usize;
        for i in 0..self.listener_count {
            let listener = self.listeners[i];
            let len = listener.event_len as usize;
            if len == event.len() && &listener.event[..len] == event {
                if count < ids.len() {
                    ids[count] = listener.fn_id;
                    count += 1;
                }
            }
        }
        for i in 0..count {
            self.invoke_function(parser, ids[i], &[])?;
        }
        Ok(())
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
            b"checked" => Ok(JsValue::Boolean(self.dom.has_attr(node_id, b"checked"))),
            b"disabled" => Ok(JsValue::Boolean(self.dom.has_attr(node_id, b"disabled"))),
            b"body" => {
                // The first <body> element, or the document root's first
                // element child when the page never named one, or Null when
                // the tree holds no element at all.
                let n = self.dom.node_count();
                for i in 0..n {
                    let nid = i as NodeId;
                    if self.dom.node(nid).is_element()
                        && self.dom.tag(nid).eq_ignore_ascii_case(b"body")
                    {
                        return Ok(JsValue::Element(nid));
                    }
                }
                let root = self.dom.root();
                match self.dom.first_child(root) {
                    Some(child) => Ok(JsValue::Element(child)),
                    None => Ok(JsValue::Null),
                }
            }
            _ => Err(JsError::UnsupportedSyntax(
                "unsupported DOM property getter",
            )),
        }
    }

    fn set_element_prop(
        &mut self,
        node_id: NodeId,
        prop: &[u8],
        val: JsValue,
    ) -> Result<(), JsError> {
        match prop {
            b"value" => {
                let bytes = val.as_str().as_bytes();
                self.dom.set_attr(node_id, b"value", bytes);
                self.mutated = true;
                Ok(())
            }
            b"checked" => {
                if val.to_bool() {
                    self.dom.set_attr(node_id, b"checked", b"checked");
                }
                self.mutated = true;
                Ok(())
            }
            b"textContent" | b"innerText" => {
                let bytes = val.as_str().as_bytes();
                self.dom.set_text(node_id, bytes);
                self.mutated = true;
                Ok(())
            }
            b"id" => {
                // Assigning `id` must store the attribute so getElementById can
                // resolve the node (used by script-built UI and event handlers).
                let bytes = val.as_str().as_bytes();
                self.dom.set_attr(node_id, b"id", bytes);
                self.mutated = true;
                Ok(())
            }
            _ => Err(JsError::UnsupportedSyntax(
                "unsupported DOM property setter",
            )),
        }
    }
}

/// Executes an inline JavaScript snippet against the browser's DOM and returns whether DOM was mutated.
pub fn execute_script_with_mutations(
    script: &[u8],
    dom: &mut Dom,
    step_budget: usize,
) -> Result<(JsValue, bool), JsError> {
    let lexer = Lexer::new(script);
    let mut parser = Parser::new(lexer)?;
    let root = parser.parse_program()?;
    let mut runtime = JsRuntime::new(dom, step_budget);
    let val = runtime.eval_program(&parser, root)?;
    Ok((val, runtime.dom_mutated()))
}

/// Executes an inline JavaScript snippet against the browser's DOM.
pub fn execute_script(
    script: &[u8],
    dom: &mut Dom,
    step_budget: usize,
) -> Result<JsValue, JsError> {
    execute_script_with_mutations(script, dom, step_budget).map(|(v, _)| v)
}

/// Executes a script, then dispatches `event` to listeners it registered.
///
/// WHY this exists beside `execute_script_with_mutations`: a page that wires
/// itself up with `addEventListener('DOMContentLoaded', ...)` does nothing
/// until that event is delivered. Running the program and then dispatching
/// the named event inside the same runtime is what makes such a page build
/// its content during load. Returns the program value and whether the DOM
/// was mutated (by the program or by a listener).
pub fn execute_script_with_events(
    script: &[u8],
    dom: &mut Dom,
    step_budget: usize,
    event: &[u8],
) -> Result<(JsValue, bool), JsError> {
    let lexer = Lexer::new(script);
    let mut parser = Parser::new(lexer)?;
    let root = parser.parse_program()?;
    let mut runtime = JsRuntime::new(dom, step_budget);
    let val = runtime.eval_program(&parser, root)?;
    runtime.dispatch_event(&parser, event)?;
    Ok((val, runtime.dom_mutated()))
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
    fn test_deeply_nested_expression_is_bounded_not_stack_exhaustion() {
        // A hostile page nesting 20_000 parens deep used to drive unbounded
        // parse_expr recursion (stack exhaustion); it must be refused through
        // the parse-depth budget instead of crashing the parser.
        let mut src = [0u8; 40_002];
        let half = src.len() / 2;
        for b in src[..half].iter_mut() {
            *b = b'(';
        }
        src[half] = b'1';
        for b in src[half + 1..].iter_mut() {
            *b = b')';
        }
        let mut dom = Dom::new();
        let err = execute_script(&src, &mut dom, usize::MAX).expect_err("deep nesting refused");
        assert_eq!(err, JsError::CapacityExceeded);
    }

    #[test]
    fn test_prefix_unary_recursion_is_bounded() {
        // `!!!!...!x` recursion: each unary prefix must count against the
        // nesting budget rather than recursing per-operator unbounded.
        let mut src = [0u8; 20_002];
        let bangs = src.len() - 2;
        for b in src[..bangs].iter_mut() {
            *b = b'!';
        }
        src[bangs] = b'1';
        src[bangs + 1] = b';';
        let mut dom = Dom::new();
        let err = execute_script(&src, &mut dom, usize::MAX).expect_err("unary chain refused");
        assert_eq!(err, JsError::CapacityExceeded);
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
        assert_eq!(res, JsValue::string_from_slice(b"alice"));
    }

    #[test]
    fn test_dom_set_element_property_mutation() {
        let mut dom = Dom::new();
        dom.parse(
            b"<div><input id=\"username\" value=\"alice\"></input><p id=\"status\">idle</p></div>",
        );
        assert!(dom.node_count() > 0);

        let src = b"let input = document.getElementById('username'); input.value = 'bob';";
        let res = execute_script(src, &mut dom, 1000).expect("mutation succeeds");
        let _ = res;

        // Verify mutated DOM attributes
        let mut found_input = false;
        for i in 0..dom.node_count() {
            let id = i as u16;
            if dom.attr(id, b"id") == Some(b"username") {
                assert_eq!(dom.attr(id, b"value"), Some(&b"bob"[..]));
                found_input = true;
            }
        }
        assert!(found_input);
    }

    #[test]
    fn test_string_concatenation() {
        let mut dom = Dom::new();
        let src = b"let greeting = 'Hello ' + 'World';";
        let res = execute_script(src, &mut dom, 1000).expect("string cat succeeds");
        assert_eq!(res.as_str(), "Hello World");
    }

    #[test]
    fn test_function_declaration_and_call() {
        let mut dom = Dom::new();
        let src = b"function add(x, y) { return x + y; } let sum = add(15, 27);";
        let res = execute_script(src, &mut dom, 1000).expect("function execution succeeds");
        assert_eq!(res, JsValue::Number(42));
    }

    #[test]
    fn test_array_literal_indexing_and_length() {
        let mut dom = Dom::new();
        let src = b"let arr = [10, 20, 30]; let first = arr[0]; let len = arr.length; let sum = first + len;";
        let res = execute_script(src, &mut dom, 1000).expect("array operations succeed");
        assert_eq!(res, JsValue::Number(13));
    }

    #[test]
    fn test_mutation_detection() {
        let mut dom = Dom::new();
        dom.parse(b"<div><p id=\"msg\">hello</p></div>");
        let src = b"let p = document.getElementById('msg'); p.textContent = 'world';";
        let (res, mutated) =
            execute_script_with_mutations(src, &mut dom, 1000).expect("script succeeds");
        let _ = res;
        assert!(mutated);
    }

    #[test]
    fn test_create_element_and_append_child_grow_dom_with_text() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body></body></html>");
        let before = dom.node_count();

        let src = b"var d=document.createElement('p'); d.textContent='hi'; document.body.appendChild(d);";
        let (_res, mutated) =
            execute_script_with_mutations(src, &mut dom, 1000).expect("DOM construction succeeds");
        assert!(mutated, "building nodes must report a DOM mutation");

        // The element and its text child were really allocated.
        assert!(
            dom.node_count() > before,
            "node_count must grow after createElement + textContent"
        );

        // The assigned text 'hi' is present in a text node in the tree.
        let mut found_text = false;
        for i in 0..dom.node_count() {
            let nid = i as NodeId;
            if dom.node(nid).is_text() && dom.text_of(nid) == b"hi" {
                found_text = true;
            }
        }
        assert!(found_text, "the assigned text 'hi' must be present in the DOM");

        // The created <p> is a child of <body>.
        let mut body = None;
        for i in 0..dom.node_count() {
            let nid = i as NodeId;
            if dom.node(nid).is_element() && dom.tag(nid).eq_ignore_ascii_case(b"body") {
                body = Some(nid);
            }
        }
        let body = body.expect("skeleton DOM has a body");
        let mut p_is_child = false;
        let mut child = dom.first_child(body);
        while let Some(c) = child {
            if dom.node(c).is_element() && dom.tag(c).eq_ignore_ascii_case(b"p") {
                p_is_child = true;
            }
            child = dom.next_sibling(c);
        }
        assert!(p_is_child, "the created <p> must be appended under <body>");
    }

    #[test]
    fn test_add_event_listener_runs_on_dispatch_and_mutates() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body></body></html>");
        let before = dom.node_count();

        // A function is registered for DOMContentLoaded; the dispatch at the
        // end of the run must call it, and the handler builds a node.
        let src = b"function boot() { var s = document.createElement('p'); s.textContent = 'BOOTED'; document.body.appendChild(s); } document.addEventListener('DOMContentLoaded', boot);";
        let (_res, mutated) =
            execute_script_with_events(src, &mut dom, 1000, b"DOMContentLoaded")
                .expect("listener dispatch succeeds");
        assert!(mutated, "a listener that builds a node must report a mutation");
        assert!(
            dom.node_count() > before,
            "the dispatched listener must allocate the element and its text"
        );

        let mut found = false;
        for i in 0..dom.node_count() {
            let nid = i as NodeId;
            if dom.node(nid).is_text() && dom.text_of(nid) == b"BOOTED" {
                found = true;
            }
        }
        assert!(found, "the listener's text 'BOOTED' must be in the DOM");
    }

    #[test]
    fn test_event_dispatch_only_fires_matching_listeners() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body></body></html>");
        let before = dom.node_count();

        // Registered for 'click', but the dispatched event is 'load': the
        // handler must NOT run, so the DOM is unchanged.
        let src = b"function onClick() { var s = document.createElement('p'); s.textContent = 'CLICKED'; document.body.appendChild(s); } document.addEventListener('click', onClick);";
        let (_res, mutated) =
            execute_script_with_events(src, &mut dom, 1000, b"load")
                .expect("non-matching dispatch succeeds");
        assert!(!mutated, "a non-matching event must not run the handler");
        assert_eq!(
            dom.node_count(),
            before,
            "no node may be added when the event name does not match"
        );
    }
}
