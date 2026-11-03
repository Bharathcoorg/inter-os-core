// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! WebAssembly Isolation & Execution Subsystem
//!
//! Exposes a pure `no_std` stack machine interpreter, sandboxed linear memory,
//! and host call bridges for local capability automations.

pub mod memory;
pub mod types;
pub mod vm;

pub use memory::{WasmMemory, WASM_MAX_MEMORY_BYTES, WASM_MAX_PAGES_LIMIT, WASM_PAGE_SIZE};
pub use types::{opcodes, Value, WasmError};
pub use vm::{
    designate_execution_domain, is_ring0_execution_permitted, HostFn, HostFunction,
    WasmExecutionDomain, WasmFunction, WasmModule, WasmVm, WASMD_SERVICE_ID,
};

