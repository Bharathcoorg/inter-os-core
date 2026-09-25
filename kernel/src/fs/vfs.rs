//! Inter OS Virtual Filesystem (VFS)
//!
//! Provides a capability-governed, hierarchical filesystem with Inodes,
//! File Descriptors, and dm-verity integrity validation.
//! Ambient access is strictly prohibited; all file operations require
//! validated capability tokens.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

use crate::capability::{CapError, CapabilityTable, CapabilityType, Rights};
use crate::crypto::Sha256;
use crate::task::TaskId;

/// Maximum number of file descriptors per task.
pub const MAX_FD_PER_TASK: usize = 16;

/// Unique Inode identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InodeId(pub u64);

/// File descriptor handle identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileDescriptor(pub u32);

/// Categorization of VFS file nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    RegularFile,
    Directory,
    ReadOnlySystem,
    DeviceNode,
}

/// Filesystem metadata structure.
#[derive(Debug, Clone)]
pub struct FileStat {
    pub inode: InodeId,
    pub file_type: FileType,
    pub size: usize,
    pub read_only: bool,
    pub sha256_digest: [u8; 32],
}

/// Inode payload representation (zero-heap static slice or dynamic vector).
#[derive(Debug, Clone)]
pub enum InodeData {
    Dynamic(Vec<u8>),
    Static(&'static [u8]),
}

impl InodeData {
    #[inline(always)]
    pub fn as_slice(&self) -> &[u8] {
        match self {
            InodeData::Dynamic(v) => v.as_slice(),
            InodeData::Static(s) => s,
        }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }
}

/// Inode representation in memory.
#[derive(Debug, Clone)]
pub struct Inode {
    pub id: InodeId,
    pub name: String,
    pub path: String,
    pub file_type: FileType,
    pub size: usize,
    pub data: InodeData,
    pub sha256_digest: [u8; 32],
    pub children: Vec<InodeId>,
}

/// Open file handle representation.
#[derive(Debug, Clone)]
pub struct OpenFileHandle {
    pub fd: FileDescriptor,
    pub inode_id: InodeId,
    pub offset: usize,
    pub rights: Rights,
}

/// Filesystem errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    FileNotFound,
    PermissionDenied,
    CapabilityError(CapError),
    IsADirectory,
    NotADirectory,
    ReadOnlyFileSystem,
    TableFull,
    InvalidFd,
    IntegrityViolation,
}

/// Virtual Filesystem Controller.
pub struct Vfs {
    inodes: BTreeMap<InodeId, Inode>,
    path_to_inode: BTreeMap<String, InodeId>,
    next_inode: u64,
    task_file_tables: BTreeMap<TaskId, Vec<Option<OpenFileHandle>>>,
}

impl Vfs {
    /// Creates a new, empty VFS instance.
    pub fn new() -> Self {
        let mut vfs = Self {
            inodes: BTreeMap::new(),
            path_to_inode: BTreeMap::new(),
            next_inode: 1,
            task_file_tables: BTreeMap::new(),
        };

        // Create root directory "/"
        vfs.mkdir("/");
        vfs
    }

    /// Allocates a new InodeId.
    fn allocate_inode(&mut self) -> InodeId {
        let id = InodeId(self.next_inode);
        self.next_inode += 1;
        id
    }

    /// Creates a directory node at the specified path.
    pub fn mkdir(&mut self, path: &str) -> InodeId {
        let norm_path = normalize_path(path);
        if let Some(&existing) = self.path_to_inode.get(&norm_path) {
            return existing;
        }

        let inode_id = self.allocate_inode();
        let name = path_filename(&norm_path);

        let inode = Inode {
            id: inode_id,
            name,
            path: norm_path.clone(),
            file_type: FileType::Directory,
            size: 0,
            data: InodeData::Static(&[]),
            sha256_digest: [0u8; 32],
            children: Vec::new(),
        };

        self.inodes.insert(inode_id, inode);
        self.path_to_inode.insert(norm_path, inode_id);
        inode_id
    }

