//! Inter OS Native Sovereign Encrypted Messaging & P2P Wire Client
//!
//! Native decentralized encrypted messaging client executing in Ring 3 (User Mode):
//! - Secure Hardware Keystore key derivation (Slot 3: P2P Messaging Key)
//! - Sovereign peer DID attestation & contact public identity verification
//! - Cellular Baseband SMS modem & P2P wire framing
//! - Sovereign Vault Sector 202 encryption for authenticated persistent chat logs
//! - Sovereign Privacy Shield message payload scrubbing to prevent metadata leakage
//! - Active Conversation Thread Store:
//!   * Conversations are populated only from the real messaging backend.
//!
//! - Dual-tone acoustic incoming/outgoing message bubble chime and capacitive haptics
//! - Asynchronous event reactor loop handling modem URCs, incoming P2P packets, and touch events
//!
//! Typed input (the reason this file was rewritten):
//! - The compose field and the search field are real editors. Tapping one focuses
//!   it, selects whatever is already in it (so the first keystroke replaces rather
//!   than appends), and the normalized keyboard pipeline types into it: printable
//!   ASCII, backspace and escape. This mirrors `apps/browser/src/main.rs`
//!   (`handle_browser_key` / `Dirty` / `render_browser_band`) exactly.
//! - Before this change both fields were drawn on screen with a live caret-style
//!   affordance but no key handler was wired at all: the app had a text field a
//!   user could see and could not type into.
//!
//! Banded rendering:
//! - Every input path names the row band it dirtied (`Dirty`) and only that band
//!   is re-rasterised. The previous renderer pushed all 1,036,800 pixels through
//!   `get_messages_pixel` for every touch event and every keystroke.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

mod store;
use store::ThreadStore;

/// Status of a messaging conversation thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadStatus {
    EncryptedP2P = 0,
    CellularSms = 1,
    PendingHandshake = 2,
}

/// Conversation thread summary.
///
/// WHY the `allow`: this type only names the element of the intentionally empty
/// `THREADS` anchor. Real threads are `store::ThreadRecord`, so none of these
/// `&'static str` fields is ever constructed or read.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct ConversationThread {
    pub peer_name: &'static str,
    pub peer_did: &'static str,
    pub last_snippet: &'static str,
    pub status: ThreadStatus,
    pub unread_count: u8,
}

/// Conversation directory populated from the real messaging backend.
///
/// WHY this stays a zero-length array while the list is realised elsewhere: this
/// static is the compile-time "no sample data" anchor (an app can never ship a
/// hardcoded conversation). Real threads live in the bounded, persisted
/// `store::ThreadStore`, which holds user-typed recipients and messages rather
/// than `&'static str` literals, so it cannot be a `static`. This array therefore
/// stays empty on purpose and is never read.
#[allow(dead_code)]
pub static THREADS: [ConversationThread; 0] = [];

/// Honest state of the last compose attempt, as reported in the header pill.
///
/// WHY an enum rather than a free string: every variant is set by an actual
/// syscall outcome. There is no "Sending..." or "Delivered" variant because no
/// transport is bound to this app, so no such state could ever be observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderStatus {
    /// Startup state: no messaging transport is bound to this app.
    NoTransport,
    /// The last composed message was sealed into vault sector 202. Nothing
    /// transmitted it - there is no transport to transmit it with.
    SealedLocally,
    /// The vault refused the seal, so the last message was not even stored.
    VaultRefused,
}

impl HeaderStatus {
    /// Header pill label. Kept short so it still fits the measured pill column.
    pub fn label(&self) -> &'static [u8] {
        match self {
            HeaderStatus::NoTransport => b"NO TRANSPORT",
            HeaderStatus::SealedLocally => b"SEALED LOCALLY",
            HeaderStatus::VaultRefused => b"VAULT REFUSED",
        }
    }
}

/// Bytes the compose editor can hold. Sized so the longest run still renders
/// inside the pill's measured text column after ellipsizing.
const COMPOSE_CAP: usize = 128;
/// Bytes the search editor can hold.
const SEARCH_CAP: usize = 48;
/// Bytes the recipient editor can hold. Must match `store::RECIP_MAX` so an
/// edited recipient stores without truncation.
const RECIP_CAP: usize = 24;

/// Production Sovereign Messages state manager.
pub struct MessagesApp {
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
    /// The whole compose buffer is selected, so the next character replaces it.
    ///
    /// WHY this exists: focusing pre-filled nothing here, but a select-all-on-focus
    /// field is what Android and iOS both do, and without it a caret parked at the
    /// end of an existing draft is indistinguishable from "append here" until the
    /// user has already typed the wrong thing.
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
    /// Real, persisted thread table. The list renders from this, never from a
    /// hardcoded array.
    pub store: ThreadStore,
}

impl MessagesApp {
    pub const fn new() -> Self {
        Self {
            total_sent: 0,
            total_received: 0,
            active_thread_idx: 0,
            keystore_derived: false,
            sector_encrypted: false,
            // WHY: fail closed. Until the kernel actually reports a baseband,
            // no inbound SMS path may be claimed.
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
            store: ThreadStore::EMPTY,
        }
    }

