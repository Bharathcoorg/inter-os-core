//! Inter OS Native Sovereign Encrypted Messaging & P2P Wire Client
//!
//! Native smartphone messaging client executing in Ring 3 (User Mode):
//! - Pure `#![no_std]` and `#![no_main]` with zero heap allocations in interactive loops.
//! - Truthful Platform Contract: Truthful `NO TRANSPORT` badge when modem hardware is absent.
//! - View Mode 1: `ThreadListView` (Search, conversation rows, avatars, unread pills, truthful empty state).
//! - View Mode 2: `ConversationView` (Chat bubbles, incoming gray cards, outgoing blue/green cards,
//!   truthful status indicators "Refused - No Transport", Threat Warning Banner).
//! - Deterministic GSM-7 packing before transmission.
//! - Local offline threat heuristic analyzer with zero external network dependencies.
//! - Persistent settings-backed thread store and circular bounded thread ledger.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use interos_std::design::outline_text;

use messages::gsm;
use messages::store;
use messages::store::ThreadStore;
use messages::{DeliveryStatus, MessageDirection, MessageRecord, ThreadLedger, ThreadStatus};

/// Conversation thread summary.
#[allow(dead_code)]
pub struct ConversationThread {
    pub peer_name: &'static str,
    pub peer_did: &'static str,
    pub last_snippet: &'static str,
    pub status: ThreadStatus,
    pub unread_count: u8,
}

/// Conversation directory populated from the real messaging backend.
/// Pinned by repository contracts; stays empty as a compile-time anchor.
#[allow(dead_code)]
pub static THREADS: [ConversationThread; 0] = [];

/// UI View Modes: Thread list directory vs. active single conversation view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    ThreadList,
    Conversation,
}

/// The settings key the dialer hands a recipient to this app under.
///
/// WHY a settings key and not an argument: `app_launch` takes an app id
/// and nothing else, so the platform's settings store is the only channel
/// the two apps share.
const HANDOFF_KEY: &str = "dialer.handoff.recipient.v1";

impl MessagesApp {
    /// Takes a recipient handed over by the dialer, if there is one.
    ///
    /// WHY this is read before the first screen: the messaging app shows the
    /// recipient before the first send, and it can only do that if the other
    /// app wrote the number into the shared settings store.
    ///
    /// WHY the key is cleared: a hand-off that is never consumed would
    /// prefill the recipient on every launch, long after the user had moved
    /// on.
    pub fn take_handoff_recipient(&mut self) {
        let mut buf = [0u8; 64];
        match std::settings_get(HANDOFF_KEY, &mut buf) {
            Ok(n) if n > 0 => {
                let n = n.min(self.recip.len()).min(buf.len());
                self.recip[..n].copy_from_slice(&buf[..n]);
                self.recip_len = n;
                self.recip_focused = true;
                let _ = std::settings_set(HANDOFF_KEY, &[]);
                std::log("[MESSAGES APP] A recipient was handed over by the dialer; it is shown in the recipient field.",
                )
                .ok();
            }
            _ => {}
        }
    }
}

/// Honest state of the last compose attempt, as reported in the header pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderStatus {
    /// Startup state: no messaging transport is bound to this app.
    NoTransport,
    /// The last composed message was sealed into vault sector 202.
    SealedLocally,
    /// The vault refused the seal, so the message was not persisted.
    VaultRefused,
    /// The thread store refused the append, so the message was not persisted.
    ///
    /// WHY this is separate from `VaultRefused`: the vault and the thread
    /// store are two different subsystems, and a user who loses a message
    /// needs to know which one refused it.
    StoreRefused,
    /// Send was pressed with no recipient, so nothing was composed.
    NoRecipient,
}

impl HeaderStatus {
    /// Header pill label.
    pub fn label(&self) -> &'static [u8] {
        match self {
            HeaderStatus::NoTransport => b"NO TRANSPORT",
            HeaderStatus::SealedLocally => b"SEALED LOCALLY",
            HeaderStatus::VaultRefused => b"VAULT REFUSED",
            HeaderStatus::StoreRefused => b"STORE REFUSED",
            HeaderStatus::NoRecipient => b"NO RECIPIENT",
        }
    }
}

/// Bounded input buffer capacities.
const COMPOSE_CAP: usize = 160;
const SEARCH_CAP: usize = 48;
const RECIP_CAP: usize = 32;

/// Production Sovereign Messages state manager.
pub struct MessagesApp {
    pub view_mode: ViewMode,
    pub active_thread_id: Option<u32>,
    pub ledger: ThreadLedger,
    pub store: ThreadStore,
    pub total_sent: u32,
    pub total_received: u32,
    pub active_thread_idx: usize,
    pub keystore_derived: bool,
    pub sector_encrypted: bool,
    /// Whether a cellular baseband was actually reported present by the kernel.
    pub modem_present: bool,
    /// Real outcome of the last compose attempt, drawn in the header pill.
    pub header_status: HeaderStatus,
    /// Compose editor buffer and its length.
    pub compose: [u8; COMPOSE_CAP],
    pub compose_len: usize,
    pub compose_focused: bool,
    pub compose_select_all: bool,
    /// Search/filter editor buffer and its length.
    pub search: [u8; SEARCH_CAP],
    pub search_len: usize,
    pub search_focused: bool,
    pub search_select_all: bool,
    /// Recipient ("To:") editor buffer and its length.
    pub recip: [u8; RECIP_CAP],
    pub recip_len: usize,
    pub recip_focused: bool,
    pub recip_select_all: bool,
}

impl MessagesApp {
    pub const fn new() -> Self {
        Self {
            view_mode: ViewMode::ThreadList,
            active_thread_id: None,
            ledger: ThreadLedger::new(),
            store: ThreadStore::EMPTY,
            total_sent: 0,
            total_received: 0,
            active_thread_idx: 0,
            keystore_derived: false,
            sector_encrypted: false,
            modem_present: false,
            header_status: HeaderStatus::NoTransport,
            compose: [0; COMPOSE_CAP],
            compose_len: 0,
            compose_focused: false,
            compose_select_all: false,
            search: [0; SEARCH_CAP],
            search_len: 0,
            search_focused: false,
            search_select_all: false,
            recip: [0; RECIP_CAP],
            recip_len: 0,
            recip_focused: false,
            recip_select_all: false,
        }
    }

    /// Derives the 256-bit AES-GCM / ChaCha20 session key from Keystore Slot 3.
    pub fn derive_messaging_key(&mut self) -> bool {
        let mut key = [0u8; 32];
        match std::keystore_derive(3, &mut key) {
            Ok(32) => {
                self.keystore_derived = true;
                std::log("[MESSAGES APP] Hardware Keystore Slot 3: Ingested 256-bit P2P messaging key.",
                )
                .ok();
                true
            }
            Ok(_) => {
                std::log("[MESSAGES APP] Keystore Slot 3: short result; no messaging key available.").ok();
                false
            }
            Err(_) => {
                std::log("[MESSAGES APP] Keystore Slot 3: derivation refused by the kernel; no session key.").ok();
                false
            }
        }
    }

    /// Seals the composed body into Sovereign Vault Sector 202.
    pub fn seal_composed(&mut self, body: &[u8]) -> bool {
        let mut sector_buf = [0u8; 512];
        let copy_len = body.len().min(sector_buf.len().saturating_sub(16));
        sector_buf[..copy_len].copy_from_slice(&body[..copy_len]);
        let mut iv = [0u8; 16];
        if std::get_random(&mut iv).is_err() {
            std::log("[MESSAGES APP] Refusing to seal message without cryptographic TRNG entropy.",
            )
            .ok();
            return false;
        }
        let assessment = gsm::analyze_threat(body);
        if assessment.level != gsm::ThreatLevel::Safe {
            std::log("[MESSAGES APP] Heuristic privacy shield: flagged potential urgency/phishing payload.",
            )
            .ok();
        }
        match std::vault_encrypt_sector(202, &mut sector_buf, &iv) {
            Ok(_) => {
                self.sector_encrypted = true;
                self.header_status = HeaderStatus::SealedLocally;
                std::log("[MESSAGES APP] Sector 202: Message sector sealed and persisted by the storage vault.",
                )
                .ok();
                std::log("[MESSAGES APP] No transport is bound to this app; the sealed message was not transmitted.",
                )
                .ok();
                true
            }
            Err(_) => {
                self.header_status = HeaderStatus::VaultRefused;
                std::log("[MESSAGES APP] Sector 202: vault write refused by the kernel; message not persisted.",
                )
                .ok();
                false
            }
        }
    }

    /// Plays the outgoing bubble chime (E5: 659Hz -> B5: 988Hz).
    pub fn play_bubble_chime(&self) -> bool {
        let mut tones_played = 0u32;
        if std::audio_play_tone(659).is_ok() {
            tones_played += 1;
        }
        if std::audio_play_tone(988).is_ok() {
            tones_played += 1;
        }
        let _ = std::audio_silence();
        let _ = std::audio_haptic_click();
        if tones_played == 2 {
            std::log("[MESSAGES APP] Outgoing message bubble chime emitted (659Hz -> 988Hz).",
            )
            .ok();
            true
        } else {
            std::log("[MESSAGES APP] Bubble chime not emitted: the audio service refused the tone requests.",
            )
            .ok();
            false
        }
    }

