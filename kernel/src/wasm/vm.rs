// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! WebAssembly Stack Machine Interpreter (kernel-internal)
//!
//! Provides a pure `no_std` execution loop for interpreter bytecode with:
//! - Strict fuel metering to prevent infinite loops and denial of service.
//! - An explicit native call-depth limit and operand-stack cap. Fuel is
//!   charged per instruction, so without these a CALL chain could recurse
//!   until the Ring-0 native stack or the kernel heap is exhausted.
//! - Sandboxed linear memory load/store bounds checking and hard 64-page / 4 MiB ceiling.
//! - Clean host call bindings for capability tools.
//!
//! =============================================================================
//! EXECUTION DOMAIN: WHAT EXISTS AND WHAT DOES NOT (PLATFORM TRUTH)
//! =============================================================================
//! WHAT EXISTS:
//! - This interpreter runs in Ring 0 and is used ONLY by trusted kernel
//!   self-tests (the boot arithmetic and fuel checks in `kernel/src/main.rs`).
//!   It is not app-reachable: the `WasmExecute` syscall returns
//!   `FeatureDisabled`, `spawn_verified_rapp_process` rejects `RappArch::Wasm`,
//!   and Wasm-architecture RAPP launches go to the browser app, not here.
//!
//! WHAT DOES NOT EXIST (NOT IMPLEMENTED):
//! - No enforced Ring-0/Ring-3 execution boundary. `WasmExecutionDomain`,
//!   `designate_execution_domain` and `is_ring0_execution_permitted` below are
//!   designation/advisory helpers with no production caller; nothing routes
//!   bytecode by them and the interpreter does not consult them.
//! - No registered `wasmd` execution service. A `wasmd` scaffold binary exists
//!   in `userspace/system-service` (`run_wasm`), but it is not in the kernel
//!   service registry, is not embedded or spawned by the kernel, contains no
//!   interpreter, and only logs "WASM"-magic IPC messages that nothing sends.
//!   Service ID 12 is `ServiceId::Inference`, not a WASM daemon.
//! - No WASM binary-container decoder: modules are built programmatically
//!   (`WasmModule::add_function` takes this interpreter's opcode stream, not
//!   `\0asm` module bytes). No magic, version or section-length parsing exists.
//! =============================================================================

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::wasm::memory::{WasmMemory, WASM_MAX_MEMORY_BYTES, WASM_MAX_PAGES_LIMIT, WASM_PAGE_SIZE};
use crate::wasm::types::{opcodes, Value, WasmError};

/// Maximum native CALL recursion depth.
///
/// WHY an explicit depth limit: fuel is charged per instruction, so a call
/// chain could otherwise recurse as deep as the fuel budget allows. This
/// interpreter runs in Ring 0, where a native stack overflow is a whole-device
/// fault with no guard-page recovery. Task stacks are 128 KiB
/// (`crate::task::TASK_STACK_SIZE`) and the boot kernel stack is 512 KiB;
/// debug builds carry fat frames, so 32 frames keeps worst-case native stack
/// use far below the smaller of the two while leaving generous headroom for
/// the arithmetic and self-test modules this interpreter actually runs.
pub const WASM_MAX_CALL_DEPTH: usize = 32;

/// Maximum number of values on the shared operand stack (16 KiB of `Value`).
///
/// WHY a cap: bytecode can push without popping (e.g. a long `i32.const`
/// run); without a cap the only limit is the kernel heap, and exhaustion
/// there halts the device (`interos_alloc_error`).
pub const WASM_MAX_STACK_VALUES: usize = 1024;

/// Maximum parameters plus locals per function frame, and maximum arguments
/// per CALL (16 KiB of `Value` per frame).
///
/// WHY a cap: `num_params`/`num_locals` come from the module and drive
/// allocations before any stack pop can fail, so an attacker-controlled
/// declaration is an allocation-amplification vector. Bounding it by the
/// operand-stack cap means a function can never declare more parameters than
/// the whole stack could hold. Worst-case heap across all frames is bounded
/// by `WASM_MAX_CALL_DEPTH * 2 * WASM_MAX_LOCALS * size_of::<Value>()` (~1 MiB).
pub const WASM_MAX_LOCALS: usize = 1024;

