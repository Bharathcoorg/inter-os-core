//! Inter OS Native Sovereign Browser & Web Client
//!
//! High-performance Ring-3 (User Mode) smartphone browser laid out for the
//! 720x1440 sovereign smartphone viewport:
//! - Full Mobile UI: Omnibox with SSL security padlock, domain highlighting,
//!   tab counter badge, overflow kebab menu, and reload
//! - Start Page / New Tab:
//!   - Sovereign browser emblem
//!   - Quick Search Pill
//!   - "Your pages": saved bookmarks and session visit history
//! - Web Viewport / Content Area:
//!   - Classified document lines (H1, H2, H3, bullet lists, code blocks, gemtext links)
//!   - Smooth vertical scrolling with scroll position indicator
//!   - Tapping links navigates directly to target
//! - Tab Switcher Sheet overlay (up to 4 tabs, switch, close, new tab)
//! - Bookmarks & History sheet overlays
//! - Kernel socket integration: calls non-blocking socket syscalls
//!   (`std::net_socket`, `net_connect`, `net_send`, `net_recv`) with truthful error reporting
//! - Strict adherence to Platform Truth & Native UI contracts

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use interos_std::design::font;
use interos_std::design::primitives::{draw_rounded_rect, Rect};

use browser::{
    format_http_get, format_proxy_request, into_url_field, parse_gemtext_link, parse_uri,
    scheme_mark, DocLine, DocumentTokenizer, LineKind, LoadingState, TabSessionManager, UriScheme,
    MAX_DOC_LINES, MAX_LINE_BYTES, MAX_TABS, URL_FIELD,
};

// -----------------------------------------------------------------------------
// Sovereign InterOS Browser Design Tokens
// -----------------------------------------------------------------------------
const CHROME_BG: u32 = 0xFF0B0F17;            // Obsidian Slate Deep Canvas
const CHROME_SURFACE: u32 = 0xFF131A26;       // Elevated Glass Surface
#[allow(dead_code)]
const CHROME_SURFACE_HOVER: u32 = 0xFF1E2838; // Slate Hover
const CHROME_BORDER: u32 = 0xFF1E293B;        // Delicate Hairline Border
const CHROME_DIVIDER: u32 = 0xFF182232;       // Section Divider
const CHROME_BLUE: u32 = 0xFF38BDF8;          // Electric Sapphire Accent
#[allow(dead_code)]
const CHROME_BLUE_DARK: u32 = 0xFF1E3A8A;     // Deep Sapphire
const CHROME_TEXT: u32 = 0xFFF8FAFC;          // Pure Crisp Off-White Text
const CHROME_MUTED: u32 = 0xFF94A3B8;         // Muted Slate Text
const CHROME_CARD: u32 = 0xFF161F30;          // Card Background
const CHROME_CARD_BORDER: u32 = 0xFF283548;   // Card Stroke
const CHROME_RED: u32 = 0xFFF87171;           // Alert / Error Coral Red
#[allow(dead_code)]
const CHROME_CODE_BG: u32 = 0xFF0F172A;       // Code Block Slate Dark

// -----------------------------------------------------------------------------
// Viewport & Layout Geometry (720x1440, 8pt Grid)
// -----------------------------------------------------------------------------
// -----------------------------------------------------------------------------
// HTTP over TCP
// -----------------------------------------------------------------------------

/// The kernel's "not ready" result code.
///
/// WHY it is named here: a TCP connection is not established the instant
/// `tcp_connect` returns -- the handshake completes asynchronously -- so
/// the transport has to tell "not yet" apart from "failed". The kernel
/// reports that as `SyscallResult::NotReady`.
const NOT_READY: i64 = -30;

/// The largest HTTP response the browser will hold.
///
/// WHY this is large: the Device Studio gateway serves a real page, and a
/// documentation page is more than 64 KB of HTML. The gateway itself caps a
/// response at 1 MB, so a response that reaches this limit has been
/// truncated by the browser and the screen says so.
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// The buffer an HTTP response is accumulated into.
///
/// WHY this is a static and not a local array: it is a quarter of a
/// megabyte, and a local of that size does not belong on a 128 KB task
/// stack.
static mut RESPONSE_BUF: [u8; MAX_RESPONSE_BYTES] = [0; MAX_RESPONSE_BYTES];

/// The buffer the page's HTML is extracted into.
static mut HTML_BUF: [u8; browser::MAX_HTML_BYTES] = [0; browser::MAX_HTML_BYTES];

/// The buffer a local document is read into.
static mut FILE_BUF: [u8; MAX_FILE_BYTES] = [0; MAX_FILE_BYTES];

/// The largest local document the browser will read.
const MAX_FILE_BYTES: usize = 64 * 1024;

/// The buffer the reader document is parsed into.
static mut READER_BUF: [u8; browser::MAX_READER_BYTES] = [0; browser::MAX_READER_BYTES];

/// How many times a send is retried while the connection finishes its
/// handshake.
///
/// WHY a retry loop and not a single send: `tcp_send` refuses with
/// `NotReady` until the connection reaches the established state, and the
/// handshake completes asynchronously -- the SYN-ACK arrives on the
/// network and is processed by the kernel's poll path. The previous
/// implementation issued one datagram and read once, so it read nothing
/// and reported a transport failure for every page.
const TCP_SEND_ATTEMPTS: usize = 200;

/// How many times a receive is retried while the response arrives.
const TCP_RECV_ATTEMPTS: usize = 200;

/// The outcome of a fetch over TCP.
struct FetchOutcome {
    /// The number of bytes received.
    bytes: usize,
    /// Whether the response filled the buffer, and so was truncated.
    truncated: bool,
    /// The last error the transport reported, if any.
    error: i64,
}

/// Fetches an HTTP response over TCP.
///
/// WHY TCP and not UDP: the Device Studio gateway speaks HTTP over TCP.
/// The browser used to open a UDP socket and send the request as a single
/// datagram, which no TCP server answers, and then read once -- before a
/// response could have arrived. The device therefore reported a transport
/// failure for every page, including the ones the gateway would have
/// served.
fn fetch_over_tcp(
    dst_ip: [u8; 4],
    dst_port: u16,
    local_port: u16,
    request: &[u8],
) -> FetchOutcome {
    let mut outcome = FetchOutcome {
        bytes: 0,
        truncated: false,
        error: 0,
    };

    // The initial sequence number is the device's choice, and a fresh one
    // per connection is what the protocol asks for.
    let mut seq_bytes = [0u8; 4];
    let seq = match std::get_random(&mut seq_bytes) {
        Ok(n) if n == 4 => u32::from_le_bytes(seq_bytes),
        _ => 0x1F2E_3D4C,
    };

    let connection = match std::tcp_connect(dst_ip, dst_port, local_port, seq) {
        Ok(id) => id,
        Err(code) => {
            outcome.error = code;
            return outcome;
        }
    };

    // The handshake is asynchronous, so the first sends are refused. The
    // refusal is `NotReady`, which is not a failure -- it means the
    // connection is not established yet.
    let mut sent = false;
    for _ in 0..TCP_SEND_ATTEMPTS {
        match std::tcp_send(connection, request) {
            Ok(()) => {
                sent = true;
                break;
            }
            Err(code) => {
                if code != NOT_READY {
                    outcome.error = code;
                    let _ = std::tcp_close(connection);
                    return outcome;
                }
            }
        }
        std::yield_now();
    }
    if !sent {
        outcome.error = NOT_READY;
        let _ = std::tcp_close(connection);
        return outcome;
    }

    // The response is read until the server closes the connection. The
    // gateway speaks HTTP/1.0 and sends no content length, so the end of
    // the body is the end of the stream.
    let mut total = 0usize;
    let mut reads = 0usize;
    while reads < TCP_RECV_ATTEMPTS {
        reads += 1;
        let buffer = unsafe { &mut *core::ptr::addr_of_mut!(RESPONSE_BUF) };
        if total >= buffer.len() {
            outcome.truncated = true;
            break;
        }
        match std::tcp_recv(connection, &mut buffer[total..]) {
            Ok(0) => break,
            Ok(n) => {
                total += n;
            }
            Err(code) => {
                if code != NOT_READY {
                    // A closed connection ends the response. The bytes
                    // already read are still the response.
                    if total == 0 {
                        outcome.error = code;
                    }
                    break;
                }
            }
        }
        std::yield_now();
    }
    if reads >= TCP_RECV_ATTEMPTS {
        outcome.truncated = true;
    }

    let _ = std::tcp_close(connection);
    outcome.bytes = total;
    outcome
}

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// y 0..48: System Status Bar
const STATUS_H: usize = 48;

/// y 48..116: Sovereign Top Omnibox App Bar
#[allow(dead_code)]
const TOOLBAR_Y0: usize = STATUS_H;
const TOOLBAR_Y1: usize = 116;

/// Sovereign Omnibox URL Capsule (x 16..476)
/// Appends bytes to a small buffer, truncating rather than overrunning.
fn append_text(buf: &mut [u8], n: usize, bytes: &[u8]) -> usize {
    let take = bytes.len().min(buf.len().saturating_sub(n));
    buf[n..n + take].copy_from_slice(&bytes[..take]);
    n + take
}

