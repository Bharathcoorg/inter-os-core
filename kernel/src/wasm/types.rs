// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! WebAssembly Virtual Machine Types & Opcodes
//!
//! Defines stack values, error types, and opcode specifications
//! for the pure `no_std` in-kernel WASM interpreter.

use alloc::string::String;

/// Fundamental values manipulated on the WASM execution stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    I32(i32),
    I64(i64),
}

impl Value {
    pub fn as_i32(self) -> Result<i32, WasmError> {
        match self {
            Value::I32(v) => Ok(v),
            _ => Err(WasmError::TypeMismatch),
        }
    }

    pub fn as_i64(self) -> Result<i64, WasmError> {
        match self {
            Value::I64(v) => Ok(v),
            _ => Err(WasmError::TypeMismatch),
        }
    }
}

/// Errors returned by the WASM sandbox environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WasmError {
    InvalidMagic,
    UnsupportedVersion,
    OutOfBoundsMemory,
    StackUnderflow,
    TypeMismatch,
    FuelExhausted,
    FunctionNotFound(String),
    UnknownOpcode(u8),
    Trap(String),
    HostCallFailed(String),
    CapabilityDenied,
}

/// WASM Opcode constants.
pub mod opcodes {
    pub const UNREACHABLE: u8 = 0x00;
    pub const NOP: u8 = 0x01;
    pub const RETURN: u8 = 0x0F;
    pub const END: u8 = 0x0B;
    pub const CALL: u8 = 0x10;
    pub const DROP: u8 = 0x1A;
    pub const LOCAL_GET: u8 = 0x20;
    pub const LOCAL_SET: u8 = 0x21;
    pub const LOCAL_TEE: u8 = 0x22;
    pub const I32_LOAD: u8 = 0x28;
    pub const I32_STORE: u8 = 0x36;
    pub const I32_CONST: u8 = 0x41;
    pub const I64_CONST: u8 = 0x42;
    pub const I32_EQ: u8 = 0x46;
    pub const I32_NE: u8 = 0x48;
    pub const I32_LT_S: u8 = 0x4A;
    pub const I32_GT_S: u8 = 0x4E;
    pub const I32_ADD: u8 = 0x6A;
    pub const I32_SUB: u8 = 0x6B;
    pub const I32_MUL: u8 = 0x6C;
    pub const I32_AND: u8 = 0x71;
    pub const I32_OR: u8 = 0x72;
    pub const I32_XOR: u8 = 0x73;
    pub const MEMORY_SIZE: u8 = 0x3F;
    pub const MEMORY_GROW: u8 = 0x40;
}

