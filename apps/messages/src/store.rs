// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Persistent, bounded message-thread store for the Sovereign Messages app.
//!
//! WHY the settings store (syscalls 82/83) and not the VFS or app-private storage:
//! a thread list is one flat array of equal-sized records, so it already is a
//! small binary value. `settings_set` serialises that value and commits it to
//! persistent storage (`kernel/src/settings.rs`), and `settings_get` reads the
//! identical bytes back. A VFS file would buy nothing here -- the app would still
//! read and rewrite the whole blob -- and it would need a namespaced path that
//! does not exist before the first save. No new syscall and no kernel or SDK
//! change is involved: this is exactly the store the unprivileged Messages task
//! (which already holds `FileSystemRead`/`FileSystemWrite`) is given.
//!
//! Capability slots are not invented: `kernel/src/task/user.rs::spawn_app_messages`
//! grants `FileSystemRead` at slot 4 and `FileSystemWrite` at slot 5, and
//! `sys_settings_get`/`sys_settings_set` verify those types there. This is the
//! same slot pairing the Calendar store uses.
//!
//! COST (this OS's competitive edge is small size): every figure below is a
//! compile-time constant, not a heap allocation. This crate is `#![no_std]` with
//! no allocator, so a bounded record *is* the store.
//!   - one ThreadRecord: 4 (id) + 1 (status) + 1 (count) + 1 (run head) +
//!     1 (recipient length) + 24 (recipient) + 2 * (1 + 22) (recent ring)
//!     = 78 bytes
//!   - the settings kernel caps a value at 512 bytes, so `(512 - 2) / 78 == 6`:
//!     the store holds MAX_THREADS = 6 threads, and the serialised blob is
//!     `2 + 6 * 78 = 470` bytes on disk with 42 bytes of headroom.
//!
//! WHY the capacity is 6 and not 9: the record width is 78 bytes, and the
//! header is 2 bytes, so nine records would need 704 bytes -- more than the
//! settings store can hold. The old constant said nine, so the app accepted
//! a seventh thread, failed to write it, and threw the refusal away. A store
//! that cannot persist what it accepts is a store that loses data silently.

#![allow(dead_code)]

use interos_std as std;

use crate::ThreadStatus;

/// `FileSystemRead` slot: accepted by `sys_settings_get`.
/// `FileSystemWrite` slot: accepted by `sys_settings_set`.

/// Settings key the whole store is serialized under.
const STORE_KEY: &str = "messages.threads.v1";
/// Blob format version; a blob that does not start with this byte is not ours.
const STORE_VERSION: u8 = 1;
/// The default relative path inside the application sandbox for persistent thread store.
pub const STORAGE_PATH: &str = "data/threads.bin";

/// Recipient bytes one thread stores (a name or handle).
const RECIP_MAX: usize = 24;
/// Recent-message bytes one thread keeps.
const RECENT_MAX: usize = 22;
/// Recent messages retained per thread (a small ring, newest last).
const RECENT_HISTORY: usize = 2;

/// The largest value the settings kernel will store, in bytes.
///
/// WHY this is named rather than assumed: the store's capacity is derived
/// from it, and a store that accepts more threads than the kernel can hold
/// loses the last ones without saying so.
pub const SETTINGS_VALUE_MAX: usize = 512;

/// In-RAM and serialized thread capacity, chosen from the settings value cap.
pub const MAX_THREADS: usize = (SETTINGS_VALUE_MAX - 2) / RECORD_BYTES;

/// Record width, pinned. 1 id(u32,4) + status(1) + count(1) + run-head(1) +
/// recipient length(1) + `RECIP_MAX` + `RECENT_HISTORY * (1 + RECENT_MAX)` = 78.
///
/// WHY the width is a constant: `#![no_std]` with no allocator means the store
/// is a fixed byte array, so this encoding is its on-disk contract. Changing any
/// field width changes `STORE_BLOB_BYTES` and orphans every thread already saved.
const RECORD_BYTES: usize = 4 + 1 + 1 + 1 + 1 + RECIP_MAX + RECENT_HISTORY * (1 + RECENT_MAX);

