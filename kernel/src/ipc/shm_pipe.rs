// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Zero-Copy DMA Shared Memory Ring Buffers & High-Throughput IPC Message Pipes
//!
//! Provides ultra-low-latency, zero-copy communication channels between tasks
//! (such as Camera sensor frames, Audio PCM stream buffers, AI embeddings/tensors,
//! and Sovereign Encrypted Vault bulk transfers).
//!
//! Enforces capability protection: tasks writing to or reading from SHM channels
//! must possess matching `ShmProducer` or `ShmConsumer` capabilities.

use crate::task::TaskId;
use spin::Mutex;

/// Standard IEEE 802.3 CRC-32 polynomial (Ethernet / ZIP / PNG).
const CRC32_POLYNOMIAL: u32 = 0xEDB8_8320;

/// Compile-time precomputed Slicing-by-4 IEEE 802.3 CRC-32 lookup tables.
/// Table 0 is the classical 256-entry table; tables 1..3 enable 4-byte chunk processing.
const CRC32_TABLES: [[u32; 256]; 4] = {
    let mut tables = [[0u32; 256]; 4];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ CRC32_POLYNOMIAL;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        tables[0][i] = crc;
        i += 1;
    }
    let mut slice = 1;
    while slice < 4 {
        let mut i = 0;
        while i < 256 {
            let prev = tables[slice - 1][i];
            tables[slice][i] = (prev >> 8) ^ tables[0][(prev & 0xFF) as usize];
            i += 1;
        }
        slice += 1;
    }
    tables
};

/// 256-entry IEEE 802.3 CRC-32 table (alias for Table 0).
pub const CRC32_TABLE: [u32; 256] = CRC32_TABLES[0];

/// Computes an authentic IEEE 802.3 32-bit Cyclic Redundancy Check using Slicing-by-4.
pub fn compute_crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    let mut chunks = data.chunks_exact(4);
    for chunk in chunks.by_ref() {
        let word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let term = crc ^ word;
        crc = CRC32_TABLES[3][(term & 0xFF) as usize]
            ^ CRC32_TABLES[2][((term >> 8) & 0xFF) as usize]
            ^ CRC32_TABLES[1][((term >> 16) & 0xFF) as usize]
            ^ CRC32_TABLES[0][(term >> 24) as usize];
    }
    for &b in chunks.remainder() {
        let idx = ((crc ^ (b as u32)) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32_TABLES[0][idx];
    }
    !crc
}

/// Errors originating from the Zero-Copy Shared Memory Subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShmError {
    PipeNotFound,
    RingFull,
    RingEmpty,
    PermissionDenied,
    InvalidArguments,
    CrcMismatch,
    ExceedsSlotCapacity,
}

/// DMA transfer flags.
pub const DMA_FLAG_ACCEL: u32 = 1 << 0;
pub const DMA_FLAG_VERIFY_CRC: u32 = 1 << 1;
pub const DMA_FLAG_INTR_COMPLETE: u32 = 1 << 2;

/// Hardware-compatible Direct Memory Access (DMA) Scatter-Gather Descriptor.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy)]
pub struct DmaDescriptor {
    pub src_addr: u64,
    pub dest_addr: u64,
    pub byte_len: u32,
    pub flags: u32,
    pub crc32: u32,
    pub status: u32,
}

impl DmaDescriptor {
    pub const fn new(src_addr: u64, dest_addr: u64, byte_len: u32, flags: u32) -> Self {
        Self {
            src_addr,
            dest_addr,
            byte_len,
            flags,
            crc32: 0,
            status: 0,
        }
    }
}

/// Maximum payload capacity per individual ring buffer slot (1 KiB).
pub const SHM_SLOT_CAPACITY: usize = 1024;

/// Cache-line aligned circular slot within the shared memory ring buffer.
#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub struct ShmSlot {
    pub sequence: u64,
    pub len: usize,
    pub timestamp_ticks: u64,
    pub crc32: u32,
    pub flags: u32,
    pub data: [u8; SHM_SLOT_CAPACITY],
}

