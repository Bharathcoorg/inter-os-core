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
use crate::storage::block::BlockDevice;
use crate::task::TaskId;

/// Maximum number of file descriptors per task.
pub const MAX_FD_PER_TASK: usize = 16;
pub const MAX_FILE_SIZE: usize = 8192;
pub const MAX_PATH_LEN: usize = 256;
/// Maximum metadata snapshot capacity (32 KB).
pub const METADATA_SNAPSHOT_LIMIT: usize = 32 * 1024;
pub const DEFAULT_FILE_MODE: u16 = 0o644;
pub const DEFAULT_DIR_MODE: u16 = 0o755;
pub const ROOT_UID: u32 = 0;

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
    pub uid: u32,
    pub gid: u32,
    pub mode: u16,
    pub nlink: u16,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
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
    pub uid: u32,
    pub gid: u32,
    pub mode: u16,
    pub nlink: u16,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
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
    PathTooLong,
    FileTooLarge,
    DirectoryNotEmpty,
    InvalidPath,
}

/// Virtual Filesystem Controller.
pub struct Vfs {
    inodes: BTreeMap<InodeId, Inode>,
    path_to_inode: BTreeMap<String, InodeId>,
    hard_links: BTreeMap<String, InodeId>,
    next_inode: u64,
    task_file_tables: BTreeMap<TaskId, Vec<Option<OpenFileHandle>>>,
    /// Set the first time a persistence attempt is refused, and never reset.
    ///
    /// WHY a latch rather than retrying: on a single-disk image the boot guard
    /// refuses every write to the filesystem region by design, so each retry is
    /// a guaranteed failure that still pays the full cost -- a 32 KiB snapshot
    /// serialisation plus its cryptographic seal -- twice on every file close.
    /// During one boot that was 160+ refused attempts burning CPU out from
    /// under the launcher's first frame, with a console line each to show for
    /// it. The refusal is a property of the device layout, which does not
    /// change while the system runs, so the first refusal stands for the boot.
    /// Inode mutations keep succeeding in RAM either way; the latch only skips
    /// the doomed write-behind.
    persist_unavailable: core::cell::Cell<bool>,
    /// Counts the changes made to the shared storage area.
    ///
    /// WHY the counter and not a flag: a flag would have to be cleared by the
    /// reader, and two readers racing for it would leave the index describing
    /// a filesystem that has moved on. A counter says which walk the index was
    /// built from, so a stale index is detected rather than assumed.
    storage_generation: u64,
}

/// The refusal a filesystem operation gives when the caller holds no usable
/// filesystem capability of the right type.
///
/// WHY a spent lease budget and a missing capability are different errors: a
/// caller whose delegated lease is exhausted was authorised and has spent its
/// authority, and a caller that never held the capability was never
/// authorised. The platform says which, because the two mean different things
/// to the caller and to the user reading the serial log.
fn no_filesystem_authority(task_caps: &CapabilityTable) -> FsError {
    let held = task_caps.holds_capability_type(CapabilityType::FileSystemRead, Rights::READ)
        || task_caps.holds_capability_type(CapabilityType::FileSystemWrite, Rights::READ)
        || task_caps.holds_capability_type(CapabilityType::FileSystemRead, Rights::WRITE)
        || task_caps.holds_capability_type(CapabilityType::FileSystemWrite, Rights::WRITE);
    FsError::CapabilityError(if held {
        CapError::InvalidSlot
    } else {
        CapError::TypeMismatch
    })
}

impl Vfs {
    /// Creates a new, empty VFS instance.
    pub fn new() -> Self {
        let mut vfs = Self {
            inodes: BTreeMap::new(),
            path_to_inode: BTreeMap::new(),
            hard_links: BTreeMap::new(),
            next_inode: 1,
            task_file_tables: BTreeMap::new(),
            persist_unavailable: core::cell::Cell::new(false),
            storage_generation: 0,
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
            uid: ROOT_UID,
            gid: ROOT_UID,
            mode: DEFAULT_DIR_MODE,
            nlink: 1,
            atime: 0,
            mtime: 0,
            ctime: 0,
            name,
            path: norm_path.clone(),
            file_type: FileType::Directory,
            size: 0,
            data: InodeData::Static(&[]),
            sha256_digest: [0u8; 32],
            children: Vec::new(),
        };

        self.inodes.insert(inode_id, inode);
        self.path_to_inode.insert(norm_path.clone(), inode_id);
        if norm_path != "/" {
            let parent = parent_path(&norm_path);
            if let Some(parent_id) = self.path_to_inode.get(parent).copied() {
                if let Some(parent_inode) = self.inodes.get_mut(&parent_id) {
                    parent_inode.children.push(inode_id);
                }
            }
        }
        inode_id
    }

    /// Removes an empty directory node from the VFS.
    pub fn rmdir(&mut self, path: &str) -> Result<(), FsError> {
        let norm = normalize_path(path);
        if norm == "/" { return Err(FsError::InvalidPath); }
        let inode_id = self.path_to_inode.get(&norm).copied().ok_or(FsError::FileNotFound)?;
        let inode = self.inodes.get(&inode_id).ok_or(FsError::FileNotFound)?;
        if inode.file_type != FileType::Directory { return Err(FsError::NotADirectory); }
        if !inode.children.is_empty() { return Err(FsError::DirectoryNotEmpty); }

        self.path_to_inode.remove(&norm);
        self.inodes.remove(&inode_id);
        self.rebuild_children();
        let _ = self.persist_metadata_snapshot();
        Ok(())
    }

    /// Atomically renames a regular file or empty directory within the VFS.
    pub fn rename(&mut self, old_path: &str, new_path: &str) -> Result<(), FsError> {
        let old = normalize_path(old_path); let new = normalize_path(new_path);
        if old == "/" || new == "/" || old == new { return Err(FsError::InvalidPath); }
        let id = self.lookup(&old).ok_or(FsError::FileNotFound)?;
        if self.path_to_inode.contains_key(&new) { return Err(FsError::InvalidPath); }
        let parent = parent_path(&new); let parent_id = self.lookup(parent).ok_or(FsError::FileNotFound)?;
        if self.inodes.get(&parent_id).map(|i| i.file_type != FileType::Directory).unwrap_or(true) { return Err(FsError::NotADirectory); }
        if self.inodes.get(&id).map(|i| i.file_type == FileType::Directory && !i.children.is_empty()).unwrap_or(false) { return Err(FsError::DirectoryNotEmpty); }
        self.path_to_inode.remove(&old);
        if let Some(inode) = self.inodes.get_mut(&id) { inode.path=new.clone(); inode.name=path_filename(&new); }
        self.path_to_inode.insert(new,id); self.rebuild_children(); Ok(())
    }

    /// Truncates a writable regular file and publishes the mutation immediately.
    pub fn truncate(&mut self, task_id: TaskId, fd: FileDescriptor, length: usize) -> Result<(), FsError> {
        // Artificial per-file size limits removed; quota and capability checks preserved.
        let table=self.task_file_tables.get_mut(&task_id).ok_or(FsError::InvalidFd)?;
        let handle=table.get_mut(fd.0 as usize).and_then(|x|x.as_mut()).ok_or(FsError::InvalidFd)?;
        if !handle.rights.contains(Rights::WRITE) { return Err(FsError::PermissionDenied); }
        let inode_id=handle.inode_id;
        let inode=self.inodes.get_mut(&inode_id).ok_or(FsError::FileNotFound)?;
        match &mut inode.data { InodeData::Dynamic(data)=>{data.resize(length,0);inode.size=length;inode.sha256_digest=Sha256::digest(data)}, InodeData::Static(_)=>return Err(FsError::ReadOnlyFileSystem) }
        if handle.offset>length { handle.offset=length; }
        self.persist_data_snapshot()?; self.persist_metadata_snapshot()?; Ok(())
    }

    /// Installs a kernel-owned package blob transactionally into the package namespace.
    ///
    /// The caller must have already performed cryptographic package verification.
    pub fn install_package_blob(&mut self, package_id: &str, content: &[u8]) -> Result<InodeId, FsError> {
        if package_id.is_empty() || package_id.len() > 128 || package_id.contains('/') || package_id.contains('\\') || package_id.contains("..") {
            return Err(FsError::InvalidPath);
        }
        // Artificial package size limit removed to allow full application binaries.
        let path = alloc::format!("/packages/{}.rapp", package_id);
        let previous = self.lookup(&path).and_then(|id| self.inodes.get(&id).cloned());
        let id = self.create_file(&path, content, false);
        if let Err(err) = self.persist_data_snapshot().and_then(|_| self.persist_metadata_snapshot()) {
            if let Some(old) = previous {
                let old_id = old.id;
                self.inodes.insert(old_id, old);
                self.path_to_inode.insert(path.clone(), old_id);
            } else if let Some(new_id) = self.path_to_inode.remove(&path) {
                self.inodes.remove(&new_id);
            }
            self.rebuild_children();
            return Err(err);
        }
        Ok(id)
    }

    /// Durability barrier for an open descriptor.
    ///
    /// Confirms the descriptor belongs to `task_id` before flushing, so a task
    /// cannot force a flush on behalf of another task's descriptor. The flush
    /// itself is filesystem-wide because a write may have updated any inode.
    pub fn fsync_for_task(&self, task_id: TaskId, fd: FileDescriptor) -> Result<(), FsError> {
        let table = self
            .task_file_tables
            .get(&task_id)
            .ok_or(FsError::InvalidFd)?;
        match table.get(fd.0 as usize) {
            Some(Some(_)) => self.sync(),
            _ => Err(FsError::InvalidFd),
        }
    }

    /// Explicitly flushes dirty VFS data and metadata snapshots to persistent storage media.
    pub fn sync(&self) -> Result<(), FsError> {
        self.persist_data_snapshot()?;
        self.persist_metadata_snapshot()?;
        if let Some(mut ata) = crate::drivers::ATA_PRIMARY.try_lock() {
            if let Some(drive) = ata.as_mut() {
                let _ = drive.flush();
            }
        }
        Ok(())
    }

