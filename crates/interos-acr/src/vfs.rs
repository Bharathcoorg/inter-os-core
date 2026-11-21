// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Virtualized Android Sandbox Filesystem (Zone 4 Isolation)
//!
//! Provides an isolated filesystem view for Android guest processes.
//! Enforces strict non-leakage invariants: Android applications can NEVER access
//! native Inter OS private storage, sovereign keys, or kernel policy partitions.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

/// Errors returned by the Android virtualized filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndroidFsError {
    /// Attempted unauthorized access to host Inter OS private state.
    SecurityViolation,
    /// Path not found in the virtual Android tree.
    NotFound,
    /// Access denied by container policy.
    PermissionDenied,
}

/// An isolated virtual filesystem inode within the Android container sandbox.
#[derive(Debug, Clone)]
pub struct AndroidVirtualInode {
    pub path: String,
    pub data: Vec<u8>,
    pub is_directory: bool,
}

/// Virtualized Android sandbox filesystem.
pub struct AndroidSandboxFs {
    inodes: BTreeMap<String, AndroidVirtualInode>,
    security_violations: usize,
}

impl AndroidSandboxFs {
    /// Creates an empty virtual Android sandbox filesystem.
    pub const fn empty() -> Self {
        Self {
            inodes: BTreeMap::new(),
            security_violations: 0,
        }
    }

    /// Populates standard Android guest mount points and virtual framework binaries.
    pub fn populate_defaults(&mut self) {
        if !self.inodes.is_empty() {
            return;
        }
        // Initialize standard Android guest mount points
        self.create_directory("/system");
        self.create_directory("/system/bin");
        self.create_directory("/system/framework");
        self.create_directory("/data");
        self.create_directory("/data/app");
        self.create_directory("/data/data");
        self.create_directory("/sdcard");
        self.create_directory("/sdcard/Android");
        self.create_directory("/sdcard/Android/data");

        // Do not fabricate framework binaries. A path is present only when a real,
        // validated runtime artifact has been installed into this container.
    }

    /// Creates a directory in the virtual Android sandbox.
    pub fn create_directory(&mut self, path: &str) {
        self.inodes.insert(
            String::from(path),
            AndroidVirtualInode {
                path: String::from(path),
                data: Vec::new(),
                is_directory: true,
            },
        );
    }

    /// Creates a file in the virtual Android sandbox.
    pub fn create_file(&mut self, path: &str, content: &[u8]) {
        self.inodes.insert(
            String::from(path),
            AndroidVirtualInode {
                path: String::from(path),
                data: content.to_vec(),
                is_directory: false,
            },
        );
    }

    /// Reads a file from the Android sandbox with strict boundary checking.
    ///
    /// Zone 4 Security Boundary:
    /// Any request targeting native Inter OS private directories
    /// (`/system/etc/policy.json`, `/system/etc/device_id`, `/packages/`, `/data/audit.log`)
    /// is intercepted, blocked, and incremented as a critical security violation.
    pub fn open_read(&mut self, target_path: &str) -> Result<&[u8], AndroidFsError> {
        // Enforce boundary: Block access to native Inter OS host files
        if target_path.starts_with("/system/etc")
            || target_path.starts_with("/packages")
            || target_path == "/data/audit.log"
            || target_path.contains("..")
        {
            self.security_violations += 1;
            return Err(AndroidFsError::SecurityViolation);
        }

        // Search virtual Android sandbox tree
        if let Some(inode) = self.inodes.get(target_path) {
            if inode.is_directory {
                return Err(AndroidFsError::PermissionDenied);
            }
            Ok(&inode.data)
        } else {
            Err(AndroidFsError::NotFound)
        }
    }

    /// Returns the total number of intercepted security boundary breaches.
    pub fn security_violations(&self) -> usize {
        self.security_violations
    }
}

/// Global system-wide virtual Android sandbox filesystem.
pub static ANDROID_SANDBOX_FS: Mutex<AndroidSandboxFs> = Mutex::new(AndroidSandboxFs::empty());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vfs_sandbox_defaults_and_creation() {
        let mut fs = AndroidSandboxFs::empty();
        fs.populate_defaults();
        assert_eq!(fs.security_violations(), 0);

        // Attempting to read a directory should yield PermissionDenied
        assert_eq!(fs.open_read("/system"), Err(AndroidFsError::PermissionDenied));
        assert_eq!(fs.open_read("/nonexistent"), Err(AndroidFsError::NotFound));

        // Create and read a regular file
        fs.create_file("/data/data/app.txt", b"android sandbox test data");
        let content = fs.open_read("/data/data/app.txt").expect("read created file");
        assert_eq!(content, b"android sandbox test data");
    }

    #[test]
    fn test_vfs_security_violations_intercepted() {
        let mut fs = AndroidSandboxFs::empty();
        fs.populate_defaults();

        // Host private directories must trigger SecurityViolation and increment counter
        assert_eq!(fs.open_read("/system/etc/policy.json"), Err(AndroidFsError::SecurityViolation));
        assert_eq!(fs.open_read("/packages/org.interos.vault.bin"), Err(AndroidFsError::SecurityViolation));
        assert_eq!(fs.open_read("/data/audit.log"), Err(AndroidFsError::SecurityViolation));
        assert_eq!(fs.open_read("/sdcard/../system/etc/shadow"), Err(AndroidFsError::SecurityViolation));

        assert_eq!(fs.security_violations(), 4);
    }
}
