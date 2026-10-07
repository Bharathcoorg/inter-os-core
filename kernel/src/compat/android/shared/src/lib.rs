//! Unified Android Untrusted Compatibility Runtime & Sandbox (ACR Shared)
//!
//! Provides bare-metal `#![no_std]` shared Android filesystem virtualization,
//! path canonicalization, Binder mediation, and container runtime structures
//! across both x86_64 and AArch64 kernel architectures.
//!
//! Enforces:
//! - Non-Negotiable Principle #9: Android applications are untrusted compatibility workloads
//!   and must never automatically access Inter OS sovereign private data, identity vaults,
//!   or raw microkernel IPC channels.
//! - Non-Negotiable Principle #11: Language and ecosystem choice does not determine trust.
//!   Strict capability tables and sandbox boundary policies determine trust.

#![no_std]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Errors returned by the Android virtualized filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndroidFsError {
    SecurityViolation,
    NotFound,
    PermissionDenied,
    InvalidPath,
}

/// An isolated virtual filesystem inode within the Android container sandbox.
#[derive(Debug, Clone)]
pub struct AndroidVirtualInode {
    pub path: String,
    pub data: Vec<u8>,
    pub is_directory: bool,
}

/// Resolves path components (`.` and `..`), checks for null bytes, and verifies
/// that the canonical path stays within authorized sandbox storage prefixes.
pub fn is_safe_android_path(path: &str, app_package: &str) -> bool {
    // 1. Reject any path containing null bytes
    if path.contains('\0') {
        return false;
    }

    // 2. Reject root escapes or reserved host partitions
    if path.starts_with("/vault")
        || path.starts_with("/identity")
        || path.starts_with("/dev")
        || path.starts_with("/system")
        || path.starts_with("/proc")
        || path.starts_with("/sys")
    {
        return false;
    }

    // 3. Canonicalize path by resolving '.' and '..' components
    let mut resolved_segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            if resolved_segments.pop().is_none() {
                // Attempted to traverse above virtual root
                return false;
            }
        } else {
            resolved_segments.push(segment);
        }
    }

    let mut canonical = String::with_capacity(path.len() + 1);
    for seg in resolved_segments {
        canonical.push('/');
        canonical.push_str(seg);
    }

    // 4. Must strictly reside within app-private directory or shared external storage
    let allowed_app_prefix = format!("/data/data/{}", app_package);
    let app_private = canonical == allowed_app_prefix
        || canonical.strip_prefix(&allowed_app_prefix)
            .map(|rest| rest.starts_with('/'))
            .unwrap_or(false);
    let shared_prefix = "/sdcard/Android/data";
    let shared_storage = canonical == shared_prefix
        || canonical.strip_prefix(shared_prefix)
            .map(|rest| rest.starts_with('/'))
            .unwrap_or(false);
    app_private || shared_storage
}

/// Virtualized Android sandbox filesystem.
pub struct AndroidSandboxFs {
    pub inodes: BTreeMap<String, AndroidVirtualInode>,
    pub security_violations: usize,
}

impl AndroidSandboxFs {
    /// Creates a new virtual Android sandbox filesystem.
    pub fn new() -> Self {
        let mut fs = Self {
            inodes: BTreeMap::new(),
            security_violations: 0,
        };
        fs.populate_defaults();
        fs
    }

    /// Populates standard Android guest mount points and virtual framework binaries.
    pub fn populate_defaults(&mut self) {
        self.create_directory("/system");
        self.create_directory("/system/bin");
        self.create_directory("/system/framework");
        self.create_directory("/data");
        self.create_directory("/data/data");
        self.create_directory("/sdcard");
        self.create_directory("/sdcard/Android/data");

        // Populate virtual framework stubs
        self.create_file("/system/bin/app_process", b"\x7FELF_ANDROID_APP_PROCESS_VIRTUAL_STUB");
        self.create_file("/system/framework/framework.jar", b"ANDROID_FRAMEWORK_JAR_ISOLATED_CONTAINER");
    }

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

    pub fn create_file(&mut self, path: &str, content: &[u8]) {
        self.inodes.insert(
            String::from(path),
            AndroidVirtualInode {
                path: String::from(path),
                data: Vec::from(content),
                is_directory: false,
            },
        );
    }