    /// Derives the 256-bit AES-GCM / ChaCha20 session key from Keystore Slot 3.
    ///
    /// Returns `true` only when a full 32-byte key was actually returned.
    pub fn derive_messaging_key(&mut self) -> bool {
        let mut key = [0u8; 32];
        match std::keystore_derive(3, &mut key) {
            Ok(32) => {
                self.keystore_derived = true;
                std::log(
                    0,
                    "[MESSAGES APP] Hardware Keystore Slot 3: Ingested 256-bit P2P messaging key.",
                )
                .ok();
                true
            }
            Ok(_) => {
                std::log(0, "[MESSAGES APP] Keystore Slot 3: short result; no messaging key available.").ok();
                false
            }
            Err(_) => {
                // WHY: raw keystore material never crosses the kernel boundary, so
                // this refusal is expected. Report it so the app never implies a
                // session key exists to encrypt or decrypt with.
                std::log(0, "[MESSAGES APP] Keystore Slot 3: derivation refused by the kernel; no session key.").ok();
                false
            }
        }
    }

    /// Seals the composed body into Sovereign Vault Sector 202 and reports the
    /// outcome the vault actually returned.
    ///
    /// WHY the body is a caller-supplied slice and not a `&'static str`: the
    /// composer holds a real 128-byte editable buffer, so the body is whatever
    /// the user typed this session. The previous signature could only be fed a
    /// literal, which is why the send control was never wired to it at all.
    ///
    /// Returns `true` only when the vault acknowledged the seal. It NEVER
    /// reports delivery: this app holds no transport and issues no
    /// transmission syscall, so nothing is ever transmitted from here.
    pub fn seal_composed(&mut self, body: &[u8]) -> bool {
        let mut sector_buf = [0u8; 512];
        let copy_len = body.len().min(sector_buf.len().saturating_sub(16));
        sector_buf[..copy_len].copy_from_slice(&body[..copy_len]);
        let mut iv = [0u8; 16];
        if std::get_random(&mut iv).is_err() {
            iv.copy_from_slice(b"INTEROS_SEC_MSG_");
        }
        // WHY: report the vault outcome. Previously a refused write left
        // `sector_encrypted` false but the send counter had already advanced.
        match std::vault_encrypt_sector(202, &mut sector_buf, &iv) {
            Ok(_) => {
                self.sector_encrypted = true;
                self.header_status = HeaderStatus::SealedLocally;
                // WHY: the syscall returns only success/failure, never the
                // cipher it used. Naming "ChaCha20-Poly1305" here invented a
                // crypto detail this app cannot observe, so report only the
                // outcome the vault actually acknowledged.
                std::log(
                    0,
                    "[MESSAGES APP] Sector 202: Message sector sealed and persisted by the storage vault.",
                )
                .ok();
                std::log(
                    0,
                    "[MESSAGES APP] No transport is bound to this app; the sealed message was not transmitted.",
                )
                .ok();
                true
            }
            Err(_) => {
                self.header_status = HeaderStatus::VaultRefused;
                std::log(
                    0,
                    "[MESSAGES APP] Sector 202: vault write refused by the kernel; message not persisted.",
                )
                .ok();
                false
            }
        }
    }

    /// Plays the outgoing bubble chime (E5: 659Hz -> B5: 988Hz).
    ///
    /// Returns `true` only when the audio service accepted both tones.
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
        // WHY: the audio service refuses tone requests without its capability.
        // Only claim the chime when it actually sounded.
        if tones_played == 2 {
            std::log(
                0,
                "[MESSAGES APP] Outgoing message bubble chime emitted (659Hz -> 988Hz).",
            )
            .ok();
            true
        } else {
            std::log(
                0,
                "[MESSAGES APP] Bubble chime not emitted: the audio service refused the tone requests.",
            )
            .ok();
            false
        }
    }

    /// Number of threads that survive the current search query.
    ///
    /// WHY this reads the persisted store and not the static anchor: the filter
    /// is real work over the real list. With zero stored threads it matches
    /// nothing, and the UI says so - it does not pretend the field is inert.
    pub fn match_count(&self) -> usize {
        let q = &self.search[..self.search_len];
        self.store.match_count(q)
    }
}

/// Copies `src` into `dst`, truncating, and returns the bytes written.
fn copy_ascii(src: &[u8], dst: &mut [u8]) -> usize {
    let n = src.len().min(dst.len());
    dst[..n].copy_from_slice(&src[..n]);
    n
}

/// Decimal `v` (a small count) into `out`, no leading zeros. Returns the digits.
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

// -----------------------------------------------------------------------------
// Visual Design Tokens (Screen 10 Clean Conversation View)
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFFF7F6F2;
const SURFACE_DARK: u32 = 0xFFFFFFFF;
const SURFACE_RAISED: u32 = 0xFFEFEEE9;
const BORDER_SUBTLE: u32 = 0xFFE1E0DB;
const ACCENT_SKY: u32 = 0xFF2563EB;
#[allow(dead_code)]
const SUCCESS_EMERALD: u32 = 0xFF2E8B57;
const TEXT_PRIMARY: u32 = 0xFF111214;
const TEXT_MUTED: u32 = 0xFF777773;

/// Recessed fill for a control that cannot act. One below `SURFACE_RAISED` so a
/// disabled control reads as pushed in rather than merely tinted.
const DISABLED_FILL: u32 = 0xFFE6E5E0;

// -----------------------------------------------------------------------------
// Canonical InterOS Typography
// -----------------------------------------------------------------------------

/// Anti-aliased run draw, clipped to the run's own measured line box.
///
/// `bg` is the local fill the run actually sits on. Blending every glyph edge
/// toward one hardcoded page colour tinted the anti-aliased fringe of any run
/// drawn on a pill, which is the same bug class as gating text to a hand-typed
/// y-window: the local fill and the run must be described by the same constants.
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
    // A run owns exactly `line_height(scale)` rows; testing only that box keeps a
    // call site from slicing the glyphs with a shorter hand-written window.
    if x < x0 || y < y0 || y >= y0 + std::design::font::line_height(scale) {
        return None;
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, false)
}

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// -----------------------------------------------------------------------------
// Measured header/pill geometry (Screen 10, 720x1440 portrait).
//
// All text below is drawn with the SDK design face (16x24 cells, proportional
// advances) via `test_text_aa`. Every rect/x0/y0 here is derived from
// `measure`/`line_height` so ink stays inside its container.
// -----------------------------------------------------------------------------

