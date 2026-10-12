//! Inter OS Native Browser Client
//!
//! Native Ring-3 (User Mode) browser shell, laid out for the 720x1440 phone
//! viewport as a mobile browser (not a desktop widget):
//! - URI parser recognising `http://`, `https://`, `interlayer://` and
//!   `gemini://`; an address without a supported scheme or host is rejected
//! - `navigate()` reports the real outcome: this app holds no network socket,
//!   so no request is issued and no page is claimed to have loaded
//! - Top chrome: a 56 px address bar with an offline state glyph (the real
//!   transport state), a bookmarks control that opens the real bookmark store,
//!   a reload control that re-issues the real navigation attempt, and a close
//!   control that leaves to the launcher
//! - Content viewport renders the honest empty/error state of the last
//!   navigation attempt, and below it the bookmark list once the user has
//!   created one. There is no tab strip and no shortcut row, because the app has
//!   no tab model and no network transport behind any destination: shortcut
//!   tiles would be dead controls dressed as content, so they are not drawn.
//!   There IS a back/forward strip now, because the app keeps a real history of
//!   the addresses the user actually navigated to.
//! - Every string is fitted at render time with `interos_std::design::font`
//!   (`measure` / `measure_bold` / `ellipsize` / `wrap_text`): nothing is ever
//!   clipped, and long addresses are ellipsized with the SDK "..."
//! - Navigation acoustic feedback (G5: 784Hz) and tactile haptic response
//! - Asynchronous event reactor loop handling touch, keyboard and timer events
//!
//! Geometry: see `scratch/browser_layout/geometry.md` for the y-extent audit
//! of every region (all on the 8 pt grid, 24 px screen margin).

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use interos_std::design::font;
use interos_std::design::primitives::{draw_rounded_rect, Rect};

// -----------------------------------------------------------------------------
// Design tokens (light theme) - shared with the InterOS design system
// -----------------------------------------------------------------------------
use interos_std::design::color::{
    ACCENT, BACKGROUND as BASE, BORDER, SURFACE, SURFACE_RAISED, TEXT as INK,
    TEXT_MUTED as MUTED, WARNING,
};
/// Recessed fill for a control that is drawn but cannot act.
///
/// WHY a token: the design system's raised surface is the only surface token
/// below `SURFACE` in the light theme, so an inert control uses it rather than
/// inventing a new grey.
const MUTED_FILL: u32 = SURFACE_RAISED;

// -----------------------------------------------------------------------------
// Layout constants (720x1440, 8 pt grid, 24 px screen margin)
// -----------------------------------------------------------------------------

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// y 0..48: system status band. This app receives no clock, radio or battery
/// readings, so the band stays neutral - no sample values are ever drawn.
const STATUS_H: usize = 48;

/// y 48..120: toolbar (address bar + reload + close).
#[allow(dead_code)]
const TOOLBAR_Y0: usize = STATUS_H;
const TOOLBAR_Y1: usize = 120;

/// Address bar: x 24..504, y 56..112 (56 px tall, capsule radius 28).
const PILL: Rect = Rect::new(24, 56, 480, 56);
const PILL_R: usize = 28;
/// Text column inside the pill: state glyph x 40..64, text starts at x 72.
const PILL_TEXT_X: usize = 72;
const PILL_TEXT_Y: usize = 72;
/// 504 (pill right) - 16 (inner padding) - 72 (text origin) = 416.
const PILL_TEXT_MAX: usize = 416;

/// Bookmarks control: x 520..568, y 60..108 (48x48 touch target).
const BOOKMARKS_BTN: Rect = Rect::new(520, 60, 48, 48);
/// Reload control: x 584..632, y 60..108 (48x48 touch target).
const RELOAD_BTN: Rect = Rect::new(584, 60, 48, 48);
/// Close control: x 648..696, y 60..108 (48x48 touch target).
const CLOSE_BTN: Rect = Rect::new(648, 60, 48, 48);
const BTN_R: usize = 14;

/// y 120..1392: content viewport (fills everything between chrome and the
/// 48 px bottom gesture inset).
const VIEW_Y0: usize = TOOLBAR_Y1;
const VIEW_Y1: usize = 1392;

/// History strip directly under the toolbar: three pills and a position
/// readout, all on the 8 pt grid.
const NAV_STRIP_Y0: usize = 128;
const NAV_STRIP_H: usize = 64;
const NAV_PILL_W: usize = 148;
const NAV_PILL_H: usize = 56;
const NAV_PILL_GAP: usize = 12;
const BACK_BTN: Rect = Rect::new(24, NAV_STRIP_Y0 + 4, NAV_PILL_W, NAV_PILL_H);
const FWD_BTN: Rect = Rect::new(
    BACK_BTN.x + NAV_PILL_W + NAV_PILL_GAP,
    NAV_STRIP_Y0 + 4,
    NAV_PILL_W,
    NAV_PILL_H,
);
const ADD_BOOK_BTN: Rect = Rect::new(
    FWD_BTN.x + NAV_PILL_W + NAV_PILL_GAP,
    NAV_STRIP_Y0 + 4,
    NAV_PILL_W,
    NAV_PILL_H,
);
/// y 232 onward: the bookmark list.
const BOOKS_Y0: usize = 232;
const BOOK_ROW_H: usize = 72;

/// Honest empty/error state block: centred in the viewport below the bookmark
/// list, max text width 608 (56 px margins).
const STATE_BOX_TOP: usize = BOOKS_Y0;
const STATE_MAX_W: usize = 608;
const STATE_ICON_H: usize = 56;
const STATE_GAP_ICON: usize = 24;
const STATE_GAP_BODY: usize = 16;
/// Gap between the last bookmark row (or the viewport top) and the state block.
const STATE_GAP_ROW: usize = 24;
const STATE_LINE_STRIDE: usize = 32; // 24 px line + 8 px leading
const GLYPH_H: usize = font::GLYPH_H; // 24

/// y 1392..1440: system gesture band (unchanged system convention).
const GESTURE_PILL: Rect = Rect::new(260, 1418, 200, 6);
const HOME_X0: usize = 240;
const HOME_X1: usize = 480;
const HOME_Y0: usize = 1400;

const PLACEHOLDER: &[u8] = b"Search or type URL";

// Honest state copy: one headline plus a wrapped body per real outcome.
const BODY_BLANK: &[u8] = b"This browser holds no network transport, so no request has \
ever been sent. There is no page content to show.";
const BODY_NO_TRANSPORT: &[u8] = b"The address was parsed and a navigation was attempted, \
but no network transport is bound to this app: no request was sent and no page was loaded.";
const BODY_INVALID: &[u8] = b"Supported schemes are http, https, interlayer, file and gemini \
followed by a host. Even a valid address loads nothing while no transport is bound.";

/// Outcome for a local-scheme address (`file://`, `interos://`).
///
/// WHY this is not folded into `NoTransport`: a local address does not need a
/// network transport at all, so saying "no network transport is bound" would
/// blame the wrong thing. This app cannot read guest files because it holds no
/// filesystem capability, and that is a different fact with a different fix.
const BODY_LOCAL_REFUSED: &[u8] = b"The address names a local document, but this app holds \
no filesystem capability, so it cannot open a guest file at all. No local content \
is available to show.";

/// Real outcome of the last action. Nothing here is decorative: each variant
/// is set by an actual code path (parse rejection, transport absence, or the
/// platform refusing the file open).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageStatus {
    /// No navigation attempted yet (startup, or the user started editing).
    Blank,
    /// Address parsed fine; the app holds no socket, so nothing was fetched.
    NoTransport,
    /// Address rejected by the parser (no scheme, or no host).
    InvalidAddress,
    /// Address is a local scheme, and the platform refused the file open.
    LocalRefused,
}

/// Supported URI schemes.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UriScheme {
    Http = 0,
    Https = 1,
    InterLayer = 2,
    Gemini = 3,
    File = 4,
    Unknown = 5,
}

impl UriScheme {
    /// True when the scheme names a document inside this guest rather than a
    /// remote host.
    ///
    /// WHY these two and not `about:`: `about:blank` is a browser-internal
    /// address with no document behind it at all, while `file://` and
    /// `interos://` name real guest files that a capable process could read.
    fn is_local(self) -> bool {
        matches!(self, UriScheme::File | UriScheme::InterLayer)
    }
}

/// Parsed sovereign URI structure. Every component is a slice of the raw
/// address that was parsed.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct ParsedUri<'a> {
    pub scheme: UriScheme,
    pub host: &'a [u8],
    pub path: &'a [u8],
    pub port: u16,
}

/// Sovereign Bookmark entry. Kept as the compile-time "no bundled demo content"
/// guard: it can only ever describe a constant, so it is pinned empty and
/// asserted by `tests/test_input_coordinate_contract.py`. Real bookmarks the
/// user creates at runtime live in `BookmarkStore` below.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct Bookmark {
    pub title: &'static str,
    pub url: &'static str,
    pub is_sovereign: bool,
}

/// Optional bookmarks loaded from real browser state. The platform exposes no
/// bookmark store, so the ledger is empty and stays empty - nothing is
/// synthesised to fill the UI.
#[allow(dead_code)]
pub static BOOKMARKS: [Bookmark; 0] = [];

// -----------------------------------------------------------------------------
// Bookmark store: bounded fixed-size records in the platform's settings store
// -----------------------------------------------------------------------------

/// Longest bookmark title the record holds.
const BM_TITLE_MAX: usize = 24;

/// Longest address the record holds.
///
/// WHY 48 bytes: a full `https://` host plus a path already exhausts it, so the
/// store says so in the editor instead of silently truncating an address into
/// one nobody typed. The settings store refuses a value over 512 bytes, so the
/// address width is what bounds the capacity.
const BM_URL_MAX: usize = 48;

/// Bounded number of bookmarks the store holds.
///
/// WHY 6: a record is `BM_RECORD_BYTES` wide and the kernel refuses a settings
/// value longer than 512 bytes, so `2 + N * BM_RECORD_BYTES <= 512` caps the
/// store. The store reports that it is full instead of dropping a bookmark.
///
/// WHY it must stay below 10: the blob's count byte and each record's id byte are
/// raw values, and the settings store rejects any value containing a 0x0A byte
/// (`kernel/src/settings.rs`, `set`). A count or id of 10 would encode to 0x0A
/// and the write would fail for a reason unrelated to the data.
const MAX_BOOKMARKS: usize = 6;

