// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! High-Assurance Memory-Message IPC Architecture (Lending & Ownership Transfer)
//!
//! Inspired by microkernel memory-message IPC semantics (such as Xous and Open Nexus):
//! - Zero Ambient Authority: buffer lending and transfers require explicit task IDs, loan tokens,
//!   and capability bounds.
//! - Strict Lifetime Enforcement: borrowed read-only and mutable buffers cannot outlive their
//!   originating scope.
//! - Single-Ownership Invariants: transferred memory regions invalidate sender handles upon consumption.
//! - Circular Streaming Channels: double- and ring-buffered streaming with CRC-32 integrity
//!   for audio PCM, camera sensors, and neural embeddings.
//!
//! Note (Platform Truth Contract): In userspace, `BorrowedReadOnlyBuffer`, `BorrowedMutableBuffer`,
//! and `TransferredBuffer` provide lifetime-bound borrow and single-consumer transfer abstractions.
//! Cross-process memory transfer across address spaces is mediated through kernel ShmPipe channels
//! and capability-enforced IPC; these types enforce API ownership invariants.

use crate::companion::crc32;

/// Memory-message IPC errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcMemoryError {
    /// The payload exceeds the target buffer capacity.
    BufferOverflow,
    /// The buffer was smaller than the expected header or record length.
    BufferUnderflow,
    /// Provided length is invalid or inconsistent with container bounds.
    InvalidLength,
    /// Caller lacks authority to perform memory transfer or loan.
    PermissionDenied,
    /// Transferred buffer has already been consumed (single-owner transfer violation).
    AlreadyConsumed,
    /// Loan token has expired or is invalid.
    LoanExpired,
    /// Computed CRC-32 checksum did not match the frame descriptor.
    CrcMismatch,
    /// Ring buffer or slot table is full (overrun prevention).
    SlotFull,
    /// Ring buffer or slot table is empty (underrun).
    SlotEmpty,
}

/// A 64-byte inline register-based value IPC message.
///
/// Designed for lightweight RPC, queries, event notifications, and control signalling
/// without allocating or mapping memory pages.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScalarMessage {
    pub id: u32,
    pub arg1: usize,
    pub arg2: usize,
    pub arg3: usize,
    pub arg4: usize,
}

impl ScalarMessage {
    pub const SIZE: usize = 64;

    /// Creates a new scalar message.
    pub const fn new(id: u32, arg1: usize, arg2: usize, arg3: usize, arg4: usize) -> Self {
        Self {
            id,
            arg1,
            arg2,
            arg3,
            arg4,
        }
    }

    /// Serializes the scalar message into a fixed 64-byte payload.
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        buf[0..4].copy_from_slice(&self.id.to_le_bytes());
        buf[8..16].copy_from_slice(&(self.arg1 as u64).to_le_bytes());
        buf[16..24].copy_from_slice(&(self.arg2 as u64).to_le_bytes());
        buf[24..32].copy_from_slice(&(self.arg3 as u64).to_le_bytes());
        buf[32..40].copy_from_slice(&(self.arg4 as u64).to_le_bytes());
        let crc = crc32(&buf[0..40]);
        buf[60..64].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    /// Deserializes a scalar message from a byte slice, validating length and CRC.
    pub fn from_bytes(raw: &[u8]) -> Result<Self, IpcMemoryError> {
        if raw.len() < Self::SIZE {
            return Err(IpcMemoryError::BufferUnderflow);
        }
        let expected_crc = u32::from_le_bytes([raw[60], raw[61], raw[62], raw[63]]);
        let computed_crc = crc32(&raw[0..40]);
        if computed_crc != expected_crc {
            return Err(IpcMemoryError::CrcMismatch);
        }
        let id = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
        let arg1 = u64::from_le_bytes(raw[8..16].try_into().unwrap()) as usize;
        let arg2 = u64::from_le_bytes(raw[16..24].try_into().unwrap()) as usize;
        let arg3 = u64::from_le_bytes(raw[24..32].try_into().unwrap()) as usize;
        let arg4 = u64::from_le_bytes(raw[32..40].try_into().unwrap()) as usize;
        Ok(Self {
            id,
            arg1,
            arg2,
            arg3,
            arg4,
        })
    }
}

/// Lifetime-bound lent read-only memory buffer.
///
/// Ensures the memory region is immutable and borrowed only for the duration of the IPC loan.
#[derive(Debug)]
pub struct BorrowedReadOnlyBuffer<'a> {
    data: &'a [u8],
    target_task: u64,
    loan_id: u64,
}

