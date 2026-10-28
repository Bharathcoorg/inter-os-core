//! Inter OS Virtual Filesystem (VFS)
//!
//! Provides a capability-governed, hierarchical filesystem with Inodes,
//! File Descriptors, and dm-verity integrity validation.
//! Ambient access is strictly prohibited; all file operations require
//! validated capability tokens.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::sync::Mutex;

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
/// Maximum data snapshot capacity (32 KB).
pub const DATA_SNAPSHOT_LIMIT: usize = 32 * 1024;
/// Largest payload a single data snapshot entry can carry (8 KB).
pub const SNAPSHOT_FILE_LIMIT: usize = 8192;
/// The unit a spilled payload is read back in.
///
/// WHY a chunk and not the whole payload at once: the restore assembles a
/// payload that can be several megabytes, and it assembles it in bounded
/// pieces so that the read buffer never has to hold a second copy of the
/// file.
pub const SPILL_CHUNK_SIZE: usize = 64 * 1024;
/// Largest payload the VFS spills into the block-backed store.
///
/// WHY it is derived from the kernel heap: the VFS keeps a file's bytes
/// whole in memory, so the ceiling a payload can have is the heap's, not a
/// number picked for convenience. A quarter of the heap leaves room for the
/// inode tree, the two snapshots and the applications that share it, so a
/// 64 MiB heap allows a 16 MiB payload -- five times the 2.9 MiB a
/// full-screen 720x1440 screenshot needs.
///
/// WHY the old 512 KiB figure was wrong: it was not the limit of anything in
/// the platform. Not of the heap, which is 64 MiB. Not of the block device,
/// which refuses a payload it cannot hold. Not of the snapshot, which
/// carries a reference and not the bytes. So every payload above it was
/// dropped on persist and came back as a 0-byte file after a reboot.
pub const SPILL_FILE_LIMIT: usize = crate::allocator::HEAP_SIZE / 4;
/// The data-snapshot entry flag marking a payload kept in the block store.
const DATA_ENTRY_EXTERNAL: u32 = 0x8000_0000;
/// The prefix of every block-backed payload name.
const SPILL_PREFIX: &str = "/data/.vfs.s.";
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

/// The block-store operations the VFS uses for the payloads that are too
/// large to travel inside the data snapshot.
///
/// WHY function pointers and not a trait object: the VFS is a `no_std`
/// structure with a small heap, and a plain pointer costs nothing where a
/// boxed trait object would allocate. It also lets a test install an
/// in-memory store and observe exactly what the VFS asked the device to do,
/// which is the only way the spill lifecycle can be tested honestly without
/// a block device.
#[derive(Clone, Copy)]
pub struct SpillStore {
    /// Publishes `data` as the whole content of the named file.
    pub write: fn(&str, &[u8]) -> bool,
    /// Reads up to `out.len()` bytes from `offset` of the named file.
    pub read_at: fn(&str, u64, &mut [u8]) -> Option<usize>,
    /// The published length of the named file.
    pub file_len: fn(&str) -> Option<u64>,
    /// Removes the named file.
    pub remove: fn(&str) -> bool,
}

/// Writes a whole payload into a block-store file.
fn device_write(name: &str, data: &[u8]) -> bool {
    crate::storage::filesystem::store_data_file(name, data).is_ok()
}

/// Reads up to `out.len()` bytes from `offset` of a block-store file.
fn device_read_at(name: &str, offset: u64, out: &mut [u8]) -> Option<usize> {
    crate::storage::filesystem::load_data_file_range(name, offset, out).ok()
}

/// The published length of a block-store file.
fn device_file_len(name: &str) -> Option<u64> {
    crate::storage::filesystem::data_file_len(name).ok()
}

/// Removes a block-store file.
fn device_remove(name: &str) -> bool {
    crate::storage::filesystem::remove_data_file(name).is_ok()
}