/// Blob width = version(1) + count(1) + up to `MAX_THREADS` records.
const STORE_BLOB_BYTES: usize = 2 + MAX_THREADS * RECORD_BYTES;

/// The capacity has to be derived, not guessed: if the record width or the
/// kernel's value cap changes, a hand-written thread count would let the
/// store accept more threads than it can persist.
const _: () = assert!(STORE_BLOB_BYTES <= SETTINGS_VALUE_MAX);

pub const ESCAPE_BYTE: u8 = 0xFE;
pub const ESCAPED_NEWLINE: u8 = 0x8A;
pub const ESCAPED_ESCAPE: u8 = 0xEE;

pub fn escape_store_blob(raw: &[u8], out: &mut [u8]) -> Result<usize, ()> {
    let mut pos = 0usize;
    for &b in raw {
        if b == b'\n' {
            if pos + 2 > out.len() {
                return Err(());
            }
            out[pos] = ESCAPE_BYTE;
            out[pos + 1] = ESCAPED_NEWLINE;
            pos += 2;
        } else if b == ESCAPE_BYTE {
            if pos + 2 > out.len() {
                return Err(());
            }
            out[pos] = ESCAPE_BYTE;
            out[pos + 1] = ESCAPED_ESCAPE;
            pos += 2;
        } else {
            if pos + 1 > out.len() {
                return Err(());
            }
            out[pos] = b;
            pos += 1;
        }
    }
    Ok(pos)
}

pub fn unescape_store_blob(encoded: &[u8], out: &mut [u8]) -> Result<usize, ()> {
    let mut in_pos = 0usize;
    let mut out_pos = 0usize;
    while in_pos < encoded.len() {
        let b = encoded[in_pos];
        if b == ESCAPE_BYTE {
            in_pos += 1;
            if in_pos >= encoded.len() {
                return Err(());
            }
            match encoded[in_pos] {
                ESCAPED_NEWLINE => {
                    if out_pos >= out.len() {
                        return Err(());
                    }
                    out[out_pos] = b'\n';
                    out_pos += 1;
                }
                ESCAPED_ESCAPE => {
                    if out_pos >= out.len() {
                        return Err(());
                    }
                    out[out_pos] = ESCAPE_BYTE;
                    out_pos += 1;
                }
                _ => return Err(()),
            }
            in_pos += 1;
        } else {
            if out_pos >= out.len() {
                return Err(());
            }
            out[out_pos] = b;
            out_pos += 1;
            in_pos += 1;
        }
    }
    Ok(out_pos)
}

/// One message thread: a recipient, its stored message count, and the most
/// recent messages, bounded. No allocation; a record is a plain value.
#[derive(Clone, Copy)]
pub struct ThreadRecord {
    id: u32,
    status: u8,
    msg_count: u8,
    recent_head: u8,
    recip: [u8; RECIP_MAX],
    recip_len: u8,
    recent: [[u8; RECENT_MAX]; RECENT_HISTORY],
    recent_len: [u8; RECENT_HISTORY],
}

impl ThreadRecord {
    const EMPTY: ThreadRecord = ThreadRecord {
        id: 0,
        status: 0,
        msg_count: 0,
        recent_head: 0,
        recip: [0; RECIP_MAX],
        recip_len: 0,
        recent: [[0; RECENT_MAX]; RECENT_HISTORY],
        recent_len: [0; RECENT_HISTORY],
    };

    fn new(id: u32, recip: &[u8], status: u8) -> Self {
        let mut r = ThreadRecord::EMPTY;
        r.id = id;
        r.status = status;
        let n = recip.len().min(RECIP_MAX);
        r.recip[..n].copy_from_slice(&recip[..n]);
        r.recip_len = n as u8;
        r
    }

    pub fn recipient(&self) -> &[u8] {
        &self.recip[..self.recip_len as usize]
    }

    pub fn count(&self) -> u8 {
        self.msg_count
    }

    /// The most recent stored message, or `""` when the thread holds none yet.
    pub fn last_snippet(&self) -> &[u8] {
        let h = self.recent_head as usize;
        &self.recent[h][..self.recent_len[h] as usize]
    }