impl ShmSlot {
    pub const fn empty() -> Self {
        Self {
            sequence: 0,
            len: 0,
            timestamp_ticks: 0,
            crc32: 0,
            flags: 0,
            data: [0u8; SHM_SLOT_CAPACITY],
        }
    }
}

/// Number of discrete slots in each high-throughput ring buffer (8 slots = 8 KiB).
pub const SHM_RING_SLOTS: usize = 8;

/// Dedicated cache-line aligned zero-copy producer-consumer ring buffer.
pub struct ShmRingBuffer {
    pub pipe_id: u32,
    pub producer: TaskId,
    pub consumer: TaskId,
    pub name: &'static str,
    pub head: usize, // Consumer read index
    pub tail: usize, // Producer write index
    pub total_bytes: usize,
    pub total_packets: u64,
    pub slots: [ShmSlot; SHM_RING_SLOTS],
}

impl ShmRingBuffer {
    pub const fn new(pipe_id: u32, producer: TaskId, consumer: TaskId, name: &'static str) -> Self {
        Self {
            pipe_id,
            producer,
            consumer,
            name,
            head: 0,
            tail: 0,
            total_bytes: 0,
            total_packets: 0,
            slots: [ShmSlot::empty(); SHM_RING_SLOTS],
        }
    }

    /// Checks if the ring buffer is completely full.
    pub fn is_full(&self) -> bool {
        ((self.tail + 1) % SHM_RING_SLOTS) == self.head
    }

    /// Checks if the ring buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    /// Returns the number of slots available for writing.
    pub fn available_write(&self) -> usize {
        if self.tail >= self.head {
            SHM_RING_SLOTS - 1 - (self.tail - self.head)
        } else {
            self.head - self.tail - 1
        }
    }

    /// Returns the number of slots available for reading.
    pub fn available_read(&self) -> usize {
        if self.tail >= self.head {
            self.tail - self.head
        } else {
            SHM_RING_SLOTS - (self.head - self.tail)
        }
    }

    /// Empties the ring without changing its endpoints.
    ///
    /// The channel's byte and packet counters are deliberately left alone: they
    /// describe what the pipe has carried over its life, and a diagnostic that
    /// reads them after a reconfigure would otherwise be told the channel is
    /// brand new when it is not.
    pub fn reset(&mut self) {
        self.head = 0;
        self.tail = 0;
        for slot in self.slots.iter_mut() {
            *slot = ShmSlot::empty();
        }
    }

    /// Writes data directly into the next available ring buffer slot.
    pub fn write(
        &mut self,
        sender: TaskId,
        data: &[u8],
        flags: u32,
        ticks: u64,
    ) -> Result<u64, ShmError> {
        if sender != self.producer {
            return Err(ShmError::PermissionDenied);
        }
        if data.len() > SHM_SLOT_CAPACITY {
            return Err(ShmError::ExceedsSlotCapacity);
        }

        if self.is_full() {
            return Err(ShmError::RingFull);
        }

        let slot = &mut self.slots[self.tail];
        slot.len = data.len();
        slot.data[..data.len()].copy_from_slice(data);
        slot.timestamp_ticks = ticks;
        slot.flags = flags;
        slot.crc32 = compute_crc32(data);
        self.total_packets += 1;
        slot.sequence = self.total_packets;
        self.total_bytes += data.len();

        let seq = slot.sequence;
        self.tail = (self.tail + 1) % SHM_RING_SLOTS;
        Ok(seq)
    }