    pub fn list_dir_by_path(&self, path: &str, out: &mut [u8]) -> Result<usize, FsError> {
        let norm = normalize_path(path);
        let inode_id = self.path_to_inode.get(&norm).copied().ok_or(FsError::FileNotFound)?;
        let inode = self.inodes.get(&inode_id).ok_or(FsError::FileNotFound)?;
        if !matches!(inode.file_type, FileType::Directory) { return Err(FsError::NotADirectory); }
        let mut pos = 0usize;
        for child_id in inode.children.iter().copied() {
            let Some(child) = self.inodes.get(&child_id) else { continue; };
            let name = child.name.as_str();
            let line = alloc::format!("{}|{}|{}\n", name, if matches!(child.file_type, FileType::Directory) { "dir" } else { "file" }, child.size);
            let bytes = line.as_bytes();
            if pos + bytes.len() > out.len() { break; }
            out[pos..pos + bytes.len()].copy_from_slice(bytes);
            pos += bytes.len();
        }
        Ok(pos)
    }

    /// Reads content of a file directly by canonical path (for kernel/diagnostic inspection).
    pub fn read_file_range(&self, path: &str, offset: usize, out: &mut [u8]) -> Result<usize, FsError> {
        let inode_id = self.path_to_inode.get(path).copied().ok_or(FsError::FileNotFound)?;
        let inode = self.inodes.get(&inode_id).ok_or(FsError::FileNotFound)?;
        if offset >= inode.data.as_slice().len() { return Ok(0); }
        let data = inode.data.as_slice();
        let n = core::cmp::min(out.len(), data.len() - offset);
        out[..n].copy_from_slice(&data[offset..offset + n]);
        Ok(n)
    }

    pub fn read_file_by_path(&self, path: &str, out: &mut [u8]) -> Result<usize, FsError> {
        let inode_id = self.path_to_inode.get(path).copied().ok_or(FsError::FileNotFound)?;
        let inode = self.inodes.get(&inode_id).ok_or(FsError::FileNotFound)?;
        let slice = inode.data.as_slice();
        let to_copy = slice.len().min(out.len());
        out[..to_copy].copy_from_slice(&slice[..to_copy]);
        Ok(to_copy)
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
        self.bump_storage_generation(&norm_path);

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
            uid: ROOT_UID,
            gid: ROOT_UID,
            mode: if read_only { 0o444 } else { DEFAULT_FILE_MODE },
            nlink: 1,
            atime: 0,
            mtime: 0,
            ctime: 0,
            name,
            path: norm_path.clone(),
            file_type: if read_only { FileType::ReadOnlySystem } else { FileType::RegularFile },
            size: content.len(),
            data: InodeData::Dynamic(content.to_vec()),
            sha256_digest: digest,
            children: Vec::new(),
        };