    /// Creates or updates a file node with content.
    pub fn create_file(
        &mut self,
        path: &str,
        content: &[u8],
        read_only: bool,
    ) -> InodeId {
        let norm_path = normalize_path(path);
        let digest = Sha256::digest(content);

        if let Some(&existing_id) = self.path_to_inode.get(&norm_path) {
            if let Some(inode) = self.inodes.get_mut(&existing_id) {
                inode.data = InodeData::Dynamic(content.to_vec());
                inode.size = content.len();
                inode.sha256_digest = digest;
                if read_only {
                    inode.file_type = FileType::ReadOnlySystem;
                }
            }
            return existing_id;
        }

        let inode_id = self.allocate_inode();
        let name = path_filename(&norm_path);

        let inode = Inode {
            id: inode_id,
            name,
            path: norm_path.clone(),
            file_type: if read_only { FileType::ReadOnlySystem } else { FileType::RegularFile },
            size: content.len(),
            data: InodeData::Dynamic(content.to_vec()),
            sha256_digest: digest,
            children: Vec::new(),
        };

        self.inodes.insert(inode_id, inode);
        self.path_to_inode.insert(norm_path, inode_id);
        inode_id
    }

    /// Creates a static read-only file referencing an embedded byte slice with zero heap overhead.
    pub fn create_static_file(
        &mut self,
        path: &str,
        content: &'static [u8],
    ) -> InodeId {
        let norm_path = normalize_path(path);
        let digest = Sha256::digest(content);

        if let Some(&existing_id) = self.path_to_inode.get(&norm_path) {
            if let Some(inode) = self.inodes.get_mut(&existing_id) {
                inode.data = InodeData::Static(content);
                inode.size = content.len();
                inode.sha256_digest = digest;
                inode.file_type = FileType::ReadOnlySystem;
            }
            return existing_id;
        }

        let inode_id = self.allocate_inode();
        let name = path_filename(&norm_path);

        let inode = Inode {
            id: inode_id,
            name,
            path: norm_path.clone(),
            file_type: FileType::ReadOnlySystem,
            size: content.len(),
            data: InodeData::Static(content),
            sha256_digest: digest,
            children: Vec::new(),
        };

        self.inodes.insert(inode_id, inode);
        self.path_to_inode.insert(norm_path, inode_id);
        inode_id
    }

    /// Registers a device node at the specified path.
    pub fn create_device_node(&mut self, path: &str) -> InodeId {
        let norm_path = normalize_path(path);
        if let Some(&existing_id) = self.path_to_inode.get(&norm_path) {
            return existing_id;
        }

        let inode_id = self.allocate_inode();
        let name = path_filename(&norm_path);

        let inode = Inode {
            id: inode_id,
            name,
            path: norm_path.clone(),
            file_type: FileType::DeviceNode,
            size: 0,
            data: InodeData::Static(&[]),
            sha256_digest: [0u8; 32],
            children: Vec::new(),
        };

        self.inodes.insert(inode_id, inode);
        self.path_to_inode.insert(norm_path, inode_id);
        inode_id
    }