    /// Reads file from sandbox storage with strict path canonicalization.
    pub fn open_read(&mut self, app_package: &str, target_path: &str) -> Result<&[u8], AndroidFsError> {
        // Read-only system and framework files are allowed
        if target_path.starts_with("/system/framework/") || target_path.starts_with("/system/bin/") {
            if target_path.contains("..") || target_path.contains('\0') {
                self.security_violations += 1;
                return Err(AndroidFsError::SecurityViolation);
            }
        } else if !is_safe_android_path(target_path, app_package) {
            self.security_violations += 1;
            return Err(AndroidFsError::SecurityViolation);
        }

        if let Some(inode) = self.inodes.get(target_path) {
            if inode.is_directory {
                return Err(AndroidFsError::PermissionDenied);
            }
            Ok(&inode.data)
        } else {
            Err(AndroidFsError::NotFound)
        }
    }

    /// Writes file into sandbox storage with strict canonicalization.
    pub fn open_write(
        &mut self,
        app_package: &str,
        target_path: &str,
        content: &[u8],
    ) -> Result<(), AndroidFsError> {
        if !is_safe_android_path(target_path, app_package) {
            self.security_violations += 1;
            return Err(AndroidFsError::SecurityViolation);
        }

        self.create_file(target_path, content);
        Ok(())
    }
}

pub trait CapabilityChecker {
    fn has_android_guest_cap(&self) -> bool;
}

impl CapabilityChecker for bool {
    fn has_android_guest_cap(&self) -> bool {
        *self
    }
}

impl<T: CapabilityChecker> CapabilityChecker for &T {
    fn has_android_guest_cap(&self) -> bool {
        (*self).has_android_guest_cap()
    }
}

/// Zero-Trust Android Binder IPC Mediator.
pub struct BinderMediator {
    pub transactions_handled: usize,
    pub blocked_calls: usize,
}

impl BinderMediator {
    pub const fn new() -> Self {
        Self {
            transactions_handled: 0,
            blocked_calls: 0,
        }
    }

    /// Mediates an Android Binder transaction targeting system services.
    pub fn mediate_transaction<C: CapabilityChecker>(
        &mut self,
        _src_pid: u32,
        target_service: &str,
        caps: C,
    ) -> Result<(), &'static str> {
        self.transactions_handled += 1;

        // Block unauthorized attempts to access sovereign security enclave
        if target_service == "inter.identity.vault"
            || target_service == "inter.telephony.raw"
            || target_service == "inter.microkernel.ipc"
        {
            self.blocked_calls += 1;
            return Err("Access to sovereign native service denied by Zero-Trust Binder Mediator");
        }

        // Allow benign Android platform services
        if target_service == "android.app.ActivityManager"
            || target_service == "android.view.WindowManager"
        {
            return Ok(());
        }

        // Any other service requires explicit capability grant
        if caps.has_android_guest_cap() {
            Ok(())
        } else {
            self.blocked_calls += 1;
            Err("Unprivileged Binder transaction rejected")
        }
    }
}

/// Sandboxed Android Application Container Instance.
#[derive(Debug, Clone)]
pub struct AndroidAppContainer {
    pub pid: u32,
    pub package_name: String,
    pub memory_quota_bytes: usize,
}

/// Android Container Runtime Manager.
pub struct AndroidRuntime {
    pub next_pid: u32,
    pub containers: Vec<AndroidAppContainer>,
    pub sandbox_fs: AndroidSandboxFs,
    pub mediator: BinderMediator,
}

impl AndroidRuntime {
    pub fn new() -> Self {
        Self {
            next_pid: 1000,
            containers: Vec::new(),
            sandbox_fs: AndroidSandboxFs::new(),
            mediator: BinderMediator::new(),
        }
    }

    pub fn spawn_app(&mut self, package_name: &str) -> u32 {
        let pid = self.next_pid;
        self.next_pid += 1;

        self.containers.push(AndroidAppContainer {
            pid,
            package_name: String::from(package_name),
            memory_quota_bytes: 64 * 1024 * 1024, // 64 MiB strict container quota
        });

        // Initialize app data directory
        let app_dir = format!("/data/data/{}", package_name);
        self.sandbox_fs.create_directory(&app_dir);

        pid
    }

    pub fn terminate_app(&mut self, pid: u32) -> bool {
        if let Some(pos) = self.containers.iter().position(|c| c.pid == pid) {
            self.containers.remove(pos);
            true
        } else {
            false
        }
    }
}
