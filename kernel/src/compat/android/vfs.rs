// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Virtualized Android Sandbox Filesystem (Zone 4 Isolation)
//!
//! Provides an isolated filesystem view for Android guest processes.
//! Enforces strict non-leakage invariants: Android applications can NEVER access
//! native Inter OS private storage, sovereign keys, or kernel policy partitions.
//!
//! Implements Android Scoped Storage and per-app directory isolation:
//! - Root-bound canonical path resolution (preventing directory traversal out of jail).
//! - Per-app isolated private sandbox (`/data/data/<package_name>/`).
//! - Scoped external storage (`/sdcard/Android/data/<package_name>/`).
//! - Symlink resolution jail (cycle detection & escape prevention).
//! - Snapshot serialization for durable persistence across guest lifecycles.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

const SNAPSHOT_MAGIC: &[u8; 16] = b"ANDR_VFS_SNAP_V1";
const MAX_SYMLINK_HOPS: usize = 8;

/// Errors returned by the Android virtualized filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndroidFsError {
    /// Attempted unauthorized access to host Inter OS private state or sandbox traversal escape.
    SecurityViolation,
    /// Path not found in the virtual Android tree.
    NotFound,
    /// Access denied by container policy or scoped storage permissions.
    PermissionDenied,
    /// Corrupt snapshot payload or deserialization failure.
    CorruptSnapshot,
}

/// Storage capability flags granted to an Android application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AndroidStorageCaps {
    pub read_external_storage: bool,
    pub write_external_storage: bool,
    pub manage_external_storage: bool,
}

impl AndroidStorageCaps {
    /// Sandboxed isolated storage only (private dirs only).
    pub const fn isolated() -> Self {
        Self {
            read_external_storage: false,
            write_external_storage: false,
            manage_external_storage: false,
        }
    }

    /// Standard media permissions.
    pub const fn standard_media() -> Self {
        Self {
            read_external_storage: true,
            write_external_storage: true,
            manage_external_storage: false,
        }
    }

    /// All files management permission (e.g. File Manager app).
    pub const fn all_files_access() -> Self {
        Self {
            read_external_storage: true,
            write_external_storage: true,
            manage_external_storage: true,
        }
    }
}

/// Caller context for capability-checked filesystem operations.
#[derive(Debug, Clone)]
pub struct AndroidCaller {
    pub package_name: String,
    pub app_id: u32,
    pub caps: AndroidStorageCaps,
}

