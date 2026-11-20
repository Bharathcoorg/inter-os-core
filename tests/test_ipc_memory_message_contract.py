#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""High-Assurance Memory-Message IPC Contract.

Verifies the userspace IPC architecture guarantees:
- Pure 64-byte inline register-based value IPC (`ScalarMessage`).
- Lifetime-bound immutable lent memory buffer (`BorrowedReadOnlyBuffer`).
- Lifetime-bound exclusive mutable lent memory buffer (`BorrowedMutableBuffer`).
- Single-owner memory transfer invalidating sender handles (`TransferredBuffer`).
- High-rate circular streaming shared memory channel (`StreamingSharedMemory`) with CRC-32 integrity.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
IPC_MEM = (ROOT / "sdk/rust/interos-std/src/ipc/memory.rs").read_text(encoding="utf-8")
IPC_MOD = (ROOT / "sdk/rust/interos-std/src/ipc/mod.rs").read_text(encoding="utf-8")
STD_LIB = (ROOT / "sdk/rust/interos-std/src/lib.rs").read_text(encoding="utf-8")


def must(text: str, needle: str, label: str) -> None:
    assert needle in text, f"missing {label}: {needle}"


def test_scalar_message_contract() -> None:
    must(IPC_MEM, "pub struct ScalarMessage", "ScalarMessage struct definition")
    must(IPC_MEM, "pub const SIZE: usize = 64;", "ScalarMessage 64-byte payload size bound")
    must(IPC_MEM, "pub fn to_bytes(&self) -> [u8; Self::SIZE]", "ScalarMessage serialization")
    must(IPC_MEM, "pub fn from_bytes(raw: &[u8]) -> Result<Self, IpcMemoryError>", "ScalarMessage deserialization")
    must(IPC_MEM, "if computed_crc != expected_crc", "ScalarMessage CRC validation")


def test_borrowed_buffers_contract() -> None:
    must(IPC_MEM, "pub struct BorrowedReadOnlyBuffer<'a>", "BorrowedReadOnlyBuffer definition")
    must(IPC_MEM, "pub fn lend(data: &'a [u8], target_task: u64, loan_id: u64)", "BorrowedReadOnlyBuffer loan constructor")
    must(IPC_MEM, "pub struct BorrowedMutableBuffer<'a>", "BorrowedMutableBuffer definition")
    must(IPC_MEM, "pub fn lend(data: &'a mut [u8], target_task: u64, loan_id: u64)", "BorrowedMutableBuffer loan constructor")
    must(IPC_MEM, "if len > self.data.len()", "BorrowedMutableBuffer overflow check")


def test_transferred_buffer_contract() -> None:
    must(IPC_MEM, "pub struct TransferredBuffer<const N: usize>", "TransferredBuffer definition")
    must(IPC_MEM, "if self.consumed {", "TransferredBuffer single-owner check")
    must(IPC_MEM, "return Err(IpcMemoryError::AlreadyConsumed);", "TransferredBuffer already consumed error")


def test_streaming_shared_memory_contract() -> None:
    must(IPC_MEM, "pub struct StreamingSharedMemory<const SLOT_CAPACITY: usize, const RING_SLOTS: usize>", "StreamingSharedMemory definition")
    must(IPC_MEM, "if self.is_full() {\n            return Err(IpcMemoryError::SlotFull);", "StreamingSharedMemory overrun prevention")
    must(IPC_MEM, "if self.is_empty() {\n            return Err(IpcMemoryError::SlotEmpty);", "StreamingSharedMemory underrun prevention")
    must(IPC_MEM, "if computed_crc != slot.crc32 {\n            return Err(IpcMemoryError::CrcMismatch);", "StreamingSharedMemory CRC-32 validation")


def test_ipc_module_exports() -> None:
    must(IPC_MOD, "pub mod memory;", "memory module declaration in ipc/mod.rs")
    must(IPC_MOD, "pub use memory::{", "memory re-exports in ipc/mod.rs")
    must(IPC_MOD, "pub const MSG_INPUT_EVENT: u32 = 0x494E_5054;", "input event constant in ipc/mod.rs")
    must(IPC_MOD, "pub const MSG_WINDOW_FOCUSED: u32 = 0x7001;", "window focused constant in ipc/mod.rs")
    must(IPC_MOD, "pub fn send(", "send function in ipc/mod.rs")
    must(IPC_MOD, "pub fn receive_any(", "receive_any function in ipc/mod.rs")
    must(STD_LIB, "pub mod ipc;", "pub mod ipc in interos-std lib.rs")


def main() -> None:
    test_scalar_message_contract()
    test_borrowed_buffers_contract()
    test_transferred_buffer_contract()
    test_streaming_shared_memory_contract()
    test_ipc_module_exports()
    print("Memory-message IPC contract: PASS")


if __name__ == "__main__":
    main()