/// y 0..96: header band (the hairline rule is drawn at y == 93).
const HEADER_Y0: usize = 0;
const HEADER_Y1: usize = 96;
const TITLE_TEXT: &[u8] = b"Messages";
const TITLE_X: usize = 116;
const TITLE_SCALE: usize = 3;
// WHY: scale 3 has line_height 72; y0=12 gives 12..84, fully inside the
// header band (border at 94) with 10px clearance. The old y0=36 ran to 108,
// bleeding past the header border, and the 36..60 draw gate sliced glyphs.
const TITLE_Y: usize = 12;

#[inline]
fn pill_text(app: &MessagesApp) -> &'static [u8] {
    app.header_status.label()
}

/// Right edge of the header pill: the 24 px screen margin (696).
const PILL_RIGHT: usize = W - 24;
const PILL_TOP: usize = 28;
const PILL_BOTTOM: usize = 68; // exclusive
const PILL_PAD_X: usize = 20;

#[inline]
fn pill_text_scale(label: &[u8]) -> usize {
    // Keep a 24px gap between the title ink end and the pill's left edge.
    let title_end = TITLE_X + std::design::font::measure(TITLE_TEXT, TITLE_SCALE);
    let avail = PILL_RIGHT.saturating_sub(title_end + 24);
    let mut scale = 2usize;
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

// -----------------------------------------------------------------------------
// Search field geometry (y 104..156). Shared by the renderer and the hit-tester
// so a tap lands exactly where the field is drawn.
// -----------------------------------------------------------------------------
const SEARCH_Y0: usize = 104;
const SEARCH_Y1: usize = 156;
/// The drawn field: x 24..696, y 106..154 (corner radius 22).
const SEARCH_FIELD: (usize, usize, usize, usize) = (24, 106, 672, 48);
/// Magnifying-glass icon occupies x 24..82, so the text column starts at 82.
const SEARCH_TEXT_X: usize = 82;
const SEARCH_TEXT_MAX_W: usize = 696 - 20 - SEARCH_TEXT_X;
const SEARCH_PLACEHOLDER: &[u8] = b"Search chats...";
const SEARCH_TEXT_SCALE: usize = 1;

// -----------------------------------------------------------------------------
// Compose field geometry (y 1330..1414).
// -----------------------------------------------------------------------------
/// Recipient ("To:") editor, drawn between the thread list and the composer.
///
/// WHY a separate band: a thread is keyed by its recipient, so composing a
/// message needs a real recipient field. It reuses the same editor machinery as
/// the composer and search field, shares its hit-test with its drawing, and
/// repaints only its own rows on a keystroke.
const RECIPIENT_BAND_Y0: usize = 1288;
const RECIPIENT_BAND_Y1: usize = 1330;
/// The drawn field: x 24..610, y 1292..1328 (corner radius 18).
const RECIPIENT_FIELD: (usize, usize, usize, usize) = (24, 1292, 586, 36);
const RECIPIENT_TEXT_X: usize = 64;
const RECIPIENT_TEXT_MAX_W: usize = 610 - 24 - RECIPIENT_TEXT_X;
const RECIPIENT_PLACEHOLDER: &[u8] = b"Recipient (name or handle)";
const RECIPIENT_TEXT_SCALE: usize = 1;

const COMPOSE_BAND_Y0: usize = 1330;
const COMPOSE_BAND_Y1: usize = 1414;
/// The drawn field: x 24..610, y 1344..1404 (corner radius 24).
const COMPOSE_FIELD: (usize, usize, usize, usize) = (24, 1344, 586, 60);
const COMPOSE_TEXT_X: usize = 52;
const COMPOSE_TEXT_MAX_W: usize = 610 - 20 - COMPOSE_TEXT_X;
const COMPOSE_PLACEHOLDER: &[u8] = b"Type a message...";
const COMPOSE_TEXT_SCALE: usize = 2;
/// Send control circle centre and radius (drawn x 630..694, y 1344..1404).
const SEND_CX: usize = 662;
const SEND_CY: usize = 1374;
const SEND_R: i32 = 29;

/// Home gesture band (y 1418..1424).
const HOME_X0: usize = 240;
const HOME_X1: usize = 480;
const HOME_Y0: usize = 1400;

/// Thread card geometry, used by both the renderer and the tap hit-tester.
/// WHY an explicit rect: the old tap region was y 110..470, which overlapped the
/// search field at y 104..156, so a tap aimed at the search box was delivered to
/// a conversation card instead.
const CARD_X0: usize = 24;
const CARD_X1: usize = 696;
const CARD_Y0: usize = 180;
const CARD_H: usize = 120;
const CARD_BODY_H: usize = 104;

/// Empty-state card (also the target of the search filter result).
/// WHY centred and shrunk: the card sat at y 180..680 while its content ran
/// 260..544, leaving ~790px of void between the card and the composer and
/// reading as a header that ran out of things to say. The block now floats
/// centred in the space between the search field and the composer, the way
/// an empty conversation list does on a modern phone.
const EMPTY_CARD: (usize, usize, usize, usize) = (24, 560, 672, 340);

#[inline(always)]
fn in_rect(r: (usize, usize, usize, usize), x: usize, y: usize) -> bool {
    x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
}

/// The fill behind selected editor text: the accent at 15% over the field.
///
/// WHY a computed tint and not a fixed colour: the field fill and the accent both
/// come from the theme, so a hard-coded selection colour would drift from them
/// the moment either changed.
fn selection_fill() -> u32 {
    const PERCENT: u32 = 38;
    let t = PERCENT * 256 / 100;
    let channel = |c: u32, bg: u32| -> u32 { (c * t + bg * (256 - t)) >> 8 };
    let r = channel((ACCENT_SKY >> 16) & 0xFF, (SURFACE_DARK >> 16) & 0xFF);
    let g = channel((ACCENT_SKY >> 8) & 0xFF, (SURFACE_DARK >> 8) & 0xFF);
    let b = channel(ACCENT_SKY & 0xFF, SURFACE_DARK & 0xFF);
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

/// Per-frame resolved text for the header pill.
///
/// WHY resolved once per frame: the pill's scale and left edge depend on the
/// measured label, and the label depends on the real syscall outcome. Measuring
/// it inside the per-pixel function would re-measure the same string 1,036,800
/// times per frame.
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

/// A resolved editor field: the ellipsized run actually drawn, its caret
/// position and its selection width.
struct EditorFrame {
    buf: [u8; COMPOSE_CAP],
    len: usize,
    x: usize,
    y: usize,
    scale: usize,
    caret_x: usize,
    select_w: usize,
    /// Colour of the drawn run: ink when the user typed, muted for a placeholder.
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
    // The caret is drawn only while the field is focused, and a selection is
    // only meaningful once the field is focused too.
    f.select_w = if focused && select_all { w } else { 0 };
    f
}

/// Draws the run + selection band + caret of a resolved editor field.
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
    /// Index of the first thread row that passes the search filter, or `None`
    /// when nothing matches.
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

#[inline(never)]
fn get_messages_pixel(x: usize, y: usize, messages: &MessagesApp, f: &Frame) -> u32 {
    // 1. Header (y 0..96)
    if y < HEADER_Y1 {
        if y >= 94 {
            return BORDER_SUBTLE;
        }

        // Circular back button disc (cx: 62, cy: 48, radius: 22)
        let dx = x as isize - 62;
        let dy = y as isize - 48;
        let dsq = dx * dx + dy * dy;
        if dsq <= 22 * 22 {
            // Anti-aliased disc border rim
            if dsq >= 20 * 20 {
                return BORDER_SUBTLE;
            }
            // Crisp vector chevron: apex at (58, 48), arms pointing up-right and down-right
            if dx <= 2 && dx >= -6 {
                let arm_dy = (dx - (-6)).abs();
                if (dy.abs() - arm_dy).abs() <= 1 {
                    return TEXT_PRIMARY;
                }
            }
            return SURFACE_RAISED;
        }

        // Header Title: "Messages" (full 24*scale line box inside header band).
        if y >= TITLE_Y && y < TITLE_Y + std::design::font::line_height(TITLE_SCALE) {
            if let Some(c) = test_text(
                x,
                y,
                TITLE_X,
                TITLE_Y,
                TITLE_SCALE,
                TITLE_TEXT,
                TEXT_PRIMARY,
                SURFACE_DARK,
            ) {
                return c;
            }
        }

        // Status pill, top-right: measured at draw time from the REAL outcome of
        // the last compose attempt, right-aligned to the 24 px margin.
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
            if let Some(c) = test_text(
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
            // Glass handle pointing down-right
            let hx = x as isize - (cx + 5) as isize;
            let hy = y as isize - (cy + 5) as isize;
            if hx >= 0 && hx <= 8 && hy >= 0 && hy <= 8 && (hx - hy).abs() <= 1 {
                return TEXT_MUTED;
            }

            // Typed query (or the placeholder) + caret, on the pill's own fill.
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

    // 3. Conversation Thread List / Empty State (y 168..1288; the recipient
    //    editor band and the composer sit below this).
    if y >= 168 && y < RECIPIENT_BAND_Y0 {
        // A search that matched nothing shows the filter's real result instead of
        // the "no conversations yet" card: the field is not decoration, so its
        // outcome has to be visible.
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

                // WHY no pictogram here: the card's icon is drawn only for the
                // "no conversations at all" state. When a query is present and
                // matches nothing, the honest content is the query itself, not a
                // picture of an empty inbox the user has already dismissed.
                let line = b"No chats match your search";
                let sub = &messages.search[..messages.search_len];
                let mut buf = [0u8; COMPOSE_CAP];
                let n = std::design::font::ellipsize(sub, 1, 624, &mut buf);
                let sub_line: &[u8] = if n == 0 { b"(empty query)" } else { &buf[..n] };
                let mut quoted = [0u8; COMPOSE_CAP + 2];
                quoted[0] = b'"';
                quoted[1..1 + n].copy_from_slice(&buf[..n]);
                quoted[1 + n] = b'"';
                let q_line: &[u8] = &quoted[..n + 2];
                let line_w = std::design::font::measure(line, 2);
                let sub_w = std::design::font::measure(sub_line, 1);
                let q_w = std::design::font::measure(q_line, 1);
                if let Some(c) = test_text(
                    x,
                    y,
                    EMPTY_CARD.0 + (EMPTY_CARD.2.saturating_sub(line_w)) / 2,
                    670,
                    2,
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
                    730,
                    1,
                    sub_line,
                    TEXT_MUTED,
                    SURFACE_DARK,
                ) {
                    return c;
                }
                if let Some(c) = test_text(
                    x,
                    y,
                    EMPTY_CARD.0 + (EMPTY_CARD.2.saturating_sub(q_w)) / 2,
                    756,
                    1,
                    q_line,
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

        if messages.store.is_empty() {
            // Empty state container card at x: 24..696, y: 560..900, centred between search and composer.
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

                // --- Vector Chat Bubble Pictogram at cx: 360, cy: 300 ---
                let cx = 360;
                let cy = 673;

                // Speech bubble body (x: cx - 60..cx + 60, y: cy - 40..cy + 30, radius 18)
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
                        // Interior micro-bars representing chat text
                        if y >= cy - 22 && y < cy - 16 && x >= cx - 40 && x < cx + 28 {
                            return ACCENT_SKY;
                        }
                        if y >= cy - 8 && y < cy - 2 && x >= cx - 40 && x < cx + 16 {
                            return BORDER_SUBTLE;
                        }
                        if y >= cy + 6 && y < cy + 12 && x >= cx - 40 && x < cx - 4 {
                            return BORDER_SUBTLE;
                        }

                        // Border rim
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

                // Speech bubble pointer tail at bottom-left: x: cx - 44..cx - 18, y: cy + 30..cy + 48
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

                // Empty state text. Every run is gated on its own measured line
                // box (24*scale rows) and centred on the card's 672 px width, so
                // no headline is sliced and none runs past the card edge.
                //
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
                if let Some(c) = test_text(
                    x,
                    y,
                    EMPTY_CARD.0
                        + std::design::font::center_x(b"Messaging unavailable: no transport", 1, EMPTY_CARD.2),
                    810,
                    1,
                    b"Messaging unavailable: no transport",
                    TEXT_MUTED,
                    SURFACE_DARK,
                ) {
                    return c;
                }

                // Subtle card border
                if rx < 2 || ry < 2 {
                    return BORDER_SUBTLE;
                }

                // Solid white container card
                return SURFACE_DARK;
            }
        } else {
            // Render real conversation threads from the persisted store, filtered
            // by the search field. Each card shows the recipient, the last stored
            // message, its local-store count, and a delete affordance.
            let idx = (y - CARD_Y0) / CARD_H;
            let card_offset_y = (y - CARD_Y0) % CARD_H;
            if card_offset_y < CARD_BODY_H && x >= CARD_X0 && x < CARD_X1 {
                if let Some(rec) = messages.store.get(idx) {
                    if messages.store.matches(idx, &messages.search[..messages.search_len]) {
                        let rx = (x - CARD_X0).min(CARD_X1 - 1 - x);
                        let ry = card_offset_y.min(CARD_BODY_H - 1 - card_offset_y);
                        if rx < 16 && ry < 16 {
                            let cdx = (16 - rx) as isize;
                            let cdy = (16 - ry) as isize;
                            if cdx * cdx + cdy * cdy >= 256 {
                                return BG_DARK;
                            }
                        }
                        let selected = idx == messages.active_thread_idx;
                        let name = rec.recipient();
                        let name_x =
                            CARD_X0 + std::design::font::center_x(name, 2, CARD_X1 - CARD_X0);
                        if let Some(c) = test_text(
                            x,
                            y,
                            name_x,
                            card_offset_y + 16,
                            2,
                            name,
                            TEXT_PRIMARY,
                            SURFACE_DARK,
                        ) {
                            return c;
                        }
                        // Last stored message, ellipsised to the card's text column.
                        let snippet = rec.last_snippet();
                        let mut buf = [0u8; 48];
                        let n = std::design::font::ellipsize(
                            snippet,
                            1,
                            CARD_X1 - CARD_X0 - 48,
                            &mut buf,
                        );
                        // Honest subline: the count of messages this device has
                        // stored locally. It never implies any were delivered -
                        // no transport is bound to this app.
                        let label: &[u8] = b"  (stored locally: ";
                        let count = rec.count();
                        let mut sub = [0u8; 96];
                        let mut sl = 0usize;
                        if n > 0 {
                            sub[..n].copy_from_slice(&buf[..n]);
                            sl = n;
                        }
                        let _ = selected;
                        if sl + label.len() + 5 <= sub.len() {
                            sub[sl..sl + label.len()].copy_from_slice(label);
                            sl += label.len();
                            let mut digit = [0u8; 4];
                            let dn = put_u8(&mut digit, count);
                            if sl + dn + 1 <= sub.len() {
                                sub[sl..sl + dn].copy_from_slice(&digit[..dn]);
                                sl += dn;
                                sub[sl] = b')';
                                sl += 1;
                            }
                        }
                        if let Some(c) = test_text(
                            x,
                            y,
                            CARD_X0 + 24,
                            card_offset_y + 68,
                            1,
                            &sub[..sl],
                            TEXT_MUTED,
                            SURFACE_DARK,
                        ) {
                            return c;
                        }
                        // Delete affordance: a small x glyph on the card's right.
                        let dxc = CARD_X1 - 44;
                        let dyc = card_offset_y + 32;
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

    // 3b. Recipient editor band (y 1288..1330): a real field the keyboard types
    //     into, sharing its rect with its hit-test and its own repaint band.
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
            if rx < 2 || ry < 2 {
                return BORDER_SUBTLE;
            }
            return SURFACE_DARK;
        }
        return BG_DARK;
    }

    // 4. Bottom Compose Input Bar (y 1330..1414)
    if y >= COMPOSE_BAND_Y0 && y < COMPOSE_BAND_Y1 {
        if y == COMPOSE_BAND_Y0 {
            return BORDER_SUBTLE;
        }

        // Text input field pill (x: 24..610, y: 1344..1404)
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

            if let Some(c) = editor_pixel(x, y, &f.compose, messages.compose_focused, SURFACE_DARK)
            {
                return c;
            }

            if rx < 2 || ry < 2 {
                return BORDER_SUBTLE;
            }
            return SURFACE_DARK;
        }

        // Send control circle.
        //
        // WHY the two treatments: an empty composer has nothing to seal, so the
        // control is drawn recessed with a muted glyph. A non-empty composer has
        // real work to do - seal the typed body into vault sector 202 - so it is
        // drawn live. It never claims delivery; the header pill reports what the
        // vault actually acknowledged.
        if x >= 630 && x < 694 && y >= 1344 && y < 1404 {
            let dx = (x as isize) - SEND_CX as isize;
            let dy = (y as isize) - SEND_CY as isize;
            if dx * dx + dy * dy <= (SEND_R * SEND_R) as isize {
                let live = messages.compose_len > 0;
                let bg = if live { ACCENT_SKY } else { DISABLED_FILL };
                let ink = if live { 0xFFFFFFFF } else { TEXT_MUTED };

                // Chevron arrow head pointing right
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

    // 5. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= HOME_X0 && x < HOME_X1 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn render_messages_ui(surface: u32, messages: &MessagesApp) {
    render_messages_band(surface, messages, HEADER_Y0, H);
}

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a band with no stride
/// bookkeeping. Every band starts on a row boundary, so the running `x`/`y`
/// cursor below is exact.
///
/// WHY this is correct rather than a shortcut: the band is chosen from what the
/// keystroke actually changed - typing in the composer moves a run inside
/// y 1330..1414 and nothing else; typing in the search field moves a run inside
/// y 104..156 and nothing else. Pixels outside a requested band are left exactly
/// as they were drawn, and they are correct because nothing about them changed.
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
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
        offset += count;
    }
}

// -----------------------------------------------------------------------------
// Input handling. Every region below is drawn at the exact same coordinates by
// get_messages_pixel (shared rect constants), so each control is hit-testable
// where it is rendered.
// -----------------------------------------------------------------------------

/// Which part of the surface an action changed.
///
/// WHY this exists: rendering messages re-rasterised all 1,036,800 pixels through
/// `get_messages_pixel` for every touch event and every keystroke, so typing a
/// message cost the same as a full screen transition. Typing moves one text run
/// between 60 and 84 of the 1440 rows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing changed; do not render.
    None,
    /// Only the search field moved: the y 104..156 band.
    Search,
    /// Only the recipient field moved: its own band between the list and composer.
    Recipient,
    /// Only the composer moved: the y 1330..1414 band.
    Compose,
    /// Focus moved between the two fields, or the header pill's label changed:
    /// the whole surface.
    All,
}

/// Merges two dirty regions reported in the same input batch.
///
/// WHY this exists rather than "last one wins": one `input_event_poll` drain can
/// carry a search keystroke and a composer keystroke together. Keeping only one
/// of them would leave the other field's pixels stale, which is exactly the
/// class of bug banding is meant to avoid.
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

/// Applies an edit to the recipient buffer and names the region it dirtied.
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

/// Sends the composed body to the typed recipient: seals it into vault sector 202
/// and appends it to the local thread store. Returns the dirty region.
///
/// WHY it still never reports delivery: no transport is bound to this app, so
/// nothing is transmitted. The header pill keeps reporting the real vault
/// outcome, and the store reports whether the thread was actually persisted.
fn send_composed(app: &mut MessagesApp) -> Dirty {
    if app.compose_len == 0 {
        // Nothing typed: the control is drawn recessed, so this is unreachable
        // from a tap, but the key path can still ask.
        return Dirty::None;
    }
    if app.recip_len == 0 {
        // WHY: a thread is keyed by its recipient. With none typed there is
        // nothing to store and nothing honest to show, so say so and keep the
        // draft rather than creating a recipient-less thread.
        std::log(
            0,
            "[MESSAGES APP] Send pressed with no recipient; nothing was stored locally.",
        )
        .ok();
        return Dirty::None;
    }
    let mut body = [0u8; COMPOSE_CAP];
    let n = copy_ascii(&app.compose[..app.compose_len], &mut body);
    let mut recip = [0u8; RECIP_CAP];
    let rn = copy_ascii(&app.recip[..app.recip_len], &mut recip);
    // WHY the seal still runs first: it records the authenticated, encrypted
    // at-rest copy and the real header-pill outcome, exactly as before.
    let sealed = app.seal_composed(&body[..n]);
    let _ = app.play_bubble_chime();
    if sealed {
        app.total_sent += 1;
        // Only now, with a real recipient and a sealed body, does the message
        // join the thread store -- and only if the write is acknowledged.
        if app.store.append(&recip[..rn], &body[..n]) {
            std::log(
                0,
                "[MESSAGES APP] Message stored to the local thread store; no transport is bound, so it was not transmitted.",
            )
            .ok();
        } else {
            std::log(
                0,
                "[MESSAGES APP] Message sealed but the thread store would not take it; it is not in the thread list.",
            )
            .ok();
        }
    }
    app.compose_len = 0;
    app.recip_len = 0;
    app.compose_focused = false;
    app.compose_select_all = false;
    app.recip_focused = false;
    app.recip_select_all = false;
    // WHY the whole surface: the header pill's label changed with the real vault
    // outcome, and the pill is right-aligned, so its left edge moves too.
    Dirty::All
}

fn handle_tap(app: &mut MessagesApp, px: u32, py: u32) -> Tap<'static> {
    let (x, y) = (px as usize, py as usize);

    // System home gesture band: leave to the launcher.
    if x >= HOME_X0 && x < HOME_X1 && y >= HOME_Y0 {
        return Tap::Leave("[MESSAGES APP] Home gesture pressed. Exiting to launcher.");
    }

    // Back control in the header: the only real "leave" action this app has.
    if x >= 24 && x < 100 && y >= 28 && y < 68 {
        return Tap::Leave("[MESSAGES APP] Back control pressed. Exiting to launcher.");
    }

    // Send control: real work, real reporting.
    let sdx = x as isize - SEND_CX as isize;
    let sdy = y as isize - SEND_CY as isize;
    if sdx * sdx + sdy * sdy <= (SEND_R * SEND_R) as isize {
        if app.compose_len == 0 {
            // The control is drawn recessed when there is nothing to send, so
            // report the reason rather than silently swallowing the tap.
            //
            // WHY no haptic here: the same reasoning as the music player's
            // transport. A click on a control that is already drawn disabled
            // tells the user the tap landed on something that can act, which is
            // exactly what it cannot.
            std::log(
                0,
                "[MESSAGES APP] Send pressed with an empty composer; nothing was sealed.",
            )
            .ok();
            return Tap::Redraw(Dirty::None);
        }
        let dirty = send_composed(app);
        std::log(0, "[MESSAGES APP] Composer cleared after the seal attempt.").ok();
        return Tap::Redraw(dirty);
    }

    // Composer: focus the editable native field.
    if in_rect(COMPOSE_FIELD, x, y) {
        focus_compose(app);
        let _ = std::audio_haptic_click();
        std::log(0, "[MESSAGES APP] Compose field focused for keyboard input.").ok();
        return Tap::Redraw(Dirty::All);
    }

    // Search field: focus the editable native filter.
    if in_rect(SEARCH_FIELD, x, y) {
        focus_search(app);
        let _ = std::audio_haptic_click();
        std::log(0, "[MESSAGES APP] Search field focused for keyboard input.").ok();
        return Tap::Redraw(Dirty::All);
    }

    // Recipient editor: focus the editable "To:" field.
    if in_rect(RECIPIENT_FIELD, x, y) {
        focus_recipient(app);
        let _ = std::audio_haptic_click();
        std::log(0, "[MESSAGES APP] Recipient field focused for keyboard input.").ok();
        return Tap::Redraw(Dirty::All);
    }

    // Conversation card: exactly the drawn card row. A tap in the card's delete
    // affordance removes the thread; a tap elsewhere selects it.
    if x >= CARD_X0 && x < CARD_X1 && y >= CARD_Y0 {
        let idx = (y - CARD_Y0) / CARD_H;
        let coff = (y - CARD_Y0) % CARD_H;
        if coff < CARD_BODY_H && app.store.matches(idx, &app.search[..app.search_len]) {
            // Delete affordance, matching the glyph the renderer draws at the
            // card's right edge (CARD_X1 - 44, card_offset_y + 32).
            let ddx = x as isize - (CARD_X1 - 44) as isize;
            let ddy = coff as isize - 32;
            if ddx.abs() <= 16 && ddy.abs() <= 12 {
                if app.store.remove(idx) {
                    std::log(
                        0,
                        "[MESSAGES APP] Thread deleted; the deletion was persisted to the settings store.",
                    )
                    .ok();
                    if app.active_thread_idx >= app.store.len() && app.active_thread_idx > 0 {
                        app.active_thread_idx -= 1;
                    }
                } else {
                    std::log(
                        0,
                        "[MESSAGES APP] Thread deletion was not persisted; the store is unchanged.",
                    )
                    .ok();
                }
                return Tap::Redraw(Dirty::All);
            }
            app.active_thread_idx = idx.min(app.store.len().saturating_sub(1));
            let _ = std::audio_haptic_click();
            return Tap::Redraw(Dirty::All);
        }
    }

    Tap::None
}

/// Mirrors `handle_browser_key` in `apps/browser/src/main.rs`: printable ASCII,
/// backspace, escape and enter, routed to whichever editor is focused.
fn handle_messages_key(app: &mut MessagesApp, key: std::InputKeyEvent) -> Dirty {
    use std::input_key_code;
    if !key.pressed {
        return Dirty::None;
    }
    // Escape defocuses. It is checked BEFORE the focus guard so it is the one
    // key that always works, whether or not a field currently has focus.
    if key.code == input_key_code::ESCAPE {
        let moved = app.compose_focused || app.search_focused || app.recip_focused;
        app.compose_focused = false;
        app.compose_select_all = false;
        app.search_focused = false;
        app.search_select_all = false;
        app.recip_focused = false;
        app.recip_select_all = false;
        return if moved { Dirty::All } else { Dirty::None };
    }
    if !app.compose_focused && !app.search_focused && !app.recip_focused {
        return Dirty::None;
    }

    if app.recip_focused {
        return match key.code {
            input_key_code::BACKSPACE => edit_recipient(app, None),
            // Enter hands focus to the message body, the natural next field.
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

    // Search field.
    match key.code {
        input_key_code::BACKSPACE => edit_search(app, None),
        code if (32..=126).contains(&code) && app.search_len < SEARCH_CAP => {
            edit_search(app, Some(code as u8))
        }
        _ => Dirty::None,
    }
}

/// Applies an edit to the search buffer and names the region it dirtied.
///
/// `push` is `Some(byte)` to insert or `None` to delete. WHY the filter is
/// re-counted around the edit instead of always returning `Dirty::All`: the query
/// itself only moves text inside the search band, so a keystroke that leaves the
/// number of matching threads unchanged really has changed nothing outside
/// y 104..156. This is the same comparison the browser makes for the first
/// keystroke that clears its status block.
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
    std::log(
        0,
        "[MESSAGES APP (Ring 3)] Sovereign Encrypted Messaging & P2P Wire Protocol online.",
    )
    .ok();

    let mut messages = MessagesApp::new();

    // Load the real, persisted thread table before the first frame so the list
    // reflects what storage actually holds (honest Empty/ReadRefused/Invalid
    // states included), never a seeded list.
    messages.store = store::ThreadStore::load();
    // Report the real first-load outcome honestly: Empty is the normal first
    // boot, and a refused or invalid read must not look like an empty store.
    let load_note: &str = match messages.store.state {
        store::StoreState::Empty => "Thread store loaded: no threads are stored yet.",
        store::StoreState::Loaded => "Thread store loaded from the settings store.",
        store::StoreState::ReadRefused => "Settings read refused; no stored threads were loaded.",
        store::StoreState::Invalid => "Stored thread data failed validation; no threads were loaded.",
        _ => "",
    };
    if !load_note.is_empty() {
        std::log(0, load_note).ok();
    }

    // 1. Ingest Hardware Keystore Key
    messages.derive_messaging_key();

    // 2. Allocate DRM/KMS Client Surface (z=200)
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(id) => {
            std::log(
                0,
                "[MESSAGES APP] DRM/KMS Client Surface allocated (z=200).",
            )
            .ok();
            id
        }
        Err(_) => 0,
    };

    // 3. Telephony Cellular Modem HAL Query
    let mut modem_buf = [0u8; 256];
    // WHY: this app holds no telephony authority, so the query is refused and the
    // absence of a log line must not be read as an undetected modem. Report the
    // refusal explicitly.
    match std::telephony_query(&mut modem_buf) {
        Ok(modem_len) if modem_len > 0 => {
            let present = core::str::from_utf8(&modem_buf[..modem_len])
                .map(|s| s.contains("\"present\":true"))
                .unwrap_or(false);
            if present {
                messages.modem_present = true;
                std::log(
                    0,
                    "[MESSAGES APP] Telephony modem active: P2P cellular wire channel online.",
                )
                .ok();
            } else {
                // WHY: the report carried no "present":true field. "In standby"
                // would assert a baseband state the HAL never reported, so state
                // only what the payload actually said.
                std::log(
                    0,
                    "[MESSAGES APP] Telephony telemetry did not report a present modem; no cellular channel is in use.",
                )
                .ok();
            }
        }
        Ok(_) => {
            std::log(
                0,
                "[MESSAGES APP] Telephony query returned an empty payload; no cellular modem reported.",
            )
            .ok();
        }
        Err(_) => {
            std::log(
                0,
                "[MESSAGES APP] Telephony query refused by the kernel; no cellular modem is attached.",
            )
            .ok();
        }
    }

    if surface > 0 {
        render_messages_ui(surface, &messages);
        let _ = std::surface_flip(6);
        std::log(
            0,
            "[MESSAGES APP] Messages canvas presented to scanout buffer.",
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
                        // Keyboard first: the normalized pipeline multiplexes
                        // keys and touches through the same poll, so a key must be
                        // recognised before the touch branch below sees kind != 1.
                        if let Some(key) = input.key_event() {
                            let changed = handle_messages_key(&mut messages, key);
                            dirty = merge(dirty, changed);
                            continue;
                        }
                        if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                            std::log(0, "[MESSAGES APP] Navigation gesture recognized. Exiting to launcher.").ok();
                            if surface > 0 {
                                let _ = std::surface_destroy(6, surface);
                            }
                            std::exit_with_code(0);
                        }
                        if input.kind == 1 {
                            match handle_tap(&mut messages, input.x, input.y) {
                                Tap::Leave(msg) => {
                                    std::log(0, msg).ok();
                                    if surface > 0 {
                                        let _ = std::surface_destroy(6, surface);
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
                        let _ = std::surface_flip(6);
                    }
                }
                // WHY: the URC bit only says a baseband event was signalled. No SMS frame was
                // read, decoded, or decrypted, and no baseband is attached, so
                // reporting an inbound message would be reporting state that was
                // never observed.
                if events & std::EVENT_MODEM_URC != 0 {
                    if messages.modem_present {
                        std::log(
                            0,
                            "[MESSAGES APP] Baseband URC signalled; no SMS payload was read from the modem.",
                        )
                        .ok();
                    } else {
                        std::log(
                            0,
                            "[MESSAGES APP] Modem URC event raised with no attached baseband; nothing was received.",
                        )
                        .ok();
                    }
                }
                if events & std::EVENT_IPC_MSG != 0 {
                    // WHY: no message body was fetched, no session key exists to
                    // decrypt with, and THREADS is empty so there is no active
                    // thread to deliver to. Report the event, not a delivery.
                    std::log(
                        0,
                        "[MESSAGES APP] IPC message event raised; no message body was fetched or decrypted.",
                    )
                    .ok();
                    if messages.store.is_empty() {
                        std::log(
                            0,
                            "[MESSAGES APP] No conversation thread exists; nothing could be delivered.",
                        )
                        .ok();
                    }
                }
                if events & std::EVENT_TIMER_TICK != 0 {
                    // Periodic thread ledger sync
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
    std::log(0, "[MESSAGES APP] Panic detected! Terminating task...").ok();
    std::exit();
}