/// Record width, pinned: 1 id + `BM_TITLE_MAX` title bytes (0x00-padded)
/// + `BM_URL_MAX` address bytes (0x00-padded) = 73 bytes.
///
/// WHY 0x00 padding instead of length prefixes: a length byte can itself be 10,
/// which is exactly the 0x0A the settings store refuses. Padding with 0x00 and
/// taking the length from the first zero keeps every byte in the record in the
/// set {0x00, printable ASCII, id 1..=6}, none of which is 0x0A.
const BM_RECORD_BYTES: usize = 1 + BM_TITLE_MAX + BM_URL_MAX;

/// Blob width: 1 version byte + 1 count byte + `MAX_BOOKMARKS` records
/// = 440 bytes, inside both the kernel's 512-byte value cap and the
/// `settings_get` output cap of 512 bytes.
const STORE_BLOB_BYTES: usize = 2 + MAX_BOOKMARKS * BM_RECORD_BYTES;

/// Store format version. A blob that does not start with this byte is not ours.
const STORE_VERSION: u8 = 1;

/// Settings key the whole bookmark store is written under.
const STORE_KEY: &str = "browser.bookmarks.v1";

/// Byte cost of the whole feature: 6 bookmarks x 73 bytes = 438 bytes of RAM for
/// the records, plus a 440-byte staging buffer that exists only for the duration
/// of one save. No heap, no allocator, no `Vec`, no `String`.
pub const BOOKMARK_STORE_RAM_BYTES: usize = MAX_BOOKMARKS * BM_RECORD_BYTES;

/// What the store's last contact with the platform storage actually did.
///
/// WHY an enum rather than a flag plus a message: the list, the log and the
/// footer must all say the same thing, and two parallel values can disagree.
/// One value is turned into words in exactly one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookmarkStoreState {
    /// The store has not been read yet.
    Untouched,
    /// The settings store holds no browser key, so there is nothing to load.
    Empty,
    /// Bookmarks were read back out of the settings store.
    Loaded,
    /// The kernel refused the read.
    ReadRefused,
    /// The stored bytes were not a store this app wrote.
    Invalid,
    /// The last write was acknowledged, so the durable list is what storage holds.
    Written,
    /// The kernel refused the write. Nothing was promoted to the durable list.
    WriteRefused,
    /// The store is at its capacity and refused another bookmark.
    Full,
}

impl BookmarkStoreState {
    /// The words this state is reported in.
    fn as_str(self) -> &'static str {
        match self {
            BookmarkStoreState::Untouched => "BOOKMARK STORE NOT READ YET",
            BookmarkStoreState::Empty => "NO SAVED BOOKMARKS",
            BookmarkStoreState::Loaded => "BOOKMARKS READ FROM THE SETTINGS STORE",
            BookmarkStoreState::ReadRefused => "SETTINGS READ REFUSED: NO SAVED BOOKMARKS",
            BookmarkStoreState::Invalid => "STORED BOOKMARK DATA FAILED VALIDATION",
            BookmarkStoreState::Written => "SAVED TO THE SETTINGS STORE",
            BookmarkStoreState::WriteRefused => "NOT SAVED: THE KERNEL REFUSED THE WRITE",
            BookmarkStoreState::Full => "STORE FULL: NO BOOKMARK ADDED",
        }
    }
}

/// One user-created bookmark, exactly as it is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookmarkRecord {
    /// 1..=MAX_BOOKMARKS. Never 0, and never 10 (see `MAX_BOOKMARKS`).
    id: u8,
    /// Printable ASCII, 0x00-padded to `BM_TITLE_MAX`.
    title: [u8; BM_TITLE_MAX],
    /// Printable ASCII, 0x00-padded to `BM_URL_MAX`.
    url: [u8; BM_URL_MAX],
}

impl BookmarkRecord {
    const EMPTY: BookmarkRecord = BookmarkRecord {
        id: 0,
        title: [0; BM_TITLE_MAX],
        url: [0; BM_URL_MAX],
    };

    fn from_parts(id: u8, title: &[u8], url: &[u8]) -> Self {
        let mut rec = BookmarkRecord::EMPTY;
        rec.id = id;
        let n = title.len().min(BM_TITLE_MAX);
        rec.title[..n].copy_from_slice(&title[..n]);
        let m = url.len().min(BM_URL_MAX);
        rec.url[..m].copy_from_slice(&url[..m]);
        rec
    }

    /// True when the record holds a title and an address the store can hand back.
    ///
    /// WHY the byte set is enforced here and not only at the editor: this is the
    /// one function that reads bytes back out of storage, so it is the one place
    /// a malformed or foreign blob can be rejected. Without it a corrupt value
    /// would surface as a bookmark nobody created.
    fn valid(&self) -> bool {
        (1..=MAX_BOOKMARKS as u8).contains(&self.id)
            && self.title_bytes().iter().all(|b| (0x20..=0x7E).contains(b))
            && self.url_bytes().iter().all(|b| (0x20..=0x7E).contains(b))
    }

    /// Length taken as the offset of the first 0x00 pad byte.
    fn title_len(&self) -> usize {
        self.title.iter().position(|&b| b == 0).unwrap_or(BM_TITLE_MAX)
    }

    fn url_len(&self) -> usize {
        self.url.iter().position(|&b| b == 0).unwrap_or(BM_URL_MAX)
    }

    fn title_bytes(&self) -> &[u8] {
        &self.title[..self.title_len()]
    }

    fn url_bytes(&self) -> &[u8] {
        &self.url[..self.url_len()]
    }

    fn encode(&self, out: &mut [u8]) {
        out[0] = self.id;
        out[1..1 + BM_TITLE_MAX].copy_from_slice(&self.title);
        out[1 + BM_TITLE_MAX..1 + BM_TITLE_MAX + BM_URL_MAX].copy_from_slice(&self.url);
    }

    /// Decodes one record, rejecting anything the store could not have written.
    fn decode(bytes: &[u8]) -> Option<BookmarkRecord> {
        if bytes.len() < BM_RECORD_BYTES {
            return None;
        }
        let mut rec = BookmarkRecord::EMPTY;
        rec.id = bytes[0];
        rec.title.copy_from_slice(&bytes[1..1 + BM_TITLE_MAX]);
        let url_at = 1 + BM_TITLE_MAX;
        rec.url
            .copy_from_slice(&bytes[url_at..url_at + BM_URL_MAX]);
        if rec.valid() {
            Some(rec)
        } else {
            None
        }
    }
}

/// The user's bookmarks: bounded records plus an honest account of the last load
/// or save.
///
/// WHY there are two lists: a bookmark only joins the durable list after the
/// kernel acknowledges the write, so the durable list is always exactly what
/// storage holds. A write the kernel refuses cannot be dropped (that would
/// delete what the user just added) and cannot be promoted (that would claim a
/// durability the device does not have), so it goes in `unsaved`, which is drawn
/// and labelled separately.
#[derive(Debug, Clone, Copy)]
pub struct BookmarkStore {
    records: [BookmarkRecord; MAX_BOOKMARKS],
    len: u8,
    unsaved: [BookmarkRecord; MAX_BOOKMARKS],
    unsaved_len: u8,
    state: BookmarkStoreState,
    /// Id to hand to the next record. Monotonic within a session so a delete
    /// never re-issues an id another open record still carries.
    next_id: u8,
}

impl BookmarkStore {
    pub const EMPTY: BookmarkStore = BookmarkStore {
        records: [BookmarkRecord::EMPTY; MAX_BOOKMARKS],
        len: 0,
        unsaved: [BookmarkRecord::EMPTY; MAX_BOOKMARKS],
        unsaved_len: 0,
        state: BookmarkStoreState::Untouched,
        next_id: 1,
    };

