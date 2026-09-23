//! WebAssembly Linear Memory Sandbox
//!
//! Provides isolated linear memory execution space with strict bounds protection.
//! Any attempt by WASM bytecode to read or write beyond allocated bounds
//! is trapped and rejected with `WasmError::OutOfBoundsMemory`.

use alloc::vec;
use alloc::vec::Vec;
use crate::wasm::types::WasmError;

/// WASM standard page size: 64 KiB.
pub const WASM_PAGE_SIZE: usize = 65536;

/// Sandboxed linear memory buffer.
#[derive(Debug, Clone)]
pub struct WasmMemory {
    data: Vec<u8>,
    max_pages: usize,
}

impl WasmMemory {
    /// Creates a new linear memory space with the specified initial pages and maximum limit.
    pub fn new(initial_pages: usize, max_pages: usize) -> Self {
        let size = initial_pages * WASM_PAGE_SIZE;
        Self {
            data: vec![0u8; size],
            max_pages,
        }
    }

    /// Creates linear memory bounded to an explicit maximum byte capacity.
    pub fn with_byte_limit(initial_bytes: usize, max_bytes: usize) -> Self {
        let max_pages = (max_bytes + WASM_PAGE_SIZE - 1) / WASM_PAGE_SIZE;
        Self {
            data: vec![0u8; initial_bytes],
            max_pages: core::cmp::max(1, max_pages),
        }
    }

    /// Returns the active size in bytes.
    pub fn size(&self) -> usize {
        self.data.len()
    }

    /// Reads a single byte at the specified linear address.
    pub fn read_u8(&self, addr: usize) -> Result<u8, WasmError> {
        self.data.get(addr).copied().ok_or(WasmError::OutOfBoundsMemory)
    }

    /// Writes a single byte at the specified linear address.
    pub fn write_u8(&mut self, addr: usize, val: u8) -> Result<(), WasmError> {
        if addr >= self.data.len() {
            return Err(WasmError::OutOfBoundsMemory);
        }
        self.data[addr] = val;
        Ok(())
    }

    /// Reads a 32-bit little-endian integer.
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

    /// Writes a 32-bit little-endian integer.
    pub fn write_i32(&mut self, addr: usize, val: i32) -> Result<(), WasmError> {
        if addr + 4 > self.data.len() {
            return Err(WasmError::OutOfBoundsMemory);
        }
        let bytes = val.to_le_bytes();
        self.data[addr..addr + 4].copy_from_slice(&bytes);
        Ok(())
    }

    /// Reads an arbitrary byte slice.
    pub fn read_slice(&self, addr: usize, len: usize) -> Result<&[u8], WasmError> {
        if addr.checked_add(len).map_or(true, |end| end > self.data.len()) {
            return Err(WasmError::OutOfBoundsMemory);
        }
        Ok(&self.data[addr..addr + len])
    }

    /// Writes an arbitrary byte slice into linear memory.
    pub fn write_slice(&mut self, addr: usize, slice: &[u8]) -> Result<(), WasmError> {
        let len = slice.len();
        if addr.checked_add(len).map_or(true, |end| end > self.data.len()) {
            return Err(WasmError::OutOfBoundsMemory);
        }
        self.data[addr..addr + len].copy_from_slice(slice);
        Ok(())
    }

    /// Grows memory by `delta_pages`.
    pub fn grow(&mut self, delta_pages: usize) -> Result<usize, WasmError> {
        let current_pages = self.data.len() / WASM_PAGE_SIZE;
        let new_pages = current_pages + delta_pages;
        if new_pages > self.max_pages {
            return Err(WasmError::OutOfBoundsMemory);
        }
        self.data.resize(new_pages * WASM_PAGE_SIZE, 0);
        Ok(current_pages)
    }
}
