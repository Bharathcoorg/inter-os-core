//! WebAssembly Stack Machine Interpreter
//!
//! Provides a pure `no_std` execution loop for WASM bytecode with:
//! - Strict fuel metering to prevent infinite loops and denial of service.
//! - Sandboxed linear memory load/store bounds checking.
//! - Clean host call bindings for MCP capability tools.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::wasm::memory::WasmMemory;
use crate::wasm::types::{opcodes, Value, WasmError};

/// Representation of an internal WASM bytecode function.
#[derive(Debug, Clone)]
pub struct WasmFunction {
    pub name: String,
    pub num_params: usize,
    pub num_locals: usize,
    pub bytecode: Vec<u8>,
}

/// Host function signature.
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

    /// Registers a host function import.
    pub fn add_host_import(&mut self, name: &str, num_params: usize, handler: HostFn) -> usize {
        let idx = self.host_imports.len();
        self.host_imports.push(HostFunction {
            name: name.to_string(),
            num_params,
            handler,
        });
        idx
    }

    /// Adds an internal bytecode function.
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

    /// Exports a function by name.
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
    /// Creates a new sandboxed virtual machine with the given module, memory, and fuel quota.
    pub fn new(module: WasmModule, memory: WasmMemory, fuel: u64) -> Self {
        Self {
            module,
            memory,
            stack: Vec::with_capacity(64),
            fuel,
            instructions_executed: 0,
        }
    }

    /// Pushes a value onto the evaluation stack.
    pub fn push(&mut self, val: Value) {
        self.stack.push(val);
    }

    /// Pops a value from the evaluation stack.
    pub fn pop(&mut self) -> Result<Value, WasmError> {
        self.stack.pop().ok_or(WasmError::StackUnderflow)
    }

    /// Pops an i32 value.
    pub fn pop_i32(&mut self) -> Result<i32, WasmError> {
        self.pop()?.as_i32()
    }

    /// Executes an exported function by name.
    pub fn invoke_export(&mut self, name: &str, args: &[Value]) -> Result<Option<Value>, WasmError> {
        let func_idx = self
            .module
            .exports
            .get(name)
            .copied()
            .ok_or_else(|| WasmError::FunctionNotFound(name.to_string()))?;

        self.execute_function(func_idx, args)
    }

    /// Executes a function by its index (handling host calls and bytecode functions).
    pub fn execute_function(
        &mut self,
        func_idx: usize,
        args: &[Value],
    ) -> Result<Option<Value>, WasmError> {
        // 1. Check if it's a host import
        if func_idx < self.module.host_imports.len() {
            let host_fn = self.module.host_imports[func_idx].handler;
            return host_fn(self, args);
        }

        // 2. Otherwise, look up internal function
        let internal_idx = func_idx - self.module.host_imports.len();
        let func = self
            .module
            .functions
            .get(internal_idx)
            .cloned()
            .ok_or_else(|| WasmError::FunctionNotFound(internal_idx.to_string()))?;

        // Initialize locals: parameters followed by zero-initialized locals
        let mut locals = Vec::with_capacity(func.num_params + func.num_locals);
        for &arg in args {
            locals.push(arg);
        }
        while locals.len() < func.num_params + func.num_locals {
            locals.push(Value::I32(0));
        }

        let bytecode = &func.bytecode;
        let mut pc = 0;

        while pc < bytecode.len() {
            // Enforce instruction fuel budget
            if self.fuel == 0 {
                return Err(WasmError::FuelExhausted);
            }
            self.fuel -= 1;
            self.instructions_executed += 1;

            let op = bytecode[pc];
            pc += 1;

            match op {
                opcodes::NOP => {}
                opcodes::DROP => {
                    self.pop()?;
                }
                opcodes::I32_CONST => {
                    let (val, len) = read_leb128_i32(&bytecode[pc..])?;
                    pc += len;
                    self.push(Value::I32(val));
                }
                opcodes::I64_CONST => {
                    let (val, len) = read_leb128_i64(&bytecode[pc..])?;
                    pc += len;
                    self.push(Value::I64(val));
                }
                opcodes::LOCAL_GET => {
                    let (idx, len) = read_leb128_u32(&bytecode[pc..])?;
                    pc += len;
                    let val = locals
                        .get(idx as usize)
                        .copied()
                        .ok_or(WasmError::StackUnderflow)?;
                    self.push(val);
                }
                opcodes::LOCAL_SET => {
                    let (idx, len) = read_leb128_u32(&bytecode[pc..])?;
                    pc += len;
                    let val = self.pop()?;
                    if (idx as usize) >= locals.len() {
                        return Err(WasmError::StackUnderflow);
                    }
                    locals[idx as usize] = val;
                }
                opcodes::LOCAL_TEE => {
                    let (idx, len) = read_leb128_u32(&bytecode[pc..])?;
                    pc += len;
                    let val = self.stack.last().copied().ok_or(WasmError::StackUnderflow)?;
                    if (idx as usize) >= locals.len() {
                        return Err(WasmError::StackUnderflow);
                    }
                    locals[idx as usize] = val;
                }
                opcodes::I32_ADD => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(a.wrapping_add(b)));
                }
                opcodes::I32_SUB => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(a.wrapping_sub(b)));
                }
                opcodes::I32_MUL => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(a.wrapping_mul(b)));
                }
                opcodes::I32_AND => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(a & b));
                }
                opcodes::I32_OR => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(a | b));
                }
                opcodes::I32_XOR => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(a ^ b));
                }
                opcodes::I32_EQ => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(if a == b { 1 } else { 0 }));
                }
                opcodes::I32_NE => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(if a != b { 1 } else { 0 }));
                }
                opcodes::I32_LT_S => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(if a < b { 1 } else { 0 }));
                }
                opcodes::I32_GT_S => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(if a > b { 1 } else { 0 }));
                }
                opcodes::I32_LOAD => {
                    let (_align, len1) = read_leb128_u32(&bytecode[pc..])?;
                    pc += len1;
                    let (offset, len2) = read_leb128_u32(&bytecode[pc..])?;
                    pc += len2;
                    let base = self.pop_i32()? as usize;
                    let addr = base.checked_add(offset as usize).ok_or(WasmError::OutOfBoundsMemory)?;
                    let val = self.memory.read_i32(addr)?;
                    self.push(Value::I32(val));
                }
                opcodes::I32_STORE => {
                    let (_align, len1) = read_leb128_u32(&bytecode[pc..])?;
                    pc += len1;
                    let (offset, len2) = read_leb128_u32(&bytecode[pc..])?;
                    pc += len2;
                    let val = self.pop_i32()?;
                    let base = self.pop_i32()? as usize;
                    let addr = base.checked_add(offset as usize).ok_or(WasmError::OutOfBoundsMemory)?;
                    self.memory.write_i32(addr, val)?;
                }
                opcodes::CALL => {
                    let (target_idx, len) = read_leb128_u32(&bytecode[pc..])?;
                    pc += len;

                    // Read argument count based on target function signature
                    let target = target_idx as usize;
                    let param_count = if target < self.module.host_imports.len() {
                        self.module.host_imports[target].num_params
                    } else {
                        let i = target - self.module.host_imports.len();
                        self.module.functions.get(i).map_or(0, |f| f.num_params)
                    };

                    let mut call_args = Vec::with_capacity(param_count);
                    for _ in 0..param_count {
                        call_args.push(self.pop()?);
                    }
                    call_args.reverse(); // Stack order to argument order

                    let result = self.execute_function(target, &call_args)?;
                    if let Some(res) = result {
                        self.push(res);
                    }
                }
                opcodes::RETURN => {
                    return Ok(self.stack.pop());
                }
                opcodes::END => {
                    break;
                }
                opcodes::UNREACHABLE => {
                    return Err(WasmError::Trap("Executed unreachable opcode".to_string()));
                }
                other => {
                    return Err(WasmError::UnknownOpcode(other));
                }
            }
        }

        Ok(self.stack.pop())
    }
}