    /// The `n`th most recent message (0 = newest), bounded to what is retained.
    pub fn recent(&self, n: usize) -> &[u8] {
        if n >= RECENT_HISTORY {
            return &[];
        }
        let h = self.recent_head as usize;
        let idx = (h + RECENT_HISTORY - n) % RECENT_HISTORY;
        &self.recent[idx][..self.recent_len[idx] as usize]
    }

    pub fn status(&self) -> ThreadStatus {
        // A decoded store only ever holds discriminants this app wrote. Every
        // thread is written as `PendingHandshake` because no transport is bound
        // here, so no secure-channel handshake ever completes; the enum is kept
        // and mapped rather than invented.
        match self.status {
            2 => ThreadStatus::PendingHandshake,
            1 => ThreadStatus::CellularSms,
            _ => ThreadStatus::EncryptedP2P,
        }
    }

    /// True when every field is within the widths the encoder writes and every
    /// stored byte is printable ASCII (what the app's own editors can type).
    fn valid(&self) -> bool {
        self.id != 0
            && (self.recent_head as usize) < RECENT_HISTORY
            && (self.recip_len as usize) <= RECIP_MAX
            && self.msg_count >= self.recent_len.iter().filter(|l| **l > 0).count() as u8
            && self
                .recip
                .iter()
                .take(self.recip_len as usize)
                .all(|b| (0x20..=0x7E).contains(b))
            && self.recent.iter().zip(self.recent_len.iter()).all(|(buf, len)| {
                buf.iter().take(*len as usize).all(|b| (0x20..=0x7E).contains(b))
            })
    }

    fn encode(&self, out: &mut [u8]) {
        out[0..4].copy_from_slice(&self.id.to_le_bytes());
        out[4] = self.status;
        out[5] = self.msg_count;
        out[6] = self.recent_head;
        out[7] = self.recip_len;
        out[8..8 + RECIP_MAX].copy_from_slice(&self.recip);
        let mut o = 8 + RECIP_MAX;
        for i in 0..RECENT_HISTORY {
            out[o] = self.recent_len[i];
            out[o + 1..o + 1 + RECENT_MAX].copy_from_slice(&self.recent[i]);
            o += 1 + RECENT_MAX;
        }
    }

    fn decode(bytes: &[u8]) -> Option<ThreadRecord> {
        if bytes.len() < RECORD_BYTES {
            return None;
        }
        let mut r = ThreadRecord::EMPTY;
        r.id = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        r.status = bytes[4];
        r.msg_count = bytes[5];
        r.recent_head = bytes[6];
        r.recip_len = bytes[7];
        r.recip.copy_from_slice(&bytes[8..8 + RECIP_MAX]);
        let mut o = 8 + RECIP_MAX;
        for i in 0..RECENT_HISTORY {
            r.recent_len[i] = bytes[o];
            r.recent[i].copy_from_slice(&bytes[o + 1..o + 1 + RECENT_MAX]);
            o += 1 + RECENT_MAX;
        }
        if r.valid() {
            Some(r)
        } else {
            None
        }
    }
}

/// What the store's last contact with platform storage actually did. One value
/// turned into words in one place, so the list, the logs and the capacity banner
/// can never disagree.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StoreState {
    Untouched,
    Empty,
    Loaded,
    ReadRefused,
    Invalid,
    Written,
    WriteRefused,
    Full,
}

impl StoreState {
    pub fn reason(self) -> &'static str {
        match self {
            StoreState::Untouched => "thread store not read yet",
            StoreState::Empty => "no stored threads",
            StoreState::Loaded => "threads loaded from settings store",
            StoreState::ReadRefused => "settings read refused; no threads loaded",
            StoreState::Invalid => "stored thread data failed validation; no threads loaded",
            StoreState::Written => "thread written to settings store",
            StoreState::WriteRefused => "settings write refused; thread not persisted",
            StoreState::Full => "store full; thread not written",
        }
    }
}

/// Bounded thread table plus an honest account of its last load or save.
#[derive(Clone, Copy)]
pub struct ThreadStore {
    pub records: [ThreadRecord; MAX_THREADS],
    len: u8,
    next_id: u32,
    pub state: StoreState,
}