/// Placeholder service ID for a userspace WASM execution daemon that is NOT IMPLEMENTED.
///
/// WHY this names no registered service: there is no `wasmd` entry in
/// `crate::services::ServiceId` (IDs 0-15 are all assigned; ID 12 is
/// `ServiceId::Inference`). The previous value 12 therefore named the
/// inference daemon and would misroute untrusted bytecode if anything ever
/// consumed this designation. `u8::MAX` is deliberately outside the registry:
/// `ServiceId::from_u8(WASMD_SERVICE_ID)` is `None`. This constant is a
/// designation placeholder only; no production code consumes it and it must
/// not be used as an IPC routing target.
pub const WASMD_SERVICE_ID: u8 = u8::MAX;

/// Execution-domain designation for WASM bytecode payloads.
///
/// DESIGNATION ONLY — NOT AN ENFORCED BOUNDARY. No production caller consumes
/// this type; nothing dispatches, routes or isolates bytecode based on it.
/// The variants describe intended policy, not implemented enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmExecutionDomain {
    /// Trusted kernel-internal execution in Ring 0 (self-tests and deterministic
    /// checks only). This is the only domain that actually exists: the
    /// interpreter always runs in Ring 0 regardless of any designation.
    KernelInternal,
    /// Intended Ring 3 userspace execution via a `wasmd` daemon that is
    /// NOT IMPLEMENTED as an execution service: the userspace scaffold is not
    /// registered, spawned or routed to by the kernel and executes no bytecode.
    /// `service_id` carries `WASMD_SERVICE_ID`, which names no registered service.
    UserspaceDaemon { service_id: u8 },
}

/// Pure designation helper: maps a trust flag to an intended execution domain.
///
/// DESIGNATION ONLY — NOT ROUTING OR ENFORCEMENT. No production caller exists
/// (only `cfg(test)`); no dispatcher, IPC route or service consumes the result.
/// Returning `UserspaceDaemon` does not move execution out of Ring 0: the
/// in-kernel interpreter always runs in Ring 0, and the `wasmd` execution
/// service is NOT IMPLEMENTED.
pub fn designate_execution_domain(is_trusted_kernel_task: bool) -> WasmExecutionDomain {
    if is_trusted_kernel_task {
        WasmExecutionDomain::KernelInternal
    } else {
        WasmExecutionDomain::UserspaceDaemon {
            service_id: WASMD_SERVICE_ID,
        }
    }
}

/// Advisory predicate only: reports the intended policy for a trust flag.
///
/// NOT AN ENFORCED CHECK. The interpreter does not consult this function and
/// no production path calls it (only `cfg(test)`); it rejects nothing. The
/// real Ring-0 protections on the execution path are fuel metering, linear
/// memory bounds, the call-depth limit and the operand-stack cap.
pub fn is_ring0_execution_permitted(is_trusted: bool) -> bool {
    is_trusted
}

/// Converts a WASM linear-memory address operand into a host offset.
///
/// A WASM linear address is an unsigned 32-bit quantity, so a negative `i32`
/// operand denotes an address at or above 4 GiB and is always out of bounds for
/// this sandbox. It must not be reinterpreted as a `usize`: on a 64-bit host
/// that sign-extends `-1` to `0xFFFF_FFFF_FFFF_FFFF`, turning a sandbox trap
/// into a kernel-space page fault at exactly that address.
fn linear_address(addr: i32) -> Result<usize, WasmError> {
    if addr < 0 {
        return Err(WasmError::OutOfBoundsMemory);
    }
    Ok(addr as usize)
}