/// Appends the decimal form of a value to a small buffer.
fn append_u64_text(buf: &mut [u8], mut n: usize, mut value: u64) -> usize {
    if value == 0 {
        return append_text(buf, n, b"0");
    }
    let mut digits = [0u8; 20];
    let mut len = 0usize;
    while value > 0 && len < digits.len() {
        digits[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    while len > 0 {
        len -= 1;
        if n < buf.len() {
            buf[n] = digits[len];
            n += 1;
        }
    }
    n
}

/// The type scale of a document title.
///
/// WHY there is a scale and not a size: the platform font offers integer
/// scales, and the reader's three heading levels were all set at scale 1 --
/// so a page title and a section heading were the same size on the screen
/// and differed only in colour.
const TYPE_H1_SCALE: usize = 2;

const PILL: Rect = Rect::new(16, 56, 460, 52);
const PILL_R: usize = 26;
const PILL_TEXT_X: usize = 56;
const PILL_TEXT_Y: usize = 70;
const PILL_TEXT_MAX: usize = 380;

/// Omnibox Right Controls
const RELOAD_BTN: Rect = Rect::new(486, 60, 44, 44);
const TABS_TOP_BTN: Rect = Rect::new(540, 60, 46, 44);
const KEBAB_BTN: Rect = Rect::new(596, 60, 44, 44);
const CLOSE_BTN: Rect = Rect::new(650, 60, 44, 44);
const BTN_R: usize = 12;

/// Start Page Favorites / Quick Access Geometry (4 columns)
#[allow(dead_code)]
const QA_ROW_Y: usize = 282;
#[allow(dead_code)]
const QA_TILE_SIZE: usize = 64;

/// Sovereign Bottom Navigation Bar (y 1350..1410)
const NAV_BAR_Y0: usize = 1350;
const NAV_BAR_Y1: usize = 1410;
const BACK_BTN: Rect = Rect::new(24, 1356, 110, 48);
const FWD_BTN: Rect = Rect::new(160, 1356, 110, 48);
const HOME_TAB_BTN: Rect = Rect::new(296, 1356, 128, 48);
const ADD_BOOK_BTN: Rect = Rect::new(448, 1356, 110, 48);
const TABS_BTN: Rect = Rect::new(582, 1356, 114, 48);

/// System Gesture Home Indicator (y 1410..1440)
const GESTURE_PILL: Rect = Rect::new(260, 1422, 200, 6);
const HOME_Y0: usize = 1410;

const PLACEHOLDER: &[u8] = b"Search or type URL";

// Contract strings pinned for test suites
const BODY_BLANK: &[u8] = b"Sovereign Decentralized Web Client on InterOS Microkernel.";
const BODY_NO_TRANSPORT: &[u8] = b"The address was parsed and a navigation was attempted, \
but no network transport is bound to this app: no request was sent and no page was loaded.";
const BODY_INVALID: &[u8] = b"Supported schemes are http, https, interlayer, file and gemini \
followed by a host. Even a valid address loads nothing while no transport is bound.";
const BODY_LOCAL_NO_CAPABILITY: &[u8] = b"The address names a local document, but this app holds \
no filesystem read capability, so it cannot open a guest file at all.";
const BODY_LOCAL_REFUSED: &[u8] = b"The address names a local document, and this app holds the \
capability to read one, but the guest filesystem refused to open it. Nothing was read.";
const BODY_LOCAL_EMPTY: &[u8] = b"The address names a local document and the guest filesystem \
opened it, but the file holds no bytes. There is nothing to show.";
const BODY_LOCAL_READ_REFUSED: &[u8] = b"The address names a local document and the guest \
filesystem opened it, but the read itself was refused. Nothing was shown.";
const BODY_LOCAL_NO_PATH: &[u8] = b"The address names a local document but carries no path, so \
there is nothing for the browser to open.";

/// Why a local document could not be shown.
///
/// WHY this is carried rather than inferred: the error screen used to
/// print one paragraph for every local failure, so a file that opened and
/// held no bytes was reported as an app that held no filesystem
/// capability. The screen has to state the failure that happened.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LocalFailure {
    /// The address carried no path.
    NoPath,
    /// The app holds no filesystem read capability.
    NoCapability,
    /// The filesystem refused to open the path.
    Refused,
    /// The document opened and held no bytes.
    Empty,
    /// The document opened and the read itself was refused.
    ReadRefused,
}

// -----------------------------------------------------------------------------
// Network Socket Syscall Wrappers
// -----------------------------------------------------------------------------
pub fn net_socket(port: u16) -> Result<u32, i64> {
    std::net_socket(port)
}

pub fn net_connect(dst_ip: [u8; 4], dst_port: u16) -> Result<u32, i64> {
    std::tcp_connect(dst_ip, dst_port, 49152, 1)
}

pub fn net_send(sock_or_conn: u32, payload: &[u8]) -> Result<(), i64> {
    std::tcp_send(sock_or_conn, payload)
}

pub fn net_recv(sock_or_conn: u32, buf: &mut [u8]) -> Result<usize, i64> {
    std::tcp_recv(sock_or_conn, buf)
}

// -----------------------------------------------------------------------------
// Bookmark Data Models & Settings Persistence
// -----------------------------------------------------------------------------
const BM_TITLE_MAX: usize = 24;
const BM_URL_MAX: usize = 48;
const MAX_BOOKMARKS: usize = 6;
const BM_RECORD_BYTES: usize = 1 + BM_TITLE_MAX + BM_URL_MAX;
const STORE_BLOB_BYTES: usize = 2 + MAX_BOOKMARKS * BM_RECORD_BYTES;
const STORE_VERSION: u8 = 1;
const STORE_KEY: &str = "browser.bookmarks.v1";

#[derive(Clone, Copy)]
pub struct Bookmark {
    pub name: &'static str,
    pub url: &'static str,
    pub is_sovereign: bool,
}
pub static BOOKMARKS: [Bookmark; 0] = [];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookmarkStoreState {
    Untouched,
    Empty,
    Loaded,
    ReadRefused,
    Invalid,
    Written,
    WriteRefused,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookmarkRecord {
    id: u8,
    title: [u8; BM_TITLE_MAX],
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

    fn valid(&self) -> bool {
        (1..=MAX_BOOKMARKS as u8).contains(&self.id)
            && self.title_bytes().iter().all(|b| (0x20..=0x7E).contains(b))
            && self.url_bytes().iter().all(|b| (0x20..=0x7E).contains(b))
    }

    fn title_len(&self) -> usize {
        self.title.iter().position(|&b| b == 0).unwrap_or(BM_TITLE_MAX)
    }

    fn url_len(&self) -> usize {
        self.url.iter().position(|&b| b == 0).unwrap_or(BM_URL_MAX)
    }

    fn title_bytes(&self) -> &[u8] {
        &self.title[..self.title_len()]
    }

    pub fn url_bytes(&self) -> &[u8] {
        &self.url[..self.url_len()]
    }

    fn encode(&self, out: &mut [u8]) {
        out[0] = self.id;
        out[1..1 + BM_TITLE_MAX].copy_from_slice(&self.title);
        out[1 + BM_TITLE_MAX..1 + BM_TITLE_MAX + BM_URL_MAX].copy_from_slice(&self.url);
    }

    fn decode(bytes: &[u8]) -> Option<BookmarkRecord> {
        if bytes.len() < BM_RECORD_BYTES {
            return None;
        }
        let mut rec = BookmarkRecord::EMPTY;
        rec.id = bytes[0];
        rec.title.copy_from_slice(&bytes[1..1 + BM_TITLE_MAX]);
        let url_at = 1 + BM_TITLE_MAX;
        rec.url.copy_from_slice(&bytes[url_at..url_at + BM_URL_MAX]);
        if rec.valid() {
            Some(rec)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BookmarkStore {
    records: [BookmarkRecord; MAX_BOOKMARKS],
    len: u8,
    unsaved: [BookmarkRecord; MAX_BOOKMARKS],
    unsaved_len: u8,
    state: BookmarkStoreState,
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

    pub fn load() -> Self {
        let mut store = BookmarkStore::EMPTY;
        let mut blob = [0u8; STORE_BLOB_BYTES];
        match std::settings_get(STORE_KEY, &mut blob) {
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
                } else {
                    store.records = records;
                    store.len = count as u8;
                    store.state = BookmarkStoreState::Loaded;
                    store.next_id = (count as u8 + 1).min(MAX_BOOKMARKS as u8);
                }
            }
            Err(_) => {
                store.state = BookmarkStoreState::ReadRefused;
            }
        }
        store
    }

    pub fn persist(&mut self) -> Result<(), i64> {
        let mut blob = [0u8; STORE_BLOB_BYTES];
        blob[0] = STORE_VERSION;
        blob[1] = self.len;
        for i in 0..self.len as usize {
            let offset = 2 + i * BM_RECORD_BYTES;
            self.records[i].encode(&mut blob[offset..offset + BM_RECORD_BYTES]);
        }
        let used = 2 + self.len as usize * BM_RECORD_BYTES;
        if blob[..used].iter().any(|&b| b == b'\n') {
            self.state = BookmarkStoreState::WriteRefused;
            return Err(-22);
        }
        match std::settings_set(STORE_KEY, &blob[..used]) {
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
            self.state = BookmarkStoreState::Full;
            return Err(-28);
        }
        self.records[self.len as usize] = rec;
        self.len += 1;
        self.next_id += 1;
        match self.persist() {
            Ok(()) => Ok(()),
            Err(code) => {
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

    pub fn state(&self) -> BookmarkStoreState {
        self.state
    }
}

// -----------------------------------------------------------------------------
// History Stack
// -----------------------------------------------------------------------------
const HISTORY_CAP: usize = 16;

#[derive(Debug, Clone, Copy)]
pub struct History {
    entries: [[u8; 64]; HISTORY_CAP],
    len: usize,
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

    pub fn push(&mut self, target: &[u8]) {
        if self.pos + 1 < self.len {
            self.len = self.pos + 1;
        }
        if self.len >= HISTORY_CAP {
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

    pub fn can_back(&self) -> bool {
        self.pos > 0
    }

    pub fn can_forward(&self) -> bool {
        self.pos + 1 < self.len
    }

    pub fn entry(&self, index: usize) -> Option<&[u8]> {
        if index >= self.len {
            return None;
        }
        let raw = &self.entries[index];
        let n = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        if n == 0 {
            None
        } else {
            Some(&raw[..n])
        }
    }
}

// -----------------------------------------------------------------------------
// Sheet Overlays
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetOverlay {
    None,
    TabSwitcher,
    Bookmarks,
    History,
    Menu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageStatus {
    Blank,
    Loaded,
    NoTransport,
    InvalidAddress,
    LocalRefused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookmarkStep {
    Idle,
    Title,
    Address,
}

// -----------------------------------------------------------------------------
// Browser Application State
// -----------------------------------------------------------------------------
pub struct BrowserApp {
    pub current_url: [u8; 64],
    pub current_url_len: usize,
    pub scroll_offset_px: usize,
    pub pages_loaded: usize,
    pub trackers_blocked: usize,
    pub active_socket_id: Option<u32>,
    pub address: [u8; 64],
    pub address_len: usize,
    pub address_focused: bool,
    pub select_all: bool,
    pub history: History,
    pub bookmarks: BookmarkStore,
    pub bm_step: BookmarkStep,
    pub bm_title: [u8; BM_TITLE_MAX],
    pub bm_title_len: usize,
    pub status: PageStatus,
    pub loading_progress: u8,
    pub error_code: i32,
    pub tabs: TabSessionManager,
    pub overlay: SheetOverlay,
    pub doc_lines: [DocLine; MAX_DOC_LINES],
    pub doc_line_count: usize,
    /// How many lines the document has, before the reader's bound.
    ///
    /// WHY it is counted: the reader holds a bounded number of lines, and
    /// a page that is longer than that is an excerpt. The screen says
    /// which, rather than presenting a fragment as the whole article.
    pub doc_total_lines: usize,
    pub kb_shift: bool,
    pub kb_symbols: bool,
    /// Whether the last response was larger than the browser's buffer.
    ///
    /// WHY this is remembered: a response that overflows the buffer is the
    /// beginning of a page, not the whole of it, and the screen has to say
    /// so rather than present a fragment as the article.
    pub response_truncated: bool,
    /// Why the last local document could not be shown.
    pub local_failure: LocalFailure,
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
            loading_progress: 0,
            error_code: 0,
            tabs: TabSessionManager::new(),
            overlay: SheetOverlay::None,
            doc_lines: [DocLine::EMPTY; MAX_DOC_LINES],
            doc_line_count: 0,
            doc_total_lines: 0,
            kb_shift: false,
            kb_symbols: false,
            response_truncated: false,
            local_failure: LocalFailure::NoPath,
        };
        app.tabs.init_default();
        let start = b"about:blank";
        app.current_url[..start.len()].copy_from_slice(start);
        app.current_url_len = start.len();
        app
    }

    fn current_url(&self) -> &[u8] {
        &self.current_url[..self.current_url_len]
    }

    /// Reads a local document from the guest filesystem and renders it.
    ///
    /// WHY the same pipeline as a fetched page: a local file and a
    /// downloaded page are both documents, and a reader that treated them
    /// differently would be two readers.
    fn load_local_document(&mut self, path: &[u8]) -> bool {
        if path.is_empty() {
            self.status = PageStatus::LocalRefused;
            self.local_failure = LocalFailure::NoPath;
            self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
            return false;
        }
        let Ok(path_str) = core::str::from_utf8(path) else {
            self.status = PageStatus::LocalRefused;
            self.local_failure = LocalFailure::NoPath;
            self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
            self.tabs.active_tab_mut().error_code = -3; // EINVAL
            return false;
        };

        // WHY the capability is asked for by its type and not by a slot: a
        // slot is a position in this task's capability table, and the
        // position depends on the order the kernel inserted the
        // capabilities in. The kernel authorises the open itself by
        // capability type, so all the browser has to know is whether it
        // holds a filesystem read authority at all.
        //
        // WHY the platform is asked rather than the answer assumed: the
        // local branch used to report that the app held no filesystem
        // capability without asking, so a refusal the browser had invented
        // was shown as a refusal the device had given. The question is
        // asked here, and only this answer is reported.
        if std::cap_lookup(std::cap_type::FILE_SYSTEM_READ, 0).is_err() {
            std::log("[BROWSER APP] This app holds no filesystem read capability, so a local document cannot be opened.")
                .ok();
            self.status = PageStatus::LocalRefused;
            self.local_failure = LocalFailure::NoCapability;
            self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
            self.tabs.active_tab_mut().error_code = -1; // EPERM
            let _ = std::audio_haptic_click();
            return false;
        }

        let fd = match std::fs_open(path_str, std::FS_READ) {
            Ok(fd) => fd,
            Err(code) => {
                std::log("[BROWSER APP] The guest filesystem refused to open the local document.")
                    .ok();
                self.status = PageStatus::LocalRefused;
                self.local_failure = LocalFailure::Refused;
                self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
                self.tabs.active_tab_mut().error_code = code as i32;
                let _ = std::audio_haptic_click();
                return false;
            }
        };

        // WHY the read's own result is kept: a refused read and an empty
        // file are different failures, and discarding the error reported
        // every one of them as an empty file.
        let read_result = {
            let buffer = unsafe { &mut *core::ptr::addr_of_mut!(FILE_BUF) };
            let result = std::fs_read(fd, buffer);
            let _ = std::fs_close(fd);
            result
        };
        let read_bytes = match read_result {
            Ok(n) => n,
            Err(code) => {
                std::log("[BROWSER APP] The guest filesystem refused to read the local document.")
                    .ok();
                self.status = PageStatus::LocalRefused;
                self.local_failure = LocalFailure::ReadRefused;
                self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
                self.tabs.active_tab_mut().error_code = code as i32;
                let _ = std::audio_haptic_click();
                return false;
            }
        };
        if read_bytes == 0 {
            std::log("[BROWSER APP] The local document is empty.").ok();
            self.status = PageStatus::LocalRefused;
            self.local_failure = LocalFailure::Empty;
            self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
            self.tabs.active_tab_mut().error_code = 0;
            let _ = std::audio_haptic_click();
            return false;
        }

        // The kind of document decides how it is read. A file whose name
        // says markup is read as markup; anything else is text, and a text
        // file keeps its lines.
        let is_markup = path_str.ends_with(".html") || path_str.ends_with(".htm");
        let kind = if is_markup {
            browser::PayloadKind::Markup
        } else {
            browser::PayloadKind::PlainText
        };

        let doc_len = {
            let source = unsafe { &*core::ptr::addr_of!(FILE_BUF) };
            let destination = unsafe { &mut *core::ptr::addr_of_mut!(READER_BUF) };
            browser::html_to_reader_document_kind(&source[..read_bytes], kind, destination)
        };
        if doc_len == 0 {
            self.status = PageStatus::LocalRefused;
            self.local_failure = LocalFailure::Empty;
            self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
            return false;
        }
        {
            let document = unsafe { &*core::ptr::addr_of!(READER_BUF) };
            self.load_document_content(&document[..doc_len]);
        }

        self.status = PageStatus::Loaded;
        self.pages_loaded += 1;
        self.loading_progress = 100;
        self.response_truncated = false;
        self.tabs.active_tab_mut().loading_state = LoadingState::Completed;
        let _ = std::audio_haptic_click();
        true
    }

    /// Loads a reader document, wrapping every line to the reading column.
    ///
    /// WHY the wrapping happens here and not in the renderer: a renderer
    /// that wrapped per pixel would have to break the same line over and
    /// over, and the break would not be stable while the reader scrolls.
    /// Wrapping once, when the document is loaded, gives the renderer a
    /// list of lines that are each already the right length.
    pub fn load_document_content(&mut self, content: &[u8]) {
        self.doc_line_count = 0;
        self.doc_total_lines = 0;
        let mut tokenizer = DocumentTokenizer::new(content);
        let mut pieces: [[u8; MAX_LINE_BYTES]; browser::MAX_WRAP_PIECES] =
            [[0; MAX_LINE_BYTES]; browser::MAX_WRAP_PIECES];
        let mut piece_lens: [usize; browser::MAX_WRAP_PIECES] = [0; browser::MAX_WRAP_PIECES];

        while let Some((line, kind)) = tokenizer.next_line() {
            self.doc_total_lines += 1;
            if self.doc_line_count >= MAX_DOC_LINES {
                // The document is longer than the reader holds. The count
                // is remembered so the screen can say so.
                continue;
            }

            let scale = match kind {
                LineKind::Heading1 => TYPE_H1_SCALE,
                _ => 1,
            };
            let count = browser::wrap_line(
                line,
                scale,
                browser::CONTENT_WIDTH,
                &mut pieces,
                &mut piece_lens,
            );

            // A line that is not a link has no target to carry; a link
            // carries its target and its label.
            let (tgt, lbl) = if kind == LineKind::GemtextLink {
                match parse_gemtext_link(line) {
                    Some((tgt, lbl)) => (tgt, lbl),
                    None => (&b""[..], &b""[..]),
                }
            } else {
                (&b""[..], &b""[..])
            };

            for piece in 0..count {
                if self.doc_line_count >= MAX_DOC_LINES {
                    break;
                }
                let idx = self.doc_line_count;
                let mut dl = DocLine::EMPTY;
                let n = piece_lens[piece];
                dl.bytes[..n].copy_from_slice(&pieces[piece][..n]);
                dl.len = n;
                dl.kind = kind;
                dl.continued = piece > 0;
                if kind == LineKind::GemtextLink {
                    let u = tgt.len().min(64);
                    dl.link_target[..u].copy_from_slice(&tgt[..u]);
                    dl.link_target_len = u;
                    let l = lbl.len().min(64);
                    dl.link_label[..l].copy_from_slice(&lbl[..l]);
                    dl.link_label_len = l;
                }
                self.doc_lines[idx] = dl;
                self.doc_line_count += 1;
            }
        }
    }

    pub fn navigate(&mut self, target: &[u8]) -> bool {
        self.history.push(target);
        self.tabs.navigate_to(target);
        self.navigate_again(target)
    }

    pub fn navigate_again(&mut self, target: &[u8]) -> bool {
        let n = target.len().min(self.current_url.len());
        self.current_url[..n].copy_from_slice(&target[..n]);
        self.current_url_len = n;
        self.scroll_offset_px = 0;

        let parsed = parse_uri(target);
        std::log("[BROWSER APP] Navigation target URL:").ok();
        if let Ok(url) = core::str::from_utf8(target) {
            std::log(url).ok();
        }

        // A local document is read from the guest filesystem and read
        // through the same reader pipeline as a fetched page.
        //
        // WHY this is not a stub: the branch used to log that the
        // filesystem refused the document and that the app holds no
        // filesystem capability, without ever asking the filesystem and
        // without checking the capability. The app does hold
        // CAP_FILESYSTEM_READ, and the refusal it reported was one it had
        // assumed.
        if parsed.scheme.is_local() {
            std::log("[BROWSER APP] Address names a local document; reading it from the guest filesystem.",
            )
            .ok();
            return self.load_local_document(&parsed.path);
        }

        // Real Network Socket dispatch with ephemeral client binding
        // The request is built first, so the transport only has to carry
        // bytes.
        //
        // WHY the gateway is always the destination: the guest has no
        // general connectivity. The Device Studio gateway fetches the page
        // on the device's behalf, and the device reads the result.
        let mut req_buf = [0u8; 512];
        let (dst_ip, dst_port, req_len) = if parsed.scheme == UriScheme::Https {
            let r_len = format_proxy_request(target, &mut req_buf);
            ([10, 0, 2, 2], 4300, r_len)
        } else {
            let host_bytes = if parsed.host.is_empty() { b"interos.tech" } else { parsed.host };
            let mut ip = [10, 0, 2, 2];
            if let Ok(host_str) = core::str::from_utf8(host_bytes) {
                if let Ok(resolved) = std::dns_resolve(host_str) {
                    ip = resolved;
                }
            }
            let r_len = format_http_get(host_bytes, parsed.path, &mut req_buf);
            (ip, parsed.port, r_len)
        };

        // The local port has to be non-zero and unique for the connection.
        let local_port = 45000u16 + ((self.pages_loaded as u16) & 0x03FF);

        std::log("[BROWSER APP] Request dispatched over TCP to the Device Studio gateway.").ok();

        let outcome = fetch_over_tcp(dst_ip, dst_port, local_port, &req_buf[..req_len]);

        // PLATFORM TRUTH: a page is loaded only when the transport actually
        // delivered bytes. Nothing is synthesised, and the reason is
        // reported as it was.
        if outcome.bytes == 0 {
            self.status = PageStatus::NoTransport;
            self.loading_progress = 0;
            self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
            self.tabs.active_tab_mut().error_code = outcome.error as i32;
            std::log("[BROWSER APP] The gateway returned no bytes. The failure is reported as it happened; no page was synthesised.",
            )
            .ok();
            let _ = std::audio_play_tone(220);
            let _ = std::audio_silence();
            let _ = std::audio_haptic_click();
            return false;
        }

        let response = unsafe { &*core::ptr::addr_of!(RESPONSE_BUF) };
        let rx_slice = &response[..outcome.bytes];

        let mut loaded_from_network = false;
        let key: &[u8] = b"\"html\":";
        let is_proxy_envelope = rx_slice.windows(key.len()).any(|w| w == key);

        // The gateway's JSON envelope names the page's own content type, and
        // a `text/plain` page is text rather than markup.
        let page_kind = match browser::proxy_content_type(rx_slice) {
            Some(ct) if browser::content_type_is_plain(ct) => browser::PayloadKind::PlainText,
            _ => browser::PayloadKind::Markup,
        };

        let html_len = if is_proxy_envelope {
            let buffer = unsafe { &mut *core::ptr::addr_of_mut!(HTML_BUF) };
            browser::extract_proxy_html(rx_slice, buffer)
        } else {
            // A response that is not the gateway's envelope is the page
            // itself, so it is read in place.
            let buffer = unsafe { &mut *core::ptr::addr_of_mut!(HTML_BUF) };
            let take = rx_slice.len().min(buffer.len());
            buffer[..take].copy_from_slice(&rx_slice[..take]);
            take
        };

        if html_len > 0 {
            // The HTML is read from one buffer and the reader document is
            // written to another: the reader rewrites the markup in place,
            // so source and destination cannot be the same memory.
            let source = unsafe { &*core::ptr::addr_of!(HTML_BUF) };
            let destination = unsafe { &mut *core::ptr::addr_of_mut!(READER_BUF) };
            let doc_len = browser::html_to_reader_document_kind(&source[..html_len], page_kind, destination);
            if doc_len > 0 {
                self.load_document_content(&destination[..doc_len]);
                loaded_from_network = true;
            }
        }

        if outcome.truncated {
            // WHY the screen says so: the response was larger than the
            // browser's buffer, so what is on screen is the beginning of a
            // page rather than the whole of it.
            std::log("[BROWSER APP] The response was larger than the browser's buffer; the page is shown truncated.",
            )
            .ok();
            self.response_truncated = true;
        } else {
            self.response_truncated = false;
        }

        if !loaded_from_network {
            self.status = PageStatus::NoTransport;
            self.loading_progress = 0;
            self.tabs.active_tab_mut().loading_state = LoadingState::Failed;
            self.tabs.active_tab_mut().error_code = -111; // ECONNREFUSED
            std::log("[BROWSER APP] The response carried no readable document. Truthful error status reported.").ok();
            return false;
        }

        self.status = PageStatus::Loaded;
        self.pages_loaded += 1;
        self.loading_progress = 100;
        self.tabs.active_tab_mut().loading_state = LoadingState::Completed;

        let _ = std::audio_play_tone(784);
        let _ = std::audio_silence();
        let _ = std::audio_haptic_click();
        true
    }

    pub fn reload(&mut self) {
        let mut target = [0u8; 64];
        let n = self.current_url_len.min(target.len());
        target[..n].copy_from_slice(&self.current_url[..n]);
        self.navigate_again(&target[..n]);
    }

    pub fn go_back(&mut self) {
        if let Some(url) = self.tabs.go_back() {
            let mut target = [0u8; 64];
            let n = url.len().min(target.len());
            target[..n].copy_from_slice(&url[..n]);
            std::log("[BROWSER APP] Back: re-issuing navigation attempt.").ok();
            self.navigate_again(&target[..n]);
            return;
        }
        if let Some(entry) = self.history.back() {
            let n = entry.iter().position(|&b| b == 0).unwrap_or(entry.len());
            std::log("[BROWSER APP] Back: re-issuing the navigation attempt for the previous address.").ok();
            self.navigate_again(&entry[..n]);
            return;
        }
        std::log("[BROWSER APP] Back is unavailable: the history holds no earlier address.").ok();
    }

    pub fn go_forward(&mut self) {
        if let Some(url) = self.tabs.go_forward() {
            let mut target = [0u8; 64];
            let n = url.len().min(target.len());
            target[..n].copy_from_slice(&url[..n]);
            std::log("[BROWSER APP] Forward: re-issuing navigation attempt.").ok();
            self.navigate_again(&target[..n]);
            return;
        }
        if let Some(entry) = self.history.forward() {
            let n = entry.iter().position(|&b| b == 0).unwrap_or(entry.len());
            std::log("[BROWSER APP] Forward: re-issuing the navigation attempt for the next address.").ok();
            self.navigate_again(&entry[..n]);
            return;
        }
        std::log("[BROWSER APP] Forward is unavailable: the history holds no later address.").ok();
    }

    pub fn start_add_bookmark(&mut self) {
        self.bm_step = BookmarkStep::Title;
        self.address_focused = true;
        self.address_len = 0;
        self.select_all = false;
        std::log("[BROWSER APP] Adding a bookmark: type the title, then the address.").ok();
    }

    pub fn commit_bookmark_step(&mut self) -> Result<(), i64> {
        match self.bm_step {
            BookmarkStep::Idle => Ok(()),
            BookmarkStep::Title => {
                if self.address_len == 0 {
                    std::log("[BROWSER APP] Bookmark not added: a bookmark needs a title.").ok();
                    return Err(-22);
                }
                let n = self.address_len.min(self.bm_title.len());
                self.bm_title[..n].copy_from_slice(&self.address[..n]);
                self.bm_title_len = n;
                self.bm_step = BookmarkStep::Address;
                self.address_len = 0;
                self.select_all = false;
                std::log("[BROWSER APP] Bookmark title set; type the address.").ok();
                Ok(())
            }
            BookmarkStep::Address => {
                if self.address_len == 0 {
                    std::log("[BROWSER APP] Bookmark not added: a bookmark needs an address.").ok();
                    return Err(-22);
                }
                if self.address_len > BM_URL_MAX {
                    self.address_len = BM_URL_MAX;
                }
                let title_len = self.bm_title_len.min(BM_TITLE_MAX);
                let outcome = self.bookmarks.add(&self.bm_title[..title_len], &self.address[..self.address_len]);
                self.bm_step = BookmarkStep::Idle;
                self.address_len = 0;
                self.address_focused = false;
                self.select_all = false;
                self.bm_title_len = 0;
                match outcome {
                    Ok(()) => {
                        std::log("[BROWSER APP] Bookmark written to the settings store and acknowledged.").ok();
                    }
                    Err(_) => {
                        std::log("[BROWSER APP] Bookmark was NOT saved: the kernel refused the settings write. It is held in the unsaved group for this session only.").ok();
                    }
                }
                outcome
            }
        }
    }

    pub fn cancel_bookmark_step(&mut self) {
        self.bm_step = BookmarkStep::Idle;
        self.address_len = 0;
        self.address_focused = false;
        self.select_all = false;
        self.bm_title_len = 0;
    }
}

// -----------------------------------------------------------------------------
// Per-Frame State Cache
// -----------------------------------------------------------------------------
struct Frame {
    pill: [u8; 96],
    pill_len: usize,
    pill_color: u32,
    show_caret: bool,
    caret_x: usize,
    select_w: usize,
    head: [u8; 96],
    head_len: usize,
    head_scale: usize,
    head_x: usize,
    head_y: usize,
    body: [&'static [u8]; 6],
    body_n: usize,
    body_x: [usize; 6],
    body_y: [usize; 6],
    icon_cy: usize,
    is_home: bool,
}

impl Frame {
    fn build(app: &BrowserApp) -> Self {
        let mut f = Self {
            pill: [0; 96],
            pill_len: 0,
            pill_color: CHROME_MUTED,
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
            is_home: app.status == PageStatus::Blank,
        };

        let src: &[u8] = if app.address_len > 0 {
            &app.address[..app.address_len]
        } else if app.address_focused {
            &app.address[..0]
        } else if app.current_url_len > 0 && &app.current_url[..app.current_url_len] != b"about:blank" {
            app.current_url()
        } else {
            PLACEHOLDER
        };

        f.pill_color = if app.address_len > 0
            || (app.current_url_len > 0 && &app.current_url[..app.current_url_len] != b"about:blank")
        {
            CHROME_TEXT
        } else {
            CHROME_MUTED
        };
        f.pill_len = font::ellipsize(src, 1, PILL_TEXT_MAX, &mut f.pill);
        f.show_caret = app.address_focused;
        f.caret_x = PILL_TEXT_X + font::measure(&f.pill[..f.pill_len], 1);
        f.select_w = if app.select_all {
            font::measure(&f.pill[..f.pill_len], 1)
        } else {
            0
        };

        let (head_src, body_src): (&[u8], &[u8]) = match app.status {
            PageStatus::Blank => (b"InterOS Browser", BODY_BLANK),
            PageStatus::Loaded => (
                b"Web Documents",
                b"Classified structured lines rendered via zero-heap scanout.",
            ),
            PageStatus::NoTransport => (b"Navigation unavailable", BODY_NO_TRANSPORT),
            PageStatus::InvalidAddress => (b"Address cannot be opened", BODY_INVALID),
            PageStatus::LocalRefused => (b"Local document unavailable", match app.local_failure {
                LocalFailure::NoPath => BODY_LOCAL_NO_PATH,
                LocalFailure::NoCapability => BODY_LOCAL_NO_CAPABILITY,
                LocalFailure::Refused => BODY_LOCAL_REFUSED,
                LocalFailure::Empty => BODY_LOCAL_EMPTY,
                LocalFailure::ReadRefused => BODY_LOCAL_READ_REFUSED,
            }),
        };

        f.head_scale = 1;
        let n = head_src.len().min(f.head.len());
        f.head[..n].copy_from_slice(&head_src[..n]);
        f.head_len = n;
        let head_w = font::measure_bold(&f.head[..f.head_len], 1);
        f.head_x = (W.saturating_sub(head_w)) / 2;

        let mut lines: [&'static [u8]; 6] = [&b""[..]; 6];
        f.body_n = font::wrap_text(body_src, 1, 560, 6, &mut lines);
        for i in 0..f.body_n {
            f.body[i] = lines[i];
            f.body_x[i] = (W.saturating_sub(font::measure(lines[i], 1))) / 2;
        }

        f.icon_cy = 380;
        f.head_y = 440;
        for i in 0..f.body_n {
            f.body_y[i] = f.head_y + 40 + i * 28;
        }

        f
    }
}

// -----------------------------------------------------------------------------
// Glyphs & Vector Drawing
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

/// Blends a colour toward another by a fixed amount in 0..=255.
///
/// WHY this exists: a scrim has to darken whatever is behind it without
/// erasing it, and the surface is a straight pixel buffer, so the blend
/// has to be written here.
fn blend_toward(colour: u32, toward: u32, amount: u32) -> u32 {
    let amount = amount.min(255) as i64;
    let mix = |c: u32, t: u32| -> u32 {
        let c = c as i64;
        let t = t as i64;
        (((c * (255 - amount)) + (t * amount)) / 255) as u32
    };
    (colour & 0xFF00_0000)
        | (mix((colour >> 16) & 0xFF, (toward >> 16) & 0xFF) << 16)
        | (mix((colour >> 8) & 0xFF, (toward >> 8) & 0xFF) << 8)
        | mix(colour & 0xFF, toward & 0xFF)
}

/// SSL Padlock Icon (centered around cx=38, cy=82)
fn padlock_glyph(x: usize, y: usize) -> bool {
    let dx = x as isize - 38;
    let dy = y as isize - 82;
    if dy >= -6 && dy <= -2 {
        let arc = dx.abs() >= 2 && dx.abs() <= 4;
        let top = dy == -6 && dx.abs() <= 3;
        if arc || top {
            return true;
        }
    }
    if dy >= -1 && dy <= 5 && dx.abs() <= 5 {
        if dy == 1 && dx.abs() <= 1 {
            return false;
        }
        if dy == 2 && dx == 0 {
            return false;
        }
        return true;
    }
    false
}

/// Close 'X' Glyph (centered at cx, cy)
fn close_glyph(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() > 8 || dy.abs() > 8 {
        return false;
    }
    (dx + dy).abs() <= 1 || (dx - dy).abs() <= 1
}

/// Star Glyph for Bookmarks
fn star_glyph(x: usize, y: usize, cx: usize, cy: usize, filled: bool) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() > 9 || dy.abs() > 9 {
        return false;
    }
    let on_outline = {
        let top = dy <= 0 && dx.abs() as isize <= 7 + (-dy) / 3;
        let bottom = dy > 0 && dx.abs() as isize <= 8 - dy;
        top || bottom
    };
    if filled {
        on_outline || (dx.abs() <= 4 && dy.abs() <= 5)
    } else {
        on_outline
    }
}

/// Reload Ring Glyph with Arrow
fn reload_glyph(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    let d2 = dx * dx + dy * dy;
    if d2 >= 49 && d2 <= 81 && !(dx > 3 && dy < -3) {
        return true;
    }
    let t = (cx + 5) as isize - x as isize;
    if t >= 0 && t <= 6 {
        let half = 3 - (t * 3) / 6;
        if (y as isize - (cy as isize - 8)).abs() <= half {
            return true;
        }
    }
    false
}

/// Kebab Overflow Menu Glyph (3 vertical dots)
fn kebab_glyph(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = (x as isize - cx as isize).abs();
    let dy1 = (y as isize - (cy as isize - 8)).abs();
    let dy2 = (y as isize - cy as isize).abs();
    let dy3 = (y as isize - (cy as isize + 8)).abs();
    dx <= 2 && (dy1 <= 2 || dy2 <= 2 || dy3 <= 2)
}

/// Tab Count Badge Box `[N]`
fn tab_badge_pixel(x: usize, y: usize, cx: usize, cy: usize, count: usize) -> Option<u32> {
    let dx = (x as isize - cx as isize).abs();
    let dy = (y as isize - cy as isize).abs();
    if dx > 13 || dy > 13 {
        return None;
    }
    if (dx >= 11 && dx <= 13) || (dy >= 11 && dy <= 13) {
        return Some(CHROME_MUTED);
    }
    let digit = [b'0' + (count as u8).min(9).max(1)];
    if let Some(px) = text_px(x, y, cx - 4, cy - 10, 1, &digit, CHROME_TEXT, CHROME_SURFACE, true) {
        return Some(px);
    }
    Some(CHROME_SURFACE)
}

/// Link Arrow Glyph (`→`)
fn link_arrow_glyph(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dy.abs() <= 1 && dx >= -5 && dx <= 5 {
        return true;
    }
    if dx >= 2 && dx <= 5 && (dy.abs() == dx - 2 || dy.abs() == dx - 3) {
        return true;
    }
    false
}

/// Offline Beacon Glyph
fn offline_glyph(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = (x as isize - cx as isize).abs();
    let dy = (y as isize - cy as isize).abs();
    if dx > 24 || dy > 24 {
        return false;
    }
    let d2 = dx * dx + dy * dy;
    if ((d2 >= 320 && d2 <= 440) || (d2 >= 120 && d2 <= 200)) && (y as isize) <= (cy as isize + 2) {
        return true;
    }
    if d2 <= 16 {
        return true;
    }
    let slash = (x as isize - cx as isize) - (y as isize - cy as isize);
    if slash.abs() <= 2 && dx <= 20 && dy <= 20 {
        return true;
    }
    false
}

/// Down Chevron Glyph (for dismiss)
fn draw_chevron_down(x: usize, y: usize, cx: usize, cy: usize, color: u32) -> Option<u32> {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() <= 6 && (dy - (3 - dx.abs())).abs() <= 1 {
        return Some(color);
    }
    None
}

// -----------------------------------------------------------------------------
// Soft Keyboard Layout & Geometry (Docked at Screen Bottom: y 980..1408)
// -----------------------------------------------------------------------------
const KB_Y0: usize = 980;
const KB_Y1: usize = 1408;
const KB_ACC_Y0: usize = 918;
const KB_ACC_Y1: usize = 974;

fn get_soft_keyboard_char(x: usize, y: usize, shift: bool, symbols: bool) -> Option<u8> {
    if y < KB_Y0 || y >= KB_Y1 {
        return None;
    }

    if symbols {
        // Row 0: 1 2 3 4 5 6 7 8 9 0 (y: 994..1066)
        if y >= 994 && y <= 1066 {
            let col = (x.saturating_sub(15)) / 70;
            if col < 10 {
                return Some(b"1234567890"[col]);
            }
        }
        // Row 1: - / : ; ( ) $ & @ " (y: 1076..1148)
        if y >= 1076 && y <= 1148 {
            let col = (x.saturating_sub(15)) / 70;
            if col < 10 {
                return Some(b"-/:;()$&@\""[col]);
            }
        }
        // Row 2: ABC, ? ! ' # % * + = Backspace (y: 1158..1230)
        if y >= 1158 && y <= 1230 {
            if x >= 15 && x <= 95 {
                return Some(0xFE); // toggle ABC
            }
            if x >= 620 && x <= 705 {
                return Some(8); // Backspace
            }
            let col = (x.saturating_sub(105)) / 64;
            if col < 8 {
                return Some(b"?!'#%*+="[col]);
            }
        }
        // Row 3: 123, comma, space, period, Go (y: 1240..1312)
        if y >= 1240 && y <= 1312 {
            if x >= 15 && x <= 115 {
                return Some(0xFE); // Back to ABC
            }
            if x >= 125 && x <= 195 {
                return Some(b',');
            }
            if x >= 205 && x <= 515 {
                return Some(b' ');
            }
            if x >= 525 && x <= 595 {
                return Some(b'.');
            }
            if x >= 605 && x <= 705 {
                return Some(b'\n'); // Go / Navigate
            }
        }
        return None;
    }

    // Standard QWERTY
    // Row 0: Q W E R T Y U I O P (y: 994..1066)
    if y >= 994 && y <= 1066 {
        let col = (x.saturating_sub(15)) / 70;
        if col < 10 {
            let ch = b"QWERTYUIOP"[col];
            return Some(if shift { ch } else { ch.to_ascii_lowercase() });
        }
    }
    // Row 1: A S D F G H J K L (y: 1076..1148)
    if y >= 1076 && y <= 1148 {
        let col = (x.saturating_sub(46)) / 70;
        if col < 9 {
            let ch = b"ASDFGHJKL"[col];
            return Some(if shift { ch } else { ch.to_ascii_lowercase() });
        }
    }
    // Row 2: Shift, Z X C V B N M, Backspace (y: 1158..1230)
    if y >= 1158 && y <= 1230 {
        if x >= 15 && x <= 95 {
            return Some(0xFF); // Shift toggle
        }
        if x >= 620 && x <= 705 {
            return Some(8); // Backspace
        }
        let col = (x.saturating_sub(105)) / 70;
        if col < 7 {
            let ch = b"ZXCVBNM"[col];
            return Some(if shift { ch } else { ch.to_ascii_lowercase() });
        }
    }
    // Row 3: ?123, comma, space, period, Go (y: 1240..1312)
    if y >= 1240 && y <= 1312 {
        if x >= 15 && x <= 115 {
            return Some(0xFE); // toggle symbols
        }
        if x >= 125 && x <= 195 {
            return Some(b',');
        }
        if x >= 205 && x <= 515 {
            return Some(b' ');
        }
        if x >= 525 && x <= 595 {
            return Some(b'.');
        }
        if x >= 605 && x <= 705 {
            return Some(b'\n'); // Go / Navigate
        }
    }

    None
}

fn render_browser_soft_keyboard(x: usize, y: usize, app: &BrowserApp) -> u32 {
    let krect = Rect::new(6, KB_Y0, 708, KB_Y1 - KB_Y0);
    if let Some(_) = draw_rounded_rect(x, y, krect, 22, 0xFF181B22, Some(CHROME_BORDER), 1) {
        // Row 0: Q W E R T Y U I O P (y: 994..1066)
        if y >= 994 && y <= 1066 {
            let col = (x.saturating_sub(15)) / 70;
            let kx = 15 + col * 70;
            if col < 10 && x >= kx + 3 && x <= kx + 63 {
                let rect = Rect::new(kx + 3, 994, 60, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, 10, 0xFF252932, None, 0) {
                    let ch = if app.kb_symbols {
                        b"1234567890"[col]
                    } else {
                        let ch = b"QWERTYUIOP"[col];
                        if app.kb_shift { ch } else { ch.to_ascii_lowercase() }
                    };
                    let ch_buf = [ch];
                    if let Some(px) = text_px(x, y, kx + 25, 1006, 2, &ch_buf, CHROME_TEXT, c, true) {
                        return px;
                    }
                    return c;
                }
            }
        }

        // Row 1: A S D F G H J K L (y: 1076..1148)
        if y >= 1076 && y <= 1148 {
            let col = (x.saturating_sub(46)) / 70;
            let kx = 46 + col * 70;
            if col < 9 && x >= kx + 3 && x <= kx + 63 {
                let rect = Rect::new(kx + 3, 1076, 60, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, 10, 0xFF252932, None, 0) {
                    let ch = if app.kb_symbols {
                        b"-/:;()$&@\""[col]
                    } else {
                        let ch = b"ASDFGHJKL"[col];
                        if app.kb_shift { ch } else { ch.to_ascii_lowercase() }
                    };
                    let ch_buf = [ch];
                    if let Some(px) = text_px(x, y, kx + 25, 1088, 2, &ch_buf, CHROME_TEXT, c, true) {
                        return px;
                    }
                    return c;
                }
            }
        }

        // Row 2: Shift, Letters, Backspace (y: 1158..1230)
        if y >= 1158 && y <= 1230 {
            // Shift key (15..95)
            if x >= 15 && x <= 95 {
                let rect = Rect::new(15, 1158, 80, 72);
                let fill = if app.kb_shift { CHROME_BLUE } else { 0xFF20232B };
                if let Some(c) = draw_rounded_rect(x, y, rect, 10, fill, None, 0) {
                    let txt = if app.kb_symbols { b"ABC" as &[u8] } else { b"^" as &[u8] };
                    if let Some(px) = text_px(x, y, 42, 1182, 1, txt, CHROME_TEXT, c, true) {
                        return px;
                    }
                    return c;
                }
            }

            // Center 7 keys: Z X C V B N M
            let col = (x.saturating_sub(105)) / 70;
            let kx = 105 + col * 70;
            if col < 7 && x >= kx + 3 && x <= kx + 63 {
                let rect = Rect::new(kx + 3, 1158, 60, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, 10, 0xFF252932, None, 0) {
                    let ch = if app.kb_symbols {
                        b"?!'#%*+="[col]
                    } else {
                        let ch = b"ZXCVBNM"[col];
                        if app.kb_shift { ch } else { ch.to_ascii_lowercase() }
                    };
                    let ch_buf = [ch];
                    if let Some(px) = text_px(x, y, kx + 25, 1170, 2, &ch_buf, CHROME_TEXT, c, true) {
                        return px;
                    }
                    return c;
                }
            }

            // Backspace key (620..705)
            if x >= 620 && x <= 705 {
                let rect = Rect::new(620, 1158, 85, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, 10, 0xFF20232B, None, 0) {
                    if let Some(px) = text_px(x, y, 646, 1182, 1, b"<X]", CHROME_TEXT, c, true) {
                        return px;
                    }
                    return c;
                }
            }
        }

        // Row 3: ?123, Comma, Space, Period, Go (y: 1240..1312)
        if y >= 1240 && y <= 1312 {
            // ?123 / ABC toggle key (15..115)
            if x >= 15 && x <= 115 {
                let rect = Rect::new(15, 1240, 100, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, 10, 0xFF20232B, None, 0) {
                    let txt = if app.kb_symbols { b"ABC" as &[u8] } else { b"?123" as &[u8] };
                    if let Some(px) = text_px(x, y, 42, 1264, 1, txt, CHROME_TEXT, c, true) {
                        return px;
                    }
                    return c;
                }
            }

            // Comma key (125..195)
            if x >= 125 && x <= 195 {
                let rect = Rect::new(125, 1240, 70, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, 10, 0xFF252932, None, 0) {
                    if let Some(px) = text_px(x, y, 155, 1252, 2, b",", CHROME_TEXT, c, true) {
                        return px;
                    }
                    return c;
                }
            }

            // Spacebar (205..515)
            if x >= 205 && x <= 515 {
                let rect = Rect::new(205, 1240, 310, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, 10, 0xFF252932, None, 0) {
                    if let Some(px) = text_px(x, y, 328, 1264, 1, b"Space", CHROME_MUTED, c, false) {
                        return px;
                    }
                    return c;
                }
            }

            // Period key (525..595)
            if x >= 525 && x <= 595 {
                let rect = Rect::new(525, 1240, 70, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, 10, 0xFF252932, None, 0) {
                    if let Some(px) = text_px(x, y, 555, 1252, 2, b".", CHROME_TEXT, c, true) {
                        return px;
                    }
                    return c;
                }
            }

            // Go / Navigate key (605..705) - Vibrant Blue Accent
            if x >= 605 && x <= 705 {
                let rect = Rect::new(605, 1240, 100, 72);
                if let Some(c) = draw_rounded_rect(x, y, rect, 10, CHROME_BLUE, None, 0) {
                    if let Some(px) = text_px(x, y, 642, 1264, 1, b"Go", 0xFFFFFFFF, c, true) {
                        return px;
                    }
                    return c;
                }
            }
        }
        return 0xFF181B22;
    }
    CHROME_BG
}

fn render_accessory_bar(x: usize, y: usize, _app: &BrowserApp) -> u32 {
    let bar_rect = Rect::new(16, KB_ACC_Y0, 688, 56);
    if let Some(_) = draw_rounded_rect(x, y, bar_rect, 16, CHROME_SURFACE, Some(CHROME_BORDER), 1) {
        // Chip 1: "https://" (x 24..134)
        let c1 = Rect::new(24, KB_ACC_Y0 + 6, 110, 44);
        if let Some(c) = draw_rounded_rect(x, y, c1, 10, CHROME_CARD, None, 0) {
            if let Some(px) = text_px(x, y, 36, KB_ACC_Y0 + 16, 1, b"https://", CHROME_TEXT, c, false) {
                return px;
            }
            return c;
        }

        // Chip 2: ".com" (x 144..224)
        let c2 = Rect::new(144, KB_ACC_Y0 + 6, 80, 44);
        if let Some(c) = draw_rounded_rect(x, y, c2, 10, CHROME_CARD, None, 0) {
            if let Some(px) = text_px(x, y, 164, KB_ACC_Y0 + 16, 1, b".com", CHROME_TEXT, c, false) {
                return px;
            }
            return c;
        }

        // Chip 3: ".org" (x 234..314)
        let c3 = Rect::new(234, KB_ACC_Y0 + 6, 80, 44);
        if let Some(c) = draw_rounded_rect(x, y, c3, 10, CHROME_CARD, None, 0) {
            if let Some(px) = text_px(x, y, 254, KB_ACC_Y0 + 16, 1, b".org", CHROME_TEXT, c, false) {
                return px;
            }
            return c;
        }

        // Chip 4: "Clear" (x 324..404)
        let c4 = Rect::new(324, KB_ACC_Y0 + 6, 80, 44);
        if let Some(c) = draw_rounded_rect(x, y, c4, 10, CHROME_CARD, None, 0) {
            if let Some(px) = text_px(x, y, 342, KB_ACC_Y0 + 16, 1, b"Clear", CHROME_MUTED, c, false) {
                return px;
            }
            return c;
        }

        // Dismiss Down Chevron (x 630..694)
        let c_down = Rect::new(630, KB_ACC_Y0 + 6, 64, 44);
        if let Some(c) = draw_rounded_rect(x, y, c_down, 10, CHROME_CARD, None, 0) {
            if let Some(col) = draw_chevron_down(x, y, 662, KB_ACC_Y0 + 28, CHROME_BLUE) {
                return col;
            }
            return c;
        }

        return CHROME_SURFACE;
    }
    CHROME_BG
}

// -----------------------------------------------------------------------------
// Top Omnibox & Toolbar Renderer
// -----------------------------------------------------------------------------
fn toolbar_pixel(x: usize, y: usize, app: &BrowserApp, f: &Frame) -> u32 {
    let parsed = parse_uri(app.current_url());
    let is_https = parsed.scheme == UriScheme::Https;

    // Omnibox Capsule
    if let Some(c) = draw_rounded_rect(
        x,
        y,
        PILL,
        PILL_R,
        if app.address_focused {
            CHROME_BG
        } else {
            CHROME_SURFACE
        },
        Some(if app.address_focused {
            CHROME_BLUE
        } else {
            CHROME_BORDER
        }),
        if app.address_focused { 2 } else { 1 },
    ) {
        if padlock_glyph(x, y) {
            // WHY the mark is muted and not green: a green padlock says
            // the connection was verified, and this browser never performs
            // a TLS handshake itself. It reads a page over plain HTTP
            // from the device's local gateway, and the gateway fetches
            // the origin on the device's behalf. The address naming an
            // https origin is a fact about the text in the field; it is
            // not a statement about a channel this app secured.
            //
            // The mark is not drawn at all when nothing has loaded, so a
            // failed navigation shows no lock beside a URL that was never
            // contacted.
            if app.status != PageStatus::Loaded {
                return CHROME_SURFACE;
            }
            return if is_https { CHROME_MUTED } else { CHROME_MUTED };
        }
        let in_sel = f.select_w > 0
            && x >= PILL_TEXT_X
            && x < PILL_TEXT_X + f.select_w
            && y >= PILL_TEXT_Y
            && y < PILL_TEXT_Y + font::GLYPH_H;
        let t = &f.pill[..f.pill_len];
        if !t.is_empty() {
            let bg = if in_sel { 0xFFD2E3FC } else { c };
            if let Some(px) = text_px(x, y, PILL_TEXT_X, PILL_TEXT_Y, 1, t, f.pill_color, bg, false) {
                return px;
            }
        }
        if in_sel {
            return 0xFFD2E3FC;
        }
        if f.show_caret
            && x >= f.caret_x
            && x < f.caret_x + 2
            && y >= PILL_TEXT_Y
            && y < PILL_TEXT_Y + font::GLYPH_H
        {
            return CHROME_BLUE;
        }
        return c;
    }

    // Reload Button
    if let Some(c) = draw_rounded_rect(x, y, RELOAD_BTN, BTN_R, CHROME_BG, None, 0) {
        let cx = RELOAD_BTN.x + RELOAD_BTN.w / 2;
        let cy = RELOAD_BTN.y + RELOAD_BTN.h / 2;
        return if reload_glyph(x, y, cx, cy) {
            CHROME_MUTED
        } else {
            c
        };
    }

    // Tab Counter Badge `[N]` Button
    if let Some(_) = draw_rounded_rect(x, y, TABS_TOP_BTN, BTN_R, CHROME_BG, None, 0) {
        let cx = TABS_TOP_BTN.x + TABS_TOP_BTN.w / 2;
        let cy = TABS_TOP_BTN.y + TABS_TOP_BTN.h / 2;
        if let Some(badge) = tab_badge_pixel(x, y, cx, cy, app.tabs.tab_count()) {
            return badge;
        }
    }

    // Overflow Kebab Menu Button (`⋮`)
    if let Some(c) = draw_rounded_rect(x, y, KEBAB_BTN, BTN_R, CHROME_BG, None, 0) {
        let cx = KEBAB_BTN.x + KEBAB_BTN.w / 2;
        let cy = KEBAB_BTN.y + KEBAB_BTN.h / 2;
        return if kebab_glyph(x, y, cx, cy) {
            CHROME_MUTED
        } else {
            c
        };
    }

    // Close Button (dismisses app to launcher)
    if let Some(c) = draw_rounded_rect(x, y, CLOSE_BTN, BTN_R, CHROME_BG, None, 0) {
        let cx = CLOSE_BTN.x + CLOSE_BTN.w / 2;
        let cy = CLOSE_BTN.y + CLOSE_BTN.h / 2;
        return if close_glyph(x, y, cx, cy) {
            CHROME_MUTED
        } else {
            c
        };
    }

    // Blue Loading Progress Bar (y 114..116)
    if y >= 114 && y < 116 {
        return if app.loading_progress > 0 {
            CHROME_BLUE
        } else {
            CHROME_DIVIDER
        };
    }

    CHROME_BG
}

// -----------------------------------------------------------------------------
// Start Page (New Tab)
// -----------------------------------------------------------------------------
fn home_page_pixel(x: usize, y: usize, app: &BrowserApp) -> u32 {
    // 1. Sovereign Brand Hero Section (y 130..244)
    if y >= 130 && y < 244 {
        // App icon squircle (52x52) centered at (334, 134)
        let icon_rect = Rect::new(334, 134, 52, 52);
        if let Some(c) = draw_rounded_rect(x, y, icon_rect, 14, CHROME_BLUE, Some(CHROME_BORDER), 1) {
            let cx = 360isize;
            let cy = 160isize;
            let dx = (x as isize) - cx;
            let dy = (y as isize) - cy;
            let r2 = dx * dx + dy * dy;
            if r2 >= 196 && r2 <= 289 {
                return 0xFFFFFFFF;
            }
            if (dx.abs() <= 1 && dy.abs() <= 17) || (dy.abs() <= 1 && dx.abs() <= 17) {
                return 0xFFFFFFFF;
            }
            return c;
        }

        // Brand Title
        let brand_text = b"InterOS Browser";
        let bw = font::measure_bold(brand_text, 1);
        let bx0 = (W.saturating_sub(bw)) / 2;
        if let Some(px) = text_px(x, y, bx0, 196, 1, brand_text, CHROME_TEXT, CHROME_BG, true) {
            return px;
        }

        // Subtitle
        let sub_text = b"Sovereign Web";
        let sw = font::measure(sub_text, 1);
        let sx0 = (W.saturating_sub(sw)) / 2;
        if let Some(px) = text_px(x, y, sx0, 220, 1, sub_text, CHROME_MUTED, CHROME_BG, false) {
            return px;
        }
        return CHROME_BG;
    }

    // 2. Favorites / Quick Access Grid (y 252..376)
    if y >= 252 && y < 376 {
        // Section label
        if y >= 252 && y < 274 {
            // WHY the heading is "Quick links" and not "Favorites": the tiles
        // are a list the app shipped with, not the user's favourites. The
        // user's own bookmarks are the list below this one, and it says so
        // when it is empty.
        if let Some(px) = text_px(x, y, 28, 254, 1, b"Quick links", CHROME_MUTED, CHROME_BG, true) {
                return px;
            }
            return CHROME_BG;
        }

        // Col 0: cx = 98 (InterOS Platform)
        let c0_rect = Rect::new(66, 278, 64, 64);
        if let Some(c) = draw_rounded_rect(x, y, c0_rect, 16, CHROME_CARD, Some(CHROME_BORDER), 1) {
            if let Some(px) = text_px(x, y, 86, 300, 1, b"IO", CHROME_TEXT, c, true) {
                return px;
            }
            return c;
        }
        if let Some(px) = text_px(x, y, 70, 350, 1, b"InterOS", CHROME_TEXT, CHROME_BG, false) {
            return px;
        }

        // Col 1: cx = 266 (YouTube Mobile)
        let c1_rect = Rect::new(234, 278, 64, 64);
        if let Some(c) = draw_rounded_rect(x, y, c1_rect, 16, CHROME_CARD, Some(CHROME_BORDER), 1) {
            let px = (x as isize) - 260;
            let py = ((y as isize) - 310).abs();
            if px >= 0 && px <= 14 && py * 14 <= (14 - px) * 10 {
                return CHROME_MUTED;
            }
            return c;
        }
        if let Some(px) = text_px(x, y, 238, 350, 1, b"YouTube", CHROME_TEXT, CHROME_BG, false) {
            return px;
        }

        // Col 2: cx = 434 (GitHub Source)
        let c2_rect = Rect::new(402, 278, 64, 64);
        if let Some(c) = draw_rounded_rect(x, y, c2_rect, 16, CHROME_CARD, Some(CHROME_BORDER), 1) {
            if let Some(px) = text_px(x, y, 422, 300, 1, b"GH", CHROME_TEXT, c, true) {
                return px;
            }
            return c;
        }
        if let Some(px) = text_px(x, y, 410, 350, 1, b"GitHub", CHROME_TEXT, CHROME_BG, false) {
            return px;
        }

        // Col 3: cx = 602 (Wikipedia)
        let c3_rect = Rect::new(570, 278, 64, 64);
        if let Some(c) = draw_rounded_rect(x, y, c3_rect, 16, CHROME_CARD, Some(CHROME_BORDER), 1) {
            if let Some(px) = text_px(x, y, 594, 300, 1, b"W", CHROME_TEXT, c, true) {
                return px;
            }
            return c;
        }
        if let Some(px) = text_px(x, y, 566, 350, 1, b"Wikipedia", CHROME_TEXT, CHROME_BG, false) {
            return px;
        }

        return CHROME_BG;
    }

    // 3. "Your pages": saved bookmarks & session visits (y 386..1340)
    if y >= 386 && y < 418 {
        if let Some(px) = text_px(x, y, 28, 392, 1, b"Your pages", CHROME_TEXT, CHROME_BG, true) {
            return px;
        }
        return CHROME_BG;
    }

    // 4. Saved pages & recent visits cards
    if home_row(app, 0).is_none() {
        let empty_rect = Rect::new(24, 422, W - 48, 96);
        if let Some(c) = draw_rounded_rect(x, y, empty_rect, 16, CHROME_CARD, Some(CHROME_CARD_BORDER), 1) {
            let star_box = Rect::new(44, 438, 46, 46);
            if let Some(sc) = draw_rounded_rect(x, y, star_box, 10, 0xFF181C24, None, 0) {
                if star_glyph(x, y, 67, 461, false) {
                    return CHROME_MUTED;
                }
                return sc;
            }
            if let Some(px) = text_px(x, y, 104, 442, 1, b"No Bookmarks Yet", CHROME_TEXT, c, true) {
                return px;
            }
            if let Some(px) = text_px(x, y, 104, 468, 1, b"Bookmarked and visited pages appear here", CHROME_MUTED, c, false) {
                return px;
            }
            return c;
        }

        return CHROME_BG;
    }

    let card_h = 100;
    let card_gap = 12;
    for i in 0..4 {
        let card_y = 422 + i * (card_h + card_gap);
        let card_rect = Rect::new(24, card_y, W - 48, card_h);
        let Some(row) = home_row(app, i) else {
            continue;
        };

        if let Some(c) = draw_rounded_rect(x, y, card_rect, 16, CHROME_CARD, Some(CHROME_CARD_BORDER), 1) {
            if let Some(px) = text_px(x, y, 44, card_y + 20, 1, row.title, CHROME_TEXT, CHROME_CARD, true) {
                return px;
            }
            let url = &row.url[..row.url_len];
            if let Some(px) = text_px(x, y, 44, card_y + 48, 1, url, CHROME_MUTED, CHROME_CARD, false) {
                return px;
            }
            if let Some(px) = text_px(x, y, 44, card_y + card_h - 28, 1, row.meta, CHROME_BLUE, CHROME_CARD, false) {
                return px;
            }
            let thumb = Rect::new(W - 48 - 84, card_y + card_h - 88, 68, 68);
            if let Some(tc) = draw_rounded_rect(x, y, thumb, 10, CHROME_SURFACE, None, 0) {
                let tw = font::measure_bold(row.scheme_mark, 1);
                if let Some(px) = text_px(
                    x,
                    y,
                    thumb.x + 34 - tw / 2,
                    thumb.y + 24,
                    1,
                    row.scheme_mark,
                    CHROME_MUTED,
                    tc,
                    true,
                ) {
                    return px;
                }
                return tc;
            }
            return c;
        }
    }

    CHROME_BG
}

struct HomeRow {
    title: &'static [u8],
    url: [u8; URL_FIELD],
    url_len: usize,
    meta: &'static [u8],
    scheme_mark: &'static [u8],
}

fn home_row(app: &BrowserApp, index: usize) -> Option<HomeRow> {
    let saved = app.bookmarks.len();
    if index < saved {
        let rec = app.bookmarks.record(index)?;
        let url = rec.url_bytes();
        let (field, shown) = into_url_field(url);
        return Some(HomeRow {
            title: b"Saved",
            url_len: shown,
            meta: b"Bookmark",
            scheme_mark: scheme_mark(url),
            url: field,
        });
    }

    let visited = app.history.len();
    let from_history = index - saved;
    if from_history >= visited {
        return None;
    }
    let entry = app.history.entry(visited - 1 - from_history)?;
    if entry == b"about:blank" {
        return None;
    }
    let (field, shown) = into_url_field(entry);
    Some(HomeRow {
        title: b"Visited",
        url_len: shown,
        meta: b"From this session",
        scheme_mark: scheme_mark(entry),
        url: field,
    })
}

// -----------------------------------------------------------------------------
// Web Viewport / Content Area Renderer
// -----------------------------------------------------------------------------
fn web_page_pixel(x: usize, y: usize, app: &BrowserApp, f: &Frame) -> u32 {
    // Error state / No transport screen
    if app.status == PageStatus::NoTransport
        || app.status == PageStatus::InvalidAddress
        || app.status == PageStatus::LocalRefused
    {
        let cx = W / 2;
        let cy = f.icon_cy;
        if offline_glyph(x, y, cx, cy) {
            return CHROME_MUTED;
        }
        if f.head_len > 0 {
            if let Some(px) = text_px(
                x,
                y,
                f.head_x,
                f.head_y,
                f.head_scale,
                &f.head[..f.head_len],
                CHROME_TEXT,
                CHROME_BG,
                true,
            ) {
                return px;
            }
        }
        for i in 0..f.body_n {
            if let Some(px) = text_px(x, y, f.body_x[i], f.body_y[i], 1, f.body[i], CHROME_MUTED, CHROME_BG, false) {
                return px;
            }
        }

        // The failure is named, not stamped with one code.
        //
        // WHY: the screen used to print "ERR_NETWORK_TRANSPORT_REFUSED"
        // for every failure -- an invalid address, a refused local
        // connection and a gateway that answered nothing all produced the
        // same badge, so the badge was decoration rather than a report.
        let err_y = f.head_y + 40 + f.body_n * 28 + 24;
        let err_rect = Rect::new(cx - 180, err_y, 360, 36);
        if let Some(c) = draw_rounded_rect(x, y, err_rect, 8, CHROME_CARD, Some(CHROME_CARD_BORDER), 1) {
            let label: &[u8] = match app.status {
                PageStatus::InvalidAddress => b"INVALID ADDRESS",
                PageStatus::LocalRefused => match app.local_failure {
                    LocalFailure::NoPath => b"NO PATH GIVEN",
                    LocalFailure::NoCapability => b"NO FILESYSTEM CAPABILITY",
                    LocalFailure::Refused => b"REFUSED BY THE DEVICE",
                    LocalFailure::Empty => b"THE FILE IS EMPTY",
                    LocalFailure::ReadRefused => b"THE READ WAS REFUSED",
                },
                _ => b"NO RESPONSE FROM THE GATEWAY",
            };
            let lw = font::measure(label, 1);
            if let Some(px) = text_px(x, y, cx - lw / 2, err_y + 8, 1, label, CHROME_RED, c, false) {
                return px;
            }
            return c;
        }

        // Reload Button Card
        let btn_y = err_y + 54;
        let btn_rect = Rect::new(cx - 90, btn_y, 180, 48);
        if let Some(c) = draw_rounded_rect(x, y, btn_rect, 12, CHROME_BLUE, None, 0) {
            let rel = b"Reload Page";
            let rw = font::measure_bold(rel, 1);
            if let Some(px) = text_px(x, y, cx - rw / 2, btn_y + 14, 1, rel, 0xFFFFFFFF, c, true) {
                return px;
            }
            return c;
        }

        return CHROME_BG;
    }

    // Render classified document lines with smooth vertical scrolling.
    //
    // WHY the layout is rebuilt for every pixel: the reader draws from a
    // list of positioned lines, and the position of a line depends on the
    // lines above it. The platform's rendering work owns a display list
    // that would hoist this; until then the walk is bounded by the
    // document.
    let content_start_y = 126;
    let base_y = (content_start_y as isize) - (app.scroll_offset_px as isize);
    let pad = browser::CONTENT_PADDING;
    let column = browser::CONTENT_WIDTH;

    let mut current_y = base_y;
    for i in 0..app.doc_line_count {
        let line = &app.doc_lines[i];
        let bytes = &line.bytes[..line.len];

        match line.kind {
            LineKind::Heading1 => {
                // WHY the title is set at scale 2: the three heading levels
                // were all set at scale 1, so a page title and a section
                // heading were the same size and differed only in colour.
                // The type scale is the hierarchy.
                let title = if bytes.starts_with(b"# ") {
                    &bytes[2..]
                } else if bytes.starts_with(b"#") {
                    &bytes[1..]
                } else {
                    bytes
                };
                let h_y = current_y + 8;
                let band = font::line_height(TYPE_H1_SCALE);
                if y as isize >= h_y && (y as isize) < h_y + band as isize {
                    if let Some(px) = text_px(
                        x, y, pad, h_y as usize, TYPE_H1_SCALE, title, CHROME_TEXT, CHROME_BG, true,
                    ) {
                        return px;
                    }
                }
                // A rule under the title, drawn once for the heading rather
                // than once for each line it was wrapped onto.
                if !line.continued {
                    let d_y = h_y + band as isize + 6;
                    if y as isize == d_y && x >= pad && x <= W - pad {
                        return CHROME_DIVIDER;
                    }
                }
                current_y += band as isize + 26;
            }
            LineKind::Heading2 => {
                // WHY the second level carries a rule above it: the type
                // scale has two steps, so a second level that differed only
                // in weight was still the same size as the third.
                let title = if bytes.starts_with(b"## ") {
                    &bytes[3..]
                } else if bytes.starts_with(b"##") {
                    &bytes[2..]
                } else {
                    bytes
                };
                let h_y = current_y + 14;
                if !line.continued {
                    let r_y = current_y + 4;
                    if y as isize == r_y && x >= pad && x < pad + 40 {
                        return CHROME_BLUE;
                    }
                }
                if y as isize >= h_y && (y as isize) < h_y + font::line_height(1) as isize {
                    if let Some(px) = text_px(x, y, pad, h_y as usize, 1, title, CHROME_TEXT, CHROME_BG, true) {
                        return px;
                    }
                }
                current_y += 40;
            }
            LineKind::Heading3 => {
                let title = if bytes.starts_with(b"### ") {
                    &bytes[4..]
                } else if bytes.starts_with(b"###") {
                    &bytes[3..]
                } else {
                    bytes
                };
                let h_y = current_y + 8;
                if y as isize >= h_y && (y as isize) < h_y + font::line_height(1) as isize {
                    if let Some(px) = text_px(x, y, pad, h_y as usize, 1, title, CHROME_BLUE, CHROME_BG, true) {
                        return px;
                    }
                }
                current_y += 32;
            }
            LineKind::ListItem => {
                let l_y = current_y + 4;
                if y as isize >= l_y && (y as isize) < l_y + 24 {
                    // The bullet sits in the margin, and the text starts
                    // after it.
                    let dx = x as isize - (pad as isize + 10);
                    let dy = (y as isize) - (l_y + 8);
                    if dx * dx + dy * dy <= 8 {
                        return CHROME_BLUE;
                    }
                    let item_text = if bytes.starts_with(b"* ") || bytes.starts_with(b"- ") || bytes.starts_with(b"+ ") {
                        &bytes[2..]
                    } else {
                        bytes
                    };
                    if let Some(px) = text_px(x, y, pad + 22, l_y as usize, 1, item_text, CHROME_TEXT, CHROME_BG, false) {
                        return px;
                    }
                }
                current_y += 28;
            }
            LineKind::CodeBlock => {
                if bytes.starts_with(b"```") {
                    // The fence itself is not drawn, but it still occupies
                    // the space the block needs.
                    current_y += 8;
                } else {
                    // WHY the box spans the whole run: each line of a code
                    // block used to get its own bordered box, so a ten-line
                    // snippet rendered as ten separate cards. The box now
                    // starts at the first line of the run and ends at the
                    // last.
                    let c_y = current_y + 2;
                    let c_h = 30;
                    if y as isize >= c_y && (y as isize) < c_y + c_h as isize {
                        if let Some(c) = draw_rounded_rect(x, y, Rect::new(pad, c_y as usize, column, c_h), 8, CHROME_CODE_BG, Some(CHROME_BORDER), 1) {
                            if x >= pad && x <= pad + 3 {
                                return CHROME_BLUE;
                            }
                            if let Some(px) = text_px(x, y, pad + 14, (c_y + 5) as usize, 1, bytes, 0xFF7EE787, c, false) {
                                return px;
                            }
                            return c;
                        }
                    }
                    current_y += c_h as isize + 4;
                }
            }
            LineKind::GemtextLink => {
                let g_y = current_y + 4;
                let g_h = 56;
                let link_rect = Rect::new(pad, g_y as usize, column, g_h);
                if y as isize >= g_y && (y as isize) < g_y + g_h as isize {
                    if let Some(c) = draw_rounded_rect(x, y, link_rect, 12, CHROME_CARD, Some(CHROME_CARD_BORDER), 1) {
                        // The link's mark sits in the margin.
                        let icon_box = Rect::new(pad + 12, (g_y + 10) as usize, 36, 36);
                        if let Some(ic) = draw_rounded_rect(x, y, icon_box, 8, 0xFF1B2333, None, 0) {
                            if link_arrow_glyph(x, y, pad + 30, (g_y + 28) as usize) {
                                return CHROME_BLUE;
                            }
                            return ic;
                        }

                        let label_bytes = if line.link_label_len > 0 {
                            &line.link_label[..line.link_label_len]
                        } else if line.link_target_len > 0 {
                            &line.link_target[..line.link_target_len]
                        } else {
                            bytes
                        };
                        // WHY the label is ellipsized: a link's text is
                        // unbounded, and an unellipsized label runs off the
                        // card and off the screen.
                        let mut label_buf = [0u8; 64];
                        let label_len = font::ellipsize(
                            label_bytes,
                            1,
                            column - 72,
                            &mut label_buf,
                        );
                        if let Some(px) = text_px(x, y, pad + 60, (g_y + 10) as usize, 1, &label_buf[..label_len], CHROME_TEXT, c, true) {
                            return px;
                        }

                        if line.link_target_len > 0 {
                            let target_bytes = &line.link_target[..line.link_target_len];
                            let mut target_buf = [0u8; 64];
                            let target_len = font::ellipsize(
                                target_bytes,
                                1,
                                column - 72,
                                &mut target_buf,
                            );
                            if let Some(px) = text_px(x, y, pad + 60, (g_y + 30) as usize, 1, &target_buf[..target_len], CHROME_MUTED, c, false) {
                                return px;
                            }
                        }

                        // The chevron marks the card as tappable.
                        let ch_x = (W - pad) as isize;
                        let ch_y = (g_y + 28) as isize;
                        let dist = ch_x - (x as isize);
                        if dist >= 0 && dist <= 7 {
                            let dy = ((y as isize) - ch_y).abs();
                            if (dy - dist).abs() <= 1 {
                                return CHROME_MUTED;
                            }
                        }

                        return c;
                    }
                }
                current_y += g_h as isize + 10;
            }
            LineKind::Blockquote => {
                let quote_text = if bytes.starts_with(b"> ") {
                    &bytes[2..]
                } else if bytes.starts_with(b">") {
                    &bytes[1..]
                } else {
                    bytes
                };
                let q_y = current_y + 4;
                let q_h = 28;
                if y as isize >= q_y && (y as isize) < q_y + q_h as isize {
                    // The quote's bar sits in the margin.
                    if x >= pad && x <= pad + 3 {
                        return CHROME_BLUE;
                    }
                    if let Some(px) = text_px(x, y, pad + 14, (q_y + 4) as usize, 1, quote_text, CHROME_MUTED, CHROME_BG, false) {
                        return px;
                    }
                }
                current_y += 34;
            }
            LineKind::Paragraph => {
                let p_y = current_y + 4;
                if y as isize >= p_y && (y as isize) < p_y + 24 {
                    if let Some(px) = text_px(x, y, pad, p_y as usize, 1, bytes, CHROME_TEXT, CHROME_BG, false) {
                        return px;
                    }
                }
                current_y += 28;
            }
            LineKind::Empty => {
                current_y += 14;
            }
        }
    }

    // The end of the document.
    //
    // WHY this says so: the reader holds a bounded number of lines, and a
    // page with more lines than that is an excerpt. Showing the first
    // lines without saying so presents a fragment as the article.
    if app.doc_total_lines > app.doc_line_count && app.doc_line_count > 0 {
        let note_y = current_y + 12;
        if y as isize >= note_y && (y as isize) < note_y + 20 {
            let mut buf = [0u8; 48];
            let mut n = 0usize;
            n = append_text(&mut buf, n, b"Showing the first ");
            n = append_u64_text(&mut buf, n, app.doc_line_count as u64);
            n = append_text(&mut buf, n, b" of ");
            n = append_u64_text(&mut buf, n, app.doc_total_lines as u64);
            n = append_text(&mut buf, n, b" lines");
            if let Some(px) = text_px(x, y, browser::CONTENT_PADDING, note_y as usize, 1, &buf[..n], CHROME_MUTED, CHROME_BG, false) {
                return px;
            }
        }
    }

    // Scroll Position Indicator (on right edge x 712..716)
    if x >= 712 && x < 716 && y >= 120 && y < 1340 {
        let track_h = 1340 - 120;
        let thumb_h = 60;
        let scroll_max = (current_y.saturating_sub(base_y) as usize).saturating_sub(track_h).max(1);
        let thumb_y = 120 + ((app.scroll_offset_px * (track_h - thumb_h)) / scroll_max).min(track_h - thumb_h);
        if y >= thumb_y && y < thumb_y + thumb_h {
            return CHROME_MUTED;
        } else {
            return CHROME_DIVIDER;
        }
    }

    CHROME_BG
}

// -----------------------------------------------------------------------------
// Sheet Overlays Renderer (Tab Switcher, Bookmarks, History, Menu)
// -----------------------------------------------------------------------------
fn render_sheet_overlay(x: usize, y: usize, app: &BrowserApp) -> Option<u32> {
    match app.overlay {
        SheetOverlay::None => None,
        SheetOverlay::TabSwitcher => {
            // Dark dim overlay across viewport
            if y < TOOLBAR_Y1 || y >= NAV_BAR_Y0 {
                return None;
            }
            // Tab Switcher Header (y 120..170)
            if y >= 120 && y < 170 {
                let header = b"Open Tabs";
                if let Some(px) = text_px(x, y, 32, 134, 1, header, CHROME_TEXT, CHROME_BG, true) {
                    return Some(px);
                }
                // "+ New Tab" button at x=530..680, y=126..164
                let new_btn = Rect::new(530, 126, 150, 38);
                if let Some(c) = draw_rounded_rect(x, y, new_btn, 8, CHROME_BLUE, None, 0) {
                    let label = b"+ New Tab";
                    if let Some(px) = text_px(x, y, 560, 138, 1, label, 0xFFFFFFFF, c, true) {
                        return Some(px);
                    }
                    return Some(c);
                }
                return Some(CHROME_BG);
            }

            // Tab cards stacked vertically (4 slots)
            let card_h = 110;
            let card_gap = 16;
            for i in 0..MAX_TABS {
                let card_y = 180 + i * (card_h + card_gap);
                let card_rect = Rect::new(24, card_y, W - 48, card_h);
                let tab = app.tabs.tab(i);
                let is_active = app.tabs.active_tab_index() == i;

                if let Some(tab_data) = tab {
                    let border_col = if is_active { CHROME_BLUE } else { CHROME_CARD_BORDER };
                    let border_w = if is_active { 2 } else { 1 };
                    if let Some(c) = draw_rounded_rect(x, y, card_rect, 14, CHROME_CARD, Some(border_col), border_w) {
                        // Tab badge `[i+1]`
                        let badge_rect = Rect::new(40, card_y + 16, 28, 24);
                        if let Some(bc) = draw_rounded_rect(x, y, badge_rect, 6, CHROME_SURFACE, None, 0) {
                            let digit = [b'1' + (i as u8)];
                            if let Some(px) = text_px(x, y, 48, card_y + 20, 1, &digit, CHROME_BLUE, bc, true) {
                                return Some(px);
                            }
                            return Some(bc);
                        }

                        // Tab Title
                        let title = tab_data.title_bytes();
                        if let Some(px) = text_px(x, y, 78, card_y + 20, 1, title, CHROME_TEXT, c, true) {
                            return Some(px);
                        }

                        // Tab URL
                        let url = tab_data.url();
                        if let Some(px) = text_px(x, y, 40, card_y + 54, 1, url, CHROME_MUTED, c, false) {
                            return Some(px);
                        }

                        // Close button 'X' (x 630..662, y card_y+16..card_y+48)
                        let close_cx = 646;
                        let close_cy = card_y + 32;
                        if close_glyph(x, y, close_cx, close_cy) {
                            return Some(CHROME_MUTED);
                        }

                        return Some(c);
                    }
                } else {
                    // Empty slot dashed/dim card
                    if let Some(c) = draw_rounded_rect(x, y, card_rect, 14, CHROME_SURFACE, Some(CHROME_DIVIDER), 1) {
                        let label = b"+ Empty Tab Slot";
                        if let Some(px) = text_px(x, y, 40, card_y + 44, 1, label, CHROME_MUTED, c, false) {
                            return Some(px);
                        }
                        return Some(c);
                    }
                }
            }
            Some(CHROME_BG)
        }
        SheetOverlay::Bookmarks => {
            if y < TOOLBAR_Y1 || y >= NAV_BAR_Y0 {
                return None;
            }
            // Bookmarks Header
            if y >= 120 && y < 170 {
                let header = b"Saved Bookmarks";
                if let Some(px) = text_px(x, y, 32, 134, 1, header, CHROME_TEXT, CHROME_BG, true) {
                    return Some(px);
                }
                // Close button 'X' at x=650, y=140
                if close_glyph(x, y, 660, 145) {
                    return Some(CHROME_MUTED);
                }
                return Some(CHROME_BG);
            }

            // List of bookmarks
            let count = app.bookmarks.len();
            if count == 0 {
                let empty_msg = b"No bookmarks saved yet.";
                if let Some(px) = text_px(x, y, 32, 220, 1, empty_msg, CHROME_MUTED, CHROME_BG, false) {
                    return Some(px);
                }
            } else {
                let row_h = 80;
                for i in 0..count {
                    let r_y = 180 + i * row_h;
                    let r_rect = Rect::new(24, r_y, W - 48, row_h - 10);
                    if let Some(rec) = app.bookmarks.record(i) {
                        if let Some(c) = draw_rounded_rect(x, y, r_rect, 10, CHROME_CARD, Some(CHROME_BORDER), 1) {
                            if star_glyph(x, y, 48, r_y + 35, true) {
                                return Some(CHROME_BLUE);
                            }
                            if let Some(px) = text_px(x, y, 70, r_y + 16, 1, rec.title_bytes(), CHROME_TEXT, c, true) {
                                return Some(px);
                            }
                            if let Some(px) = text_px(x, y, 70, r_y + 42, 1, rec.url_bytes(), CHROME_MUTED, c, false) {
                                return Some(px);
                            }
                            return Some(c);
                        }
                    }
                }
            }
            Some(CHROME_BG)
        }
        SheetOverlay::History => {
            if y < TOOLBAR_Y1 || y >= NAV_BAR_Y0 {
                return None;
            }
            // History Header
            if y >= 120 && y < 170 {
                let header = b"Navigation History";
                if let Some(px) = text_px(x, y, 32, 134, 1, header, CHROME_TEXT, CHROME_BG, true) {
                    return Some(px);
                }
                if close_glyph(x, y, 660, 145) {
                    return Some(CHROME_MUTED);
                }
                return Some(CHROME_BG);
            }

            let count = app.history.len();
            if count == 0 {
                let empty_msg = b"No history recorded for this session.";
                if let Some(px) = text_px(x, y, 32, 220, 1, empty_msg, CHROME_MUTED, CHROME_BG, false) {
                    return Some(px);
                }
            } else {
                let row_h = 60;
                for i in 0..count.min(8) {
                    let r_y = 180 + i * row_h;
                    let r_rect = Rect::new(24, r_y, W - 48, row_h - 8);
                    if let Some(entry) = app.history.entry(count - 1 - i) {
                        if let Some(c) = draw_rounded_rect(x, y, r_rect, 8, CHROME_CARD, Some(CHROME_BORDER), 1) {
                            if let Some(px) = text_px(x, y, 40, r_y + 18, 1, entry, CHROME_TEXT, c, false) {
                                return Some(px);
                            }
                            return Some(c);
                        }
                    }
                }
            }
            Some(CHROME_BG)
        }
        SheetOverlay::Menu => {
            // Kebab Overflow Menu dropdown card at top right (x 440..696, y 116..360)
            let menu_rect = Rect::new(440, 116, 256, 240);
            if let Some(c) = draw_rounded_rect(x, y, menu_rect, 12, CHROME_CARD, Some(CHROME_BORDER), 2) {
                let items: [&[u8]; 5] = [
                    b"New Tab",
                    b"Bookmarks",
                    b"History",
                    b"Reload Page",
                    b"Close Menu",
                ];
                for (idx, item) in items.iter().enumerate() {
                    let item_y = 130 + idx * 44;
                    if let Some(px) = text_px(x, y, 464, item_y, 1, item, CHROME_TEXT, c, false) {
                        return Some(px);
                    }
                    if idx < 4 && y == item_y + 30 {
                        return Some(CHROME_DIVIDER);
                    }
                }
                return Some(c);
            }
            None
        }
    }
}

// -----------------------------------------------------------------------------
// Bottom Navigation Bar (y 1350..1410)
// -----------------------------------------------------------------------------
fn bottom_bar_pixel(x: usize, y: usize, app: &BrowserApp) -> u32 {
    if y == NAV_BAR_Y0 {
        return CHROME_DIVIDER;
    }

    // 1. Back Button `<`
    if let Some(_) = draw_rounded_rect(x, y, BACK_BTN, BTN_R, CHROME_BG, None, 0) {
        let cx = BACK_BTN.x + BACK_BTN.w / 2;
        let cy = BACK_BTN.y + BACK_BTN.h / 2;
        let active = app.tabs.can_go_back() || app.history.can_back();
        let col = if active { CHROME_TEXT } else { CHROME_BORDER };
        if let Some(c) = std::design::back_button::chevron_ink(x, y, cx, cy, col) {
            return c;
        }
    }

    // 2. Forward Button `>`
    if let Some(_) = draw_rounded_rect(x, y, FWD_BTN, BTN_R, CHROME_BG, None, 0) {
        let cx = FWD_BTN.x + FWD_BTN.w / 2;
        let cy = FWD_BTN.y + FWD_BTN.h / 2;
        let active = app.tabs.can_go_forward() || app.history.can_forward();
        let col = if active { CHROME_TEXT } else { CHROME_BORDER };
        let mirrored_x = if (x as isize) <= cx as isize {
            cx + (cx - x)
        } else {
            cx.saturating_sub(x - cx)
        };
        if let Some(c) = std::design::back_button::chevron_ink(mirrored_x, y, cx, cy, col) {
            return c;
        }
    }

    // 3. Home / New Tab `+` Button
    if let Some(_) = draw_rounded_rect(x, y, HOME_TAB_BTN, BTN_R, CHROME_SURFACE, Some(CHROME_BORDER), 1) {
        let cx = HOME_TAB_BTN.x + HOME_TAB_BTN.w / 2;
        let cy = HOME_TAB_BTN.y + HOME_TAB_BTN.h / 2;
        let dx = (x as isize - cx as isize).abs();
        let dy = (y as isize - cy as isize).abs();
        if (dx <= 1 && dy <= 7) || (dy <= 1 && dx <= 7) {
            return CHROME_TEXT;
        }
    }

    // 4. Bookmark Star Button
    if let Some(_) = draw_rounded_rect(x, y, ADD_BOOK_BTN, BTN_R, CHROME_BG, None, 0) {
        let cx = ADD_BOOK_BTN.x + ADD_BOOK_BTN.w / 2;
        let cy = ADD_BOOK_BTN.y + ADD_BOOK_BTN.h / 2;
        let filled = app.bookmarks.len() + app.bookmarks.unsaved_len() > 0;
        if star_glyph(x, y, cx, cy, filled) {
            return CHROME_MUTED;
        }
    }

    // 5. Tabs Counter `[N]`
    if let Some(_) = draw_rounded_rect(x, y, TABS_BTN, BTN_R, CHROME_BG, None, 0) {
        let cx = TABS_BTN.x + TABS_BTN.w / 2;
        let cy = TABS_BTN.y + TABS_BTN.h / 2;
        if let Some(px) = tab_badge_pixel(x, y, cx, cy, app.tabs.tab_count()) {
            return px;
        }
    }

    CHROME_BG
}

// -----------------------------------------------------------------------------
// Pixel Dispatch
// -----------------------------------------------------------------------------
fn get_browser_pixel(x: usize, y: usize, app: &BrowserApp, f: &Frame) -> u32 {
    let base = get_browser_pixel_content(x, y, app, f);
    if app.overlay == SheetOverlay::None {
        return base;
    }
    // WHY the scrim: the sheet's own card drew straight over the content
    // with nothing dimming it, so a dropdown read as a layout fault
    // rather than a layer. The scrim is the content pixel blended toward
    // the background, so the content underneath is still visible but
    // clearly behind.
    if let Some(overlay_px) = render_sheet_overlay(x, y, app) {
        return overlay_px;
    }
    blend_toward(base, CHROME_BG, 150)
}

/// The screen without the sheet overlay applied.
///
/// WHY this is separate: an open sheet dims the content behind it, and
/// that dim has to be computed from the content pixel, so the content has
/// to be renderable on its own.
fn get_browser_pixel_content(x: usize, y: usize, app: &BrowserApp, f: &Frame) -> u32 {
    if y < STATUS_H {
        return CHROME_BG;
    }
    if y < TOOLBAR_Y1 {
        return toolbar_pixel(x, y, app, f);
    }
    if y >= NAV_BAR_Y0 && y < NAV_BAR_Y1 {
        return bottom_bar_pixel(x, y, app);
    }
    if y >= NAV_BAR_Y1 {
        if GESTURE_PILL.contains(x, y) {
            return CHROME_BORDER;
        }
        return CHROME_BG;
    }

    // Active Sheet Overlay if present
    // Active Sheet Overlay if present.
    //
    // WHY the scrim: the sheet's own card drew straight over the
    // content with nothing dimming it, so a dropdown read as a layout
    // fault rather than a layer. The scrim is the content pixel blended
    // toward the background, so the content underneath is still visible
    // but clearly behind.

    // Soft Keyboard and Accessory Strip overlay when address is focused
    if app.address_focused && y >= KB_ACC_Y0 && y <= KB_Y1 {
        if y <= KB_ACC_Y1 {
            return render_accessory_bar(x, y, app);
        } else {
            return render_browser_soft_keyboard(x, y, app);
        }
    }

    // Search and Direct Navigation suggestion cards when address is focused
    if app.address_focused && y >= 116 && y < KB_ACC_Y0 {
        if app.address_len > 0 {
            // Search suggestion card: y: 130..198
            let s_rect = Rect::new(24, 130, W - 48, 68);
            if let Some(c) = draw_rounded_rect(x, y, s_rect, 14, CHROME_CARD, Some(CHROME_BORDER), 1) {
                let icon_box = Rect::new(38, 144, 40, 40);
                if let Some(ic) = draw_rounded_rect(x, y, icon_box, 8, CHROME_SURFACE, None, 0) {
                    let dx = (x as isize) - 58;
                    let dy = (y as isize) - 164;
                    if (dx * dx + dy * dy >= 9 && dx * dx + dy * dy <= 25) || (dx >= 3 && dx <= 6 && dy >= 3 && dy <= 6) {
                        return CHROME_BLUE;
                    }
                    return ic;
                }
                if let Some(px) = text_px(x, y, 92, 144, 1, b"Search Web", CHROME_MUTED, c, false) {
                    return px;
                }
                let q = &app.address[..app.address_len.min(32)];
                if let Some(px) = text_px(x, y, 92, 168, 1, q, CHROME_BLUE, c, true) {
                    return px;
                }
                return c;
            }

            // Direct Navigate card: y: 208..276
            let d_rect = Rect::new(24, 208, W - 48, 68);
            if let Some(c) = draw_rounded_rect(x, y, d_rect, 14, CHROME_CARD, Some(CHROME_BORDER), 1) {
                let icon_box = Rect::new(38, 222, 40, 40);
                if let Some(ic) = draw_rounded_rect(x, y, icon_box, 8, CHROME_CARD, None, 0) {
                    let dx = (x as isize) - 58;
                    let dy = (y as isize) - 242;
                    if dx * dx + dy * dy <= 16 {
                        return CHROME_TEXT;
                    }
                    return ic;
                }
                if let Some(px) = text_px(x, y, 92, 222, 1, b"Open Address", CHROME_MUTED, c, false) {
                    return px;
                }
                let q = &app.address[..app.address_len.min(32)];
                if let Some(px) = text_px(x, y, 92, 246, 1, q, CHROME_TEXT, c, true) {
                    return px;
                }
                return c;
            }
            return CHROME_BG;
        } else {
            // When address_len == 0, show Quick Destinations
            if y >= 130 && y < 156 {
                if let Some(px) = text_px(x, y, 28, 134, 1, b"Quick links", CHROME_MUTED, CHROME_BG, true) {
                    return px;
                }
                return CHROME_BG;
            }
            let q1 = Rect::new(24, 160, W - 48, 62);
            if let Some(c) = draw_rounded_rect(x, y, q1, 12, CHROME_CARD, Some(CHROME_BORDER), 1) {
                if let Some(px) = text_px(x, y, 44, 172, 1, b"InterOS Documentation", CHROME_TEXT, c, true) {
                    return px;
                }
                if let Some(px) = text_px(x, y, 44, 196, 1, b"https://interos.tech", CHROME_MUTED, c, false) {
                    return px;
                }
                return c;
            }
            let q2 = Rect::new(24, 232, W - 48, 62);
            if let Some(c) = draw_rounded_rect(x, y, q2, 12, CHROME_CARD, Some(CHROME_BORDER), 1) {
                if let Some(px) = text_px(x, y, 44, 244, 1, b"Gemini Sovereign Space", CHROME_TEXT, c, true) {
                    return px;
                }
                if let Some(px) = text_px(x, y, 44, 268, 1, b"gemini://geminispace.info", CHROME_MUTED, c, false) {
                    return px;
                }
                return c;
            }
            let q3 = Rect::new(24, 304, W - 48, 62);
            if let Some(c) = draw_rounded_rect(x, y, q3, 12, CHROME_CARD, Some(CHROME_BORDER), 1) {
                if let Some(px) = text_px(x, y, 44, 316, 1, b"Core Source Repository", CHROME_TEXT, c, true) {
                    return px;
                }
                if let Some(px) = text_px(x, y, 44, 340, 1, b"https://github.com", CHROME_MUTED, c, false) {
                    return px;
                }
                return c;
            }
            return CHROME_BG;
        }
    }

    // Content Viewport (y 116..1350)
    if f.is_home {
        home_page_pixel(x, y, app)
    } else {
        web_page_pixel(x, y, app, f)
    }
}

fn render_browser_ui(surface: u32, app: &BrowserApp) {
    render_browser_band(surface, app, 0, H);
}

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
        let _ = std::surface_write(surface, offset, &chunk[..count]);
        offset += count;
    }
}

// -----------------------------------------------------------------------------
// Touch & Pointer Reactor
// -----------------------------------------------------------------------------
enum Tap<'a> {
    None,
    Redraw,
    Leave(&'a str),
}

fn handle_tap(app: &mut BrowserApp, px: u32, py: u32) -> Tap<'static> {
    let (x, y) = (px as usize, py as usize);

    // Gesture Home Bar (y >= 1410): Exit to Launcher
    if y >= HOME_Y0 {
        return Tap::Leave("[BROWSER APP] Home gesture pressed. Exiting to launcher.");
    }

    // Top Omnibox Close Button
    if CLOSE_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        return Tap::Leave("[BROWSER APP] Close control pressed. Exiting to launcher.");
    }

    // Kebab Menu Button
    if KEBAB_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        app.overlay = if app.overlay == SheetOverlay::Menu {
            SheetOverlay::None
        } else {
            SheetOverlay::Menu
        };
        return Tap::Redraw;
    }

    // Tabs Top Badge Button
    if TABS_TOP_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        app.overlay = if app.overlay == SheetOverlay::TabSwitcher {
            SheetOverlay::None
        } else {
            SheetOverlay::TabSwitcher
        };
        return Tap::Redraw;
    }

    // Reload Button
    if RELOAD_BTN.contains(x, y) {
        app.reload();
        return Tap::Redraw;
    }

    // Omnibox URL Bar Tap
    if PILL.contains(x, y) {
        if app.bm_step == BookmarkStep::Idle && !app.address_focused {
            if app.current_url() == b"about:blank" {
                app.address_len = 0;
            } else {
                let n = app.current_url_len.min(app.address.len());
                app.address[..n].copy_from_slice(&app.current_url[..n]);
                app.address_len = n;
            }
        }
        app.address_focused = true;
        app.select_all = app.address_len > 0;
        app.overlay = SheetOverlay::None;
        let _ = std::audio_haptic_click();
        std::log("[BROWSER APP] Address field focused for keyboard input.").ok();
        return Tap::Redraw;
    }

    // If address bar is focused, handle keyboard, accessory bar, and suggestion taps
    if app.address_focused {
        // 1. Accessory bar chips (y: KB_ACC_Y0..KB_ACC_Y1)
        if y >= KB_ACC_Y0 && y <= KB_ACC_Y1 {
            // Chip 1: "https://" (x 24..134)
            if x >= 24 && x <= 134 {
                let _ = std::audio_haptic_click();
                let prefix = b"https://";
                if !app.address[..app.address_len].starts_with(prefix) {
                    let mut new_addr = [0u8; 128];
                    let p_len = prefix.len();
                    new_addr[..p_len].copy_from_slice(prefix);
                    let copy_n = app.address_len.min(128 - p_len);
                    new_addr[p_len..p_len + copy_n].copy_from_slice(&app.address[..copy_n]);
                    let total = p_len + copy_n;
                    app.address[..total].copy_from_slice(&new_addr[..total]);
                    app.address_len = total;
                    app.select_all = false;
                }
                return Tap::Redraw;
            }
            // Chip 2: ".com" (x 144..224)
            if x >= 144 && x <= 224 {
                let _ = std::audio_haptic_click();
                let suffix = b".com";
                if app.address_len + suffix.len() <= app.address.len() {
                    app.address[app.address_len..app.address_len + suffix.len()].copy_from_slice(suffix);
                    app.address_len += suffix.len();
                    app.select_all = false;
                }
                return Tap::Redraw;
            }
            // Chip 3: ".org" (x 234..314)
            if x >= 234 && x <= 314 {
                let _ = std::audio_haptic_click();
                let suffix = b".org";
                if app.address_len + suffix.len() <= app.address.len() {
                    app.address[app.address_len..app.address_len + suffix.len()].copy_from_slice(suffix);
                    app.address_len += suffix.len();
                    app.select_all = false;
                }
                return Tap::Redraw;
            }
            // Chip 4: "Clear" (x 324..404)
            if x >= 324 && x <= 404 {
                let _ = std::audio_haptic_click();
                app.address_len = 0;
                app.select_all = false;
                return Tap::Redraw;
            }
            // Dismiss down chevron (x 630..694)
            if x >= 630 && x <= 694 {
                let _ = std::audio_haptic_click();
                app.address_focused = false;
                app.select_all = false;
                return Tap::Redraw;
            }
            return Tap::Redraw;
        }

        // 2. Keyboard keys (y: KB_Y0..KB_Y1)
        if y >= KB_Y0 && y <= KB_Y1 {
            if let Some(ch) = get_soft_keyboard_char(x, y, app.kb_shift, app.kb_symbols) {
                let _ = std::audio_haptic_click();
                match ch {
                    0xFF => {
                        app.kb_shift = !app.kb_shift;
                    }
                    0xFE => {
                        app.kb_symbols = !app.kb_symbols;
                    }
                    8 => {
                        // Backspace
                        if app.select_all {
                            app.address_len = 0;
                            app.select_all = false;
                        } else if app.address_len > 0 {
                            app.address_len -= 1;
                        }
                    }
                    b'\n' => {
                        // Go / Navigate
                        if app.bm_step != BookmarkStep::Idle {
                            let _ = app.commit_bookmark_step();
                        } else if app.address_len > 0 {
                            app.address_focused = false;
                            app.select_all = false;
                            let mut url = [0u8; 128];
                            let n = app.address_len.min(url.len());
                            url[..n].copy_from_slice(&app.address[..n]);
                            app.navigate(&url[..n]);
                        }
                    }
                    c => {
                        if app.select_all {
                            app.address[0] = c;
                            app.address_len = 1;
                            app.select_all = false;
                        } else if app.address_len < app.address.len() {
                            app.address[app.address_len] = c;
                            app.address_len += 1;
                        }
                    }
                }
                return Tap::Redraw;
            }
            return Tap::Redraw;
        }

        // 3. Typing suggestion cards
        if app.address_len > 0 && x >= 24 && x <= W - 24 {
            if y >= 130 && y <= 198 {
                let _ = std::audio_haptic_click();
                app.address_focused = false;
                app.select_all = false;
                let mut search_url = [0u8; 128];
                let prefix = b"https://interos.tech/search?q=";
                let p_len = prefix.len();
                search_url[..p_len].copy_from_slice(prefix);
                let q_len = app.address_len.min(128 - p_len);
                search_url[p_len..p_len + q_len].copy_from_slice(&app.address[..q_len]);
                let total = p_len + q_len;
                app.navigate(&search_url[..total]);
                return Tap::Redraw;
            }
            if y >= 208 && y <= 276 {
                let _ = std::audio_haptic_click();
                app.address_focused = false;
                app.select_all = false;
                let mut url = [0u8; 128];
                let n = app.address_len.min(url.len());
                url[..n].copy_from_slice(&app.address[..n]);
                app.navigate(&url[..n]);
                return Tap::Redraw;
            }
        } else if app.address_len == 0 && x >= 24 && x <= W - 24 {
            if y >= 160 && y <= 222 {
                let _ = std::audio_haptic_click();
                app.address_focused = false;
                app.navigate(b"https://interos.tech");
                return Tap::Redraw;
            }
            if y >= 232 && y <= 294 {
                let _ = std::audio_haptic_click();
                app.address_focused = false;
                app.navigate(b"gemini://geminispace.info");
                return Tap::Redraw;
            }
            if y >= 304 && y <= 366 {
                let _ = std::audio_haptic_click();
                app.address_focused = false;
                app.navigate(b"https://github.com");
                return Tap::Redraw;
            }
        }

        // 4. Outside tap: dismiss keyboard
        if !PILL.contains(x, y) {
            app.address_focused = false;
            app.select_all = false;
            return Tap::Redraw;
        }
    }

    // Bottom Navigation Bar Buttons
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
    if HOME_TAB_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        let start = b"about:blank";
        app.current_url[..start.len()].copy_from_slice(start);
        app.current_url_len = start.len();
        app.status = PageStatus::Blank;
        app.address_focused = false;
        app.overlay = SheetOverlay::None;
        std::log("[BROWSER APP] Returned to Sovereign Browser New Tab start page.").ok();
        return Tap::Redraw;
    }
    if ADD_BOOK_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        app.overlay = if app.overlay == SheetOverlay::Bookmarks {
            SheetOverlay::None
        } else {
            SheetOverlay::Bookmarks
        };
        return Tap::Redraw;
    }
    if TABS_BTN.contains(x, y) {
        let _ = std::audio_haptic_click();
        app.overlay = if app.overlay == SheetOverlay::TabSwitcher {
            SheetOverlay::None
        } else {
            SheetOverlay::TabSwitcher
        };
        return Tap::Redraw;
    }

    // Handle Active Overlays Interactions
    match app.overlay {
        SheetOverlay::Menu => {
            if x >= 440 && x <= 696 && y >= 116 && y <= 356 {
                let row = (y - 116) / 48;
                let _ = std::audio_haptic_click();
                match row {
                    0 => {
                        // New Tab
                        app.tabs.new_tab(b"about:blank");
                        let start = b"about:blank";
                        app.current_url[..start.len()].copy_from_slice(start);
                        app.current_url_len = start.len();
                        app.status = PageStatus::Blank;
                    }
                    1 => app.overlay = SheetOverlay::Bookmarks,
                    2 => app.overlay = SheetOverlay::History,
                    3 => app.reload(),
                    _ => app.overlay = SheetOverlay::None,
                }
                if row != 1 && row != 2 {
                    app.overlay = SheetOverlay::None;
                }
                // WHY the address field is released: a sheet opened from the
                // toolbar has to dismiss the soft keyboard, or the two
                // layers are drawn over each other and neither can be read.
                app.address_focused = false;
                app.select_all = false;
                return Tap::Redraw;
            }
            app.overlay = SheetOverlay::None;
            return Tap::Redraw;
        }
        SheetOverlay::TabSwitcher => {
            // New Tab button in header (x 530..680, y 126..164)
            if x >= 530 && x <= 680 && y >= 126 && y <= 164 {
                let _ = std::audio_haptic_click();
                app.tabs.new_tab(b"about:blank");
                let start = b"about:blank";
                app.current_url[..start.len()].copy_from_slice(start);
                app.current_url_len = start.len();
                app.status = PageStatus::Blank;
                app.overlay = SheetOverlay::None;
                return Tap::Redraw;
            }

            // Tab cards interaction (4 slots)
            let card_h = 110;
            let card_gap = 16;
            for i in 0..MAX_TABS {
                let card_y = 180 + i * (card_h + card_gap);
                if y >= card_y && y < card_y + card_h && x >= 24 && x <= W - 24 {
                    let _ = std::audio_haptic_click();
                    // Close button tapped? (x 630..670)
                    if x >= 630 {
                        app.tabs.close_tab(i);
                        let cur = app.tabs.active_tab().url();
                        let n = cur.len().min(app.current_url.len());
                        app.current_url[..n].copy_from_slice(&cur[..n]);
                        app.current_url_len = n;
                    } else if app.tabs.tab(i).is_some() {
                        app.tabs.switch_tab(i);
                        let cur = app.tabs.active_tab().url();
                        let n = cur.len().min(app.current_url.len());
                        app.current_url[..n].copy_from_slice(&cur[..n]);
                        app.current_url_len = n;
                        app.overlay = SheetOverlay::None;
                    } else {
                        // Empty slot tapped: create new tab
                        app.tabs.new_tab(b"about:blank");
                        let start = b"about:blank";
                        app.current_url[..start.len()].copy_from_slice(start);
                        app.current_url_len = start.len();
                        app.status = PageStatus::Blank;
                        app.overlay = SheetOverlay::None;
                    }
                    return Tap::Redraw;
                }
            }
        }
        SheetOverlay::Bookmarks => {
            if x >= 640 && y >= 120 && y <= 165 {
                app.overlay = SheetOverlay::None;
                return Tap::Redraw;
            }
            let row_h = 80;
            for i in 0..app.bookmarks.len() {
                let r_y = 180 + i * row_h;
                if y >= r_y && y < r_y + row_h && x >= 24 && x <= W - 24 {
                    let _ = std::audio_haptic_click();
                    if let Some(rec) = app.bookmarks.record(i) {
                        let mut url = [0u8; 64];
                        let u = rec.url_bytes();
                        let n = u.len().min(url.len());
                        url[..n].copy_from_slice(&u[..n]);
                        app.overlay = SheetOverlay::None;
                        app.navigate(&url[..n]);
                        return Tap::Redraw;
                    }
                }
            }
        }
        SheetOverlay::History => {
            if x >= 640 && y >= 120 && y <= 165 {
                app.overlay = SheetOverlay::None;
                return Tap::Redraw;
            }
            let row_h = 60;
            let count = app.history.len();
            for i in 0..count.min(8) {
                let r_y = 180 + i * row_h;
                if y >= r_y && y < r_y + row_h && x >= 24 && x <= W - 24 {
                    let _ = std::audio_haptic_click();
                    if let Some(entry) = app.history.entry(count - 1 - i) {
                        let mut url = [0u8; 64];
                        let n = entry.len().min(url.len());
                        url[..n].copy_from_slice(&entry[..n]);
                        app.overlay = SheetOverlay::None;
                        app.navigate(&url[..n]);
                        return Tap::Redraw;
                    }
                }
            }
        }
        SheetOverlay::None => {}
    }

    // Start Page Favorites / Quick Access Tiles Tap (y: 278..342)
    if app.status == PageStatus::Blank && y >= 278 && y <= 342 {
        // Col 0: InterOS (x: 66..130)
        if x >= 66 && x <= 130 {
            let _ = std::audio_haptic_click();
            app.navigate(b"https://interos.tech");
            return Tap::Redraw;
        }
        // Col 1: YouTube (x: 234..298)
        if x >= 234 && x <= 298 {
            let _ = std::audio_haptic_click();
            app.navigate(b"https://m.youtube.com");
            return Tap::Redraw;
        }
        // Col 2: GitHub (x: 402..466)
        if x >= 402 && x <= 466 {
            let _ = std::audio_haptic_click();
            app.navigate(b"https://github.com");
            return Tap::Redraw;
        }
        // Col 3: Wikipedia (x: 570..634)
        if x >= 570 && x <= 634 {
            let _ = std::audio_haptic_click();
            app.navigate(b"https://wikipedia.org");
            return Tap::Redraw;
        }
    }

    // Start Page "Your pages" Cards Tap (y: 422..1340)
    if app.status == PageStatus::Blank && x >= 24 && x < W - 24 {
        let card_h = 100;
        let card_gap = 12;
        for i in 0..4 {
            let card_y = 422 + i * (card_h + card_gap);
            if y >= card_y && y < card_y + card_h {
                if let Some(row) = home_row(app, i) {
                    let _ = std::audio_haptic_click();
                    app.address_focused = false;
                    app.select_all = false;
                    app.address_len = 0;
                    let url = &row.url[..row.url_len];
                    app.navigate(url);
                    return Tap::Redraw;
                }
            }
        }
    }

    // Content Viewport Links Tap
    if app.status == PageStatus::Loaded && y >= 120 && y < 1340 {
        let content_start_y = 126;
        let base_y = (content_start_y as isize) - (app.scroll_offset_px as isize);
        let mut cur_y = base_y;

        let mut clicked_target: Option<([u8; 64], usize)> = None;
        for i in 0..app.doc_line_count {
            let line = &app.doc_lines[i];
            let bytes = &line.bytes[..line.len];
            let line_h = match line.kind {
                LineKind::Heading1 => 42,
                LineKind::Heading2 => 36,
                LineKind::Heading3 => 30,
                LineKind::ListItem => 28,
                LineKind::CodeBlock => {
                    if bytes.starts_with(b"```") {
                        0
                    } else {
                        44
                    }
                }
                LineKind::GemtextLink => 66,
                LineKind::Blockquote => 34,
                LineKind::Paragraph => 28,
                LineKind::Empty => 14,
            };

            if line.kind == LineKind::GemtextLink && line.link_target_len > 0 {
                let g_y = cur_y + 4;
                let g_h = 56;
                if y as isize >= g_y && (y as isize) < g_y + g_h as isize && x >= 24 && x <= W - 24 {
                    let mut buf = [0u8; 64];
                    let u = line.link_target_len.min(64);
                    buf[..u].copy_from_slice(&line.link_target[..u]);
                    clicked_target = Some((buf, u));
                    break;
                }
            }
            cur_y += line_h;
        }

        if let Some((target_buf, target_len)) = clicked_target {
            let _ = std::audio_haptic_click();
            app.navigate(&target_buf[..target_len]);
            return Tap::Redraw;
        }

        // Tap on content area scrolls if not a link
        if y > 1000 {
            app.scroll_offset_px = app.scroll_offset_px.saturating_add(60);
            return Tap::Redraw;
        } else if y < 400 && app.scroll_offset_px > 0 {
            app.scroll_offset_px = app.scroll_offset_px.saturating_sub(60);
            return Tap::Redraw;
        }
    }

    // Offline / Error screen Reload Button tap
    if (app.status == PageStatus::NoTransport
        || app.status == PageStatus::InvalidAddress
        || app.status == PageStatus::LocalRefused)
        && ((y >= 630 && y <= 720) || (y >= 920 && y <= 968))
        && x >= (W / 2 - 90)
        && x <= (W / 2 + 90)
    {
        let _ = std::audio_haptic_click();
        app.reload();
        return Tap::Redraw;
    }

    Tap::None
}

// -----------------------------------------------------------------------------
// Native Keyboard Input Reactor
// -----------------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    None,
    Toolbar,
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
                if app.address_len > 0 {
                    let mut target = [0u8; 64];
                    let n = app.address_len.min(target.len());
                    target[..n].copy_from_slice(&app.address[..n]);
                    app.navigate(&target[..n]);
                }
            } else {
                let _ = app.commit_bookmark_step();
            }
            Dirty::All
        }
        input_key_code::ESCAPE => {
            if app.bm_step != BookmarkStep::Idle {
                app.cancel_bookmark_step();
                std::log("[BROWSER APP] Add-bookmark cancelled.").ok();
                return Dirty::All;
            }
            app.address_focused = false;
            app.select_all = false;
            Dirty::Toolbar
        }
        input_key_code::BACKSPACE => {
            if app.select_all {
                app.address_len = 0;
                app.select_all = false;
            } else {
                app.address_len = app.address_len.saturating_sub(1);
            }
            Dirty::Toolbar
        }
        code if (32..=126).contains(&code) && app.address_len < app.address.len() => {
            if app.select_all {
                app.address_len = 0;
                app.select_all = false;
            }
            app.address[app.address_len] = code as u8;
            app.address_len += 1;
            let status_moved = app.status != PageStatus::Blank;
            app.status = PageStatus::Blank;
            if status_moved {
                Dirty::All
            } else {
                Dirty::Toolbar
            }
        }
        _ => Dirty::None,
    }
}