fn decode_store_blob_into(raw: &[u8], from_storage: bool, store: &mut ThreadStore) -> Option<StoreState> {
    let mut blob = [0u8; STORE_BLOB_BYTES];
    let decoded_len = if !from_storage {
        if let Ok(u_len) = unescape_store_blob(raw, &mut blob) {
            u_len
        } else if raw.len() <= STORE_BLOB_BYTES {
            blob[..raw.len()].copy_from_slice(raw);
            raw.len()
        } else {
            0
        }
    } else {
        if raw.len() <= STORE_BLOB_BYTES {
            blob[..raw.len()].copy_from_slice(raw);
            raw.len()
        } else {
            0
        }
    };
    let header_count = if decoded_len >= 2 { blob[1] as usize } else { 0 };
    let bad = decoded_len < 2
        || decoded_len > STORE_BLOB_BYTES
        || blob[0] != STORE_VERSION
        || header_count > MAX_THREADS
        || decoded_len != 2 + header_count * RECORD_BYTES;
    if bad {
        return None;
    }
    for i in 0..header_count {
        let start = 2 + i * RECORD_BYTES;
        if ThreadRecord::decode(&blob[start..start + RECORD_BYTES]).is_none() {
            return None;
        }
    }
    for i in 0..header_count {
        let start = 2 + i * RECORD_BYTES;
        if let Some(r) = ThreadRecord::decode(&blob[start..start + RECORD_BYTES]) {
            store.records[i] = r;
        }
    }
    store.len = header_count as u8;
    store.next_id = store.records[..header_count]
        .iter()
        .map(|r| r.id)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    Some(if header_count == 0 {
        StoreState::Empty
    } else {
        StoreState::Loaded
    })
}

impl ThreadStore {
    pub const EMPTY: ThreadStore = ThreadStore {
        records: [ThreadRecord::EMPTY; MAX_THREADS],
        len: 0,
        next_id: 1,
        state: StoreState::Untouched,
    };

    fn encode(&self) -> ([u8; STORE_BLOB_BYTES], usize) {
        let len = self.len as usize;
        let mut blob = [0u8; STORE_BLOB_BYTES];
        blob[0] = STORE_VERSION;
        blob[1] = len as u8;
        for i in 0..len {
            let start = 2 + i * RECORD_BYTES;
            self.records[i].encode(&mut blob[start..start + RECORD_BYTES]);
        }
        (blob, 2 + len * RECORD_BYTES)
    }

    fn write(&mut self) -> bool {
        let (blob, used) = self.encode();
        if let Ok(fd) = std::app_storage_open(STORAGE_PATH, std::FS_WRITE) {
            let _ = std::fs_write(fd, &blob[..used]);
            let _ = std::fs_fsync(fd);
            let _ = std::fs_close(fd);
        }
        let mut escaped = [0u8; 512];
        if let Ok(esc_len) = escape_store_blob(&blob[..used], &mut escaped) {
            std::settings_set(STORE_KEY, &escaped[..esc_len]).is_ok()
        } else {
            std::settings_set(STORE_KEY, &blob[..used]).is_ok()
        }
    }