// -----------------------------------------------------------------------------
// LEB128 Variable-Length Integer Decoders
// -----------------------------------------------------------------------------

fn read_leb128_u32(bytes: &[u8]) -> Result<(u32, usize), WasmError> {
    let mut result = 0u32;
    let mut shift = 0;
    let mut count = 0;

    for &byte in bytes {
        count += 1;
        result |= ((byte & 0x7F) as u32) << shift;
        if (byte & 0x80) == 0 {
            return Ok((result, count));
        }
        shift += 7;
        if shift >= 35 {
            return Err(WasmError::Trap("LEB128 overflow".to_string()));
        }
    }
    Err(WasmError::Trap("Truncated LEB128".to_string()))
}

fn read_leb128_i32(bytes: &[u8]) -> Result<(i32, usize), WasmError> {
    let mut result = 0i32;
    let mut shift = 0;
    let mut count = 0;

    for &byte in bytes {
        count += 1;
        result |= ((byte & 0x7F) as i32) << shift;
        shift += 7;
        if (byte & 0x80) == 0 {
            if shift < 32 && (byte & 0x40) != 0 {
                result |= !0 << shift;
            }
            return Ok((result, count));
        }
        if shift >= 35 {
            return Err(WasmError::Trap("LEB128 overflow".to_string()));
        }
    }
    Err(WasmError::Trap("Truncated LEB128".to_string()))
}

fn read_leb128_i64(bytes: &[u8]) -> Result<(i64, usize), WasmError> {
    let mut result = 0i64;
    let mut shift = 0;
    let mut count = 0;

    for &byte in bytes {
        count += 1;
        result |= ((byte & 0x7F) as i64) << shift;
        shift += 7;
        if (byte & 0x80) == 0 {
            if shift < 64 && (byte & 0x40) != 0 {
                result |= !0 << shift;
            }
            return Ok((result, count));
        }
        if shift >= 70 {
            return Err(WasmError::Trap("LEB128 overflow".to_string()));
        }
    }
    Err(WasmError::Trap("Truncated LEB128".to_string()))
}