    /// Reads data directly from the current head slot of the ring buffer.
    pub fn read(
        &mut self,
        receiver: TaskId,
        out_buf: &mut [u8],
    ) -> Result<(usize, u64), ShmError> {
        if receiver != self.consumer {
            return Err(ShmError::PermissionDenied);
        }
        if self.is_empty() {
            return Err(ShmError::RingEmpty);
        }

        let slot = &self.slots[self.head];
        if out_buf.len() < slot.len {
            return Err(ShmError::InvalidArguments);
        }

        // Verify CRC-32 integrity
        let actual_crc = compute_crc32(&slot.data[..slot.len]);
        if actual_crc != slot.crc32 {
            return Err(ShmError::CrcMismatch);
        }

        out_buf[..slot.len].copy_from_slice(&slot.data[..slot.len]);
        let read_len = slot.len;
        let seq = slot.sequence;

        self.head = (self.head + 1) % SHM_RING_SLOTS;
        Ok((read_len, seq))
    }
}

/// System-wide Shared Memory Pipe Manager supporting up to 8 concurrent channels.
pub struct ShmPipeManager {
    pub pipes: [Option<ShmRingBuffer>; 8],
}

/// Standard system-wide SHM pipe IDs
pub const PIPE_CAMERA_RAW: u32 = 0;
pub const PIPE_AUDIO_PCM: u32 = 1;
pub const PIPE_VAULT_BULK: u32 = 2;
pub const PIPE_MESH_DMA: u32 = 3;
pub const PIPE_COMPOSITOR_DISPLAY_LIST: u32 = 4;

/// The first pipe id a task may claim for itself.
///
/// WHY the fixed channels are excluded: their endpoints are compiled in, so a
/// request naming one of them is an attempt to re-point a channel the platform
/// set up at boot, not a request for a new channel.
pub const PIPE_DYNAMIC_FIRST: u32 = 5;

/// How many pipes tasks may claim, which is how many bulk transfers the device
/// can carry at once. A claim is held by the pair it was made for and is freed
/// when either endpoint exits.
pub const PIPE_DYNAMIC_COUNT: u32 = 3;

/// Whether an id is one a task is allowed to claim for itself.
pub fn is_dynamic_pipe_id(pipe_id: u32) -> bool {
    pipe_id >= PIPE_DYNAMIC_FIRST && pipe_id < PIPE_DYNAMIC_FIRST + PIPE_DYNAMIC_COUNT
}

impl ShmPipeManager {
    pub const fn new() -> Self {
        Self {
            pipes: [
                Some(ShmRingBuffer::new(0, TaskId(9), TaskId(10), "camera.raw_stream")),
                Some(ShmRingBuffer::new(1, TaskId(8), TaskId(10), "audio.pcm_stream")),
                Some(ShmRingBuffer::new(2, TaskId(4), TaskId(1), "vault.bulk_crypt")),
                Some(ShmRingBuffer::new(3, TaskId(5), TaskId(5), "mesh.dma_pipe")),
                Some(ShmRingBuffer::new(4, TaskId(6), TaskId(10), "compositor.display_list")),
                None,
                None,
                None,
            ],
        }
    }

    /// Allocates or reconfigures a shared memory pipe.
    pub fn create_pipe(
        &mut self,
        pipe_id: u32,
        producer: TaskId,
        consumer: TaskId,
        name: &'static str,
    ) -> Result<(), ShmError> {
        let idx = pipe_id as usize;
        if idx >= self.pipes.len() || producer == consumer { return Err(ShmError::InvalidArguments); }
        // WHY an identical pair is allowed to re-ask: a producer that answers one
        // query per request needs the same channel again on the next request, and
        // the endpoints that were authorised last time are the endpoints being
        // named now, so nothing new is being granted. The ring is emptied first
        // because the bytes left in it belong to the turn that has already ended,
        // and a channel that carried two answers would hand the consumer the older
        // answer first. Any other pair is still refused, which is what stops one
        // task pointing another task's channel at itself.
        if let Some(pipe) = self.pipes[idx].as_mut() {
            if pipe.producer == producer && pipe.consumer == consumer {
                pipe.reset();
                return Ok(());
            }
            return Err(ShmError::PermissionDenied);
        }
        self.pipes[idx] = Some(ShmRingBuffer::new(pipe_id, producer, consumer, name));
        Ok(())
    }

