#![no_std]

pub use interos_std as std;

/// The app's view of the platform's design system.
pub mod theme;

/// The library's documents in the shared storage area, and the import that
/// reads the audio files the device holds.
pub mod library;

/// Streaming playback of an imported file over the platform's PCM output.
pub mod playback;

/// Bounded zero-heap extraction of ID3v2 APIC and FLAC PICTURE album art.
pub mod art;

use library::ImportOutcome;

/// Capacity of the now-playing time line.
pub const CAPTION_MAX: usize = 24;

/// Capacity of the now-playing line (artist prefix plus the time line).
pub const NOW_PLAYING_MAX: usize = 64;

/// Capacity of a transient feedback line.
pub const FEEDBACK_MAX: usize = 64;

/// `FileSystemRead` slot: accepted by `sys_settings_get`.
pub const STORE_CAP_READ: usize = 4;
/// `FileSystemWrite` slot: accepted by `sys_settings_set`.
pub const STORE_CAP_WRITE: usize = 5;

/// Settings key the whole library is serialized under.
pub const STORE_KEY: &str = "music.library.v1";
/// Blob format version; a blob not starting with this byte is not ours.
pub const STORE_VERSION: u8 = 1;

/// Settings key for currently playing track title.
pub const NOW_PLAYING_TITLE_KEY: &str = "audio.now_playing.title";
/// Settings key for currently playing artist name.
pub const NOW_PLAYING_ARTIST_KEY: &str = "audio.now_playing.artist";
/// Settings key for playback state ("playing", "paused", "stopped").
pub const NOW_PLAYING_STATE_KEY: &str = "audio.now_playing.state";
/// Settings key for currently active audio app package.
pub const NOW_PLAYING_APP_KEY: &str = "audio.now_playing.app";
/// Settings key for system ringtone path.
pub const RINGTONE_KEY: &str = "audio.ringtone";
/// Legacy/telephony ringtone setting key for cross-subsystem compatibility.
pub const TELEPHONY_RINGTONE_KEY: &str = "telephony.ringtone";

/// Publishes now-playing metadata to the system settings store.
///
/// WHY this is published to the settings store: the Launcher Notification Shade
/// reads these keys to present a live Now Playing media card without requiring
/// a direct peer-to-peer IPC connection when the music player is backgrounded.
pub fn publish_now_playing(title: &[u8], artist: &[u8], state: &[u8]) -> bool {
    let t_ok = std::settings_set(NOW_PLAYING_TITLE_KEY, title).is_ok();
    let a_ok = std::settings_set(NOW_PLAYING_ARTIST_KEY, artist).is_ok();
    let s_ok = std::settings_set(NOW_PLAYING_STATE_KEY, state).is_ok();
    let p_ok = std::settings_set(NOW_PLAYING_APP_KEY, b"org.interos.music").is_ok();
    t_ok && a_ok && s_ok && p_ok
}

/// Sets the given audio track or document path as the system-wide ringtone.
///
/// Writes both `audio.ringtone` and `telephony.ringtone` for seamless
/// cross-application compatibility with the dialer and system settings.
pub fn set_ringtone(path_or_title: &[u8]) -> bool {
    if path_or_title.is_empty() {
        return false;
    }
    let a = std::settings_set(RINGTONE_KEY, path_or_title).is_ok();
    let b = std::settings_set(TELEPHONY_RINGTONE_KEY, path_or_title).is_ok();
    a || b
}

/// Title bytes one track stores.
pub const TITLE_MAX: usize = 40;

/// The longest source path one track holds.
///
/// WHY the path stays in the record and not in the settings blob: the store's
/// whole value must fit the platform's 512-byte settings limit, and a path per
/// track would push a full library past it. The path is resolved from the
/// shared storage, which is where the file itself lives.
pub const SOURCE_MAX: usize = 96;

/// The position a stream of consumed frames represents, in milliseconds.
///
/// The frames are the device's count of what it took from the ring, so this is
/// a position read from the stream rather than an estimate from wall time.
pub fn position_millis(frames_consumed: u64) -> u64 {
    frames_consumed.saturating_mul(1_000) / interos_std::wav::OUTPUT_SAMPLE_RATE as u64
}

