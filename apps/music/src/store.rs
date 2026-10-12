//! Persistent, bounded local track library for the music player.
//!
//! WHY the settings store (syscalls 82/83) and not the VFS or app-private storage:
//! the library is one flat array of equal-sized records -- a small binary value.
//! `settings_set` serialises it and commits it to persistent storage
//! (`kernel/src/settings.rs`); `settings_get` reads the identical bytes back. A
//! VFS file would buy nothing (the app still rewrites the whole blob) and would
//! need a namespaced path that does not exist before the first save. No new
//! syscall and no kernel or SDK change is involved.
//!
//! Capability slots are not invented: `kernel/src/task/user.rs::spawn_app_music`
//! grants `FileSystemRead` at slot 4 and `FileSystemWrite` at slot 5, and
//! `sys_settings_get`/`sys_settings_set` verify those types there.
//!
//! COST (this OS's competitive edge is small size): every figure is a
//! compile-time constant, not a heap allocation. `#![no_std]`, no allocator.
//!   - one TrackRecord: 4 id + 1 title length + 40 title + 4 duration = 49 bytes
//!   - MAX_TRACKS = 10 = 490 bytes in RAM; the serialized blob is
//!     `2 + 10 * 49 = 492` bytes, inside the 512 the kernel caps a settings value.

#![allow(dead_code)]

use interos_std as std;

/// `FileSystemRead` slot: accepted by `sys_settings_get`.
const STORE_CAP_READ: usize = 4;
/// `FileSystemWrite` slot: accepted by `sys_settings_set`.
const STORE_CAP_WRITE: usize = 5;

/// Settings key the whole library is serialized under.
const STORE_KEY: &str = "music.library.v1";
/// Blob format version; a blob not starting with this byte is not ours.
const STORE_VERSION: u8 = 1;

/// Title bytes one track stores.
const TITLE_MAX: usize = 40;

/// Library capacity, chosen so the serialized blob stays within the 512-byte cap.
const MAX_TRACKS: usize = 10;

/// Record width, pinned: id(u32,4) + title length(1) + `TITLE_MAX` + duration
/// (u32,4) = 49 bytes.
///
/// WHY the width is a constant: `#![no_std]` with no allocator means the store is
/// a fixed byte array, so this encoding is its on-disk contract. Changing any
/// field width orphans every track already saved.
const RECORD_BYTES: usize = 4 + 1 + TITLE_MAX + 4;

/// Blob width = version(1) + count(1) + up to `MAX_TRACKS` records.
const STORE_BLOB_BYTES: usize = 2 + MAX_TRACKS * RECORD_BYTES;

/// Longest and shortest track duration the store accepts, in seconds. The floor
/// keeps a track from being a duration-less placeholder.
const DUR_MIN: u32 = 1;
const DUR_MAX: u32 = 100_000;

/// One locally stored track: a title and a duration. No allocation.
#[derive(Clone, Copy)]
pub struct TrackRecord {
    id: u32,
    title: [u8; TITLE_MAX],
    title_len: u8,
    duration_sec: u32,
}

impl TrackRecord {
    const EMPTY: TrackRecord = TrackRecord {
        id: 0,
        title: [0; TITLE_MAX],
        title_len: 0,
        duration_sec: 0,
    };

    pub fn title(&self) -> &[u8] {
        &self.title[..self.title_len as usize]
    }

    pub fn duration_sec(&self) -> u32 {
        self.duration_sec
    }

    fn valid(&self) -> bool {
        self.id != 0
            && (self.title_len as usize) <= TITLE_MAX
            && self.title_len > 0
            && (DUR_MIN..=DUR_MAX).contains(&self.duration_sec)
            && self
                .title
                .iter()
                .take(self.title_len as usize)
                .all(|b| (0x20..=0x7E).contains(b))
    }

    fn encode(&self, out: &mut [u8]) {
        out[0..4].copy_from_slice(&self.id.to_le_bytes());
        out[4] = self.title_len;
        out[5..5 + TITLE_MAX].copy_from_slice(&self.title);
        out[5 + TITLE_MAX..5 + TITLE_MAX + 4].copy_from_slice(&self.duration_sec.to_le_bytes());
    }