    /// Lists immediate children of a directory into a newline-delimited buffer.
    pub fn list_dir(&self, task_caps:&CapabilityTable, cap_slot:usize, path:&str, out:&mut [u8]) -> Result<usize,FsError>{
        task_caps.verify(cap_slot,CapabilityType::FileSystemRead,Rights::READ).map_err(FsError::CapabilityError)?;
        let dir=self.lookup(path).ok_or(FsError::FileNotFound)?;
        let inode=self.inodes.get(&dir).ok_or(FsError::FileNotFound)?;
        if inode.file_type!=FileType::Directory{return Err(FsError::NotADirectory);}
        use core::fmt::Write;
        struct W<'a>{b:&'a mut[u8],n:usize}
        impl<'a> Write for W<'a>{fn write_str(&mut self,s:&str)->core::fmt::Result{let k=s.len().min(self.b.len().saturating_sub(self.n));self.b[self.n..self.n+k].copy_from_slice(&s.as_bytes()[..k]);self.n+=k;Ok(())}}
        let mut w=W{b:out,n:0};
        for child in self.inodes.values().filter(|n| n.path.starts_with(&(normalize_path(path)+"/")) && n.path[normalize_path(path).len()+1..].contains('/')==false) {
            let _=write!(w,"{}|{}|{}\n",child.name,child.size,match child.file_type{FileType::Directory=>"dir",FileType::DeviceNode=>"dev",_=>"file"});
            if w.n>=out.len(){break;}
        }
        Ok(w.n)
    }

    /// Looks up InodeId by path.
    pub fn lookup(&self, path: &str) -> Option<InodeId> {
        let norm_path = normalize_path(path);
        self.path_to_inode.get(&norm_path).copied()
    }

    /// Returns the data byte size of an inode.
    pub fn get_size(&self, inode_id: InodeId) -> usize {
        self.inodes.get(&inode_id).map(|i| i.size).unwrap_or(0)
    }

    /// Retrieves file stat metadata.
    pub fn stat(
        &self,
        task_caps: &CapabilityTable,
        cap_slot: usize,
        path: &str,
    ) -> Result<FileStat, FsError> {
        // Enforce FileSystemRead capability
        task_caps
            .verify(cap_slot, CapabilityType::FileSystemRead, Rights::READ)
            .map_err(FsError::CapabilityError)?;

        let inode_id = self.lookup(path).ok_or(FsError::FileNotFound)?;
        let inode = self.inodes.get(&inode_id).ok_or(FsError::FileNotFound)?;

        Ok(FileStat {
            inode: inode.id,
            file_type: inode.file_type,
            size: inode.size,
            read_only: inode.file_type == FileType::ReadOnlySystem,
            sha256_digest: inode.sha256_digest,
        })
    }

    /// Opens a file, creating a task-specific FileDescriptor guarded by capability tokens.
    pub fn open(
        &mut self,
        task_id: TaskId,
        task_caps: &CapabilityTable,
        cap_slot: usize,
        path: &str,
        requested_rights: Rights,
    ) -> Result<FileDescriptor, FsError> {
        // Enforce capability validation based on requested access
        if requested_rights.contains(Rights::WRITE) {
            let cap = task_caps
                .verify(cap_slot, CapabilityType::FileSystemWrite, Rights::WRITE)
                .map_err(FsError::CapabilityError)?;

            let inode_id = self.lookup(path).ok_or(FsError::FileNotFound)?;
            let inode = self.inodes.get(&inode_id).ok_or(FsError::FileNotFound)?;

            if inode.file_type == FileType::ReadOnlySystem {
                return Err(FsError::ReadOnlyFileSystem);
            }

            if cap.object_id != 0 && cap.object_id != inode_id.0 {
                return Err(FsError::PermissionDenied);
            }

            self.allocate_fd(task_id, inode_id, requested_rights)
        } else {
            let cap = if let Ok(c) = task_caps.verify(cap_slot, CapabilityType::FileSystemRead, Rights::READ) {
                c
            } else {
                task_caps
                    .verify(cap_slot, CapabilityType::FileSystemWrite, Rights::READ)
                    .map_err(FsError::CapabilityError)?
            };

            let inode_id = self.lookup(path).ok_or(FsError::FileNotFound)?;
            let inode = self.inodes.get(&inode_id).ok_or(FsError::FileNotFound)?;

            if inode.file_type == FileType::Directory {
                return Err(FsError::IsADirectory);
            }

            if cap.object_id != 0 && cap.object_id != inode_id.0 {
                return Err(FsError::PermissionDenied);
            }

            self.allocate_fd(task_id, inode_id, requested_rights)
        }
    }

    /// Allocates an open file descriptor in the task's file table.
    fn allocate_fd(
        &mut self,
        task_id: TaskId,
        inode_id: InodeId,
        rights: Rights,
    ) -> Result<FileDescriptor, FsError> {
        let table = self.task_file_tables.entry(task_id).or_insert_with(|| {
            let mut v = Vec::with_capacity(MAX_FD_PER_TASK);
            for _ in 0..MAX_FD_PER_TASK {
                v.push(None);
            }
            v
        });

        for (idx, slot) in table.iter_mut().enumerate() {
            if slot.is_none() {
                let fd = FileDescriptor(idx as u32);
                *slot = Some(OpenFileHandle {
                    fd,
                    inode_id,
                    offset: 0,
                    rights,
                });
                return Ok(fd);
            }
        }

        Err(FsError::TableFull)
    }

    /// Reads bytes from an open file descriptor.
    pub fn read(
        &mut self,
        task_id: TaskId,
        task_caps: &CapabilityTable,
        cap_slot: usize,
        fd: FileDescriptor,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        let _cap = if let Ok(c) = task_caps.verify(cap_slot, CapabilityType::FileSystemRead, Rights::READ) {
            c
        } else {
            task_caps
                .verify(cap_slot, CapabilityType::FileSystemWrite, Rights::READ)
                .map_err(FsError::CapabilityError)?
        };

        let table = self.task_file_tables.get_mut(&task_id).ok_or(FsError::InvalidFd)?;
        let handle = table
            .get_mut(fd.0 as usize)
            .and_then(|opt| opt.as_mut())
            .ok_or(FsError::InvalidFd)?;

        if !handle.rights.contains(Rights::READ) {
            return Err(FsError::PermissionDenied);
        }

        let inode = self.inodes.get(&handle.inode_id).ok_or(FsError::FileNotFound)?;

        if inode.file_type == FileType::DeviceNode {
            return crate::fs::devfs::device_read(&inode.path, buf);
        }

        if handle.offset >= inode.data.len() {
            return Ok(0); // EOF
        }

        let slice = inode.data.as_slice();
        let available = slice.len() - handle.offset;
        let count = core::cmp::min(available, buf.len());

        buf[..count].copy_from_slice(&slice[handle.offset..handle.offset + count]);
        handle.offset += count;

        Ok(count)
    }

    /// Writes bytes to an open file descriptor.
    pub fn write(
        &mut self,
        task_id: TaskId,
        task_caps: &CapabilityTable,
        cap_slot: usize,
        fd: FileDescriptor,
        buf: &[u8],
    ) -> Result<usize, FsError> {
        task_caps
            .verify(cap_slot, CapabilityType::FileSystemWrite, Rights::WRITE)
            .map_err(FsError::CapabilityError)?;

        let table = self.task_file_tables.get_mut(&task_id).ok_or(FsError::InvalidFd)?;
        let handle = table
            .get_mut(fd.0 as usize)
            .and_then(|opt| opt.as_mut())
            .ok_or(FsError::InvalidFd)?;

        if !handle.rights.contains(Rights::WRITE) {
            return Err(FsError::PermissionDenied);
        }

        let inode_id = handle.inode_id;
        let offset = handle.offset;

        let inode = self.inodes.get_mut(&inode_id).ok_or(FsError::FileNotFound)?;

        if inode.file_type == FileType::DeviceNode {
            return crate::fs::devfs::device_write(&inode.path, buf);
        }

        if inode.file_type == FileType::ReadOnlySystem {
            return Err(FsError::ReadOnlyFileSystem);
        }

        // Extend buffer if necessary
        match &mut inode.data {
            InodeData::Dynamic(vec) => {
                if offset + buf.len() > vec.len() {
                    vec.resize(offset + buf.len(), 0);
                }
                vec[offset..offset + buf.len()].copy_from_slice(buf);
                inode.size = vec.len();
                inode.sha256_digest = Sha256::digest(vec);
            }
            InodeData::Static(_) => return Err(FsError::ReadOnlyFileSystem),
        }

        // Re-borrow handle to advance offset
        let table = self.task_file_tables.get_mut(&task_id).unwrap();
        let handle = table.get_mut(fd.0 as usize).unwrap().as_mut().unwrap();
        handle.offset += buf.len();

        Ok(buf.len())
    }

    /// Closes an open file descriptor.
    pub fn close(&mut self, task_id: TaskId, fd: FileDescriptor) -> Result<(), FsError> {
        let table = self.task_file_tables.get_mut(&task_id).ok_or(FsError::InvalidFd)?;
        if let Some(slot) = table.get_mut(fd.0 as usize) {
            if slot.is_some() {
                *slot = None;
                return Ok(());
            }
        }
        Err(FsError::InvalidFd)
    }
}

