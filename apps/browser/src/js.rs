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

/// Maximum event objects alive at once in one run.
///
/// WHY bounded: an event object is handed to a handler, and a handler may
/// dispatch another event. The runtime keeps the live events in a fixed
/// stack, and a page that nests deeper than this is refused with
/// `CapacityExceeded` rather than losing track of an event.
pub const MAX_EVENTS: usize = 4;

/// Maximum event names one event object carries.
pub const MAX_EVENT_NAME_LEN: usize = 16;

/// Maximum timers one script run may schedule.
///
/// WHY bounded: the timer table is a fixed arena with no heap, so a page
/// that schedules more than this is refused with `CapacityExceeded` rather
/// than losing a timer it was told was accepted.
pub const MAX_TIMERS: usize = 16;

/// The longest delay a timer may ask for.
///
/// WHY a bound: a page that asks for a timer years away would keep the
/// entry for years. The delay is cut to this bound, and the cut is reported
/// as the delay the timer was given.
pub const MAX_TIMER_DELAY_MS: u64 = 60_000;

/// One timer a page scheduled and the browser has not run yet.
///
/// WHY the function is named rather than numbered: the runtime's function
/// arena belongs to the run that scheduled the timer. A timer that outlives
/// its run is entered later through the page's own program, and the name is
/// how the page's program is asked for that function again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScheduledTimer {
    pub id: u16,
    pub function: [u8; 32],
    pub function_len: u8,
    pub delay_ms: u64,
}

/// One timer inside a run.
#[derive(Debug, Clone, Copy)]
struct JsTimer {
    id: u16,
    fn_id: u8,
    delay_ms: u64,
    name: [u8; 32],
    name_len: u8,
    cancelled: bool,
}

