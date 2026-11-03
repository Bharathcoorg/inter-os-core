// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Virtual Filesystem Subsystem for Inter OS
//!
//! Exposes hierarchical file structures, inode management, and zero-trust
//! capability checks for file reads and writes.

pub mod vfs;
pub mod devfs;

pub use vfs::{
    FileDescriptor, FileStat, FileType, FsError, Inode, InodeId, Vfs, VFS,
    init, MAX_FD_PER_TASK,
};
pub use devfs::{DeviceKind, device_read, device_write, resolve_device};

/// Releases all VFS descriptors owned by a terminating task.
pub fn cleanup_task(task_id: crate::task::TaskId) -> usize {
    VFS.lock().cleanup_task(task_id)
}