/// Appends `bytes` to `out` at `at`, keeping the cursor inside the buffer.
fn push_bytes(out: &mut [u8], at: &mut usize, bytes: &[u8]) {
    for &byte in bytes {
        if *at >= out.len() {
            return;
        }
        out[*at] = byte;
        *at += 1;
    }
}

/// Appends the decimal form of `value` to `out` at `at`.
fn push_decimal(out: &mut [u8], at: &mut usize, value: u32) {
    let mut digits = [0u8; 10];
    let mut n = 0usize;
    let mut rest = value;
    loop {
        digits[n] = b'0' + (rest % 10) as u8;
        rest /= 10;
        n += 1;
        if rest == 0 {
            break;
        }
    }
    for i in 0..n {
        if *at >= out.len() {
            return;
        }
        out[*at] = digits[n - 1 - i];
        *at += 1;
    }
}

/// Formats `secs` as `m:ss` (or `h:mm:ss` for an hour or more).
pub fn format_mmss(out: &mut [u8; 12], secs: u32) -> usize {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    let mut n = 0usize;
    if h > 0 {
        let mut hb = [0u8; 4];
        let hn = format_u32(&mut hb, h);
        out[..hn].copy_from_slice(&hb[..hn]);
        n += hn;
        out[n] = b':';
        n += 1;
        out[n] = b'0' + (m / 10) as u8;
        out[n + 1] = b'0' + (m % 10) as u8;
        n += 2;
    } else {
        let mut mb = [0u8; 4];
        let mn = format_u32(&mut mb, m);
        out[..mn].copy_from_slice(&mb[..mn]);
        n += mn;
    }
    out[n] = b':';
    out[n + 1] = b'0' + (s / 10) as u8;
    out[n + 2] = b'0' + (s % 10) as u8;
    n + 3
}

/// Decimal `v` into a four-byte scratch buffer, no leading zeros.
fn format_u32(scratch: &mut [u8; 4], v: u32) -> usize {
    let mut tmp = [0u8; 4];
    let mut n = 0usize;
    let mut x = v.min(9999);
    loop {
        tmp[n] = b'0' + (x % 10) as u8;
        x /= 10;
        n += 1;
        if x == 0 {
            break;
        }
    }
    for i in 0..n {
        scratch[i] = tmp[n - 1 - i];
    }
    n
}

/// The time line the now-playing row draws.
///
/// WHY the total comes only from the decoder: a playing stream knows how many
/// frames it holds (WAV and FLAC report a length, MP3 does not), while the
/// track record may hold a number the person typed when the track was added.
/// Substituting that number for a total the decoder did not report would put a
/// length on screen the file never claimed. When the decoder has no total the
/// line says `--:--`; when nothing is playing it reports the stored length or
/// says the length was never read.
pub fn playback_caption(
    position_ms: Option<u64>,
    total_ms: Option<u64>,
    record_duration_sec: u32,
    record_duration_known: bool,
    out: &mut [u8; CAPTION_MAX],
) -> usize {
    let mut at = 0usize;
    if let Some(position_ms) = position_ms {
        let mut buf = [0u8; 12];
        let secs = (position_ms / 1_000).min(u32::MAX as u64) as u32;
        let n = format_mmss(&mut buf, secs);
        push_bytes(out, &mut at, &buf[..n]);
        push_bytes(out, &mut at, b" / ");
        match total_ms {
            Some(total_ms) => {
                let secs = (total_ms / 1_000).min(u32::MAX as u64) as u32;
                let n = format_mmss(&mut buf, secs);
                push_bytes(out, &mut at, &buf[..n]);
            }
            // WHY the decoder's silence is shown as silence: an MP3 stream has
            // no trusted total, and the record's stored number is not one.
            None => push_bytes(out, &mut at, b"--:--"),
        }
        return at;
    }
    if record_duration_known {
        let mut buf = [0u8; 12];
        let n = format_mmss(&mut buf, record_duration_sec);
        push_bytes(out, &mut at, b"Duration ");
        push_bytes(out, &mut at, &buf[..n]);
    } else {
        push_bytes(out, &mut at, b"Duration not read");
    }
    at
}