    /// Reads the store back out of the platform settings store.
    ///
    /// WHY the whole blob is rejected when any record fails validation: the blob
    /// is written as a single settings value, so a valid store never splits
    /// across reads. Accepting a prefix would present a partial list as the
    /// stored one, so the load reports what it actually has instead.
    pub fn load() -> Self {
        let mut store = BookmarkStore::EMPTY;
        let mut blob = [0u8; STORE_BLOB_BYTES];
        match std::settings_get(STORE_CAP_SLOT, STORE_KEY, &mut blob) {
            Ok(0) => store.state = BookmarkStoreState::Empty,
            Ok(n) => {
                let mut records = [BookmarkRecord::EMPTY; MAX_BOOKMARKS];
                let mut count = 0usize;
                let mut rejected = false;
                if n > STORE_BLOB_BYTES || blob[0] != STORE_VERSION {
                    rejected = true;
                } else {
                    let header_count = blob[1] as usize;
                    if header_count > MAX_BOOKMARKS || n != 2 + header_count * BM_RECORD_BYTES {
                        rejected = true;
                    } else {
                        for i in 0..header_count {
                            let start = 2 + i * BM_RECORD_BYTES;
                            match BookmarkRecord::decode(&blob[start..start + BM_RECORD_BYTES]) {
                                Some(rec) => {
                                    records[i] = rec;
                                    count += 1;
                                }
                                None => {
                                    rejected = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                if rejected {
                    store.state = BookmarkStoreState::Invalid;
                } else if count == 0 {
                    store.state = BookmarkStoreState::Empty;
                } else {
                    store.records = records;
                    store.len = count as u8;
                    // WHY the id cursor starts past whatever was loaded: re-using
                    // an id already on disk would make a later update ambiguous.
                    store.next_id = records
                        .iter()
                        .take(count)
                        .map(|r| r.id)
                        .max()
                        .unwrap_or(0)
                        .saturating_add(1);
                    store.state = BookmarkStoreState::Loaded;
                }
            }
            Err(_) => store.state = BookmarkStoreState::ReadRefused,
        }
        store
    }

    /// Encodes the durable list into one settings value and writes it.
    ///
    /// WHY the whole blob is rewritten every time: the store is small enough that
    /// a record-at-a-time append would need a cursor the settings store does not
    /// have, and a whole-value write leaves no half-updated record behind.
    ///
    /// Returns the kernel's own answer. `Ok(())` means the list on screen is now
    /// what persistent storage holds.
    fn persist(&mut self) -> Result<(), i64> {
        let mut blob = [0u8; STORE_BLOB_BYTES];
        blob[0] = STORE_VERSION;
        blob[1] = self.len;
        for i in 0..self.len as usize {
            let start = 2 + i * BM_RECORD_BYTES;
            self.records[i].encode(&mut blob[start..start + BM_RECORD_BYTES]);
        }
        let used = 2 + self.len as usize * BM_RECORD_BYTES;
        if blob[..used].iter().any(|&b| b == b'\n') {
            self.state = BookmarkStoreState::WriteRefused;
            return Err(-22);
        }
        match std::settings_set(STORE_CAP_SLOT, STORE_KEY, &blob[..used]) {
            Ok(()) => {
                self.state = BookmarkStoreState::Written;
                Ok(())
            }
            Err(code) => {
                self.state = BookmarkStoreState::WriteRefused;
                Err(code)
            }
        }
    }

    /// Adds a bookmark. The durable list only gains it once the write is
    /// acknowledged; otherwise it lands in `unsaved` and the refusal is recorded.
    pub fn add(&mut self, title: &[u8], url: &[u8]) -> Result<(), i64> {
        if title.is_empty() || url.is_empty() {
            return Err(-22);
        }
        if self.next_id as usize > MAX_BOOKMARKS || self.next_id == 0 {
            return Err(-22);
        }
        let id = self.next_id;
        let rec = BookmarkRecord::from_parts(id, title, url);
        if !rec.valid() {
            return Err(-22);
        }
        if self.len as usize >= MAX_BOOKMARKS {
            // WHY capacity is checked against the durable list only: a store that
            // is full of acknowledged bookmarks cannot take another one, but
            // unsaved records do not occupy storage, so they never make it full.
            self.state = BookmarkStoreState::Full;
            return Err(-28);
        }
        // WHY the record is appended before the write: `persist` encodes the
        // durable list, so the new bookmark has to be in it for the write to mean
        // "this list, plus this bookmark, is now what storage holds".
        self.records[self.len as usize] = rec;
        self.len += 1;
        self.next_id += 1;
        match self.persist() {
            Ok(()) => Ok(()),
            Err(code) => {
                // The write was refused, so the record must not stay in the
                // durable list: that list is a statement about storage. It moves
                // to `unsaved`, where it is drawn and labelled session-only.
                self.len -= 1;
                self.records[self.len as usize] = BookmarkRecord::EMPTY;
                if (self.unsaved_len as usize) < MAX_BOOKMARKS {
                    let mut held = rec;
                    held.id = id;
                    self.unsaved[self.unsaved_len as usize] = held;
                    self.unsaved_len += 1;
                }
                Err(code)
            }
        }
    }

    /// Removes the durable record at `index` and writes the store.
    pub fn remove(&mut self, index: usize) -> Result<(), i64> {
        if index >= self.len as usize {
            return Err(-22);
        }
        for i in index..self.len as usize - 1 {
            self.records[i] = self.records[i + 1];
        }
        self.records[self.len as usize - 1] = BookmarkRecord::EMPTY;
        self.len -= 1;
        self.persist()
    }

    /// Removes an entry from the unsaved list, which needs no write because
    /// storage never held it.
    pub fn remove_unsaved(&mut self, index: usize) -> Result<(), i64> {
        if index >= self.unsaved_len as usize {
            return Err(-22);
        }
        for i in index..self.unsaved_len as usize - 1 {
            self.unsaved[i] = self.unsaved[i + 1];
        }
        self.unsaved[self.unsaved_len as usize - 1] = BookmarkRecord::EMPTY;
        self.unsaved_len -= 1;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn unsaved_len(&self) -> usize {
        self.unsaved_len as usize
    }

    pub fn record(&self, index: usize) -> Option<&BookmarkRecord> {
        if index < self.len as usize {
            Some(&self.records[index])
        } else {
            None
        }
    }

    pub fn unsaved_record(&self, index: usize) -> Option<&BookmarkRecord> {
        if index < self.unsaved_len as usize {
            Some(&self.unsaved[index])
        } else {
            None
        }
    }

    pub fn state(&self) -> BookmarkStoreState {
        self.state
    }

    pub fn is_full(&self) -> bool {
        self.len as usize >= MAX_BOOKMARKS
    }
}

/// The bookmark being typed: step 0 is idle, step 1 is the title, step 2 is the
/// address.
///
/// WHY a step and not two fields: the app already has exactly one editable
/// surface, the address pill, wired to the platform's keyboard funnel. Reusing
/// it means the editor needs no new text machinery and no new input path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookmarkStep {
    /// Not adding a bookmark.
    Idle,
    /// Typing the title; the pill holds the title so far.
    Title,
    /// Typing the address; the pill holds the address so far.
    Address,
}

/// Real history of the addresses the user navigated to.
///
/// WHY the history holds attempted addresses rather than loaded pages: no page
/// has ever loaded, so a history of loaded pages would be empty by construction.
/// The addresses the user asked for are the real thing this app can record.
const HISTORY_CAP: usize = 16;

/// Real history of navigated addresses, plus the cursor into it.
#[derive(Debug, Clone, Copy)]
pub struct History {
    entries: [[u8; 64]; HISTORY_CAP],
    len: usize,
    /// Index of the entry currently shown. `len` is one past the newest entry,
    /// so `pos == len - 1` is the newest.
    pos: usize,
}

impl History {
    const EMPTY: History = History {
        entries: [[0; 64]; HISTORY_CAP],
        len: 0,
        pos: 0,
    };

    pub fn len(&self) -> usize {
        self.len
    }

    /// Position of the current entry, counted from 1, for the readout.
    pub fn position(&self) -> usize {
        (self.pos + 1).min(self.len).max(1)
    }

    /// Pushes an address, dropping any forward entries.
    ///
    /// WHY forward entries are dropped: that is what every browser does, and
    /// keeping them would make "forward" step into an address the user navigated
    /// away from and then replaced.
    pub fn push(&mut self, target: &[u8]) {
        if self.pos + 1 < self.len {
            self.len = self.pos + 1;
        }
        if self.len >= HISTORY_CAP {
            // WHY the oldest is dropped rather than the push refused: the newest
            // address is the one the user just asked for, and refusing it would
            // lose the navigation they just made.
            for i in 0..self.len - 1 {
                self.entries[i] = self.entries[i + 1];
            }
            self.len -= 1;
            self.pos = self.pos.saturating_sub(1);
        }
        let n = target.len().min(self.entries[self.len].len());
        self.entries[self.len][..n].copy_from_slice(&target[..n]);
        self.len += 1;
        self.pos = self.len - 1;
    }

    /// Moves back one entry. Returns the address moved to, if any.
    pub fn back(&mut self) -> Option<[u8; 64]> {
        if self.pos == 0 {
            return None;
        }
        self.pos -= 1;
        let mut out = [0u8; 64];
        let n = self.entries[self.pos]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.entries[self.pos].len());
        out[..n].copy_from_slice(&self.entries[self.pos][..n]);
        Some(out)
    }

    /// Moves forward one entry. Returns the address moved to, if any.
    pub fn forward(&mut self) -> Option<[u8; 64]> {
        if self.pos + 1 >= self.len {
            return None;
        }
        self.pos += 1;
        let mut out = [0u8; 64];
        let n = self.entries[self.pos]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.entries[self.pos].len());
        out[..n].copy_from_slice(&self.entries[self.pos][..n]);
        Some(out)
    }

    /// True when there is an entry to go back to.
    pub fn can_back(&self) -> bool {
        self.pos > 0
    }

    /// True when there is an entry to go forward to.
    pub fn can_forward(&self) -> bool {
        self.pos + 1 < self.len
    }
}

/// Capability slot used for both the bookmark read and the attempted write.
///
/// WHY this choice is forced, and what it really means: the browser task is
/// granted NO filesystem capability at all. `spawn_app_browser` in
/// `kernel/src/task/user.rs` grants exactly seven capabilities, none of them a
/// `FileSystemRead` or a `FileStorageWrite`: SerialOutput, IpcSend, TimerTickRead,
/// NetworkSocket, NetworkSend, NetworkReceive, FrameBufferDirect and EventPoll.
/// `sys_settings_get` needs one of the two filesystem types with `Rights::READ`
/// at the slot it is handed (`kernel/src/syscall/mod.rs`), and
/// `sys_settings_set` needs a `FileStorageWrite` with `Rights::WRITE`. This task
/// holds neither, so on this kernel both calls return `PermissionDenied`.
///
/// The store therefore still goes through the real durable path -- a real read
/// at startup and a real write on every mutation -- because that is the only
/// thing that can be honest about why a bookmark does not survive a reboot, and
/// because the same code persists the moment the browser task is granted the
/// capability. `BookmarkStoreState::ReadRefused` and `::WriteRefused` record the
/// kernel's own answers, and the bookmark panel states them on screen rather
/// than claiming a persistence the platform refuses.
///
/// WHY slot 6 specifically: it is the slot that would hold a `FileStorageWrite`
/// if the kernel ever granted one (0-based: 0=SerialOutput, 1=IpcSend,
/// 2=TimerTickRead, 3=NetworkSocket, 4=NetworkSend, 5=NetworkReceive,
/// 6=FileStorageWrite, 7=FrameBufferDirect, 8=EventPoll). Passing the slot the
/// capability would occupy means the store starts working the day it is granted,
/// with no further change here.
const STORE_CAP_SLOT: usize = 6;

/// Production Sovereign Browser state engine.
#[allow(dead_code)]
pub struct BrowserApp {
    /// Address of the current/last navigation attempt. Nothing is loaded:
    /// this app has no transport, so this is the attempted address only.
    pub current_url: [u8; 64],
    pub current_url_len: usize,
    /// Real scroll state. The content viewport never overflows, so the offset
    /// stays 0 - no scrollbar is drawn because nothing scrolls.
    pub scroll_offset_px: i32,
    pub pages_loaded: u32,
    pub trackers_blocked: u32,
    pub active_socket_id: Option<u64>,
    /// User-edited address text. No page is claimed loaded without a real
    /// transport.
    pub address: [u8; 64],
    pub address_len: usize,
    pub address_focused: bool,
    /// The whole address is selected, so the next character replaces it.
    ///
    /// WHY this exists: focusing the bar pre-filled the attempted address into
    /// the edit buffer and typing appended to it, so tapping a bar reading
    /// "about:blank" and typing "example" produced "about:blankexample", which
    /// is not an address anything can navigate to. Android and iOS both select
    /// the whole field on focus for exactly this reason, and a caret parked at
    /// the end of the text gave no hint that a selection existed.
    pub select_all: bool,
    /// Real history of the addresses navigated to, with the cursor into it.
    pub history: History,
    /// The user's bookmarks, read back out of the platform settings store.
    pub bookmarks: BookmarkStore,
    /// Which stage of the add-bookmark editor the address pill is in.
    pub bm_step: BookmarkStep,
    /// The title typed at the first step, held while the address is typed.
    ///
    /// WHY it cannot live in the address pill: the pill is the surface typed
    /// into, so the title has to be moved somewhere else the moment the address
    /// step starts using it.
    pub bm_title: [u8; BM_TITLE_MAX],
    pub bm_title_len: usize,
    /// Real outcome of the last navigation / action.
    pub status: PageStatus,
}

impl BrowserApp {
    pub fn new() -> Self {
        let mut app = Self {
            current_url: [0; 64],
            current_url_len: 0,
            scroll_offset_px: 0,
            pages_loaded: 0,
            trackers_blocked: 0,
            active_socket_id: None,
            address: [0; 64],
            address_len: 0,
            address_focused: false,
            select_all: false,
            history: History::EMPTY,
            bookmarks: BookmarkStore::load(),
            bm_step: BookmarkStep::Idle,
            bm_title: [0; BM_TITLE_MAX],
            bm_title_len: 0,
            status: PageStatus::Blank,
        };
        let start = b"about:blank";
        app.current_url[..start.len()].copy_from_slice(start);
        app.current_url_len = start.len();
        app
    }

    fn current_url(&self) -> &[u8] {
        &self.current_url[..self.current_url_len]
    }

    /// Parses a URI byte string into its protocol scheme, host, and path
    /// components. Every returned component is a slice of `raw`, so a parsed
    /// URI always reflects the actual address the user navigated to.
    ///
    /// WHY `file://` and `interos://` are recognised now: they used to fall
    /// through to `UriScheme::Unknown`, so a local address was reported as an
    /// unsupported scheme rather than as a scheme this app cannot open. A real
    /// parser says which scheme an address uses; whether anything can be read
    /// from it is a separate, separately-reported fact.
    pub fn parse_uri<'a>(&self, raw: &'a [u8]) -> ParsedUri<'a> {
        let (scheme, rest) = if let Some(r) = raw.strip_prefix(&b"interos://"[..]) {
            (UriScheme::InterLayer, r)
        } else if let Some(r) = raw.strip_prefix(&b"interlayer://"[..]) {
            (UriScheme::InterLayer, r)
        } else if let Some(r) = raw.strip_prefix(&b"file://"[..]) {
            (UriScheme::File, r)
        } else if let Some(r) = raw.strip_prefix(&b"https://"[..]) {
            (UriScheme::Https, r)
        } else if let Some(r) = raw.strip_prefix(&b"http://"[..]) {
            (UriScheme::Http, r)
        } else if let Some(r) = raw.strip_prefix(&b"gemini://"[..]) {
            (UriScheme::Gemini, r)
        } else {
            return ParsedUri {
                scheme: UriScheme::Unknown,
                host: &raw[..0],
                path: raw,
                port: 0,
            };
        };

        // Split the authority from the path at the first '/'.
        let (authority, path) = match rest.iter().position(|&b| b == b'/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, &rest[..0]),
        };

        // Split the host from an explicit ":port" suffix. A port is only
        // recognised when the suffix is all decimal digits, so a host
        // containing a colon is not silently misread as host:port.
        let (host, port) = match authority.iter().rposition(|&b| b == b':') {
            Some(i) => {
                let digits = &authority[i + 1..];
                let mut value: u16 = 0;
                let mut all_digits = !digits.is_empty();
                for &b in digits {
                    if b.is_ascii_digit() {
                        value = value.saturating_mul(10).saturating_add((b - b'0') as u16);
                    } else {
                        all_digits = false;
                        break;
                    }
                }
                if all_digits {
                    (&authority[..i], value)
                } else {
                    (authority, 0)
                }
            }
            None => (authority, 0),
        };

        ParsedUri {
            scheme,
            host,
            path,
            port,
        }
    }

    /// Attempts to navigate to a target address.
    ///
    /// Returns `true` only when a transport actually fetched something. It
    /// never does: this app opens no socket, so no request is issued, no page
    /// is counted, and the returned status records the real outcome (invalid
    /// address vs. no transport).
    ///
    /// This is the pushing entry point: it records the address in the real
    /// history first, so every address the user asks for is reachable by Back.
    /// Back, Forward and Reload use `navigate_again`, which does not push.
    pub fn navigate(&mut self, target: &[u8]) -> bool {
        self.history.push(target);
        self.navigate_again(target)
    }

    /// Re-attempts navigation to `target` without adding a history entry.
    ///
    /// WHY back, forward and reload share this: they all re-issue the same real
    /// attempt for an address the history already holds. Pushing another entry
    /// for them would make Back walk the same address twice and would turn
    /// Reload into a duplicate.
    pub fn navigate_again(&mut self, target: &[u8]) -> bool {
        // Remember the attempted address so the address bar can show it
        // truthfully as "attempted, not loaded".
        let n = target.len().min(self.current_url.len());
        self.current_url[..n].copy_from_slice(&target[..n]);
        self.current_url_len = n;

        let parsed = self.parse_uri(target);
        if parsed.scheme == UriScheme::Unknown {
            self.status = PageStatus::InvalidAddress;
            std::log(0, "[BROWSER APP] Navigation rejected: the address has no supported scheme.").ok();
            return false;
        }
        if parsed.host.is_empty() {
            self.status = PageStatus::InvalidAddress;
            std::log(0, "[BROWSER APP] Navigation rejected: the address has no host.").ok();
            return false;
        }
        // WHY not "from the address bar": this function is also reached from
        // the quick shortcuts, where no address text was submitted.
        std::log(0, "[BROWSER APP] Navigation target URL:").ok();
        match core::str::from_utf8(target) {
            Ok(url) => {
                std::log(0, url).ok();
            }
            Err(_) => {
                std::log(0, "[BROWSER APP] (address is not valid UTF-8)").ok();
            }
        }

        // A local scheme names a document inside this guest rather than a remote
        // host, so the honest report is a different one from "no transport".
        //
        // WHY the read is still attempted: a file open is a real syscall with a
        // real answer, and that answer is what decides which of the two reports
        // is true. Skipping it to save a syscall would mean reporting a refusal
        // the kernel never actually gave.
        if parsed.scheme.is_local() {
            std::log(
                0,
                "[BROWSER APP] Address names a local document; attempting to read it from the guest filesystem.",
            )
            .ok();
            // WHY the path is the raw address: the VFS is addressed by absolute
            // path, and a local URI's own text is that path. The open is a real
            // syscall with a real answer, and that answer is what decides which
            // of the two reports is true - skipping it to save a syscall would
            // mean reporting a refusal the kernel never actually gave.
            let path = match core::str::from_utf8(target) {
                Ok(p) => p,
                Err(_) => {
                    self.status = PageStatus::LocalRefused;
                    std::log(
                        0,
                        "[BROWSER APP] The address is not valid UTF-8, so it cannot name a guest file.",
                    )
                    .ok();
                    let _ = std::audio_haptic_click();
                    return false;
                }
            };
            match std::fs_open(STORE_CAP_SLOT, path, std::FS_READ) {
                Ok(fd) => {
                    let opened = std::fs_close(fd).is_ok();
                    if !opened {
                        std::log(
                            0,
                            "[BROWSER APP] The guest filesystem opened the document but refused to close it.",
                        )
                        .ok();
                    }
                    // The file really exists. This app still renders no content:
                    // it has no document layout engine, and reporting that is the
                    // honest outcome for an address this app cannot display.
                    std::log(
                        0,
                        "[BROWSER APP] The guest filesystem opened the document, but this app has no document renderer, so no content is shown.",
                    )
                    .ok();
                    self.status = PageStatus::LocalRefused;
                }
                Err(_) => {
                    std::log(
                        0,
                        "[BROWSER APP] The guest filesystem refused to open the document. This app holds no filesystem capability, so local content is unavailable.",
                    )
                    .ok();
                    self.status = PageStatus::LocalRefused;
                }
            }
            let _ = std::audio_haptic_click();
            return false;
        }

        std::log(
            0,
            "[BROWSER APP] No transport is bound to this app; no request was issued and no page was loaded.",
        )
        .ok();

        // Navigation Acoustic Feedback & Haptic Click (G5: 784Hz).
        // WHY: only report it when the audio service accepted the tone.
        if std::audio_play_tone(784).is_ok() {
            std::log(0, "[BROWSER APP] Navigation chime emitted (784Hz).").ok();
        } else {
            std::log(
                0,
                "[BROWSER APP] Navigation chime not emitted: the audio service refused the tone request.",
            )
            .ok();
        }
        let _ = std::audio_silence();
        let _ = std::audio_haptic_click();

        // The attempt reached the transport stage and stopped there.
        self.status = PageStatus::NoTransport;
        false
    }

    /// Reloads the address currently shown.
    ///
    /// WHY this does not push a history entry: reloading is re-asking for the
    /// same address, and pushing would make Back walk away from the address the
    /// user is looking at instead of back through what they visited before it.
    pub fn reload(&mut self) {
        let mut target = [0u8; 64];
        let n = self.current_url_len.min(target.len());
        target[..n].copy_from_slice(&self.current_url[..n]);
        self.navigate_again(&target[..n]);
    }

    /// Steps back through the real history.
    pub fn go_back(&mut self) {
        match self.history.back() {
            Some(entry) => {
                let n = entry.iter().position(|&b| b == 0).unwrap_or(entry.len());
                std::log(0, "[BROWSER APP] Back: re-issuing the navigation attempt for the previous address.").ok();
                self.navigate_again(&entry[..n]);
                return;
            }
            None => {}
        }
        std::log(
            0,
            "[BROWSER APP] Back is unavailable: the history holds no earlier address.",
        )
        .ok();
    }

    /// Steps forward through the real history.
    pub fn go_forward(&mut self) {
        match self.history.forward() {
            Some(entry) => {
                let n = entry.iter().position(|&b| b == 0).unwrap_or(entry.len());
                std::log(0, "[BROWSER APP] Forward: re-issuing the navigation attempt for the next address.").ok();
                self.navigate_again(&entry[..n]);
                return;
            }
            None => {}
        }
        std::log(
            0,
            "[BROWSER APP] Forward is unavailable: the history holds no later address.",
        )
        .ok();
    }

    /// Starts the add-bookmark editor at the title step.
    pub fn start_add_bookmark(&mut self) {
        self.bm_step = BookmarkStep::Title;
        self.address_focused = true;
        self.address_len = 0;
        self.select_all = false;
        std::log(
            0,
            "[BROWSER APP] Adding a bookmark: type the title, then the address.",
        )
        .ok();
    }

    /// Advances or commits the add-bookmark editor.
    ///
    /// WHY the title is stored before the address: the pill is the only editable
    /// surface, so a two-step flow is what the platform's input path allows. The
    /// address is the part that gets navigated to, so it is typed last.
    pub fn commit_bookmark_step(&mut self) -> Result<(), i64> {
        let step = self.bm_step;
        match step {
            BookmarkStep::Idle => Ok(()),
            BookmarkStep::Title => {
                if self.address_len == 0 {
                    std::log(0, "[BROWSER APP] Bookmark not added: a bookmark needs a title.").ok();
                    return Err(-22);
                }
                // Move the title out of the pill and into its own buffer, so the
                // pill is free for the address step.
                let n = self.address_len.min(self.bm_title.len());
                self.bm_title[..n].copy_from_slice(&self.address[..n]);
                self.bm_title_len = n;
                self.bm_step = BookmarkStep::Address;
                self.address_len = 0;
                self.select_all = false;
                std::log(0, "[BROWSER APP] Bookmark title set; type the address.").ok();
                Ok(())
            }
            BookmarkStep::Address => {
                if self.address_len == 0 {
                    std::log(0, "[BROWSER APP] Bookmark not added: a bookmark needs an address.").ok();
                    return Err(-22);
                }
                if self.address_len > BM_URL_MAX {
                    self.address_len = BM_URL_MAX;
                }
                let title_len = self.bm_title_len.min(BM_TITLE_MAX);
                let outcome = self
                    .bookmarks
                    .add(&self.bm_title[..title_len], &self.address[..self.address_len]);
                self.bm_step = BookmarkStep::Idle;
                self.address_len = 0;
                self.address_focused = false;
                self.select_all = false;
                self.bm_title_len = 0;
                match outcome {
                    Ok(()) => {
                        std::log(
                            0,
                            "[BROWSER APP] Bookmark written to the settings store and acknowledged.",
                        )
                        .ok();
                    }
                    Err(_) => {
                        std::log(
                            0,
                            "[BROWSER APP] Bookmark was NOT saved: the kernel refused the settings write. It is held in the unsaved group for this session only.",
                        )
                        .ok();
                    }
                }
                outcome
            }
        }
    }

    /// Cancels the add-bookmark editor, discarding what was typed.
    pub fn cancel_bookmark_step(&mut self) {
        self.bm_step = BookmarkStep::Idle;
        self.address_len = 0;
        self.address_focused = false;
        self.select_all = false;
        self.bm_title_len = 0;
    }

    /// Reports whether the parsed endpoint has both a host and a port.
    #[allow(dead_code)]
    pub fn endpoint_is_complete(&self, uri: &ParsedUri) -> bool {
        !uri.host.is_empty() && uri.port != 0
    }
}

// -----------------------------------------------------------------------------
// Per-frame resolved text. Every string is measured / ellipsized / wrapped ONCE
// per frame with interos_std::design::font, then rendered from these buffers.
// -----------------------------------------------------------------------------
struct Frame {
    /// Address bar text, ellipsized to the pill's 480 px text column.
    pill: [u8; 96],
    pill_len: usize,
    pill_color: u32,
    show_caret: bool,
    caret_x: usize,
    /// Width of the selection band across the pill text, 0 when nothing is
    /// selected. Drawn because a replace-on-type field that shows no selection
    /// is indistinguishable from an append-at-cursor one until it is too late.
    select_w: usize,
    /// Honest state block: bold headline (scale chosen to fit) + wrapped body.
    head: [u8; 96],
    head_len: usize,
    head_scale: usize,
    head_x: usize,
    head_y: usize,
    body: [&'static [u8]; 6],
    body_n: usize,
    body_x: [usize; 6],
    body_y: [usize; 6],
    /// Centre of the offline globe glyph above the headline.
    icon_cy: usize,
    /// First y the bookmark rows occupy, and how many rows there are.
    books_y: usize,
    books_n: usize,
    /// First y of the store's own status line under the last bookmark row.
    store_y: usize,
    store: [u8; 96],
    store_len: usize,
    store_ink: u32,
    /// Which add-bookmark step the pill is in, and the title typed so far.
    bm_step: BookmarkStep,
    bm_title: [u8; BM_TITLE_MAX],
    bm_title_len: usize,
}

impl Frame {
    fn build(app: &BrowserApp) -> Self {
        let mut f = Self {
            pill: [0; 96],
            pill_len: 0,
            pill_color: MUTED,
            show_caret: false,
            caret_x: PILL_TEXT_X,
            select_w: 0,
            head: [0; 96],
            head_len: 0,
            head_scale: 1,
            head_x: 0,
            head_y: 0,
            body: [&b""[..]; 6],
            body_n: 0,
            body_x: [0; 6],
            body_y: [0; 6],
            icon_cy: 0,
            books_y: 0,
            books_n: 0,
            store_y: 0,
            store: [0; 96],
            store_len: 0,
            store_ink: MUTED,
            bm_step: BookmarkStep::Idle,
            bm_title: [0; BM_TITLE_MAX],
            bm_title_len: 0,
        };

        // Address bar text. While editing, the caret sits on the real buffer;
        // otherwise the bar shows the attempted address (muted: not loaded)
        // or the placeholder when there is no address at all.
        let src: &[u8] = if app.address_len > 0 {
            &app.address[..app.address_len]
        } else if app.address_focused {
            &app.address[..0]
        } else if app.current_url_len > 0 {
            app.current_url()
        } else {
            PLACEHOLDER
        };
        f.pill_color = if app.address_len > 0 { INK } else { MUTED };
        f.pill_len = font::ellipsize(src, 1, PILL_TEXT_MAX, &mut f.pill);
        f.show_caret = app.address_focused;
        f.caret_x = PILL_TEXT_X + font::measure(&f.pill[..f.pill_len], 1);
        f.select_w = if app.select_all {
            font::measure(&f.pill[..f.pill_len], 1)
        } else {
            0
        };

        // Honest state block content.
        let (head_src, body_src): (&[u8], &[u8]) = match app.status {
            PageStatus::Blank => (b"No page is loaded", BODY_BLANK),
            PageStatus::NoTransport => (b"Navigation unavailable", BODY_NO_TRANSPORT),
            PageStatus::InvalidAddress => (b"Address cannot be opened", BODY_INVALID),
            PageStatus::LocalRefused => (b"Local document unavailable", BODY_LOCAL_REFUSED),
        };

        // Headline scale: pick the largest scale that fits the screen width.
        let max_line = W - 48;
        let mut head_scale = 2;
        if font::measure_bold(head_src, 2) > max_line {
            head_scale = 1;
        }
        f.head_scale = head_scale;
        if font::measure_bold(head_src, head_scale) > max_line {
            // Bold advances are at most one pixel wider per glyph than the
            // regular advances `ellipsize` uses, so budget the difference:
            // measure_bold(result) <= measure(result) + len <= max_line.
            let budget = max_line.saturating_sub(head_src.len());
            f.head_len = font::ellipsize(head_src, head_scale, budget, &mut f.head);
        } else {
            let n = head_src.len().min(f.head.len());
            f.head[..n].copy_from_slice(&head_src[..n]);
            f.head_len = n;
        }
        let head_w = font::measure_bold(&f.head[..f.head_len], head_scale);
        f.head_x = (W - head_w) / 2;

        // Body: word-wrapped by the SDK; wrap_text guarantees every produced
        // line measures <= STATE_MAX_W at scale 1.
        let mut lines: [&'static [u8]; 6] = [&b""[..]; 6];
        f.body_n = font::wrap_text(body_src, 1, STATE_MAX_W, 6, &mut lines);
        for i in 0..f.body_n {
            f.body[i] = lines[i];
            f.body_x[i] = (W - font::measure(lines[i], 1)) / 2;
        }

        // Bookmark rows, then the store's own status line, then the state block
        // centred in whatever is left.
        //
        // WHY they are stacked in one column rather than on two surfaces: the
        // bookmark list is a real panel of this app's own content, and the state
        // block is the honest report of the last navigation. Both belong on
        // screen at once, and the stack keeps the state block reachable without
        // a second tap.
        f.books_n = app.bookmarks.len() + app.bookmarks.unsaved_len();
        f.books_y = BOOKS_Y0;

        // The store's status line, in the store's own words. It is drawn whether
        // or not any bookmark is on screen, because "no bookmarks" and "the
        // kernel refused the read" are different facts with different causes.
        //
        // WHY the words come from `BookmarkStoreState::as_str` rather than being
        // restated here: the log line and this line have to say the same thing,
        // and two copies of the same sentence drift apart the first time one is
        // edited.
        let store_state = app.bookmarks.state();
        f.store_ink = if matches!(
            store_state,
            BookmarkStoreState::Loaded | BookmarkStoreState::Written
        ) {
            MUTED
        } else {
            WARNING
        };
        let mut store_src = [0u8; 96];
        let head: &[u8] = store_state.as_str().as_bytes();
        store_src[..head.len()].copy_from_slice(head);
        let mut store_n = head.len();
        if app.bookmarks.unsaved_len() > 0 {
            let tail = b" | unsaved this session";
            store_src[store_n..store_n + tail.len()].copy_from_slice(tail);
            store_n += tail.len();
        }
        f.store_len = font::ellipsize(&store_src[..store_n], 1, W - 48, &mut f.store);

        // The add-bookmark editor's step, so the pill can say what it wants.
        f.bm_step = app.bm_step;
        let tn = app.bm_title_len.min(BM_TITLE_MAX);
        f.bm_title[..tn].copy_from_slice(&app.bm_title[..tn]);
        f.bm_title_len = tn;

        // Vertical placement: centre the state block in the space under the
        // bookmark rows and their status line, snapped to the 8 pt grid.
        let list_bottom = f.books_y + f.books_n * BOOK_ROW_H + STATE_GAP_ROW;
        f.store_y = list_bottom;
        let head_h = head_scale * GLYPH_H;
        let body_h = if f.body_n == 0 {
            0
        } else {
            (f.body_n - 1) * STATE_LINE_STRIDE + GLYPH_H
        };
        let body_part = if f.body_n == 0 {
            0
        } else {
            STATE_GAP_BODY + body_h
        };
        let block_h = STATE_ICON_H + STATE_GAP_ICON + head_h + body_part;
        let area_h = VIEW_Y1 - STATE_BOX_TOP;
        let top = (STATE_BOX_TOP + area_h / 2 - block_h / 2) & !7usize;
        let _ = top;
        // Centred in the space the bookmark list leaves, not in the whole
        // viewport: with bookmarks on screen the state block has to sit below
        // them rather than over them.
        let top = (list_bottom + (VIEW_Y1 - list_bottom) / 2 - block_h / 2) & !7usize;
        f.icon_cy = top + STATE_ICON_H / 2;
        f.head_y = top + STATE_ICON_H + STATE_GAP_ICON;
        let body_y0 = f.head_y + head_h + STATE_GAP_BODY;
        for i in 0..f.body_n {
            f.body_y[i] = body_y0 + i * STATE_LINE_STRIDE;
        }

        f
    }
}

// -----------------------------------------------------------------------------
// Glyphs (24x24-family rounded line geometry, monochrome)
// -----------------------------------------------------------------------------

/// Offline globe inside the address bar: ring, equator, meridian arcs and a
/// diagonal slash. The slash cuts a background gap through the globe so the
/// glyph reads as "offline" at 18 px. Returns `Some(bg)` for the gap itself.
fn pill_state_glyph(x: usize, y: usize) -> Option<u32> {
    let dx = x as isize - 52;
    let dy = y as isize - 84;
    if dx.abs() > 12 || dy.abs() > 12 {
        return None;
    }
    let s = dx + dy;
    if s.abs() <= 2 {
        return Some(if s.abs() <= 1 { MUTED } else { SURFACE });
    }
    let d2 = dx * dx + dy * dy;
    if d2 >= 49 && d2 <= 81 {
        return Some(MUTED); // ring (r 7..9)
    }
    if dy >= -1 && dy <= 0 && d2 <= 81 {
        return Some(MUTED); // equator
    }
    // Meridian ellipse (semi-axes 4 x 9): G = dx^2 * 81 + dy^2 * 16.
    let g = dx * dx * 81 + dy * dy * 16;
    if g >= 933 && g <= 1296 {
        return Some(MUTED);
    }
    None
}

/// Close glyph: an 18 px cross, 2 px diagonal strokes.
fn close_glyph(x: usize, y: usize) -> bool {
    let cx = CLOSE_BTN.x + CLOSE_BTN.w / 2;
    let cy = CLOSE_BTN.y + CLOSE_BTN.h / 2;
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() > 9 || dy.abs() > 9 {
        return false;
    }
    (dx + dy).abs() <= 1 || (dx - dy).abs() <= 1
}

/// Bookmarks glyph: a five-point star inside the 48 px control.
///
/// WHY filled and outlined states: the store's own status line sits far down
/// the viewport, so the control itself has to say whether it opens a list or an
/// empty panel. Drawing the same glyph either way would leave the user tapping
/// into a panel with nothing in it, twice.
fn star_glyph(x: usize, y: usize, filled: bool) -> bool {
    let cx = BOOKMARKS_BTN.x + BOOKMARKS_BTN.w / 2;
    let cy = BOOKMARKS_BTN.y + BOOKMARKS_BTN.h / 2;
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() > 10 || dy.abs() > 10 {
        return false;
    }
    // Five-point outline: the two crossing diagonals of the upper triangle and
    // the two lower edges, plus the same set resolved inward when filled.
    let on_outline = {
        let top = dy <= 0 && dx.abs() as isize <= 8 + (-dy) / 3;
        let bottom = dy > 0 && dx.abs() as isize <= 9 - dy;
        top || bottom
    };
    if filled {
        on_outline || (dx.abs() <= 5 && dy.abs() <= 6)
    } else {
        on_outline
    }
}

/// Reload glyph: a 20 px ring with a gap at the top right and an arrow head
/// pointing into the gap.
fn reload_glyph(x: usize, y: usize) -> bool {
    let cx = RELOAD_BTN.x + RELOAD_BTN.w / 2;
    let cy = RELOAD_BTN.y + RELOAD_BTN.h / 2;
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    let d2 = dx * dx + dy * dy;
    if d2 >= 81 && d2 <= 100 && !(dx > 4 && dy < -4) {
        return true; // ring (r 8..10), gap top right
    }
    // Arrow head: right-pointing triangle, apex (cx+6, cy-10), base x = cx-2.
    let t = (cx + 6) as isize - x as isize;
    if t >= 0 && t <= 8 {
        let half = 4 - (t * 4) / 8;
        if (y as isize - (cy as isize - 10)).abs() <= half {
            return true;
        }
    }
    false
}

/// Large offline globe in the empty state (56 px box). Same construction as
/// the address-bar glyph, scaled up; the slash cut uses the base colour.
fn state_icon(x: usize, y: usize, cy: usize) -> Option<u32> {
    let dx = x as isize - (W / 2) as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() > 30 || dy.abs() > 30 {
        return None;
    }
    let s = dx + dy;
    if s.abs() <= 3 {
        return Some(if s.abs() <= 1 { MUTED } else { BASE });
    }
    let d2 = dx * dx + dy * dy;
    if d2 >= 441 && d2 <= 576 {
        return Some(MUTED); // ring (r 21..24)
    }
    if dy >= -1 && dy <= 0 && d2 <= 576 {
        return Some(MUTED); // equator
    }
    // Meridian ellipse (semi-axes 16 x 24): G = dx^2 * 576 + dy^2 * 256.
    let g = dx * dx * 576 + dy * dy * 256;
    if g >= 106000 && g <= 147456 {
        return Some(MUTED);
    }
    None
}

// -----------------------------------------------------------------------------
// Software rasteriser
// -----------------------------------------------------------------------------

#[inline(always)]
fn text_px(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
    bg: u32,
    bold: bool,
) -> Option<u32> {
    if x < x0 || y < y0 || y >= y0 + font::GLYPH_H * scale {
        return None;
    }
    font::test_text_aa(x, y, x0, y0, scale, text, color, bg, bold)
}

/// y 0..120: neutral status band + toolbar.
fn toolbar_pixel(x: usize, y: usize, app: &BrowserApp, f: &Frame) -> u32 {
    // Close control (surface + cross glyph).
    if let Some(c) = draw_rounded_rect(x, y, CLOSE_BTN, BTN_R, SURFACE, Some(BORDER), 1) {
        return if close_glyph(x, y) { INK } else { c };
    }
    // Reload control (surface + refresh glyph).
    if let Some(c) = draw_rounded_rect(x, y, RELOAD_BTN, BTN_R, SURFACE, Some(BORDER), 1) {
        return if reload_glyph(x, y) { MUTED } else { c };
    }
    // Bookmarks control: a filled star when the store holds at least one
    // bookmark, an outlined one when it does not. WHY the state is on the
    // control: the store's own status line is far down the viewport, and the
    // user needs to know whether this control opens a list or an empty panel.
    if let Some(c) = draw_rounded_rect(x, y, BOOKMARKS_BTN, BTN_R, SURFACE, Some(BORDER), 1) {
        let filled = app.bookmarks.len() + app.bookmarks.unsaved_len() > 0;
        return if star_glyph(x, y, filled) { INK } else { c };
    }
    // Address bar (capsule; accent border while focused).
    if let Some(c) = draw_rounded_rect(
        x,
        y,
        PILL,
        PILL_R,
        SURFACE,
        Some(if app.address_focused { ACCENT } else { BORDER }),
        1,
    ) {
        if let Some(g) = pill_state_glyph(x, y) {
            return g;
        }
        // Selection band, drawn under the text: the accent washed into the pill
        // fill, so a chosen address reads as chosen without a hard block of
        // colour sitting inside a rounded field.
        //
        // WHY it is computed and not returned outright: returning the fill as
        // soon as a pixel is inside the band would return it for the glyph's own
        // pixels too, painting the text out of existence. The band is instead
        // handed to the text renderer as the background its anti-aliased edges
        // blend against, and used as the fill only where no glyph covers the
        // pixel.
        let in_sel = f.select_w > 0
            && x >= PILL_TEXT_X
            && x < PILL_TEXT_X + f.select_w
            && y >= PILL_TEXT_Y
            && y < PILL_TEXT_Y + GLYPH_H;
        let t = &f.pill[..f.pill_len];
        if !t.is_empty() {
            let bg = if in_sel { selection_fill() } else { SURFACE };
            if let Some(px) = text_px(x, y, PILL_TEXT_X, PILL_TEXT_Y, 1, t, f.pill_color, bg, false)
            {
                return px;
            }
        }
        if in_sel {
            return selection_fill();
        }
        if f.show_caret
            && x >= f.caret_x
            && x < f.caret_x + 2
            && y >= PILL_TEXT_Y
            && y < PILL_TEXT_Y + GLYPH_H
        {
            return ACCENT;
        }
        return c;
    }
    BASE
}

/// y 120..1392: history strip, bookmark rows, store status and the honest
/// state block.
///
/// WHY all four live in one painter: they are one column on one surface, and
/// splitting them would mean four hit-testers agreeing on the same y-extents.
fn viewport_pixel(x: usize, y: usize, app: &BrowserApp, f: &Frame) -> u32 {
    // --- History strip -----------------------------------------------------
    if (NAV_STRIP_Y0..NAV_STRIP_Y0 + NAV_STRIP_H).contains(&y) {
        let pills: [(Rect, bool, &[u8]); 3] = [
            (BACK_BTN, app.history.can_back(), b"Back"),
            (FWD_BTN, app.history.can_forward(), b"Forward"),
            (ADD_BOOK_BTN, true, b"Bookmark"),
        ];
        for (bounds, active, label) in pills {
            if !bounds.contains(x, y) {
                continue;
            }
            // WHY an unavailable pill is recessed rather than merely greyed:
            // Back with nothing behind it is a control that cannot act, so it
            // is drawn in the same treatment the dialer uses for its inert
            // controls, and tapping it reports why rather than doing nothing.
            let (fill, ink) = if active {
                (SURFACE, INK)
            } else {
                (MUTED_FILL, MUTED)
            };
            let Some(c) = draw_rounded_rect(x, y, bounds, BTN_R, fill, Some(BORDER), 1) else {
                return BASE;
            };
            let mut buf = [0u8; 32];
            let n = font::ellipsize(label, 1, bounds.w - 16, &mut buf);
            if n > 0 {
                let tw = font::measure(&buf[..n], 1);
                let tx = bounds.x + (bounds.w - tw) / 2;
                let ty = bounds.y + (bounds.h - GLYPH_H) / 2;
                if let Some(px) = text_px(x, y, tx, ty, 1, &buf[..n], ink, fill, false) {
                    return px;
                }
            }
            return c;
        }
        // Position readout: "3 of 7" of the real history. WHY it is only drawn
        // when the history is non-empty: "1 of 0" would be a fabricated count.
        if app.history.len() > 0 {
            let mut buf = [0u8; 32];
            let mut n = 0usize;
            let pos = app.history.position();
            let len = app.history.len();
            buf[n] = b'0' + pos as u8;
            n += 1;
            let sep = b" of ";
            buf[n..n + sep.len()].copy_from_slice(sep);
            n += sep.len();
            buf[n] = b'0' + len as u8;
            n += 1;
            let tw = font::measure(&buf[..n], 1);
            let tx = W - 24 - tw;
            let ty = NAV_STRIP_Y0 + (NAV_STRIP_H - GLYPH_H) / 2;
            if let Some(px) = text_px(x, y, tx, ty, 1, &buf[..n], MUTED, BASE, false) {
                return px;
            }
        }
        return BASE;
    }

    // --- Bookmark rows -----------------------------------------------------
    if (BOOKS_Y0..BOOKS_Y0 + f.books_n * BOOK_ROW_H).contains(&y) {
        let row = (y - BOOKS_Y0) / BOOK_ROW_H;
        let row_y = BOOKS_Y0 + row * BOOK_ROW_H;
        // Saved rows come first, then unsaved ones, because the durable list is
        // the one the user asked the device to keep.
        let rec = if row < app.bookmarks.len() {
            app.bookmarks.record(row)
        } else {
            app.bookmarks.unsaved_record(row - app.bookmarks.len())
        };
        let Some(rec) = rec else {
            return BASE;
        };
        let Some(c) = draw_rounded_rect(
            x,
            y,
            Rect::new(24, row_y, W - 48, BOOK_ROW_H),
            12,
            SURFACE,
            Some(BORDER),
            1,
        ) else {
            return BASE;
        };
        let mut tbuf = [0u8; BM_TITLE_MAX];
        let tn = font::ellipsize(rec.title_bytes(), 1, W - 160, &mut tbuf);
        if tn > 0 {
            if let Some(px) =
                text_px(x, y, 48, row_y + 10, 1, &tbuf[..tn], INK, SURFACE, false)
            {
                return px;
            }
        }
        let mut ubuf = [0u8; BM_URL_MAX];
        let un = font::ellipsize(rec.url_bytes(), 1, W - 160, &mut ubuf);
        if un > 0 {
            if let Some(px) =
                text_px(x, y, 48, row_y + 40, 1, &ubuf[..un], MUTED, SURFACE, false)
            {
                return px;
            }
        }
        // Remove control: a 2 px cross centred on the row's right edge.
        let cx = W - 48;
        let dx = x as isize - cx as isize;
        let dy = y as isize - (row_y + BOOK_ROW_H / 2) as isize;
        if dx.abs() <= 12 && dy.abs() <= 12 && ((dx + dy).abs() <= 2 || (dx - dy).abs() <= 2) {
            return MUTED;
        }
        return c;
    }

    // --- Bookmark store status line ---------------------------------------
    if (f.store_y..f.store_y + GLYPH_H).contains(&y) && f.store_len > 0 {
        if let Some(px) =
            text_px(x, y, 24, f.store_y, 1, &f.store[..f.store_len], f.store_ink, BASE, false)
        {
            return px;
        }
    }

    // --- Honest state block ------------------------------------------------
    // Offline globe (its slash cut may overlay only its own 60 px box).
    if let Some(g) = state_icon(x, y, f.icon_cy) {
        return g;
    }
    // Headline.
    if f.head_len > 0 {
        if let Some(px) = text_px(
            x,
            y,
            f.head_x,
            f.head_y,
            f.head_scale,
            &f.head[..f.head_len],
            INK,
            BASE,
            true,
        ) {
            return px;
        }
    }
    // Wrapped body lines.
    for i in 0..f.body_n {
        if let Some(px) = text_px(x, y, f.body_x[i], f.body_y[i], 1, f.body[i], MUTED, BASE, false)
        {
            return px;
        }
    }
    BASE
}

fn get_browser_pixel(x: usize, y: usize, app: &BrowserApp, f: &Frame) -> u32 {
    if y < STATUS_H {
        // Neutral system band: no clock, radio or battery readings are bound
        // to this app, so none are drawn.
        return BASE;
    }
    if y < TOOLBAR_Y1 {
        return toolbar_pixel(x, y, app, f);
    }
    if y < VIEW_Y1 {
        return viewport_pixel(x, y, app, f);
    }
    // System gesture band.
    if GESTURE_PILL.contains(x, y) {
        return MUTED;
    }
    BASE
}

fn render_browser_ui(surface: u32, app: &BrowserApp) {
    render_browser_band(surface, app, 0, H);
}

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a band with no stride
/// bookkeeping. Every band starts on a row boundary, so the running `x`/`y`
/// cursor below is exact.
///
/// WHY this is correct rather than a shortcut: the band is chosen from what the
/// keystroke actually changed, and a keystroke that can change anything outside
/// it - navigation, or the first character that clears a previous status - asks
/// for the whole surface. The pixels outside a requested band are left as they
/// were drawn, and they are correct because nothing about them changed.
fn render_browser_band(surface: u32, app: &BrowserApp, y0: usize, y1: usize) {
    let frame = Frame::build(app);
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_browser_pixel(x, y, app, &frame);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
        offset += count;
    }
}

// -----------------------------------------------------------------------------
// Input handling. Every region below is drawn at the exact same coordinates by
// get_browser_pixel (shared Rect constants), so each control is hit-testable
// where it is rendered.
// -----------------------------------------------------------------------------

enum Tap<'a> {
    None,
    Redraw,
    Leave(&'a str),
}

/// The fill behind selected address text: the accent at 22% over the pill.
///
/// WHY a computed tint and not a fixed colour: the pill and the accent both come
/// from the theme, so a hard-coded selection colour would drift from them the
/// moment either changed - which is the same class of bug as hard-coding a
/// background behind anti-aliased text.
fn selection_fill() -> u32 {
    // WHY 38 and not a token amount: at 22 the tint was invisible on the pill's
    // own fill, and a selection nobody can see is not a selection - the field
    // still looked like it would append at the caret. 38 reads as a chosen run
    // without competing with the ink drawn on top of it.
    const PERCENT: u32 = 38;
    let t = PERCENT * 256 / 100;
    let channel = |c: u32, bg: u32| -> u32 { (c * t + bg * (256 - t)) >> 8 };
    let r = channel((ACCENT >> 16) & 0xFF, (SURFACE >> 16) & 0xFF);
    let g = channel((ACCENT >> 8) & 0xFF, (SURFACE >> 8) & 0xFF);
    let b = channel(ACCENT & 0xFF, SURFACE & 0xFF);
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

fn handle_tap(app: &mut BrowserApp, px: u32, py: u32) -> Tap<'static> {
    let (x, y) = (px as usize, py as usize);

    // System home gesture band: leave to the launcher (unchanged).
    if x >= HOME_X0 && x < HOME_X1 && y >= HOME_Y0 {
        return Tap::Leave("[BROWSER APP] Home gesture pressed. Exiting to launcher.");
    }

    // Close control: the only real "leave" action this app has.
    if CLOSE_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        return Tap::Leave("[BROWSER APP] Close control pressed. Exiting to launcher.");
    }