/// Representation of an internal WASM bytecode function.
#[derive(Debug, Clone)]
pub struct WasmFunction {
    pub name: String,
    pub num_params: usize,
    pub num_locals: usize,
    /// WHY shared (`Rc`) instead of an owned `Vec`: every CALL frame used to
    /// `.cloned()` the whole function, copying the entire bytecode `Vec` per
    /// frame — heap amplification proportional to depth × bytecode size.
    /// A refcount bump per frame removes the copy; the bytecode is immutable
    /// during execution, and the VM is never shared across threads (smp 1,
    /// boot-self-test-local), so `Rc`'s lack of `Send`/`Sync` is not a constraint.
    pub bytecode: Rc<[u8]>,
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

/// A programmatically built WASM module containing functions and host imports.
///
/// NOT A BINARY CONTAINER: there is no WASM binary decoder (NOT IMPLEMENTED).
/// `add_function` takes this interpreter's raw opcode stream, not `\0asm`
/// module bytes; no magic, version or section-length parsing exists, and
/// `WasmError::InvalidMagic` / `WasmError::UnsupportedVersion` are reserved
/// variants that are never constructed.
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
    ///
    /// `bytecode` is this interpreter's opcode stream (see `opcodes`), not a
    /// standard WASM binary module. `num_params`/`num_locals` are bounded at
    /// execution time by `WASM_MAX_LOCALS`; the bytecode is shared (`Rc`) so
    /// a CALL frame never copies it.
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
            bytecode: bytecode.into(),
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
    /// Current native CALL recursion depth. Private accounting: incremented
    /// and restored by `execute_function` on every path, including errors.
    call_depth: usize,
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
            call_depth: 0,
        }
    }

    /// Pushes a value onto the evaluation stack, enforcing the operand-stack cap.
    ///
    /// WHY fallible: bytecode can push without popping, and an unbounded stack
    /// would grow until the kernel heap halts the device. A failed bounded
    /// reservation is reported as the same sandbox resource error rather than
    /// reaching the global halt-on-OOM policy (`interos_alloc_error`).
    pub fn push(&mut self, val: Value) -> Result<(), WasmError> {
        if self.stack.len() >= WASM_MAX_STACK_VALUES {
            return Err(WasmError::StackOverflow);
        }
        self.stack.try_reserve(1).map_err(|_| WasmError::StackOverflow)?;
        self.stack.push(val);
        Ok(())
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
    ///
    /// WHY a depth-guard wrapper: CALL recurses natively through this method,
    /// and fuel is charged per instruction, so without an explicit limit a
    /// call chain could recurse as deep as the fuel budget allows and overflow
    /// the Ring-0 stack (a whole-device fault). The guard also bounds host
    /// handlers that re-enter the VM. Depth is restored on every exit path,
    /// including errors, because no `?` runs between increment and decrement.
    pub fn execute_function(
        &mut self,
        func_idx: usize,
        args: &[Value],
    ) -> Result<Option<Value>, WasmError> {
        if self.call_depth >= WASM_MAX_CALL_DEPTH {
            return Err(WasmError::CallDepthExceeded);
        }
        self.call_depth += 1;
        let result = self.execute_function_inner(func_idx, args);
        self.call_depth -= 1;
        result
    }

    fn execute_function_inner(
        &mut self,
        func_idx: usize,
        args: &[Value],
    ) -> Result<Option<Value>, WasmError> {
        // 1. Check if it's a host import
        if func_idx < self.module.host_imports.len() {
            let host_fn = self.module.host_imports[func_idx].handler;
            return host_fn(self, args);
        }

        // 2. Otherwise, look up internal function.
        // WHY copy the signature and share the bytecode (`Rc`) instead of
        // `.cloned()`ing the whole function: the old clone copied the entire
        // bytecode `Vec` (and the name `String`) on every CALL frame, so heap
        // use grew with depth × bytecode size. The refcount bump is O(1) and
        // keeps the bytecode alive even if a host handler mutates the module.
        let internal_idx = func_idx - self.module.host_imports.len();
        let (num_params, num_locals, bytecode) = {
            let func = self
                .module
                .functions
                .get(internal_idx)
                .ok_or_else(|| WasmError::FunctionNotFound(internal_idx.to_string()))?;
            (func.num_params, func.num_locals, Rc::clone(&func.bytecode))
        };
        let bytecode = bytecode.as_ref();

        // Enforce sandbox memory quota limits: reject execution if linear memory exceeds 64 pages / 4 MiB limit
        if self.memory.size() > WASM_MAX_MEMORY_BYTES || self.memory.max_pages() > WASM_MAX_PAGES_LIMIT {
            return Err(WasmError::OutOfBoundsMemory);
        }

        // WHY bound the declaration before allocating: `num_params`/`num_locals`
        // come from the module and drive the locals allocation before any pop
        // can fail, so a huge declaration is an allocation-amplification vector
        // against a heap whose exhaustion policy halts the device. A failed
        // bounded reservation is reported as the same sandbox resource error
        // instead of reaching that policy.
        let total_locals = num_params.saturating_add(num_locals);
        if total_locals > WASM_MAX_LOCALS || args.len() > WASM_MAX_LOCALS {
            return Err(WasmError::TooManyLocals);
        }
        let mut locals = Vec::new();
        locals
            .try_reserve(total_locals.max(args.len()))
            .map_err(|_| WasmError::TooManyLocals)?;

        // Initialize locals: parameters followed by zero-initialized locals
        for &arg in args {
            locals.push(arg);
        }
        while locals.len() < total_locals {
            locals.push(Value::I32(0));
        }

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
                    self.push(Value::I32(val))?;
                }
                opcodes::I64_CONST => {
                    let (val, len) = read_leb128_i64(&bytecode[pc..])?;
                    pc += len;
                    self.push(Value::I64(val))?;
                }
                opcodes::LOCAL_GET => {
                    let (idx, len) = read_leb128_u32(&bytecode[pc..])?;
                    pc += len;
                    let val = locals
                        .get(idx as usize)
                        .copied()
                        .ok_or(WasmError::StackUnderflow)?;
                    self.push(val)?;
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
                    self.push(Value::I32(a.wrapping_add(b)))?;
                }
                opcodes::I32_SUB => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(a.wrapping_sub(b)))?;
                }
                opcodes::I32_MUL => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(a.wrapping_mul(b)))?;
                }
                opcodes::I32_AND => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(a & b))?;
                }
                opcodes::I32_OR => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(a | b))?;
                }
                opcodes::I32_XOR => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(a ^ b))?;
                }
                opcodes::I32_EQ => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(if a == b { 1 } else { 0 }))?;
                }
                opcodes::I32_NE => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(if a != b { 1 } else { 0 }))?;
                }
                opcodes::I32_LT_S => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(if a < b { 1 } else { 0 }))?;
                }
                opcodes::I32_GT_S => {
                    let b = self.pop_i32()?;
                    let a = self.pop_i32()?;
                    self.push(Value::I32(if a > b { 1 } else { 0 }))?;
                }
                opcodes::I32_LOAD => {
                    let (_align, len1) = read_leb128_u32(&bytecode[pc..])?;
                    pc += len1;
                    let (offset, len2) = read_leb128_u32(&bytecode[pc..])?;
                    pc += len2;
                    let base = linear_address(self.pop_i32()?)?;
                    let addr = base.checked_add(offset as usize).ok_or(WasmError::OutOfBoundsMemory)?;
                    let val = self.memory.read_i32(addr)?;
                    self.push(Value::I32(val))?;
                }
                opcodes::I32_STORE => {
                    let (_align, len1) = read_leb128_u32(&bytecode[pc..])?;
                    pc += len1;
                    let (offset, len2) = read_leb128_u32(&bytecode[pc..])?;
                    pc += len2;
                    let val = self.pop_i32()?;
                    let base = linear_address(self.pop_i32()?)?;
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

                    // WHY bound before allocating: `param_count` comes from the
                    // module and drove `Vec::with_capacity` before any pop could
                    // fail, so a huge declared signature was an allocation
                    // amplification. A callee can never legitimately take more
                    // arguments than the whole operand stack could hold.
                    if param_count > WASM_MAX_LOCALS {
                        return Err(WasmError::TooManyLocals);
                    }
                    let mut call_args = Vec::new();
                    call_args
                        .try_reserve(param_count)
                        .map_err(|_| WasmError::TooManyLocals)?;
                    for _ in 0..param_count {
                        call_args.push(self.pop()?);
                    }
                    call_args.reverse(); // Stack order to argument order

                    let result = self.execute_function(target, &call_args)?;
                    if let Some(res) = result {
                        self.push(res)?;
                    }
                }
                opcodes::MEMORY_SIZE => {
                    if pc >= bytecode.len() {
                        return Err(WasmError::Trap("Truncated memory.size instruction".to_string()));
                    }
                    let _reserved = bytecode[pc];
                    pc += 1;
                    let pages = (self.memory.size() / WASM_PAGE_SIZE) as i32;
                    self.push(Value::I32(pages))?;
                }
                opcodes::MEMORY_GROW => {
                    if pc >= bytecode.len() {
                        return Err(WasmError::Trap("Truncated memory.grow instruction".to_string()));
                    }
                    let _reserved = bytecode[pc];
                    pc += 1;
                    let delta = self.pop_i32()?;
                    if delta < 0 {
                        self.push(Value::I32(-1))?;
                    } else {
                        match self.memory.grow(delta as usize) {
                            Ok(prev_pages) => self.push(Value::I32(prev_pages as i32))?,
                            Err(_) => self.push(Value::I32(-1))?,
                        }
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

    /// Returns the current active memory sandbox size in bytes.
    pub fn memory_size(&self) -> usize {
        self.memory.size()
    }

    /// Explicitly attempts to grow the VM's linear memory sandbox by `delta_pages`,
    /// enforcing the strict 64-page / 4 MiB platform limit.
    pub fn grow_memory(&mut self, delta_pages: usize) -> Result<usize, WasmError> {
        self.memory.grow(delta_pages)
    }

    /// Invokes an exported function with Resource Manager fuel and memory accounting.
    pub fn invoke_export_with_resource_manager(
        &mut self,
        name: &str,
        args: &[Value],
        rm: &mut crate::resource::ResourceManager,
    ) -> Result<Option<Value>, WasmError> {
        rm.meter_wasm_instruction_fuel(self.fuel)
            .map_err(|_| WasmError::FuelExhausted)?;

        let result = self.invoke_export(name, args);

        let mem_size = self.memory.size();
        if mem_size > 0 {
            let _ = rm.allocate_wasm_memory(mem_size);
        }

        result
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm::memory::WasmMemory;

    /// Builds a one-function module that exports `probe` and runs it.
    fn run(code: Vec<u8>, args: &[Value], fuel: u64) -> Result<Option<Value>, WasmError> {
        let mut module = WasmModule::new();
        let idx = module.add_function("probe", args.len(), 0, code);
        module.add_export("probe", idx);
        let mut vm = WasmVm::new(module, WasmMemory::new(1, 1), fuel);
        vm.invoke_export("probe", args)
    }

    /// `i32.const -1; i32.load align=2 offset=0; end`
    fn i32_load_of_minus_one() -> Vec<u8> {
        alloc::vec![
            opcodes::I32_CONST,
            0x7F,
            opcodes::I32_LOAD,
            0x02,
            0x00,
            opcodes::END,
        ]
    }

    /// `i32.const -1; i32.const 0; i32.store align=2 offset=0; end`
    ///
    /// `i32.store` pops the value first, so the base address is pushed first.
    fn i32_store_at_minus_one() -> Vec<u8> {
        alloc::vec![
            opcodes::I32_CONST,
            0x7F,
            opcodes::I32_CONST,
            0x00,
            opcodes::I32_STORE,
            0x02,
            0x00,
            opcodes::END,
        ]
    }

    /// The reported boot signature: CR2 = 0xFFFF_FFFF_FFFF_FFFF with a read
    /// error code of zero. A negative WASM address reinterpreted as a `usize`
    /// produces exactly that address, so the interpreter has to trap in the
    /// sandbox instead of dereferencing it.
    #[test]
    fn a_negative_i32_load_address_traps_instead_of_reaching_the_host() {
        assert_eq!(
            linear_address(-1),
            Err(WasmError::OutOfBoundsMemory),
            "-1 as a usize is 0xFFFF_FFFF_FFFF_FFFF, the reported fault address"
        );
        assert_eq!(
            linear_address(i32::MIN),
            Err(WasmError::OutOfBoundsMemory)
        );
        assert_eq!(linear_address(0), Ok(0));
        assert_eq!(linear_address(0x7FFF_FFFF), Ok(0x7FFF_FFFF));

        assert_eq!(
            run(i32_load_of_minus_one(), &[], 16),
            Err(WasmError::OutOfBoundsMemory)
        );
    }

    #[test]
    fn a_negative_i32_store_address_traps_instead_of_writing_the_host() {
        assert_eq!(
            run(i32_store_at_minus_one(), &[], 16),
            Err(WasmError::OutOfBoundsMemory)
        );
    }

    #[test]
    fn an_in_bounds_store_and_load_round_trips() {
        // i32.const 64; i32.const 0x1234; i32.store; then load it back.
        let code = alloc::vec![
            opcodes::I32_CONST,
            0xC0,
            0x00,
            opcodes::I32_CONST,
            0xB4,
            0x24,
            opcodes::I32_STORE,
            0x02,
            0x00,
            opcodes::I32_CONST,
            0xC0,
            0x00,
            opcodes::I32_LOAD,
            0x02,
            0x00,
            opcodes::END,
        ];
        assert_eq!(run(code, &[], 32), Ok(Some(Value::I32(0x1234))));
    }

    #[test]
    fn an_out_of_bounds_unsigned_address_still_traps() {
        // Address 70000 is past the single 64 KiB page.
        let code = alloc::vec![
            opcodes::I32_CONST,
            0xF0,
            0xA2,
            0x04,
            opcodes::I32_LOAD,
            0x02,
            0x00,
            opcodes::END,
        ];
        assert_eq!(run(code, &[], 16), Err(WasmError::OutOfBoundsMemory));
    }

    #[test]
    fn runtime_isolation_bridge_designation() {
        assert_eq!(
            designate_execution_domain(true),
            WasmExecutionDomain::KernelInternal
        );
        assert_eq!(
            designate_execution_domain(false),
            WasmExecutionDomain::UserspaceDaemon {
                service_id: WASMD_SERVICE_ID,
            }
        );
        assert!(is_ring0_execution_permitted(true));
        assert!(!is_ring0_execution_permitted(false));
        // The designation placeholder must name no registered service: there is
        // no `wasmd` in the service registry, and ID 12 belongs to Inference.
        // This pins the placeholder's honesty; it does NOT test enforced
        // isolation, which does not exist (see the module documentation).
        assert!(crate::services::ServiceId::from_u8(WASMD_SERVICE_ID).is_none());
        assert_ne!(WASMD_SERVICE_ID, crate::services::ServiceId::Inference as u8);
    }

    #[test]
    fn memory_size_and_grow_instructions_enforce_quotas() {
        // Test memory.size (0x3F, 0x00)
        let size_code = alloc::vec![
            opcodes::MEMORY_SIZE,
            0x00, // reserved flag
            opcodes::END,
        ];
        assert_eq!(run(size_code, &[], 16), Ok(Some(Value::I32(1))));

        // Test memory.grow by 2 pages
        let mut module = WasmModule::new();
        let grow_code = alloc::vec![
            opcodes::I32_CONST,
            0x02, // grow by 2 pages
            opcodes::MEMORY_GROW,
            0x00,
            opcodes::END,
        ];
        let idx = module.add_function("grow", 0, 0, grow_code);
        module.add_export("grow", idx);
        let mut vm = WasmVm::new(module, WasmMemory::new(1, 4), 16);
        let res = vm.invoke_export("grow", &[]);
        assert_eq!(res, Ok(Some(Value::I32(1)))); // Returns previous size: 1 page
        assert_eq!(vm.memory_size(), 3 * WASM_PAGE_SIZE);

        // Exceeding max_pages via grow returns -1
        let mut exceed_module = WasmModule::new();
        let exceed_code = alloc::vec![
            opcodes::I32_CONST,
            0x10, // grow by 16 pages, exceeding 4-page max
            opcodes::MEMORY_GROW,
            0x00,
            opcodes::END,
        ];
        let ex_idx = exceed_module.add_function("exceed", 0, 0, exceed_code);
        exceed_module.add_export("exceed", ex_idx);
        let mut exceed_vm = WasmVm::new(exceed_module, WasmMemory::new(1, 4), 16);
        let ex_res = exceed_vm.invoke_export("exceed", &[]);
        assert_eq!(ex_res, Ok(Some(Value::I32(-1)))); // Fails with -1
        assert_eq!(exceed_vm.memory_size(), WASM_PAGE_SIZE);
    }

    #[test]
    fn resource_manager_accounting_with_vm() {
        let mut rm = crate::resource::ResourceManager::new();
        let mut module = WasmModule::new();
        let code = alloc::vec![
            opcodes::I32_CONST,
            0x2A,
            opcodes::RETURN,
            opcodes::END,
        ];
        let idx = module.add_function("ans", 0, 0, code);
        module.add_export("ans", idx);
        let mut vm = WasmVm::new(module, WasmMemory::new(1, 1), 50);

        let res = vm.invoke_export_with_resource_manager("ans", &[], &mut rm);
        assert_eq!(res, Ok(Some(Value::I32(42))));
        assert_eq!(rm.wasm_fuel_metered_this_frame, 50);
        assert_eq!(rm.wasm_memory_allocated_bytes, WASM_PAGE_SIZE);
    }

    /// Builds a chain of `len` functions where each CALLs the next and the last
    /// returns 42, exports the first as "top", and runs it through
    /// `invoke_export` — the same entry point the boot self-test uses.
    /// Indices stay below 128 so every CALL target is a single-byte LEB128.
    fn run_call_chain(len: usize, fuel: u64) -> (Result<Option<Value>, WasmError>, WasmVm) {
        let mut module = WasmModule::new();
        for i in 0..len {
            let code = if i + 1 == len {
                alloc::vec![opcodes::I32_CONST, 42, opcodes::END]
            } else {
                alloc::vec![opcodes::CALL, (i + 1) as u8, opcodes::END]
            };
            module.add_function("f", 0, 0, code);
        }
        module.add_export("top", 0);
        let mut vm = WasmVm::new(module, WasmMemory::new(1, 1), fuel);
        let res = vm.invoke_export("top", &[]);
        (res, vm)
    }

    /// Positive control: the exact boot self-test module from `kernel/src/main.rs`
    /// (Test 8f) must produce the same result through the same entry point.
    #[test]
    fn boot_self_test_arithmetic_is_unchanged() {
        let mut module = WasmModule::new();
        let code = alloc::vec![
            opcodes::LOCAL_GET, 0,
            opcodes::LOCAL_GET, 1,
            opcodes::I32_ADD,
            opcodes::I32_CONST, 10,
            opcodes::I32_MUL,
            opcodes::RETURN,
            opcodes::END,
        ];
        let idx = module.add_function("compute", 2, 0, code);
        module.add_export("compute", idx);
        let mut vm = WasmVm::new(module, WasmMemory::new(1, 1), 100);
        assert_eq!(
            vm.invoke_export("compute", &[Value::I32(5), Value::I32(7)]),
            Ok(Some(Value::I32(120)))
        );
    }

    /// A chain exactly at the depth limit still executes and unwinds cleanly.
    #[test]
    fn a_call_chain_at_the_depth_limit_still_executes() {
        let (res, vm) = run_call_chain(WASM_MAX_CALL_DEPTH, 10_000);
        assert_eq!(res, Ok(Some(Value::I32(42))));
        assert_eq!(vm.call_depth, 0, "depth must be restored after success");
    }

    /// One frame past the limit must be a sandbox error, not a native stack
    /// overflow. The chain is bounded (33 frames) so a broken limit fails this
    /// assertion cleanly instead of crashing the test host.
    #[test]
    fn a_call_chain_one_past_the_depth_limit_is_refused() {
        let (res, vm) = run_call_chain(WASM_MAX_CALL_DEPTH + 1, 10_000);
        assert_eq!(res, Err(WasmError::CallDepthExceeded));
        assert_eq!(vm.call_depth, 0, "depth must be restored on the error path");
        assert!(vm.fuel > 0, "the depth limit must bind while fuel remains");
    }

    /// Fuel and depth are independent limits: ample fuel cannot buy deeper
    /// recursion, and a tiny fuel budget still stops a shallow chain first.
    #[test]
    fn depth_binds_independently_of_fuel() {
        // Self-recursion with a huge fuel budget: only the depth limit can stop
        // it. Each frame executes exactly one CALL before recursing, so exactly
        // WASM_MAX_CALL_DEPTH instructions run before the guard refuses entry.
        let mut module = WasmModule::new();
        let idx = module.add_function("recurse", 0, 0, alloc::vec![opcodes::CALL, 0, opcodes::END]);
        module.add_export("recurse", idx);
        let mut vm = WasmVm::new(module, WasmMemory::new(1, 1), 1_000_000);
        assert_eq!(vm.invoke_export("recurse", &[]), Err(WasmError::CallDepthExceeded));
        assert_eq!(vm.instructions_executed, WASM_MAX_CALL_DEPTH as u64);
        assert_eq!(vm.fuel, 1_000_000 - WASM_MAX_CALL_DEPTH as u64);
        assert_eq!(vm.call_depth, 0);

        // The same module with fuel below the depth limit exhausts fuel first.
        let mut shallow = WasmVm::new(vm.module.clone(), WasmMemory::new(1, 1), 3);
        assert_eq!(shallow.invoke_export("recurse", &[]), Err(WasmError::FuelExhausted));
    }

    /// Bytecode that pushes without popping must hit the operand-stack cap
    /// with a proper error instead of growing the stack until the heap halts.
    #[test]
    fn operand_stack_overflow_is_refused() {
        let mut code = Vec::new();
        for _ in 0..=WASM_MAX_STACK_VALUES {
            code.push(opcodes::I32_CONST);
            code.push(0);
        }
        code.push(opcodes::END);
        let mut module = WasmModule::new();
        let idx = module.add_function("fill", 0, 0, code);
        module.add_export("fill", idx);
        let mut vm = WasmVm::new(module, WasmMemory::new(1, 1), 4096);
        assert_eq!(vm.invoke_export("fill", &[]), Err(WasmError::StackOverflow));
        assert_eq!(vm.stack.len(), WASM_MAX_STACK_VALUES);
    }

    /// An attacker-controlled huge `num_params`/`num_locals` must be refused
    /// before the amplifying allocation, on both the CALL path and direct entry.
    #[test]
    fn a_huge_param_count_is_refused_before_allocation() {
        const HUGE: usize = 1_000_000;

        // CALL path: the callee (function index 0, no host imports) declares a
        // million parameters. The old code did Vec::with_capacity(1_000_000)
        // (16 MB) before the first pop could fail; the cap must refuse first.
        let mut module = WasmModule::new();
        let victim = module.add_function("victim", HUGE, 0, alloc::vec![opcodes::END]);
        assert_eq!(victim, 0);
        let entry = module.add_function("entry", 0, 0, alloc::vec![opcodes::CALL, 0, opcodes::END]);
        module.add_export("entry", entry);
        module.add_export("victim", victim);
        let mut vm = WasmVm::new(module, WasmMemory::new(1, 1), 100);
        assert_eq!(vm.invoke_export("entry", &[]), Err(WasmError::TooManyLocals));

        // Direct entry: the locals declaration alone must be refused.
        assert_eq!(vm.invoke_export("victim", &[]), Err(WasmError::TooManyLocals));

        // Same amplification via num_locals instead of num_params.
        let mut locals_module = WasmModule::new();
        let idx = locals_module.add_function("bloated", 0, HUGE, alloc::vec![opcodes::END]);
        locals_module.add_export("bloated", idx);
        let mut locals_vm = WasmVm::new(locals_module, WasmMemory::new(1, 1), 100);
        assert_eq!(locals_vm.invoke_export("bloated", &[]), Err(WasmError::TooManyLocals));
    }

    /// Host imports still execute through the depth-guard wrapper, and the
    /// wrapper restores depth after a host call returns.
    #[test]
    fn host_import_calls_still_work_through_the_depth_guard() {
        fn host_add(_vm: &mut WasmVm, args: &[Value]) -> Result<Option<Value>, WasmError> {
            let mut sum = 0i32;
            for arg in args {
                sum = sum.wrapping_add(arg.as_i32()?);
            }
            Ok(Some(Value::I32(sum)))
        }

        let mut module = WasmModule::new();
        let host = module.add_host_import("host_add", 2, host_add);
        assert_eq!(host, 0);
        let code = alloc::vec![
            opcodes::I32_CONST, 3,
            opcodes::I32_CONST, 4,
            opcodes::CALL, 0,
            opcodes::END,
        ];
        let idx = module.add_function("entry", 0, 0, code);
        module.add_export("entry", idx);
        let mut vm = WasmVm::new(module, WasmMemory::new(1, 1), 100);
        assert_eq!(vm.invoke_export("entry", &[]), Ok(Some(Value::I32(7))));
        assert_eq!(vm.call_depth, 0);
    }
}

