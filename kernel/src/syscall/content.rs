// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The content layer's system call surface.
//!
//! WHY these calls take the caller's authority and not a slot: a slot is a
//! position in a table the kernel laid out, and the position depends on the
//! order the kernel inserted the capabilities in. The capability *type* is
//! what the operation means, so that is what the kernel checks.
//!
//! WHY the index and the registry are separate calls: they answer different
//! questions, and a caller that may read the catalogue of documents has no
//! business dispatching one to a program, and a caller that may dispatch an
//! intent has no business enumerating the catalogue.

use crate::capability::{CapabilityType, Rights};
use crate::content::index::{DocumentRecord, CONTENT_INDEX};
use crate::content::intent::{IntentKind, INTENT_REGISTRY};
use crate::content::mime::MimeProvenance;
use crate::content::SHARED_STORAGE_ROOT;
use crate::syscall::{SyscallResult, SCHEDULER};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// The largest parameter the index and registry calls accept.
const MAX_PARAM_LEN: usize = 256;
/// The largest answer the index and registry calls produce.
const MAX_OUT_LEN: usize = 4096;

/// The query kinds the index answers.
pub const INDEX_QUERY_ALL: u64 = 0;
pub const INDEX_QUERY_NAME: u64 = 1;
pub const INDEX_QUERY_MIME: u64 = 2;
pub const INDEX_QUERY_ID: u64 = 3;

/// Requires the content index capability of the calling task.
fn require_content_index() -> Result<crate::task::TaskId, SyscallResult> {
    let sched = SCHEDULER.lock();
    let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    if !task.cap_table.authorize_capability(CapabilityType::ContentIndexRead, Rights::READ) {
        return Err(SyscallResult::PermissionDenied);
    }
    Ok(task.id)
}

/// Requires the intent dispatch capability of the calling task.
fn require_content_dispatch() -> Result<crate::task::TaskId, SyscallResult> {
    let sched = SCHEDULER.lock();
    let task = sched.current_task().ok_or(SyscallResult::PermissionDenied)?;
    if !task.cap_table.authorize_capability(CapabilityType::ContentIntentDispatch, Rights::INVOKE) {
        return Err(SyscallResult::PermissionDenied);
    }
    Ok(task.id)
}

/// The package the calling task belongs to.
///
/// WHY this is needed: the pending intent is a device-wide slot, and a program
/// that could read another program's pending intent would learn what the user
/// is about to open. The kernel therefore answers a pending-intent read only
/// to the package the intent was dispatched to.
fn caller_package_id() -> Option<String> {
    let name = {
        let sched = SCHEDULER.lock();
        sched.current_task().map(|task| task.name)?
    };
    if let Some(descriptor) = crate::package::first_party_package_by_process_name(name) {
        return Some(descriptor.package_id.to_string());
    }
    // A verified RAPP runs under a process named after its package id.
    if name.contains('.')
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
    {
        return Some(name.to_string());
    }
    None
}

/// Rebuilds the index from the shared storage area when the storage has moved
/// on since the last walk.
fn refresh_index_if_stale() {
    let (generation, files) = {
        let vfs = crate::fs::VFS.lock();
        let generation = vfs.storage_generation();
        let files = vfs.walk_files(SHARED_STORAGE_ROOT, crate::content::MIME_SNIFF_BYTES);
        (generation, files)
    };
    let mut index = CONTENT_INDEX.lock();
    if index.is_current(generation) {
        return;
    }
    index.replace(&files, generation);
}

/// Whether the calling task may see a path under the per-app path policy.
///
/// WHY the index asks the filesystem: the policy is the filesystem's, and a
/// second copy of it in the content layer would be a second thing to keep
/// true.
fn caller_may_see(task_id: crate::task::TaskId, path: &str) -> bool {
    crate::fs::VFS.lock().app_path_visible(task_id, path)
}

/// Writes one document record into the answer buffer.
fn write_record(out: &mut [u8], pos: &mut usize, record: &DocumentRecord) {
    use core::fmt::Write;
    struct W<'a> {
        b: &'a mut [u8],
        n: usize,
    }
    impl<'a> Write for W<'a> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let k = s.len().min(self.b.len().saturating_sub(self.n));
            self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
            self.n += k;
            Ok(())
        }
    }
    let mut w = W { b: out, n: *pos };
    let _ = write!(
        w,
        "{:016x}|{}|{}|{}|{}|{}\n",
        record.id,
        record.path,
        record.mime,
        record.provenance.as_str(),
        record.length,
        record.mtime
    );
    *pos = w.n;
}

/// Answers a query against the shared document index.
pub fn sys_content_index_query(
    query: u64,
    param: &str,
    out: &mut [u8],
) -> Result<usize, SyscallResult> {
    if param.len() > MAX_PARAM_LEN || out.len() > MAX_OUT_LEN {
        return Err(SyscallResult::InvalidArguments);
    }
    let task_id = require_content_index()?;
    refresh_index_if_stale();

    let index = CONTENT_INDEX.lock();
    let selected: Vec<&DocumentRecord> = match query {
        INDEX_QUERY_ALL => index.records().iter().collect(),
        INDEX_QUERY_NAME => index.search_by_name(param),
        INDEX_QUERY_MIME => index.search_by_mime(param),
        INDEX_QUERY_ID => {
            let id = u64::from_str_radix(param.trim(), 16).map_err(|_| SyscallResult::InvalidArguments)?;
            match index.find_by_id(id) {
                Some(record) => vec![record],
                None => Vec::new(),
            }
        }
        _ => return Err(SyscallResult::InvalidArguments),
    };

    let mut pos = 0usize;
    for record in selected {
        if !caller_may_see(task_id, &record.path) {
            continue;
        }
        write_record(out, &mut pos, record);
        if pos >= out.len() {
            break;
        }
    }
    Ok(pos)
}

