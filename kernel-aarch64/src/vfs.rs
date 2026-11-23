// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Virtual Filesystem (VFS) for sovereign userspace tasks (AArch64).
//!
//! Provides capability-governed hierarchical filesystem access without ambient authority.

extern crate alloc;
use crate::capability::{CapabilityTable, CapabilityType, Rights};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

pub const MAX_FD_PER_TASK: usize = 16;
pub const FILE_STAT_BYTES: usize = 96;

#[derive(Clone)]
pub struct VfsNode {
    pub inode: u64,
    pub path: String,
    pub data: Vec<u8>,
    pub is_dir: bool,
    pub uid: u32,
    pub gid: u32,
    pub mode: u16,
    pub mtime: u64,
    pub file_type: u32, // 1=RegularFile, 2=ReadOnlySystem, 3=Directory, 4=DeviceNode
    pub read_only: bool,
}

#[derive(Clone, Copy)]
pub struct OpenFile {
    pub inode: u64,
    pub offset: usize,
    pub is_write: bool,
}

pub struct Vfs {
    nodes: BTreeMap<String, VfsNode>,
    next_inode: u64,
    descriptors: BTreeMap<u32, BTreeMap<u32, OpenFile>>,
    next_fd: BTreeMap<u32, u32>,
}

impl Vfs {
    pub const fn empty() -> Self {
        Self {
            nodes: BTreeMap::new(),
            next_inode: 1,
            descriptors: BTreeMap::new(),
            next_fd: BTreeMap::new(),
        }
    }

    pub fn new() -> Self {
        let mut vfs = Self::empty();
        vfs.init_defaults();
        vfs
    }

    pub fn ensure_initialized(&mut self) {
        if self.nodes.is_empty() {
            self.init_defaults();
            if interos_kernel_core::storage::persistent::is_block_device_bound() {
                self.restore_persistent_snapshot();
            }
        }
    }

    fn init_defaults(&mut self) {
        self.mkdir_internal("/");
        self.mkdir_internal("/system");
        self.mkdir_internal("/system/bin");
        self.mkdir_internal("/system/etc");
        self.mkdir_internal("/packages");
        self.mkdir_internal("/data");
        self.mkdir_internal("/data/models");
        self.mkdir_internal("/storage");
        self.mkdir_internal("/storage/shared");
        self.mkdir_internal("/tmp");
        self.mkdir_internal("/dev");

        self.add_system_file(
            "/system/etc/os-release",
            b"NAME=\"InterOS\"\nVERSION=\"1.0\"\nID=interos\n",
        );
        self.add_system_file("/system/etc/hostname", b"interos-phone\n");

        self.add_package_file(
            "/packages/org.interos.calculator.rapp",
            b"RAPP\x02\x00\x00\x00org.interos.calculator\x001.0.0\x00",
        );
        self.add_package_file(
            "/packages/org.interos.settings.rapp",
            b"RAPP\x02\x00\x00\x00org.interos.settings\x001.0.0\x00",
        );
        self.add_package_file(
            "/packages/org.interos.notes.rapp",
            b"RAPP\x02\x00\x00\x00org.interos.notes\x001.0.0\x00",
        );
        self.add_package_file(
            "/packages/org.interos.browser.rapp",
            b"RAPP\x02\x00\x00\x00org.interos.browser\x001.0.0\x00",
        );
        self.add_package_file(
            "/packages/org.interos.file_manager.rapp",
            b"RAPP\x02\x00\x00\x00org.interos.file_manager\x001.0.0\x00",
        );
    }

    fn mkdir_internal(&mut self, path: &str) {
        let inode = self.next_inode;
        self.next_inode += 1;
        self.nodes.insert(
            path.to_string(),
            VfsNode {
                inode,
                path: path.to_string(),
                data: Vec::new(),
                is_dir: true,
                uid: 0,
                gid: 0,
                mode: 0o755,
                mtime: 1700000000,
                file_type: 3, // Directory
                read_only: false,
            },
        );
    }

    fn add_system_file(&mut self, path: &str, content: &[u8]) {
        let inode = self.next_inode;
        self.next_inode += 1;
        self.nodes.insert(
            path.to_string(),
            VfsNode {
                inode,
                path: path.to_string(),
                data: content.to_vec(),
                is_dir: false,
                uid: 0,
                gid: 0,
                mode: 0o644,
                mtime: 1700000000,
                file_type: 2, // ReadOnlySystem
                read_only: true,
            },
        );
    }