fn destroy_and_exit(surface: u32) -> ! {
    if surface > 0 {
        // WHY the slot is resolved by type here too: a slot number is a
        // position in a table, and the position depends on the order the
        // task's capabilities were inserted in.
    }
    std::exit_with_code(0);
}

// -----------------------------------------------------------------------------
// Application Entry Point (_start)
// -----------------------------------------------------------------------------
#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[BROWSER APP (Ring 3)] Sovereign Decentralized Web Client online.",
    )
    .ok();

    let mut browser = BrowserApp::new();

    match browser.bookmarks.state() {
        BookmarkStoreState::Loaded => {
            std::log("[BROWSER APP] Bookmarks read back out of the settings store.").ok();
        }
        BookmarkStoreState::Empty => {
            std::log("[BROWSER APP] No saved bookmarks: the settings store holds no browser bookmarks key.").ok();
        }
        BookmarkStoreState::ReadRefused => {
            std::log("[BROWSER APP] Bookmark read REFUSED by the kernel: this task holds no filesystem capability, so no stored bookmarks were loaded and none will be shown as saved.").ok();
        }
        BookmarkStoreState::Invalid => {
            std::log("[BROWSER APP] Stored bookmark bytes failed validation; no bookmarks were loaded from them.").ok();
        }
        BookmarkStoreState::Untouched
        | BookmarkStoreState::Written
        | BookmarkStoreState::WriteRefused
        | BookmarkStoreState::Full => {
            std::log("[BROWSER APP] Bookmark store reported a state it cannot be in before any write; no bookmarks are shown as saved.").ok();
        }
    }

    std::log("[BROWSER APP] History and bookmarks are real, user-created records. Zero heap allocations in event loop.",
    )
    .ok();

    // Allocate DRM/KMS Client Surface (z=200)
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(id) => {
            std::log("[BROWSER APP] DRM/KMS Client Surface allocated (z=200).").ok();
            id
        }
        Err(_) => 0,
    };

    if surface > 0 {
        render_browser_ui(surface, &browser);
        let _ = std::surface_flip();
        std::log("[BROWSER APP] Browser canvas presented to scanout buffer.").ok();
    }

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
                            std::log("[BROWSER APP] Navigation gesture recognized. Exiting to launcher.").ok();
                            destroy_and_exit(surface);
                        }
                        // Touch drag / vertical scroll
                        if input.kind == 2 && browser.status == PageStatus::Loaded {
                            if input.dy != 0 {
                                browser.scroll_offset_px = browser
                                    .scroll_offset_px
                                    .saturating_add_signed(-(input.dy as isize));
                                dirty = Dirty::All;
                            }
                        }
                        if input.kind == 1 {
                            match handle_tap(&mut browser, input.x, input.y) {
                                Tap::Leave(msg) => {
                                    std::log(msg).ok();
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
                                render_browser_band(surface, &browser, STATUS_H, TOOLBAR_Y1);
                            }
                            Dirty::All => render_browser_ui(surface, &browser),
                        }
                        let _ = std::surface_flip();
                    }
                }
                if events & std::EVENT_IPC_MSG != 0 {
                    std::log("[BROWSER APP] IPC message event raised; no navigation intent was parsed from it.").ok();
                }
                if events & std::EVENT_NETWORK_PACKET != 0 {
                    match browser.active_socket_id {
                        Some(sock_id) => {
                            std::log("[BROWSER APP] Network packet arrived via e1000 socket.").ok();
                            let mut rx_buf = [0u8; 2048];
                            if let Ok(n) = std::net_recvfrom(sock_id, &mut rx_buf) {
                                if n > 0 {
                                    if let Some(http_resp) = browser::parse_http_response(&rx_buf[..n]) {
                                        let mut doc_buf = [0u8; 2048];
                                        let doc_len = browser::html_to_reader_document(http_resp.body, &mut doc_buf);
                                        if doc_len > 0 {
                                            browser.load_document_content(&doc_buf[..doc_len]);
                                            if surface > 0 {
                                                render_browser_ui(surface, &browser);
                                                let _ = std::surface_flip();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        None => {
                            std::log("[BROWSER APP] Network event raised with no open socket; no packet was received.").ok();
                        }
                    }
                }
                if events & std::EVENT_TIMER_TICK != 0 {
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
    std::log("[BROWSER APP] Panic detected! Terminating task...").ok();
    std::exit();
}