/// Forces a fresh walk of the shared storage area.
pub fn sys_content_index_refresh() -> Result<usize, SyscallResult> {
    require_content_index()?;
    let (generation, files) = {
        let vfs = crate::fs::VFS.lock();
        let generation = vfs.storage_generation();
        let files = vfs.walk_files(SHARED_STORAGE_ROOT, crate::content::MIME_SNIFF_BYTES);
        (generation, files)
    };
    let count = files.len();
    let mut index = CONTENT_INDEX.lock();
    index.replace(&files, generation);
    Ok(count)
}

/// Resolves the package that handles a media type, scheme or action.
pub fn sys_intent_resolve(
    kind: u64,
    value: &str,
    out: &mut [u8],
) -> Result<usize, SyscallResult> {
    if value.is_empty() || value.len() > MAX_PARAM_LEN || out.len() > MAX_OUT_LEN {
        return Err(SyscallResult::InvalidArguments);
    }
    require_content_index()?;
    let kind = IntentKind::from_raw(kind).ok_or(SyscallResult::InvalidArguments)?;
    let handler = {
        let registry = INTENT_REGISTRY.lock();
        registry.resolve(kind, value).map(|id| id.to_string())
    };
    // WHY a refusal names nothing: a caller that asked who handles a media
    // type with no handler must be told that there is no handler, and the
    // platform must not offer a substitute.
    let Some(handler) = handler else {
        crate::serial_println!(
            "[CONTENT] No installed package declares a handler for {} '{}'.",
            kind.as_str(),
            value
        );
        return Err(SyscallResult::TargetNotFound);
    };
    let n = handler.len().min(out.len());
    out[..n].copy_from_slice(&handler.as_bytes()[..n]);
    Ok(n)
}

/// Lists every handler the installed packages have declared.
pub fn sys_intent_list(out: &mut [u8]) -> Result<usize, SyscallResult> {
    if out.len() > MAX_OUT_LEN {
        return Err(SyscallResult::InvalidArguments);
    }
    require_content_index()?;
    use core::fmt::Write;
    struct W<'a> {
        b: &'a mut [u8],
        n: usize,
    }
    impl<'a> Write for W<'a> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let k = s.len().min(self.b.len().saturating_sub(self.n));
            self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
            self.n += k;
            Ok(())
        }
    }
    let registry = INTENT_REGISTRY.lock();
    let capacity = out.len();
    let mut w = W { b: out, n: 0 };
    for handler in registry.handlers() {
        let _ = write!(w, "{}|{}|{}\n", handler.kind.as_str(), handler.value, handler.package_id);
        if w.n >= capacity {
            break;
        }
    }
    Ok(w.n)
}

/// Dispatches an intent to the package that declared a handler for it.
pub fn sys_intent_dispatch(
    kind: u64,
    value: &str,
    payload: &str,
) -> Result<u64, SyscallResult> {
    if value.is_empty() || value.len() > MAX_PARAM_LEN || payload.len() > MAX_PARAM_LEN {
        return Err(SyscallResult::InvalidArguments);
    }
    let task_id = require_content_dispatch()?;
    let kind = IntentKind::from_raw(kind).ok_or(SyscallResult::InvalidArguments)?;
    let handler = {
        let registry = INTENT_REGISTRY.lock();
        registry.resolve(kind, value).map(|id| id.to_string())
    };
    let Some(handler) = handler else {
        crate::serial_println!(
            "[CONTENT] Intent {} '{}' was refused: no installed package declares a handler for it.",
            kind.as_str(),
            value
        );
        return Err(SyscallResult::TargetNotFound);
    };
    {
        let mut registry = INTENT_REGISTRY.lock();
        registry.set_pending(kind, value, &handler, payload);
    }
    let tid = super::launch_registered_package(&handler, task_id)?;
    crate::serial_println!(
        "[CONTENT] Dispatched {} '{}' to '{}' as Task {}.",
        kind.as_str(),
        value,
        handler,
        tid.0
    );
    Ok(tid.0)
}

/// The intent dispatched to the calling package, if there is one.
pub fn sys_intent_pending(out: &mut [u8]) -> Result<usize, SyscallResult> {
    if out.len() > MAX_OUT_LEN {
        return Err(SyscallResult::InvalidArguments);
    }
    require_content_index()?;
    let Some(package_id) = caller_package_id() else {
        return Ok(0);
    };
    let pending = {
        let registry = INTENT_REGISTRY.lock();
        registry.peek_pending(&package_id).cloned()
    };
    let Some(pending) = pending else {
        return Ok(0);
    };
    use core::fmt::Write;
    struct W<'a> {
        b: &'a mut [u8],
        n: usize,
    }
    impl<'a> Write for W<'a> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let k = s.len().min(self.b.len().saturating_sub(self.n));
            self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
            self.n += k;
            Ok(())
        }
    }
    let mut w = W { b: out, n: 0 };
    let _ = write!(
        w,
        "{}|{}|{}|{}\n",
        pending.kind.as_str(),
        pending.value,
        pending.payload,
        pending.token
    );
    Ok(w.n)
}

/// The provenance of a media type, for the kernel's own tests.
pub fn provenance_label(provenance: MimeProvenance) -> &'static str {
    provenance.as_str()
}