    /// Dispatches a high-throughput zero-copy DMA transfer to the specified pipe.
    pub fn transfer(
        &mut self,
        pipe_id: u32,
        sender: TaskId,
        data: &[u8],
        flags: u32,
        ticks: u64,
    ) -> Result<u64, ShmError> {
        let idx = pipe_id as usize;
        if idx >= self.pipes.len() {
            return Err(ShmError::PipeNotFound);
        }
        let pipe = self.pipes[idx].as_mut().ok_or(ShmError::PipeNotFound)?;
        let consumer = pipe.consumer;
        let seq = pipe.write(sender, data, flags, ticks)?;
        crate::event::post_event(consumer, crate::event::EVENT_SHM_READABLE);
        Ok(seq)
    }

    /// Reads incoming zero-copy data from the specified pipe.
    pub fn receive(
        &mut self,
        pipe_id: u32,
        receiver: TaskId,
        out_buf: &mut [u8],
    ) -> Result<(usize, u64), ShmError> {
        let idx = pipe_id as usize;
        if idx >= self.pipes.len() {
            return Err(ShmError::PipeNotFound);
        }
        let pipe = self.pipes[idx].as_mut().ok_or(ShmError::PipeNotFound)?;
        let producer = pipe.producer;
        let res = pipe.read(receiver, out_buf)?;
        crate::event::post_event(producer, crate::event::EVENT_SHM_WRITABLE);
        Ok(res)
    }

    /// Removes shared-memory channels owned by or connected to a dead task.
    ///
    /// This prevents stale TaskIds from retaining producer/consumer authority
    /// after task teardown and makes fixed pipe slots reusable.
    pub fn cleanup_task(&mut self, task_id: TaskId) -> usize {
        let mut removed = 0;
        for pipe in self.pipes.iter_mut() {
            if pipe.as_ref().map(|p| p.producer == task_id || p.consumer == task_id).unwrap_or(false) {
                *pipe = None;
                removed += 1;
            }
        }
        removed
    }

    /// Returns performance statistics for a given pipe.
    pub fn get_stats(&self, pipe_id: u32) -> Option<(usize, u64, usize, usize)> {
        let idx = pipe_id as usize;
        if idx >= self.pipes.len() {
            return None;
        }
        self.pipes[idx].as_ref().map(|p| {
            (
                p.total_bytes,
                p.total_packets,
                p.available_read(),
                p.available_write(),
            )
        })
    }

    /// Executes thorough self-tests on the Zero-Copy DMA ring buffer implementation.
    pub fn self_test(&mut self) {
        let pipe = self.pipes[0].as_mut().expect("Pipe 0 must exist");
        let producer = pipe.producer;
        let consumer = pipe.consumer;

        // 1. Single packet round-trip transfer
        let mut test_payload = [0u8; 1024];
        let header = b"TEST_DMA_RAW_FRAME_1024";
        test_payload[..header.len()].copy_from_slice(header);
        test_payload[1020..1024].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());

        let seq1 = pipe
            .write(producer, &test_payload, DMA_FLAG_ACCEL, 100)
            .expect("Failed to write to empty ring buffer");
        assert_eq!(seq1, 1, "First sequence must be 1");

        let mut rx_buf = [0u8; 1024];
        let (rx_len, rx_seq) = pipe
            .read(consumer, &mut rx_buf)
            .expect("Failed to read from ring buffer");
        assert_eq!(rx_len, 1024);
        assert_eq!(rx_seq, 1);
        assert_eq!(&rx_buf[..header.len()], header);
        assert_eq!(&rx_buf[1020..1024], &0xDEADBEEFu32.to_le_bytes());

        // 1b. Ownership boundary: unrelated tasks cannot inject or consume frames.
        assert_eq!(pipe.write(TaskId(99), b"FORGED", DMA_FLAG_ACCEL, 100), Err(ShmError::PermissionDenied));
        assert_eq!(pipe.read(TaskId(99), &mut rx_buf), Err(ShmError::PermissionDenied));

