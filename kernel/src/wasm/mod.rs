//! WebAssembly Isolation & Execution Subsystem
//!
//! Exposes a pure `no_std` stack machine interpreter, sandboxed linear memory,
//! and host call bridges for local capability automations.

pub mod memory;
pub mod types;
pub mod vm;

pub use memory::{WasmMemory, WASM_PAGE_SIZE};
pub use types::{opcodes, Value, WasmError};
pub use vm::{HostFn, HostFunction, WasmFunction, WasmModule, WasmVm};