/// Builds the now-playing line, naming an artist only when one is known.
pub fn now_playing_line(
    artist: Option<&[u8]>,
    position_ms: Option<u64>,
    total_ms: Option<u64>,
    record_duration_sec: u32,
    record_duration_known: bool,
    out: &mut [u8; NOW_PLAYING_MAX],
) -> usize {
    let mut at = 0usize;
    if let Some(artist) = artist {
        push_bytes(out, &mut at, artist);
        push_bytes(out, &mut at, b" - ");
    }
    let mut caption = [0u8; CAPTION_MAX];
    let n = playback_caption(
        position_ms,
        total_ms,
        record_duration_sec,
        record_duration_known,
        &mut caption,
    );
    push_bytes(out, &mut at, &caption[..n]);
    at
}

/// The position of `current` in the queue the list shows, one-based.
///
/// WHY the order table: a search hides tracks, so the current track's place in
/// the visible queue is not its index in the store.
pub fn queue_position(order: &[u8], order_len: usize, current: usize) -> Option<(usize, usize)> {
    let len = order_len.min(order.len());
    for n in 0..len {
        if order[n] as usize == current {
            return Some((n + 1, len));
        }
    }
    None
}

/// What the import did, in words the feedback line may show.
///
/// WHY the refused count is named: a person who put audio in shared storage and
/// got a refused settings write must not be told that no audio was found.
pub fn import_feedback(outcome: ImportOutcome, out: &mut [u8]) -> usize {
    let mut at = 0usize;
    match outcome {
        ImportOutcome::Imported(n) => {
            push_bytes(out, &mut at, b"Imported ");
            push_decimal(out, &mut at, n as u32);
            push_bytes(
                out,
                &mut at,
                if n == 1 {
                    b" track from storage"
                } else {
                    b" tracks from storage"
                },
            );
        }
        ImportOutcome::PartiallyImported(n, refused) => {
            push_bytes(out, &mut at, b"Imported ");
            push_decimal(out, &mut at, n as u32);
            push_bytes(out, &mut at, if n == 1 { b" track; " } else { b" tracks; " });
            push_decimal(out, &mut at, refused as u32);
            push_bytes(out, &mut at, b" refused");
        }
        ImportOutcome::WriteRefused(found) => {
            push_bytes(out, &mut at, b"Found ");
            push_decimal(out, &mut at, found as u32);
            push_bytes(
                out,
                &mut at,
                if found == 1 {
                    b" audio file; store refused"
                } else {
                    b" audio files; store refused"
                },
            );
        }
        ImportOutcome::ListRefused => {
            push_bytes(out, &mut at, b"Storage list refused; nothing imported");
        }
        ImportOutcome::NoneFound => {
            push_bytes(out, &mut at, b"No new audio files in shared storage");
        }
    }
    at
}

/// What the player says when a track played out.
///
/// WHY the title is only appended when the next track started: the older
/// wording said "track ended" whether or not anything followed it, which read
/// as an auto-advance that never happened.
pub fn ended_feedback(next_title: Option<&[u8]>, out: &mut [u8]) -> usize {
    let mut at = 0usize;
    match next_title {
        Some(title) => {
            push_bytes(out, &mut at, b"Track ended - playing: ");
            push_bytes(out, &mut at, title);
        }
        None => push_bytes(out, &mut at, b"Track ended - no playable track remains"),
    }
    at
}

/// The artist the UI may name, or `None` when none is known.
///
/// WHY a control byte disqualifies the value: an artist name is text, and a
/// run of binary would be drawn as glyph soup that claims to be a credit.
pub fn known_artist(artist: &[u8]) -> Option<&[u8]> {
    if artist.is_empty() || !artist.iter().all(|b| (0x20..=0x7e).contains(b)) {
        None
    } else {
        Some(artist)
    }
}