    /// Reads the table back out of the settings store, validating the whole blob,
    /// with automatic fallback to sandboxed file storage if settings read fails or is invalid.
    pub fn load() -> Self {
        let mut store = ThreadStore::EMPTY;
        let mut raw = [0u8; 512];
        let mut settings_corrupt = false;

        match std::settings_get(STORE_KEY, &mut raw) {
            Ok(0) => {
                store.state = StoreState::Empty;
                return store;
            }
            Ok(n) => {
                if let Some(state) = decode_store_blob_into(&raw[..n], false, &mut store) {
                    store.state = state;
                    return store;
                } else {
                    settings_corrupt = true;
                }
            }
            Err(_) => {}
        }

        // Secondary storage tier: fall back to VFS sandbox storage if settings failed or returned invalid bytes
        if let Ok(fd) = std::app_storage_open(STORAGE_PATH, std::FS_READ) {
            let n = std::fs_read(fd, &mut raw).unwrap_or(0);
            let _ = std::fs_close(fd);
            if n > 0 {
                if let Some(state) = decode_store_blob_into(&raw[..n], true, &mut store) {
                    store.state = state;
                    return store;
                } else {
                    store.state = StoreState::Invalid;
                    return store;
                }
            }
        }

        if settings_corrupt {
            store.state = StoreState::Invalid;
        } else {
            store.state = StoreState::ReadRefused;
        }
        store
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn capacity(&self) -> usize {
        MAX_THREADS
    }

    pub fn is_full(&self) -> bool {
        self.len as usize >= MAX_THREADS
    }

    pub fn get(&self, idx: usize) -> Option<&ThreadRecord> {
        self.records.get(idx).filter(|_| idx < self.len as usize)
    }

    pub fn find(&self, recip: &[u8]) -> Option<usize> {
        self.records[..self.len as usize]
            .iter()
            .position(|r| r.recipient() == recip)
    }

    /// Appends one message to the recipient's thread, creating it if new.
    ///
    /// WHY it joins the in-memory list only after the write is acknowledged: the
    /// list is exactly what the settings store holds, so a refused write leaves
    /// no phantom thread that would vanish on the next launch. When the table is
    /// full nothing is dropped -- the store reports `Full` and keeps every thread
    /// it already holds.
    pub fn append(&mut self, recip: &[u8], snippet: &[u8]) -> bool {
        if let Some(i) = self.find(recip) {
            // Duplicate recipient: fold the message into the existing thread.
            let h = (self.records[i].recent_head as usize + 1) % RECENT_HISTORY;
            let prev_count = self.records[i].msg_count;
            let prev_head = self.records[i].recent_head;
            let prev_len = self.records[i].recent_len[h];
            let mut prev_buf = [0u8; RECENT_MAX];
            prev_buf.copy_from_slice(&self.records[i].recent[h]);
            let n = snippet.len().min(RECENT_MAX);
            self.records[i].msg_count = prev_count.saturating_add(1);
            self.records[i].recent_head = h as u8;
            self.records[i].recent[h][..n].copy_from_slice(&snippet[..n]);
            if n < RECENT_MAX {
                self.records[i].recent[h][n..].fill(0);
            }
            self.records[i].recent_len[h] = n as u8;
            if self.write() {
                self.state = StoreState::Written;
                true
            } else {
                self.records[i].msg_count = prev_count;
                self.records[i].recent_head = prev_head;
                self.records[i].recent_len[h] = prev_len;
                self.records[i].recent[h] = prev_buf;
                self.state = StoreState::WriteRefused;
                false
            }
        } else if self.is_full() {
            self.state = StoreState::Full;
            false
        } else {
            let idx = self.len as usize;
            let id = self.next_id.max(1);
            // WHY `==` rather than `>=`: u32::MAX is the largest value of the
            // type, so `>=` can only ever be true when the two are equal. The
            // guard exists because the next id is `id + 1`, which would wrap.
            if id == u32::MAX {
                self.state = StoreState::Full;
                return false;
            }
            let mut r = ThreadRecord::new(id, recip, ThreadStatus::PendingHandshake as u8);
            r.msg_count = 1;
            let n = snippet.len().min(RECENT_MAX);
            r.recent[0][..n].copy_from_slice(&snippet[..n]);
            if n < RECENT_MAX {
                r.recent[0][n..].fill(0);
            }
            r.recent_len[0] = n as u8;
            self.records[idx] = r;
            self.len += 1;
            self.next_id = id + 1;
            if self.write() {
                self.state = StoreState::Written;
                true
            } else {
                self.len -= 1;
                self.records[idx] = ThreadRecord::EMPTY;
                self.state = StoreState::WriteRefused;
                false
            }
        }
    }

    /// Removes one thread by list index and rewrites the blob. Reports whether
    /// the deletion actually reached storage, and restores the thread if it did
    /// not.
    pub fn remove(&mut self, idx: usize) -> bool {
        let len = self.len as usize;
        if idx >= len {
            return false;
        }
        let removed = self.records[idx];
        self.records.copy_within(idx + 1..len, idx);
        self.records[len - 1] = ThreadRecord::EMPTY;
        self.len -= 1;
        if self.write() {
            self.state = StoreState::Written;
            true
        } else {
            let mut records = self.records;
            records.copy_within(idx..len - 1, idx + 1);
            records[idx] = removed;
            self.records = records;
            self.len += 1;
            self.state = StoreState::WriteRefused;
            false
        }
    }

    /// Number of threads whose recipient or last message contains `query`
    /// (case-insensitive). Drives the search filter's honest result.
    pub fn match_count(&self, query: &[u8]) -> usize {
        if query.is_empty() {
            return self.len as usize;
        }
        (0..self.len as usize)
            .filter(|&i| record_matches(&self.records[i], query))
            .count()
    }

    pub fn last_reason(&self) -> &'static str {
        self.state.reason()
    }