    // Address bar: focus the editable native field, pre-filling the real
    // attempted address so the caret sits on what would be edited.
    if PILL.contains(x, y) {
        // While the add-bookmark editor is running, the pill is the title or
        // address being typed, so focusing it must not overwrite it with the
        // attempted address.
        if app.bm_step == BookmarkStep::Idle && !app.address_focused {
            let n = app.current_url_len.min(app.address.len());
            app.address[..n].copy_from_slice(&app.current_url[..n]);
            app.address_len = n;
        }
        app.address_focused = true;
        // Select what is already there, so the first keystroke replaces it
        // instead of being appended to the end of "about:blank".
        app.select_all = app.address_len > 0;
        app.status = PageStatus::Blank;
        let _ = std::audio_haptic_click();
        std::log(0, "[BROWSER APP] Address field focused for keyboard input.").ok();
        return Tap::Redraw;
    }

    // Reload control: re-issue the real navigation attempt for the current
    // address. It reports the honest outcome (no transport, nothing loaded).
    if RELOAD_BTN.contains(x, y) {
        app.reload();
        return Tap::Redraw;
    }

    // Bookmarks control: starts the add-bookmark editor.
    if BOOKMARKS_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        app.start_add_bookmark();
        std::log(
            0,
            "[BROWSER APP] Bookmark control pressed: type the title, then the address.",
        )
        .ok();
        return Tap::Redraw;
    }