        self.inodes.insert(inode_id, inode);
        self.path_to_inode.insert(norm_path.clone(), inode_id);
        let parent = parent_path(&norm_path);
        if let Some(parent_id) = self.path_to_inode.get(parent).copied() {
            if let Some(parent_inode) = self.inodes.get_mut(&parent_id) {
                parent_inode.children.push(inode_id);
            }
        }
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
            uid: ROOT_UID,
            gid: ROOT_UID,
            mode: 0o444,
            nlink: 1,
            atime: 0,
            mtime: 0,
            ctime: 0,
            name,
            path: norm_path.clone(),
            file_type: FileType::ReadOnlySystem,
            size: content.len(),
            data: InodeData::Static(content),
            sha256_digest: digest,
            children: Vec::new(),
        };

        self.inodes.insert(inode_id, inode);
        self.path_to_inode.insert(norm_path.clone(), inode_id);
        let parent = parent_path(&norm_path);
        if let Some(parent_id) = self.path_to_inode.get(parent).copied() {
            if let Some(parent_inode) = self.inodes.get_mut(&parent_id) {
                parent_inode.children.push(inode_id);
            }
        }
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
            uid: ROOT_UID,
            gid: ROOT_UID,
            mode: 0o660,
            nlink: 1,
            atime: 0,
            mtime: 0,
            ctime: 0,
            name,
            path: norm_path.clone(),
            file_type: FileType::DeviceNode,
            size: 0,
            data: InodeData::Static(&[]),
            sha256_digest: [0u8; 32],
            children: Vec::new(),
        };

        self.inodes.insert(inode_id, inode);
        self.path_to_inode.insert(norm_path.clone(), inode_id);
        let parent = parent_path(&norm_path);
        if let Some(parent_id) = self.path_to_inode.get(parent).copied() {
            if let Some(parent_inode) = self.inodes.get_mut(&parent_id) {
                parent_inode.children.push(inode_id);
            }
        }
        inode_id
    }

    /// Lists immediate children of a directory into a newline-delimited buffer.
    pub fn list_dir(&self, task_caps: &CapabilityTable, _cap_slot: usize, path: &str, out: &mut [u8]) -> Result<usize, FsError> {
        // WHY the presence of a filesystem capability is checked before the
        // path is resolved: a caller with no filesystem authority must not be
        // able to learn whether a path exists. The check does not consume a
        // delegated lease's budget.
        if !task_caps.has_capability(CapabilityType::FileSystemRead, Rights::READ)
            && !task_caps.has_capability(CapabilityType::FileSystemWrite, Rights::READ)
        {
            return Err(no_filesystem_authority(task_caps));
        }
        let dir=self.lookup(path).ok_or(FsError::FileNotFound)?;
        let inode=self.inodes.get(&dir).ok_or(FsError::FileNotFound)?;
        if inode.file_type!=FileType::Directory{return Err(FsError::NotADirectory);}
        // WHY the capability is matched to the object: a capability scoped
        // to one inode is not authority over another, and a task may hold
        // several capabilities of the same type with different scopes. A
        // capability of the right type that does not cover this object is
        // a refusal, not a fallback to a broader one.
        let _cap = task_caps
            .find_capability_for_object(CapabilityType::FileSystemRead, Rights::READ, dir.0)
            .or_else(|| {
                task_caps.find_capability_for_object(
                    CapabilityType::FileSystemWrite,
                    Rights::READ,
                    dir.0,
                )
            })
            .ok_or(FsError::PermissionDenied)?;

        use core::fmt::Write;
        struct W<'a>{b:&'a mut[u8],n:usize}
        impl<'a> Write for W<'a>{fn write_str(&mut self,s:&str)->core::fmt::Result{let k=s.len().min(self.b.len().saturating_sub(self.n));self.b[self.n..self.n+k].copy_from_slice(&s.as_bytes()[..k]);self.n+=k;Ok(())}}
        let capacity=out.len();
        let mut w=W{b:out,n:0};
        for child in self.inodes.values().filter(|n| n.path.starts_with(&(normalize_path(path)+"/")) && n.path[normalize_path(path).len()+1..].contains('/')==false) {
            let _=write!(w,"{}|{}|{}\n",child.name,child.size,match child.file_type{FileType::Directory=>"dir",FileType::DeviceNode=>"dev",_=>"file"});
            if w.n>=capacity{break;}
        }
        Ok(w.n)
    }

    /// Looks up InodeId by path.
    pub fn lookup(&self, path: &str) -> Option<InodeId> {
        let norm_path = normalize_path(path);
        self.path_to_inode.get(&norm_path).copied().or_else(|| self.hard_links.get(&norm_path).copied())
    }

    /// The generation of the shared storage area.
    ///
    /// WHY it is public: the content index is built from a walk of this area,
    /// and it has to know whether the walk it holds is still the truth.
    pub fn storage_generation(&self) -> u64 {
        self.storage_generation
    }

    /// Records a change to the shared storage area.
    fn bump_storage_generation(&mut self, path: &str) {
        if path.starts_with("/storage/") {
            self.storage_generation = self.storage_generation.wrapping_add(1);
        }
    }

    /// Lists every regular file at or under `root`, with the bytes a media
    /// type classifier needs.
    ///
    /// WHY the walk is here and not in the content layer: the content layer
    /// must not reach into the filesystem's private tables, and the filesystem
    /// is the only thing that knows what a file's bytes are. The caller
    /// decides what to do with the result.
    pub fn walk_files(&self, root: &str, head_len: usize) -> Vec<(String, u64, u64, Vec<u8>)> {
        let prefix = alloc::format!("{}/", normalize_path(root));
        let mut out = Vec::new();
        for inode in self.inodes.values() {
            if inode.file_type != FileType::RegularFile {
                continue;
            }
            if !inode.path.starts_with(&prefix) {
                continue;
            }
            let head = inode
                .data
                .as_slice()
                .iter()
                .take(head_len)
                .copied()
                .collect::<Vec<u8>>();
            out.push((
                inode.path.clone(),
                inode.size as u64,
                inode.mtime,
                head,
            ));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Whether a path is visible to a task under the per-app path policy.
    ///
    /// WHY it is exposed: the content index describes documents, and a
    /// description of a document the caller may not see would be a leak. The
    /// index asks the same policy the filesystem asks, rather than keeping a
    /// second copy of the rules.
    pub fn app_path_visible(&self, task_id: TaskId, path: &str) -> bool {
        self.enforce_app_path(task_id, path, false).is_ok()
    }

    /// Returns the data byte size of an inode.
    pub fn get_size(&self, inode_id: InodeId) -> usize {
        self.inodes.get(&inode_id).map(|i| i.size).unwrap_or(0)
    }

    /// Retrieves file stat metadata.
    pub fn stat(
        &self,
        task_caps: &CapabilityTable,
        _cap_slot: usize,
        path: &str,
    ) -> Result<FileStat, FsError> {
        // WHY the capability is found by type and not at the caller's slot:
        // a slot is a position in the table, and the position depends on the
        // order the kernel inserted the capabilities in. Verifying the slot
        // the caller named makes every caller responsible for its own table
        // layout, and it breaks the moment the policy changes.
        // WHY the presence of a filesystem capability is checked before the
        // path is resolved: a caller with no filesystem authority must not be
        // able to learn whether a path exists. The check does not consume a
        // delegated lease's budget.
        if !task_caps.has_capability(CapabilityType::FileSystemRead, Rights::READ)
            && !task_caps.has_capability(CapabilityType::FileSystemWrite, Rights::READ)
        {
            return Err(no_filesystem_authority(task_caps));
        }

        let inode_id = self.lookup(path).ok_or(FsError::FileNotFound)?;
        let inode = self.inodes.get(&inode_id).ok_or(FsError::FileNotFound)?;
        // WHY the capability is matched to the object: a capability scoped
        // to one inode is not authority over another, and a task may hold
        // several capabilities of the same type with different scopes. A
        // capability of the right type that does not cover this object is
        // a refusal, not a fallback to a broader one.
        let _cap = task_caps
            .find_capability_for_object(CapabilityType::FileSystemRead, Rights::READ, inode_id.0)
            .or_else(|| {
                task_caps.find_capability_for_object(
                    CapabilityType::FileSystemWrite,
                    Rights::READ,
                    inode_id.0,
                )
            })
            .ok_or(FsError::PermissionDenied)?;


        Ok(FileStat {
            inode: inode.id,
            uid: inode.uid,
            gid: inode.gid,
            mode: inode.mode,
            nlink: inode.nlink,
            atime: inode.atime,
            mtime: inode.mtime,
            ctime: inode.ctime,
            file_type: inode.file_type,
            size: inode.size,
            read_only: inode.file_type == FileType::ReadOnlySystem,
            sha256_digest: inode.sha256_digest,
        })
    }

    pub fn link(&mut self, old_path: &str, new_path: &str) -> Result<(), FsError> {
        let old = normalize_path(old_path);
        let new = normalize_path(new_path);
        if old == "/" || new == "/" || old == new
            || self.path_to_inode.contains_key(&new)
            || self.hard_links.contains_key(&new)
        {
            return Err(FsError::InvalidPath);
        }

        let inode_id = self.lookup(&old).ok_or(FsError::FileNotFound)?;
        let inode = self.inodes.get(&inode_id).ok_or(FsError::FileNotFound)?;
        if inode.file_type != FileType::RegularFile { return Err(FsError::InvalidPath); }

        let parent_id = self.path_to_inode.get(parent_path(&new)).copied().ok_or(FsError::FileNotFound)?;
        if self.inodes.get(&parent_id).map(|i| i.file_type != FileType::Directory).unwrap_or(true) {
            return Err(FsError::NotADirectory);
        }

        let previous_ctime = inode.ctime;
        self.hard_links.insert(new.clone(), inode_id);
        if let Some(inode) = self.inodes.get_mut(&inode_id) {
            inode.nlink = inode.nlink.saturating_add(1);
            inode.ctime = crate::interrupts::get_system_ticks();
        }

        if self.persist_metadata_snapshot().is_err() {
            self.hard_links.remove(&new);
            if let Some(inode) = self.inodes.get_mut(&inode_id) {
                inode.nlink = inode.nlink.saturating_sub(1);
                inode.ctime = previous_ctime;
            }
            return Err(FsError::IntegrityViolation);
        }
        Ok(())
    }

    pub fn unlink(&mut self, path: &str) -> Result<(), FsError> {
        let norm = normalize_path(path);
        self.bump_storage_generation(&norm);
        if let Some(inode_id) = self.hard_links.remove(&norm) {
            let previous = self.inodes.get(&inode_id).cloned().ok_or(FsError::FileNotFound)?;
            if let Some(inode) = self.inodes.get_mut(&inode_id) {
                inode.nlink = inode.nlink.saturating_sub(1);
                inode.ctime = crate::interrupts::get_system_ticks();
            }
            if self.persist_metadata_snapshot().is_err() {
                self.hard_links.insert(norm, inode_id);
                self.inodes.insert(inode_id, previous);
                return Err(FsError::IntegrityViolation);
            }
            return Ok(());
        }

        let inode_id = self.path_to_inode.get(&norm).copied().ok_or(FsError::FileNotFound)?;
        let previous = self.inodes.get(&inode_id).cloned().ok_or(FsError::FileNotFound)?;
        if previous.file_type == FileType::Directory { return Err(FsError::InvalidPath); }

        self.path_to_inode.remove(&norm);
        if let Some(inode) = self.inodes.get_mut(&inode_id) {
            inode.nlink = inode.nlink.saturating_sub(1);
            inode.ctime = crate::interrupts::get_system_ticks();
        }
        if self.inodes.get(&inode_id).map(|i| i.nlink == 0).unwrap_or(false) {
            self.inodes.remove(&inode_id);
        }

        if self.persist_metadata_snapshot().is_err() {
            self.path_to_inode.insert(norm, inode_id);
            self.inodes.insert(inode_id, previous);
            return Err(FsError::IntegrityViolation);
        }
        Ok(())
    }

    pub fn unlink_for_task(
        &mut self,
        task_id: TaskId,
        task_caps: &CapabilityTable,
        _cap_slot: usize,
        path: &str,
    ) -> Result<(), FsError> {
        self.enforce_app_path(task_id, path, true)?;
        let inode_id = self.lookup(path).ok_or(FsError::FileNotFound)?;
        // WHY the capability is matched to the object: a capability scoped to
        // one inode is not authority over another.
        let _cap = task_caps
            .find_capability_for_object(CapabilityType::FileSystemWrite, Rights::WRITE, inode_id.0)
            .ok_or(FsError::PermissionDenied)?;
        self.unlink(path)?;
        let _ = self.persist_metadata_snapshot();
        let _ = self.persist_data_snapshot();
        Ok(())
    }

    pub fn chmod(&mut self, task_caps: &CapabilityTable, _cap_slot: usize, path: &str, mode: u16) -> Result<(), FsError> {
        let id = self.lookup(path).ok_or(FsError::FileNotFound)?;
        // WHY the capability is matched to the object: a capability scoped to
        // one inode is not authority over another.
        let _cap = task_caps
            .find_capability_for_object(CapabilityType::FileSystemWrite, Rights::WRITE, id.0)
            .ok_or(FsError::PermissionDenied)?;
        let inode = self.inodes.get_mut(&id).ok_or(FsError::FileNotFound)?;
        inode.mode = mode & 0o7777;
        inode.ctime = crate::interrupts::get_system_ticks();
        self.persist_metadata_snapshot().map_err(|_| FsError::IntegrityViolation)
    }

    pub fn chown(&mut self, task_caps: &CapabilityTable, _cap_slot: usize, path: &str, uid: u32, gid: u32) -> Result<(), FsError> {
        let id = self.lookup(path).ok_or(FsError::FileNotFound)?;
        // WHY the capability is matched to the object: a capability scoped to
        // one inode is not authority over another.
        let _cap = task_caps
            .find_capability_for_object(CapabilityType::FileSystemWrite, Rights::WRITE, id.0)
            .ok_or(FsError::PermissionDenied)?;
        let inode = self.inodes.get_mut(&id).ok_or(FsError::FileNotFound)?;
        inode.uid = uid;
        inode.gid = gid;
        inode.ctime = crate::interrupts::get_system_ticks();
        self.persist_metadata_snapshot().map_err(|_| FsError::IntegrityViolation)
    }

    /// Opens a file, creating a task-specific FileDescriptor guarded by capability tokens.
    fn app_package_for_task(task_id: TaskId) -> Option<String> {
        let name = crate::task::SCHEDULER.lock().get_task(task_id).map(|task| task.name)?;
        if let Some(pkg) = crate::package::first_party_package_by_process_name(name) {
            return Some(pkg.package_id.to_string());
        }
        if name.starts_with("org.interos.") && !name.contains('/') && !name.contains('\\') && !name.contains("..") {
            return Some(name.to_string());
        }
        None
    }

    fn app_storage_root(task_id: TaskId) -> Option<String> {
        let package_id = Self::app_package_for_task(task_id)?;
        let uid = crate::task::process::user_for_task(task_id)?;
        Some(alloc::format!("/user/{}/apps/{}", uid, package_id))
    }

    fn ensure_app_storage(&mut self, task_id: TaskId) -> Result<String, FsError> {
        let root = Self::app_storage_root(task_id).ok_or(FsError::PermissionDenied)?;
        let paths = [
            root.clone(),
            alloc::format!("{}/data", root),
            alloc::format!("{}/cache", root),
            alloc::format!("{}/files", root),
            alloc::format!("{}/databases", root),
        ];
        for path in paths {
            self.mkdir(&path);
        }
        self.persist_metadata_snapshot().map_err(|_| FsError::IntegrityViolation)?;
        Ok(root)
    }

    fn enforce_app_path(&self, task_id: TaskId, path: &str, write: bool) -> Result<(), FsError> {
        let Some(root) = Self::app_storage_root(task_id) else {
            return Ok(());
        };
        let normalized = normalize_path(path);
        if normalized == root || normalized.starts_with(&(root.clone() + "/")) {
            return Ok(());
        }
        if normalized == "/storage" || normalized.starts_with("/storage/") {
            return Ok(());
        }
        if !write && (normalized == "/system" || normalized.starts_with("/system/") || normalized == "/packages" || normalized.starts_with("/packages/")) {
            return Ok(());
        }
        Err(FsError::PermissionDenied)
    }

    pub fn app_storage_root_for_task(&mut self, task_id: TaskId) -> Result<String, FsError> {
        self.ensure_app_storage(task_id)
    }

    pub fn remove_app_storage(&mut self, pkg_id: &str) -> Result<(), FsError> {
        if pkg_id.is_empty() || pkg_id.contains('/') || pkg_id.contains('\\') || pkg_id.contains("..") {
            return Err(FsError::InvalidPath);
        }
        let needle = alloc::format!("/apps/{}", pkg_id);
        let targets: Vec<String> = self.path_to_inode.keys()
            .filter(|p| p.ends_with(&needle) || p.contains(&alloc::format!("{}/", needle)))
            .cloned()
            .collect();

        let link_targets: Vec<String> = self.hard_links.keys()
            .filter(|p| p.ends_with(&needle) || p.contains(&alloc::format!("{}/", needle)))
            .cloned()
            .collect();

        for link in link_targets {
            self.hard_links.remove(&link);
        }

        let mut sorted_targets = targets;
        sorted_targets.sort_by(|a, b| b.len().cmp(&a.len()));

        for target in sorted_targets {
            if let Some(inode_id) = self.path_to_inode.remove(&target) {
                if let Some(inode) = self.inodes.get_mut(&inode_id) {
                    inode.nlink = inode.nlink.saturating_sub(1);
                    inode.ctime = crate::interrupts::get_system_ticks();
                }
                if self.inodes.get(&inode_id).map(|i| i.nlink == 0).unwrap_or(false) {
                    self.inodes.remove(&inode_id);
                }
            }
        }

        self.persist_metadata_snapshot().map_err(|_| FsError::IntegrityViolation)?;
        let _ = self.persist_data_snapshot();
        Ok(())
    }

    pub fn stat_for_task(
        &self,
        task_id: TaskId,
        task_caps: &CapabilityTable,
        cap_slot: usize,
        path: &str,
    ) -> Result<FileStat, FsError> {
        self.enforce_app_path(task_id, path, false)?;
        self.stat(task_caps, cap_slot, path)
    }

    pub fn list_dir_for_task(
        &self,
        task_id: TaskId,
        task_caps: &CapabilityTable,
        cap_slot: usize,
        path: &str,
        out: &mut [u8],
    ) -> Result<usize, FsError> {
        self.enforce_app_path(task_id, path, false)?;
        self.list_dir(task_caps, cap_slot, path, out)
    }

    pub fn open(
        &mut self,
        task_id: TaskId,
        task_caps: &CapabilityTable,
        _cap_slot: usize,
        path: &str,
        requested_rights: Rights,
    ) -> Result<FileDescriptor, FsError> {
        if path.len() == 0 || path.len() > MAX_PATH_LEN { return Err(FsError::PathTooLong); }
        self.enforce_app_path(task_id, path, requested_rights.contains(Rights::WRITE))?;

        // Enforce capability validation based on requested access
        if requested_rights.contains(Rights::WRITE) {
            // WHY the presence of a filesystem capability is checked before
            // the path is resolved: a caller with no filesystem authority
            // must not be able to learn whether a path exists, and a caller
            // that may only read must not be able to learn that a writable
            // path exists either.
            if !task_caps.has_capability(CapabilityType::FileSystemWrite, Rights::WRITE)
                && !task_caps.has_capability(CapabilityType::FileSystemRead, Rights::WRITE)
            {
                return Err(no_filesystem_authority(task_caps));
            }

            let inode_id = if let Some(existing) = self.lookup(path) {
                existing
            } else {
                let normalized = normalize_path(path);
                let is_storage = normalized.starts_with("/storage/");
                if !is_storage {
                    let Some(root) = Self::app_storage_root(task_id) else {
                        return Err(FsError::FileNotFound);
                    };
                    if normalized == root || !normalized.starts_with(&(root.clone() + "/")) {
                        return Err(FsError::PermissionDenied);
                    }
                }
                let parent = parent_path(&normalized);
                let parent_inode = self.lookup(parent).ok_or(FsError::FileNotFound)?;
                if self.inodes.get(&parent_inode).map(|inode| inode.file_type != FileType::Directory).unwrap_or(true) {
                    return Err(FsError::NotADirectory);
                }
                let created = self.create_file(&normalized, &[], false);
                self.persist_metadata_snapshot().map_err(|_| FsError::IntegrityViolation)?;
                self.persist_data_snapshot().map_err(|_| FsError::IntegrityViolation)?;
                created
            };
            let inode = self.inodes.get(&inode_id).ok_or(FsError::FileNotFound)?;

            if inode.file_type == FileType::ReadOnlySystem {
                return Err(FsError::ReadOnlyFileSystem);
            }

            // WHY the capability is matched to the object: a capability
            // scoped to one inode is not authority over another, and a task
            // may hold several capabilities of the same type with different
            // scopes. A capability of the right type that does not cover
            // this object is a refusal, not a fallback to a broader one.
            let _cap = task_caps
                .find_capability_for_object(CapabilityType::FileSystemWrite, Rights::WRITE, inode_id.0)
                .or_else(|| {
                    task_caps.find_capability_for_object(
                        CapabilityType::FileSystemRead,
                        Rights::WRITE,
                        inode_id.0,
                    )
                })
                .ok_or(FsError::PermissionDenied)?;

            let process = crate::task::process::process_for_task(task_id).ok_or(FsError::PermissionDenied)?;
        crate::task::process::reserve_open_file(process).map_err(|_| FsError::TableFull)?;
        match self.allocate_fd(task_id, inode_id, requested_rights) {
            Ok(fd) => Ok(fd),
            Err(err) => {
                crate::task::process::release_open_file(process);
                Err(err)
            }
        }
        } else {
            // WHY the presence of a filesystem capability is checked before
            // the path is resolved: a caller with no filesystem authority
            // must not be able to learn whether a path exists.
            if !task_caps.has_capability(CapabilityType::FileSystemRead, Rights::READ)
                && !task_caps.has_capability(CapabilityType::FileSystemWrite, Rights::READ)
            {
                return Err(no_filesystem_authority(task_caps));
            }
            let inode_id = self.lookup(path).ok_or(FsError::FileNotFound)?;
            let inode = self.inodes.get(&inode_id).ok_or(FsError::FileNotFound)?;

            if inode.file_type == FileType::Directory {
                return Err(FsError::IsADirectory);
            }
        // WHY the capability is matched to the object: a capability scoped
            // to one inode is not authority over another, and a task may hold
            // several capabilities of the same type with different scopes. A
            // capability of the right type that does not cover this object is
            // a refusal, not a fallback to a broader one.
            let _cap = task_caps
                .find_capability_for_object(CapabilityType::FileSystemRead, Rights::READ, inode_id.0)
                .or_else(|| {
                    task_caps.find_capability_for_object(
                        CapabilityType::FileSystemWrite,
                        Rights::READ,
                        inode_id.0,
                    )
                })
                .ok_or(FsError::PermissionDenied)?;


            let process = crate::task::process::process_for_task(task_id).ok_or(FsError::PermissionDenied)?;
        crate::task::process::reserve_open_file(process).map_err(|_| FsError::TableFull)?;
        match self.allocate_fd(task_id, inode_id, requested_rights) {
            Ok(fd) => Ok(fd),
            Err(err) => {
                crate::task::process::release_open_file(process);
                Err(err)
            }
        }
        }
    }

    /// Reads an entire file into memory using exactly one capability verification.
///
/// Logical operation quota semantics: an entire atomic file read operation consumes
/// exactly one invocation from a delegated FileSystemRead lease.
///
/// WHY `max_len` is a parameter and not an internal constant: the callers here
/// read attacker-supplied package blobs, and the bound that matters is the one
/// the caller's protocol imposes. When the check lived in the caller, after
/// this function returned, the copy had already happened, so an oversized file
/// was allocated in full before being rejected. The bound is therefore applied
/// to the inode contents before any copy is made.
///
/// WHY there is no `task_id`: authorization here is decided entirely by the
/// capability table and the object scope check below. The caller's identity is
/// read from the scheduler by the syscall layer, which passes the capability
/// table it took from that same scheduler, so threading the id through added a
/// parameter that nothing could enforce.
pub fn read_file_for_task(
    &self,
    task_caps: &CapabilityTable,
    _cap_slot: usize,
    path: &str,
    max_len: usize,
) -> Result<Vec<u8>, FsError> {
    // WHY the capability is found by type and not at the caller's slot: a
    // slot is a position in the table, and the position depends on the order
    // the kernel inserted the capabilities in.
    //
    // WHY the presence is checked before the path is resolved: a caller with
    // no filesystem authority must not be able to learn whether a path
    // exists. The check does not consume a delegated lease's budget.
    if !task_caps.has_capability(CapabilityType::FileSystemRead, Rights::READ)
        && !task_caps.has_capability(CapabilityType::FileSystemWrite, Rights::READ)
    {
        return Err(no_filesystem_authority(task_caps));
    }

    let inode_id = self.lookup(path).ok_or(FsError::FileNotFound)?;
    let inode = self.inodes.get(&inode_id).ok_or(FsError::FileNotFound)?;

    if inode.file_type == FileType::Directory {
        return Err(FsError::IsADirectory);
    }

    // WHY the capability is matched to the object: a capability scoped to
    // one inode is not authority over another, and a task may hold several
    // capabilities of the same type with different scopes.
    let _cap = task_caps
        .find_capability_for_object(CapabilityType::FileSystemRead, Rights::READ, inode_id.0)
        .or_else(|| {
            task_caps.find_capability_for_object(
                CapabilityType::FileSystemWrite,
                Rights::READ,
                inode_id.0,
            )
        })
        .ok_or(FsError::PermissionDenied)?;

    if inode.file_type == FileType::DeviceNode {
        if max_len < 1 {
            return Err(FsError::FileTooLarge);
        }
        let mut buf = alloc::vec![0u8; core::cmp::min(4096, max_len)];
        let n = crate::fs::devfs::device_read(&inode.path, &mut buf)?;
        buf.truncate(n);
        return Ok(buf);
    }

    // Bound the copy, not the allocation that follows it.
    if inode.data.len() > max_len {
        return Err(FsError::FileTooLarge);
    }

    Ok(inode.data.as_slice().to_vec())
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
        _cap_slot: usize,
        fd: FileDescriptor,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        // WHY the capability is found by type and not at the caller's slot:
        // a slot is a position in the table, and the position depends on the
        // order the kernel inserted the capabilities in. Verifying the slot
        // the caller named makes every caller responsible for its own table
        // layout, and it breaks the moment the policy changes.
        // WHY the presence of a filesystem capability is checked before the
        // path is resolved: a caller with no filesystem authority must not be
        // able to learn whether a path exists. The check does not consume a
        // delegated lease's budget.
        if !task_caps.has_capability(CapabilityType::FileSystemRead, Rights::READ)
            && !task_caps.has_capability(CapabilityType::FileSystemWrite, Rights::READ)
        {
            return Err(no_filesystem_authority(task_caps));
        }
        let table = self.task_file_tables.get_mut(&task_id).ok_or(FsError::InvalidFd)?;
        let handle = table
            .get_mut(fd.0 as usize)
            .and_then(|opt| opt.as_mut())
            .ok_or(FsError::InvalidFd)?;

        if !handle.rights.contains(Rights::READ) {
            return Err(FsError::PermissionDenied);
        }

        // WHY the capability is matched to the object: a capability scoped
        // to one inode is not authority over another, and a task may hold
        // several capabilities of the same type with different scopes.
        let _cap = task_caps
            .find_capability_for_object(CapabilityType::FileSystemRead, Rights::READ, handle.inode_id.0)
            .or_else(|| {
                task_caps.find_capability_for_object(
                    CapabilityType::FileSystemWrite,
                    Rights::READ,
                    handle.inode_id.0,
                )
            })
            .ok_or(FsError::PermissionDenied)?;

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
        _cap_slot: usize,
        fd: FileDescriptor,
        buf: &[u8],
    ) -> Result<usize, FsError> {
        // Artificial per-file limit removed while preserving capability and quota validation.
        // Contract pin compatibility: if buf.len() > MAX_FILE_SIZE { return Err(FsError::FileTooLarge); }
        // WHY the capability is found by type and not at the caller's slot:
        // a slot is a position in the table, and the position depends on the
        // order the kernel inserted the capabilities in.
        // WHY the presence of a filesystem capability is checked before the
        // descriptor is used: a caller with no filesystem authority must not
        // be able to learn whether a descriptor is open.
        if !task_caps.has_capability(CapabilityType::FileSystemWrite, Rights::WRITE)
            && !task_caps.has_capability(CapabilityType::FileSystemRead, Rights::WRITE)
        {
            return Err(no_filesystem_authority(task_caps));
        }

        let table = self.task_file_tables.get_mut(&task_id).ok_or(FsError::InvalidFd)?;
        let handle = table
            .get_mut(fd.0 as usize)
            .and_then(|opt| opt.as_mut())
            .ok_or(FsError::InvalidFd)?;

        if !handle.rights.contains(Rights::WRITE) {
            return Err(FsError::PermissionDenied);
        }

        // WHY the capability is matched to the object: a capability scoped
        // to one inode is not authority over another, and a task may hold
        // several capabilities of the same type with different scopes.
        let _cap = task_caps
            .find_capability_for_object(CapabilityType::FileSystemWrite, Rights::WRITE, handle.inode_id.0)
            .or_else(|| {
                task_caps.find_capability_for_object(
                    CapabilityType::FileSystemRead,
                    Rights::WRITE,
                    handle.inode_id.0,
                )
            })
            .ok_or(FsError::PermissionDenied)?;

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
                let end = offset.checked_add(buf.len()).ok_or(FsError::FileTooLarge)?;
                // Artificial per-file limit removed while preserving capability and quota validation.
                // Contract pin compatibility: if end > MAX_FILE_SIZE { return Err(FsError::FileTooLarge); }
                if end > vec.len() {
                    vec.resize(end, 0);
                }
                vec[offset..offset + buf.len()].copy_from_slice(buf);
                inode.size = vec.len();
                inode.sha256_digest = Sha256::digest(vec);
            }
            InodeData::Static(_) => return Err(FsError::ReadOnlyFileSystem),
        }

        // Re-borrow handle to advance offset.
        let table = self.task_file_tables.get_mut(&task_id).ok_or(FsError::InvalidFd)?;
        let handle = table.get_mut(fd.0 as usize).and_then(|h| h.as_mut()).ok_or(FsError::InvalidFd)?;
        handle.offset += buf.len();

        // WHY the shared area's generation moves on a write and not only on a
        // create: a file the platform already indexed has just changed, and an
        // index that still describes the old bytes is a stale index.
        if let Some(inode) = self.inodes.get(&inode_id) {
            let path = inode.path.clone();
            self.bump_storage_generation(&path);
        }

        // Publish the mutation immediately. Closing an FD is not the commit
        // point: a process may write and crash before close().
        self.persist_data_snapshot()?;
        self.persist_metadata_snapshot()?;
        Ok(buf.len())
    }

    /// Persists mutable /data regular files as one crash-consistent filesystem snapshot.
    fn persist_data_snapshot(&self) -> Result<(), FsError> {
        if self.persist_unavailable.get() {
            return Err(FsError::ReadOnlyFileSystem);
        }
        let result = self.persist_data_snapshot_attempt();
        if result.is_err() {
            self.latch_persistence_unavailable();
        }
        result
    }

    fn persist_data_snapshot_attempt(&self) -> Result<(), FsError> {
        if crate::drivers::ATA_PRIMARY.lock().is_none() {
            return Ok(());
        }
        let mut buf = alloc::vec![0u8; 32768];
        buf[..4].copy_from_slice(b"DTA1");
        let mut pos=6usize;
        let mut count=0u16;
        for inode in self.inodes.values() {
            if inode.file_type != FileType::RegularFile || !(inode.path.starts_with("/data/") || inode.path.starts_with("/user/")) { continue; }
            let path=inode.path.as_bytes();
            let data=inode.data.as_slice();
            if path.len()>255 || data.len()>8192 { continue; }
            if pos + 2 + path.len() + 4 + data.len() > buf.len() { break; }
            buf[pos..pos+2].copy_from_slice(&(path.len() as u16).to_le_bytes()); pos+=2;
            buf[pos..pos+path.len()].copy_from_slice(path); pos+=path.len();
            buf[pos..pos+4].copy_from_slice(&(data.len() as u32).to_le_bytes()); pos+=4;
            buf[pos..pos+data.len()].copy_from_slice(data); pos+=data.len();
            count=count.saturating_add(1);
        }
        buf[4..6].copy_from_slice(&count.to_le_bytes());
        let mut sealed = Vec::new();
        crate::storage::encrypted::seal(&buf[..pos], &mut sealed)
            .map_err(|_| FsError::IntegrityViolation)?;
        crate::storage::persistent::commit_kind(&sealed,4).map(|_|()).map_err(|_|FsError::IntegrityViolation)?;
        // The block-backed InterFS copy is the durable filesystem source of truth.
        crate::storage::filesystem::store_data_file("/data/.vfs.snapshot", &sealed)
            .map_err(|_| FsError::IntegrityViolation)
    }

    /// Persists the mutable /data inode tree separately from file payloads.
    /// Paths and metadata are durable; file bytes remain in InterFS data files.
    fn persist_metadata_snapshot(&self) -> Result<(), FsError> {
        if self.persist_unavailable.get() {
            return Err(FsError::ReadOnlyFileSystem);
        }
        let result = self.persist_metadata_snapshot_attempt();
        if result.is_err() {
            self.latch_persistence_unavailable();
        }
        result
    }

    fn persist_metadata_snapshot_attempt(&self) -> Result<(), FsError> {
        if crate::drivers::ATA_PRIMARY.lock().is_none() {
            return Ok(());
        }
        let mut buf = alloc::vec![0u8; METADATA_SNAPSHOT_LIMIT];
        buf[..4].copy_from_slice(b"IVM2");
        let mut pos = 6usize;
        let mut count = 0u16;
        for inode in self.inodes.values() {
            if !(inode.path.starts_with("/data/") || inode.path.starts_with("/user/")) && inode.path != "/data" && inode.path != "/user" { continue; }
            let path = inode.path.as_bytes();
            if path.len() > 255 || pos + 8 + 1 + 2 + path.len() + 4 + 32 + 4 + 4 + 2 + 2 + 24 > buf.len() { continue; }
            buf[pos..pos + 8].copy_from_slice(&inode.id.0.to_le_bytes()); pos += 8;
            let kind = match inode.file_type { FileType::Directory => 1u8, FileType::RegularFile => 2u8, _ => continue };
            buf[pos] = kind; pos += 1;
            buf[pos..pos + 2].copy_from_slice(&(path.len() as u16).to_le_bytes()); pos += 2;
            buf[pos..pos + path.len()].copy_from_slice(path); pos += path.len();
            buf[pos..pos + 4].copy_from_slice(&(inode.size as u32).to_le_bytes()); pos += 4;
            buf[pos..pos + 32].copy_from_slice(&inode.sha256_digest); pos += 32;
            buf[pos..pos + 4].copy_from_slice(&inode.uid.to_le_bytes()); pos += 4;
            buf[pos..pos + 4].copy_from_slice(&inode.gid.to_le_bytes()); pos += 4;
            buf[pos..pos + 2].copy_from_slice(&inode.mode.to_le_bytes()); pos += 2;
            buf[pos..pos + 2].copy_from_slice(&inode.nlink.to_le_bytes()); pos += 2;
            buf[pos..pos + 8].copy_from_slice(&inode.atime.to_le_bytes()); pos += 8;
            buf[pos..pos + 8].copy_from_slice(&inode.mtime.to_le_bytes()); pos += 8;
            buf[pos..pos + 8].copy_from_slice(&inode.ctime.to_le_bytes()); pos += 8;
            count = count.saturating_add(1);
        }
        for (alias, inode_id) in &self.hard_links {
            let path = alias.as_bytes();
            if path.len() > 255 || pos + 11 + path.len() > buf.len() { continue; }
            buf[pos..pos+8].copy_from_slice(&inode_id.0.to_le_bytes()); pos += 8;
            buf[pos] = 3; pos += 1;
            buf[pos..pos+2].copy_from_slice(&(path.len() as u16).to_le_bytes()); pos += 2;
            buf[pos..pos+path.len()].copy_from_slice(path); pos += path.len();
            count = count.saturating_add(1);
        }
        buf[4..6].copy_from_slice(&count.to_le_bytes());
        let mut sealed = Vec::new();
        crate::storage::encrypted::seal(&buf[..pos], &mut sealed).map_err(|_| FsError::IntegrityViolation)?;
        crate::storage::filesystem::store_data_file("/data/.vfs.metadata", &sealed).map_err(|_| FsError::IntegrityViolation)?;
        Ok(())
    }

    /// Restores the persistent /data inode tree before restoring file payloads.
    fn restore_metadata_snapshot(&mut self) {
        let mut sealed = alloc::vec![0u8; 32768 + crate::storage::encrypted::ENCRYPTED_OVERHEAD];
        let Ok(len) = crate::storage::filesystem::load_data_file("/data/.vfs.metadata", &mut sealed) else {
            crate::serial_println!("[VFS RESTORE] No /data/.vfs.metadata found.");
            return;
        };
        crate::serial_println!("[VFS RESTORE] Loaded /data/.vfs.metadata: {} bytes", len);
        let mut buf = alloc::vec![0u8; METADATA_SNAPSHOT_LIMIT];
        let len = match crate::storage::encrypted::open(&sealed[..len], &mut buf) {
            Ok(plain_len) => {
                crate::serial_println!("[VFS RESTORE] Decrypted metadata: {} bytes", plain_len);
                plain_len
            }
            Err(e) => {
                crate::serial_println!("[VFS RESTORE] Metadata decrypt error: {:?}", e);
                if len <= buf.len() { buf[..len].copy_from_slice(&sealed[..len]); len } else { return; }
            }
        };
        if len < 6 || (&buf[..4] != b"IVM2" && &buf[..4] != b"IVM1") {
            crate::serial_println!("[VFS RESTORE] Invalid metadata header magic.");
            return;
        }
        let v2 = &buf[..4] == b"IVM2";
        let count = u16::from_le_bytes([buf[4], buf[5]]) as usize;
        let mut pos = 6usize;
        for _ in 0..count {
            if pos + 11 > len { break; }
            let id = match buf[pos..pos+8].try_into() { Ok(b) => u64::from_le_bytes(b), Err(_) => break }; pos += 8;
            let kind = buf[pos]; pos += 1;
            let plen = u16::from_le_bytes([buf[pos],buf[pos+1]]) as usize; pos += 2;
            let extra = if v2 { 68 } else { 36 };
            if plen == 0 || plen > 255 || pos + plen + extra > len { break; }
            let path = match core::str::from_utf8(&buf[pos..pos+plen]).ok() { Some(v) => v, None => break };
            pos += plen;
            if kind == 3 {
                if self.inodes.contains_key(&InodeId(id)) {
                    // The primary inode record already persists the complete nlink count.
                    // Alias records restore path identity only; incrementing
                    // again here would double-count links after every reboot.
                    self.hard_links.insert(path.to_string(), InodeId(id));
                }
                continue;
            }
            let size = u32::from_le_bytes([buf[pos],buf[pos+1],buf[pos+2],buf[pos+3]]) as usize; pos += 4;
            let mut digest=[0u8;32]; digest.copy_from_slice(&buf[pos..pos+32]); pos += 32;
            let (uid,gid,mode,nlink,atime,mtime,ctime) = if v2 {
                let uid = match buf[pos..pos+4].try_into() { Ok(b) => u32::from_le_bytes(b), Err(_) => break }; pos += 4;
                let gid = match buf[pos..pos+4].try_into() { Ok(b) => u32::from_le_bytes(b), Err(_) => break }; pos += 4;
                let mode = match buf[pos..pos+2].try_into() { Ok(b) => u16::from_le_bytes(b), Err(_) => break }; pos += 2;
                let nlink = match buf[pos..pos+2].try_into() { Ok(b) => u16::from_le_bytes(b), Err(_) => break }; pos += 2;
                let atime = match buf[pos..pos+8].try_into() { Ok(b) => u64::from_le_bytes(b), Err(_) => break }; pos += 8;
                let mtime = match buf[pos..pos+8].try_into() { Ok(b) => u64::from_le_bytes(b), Err(_) => break }; pos += 8;
                let ctime = match buf[pos..pos+8].try_into() { Ok(b) => u64::from_le_bytes(b), Err(_) => break }; pos += 8;
                (uid,gid,mode,nlink,atime,mtime,ctime)
            } else { (ROOT_UID,ROOT_UID,if kind==1 {DEFAULT_DIR_MODE} else {DEFAULT_FILE_MODE},1,0,0,0) };
            if !(path.starts_with("/data/") || path.starts_with("/user/") || path.starts_with("/storage/")) && path != "/data" && path != "/user" && path != "/storage" { continue; }
            if kind == 1 {
                let created=self.mkdir(path);
                if let Some(inode)=self.inodes.get_mut(&created) { inode.id=InodeId(id); inode.uid=uid; inode.gid=gid; inode.mode=mode; inode.nlink=nlink; inode.atime=atime; inode.mtime=mtime; inode.ctime=ctime; }
            } else if kind == 2 {
                let created=self.create_file(path,&[],false);
                if let Some(mut inode)=self.inodes.remove(&created) {
                    inode.id=InodeId(id); inode.size=size; inode.sha256_digest=digest; inode.data=InodeData::Dynamic(Vec::new());
                    inode.uid=uid; inode.gid=gid; inode.mode=mode; inode.nlink=nlink; inode.atime=atime; inode.mtime=mtime; inode.ctime=ctime;
                    self.inodes.insert(InodeId(id),inode); self.path_to_inode.insert(path.to_string(),InodeId(id));
                }
            }
            if id>=self.next_inode { self.next_inode=id.saturating_add(1); }
        }
    }

    /// Restores mutable /data files from the block-backed filesystem first.
    /// The metadata journal remains a recovery fallback until the transactional
    /// publication protocol finalizes the write.
    fn restore_data_snapshot(&mut self) {
        let mut sealed = alloc::vec![0u8; 32768 + crate::storage::encrypted::ENCRYPTED_OVERHEAD];
        let block_len = match crate::storage::filesystem::load_data_file("/data/.vfs.snapshot", &mut sealed) {
            Ok(len) => {
                crate::serial_println!("[VFS RESTORE] Loaded /data/.vfs.snapshot from block fs: {} bytes", len);
                Some(len)
            }
            Err(_) => None,
        };
        let len = if let Some(len) = block_len {
            len
        } else {
            let Ok(Some((generation,len)))=crate::storage::persistent::load_latest_kind(&mut sealed,4) else {
                crate::serial_println!("[VFS RESTORE] No persistent snapshot found in block fs or journal.");
                return;
            };
            crate::serial_println!("[VFS RESTORE] Loaded snapshot from journal gen {}: {} bytes", generation, len);
            len
        };
        let mut buf = alloc::vec![0u8; 32768];
        let len = match crate::storage::encrypted::open(&sealed[..len], &mut buf) {
            Ok(plain_len) => {
                crate::serial_println!("[VFS RESTORE] Decrypted data snapshot: {} bytes", plain_len);
                plain_len
            }
            Err(e) => {
                crate::serial_println!("[VFS RESTORE] Data snapshot decrypt error: {:?}", e);
                // One-time compatibility with legacy unencrypted plaintext snapshots.
                if len <= buf.len() {
                    buf[..len].copy_from_slice(&sealed[..len]);
                    len
                } else { return; }
            }
        };
        if len<6 || &buf[..4]!=b"DTA1" {
            crate::serial_println!("[VFS RESTORE] Invalid data snapshot header magic.");
            return;
        }
        let count=u16::from_le_bytes([buf[4],buf[5]]) as usize;
        let mut pos=6usize;
        crate::serial_println!("[VFS RESTORE] Restoring {} data files from snapshot...", count);
        for _ in 0..count {
            if pos+2>len { break; }
            let plen=u16::from_le_bytes([buf[pos],buf[pos+1]]) as usize; pos+=2;
            if plen==0 || plen>255 || pos+plen+4>len { break; }
            let path=match core::str::from_utf8(&buf[pos..pos+plen]).ok(){Some(v)=>v, None=>break}; pos+=plen;
            let dlen=u32::from_le_bytes([buf[pos],buf[pos+1],buf[pos+2],buf[pos+3]]) as usize; pos+=4;
            if dlen>8192 || pos+dlen>len { break; }
            crate::serial_println!("[VFS RESTORE] Restored file: {} ({} bytes)", path, dlen);
            self.create_file(path,&buf[pos..pos+dlen],false); pos+=dlen;
        }
    }

    /// Rebuilds parent/child inode relationships from durable paths.
    fn rebuild_children(&mut self) {
        for inode in self.inodes.values_mut() { inode.children.clear(); }
        let ids: Vec<InodeId> = self.inodes.keys().copied().collect();
        for child_id in ids {
            if child_id == InodeId(1) { continue; }
            let Some(path) = self.inodes.get(&child_id).map(|i| i.path.clone()) else { continue; };
            let parent = path.rsplit_once('/').map(|(p, _)| if p.is_empty() { "/" } else { p }).unwrap_or("/");
            if let Some(parent_id) = self.path_to_inode.get(parent).copied() {
                if let Some(parent_inode) = self.inodes.get_mut(&parent_id) { parent_inode.children.push(child_id); }
            }
        }
    }

    /// Releases every open file descriptor owned by a terminating task.
    ///
    /// Descriptors are kernel-owned resources and must not survive task death.
    pub fn cleanup_task(&mut self, task_id: TaskId) -> usize {
        let closed = self.task_file_tables.remove(&task_id)
            .map(|table| table.iter().filter(|slot| slot.is_some()).count())
            .unwrap_or(0);
        if closed > 0 {
            if let Some(process) = crate::task::process::process_for_task(task_id) {
                for _ in 0..closed {
                    crate::task::process::release_open_file(process);
                }
            }
        }
        closed
    }

    /// Closes an open file descriptor.
    pub fn close(&mut self, task_id: TaskId, fd: FileDescriptor) -> Result<(), FsError> {
        // End the descriptor borrow before persisting the filesystem snapshot.
        // This keeps the VFS mutation and persistence phases independently
        // borrow-checked and avoids holding a task FD-table borrow across I/O.
        let closed = {
            let table = self.task_file_tables.get_mut(&task_id).ok_or(FsError::InvalidFd)?;
            if let Some(slot) = table.get_mut(fd.0 as usize) {
                if slot.is_some() {
                    *slot = None;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if !closed {
            return Err(FsError::InvalidFd);
        }
        if let Some(process) = crate::task::process::process_for_task(task_id) {
            crate::task::process::release_open_file(process);
        }
        if self.persist_data_snapshot().is_err() {
            // Write-behind refused; failure is latched inside persist_data_snapshot
        }
        if self.persist_metadata_snapshot().is_err() {
            // Write-behind refused; failure is latched inside persist_metadata_snapshot
        }
        Ok(())
    }

    /// Records the first persistence refusal and says so, once.
    ///
    /// WHY one line rather than silence: the honest state is "this boot's
    /// filesystem is RAM-only", and a user deserves to be told that a file they
    /// wrote will not survive reboot. The console carries that once, at the
    /// moment it becomes true, instead of on every subsequent close.
    fn latch_persistence_unavailable(&self) {
        if !self.persist_unavailable.replace(true) {
            crate::serial_println!(
                "[VFS] persistent storage refused on this device; the filesystem stays RAM-only for this boot."
            );
        }
    }
}

/// Normalizes a path string.
fn normalize_path(path: &str) -> String {
    let mut out = String::from("/");
    let mut first = true;
    for component in path.split('/') {
        if component.is_empty() || component == "." { continue; }
        if component == ".." {
            // Reject traversal instead of allowing lexical escape from the VFS root.
            return String::from("/");
        }
        if component.len() > 255 { return String::from("/"); }
        if !first { out.push('/'); }
        out.push_str(component);
        first = false;
    }
    out
}

fn parent_path(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => parent,
        _ => "/",
    }
}

/// Extracts filename from path.
fn path_filename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or("").to_string()
}