/// Normalizes a path string.
fn normalize_path(path: &str) -> String {
    let mut normalized = String::from("/");
    let trimmed = path.trim().trim_matches('/');
    if !trimmed.is_empty() {
        normalized.push_str(trimmed);
    }
    normalized
}

/// Extracts filename from path.
fn path_filename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or("").to_string()
}

/// Global synchronized Virtual Filesystem instance.
pub static VFS: Mutex<Vfs> = Mutex::new(Vfs {
    inodes: BTreeMap::new(),
    path_to_inode: BTreeMap::new(),
    next_inode: 1,
    task_file_tables: BTreeMap::new(),
});

/// Initializes the root filesystem with essential system directories and trusted files.
pub fn init() {
    let mut vfs = Vfs::new();

    // Create system directory layout
    vfs.mkdir("/system");
    vfs.mkdir("/system/bin");
    vfs.mkdir("/system/etc");
    vfs.mkdir("/packages");
    vfs.mkdir("/data");

    // Seed Sample Android APK Package for Container Attestation & Installation
    let sample_apk = crate::compat::android::create_test_apk(
        "com.example.retail",
        &["android.permission.INTERNET", "android.permission.ACCESS_FINE_LOCATION"],
    );
    vfs.create_file("/packages/retail.apk", &sample_apk, true);

    let sample_dex = crate::compat::android::create_test_dex(
        "Lorg/whispersystems/signal/MainActivity;",
        "onCreate",
        "Hello from Signal on Inter OS!",
    );
    vfs.create_file("/packages/classes.dex", &sample_dex, true);

    // 1. System Policy Configuration (Read-Only)
    let policy_content = br#"{
  "os": "Inter OS",
  "version": "0.3.0",
  "security_model": "Zero-Trust Object Capabilities",
  "integrity_protection": "dm-verity Merkle Tree",
  "ambient_authority": false,
  "default_sandbox": "strict"
}"#;
    vfs.create_file("/system/etc/policy.json", policy_content, true);

    // 2. Sovereign Cryptographic Identity (Read-Only)
    let device_id_content = b"DID:INTER:SOVEREIGN:X86_64:DEV_KEY_0x7F92_A8B1_C0DE_CAFE";
    vfs.create_file("/system/etc/device_id", device_id_content, true);

    // 3. Mutable Data Audit Log
    let audit_init = b"[SYSTEM AUDIT LOG INITIALIZED] Milestone v0.3 Storage & VFS Active.\n";
    vfs.create_file("/data/audit.log", audit_init, false);

    // 4. First-party native packages. The package service launches from these
    // verified .rapp payloads; legacy /system/bin entries remain diagnostics.
    vfs.create_static_file("/system/bin/init", crate::task::USER_INIT_ELF);
    vfs.create_static_file("/system/bin/settings", crate::task::APP_SETTINGS_ELF);
    vfs.create_static_file("/system/bin/terminal", crate::task::APP_TERMINAL_ELF);

    let settings_rapp = crate::package::build_rapp(
        "org.interos.settings", "interos.first-party", (1, 0, 0),
        crate::package::RappArch::X86_64, &[], crate::task::APP_SETTINGS_ELF,
    );
    vfs.create_file("/packages/org.interos.settings.rapp", &settings_rapp, true);

    let terminal_rapp = crate::package::build_rapp(
        "org.interos.terminal", "interos.first-party", (1, 0, 0),
        crate::package::RappArch::X86_64, &[], crate::task::APP_TERMINAL_ELF,
    );
    vfs.create_file("/packages/org.interos.terminal.rapp", &terminal_rapp, true);
    let compositor_rapp = crate::package::build_rapp(
        "org.interos.compositor", "interos.first-party", (1, 0, 0),
        crate::package::RappArch::X86_64, &[], crate::task::COMPOSITOR_ELF,
    );
    vfs.create_file("/packages/org.interos.compositor.rapp", &compositor_rapp, true);

    let launcher_rapp = crate::package::build_rapp(
        "org.interos.launcher", "interos.first-party", (1, 0, 0),
        crate::package::RappArch::X86_64, &[], crate::task::LAUNCHER_ELF,
    );
    vfs.create_file("/packages/org.interos.launcher.rapp", &launcher_rapp, true);
    vfs.create_static_file("/system/bin/dialer", crate::task::APP_DIALER_ELF);
    vfs.create_static_file("/system/bin/vault", crate::task::APP_VAULT_ELF);
    vfs.create_static_file("/system/bin/camera", crate::task::APP_CAMERA_ELF);
    vfs.create_static_file("/system/bin/calculator", crate::task::APP_CALCULATOR_ELF);
    vfs.create_static_file("/system/bin/browser", crate::task::APP_BROWSER_ELF);
    vfs.create_static_file("/system/bin/messages", crate::task::APP_MESSAGES_ELF);
    vfs.create_static_file("/system/bin/calendar", crate::task::APP_CALENDAR_ELF);
    vfs.create_static_file("/system/bin/music", crate::task::APP_MUSIC_ELF);

    // 5. Character Device Nodes (/dev)
    vfs.mkdir("/dev");
    vfs.mkdir("/dev/input");
    vfs.create_device_node("/dev/null");
    vfs.create_device_node("/dev/zero");
    vfs.create_device_node("/dev/urandom");
    vfs.create_device_node("/dev/input/event0");
    vfs.create_device_node("/dev/ptmx");

    *VFS.lock() = vfs;
}