/// The caption the artwork card shows when it cannot show embedded art.
///
/// WHY this is a caption and not a cover: the platform's decoders expose no
/// ID3 APIC or FLAC PICTURE block to this app, so there is no artwork to draw.
/// The card stays a designed placeholder and says which fact it is missing.
pub fn artwork_caption(has_track: bool, embedded_art: bool) -> &'static [u8] {
    if embedded_art {
        b""
    } else if has_track {
        b"No embedded album art"
    } else {
        b"No track selected"
    }
}

/// Library capacity, chosen so the serialized blob with escapes stays safely
/// within the kernel's 512-byte settings value cap.
pub const MAX_TRACKS: usize = 8;

/// Record width: id(u32,4) + title length(1) + TITLE_MAX(40) + duration(u32,4) = 49 bytes.
pub const RECORD_BYTES: usize = 4 + 1 + TITLE_MAX + 4;

/// Raw blob width = version(1) + count(1) + up to MAX_TRACKS records.
pub const STORE_BLOB_BYTES: usize = 2 + MAX_TRACKS * RECORD_BYTES;

pub const DUR_MIN: u32 = 1;
pub const DUR_MAX: u32 = 100_000;

/// How the library is ordered.
///
/// WHY this is an enum and not a number: the order is a choice the user
/// makes, and the names are what the screen shows next to the control that
/// changes it. A number would leave the screen to spell it out again.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortMode {
    /// The order the tracks were added. This is the queue's own order.
    Added,
    /// Alphabetical by title.
    Title,
    /// Shortest first.
    Duration,
}

impl SortMode {
    /// The label the sort control shows.
    pub const fn label(self) -> &'static str {
        match self {
            SortMode::Added => "ADDED",
            SortMode::Title => "TITLE",
            SortMode::Duration => "LENGTH",
        }
    }

    /// The next mode the control cycles through.
    pub const fn next(self) -> SortMode {
        match self {
            SortMode::Added => SortMode::Title,
            SortMode::Title => SortMode::Duration,
            SortMode::Duration => SortMode::Added,
        }
    }
}