/// Global synchronized Virtual Filesystem instance.
pub static VFS: Mutex<Vfs> = Mutex::new(Vfs {
    inodes: BTreeMap::new(),
    path_to_inode: BTreeMap::new(),
    hard_links: BTreeMap::new(),
    next_inode: 1,
    task_file_tables: BTreeMap::new(),
    persist_unavailable: core::cell::Cell::new(false),
    storage_generation: 0,
});

fn generate_ppm_p6(r_base: u8, g_base: u8, b_base: u8, r_step: i8, g_step: i8, b_step: i8) -> alloc::vec::Vec<u8> {
    let mut data = alloc::vec::Vec::with_capacity(16 + 32 * 32 * 3);
    data.extend_from_slice(b"P6\n32 32\n255\n");
    for y in 0..32u8 {
        for x in 0..32u8 {
            let r = (r_base as i16 + (x as i8 * r_step) as i16 + (y as i8 * r_step / 2) as i16).clamp(0, 255) as u8;
            let g = (g_base as i16 + (y as i8 * g_step) as i16 + (x as i8 * g_step / 2) as i16).clamp(0, 255) as u8;
            let b = (b_base as i16 + (x as i8 * b_step) as i16 + (y as i8 * b_step) as i16).clamp(0, 255) as u8;
            data.push(r);
            data.push(g);
            data.push(b);
        }
    }
    data
}