        // 2. Multi-packet streaming & sequence verification
        for i in 0..7 {
            let mut chunk = [0u8; 512];
            chunk[0] = i as u8;
            chunk[511] = (i * 3) as u8;
            let seq = pipe
                .write(producer, &chunk, DMA_FLAG_ACCEL, 101 + i as u64)
                .expect("Streaming write failed");
            assert_eq!(seq, 2 + i as u64);
        }

        // 3. Ring buffer full / overflow protection test
        let overflow_attempt = pipe.write(producer, b"OVERFLOW", DMA_FLAG_ACCEL, 200);
        assert_eq!(
            overflow_attempt,
            Err(ShmError::RingFull),
            "Ring buffer overflow must be cleanly rejected"
        );

        // 4. Drain the ring buffer
        for i in 0..7 {
            let mut drain_buf = [0u8; 512];
            let (len, seq) = pipe
                .read(consumer, &mut drain_buf)
                .expect("Failed to drain stream");
            assert_eq!(len, 512);
            assert_eq!(seq, 2 + i as u64);
            assert_eq!(drain_buf[0], i as u8);
            assert_eq!(drain_buf[511], (i * 3) as u8);
        }

        // 5. Verify empty condition
        let empty_attempt = pipe.read(consumer, &mut rx_buf);
        assert_eq!(
            empty_attempt,
            Err(ShmError::RingEmpty),
            "Reading from empty ring buffer must return RingEmpty"
        );

        crate::serial_println!(
            "[TEST] Zero-Copy DMA SHM Pipe: High-throughput 8 KiB ring buffer streaming & CRC-32 verified PASSED."
        );
    }
}

/// Global system-wide Shared Memory Pipe Manager.
pub static SHM_PIPE_MANAGER: Mutex<ShmPipeManager> = Mutex::new(ShmPipeManager::new());