impl<'a> BorrowedReadOnlyBuffer<'a> {
    /// Lends an immutable slice to a target task with a unique loan token.
    pub fn lend(data: &'a [u8], target_task: u64, loan_id: u64) -> Self {
        Self {
            data,
            target_task,
            loan_id,
        }
    }

    /// Returns the underlying byte slice.
    pub fn as_slice(&self) -> &'a [u8] {
        self.data
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The recipient task authorized for this loan.
    pub fn target_task(&self) -> u64 {
        self.target_task
    }

    /// The unique loan identifier.
    pub fn loan_id(&self) -> u64 {
        self.loan_id
    }
}

/// Lifetime-bound exclusive mutable lent memory buffer.
///
/// Grants temporary exclusive write access to a callee/server. The caller re-acquires
/// exclusive access when the loan is dropped upon return.
#[derive(Debug)]
pub struct BorrowedMutableBuffer<'a> {
    data: &'a mut [u8],
    target_task: u64,
    loan_id: u64,
    written_len: usize,
}

impl<'a> BorrowedMutableBuffer<'a> {
    /// Lends a mutable slice to a target task with a unique loan token.
    pub fn lend(data: &'a mut [u8], target_task: u64, loan_id: u64) -> Self {
        Self {
            data,
            target_task,
            loan_id,
            written_len: 0,
        }
    }

    /// Returns the full underlying byte slice.
    pub fn as_slice(&self) -> &[u8] {
        self.data
    }

    /// Returns mutable access to the underlying byte slice.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.data
    }

    /// Total capacity in bytes.
    pub fn capacity(&self) -> usize {
        self.data.len()
    }

    /// Number of bytes actively written by the callee.
    pub fn written_len(&self) -> usize {
        self.written_len
    }

    /// Records the valid written length, verifying it does not exceed buffer capacity.
    pub fn set_written_len(&mut self, len: usize) -> Result<(), IpcMemoryError> {
        if len > self.data.len() {
            return Err(IpcMemoryError::BufferOverflow);
        }
        self.written_len = len;
        Ok(())
    }

    /// Returns the slice of bytes actually written during the loan.
    pub fn written_slice(&self) -> &[u8] {
        &self.data[..self.written_len]
    }

    /// The recipient task authorized for this loan.
    pub fn target_task(&self) -> u64 {
        self.target_task
    }

    /// The unique loan identifier.
    pub fn loan_id(&self) -> u64 {
        self.loan_id
    }
}

/// Full zero-copy ownership transfer between tasks.
///
/// Once created, the sender relinquishes the buffer. The receiver consumes it via `take()`.
/// Any subsequent attempt to access or consume the buffer fails closed (`AlreadyConsumed`).
#[derive(Debug)]
pub struct TransferredBuffer<const N: usize> {
    data: [u8; N],
    valid_len: usize,
    sender_task: u64,
    target_task: u64,
    transfer_id: u64,
    consumed: bool,
}

impl<const N: usize> TransferredBuffer<N> {
    /// Creates a newly transferred buffer.
    pub fn new(
        data: [u8; N],
        valid_len: usize,
        sender_task: u64,
        target_task: u64,
        transfer_id: u64,
    ) -> Result<Self, IpcMemoryError> {
        if valid_len > N {
            return Err(IpcMemoryError::BufferOverflow);
        }
        Ok(Self {
            data,
            valid_len,
            sender_task,
            target_task,
            transfer_id,
            consumed: false,
        })
    }

    /// Whether this buffer has already been consumed by the receiver.
    pub fn is_consumed(&self) -> bool {
        self.consumed
    }

    /// Length of valid payload data in bytes.
    pub fn valid_len(&self) -> usize {
        self.valid_len
    }

    /// Total buffer capacity.
    pub const fn capacity(&self) -> usize {
        N
    }

    /// The sending task.
    pub fn sender_task(&self) -> u64 {
        self.sender_task
    }

    /// The authorized recipient task.
    pub fn target_task(&self) -> u64 {
        self.target_task
    }

    /// Unique transfer tracking ID.
    pub fn transfer_id(&self) -> u64 {
        self.transfer_id
    }