    fn decode(bytes: &[u8]) -> Option<TrackRecord> {
        if bytes.len() < RECORD_BYTES {
            return None;
        }
        let mut r = TrackRecord::EMPTY;
        r.id = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        r.title_len = bytes[4];
        r.title.copy_from_slice(&bytes[5..5 + TITLE_MAX]);
        r.duration_sec = u32::from_le_bytes([
            bytes[5 + TITLE_MAX],
            bytes[5 + TITLE_MAX + 1],
            bytes[5 + TITLE_MAX + 2],
            bytes[5 + TITLE_MAX + 3],
        ]);
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
            StoreState::Untouched => "track store not read yet",
            StoreState::Empty => "no stored tracks",
            StoreState::Loaded => "tracks loaded from settings store",
            StoreState::ReadRefused => "settings read refused; no tracks loaded",
            StoreState::Invalid => "stored track data failed validation; no tracks loaded",
            StoreState::Written => "track written to settings store",
            StoreState::WriteRefused => "settings write refused; track not persisted",
            StoreState::Full => "store full; track not written",
        }
    }
}

/// Bounded track table plus an honest account of its last load or save.
#[derive(Clone, Copy)]
pub struct TrackStore {
    pub records: [TrackRecord; MAX_TRACKS],
    len: u8,
    next_id: u32,
    pub state: StoreState,
}

impl TrackStore {
    pub const EMPTY: TrackStore = TrackStore {
        records: [TrackRecord::EMPTY; MAX_TRACKS],
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
        std::settings_set(STORE_CAP_WRITE, STORE_KEY, &blob[..used]).is_ok()
    }

    /// Reads the library back out of the settings store, validating the blob.
    pub fn load() -> Self {
        let mut store = TrackStore::EMPTY;
        let mut blob = [0u8; STORE_BLOB_BYTES];
        match std::settings_get(STORE_CAP_READ, STORE_KEY, &mut blob) {
            Ok(0) => store.state = StoreState::Empty,
            Ok(n) => {
                let header_count = blob[1] as usize;
                let bad = n < 2
                    || n > STORE_BLOB_BYTES
                    || blob[0] != STORE_VERSION
                    || header_count > MAX_TRACKS
                    || n != 2 + header_count * RECORD_BYTES;
                if bad {
                    store.state = StoreState::Invalid;
                } else {
                    let mut ok = true;
                    for i in 0..header_count {
                        let start = 2 + i * RECORD_BYTES;
                        if TrackRecord::decode(&blob[start..start + RECORD_BYTES]).is_none() {
                            ok = false;
                            break;
                        }
                    }
                    if !ok {
                        store.state = StoreState::Invalid;
                    } else {
                        for i in 0..header_count {
                            let start = 2 + i * RECORD_BYTES;
                            if let Some(r) = TrackRecord::decode(&blob[start..start + RECORD_BYTES]) {
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
                        store.state = if header_count == 0 {
                            StoreState::Empty
                        } else {
                            StoreState::Loaded
                        };
                    }
                }
            }
            Err(_) => store.state = StoreState::ReadRefused,
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
        MAX_TRACKS
    }

    pub fn is_full(&self) -> bool {
        self.len as usize >= MAX_TRACKS
    }

    /// The word to log for the store's last outcome (one phrasing, in one place).
    pub fn last_reason(&self) -> &'static str {
        self.state.reason()
    }

    pub fn get(&self, idx: usize) -> Option<&TrackRecord> {
        self.records.get(idx).filter(|_| idx < self.len as usize)
    }

    /// Adds one track and rewrites the blob. Joins the in-memory list only after
    /// the write is acknowledged, so the list is exactly what storage holds.
    /// When full it reports `Full` and refuses rather than dropping a track.
    pub fn add(&mut self, title: &[u8], duration_sec: u32) -> bool {
        if self.is_full() {
            self.state = StoreState::Full;
            return false;
        }
        if title.is_empty() || title.len() > TITLE_MAX || !(DUR_MIN..=DUR_MAX).contains(&duration_sec) {
            self.state = StoreState::Invalid;
            return false;
        }
        let idx = self.len as usize;
        let id = self.next_id.max(1);
        if id >= u32::MAX {
            self.state = StoreState::Full;
            return false;
        }
        let mut r = TrackRecord::EMPTY;
        r.id = id;
        let n = title.len().min(TITLE_MAX);
        r.title[..n].copy_from_slice(&title[..n]);
        r.title_len = n as u8;
        r.duration_sec = duration_sec;
        self.records[idx] = r;
        self.len += 1;
        self.next_id = id + 1;
        if self.write() {
            self.state = StoreState::Written;
            true
        } else {
            self.len -= 1;
            self.records[idx] = TrackRecord::EMPTY;
            self.state = StoreState::WriteRefused;
            false
        }
    }

    /// Removes one track by list index and rewrites the blob. Reports whether the
    /// deletion reached storage, restoring the track if it did not.
    pub fn remove(&mut self, idx: usize) -> bool {
        let len = self.len as usize;
        if idx >= len {
            return false;
        }
        let removed = self.records[idx];
        self.records.copy_within(idx + 1..len, idx);
        self.records[len - 1] = TrackRecord::EMPTY;
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
}
