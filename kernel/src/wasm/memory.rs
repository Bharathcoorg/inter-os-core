// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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

/// Maximum linear memory pages permitted by InterOS WASM sandbox (64 pages = 4 MiB).
pub const WASM_MAX_PAGES_LIMIT: usize = 64;

/// Maximum linear memory bytes permitted by InterOS WASM sandbox (4 MiB).
pub const WASM_MAX_MEMORY_BYTES: usize = WASM_MAX_PAGES_LIMIT * WASM_PAGE_SIZE;

/// Sandboxed linear memory buffer.
#[derive(Debug, Clone)]
pub struct WasmMemory {
    data: Vec<u8>,
    max_pages: usize,
}

impl WasmMemory {
    /// Creates a new linear memory space with the specified initial pages and maximum limit,
    /// enforcing the strict platform ceiling of 64 pages / 4 MiB.
    pub fn new(initial_pages: usize, max_pages: usize) -> Self {
        let max_pages = max_pages.min(WASM_MAX_PAGES_LIMIT);
        let initial_pages = initial_pages.min(max_pages);
        let size = initial_pages.saturating_mul(WASM_PAGE_SIZE);
        Self {
            data: vec![0u8; size],
            max_pages,
        }
    }

    /// Creates linear memory bounded to an explicit maximum byte capacity,
    /// enforcing the strict platform ceiling of 64 pages / 4 MiB.
    pub fn with_byte_limit(initial_bytes: usize, max_bytes: usize) -> Self {
        let max_pages = max_bytes
            .saturating_add(WASM_PAGE_SIZE - 1)
            .checked_div(WASM_PAGE_SIZE)
            .unwrap_or(0);
        let max_pages = core::cmp::max(1, max_pages).min(WASM_MAX_PAGES_LIMIT);
        let initial_bytes = initial_bytes.min(max_pages.saturating_mul(WASM_PAGE_SIZE));
        Self {
            data: vec![0u8; initial_bytes],
            max_pages,
        }
    }

    /// Returns the active size in bytes.
    pub fn size(&self) -> usize {
        self.data.len()
    }

    /// Returns the maximum allowed page count for this linear memory space.
    pub fn max_pages(&self) -> usize {
        self.max_pages
    }