    fn add_package_file(&mut self, path: &str, content: &[u8]) {
        let inode = self.next_inode;
        self.next_inode += 1;
        self.nodes.insert(
            path.to_string(),
            VfsNode {
                inode,
                path: path.to_string(),
                data: content.to_vec(),
                is_dir: false,
                uid: 0,
                gid: 0,
                mode: 0o644,
                mtime: 1700000000,
                file_type: 1, // RegularFile
                read_only: true,
            },
        );
    }

    pub fn open(
        &mut self,
        task_id: u32,
        path: &str,
        rights: u32,
        caps: &CapabilityTable,
    ) -> Result<u32, i64> {
        self.ensure_initialized();
        let is_write = (rights & 2) != 0;
        if is_write {
            if caps
                .check(CapabilityType::FileSystemWrite, Rights::WRITE)
                .is_err()
            {
                return Err(-13); // -EACCES
            }
        } else {
            if caps
                .check(CapabilityType::FileSystemRead, Rights::READ)
                .is_err()
                && caps
                    .check(CapabilityType::FileSystemWrite, Rights::READ)
                    .is_err()
            {
                return Err(-13); // -EACCES
            }
        }

        let node = if let Some(n) = self.nodes.get(path) {
            if n.is_dir {
                return Err(-21); // -EISDIR
            }
            if is_write && n.read_only {
                return Err(-30); // -EROFS
            }
            n.clone()
        } else {
            if is_write {
                let inode = self.next_inode;
                self.next_inode += 1;
                let new_node = VfsNode {
                    inode,
                    path: path.to_string(),
                    data: Vec::new(),
                    is_dir: false,
                    uid: task_id,
                    gid: 0,
                    mode: 0o644,
                    mtime: 1700000000,
                    file_type: 1,
                    read_only: false,
                };
                self.nodes.insert(path.to_string(), new_node.clone());
                new_node
            } else {
                return Err(-2); // -ENOENT
            }
        };

        let task_fds = self
            .descriptors
            .entry(task_id)
            .or_insert_with(BTreeMap::new);
        if task_fds.len() >= MAX_FD_PER_TASK {
            return Err(-24); // -EMFILE
        }
        let fd_counter = self.next_fd.entry(task_id).or_insert(3);
        let fd = *fd_counter;
        *fd_counter = fd_counter.wrapping_add(1);
        if *fd_counter < 3 {
            *fd_counter = 3;
        }

        task_fds.insert(
            fd,
            OpenFile {
                inode: node.inode,
                offset: 0,
                is_write,
            },
        );
        Ok(fd)
    }

    pub fn read(
        &mut self,
        task_id: u32,
        fd: u32,
        buf: &mut [u8],
        caps: &CapabilityTable,
    ) -> Result<usize, i64> {
        if caps
            .check(CapabilityType::FileSystemRead, Rights::READ)
            .is_err()
            && caps
                .check(CapabilityType::FileSystemWrite, Rights::READ)
                .is_err()
        {
            return Err(-13); // -EACCES
        }
        let task_fds = self.descriptors.get_mut(&task_id).ok_or(-9i64)?;
        let open_file = task_fds.get_mut(&fd).ok_or(-9i64)?;
        let inode = open_file.inode;
        let offset = open_file.offset;

        let node = self
            .nodes
            .values()
            .find(|n| n.inode == inode)
            .ok_or(-2i64)?;
        if offset >= node.data.len() {
            return Ok(0);
        }
        let avail = &node.data[offset..];
        let to_copy = avail.len().min(buf.len());
        buf[..to_copy].copy_from_slice(&avail[..to_copy]);
        open_file.offset += to_copy;
        Ok(to_copy)
    }