/// Initializes the Zero-Copy DMA SHM Pipe subsystem and executes boot self-tests.
pub fn init() {
    SHM_PIPE_MANAGER.lock().self_test();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_standard_vectors_match_ieee8023() {
        assert_eq!(compute_crc32(b""), 0);
        assert_eq!(compute_crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(compute_crc32(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
    }

    #[test]
    fn crc32_sliced_matches_byte_by_byte_for_all_lengths() {
        for len in 0..128 {
            let mut buf = [0u8; 128];
            for i in 0..len {
                buf[i] = ((i * 31 + 7) & 0xFF) as u8;
            }
            let sliced = compute_crc32(&buf[..len]);
            let mut ref_crc = 0xFFFF_FFFFu32;
            for &b in &buf[..len] {
                let idx = ((ref_crc ^ (b as u32)) & 0xFF) as usize;
                ref_crc = (ref_crc >> 8) ^ CRC32_TABLE[idx];
            }
            let expected = !ref_crc;
            assert_eq!(sliced, expected, "CRC mismatch at length {}", len);
        }
    }

    #[test]
    fn a_pair_can_reclaim_its_channel_and_starts_from_empty() {
        let mut mgr = ShmPipeManager::new();
        let service = TaskId(12);
        let app = TaskId(30);
        mgr.create_pipe(PIPE_DYNAMIC_FIRST, service, app, "user.dma_pipe")
            .expect("a free dynamic id is claimable");

        // One answer carried, and the consumer never drained the last byte.
        mgr.transfer(PIPE_DYNAMIC_FIRST, service, b"the first answer", 0, 10)
            .expect("stream to the claimed pipe");

        // The same pair asks again for its next turn.
        assert_eq!(
            mgr.create_pipe(PIPE_DYNAMIC_FIRST, service, app, "user.dma_pipe"),
            Ok(()),
            "re-asking for the channel this pair already holds grants nothing new"
        );

        // WHY this is the assertion that matters: without the reset the consumer
        // would read the previous turn's bytes first and never learn they were
        // stale.
        let mut rx = [0u8; 64];
        assert_eq!(
            mgr.receive(PIPE_DYNAMIC_FIRST, app, &mut rx),
            Err(ShmError::RingEmpty),
            "a reclaimed channel must not carry over the bytes of the turn that ended"
        );

        // A different pair may not take the slot over.
        assert_eq!(
            mgr.create_pipe(PIPE_DYNAMIC_FIRST, service, TaskId(31), "user.dma_pipe"),
            Err(ShmError::PermissionDenied),
            "another consumer must not be able to point this channel at itself"
        );
    }

    #[test]
    fn the_compiled_in_channels_are_not_claimable() {
        // WHY the range is asserted here: the fixed channels' endpoints are set at
        // boot, and a userspace request naming one is an attempt to re-point a
        // channel the platform owns.
        assert!(!is_dynamic_pipe_id(PIPE_COMPOSITOR_DISPLAY_LIST));
        assert!(is_dynamic_pipe_id(PIPE_DYNAMIC_FIRST));
        assert!(!is_dynamic_pipe_id(PIPE_DYNAMIC_FIRST + PIPE_DYNAMIC_COUNT));
        assert_eq!(PIPE_DYNAMIC_FIRST + PIPE_DYNAMIC_COUNT, 8);
    }

    #[test]
    fn a_claimed_channel_is_freed_when_either_endpoint_exits() {
        let mut mgr = ShmPipeManager::new();
        let service = TaskId(12);
        let app = TaskId(30);
        mgr.create_pipe(PIPE_DYNAMIC_FIRST + 1, service, app, "user.dma_pipe")
            .expect("claim");
        assert_eq!(mgr.cleanup_task(app), 1, "the consumer exiting frees the slot");
        mgr.create_pipe(PIPE_DYNAMIC_FIRST + 1, service, TaskId(31), "user.dma_pipe")
            .expect("a freed slot is claimable by the next pair");
    }

    #[test]
    fn compositor_display_list_pipe_roundtrip_transfer() {
        let mut mgr = ShmPipeManager::new();
        // Pipe 4 is compositor.display_list, producer=TaskId(6), consumer=TaskId(10)
        let mut dplt_wire = [0u8; 32];
        dplt_wire[0..4].copy_from_slice(b"DPLT");
        dplt_wire[4] = 1; // version
        dplt_wire[5] = 1; // flags (has damage rect)
        dplt_wire[8..12].copy_from_slice(&0xFF123456u32.to_le_bytes()); // bg_color
        dplt_wire[12] = 5; // OPCODE_FILL_RECT
        dplt_wire[13..15].copy_from_slice(&10u16.to_le_bytes()); // x
        dplt_wire[15..17].copy_from_slice(&20u16.to_le_bytes()); // y
        dplt_wire[17..19].copy_from_slice(&100u16.to_le_bytes()); // w
        dplt_wire[19..21].copy_from_slice(&50u16.to_le_bytes()); // h
        dplt_wire[21..25].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // fill

        let seq = mgr
            .transfer(PIPE_COMPOSITOR_DISPLAY_LIST, TaskId(6), &dplt_wire[..25], 0, 100)
            .expect("Transfer display list over pipe");
        assert_eq!(seq, 1);

        let mut rx_buf = [0u8; 64];
        let (rx_len, rx_seq) = mgr
            .receive(PIPE_COMPOSITOR_DISPLAY_LIST, TaskId(10), &mut rx_buf)
            .expect("Receive display list from compositor pipe");
        assert_eq!(rx_len, 25);
        assert_eq!(rx_seq, 1);
        assert_eq!(&rx_buf[0..4], b"DPLT");
        assert_eq!(&rx_buf[8..12], &0xFF123456u32.to_le_bytes());
        assert_eq!(rx_buf[12], 5); // OPCODE_FILL_RECT
    }
}

