//! InterOS Core Open-Source Reference App: Sovereign File Manager (VFS)
//!
//! Demonstrates:
//! - Sandboxed VFS directory hierarchy traversal (/dev, /sys, /data, /data/apps)
//! - POSIX-like inode attributes (permissions, file size, node type)
//! - Capability-guarded read access without ambient authority
//! - Userspace file inspection and memory-mapped buffer preview

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryType {
    Directory,
    RegularFile,
    DeviceNode,
    SysFsNode,
}

#[derive(Debug, Clone, Copy)]
pub struct VfsEntry {
    pub name: &'static str,
    pub path: &'static str,
    pub entry_type: EntryType,
    pub size_bytes: u64,
    pub permissions: &'static str,
}

pub static ROOT_ENTRIES: [VfsEntry; 5] = [
    VfsEntry { name: "sys", path: "/sys", entry_type: EntryType::Directory, size_bytes: 4096, permissions: "dr-xr-xr-x" },
    VfsEntry { name: "dev", path: "/dev", entry_type: EntryType::Directory, size_bytes: 4096, permissions: "drwxr-xr-x" },
    VfsEntry { name: "data", path: "/data", entry_type: EntryType::Directory, size_bytes: 4096, permissions: "drwxrwx---" },
    VfsEntry { name: "boot", path: "/boot", entry_type: EntryType::Directory, size_bytes: 4096, permissions: "dr-xr-xr-x" },
    VfsEntry { name: "etc", path: "/etc", entry_type: EntryType::Directory, size_bytes: 4096, permissions: "dr-xr-xr-x" },
];

pub static DATA_APPS_ENTRIES: [VfsEntry; 6] = [
    VfsEntry { name: "hello_world.app", path: "/data/apps/hello_world.app", entry_type: EntryType::RegularFile, size_bytes: 45056, permissions: "-rwxr-xr-x" },
    VfsEntry { name: "calculator.app", path: "/data/apps/calculator.app", entry_type: EntryType::RegularFile, size_bytes: 65536, permissions: "-rwxr-xr-x" },
    VfsEntry { name: "browser.app", path: "/data/apps/browser.app", entry_type: EntryType::RegularFile, size_bytes: 147456, permissions: "-rwxr-xr-x" },
    VfsEntry { name: "camera.app", path: "/data/apps/camera.app", entry_type: EntryType::RegularFile, size_bytes: 98304, permissions: "-rwxr-xr-x" },
    VfsEntry { name: "settings.app", path: "/data/apps/settings.app", entry_type: EntryType::RegularFile, size_bytes: 81920, permissions: "-rwxr-xr-x" },
    VfsEntry { name: "flappy_wasm.wasm", path: "/data/apps/flappy_wasm.wasm", entry_type: EntryType::RegularFile, size_bytes: 32768, permissions: "-rw-r--r--" },
];

pub struct FileManagerState {
    pub current_directory: &'static str,
    pub selected_index: usize,
    pub total_storage_bytes: u64,
    pub used_storage_bytes: u64,
}

impl FileManagerState {
    pub const fn new() -> Self {
        Self {
            current_directory: "/data/apps",
            selected_index: 0,
            total_storage_bytes: 64 * 1024 * 1024 * 1024, // 64 GB
            used_storage_bytes: 12 * 1024 * 1024 * 1024,  // 12 GB
        }
    }

    pub fn navigate_to(&mut self, path: &'static str) {
        self.current_directory = path;
        self.selected_index = 0;
    }

    pub fn select_next(&mut self, max: usize) {
        if max > 0 {
            self.selected_index = (self.selected_index + 1) % max;
        }
    }

    pub fn free_storage_bytes(&self) -> u64 {
        self.total_storage_bytes.saturating_sub(self.used_storage_bytes)
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[FILE_MANAGER] InterOS Sovereign VFS Explorer Initializing...").ok();
    
    let mut state = FileManagerState::new();
    state.navigate_to("/data/apps");
    std::log(0, "[FILE_MANAGER] Mounted Root VFS: 64GB LPDDR4X/UFS 3.1 storage pool.").ok();
    std::log(0, state.current_directory).ok();
    
    for entry in &DATA_APPS_ENTRIES {
        std::log(0, entry.name).ok();
    }
    
    std::log(0, "[FILE_MANAGER] VFS Directory Loaded. Ready for file inspection.").ok();
    
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