    // History strip.
    if BACK_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        app.go_back();
        return Tap::Redraw;
    }
    if FWD_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        app.go_forward();
        return Tap::Redraw;
    }
    if ADD_BOOK_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        if app.bm_step == BookmarkStep::Idle {
            app.start_add_bookmark();
        } else {
            // Pressing it again during the editor cancels, so a mis-tap on the
            // only entry point cannot strand the user in a half-typed bookmark.
            app.cancel_bookmark_step();
            std::log(0, "[BROWSER APP] Add-bookmark cancelled.").ok();
        }
        return Tap::Redraw;
    }

    // Bookmark rows: the body navigates to the address, the cross removes it.
    let books_n = app.bookmarks.len() + app.bookmarks.unsaved_len();
    if (BOOKS_Y0..BOOKS_Y0 + books_n * BOOK_ROW_H).contains(&y)
        && x >= 24
        && x < W - 24
    {
        let row = (y - BOOKS_Y0) / BOOK_ROW_H;
        let row_y = BOOKS_Y0 + row * BOOK_ROW_H;
        let cx = W - 48;
        let dx = x as isize - cx as isize;
        let dy = y as isize - (row_y + BOOK_ROW_H / 2) as isize;
        if dx.abs() <= 12 && dy.abs() <= 12 {
            // Removal hits the real store: a durable bookmark is deleted from
            // storage and the in-memory list follows only if the write is
            // acknowledged; an unsaved one was never stored.
            let _ = std::audio_haptic_click();
            if row < app.bookmarks.len() {
                match app.bookmarks.remove(row) {
                    Ok(()) => {
                        std::log(
                            0,
                            "[BROWSER APP] Bookmark removed and the store write was acknowledged.",
                        )
                        .ok();
                    }
                    Err(_) => {
                        std::log(
                            0,
                            "[BROWSER APP] NOT deleted from storage: the kernel refused the store write, so the stored copy is unchanged.",
                        )
                        .ok();
                    }
                }
            } else {
                let _ = app.bookmarks.remove_unsaved(row - app.bookmarks.len());
                std::log(
                    0,
                    "[BROWSER APP] Unsaved bookmark dropped. It was never written to storage, so nothing had to be removed from it.",
                )
                .ok();
            }
            return Tap::Redraw;
        }
        let rec = if row < app.bookmarks.len() {
            app.bookmarks.record(row)
        } else {
            app.bookmarks.unsaved_record(row - app.bookmarks.len())
        };
        if let Some(rec) = rec {
            let mut target = [0u8; 64];
            let n = rec.url_bytes().len().min(target.len());
            target[..n].copy_from_slice(&rec.url_bytes()[..n]);
            let _ = std::audio_haptic_click();
            std::log(0, "[BROWSER APP] Bookmark navigation attempt:").ok();
            app.navigate(&target[..n]);
            return Tap::Redraw;
        }
    }

    Tap::None
}