    pub fn read_at(
        &mut self,
        task_id: u32,
        fd: u32,
        offset: usize,
        buf: &mut [u8],
        caps: &CapabilityTable,
    ) -> Result<usize, i64> {
        if caps
            .check(CapabilityType::FileSystemRead, Rights::READ)
            .is_err()
            && caps
                .check(CapabilityType::FileSystemWrite, Rights::READ)
                .is_err()
        {
            return Err(-13); // -EACCES
        }
        let task_fds = self.descriptors.get(&task_id).ok_or(-9i64)?;
        let open_file = task_fds.get(&fd).ok_or(-9i64)?;
        let inode = open_file.inode;

        let node = self
            .nodes
            .values()
            .find(|n| n.inode == inode)
            .ok_or(-2i64)?;
        if offset >= node.data.len() {
            return Ok(0);
        }
        let avail = &node.data[offset..];
        let to_copy = avail.len().min(buf.len());
        buf[..to_copy].copy_from_slice(&avail[..to_copy]);
        Ok(to_copy)
    }

    pub fn write(
        &mut self,
        task_id: u32,
        fd: u32,
        buf: &[u8],
        caps: &CapabilityTable,
    ) -> Result<usize, i64> {
        if caps
            .check(CapabilityType::FileSystemWrite, Rights::WRITE)
            .is_err()
        {
            return Err(-13); // -EACCES
        }
        let task_fds = self.descriptors.get_mut(&task_id).ok_or(-9i64)?;
        let open_file = task_fds.get_mut(&fd).ok_or(-9i64)?;
        if !open_file.is_write {
            return Err(-9); // -EBADF
        }
        let inode = open_file.inode;
        let offset = open_file.offset;

        let node = self
            .nodes
            .values_mut()
            .find(|n| n.inode == inode)
            .ok_or(-2i64)?;
        if node.read_only {
            return Err(-30); // -EROFS
        }
        if offset + buf.len() > node.data.len() {
            node.data.resize(offset + buf.len(), 0);
        }
        node.data[offset..offset + buf.len()].copy_from_slice(buf);
        open_file.offset += buf.len();
        Ok(buf.len())
    }

    pub fn close(&mut self, task_id: u32, fd: u32) -> Result<(), i64> {
        let task_fds = self.descriptors.get_mut(&task_id).ok_or(-9i64)?;
        let open_file = task_fds.remove(&fd).ok_or(-9i64)?;
        if open_file.is_write {
            let _ = self.sync();
        }
        Ok(())
    }

    pub fn is_persistent_path(path: &str) -> bool {
        (path.starts_with("/data/") || path.starts_with("/user/") || path.starts_with("/storage/"))
            && !path.starts_with("/data/.vfs.")
    }

    pub fn ensure_parents(&mut self, path: &str) {
        let mut idx = 0;
        let bytes = path.as_bytes();
        while idx < bytes.len() {
            if bytes[idx] == b'/' && idx > 0 {
                let parent = &path[..idx];
                if !self.nodes.contains_key(parent) {
                    self.mkdir_internal(parent);
                }
            }
            idx += 1;
        }
    }

    pub fn create_file(&mut self, path: &str, content: &[u8], read_only: bool) -> Result<u64, i64> {
        self.ensure_initialized();
        self.ensure_parents(path);
        let inode = self.next_inode;
        self.next_inode += 1;
        self.nodes.insert(
            path.to_string(),
            VfsNode {
                inode,
                path: path.to_string(),
                data: content.to_vec(),
                is_dir: false,
                uid: 0,
                gid: 0,
                mode: if read_only { 0o444 } else { 0o644 },
                mtime: 1700000000,
                file_type: 1,
                read_only,
            },
        );
        if Self::is_persistent_path(path) {
            let _ = self.sync();
        }
        Ok(inode)
    }

    pub fn mkdir(&mut self, path: &str, caps: &CapabilityTable) -> Result<u64, i64> {
        self.ensure_initialized();
        if caps
            .check(CapabilityType::FileSystemWrite, Rights::WRITE)
            .is_err()
        {
            return Err(-13); // -EACCES
        }
        if self.nodes.contains_key(path) {
            return Err(-17); // -EEXIST
        }
        self.ensure_parents(path);
        let inode = self.next_inode;
        self.next_inode += 1;
        self.nodes.insert(
            path.to_string(),
            VfsNode {
                inode,
                path: path.to_string(),
                data: Vec::new(),
                is_dir: true,
                uid: 0,
                gid: 0,
                mode: 0o755,
                mtime: 1700000000,
                file_type: 3,
                read_only: false,
            },
        );
        if Self::is_persistent_path(path) {
            let _ = self.sync();
        }
        Ok(inode)
    }