/// An isolated virtual filesystem inode within the Android container sandbox.
#[derive(Debug, Clone)]
pub struct AndroidVirtualInode {
    pub path: String,
    pub data: Vec<u8>,
    pub is_directory: bool,
    pub symlink_target: Option<String>,
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
        self.create_directory("/sdcard/DCIM");
        self.create_directory("/sdcard/Download");
        self.create_directory("/sdcard/Documents");
    }

    /// Provisions per-application isolated directories for a given package.
    pub fn provision_app_sandbox(&mut self, package_name: &str) {
        let internal_base = format!("/data/data/{}", package_name);
        self.create_directory(&internal_base);
        self.create_directory(&format!("{}/files", internal_base));
        self.create_directory(&format!("{}/shared_prefs", internal_base));
        self.create_directory(&format!("{}/databases", internal_base));
        self.create_directory(&format!("{}/cache", internal_base));

        let external_base = format!("/sdcard/Android/data/{}", package_name);
        self.create_directory(&external_base);
        self.create_directory(&format!("{}/files", external_base));
        self.create_directory(&format!("{}/cache", external_base));
    }

    /// Normalizes and canonicalizes a path, verifying it does not traverse host Inter OS boundaries.
    pub fn canonicalize_path(&mut self, path: &str) -> Result<String, AndroidFsError> {
        if path.is_empty() || path.contains('\0') {
            self.security_violations += 1;
            return Err(AndroidFsError::SecurityViolation);
        }

        // Host security boundary check
        if path.starts_with("/system/etc")
            || path.starts_with("/packages")
            || path.starts_with("/system/recovery")
            || path == "/data/audit.log"
        {
            self.security_violations += 1;
            return Err(AndroidFsError::SecurityViolation);
        }

        let mut segments: Vec<&str> = Vec::new();
        for seg in path.split('/') {
            if seg.is_empty() || seg == "." {
                continue;
            } else if seg == ".." {
                if segments.pop().is_none() {
                    // Attempting to escape root '/'
                    self.security_violations += 1;
                    return Err(AndroidFsError::SecurityViolation);
                }
            } else {
                segments.push(seg);
            }
        }

        let mut canonical = String::from("/");
        for (i, seg) in segments.iter().enumerate() {
            if i > 0 {
                canonical.push('/');
            }
            canonical.push_str(seg);
        }

        // Re-check normalized path against sensitive host files
        if canonical.starts_with("/system/etc")
            || canonical.starts_with("/packages")
            || canonical.starts_with("/system/recovery")
            || canonical == "/data/audit.log"
        {
            self.security_violations += 1;
            return Err(AndroidFsError::SecurityViolation);
        }

        Ok(canonical)
    }

    /// Resolves symlinks iteratively up to MAX_SYMLINK_HOPS, detecting cycles and escapes.
    fn resolve_symlinks(&mut self, start_path: &str) -> Result<String, AndroidFsError> {
        let mut current = self.canonicalize_path(start_path)?;
        let mut hops = 0;

        while hops < MAX_SYMLINK_HOPS {
            let target_opt = self.inodes.get(&current).and_then(|inode| inode.symlink_target.clone());
            if let Some(target) = target_opt {
                current = self.canonicalize_path(&target)?;
                hops += 1;
                continue;
            }
            return Ok(current);
        }

        // Exceeded symlink hop limit (cycle or excessive chaining)
        self.security_violations += 1;
        Err(AndroidFsError::SecurityViolation)
    }

    /// Checks if a caller is authorized to access the canonical target path.
    fn check_caller_access(
        &mut self,
        caller: &AndroidCaller,
        canonical_path: &str,
        is_write: bool,
    ) -> Result<(), AndroidFsError> {
        // 1. Private data access: /data/data/<pkg>/**
        if let Some(rest) = canonical_path.strip_prefix("/data/data/") {
            let pkg_owner = rest.split('/').next().unwrap_or("");
            if pkg_owner != caller.package_name {
                // Cross-app private data access strictly forbidden
                self.security_violations += 1;
                return Err(AndroidFsError::SecurityViolation);
            }
            return Ok(());
        }

        // 2. Scoped external storage: /sdcard/Android/data/<pkg>/**
        if let Some(rest) = canonical_path.strip_prefix("/sdcard/Android/data/") {
            let pkg_owner = rest.split('/').next().unwrap_or("");
            if pkg_owner == caller.package_name {
                return Ok(());
            }
            // Another app's scoped external data requires manage_external_storage
            if !caller.caps.manage_external_storage {
                return Err(AndroidFsError::PermissionDenied);
            }
            return Ok(());
        }

        // 3. Shared external storage: /sdcard/**
        if canonical_path.starts_with("/sdcard") {
            if caller.caps.manage_external_storage {
                return Ok(());
            }
            if is_write {
                if !caller.caps.write_external_storage {
                    return Err(AndroidFsError::PermissionDenied);
                }
            } else {
                if !caller.caps.read_external_storage {
                    return Err(AndroidFsError::PermissionDenied);
                }
            }
            return Ok(());
        }

        // 4. System framework paths: /system/** (read-only for apps)
        if canonical_path.starts_with("/system") {
            if is_write {
                return Err(AndroidFsError::PermissionDenied);
            }
            return Ok(());
        }

        Ok(())
    }

    /// Reads a file with caller capability gating and symlink resolution.
    pub fn open_read_as(&mut self, caller: &AndroidCaller, target_path: &str) -> Result<&[u8], AndroidFsError> {
        let canonical = self.resolve_symlinks(target_path)?;
        self.check_caller_access(caller, &canonical, false)?;

        if let Some(inode) = self.inodes.get(&canonical) {
            if inode.is_directory {
                return Err(AndroidFsError::PermissionDenied);
            }
            Ok(&inode.data)
        } else {
            Err(AndroidFsError::NotFound)
        }
    }

    /// Writes a file with caller capability gating and symlink resolution.
    pub fn open_write_as(
        &mut self,
        caller: &AndroidCaller,
        target_path: &str,
        data: &[u8],
    ) -> Result<(), AndroidFsError> {
        let canonical = self.resolve_symlinks(target_path)?;
        self.check_caller_access(caller, &canonical, true)?;
        self.create_file(&canonical, data);
        Ok(())
    }

    /// Deletes a file with caller capability gating.
    pub fn delete_file_as(&mut self, caller: &AndroidCaller, target_path: &str) -> Result<bool, AndroidFsError> {
        let canonical = self.resolve_symlinks(target_path)?;
        self.check_caller_access(caller, &canonical, true)?;
        Ok(self.inodes.remove(&canonical).is_some())
    }

    /// Creates a symlink in the virtual filesystem with caller capability checks.
    pub fn create_symlink_as(
        &mut self,
        caller: &AndroidCaller,
        link_path: &str,
        target_path: &str,
    ) -> Result<(), AndroidFsError> {
        let canonical_link = self.canonicalize_path(link_path)?;
        let canonical_target = self.canonicalize_path(target_path)?;
        self.check_caller_access(caller, &canonical_link, true)?;

        self.inodes.insert(
            canonical_link.clone(),
            AndroidVirtualInode {
                path: canonical_link,
                data: Vec::new(),
                is_directory: false,
                symlink_target: Some(canonical_target),
            },
        );
        Ok(())
    }

    /// Renames a file with caller capability checks on both source and destination.
    pub fn rename_file_as(
        &mut self,
        caller: &AndroidCaller,
        from_path: &str,
        to_path: &str,
    ) -> Result<(), AndroidFsError> {
        let from_canon = self.resolve_symlinks(from_path)?;
        let to_canon = self.canonicalize_path(to_path)?;
        self.check_caller_access(caller, &from_canon, true)?;
        self.check_caller_access(caller, &to_canon, true)?;

        if let Some(mut inode) = self.inodes.remove(&from_canon) {
            inode.path = to_canon.clone();
            self.inodes.insert(to_canon, inode);
            Ok(())
        } else {
            Err(AndroidFsError::NotFound)
        }
    }

    /// Creates a directory in the virtual Android sandbox.
    pub fn create_directory(&mut self, path: &str) {
        self.inodes.insert(
            String::from(path),
            AndroidVirtualInode {
                path: String::from(path),
                data: Vec::new(),
                is_directory: true,
                symlink_target: None,
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
                symlink_target: None,
            },
        );
    }

    /// Reads a file from the Android sandbox (backwards-compatible privileged helper).
    pub fn open_read(&mut self, target_path: &str) -> Result<&[u8], AndroidFsError> {
        let canonical = self.canonicalize_path(target_path)?;
        if let Some(inode) = self.inodes.get(&canonical) {
            if inode.is_directory {
                return Err(AndroidFsError::PermissionDenied);
            }
            Ok(&inode.data)
        } else {
            Err(AndroidFsError::NotFound)
        }
    }

    /// Writes a file into the Android sandbox (backwards-compatible privileged helper).
    pub fn open_write(&mut self, target_path: &str, data: &[u8]) -> Result<(), AndroidFsError> {
        let canonical = self.canonicalize_path(target_path)?;
        self.create_file(&canonical, data);
        Ok(())
    }

    /// Deletes a file or directory from the sandbox (backwards-compatible privileged helper).
    pub fn delete_file(&mut self, target_path: &str) -> Result<bool, AndroidFsError> {
        let canonical = self.canonicalize_path(target_path)?;
        Ok(self.inodes.remove(&canonical).is_some())
    }

    /// Checks if a path exists in the sandbox.
    pub fn exists(&self, target_path: &str) -> bool {
        if target_path.starts_with("/system/etc")
            || target_path.starts_with("/packages")
            || target_path.starts_with("/system/recovery")
            || target_path == "/data/audit.log"
            || target_path.contains("..")
        {
            return false;
        }
        self.inodes.contains_key(target_path)
    }

    /// Checks if a path is a directory.
    pub fn is_directory(&self, target_path: &str) -> bool {
        self.inodes.get(target_path).map(|i| i.is_directory).unwrap_or(false)
    }

    /// Lists directory entries directly under target_path.
    pub fn list_directory(&self, target_path: &str) -> Result<Vec<String>, AndroidFsError> {
        let prefix = if target_path.ends_with('/') {
            String::from(target_path)
        } else {
            alloc::format!("{}/", target_path)
        };
        let mut entries = Vec::new();
        for key in self.inodes.keys() {
            if let Some(sub) = key.strip_prefix(&prefix) {
                if !sub.is_empty() && !sub.contains('/') {
                    entries.push(String::from(sub));
                }
            }
        }
        Ok(entries)
    }

    /// Returns the total number of intercepted security boundary breaches.
    pub fn security_violations(&self) -> usize {
        self.security_violations
    }

    /// Serializes the entire virtual filesystem state for persistent storage.
    pub fn export_snapshot(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(SNAPSHOT_MAGIC);

        let count = self.inodes.len() as u32;
        buf.extend_from_slice(&count.to_le_bytes());

        for inode in self.inodes.values() {
            let path_bytes = inode.path.as_bytes();
            buf.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(path_bytes);

            buf.push(if inode.is_directory { 1 } else { 0 });

            if let Some(ref target) = inode.symlink_target {
                let target_bytes = target.as_bytes();
                buf.extend_from_slice(&(target_bytes.len() as u32).to_le_bytes());
                buf.extend_from_slice(target_bytes);
            } else {
                buf.extend_from_slice(&0u32.to_le_bytes());
            }

            buf.extend_from_slice(&(inode.data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&inode.data);
        }

        buf
    }

    /// Restores filesystem state from a serialized snapshot.
    pub fn restore_snapshot(&mut self, bytes: &[u8]) -> Result<(), AndroidFsError> {
        if bytes.len() < 20 || &bytes[..16] != SNAPSHOT_MAGIC {
            return Err(AndroidFsError::CorruptSnapshot);
        }

        let mut offset = 16;
        let count = u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]) as usize;
        offset += 4;

        let mut restored_inodes = BTreeMap::new();

        for _ in 0..count {
            if offset + 4 > bytes.len() {
                return Err(AndroidFsError::CorruptSnapshot);
            }
            let path_len = u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]) as usize;
            offset += 4;
            if offset + path_len > bytes.len() {
                return Err(AndroidFsError::CorruptSnapshot);
            }
            let path_str = core::str::from_utf8(&bytes[offset..offset + path_len])
                .map_err(|_| AndroidFsError::CorruptSnapshot)?;
            offset += path_len;

            if offset >= bytes.len() {
                return Err(AndroidFsError::CorruptSnapshot);
            }
            let is_directory = bytes[offset] == 1;
            offset += 1;

            if offset + 4 > bytes.len() {
                return Err(AndroidFsError::CorruptSnapshot);
            }
            let symlink_len = u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]) as usize;
            offset += 4;

            let symlink_target = if symlink_len > 0 {
                if offset + symlink_len > bytes.len() {
                    return Err(AndroidFsError::CorruptSnapshot);
                }
                let target_str = core::str::from_utf8(&bytes[offset..offset + symlink_len])
                    .map_err(|_| AndroidFsError::CorruptSnapshot)?;
                offset += symlink_len;
                Some(String::from(target_str))
            } else {
                None
            };

            if offset + 4 > bytes.len() {
                return Err(AndroidFsError::CorruptSnapshot);
            }
            let data_len = u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]) as usize;
            offset += 4;

            if offset + data_len > bytes.len() {
                return Err(AndroidFsError::CorruptSnapshot);
            }
            let data = bytes[offset..offset + data_len].to_vec();
            offset += data_len;

            restored_inodes.insert(
                String::from(path_str),
                AndroidVirtualInode {
                    path: String::from(path_str),
                    data,
                    is_directory,
                    symlink_target,
                },
            );
        }

        self.inodes = restored_inodes;
        Ok(())
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

    #[test]
    fn test_vfs_per_app_sandbox_isolation_and_boundary_checks() {
        let mut fs = AndroidSandboxFs::empty();
        fs.populate_defaults();

        let app_a = AndroidCaller {
            package_name: String::from("org.fossify.filemanager"),
            app_id: 10001,
            caps: AndroidStorageCaps::isolated(),
        };
        let app_b = AndroidCaller {
            package_name: String::from("com.example.malicious"),
            app_id: 10002,
            caps: AndroidStorageCaps::isolated(),
        };

        fs.provision_app_sandbox("org.fossify.filemanager");
        fs.provision_app_sandbox("com.example.malicious");

        // App A writes to its private storage
        let file_a = "/data/data/org.fossify.filemanager/files/keys.json";
        fs.open_write_as(&app_a, file_a, b"{\"secret\":\"12345\"}").expect("write private file");

        // App A can read its own private storage
        let content = fs.open_read_as(&app_a, file_a).expect("read own private file");
        assert_eq!(content, b"{\"secret\":\"12345\"}");

        // App B attempts to read App A's private storage -> SecurityViolation
        let breach = fs.open_read_as(&app_b, file_a);
        assert_eq!(breach, Err(AndroidFsError::SecurityViolation));

        // Traversal attempt by App B: /data/data/com.example.malicious/../org.fossify.filemanager/files/keys.json
        let escape_attempt = "/data/data/com.example.malicious/../org.fossify.filemanager/files/keys.json";
        let escaped = fs.open_read_as(&app_b, escape_attempt);
        assert_eq!(escaped, Err(AndroidFsError::SecurityViolation));
    }

    #[test]
    fn test_vfs_scoped_storage_capabilities() {
        let mut fs = AndroidSandboxFs::empty();
        fs.populate_defaults();
        fs.create_file("/sdcard/DCIM/photo.jpg", b"JPEG_DATA_1234");

        let app_isolated = AndroidCaller {
            package_name: String::from("com.isolated.app"),
            app_id: 10003,
            caps: AndroidStorageCaps::isolated(),
        };
        let app_media = AndroidCaller {
            package_name: String::from("com.gallery.app"),
            app_id: 10004,
            caps: AndroidStorageCaps::standard_media(),
        };

        // Isolated app denied shared storage read
        assert_eq!(
            fs.open_read_as(&app_isolated, "/sdcard/DCIM/photo.jpg"),
            Err(AndroidFsError::PermissionDenied)
        );

        // Media app permitted shared storage read
        let data = fs.open_read_as(&app_media, "/sdcard/DCIM/photo.jpg").expect("read shared photo");
        assert_eq!(data, b"JPEG_DATA_1234");
    }

    #[test]
    fn test_vfs_symlink_jail_and_cycle_prevention() {
        let mut fs = AndroidSandboxFs::empty();
        fs.populate_defaults();

        let app = AndroidCaller {
            package_name: String::from("com.test.app"),
            app_id: 10005,
            caps: AndroidStorageCaps::isolated(),
        };
        fs.provision_app_sandbox("com.test.app");

        // Symlink pointing to host private file is rejected on creation/access
        let link_path = "/data/data/com.test.app/files/bad_symlink";
        assert_eq!(
            fs.create_symlink_as(&app, link_path, "/system/etc/shadow"),
            Err(AndroidFsError::SecurityViolation)
        );

        // Circular symlink: a -> b -> a
        let link_a = "/data/data/com.test.app/files/link_a";
        let link_b = "/data/data/com.test.app/files/link_b";
        fs.create_symlink_as(&app, link_a, link_b).unwrap();
        fs.create_symlink_as(&app, link_b, link_a).unwrap();

        assert_eq!(fs.open_read_as(&app, link_a), Err(AndroidFsError::SecurityViolation));
    }

    #[test]
    fn test_vfs_snapshot_persistence_roundtrip() {
        let mut fs = AndroidSandboxFs::empty();
        fs.populate_defaults();
        fs.provision_app_sandbox("org.fossify.filemanager");
        fs.create_file("/data/data/org.fossify.filemanager/files/config.xml", b"<config>val</config>");

        let snapshot = fs.export_snapshot();
        assert!(snapshot.len() > 32);

        // Create fresh empty filesystem and restore snapshot
        let mut restored_fs = AndroidSandboxFs::empty();
        restored_fs.restore_snapshot(&snapshot).expect("restore snapshot");

        let content = restored_fs.open_read("/data/data/org.fossify.filemanager/files/config.xml").expect("read restored file");
        assert_eq!(content, b"<config>val</config>");
    }
}
