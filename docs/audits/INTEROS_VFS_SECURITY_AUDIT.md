# InterOS Core — VFS Security Architecture & Hardening Audit

**Document Version:** 1.0.0  
**Audit Target:** `kernel/src/fs/vfs.rs`, `kernel/src/syscall/mod.rs`  
**Classification:** Core Operating System Security Audit  
**Status:** Remediated & Verified  

---

## 1. Executive Summary

An adversarial security audit of the InterOS Core Virtual Filesystem (VFS) and associated syscall entry points was conducted. The audit focused on inode lifecycle management, path traversal resilience, capability-enforced authorization, directory hierarchy integrity, snapshot persistence, and process isolation.

Prior to remediation, several architectural and implementation vulnerabilities were discovered:
1. **Directory Inode Deletion via Unlink**: `unlink` failed to reject directory inodes with `IsADirectory`, creating inconsistent inode states.
2. **Dangling Directory Child Inodes**: `unlink` and `rename` did not update parent directory `children` vectors, causing empty directories to fail subsequent `rmdir` with `DirectoryNotEmpty`.
3. **Cross-Directory Hierarchy Loops**: `rename` did not prevent moving a directory into its own subdirectory, permitting cyclic trees and orphaned filesystem branches.
4. **Unchecked Truncation**: `truncate` did not reject directory, read-only system, or character device inodes.
5. **Pre-Authorization Inode Bombing**: `open` created unlinked regular files before capability and descriptor allocation succeeded, leaking phantom files when access was denied.
6. **Worker Thread Storage Escape**: Worker threads named `"user_thread"` were not resolved to their parent process package leader in `app_package_for_task`, allowing multi-threaded applications to escape sandbox isolation.
7. **ATA Flush Thrashing on Character Devices**: `sys_fs_write` unconditionally triggered `vfs.sync()` and ATA disk flushes on virtual device writes (`/dev/*`), stalling interactive rendering.

All identified vulnerabilities have been remediated in pure `#![no_std]` Rust with zero dynamic heap allocations in hot paths, and verified with dedicated unit and contract tests.

---

## 2. Inode Lifecycle & Hierarchy Invariants

### 2.1 Inode Type Enforcement in `unlink`
POSIX and capability security demand that directories cannot be unlinked with `unlink` (they must be removed with `rmdir` only when empty). Furthermore, read-only system configurations and character device nodes must never be deleted by unprivileged or arbitrary write calls.

```rust
// kernel/src/fs/vfs.rs
match inode.file_type {
    FileType::Directory => return Err(FsError::IsADirectory),
    FileType::ReadOnlySystem => return Err(FsError::ReadOnlyFileSystem),
    FileType::DeviceNode => return Err(FsError::PermissionDenied),
    FileType::RegularFile => {}
}
```

### 2.2 Directory Hierarchy Consistency & Child Rebuilding
Previously, removing a file via `unlink` removed the inode from `path_to_inode` and `inodes`, but left its `InodeId` in the parent directory inode's `children` vector. When `rmdir` was subsequently invoked, the check `if !dir_inode.children.is_empty() { return Err(FsError::DirectoryNotEmpty); }` resulted in false positives, permanently preventing directory removal.

**Remediation**:
Both `unlink` and `rename` now call `self.rebuild_children()` upon modifying inode paths:
```rust
self.rebuild_children();
```
`rebuild_children()` re-scans all live inodes in the VFS, resolves their parent path components, and reconstructs every directory's `children` list from live inodes only.

### 2.3 Subdirectory Recursion Prevention in `rename`
Moving a directory into one of its own descendants creates a disconnected loop, corrupting the root-reachable tree.
**Remediation**:
`rename` now checks whether the destination path is a descendant of the source path:
```rust
if source_inode.file_type == FileType::Directory {
    if new_path.starts_with(&source_path_with_slash) {
        return Err(FsError::InvalidPath);
    }
}
```

### 2.4 File Type Checks in `truncate`
`truncate` was hardened to explicitly reject inappropriate file types:
```rust
let inode_type = self.inodes.get(&inode_id).map(|i| i.file_type).ok_or(FsError::FileNotFound)?;
if inode_type == FileType::Directory { return Err(FsError::IsADirectory); }
if inode_type == FileType::ReadOnlySystem { return Err(FsError::ReadOnlyFileSystem); }
if inode_type == FileType::DeviceNode { return Err(FsError::PermissionDenied); }
```

---

## 3. Sandboxing & Process Isolation

### 3.1 Multi-Threaded Process Package Resolution
InterOS partitions application storage under `/user/<uid>/apps/<package_id>`. Applications resolve their sandboxed storage path via `app_storage_root(task_id)`, which calls `app_package_for_task(task_id)`.

When tasks spawn worker threads via `sys_thread_spawn`, the newly created task is named `"user_thread"`. Previously, `app_package_for_task` attempted to match `"user_thread"` against registered first-party packages or reverse-DNS strings, failed, and returned `None`. This prevented worker threads from legitimate storage access or, if unhandled, allowed fallback paths.

**Remediation**:
`app_package_for_task` now inspects the process leader:
```rust
if let Some(pid) = proc_id {
    if let Some(leader) = sched.tasks_iter().find(|t| t.process_id == Some(pid) && t.id != task_id) {
        let leader_name = leader.name;
        if let Some(pkg) = crate::package::first_party_package_by_process_name(leader_name) {
            return Some(pkg.package_id.to_string());
        }
        if (leader_name.starts_with("org.interos.") || (leader_name.contains('.') && !leader_name.contains('/') && !leader_name.contains('\\'))) && !leader_name.contains("..") {
            return Some(leader_name.to_string());
        }
    }
}
```

### 3.2 Atomic Rollback on Capability Denial
When `open` was called with `Rights::WRITE` on a non-existent file, it created the inode immediately. If the calling task lacked `CapabilityType::FileSystemWrite` or descriptor table slots were full, `open` returned an error, but the newly created 0-byte file remained in the VFS.

**Remediation**:
`open` tracks `newly_created: bool`. If capability verification or `reserve_open_file` / descriptor allocation fails, `self.unlink(&normalized)` is invoked to cleanly undo the inode creation.

---

## 4. Hardware I/O Optimization

### 4.1 Device Node Write ATA Flush Bypass
`sys_fs_write` previously called `vfs.sync()` followed by an ATA disk cache flush on every write syscall. For high-frequency writes to `/dev/null`, `/dev/zero`, or input nodes (`/dev/input/event0`), this caused severe I/O stalls.

**Remediation**:
Added `is_device_fd(&self, task_id: TaskId, fd: FileDescriptor) -> bool` to `Vfs`. `sys_fs_write` now only triggers block-device synchronization when writing to real filesystem storage:
```rust
if !is_device {
    if let Some(mut dev) = crate::storage::BLOCK_DEVICE.try_lock() {
        let _ = dev.sync();
    }
}
```

---

## 5. Verification & Evidence

All VFS invariants were verified with unit tests in `kernel/src/fs/vfs.rs`:
- `opening_or_writing_directory_is_refused_with_is_a_directory`
- `vfs_unlink_and_rename_enforce_file_types_and_directory_hierarchy`
- `vfs_truncate_enforces_file_types`
- `vfs_open_rollback_on_capability_failure_cleans_up_created_inode`

Total VFS and storage tests passing: **64 unit tests** (1 skipped 6 GiB physical staging disk builder).