/// What one script run left behind.
///
/// WHY the timers are reported: a timer with a delay outlives the run that
/// scheduled it, so the caller has to learn which callbacks are waiting and
/// how long they asked to wait.
#[derive(Debug, Clone, Copy)]
pub struct ScriptRun {
    pub value: JsValue,
    pub mutated: bool,
    pub default_prevented: bool,
    pub timers: [ScheduledTimer; MAX_TIMERS],
    pub timer_count: usize,
}

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
    /// The event object handed to a listener.
    ///
    /// WHY an index and not a pointer: the runtime is a fixed arena, so an
    /// event is a slot in the event stack and the value names the slot.
    Event(u8),
    /// The page's own storage: zero for the origin's lasting storage, one
    /// for the tab's session storage.
    Storage(u8),
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
            JsValue::Event(_) => "[event]",
            _ => "",
        }
    }

    pub fn to_bool(&self) -> bool {
        match self {
            JsValue::Undefined | JsValue::Null => false,
            JsValue::Boolean(b) => *b,
            JsValue::Number(n) => *n != 0,
            JsValue::String(_, len) => *len > 0,
            JsValue::Element(_) | JsValue::Array(_) | JsValue::Function(_) | JsValue::Event(_) | JsValue::Storage(_) => true,
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
            JsValue::Element(_) | JsValue::Array(_) | JsValue::Function(_) | JsValue::Event(_) | JsValue::Storage(_) => 0,
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
    /// Whether this listener takes part in the capture phase.
    ///
    /// WHY it is stored and not derived: `addEventListener` names the phase
    /// when the listener is registered, and the phase decides whether the
    /// listener runs while the event travels down to the target or while it
    /// travels back up.
    capture: bool,
}

/// The state one live event carries while it is being dispatched.
///
/// WHY this is a table and not a field: a handler may dispatch another
/// event, so the events in flight are a stack. Each slot says whether the
/// event has been stopped and whether its default action was prevented.
#[derive(Debug, Clone, Copy)]
struct JsEventState {
    name: [u8; MAX_EVENT_NAME_LEN],
    name_len: u8,
    propagation_stopped: bool,
    immediate_stopped: bool,
    default_prevented: bool,
}

impl JsEventState {
    const EMPTY: Self = Self {
        name: [0; MAX_EVENT_NAME_LEN],
        name_len: 0,
        propagation_stopped: false,
        immediate_stopped: false,
        default_prevented: false,
    };
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
    events: [JsEventState; MAX_EVENTS],
    event_depth: usize,
    /// The timers this run scheduled.
    timers: [JsTimer; MAX_TIMERS],
    timer_count: usize,
    next_timer_id: u16,
    /// The storage the page's scripts may reach, when the browser attached
    /// one.
    ///
    /// WHY it is optional: a runtime with no storage attached has no
    /// `localStorage` at all, and a page that uses one is told the store is
    /// unavailable rather than being handed an empty store that looks like a
    /// store that lost its contents.
    storage: Option<&'a mut crate::WebStorage>,
    /// The origin the page's storage belongs to.
    origin: [u8; crate::MAX_STORAGE_ORIGIN_LEN],
    origin_len: u8,
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
                capture: false,
            }; MAX_LISTENERS],
            listener_count: 0,
            events: [JsEventState::EMPTY; MAX_EVENTS],
            event_depth: 0,
            timers: [JsTimer {
                id: 0,
                fn_id: 0,
                delay_ms: 0,
                name: [0; 32],
                name_len: 0,
                cancelled: false,
            }; MAX_TIMERS],
            timer_count: 0,
            next_timer_id: 1,
            storage: None,
            origin: [0; crate::MAX_STORAGE_ORIGIN_LEN],
            origin_len: 0,
            call_depth: 0,
            dom,
            remaining_steps: step_budget,
            mutated: false,
        }
    }

    pub fn dom_mutated(&self) -> bool {
        self.mutated
    }

    /// Builds a runtime whose page may reach `storage` under `origin`.
    ///
    /// WHY the origin travels with the storage: the page's scripts are only
    /// given the entries their own origin wrote. The runtime never learns
    /// another origin's name, so a page cannot ask for one.
    pub fn with_storage(
        dom: &'a mut Dom,
        step_budget: usize,
        storage: &'a mut crate::WebStorage,
        origin: &[u8],
    ) -> Self {
        let mut runtime = Self::new(dom, step_budget);
        let nlen = origin.len().min(crate::MAX_STORAGE_ORIGIN_LEN);
        runtime.origin[..nlen].copy_from_slice(&origin[..nlen]);
        runtime.origin_len = nlen as u8;
        runtime.storage = Some(storage);
        runtime
    }

    /// The origin the page's storage belongs to.
    fn storage_origin(&self) -> &[u8] {
        &self.origin[..self.origin_len as usize]
    }

    /// Records a timer the page asked for and returns its id.
    ///
    /// WHY a callback without a name is refused: the timer outlives this
    /// run, and the page's program is the only way back to the function. A
    /// callback the program cannot name could never be entered, so accepting
    /// it would be accepting a timer that cannot fire.
    fn schedule_timer(&mut self, handler: u8, delay_ms: u64) -> Result<JsValue, JsError> {
        let handler = handler as usize;
        if handler >= self.function_count {
            return Err(JsError::ReferenceError);
        }
        let definition = self.functions[handler];
        if definition.name_len == 0 {
            return Err(JsError::UnsupportedSyntax(
                "a timer callback must be a named function",
            ));
        }
        if self.timer_count >= MAX_TIMERS {
            return Err(JsError::CapacityExceeded);
        }
        let id = self.next_timer_id;
        self.next_timer_id = self.next_timer_id.wrapping_add(1);
        let mut name = [0u8; 32];
        name[..definition.name_len as usize]
            .copy_from_slice(&definition.name[..definition.name_len as usize]);
        self.timers[self.timer_count] = JsTimer {
            id,
            fn_id: handler as u8,
            delay_ms,
            name,
            name_len: definition.name_len,
            cancelled: false,
        };
        self.timer_count += 1;
        Ok(JsValue::Number(id as i64))
    }

    /// Removes the timer with this id.
    fn cancel_timer(&mut self, id: u16) {
        let mut index = 0usize;
        while index < self.timer_count {
            if self.timers[index].id == id {
                for position in index..self.timer_count - 1 {
                    self.timers[position] = self.timers[position + 1];
                }
                self.timer_count -= 1;
                return;
            }
            index += 1;
        }
    }

    /// Runs the timers whose delay has passed, in the order they were
    /// scheduled, and returns how many ran.
    ///
    /// WHY only the timers whose delay has passed: a timer that asked to wait
    /// is still waiting, and running it early would be a lie about when the
    /// page's own code was entered.
    fn fire_ready_timers(&mut self, parser: &Parser) -> Result<usize, JsError> {
        let mut fired = 0usize;
        while fired < MAX_TIMERS {
            let mut found = None;
            for index in 0..self.timer_count {
                let timer = self.timers[index];
                if !timer.cancelled && timer.delay_ms == 0 {
                    found = Some(index);
                    break;
                }
            }
            let Some(index) = found else {
                break;
            };
            let timer = self.timers[index];
            for position in index..self.timer_count - 1 {
                self.timers[position] = self.timers[position + 1];
            }
            self.timer_count -= 1;
            self.invoke_function(parser, timer.fn_id, &[])?;
            fired += 1;
        }
        Ok(fired)
    }

    /// The timers this run left waiting, with the delay each asked for.
    fn pending_timers(&self) -> ([ScheduledTimer; MAX_TIMERS], usize) {
        let mut out = [ScheduledTimer::default(); MAX_TIMERS];
        let mut count = 0usize;
        for index in 0..self.timer_count {
            let timer = self.timers[index];
            if timer.cancelled {
                continue;
            }
            let mut entry = ScheduledTimer::default();
            entry.id = timer.id;
            entry.function[..timer.name_len as usize]
                .copy_from_slice(&timer.name[..timer.name_len as usize]);
            entry.function_len = timer.name_len;
            entry.delay_ms = timer.delay_ms;
            out[count] = entry;
            count += 1;
        }
        (out, count)
    }

    /// Defines the program's functions without running its other statements.
    ///
    /// WHY this exists: a timer callback has to outlive the run that
    /// scheduled it, and the runtime's function arena does not. The page's
    /// program is parsed again and only its function declarations are
    /// evaluated, so the callback can be entered later without repeating the
    /// page's top-level work.
    fn eval_declarations(&mut self, parser: &Parser, root: usize) -> Result<(), JsError> {
        let Some(node) = parser.nodes.get(root) else {
            return Err(JsError::UnexpectedToken);
        };
        let Some(StmtKind::Block {
            stmts_start,
            stmts_count,
        }) = node.stmt.as_ref()
        else {
            return Err(JsError::UnexpectedToken);
        };
        for index in 0..*stmts_count {
            let stmt_id = parser.stmts_pool[stmts_start + index];
            let Some(stmt_node) = parser.nodes.get(stmt_id) else {
                continue;
            };
            if matches!(stmt_node.stmt, Some(StmtKind::FunctionDecl { .. })) {
                self.eval_stmt(parser, stmt_id)?;
            }
        }
        Ok(())
    }

    /// The id of the function this run defined under this name.
    fn function_by_name(&self, name: &[u8]) -> Option<u8> {
        for index in 0..self.function_count {
            let definition = &self.functions[index];
            let len = definition.name_len as usize;
            if len == name.len() && &definition.name[..len] == name {
                return Some(index as u8);
            }
        }
        None
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
                // WHY the storage globals are only there when a store is
                // attached: a page that asks for storage the browser did not
                // give it is told the store is missing, rather than being
                // handed an empty one that reads like a store that lost
                // everything.
                if slice == b"localStorage" || slice == b"sessionStorage" {
                    return if self.storage.is_some() {
                        Ok(JsValue::Storage(u8::from(slice == b"sessionStorage")))
                    } else {
                        Ok(JsValue::Undefined)
                    };
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
                    JsValue::Event(index) => {
                        let Some(state) = self.events.get(index as usize) else {
                            return Err(JsError::ReferenceError);
                        };
                        match prop_slice {
                            b"type" => Ok(JsValue::string_from_slice(
                                &state.name[..state.name_len as usize],
                            )),
                            b"defaultPrevented" => Ok(JsValue::Boolean(state.default_prevented)),
                            b"cancelBubble" => Ok(JsValue::Boolean(state.propagation_stopped)),
                            _ => Err(JsError::UnsupportedSyntax("unsupported event property")),
                        }
                    }
                    JsValue::Storage(store) => {
                        let session = store == 1;
                        let Some(storage) = self.storage.as_ref() else {
                            return Err(JsError::UnsupportedSyntax("storage is not available"));
                        };
                        match prop_slice {
                            b"length" => Ok(JsValue::Number(
                                storage.count_for(self.storage_origin(), session) as i64,
                            )),
                            _ => Err(JsError::UnsupportedSyntax("unsupported storage property")),
                        }
                    }
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
                    Some(ExprKind::Ident(name, nlen)) => {
                        let global = &name[..*nlen as usize];
                        // Method: setTimeout(handler, delay)
                        //
                        // WHY the callback is named: the runtime's function
                        // arena belongs to this run, and a timer that outlives
                        // the run is entered later through the page's own
                        // program, where the name is how that program is
                        // asked for the function again.
                        if global == b"setTimeout" {
                            if *args_count < 1 {
                                return Err(JsError::TypeError);
                            }
                            let handler_id = parser.args_pool[*args_start];
                            let handler_val = self.eval_expr(parser, handler_id)?;
                            let JsValue::Function(handler) = handler_val else {
                                return Err(JsError::TypeError);
                            };
                            let delay = if *args_count >= 2 {
                                let delay_id = parser.args_pool[*args_start + 1];
                                let delay_val = self.eval_expr(parser, delay_id)?;
                                delay_val.to_number()
                            } else {
                                0
                            };
                            let delay = if delay < 0 {
                                0
                            } else {
                                delay.min(MAX_TIMER_DELAY_MS as i64) as u64
                            };
                            return self.schedule_timer(handler, delay);
                        }
                        // Method: clearTimeout(id)
                        //
                        // WHY a cancelled timer is removed rather than left
                        // behind: a cancelled timer that later ran would be a
                        // callback the page was told would not run.
                        if global == b"clearTimeout" {
                            if *args_count < 1 {
                                return Err(JsError::TypeError);
                            }
                            let id_id = parser.args_pool[*args_start];
                            let id_val = self.eval_expr(parser, id_id)?;
                            let id = id_val.to_number();
                            if id < 0 {
                                return Ok(JsValue::Undefined);
                            }
                            self.cancel_timer(id as u16);
                            return Ok(JsValue::Undefined);
                        }
                    }
                    _ => {}
                }
                match callee_node.expr.as_ref() {
                    Some(ExprKind::Member {
                        object,
                        prop,
                        prop_len,
                    }) => {
                        let obj = self.eval_expr(parser, *object)?;
                        let method = &prop[..*prop_len as usize];

                        // Method: event.stopPropagation()
                        // Method: event.preventDefault()
                        // Method: event.stopImmediatePropagation()
                        //
                        // WHY these are handled first: the event object is not
                        // an element, and a handler that calls
                        // `stopPropagation()` is asking the dispatch to stop
                        // travelling -- a request the runtime has to answer
                        // rather than pass on.
                        if let JsValue::Event(index) = obj {
                            match method {
                                b"stopPropagation" => {
                                    if let Some(state) = self.events.get_mut(index as usize) {
                                        state.propagation_stopped = true;
                                    }
                                    return Ok(JsValue::Undefined);
                                }
                                b"stopImmediatePropagation" => {
                                    if let Some(state) = self.events.get_mut(index as usize) {
                                        state.propagation_stopped = true;
                                        state.immediate_stopped = true;
                                    }
                                    return Ok(JsValue::Undefined);
                                }
                                b"preventDefault" => {
                                    if let Some(state) = self.events.get_mut(index as usize) {
                                        state.default_prevented = true;
                                    }
                                    return Ok(JsValue::Undefined);
                                }
                                _ => return Err(JsError::UnsupportedSyntax("unsupported event method")),
                            }
                        }

                        // Method: localStorage.setItem(key, value)
                        // Method: localStorage.getItem(key)
                        // Method: localStorage.removeItem(key)
                        // Method: localStorage.clear()
                        // Method: localStorage.key(index)
                        //
                        // WHY the store is the page's own: the runtime holds
                        // the origin the page was served from and asks the
                        // store for that origin's entries, so a page cannot
                        // read or write another origin's storage even by
                        // naming it.
                        if let JsValue::Storage(store) = obj {
                            let session = store == 1;
                            // WHY the origin is copied out first: the store is
                            // borrowed from the runtime, and the runtime is
                            // what evaluates the arguments.
                            let mut origin = [0u8; crate::MAX_STORAGE_ORIGIN_LEN];
                            let origin_len = self.origin_len as usize;
                            origin[..origin_len].copy_from_slice(&self.origin[..origin_len]);
                            let origin = &origin[..origin_len];
                            // WHY the arguments are evaluated before the store
                            // is borrowed: evaluating them needs the runtime,
                            // and the store is part of the runtime.
                            let first = if *args_count >= 1 {
                                let arg_id = parser.args_pool[*args_start];
                                Some(self.eval_expr(parser, arg_id)?)
                            } else {
                                None
                            };
                            let second = if *args_count >= 2 {
                                let arg_id = parser.args_pool[*args_start + 1];
                                Some(self.eval_expr(parser, arg_id)?)
                            } else {
                                None
                            };
                            let Some(storage) = self.storage.as_mut() else {
                                return Err(JsError::UnsupportedSyntax("storage is not available"));
                            };
                            match method {
                                b"setItem" => {
                                    let (Some(key), Some(value)) = (first, second) else {
                                        return Err(JsError::TypeError);
                                    };
                                    let outcome = storage.set(
                                        origin,
                                        key.as_str().as_bytes(),
                                        value.as_str().as_bytes(),
                                        session,
                                    );
                                    if outcome == crate::StorageOutcome::Refused {
                                        return Err(JsError::CapacityExceeded);
                                    }
                                    return Ok(JsValue::Undefined);
                                }
                                b"getItem" => {
                                    let Some(key) = first else {
                                        return Err(JsError::TypeError);
                                    };
                                    return Ok(match storage.get(
                                        origin,
                                        key.as_str().as_bytes(),
                                        session,
                                    ) {
                                        Some(value) => JsValue::string_from_slice(value),
                                        None => JsValue::Null,
                                    });
                                }
                                b"removeItem" => {
                                    let Some(key) = first else {
                                        return Err(JsError::TypeError);
                                    };
                                    storage.remove(origin, key.as_str().as_bytes(), session);
                                    return Ok(JsValue::Undefined);
                                }
                                b"clear" => {
                                    storage.clear_origin(origin, session);
                                    return Ok(JsValue::Undefined);
                                }
                                b"key" => {
                                    let Some(index) = first else {
                                        return Err(JsError::TypeError);
                                    };
                                    let index = index.to_number();
                                    if index < 0 {
                                        return Ok(JsValue::Null);
                                    }
                                    return Ok(match storage.key_at(origin, session, index as usize) {
                                        Some(key) => JsValue::string_from_slice(key),
                                        None => JsValue::Null,
                                    });
                                }
                                _ => {
                                    return Err(JsError::UnsupportedSyntax(
                                        "unsupported storage method",
                                    ))
                                }
                            }
                        }

                        // Method: document.getElementById(id_str)
                        if method == b"getElementById" && *args_count >= 1 {
                            if !matches!(obj, JsValue::Element(_)) {
                                return Err(JsError::TypeError);
                            }
                            let arg_id = parser.args_pool[*args_start];
                            let arg_val = self.eval_expr(parser, arg_id)?;
                            let target_id = arg_val.as_str().as_bytes();
                            let found = self.find_element_by_id(target_id);
                            return Ok(found.map(JsValue::Element).unwrap_or(JsValue::Null));
                        }

                        // Method: document.getElementsByTagName(tag_str)
                        // Returns an array of matching attached element nodes
                        // under `obj` (empty when none match), bounded by the
                        // array arena.
                        if method == b"getElementsByTagName" && *args_count >= 1 {
                            let scope = match obj {
                                JsValue::Element(n) => n,
                                _ => return Err(JsError::TypeError),
                            };
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
                                    && self.is_descendant_of(nid, scope)
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

                        // Method: element.addEventListener(event, handler, capture)
                        // Registers a function-valued handler for a named event
                        // on this element. The handler runs when the runtime
                        // dispatches that event (see `dispatch_event`). The third
                        // argument names the phase: a true value puts the
                        // listener in the capture phase, which runs while the
                        // event travels down to the target instead of back up
                        // from it.
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
                            let capture = if *args_count >= 3 {
                                let cap_id = parser.args_pool[*args_start + 2];
                                let cap_val = self.eval_expr(parser, cap_id)?;
                                cap_val.to_bool()
                            } else {
                                false
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
                                capture,
                            };
                            self.listener_count += 1;
                            return Ok(JsValue::Undefined);
                        }

                        // Method: element.removeEventListener(event, handler, capture)
                        // Removes the first listener matching this node, event
                        // name, function id and phase (the identity
                        // addEventListener keys on). A no-op when none is
                        // registered.
                        if method == b"removeEventListener" && *args_count >= 2 {
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
                            let capture = if *args_count >= 3 {
                                let cap_id = parser.args_pool[*args_start + 2];
                                let cap_val = self.eval_expr(parser, cap_id)?;
                                cap_val.to_bool()
                            } else {
                                false
                            };
                            let mut event = [0u8; 16];
                            let elen = ev_bytes.len().min(event.len());
                            event[..elen].copy_from_slice(&ev_bytes[..elen]);
                            let elen = elen as u8;
                            let mut i = 0;
                            while i < self.listener_count {
                                let l = self.listeners[i];
                                if l.node == node
                                    && l.event_len == elen
                                    && &l.event[..elen as usize] == &event[..elen as usize]
                                    && l.fn_id == handler
                                    && l.capture == capture
                                {
                                    let mut j = i;
                                    while j + 1 < self.listener_count {
                                        self.listeners[j] = self.listeners[j + 1];
                                        j += 1;
                                    }
                                    self.listener_count -= 1;
                                    self.listeners[self.listener_count] = JsListener {
                                        node: NO_NODE,
                                        event: [0; 16],
                                        event_len: 0,
                                        fn_id: 0,
                                        capture: false,
                                    };
                                    break;
                                }
                                i += 1;
                            }
                            return Ok(JsValue::Undefined);
                        }

                        // Method: console.log(...)
                        if method == b"log" {
                            return Ok(JsValue::Undefined);
                        }

                        // WHY a method on nothing is a type error: a page that
                        // asks for a store the browser did not give it holds
                        // `undefined`, and calling a method on it is a type
                        // error rather than an unknown method.
                        if matches!(obj, JsValue::Undefined | JsValue::Null) {
                            return Err(JsError::TypeError);
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

    /// Dispatches `event` to every listener registered on `document` for that name.
    ///
    /// Returns whether the event's default action was prevented.
    fn dispatch_event(&mut self, parser: &Parser, event: &[u8]) -> Result<bool, JsError> {
        let root = self.dom.root();
        self.dispatch_event_on(parser, root, event)
    }

    /// Dispatches `event` to every listener registered on `target` for that name.
    ///
    /// The dispatch follows the DOM event path (DOM Standard §2.9): the
    /// capture phase runs from the root down to the target's parent, the
    /// target itself runs its listeners in registration order, and the bubble
    /// phase runs from the target's parent back up to the root. A listener
    /// registered with a true third argument belongs to the capture phase;
    /// every other listener belongs to the bubble phase.
    ///
    /// The matching function ids are gathered first so the mutable borrow
    /// taken by [`JsRuntime::invoke_function`] does not overlap the listener
    /// table read. A handler that overruns its budget aborts the dispatch
    /// with the same error an ordinary call would report.
    fn dispatch_event_on(
        &mut self,
        parser: &Parser,
        target: NodeId,
        event: &[u8],
    ) -> Result<bool, JsError> {
        // The path is collected first so a handler that mutates the tree cannot
        // change it mid-dispatch.
        let mut path = [NO_NODE; 64];
        let mut depth = 0usize;
        let root = self.dom.root();
        let limit = self.dom.node_count();
        let mut guard = 0usize;
        let mut cur = Some(target);
        while let Some(nid) = cur {
            if depth >= path.len() {
                break;
            }
            path[depth] = nid;
            depth += 1;
            if nid == root {
                break;
            }
            guard += 1;
            if guard > limit {
                break;
            }
            cur = self.dom.parent(nid);
        }
        if depth == 0 {
            return Ok(false);
        }

        // The event object handed to every handler. It lives in the runtime's
        // event stack while the dispatch runs, so a handler that dispatches
        // another event gets its own object.
        let event_value = self.begin_event(event)?;

        // The nodes the event visits, in the order the DOM visits them: the
        // root first for the capture phase, then the target, then the target's
        // parent upwards for the bubble phase.
        let mut nodes = [NO_NODE; 64];
        let mut node_count = 0usize;
        // Capture phase: the root down to the target's parent.
        for i in (1..depth).rev() {
            if node_count < nodes.len() {
                nodes[node_count] = path[i];
                node_count += 1;
            }
        }
        // The target.
        if node_count < nodes.len() {
            nodes[node_count] = path[0];
            node_count += 1;
        }
        // Bubble phase: the target's parent up to the root.
        for i in 1..depth {
            if node_count < nodes.len() {
                nodes[node_count] = path[i];
                node_count += 1;
            }
        }

        for (position, node) in nodes[..node_count].iter().enumerate() {
            let is_target = position + 1 == depth && *node == path[0];
            let in_capture_phase = position + 1 < depth;
            let mut ids = [0u8; MAX_LISTENERS];
            let mut count = 0usize;
            for j in 0..self.listener_count {
                let listener = self.listeners[j];
                let len = listener.event_len as usize;
                if listener.node == *node && len == event.len() && &listener.event[..len] == event {
                    // A listener belongs to the capture phase only if it asked
                    // for it; the target runs both kinds, in the order they
                    // were registered.
                    let takes_part = listener.capture == in_capture_phase || is_target;
                    if takes_part && count < ids.len() {
                        ids[count] = listener.fn_id;
                        count += 1;
                    }
                }
            }
            for k in 0..count {
                self.invoke_function(parser, ids[k], &[event_value])?;
                if self.event_immediate_stopped(event_value) {
                    break;
                }
            }
            if self.event_propagation_stopped(event_value) {
                break;
            }
        }

        let prevented = self.event_default_prevented(event_value);
        self.end_event();
        Ok(prevented)
    }

    /// Opens an event object for a dispatch and returns the value a handler
    /// receives as its event argument.
    fn begin_event(&mut self, name: &[u8]) -> Result<JsValue, JsError> {
        if self.event_depth >= MAX_EVENTS {
            return Err(JsError::CapacityExceeded);
        }
        let mut state = JsEventState::EMPTY;
        let nlen = name.len().min(MAX_EVENT_NAME_LEN);
        state.name[..nlen].copy_from_slice(&name[..nlen]);
        state.name_len = nlen as u8;
        self.events[self.event_depth] = state;
        self.event_depth += 1;
        Ok(JsValue::Event((self.event_depth - 1) as u8))
    }

    /// Closes the innermost event object.
    fn end_event(&mut self) {
        if self.event_depth > 0 {
            self.event_depth -= 1;
        }
    }

    fn event_state(&self, value: JsValue) -> Option<&JsEventState> {
        match value {
            JsValue::Event(index) => self.events.get(index as usize),
            _ => None,
        }
    }

    fn event_propagation_stopped(&self, value: JsValue) -> bool {
        self.event_state(value)
            .map(|state| state.propagation_stopped)
            .unwrap_or(false)
    }

    fn event_immediate_stopped(&self, value: JsValue) -> bool {
        self.event_state(value)
            .map(|state| state.immediate_stopped)
            .unwrap_or(false)
    }

    fn event_default_prevented(&self, value: JsValue) -> bool {
        self.event_state(value)
            .map(|state| state.default_prevented)
            .unwrap_or(false)
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

    fn is_descendant_of(&self, node: NodeId, ancestor: NodeId) -> bool {
        if node == ancestor {
            return false;
        }
        let mut cur = self.dom.parent(node);
        let mut guard = 0usize;
        let limit = self.dom.node_count();
        while let Some(nid) = cur {
            if nid == ancestor {
                return true;
            }
            if guard > limit {
                return false;
            }
            guard += 1;
            cur = self.dom.parent(nid);
        }
        false
    }

    fn is_in_document(&self, node: NodeId) -> bool {
        let root = self.dom.root();
        node == root || self.is_descendant_of(node, root)
    }

    fn find_element_by_id(&self, target_id: &[u8]) -> Option<NodeId> {
        let n = self.dom.node_count();
        for id in 0..n {
            let nid = id as NodeId;
            if self.dom.node(nid).is_element() && self.is_in_document(nid) {
                if let Some(id_attr) = self.dom.attr(nid, b"id") {
                    if id_attr == target_id {
                        return Some(nid);
                    }
                }
            }
        }
        None
    }

    fn collect_text_content(&self, node_id: NodeId) -> JsValue {
        let mut buf = [0u8; MAX_STR_LEN];
        if self.dom.node(node_id).is_text() {
            let t = self.dom.text_of(node_id);
            let take = t.len().min(MAX_STR_LEN);
            buf[..take].copy_from_slice(&t[..take]);
            return JsValue::String(buf, take as u8);
        }
        let mut len = 0usize;
        let mut cur = self.dom.first_child(node_id);
        let mut guard = 0usize;
        let max_steps = self.dom.node_count().saturating_mul(2).max(1);
        while let Some(id) = cur {
            if guard > max_steps || len >= MAX_STR_LEN {
                break;
            }
            guard += 1;
            if self.dom.node(id).is_text() {
                let t = self.dom.text_of(id);
                let take = t.len().min(MAX_STR_LEN - len);
                buf[len..len + take].copy_from_slice(&t[..take]);
                len += take;
            }
            if let Some(child) = self.dom.first_child(id) {
                cur = Some(child);
            } else {
                let mut backtrack = Some(id);
                cur = None;
                while let Some(b) = backtrack {
                    if guard > max_steps {
                        break;
                    }
                    guard += 1;
                    if b == node_id {
                        break;
                    }
                    if let Some(sib) = self.dom.next_sibling(b) {
                        cur = Some(sib);
                        break;
                    }
                    backtrack = self.dom.parent(b);
                    if backtrack == Some(node_id) {
                        break;
                    }
                }
            }
        }
        JsValue::String(buf, len as u8)
    }

    fn get_element_prop(&self, node_id: NodeId, prop: &[u8]) -> Result<JsValue, JsError> {
        if (node_id as usize) >= self.dom.node_count() {
            return Err(JsError::TypeError);
        }
        match prop {
            b"id" => {
                if let Some(id_val) = self.dom.attr(node_id, b"id") {
                    Ok(JsValue::string_from_slice(id_val))
                } else {
                    Ok(JsValue::string_from_slice(b""))
                }
            }
            b"value" => {
                if let Some(val) = self.dom.attr(node_id, b"value") {
                    Ok(JsValue::string_from_slice(val))
                } else {
                    Ok(JsValue::string_from_slice(b""))
                }
            }
            b"textContent" | b"innerText" => Ok(self.collect_text_content(node_id)),
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
        if (node_id as usize) >= self.dom.node_count() {
            return Err(JsError::TypeError);
        }
        match prop {
            b"value" => {
                if !self.dom.node(node_id).is_element() {
                    return Err(JsError::TypeError);
                }
                let bytes = val.as_str().as_bytes();
                if !self.dom.set_attr(node_id, b"value", bytes) {
                    return Err(JsError::CapacityExceeded);
                }
                self.mutated = true;
                Ok(())
            }
            b"checked" => {
                if !self.dom.node(node_id).is_element() {
                    return Err(JsError::TypeError);
                }
                if val.to_bool() {
                    if !self.dom.set_attr(node_id, b"checked", b"checked") {
                        return Err(JsError::CapacityExceeded);
                    }
                    self.mutated = true;
                } else if self.dom.remove_attr(node_id, b"checked") {
                    self.mutated = true;
                }
                Ok(())
            }
            b"textContent" | b"innerText" => {
                if !self.dom.node(node_id).is_element() && !self.dom.node(node_id).is_text() {
                    return Err(JsError::TypeError);
                }
                let bytes = val.as_str().as_bytes();
                if !self.dom.set_text(node_id, bytes) {
                    return Err(JsError::CapacityExceeded);
                }
                self.mutated = true;
                Ok(())
            }
            b"id" => {
                if !self.dom.node(node_id).is_element() {
                    return Err(JsError::TypeError);
                }
                // Assigning `id` must store the attribute so getElementById can
                // resolve the node (used by script-built UI and event handlers).
                let bytes = val.as_str().as_bytes();
                if !self.dom.set_attr(node_id, b"id", bytes) {
                    return Err(JsError::CapacityExceeded);
                }
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

/// Executes a script with the page's storage attached and returns whether the
/// DOM was mutated.
///
/// WHY the origin travels with the store: the script may only reach the
/// entries its own origin wrote, so the runtime is told which origin the
/// page was served from and asks the store for that origin alone.
pub fn execute_script_with_mutations_and_storage(
    script: &[u8],
    dom: &mut Dom,
    step_budget: usize,
    storage: &mut crate::WebStorage,
    origin: &[u8],
) -> Result<(JsValue, bool), JsError> {
    let lexer = Lexer::new(script);
    let mut parser = Parser::new(lexer)?;
    let root = parser.parse_program()?;
    let mut runtime = JsRuntime::with_storage(dom, step_budget, storage, origin);
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
    execute_script_with_events_detailed(script, dom, step_budget, event).map(|(v, mutated, _)| (v, mutated))
}

/// Runs a script, dispatches `event`, and reports what the dispatch decided.
///
/// The third value says whether a handler called `preventDefault()`: the
/// event's default action was called off, and the page that asked for that
/// has to be told the request was handled rather than defaulted.
pub fn execute_script_with_events_detailed(
    script: &[u8],
    dom: &mut Dom,
    step_budget: usize,
    event: &[u8],
) -> Result<(JsValue, bool, bool), JsError> {
    let lexer = Lexer::new(script);
    let mut parser = Parser::new(lexer)?;
    let root = parser.parse_program()?;
    let mut runtime = JsRuntime::new(dom, step_budget);
    let val = runtime.eval_program(&parser, root)?;
    let default_prevented = runtime.dispatch_event(&parser, event)?;
    runtime.fire_ready_timers(&parser)?;
    Ok((val, runtime.dom_mutated(), default_prevented))
}

/// Runs a script, dispatches `event`, and reports what the run left behind.
///
/// The timers the run did not fire are reported so the caller can run them
/// when their delay has passed.
pub fn execute_script_reporting(
    script: &[u8],
    dom: &mut Dom,
    step_budget: usize,
    event: &[u8],
) -> Result<ScriptRun, JsError> {
    let lexer = Lexer::new(script);
    let mut parser = Parser::new(lexer)?;
    let root = parser.parse_program()?;
    let mut runtime = JsRuntime::new(dom, step_budget);
    let value = runtime.eval_program(&parser, root)?;
    let default_prevented = runtime.dispatch_event(&parser, event)?;
    runtime.fire_ready_timers(&parser)?;
    let (timers, timer_count) = runtime.pending_timers();
    Ok(ScriptRun {
        value,
        mutated: runtime.dom_mutated(),
        default_prevented,
        timers,
        timer_count,
    })
}

/// Runs a script with the page's storage attached and reports what the run
/// left behind.
///
/// WHY the origin travels with the store: the page may only reach the
/// entries its own origin wrote, so the runtime is told which origin the
/// page was served from and asks the store for that origin alone.
pub fn execute_script_reporting_with_storage(
    script: &[u8],
    dom: &mut Dom,
    step_budget: usize,
    event: &[u8],
    storage: &mut crate::WebStorage,
    origin: &[u8],
) -> Result<ScriptRun, JsError> {
    let lexer = Lexer::new(script);
    let mut parser = Parser::new(lexer)?;
    let root = parser.parse_program()?;
    let mut runtime = JsRuntime::with_storage(dom, step_budget, storage, origin);
    let value = runtime.eval_program(&parser, root)?;
    let default_prevented = runtime.dispatch_event(&parser, event)?;
    runtime.fire_ready_timers(&parser)?;
    let (timers, timer_count) = runtime.pending_timers();
    Ok(ScriptRun {
        value,
        mutated: runtime.dom_mutated(),
        default_prevented,
        timers,
        timer_count,
    })
}

/// Runs a page's program far enough to define its functions, then calls the
/// named one.
///
/// WHY the definitions are made without running the page's other statements:
/// a timer callback is entered later, and running the page's top-level work
/// again would repeat work the page already did. The callback therefore
/// starts from the page's functions and not from the page's variables, and
/// a callback that needs a variable the page set up sees it as undefined.
pub fn execute_named_callback(
    script: &[u8],
    dom: &mut Dom,
    step_budget: usize,
    name: &[u8],
) -> Result<bool, JsError> {
    let lexer = Lexer::new(script);
    let mut parser = Parser::new(lexer)?;
    let root = parser.parse_program()?;
    let mut runtime = JsRuntime::new(dom, step_budget);
    runtime.eval_declarations(&parser, root)?;
    let Some(handler) = runtime.function_by_name(name) else {
        return Err(JsError::ReferenceError);
    };
    runtime.invoke_function(&parser, handler, &[])?;
    Ok(runtime.dom_mutated())
}

/// Runs a script with the page's storage attached, then dispatches `event`.
///
/// WHY the origin travels with the store: the page may only reach the
/// entries its own origin wrote, so the runtime is told which origin the
/// page was served from and asks the store for that origin alone.
pub fn execute_script_with_events_and_storage(
    script: &[u8],
    dom: &mut Dom,
    step_budget: usize,
    event: &[u8],
    storage: &mut crate::WebStorage,
    origin: &[u8],
) -> Result<(JsValue, bool, bool), JsError> {
    let lexer = Lexer::new(script);
    let mut parser = Parser::new(lexer)?;
    let root = parser.parse_program()?;
    let mut runtime = JsRuntime::with_storage(dom, step_budget, storage, origin);
    let val = runtime.eval_program(&parser, root)?;
    let default_prevented = runtime.dispatch_event(&parser, event)?;
    Ok((val, runtime.dom_mutated(), default_prevented))
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

    #[test]
    fn test_multi_attribute_set_attr_and_get_element_by_id() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body><p class=\"card\">init</p><span id=\"other\">x</span></body></html>");
        let src = b"var list = document.getElementsByTagName('p'); \
                    var p = list[0]; \
                    p.id = 'target'; \
                    p.textContent = 'updated'; \
                    var resolved = document.getElementById('target'); \
                    var result = resolved.id + ':' + resolved.textContent;";
        let (res, mutated) =
            execute_script_with_mutations(src, &mut dom, 2000).expect("script succeeds");
        assert!(mutated);
        assert_eq!(res.as_str(), "target:updated");
        let other = (0..dom.node_count() as NodeId)
            .find(|&id| dom.attr(id, b"id") == Some(&b"other"[..]));
        assert!(other.is_some(), "neighbor element attribute must remain intact");
    }

    #[test]
    fn test_set_element_prop_fails_cleanly_on_attr_capacity_exhaustion() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body><p id=\"t\">keep</p></body></html>");
        let mut count = 0usize;
        'fill: while count < crate::layout::dom::MAX_ATTRS {
            let Some(n) = dom.create_element(b"span") else {
                break;
            };
            for _ in 0..8 {
                if count >= crate::layout::dom::MAX_ATTRS {
                    break 'fill;
                }
                let mut key = [b'k', b'0', b'0', b'0', b'0'];
                key[1] = b'0' + ((count / 1000) % 10) as u8;
                key[2] = b'0' + ((count / 100) % 10) as u8;
                key[3] = b'0' + ((count / 10) % 10) as u8;
                key[4] = b'0' + (count % 10) as u8;
                if !dom.set_attr(n, &key, b"v") {
                    break 'fill;
                }
                count += 1;
            }
        }
        let mut rt = JsRuntime::new(&mut dom, 1000);
        let t = rt.find_element_by_id(b"t").expect("target exists");
        let err = rt
            .set_element_prop(t, b"value", JsValue::string_from_slice(b"new"))
            .expect_err("attribute capacity exhaustion must fail");
        assert_eq!(err, JsError::CapacityExceeded);
        assert!(!rt.dom_mutated(), "failed mutation must not mark runtime mutated");
        assert_eq!(dom.attr(t, b"id"), Some(&b"t"[..]));
        assert_eq!(dom.attr(t, b"value"), None);
    }

    #[test]
    fn test_checked_false_removes_attribute() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body><input id=\"cb\" type=\"checkbox\" checked></body></html>");
        let src = b"var cb = document.getElementById('cb'); \
                    var was = cb.checked; \
                    cb.checked = false; \
                    var now = cb.checked; \
                    was && !now;";
        let (res, mutated) =
            execute_script_with_mutations(src, &mut dom, 1000).expect("checked toggle succeeds");
        assert!(mutated);
        assert_eq!(res, JsValue::Boolean(true));
        let cb = (0..dom.node_count() as NodeId)
            .find(|&id| dom.attr(id, b"id") == Some(&b"cb"[..]))
            .expect("checkbox exists");
        assert!(!dom.has_attr(cb, b"checked"));
    }

    #[test]
    fn test_el_id_and_text_content_getters_and_detached_nodes() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body><div id=\"box\"><span>hello </span>world</div></body></html>");
        let src = b"var box = document.getElementById('box'); \
                    var d = document.createElement('p'); \
                    d.id = 'detached'; \
                    var ghost = document.getElementById('detached'); \
                    var ps = document.getElementsByTagName('p'); \
                    var ok_before = (ghost == null) && (ps.length == 0); \
                    document.body.appendChild(d); \
                    var found = document.getElementById('detached'); \
                    var ps_after = document.getElementsByTagName('p'); \
                    var ok_after = ps_after.length == 1; \
                    box.id + '|' + box.textContent + '|' + ok_before + '|' + found.id + '|' + ok_after;";
        let res = execute_script(src, &mut dom, 2000).expect("getters and detached checks succeed");
        assert_eq!(res.as_str(), "box|hello world|true|detached|true");
    }

    #[test]
    fn test_listener_targeted_by_node() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body><button id=\"b\">Go</button></body></html>");
        let before = dom.node_count();
        let src = b"function onBtn() { var p = document.createElement('p'); p.textContent = 'WRONG'; document.body.appendChild(p); } \
                    var b = document.getElementById('b'); \
                    b.addEventListener('DOMContentLoaded', onBtn);";
        let (_res, mutated) =
            execute_script_with_events(src, &mut dom, 1000, b"DOMContentLoaded")
                .expect("dispatch succeeds");
        assert!(!mutated, "element listener must not fire on document DOMContentLoaded");
        assert_eq!(dom.node_count(), before);
    }

    #[test]
    fn test_remove_event_listener_prevents_dispatch() {
        // Without removeEventListener the script errors with UnsupportedSyntax
        // and the .expect panics; with it, the removed handler must not run.
        let mut dom = Dom::new();
        dom.parse(b"<html><body></body></html>");
        let before = dom.node_count();
        let src = b"function boot() { var s = document.createElement('p'); s.textContent = 'BOOTED'; document.body.appendChild(s); } document.addEventListener('DOMContentLoaded', boot); document.removeEventListener('DOMContentLoaded', boot);";
        let (_res, mutated) =
            execute_script_with_events(src, &mut dom, 1000, b"DOMContentLoaded")
                .expect("removeEventListener dispatch succeeds");
        assert!(!mutated, "a removed listener must not run on dispatch");
        assert_eq!(dom.node_count(), before, "a removed listener must not mutate the DOM");
    }

    /// Runs a script and dispatches `event` on the element with that id.
    ///
    /// WHY a helper: the phase rules only show themselves when the event is
    /// dispatched on a node below the root, so the tests need the element the
    /// page names rather than the document.
    fn dispatch_on_element(
        src: &[u8],
        dom: &mut Dom,
        element_id: &[u8],
        event: &[u8],
    ) -> Result<(JsValue, bool), JsError> {
        let lexer = Lexer::new(src);
        let mut parser = Parser::new(lexer)?;
        let root = parser.parse_program()?;
        let mut runtime = JsRuntime::new(dom, 2000);
        runtime.eval_program(&parser, root)?;
        let target = runtime
            .find_element_by_id(element_id)
            .ok_or(JsError::ReferenceError)?;
        let prevented = runtime.dispatch_event_on(&parser, target, event)?;
        let order = runtime.get_var(b"order").unwrap_or(JsValue::Undefined);
        Ok((order, prevented))
    }

    #[test]
    fn a_capture_listener_runs_before_the_target_and_the_bubble_listener() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body><div id=\"outer\"><div id=\"inner\"></div></div></body></html>");
        let src = b"var order = '-'; \
                    function outerCapture() { order = order + 'C'; } \
                    function outerBubble() { order = order + 'B'; } \
                    function innerHandler() { order = order + 'I'; } \
                    var outer = document.getElementById('outer'); \
                    var inner = document.getElementById('inner'); \
                    outer.addEventListener('click', outerCapture, true); \
                    outer.addEventListener('click', outerBubble); \
                    inner.addEventListener('click', innerHandler);";
        let (order, prevented) =
            dispatch_on_element(src, &mut dom, b"inner", b"click").expect("dispatch succeeds");
        assert_eq!(
            order.as_str(), "-CIB",
            "the capture phase runs while the event travels down, the target runs next, and the bubble phase runs last"
        );
        assert!(!prevented, "a handler that says nothing must not prevent the default");
    }

    #[test]
    fn a_capture_listener_that_stops_the_event_keeps_the_target_from_running() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body><div id=\"outer\"><div id=\"inner\"></div></div></body></html>");
        let src = b"var order = '-'; \
                    function stopper(e) { order = order + 'S'; e.stopPropagation(); } \
                    function outerBubble() { order = order + 'B'; } \
                    function innerHandler() { order = order + 'I'; } \
                    var outer = document.getElementById('outer'); \
                    var inner = document.getElementById('inner'); \
                    outer.addEventListener('click', stopper, true); \
                    outer.addEventListener('click', outerBubble); \
                    inner.addEventListener('click', innerHandler);";
        let (order, prevented) =
            dispatch_on_element(src, &mut dom, b"inner", b"click").expect("dispatch succeeds");
        assert_eq!(
            order.as_str(), "-S",
            "a stopped event must not reach the target or the bubble phase"
        );
        assert!(!prevented);
    }

    #[test]
    fn a_handler_that_prevents_the_default_is_reported() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body><button id=\"go\">Go</button></body></html>");
        let src = b"var order = '-'; \
                    function onGo(e) { order = order + 'P'; e.preventDefault(); } \
                    var go = document.getElementById('go'); \
                    go.addEventListener('click', onGo);";
        let (order, prevented) =
            dispatch_on_element(src, &mut dom, b"go", b"click").expect("dispatch succeeds");
        assert_eq!(order.as_str(), "-P");
        assert!(
            prevented,
            "a handler that called preventDefault must be reported to the caller"
        );
    }

    #[test]
    fn a_handler_receives_the_event_it_was_called_for() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body><button id=\"go\">Go</button></body></html>");
        let src = b"var order = '-'; \
                    function onGo(e) { order = order + e.type; } \
                    var go = document.getElementById('go'); \
                    go.addEventListener('submit', onGo);";
        let (order, prevented) =
            dispatch_on_element(src, &mut dom, b"go", b"submit").expect("dispatch succeeds");
        assert_eq!(order.as_str(), "-submit", "the handler must receive the event it was called for");
        assert!(!prevented);
    }

    #[test]
    fn a_removed_capture_listener_does_not_run() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body><div id=\"outer\"><div id=\"inner\"></div></div></body></html>");
        let src = b"var order = '-'; \
                    function outerCapture() { order = order + 'C'; } \
                    function innerHandler() { order = order + 'I'; } \
                    var outer = document.getElementById('outer'); \
                    var inner = document.getElementById('inner'); \
                    outer.addEventListener('click', outerCapture, true); \
                    outer.removeEventListener('click', outerCapture, true); \
                    inner.addEventListener('click', innerHandler);";
        let (order, prevented) =
            dispatch_on_element(src, &mut dom, b"inner", b"click").expect("dispatch succeeds");
        assert_eq!(
            order.as_str(), "-I",
            "a removed capture listener must not run while the event travels down"
        );
        assert!(!prevented);
    }

    /// Runs a script with a storage attached and returns the program's value.
    fn run_with_storage(
        src: &[u8],
        dom: &mut Dom,
        storage: &mut crate::WebStorage,
        origin: &[u8],
    ) -> Result<JsValue, JsError> {
        let lexer = Lexer::new(src);
        let mut parser = Parser::new(lexer)?;
        let root = parser.parse_program()?;
        let mut runtime = JsRuntime::with_storage(dom, 2000, storage, origin);
        runtime.eval_program(&parser, root)
    }

    #[test]
    fn a_page_reads_back_the_storage_its_own_origin_wrote() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body></body></html>");
        let mut storage = crate::WebStorage::new();
        let src = b"var missing = localStorage.getItem('absent'); \
                    localStorage.setItem('name', 'interos'); \
                    var read = localStorage.getItem('name');";
        let res = run_with_storage(src, &mut dom, &mut storage, b"https://example.test")
            .expect("storage script succeeds");
        assert_eq!(res.as_str(), "interos");
        assert_eq!(
            storage.get(b"https://example.test", b"name", false),
            Some(&b"interos"[..]),
            "the entry belongs to the origin that wrote it"
        );
        assert_eq!(
            storage.get(b"https://example.test", b"absent", false),
            None,
            "a key that was never written has no value"
        );
    }

    #[test]
    fn another_origin_cannot_see_the_entries_of_this_one() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body></body></html>");
        let mut storage = crate::WebStorage::new();
        let src = b"localStorage.setItem('name', 'interos'); \
                    localStorage.setItem('shared', 'first');";
        run_with_storage(src, &mut dom, &mut storage, b"https://one.test").expect("first page writes");

        // A page served from another origin sees an empty store.
        let mut other_dom = Dom::new();
        other_dom.parse(b"<html><body></body></html>");
        let src = b"var seen = localStorage.getItem('shared'); \
                    var count = localStorage.length; \
                    (seen == null) && (count == 0);";
        let res = run_with_storage(
            src,
            &mut other_dom,
            &mut storage,
            b"https://two.test",
        )
        .expect("second page reads");
        assert_eq!(
            res.as_str(),
            "true",
            "another origin must not see this origin's entries"
        );

        // The first origin's entry is still there for the first origin.
        assert_eq!(
            storage.get(b"https://one.test", b"shared", false),
            Some(&b"first"[..])
        );
    }

    #[test]
    fn the_session_store_is_not_the_origins_own_store() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body></body></html>");
        let mut storage = crate::WebStorage::new();
        let src = b"localStorage.setItem('k', 'lasting'); \
                    sessionStorage.setItem('k', 'tab'); \
                    var lasting = localStorage.getItem('k'); \
                    var tab = sessionStorage.getItem('k'); \
                    lasting + '/' + tab;";
        let res = run_with_storage(src, &mut dom, &mut storage, b"https://example.test")
            .expect("storage script succeeds");
        assert_eq!(
            res.as_str(),
            "lasting/tab",
            "the tab's session store and the origin's store are separate"
        );
        assert_eq!(storage.get(b"https://example.test", b"k", true), Some(&b"tab"[..]));
        assert_eq!(
            storage.get(b"https://example.test", b"k", false),
            Some(&b"lasting"[..])
        );
    }

    #[test]
    fn a_page_with_no_storage_attached_is_told_the_store_is_missing() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body></body></html>");
        let src = b"localStorage.setItem('k', 'v');";
        let err = execute_script(src, &mut dom, 1000).expect_err("storage must not be invented");
        assert_eq!(
            err,
            JsError::TypeError,
            "a page with no store must fail rather than write into one that does not exist"
        );
    }

    #[test]
    fn a_timer_that_asked_to_wait_is_reported_rather_than_run_early() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body></body></html>");
        let src = b"function later() { var s = document.createElement('p'); s.textContent = 'LATE'; document.body.appendChild(s); } \
                    setTimeout(later, 250);";
        let run = execute_script_reporting(src, &mut dom, 2000, b"DOMContentLoaded")
            .expect("the page runs");
        assert_eq!(
            run.timer_count, 1,
            "the timer that is waiting has to be reported to the caller"
        );
        assert_eq!(&run.timers[0].function[..5], b"later");
        assert_eq!(run.timers[0].delay_ms, 250);
        assert!(
            !run.mutated,
            "a timer that has not come due must not have run the page's code"
        );
    }

    #[test]
    fn a_timer_with_no_delay_runs_after_the_page_has_loaded() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body></body></html>");
        let before = dom.node_count();
        let src = b"function soon() { var s = document.createElement('p'); s.textContent = 'SOON'; document.body.appendChild(s); } \
                    setTimeout(soon, 0);";
        let run = execute_script_reporting(src, &mut dom, 2000, b"DOMContentLoaded")
            .expect("the page runs");
        assert_eq!(run.timer_count, 0, "a timer that ran is not one still waiting");
        assert!(run.mutated, "a timer with no delay runs at the end of the run");
        assert!(dom.node_count() > before, "the callback built its node");
    }

    #[test]
    fn a_cancelled_timer_does_not_run_and_is_not_reported() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body></body></html>");
        let before = dom.node_count();
        let src = b"function soon() { var s = document.createElement('p'); s.textContent = 'SOON'; document.body.appendChild(s); } \
                    var id = setTimeout(soon, 0); \
                    clearTimeout(id);";
        let run = execute_script_reporting(src, &mut dom, 2000, b"DOMContentLoaded")
            .expect("the page runs");
        assert_eq!(run.timer_count, 0);
        assert!(!run.mutated, "a cancelled timer must not run the page's code");
        assert_eq!(dom.node_count(), before, "a cancelled timer must not touch the page");
    }

    #[test]
    fn a_named_callback_runs_the_pages_function_without_repeating_the_page() {
        let mut dom = Dom::new();
        dom.parse(b"<html><body></body></html>");
        let src = b"var side = 0; \
                    function later() { var s = document.createElement('p'); s.textContent = 'LATE'; document.body.appendChild(s); } \
                    side = 1;";
        let before = dom.node_count();
        let mutated = execute_named_callback(src, &mut dom, 2000, b"later")
            .expect("the named callback runs");
        assert!(mutated, "the callback built its node");
        assert_eq!(dom.node_count(), before + 2, "the callback added its element and text");
    }
}
