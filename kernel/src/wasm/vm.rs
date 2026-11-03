// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! WebAssembly Stack Machine Interpreter & Runtime Isolation Subsystem
//!
//! Provides a pure `no_std` execution loop for WASM bytecode with:
//! - Strict fuel metering to prevent infinite loops and denial of service.
//! - Sandboxed linear memory load/store bounds checking and hard 64-page / 4 MiB ceiling.
//! - Clean host call bindings for capability tools.
//!
//! =============================================================================
//! RUNTIME ISOLATION ARCHITECTURE: RING 0 SUPERVISOR VS. RING 3 WASMD DAEMON
//! =============================================================================
//! The InterOS Sovereign Smartphone Platform enforces strict execution domain separation:
//!
//! 1. Ring 0 Kernel Supervisor:
//!    - Owns MMU address spaces, capability matrix validation, hardware interrupts,
//!      and preemptive thread scheduling.
//!    - The in-kernel interpreter (`WasmVm`) is strictly restricted to trusted internal
//!      boot arithmetic validation, crypto verification harnesses, and kernel self-tests.
//!    - Untrusted user bytecode (such as third-party `.rapp` packages) MUST NEVER
//!      execute inside supervisory Ring 0 memory space.
//!
//! 2. Ring 3 Userspace `wasmd` Execution Daemon (Service ID 12):
//!    - Ring 0 designates unprivileged userspace daemon `wasmd` as the sole
//!      authoritative execution domain for untrusted `.rapp` bytecode.
//!    - `wasmd` executes in unprivileged User Mode (Ring 3), guarded by explicit,
//!      unforgeable capability handles (`CapHandle`) and non-blocking IPC mailboxes.
//!    - Memory allocations are constrained to a strict 64-page (4 MiB) sandbox limit,
//!      preventing heap exhaustion or denial-of-service in supervisory space.
//!    - Any bytecode traps, unbounded recursion, or memory faults are safely contained
//!      within the userspace runner without compromising supervisory kernel stability.
//! =============================================================================

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::wasm::memory::{WasmMemory, WASM_MAX_MEMORY_BYTES, WASM_MAX_PAGES_LIMIT, WASM_PAGE_SIZE};
use crate::wasm::types::{opcodes, Value, WasmError};

/// The authoritative userspace daemon service ID responsible for executing untrusted WASM bytecode.
pub const WASMD_SERVICE_ID: u8 = 12;

/// Authoritative execution domain designation for WASM bytecode payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmExecutionDomain {
    /// Trusted kernel-internal execution in Ring 0 (self-tests and deterministic checks only).
    KernelInternal,
    /// Authoritative isolated Ring 3 userspace execution via `wasmd` daemon.
    UserspaceDaemon { service_id: u8 },
}

/// Designates the authoritative execution domain for a given WASM module.
///
/// Untrusted application bytecode is routed to unprivileged Ring 3 `wasmd` (Service ID 12),
/// completely isolating Ring 0 from untrusted execution.
pub fn designate_execution_domain(is_trusted_kernel_task: bool) -> WasmExecutionDomain {
    if is_trusted_kernel_task {
        WasmExecutionDomain::KernelInternal
    } else {
        WasmExecutionDomain::UserspaceDaemon {
            service_id: WASMD_SERVICE_ID,
        }
    }
}

/// Bridge check validating whether Ring 0 execution is permitted for a bytecode module.
/// Untrusted bytecode is rejected from Ring 0 and must execute via `wasmd`.
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

        // Enforce sandbox memory quota limits: reject execution if linear memory exceeds 64 pages / 4 MiB limit
        if self.memory.size() > WASM_MAX_MEMORY_BYTES || self.memory.max_pages() > WASM_MAX_PAGES_LIMIT {
            return Err(WasmError::OutOfBoundsMemory);
        }

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
                    let base = linear_address(self.pop_i32()?)?;
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
                opcodes::MEMORY_SIZE => {
                    if pc >= bytecode.len() {
                        return Err(WasmError::Trap("Truncated memory.size instruction".to_string()));
                    }
                    let _reserved = bytecode[pc];
                    pc += 1;
                    let pages = (self.memory.size() / WASM_PAGE_SIZE) as i32;
                    self.push(Value::I32(pages));
                }
                opcodes::MEMORY_GROW => {
                    if pc >= bytecode.len() {
                        return Err(WasmError::Trap("Truncated memory.grow instruction".to_string()));
                    }
                    let _reserved = bytecode[pc];
                    pc += 1;
                    let delta = self.pop_i32()?;
                    if delta < 0 {
                        self.push(Value::I32(-1));
                    } else {
                        match self.memory.grow(delta as usize) {
                            Ok(prev_pages) => self.push(Value::I32(prev_pages as i32)),
                            Err(_) => self.push(Value::I32(-1)),
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
}