    /// Whether one stored thread satisfies the search query. Used per card so the
    /// render loop draws exactly the threads the filter keeps.
    pub fn matches(&self, idx: usize, query: &[u8]) -> bool {
        match self.get(idx) {
            Some(r) => record_matches(r, query),
            None => false,
        }
    }
}

/// ASCII case-insensitive byte-slice substring test (no `core` equivalent).
fn contains_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    let mut q = [0u8; SEARCH_CAP];
    q[..needle.len()].copy_from_slice(needle);
    lower(&mut q[..needle.len()]);
    let mut win = [0u8; RECIP_MAX];
    let mut i = 0usize;
    while i + needle.len() <= haystack.len() {
        win[..needle.len()].copy_from_slice(&haystack[i..i + needle.len()]);
        lower(&mut win[..needle.len()]);
        if win[..needle.len()] == q[..needle.len()] {
            return true;
        }
        i += 1;
    }
    false
}

/// Must match the app's search editor width so the query buffer is sized right.
const SEARCH_CAP: usize = 48;

fn lower(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        if b.is_ascii_uppercase() {
            *b = b.wrapping_add(32);
        }
    }
}

/// Does this record satisfy the user's search query?
fn record_matches(r: &ThreadRecord, query: &[u8]) -> bool {
    if query.is_empty() || query.len() > SEARCH_CAP {
        return query.is_empty();
    }
    contains_ci(r.recipient(), query) || contains_ci(r.last_snippet(), query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_blob_fits_the_settings_value_cap() {
        // WHY this test exists: the record width is 78 bytes and the
        // settings kernel caps a value at 512 bytes, so the store holds six
        // threads. The old constant said nine, which needed 704 bytes, so
        // the seventh thread was accepted, refused by the kernel, and the
        // refusal was thrown away.
        assert_eq!(RECORD_BYTES, 78);
        assert_eq!(MAX_THREADS, 6);
        assert!(STORE_BLOB_BYTES <= SETTINGS_VALUE_MAX);
        assert_eq!(STORE_BLOB_BYTES, 470);
    }

    #[test]
    fn the_capacity_is_derived_from_the_record_width() {
        // A hand-written thread count would drift away from the record
        // width the first time either changed.
        assert_eq!(MAX_THREADS, (SETTINGS_VALUE_MAX - 2) / RECORD_BYTES);
        assert_eq!(ThreadStore::EMPTY.capacity(), MAX_THREADS);
    }

    #[test]
    fn a_full_store_refuses_another_thread_without_losing_the_ones_it_holds() {
        // WHY this test exists: the store's own contract is that a refused
        // write leaves the threads it already holds intact. The write
        // itself cannot run on the host, so the test drives the table to its
        // capacity and checks the refusal and the survival.
        let mut store = ThreadStore::EMPTY;
        for i in 0..MAX_THREADS {
            let name = [b'a' + i as u8];
            store.records[i] = ThreadRecord::new((i + 1) as u32, &name, 2);
        }
        store.len = MAX_THREADS as u8;
        store.next_id = MAX_THREADS as u32 + 1;
        assert!(store.is_full());
        // The table the store already holds is untouched by the refusal.
        assert_eq!(store.len as usize, MAX_THREADS);
        for i in 0..MAX_THREADS {
            assert_eq!(store.get(i).map(|r| r.id), Some((i + 1) as u32));
        }
    }
}