    /// Returns the maximum allowable capacity in bytes.
    pub fn max_capacity_bytes(&self) -> usize {
        self.max_pages.saturating_mul(WASM_PAGE_SIZE)
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

    /// True when `addr .. addr + 4` lies wholly inside the buffer.
    ///
    /// `addr + 4` must be computed with `checked_add`: a sandboxed address that
    /// reached here as `0xFFFF_FFFF_FFFF_FFFF` (a negative `i32` reinterpreted as
    /// a `usize`, or a pointer one below null) wraps to `3`, so a plain add
    /// both overflows and defeats the bounds check that was supposed to reject
    /// it. Wrapping would let the read land on the raw address and fault with
    /// CR2 = 0xFFFF_FFFF_FFFF_FFFF instead of trapping in the sandbox.
    #[inline]
    fn i32_slot_in_bounds(&self, addr: usize) -> bool {
        match addr.checked_add(4) {
            Some(end) => end <= self.data.len(),
            None => false,
        }
    }

    /// Reads a 32-bit little-endian integer.
    pub fn read_i32(&self, addr: usize) -> Result<i32, WasmError> {
        if !self.i32_slot_in_bounds(addr) {
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
        if !self.i32_slot_in_bounds(addr) {
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

    /// Grows memory by `delta_pages`, strictly enforcing the configured quota and the 64-page / 4 MiB limit.
    pub fn grow(&mut self, delta_pages: usize) -> Result<usize, WasmError> {
        let current_pages = self.data.len() / WASM_PAGE_SIZE;
        let new_pages = current_pages.checked_add(delta_pages).ok_or(WasmError::OutOfBoundsMemory)?;
        if new_pages > self.max_pages || new_pages > WASM_MAX_PAGES_LIMIT {
            return Err(WasmError::OutOfBoundsMemory);
        }
        let new_bytes = new_pages.checked_mul(WASM_PAGE_SIZE).ok_or(WasmError::OutOfBoundsMemory)?;
        if new_bytes > WASM_MAX_MEMORY_BYTES {
            return Err(WasmError::OutOfBoundsMemory);
        }
        self.data.resize(new_bytes, 0);
        Ok(current_pages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `usize::MAX` base address plus four wraps to three, so an unchecked
    /// `addr + 4 > len` guard accepts it and the read lands on the raw address.
    /// On bare metal that is a page fault at CR2 = 0xFFFF_FFFF_FFFF_FFFF; on the
    /// host it is an index panic. Either way it must not escape as a fault.
    #[test]
    fn a_wrapped_end_address_cannot_index_past_the_buffer() {
        let memory = WasmMemory::new(1, 1);

        assert_eq!(
            memory.read_i32(usize::MAX),
            Err(WasmError::OutOfBoundsMemory)
        );
        assert_eq!(
            memory.read_i32(usize::MAX - 3),
            Err(WasmError::OutOfBoundsMemory)
        );
        assert_eq!(
            memory.read_i32(usize::MAX - 2),
            Err(WasmError::OutOfBoundsMemory)
        );
        assert_eq!(
            memory.read_i32(usize::MAX - 1),
            Err(WasmError::OutOfBoundsMemory)
        );
    }

    #[test]
    fn a_wrapped_end_address_cannot_be_written_past_the_buffer() {
        let mut memory = WasmMemory::new(1, 1);

        assert_eq!(
            memory.write_i32(usize::MAX, 1),
            Err(WasmError::OutOfBoundsMemory)
        );
        assert_eq!(
            memory.write_i32(usize::MAX - 3, 1),
            Err(WasmError::OutOfBoundsMemory)
        );
        // Nothing may have leaked into the low addresses the wrap lands on.
        assert_eq!(memory.size(), WASM_PAGE_SIZE);
        assert!(memory.read_slice(0, WASM_PAGE_SIZE).is_ok());
    }

    #[test]
    fn the_last_in_bounds_word_is_still_reachable() {
        let mut memory = WasmMemory::new(1, 1);
        let last = WASM_PAGE_SIZE - 4;

        assert_eq!(memory.write_i32(last, 0x1234_5678), Ok(()));
        assert_eq!(memory.read_i32(last), Ok(0x1234_5678));
        assert_eq!(
            memory.read_i32(last + 4),
            Err(WasmError::OutOfBoundsMemory)
        );
        assert_eq!(
            memory.write_i32(last + 4, 0),
            Err(WasmError::OutOfBoundsMemory)
        );
    }

    #[test]
    fn a_negative_i32_used_as_an_address_is_out_of_bounds() {
        let memory = WasmMemory::new(1, 1);

        // `(-1i32) as usize` is 0xFFFF_FFFF_FFFF_FFFF on a 64-bit host. The
        // sandbox must reject it rather than dereference it.
        let signed = -1i32 as usize;
        assert_eq!(signed, usize::MAX);
        assert_eq!(
            memory.read_i32(signed),
            Err(WasmError::OutOfBoundsMemory)
        );
        assert_eq!(memory.read_u8(signed), Err(WasmError::OutOfBoundsMemory));
        assert_eq!(
            memory.read_slice(signed, 1),
            Err(WasmError::OutOfBoundsMemory)
        );
    }

    #[test]
    fn page_arithmetic_cannot_wrap() {
        let mut memory = WasmMemory::new(1, 2);
        assert_eq!(memory.grow(usize::MAX), Err(WasmError::OutOfBoundsMemory));
        assert_eq!(memory.size(), WASM_PAGE_SIZE);

        let capped = WasmMemory::with_byte_limit(16, usize::MAX);
        assert!(capped.max_pages >= 1);
        assert!(capped.max_pages <= WASM_MAX_PAGES_LIMIT);
    }

    #[test]
    fn memory_quota_strictly_enforces_64_pages_limit() {
        let mut memory = WasmMemory::new(1, 100);
        assert_eq!(memory.max_pages(), WASM_MAX_PAGES_LIMIT);
        assert_eq!(memory.max_capacity_bytes(), 64 * WASM_PAGE_SIZE);

        // Can grow up to 64 pages (63 additional pages)
        assert_eq!(memory.grow(63), Ok(1));
        assert_eq!(memory.size(), 64 * WASM_PAGE_SIZE);

        // Cannot grow beyond 64 pages
        assert_eq!(memory.grow(1), Err(WasmError::OutOfBoundsMemory));
    }
}

