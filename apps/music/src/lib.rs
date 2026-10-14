#![no_std]

pub use interos_std as std;

/// `FileSystemRead` slot: accepted by `sys_settings_get`.
pub const STORE_CAP_READ: usize = 4;
/// `FileSystemWrite` slot: accepted by `sys_settings_set`.
pub const STORE_CAP_WRITE: usize = 5;

/// Settings key the whole library is serialized under.
pub const STORE_KEY: &str = "music.library.v1";
/// Blob format version; a blob not starting with this byte is not ours.
pub const STORE_VERSION: u8 = 1;

/// Title bytes one track stores.
pub const TITLE_MAX: usize = 40;

/// Library capacity, chosen so the serialized blob with escapes stays safely
/// within the kernel's 512-byte settings value cap.
pub const MAX_TRACKS: usize = 8;

/// Record width: id(u32,4) + title length(1) + TITLE_MAX(40) + duration(u32,4) = 49 bytes.
pub const RECORD_BYTES: usize = 4 + 1 + TITLE_MAX + 4;

/// Raw blob width = version(1) + count(1) + up to MAX_TRACKS records.
pub const STORE_BLOB_BYTES: usize = 2 + MAX_TRACKS * RECORD_BYTES;

pub const DUR_MIN: u32 = 1;
pub const DUR_MAX: u32 = 100_000;

pub const ESCAPE_BYTE: u8 = 0xFE;
pub const ESCAPED_NEWLINE: u8 = 0x8A;
pub const ESCAPED_ESCAPE: u8 = 0xEE;

/// Escapes any 0x0A ('\n') or 0xFE bytes in a raw store blob so the serialized
/// settings value never contains a newline.
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

/// Unescapes a store blob read from the settings store. Handles both escaped
/// payloads and legacy unescaped blobs transparently.
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

/// One locally stored track: a title and a duration.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TrackRecord {
    pub id: u32,
    pub title: [u8; TITLE_MAX],
    pub title_len: u8,
    pub duration_sec: u32,
}

impl TrackRecord {
    pub const EMPTY: TrackRecord = TrackRecord {
        id: 0,
        title: [0; TITLE_MAX],
        title_len: 0,
        duration_sec: 0,
    };

    pub fn from_parts(id: u32, title: &[u8], duration_sec: u32) -> Self {
        let mut r = TrackRecord::EMPTY;
        r.id = id;
        let n = title.len().min(TITLE_MAX);
        r.title[..n].copy_from_slice(&title[..n]);
        r.title_len = n as u8;
        r.duration_sec = duration_sec;
        r
    }

    pub fn title(&self) -> &[u8] {
        &self.title[..self.title_len as usize]
    }

    pub fn duration_sec(&self) -> u32 {
        self.duration_sec
    }