/// Case-insensitive substring match.
///
/// WHY the search is case-insensitive: a person looking for "nocturne" types
/// it in lower case whatever the track is called, and a search that only
/// matched the exact case would report that the library does not hold the
/// track it holds.
pub fn contains_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    for i in 0..=(haystack.len() - needle.len()) {
        let mut matched = true;
        for j in 0..needle.len() {
            if !haystack[i + j].eq_ignore_ascii_case(&needle[j]) {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}

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
    /// The file the track was imported from. Empty when the track came from a
    /// library that predates sources, in which case the player resolves it
    /// from the shared storage by title.
    pub source: [u8; SOURCE_MAX],
    pub source_len: u8,
}

impl TrackRecord {
    pub const EMPTY: TrackRecord = TrackRecord {
        id: 0,
        title: [0; TITLE_MAX],
        title_len: 0,
        duration_sec: 0,
        source: [0; SOURCE_MAX],
        source_len: 0,
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

    /// The artist this record can claim.
    ///
    /// WHY this is empty: the library blob has no artist field -- the 512-byte
    /// settings limit leaves no room for one -- and the platform's decoders
    /// expose no ID3/FLAC text metadata to this app. An empty answer is honest;
    /// the old player published the literal "Local Library" as an artist,
    /// which named the store rather than the performer.
    pub fn artist(&self) -> &[u8] {
        b""
    }

    /// Records the file this track plays from.
    ///
    /// Returns `false` when the path is not text the platform's filesystem
    /// can open, so a malformed import is refused instead of stored.
    pub fn set_source(&mut self, path: &[u8]) -> bool {
        if path.is_empty() || path.len() > SOURCE_MAX || path[0] != b'/' {
            return false;
        }
        if !path.iter().all(|b| (0x20..=0x7e).contains(b)) {
            return false;
        }
        self.source[..path.len()].copy_from_slice(path);
        self.source_len = path.len() as u8;
        true
    }

    pub fn source(&self) -> &[u8] {
        &self.source[..self.source_len as usize]
    }

    pub fn duration_sec(&self) -> u32 {
        self.duration_sec
    }

    /// Whether the track's length is known.
    ///
    /// WHY zero means "unknown" and not "zero seconds": a track imported
    /// from the shared storage has a real file behind it and no length
    /// anybody has read, and a length of zero would be a claim that the
    /// track is empty.
    pub fn duration_known(&self) -> bool {
        self.duration_sec != 0
    }

    pub fn valid(&self) -> bool {
        self.id != 0
            && (self.title_len as usize) <= TITLE_MAX
            && self.title_len > 0
            && (self.source_len as usize) <= SOURCE_MAX
            // A duration of zero is the "not read" marker, not a length.
            && self.duration_sec <= DUR_MAX
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
        std::settings_set(STORE_KEY, &enc[..enc_len]).is_ok()
    }

    /// Reads the library back out of the settings store, validating the blob.
    pub fn load() -> Self {
        let mut store = TrackStore::EMPTY;
        let mut enc = [0u8; 512];
        match std::settings_get(STORE_KEY, &mut enc) {
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
        self.add_with_source(title, duration_sec, b"")
    }

    /// Adds a track together with the file it plays from.
    pub fn add_with_source(
        &mut self,
        title: &[u8],
        duration_sec: u32,
        source: &[u8],
    ) -> bool {
        if self.is_full() {
            self.state = StoreState::Full;
            return false;
        }
        // WHY a duration of zero is accepted: a track imported from the
        // shared storage has a real file behind it and a length nobody has
        // read. Refusing it would leave the file out of the library, and
        // storing a made-up length would be worse.
        let known = (DUR_MIN..=DUR_MAX).contains(&duration_sec);
        if title.is_empty() || title.len() > TITLE_MAX || (!known && duration_sec != 0) {
            self.state = StoreState::Invalid;
            return false;
        }
        let idx = self.len as usize;
        let id = self.next_id.max(1);
        // WHY `==` rather than `>=`: u32::MAX is the largest value of the type,
        // so `>=` can only ever be true when the two are equal. The guard exists
        // because the next id is `id + 1`, which would wrap at u32::MAX.
        if id == u32::MAX {
            self.state = StoreState::Full;
            return false;
        }
        let mut r = TrackRecord::from_parts(id, title, duration_sec);
        if !source.is_empty() && !r.set_source(source) {
            self.state = StoreState::Invalid;
            return false;
        }
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

    /// Moves the track at `idx` to `to`, keeping every other track in place.
    ///
    /// WHY this is the queue's reorder action: the library is one ordered
    /// list and the queue is that list in play order, so a track that moves
    /// up in the queue moves up in the library.
    ///
    /// Returns `true` only when the new order was persisted. A refused
    /// write leaves the queue exactly as it was.
    pub fn move_to(&mut self, idx: usize, to: usize) -> bool {
        let original = self.records;
        if !move_record(&mut self.records, idx, to) {
            return false;
        }
        if self.write() {
            self.state = StoreState::Written;
            true
        } else {
            self.records = original;
            self.state = StoreState::WriteRefused;
            false
        }
    }

    /// Orders the library by the order the tracks were added.
    ///
    /// WHY the id is the order: every track is given the next id when it is
    /// added, so the ids are the order the tracks arrived in and nothing
    /// else has to be stored to remember it.
    pub fn sort_by_added(&mut self) -> bool {
        self.sort_records(added_less)
    }

    /// Orders the library by title, keeping tracks with the same title in the
    /// order they were added.
    pub fn sort_by_title(&mut self) -> bool {
        self.sort_records(title_less)
    }

    /// Orders the library by length, shortest first.
    pub fn sort_by_duration(&mut self) -> bool {
        self.sort_records(duration_less)
    }

    /// Sorts the records with `less` and persists the new order.
    fn sort_records(&mut self, less: fn(&TrackRecord, &TrackRecord) -> bool) -> bool {
        let len = self.len as usize;
        if len < 2 {
            return true;
        }
        let original = self.records;
        if !sort_records(&mut self.records, len, less) {
            return false;
        }
        if self.write() {
            self.state = StoreState::Written;
            true
        } else {
            self.records = original;
            self.state = StoreState::WriteRefused;
            false
        }
    }
}

/// Moves the record at `idx` to `to` inside `records`, keeping the rest in
/// place.
///
/// WHY the permutation and the persistence are separate functions: the
/// permutation is the queue's behaviour and can be checked on its own, and
/// the persistence is the device's answer about it.
pub fn move_record(records: &mut [TrackRecord; MAX_TRACKS], idx: usize, to: usize) -> bool {
    let len = records.len();
    if idx >= len || to >= len || idx == to {
        return false;
    }
    let moved = records[idx];
    if idx < to {
        records.copy_within(idx + 1..=to, idx);
    } else {
        records.copy_within(to..idx, to + 1);
    }
    records[to] = moved;
    true
}

/// Sorts the first `len` records with `less`, keeping equal records in their
/// old order.
///
/// WHY an insertion sort: the library holds eight tracks, and a sort that
/// allocates is a sort the render path cannot pay for.
pub fn sort_records(
    records: &mut [TrackRecord; MAX_TRACKS],
    len: usize,
    less: fn(&TrackRecord, &TrackRecord) -> bool,
) -> bool {
    if len < 2 {
        return true;
    }
    for a in 1..len {
        let key = records[a];
        let mut b = a;
        while b > 0 && less(&key, &records[b - 1]) {
            records[b] = records[b - 1];
            b -= 1;
        }
        records[b] = key;
    }
    true
}

/// Whether `a` sorts before `b` by title, ignoring case.
pub fn title_less(a: &TrackRecord, b: &TrackRecord) -> bool {
    let at = a.title();
    let bt = b.title();
    let n = at.len().min(bt.len());
    for i in 0..n {
        let x = at[i].to_ascii_lowercase();
        let y = bt[i].to_ascii_lowercase();
        if x != y {
            return x < y;
        }
    }
    at.len() < bt.len()
}

/// Whether `a` sorts before `b` by the order the tracks were added.
///
/// WHY the id is the order: every track is given the next id when it is
/// added, so the ids are the order the tracks arrived in.
pub fn added_less(a: &TrackRecord, b: &TrackRecord) -> bool {
    a.id < b.id
}

/// Whether `a` sorts before `b` by length.
pub fn duration_less(a: &TrackRecord, b: &TrackRecord) -> bool {
    a.duration_sec() < b.duration_sec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_track_nobody_measured_is_stored_as_an_unknown_length() {
        // WHY this matters: a track imported from the shared storage has a
        // real file behind it and a length nobody read. Storing a length of
        // zero would claim the track is empty, and refusing the track would
        // leave the file out of the library.
        let unknown = TrackRecord::from_parts(1, b"nocturne", 0);
        assert!(unknown.valid());
        assert!(!unknown.duration_known());

        let known = TrackRecord::from_parts(2, b"nocturne", 225);
        assert!(known.valid());
        assert!(known.duration_known());
    }

    #[test]
    fn a_length_outside_the_platform_s_range_is_refused() {
        let too_long = TrackRecord::from_parts(1, b"nocturne", DUR_MAX + 1);
        assert!(!too_long.valid());
    }

    #[test]
    fn moving_a_track_up_reorders_the_queue() {
        // WHY this matters: the queue is the order the tracks are played in,
        // and the reorder action has to move the track and leave every
        // other track where it was.
        let mut records = [TrackRecord::EMPTY; MAX_TRACKS];
        records[0] = TrackRecord::from_parts(1, b"first", 10);
        records[1] = TrackRecord::from_parts(2, b"second", 20);
        records[2] = TrackRecord::from_parts(3, b"third", 30);

        assert!(move_record(&mut records, 2, 1));
        assert_eq!(records[0].title(), b"first");
        assert_eq!(records[1].title(), b"third");
        assert_eq!(records[2].title(), b"second");

        // A move that is out of the queue changes nothing.
        let before = records;
        assert!(!move_record(&mut records, 0, 9));
        assert_eq!(records[0].title(), before[0].title());
    }

    #[test]
    fn sorting_by_title_orders_the_queue_and_keeps_equal_titles_in_order() {
        let mut records = [TrackRecord::EMPTY; MAX_TRACKS];
        records[0] = TrackRecord::from_parts(1, b"banana", 10);
        records[1] = TrackRecord::from_parts(2, b"Apple", 20);
        records[2] = TrackRecord::from_parts(3, b"apple", 30);

        assert!(sort_records(&mut records, 3, title_less));
        assert_eq!(records[0].title(), b"Apple");
        assert_eq!(records[1].title(), b"apple");
        assert_eq!(records[2].title(), b"banana");
    }

    #[test]
    fn sorting_by_added_restores_the_order_the_tracks_arrived_in() {
        // WHY the id is the order: every track is given the next id when it
        // is added, so the ids are the order the tracks arrived in and
        // nothing else has to be stored to remember it.
        let mut records = [TrackRecord::EMPTY; MAX_TRACKS];
        records[0] = TrackRecord::from_parts(3, b"third", 10);
        records[1] = TrackRecord::from_parts(1, b"first", 20);
        records[2] = TrackRecord::from_parts(2, b"second", 30);

        assert!(sort_records(&mut records, 3, added_less));
        assert_eq!(records[0].title(), b"first");
        assert_eq!(records[1].title(), b"second");
        assert_eq!(records[2].title(), b"third");
    }

    #[test]
    fn the_search_matches_a_title_whatever_the_case_it_was_typed_in() {
        // WHY this matters: a person looking for "nocturne" types it in
        // lower case whatever the track is called, and a search that only
        // matched the exact case would report that the library does not
        // hold the track it holds.
        assert!(contains_ci(b"Nocturne in C", b"nocturne"));
        assert!(contains_ci(b"Nocturne in C", b"NOCTURNE"));
        assert!(contains_ci(b"Nocturne in C", b"in c"));
        assert!(!contains_ci(b"Nocturne in C", b"prelude"));
        assert!(contains_ci(b"Nocturne in C", b""));
    }

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
    fn a_track_remembers_the_file_it_was_imported_from() {
        let mut record = TrackRecord::from_parts(1, b"Tone", 0);
        assert_eq!(record.source(), b"");
        assert!(record.set_source(b"/storage/audio/tone.wav"));
        assert_eq!(record.source(), b"/storage/audio/tone.wav");
        // A path the platform cannot open is refused rather than stored as a
        // promise the player cannot keep.
        assert!(!record.set_source(b"/storage/audio/\x01bad.wav"));
        assert!(!record.set_source(&[b'a'; SOURCE_MAX + 1]));
    }

    #[test]
    fn a_playback_position_is_derived_from_the_frames_the_device_took() {
        // WHY the device's own counter: the player knows how many frames it
        // handed over, and a position read from that counter is a fact about
        // the stream rather than an estimate from wall time.
        assert_eq!(position_millis(0), 0);
        assert_eq!(position_millis(24_000), 500);
        assert_eq!(position_millis(48_000), 1_000);
        assert_eq!(position_millis(72_000), 1_500);
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

    #[test]
    fn the_time_caption_reads_its_total_from_the_decoder() {
        // WHY: an imported MP3 has a record duration of zero and no trusted
        // total from the decoder. The line must not fill the gap with a number
        // the file never reported.
        let mut out = [0u8; CAPTION_MAX];
        let n = playback_caption(Some(7_000), Some(192_000), 0, false, &mut out);
        assert_eq!(&out[..n], b"0:07 / 3:12");

        let n = playback_caption(Some(7_000), None, 225, true, &mut out);
        assert_eq!(&out[..n], b"0:07 / --:--");

        // Nothing playing: the record's length is a stored fact, and an
        // unmeasured track says so instead of claiming zero seconds.
        let n = playback_caption(None, None, 225, true, &mut out);
        assert_eq!(&out[..n], b"Duration 3:45");
        let n = playback_caption(None, None, 0, false, &mut out);
        assert_eq!(&out[..n], b"Duration not read");
    }

    #[test]
    fn an_artist_is_named_only_when_one_is_really_known() {
        let mut out = [0u8; NOW_PLAYING_MAX];
        let n = now_playing_line(None, Some(7_000), Some(192_000), 0, false, &mut out);
        assert_eq!(&out[..n], b"0:07 / 3:12");

        let n = now_playing_line(Some(b"Aria"), Some(7_000), Some(192_000), 0, false, &mut out);
        assert_eq!(&out[..n], b"Aria - 0:07 / 3:12");

        // A run of control bytes is not an artist name.
        assert!(known_artist(b"").is_none());
        assert!(known_artist(b"Local Library").is_some());
        assert!(known_artist(b"bad\x01name").is_none());
    }

    #[test]
    fn the_visible_queue_position_follows_the_filtered_order() {
        // WHY: the list shows the tracks a search matched, so a current track
        // at store index 0 can be second in the visible queue.
        let order = [3u8, 0, 2];
        assert_eq!(queue_position(&order, 3, 0), Some((2, 3)));
        assert_eq!(queue_position(&order, 3, 3), Some((1, 3)));
        assert_eq!(queue_position(&order, 3, 1), None);
        assert_eq!(queue_position(&order, 2, 2), None);
        assert_eq!(queue_position(&order, 9, 0), Some((2, 3)));
    }

    #[test]
    fn an_import_feedback_line_names_what_actually_happened() {
        use crate::library::ImportOutcome;

        let cases: [(ImportOutcome, &[u8]); 8] = [
            (ImportOutcome::Imported(1), b"Imported 1 track from storage"),
            (ImportOutcome::Imported(3), b"Imported 3 tracks from storage"),
            (
                ImportOutcome::PartiallyImported(2, 1),
                b"Imported 2 tracks; 1 refused",
            ),
            (
                ImportOutcome::PartiallyImported(1, 3),
                b"Imported 1 track; 3 refused",
            ),
            (
                ImportOutcome::WriteRefused(1),
                b"Found 1 audio file; store refused",
            ),
            (
                ImportOutcome::WriteRefused(4),
                b"Found 4 audio files; store refused",
            ),
            (
                ImportOutcome::ListRefused,
                b"Storage list refused; nothing imported",
            ),
            (
                ImportOutcome::NoneFound,
                b"No new audio files in shared storage",
            ),
        ];
        for (outcome, expected) in cases {
            let mut out = [0u8; FEEDBACK_MAX];
            let n = import_feedback(outcome, &mut out);
            assert!(n <= FEEDBACK_MAX);
            assert_eq!(&out[..n], expected, "outcome {outcome:?}");
            assert!(!out[..n].iter().any(|&b| b == b'\n'));
        }
    }

    #[test]
    fn a_track_that_played_out_says_whether_anything_followed_it() {
        let mut out = [0u8; FEEDBACK_MAX];
        let n = ended_feedback(Some(b"Nocturne"), &mut out);
        assert_eq!(&out[..n], b"Track ended - playing: Nocturne");

        let n = ended_feedback(None, &mut out);
        assert_eq!(&out[..n], b"Track ended - no playable track remains");

        // A long title is cut at the buffer, never written past it.
        let long = [b'x'; 60];
        let n = ended_feedback(Some(&long), &mut out);
        assert_eq!(n, FEEDBACK_MAX);
        assert!(out[..n].starts_with(b"Track ended - playing: "));
    }

    #[test]
    fn the_artwork_card_never_invents_a_cover() {
        // WHY: the platform exposes no ID3 APIC or FLAC PICTURE block, so the
        // card is a placeholder and says which fact is missing.
        assert_eq!(artwork_caption(false, false), b"No track selected");
        assert_eq!(artwork_caption(true, false), b"No embedded album art");
        assert!(artwork_caption(true, true).is_empty());
    }

    #[test]
    fn a_library_record_claims_no_artist_it_did_not_keep() {
        let record = TrackRecord::from_parts(1, b"Nocturne", 225);
        assert_eq!(record.artist(), b"");
        assert!(known_artist(record.artist()).is_none());
    }
}
