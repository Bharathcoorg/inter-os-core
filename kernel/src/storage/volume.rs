// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Platform-neutral storage volume and partition descriptions.

use crate::storage::block::BlockDevice;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeKind { System, Data, Recovery, Vendor, User }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeDescriptor {
    pub id: u32,
    pub kind: VolumeKind,
    pub start_lba: u32,
    pub sector_count: u32,
    pub read_only: bool,
}

impl VolumeDescriptor {
    pub const fn new(id: u32, kind: VolumeKind, start_lba: u32, sector_count: u32, read_only: bool) -> Self {
        Self { id, kind, start_lba, sector_count, read_only }
    }

    pub fn end_lba(&self) -> Option<u32> { self.start_lba.checked_add(self.sector_count) }

    pub fn contains(&self, lba: u32) -> bool {
        lba >= self.start_lba && lba < self.end_lba().unwrap_or(u32::MAX)
    }

    pub fn validate_against(&self, device: &dyn BlockDevice) -> bool {
        self.sector_count > 0 && self.end_lba().map(|end| end <= device.sector_count()).unwrap_or(false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeError { InvalidRange, Overlap, ReadOnly, NotFound, TableFull }

pub const MAX_VOLUMES: usize = 16;

pub struct VolumeTable {
    entries: [Option<VolumeDescriptor>; MAX_VOLUMES],
}

impl VolumeTable {
    pub const fn new() -> Self { Self { entries: [None; MAX_VOLUMES] } }

    pub fn register(&mut self, volume: VolumeDescriptor, device: &dyn BlockDevice) -> Result<(), VolumeError> {
        if !volume.validate_against(device) { return Err(VolumeError::InvalidRange); }
        for existing in self.entries.iter().flatten() {
            if existing.id == volume.id || existing.contains(volume.start_lba) || volume.contains(existing.start_lba) {
                return Err(VolumeError::Overlap);
            }
        }
        if let Some(slot) = self.entries.iter_mut().find(|entry| entry.is_none()) {
            *slot = Some(volume);
            Ok(())
        } else { Err(VolumeError::TableFull) }
    }

    pub fn get(&self, id: u32) -> Option<VolumeDescriptor> {
        self.entries.iter().flatten().find(|v| v.id == id).copied()
    }

    /// Resolves a logical sector to its owning volume.
    pub fn resolve_lba(&self, lba: u32) -> Option<VolumeDescriptor> {
        self.entries.iter().flatten().find(|volume| volume.contains(lba)).copied()
    }

    /// Removes a registered volume descriptor without touching its underlying data.
    ///
    /// Data destruction is deliberately outside this registry API; callers must
    /// explicitly format or erase storage through the appropriate backend.
    pub fn unregister(&mut self, id: u32) -> Result<VolumeDescriptor, VolumeError> {
        let slot = self.entries.iter_mut()
            .find(|entry| entry.as_ref().map(|volume| volume.id == id).unwrap_or(false))
            .ok_or(VolumeError::NotFound)?;
        slot.take().ok_or(VolumeError::NotFound)
    }

    pub fn descriptors(&self, out: &mut [VolumeDescriptor]) -> usize {
        let mut count = 0;
        for volume in self.entries.iter().flatten() {
            if count == out.len() { break; }
            out[count] = *volume;
            count += 1;
        }
        count
    }
}
