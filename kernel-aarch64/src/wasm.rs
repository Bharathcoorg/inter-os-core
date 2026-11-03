// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Inter OS — AArch64 Sovereign WebAssembly (WASM) / Bytecode Micro-Runtime
//!
//! Enforces:
//! - Non-Negotiable Principle #10: Native applications can be written in Rust; the platform
//!   is language-independent and progressively supports Go, TypeScript/WASM, Zig, and C/C++.
//! - Non-Negotiable Principle #11: Language choice does not determine trust. Sandboxed WASM
//!   memory partitions, fuel metering, and capability tables determine trust.
//! - Non-Negotiable Principle #12: User-created and approved automations run within bounded sandboxes.
//! - Non-Negotiable Principle #13: Arbitrary script code never becomes privileged execution automatically.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use crate::capability::{CapabilityTable, CapabilityType, Rights};
use crate::crypto::Sha256;

/// WASM standard page size: 64 KiB.
pub const WASM_PAGE_SIZE: usize = 65536;

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
    OutOfBoundsMemory,
    StackUnderflow,
    TypeMismatch,
    FuelExhausted,
    FunctionNotFound(String),
    UnknownOpcode(u8),
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
    pub const I32_LOAD: u8 = 0x28;
    pub const I32_STORE: u8 = 0x36;
    pub const I32_CONST: u8 = 0x41;
    pub const I64_CONST: u8 = 0x42;
    pub const I32_ADD: u8 = 0x6A;
    pub const I32_SUB: u8 = 0x6B;
    pub const I32_MUL: u8 = 0x6C;
}

/// Sandboxed linear memory buffer.
#[derive(Debug, Clone)]
pub struct WasmMemory {
    data: Vec<u8>,
    max_pages: usize,
}

impl WasmMemory {
    pub fn new(initial_pages: usize, max_pages: usize) -> Self {
        let size = initial_pages * WASM_PAGE_SIZE;
        Self {
            data: vec![0u8; size],
            max_pages,
        }
    }

    pub fn size(&self) -> usize {
        self.data.len()
    }

    pub fn max_pages(&self) -> usize {
        self.max_pages
    }

    pub fn grow(&mut self, delta_pages: usize) -> Result<usize, WasmError> {
        let current_pages = self.data.len() / WASM_PAGE_SIZE;
        let new_pages = current_pages + delta_pages;
        if new_pages > self.max_pages {
            return Err(WasmError::OutOfBoundsMemory);
        }
        self.data.resize(new_pages * WASM_PAGE_SIZE, 0);
        Ok(current_pages)
    }

    pub fn read_u8(&self, addr: usize) -> Result<u8, WasmError> {
        self.data.get(addr).copied().ok_or(WasmError::OutOfBoundsMemory)
    }

    pub fn write_u8(&mut self, addr: usize, val: u8) -> Result<(), WasmError> {
        if addr >= self.data.len() {
            return Err(WasmError::OutOfBoundsMemory);
        }
        self.data[addr] = val;
        Ok(())
    }

    pub fn read_i32(&self, addr: usize) -> Result<i32, WasmError> {
        if addr + 4 > self.data.len() {
            return Err(WasmError::OutOfBoundsMemory);
        }
        let bytes = [
            self.data[addr],
            self.data[addr + 1],
            self.data[addr + 2],
            self.data[addr + 3],
        ];
        Ok(i32::from_le_bytes(bytes))
    }

    pub fn write_i32(&mut self, addr: usize, val: i32) -> Result<(), WasmError> {
        if addr + 4 > self.data.len() {
            return Err(WasmError::OutOfBoundsMemory);
        }
        let bytes = val.to_le_bytes();
        self.data[addr..addr + 4].copy_from_slice(&bytes);
        Ok(())
    }

    pub fn read_slice(&self, addr: usize, len: usize) -> Result<&[u8], WasmError> {
        if addr.checked_add(len).map_or(true, |end| end > self.data.len()) {
            return Err(WasmError::OutOfBoundsMemory);
        }
        Ok(&self.data[addr..addr + len])
    }

    pub fn write_slice(&mut self, addr: usize, slice: &[u8]) -> Result<(), WasmError> {
        let len = slice.len();
        if addr.checked_add(len).map_or(true, |end| end > self.data.len()) {
            return Err(WasmError::OutOfBoundsMemory);
        }
        self.data[addr..addr + len].copy_from_slice(slice);
        Ok(())
    }
}

/// Representation of an internal WASM bytecode function.
#[derive(Debug, Clone)]
pub struct WasmFunction {
    pub name: String,
    pub num_params: usize,
    pub num_locals: usize,
    pub bytecode: Vec<u8>,
}

/// Host function pointer signature.
pub type HostFn = fn(vm: &mut WasmVm, args: &[Value]) -> Result<Option<Value>, WasmError>;

/// Representation of an imported host function.
#[derive(Clone)]
pub struct HostFunction {
    pub name: String,
    pub num_params: usize,
    pub handler: HostFn,
}

