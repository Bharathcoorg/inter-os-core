// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! A fault-injecting fake block device for the host tests.
//!
//! WHY A FAKE AND NOT A FILE: the failures that matter to this subsystem are
//! not "the disk filled up" but "this sector write did not land", "this
//! sector landed half-written", "the flush failed", "this sector is
//! unreadable". A fake can be told to do exactly those things at exactly
//! these sectors, so a test can assert that the *previous* durable state
//! survives each one. A file-backed device can only fail coarsely.
//!
//! The fake models a sector-granular device, which is the granularity every
//! crash-consistency argument in this crate is made at.

use alloc::vec;
use alloc::vec::Vec;

use crate::state::SECTOR_SIZE;
use crate::store::{BlockDevice, BlockError};

/// A device whose failures can be scripted.
///
/// WHY IT IS `Clone`: a test needs the same media in two places at once — one
/// handle that fails on purpose and one that reads the result back — and a
/// clone of the whole device gives both handles the same bytes and the same
/// fault script. `clone_media` is the different thing: a power cycle, which
/// keeps the bytes and drops the faults.
#[derive(Clone)]
pub struct FakeDevice {
    sectors: Vec<u8>,
    total: u32,
    /// The sector whose write fails outright.
    pub fail_write_at_lba: Option<u32>,
    /// The sector whose write lands only this many bytes before failing.
    pub partial_write: Option<(u32, usize)>,
    /// The 1-based number of the write that fails outright.
    pub fail_nth_write: Option<usize>,
    /// Whether the next write fails outright. The flag clears itself, so a
    /// test can arm it immediately before the operation it wants to fail.
    pub fail_next_write: bool,
    /// The sector whose read fails.
    pub fail_read_at_lba: Option<u32>,
    /// Whether every flush fails.
    pub fail_flush: bool,
    /// How many writes have been attempted.
    pub writes: usize,
    /// How many flushes have been attempted.
    pub flushes: usize,
    /// The LBAs every successful write went to, in order.
    pub write_log: Vec<u32>,
}

impl FakeDevice {
    /// A device of `sectors` zeroed sectors.
    pub fn new(sectors: u32) -> FakeDevice {
        FakeDevice {
            sectors: vec![0u8; sectors as usize * SECTOR_SIZE],
            total: sectors,
            fail_write_at_lba: None,
            partial_write: None,
            fail_nth_write: None,
            fail_next_write: false,
            fail_read_at_lba: None,
            fail_flush: false,
            writes: 0,
            flushes: 0,
            write_log: Vec::new(),
        }
    }

    /// The raw bytes of the device.
    pub fn bytes(&self) -> &[u8] {
        &self.sectors
    }

    /// The raw bytes of one sector.
    pub fn sector(&self, lba: u32) -> &[u8] {
        let start = lba as usize * SECTOR_SIZE;
        &self.sectors[start..start + SECTOR_SIZE]
    }

    /// Flips bits in one byte of the media, as bit rot or a tamper would.
    pub fn corrupt(&mut self, lba: u32, offset: usize, xor: u8) {
        let start = lba as usize * SECTOR_SIZE + offset;
        self.sectors[start] ^= xor;
    }

    /// Writes bytes directly into the media, as a foreign writer would.
    pub fn poke(&mut self, lba: u32, offset: usize, bytes: &[u8]) {
        let start = lba as usize * SECTOR_SIZE + offset;
        self.sectors[start..start + bytes.len()].copy_from_slice(bytes);
    }

    /// Shrinks the device, so reads and writes past the end fail.
    pub fn truncate_to(&mut self, sectors: u32) {
        self.total = sectors;
        self.sectors.truncate(sectors as usize * SECTOR_SIZE);
    }

    /// Marks the device as bootable, with a master boot record signature.
    pub fn install_boot_signature(&mut self) {
        self.poke(0, 510, &[0x55, 0xAA]);
    }

    /// A second device holding exactly the same bytes.
    ///
    /// THIS IS THE POWER CYCLE. The engine's state is dropped and a new engine
    /// is built over a device that holds the same media, which is what a
    /// complete QEMU termination and relaunch looks like to the store.
    pub fn clone_media(&self) -> FakeDevice {
        let mut copy = FakeDevice::new(self.total);
        copy.sectors = self.sectors.clone();
        copy
    }
}

impl BlockDevice for FakeDevice {
    fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockError> {
        if self.fail_read_at_lba == Some(lba) {
            return Err(BlockError::ReadError);
        }
        if lba >= self.total {
            return Err(BlockError::LbaOutOfRange);
        }
        let start = lba as usize * SECTOR_SIZE;
        buf.copy_from_slice(&self.sectors[start..start + SECTOR_SIZE]);
        Ok(())
    }

    fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockError> {
        self.writes += 1;
        if self.fail_next_write {
            self.fail_next_write = false;
            return Err(BlockError::WriteError);
        }
        if self.fail_nth_write == Some(self.writes) {
            return Err(BlockError::WriteError);
        }
        if self.fail_write_at_lba == Some(lba) {
            return Err(BlockError::WriteError);
        }
        if lba >= self.total {
            return Err(BlockError::LbaOutOfRange);
        }
        let start = lba as usize * SECTOR_SIZE;
        if let Some((target, bytes)) = self.partial_write {
            if target == lba {
                // The sector landed half-written: the first `bytes` bytes are
                // on the media, the rest never arrive, and the call reports a
                // failure. This is what a torn write looks like.
                let take = bytes.min(SECTOR_SIZE);
                self.sectors[start..start + take].copy_from_slice(&buf[..take]);
                return Err(BlockError::WriteError);
            }
        }
        self.sectors[start..start + SECTOR_SIZE].copy_from_slice(buf);
        self.write_log.push(lba);
        Ok(())
    }

    fn sector_count(&self) -> u32 {
        self.total
    }

    fn flush(&mut self) -> Result<(), BlockError> {
        self.flushes += 1;
        if self.fail_flush {
            return Err(BlockError::FlushError);
        }
        Ok(())
    }
}