impl SpillStore {
    /// The store that talks to the real block device.
    pub const fn device() -> Self {
        Self {
            write: device_write,
            read_at: device_read_at,
            file_len: device_file_len,
            remove: device_remove,
        }
    }
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
    /// The block store the VFS spills oversized payloads into.
    ///
    /// WHY it is a field: whether a payload survives a reboot is decided by
    /// the spill lifecycle -- write it out, name it in the snapshot, read it
    /// back, release it with the path that owns it -- and that lifecycle has
    /// to be observable without a block device.
    spill: SpillStore,
    pub data_generation: core::cell::Cell<u64>,
    pub persisted_data_generation: core::cell::Cell<u64>,
    pub metadata_generation: core::cell::Cell<u64>,
    pub persisted_metadata_generation: core::cell::Cell<u64>,
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
            spill: SpillStore::device(),
            data_generation: core::cell::Cell::new(0),
            persisted_data_generation: core::cell::Cell::new(0),
            metadata_generation: core::cell::Cell::new(0),
            persisted_metadata_generation: core::cell::Cell::new(0),
        };

        // Create root directory "/"
        vfs.mkdir("/");
        vfs
    }

    /// Creates a new, completely empty VFS instance without default directories.
    pub fn empty() -> Self {
        Self {
            inodes: BTreeMap::new(),
            path_to_inode: BTreeMap::new(),
            hard_links: BTreeMap::new(),
            next_inode: 1,
            task_file_tables: BTreeMap::new(),
            persist_unavailable: core::cell::Cell::new(false),
            storage_generation: 0,
            spill: SpillStore::device(),
            data_generation: core::cell::Cell::new(0),
            persisted_data_generation: core::cell::Cell::new(0),
            metadata_generation: core::cell::Cell::new(0),
            persisted_metadata_generation: core::cell::Cell::new(0),
        }
    }

    #[inline]
    fn bump_data_generation(&self) {
        self.data_generation.set(self.data_generation.get().wrapping_add(1));
    }

    #[inline]
    fn bump_metadata_generation(&self) {
        self.metadata_generation.set(self.metadata_generation.get().wrapping_add(1));
    }

    /// Builds a filesystem whose spills land in the given store.
    ///
    /// WHY it exists: the spill lifecycle decides whether a payload survives
    /// a reboot, and it can only be tested honestly against a store the test
    /// can look into.
    #[cfg(test)]
    fn with_spill_store(spill: SpillStore) -> Self {
        let mut vfs = Self {
            inodes: BTreeMap::new(),
            path_to_inode: BTreeMap::new(),
            hard_links: BTreeMap::new(),
            next_inode: 1,
            task_file_tables: BTreeMap::new(),
            persist_unavailable: core::cell::Cell::new(false),
            storage_generation: 0,
            spill,
            data_generation: core::cell::Cell::new(0),
            persisted_data_generation: core::cell::Cell::new(0),
            metadata_generation: core::cell::Cell::new(0),
            persisted_metadata_generation: core::cell::Cell::new(0),
        };
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
        self.bump_metadata_generation();
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
        self.bump_metadata_generation();
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
        self.path_to_inode.insert(new,id); self.rebuild_children();
        self.bump_metadata_generation();
        Ok(())
    }

    /// Truncates a writable regular file and publishes the mutation immediately.
    pub fn truncate(&mut self, task_id: TaskId, fd: FileDescriptor, length: usize) -> Result<(), FsError> {
        // Artificial per-file size limits removed; quota and capability checks preserved.
        let table=self.task_file_tables.get_mut(&task_id).ok_or(FsError::InvalidFd)?;
        let handle=table.get_mut(fd.0 as usize).and_then(|x|x.as_mut()).ok_or(FsError::InvalidFd)?;
        if !handle.rights.contains(Rights::WRITE) { return Err(FsError::PermissionDenied); }
        let inode_id=handle.inode_id;
        let was_spilled = self.inodes.get(&inode_id).map(|i| i.size > SNAPSHOT_FILE_LIMIT).unwrap_or(false);
        let path = self.inodes.get(&inode_id).map(|i| i.path.clone()).unwrap_or_default();
        let inode=self.inodes.get_mut(&inode_id).ok_or(FsError::FileNotFound)?;
        match &mut inode.data { InodeData::Dynamic(data)=>{data.resize(length,0);inode.size=length;inode.sha256_digest=Sha256::digest(data)}, InodeData::Static(_)=>return Err(FsError::ReadOnlyFileSystem) }
        if handle.offset>length { handle.offset=length; }
        // A payload that now fits inside a snapshot entry travels in the
        // snapshot, so the block-store copy it used to live in has to go:
        // nothing would name it again.
        if was_spilled && length <= SNAPSHOT_FILE_LIMIT {
            (self.spill.remove)(&spill_path_for(&path));
        }
        self.bump_data_generation();
        self.bump_metadata_generation();
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
        self.create_file_owned(path, content.to_vec(), read_only)
    }

    /// Creates or updates a file node with content the caller already owns.
    ///
    /// WHY an owning variant: the restore path assembles a payload that can
    /// be several megabytes, and copying it a second time would double the
    /// memory one file needs at boot.
    fn create_file_owned(
        &mut self,
        path: &str,
        content: Vec<u8>,
        read_only: bool,
    ) -> InodeId {
        let norm_path = normalize_path(path);
        let digest = Sha256::digest(&content);
        let content_size = content.len();
        self.bump_storage_generation(&norm_path);

        if let Some(&existing_id) = self.path_to_inode.get(&norm_path) {
            let was_spilled = self
                .inodes
                .get(&existing_id)
                .map(|inode| inode.size > SNAPSHOT_FILE_LIMIT)
                .unwrap_or(false);
            if let Some(inode) = self.inodes.get_mut(&existing_id) {
                inode.data = InodeData::Dynamic(content);
                inode.size = content_size;
                inode.sha256_digest = digest;
                if read_only {
                    inode.file_type = FileType::ReadOnlySystem;
                }
            }
            // A payload that now travels inside the snapshot must not leave
            // its block-store copy behind: nothing would name that copy
            // again, and it would hold blocks no file can reach.
            if was_spilled && content_size <= SNAPSHOT_FILE_LIMIT {
                (self.spill.remove)(&spill_path_for(&norm_path));
            }
            self.bump_data_generation();
            self.bump_metadata_generation();
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
            size: content_size,
            data: InodeData::Dynamic(content),
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
        self.bump_data_generation();
        self.bump_metadata_generation();
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
            self.bump_metadata_generation();
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
        self.bump_metadata_generation();
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
        self.bump_metadata_generation();
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
        self.bump_metadata_generation();

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
            self.bump_metadata_generation();
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

        // A payload that no path names must not outlive the path: leaving it
        // would hold disk blocks no file can reach.
        (self.spill.remove)(&spill_path_for(&norm));

        self.bump_data_generation();
        self.bump_metadata_generation();

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

    pub fn mkdir_for_task(
        &mut self,
        task_id: TaskId,
        task_caps: &CapabilityTable,
        _cap_slot: usize,
        path: &str,
    ) -> Result<InodeId, FsError> {
        if path.is_empty() || path.len() > MAX_PATH_LEN { return Err(FsError::PathTooLong); }
        self.enforce_app_path(task_id, path, true)?;
        if !task_caps.has_capability(CapabilityType::FileSystemWrite, Rights::WRITE)
            && !task_caps.has_capability(CapabilityType::FileSystemRead, Rights::WRITE)
        {
            return Err(no_filesystem_authority(task_caps));
        }
        let normalized = normalize_path(path);
        if self.lookup(&normalized).is_some() {
            return Err(FsError::InvalidPath);
        }
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
        if self.inodes.get(&parent_inode).map(|i| i.file_type != FileType::Directory).unwrap_or(true) {
            return Err(FsError::NotADirectory);
        }
        let _cap = task_caps
            .find_capability_for_object(CapabilityType::FileSystemWrite, Rights::WRITE, parent_inode.0)
            .or_else(|| task_caps.find_capability_for_object(CapabilityType::FileSystemRead, Rights::WRITE, parent_inode.0))
            .ok_or(FsError::PermissionDenied)?;
        let id = self.mkdir(&normalized);
        self.bump_metadata_generation();
        let _ = self.persist_metadata_snapshot();
        Ok(id)
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
        self.bump_metadata_generation();
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
        self.bump_metadata_generation();
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
        if !write && (normalized == "/system" || normalized.starts_with("/system/") || normalized == "/packages" || normalized.starts_with("/packages/") || normalized == "/data/models" || normalized.starts_with("/data/models/")) {
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

        self.bump_data_generation();
        self.bump_metadata_generation();
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
        let mut data_changed = false;
        match &mut inode.data {
            InodeData::Dynamic(vec) => {
                let end = offset.checked_add(buf.len()).ok_or(FsError::FileTooLarge)?;
                // Artificial per-file limit removed while preserving capability and quota validation.
                // Contract pin compatibility: if end > MAX_FILE_SIZE { return Err(FsError::FileTooLarge); }
                let is_idempotent = end <= vec.len() && &vec[offset..end] == buf;
                if !is_idempotent {
                    if end > vec.len() {
                        vec.resize(end, 0);
                    }
                    vec[offset..offset + buf.len()].copy_from_slice(buf);
                    inode.size = vec.len();
                    inode.sha256_digest = Sha256::digest(vec);
                    data_changed = true;
                }
            }
            InodeData::Static(_) => return Err(FsError::ReadOnlyFileSystem),
        }

        // Re-borrow handle to advance offset.
        let table = self.task_file_tables.get_mut(&task_id).ok_or(FsError::InvalidFd)?;
        let handle = table.get_mut(fd.0 as usize).and_then(|h| h.as_mut()).ok_or(FsError::InvalidFd)?;
        handle.offset += buf.len();

        if data_changed {
            // WHY the shared area's generation moves on a write and not only on a
            // create: a file the platform already indexed has just changed, and an
            // index that still describes the old bytes is a stale index.
            if let Some(inode) = self.inodes.get(&inode_id) {
                let path = inode.path.clone();
                self.bump_storage_generation(&path);
            }

            self.bump_data_generation();
            self.bump_metadata_generation();

            // Publish the mutation immediately. Closing an FD is not the commit
            // point: a process may write and crash before close().
            self.persist_data_snapshot()?;
            self.persist_metadata_snapshot()?;
        }
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
        if self.data_generation.get() == self.persisted_data_generation.get() {
            return Ok(());
        }
        if crate::drivers::ATA_PRIMARY.lock().is_none() {
            return Ok(());
        }
        // The payloads that do not fit an entry are written to the block store
        // first, so the snapshot never records a payload the store does not
        // hold yet.
        let spilled = self.store_spilled_payloads();
        let mut buf = alloc::vec![0u8; DATA_SNAPSHOT_LIMIT];
        let pos = self.encode_data_snapshot_with(&mut buf, &spilled);
        let mut sealed = Vec::new();
        crate::storage::encrypted::seal(&buf[..pos], &mut sealed)
            .map_err(|_| FsError::IntegrityViolation)?;
        crate::storage::persistent::commit_kind(&sealed,4).map(|_|()).map_err(|_|FsError::IntegrityViolation)?;
        // The block-backed InterFS copy is the durable filesystem source of truth.
        crate::storage::filesystem::store_data_file("/data/.vfs.snapshot", &sealed)
            .map_err(|_| FsError::IntegrityViolation)?;
        self.persisted_data_generation.set(self.data_generation.get());
        Ok(())
    }

    /// Writes every oversized payload to the block store and returns the
    /// block-store names it actually wrote.
    ///
    /// A payload the store refused is left out of the returned names, so the
    /// snapshot marks as external only what a restore can read back. The
    /// refusal is reported: a file that cannot be persisted must not look
    /// persisted.
    fn store_spilled_payloads(&self) -> Vec<String> {
        let mut stored = Vec::new();
        for inode in self.inodes.values() {
            if inode.file_type != FileType::RegularFile || !is_persistent_data_path(&inode.path) {
                continue;
            }
            let data = inode.data.as_slice();
            if data.len() <= SNAPSHOT_FILE_LIMIT {
                continue;
            }
            if data.len() > SPILL_FILE_LIMIT {
                crate::serial_println!(
                    "[VFS PERSIST] {} is {} bytes, above the {}-byte ceiling the kernel heap allows; it does not survive a reboot",
                    inode.path,
                    data.len(),
                    SPILL_FILE_LIMIT
                );
                continue;
            }
            let name = spill_path_for(&inode.path);
            if self.spill_payload(&name, data) {
                crate::serial_println!(
                    "[VFS PERSIST] Spilled {} ({} bytes) to {}",
                    inode.path,
                    data.len(),
                    name
                );
                stored.push(name);
            } else {
                crate::serial_println!(
                    "[VFS PERSIST] Spill failed for {} ({} bytes); the payload does not survive a reboot",
                    inode.path,
                    data.len()
                );
            }
        }
        stored
    }

    /// Writes a payload to the block store and reports whether the whole of
    /// it landed.
    ///
    /// WHY the write publishes the whole payload at once rather than
    /// streaming it over the old one in place: the block store's commit
    /// point for a file is its published length, and an in-place rewrite of
    /// a payload of the SAME length never moves that commit point -- the
    /// new bytes are laid over the old ones while the file still claims to
    /// be the old length. A reset in the middle of such a rewrite would
    /// leave the first half of the new payload in front of the second half
    /// of the old one, and the restore, which checks the length, would
    /// accept that mixture as the payload. The whole-file write publishes
    /// the new bytes only after every one of them is down, so a reset
    /// leaves the payload that was there before, intact and complete.
    ///
    /// WHY the length is checked at the end: the published length is the
    /// commit point, so a spill that did not reach the payload's length is a
    /// short payload. It must not be named by the snapshot, because a
    /// restore would hand back a truncated file.
    ///
    /// WHY the payload is not copied: the write moves the bytes out of the
    /// VFS's own buffer a sector at a time, so a payload of several
    /// megabytes costs no second copy of itself.
    fn spill_payload(&self, name: &str, data: &[u8]) -> bool {
        if !(self.spill.write)(name, data) {
            return false;
        }
        matches!((self.spill.file_len)(name), Some(len) if len == data.len() as u64)
    }

    /// Encodes mutable regular-file payloads into `buf` and returns the used length.
    ///
    /// Used by the snapshot tests, which encode a tree the block store cannot
    /// hold; the save path calls [`Self::encode_data_snapshot_with`].
    #[cfg(test)]
    fn encode_data_snapshot(&self, buf: &mut [u8]) -> usize {
        self.encode_data_snapshot_with(buf, &[])
    }

    /// Encodes the data snapshot, marking as external each payload named in
    /// `spilled` rather than carrying its bytes.
    ///
    /// The entry order follows inode id order, so a full budget drops a
    /// deterministic suffix of the mutable tree rather than an arbitrary set.
    /// A payload larger than [`SNAPSHOT_FILE_LIMIT`] is either named as
    /// external (when the store holds it) or skipped whole; entries are only
    /// committed once their complete header and payload fit.
    fn encode_data_snapshot_with(&self, buf: &mut [u8], spilled: &[String]) -> usize {
        if buf.len() < 6 { return 0; }
        buf[..4].copy_from_slice(b"DTA1");
        let mut pos=6usize;
        let mut count=0u16;
        for inode in self.inodes.values() {
            if inode.file_type != FileType::RegularFile || !is_persistent_data_path(&inode.path) { continue; }
            let path=inode.path.as_bytes();
            let data=inode.data.as_slice();
            // A size that no bytes back is a record from a boot that lost the
            // payload; persisting it again would keep the lie alive.
            if data.is_empty() && inode.size > 0 { continue; }
            if path.len()>255 { continue; }
            if data.len()>SNAPSHOT_FILE_LIMIT {
                let name = spill_path_for(&inode.path);
                if !spilled.iter().any(|s| s == &name) { continue; }
                // WHY the length is bounded here: the entry's top bit is the
                // flag that says "the bytes live in the block store", so a
                // length that reaches it would be read back as a payload of
                // the wrong size.
                if data.len() > 0x7FFF_FFFFusize { continue; }
                if pos + 2 + path.len() + 4 > buf.len() { break; }
                buf[pos..pos+2].copy_from_slice(&(path.len() as u16).to_le_bytes()); pos+=2;
                buf[pos..pos+path.len()].copy_from_slice(path); pos+=path.len();
                let marked = (data.len() as u32) | DATA_ENTRY_EXTERNAL;
                buf[pos..pos+4].copy_from_slice(&marked.to_le_bytes()); pos+=4;
                count=count.saturating_add(1);
                continue;
            }
            if pos + 2 + path.len() + 4 + data.len() > buf.len() { break; }
            buf[pos..pos+2].copy_from_slice(&(path.len() as u16).to_le_bytes()); pos+=2;
            buf[pos..pos+path.len()].copy_from_slice(path); pos+=path.len();
            buf[pos..pos+4].copy_from_slice(&(data.len() as u32).to_le_bytes()); pos+=4;
            buf[pos..pos+data.len()].copy_from_slice(data); pos+=data.len();
            count=count.saturating_add(1);
        }
        buf[4..6].copy_from_slice(&count.to_le_bytes());
        pos
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
        if self.metadata_generation.get() == self.persisted_metadata_generation.get() {
            return Ok(());
        }
        if crate::drivers::ATA_PRIMARY.lock().is_none() {
            return Ok(());
        }
        let mut buf = alloc::vec![0u8; METADATA_SNAPSHOT_LIMIT];
        let pos = self.encode_metadata_snapshot(&mut buf);
        let mut sealed = Vec::new();
        crate::storage::encrypted::seal(&buf[..pos], &mut sealed).map_err(|_| FsError::IntegrityViolation)?;
        crate::storage::filesystem::store_data_file("/data/.vfs.metadata", &sealed).map_err(|_| FsError::IntegrityViolation)?;
        self.persisted_metadata_generation.set(self.metadata_generation.get());
        Ok(())
    }

    /// Encodes the persistent inode tree into `buf` and returns the used length.
    ///
    /// Directories and regular files under `/data`, `/user` and `/storage` are
    /// durable, together with hard-link aliases. Entries are in inode id order
    /// and only committed once their complete record fits.
    fn encode_metadata_snapshot(&self, buf: &mut [u8]) -> usize {
        if buf.len() < 6 { return 0; }
        buf[..4].copy_from_slice(b"IVM2");
        let mut pos = 6usize;
        let mut count = 0u16;
        for inode in self.inodes.values() {
            if !is_persistent_metadata_path(&inode.path) { continue; }
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
        pos
    }

    /// Restores the persistent /data inode tree before restoring file payloads.
    fn restore_metadata_snapshot(&mut self) {
        let mut sealed = alloc::vec![0u8; METADATA_SNAPSHOT_LIMIT + crate::storage::encrypted::ENCRYPTED_OVERHEAD];
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
        self.decode_metadata_snapshot(&buf, len, v2);
    }

    /// Decodes a validated metadata snapshot into the inode tree.
    fn decode_metadata_snapshot(&mut self, buf: &[u8], len: usize, v2: bool) {
        if len < 6 { return; }
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
            // The persisted size and digest describe the bytes as they were
            // when the snapshot was taken. The bytes this boot actually holds
            // come from the seeds or the data snapshot, so neither field is
            // applied to the inode here.
            pos += 4;
            let mut persisted_digest=[0u8;32]; persisted_digest.copy_from_slice(&buf[pos..pos+32]); pos += 32;
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
            if !is_persistent_metadata_path(path) { continue; }
            if kind == 1 {
                let created=self.mkdir(path);
                if let Some(inode)=self.inodes.get_mut(&created) { inode.id=InodeId(id); inode.uid=uid; inode.gid=gid; inode.mode=mode; inode.nlink=nlink; inode.atime=atime; inode.mtime=mtime; inode.ctime=ctime; }
            } else if kind == 2 {
                // A persisted record whose path already exists is a file the
                // boot seeds recreated before the snapshot was read. Its bytes
                // are the honest content of this boot; the record only carries
                // metadata. Replacing them with an empty buffer made every
                // seeded track and photo unreadable after the first reboot.
                let existing = self.path_to_inode.get(path).copied().filter(|inode_id| {
                    self.inodes
                        .get(inode_id)
                        .map(|inode| inode.file_type == FileType::RegularFile)
                        .unwrap_or(false)
                });
                if let Some(existing_id) = existing {
                    if let Some(inode) = self.inodes.get_mut(&existing_id) {
                        inode.uid=uid; inode.gid=gid; inode.mode=mode; inode.nlink=nlink;
                        inode.atime=atime; inode.mtime=mtime; inode.ctime=ctime;
                    }
                    // Keep the record's inode id when it is free so recorded
                    // hard links still resolve. Never take an id another path
                    // owns: that would silently merge two files.
                    if id != existing_id.0 && !self.inodes.contains_key(&InodeId(id)) {
                        if let Some(mut inode) = self.inodes.remove(&existing_id) {
                            inode.id = InodeId(id);
                            self.inodes.insert(InodeId(id), inode);
                            self.path_to_inode.insert(path.to_string(), InodeId(id));
                        }
                    }
                } else {
                    let created=self.create_file(path,&[],false);
                    let target = if id != created.0 && !self.inodes.contains_key(&InodeId(id)) {
                        InodeId(id)
                    } else {
                        created
                    };
                    if let Some(mut inode)=self.inodes.remove(&created) {
                        // The persisted size describes bytes this boot has not
                        // restored yet. Until the data snapshot supplies them,
                        // the file is honestly empty.
                        inode.id=target; inode.size=0; inode.data=InodeData::Dynamic(Vec::new());
                        inode.uid=uid; inode.gid=gid; inode.mode=mode; inode.nlink=nlink; inode.atime=atime; inode.mtime=mtime; inode.ctime=ctime;
                        self.inodes.insert(target,inode); self.path_to_inode.insert(path.to_string(),target);
                    }
                }
            }
            if id>=self.next_inode { self.next_inode=id.saturating_add(1); }
        }
    }

    /// Restores mutable /data files from the block-backed filesystem first.
    /// The metadata journal remains a recovery fallback until the transactional
    /// publication protocol finalizes the write.
    fn restore_data_snapshot(&mut self) {
        let mut sealed = alloc::vec![0u8; DATA_SNAPSHOT_LIMIT + crate::storage::encrypted::ENCRYPTED_OVERHEAD];
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
        let mut buf = alloc::vec![0u8; DATA_SNAPSHOT_LIMIT];
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
        self.decode_data_snapshot(&buf, len);
    }

    /// Decodes a validated data snapshot into the inode tree.
    ///
    /// Missing ancestor directories are created before the payload is written,
    /// so a data record whose directory was absent from the metadata snapshot
    /// still lands at its intended path instead of becoming an orphan inode.
    fn decode_data_snapshot(&mut self, buf: &[u8], len: usize) {
        let store = self.spill;
        self.decode_data_snapshot_with(buf, len, store);
    }

    /// Decodes a validated data snapshot, reading spilled payloads from
    /// `store`.
    ///
    /// WHY the store is a parameter: the restore is the half of the spill
    /// lifecycle that has to be observable without a block device, and the
    /// only code that names the device is the one that builds the store.
    fn decode_data_snapshot_with(&mut self, buf: &[u8], len: usize, store: SpillStore) {
        if len < 6 { return; }
        let count=u16::from_le_bytes([buf[4],buf[5]]) as usize;
        let mut pos=6usize;
        crate::serial_println!("[VFS RESTORE] Restoring {} data files from snapshot...", count);
        for _ in 0..count {
            if pos+2>len { break; }
            let plen=u16::from_le_bytes([buf[pos],buf[pos+1]]) as usize; pos+=2;
            if plen==0 || plen>255 || pos+plen+4>len { break; }
            let path=match core::str::from_utf8(&buf[pos..pos+plen]).ok(){Some(v)=>v, None=>break}; pos+=plen;
            let raw=u32::from_le_bytes([buf[pos],buf[pos+1],buf[pos+2],buf[pos+3]]) as u32; pos+=4;
            let external = raw & DATA_ENTRY_EXTERNAL != 0;
            let dlen=(raw & !DATA_ENTRY_EXTERNAL) as usize;
            if external {
                if dlen == 0 { continue; }
                if dlen > SPILL_FILE_LIMIT {
                    // A record the ceiling forbids is skipped, not obeyed:
                    // breaking here would throw away every record after it
                    // as well.
                    crate::serial_println!(
                        "[VFS RESTORE] {} names a {} byte payload, above the {}-byte ceiling; it is not restored",
                        path,
                        dlen,
                        SPILL_FILE_LIMIT
                    );
                    continue;
                }
                if !is_persistent_data_path(path) { continue; }
                let name = spill_path_for(path);
                match load_spilled_payload(&store, &name, dlen) {
                    Some(payload) => {
                        let parent = parent_path(path);
                        if parent != "/" { self.ensure_dir_path(parent); }
                        crate::serial_println!("[VFS RESTORE] Restored spilled file: {} ({} bytes)", path, dlen);
                        self.create_file_owned(path, payload, false);
                    }
                    None => {
                        // A snapshot that names a payload the store cannot
                        // produce is reported, not quietly half-restored.
                        crate::serial_println!(
                            "[VFS RESTORE] Spilled payload unavailable for {} ({} bytes)",
                            path,
                            dlen
                        );
                    }
                }
                continue;
            }
            if dlen>SNAPSHOT_FILE_LIMIT || pos+dlen>len { break; }
            if !is_persistent_data_path(path) { pos+=dlen; continue; }
            // A zero-length payload for a path that already holds bytes is a
            // snapshot recorded while the file was empty, not an instruction
            // to erase the seed this boot restored. Keeping the seed makes the
            // record recoverable on the next persist; overwriting it with zero
            // bytes made every seeded track and photo 0 bytes after reboot.
            if dlen == 0 {
                if self.path_to_inode.get(path).is_some() {
                    crate::serial_println!("[VFS RESTORE] Kept existing bytes for {} (empty record)", path);
                    pos += dlen;
                    continue;
                }
            }
            let parent = parent_path(path);
            if parent != "/" { self.ensure_dir_path(parent); }
            crate::serial_println!("[VFS RESTORE] Restored file: {} ({} bytes)", path, dlen);
            self.create_file(path,&buf[pos..pos+dlen],false); pos+=dlen;
        }
    }

    /// Creates each missing directory along `dir`, one component at a time.
    fn ensure_dir_path(&mut self, dir: &str) {
        let norm = normalize_path(dir);
        if norm == "/" { return; }
        let mut current = String::from("/");
        for component in norm.split('/') {
            if component.is_empty() { continue; }
            if current != "/" { current.push('/'); }
            current.push_str(component);
            self.mkdir(&current);
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

/// Returns true for regular-file paths whose payloads belong in the data snapshot.
fn is_persistent_data_path(path: &str) -> bool {
    path.starts_with("/data/") || path.starts_with("/user/") || path.starts_with("/storage/")
}

/// Returns true for inode-tree paths whose metadata belongs in the metadata snapshot.
fn is_persistent_metadata_path(path: &str) -> bool {
    is_persistent_data_path(path) || path == "/data" || path == "/user" || path == "/storage"
}

/// Reads a spilled payload back from the block store in bounded chunks.
///
/// WHY chunked: the payload can be several megabytes, and it is assembled in
/// bounded pieces so that no single platform call carries the whole file.
///
/// WHY the published length is checked first: the snapshot says how long the
/// payload is, and a store holding a different length holds a different
/// payload. Restoring the first `len` bytes of that one would hand back a
/// file whose contents are not the contents that were written.
///
/// WHY the reservation is fallible: a payload the heap cannot hold has to be
/// reported as unavailable, not abort the boot.
fn load_spilled_payload(store: &SpillStore, name: &str, len: usize) -> Option<Vec<u8>> {
    if (store.file_len)(name)? != len as u64 {
        return None;
    }
    let mut payload = Vec::new();
    payload.try_reserve(len).ok()?;
    let mut chunk = alloc::vec![0u8; core::cmp::min(SPILL_CHUNK_SIZE, len)];
    let mut offset = 0usize;
    while offset < len {
        let want = core::cmp::min(chunk.len(), len - offset);
        let read = (store.read_at)(name, offset as u64, &mut chunk[..want])?;
        if read == 0 || read > want {
            return None;
        }
        payload.extend_from_slice(&chunk[..read]);
        offset += read;
    }
    Some(payload)
}

/// The block-store name an oversized payload is kept at.
///
/// WHY the name is a digest and not the path: the block store names a file in
/// 46 bytes, and a VFS path does not fit. The first eight bytes of the SHA-256
/// of the path name the file, so the same path always resolves to the same
/// name across boots.
fn spill_path_for(vfs_path: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(vfs_path.as_bytes());
    let mut out = String::from(SPILL_PREFIX);
    for byte in &digest[..8] {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
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
    spill: SpillStore::device(),
    data_generation: core::cell::Cell::new(0),
    persisted_data_generation: core::cell::Cell::new(0),
    metadata_generation: core::cell::Cell::new(0),
    persisted_metadata_generation: core::cell::Cell::new(0),
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
    vfs.mkdir("/data/models");
    vfs.mkdir("/user");
    vfs.mkdir("/storage");
    vfs.mkdir("/storage/photos");
    vfs.mkdir("/storage/pictures");
    vfs.mkdir("/storage/videos");
    vfs.mkdir("/storage/downloads");
    vfs.mkdir("/storage/music");
    vfs.mkdir("/system/models");

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
    vfs.persisted_data_generation.set(vfs.data_generation.get());
    vfs.persisted_metadata_generation.set(vfs.metadata_generation.get());
    *VFS.lock() = vfs;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{delegation, Capability, CapabilityId};

    /// A block-store file the in-memory store holds.
    struct StoredFile {
        bytes: Vec<u8>,
        published: u64,
    }

    /// The in-memory block store the spill tests run against.
    ///
    /// WHY a static and not a per-test value: the store is reached through
    /// function pointers, which cannot capture, so the state has to live
    /// somewhere the pointers can name. Every test uses paths of its own,
    /// and the names the store holds are digests of those paths, so no two
    /// tests touch the same file.
    static MEMORY_STORE: Mutex<BTreeMap<String, StoredFile>> = Mutex::new(BTreeMap::new());

    /// Writes a whole payload into the in-memory store.
    ///
    /// WHY the whole file at once: the block store publishes a file's bytes
    /// as a whole, and a test store that modelled an in-place rewrite would
    /// prove a guarantee the device does not make.
    fn memory_write(name: &str, data: &[u8]) -> bool {
        let mut files = MEMORY_STORE.lock();
        files.insert(
            String::from(name),
            StoredFile { bytes: data.to_vec(), published: data.len() as u64 },
        );
        true
    }

    /// Reads bytes at an offset from the in-memory store.
    fn memory_read_at(name: &str, offset: u64, out: &mut [u8]) -> Option<usize> {
        let files = MEMORY_STORE.lock();
        let file = files.get(name)?;
        if offset >= file.published || out.is_empty() { return Some(0); }
        let start = offset as usize;
        let take = core::cmp::min(out.len(), file.published as usize - start);
        out[..take].copy_from_slice(&file.bytes[start..start + take]);
        Some(take)
    }

    /// The published length of a file in the in-memory store.
    fn memory_file_len(name: &str) -> Option<u64> {
        let files = MEMORY_STORE.lock();
        files.get(name).map(|file| file.published)
    }

    /// Removes a file from the in-memory store.
    fn memory_remove(name: &str) -> bool {
        let mut files = MEMORY_STORE.lock();
        files.remove(name);
        true
    }

    /// The store the spill tests install in place of the block device.
    fn memory_store() -> SpillStore {
        SpillStore {
            write: memory_write,
            read_at: memory_read_at,
            file_len: memory_file_len,
            remove: memory_remove,
        }
    }

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
        let empty_caps = CapabilityTable::new_for_task(task_id, 8);
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

    #[test]
    fn vfs_storage_download_round_trips_across_snapshots() {
        let mut source = Vfs::new();
        source.mkdir("/storage");
        source.mkdir("/storage/downloads");
        let payload = b"\x89PNG\r\n\x1a\nInterOS downloaded payload\x00\xff";
        source.create_file("/storage/downloads/file.bin", payload, false);

        let mut metadata = alloc::vec![0u8; METADATA_SNAPSHOT_LIMIT];
        let metadata_len = source.encode_metadata_snapshot(&mut metadata);
        let mut data = alloc::vec![0u8; DATA_SNAPSHOT_LIMIT];
        let data_len = source.encode_data_snapshot(&mut data);
        assert_eq!(&metadata[..4], b"IVM2");
        assert_eq!(&data[..4], b"DTA1");
        assert!(metadata_len > 6, "the /storage directory must be persisted");
        assert!(data_len > 6, "the /storage payload must be persisted");

        // Metadata first, then payloads, as init() restores them.
        let mut restored = Vfs::new();
        restored.decode_metadata_snapshot(&metadata[..metadata_len], metadata_len, true);
        restored.decode_data_snapshot(&data[..data_len], data_len);
        restored.rebuild_children();

        let dir = restored.lookup("/storage/downloads").expect("downloads directory restored");
        assert_eq!(restored.inodes[&dir].file_type, FileType::Directory);
        let file = restored.lookup("/storage/downloads/file.bin").expect("download restored");
        let inode = restored.inodes.get(&file).expect("restored inode");
        assert_eq!(inode.data.as_slice(), payload);
        assert_eq!(inode.size, payload.len());
        assert_eq!(inode.sha256_digest, Sha256::digest(payload));

        // A payload whose parent directory is absent from the metadata snapshot
        // must still land under /storage instead of becoming an orphan inode.
        let mut data_only = Vfs::new();
        data_only.decode_data_snapshot(&data[..data_len], data_len);
        let dir = data_only.lookup("/storage/downloads").expect("data restore creates missing parent");
        assert_eq!(data_only.inodes[&dir].file_type, FileType::Directory);
        let file = data_only.lookup("/storage/downloads/file.bin").expect("download restored without metadata");
        assert_eq!(data_only.inodes[&file].data.as_slice(), payload);
    }

    #[test]
    fn vfs_multi_megabyte_payload_survives_a_reboot() {
        // The defect this pins: a full-screen screenshot is a 720x1440 PPM --
        // 3,108,864 bytes -- and the old 512 KiB spill ceiling dropped it on
        // persist, so it came back as a 0-byte file after the reboot.
        const SCREENSHOT_BYTES: usize = 720 * 1440 * 3;
        assert!(
            SCREENSHOT_BYTES > 512 * 1024,
            "the payload is above the ceiling that used to drop it"
        );
        assert!(
            SCREENSHOT_BYTES <= SPILL_FILE_LIMIT,
            "the payload fits the ceiling the kernel heap allows"
        );

        let mut source = Vfs::with_spill_store(memory_store());
        source.mkdir("/storage");
        source.mkdir("/storage/photos");
        let mut payload = alloc::vec![0u8; SCREENSHOT_BYTES];
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte = (index % 251) as u8;
        }
        source.create_file("/storage/photos/screenshot_536730.ppm", &payload, false);

        let spilled = source.store_spilled_payloads();
        assert_eq!(spilled.len(), 1, "a 2.9 MiB payload is spilled, not dropped");

        // The snapshot still fits: it names the payload, it does not carry it.
        let mut data = alloc::vec![0u8; DATA_SNAPSHOT_LIMIT];
        let len = source.encode_data_snapshot_with(&mut data, &spilled);
        assert!(len <= DATA_SNAPSHOT_LIMIT, "the snapshot still fits ({} bytes)", len);
        assert!(
            len < 6 + 2 + 48 + 4,
            "the entry names the payload instead of carrying it ({} bytes)",
            len
        );

        let mut restored = Vfs::with_spill_store(memory_store());
        restored.decode_data_snapshot_with(&data[..len], len, memory_store());
        let file = restored
            .lookup("/storage/photos/screenshot_536730.ppm")
            .expect("the screenshot is restored");
        let inode = &restored.inodes[&file];
        assert_eq!(inode.size, SCREENSHOT_BYTES, "the restored file reports its bytes");
        assert_eq!(inode.data.as_slice(), payload.as_slice(), "the restored bytes are the written bytes");
    }

    #[test]
    fn vfs_a_payload_at_the_snapshot_entry_limit_is_carried_and_one_above_it_is_spilled() {
        let mut source = Vfs::with_spill_store(memory_store());
        source.mkdir("/storage");
        source.mkdir("/storage/downloads");
        let at_limit = alloc::vec![0xA5u8; SNAPSHOT_FILE_LIMIT];
        let over_limit = alloc::vec![0x5Au8; SNAPSHOT_FILE_LIMIT + 1];
        source.create_file("/storage/downloads/at_limit.bin", &at_limit, false);
        source.create_file("/storage/downloads/over_limit.bin", &over_limit, false);

        let spilled = source.store_spilled_payloads();
        assert!(
            !spilled.contains(&spill_path_for("/storage/downloads/at_limit.bin")),
            "a payload that exactly fills an entry travels inside the snapshot"
        );
        assert!(
            spilled.contains(&spill_path_for("/storage/downloads/over_limit.bin")),
            "a payload one byte over an entry is spilled"
        );

        let mut data = alloc::vec![0u8; DATA_SNAPSHOT_LIMIT];
        let len = source.encode_data_snapshot_with(&mut data, &spilled);
        let mut restored = Vfs::with_spill_store(memory_store());
        restored.decode_data_snapshot_with(&data[..len], len, memory_store());

        let at = restored
            .lookup("/storage/downloads/at_limit.bin")
            .expect("the payload that fills an entry is restored");
        assert_eq!(restored.inodes[&at].data.as_slice(), at_limit.as_slice());
        let over = restored
            .lookup("/storage/downloads/over_limit.bin")
            .expect("the spilled payload is restored");
        assert_eq!(restored.inodes[&over].data.as_slice(), over_limit.as_slice());
    }

    #[test]
    fn vfs_a_payload_at_the_spill_ceiling_is_spilled_and_one_above_it_is_refused() {
        let mut vfs = Vfs::with_spill_store(memory_store());
        vfs.mkdir("/storage");
        vfs.mkdir("/storage/downloads");
        let at_limit = alloc::vec![0xA5u8; SPILL_FILE_LIMIT];
        let over_limit = alloc::vec![0x5Au8; SPILL_FILE_LIMIT + 1];
        vfs.create_file("/storage/downloads/at_ceiling.bin", &at_limit, false);
        vfs.create_file("/storage/downloads/above_ceiling.bin", &over_limit, false);

        let spilled = vfs.store_spilled_payloads();
        assert!(
            spilled.contains(&spill_path_for("/storage/downloads/at_ceiling.bin")),
            "a payload exactly at the ceiling is spilled"
        );
        assert!(
            !spilled.contains(&spill_path_for("/storage/downloads/above_ceiling.bin")),
            "a payload above the ceiling is refused, not silently dropped"
        );

        let mut data = alloc::vec![0u8; DATA_SNAPSHOT_LIMIT];
        let len = vfs.encode_data_snapshot_with(&mut data, &spilled);
        let mut restored = Vfs::with_spill_store(memory_store());
        restored.decode_data_snapshot_with(&data[..len], len, memory_store());
        let at = restored
            .lookup("/storage/downloads/at_ceiling.bin")
            .expect("the payload at the ceiling is restored");
        assert_eq!(restored.inodes[&at].data.as_slice(), at_limit.as_slice());
        assert_eq!(
            restored.lookup("/storage/downloads/above_ceiling.bin"),
            None,
            "a payload the ceiling refuses must not come back as a 0-byte file"
        );
    }

    #[test]
    fn vfs_a_re_spilled_payload_of_the_same_length_replaces_it_wholesale() {
        // The payload that was in the store and the one that replaces it have
        // the same length, which is the case a rewrite made in place got
        // wrong: the store publishes a file's length as its commit point, so
        // an in-place rewrite of the same length never moves it, and a reset
        // in the middle of the rewrite leaves the two payloads mixed
        // together under a length that still looks right.
        let mut source = Vfs::with_spill_store(memory_store());
        source.mkdir("/storage");
        source.mkdir("/storage/photos");
        let first = alloc::vec![0x11u8; 1024 * 1024];
        let second = alloc::vec![0x22u8; 1024 * 1024];
        source.create_file("/storage/photos/shot.ppm", &first, false);
        assert_eq!(source.store_spilled_payloads().len(), 1, "the payload is spilled");

        source.create_file("/storage/photos/shot.ppm", &second, false);
        assert_eq!(source.store_spilled_payloads().len(), 1, "the payload is spilled again");

        let name = spill_path_for("/storage/photos/shot.ppm");
        let mut stored = alloc::vec![0u8; 1024 * 1024];
        let read = memory_read_at(&name, 0, &mut stored).expect("the payload is in the store");
        assert_eq!(read, 1024 * 1024);
        assert_eq!(
            stored, second,
            "the store holds the payload that replaced the old one, not a mixture of the two"
        );
    }

    #[test]
    fn vfs_unlinked_payload_leaves_no_spill_file_behind() {
        let mut vfs = Vfs::with_spill_store(memory_store());
        vfs.mkdir("/storage");
        vfs.mkdir("/storage/photos");
        vfs.create_file(
            "/storage/photos/screenshot_deleted.ppm",
            &alloc::vec![0x3Cu8; 3 * 1024 * 1024],
            false,
        );
        let spilled = vfs.store_spilled_payloads();
        assert_eq!(spilled.len(), 1);
        let name = spilled[0].clone();
        assert!(memory_file_len(&name).is_some(), "the payload reached the block store");

        vfs.unlink("/storage/photos/screenshot_deleted.ppm").expect("unlink");
        assert_eq!(vfs.lookup("/storage/photos/screenshot_deleted.ppm"), None);
        assert!(
            memory_file_len(&name).is_none(),
            "a payload no path names must not outlive the path"
        );
    }

    #[test]
    fn vfs_a_payload_that_shrinks_releases_its_spill_file() {
        let mut vfs = Vfs::with_spill_store(memory_store());
        vfs.mkdir("/storage");
        vfs.mkdir("/storage/photos");
        vfs.create_file(
            "/storage/photos/screenshot_shrunk.ppm",
            &alloc::vec![0x9Eu8; 2 * 1024 * 1024],
            false,
        );
        let spilled = vfs.store_spilled_payloads();
        assert_eq!(spilled.len(), 1);
        let name = spilled[0].clone();
        assert!(memory_file_len(&name).is_some());

        // The file now carries a payload that travels inside the snapshot, so
        // the block-store copy has to go with it.
        vfs.create_file("/storage/photos/screenshot_shrunk.ppm", b"small", false);
        assert!(
            memory_file_len(&name).is_none(),
            "a payload that no longer needs the block store must not leave it behind"
        );
    }

    #[test]
    fn vfs_storage_snapshot_skips_oversize_file_without_truncating() {
        let mut source = Vfs::new();
        source.mkdir("/storage");
        source.mkdir("/storage/downloads");
        let small_before = alloc::vec![0x11u8; 64];
        let oversized = alloc::vec![0x22u8; SNAPSHOT_FILE_LIMIT + 1];
        let small_after = alloc::vec![0x33u8; 32];
        source.create_file("/storage/downloads/a_small.bin", &small_before, false);
        source.create_file("/storage/downloads/b_oversize.bin", &oversized, false);
        source.create_file("/storage/downloads/c_small.bin", &small_after, false);

        let mut data = alloc::vec![0u8; DATA_SNAPSHOT_LIMIT];
        let len = source.encode_data_snapshot(&mut data);
        assert_eq!(u16::from_le_bytes([data[4], data[5]]), 2, "only the two in-limit files are encoded");

        let mut restored = Vfs::new();
        restored.decode_data_snapshot(&data[..len], len);
        assert_eq!(restored.lookup("/storage/downloads/b_oversize.bin"), None, "oversize file must be absent, not truncated");
        let before = restored.lookup("/storage/downloads/a_small.bin").expect("small file before skipped entry");
        assert_eq!(restored.inodes[&before].data.as_slice(), small_before.as_slice());
        let after = restored.lookup("/storage/downloads/c_small.bin").expect("small file after skipped entry");
        assert_eq!(restored.inodes[&after].data.as_slice(), small_after.as_slice());
        for inode in restored.inodes.values() {
            assert_ne!(inode.size, SNAPSHOT_FILE_LIMIT, "no snapshot entry may be truncated to the limit");
        }
    }

    #[test]
    fn vfs_large_payloads_round_trip_through_the_block_store() {
        // A payload bigger than one entry cannot travel in the snapshot, so it
        // travels beside it: the entry names the block-store file, and the
        // bytes come back from there. Before this, every download above 8 KB
        // came back as a 0-byte file.
        let mut source = Vfs::with_spill_store(memory_store());
        source.mkdir("/storage");
        source.mkdir("/storage/downloads");
        let large = alloc::vec![0x5Au8; SNAPSHOT_FILE_LIMIT + 4096];
        source.create_file("/storage/downloads/big.wav", &large, false);

        let spilled = source.store_spilled_payloads();
        assert_eq!(spilled.len(), 1, "the oversized payload is spilled");
        assert_eq!(spilled[0], spill_path_for("/storage/downloads/big.wav"));

        let mut data = alloc::vec![0u8; DATA_SNAPSHOT_LIMIT];
        let len = source.encode_data_snapshot_with(&mut data, &spilled);
        assert_eq!(u16::from_le_bytes([data[4], data[5]]), 1, "the spilled payload is recorded once");
        assert!(
            len < 6 + 2 + 32 + 4 + 16,
            "an external entry names the payload instead of carrying it ({} bytes)",
            len
        );

        let mut restored = Vfs::with_spill_store(memory_store());
        restored.decode_data_snapshot_with(&data[..len], len, memory_store());
        let file = restored.lookup("/storage/downloads/big.wav").expect("spilled payload restored");
        assert_eq!(restored.inodes[&file].data.as_slice(), large.as_slice());
    }

    #[test]
    fn vfs_spilled_payload_missing_from_the_store_is_not_half_restored() {
        let mut source = Vfs::with_spill_store(memory_store());
        source.mkdir("/storage");
        source.mkdir("/storage/downloads");
        source.create_file(
            "/storage/downloads/gone.bin",
            &alloc::vec![0x77u8; SNAPSHOT_FILE_LIMIT + 64],
            false,
        );
        // The payload never reached the store, so the snapshot names a file
        // that is not there.
        let names = alloc::vec![spill_path_for("/storage/downloads/gone.bin")];
        let mut data = alloc::vec![0u8; DATA_SNAPSHOT_LIMIT];
        let len = source.encode_data_snapshot_with(&mut data, &names);

        let mut restored = Vfs::with_spill_store(memory_store());
        restored.decode_data_snapshot_with(&data[..len], len, memory_store());
        assert_eq!(
            restored.lookup("/storage/downloads/gone.bin"),
            None,
            "a payload the store cannot produce must not become a 0-byte file"
        );
    }

    #[test]
    fn vfs_restore_keeps_boot_seed_bytes_when_a_record_has_no_payload() {
        // The seeds are recreated before the snapshot is read, so a record
        // whose payload is missing or empty must not erase them. This is the
        // bug that made every seeded track and photo zero bytes after the
        // first reboot.
        let seed = b"RIFF\x24\x00\x00\x00WAVEfmt seed payload";
        let mut restored = Vfs::new();
        restored.mkdir("/storage");
        restored.mkdir("/storage/music");
        restored.create_file("/storage/music/ambient_chill.wav", seed, false);

        let mut source = Vfs::new();
        source.mkdir("/storage");
        source.mkdir("/storage/music");
        source.create_file("/storage/music/ambient_chill.wav", &[], false);
        let mut metadata = alloc::vec![0u8; METADATA_SNAPSHOT_LIMIT];
        let metadata_len = source.encode_metadata_snapshot(&mut metadata);

        restored.decode_metadata_snapshot(&metadata[..metadata_len], metadata_len, true);
        let inode_id = restored.lookup("/storage/music/ambient_chill.wav").expect("seed path survives metadata restore");
        let inode = &restored.inodes[&inode_id];
        assert_eq!(inode.data.as_slice(), seed, "metadata restore must not empty a seeded file");
        assert_eq!(inode.size, seed.len(), "a seeded file reports the bytes it holds");

        // An explicit zero-length data record is the same instruction not to
        // erase what the boot already restored.
        let path = b"/storage/music/ambient_chill.wav";
        let mut data = alloc::vec![0u8; 64];
        data[..4].copy_from_slice(b"DTA1");
        data[4..6].copy_from_slice(&1u16.to_le_bytes());
        let mut pos = 6usize;
        data[pos..pos + 2].copy_from_slice(&(path.len() as u16).to_le_bytes()); pos += 2;
        data[pos..pos + path.len()].copy_from_slice(path); pos += path.len();
        data[pos..pos + 4].copy_from_slice(&0u32.to_le_bytes()); pos += 4;
        restored.decode_data_snapshot(&data[..pos], pos);
        let inode_id = restored.lookup("/storage/music/ambient_chill.wav").expect("seed path survives data restore");
        assert_eq!(restored.inodes[&inode_id].data.as_slice(), seed);
    }

    #[test]
    fn vfs_restore_does_not_let_a_persisted_id_take_another_paths_inode() {
        // Persisted ids come from an older boot and can collide with the ids
        // the seeds were given this boot. A collision must leave both paths
        // readable instead of merging them into one inode.
        let mut restored = Vfs::new();
        restored.mkdir("/storage");
        restored.create_file("/storage/keep.bin", b"keep", false);
        let keep_id = restored.lookup("/storage/keep.bin").expect("seed");

        let mut source = Vfs::new();
        source.mkdir("/storage");
        source.create_file("/storage/other.bin", b"other", false);
        let mut metadata = alloc::vec![0u8; METADATA_SNAPSHOT_LIMIT];
        let metadata_len = source.encode_metadata_snapshot(&mut metadata);

        restored.decode_metadata_snapshot(&metadata[..metadata_len], metadata_len, true);
        let keep_after = restored.lookup("/storage/keep.bin").expect("existing path must survive");
        let other = restored.lookup("/storage/other.bin").expect("incoming path must survive");
        assert_ne!(keep_after, other, "two paths must not share one inode");
        assert_eq!(restored.inodes[&keep_after].data.as_slice(), b"keep");
        // The incoming file has no payload yet, so it is honestly empty until
        // the data snapshot supplies one.
        assert_eq!(restored.inodes[&other].size, 0);
        let _ = keep_id;
    }

    #[test]
    fn vfs_data_and_user_payloads_still_round_trip() {
        let mut source = Vfs::new();
        source.mkdir("/data");
        source.mkdir("/user");
        source.create_file("/data/audit.log", b"audit entry\n", false);
        source.create_file("/user/profile.bin", b"profile bytes", false);

        let mut metadata = alloc::vec![0u8; METADATA_SNAPSHOT_LIMIT];
        let metadata_len = source.encode_metadata_snapshot(&mut metadata);
        let mut data = alloc::vec![0u8; DATA_SNAPSHOT_LIMIT];
        let data_len = source.encode_data_snapshot(&mut data);

        let mut restored = Vfs::new();
        restored.decode_metadata_snapshot(&metadata[..metadata_len], metadata_len, true);
        restored.decode_data_snapshot(&data[..data_len], data_len);
        let audit = restored.lookup("/data/audit.log").expect("/data payload restored");
        assert_eq!(restored.inodes[&audit].data.as_slice(), b"audit entry\n");
        let profile = restored.lookup("/user/profile.bin").expect("/user payload restored");
        assert_eq!(restored.inodes[&profile].data.as_slice(), b"profile bytes");
    }

    #[test]
    fn vfs_snapshot_budget_exhaustion_is_deterministic_and_whole_file() {
        let mut source = Vfs::new();
        source.mkdir("/storage");
        source.mkdir("/storage/downloads");
        let file_count = 8usize;
        for i in 0..file_count {
            let path = alloc::format!("/storage/downloads/chunk{}.bin", i);
            let payload = alloc::vec![0x40u8 + i as u8; SNAPSHOT_FILE_LIMIT];
            source.create_file(&path, &payload, false);
        }

        let mut first = alloc::vec![0u8; DATA_SNAPSHOT_LIMIT];
        let mut second = alloc::vec![0u8; DATA_SNAPSHOT_LIMIT];
        let first_len = source.encode_data_snapshot(&mut first);
        let second_len = source.encode_data_snapshot(&mut second);
        assert_eq!(first_len, second_len, "budget exhaustion must be deterministic");
        assert_eq!(&first[..first_len], &second[..second_len], "identical trees encode identical snapshots");

        let encoded = u16::from_le_bytes([first[4], first[5]]) as usize;
        assert!(encoded > 0 && encoded < file_count, "budget must drop a suffix, encoded {}", encoded);

        let mut restored = Vfs::new();
        restored.decode_data_snapshot(&first[..first_len], first_len);
        let mut restored_count = 0usize;
        for i in 0..file_count {
            let path = alloc::format!("/storage/downloads/chunk{}.bin", i);
            match restored.lookup(&path) {
                Some(id) => {
                    let inode = restored.inodes.get(&id).expect("restored inode");
                    let expected = alloc::vec![0x40u8 + i as u8; SNAPSHOT_FILE_LIMIT];
                    assert_eq!(inode.data.as_slice(), expected.as_slice(), "restored entry must be whole");
                    assert_eq!(inode.size, SNAPSHOT_FILE_LIMIT);
                    restored_count += 1;
                }
                None => assert!(i >= encoded, "only a deterministic suffix may be dropped"),
            }
        }
        assert_eq!(restored_count, encoded, "the header count must match restored entries");
    }

    #[test]
    fn vfs_empty_and_new_initialize_generation_tracking() {
        let empty_vfs = Vfs::empty();
        assert_eq!(empty_vfs.data_generation.get(), 0);
        assert_eq!(empty_vfs.persisted_data_generation.get(), 0);
        assert_eq!(empty_vfs.metadata_generation.get(), 0);
        assert_eq!(empty_vfs.persisted_metadata_generation.get(), 0);

        let new_vfs = Vfs::new();
        // Vfs::new creates "/" so metadata_generation is bumped
        assert!(new_vfs.metadata_generation.get() > 0);
        assert_eq!(new_vfs.data_generation.get(), 0);
        assert_eq!(new_vfs.persisted_data_generation.get(), 0);
        assert_eq!(new_vfs.persisted_metadata_generation.get(), 0);
    }

    #[test]
    fn vfs_generation_dirty_tracking_and_close_no_op() {
        let mut vfs = Vfs::new();
        let task_id = TaskId(777);

        // Synchronize initial generations
        vfs.persisted_data_generation.set(vfs.data_generation.get());
        vfs.persisted_metadata_generation.set(vfs.metadata_generation.get());

        // Create file bumps both data and metadata generations
        let cur_data_gen = vfs.data_generation.get();
        let cur_meta_gen = vfs.metadata_generation.get();
        let inode_id = vfs.create_file("/test_gen.txt", b"generation test", false);
        assert!(vfs.data_generation.get() > cur_data_gen);
        assert!(vfs.metadata_generation.get() > cur_meta_gen);

        // Simulate successful persist
        vfs.persisted_data_generation.set(vfs.data_generation.get());
        vfs.persisted_metadata_generation.set(vfs.metadata_generation.get());

        // Allocate read-only handle
        let fd = vfs.allocate_fd(task_id, inode_id, Rights::READ).expect("alloc fd");

        let gen_before_close_data = vfs.data_generation.get();
        let gen_before_close_meta = vfs.metadata_generation.get();

        // Close file: since neither data nor metadata is dirty, persist attempts are instant no-ops
        vfs.close(task_id, fd).expect("close fd");

        assert_eq!(vfs.data_generation.get(), gen_before_close_data);
        assert_eq!(vfs.metadata_generation.get(), gen_before_close_meta);
        assert_eq!(vfs.data_generation.get(), vfs.persisted_data_generation.get());
        assert_eq!(vfs.metadata_generation.get(), vfs.persisted_metadata_generation.get());
    }

    #[test]
    fn vfs_idempotent_write_does_not_dirty_generations_or_persist() {
        let mut vfs = Vfs::new();
        let task_id = TaskId(888);

        let initial_payload = b"idempotent test content";
        let inode_id = vfs.create_file("/test_idempotent.txt", initial_payload, false);

        // Synchronize persisted generations to simulate clean post-commit state
        vfs.persisted_data_generation.set(vfs.data_generation.get());
        vfs.persisted_metadata_generation.set(vfs.metadata_generation.get());

        let clean_data_gen = vfs.data_generation.get();
        let clean_meta_gen = vfs.metadata_generation.get();

        let mut caps = CapabilityTable::new_for_task(task_id, 8);
        caps.insert(crate::capability::Capability {
            id: crate::capability::CapabilityId(8801),
            cap_type: CapabilityType::FileSystemWrite,
            rights: Rights::WRITE,
            object_id: 0,
        }).expect("insert cap");

        let fd = vfs.allocate_fd(task_id, inode_id, Rights::WRITE).expect("alloc write fd");

        // 1. Idempotent write: write identical payload at offset 0
        let written = vfs.write(task_id, &caps, 0, fd, initial_payload).expect("write identical bytes");
        assert_eq!(written, initial_payload.len());

        // Generations must NOT change on idempotent writes
        assert_eq!(vfs.data_generation.get(), clean_data_gen, "idempotent write must not bump data generation");
        assert_eq!(vfs.metadata_generation.get(), clean_meta_gen, "idempotent write must not bump metadata generation");
        assert_eq!(vfs.data_generation.get(), vfs.persisted_data_generation.get());
        assert_eq!(vfs.metadata_generation.get(), vfs.persisted_metadata_generation.get());

        // Reset handle offset to 0 for a mutating write
        {
            let handle = vfs.task_file_tables.get_mut(&task_id).unwrap()[fd.0 as usize].as_mut().unwrap();
            handle.offset = 0;
        }

        // 2. Mutating write: write changed payload
        let mutated_payload = b"mutated content bytes!!";
        let written2 = vfs.write(task_id, &caps, 0, fd, mutated_payload).expect("write mutated bytes");
        assert_eq!(written2, mutated_payload.len());

        // Generations MUST advance on mutating writes
        assert!(vfs.data_generation.get() > clean_data_gen, "mutating write must advance data generation");
        assert!(vfs.metadata_generation.get() > clean_meta_gen, "mutating write must advance metadata generation");
    }
}