    pub fn valid(&self) -> bool {
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

    pub fn encode(&self, out: &mut [u8]) {
        out[0..4].copy_from_slice(&self.id.to_le_bytes());
        out[4] = self.title_len;
        out[5..5 + TITLE_MAX].copy_from_slice(&self.title);
        out[5 + TITLE_MAX..5 + TITLE_MAX + 4].copy_from_slice(&self.duration_sec.to_le_bytes());
    }

    pub fn decode(bytes: &[u8]) -> Option<TrackRecord> {
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

/// What the store's last contact with platform storage actually did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    pub len: u8,
    pub next_id: u32,
    pub state: StoreState,
}

impl TrackStore {
    pub const EMPTY: TrackStore = TrackStore {
        records: [TrackRecord::EMPTY; MAX_TRACKS],
        len: 0,
        next_id: 1,
        state: StoreState::Untouched,
    };

    pub fn encode(&self) -> ([u8; STORE_BLOB_BYTES], usize) {
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

    pub fn write(&mut self) -> bool {
        let (raw, used) = self.encode();
        let mut enc = [0u8; 512];
        let enc_len = match escape_store_blob(&raw[..used], &mut enc) {
            Ok(n) => n,
            Err(_) => return false,
        };
        std::settings_set(STORE_CAP_WRITE, STORE_KEY, &enc[..enc_len]).is_ok()
    }

    /// Reads the library back out of the settings store, validating the blob.
    pub fn load() -> Self {
        let mut store = TrackStore::EMPTY;
        let mut enc = [0u8; 512];
        match std::settings_get(STORE_CAP_READ, STORE_KEY, &mut enc) {
            Ok(0) => store.state = StoreState::Empty,
            Ok(n) => {
                let mut blob = [0u8; STORE_BLOB_BYTES];
                let dec_len = match unescape_store_blob(&enc[..n], &mut blob) {
                    Ok(m) => m,
                    Err(_) => {
                        store.state = StoreState::Invalid;
                        return store;
                    }
                };
                let header_count = blob[1] as usize;
                let bad = dec_len < 2
                    || dec_len > STORE_BLOB_BYTES
                    || blob[0] != STORE_VERSION
                    || header_count > MAX_TRACKS
                    || dec_len != 2 + header_count * RECORD_BYTES;
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

    pub fn last_reason(&self) -> &'static str {
        self.state.reason()
    }

    pub fn get(&self, idx: usize) -> Option<&TrackRecord> {
        self.records.get(idx).filter(|_| idx < self.len as usize)
    }

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
        let r = TrackRecord::from_parts(id, title, duration_sec);
        if !r.valid() {
            self.state = StoreState::Invalid;
            return false;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_and_unescape_roundtrip_with_0x0a() {
        let raw = [STORE_VERSION, 10, 0x0A, 42, 0xFE, 99];
        let mut enc = [0u8; 32];
        let enc_len = escape_store_blob(&raw, &mut enc).expect("escape");
        assert!(!enc[..enc_len].iter().any(|&b| b == b'\n'));

        let mut dec = [0u8; 32];
        let dec_len = unescape_store_blob(&enc[..enc_len], &mut dec).expect("unescape");
        assert_eq!(&dec[..dec_len], &raw[..]);
    }

    #[test]
    fn test_track_record_encode_decode() {
        let rec = TrackRecord::from_parts(1, b"Nocturne in C-sharp Minor", 245);
        assert!(rec.valid());

        let mut buf = [0u8; RECORD_BYTES];
        rec.encode(&mut buf);

        let dec = TrackRecord::decode(&buf).expect("decode valid track");
        assert_eq!(dec.id, 1);
        assert_eq!(dec.title(), b"Nocturne in C-sharp Minor");
        assert_eq!(dec.duration_sec(), 245);
    }

    #[test]
    fn test_track_record_with_0x0a_duration_escaped() {
        // duration 10 has byte 0 = 0x0A ('\n')
        let rec = TrackRecord::from_parts(10, b"Short Jingle", 10);
        assert!(rec.valid());

        let mut buf = [0u8; RECORD_BYTES];
        rec.encode(&mut buf);

        // buf MUST contain 0x0A in the raw duration bytes
        assert!(buf.iter().any(|&b| b == b'\n'));

        // Escape it
        let mut enc = [0u8; RECORD_BYTES + 16];
        let enc_len = escape_store_blob(&buf, &mut enc).expect("escape");

        // Escaped output MUST have NO 0x0A
        assert!(!enc[..enc_len].iter().any(|&b| b == b'\n'));

        // Unescape it
        let mut dec_buf = [0u8; RECORD_BYTES];
        let dec_len = unescape_store_blob(&enc[..enc_len], &mut dec_buf).expect("unescape");
        assert_eq!(dec_len, RECORD_BYTES);

        let dec = TrackRecord::decode(&dec_buf).expect("decode unescaped track");
        assert_eq!(dec.id, 10);
        assert_eq!(dec.title(), b"Short Jingle");
        assert_eq!(dec.duration_sec(), 10);
    }
}