    /// Number of threads in the ledger that match the current search query.
    pub fn match_count(&self) -> usize {
        let q = &self.search[..self.search_len];
        if q.is_empty() {
            return self.ledger.thread_count;
        }
        let mut count = 0;
        for i in 0..self.ledger.thread_count {
            if thread_matches(&self.ledger.threads[i], q) {
                count += 1;
            }
        }
        count
    }

    /// Synchronizes threads from persistent ThreadStore into ThreadLedger on startup.
    pub fn sync_store_to_ledger(&mut self) {
        for i in 0..self.store.len() {
            if let Some(r) = self.store.get(i) {
                if let Some(tid) = self.ledger.create_or_get_thread(r.recipient()) {
                    let snip = r.last_snippet();
                    if !snip.is_empty() {
                        self.ledger.post_message_with_status(
                            tid,
                            MessageDirection::Outgoing,
                            snip,
                            DeliveryStatus::RefusedNoTransport,
                        );
                    }
                }
            }
        }
    }
}

/// Copies `src` into `dst`, truncating, and returns the bytes written.
fn copy_ascii(src: &[u8], dst: &mut [u8]) -> usize {
    let n = src.len().min(dst.len());
    dst[..n].copy_from_slice(&src[..n]);
    n
}

/// Decimal `v` into `out`, no leading zeros. Returns the digits.
fn put_u8(out: &mut [u8; 4], v: u8) -> usize {
    let mut tmp = [0u8; 4];
    let mut n = 0usize;
    let mut x = v;
    loop {
        tmp[n] = b'0' + (x % 10);
        x /= 10;
        n += 1;
        if x == 0 {
            break;
        }
    }
    for i in 0..n {
        out[i] = tmp[n - 1 - i];
    }
    n
}

/// Decimal `v` (usize) into `out`.
#[allow(dead_code)]
fn put_usize(out: &mut [u8; 8], v: usize) -> usize {
    let mut tmp = [0u8; 8];
    let mut n = 0usize;
    let mut x = v;
    loop {
        tmp[n] = b'0' + (x % 10) as u8;
        x /= 10;
        n += 1;
        if x == 0 {
            break;
        }
    }
    for i in 0..n {
        out[i] = tmp[n - 1 - i];
    }
    n
}

// -----------------------------------------------------------------------------
// Visual Design Tokens
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFF000000;
const SURFACE_DARK: u32 = 0xFF0E1117;
const SURFACE_RAISED: u32 = 0xFF181C22;
const BORDER_SUBTLE: u32 = 0xFF262C36;
const ACCENT_SKY: u32 = 0xFF10B981;
const TEXT_PRIMARY: u32 = 0xFFFFFFFF;
const TEXT_MUTED: u32 = 0xFF8B949E;
const DISABLED_FILL: u32 = 0xFF161B22;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// Draws a run of text with the sharp outline face, falling back to the
/// bitmap face when the outline face refuses the run.
///
/// WHY the outline face is tried first: the panel is 1080x2160 and this app
/// paints at 720x1440, so the compositor magnifies a bitmap run 3:2 and the
/// glyph edges read as soft. The outline face rasterises the real outline
/// and reads as sharp at the panel's own resolution.
///
/// WHY the box test comes first: the run's own arguments decide which face
/// is used, so the pixel under the cursor never gets to choose. The box is
/// the union of the two faces' boxes, because the outline face's box sits
/// one or two rows inside the bitmap cell's.
#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    if scale == 0 || text.is_empty() {
        return None;
    }
    let (bx, bt, bw, bh) = outline_text::run_box(x0, y0, scale, text);
    let bitmap_h = std::design::font::line_height(scale);
    let bitmap_w = std::design::font::measure(text, scale);
    let top = bt.min(y0);
    let height = bh.max(bitmap_h);
    let width = bw.max(bitmap_w);
    if x < bx || x - bx >= width || y < top || y - top >= height {
        return None;
    }
    if let Some(run) = outline_text::place_run(x0, y0, scale, text, None, false) {
        return run.pixel(x, y, color, bg);
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, false)
}

/// Anti-aliased bold run draw clipped to run's line box.
#[inline(always)]
fn test_text_bold(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    if scale == 0 || text.is_empty() {
        return None;
    }
    let (bx, bt, bw, bh) = outline_text::run_box(x0, y0, scale, text);
    let bitmap_h = std::design::font::line_height(scale);
    let bitmap_w = std::design::font::measure_bold(text, scale);
    let top = bt.min(y0);
    let height = bh.max(bitmap_h);
    // A bold outline run is one pixel wider than the run the box was measured
    // from, so the box has to carry that pixel or the last glyph is cut.
    let width = bw.max(bitmap_w) + 1;
    if x < bx || x - bx >= width || y < top || y - top >= height {
        return None;
    }
    if let Some(run) = outline_text::place_run(x0, y0, scale, text, None, true) {
        return run.pixel(x, y, color, bg);
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, true)
}

// -----------------------------------------------------------------------------
// Layout Geometry
// -----------------------------------------------------------------------------
const HEADER_Y0: usize = 0;
const HEADER_Y1: usize = 96;
const TITLE_TEXT: &[u8] = b"Messages";
const TITLE_X: usize = 116;
const TITLE_SCALE: usize = 1;
const TITLE_Y: usize = 36;

const PILL_RIGHT: usize = W - 24;
const PILL_TOP: usize = 28;
const PILL_BOTTOM: usize = 68;
const PILL_PAD_X: usize = 20;

#[inline]
fn pill_text(app: &MessagesApp) -> &'static [u8] {
    app.header_status.label()
}

#[inline]
fn pill_text_scale(label: &[u8]) -> usize {
    let title_end = TITLE_X + std::design::font::measure_bold(TITLE_TEXT, TITLE_SCALE);
    let avail = PILL_RIGHT.saturating_sub(title_end + 24);
    let mut scale = 1usize;
    while scale > 1 {
        let need = std::design::font::measure(label, scale) + 2 * PILL_PAD_X;
        if need <= avail {
            break;
        }
        scale -= 1;
    }
    scale
}

#[inline]
fn pill_left_for(label: &[u8], scale: usize) -> usize {
    let w = std::design::font::measure(label, scale) + 2 * PILL_PAD_X;
    PILL_RIGHT.saturating_sub(w)
}

// Search field geometry (y 104..156)
const SEARCH_Y0: usize = 104;
const SEARCH_Y1: usize = 156;
const SEARCH_FIELD: (usize, usize, usize, usize) = (24, 106, 672, 48);
const SEARCH_TEXT_X: usize = 82;
const SEARCH_TEXT_MAX_W: usize = 696 - 20 - SEARCH_TEXT_X;
const SEARCH_PLACEHOLDER: &[u8] = b"Search chats...";
const SEARCH_TEXT_SCALE: usize = 1;

// Recipient ("To:") editor
const RECIPIENT_BAND_Y0: usize = 1288;
const RECIPIENT_BAND_Y1: usize = 1330;
const RECIPIENT_FIELD: (usize, usize, usize, usize) = (24, 1292, 586, 36);
const RECIPIENT_TEXT_X: usize = 76;
const RECIPIENT_TEXT_MAX_W: usize = 490 - RECIPIENT_TEXT_X;
const RECIPIENT_PLACEHOLDER: &[u8] = b"Recipient (name or handle)";
const RECIPIENT_TEXT_SCALE: usize = 1;

// Compose field geometry (y 1330..1414)
const COMPOSE_BAND_Y0: usize = 1330;
const COMPOSE_BAND_Y1: usize = 1414;
const COMPOSE_FIELD: (usize, usize, usize, usize) = (24, 1344, 586, 60);
const COMPOSE_TEXT_X: usize = 52;
const COMPOSE_TEXT_MAX_W: usize = 610 - 20 - COMPOSE_TEXT_X;
const COMPOSE_PLACEHOLDER: &[u8] = b"Type a message...";
const COMPOSE_TEXT_SCALE: usize = 2;
const SEND_CX: usize = 662;
const SEND_CY: usize = 1374;
const SEND_R: i32 = 29;

// Home gesture band (y 1418..1424)
const HOME_X0: usize = 240;
const HOME_X1: usize = 480;
const HOME_Y0: usize = 1400;

// Thread card geometry
const CARD_X0: usize = 24;
const CARD_X1: usize = 696;
const CARD_Y0: usize = 180;
const CARD_H: usize = 120;
const CARD_BODY_H: usize = 104;

// Empty-state card
const EMPTY_CARD: (usize, usize, usize, usize) = (24, 560, 672, 340);

#[inline(always)]
fn in_rect(r: (usize, usize, usize, usize), x: usize, y: usize) -> bool {
    x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
}