/// Which part of the surface a keystroke changed.
///
/// WHY this exists: rendering the browser re-rasterised all 1,036,800 pixels
/// through `get_browser_pixel` for every single keystroke, so typing a URL cost
/// the same as a full screen transition. Typing moves the address text, the
/// caret, and at most the status block - between 72 and 200 of the 1440 rows.
/// Naming the region lets the renderer skip the rest, which is where the
/// perceived latency of typing was going.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing changed; do not render.
    None,
    /// Only the address text, caret or focus ring moved: the toolbar band.
    Toolbar,
    /// Navigation ran or the status block changed: the whole surface.
    All,
}

fn handle_browser_key(app: &mut BrowserApp, key: std::InputKeyEvent) -> Dirty {
    use std::input_key_code;
    if !key.pressed || !app.address_focused {
        return Dirty::None;
    }
    match key.code {
        input_key_code::ENTER => {
            app.address_focused = false;
            app.select_all = false;
            if app.bm_step == BookmarkStep::Idle {
                // Submitting the typed address performs a real navigation attempt
                // against the real parser and transport state.
                if app.address_len > 0 {
                    let mut target = [0u8; 64];
                    let n = app.address_len.min(target.len());
                    target[..n].copy_from_slice(&app.address[..n]);
                    app.navigate(&target[..n]);
                }
            } else {
                // Enter commits the step the bookmark editor is on, and the
                // pill then holds whatever the next step asks for.
                let _ = app.commit_bookmark_step();
            }
            Dirty::All
        }
        input_key_code::ESCAPE => {
            // Escape out of a half-typed bookmark cancels it rather than
            // leaving the pill in a mode the toolbar no longer names.
            if app.bm_step != BookmarkStep::Idle {
                app.cancel_bookmark_step();
                std::log(0, "[BROWSER APP] Add-bookmark cancelled.").ok();
                return Dirty::All;
            }
            app.address_focused = false;
            app.select_all = false;
            Dirty::Toolbar
        }
        input_key_code::BACKSPACE => {
            // Backspace over a selection deletes the selection, which is what
            // every text field does; otherwise it deletes one character.
            if app.select_all {
                app.address_len = 0;
                app.select_all = false;
            } else {
                app.address_len = app.address_len.saturating_sub(1);
            }
            Dirty::Toolbar
        }
        code if (32..=126).contains(&code) && app.address_len < app.address.len() => {
            // A character typed over a selection replaces it. Clearing first is
            // also what makes room: the field is 64 bytes and a full buffer
            // would otherwise refuse the keystroke outright.
            if app.select_all {
                app.address_len = 0;
                app.select_all = false;
            }
            app.address[app.address_len] = code as u8;
            app.address_len += 1;
            // Editing again: the previous attempt's message is superseded.
            //
            // WHY the status is compared before it is cleared: clearing it can
            // change the state block far below the toolbar, so on the keystroke
            // that first clears it the surface genuinely changed everywhere. On
            // every keystroke after that the status is already Blank, only the
            // address moved, and the toolbar band is the honest region.
            let status_moved = app.status != PageStatus::Blank;
            app.status = PageStatus::Blank;
            if status_moved {
                Dirty::All
            } else {
                Dirty::Toolbar
            }
        }
        _ => return Dirty::None,
    }
}