    pub fn unlink(&mut self, path: &str, caps: &CapabilityTable) -> Result<(), i64> {
        self.ensure_initialized();
        if caps
            .check(CapabilityType::FileSystemWrite, Rights::WRITE)
            .is_err()
        {
            return Err(-13); // -EACCES
        }
        let node = self.nodes.get(path).ok_or(-2i64)?;
        if node.read_only {
            return Err(-30); // -EROFS
        }
        if node.is_dir {
            return Err(-21); // -EISDIR
        }
        self.nodes.remove(path);
        if Self::is_persistent_path(path) {
            let _ = self.sync();
        }
        Ok(())
    }

    pub fn stat(
        &mut self,
        path: &str,
        out: &mut [u8],
        caps: &CapabilityTable,
    ) -> Result<usize, i64> {
        self.ensure_initialized();
        if caps
            .check(CapabilityType::FileSystemRead, Rights::READ)
            .is_err()
            && caps
                .check(CapabilityType::FileSystemWrite, Rights::READ)
                .is_err()
        {
            return Err(-13); // -EACCES
        }
        if out.len() < FILE_STAT_BYTES {
            return Err(-22); // -EINVAL
        }
        let node = self.nodes.get(path).ok_or(-2i64)?;

        out.fill(0);
        out[0..8].copy_from_slice(&node.inode.to_le_bytes());
        out[8..12].copy_from_slice(&node.uid.to_le_bytes());
        out[12..16].copy_from_slice(&node.gid.to_le_bytes());
        out[16..18].copy_from_slice(&node.mode.to_le_bytes());
        out[18..20].copy_from_slice(&1u16.to_le_bytes()); // nlink
        out[20..28].copy_from_slice(&node.mtime.to_le_bytes()); // atime
        out[28..36].copy_from_slice(&node.mtime.to_le_bytes()); // mtime
        out[36..44].copy_from_slice(&node.mtime.to_le_bytes()); // ctime
        out[44..48].copy_from_slice(&node.file_type.to_le_bytes());
        out[48] = if node.read_only { 1 } else { 0 };
        out[88..96].copy_from_slice(&(node.data.len() as u64).to_le_bytes());
        Ok(FILE_STAT_BYTES)
    }

    pub fn list(
        &mut self,
        path: &str,
        out: &mut [u8],
        caps: &CapabilityTable,
    ) -> Result<usize, i64> {
        self.ensure_initialized();
        if caps
            .check(CapabilityType::FileSystemRead, Rights::READ)
            .is_err()
            && caps
                .check(CapabilityType::FileSystemWrite, Rights::READ)
                .is_err()
        {
            return Err(-13); // -EACCES
        }
        let mut prefix = path.to_string();
        if !prefix.ends_with('/') {
            prefix.push('/');
        }

        let mut written = 0usize;
        for (node_path, _) in &self.nodes {
            if node_path.starts_with(&prefix) && node_path != path {
                let sub = &node_path[prefix.len()..];
                if !sub.contains('/') || (sub.ends_with('/') && sub.matches('/').count() == 1) {
                    let entry_bytes = sub.trim_end_matches('/').as_bytes();
                    if written + entry_bytes.len() + 1 <= out.len() {
                        out[written..written + entry_bytes.len()].copy_from_slice(entry_bytes);
                        written += entry_bytes.len();
                        out[written] = b'\n';
                        written += 1;
                    }
                }
            }
        }
        Ok(written)
    }