fn selection_fill() -> u32 {
    const PERCENT: u32 = 38;
    let t = PERCENT * 256 / 100;
    let channel = |c: u32, bg: u32| -> u32 { (c * t + bg * (256 - t)) >> 8 };
    let r = channel((ACCENT_SKY >> 16) & 0xFF, (SURFACE_DARK >> 16) & 0xFF);
    let g = channel((ACCENT_SKY >> 8) & 0xFF, (SURFACE_DARK >> 8) & 0xFF);
    let b = channel(ACCENT_SKY & 0xFF, SURFACE_DARK & 0xFF);
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

struct PillFrame {
    label: &'static [u8],
    scale: usize,
    left: usize,
    text_x: usize,
    text_y: usize,
}

fn build_pill(app: &MessagesApp) -> PillFrame {
    let label = pill_text(app);
    let scale = pill_text_scale(label);
    let left = pill_left_for(label, scale);
    let lh = std::design::font::line_height(scale);
    let cy = (PILL_TOP + PILL_BOTTOM) / 2;
    PillFrame {
        label,
        scale,
        left,
        text_x: left + PILL_PAD_X,
        text_y: cy.saturating_sub(lh / 2),
    }
}

struct EditorFrame {
    buf: [u8; COMPOSE_CAP],
    len: usize,
    x: usize,
    y: usize,
    scale: usize,
    caret_x: usize,
    select_w: usize,
    color: u32,
}

fn build_editor(
    raw: &[u8],
    raw_len: usize,
    placeholder: &[u8],
    scale: usize,
    max_w: usize,
    x: usize,
    y: usize,
    focused: bool,
    select_all: bool,
) -> EditorFrame {
    let mut f = EditorFrame {
        buf: [0; COMPOSE_CAP],
        len: 0,
        x,
        y,
        scale,
        caret_x: x,
        select_w: 0,
        color: TEXT_MUTED,
    };
    if raw_len == 0 {
        f.len = std::design::font::ellipsize(placeholder, scale, max_w, &mut f.buf);
    } else {
        f.color = TEXT_PRIMARY;
        f.len = std::design::font::ellipsize(&raw[..raw_len], scale, max_w, &mut f.buf);
    }
    let w = std::design::font::measure(&f.buf[..f.len], scale);
    f.caret_x = x + w;
    f.select_w = if focused && select_all { w } else { 0 };
    f
}

#[inline(always)]
fn editor_pixel(
    x: usize,
    y: usize,
    f: &EditorFrame,
    focused: bool,
    fill: u32,
) -> Option<u32> {
    let lh = std::design::font::line_height(f.scale);
    if y < f.y || y >= f.y + lh {
        return None;
    }
    let in_sel = f.select_w > 0 && x >= f.x && x < f.x + f.select_w;
    let bg = if in_sel { selection_fill() } else { fill };
    if let Some(px) = test_text(
        x,
        y,
        f.x,
        f.y,
        f.scale,
        &f.buf[..f.len],
        f.color,
        bg,
    ) {
        return Some(px);
    }
    if in_sel {
        return Some(selection_fill());
    }
    if focused && x >= f.caret_x && x < f.caret_x + 2 {
        return Some(ACCENT_SKY);
    }
    None
}

struct Frame {
    pill: PillFrame,
    search: EditorFrame,
    recipient: EditorFrame,
    compose: EditorFrame,
    filtered: Option<usize>,
}

fn build_frame(messages: &MessagesApp) -> Frame {
    let search_y = (SEARCH_FIELD.1 + SEARCH_FIELD.3 / 2)
        .saturating_sub(std::design::font::line_height(SEARCH_TEXT_SCALE) / 2);
    let recipient_y = (RECIPIENT_FIELD.1 + RECIPIENT_FIELD.3 / 2)
        .saturating_sub(std::design::font::line_height(RECIPIENT_TEXT_SCALE) / 2);
    let compose_y = (COMPOSE_FIELD.1 + COMPOSE_FIELD.3 / 2)
        .saturating_sub(std::design::font::line_height(COMPOSE_TEXT_SCALE) / 2);
    let filtered = if messages.search_len > 0 && messages.match_count() == 0 {
        None
    } else {
        Some(messages.active_thread_idx)
    };
    Frame {
        pill: build_pill(messages),
        search: build_editor(
            &messages.search,
            messages.search_len,
            SEARCH_PLACEHOLDER,
            SEARCH_TEXT_SCALE,
            SEARCH_TEXT_MAX_W,
            SEARCH_TEXT_X,
            search_y,
            messages.search_focused,
            messages.search_select_all,
        ),
        recipient: build_editor(
            &messages.recip,
            messages.recip_len,
            RECIPIENT_PLACEHOLDER,
            RECIPIENT_TEXT_SCALE,
            RECIPIENT_TEXT_MAX_W,
            RECIPIENT_TEXT_X,
            recipient_y,
            messages.recip_focused,
            messages.recip_select_all,
        ),
        compose: build_editor(
            &messages.compose,
            messages.compose_len,
            COMPOSE_PLACEHOLDER,
            COMPOSE_TEXT_SCALE,
            COMPOSE_TEXT_MAX_W,
            COMPOSE_TEXT_X,
            compose_y,
            messages.compose_focused,
            messages.compose_select_all,
        ),
        filtered,
    }
}

/// The backdrop colour of a peer's initial disc.
///
/// WHY this is one neutral colour and not a hash of the handle: a colour
/// picked from the handle looks like a contact's chosen identity, and the
/// app has never received a contact photo or a profile colour. The disc
/// shows the peer's real initial against a neutral surface, which is what
/// the app actually knows.
fn avatar_color(_peer: &[u8]) -> u32 {
    0xFF374151 // Slate
}

/// Case-insensitive substring match.
fn contains_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    for i in 0..=(haystack.len() - needle.len()) {
        let mut matched = true;
        for j in 0..needle.len() {
            if haystack[i + j].to_ascii_lowercase() != needle[j].to_ascii_lowercase() {
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

fn thread_matches(tr: &messages::ThreadRecord, query: &[u8]) -> bool {
    contains_ci(tr.peer(), query)
}

/// Master pixel scanout pipeline (zero heap allocations).
#[inline(never)]
fn get_messages_pixel(x: usize, y: usize, messages: &MessagesApp, f: &Frame) -> u32 {
    // 1. Header (y 0..96)
    if y < HEADER_Y1 {
        if y >= 94 {
            return BORDER_SUBTLE;
        }

        // Standardized back button squircle (24..72, 24..72)
        if let Some(c) = std::design::back_button::pixel(x, y, TEXT_PRIMARY, BORDER_SUBTLE, SURFACE_RAISED) {
            return c;
        }

        // Electric red dot accent at (96, 48)
        let dot_dx = x as i32 - 96;
        let dot_dy = y as i32 - 48;
        if dot_dx * dot_dx + dot_dy * dot_dy <= 16 {
            return 0xFFEF4444;
        }

        // Header Title: In ThreadList mode, "Messages"; in Conversation mode, peer handle or "Messages"
        if y >= TITLE_Y && y < TITLE_Y + std::design::font::line_height(TITLE_SCALE) {
            let title = if messages.view_mode == ViewMode::Conversation {
                if let Some(tid) = messages.active_thread_id {
                    if let Some(tr) = messages.ledger.get_thread(tid) {
                        tr.peer()
                    } else {
                        TITLE_TEXT
                    }
                } else {
                    TITLE_TEXT
                }
            } else {
                TITLE_TEXT
            };

            if let Some(c) = test_text_bold(
                x,
                y,
                TITLE_X,
                TITLE_Y,
                TITLE_SCALE,
                title,
                TEXT_PRIMARY,
                SURFACE_DARK,
            ) {
                return c;
            }
        }

        // Status pill, top-right: truthful NO TRANSPORT / outcome
        if x >= f.pill.left && x < PILL_RIGHT && y >= PILL_TOP && y < PILL_BOTTOM {
            let rx = (x - f.pill.left).min(PILL_RIGHT - 1 - x);
            let ry = (y - PILL_TOP).min(PILL_BOTTOM - 1 - y);
            if rx < 20 && ry < 20 {
                let cdx = (20 - rx) as isize;
                let cdy = (20 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 400 {
                    return SURFACE_DARK;
                }
            }
            if let Some(c) = test_text_bold(
                x,
                y,
                f.pill.text_x,
                f.pill.text_y,
                f.pill.scale,
                f.pill.label,
                TEXT_MUTED,
                SURFACE_RAISED,
            ) {
                return c;
            }
            if rx < 2 || ry < 2 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }

        return SURFACE_DARK;
    }

    // -------------------------------------------------------------------------
    // ViewMode 1: ThreadListView
    // -------------------------------------------------------------------------
    if messages.view_mode == ViewMode::ThreadList {
        // 2. Search & Filter Bar (y 104..156)
        if y >= SEARCH_Y0 && y < SEARCH_Y1 {
            if in_rect(SEARCH_FIELD, x, y) {
                let fx = SEARCH_FIELD.0;
                let fy = SEARCH_FIELD.1;
                let rx = (x - fx).min(fx + SEARCH_FIELD.2 - 1 - x);
                let ry = (y - fy).min(fy + SEARCH_FIELD.3 - 1 - y);
                if rx < 22 && ry < 22 {
                    let cdx = (22 - rx) as isize;
                    let cdy = (22 - ry) as isize;
                    if cdx * cdx + cdy * cdy >= 484 {
                        return BG_DARK;
                    }
                }

                // Vector magnifying glass icon at (cx: 56, cy: 130)
                let cx = 56;
                let cy = 130;
                let dx = (x as isize - cx as isize).abs();
                let dy = (y as isize - cy as isize).abs();
                let d_sq = dx * dx + dy * dy;
                if d_sq >= 20 && d_sq <= 48 {
                    return TEXT_MUTED;
                }
                let hx = x as isize - (cx + 5) as isize;
                let hy = y as isize - (cy + 5) as isize;
                if hx >= 0 && hx <= 8 && hy >= 0 && hy <= 8 && (hx - hy).abs() <= 1 {
                    return TEXT_MUTED;
                }

                if let Some(c) = editor_pixel(x, y, &f.search, messages.search_focused, SURFACE_DARK) {
                    return c;
                }

                if rx < 2 || ry < 2 {
                    return BORDER_SUBTLE;
                }
                return SURFACE_DARK;
            }
            return BG_DARK;
        }

        // 3. Thread List / Empty State (y 168..1288)
        if y >= 168 && y < RECIPIENT_BAND_Y0 {
            // Filter matched nothing
            if f.filtered.is_none() {
                if in_rect(EMPTY_CARD, x, y) {
                    let rx = (x - EMPTY_CARD.0).min(EMPTY_CARD.0 + EMPTY_CARD.2 - 1 - x);
                    let ry = (y - EMPTY_CARD.1).min(EMPTY_CARD.1 + EMPTY_CARD.3 - 1 - y);
                    if rx < 24 && ry < 24 {
                        let cdx = (24 - rx) as isize;
                        let cdy = (24 - ry) as isize;
                        if cdx * cdx + cdy * cdy >= 576 {
                            return BG_DARK;
                        }
                    }

                    let line = b"No chats match your search";
                    let sub = &messages.search[..messages.search_len];
                    let mut buf = [0u8; COMPOSE_CAP];
                    let n = std::design::font::ellipsize(sub, 1, 624, &mut buf);
                    let sub_line: &[u8] = if n == 0 { b"(empty query)" } else { &buf[..n] };
                    let line_w = std::design::font::measure_bold(line, 1);
                    let sub_w = std::design::font::measure(sub_line, 1);
                    if let Some(c) = test_text_bold(
                        x,
                        y,
                        EMPTY_CARD.0 + (EMPTY_CARD.2.saturating_sub(line_w)) / 2,
                        676,
                        1,
                        line,
                        TEXT_PRIMARY,
                        SURFACE_DARK,
                    ) {
                        return c;
                    }
                    if let Some(c) = test_text(
                        x,
                        y,
                        EMPTY_CARD.0 + (EMPTY_CARD.2.saturating_sub(sub_w)) / 2,
                        716,
                        1,
                        sub_line,
                        TEXT_MUTED,
                        SURFACE_DARK,
                    ) {
                        return c;
                    }
                    if rx < 2 || ry < 2 {
                        return BORDER_SUBTLE;
                    }
                    return SURFACE_DARK;
                }
                return BG_DARK;
            }

            // Zero conversation threads: Honest Empty State Card
            if messages.ledger.thread_count == 0 {
                if in_rect(EMPTY_CARD, x, y) {
                    let rx = (x - EMPTY_CARD.0).min(EMPTY_CARD.0 + EMPTY_CARD.2 - 1 - x);
                    let ry = (y - EMPTY_CARD.1).min(EMPTY_CARD.1 + EMPTY_CARD.3 - 1 - y);
                    if rx < 24 && ry < 24 {
                        let cdx = (24 - rx) as isize;
                        let cdy = (24 - ry) as isize;
                        if cdx * cdx + cdy * cdy >= 576 {
                            return BG_DARK;
                        }
                    }

                    // Vector Chat Bubble Pictogram at cx: 360, cy: 673
                    let cx = 360;
                    let cy = 673;
                    if x >= cx - 60 && x < cx + 60 && y >= cy - 40 && y < cy + 30 {
                        let bx = (x - (cx - 60)).min((cx + 59) - x);
                        let by = (y - (cy - 40)).min((cy + 29) - y);
                        let in_corner = bx < 18 && by < 18;
                        let corner_out = in_corner && {
                            let cdx = (18 - bx) as isize;
                            let cdy = (18 - by) as isize;
                            cdx * cdx + cdy * cdy >= 324
                        };

                        if !corner_out {
                            if y >= cy - 22 && y < cy - 16 && x >= cx - 40 && x < cx + 28 {
                                return ACCENT_SKY;
                            }
                            if y >= cy - 8 && y < cy - 2 && x >= cx - 40 && x < cx + 16 {
                                return BORDER_SUBTLE;
                            }
                            if y >= cy + 6 && y < cy + 12 && x >= cx - 40 && x < cx - 4 {
                                return BORDER_SUBTLE;
                            }
                            let is_rim = bx < 2 || by < 2 || (in_corner && {
                                let cdx = (18 - bx) as isize;
                                let cdy = (18 - by) as isize;
                                cdx * cdx + cdy * cdy >= 256
                            });
                            if is_rim {
                                return BORDER_SUBTLE;
                            }
                            return SURFACE_RAISED;
                        }
                    }

                    // Speech bubble pointer tail at bottom-left
                    if x >= cx - 44 && x < cx - 18 && y >= cy + 30 && y < cy + 48 {
                        let rel_x = (x - (cx - 44)) as isize;
                        let rel_y = (y - (cy + 30)) as isize;
                        if rel_x >= (rel_y * 24) / 18 {
                            if rel_x == (rel_y * 24) / 18 || x == cx - 19 || y == cy + 47 {
                                return BORDER_SUBTLE;
                            }
                            return SURFACE_RAISED;
                        }
                    }

                    // Strict contract coordinates: 740 and 810
                    if let Some(c) = test_text(
                        x,
                        y,
                        EMPTY_CARD.0 + std::design::font::center_x(b"No Conversations Yet", 2, EMPTY_CARD.2),
                        740,
                        2,
                        b"No Conversations Yet",
                        TEXT_PRIMARY,
                        SURFACE_DARK,
                    ) {
                        return c;
                    }
                    // WHY the subline follows the store's own state: an
                    // unreadable or invalid store is not the same fact as
                    // a device with no conversations, and the old code
                    // showed the same two lines for both.
                    let (subline, subline_color): (&[u8], u32) = match messages.store.state {
                        store::StoreState::ReadRefused => (
                            b"Stored conversations could not be read",
                            0xFFF59E0B,
                        ),
                        store::StoreState::Invalid => (
                            b"Stored conversations failed validation",
                            0xFFEF4444,
                        ),
                        store::StoreState::Full => (
                            b"Conversation store is full; older threads were not kept",
                            0xFFF59E0B,
                        ),
                        _ => (b"Messaging unavailable: no transport", TEXT_MUTED),
                    };
                    if let Some(c) = test_text(
                        x,
                        y,
                        EMPTY_CARD.0 + std::design::font::center_x(subline, 1, EMPTY_CARD.2),
                        810,
                        1,
                        subline,
                        subline_color,
                        SURFACE_DARK,
                    ) {
                        return c;
                    }

                    if rx < 2 || ry < 2 {
                        return BORDER_SUBTLE;
                    }
                    return SURFACE_DARK;
                }
            } else {
                // Render conversation thread rows
                let idx = (y - CARD_Y0) / CARD_H;
                let card_top = CARD_Y0 + idx * CARD_H;
                let card_offset_y = (y - CARD_Y0) % CARD_H;
                if card_offset_y < CARD_BODY_H && x >= CARD_X0 && x < CARD_X1 {
                    if let Some(tr) = messages.ledger.get_thread_by_idx(idx) {
                        if thread_matches(tr, &messages.search[..messages.search_len]) {
                            let rx = (x - CARD_X0).min(CARD_X1 - 1 - x);
                            let ry = card_offset_y.min(CARD_BODY_H - 1 - card_offset_y);
                            if rx < 16 && ry < 16 {
                                let cdx = (16 - rx) as isize;
                                let cdy = (16 - ry) as isize;
                                if cdx * cdx + cdy * cdy >= 256 {
                                    return BG_DARK;
                                }
                            }

                            // 1. Avatar glyph with colored backdrop
                            let acx = CARD_X0 + 44;
                            let acy = card_top + 52;
                            let adx = x as isize - acx as isize;
                            let ady = y as isize - acy as isize;
                            if adx * adx + ady * ady <= 576 {
                                let initial = if tr.handle_len > 0 { tr.peer_handle[0] } else { b'?' };
                                let glyph = [initial];
                                if let Some(c) = test_text_bold(
                                    x,
                                    y,
                                    acx.saturating_sub(7),
                                    acy.saturating_sub(12),
                                    1,
                                    &glyph,
                                    0xFFFFFFFF,
                                    avatar_color(tr.peer()),
                                ) {
                                    return c;
                                }
                                return avatar_color(tr.peer());
                            }

                            // 2. Peer handle / name (Scale 1 Bold with 10px spacing above snippet)
                            if let Some(c) = test_text_bold(
                                x,
                                y,
                                CARD_X0 + 84,
                                card_top + 22,
                                1,
                                tr.peer(),
                                TEXT_PRIMARY,
                                SURFACE_DARK,
                            ) {
                                return c;
                            }

                            // 3. Last message preview
                            let mut snippet = [0u8; 48];
                            let slen = if let Some(last_msg) = messages.ledger.latest_message(tr.id) {
                                std::design::font::ellipsize(
                                    last_msg.text(),
                                    1,
                                    CARD_X1 - CARD_X0 - 180,
                                    &mut snippet,
                                )
                            } else {
                                let placeholder = b"(no messages)";
                                copy_ascii(placeholder, &mut snippet)
                            };

                            if let Some(c) = test_text(
                                x,
                                y,
                                CARD_X0 + 84,
                                card_top + 56,
                                1,
                                &snippet[..slen],
                                TEXT_MUTED,
                                SURFACE_DARK,
                            ) {
                                return c;
                            }

                            // 4. Security indicator badge dot
                            let threat = gsm::analyze_threat(&snippet[..slen]);
                            let s_dx = x as isize - (CARD_X0 + 20) as isize;
                            let s_dy = y as isize - (card_top + 24) as isize;
                            if s_dx * s_dx + s_dy * s_dy <= 16 {
                                return threat.level.color();
                            }

                            // 5. Unread counter pill (if unread_count > 0)
                            if tr.unread_count > 0 {
                                let px0 = CARD_X1 - 92;
                                let px1 = CARD_X1 - 56;
                                let py0 = card_top + 34;
                                let py1 = card_top + 68;
                                if x >= px0 && x < px1 && y >= py0 && y < py1 {
                                    let mut digit = [0u8; 4];
                                    let dn = put_u8(&mut digit, tr.unread_count);
                                    if let Some(c) = test_text(
                                        x,
                                        y,
                                        px0 + 12,
                                        py0 + 5,
                                        1,
                                        &digit[..dn],
                                        0xFFFFFFFF,
                                        ACCENT_SKY,
                                    ) {
                                        return c;
                                    }
                                    return ACCENT_SKY;
                                }
                            }

                            // 6. Delete affordance 'x' glyph
                            let dxc = CARD_X1 - 38;
                            let dyc = card_top + 50;
                            let gx = x as isize - dxc as isize;
                            let gy = y as isize - dyc as isize;
                            if gx.abs() <= 6 && gy.abs() <= 6 && (gx - gy).abs() <= 2 {
                                return ACCENT_SKY;
                            }

                            if rx < 2 || ry < 2 {
                                return BORDER_SUBTLE;
                            }
                            return SURFACE_DARK;
                        }
                    }
                }
            }
            return BG_DARK;
        }

        // 3b. Recipient editor band (y 1288..1330)
        if y >= RECIPIENT_BAND_Y0 && y < RECIPIENT_BAND_Y1 {
            if in_rect(RECIPIENT_FIELD, x, y) {
                let fx = RECIPIENT_FIELD.0;
                let fy = RECIPIENT_FIELD.1;
                let rx = (x - fx).min(fx + RECIPIENT_FIELD.2 - 1 - x);
                let ry = (y - fy).min(fy + RECIPIENT_FIELD.3 - 1 - y);
                if rx < 18 && ry < 18 {
                    let cdx = (18 - rx) as isize;
                    let cdy = (18 - ry) as isize;
                    if cdx * cdx + cdy * cdy >= 324 {
                        return BG_DARK;
                    }
                }
                if let Some(c) = test_text(x, y, 40, f.recipient.y, 1, b"To:", TEXT_MUTED, SURFACE_DARK) {
                    return c;
                }
                if let Some(c) = editor_pixel(x, y, &f.recipient, messages.recip_focused, SURFACE_DARK) {
                    return c;
                }

                // Character count indicator (e.g. 0/160)
                let mut count_buf = [0u8; 8];
                let cn = format_char_count(messages.compose_len, &mut count_buf);
                if let Some(c) = test_text(x, y, 530, f.recipient.y, 1, &count_buf[..cn], TEXT_MUTED, SURFACE_DARK) {
                    return c;
                }

                if rx < 2 || ry < 2 {
                    return BORDER_SUBTLE;
                }
                return SURFACE_DARK;
            }
            return BG_DARK;
        }

        // 4. Compose Input Bar (y 1330..1414)
        if y >= COMPOSE_BAND_Y0 && y < COMPOSE_BAND_Y1 {
            if y == COMPOSE_BAND_Y0 {
                return BORDER_SUBTLE;
            }
            if in_rect(COMPOSE_FIELD, x, y) {
                let fx = COMPOSE_FIELD.0;
                let fy = COMPOSE_FIELD.1;
                let rx = (x - fx).min(fx + COMPOSE_FIELD.2 - 1 - x);
                let ry = (y - fy).min(fy + COMPOSE_FIELD.3 - 1 - y);
                if rx < 24 && ry < 24 {
                    let cdx = (24 - rx) as isize;
                    let cdy = (24 - ry) as isize;
                    if cdx * cdx + cdy * cdy >= 576 {
                        return BG_DARK;
                    }
                }
                if let Some(c) = editor_pixel(x, y, &f.compose, messages.compose_focused, SURFACE_DARK) {
                    return c;
                }
                if rx < 2 || ry < 2 {
                    return BORDER_SUBTLE;
                }
                return SURFACE_DARK;
            }

            // Send control circle
            if x >= 630 && x < 694 && y >= 1344 && y < 1404 {
                let dx = (x as isize) - SEND_CX as isize;
                let dy = (y as isize) - SEND_CY as isize;
                if dx * dx + dy * dy <= (SEND_R * SEND_R) as isize {
                    let live = messages.compose_len > 0;
                    let bg = if live { ACCENT_SKY } else { DISABLED_FILL };
                    let ink = if live { 0xFFFFFFFF } else { TEXT_MUTED };
                    let rel_x = dx;
                    let rel_y = dy;
                    let is_arrow = (rel_y.abs() <= 10 && rel_x >= -8 + rel_y.abs() && rel_x <= -4 + rel_y.abs())
                        || (rel_y.abs() <= 2 && rel_x >= -12 && rel_x <= 6);
                    if is_arrow {
                        return ink;
                    }
                    return bg;
                }
                return BG_DARK;
            }
            return BG_DARK;
        }
    } else {
        // ---------------------------------------------------------------------
        // ViewMode 2: ConversationView
        // ---------------------------------------------------------------------

        // Threat Warning Banner or clean header info bar (y 104..156)
        if y >= SEARCH_Y0 && y < SEARCH_Y1 {
            let active_id = messages.active_thread_id.unwrap_or(0);
            let mut msgs = [MessageRecord::EMPTY; messages::MAX_MESSAGES_PER_THREAD];
            let count = messages.ledger.get_thread_messages(active_id, &mut msgs);

            // Check if thread contains any flagged urgent or phishing messages
            let mut flagged_urgency = false;
            let mut flagged_phishing = false;
            for i in 0..count {
                let assessment = gsm::analyze_threat(msgs[i].text());
                if assessment.level == gsm::ThreatLevel::HighRisk {
                    flagged_phishing = true;
                    break;
                } else if assessment.level == gsm::ThreatLevel::Suspicious {
                    flagged_urgency = true;
                }
            }

            let show_threat = flagged_phishing || flagged_urgency;
            // WHY the two severities read differently and why neither calls
            // the sender unverified: the app verified nothing. What it has
            // is a local pattern match on the message text, and the banner
            // says that rather than making a claim about the sender.
            let (bar_bg, bar_border, bar_text) = if flagged_phishing {
                (
                    0xFF381010,
                    0xFFEF4444,
                    b"Caution: text matches a known scam script" as &[u8],
                )
            } else if flagged_urgency {
                (
                    0xFF2E1F08,
                    0xFFF59E0B,
                    b"Caution: text matches urgency language" as &[u8],
                )
            } else {
                // WHY this is no longer "Verified": the banner sat next to
                // a refusal status and a "no transport" state, and no
                // handshake, key exchange or peer contact ever happened.
                // The thread is local and unsent; that is what it can say.
                (SURFACE_DARK, BORDER_SUBTLE, b"Local thread - not delivered" as &[u8])
            };

            if in_rect(SEARCH_FIELD, x, y) {
                let fx = SEARCH_FIELD.0;
                let fy = SEARCH_FIELD.1;
                let rx = (x - fx).min(fx + SEARCH_FIELD.2 - 1 - x);
                let ry = (y - fy).min(fy + SEARCH_FIELD.3 - 1 - y);
                if rx < 18 && ry < 18 {
                    let cdx = (18 - rx) as isize;
                    let cdy = (18 - ry) as isize;
                    if cdx * cdx + cdy * cdy >= 324 {
                        return BG_DARK;
                    }
                }

                if show_threat {
                    // Warning icon at cx: 56, cy: 130
                    let cx = 56;
                    let cy = 130;
                    let dx = (x as isize - cx as isize).abs();
                    let dy = (y as isize - cy as isize).abs();
                    if dx <= 2 && dy <= 8 {
                        return bar_border;
                    }
                    if dx <= 2 && dy >= 10 && dy <= 12 {
                        return bar_border;
                    }
                }

                let text_x = if show_threat { 76 } else { 40 };
                let text_y = (SEARCH_FIELD.1 + SEARCH_FIELD.3 / 2).saturating_sub(12);
                if let Some(c) = test_text(
                    x,
                    y,
                    text_x,
                    text_y,
                    1,
                    bar_text,
                    if show_threat { 0xFFFFFFFF } else { TEXT_MUTED },
                    bar_bg,
                ) {
                    return c;
                }

                if rx < 2 || ry < 2 {
                    return bar_border;
                }
                return bar_bg;
            }
            return BG_DARK;
        }

        // Chat Message Bubbles Area (y 168..1280)
        if y >= 168 && y < RECIPIENT_BAND_Y0 {
            let active_id = messages.active_thread_id.unwrap_or(0);
            let mut msgs = [MessageRecord::EMPTY; messages::MAX_MESSAGES_PER_THREAD];
            let count = messages.ledger.get_thread_messages(active_id, &mut msgs);

            if count == 0 {
                // Empty conversation info banner
                let line1 = b"No messages in this chat yet";
                let line2 = b"Type a draft below to compose";
                let l1_w = std::design::font::measure_bold(line1, 1);
                let l2_w = std::design::font::measure(line2, 1);
                if let Some(c) = test_text_bold(x, y, (W.saturating_sub(l1_w)) / 2, 676, 1, line1, TEXT_PRIMARY, BG_DARK) {
                    return c;
                }
                if let Some(c) = test_text(x, y, (W.saturating_sub(l2_w)) / 2, 716, 1, line2, TEXT_MUTED, BG_DARK) {
                    return c;
                }
            } else {
                let bubble_h = 76usize;
                let bubble_gap = 14usize;
                let step = bubble_h + bubble_gap;
                let total_h = count * step;
                let start_y = if total_h < (RECIPIENT_BAND_Y0 - 180) {
                    RECIPIENT_BAND_Y0 - total_h - 16
                } else {
                    180
                };

                for i in 0..count {
                    let b_top = start_y + i * step;
                    let b_bot = b_top + bubble_h;
                    if y >= b_top && y < b_bot {
                        let is_incoming = msgs[i].direction == MessageDirection::Incoming;
                        let (bx0, bx1, fill_color, border_color) = if is_incoming {
                            (32usize, 520usize, SURFACE_RAISED, BORDER_SUBTLE)
                        } else {
                            (200usize, 688usize, 0xFF0E382B, 0xFF10B981)
                        };

                        if x >= bx0 && x < bx1 {
                            let rx = (x - bx0).min(bx1 - 1 - x);
                            let ry = (y - b_top).min(b_bot - 1 - y);
                            if rx < 16 && ry < 16 {
                                let cdx = (16 - rx) as isize;
                                let cdy = (16 - ry) as isize;
                                if cdx * cdx + cdy * cdy >= 256 {
                                    return BG_DARK;
                                }
                            }

                            // Message body text (scale 1)
                            let mut buf = [0u8; 96];
                            let n = std::design::font::ellipsize(
                                msgs[i].text(),
                                1,
                                bx1 - bx0 - 32,
                                &mut buf,
                            );
                            if let Some(c) = test_text(
                                x,
                                y,
                                bx0 + 16,
                                b_top + 14,
                                1,
                                &buf[..n],
                                TEXT_PRIMARY,
                                fill_color,
                            ) {
                                return c;
                            }

                            // Subline / Status indicator
                            let (status_label, status_color) = if is_incoming {
                                // WHY this does not name a transport: the
                                // record carries a direction flag, not the
                                // channel it arrived on. Nothing in this app
                                // received an SMS -- there is no transport
                                // bound -- so naming one would be a claim
                                // the app cannot support.
                                (b"Received (no transport)" as &[u8], TEXT_MUTED)
                            } else {
                                // WHY every state is named rather than
                                // colour-coded: "Sent" has to mean the
                                // transport took the message, "Not sent" has
                                // to mean nobody did, and a state with no
                                // transport behind it has to say so.
                                match msgs[i].status {
                                    DeliveryStatus::RefusedNoTransport => {
                                        (b"Refused - No Transport" as &[u8], 0xFFF59E0B)
                                    }
                                    DeliveryStatus::Sent => (b"Sent" as &[u8], 0xFF10B981),
                                    DeliveryStatus::Failed => (b"Failed" as &[u8], 0xFFEF4444),
                                    DeliveryStatus::LocalOnly => {
                                        (b"Local only - not sent" as &[u8], TEXT_MUTED)
                                    }
                                    DeliveryStatus::Pending => (b"Not sent" as &[u8], TEXT_MUTED),
                                }
                            };

                            if let Some(c) = test_text(
                                x,
                                y,
                                bx0 + 16,
                                b_top + 44,
                                1,
                                status_label,
                                status_color,
                                fill_color,
                            ) {
                                return c;
                            }

                            if rx < 2 || ry < 2 {
                                return border_color;
                            }
                            return fill_color;
                        }
                    }
                }
            }
            return BG_DARK;
        }

        // Draft Bar: Recipient row (y 1288..1330)
        if y >= RECIPIENT_BAND_Y0 && y < RECIPIENT_BAND_Y1 {
            if in_rect(RECIPIENT_FIELD, x, y) {
                let fx = RECIPIENT_FIELD.0;
                let fy = RECIPIENT_FIELD.1;
                let rx = (x - fx).min(fx + RECIPIENT_FIELD.2 - 1 - x);
                let ry = (y - fy).min(fy + RECIPIENT_FIELD.3 - 1 - y);
                if rx < 18 && ry < 18 {
                    let cdx = (18 - rx) as isize;
                    let cdy = (18 - ry) as isize;
                    if cdx * cdx + cdy * cdy >= 324 {
                        return BG_DARK;
                    }
                }
                if let Some(c) = test_text(x, y, 40, f.recipient.y, 1, b"To:", TEXT_MUTED, SURFACE_DARK) {
                    return c;
                }
                if let Some(c) = editor_pixel(x, y, &f.recipient, messages.recip_focused, SURFACE_DARK) {
                    return c;
                }

                // Character count indicator (e.g. 0/160)
                let mut count_buf = [0u8; 8];
                let cn = format_char_count(messages.compose_len, &mut count_buf);
                if let Some(c) = test_text(x, y, 530, f.recipient.y, 1, &count_buf[..cn], TEXT_MUTED, SURFACE_DARK) {
                    return c;
                }

                if rx < 2 || ry < 2 {
                    return BORDER_SUBTLE;
                }
                return SURFACE_DARK;
            }
            return BG_DARK;
        }

        // Compose input field (y 1330..1414)
        if y >= COMPOSE_BAND_Y0 && y < COMPOSE_BAND_Y1 {
            if y == COMPOSE_BAND_Y0 {
                return BORDER_SUBTLE;
            }
            if in_rect(COMPOSE_FIELD, x, y) {
                let fx = COMPOSE_FIELD.0;
                let fy = COMPOSE_FIELD.1;
                let rx = (x - fx).min(fx + COMPOSE_FIELD.2 - 1 - x);
                let ry = (y - fy).min(fy + COMPOSE_FIELD.3 - 1 - y);
                if rx < 24 && ry < 24 {
                    let cdx = (24 - rx) as isize;
                    let cdy = (24 - ry) as isize;
                    if cdx * cdx + cdy * cdy >= 576 {
                        return BG_DARK;
                    }
                }
                if let Some(c) = editor_pixel(x, y, &f.compose, messages.compose_focused, SURFACE_DARK) {
                    return c;
                }
                if rx < 2 || ry < 2 {
                    return BORDER_SUBTLE;
                }
                return SURFACE_DARK;
            }

            // Send control circle
            if x >= 630 && x < 694 && y >= 1344 && y < 1404 {
                let dx = (x as isize) - SEND_CX as isize;
                let dy = (y as isize) - SEND_CY as isize;
                if dx * dx + dy * dy <= (SEND_R * SEND_R) as isize {
                    let live = messages.compose_len > 0;
                    let bg = if live { ACCENT_SKY } else { DISABLED_FILL };
                    let ink = if live { 0xFFFFFFFF } else { TEXT_MUTED };
                    let rel_x = dx;
                    let rel_y = dy;
                    let is_arrow = (rel_y.abs() <= 10 && rel_x >= -8 + rel_y.abs() && rel_x <= -4 + rel_y.abs())
                        || (rel_y.abs() <= 2 && rel_x >= -12 && rel_x <= 6);
                    if is_arrow {
                        return ink;
                    }
                    return bg;
                }
                return BG_DARK;
            }
            return BG_DARK;
        }
    }

    // 5. Home Gesture Pill (x: 240..480, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= HOME_X0 && x < HOME_X1 {
        return TEXT_MUTED;
    }

    BG_DARK
}

/// Formats the character count as `used/limit`.
///
/// WHY the limit is written from the constant and not typed in: the compose
/// buffer's capacity is the limit the app actually enforces, and a typed-in
/// denominator drifts away from it the first time the capacity changes.
fn format_char_count(len: usize, out: &mut [u8; 8]) -> usize {
    let mut tmp = [0u8; 4];
    let mut n = 0usize;
    let mut x = len.min(COMPOSE_CAP);
    loop {
        tmp[n] = b'0' + (x % 10) as u8;
        x /= 10;
        n += 1;
        if x == 0 {
            break;
        }
    }
    let mut o = 0usize;
    for i in 0..n {
        out[o] = tmp[n - 1 - i];
        o += 1;
    }
    out[o] = b'/';
    o += 1;
    let mut limit = [0u8; 4];
    let mut ln = 0usize;
    let mut lv = COMPOSE_CAP;
    loop {
        limit[ln] = b'0' + (lv % 10) as u8;
        lv /= 10;
        ln += 1;
        if lv == 0 {
            break;
        }
    }
    for i in 0..ln {
        out[o] = limit[ln - 1 - i];
        o += 1;
    }
    o
}

fn render_messages_ui(surface: u32, messages: &MessagesApp) {
    render_messages_band(surface, messages, HEADER_Y0, H);
}

fn render_messages_band(surface: u32, messages: &MessagesApp, y0: usize, y1: usize) {
    let frame = build_frame(messages);
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_messages_pixel(x, y, messages, &frame);
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
// Input Handling
// -----------------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    None,
    Search,
    Recipient,
    Compose,
    All,
}

fn merge(a: Dirty, b: Dirty) -> Dirty {
    if a == Dirty::None {
        return b;
    }
    if b == Dirty::None || a == b {
        return a;
    }
    Dirty::All
}

enum Tap<'a> {
    None,
    Redraw(Dirty),
    Leave(&'a str),
}

fn focus_compose(app: &mut MessagesApp) {
    if !app.compose_focused {
        app.compose_select_all = app.compose_len > 0;
    }
    app.compose_focused = true;
    app.search_focused = false;
    app.search_select_all = false;
    app.recip_focused = false;
    app.recip_select_all = false;
}

fn focus_search(app: &mut MessagesApp) {
    if !app.search_focused {
        app.search_select_all = app.search_len > 0;
    }
    app.search_focused = true;
    app.compose_focused = false;
    app.compose_select_all = false;
    app.recip_focused = false;
    app.recip_select_all = false;
}

fn focus_recipient(app: &mut MessagesApp) {
    if !app.recip_focused {
        app.recip_select_all = app.recip_len > 0;
    }
    app.recip_focused = true;
    app.compose_focused = false;
    app.compose_select_all = false;
    app.search_focused = false;
    app.search_select_all = false;
}

fn edit_recipient(app: &mut MessagesApp, push: Option<u8>) -> Dirty {
    match push {
        Some(b) => {
            if app.recip_select_all {
                app.recip_len = 0;
                app.recip_select_all = false;
            }
            if app.recip_len < RECIP_CAP {
                app.recip[app.recip_len] = b;
                app.recip_len += 1;
            }
        }
        None => {
            if app.recip_select_all {
                app.recip_len = 0;
                app.recip_select_all = false;
            } else {
                app.recip_len = app.recip_len.saturating_sub(1);
            }
        }
    }
    Dirty::Recipient
}

/// What the transport did with a composed message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransportOutcome {
    /// The transport accepted the message.
    Accepted,
    /// The transport refused the message; the code is the platform's.
    Refused(i64),
    /// The recipient is a local handle, not a dialable number, so no
    /// transport applies and none was asked.
    NotDialable,
}

/// Whether a recipient is something a telephone could be dialled with.
///
/// WHY this is checked: the recipient field accepts a name or a handle as
/// well as a number. Handing a name to a modem is not a transmission, it is
/// a mistake, and the message has to stay local instead.
fn is_dialable(recipient: &[u8]) -> bool {
    messages::is_dialable(recipient)
}

/// Hands the composed message to the transport and reports what it said.
///
/// WHY the app asks the transport instead of deciding: the delivery state
/// used to be derived from a hardware-presence query, so a modem that was
/// present produced "Sent" without a single message ever being handed to
/// it. A delivery state has to come from the transport that carries the
/// message, or it does not get drawn.
fn attempt_transport(recipient: &[u8], body: &[u8]) -> TransportOutcome {
    if !is_dialable(recipient) {
        return TransportOutcome::NotDialable;
    }
    let dest = match core::str::from_utf8(recipient) {
        Ok(s) => s,
        Err(_) => return TransportOutcome::NotDialable,
    };
    let text = match core::str::from_utf8(body) {
        Ok(s) => s,
        Err(_) => "",
    };
    match std::telephony_send_sms(dest, text) {
        Ok(_) => TransportOutcome::Accepted,
        Err(code) => {
            std::log("[MESSAGES APP] Telephony refused the composed message; it was not transmitted.",
            )
            .ok();
            TransportOutcome::Refused(code)
        }
    }
}

/// Sends the composed message, packing via GSM-7, attempting dispatch,
/// recording the transport's own answer, appending to ThreadLedger and
/// persistent ThreadStore.
fn send_composed(app: &mut MessagesApp) -> Dirty {
    if app.compose_len == 0 {
        return Dirty::None;
    }
    if app.recip_len == 0 {
        std::log("[MESSAGES APP] Send pressed with no recipient; nothing was stored locally.",
        )
        .ok();
        app.header_status = HeaderStatus::NoRecipient;
        return Dirty::None;
    }

    let mut body = [0u8; COMPOSE_CAP];
    let n = copy_ascii(&app.compose[..app.compose_len], &mut body);
    let mut recip = [0u8; RECIP_CAP];
    let rn = copy_ascii(&app.recip[..app.recip_len], &mut recip);

    // 1. Pack with GSM-7 septet encoder
    let mut gsm_packed = [0u8; 140];
    let _packed_len = gsm::pack_gsm7_septets(&body[..n], &mut gsm_packed);

    // 2. Hand it to the transport and keep the transport's answer.
    let delivery_status = match attempt_transport(&recip[..rn], &body[..n]) {
        TransportOutcome::Accepted => DeliveryStatus::Sent,
        TransportOutcome::Refused(_) => DeliveryStatus::RefusedNoTransport,
        TransportOutcome::NotDialable => DeliveryStatus::LocalOnly,
    };

    // 3. Post to ThreadLedger
    if let Some(tid) = app.ledger.create_or_get_thread(&recip[..rn]) {
        app.active_thread_id = Some(tid);
        app.ledger.post_message_with_status(tid, MessageDirection::Outgoing, &body[..n], delivery_status);
    }

    // 4. Seal into vault sector 202
    let sealed = app.seal_composed(&body[..n]);
    let _ = app.play_bubble_chime();

    // 5. Commit to persistent settings store
    if sealed {
        app.total_sent += 1;
        // WHY the result is checked: the store is bounded by the settings
        // kernel's 512-byte value cap, so an append past the cap is refused.
        // The old code threw the answer away, so a thread that could not be
        // persisted looked exactly like one that could.
        if app.store.append(&recip[..rn], &body[..n]) {
            app.header_status = HeaderStatus::SealedLocally;
        } else {
            app.header_status = HeaderStatus::StoreRefused;
            std::log("[MESSAGES APP] The thread store refused the append; the message was not persisted.",
            )
            .ok();
        }
    }

    // 6. Transition to conversation view
    app.view_mode = ViewMode::Conversation;
    app.compose_len = 0;
    app.compose_focused = false;
    app.compose_select_all = false;
    app.recip_focused = false;
    app.recip_select_all = false;

    Dirty::All
}

fn handle_tap(app: &mut MessagesApp, px: u32, py: u32) -> Tap<'static> {
    let (x, y) = (px as usize, py as usize);

    // System home gesture: leave to launcher
    if x >= HOME_X0 && x < HOME_X1 && y >= HOME_Y0 {
        return Tap::Leave("[MESSAGES APP] Home gesture pressed. Exiting to launcher.");
    }

    // Header Back button (24..72, 24..72)
    if std::design::back_button::contains(x, y) || (x >= 24 && x < 100 && y >= 28 && y < 68) {
        if app.view_mode == ViewMode::Conversation {
            // Return to conversation directory
            app.view_mode = ViewMode::ThreadList;
            app.compose_focused = false;
            app.compose_select_all = false;
            std::log("[MESSAGES APP] Back pressed: Navigating to ThreadListView.").ok();
            return Tap::Redraw(Dirty::All);
        } else {
            return Tap::Leave("[MESSAGES APP] Back control pressed. Exiting to launcher.");
        }
    }

    // Send button circle
    let sdx = x as isize - SEND_CX as isize;
    let sdy = y as isize - SEND_CY as isize;
    if sdx * sdx + sdy * sdy <= (SEND_R * SEND_R) as isize {
        if app.compose_len == 0 {
            std::log("[MESSAGES APP] Send pressed with an empty composer; nothing was sealed.",
            )
            .ok();
            return Tap::Redraw(Dirty::None);
        }
        let dirty = send_composed(app);
        std::log("[MESSAGES APP] Message composed and processed.").ok();
        return Tap::Redraw(dirty);
    }

    // Composer text field
    if in_rect(COMPOSE_FIELD, x, y) {
        focus_compose(app);
        let _ = std::audio_haptic_click();
        std::log("[MESSAGES APP] Compose field focused for keyboard input.").ok();
        return Tap::Redraw(Dirty::All);
    }

    // Recipient text field
    if in_rect(RECIPIENT_FIELD, x, y) {
        focus_recipient(app);
        let _ = std::audio_haptic_click();
        std::log("[MESSAGES APP] Recipient field focused for keyboard input.").ok();
        return Tap::Redraw(Dirty::All);
    }

    // ThreadListView specific touches
    if app.view_mode == ViewMode::ThreadList {
        // Search field
        if in_rect(SEARCH_FIELD, x, y) {
            focus_search(app);
            let _ = std::audio_haptic_click();
            std::log("[MESSAGES APP] Search field focused for keyboard input.").ok();
            return Tap::Redraw(Dirty::All);
        }

        // Tapping a conversation row opens ConversationView
        if x >= CARD_X0 && x < CARD_X1 && y >= CARD_Y0 {
            let idx = (y - CARD_Y0) / CARD_H;
            let coff = (y - CARD_Y0) % CARD_H;
            if coff < CARD_BODY_H {
                if let Some(tr) = app.ledger.get_thread_by_idx(idx) {
                    if thread_matches(tr, &app.search[..app.search_len]) {
                        // Delete affordance tap on card's right edge
                        let ddx = x as isize - (CARD_X1 - 38) as isize;
                        let ddy = coff as isize - 50;
                        if ddx.abs() <= 16 && ddy.abs() <= 16 {
                            let tid = tr.id;
                            app.ledger.remove_thread(tid);
                            app.store.remove(idx);
                            std::log("[MESSAGES APP] Thread deleted.").ok();
                            return Tap::Redraw(Dirty::All);
                        }

                        // Open the conversation!
                        let tid = tr.id;
                        app.active_thread_id = Some(tid);
                        app.active_thread_idx = idx;
                        app.recip = [0u8; RECIP_CAP];
                        let r = tr.peer();
                        let n = r.len().min(RECIP_CAP);
                        app.recip[..n].copy_from_slice(&r[..n]);
                        app.recip_len = n;
                        app.ledger.mark_thread_read(tid);
                        app.view_mode = ViewMode::Conversation;
                        let _ = std::audio_haptic_click();
                        std::log("[MESSAGES APP] Conversation opened.").ok();
                        return Tap::Redraw(Dirty::All);
                    }
                }
            }
        }
    }

    Tap::None
}

fn handle_messages_key(app: &mut MessagesApp, key: std::InputKeyEvent) -> Dirty {
    use std::input_key_code;
    if !key.pressed {
        return Dirty::None;
    }
    // Escape defocuses or exits conversation view
    if key.code == input_key_code::ESCAPE {
        if app.compose_focused || app.search_focused || app.recip_focused {
            app.compose_focused = false;
            app.compose_select_all = false;
            app.search_focused = false;
            app.search_select_all = false;
            app.recip_focused = false;
            app.recip_select_all = false;
            return Dirty::All;
        } else if app.view_mode == ViewMode::Conversation {
            app.view_mode = ViewMode::ThreadList;
            return Dirty::All;
        }
        return Dirty::None;
    }

    if !app.compose_focused && !app.search_focused && !app.recip_focused {
        return Dirty::None;
    }

    if app.recip_focused {
        return match key.code {
            input_key_code::BACKSPACE => edit_recipient(app, None),
            input_key_code::ENTER => {
                focus_compose(app);
                Dirty::All
            }
            code if (32..=126).contains(&code) && app.recip_len < RECIP_CAP => {
                edit_recipient(app, Some(code as u8))
            }
            _ => Dirty::None,
        };
    }

    if app.compose_focused {
        return match key.code {
            input_key_code::BACKSPACE => {
                if app.compose_select_all {
                    app.compose_len = 0;
                    app.compose_select_all = false;
                } else {
                    app.compose_len = app.compose_len.saturating_sub(1);
                }
                Dirty::Compose
            }
            input_key_code::ENTER => send_composed(app),
            code if (32..=126).contains(&code) && app.compose_len < COMPOSE_CAP => {
                if app.compose_select_all {
                    app.compose_len = 0;
                    app.compose_select_all = false;
                }
                app.compose[app.compose_len] = code as u8;
                app.compose_len += 1;
                Dirty::Compose
            }
            _ => Dirty::None,
        };
    }

    // Search field
    match key.code {
        input_key_code::BACKSPACE => edit_search(app, None),
        code if (32..=126).contains(&code) && app.search_len < SEARCH_CAP => {
            edit_search(app, Some(code as u8))
        }
        _ => Dirty::None,
    }
}

fn edit_search(app: &mut MessagesApp, push: Option<u8>) -> Dirty {
    let before = app.match_count();
    match push {
        Some(b) => {
            if app.search_select_all {
                app.search_len = 0;
                app.search_select_all = false;
            }
            app.search[app.search_len] = b;
            app.search_len += 1;
        }
        None => {
            if app.search_select_all {
                app.search_len = 0;
                app.search_select_all = false;
            } else {
                app.search_len = app.search_len.saturating_sub(1);
            }
        }
    }
    let after = app.match_count();
    if before != after {
        Dirty::All
    } else {
        Dirty::Search
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // WHY this does not announce a wire protocol: the app binds no
    // transport until one is proven to exist, and the banner it does draw
    // says the thread is local and undelivered. A startup line claiming a
    // P2P wire protocol was online was a claim about a channel the app
    // never had.
    std::log("[MESSAGES APP (Ring 3)] Started; no messaging transport is bound.",
    )
    .ok();

    let mut messages = MessagesApp::new();

    // Load persisted threads and synchronize to ThreadLedger
    messages.store = store::ThreadStore::load();
    messages.sync_store_to_ledger();

    // A recipient handed over by the dialer, if there is one.
    messages.take_handoff_recipient();

    let load_note: &str = match messages.store.state {
        store::StoreState::Empty => "Thread store loaded: no threads are stored yet.",
        store::StoreState::Loaded => "Thread store loaded from the settings store.",
        store::StoreState::ReadRefused => "Settings read refused; no stored threads were loaded.",
        store::StoreState::Invalid => "Stored thread data failed validation; no threads were loaded.",
        _ => "",
    };
    if !load_note.is_empty() {
        std::log(load_note).ok();
    }

    // 1. Ingest Hardware Keystore Key
    messages.derive_messaging_key();

    // 2. Allocate DRM/KMS Client Surface (z=200)
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(id) => {
            std::log("[MESSAGES APP] DRM/KMS Client Surface allocated (z=200).",
            )
            .ok();
            id
        }
        Err(_) => 0,
    };

    // 3. Telephony Cellular Modem HAL Query
    let mut modem_buf = [0u8; 256];
    match std::telephony_query(&mut modem_buf) {
        Ok(modem_len) if modem_len > 0 => {
            let present = core::str::from_utf8(&modem_buf[..modem_len])
                .map(|s| s.contains("\"present\":true"))
                .unwrap_or(false);
            if present {
                messages.modem_present = true;
                std::log("[MESSAGES APP] Telephony reported a present modem; a transport may be bound.",
                )
                .ok();
            } else {
                std::log("[MESSAGES APP] Telephony telemetry did not report a present modem; no cellular channel is in use.",
                )
                .ok();
            }
        }
        Ok(_) => {
            std::log("[MESSAGES APP] Telephony query returned an empty payload; no cellular modem reported.",
            )
            .ok();
        }
        Err(_) => {
            std::log("[MESSAGES APP] Telephony query refused by the kernel; no cellular modem is attached.",
            )
            .ok();
        }
    }

    if surface > 0 {
        render_messages_ui(surface, &messages);
        let _ = std::surface_flip();
        std::log("[MESSAGES APP] Messages canvas presented to scanout buffer.",
        )
        .ok();
    }

    // 4. Sovereign Asynchronous Event Reactor Loop
    let interest_mask =
        std::EVENT_TOUCH_INPUT | std::EVENT_IPC_MSG | std::EVENT_MODEM_URC | std::EVENT_TIMER_TICK;
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
                            let changed = handle_messages_key(&mut messages, key);
                            dirty = merge(dirty, changed);
                            continue;
                        }
                        if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                            if input.kind == 6 && messages.view_mode == ViewMode::Conversation {
                                // Back gesture returns to ThreadListView
                                messages.view_mode = ViewMode::ThreadList;
                                dirty = merge(dirty, Dirty::All);
                                continue;
                            }
                            std::log("[MESSAGES APP] Navigation gesture recognized. Exiting to launcher.").ok();
                            if surface > 0 {
                                let _ = std::surface_destroy(surface);
                            }
                            std::exit_with_code(0);
                        }
                        if input.kind == 1 {
                            match handle_tap(&mut messages, input.x, input.y) {
                                Tap::Leave(msg) => {
                                    std::log(msg).ok();
                                    if surface > 0 {
                                        let _ = std::surface_destroy(surface);
                                    }
                                    std::exit_with_code(0);
                                }
                                Tap::Redraw(d) => dirty = merge(dirty, d),
                                Tap::None => {}
                            }
                        }
                    }
                    if dirty != Dirty::None && surface > 0 {
                        match dirty {
                            Dirty::None => {}
                            Dirty::Search => {
                                render_messages_band(surface, &messages, SEARCH_Y0, SEARCH_Y1);
                            }
                            Dirty::Recipient => {
                                render_messages_band(surface, &messages, RECIPIENT_BAND_Y0, RECIPIENT_BAND_Y1);
                            }
                            Dirty::Compose => {
                                render_messages_band(surface, &messages, COMPOSE_BAND_Y0, COMPOSE_BAND_Y1);
                            }
                            Dirty::All => render_messages_ui(surface, &messages),
                        }
                        let _ = std::surface_flip();
                    }
                }
                if events & std::EVENT_MODEM_URC != 0 {
                    if messages.modem_present {
                        std::log("[MESSAGES APP] Baseband URC signalled; no SMS payload was read from the modem.",
                        )
                        .ok();
                    } else {
                        std::log("[MESSAGES APP] Modem URC event raised with no attached baseband; nothing was received.",
                        )
                        .ok();
                    }
                }
                if events & std::EVENT_IPC_MSG != 0 {
                    std::log("[MESSAGES APP] IPC message event raised; no message body was fetched or decrypted.",
                    )
                    .ok();
                    if messages.ledger.thread_count == 0 {
                        std::log("[MESSAGES APP] No conversation thread exists; nothing could be delivered.",
                        )
                        .ok();
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
    std::log("[MESSAGES APP] Panic detected! Terminating task...").ok();
    std::exit();
}