/// A parsed/compiled WASM Module containing functions and host imports.
#[derive(Clone)]
pub struct WasmModule {
    pub functions: Vec<WasmFunction>,
    pub host_imports: Vec<HostFunction>,
    pub exports: BTreeMap<String, usize>,
}

impl WasmModule {
    pub fn new() -> Self {
        Self {
            functions: Vec::new(),
            host_imports: Vec::new(),
            exports: BTreeMap::new(),
        }
    }

    pub fn add_host_import(&mut self, name: &str, num_params: usize, handler: HostFn) -> usize {
        let idx = self.host_imports.len();
        self.host_imports.push(HostFunction {
            name: name.to_string(),
            num_params,
            handler,
        });
        idx
    }

    pub fn add_function(
        &mut self,
        name: &str,
        num_params: usize,
        num_locals: usize,
        bytecode: Vec<u8>,
    ) -> usize {
        let idx = self.host_imports.len() + self.functions.len();
        self.functions.push(WasmFunction {
            name: name.to_string(),
            num_params,
            num_locals,
            bytecode,
        });
        idx
    }

    pub fn add_export(&mut self, export_name: &str, func_idx: usize) {
        self.exports.insert(export_name.to_string(), func_idx);
    }
}

/// Active WASM Virtual Machine instance.
pub struct WasmVm {
    pub module: WasmModule,
    pub memory: WasmMemory,
    pub stack: Vec<Value>,
    pub fuel: u64,
    pub instructions_executed: u64,
}

impl WasmVm {
    pub fn new(module: WasmModule, memory: WasmMemory, fuel: u64) -> Self {
        Self {
            module,
            memory,
            stack: Vec::with_capacity(256),
            fuel,
            instructions_executed: 0,
        }
    }

    pub fn push(&mut self, val: Value) {
        self.stack.push(val);
    }

    pub fn pop(&mut self) -> Result<Value, WasmError> {
        self.stack.pop().ok_or(WasmError::StackUnderflow)
    }

    /// Executes an exported function by name, guarded by zero-trust capabilities.
    pub fn call_export(
        &mut self,
        name: &str,
        args: &[Value],
        caps: &CapabilityTable,
    ) -> Result<Option<Value>, WasmError> {
        // Enforce capability check
        if caps.check(CapabilityType::WasmExecute, Rights::EXECUTE).is_err() {
            return Err(WasmError::CapabilityDenied);
        }

        let func_idx = *self
            .module
            .exports
            .get(name)
            .ok_or_else(|| WasmError::FunctionNotFound(name.to_string()))?;

        self.execute_function(func_idx, args)
    }

    /// Executes function by index.
    pub fn execute_function(
        &mut self,
        func_idx: usize,
        args: &[Value],
    ) -> Result<Option<Value>, WasmError> {
        let num_imports = self.module.host_imports.len();

        if func_idx < num_imports {
            let host_fn = self.module.host_imports[func_idx].clone();
            return (host_fn.handler)(self, args);
        }

        let internal_idx = func_idx - num_imports;
        let func = self
            .module
            .functions
            .get(internal_idx)
            .ok_or_else(|| WasmError::FunctionNotFound(alloc::format!("Index {}", func_idx)))?
            .clone();

        let mut locals = Vec::with_capacity(func.num_params + func.num_locals);
        locals.extend_from_slice(args);
        while locals.len() < func.num_params + func.num_locals {
            locals.push(Value::I32(0));
        }

        let mut pc = 0;
        let code = &func.bytecode;

        while pc < code.len() {
            if self.fuel == 0 {
                return Err(WasmError::FuelExhausted);
            }
            self.fuel -= 1;
            self.instructions_executed += 1;

            let opcode = code[pc];
            pc += 1;

            match opcode {
                opcodes::NOP => {}
                opcodes::UNREACHABLE => return Err(WasmError::HostCallFailed("unreachable".to_string())),
                opcodes::RETURN | opcodes::END => break,
                opcodes::DROP => {
                    self.pop()?;
                }
                opcodes::I32_CONST => {
                    let val = read_var_i32(code, &mut pc)?;
                    self.push(Value::I32(val));
                }
                opcodes::I64_CONST => {
                    let val = read_var_i64(code, &mut pc)?;
                    self.push(Value::I64(val));
                }
                opcodes::LOCAL_GET => {
                    let idx = read_var_u32(code, &mut pc)? as usize;
                    let val = locals.get(idx).copied().ok_or(WasmError::TypeMismatch)?;
                    self.push(val);
                }
                opcodes::LOCAL_SET => {
                    let idx = read_var_u32(code, &mut pc)? as usize;
                    let val = self.pop()?;
                    if idx >= locals.len() {
                        return Err(WasmError::TypeMismatch);
                    }
                    locals[idx] = val;
                }
                opcodes::I32_ADD => {
                    let b = self.pop()?.as_i32()?;
                    let a = self.pop()?.as_i32()?;
                    self.push(Value::I32(a.wrapping_add(b)));
                }
                opcodes::I32_SUB => {
                    let b = self.pop()?.as_i32()?;
                    let a = self.pop()?.as_i32()?;
                    self.push(Value::I32(a.wrapping_sub(b)));
                }
                opcodes::I32_MUL => {
                    let b = self.pop()?.as_i32()?;
                    let a = self.pop()?.as_i32()?;
                    self.push(Value::I32(a.wrapping_mul(b)));
                }
                opcodes::I32_LOAD => {
                    let _align = read_var_u32(code, &mut pc)?;
                    let offset = read_var_u32(code, &mut pc)? as usize;
                    let base = self.pop()?.as_i32()? as usize;
                    let addr = base.checked_add(offset).ok_or(WasmError::OutOfBoundsMemory)?;
                    let val = self.memory.read_i32(addr)?;
                    self.push(Value::I32(val));
                }
                opcodes::I32_STORE => {
                    let _align = read_var_u32(code, &mut pc)?;
                    let offset = read_var_u32(code, &mut pc)? as usize;
                    let val = self.pop()?.as_i32()?;
                    let base = self.pop()?.as_i32()? as usize;
                    let addr = base.checked_add(offset).ok_or(WasmError::OutOfBoundsMemory)?;
                    self.memory.write_i32(addr, val)?;
                }
                opcodes::CALL => {
                    let target_idx = read_var_u32(code, &mut pc)? as usize;
                    let num_params = if target_idx < num_imports {
                        self.module.host_imports[target_idx].num_params
                    } else {
                        self.module.functions[target_idx - num_imports].num_params
                    };

                    let mut call_args = Vec::with_capacity(num_params);
                    for _ in 0..num_params {
                        call_args.push(self.pop()?);
                    }
                    call_args.reverse();

                    let ret = self.execute_function(target_idx, &call_args)?;
                    if let Some(r) = ret {
                        self.push(r);
                    }
                }
                unknown => return Err(WasmError::UnknownOpcode(unknown)),
            }
        }

        Ok(self.stack.pop())
    }
}

