// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! WebAssembly Interpreter Subsystem (kernel-internal)
//!
//! Exposes a pure `no_std` stack machine interpreter, sandboxed linear memory,
//! and host call bridges for local capability automations.
//!
//! PLATFORM TRUTH: this is a Ring-0 kernel-internal interpreter used by boot
//! self-tests only; it is not app-reachable. There is NO enforced Ring-0/Ring-3
//! execution boundary, no registered `wasmd` execution service, and no WASM
//! binary-container decoder (all NOT IMPLEMENTED). The execution-domain helpers
//! re-exported below are designation-only and have no production caller.
//! See `vm.rs` for the full what-exists / what-does-not statement.

pub mod memory;
pub mod types;
pub mod vm;

pub use memory::{WasmMemory, WASM_MAX_MEMORY_BYTES, WASM_MAX_PAGES_LIMIT, WASM_PAGE_SIZE};
pub use types::{opcodes, Value, WasmError};
pub use vm::{
    designate_execution_domain, is_ring0_execution_permitted, HostFn, HostFunction,
    WasmExecutionDomain, WasmFunction, WasmModule, WasmVm, WASMD_SERVICE_ID,
    WASM_MAX_CALL_DEPTH, WASM_MAX_LOCALS, WASM_MAX_STACK_VALUES,
};