fn destroy_and_exit(surface: u32) -> ! {
    if surface > 0 {
        let _ = std::surface_destroy(6, surface);
    }
    std::exit_with_code(0);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(
        0,
        "[BROWSER APP (Ring 3)] Sovereign Decentralized Web & InterLayer Browser online.",
    )
    .ok();

    let mut browser = BrowserApp::new();

    // Allocate DRM/KMS Client Surface (z=200)
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(id) => {
            std::log(0, "[BROWSER APP] DRM/KMS Client Surface allocated (z=200).").ok();
            id
        }
        Err(_) => 0,
    };

    if surface > 0 {
        render_browser_ui(surface, &browser);
        let _ = std::surface_flip(6);
        std::log(
            0,
            "[BROWSER APP] Browser canvas presented to scanout buffer.",
        )
        .ok();
    }

    // Sovereign Asynchronous Event Reactor Loop
    let interest_mask = std::EVENT_TOUCH_INPUT
        | std::EVENT_IPC_MSG
        | std::EVENT_NETWORK_PACKET
        | std::EVENT_TIMER_TICK;
    loop {
        match std::event_poll(interest_mask, 150) {
            Ok(events) => {
                if events & std::EVENT_TOUCH_INPUT != 0 {
                    let mut input = std::InputEvent {
                        kind: 0,
                        x: 0,
                        y: 0,
                        dx: 0,
                        dy: 0,
                        scale_milli: 1000,
                    };
                    let mut dirty = Dirty::None;
                    while std::input_event_poll(&mut input).is_ok() {
                        if let Some(key) = input.key_event() {
                            let changed = handle_browser_key(&mut browser, key);
                            if changed != Dirty::None {
                                dirty = Dirty::All;
                            }
                            continue;
                        }
                        if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                            std::log(
                                0,
                                "[BROWSER APP] Navigation gesture recognized. Exiting to launcher.",
                            )
                            .ok();
                            destroy_and_exit(surface);
                        }
                        if input.kind == 1 {
                            match handle_tap(&mut browser, input.x, input.y) {
                                Tap::Leave(msg) => {
                                    std::log(0, msg).ok();
                                    destroy_and_exit(surface);
                                }
                                Tap::Redraw => dirty = Dirty::All,
                                Tap::None => {}
                            }
                        }
                    }
                    if dirty != Dirty::None && surface > 0 {
                        match dirty {
                            Dirty::None => {}
                            Dirty::Toolbar => {
                                // The toolbar band, generously: STATUS_H..TOOLBAR_Y1
                                // covers the pill, its focus ring, and the
                                // reload/close controls in the same row.
                                render_browser_band(surface, &browser, STATUS_H, TOOLBAR_Y1);
                            }
                            Dirty::All => render_browser_ui(surface, &browser),
                        }
                        let _ = std::surface_flip(6);
                    }
                }
                if events & std::EVENT_IPC_MSG != 0 {
                    // WHY: no navigation intent was fetched or parsed out of this
                    // event. Report the event itself, not a handled intent.
                    std::log(
                        0,
                        "[BROWSER APP] IPC message event raised; no navigation intent was parsed from it.",
                    )
                    .ok();
                }
                if events & std::EVENT_NETWORK_PACKET != 0 {
                    // WHY: this app opens no socket and reads no bytes from one,
                    // so no packet was received or parsed. Report the event and
                    // the absence of a transport.
                    match browser.active_socket_id {
                        Some(_) => {
                            std::log(
                                0,
                                "[BROWSER APP] Network event raised; no packet body was read from the socket.",
                            )
                            .ok();
                        }
                        None => {
                            std::log(
                                0,
                                "[BROWSER APP] Network event raised with no open socket; no packet was received.",
                            )
                            .ok();
                        }
                    }
                }
                if events & std::EVENT_TIMER_TICK != 0 {
                    // Periodic cache maintenance
                    std::yield_now();
                }
            }
            Err(_) => {
                std::yield_now();
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[BROWSER APP] Panic detected! Terminating task...").ok();
    std::exit();
}