    pub fn sync(&self) -> Result<(), i64> {
        if !interos_kernel_core::storage::persistent::is_block_device_bound() {
            return Ok(());
        }
        let mut buf = alloc::vec![0u8; 64 * 1024];
        buf[0..4].copy_from_slice(b"DTA1");
        let mut pos = 6usize;
        let mut count = 0u16;

        for (path, node) in &self.nodes {
            if node.read_only || !Self::is_persistent_path(path) {
                continue;
            }
            let p_bytes = path.as_bytes();
            if p_bytes.len() > 255 {
                continue;
            }
            let is_dir = node.is_dir;
            let d_len = if is_dir { 0 } else { node.data.len() };
            let marked_len = (d_len as u32) | if is_dir { 0x2000_0000 } else { 0 };
            if pos + 2 + p_bytes.len() + 4 + d_len > buf.len() {
                break;
            }
            buf[pos..pos + 2].copy_from_slice(&(p_bytes.len() as u16).to_le_bytes());
            pos += 2;
            buf[pos..pos + p_bytes.len()].copy_from_slice(p_bytes);
            pos += p_bytes.len();
            buf[pos..pos + 4].copy_from_slice(&marked_len.to_le_bytes());
            pos += 4;
            if !is_dir && d_len > 0 {
                buf[pos..pos + d_len].copy_from_slice(&node.data);
                pos += d_len;
            }
            count = count.saturating_add(1);
        }
        buf[4..6].copy_from_slice(&count.to_le_bytes());
        match interos_kernel_core::storage::persistent::commit_kind(&buf[..pos], 4) {
            Ok(gen) => {
                crate::kprintln!(
                    "[VFS] Persisted {} nodes ({} bytes) to journal generation {}.",
                    count,
                    pos,
                    gen
                );
                Ok(())
            }
            Err(_) => Err(-5), // -EIO
        }
    }

    pub fn restore_persistent_snapshot(&mut self) {
        self.ensure_initialized();
        if !interos_kernel_core::storage::persistent::is_block_device_bound() {
            return;
        }
        let mut buf = alloc::vec![0u8; 64 * 1024];
        let (gen, len) =
            match interos_kernel_core::storage::persistent::load_latest_kind(&mut buf, 4) {
                Ok(Some(res)) => res,
                Ok(None) => {
                    crate::kprintln!("[VFS RESTORE] No persistent snapshot found in journal.");
                    return;
                }
                Err(err) => {
                    crate::kprintln!("[VFS RESTORE] Failed to load snapshot: {:?}", err);
                    return;
                }
            };
        if len < 6 || &buf[..4] != b"DTA1" {
            crate::kprintln!(
                "[VFS RESTORE] Invalid data snapshot header magic in journal generation {}.",
                gen
            );
            return;
        }
        let count = u16::from_le_bytes([buf[4], buf[5]]) as usize;
        let mut pos = 6usize;
        let mut restored = 0usize;
        for _ in 0..count {
            if pos + 2 > len {
                break;
            }
            let plen = u16::from_le_bytes([buf[pos], buf[pos + 1]]) as usize;
            pos += 2;
            if plen == 0 || plen > 255 || pos + plen + 4 > len {
                break;
            }
            let path = match core::str::from_utf8(&buf[pos..pos + plen]).ok() {
                Some(v) => v,
                None => break,
            };
            pos += plen;
            let raw = u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
            pos += 4;
            let is_dir = (raw & 0x2000_0000) != 0 || path.ends_with('/');
            let dlen = (raw & 0x1fff_ffff) as usize;
            if !is_dir && (dlen > 64 * 1024 || pos + dlen > len) {
                break;
            }
            if !Self::is_persistent_path(path) {
                if !is_dir {
                    pos += dlen;
                }
                continue;
            }
            if is_dir {
                self.mkdir_internal(path);
            } else {
                let file_bytes = &buf[pos..pos + dlen];
                self.ensure_parents(path);
                let inode = self.next_inode;
                self.next_inode += 1;
                self.nodes.insert(
                    path.to_string(),
                    VfsNode {
                        inode,
                        path: path.to_string(),
                        data: file_bytes.to_vec(),
                        is_dir: false,
                        uid: 0,
                        gid: 0,
                        mode: 0o644,
                        mtime: 1700000000,
                        file_type: 1,
                        read_only: false,
                    },
                );
                pos += dlen;
            }
            restored += 1;
        }
        crate::kprintln!(
            "[VFS RESTORE] Restored {} persistent nodes from journal generation {} ({} bytes).",
            restored,
            gen,
            len
        );
    }

    pub fn get_file_content(&mut self, path: &str) -> Option<Vec<u8>> {
        self.ensure_initialized();
        let node = self.nodes.get(path)?;
        if node.is_dir {
            return None;
        }
        Some(node.data.clone())
    }
}

pub static GLOBAL_VFS: Mutex<Vfs> = Mutex::new(Vfs::empty());