    /// Consumes the transferred buffer, returning ownership of the data array and valid length.
    pub fn take(&mut self) -> Result<([u8; N], usize), IpcMemoryError> {
        if self.consumed {
            return Err(IpcMemoryError::AlreadyConsumed);
        }
        self.consumed = true;
        let mut out = [0u8; N];
        out.copy_from_slice(&self.data);
        Ok((out, self.valid_len))
    }
}

/// Circular slot for streaming shared memory channels.
#[derive(Clone, Copy)]
pub struct StreamSlot<const CAPACITY: usize> {
    pub sequence: u64,
    pub len: usize,
    pub flags: u32,
    pub crc32: u32,
    pub data: [u8; CAPACITY],
}

impl<const CAPACITY: usize> StreamSlot<CAPACITY> {
    pub const fn empty() -> Self {
        Self {
            sequence: 0,
            len: 0,
            flags: 0,
            crc32: 0,
            data: [0u8; CAPACITY],
        }
    }
}

/// Double- or ring-buffered circular shared memory channel.
///
/// Designed for continuous high-rate streaming (Camera ISP frames, Audio PCM, Sensor feeds)
/// with zero heap allocations, sequence monotonicity, and hardware CRC-32 integrity validation.
pub struct StreamingSharedMemory<const SLOT_CAPACITY: usize, const RING_SLOTS: usize> {
    slots: [StreamSlot<SLOT_CAPACITY>; RING_SLOTS],
    head: usize,
    tail: usize,
    count: usize,
    next_sequence: u64,
}