/// Host function: SHA-256 digest inside sandboxed linear memory.
/// Signature: `inter_crypto_hash(src_addr: i32, src_len: i32, dest_addr: i32) -> i32`
pub fn host_crypto_hash(vm: &mut WasmVm, args: &[Value]) -> Result<Option<Value>, WasmError> {
    if args.len() < 3 {
        return Err(WasmError::TypeMismatch);
    }
    let src_addr = args[0].as_i32()? as usize;
    let src_len = args[1].as_i32()? as usize;
    let dest_addr = args[2].as_i32()? as usize;

    let src_data = vm.memory.read_slice(src_addr, src_len)?;
    let digest = Sha256::digest(src_data);
    vm.memory.write_slice(dest_addr, &digest)?;

    Ok(Some(Value::I32(32))) // Returns 32 bytes written
}

// Bytecode LEB128 decoding helpers
fn read_var_u32(code: &[u8], pc: &mut usize) -> Result<u32, WasmError> {
    let mut result: u32 = 0;
    let mut shift = 0;
    loop {
        if *pc >= code.len() {
            return Err(WasmError::OutOfBoundsMemory);
        }
        let byte = code[*pc];
        *pc += 1;
        result |= ((byte & 0x7F) as u32) << shift;
        if (byte & 0x80) == 0 {
            break;
        }
        shift += 7;
        if shift >= 35 {
            return Err(WasmError::TypeMismatch);
        }
    }
    Ok(result)
}

fn read_var_i32(code: &[u8], pc: &mut usize) -> Result<i32, WasmError> {
    let mut result: i32 = 0;
    let mut shift = 0;
    let mut byte;
    loop {
        if *pc >= code.len() {
            return Err(WasmError::OutOfBoundsMemory);
        }
        byte = code[*pc];
        *pc += 1;
        result |= ((byte & 0x7F) as i32) << shift;
        shift += 7;
        if (byte & 0x80) == 0 {
            break;
        }
        if shift >= 35 {
            return Err(WasmError::TypeMismatch);
        }
    }
    if (shift < 32) && ((byte & 0x40) != 0) {
        result |= !0 << shift;
    }
    Ok(result)
}

fn read_var_i64(code: &[u8], pc: &mut usize) -> Result<i64, WasmError> {
    let mut result: i64 = 0;
    let mut shift = 0;
    let mut byte;
    loop {
        if *pc >= code.len() {
            return Err(WasmError::OutOfBoundsMemory);
        }
        byte = code[*pc];
        *pc += 1;
        result |= ((byte & 0x7F) as i64) << shift;
        shift += 7;
        if (byte & 0x80) == 0 {
            break;
        }
        if shift >= 70 {
            return Err(WasmError::TypeMismatch);
        }
    }
    if (shift < 64) && ((byte & 0x40) != 0) {
        result |= !0 << shift;
    }
    Ok(result)
}