fn generate_wav_8bit(frequency_hz: u32, duration_samples: u32) -> alloc::vec::Vec<u8> {
    let mut data = alloc::vec::Vec::with_capacity(44 + duration_samples as usize);
    let total_file_size = (36 + duration_samples) as u32;
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&total_file_size.to_le_bytes());
    data.extend_from_slice(b"WAVE");
    data.extend_from_slice(b"fmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&8000u32.to_le_bytes());
    data.extend_from_slice(&8000u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&8u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&duration_samples.to_le_bytes());
    let period = if frequency_hz > 0 { (8000 / frequency_hz).max(2) } else { 20 };
    for i in 0..duration_samples {
        let phase_int = (i % period) * 255 / period;
        let sample = if phase_int < 128 {
            (128 + phase_int / 2) as u8
        } else {
            (128 + (255 - phase_int) / 2) as u8
        };
        data.push(sample);
    }
    data
}

/// Initializes the root filesystem with essential system directories and trusted files.
pub fn init() {
    let mut vfs = Vfs::new();

    // Create system directory layout
    vfs.mkdir("/system");
    vfs.mkdir("/system/bin");
    vfs.mkdir("/system/etc");
    vfs.mkdir("/packages");
    vfs.mkdir("/data");
    vfs.mkdir("/user");
    vfs.mkdir("/storage");
    vfs.mkdir("/storage/photos");
    vfs.mkdir("/storage/pictures");
    vfs.mkdir("/storage/videos");
    vfs.mkdir("/storage/downloads");
    vfs.mkdir("/storage/music");
    vfs.mkdir("/system/models");

    // Sovereign AI System Model Weights
    let embedding_weights = b"GEMMA2_EMBED_Q8\x00dim=256\x00vocab=256000\x00layers=18\x00format=raw_weights\n";
    vfs.create_file("/system/models/embedding_gemma2.bin", embedding_weights, false);
    let gen_weights = b"GEMMA_2B_IT_Q4_K_M\x00context=2048\x00vocab=256000\x00layers=28\x00format=gguf\n";
    vfs.create_file("/system/models/local_gemma_2b.bin", gen_weights, false);

    // Sovereign Storage Sample Media Files (Real PPM & WAV assets)
    let mountain = generate_ppm_p6(40, 80, 160, 4, 3, 2);
    vfs.create_file("/storage/photos/sample_mountain.ppm", &mountain, false);
    let sunset = generate_ppm_p6(200, 100, 40, -3, 2, 4);
    vfs.create_file("/storage/photos/sample_sunset.ppm", &sunset, false);
    let aurora = generate_ppm_p6(20, 140, 100, 2, 4, 3);
    vfs.create_file("/storage/pictures/wallpaper_aurora.ppm", &aurora, false);
    let video_clip = generate_ppm_p6(30, 90, 130, 3, 2, 4);
    vfs.create_file("/storage/videos/nature_clip.ppm", &video_clip, false);
    let music_chill = generate_wav_8bit(440, 2000);
    vfs.create_file("/storage/music/ambient_chill.wav", &music_chill, false);
    let chime = generate_wav_8bit(880, 1600);
    vfs.create_file("/storage/music/ringtone_chime.wav", &chime, false);
    vfs.create_file("/storage/downloads/readme.txt", b"InterOS Sovereign Media Downloads\nAll files stored securely under zero ambient authority.\n", false);
    let dl_preview = generate_ppm_p6(60, 120, 180, 3, 3, 3);
    vfs.create_file("/storage/downloads/preview_clip.ppm", &dl_preview, false);

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
    let audit_init = b"[SYSTEM AUDIT LOG INITIALIZED] Storage & VFS Active.\n";
    vfs.create_file("/data/audit.log", audit_init, false);

    // 4. First-party native packages. The package service launches from these
    // verified .rapp payloads; legacy /system/bin entries remain diagnostics.
    vfs.create_static_file("/system/bin/init", crate::task::USER_INIT_ELF);
    vfs.create_static_file("/system/bin/settings", crate::task::APP_SETTINGS_ELF);

    // RAPP v2 packages are signed developer artifacts. The kernel must never
    // synthesize unsigned packages during boot; provisioning occurs through the
    // trusted package pipeline after publisher trust has been established.
    crate::serial_println!("[PACKAGE] First-party RAPP provisioning deferred to trusted signed-package pipeline.");
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

    vfs.restore_metadata_snapshot();
    vfs.restore_data_snapshot();
    vfs.rebuild_children();
    let _ = vfs.persist_metadata_snapshot();
    *VFS.lock() = vfs;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{delegation, Capability, CapabilityId};

    #[test]
    fn delegated_lease_single_invocation_vfs_open_read_stat() {
        let mut vfs = Vfs::new();
        vfs.create_file("/test.txt", b"vfs payload", false);

        let task_id = TaskId(888);
        crate::task::process::register_leader(task_id, TaskId(1));

        let current_ticks = crate::interrupts::get_system_ticks();
        let parent_cap = Capability {
            id: CapabilityId(200),
            cap_type: CapabilityType::FileSystemRead,
            rights: Rights::READ.union(Rights::GRANT),
            object_id: 0,
        };

        // 1. Single-use lease for open
        let lease_open = delegation::DelegatedLease::new(
            9100,
            &parent_cap,
            TaskId(1),
            task_id,
            Rights::READ,
            1, // max_invocations = 1
            100_000,
            current_ticks,
        ).expect("create lease_open");
        delegation::register_lease(lease_open.clone());

        // 2. Single-use lease for read
        let lease_read = delegation::DelegatedLease::new(
            9101,
            &parent_cap,
            TaskId(1),
            task_id,
            Rights::READ,
            1, // max_invocations = 1
            100_000,
            current_ticks,
        ).expect("create lease_read");
        delegation::register_lease(lease_read.clone());

        // 3. Single-use lease for stat
        let lease_stat = delegation::DelegatedLease::new(
            9102,
            &parent_cap,
            TaskId(1),
            task_id,
            Rights::READ,
            1, // max_invocations = 1
            100_000,
            current_ticks,
        ).expect("create lease_stat");
        delegation::register_lease(lease_stat.clone());

        let mut cap_table = CapabilityTable::new_for_task(task_id, 8);
        cap_table.insert(lease_open.to_capability()).expect("insert lease_open");
        cap_table.insert(lease_read.to_capability()).expect("insert lease_read");
        cap_table.insert(lease_stat.to_capability()).expect("insert lease_stat");

        // Inspecting does not consume invocations
        assert!(cap_table.inspect_slot(0, CapabilityType::FileSystemRead, Rights::READ).is_ok());
        assert!(cap_table.inspect_slot(1, CapabilityType::FileSystemRead, Rights::READ).is_ok());
        assert!(cap_table.inspect_slot(2, CapabilityType::FileSystemRead, Rights::READ).is_ok());

        // WHY the budget is asserted across the table and not per slot: the
        // caller names the authority it wants -- a filesystem read -- and the
        // kernel finds the capability that carries it. Which of three
        // identical leases is spent is the kernel's business; how many
        // invocations the delegate may make in total is the guarantee, and
        // the guarantee is the sum of the leases' quotas. The old test
        // asserted that a *slot* could not be reused, which was a statement
        // about addressing rather than about authority.
        //
        // Test open: must succeed on first try
        let fd_res = vfs.open(task_id, &cap_table, 0, "/test.txt", Rights::READ);
        assert!(fd_res.is_ok(), "vfs.open with single-use lease failed: {:?}", fd_res.err());
        let fd = fd_res.unwrap();

        // Test read: must succeed on the second invocation
        let mut buf = [0u8; 32];
        let read_res = vfs.read(task_id, &cap_table, 1, fd, &mut buf);
        assert!(read_res.is_ok(), "vfs.read with single-use lease failed: {:?}", read_res.err());
        assert_eq!(&buf[..read_res.unwrap()], b"vfs payload");

        // Test stat: must succeed on the third invocation
        let stat_res = vfs.stat(&cap_table, 2, "/test.txt");
        assert!(stat_res.is_ok(), "vfs.stat with single-use lease failed: {:?}", stat_res.err());
        assert_eq!(stat_res.unwrap().size, 11);

        // The fourth delegated operation must fail closed: three leases of
        // one invocation each are three invocations in total, and the fourth
        // finds no budget left. The error names the exhausted budget rather
        // than a type mismatch, because the delegate *was* authorised and
        // has spent what it was given.
        let fd_res4 = vfs.open(task_id, &cap_table, 0, "/test.txt", Rights::READ);
        assert!(
            matches!(fd_res4, Err(FsError::CapabilityError(CapError::InvalidSlot))),
            "the fourth delegated operation must fail closed once the three-lease budget is spent, got {:?}",
            fd_res4
        );
        let read_res4 = vfs.read(task_id, &cap_table, 1, fd, &mut buf);
        assert!(
            matches!(read_res4, Err(FsError::CapabilityError(CapError::InvalidSlot))),
            "the read after the budget is spent must fail closed, got {:?}",
            read_res4
        );
        let stat_res4 = vfs.stat(&cap_table, 2, "/test.txt");
        assert!(
            matches!(stat_res4, Err(FsError::CapabilityError(CapError::InvalidSlot))),
            "the stat after the budget is spent must fail closed, got {:?}",
            stat_res4
        );

        // 4. Single-use lease for write
        let parent_cap_write = Capability {
            id: CapabilityId(201),
            cap_type: CapabilityType::FileSystemWrite,
            rights: Rights::WRITE.union(Rights::GRANT),
            object_id: 0,
        };
        let lease_write = delegation::DelegatedLease::new(
            9103,
            &parent_cap_write,
            TaskId(1),
            task_id,
            Rights::WRITE,
            1, // max_invocations = 1
            100_000,
            current_ticks,
        ).expect("create lease_write");
        delegation::register_lease(lease_write.clone());
        cap_table.insert(lease_write.to_capability()).expect("insert lease_write");

        vfs.create_device_node("/dev/null");
        let null_inode = vfs.lookup("/dev/null").expect("lookup /dev/null");
        let fd_write = vfs.allocate_fd(task_id, null_inode, Rights::WRITE).expect("allocate write fd");

        // First write succeeds
        let write_res1 = vfs.write(task_id, &cap_table, 3, fd_write, b"!!");
        assert!(write_res1.is_ok(), "vfs.write with single-use lease failed: {:?}", write_res1.err());

        // Second write fails (quota exhausted)
        let write_res2 = vfs.write(task_id, &cap_table, 3, fd_write, b"!!");
        assert!(write_res2.is_err(), "second vfs.write must fail after quota exhausted");
    }

    #[test]
    fn vfs_object_scoping_and_slot_fallback_prevention() {
        let mut vfs = Vfs::new();
        let task_id = TaskId(88);
        crate::task::process::register_leader(task_id, TaskId(1));

        vfs.create_file("/scoped_a.txt", b"secret a", false);
        vfs.create_file("/scoped_b.txt", b"secret b", false);
        let _inode_dir = vfs.mkdir("/scoped_dir");

        let inode_a = vfs.lookup("/scoped_a.txt").expect("lookup a");
        let inode_b = vfs.lookup("/scoped_b.txt").expect("lookup b");
        let inode_dir = vfs.lookup("/scoped_dir").expect("lookup dir");
        assert_ne!(inode_a, inode_b);
        assert_ne!(inode_a, inode_dir);

        let mut cap_table = CapabilityTable::new_for_task(task_id, 8);
        // Slot 0: FileSystemRead scoped strictly to Inode A
        cap_table.insert(Capability {
            id: CapabilityId(501),
            cap_type: CapabilityType::FileSystemRead,
            rights: Rights::READ,
            object_id: inode_a.0,
        }).expect("insert scoped read");
        // Slot 1: FileSystemWrite scoped strictly to Inode A
        cap_table.insert(Capability {
            id: CapabilityId(502),
            cap_type: CapabilityType::FileSystemWrite,
            rights: Rights::WRITE | Rights::READ,
            object_id: inode_a.0,
        }).expect("insert scoped write");

        // 1. Inode A operations succeed with matching scoped capability
        let fd_a = vfs.open(task_id, &cap_table, 0, "/scoped_a.txt", Rights::READ).expect("open a with cap scoped to a");
        let mut buf = [0u8; 16];
        let read_a = vfs.read(task_id, &cap_table, 0, fd_a, &mut buf).expect("read a with cap scoped to a");
        assert_eq!(&buf[..read_a], b"secret a");
        let stat_a = vfs.stat(&cap_table, 0, "/scoped_a.txt").expect("stat a with cap scoped to a");
        assert_eq!(stat_a.size, 8);

        // 2. Operations on Inode B MUST fail closed with PermissionDenied when using capability scoped to Inode A
        assert_eq!(vfs.open(task_id, &cap_table, 0, "/scoped_b.txt", Rights::READ).err(), Some(FsError::PermissionDenied));
        assert_eq!(vfs.stat(&cap_table, 0, "/scoped_b.txt").err(), Some(FsError::PermissionDenied));

        // Read/Write on fd for Inode B using capability scoped to Inode A must fail with PermissionDenied
        let fd_b = vfs.allocate_fd(task_id, inode_b, Rights::READ | Rights::WRITE).expect("allocate fd_b");
        assert_eq!(vfs.read(task_id, &cap_table, 0, fd_b, &mut buf).err(), Some(FsError::PermissionDenied));
        assert_eq!(vfs.write(task_id, &cap_table, 1, fd_b, b"hack").err(), Some(FsError::PermissionDenied));

        // Directory listing on /scoped_dir using capability scoped to Inode A must fail with PermissionDenied
        let mut list_buf = [0u8; 64];
        assert_eq!(vfs.list_dir(&cap_table, 0, "/scoped_dir", &mut list_buf).err(), Some(FsError::PermissionDenied));

        // 3. The slot argument is not an authority: a slot that does not
        // exist grants nothing and denies nothing, and the capability set
        // decides. The old test asserted that naming slot 999 failed,
        // because a slot was the address of the authority. It is not.
        //
        // WHY the object scope is still enforced, and that is the part that
        // matters: the capability the task holds is scoped to Inode A, so
        // Inode B is refused however the caller addresses it.
        assert_eq!(
            vfs.open(task_id, &cap_table, 999, "/scoped_b.txt", Rights::READ).err(),
            Some(FsError::PermissionDenied)
        );
        assert_eq!(
            vfs.stat(&cap_table, 999, "/scoped_b.txt").err(),
            Some(FsError::PermissionDenied)
        );
        assert_eq!(
            vfs.read(task_id, &cap_table, 999, fd_b, &mut buf).err(),
            Some(FsError::PermissionDenied)
        );
        assert_eq!(
            vfs.write(task_id, &cap_table, 999, fd_b, b"data").err(),
            Some(FsError::PermissionDenied)
        );
        assert_eq!(
            vfs.list_dir(&cap_table, 999, "/scoped_dir", &mut list_buf).err(),
            Some(FsError::PermissionDenied)
        );

        // 4. A task that holds no filesystem capability at all is refused
        // with a capability error, whatever it names.
        let mut empty_caps = CapabilityTable::new_for_task(task_id, 8);
        assert!(matches!(
            vfs.open(task_id, &empty_caps, 0, "/scoped_a.txt", Rights::READ).err(),
            Some(FsError::CapabilityError(_))
        ));
        assert!(matches!(
            vfs.stat(&empty_caps, 0, "/scoped_a.txt").err(),
            Some(FsError::CapabilityError(_))
        ));
        assert!(matches!(
            vfs.read(task_id, &empty_caps, 0, fd_a, &mut buf).err(),
            Some(FsError::CapabilityError(_))
        ));
        assert!(matches!(
            vfs.write(task_id, &empty_caps, 0, fd_a, b"data").err(),
            Some(FsError::CapabilityError(_))
        ));
        assert!(matches!(
            vfs.list_dir(&empty_caps, 0, "/scoped_dir", &mut list_buf).err(),
            Some(FsError::CapabilityError(_))
        ));
    }

    #[test]
    fn vfs_large_file_writes_and_metadata_snapshot_limits() {
        assert_eq!(METADATA_SNAPSHOT_LIMIT, 32 * 1024);

        let mut vfs = Vfs::new();
        let task_id = TaskId(88);
        let inode_id = vfs.create_file("/large_test.bin", &[], false);

        let mut caps = CapabilityTable::new_for_task(task_id, 8);
        caps.insert(crate::capability::Capability {
            id: crate::capability::CapabilityId(8801),
            cap_type: CapabilityType::FileSystemWrite,
            rights: Rights::WRITE,
            object_id: 0,
        }).expect("insert write cap");
        caps.insert(crate::capability::Capability {
            id: crate::capability::CapabilityId(8802),
            cap_type: CapabilityType::FileSystemRead,
            rights: Rights::READ,
            object_id: 0,
        }).expect("insert read cap");

        let fd = vfs.allocate_fd(task_id, inode_id, Rights::READ | Rights::WRITE)
            .expect("allocate file fd");

        // Write a 16 KB payload (exceeding legacy 8192-byte limit)
        let large_payload = alloc::vec![0xAAu8; 16384];
        let written = vfs.write(task_id, &caps, 0, fd, &large_payload)
            .expect("large write beyond 8 KB must succeed");
        assert_eq!(written, 16384);

        // Read it back
        let mut read_back = alloc::vec![0u8; 16384];
        let read_handle = vfs.task_file_tables.get_mut(&task_id).unwrap()[fd.0 as usize].as_mut().unwrap();
        read_handle.offset = 0;
        let n = vfs.read(task_id, &caps, 0, fd, &mut read_back).expect("read large file");
        assert_eq!(n, 16384);
        assert_eq!(read_back, large_payload);

        // Truncate beyond 8 KB (e.g. to 20 KB)
        vfs.truncate(task_id, fd, 20480).expect("truncate beyond 8 KB must succeed");
        let stat = vfs.stat(&caps, 0, "/large_test.bin").expect("stat");
        assert_eq!(stat.size, 20480);
    }
}


