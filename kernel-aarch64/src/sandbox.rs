//! Inter OS — AArch64 Sovereign Android Untrusted Compatibility Sandbox
//!
//! Enforces:
//! - Non-Negotiable Principle #9: Android applications are untrusted compatibility workloads
//!   and must never automatically access Inter OS sovereign private data, identity vaults,
//!   or raw microkernel IPC channels.
//! - Non-Negotiable Principle #11: Language and ecosystem choice does not determine trust.
//!   Strict capability tables and sandbox boundary policies determine trust.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use crate::capability::{CapabilityTable, CapabilityType, Rights};

// Linux / Android ARM64 Syscall Numbers
pub const ANDROID_SYS_IOCTL: u64 = 29;
pub const ANDROID_SYS_OPENAT: u64 = 56;
pub const ANDROID_SYS_CLOSE: u64 = 57;
pub const ANDROID_SYS_READ: u64 = 63;
pub const ANDROID_SYS_WRITE: u64 = 64;
pub const ANDROID_SYS_MMAP: u64 = 222;
pub const ANDROID_SYS_BINDER: u64 = 0xB1D0;

// POSIX / Bionic Error Codes
pub const EPERM: i64 = -1;
pub const ENOENT: i64 = -2;
pub const EBADF: i64 = -9;
pub const EACCES: i64 = -13;
pub const EINVAL: i64 = -22;

pub use interos_android_shared::{
    is_safe_android_path, AndroidFsError, AndroidSandboxFs, AndroidVirtualInode, BinderMediator,
    CapabilityChecker,
};

impl CapabilityChecker for CapabilityTable {
    fn has_android_guest_cap(&self) -> bool {
        self.check(CapabilityType::AndroidContainerGuest, Rights::INVOKE).is_ok()
    }
}

/// Sandboxed Android Application Container Instance.
#[derive(Debug, Clone)]
pub struct AndroidAppContainer {
    pub pid: u32,
    pub package_name: String,
    pub memory_quota_bytes: usize,
    pub cap_table: CapabilityTable,
}

/// Android Container Runtime Manager.
pub struct AndroidRuntime {
    pub next_pid: u32,
    pub active_apps: BTreeMap<u32, AndroidAppContainer>,
    pub sandbox_fs: AndroidSandboxFs,
    pub binder: BinderMediator,
}

impl AndroidRuntime {
    pub fn new() -> Self {
        Self {
            next_pid: 10001,
            active_apps: BTreeMap::new(),
            sandbox_fs: AndroidSandboxFs::new(),
            binder: BinderMediator::new(),
        }
    }

    /// Spawns a sandboxed Android guest container with zero ambient authority.
    pub fn spawn_app(&mut self, package_name: &str) -> u32 {
        let pid = self.next_pid;
        self.next_pid += 1;

        // Initialize sandboxed directories in virtual VFS
        let app_data_dir = format!("/data/data/{}", package_name);
        let app_sdcard_dir = format!("/sdcard/Android/data/{}", package_name);
        self.sandbox_fs.create_directory(&app_data_dir);
        self.sandbox_fs.create_directory(&app_sdcard_dir);

        // Zero ambient authority: empty capability table
        let mut caps = CapabilityTable::new();
        caps.mint(CapabilityType::AndroidContainerGuest, Rights::READ, None);

        self.active_apps.insert(
            pid,
            AndroidAppContainer {
                pid,
                package_name: String::from(package_name),
                memory_quota_bytes: 16 * 1024 * 1024, // 16 MiB container quota
                cap_table: caps,
            },
        );

        pid
    }

    /// Dispatches a Bionic system call through the untrusted sandbox gate.
    pub fn dispatch_syscall(&mut self, pid: u32, nr: u64, path: &str, data: &[u8]) -> i64 {
        let app = match self.active_apps.get(&pid) {
            Some(a) => a,
            None => return EPERM,
        };
        let pkg = app.package_name.clone();

        match nr {
            ANDROID_SYS_OPENAT => {
                match self.sandbox_fs.open_read(&pkg, path) {
                    Ok(bytes) => bytes.len() as i64,
                    Err(AndroidFsError::SecurityViolation) => EACCES,
                    Err(AndroidFsError::NotFound) => ENOENT,
                    Err(AndroidFsError::PermissionDenied) => EPERM,
                    Err(AndroidFsError::InvalidPath) => EINVAL,
                }
            }
            ANDROID_SYS_WRITE => {
                match self.sandbox_fs.open_write(&pkg, path, data) {
                    Ok(_) => data.len() as i64,
                    Err(AndroidFsError::SecurityViolation) => EACCES,
                    Err(_) => EPERM,
                }
            }
            ANDROID_SYS_CLOSE => 0,
            ANDROID_SYS_READ => data.len() as i64,
            ANDROID_SYS_MMAP => 0x0040_0000, // Sandboxed heap address
            _ => EINVAL,
        }
    }
}