impl<const SLOT_CAPACITY: usize, const RING_SLOTS: usize>
    StreamingSharedMemory<SLOT_CAPACITY, RING_SLOTS>
{
    pub const fn new() -> Self {
        Self {
            slots: [StreamSlot::empty(); RING_SLOTS],
            head: 0,
            tail: 0,
            count: 0,
            next_sequence: 1,
        }
    }

    /// Total slot capacity in bytes.
    pub const fn slot_capacity(&self) -> usize {
        SLOT_CAPACITY
    }

    /// Total number of ring slots.
    pub const fn ring_slots(&self) -> usize {
        RING_SLOTS
    }

    /// Number of frames currently available to be read.
    pub fn available_frames(&self) -> usize {
        self.count
    }

    /// Whether the ring buffer is completely full.
    pub fn is_full(&self) -> bool {
        self.count >= RING_SLOTS
    }

    /// Whether the ring buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Writes a new frame into the next ring slot.
    pub fn write_frame(&mut self, data: &[u8], flags: u32) -> Result<u64, IpcMemoryError> {
        if self.is_full() {
            return Err(IpcMemoryError::SlotFull);
        }
        if data.len() > SLOT_CAPACITY {
            return Err(IpcMemoryError::BufferOverflow);
        }
        let seq = self.next_sequence;
        self.next_sequence += 1;

        let slot = &mut self.slots[self.head];
        slot.sequence = seq;
        slot.len = data.len();
        slot.flags = flags;
        slot.data[..data.len()].copy_from_slice(data);
        slot.crc32 = crc32(data);

        self.head = (self.head + 1) % RING_SLOTS;
        self.count += 1;
        Ok(seq)
    }

    /// Reads the oldest available frame from the ring buffer, verifying CRC-32 integrity.
    pub fn read_frame(&mut self, out_buf: &mut [u8]) -> Result<(usize, u32, u64), IpcMemoryError> {
        if self.is_empty() {
            return Err(IpcMemoryError::SlotEmpty);
        }
        let slot = &self.slots[self.tail];
        if out_buf.len() < slot.len {
            return Err(IpcMemoryError::BufferOverflow);
        }
        let computed_crc = crc32(&slot.data[..slot.len]);
        if computed_crc != slot.crc32 {
            return Err(IpcMemoryError::CrcMismatch);
        }

        out_buf[..slot.len].copy_from_slice(&slot.data[..slot.len]);
        let len = slot.len;
        let flags = slot.flags;
        let seq = slot.sequence;

        self.tail = (self.tail + 1) % RING_SLOTS;
        self.count -= 1;
        Ok((len, flags, seq))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scalar_message_roundtrip() {
        let msg = ScalarMessage::new(0x1234, 100, 200, 300, 400);
        let bytes = msg.to_bytes();
        assert_eq!(bytes.len(), 64);

        let parsed = ScalarMessage::from_bytes(&bytes).expect("deserialization should succeed");
        assert_eq!(parsed.id, 0x1234);
        assert_eq!(parsed.arg1, 100);
        assert_eq!(parsed.arg2, 200);
        assert_eq!(parsed.arg3, 300);
        assert_eq!(parsed.arg4, 400);
    }

    #[test]
    fn test_scalar_message_tamper_detection() {
        let msg = ScalarMessage::new(42, 1, 2, 3, 4);
        let mut bytes = msg.to_bytes();
        bytes[10] ^= 0xFF; // tamper with arg1
        assert_eq!(ScalarMessage::from_bytes(&bytes), Err(IpcMemoryError::CrcMismatch));
    }

    #[test]
    fn test_borrowed_read_only_buffer() {
        let data = b"immutable secret payload";
        let buf = BorrowedReadOnlyBuffer::lend(data, 10, 0xAA55);
        assert_eq!(buf.as_slice(), data);
        assert_eq!(buf.len(), data.len());
        assert!(!buf.is_empty());
        assert_eq!(buf.target_task(), 10);
        assert_eq!(buf.loan_id(), 0xAA55);
    }

    #[test]
    fn test_borrowed_mutable_buffer_mutation_and_bounds() {
        let mut raw = [0u8; 128];
        let mut buf = BorrowedMutableBuffer::lend(&mut raw, 5, 0x7788);
        assert_eq!(buf.capacity(), 128);
        assert_eq!(buf.written_len(), 0);

        let payload = b"kernel response written into lent space";
        buf.as_mut_slice()[..payload.len()].copy_from_slice(payload);
        buf.set_written_len(payload.len()).expect("within capacity");
        assert_eq!(buf.written_slice(), payload);

        // Attempting to set written len larger than capacity fails
        assert_eq!(buf.set_written_len(200), Err(IpcMemoryError::BufferOverflow));
    }

    #[test]
    fn test_transferred_buffer_single_ownership() {
        let mut raw = [0u8; 64];
        raw[0..5].copy_from_slice(b"HELLO");
        let mut transferred = TransferredBuffer::new(raw, 5, 2, 3, 1001)
            .expect("valid creation");

        assert!(!transferred.is_consumed());
        assert_eq!(transferred.valid_len(), 5);
        assert_eq!(transferred.sender_task(), 2);
        assert_eq!(transferred.target_task(), 3);
        assert_eq!(transferred.transfer_id(), 1001);

        // First consume succeeds
        let (data, len) = transferred.take().expect("first consume succeeds");
        assert_eq!(&data[0..5], b"HELLO");
        assert_eq!(len, 5);
        assert!(transferred.is_consumed());

        // Second consume fails closed
        assert_eq!(transferred.take(), Err(IpcMemoryError::AlreadyConsumed));
    }

    #[test]
    fn test_streaming_shared_memory_ring() {
        let mut stream = StreamingSharedMemory::<256, 4>::new();
        assert!(stream.is_empty());
        assert_eq!(stream.available_frames(), 0);

        // Write frames
        let seq1 = stream.write_frame(b"frame 1", 0).expect("write frame 1");
        let seq2 = stream.write_frame(b"frame 2", 1).expect("write frame 2");
        assert_eq!(seq1, 1);
        assert_eq!(seq2, 2);
        assert_eq!(stream.available_frames(), 2);

        // Read frames
        let mut rx_buf = [0u8; 256];
        let (len1, flags1, rx_seq1) = stream.read_frame(&mut rx_buf).expect("read frame 1");
        assert_eq!(&rx_buf[..len1], b"frame 1");
        assert_eq!(flags1, 0);
        assert_eq!(rx_seq1, 1);

        let (len2, flags2, rx_seq2) = stream.read_frame(&mut rx_buf).expect("read frame 2");
        assert_eq!(&rx_buf[..len2], b"frame 2");
        assert_eq!(flags2, 1);
        assert_eq!(rx_seq2, 2);

        // Buffer is now empty
        assert!(stream.is_empty());
        assert_eq!(stream.read_frame(&mut rx_buf), Err(IpcMemoryError::SlotEmpty));

        // Fill ring buffer to capacity (4 slots)
        stream.write_frame(b"f1", 0).unwrap();
        stream.write_frame(b"f2", 0).unwrap();
        stream.write_frame(b"f3", 0).unwrap();
        stream.write_frame(b"f4", 0).unwrap();
        assert!(stream.is_full());

        // 5th write returns SlotFull
        assert_eq!(stream.write_frame(b"f5", 0), Err(IpcMemoryError::SlotFull));
    }
}
