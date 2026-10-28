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
use messages::theme::{MIN_TOUCH_TARGET, THEME, radius, spacing, type_scale};
use interos_std::design::components;
use interos_std::design::outline_text;
use interos_std::design::monochrome;
use interos_std::design::primitives;
use interos_std::design::scroll::ScrollState;

use messages::gsm;
use messages::keyboard::{
    Editor, Editors, Field, KeyAction, Keyboard, BANDS_H, BANDS_TOP_IDLE, KB_Y0, KB_Y1,
    RECIPIENT_BAND_H,
};
use messages::privacy;
use messages::publish;
use messages::store;
use messages::store::ThreadStore;
use messages::{
    send_arrow_ink, DeliveryStatus, HIDDEN_PREVIEW_LABEL, INCOMING_LOCAL_LABEL, MessageDirection,
    MessageRecord, ThreadLedger, ThreadStatus,
};

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
    /// The details of the open thread: what the app knows about it, and what
    /// the platform does not offer.
    Details,
    /// The recipient picker: every recipient the platform can hand over.
    Recipients,
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
                // WHY the field is focused: the hand-off filled the field,
                // and a filled field the user cannot see the caret in is a
                // field they do not know they can type into. A focused field
                // shows the keyboard, so the caret and the keyboard agree.
                self.keyboard.focus(Field::Recipient);
                // WHY the hand-off is kept: the recipient picker offers the
                // recipients the platform can produce, and the dialer's
                // hand-off is one of them. The settings key is cleared, but
                // the value it carried is not thrown away with it.
                self.handoff[..n].copy_from_slice(&buf[..n]);
                self.handoff_len = n;
                let _ = std::settings_set(HANDOFF_KEY, &[]);
                std::log("[MESSAGES APP] A recipient was handed over by the dialer; it is shown in the recipient field.",
                )
                .ok();
            }
            _ => {}
        }
    }

    /// Records a line of text telling the user what the last action did.
    pub fn set_notice(&mut self, text: &[u8]) {
        let n = text.len().min(NOTICE_CAP);
        self.notice[..n].copy_from_slice(&text[..n]);
        self.notice_len = n;
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
    pub compose_select_all: bool,
    /// Search/filter editor buffer and its length.
    pub search: [u8; SEARCH_CAP],
    pub search_len: usize,
    pub search_select_all: bool,
    /// Recipient ("To:") editor buffer and its length.
    pub recip: [u8; RECIP_CAP],
    pub recip_len: usize,
    pub recip_select_all: bool,
    /// The on-screen keyboard: which field it types into, and its own state.
    ///
    /// WHY the focus lives here and not in a flag per field: the keyboard
    /// and the focused field are the same fact. A focused field shows the
    /// keyboard, and a keyboard with no focused field has nothing to type
    /// into.
    pub keyboard: Keyboard,
    /// The recipient the dialer handed over, kept for the recipient picker.
    ///
    /// WHY it is kept after the field is filled: the picker offers the
    /// recipients the platform can produce, and the dialer's hand-off is one
    /// of them. Clearing it with the settings key would leave the picker
    /// offering nothing the user was just given.
    pub handoff: [u8; RECIP_CAP],
    pub handoff_len: usize,
    /// The thread list's scroll position.
    ///
    /// WHY the list scrolls: the ledger holds eight threads and the visible
    /// band holds fewer, and a thread below the band was unreachable.
    pub list_scroll: ScrollState,
    /// The conversation's scroll position.
    ///
    /// WHY the conversation scrolls: a thread holds sixteen messages and the
    /// band holds twelve, so the oldest messages were drawn off the top of
    /// the screen with no way to reach them.
    pub convo_scroll: ScrollState,
    /// The path of the document the open thread was published at.
    pub published_path: [u8; publish::PATH_CAP],
    pub published_path_len: usize,
    /// A line of text telling the user what the last action did.
    pub notice: [u8; NOTICE_CAP],
    pub notice_len: usize,
    /// Whether each thread hides its preview in the thread list.
    ///
    /// WHY it is cached on the app: the answer lives in the platform's
    /// settings store, and asking it for every pixel of the list would mean
    /// a system call per pixel. The cache is refreshed when the ledger
    /// changes and when the user changes the setting.
    pub hidden_previews: [bool; messages::MAX_THREADS],
    /// The screen the recipient picker was opened from.
    pub picker_return: ViewMode,
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
            compose_select_all: false,
            search: [0; SEARCH_CAP],
            search_len: 0,
            search_select_all: false,
            recip: [0; RECIP_CAP],
            recip_len: 0,
            recip_select_all: false,
            keyboard: Keyboard::new(),
            handoff: [0; RECIP_CAP],
            handoff_len: 0,
            list_scroll: ScrollState::new(0, LIST_VIEWPORT_H, 0),
            convo_scroll: ScrollState::new(0, CONVO_VIEWPORT_H, 0),
            published_path: [0; publish::PATH_CAP],
            published_path_len: 0,
            notice: [0; NOTICE_CAP],
            notice_len: 0,
            hidden_previews: [false; messages::MAX_THREADS],
            picker_return: ViewMode::ThreadList,
        }
    }

    /// The ledger index of the thread with `tid`.
    ///
    /// WHY it is looked up by id: the privacy cache is indexed by the
    /// ledger's own order, and the thread the user has open is named by its
    /// id.
    pub fn thread_ledger_index(&self, tid: u32) -> Option<usize> {
        (0..self.ledger.thread_count).find(|i| self.ledger.threads[*i].id == tid)
    }

    /// Reads the per-thread privacy settings out of the platform's settings
    /// store and caches them.
    ///
    /// WHY the store is the source: the setting has to outlive the app, and
    /// the settings store is the one place on the device where a preference
    /// is kept between runs.
    pub fn refresh_privacy(&mut self) {
        for i in 0..self.ledger.thread_count {
            if i >= self.hidden_previews.len() {
                break;
            }
            let peer = self.ledger.threads[i].peer();
            self.hidden_previews[i] = privacy::previews_hidden(peer);
        }
    }

    /// Re-derives the thread list's scroll extent from the number of rows.
    ///
    /// WHY the extent is re-derived and not set once: the row count changes
    /// when a thread is added or deleted, and a scroll extent that is stale
    /// would let the list scroll past its last row or clip its last row.
    pub fn sync_list_extent(&mut self) {
        let rows = self.match_count();
        // WHY the viewport is derived and not a constant: the keyboard takes
        // the bottom of the screen when it is up, so the visible band is
        // shorter then, and a scroll extent built for the tall band would
        // let the list scroll under the keyboard.
        self.list_scroll = ScrollState::new(
            rows.saturating_mul(CARD_H),
            list_viewport_h(self),
            self.list_scroll.offset,
        );
    }

    /// Re-derives the conversation's scroll extent from the number of bubbles.
    ///
    /// WHY the extent is re-derived: a thread's message count changes when a
    /// message is posted, and the scroll offset has to be re-clamped against
    /// the new content or the view would end up past the last bubble.
    pub fn sync_convo_extent(&mut self) {
        let mut msgs = [MessageRecord::EMPTY; messages::MAX_MESSAGES_PER_THREAD];
        let count = match self.active_thread_id {
            Some(tid) => self.ledger.get_thread_messages(tid, &mut msgs),
            None => 0,
        };
        self.convo_scroll = ScrollState::new(
            count.saturating_mul(BUBBLE_STEP),
            convo_viewport_h(self),
            self.convo_scroll.offset,
        );
    }

    /// Brings the newest message of the open thread into view.
    ///
    /// WHY the bottom is where a conversation rests: a new message is
    /// written at the end of the thread, and a view that stayed where it was
    /// would not show the message the user just sent.
    pub fn scroll_convo_to_newest(&mut self) {
        self.sync_convo_extent();
        self.convo_scroll.offset = self.convo_scroll.max_offset();
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

    /// The ledger index of the `n`th thread that matches the search.
    ///
    /// WHY the list is addressed by the matching index and not by the
    /// ledger's own: a filtered list draws only the rows that match, so the
    /// row under the finger is the `n`th match and not the `n`th thread. The
    /// two are the same only when nothing is filtered.
    pub fn filtered_index(&self, n: usize) -> Option<usize> {
        messages::filtered_thread_index(
            &self.ledger.threads,
            self.ledger.thread_count,
            &self.search[..self.search_len],
            n,
        )
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
//
// WHY there is no palette here any more: the app used to carry its own
// colours, which is a second design system inside one app. Every colour now
// resolves through the platform's theme, so a third-party app that asks the
// SDK for the same roles draws the same messages surface. See
// `messages::theme` for the mapping from the old names to the platform's roles.
// -----------------------------------------------------------------------------


/// The platform's corner radii, under the short names the renderer uses.
///
/// WHY the aliases exist: the corner test names its radius four times, and a
/// number that appears four times is a number that can drift from the
/// platform. The alias resolves to the platform's step, so the drift is the
/// platform's and not this app's.
#[allow(dead_code)]
const R_CARD: usize = radius::CARD;
#[allow(dead_code)]
const R_SHEET: usize = radius::SHEET;
const R_SOFT: usize = radius::SOFT_PILL;

/// The platform's card treatment, at the elevation the app's cards sit at.
///
/// WHY the app's cards are the platform's cards: a card that invents its own
/// fill and its own hairline is a card a third-party app cannot match, and
/// the platform already says what a card looks like.
const CARD: components::CardStyle =
    components::card_style(&THEME, components::Elevation::Resting);

/// The card's hairline, or the card's own fill when the card has no hairline.
#[allow(dead_code)]
const CARD_BORDER: u32 = match CARD.border {
    Some(c) => c,
    None => CARD.fill,
};

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// Draws a run of text with the sharp outline face, falling back to the
/// bitmap face when the outline face refuses the run.
///
/// WHY the outline face is tried first: the platform scanout is 720x1440,
/// the same as the space this app paints, so composition is 1:1 and the
/// compositor magnifies nothing. The outline face is still preferred because
/// it rasterises the real outline at the requested cap height instead of
/// magnifying the 16x24 bitmap fallback face, so it stays sharp at every
/// size.
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
    if x < bx || x >= W || x - bx >= width || y < top || y - top >= height {
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
    if x < bx || x >= W || x - bx >= width || y < top || y - top >= height {
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
const TITLE_SCALE: usize = type_scale::TITLE;
const TITLE_Y: usize = 36;

const PILL_RIGHT: usize = W - 24;
const PILL_TOP: usize = 28;
const PILL_BOTTOM: usize = 68;
const PILL_PAD_X: usize = spacing::GRID_20;

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
const SEARCH_TEXT_SCALE: usize = type_scale::BODY;

// Recipient ("To:") editor
//
// WHY the band moves: the band heights and the positions they sit at are
// the keyboard module's, so the rule that keeps the keyboard off the
// compose controls is one rule with one place to keep it.
// WHY the field is 44 tall and starts at the band's top: the design system's
// minimum touch target is 44 logical pixels, and this field was 36, so a
// 36-row strip had to be tapped to focus it. The band moved up by the four
// rows the field gained; the thread list above ends at the band, so the list
// gives up those four rows rather than the field overlapping the compose
// band below.
const RECIPIENT_FIELD_H: usize = MIN_TOUCH_TARGET;
const RECIPIENT_FIELD_W: usize = 586;
const RECIPIENT_TEXT_X: usize = 76;
const RECIPIENT_TEXT_MAX_W: usize = 490 - RECIPIENT_TEXT_X;
const RECIPIENT_PLACEHOLDER: &[u8] = b"Recipient (name or handle)";
const RECIPIENT_TEXT_SCALE: usize = type_scale::BODY;

// Compose field geometry
const COMPOSE_FIELD_H: usize = 60;
const COMPOSE_FIELD_W: usize = 586;
const COMPOSE_TEXT_X: usize = 52;
// WHY the text stops before the field's right edge: the last ~60 px of the
// field are the character counter and the clear control, and text that ran
// underneath them made both unreadable.
const COMPOSE_COUNT_RIGHT: usize = 572;
const COMPOSE_TEXT_MAX_W: usize = COMPOSE_COUNT_RIGHT - 56 - COMPOSE_TEXT_X;
const COMPOSE_PLACEHOLDER: &[u8] = b"Type a message...";
const COMPOSE_TEXT_SCALE: usize = type_scale::BODY;
const SEND_CX: usize = 662;
const SEND_R: i32 = 29;

/// The top of the two bands in the layout the app is drawing right now.
#[inline(always)]
fn bands_top(app: &MessagesApp) -> usize {
    messages::keyboard::bands_top(&app.keyboard)
}

/// The top of the recipient band.
#[inline(always)]
fn recipient_band_y0(app: &MessagesApp) -> usize {
    bands_top(app)
}

/// The bottom of the recipient band.
#[inline(always)]
fn recipient_band_y1(app: &MessagesApp) -> usize {
    bands_top(app) + RECIPIENT_BAND_H
}

/// The top of the compose band.
#[inline(always)]
fn compose_band_y0(app: &MessagesApp) -> usize {
    bands_top(app) + RECIPIENT_BAND_H
}

/// The bottom of the compose band.
#[inline(always)]
fn compose_band_y1(app: &MessagesApp) -> usize {
    bands_top(app) + BANDS_H
}

/// The recipient field's rectangle.
#[inline(always)]
fn recipient_field(app: &MessagesApp) -> (usize, usize, usize, usize) {
    (
        24,
        recipient_band_y0(app),
        RECIPIENT_FIELD_W,
        RECIPIENT_FIELD_H,
    )
}

/// The compose field's rectangle.
#[inline(always)]
fn compose_field(app: &MessagesApp) -> (usize, usize, usize, usize) {
    (
        24,
        compose_band_y0(app) + 14,
        COMPOSE_FIELD_W,
        COMPOSE_FIELD_H,
    )
}

/// The recipient picker's control, at the right of the recipient band.
#[inline(always)]
fn pick_control(app: &MessagesApp) -> (usize, usize, usize, usize) {
    (616, recipient_band_y0(app), 80, MIN_TOUCH_TARGET)
}

/// The centre of the send button.
#[inline(always)]
fn send_cy(app: &MessagesApp) -> usize {
    compose_band_y0(app) + 44
}

/// The height of the thread list's visible band.
#[inline(always)]
fn list_viewport_h(app: &MessagesApp) -> usize {
    bands_top(app) - CARD_Y0
}

/// The height of the conversation's visible band.
#[inline(always)]
fn convo_viewport_h(app: &MessagesApp) -> usize {
    bands_top(app) - CONVO_TOP
}

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

/// The width a thread row's bold peer name may use.
///
/// The run starts at `CARD_X0 + 84` and has to clear the unread pill's left
/// edge at `CARD_X1 - 92`; the 12 px slack absorbs bold advances being wider
/// than the regular advances the trim is measured with.
const PEER_NAME_CAP: usize = 96;
const PEER_NAME_MAX_W: usize = (CARD_X1 - 92) - (CARD_X0 + 84) - 12;

/// The height of the thread list's visible band when the keyboard is down.
///
/// WHY it is derived from the band's edges and not typed in: the band is
/// bounded by the card row above it and the recipient field below it, and a
/// typed-in height drifts away from both the first time either moves.
const LIST_VIEWPORT_H: usize = BANDS_TOP_IDLE - CARD_Y0;

/// The top of the conversation's visible band.
const CONVO_TOP: usize = 180;

/// The height of the conversation's visible band when the keyboard is down.
const CONVO_VIEWPORT_H: usize = BANDS_TOP_IDLE - CONVO_TOP;

/// The height of a chat bubble.
const BUBBLE_H: usize = 76;

/// The narrowest a bubble may be drawn, so a one-word message still reads as
/// a bubble and not as a floating chip.
const BUBBLE_MIN_W: usize = 168;

/// The widest a bubble may be drawn. Long messages still wrap short of the
/// screen edge instead of touching it.
const BUBBLE_MAX_W: usize = 488;

/// The gap between two chat bubbles.
const BUBBLE_GAP: usize = 14;

/// The distance from one bubble's top to the next one's top.
const BUBBLE_STEP: usize = BUBBLE_H + BUBBLE_GAP;

/// The longest line of notice text the app shows.
const NOTICE_CAP: usize = 96;

// Details and recipient-picker geometry
const DETAILS_TOP: usize = 180;
const DETAIL_ROW_H: usize = 96;
const DETAIL_ROWS: usize = 12;
const RECIP_ROW_TOP: usize = 180;
const RECIP_ROW_H: usize = 88;
const RECIP_ROWS: usize = 12;
const PICK_LABEL: &[u8] = b"PICK";
const INFO_LABEL: &[u8] = b"INFO";

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
    let r = channel((THEME.primary >> 16) & 0xFF, (THEME.surface_container >> 16) & 0xFF);
    let g = channel((THEME.primary >> 8) & 0xFF, (THEME.surface_container >> 8) & 0xFF);
    let b = channel(THEME.primary & 0xFF, THEME.surface_container & 0xFF);
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
        color: THEME.on_surface_variant,
    };
    if raw_len == 0 {
        f.len = std::design::font::ellipsize(placeholder, scale, max_w, &mut f.buf);
    } else {
        f.color = THEME.on_surface;
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
        return Some(THEME.primary);
    }
    None
}

/// What tapping a row of the thread details screen does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailAction {
    /// The row is a fact and does nothing.
    None,
    /// Publish the thread as a document the device can find.
    Publish,
    /// Hand the published document to the app that handles it.
    HandOff,
    /// Turn the thread's previews in the list on or off.
    TogglePreviews,
    /// Hand off phone number to Dialer and launch it.
    CallContact,
}

/// One row of the thread details screen.
#[derive(Clone, Copy)]
struct DetailLine {
    label: &'static [u8],
    value: [u8; 72],
    value_len: usize,
    action: DetailAction,
}

/// One row of the recipient picker.
#[derive(Clone, Copy)]
struct PickerLine {
    title: [u8; 32],
    title_len: usize,
    value: [u8; 72],
    value_len: usize,
    /// Whether tapping the row fills the recipient field.
    selectable: bool,
}

impl PickerLine {
    pub const EMPTY: Self = Self {
        title: [0; 32],
        title_len: 0,
        value: [0; 72],
        value_len: 0,
        selectable: false,
    };

    pub fn title(&self) -> &[u8] {
        &self.title[..self.title_len]
    }
}

/// Adds one row to the details screen.
fn detail_push(
    out: &mut [DetailLine],
    n: &mut usize,
    label: &'static [u8],
    value: &[u8],
    action: DetailAction,
) {
    if *n >= out.len() {
        return;
    }
    let line = &mut out[*n];
    line.label = label;
    let v = value.len().min(line.value.len());
    line.value[..v].copy_from_slice(&value[..v]);
    line.value_len = v;
    line.action = action;
    *n += 1;
}

/// Adds one row to the recipient picker.
fn picker_push(
    out: &mut [PickerLine],
    n: &mut usize,
    title: &[u8],
    value: &[u8],
    selectable: bool,
) {
    if *n >= out.len() {
        return;
    }
    let line = &mut out[*n];
    let t = title.len().min(line.title.len());
    line.title[..t].copy_from_slice(&title[..t]);
    line.title_len = t;
    let v = value.len().min(line.value.len());
    line.value[..v].copy_from_slice(&value[..v]);
    line.value_len = v;
    line.selectable = selectable;
    *n += 1;
}

/// Builds the rows of the thread details screen.
///
/// WHY the rows say what the platform does not offer: the design document
/// asks for read receipts, a notification control and attachments, and this
/// app has none of them. A row that named a control the app cannot honour
/// would be a dead control, so the row states the fact instead.
fn build_detail_lines(app: &MessagesApp, out: &mut [DetailLine; DETAIL_ROWS]) -> usize {
    let mut n = 0usize;
    let mut count_buf = [0u8; 8];

    let (peer, messages_count, unread) = match app.active_thread_id {
        Some(tid) => match app.ledger.get_thread(tid) {
            Some(tr) => (
                tr.peer(),
                tr.message_count,
                tr.unread_count,
            ),
            None => (b"(no thread)".as_ref(), 0, 0),
        },
        None => (b"(no thread)".as_ref(), 0, 0),
    };

    detail_push(out, &mut n, b"Recipient", peer, DetailAction::None);
    let cn = put_usize(&mut count_buf, messages_count as usize);
    detail_push(out, &mut n, b"Messages", &count_buf[..cn], DetailAction::None);
    let un = put_usize(&mut count_buf, unread as usize);
    detail_push(out, &mut n, b"Unread", &count_buf[..un], DetailAction::None);
    detail_push(
        out,
        &mut n,
        b"Transport",
        b"None bound to this app",
        DetailAction::None,
    );
    detail_push(
        out,
        &mut n,
        b"Stored in",
        b"Settings store; vault sector 202",
        DetailAction::None,
    );
    if app.published_path_len > 0 {
        detail_push(
            out,
            &mut n,
            b"Published document",
            &app.published_path[..app.published_path_len],
            DetailAction::Publish,
        );
    } else {
        detail_push(
            out,
            &mut n,
            b"Published document",
            b"Not published yet",
            DetailAction::Publish,
        );
    }
    let hidden = app
        .active_thread_id
        .and_then(|tid| app.thread_ledger_index(tid))
        .map(|idx| app.hidden_previews[idx])
        .unwrap_or(false);
    detail_push(
        out,
        &mut n,
        b"Previews in the list",
        if hidden { b"Hidden" } else { b"Shown" },
        DetailAction::TogglePreviews,
    );
    detail_push(
        out,
        &mut n,
        b"Hand to another app",
        b"Share the published document",
        DetailAction::HandOff,
    );
    let peer = if app.recip_len > 0 {
        &app.recip[..app.recip_len]
    } else if let Some(tid) = app.active_thread_id {
        app.ledger.get_thread(tid).map(|tr| tr.peer()).unwrap_or(&[])
    } else {
        &[]
    };
    if messages::is_dialable(peer) {
        detail_push(
            out,
            &mut n,
            b"Call via Dialer",
            b"Launch Dialer to place cellular call",
            DetailAction::CallContact,
        );
    }
    detail_push(
        out,
        &mut n,
        b"Read receipts",
        b"Unavailable: no transport is bound",
        DetailAction::None,
    );
    detail_push(
        out,
        &mut n,
        b"Notifications",
        b"Unavailable: no channel is bound",
        DetailAction::None,
    );
    detail_push(
        out,
        &mut n,
        b"Attachments",
        b"Unavailable: no attachment service",
        DetailAction::None,
    );
    detail_push(
        out,
        &mut n,
        b"Group participants",
        b"Unavailable: no transport is bound",
        DetailAction::None,
    );
    n
}

/// Builds the rows of the recipient picker.
///
/// WHY the picker offers what the platform can produce and no more: the
/// device has no contacts store, so the only recipients it can name are the
/// one the dialer handed over and the peers of the conversations this device
/// already holds. The row that says so is what keeps the picker from
/// looking like a directory of contacts it cannot see.
fn build_picker_lines(app: &MessagesApp, out: &mut [PickerLine; RECIP_ROWS]) -> usize {
    let mut n = 0usize;
    if app.handoff_len > 0 {
        picker_push(
            out,
            &mut n,
            b"From Dialer",
            &app.handoff[..app.handoff_len],
            true,
        );
    }
    let mut saved = [messages::contacts::SavedContact::EMPTY; messages::contacts::MAX_SAVED_CONTACTS];
    let saved_count = messages::contacts::load_saved_contacts(&mut saved);
    for i in 0..saved_count {
        if n >= RECIP_ROWS {
            break;
        }
        let c = &saved[i];
        picker_push(out, &mut n, c.name(), c.number(), true);
    }
    let mut shown = 0usize;
    for i in 0..app.ledger.thread_count {
        if n >= RECIP_ROWS || shown >= 6 {
            break;
        }
        let tr = &app.ledger.threads[i];
        let peer = tr.peer();
        let already = saved[..saved_count].iter().any(|c| messages::contacts::phone_numbers_match(c.number(), peer));
        if !already {
            picker_push(out, &mut n, b"Recent conversation", peer, true);
            shown += 1;
        }
    }
    if n == 0 {
        picker_push(
            out,
            &mut n,
            b"No contacts saved",
            b"Add contacts in Dialer or enter number",
            false,
        );
    }
    n
}

/// The pixel of the recipient picker's control.
///
/// WHY it is a pill and not a text link: the platform's control treatment
/// for a small, always-available action is a pill, and a control that
/// borrowed another shape would not match the shell.
#[inline(always)]
fn pick_control_pixel(
    x: usize,
    y: usize,
    rect: (usize, usize, usize, usize),
    label: &[u8],
) -> Option<u32> {
    if !in_rect(rect, x, y) {
        return None;
    }
    let rect = primitives::Rect::new(rect.0, rect.1, rect.2, rect.3);
    let label_w = std::design::font::measure(label, 1);
    let label_x = rect.x + (rect.w.saturating_sub(label_w)) / 2;
    let label_y = rect.y + (rect.h.saturating_sub(std::design::font::line_height(1))) / 2;
    if let Some(c) = test_text(
        x,
        y,
        label_x,
        label_y,
        1,
        label,
        THEME.on_surface,
        THEME.surface_container_high,
    ) {
        return Some(c);
    }
    monochrome::control_px(x, y, rect, THEME.surface_container_high, Some(THEME.border))
}

/// The pixel of a card-shaped row carrying a caption line and a body line.
///
/// WHY the card treatment is the platform's: a row that invents its own
/// fill and its own hairline is a row a third-party app cannot match, and
/// the platform already says what a card looks like.
#[inline(always)]
fn card_row_pixel(
    x: usize,
    y: usize,
    rect: (usize, usize, usize, usize),
    label: &[u8],
    value: &[u8],
) -> Option<u32> {
    let fx = rect.0;
    let fy = rect.1;
    if x < fx || x >= fx + rect.2 || y < fy || y >= fy + rect.3 {
        return None;
    }
    // WHY the platform's card and not a hand-drawn one: this function used
    // to cut the corners itself and then draw the hairline as "the nearest
    // edge is within two pixels". The cut came first and ate the hairline
    // at the corners, so every card in this app had a broken outline, and
    // the outline was a second implementation of what the platform already
    // draws. The platform's card has a continuous hairline that follows the
    // corner, and a third-party app that draws a card gets the same one.
    if let Some(c) = monochrome::card_px(
        x,
        y,
        primitives::Rect::new(fx, fy, rect.2, rect.3),
        CARD.fill,
        CARD.border,
    ) {
        let max_w = rect.2.saturating_sub(48);
        let mut lbl_buf = [0u8; 64];
        let lbl_len = std::design::font::ellipsize(label, 1, max_w, &mut lbl_buf);
        let lbl_disp = if lbl_len > 0 { &lbl_buf[..lbl_len] } else { label };
        if let Some(t) = test_text(x, y, fx + 24, fy + 16, 1, lbl_disp, THEME.on_surface_variant, CARD.fill) {
            return Some(t);
        }
        let mut val_buf = [0u8; 96];
        let val_len = std::design::font::ellipsize(value, 2, max_w, &mut val_buf);
        let val_disp = if val_len > 0 { &val_buf[..val_len] } else { value };
        if let Some(t) = test_text(x, y, fx + 24, fy + 44, 2, val_disp, THEME.on_surface, CARD.fill) {
            return Some(t);
        }
        return Some(c);
    }
    None
}

/// Capacity of the header title buffer. A contact name is at most
/// `contacts::CONTACT_NAME_MAX` bytes; a raw peer handle is what remains.
const TITLE_CAP: usize = 96;

struct Frame {
    pill: PillFrame,
    /// The header title, already cut to the space left of the status pill.
    ///
    /// WHY it is cut here and not while drawing: the limit is the pill's left
    /// edge, and that edge only exists once the pill has been measured.
    title: [u8; TITLE_CAP],
    title_len: usize,
    search: EditorFrame,
    recipient: EditorFrame,
    compose: EditorFrame,
    filtered: Option<usize>,
    /// The threat level of each thread's newest message.
    ///
    /// WHY it is computed once per frame and not once per pixel: the
    /// analyzer scans the message against a keyword list, and doing that for
    /// every pixel of the list would cost a screen of keyword scans for one
    /// answer.
    threat: [gsm::ThreatLevel; messages::MAX_THREADS],
    /// The rows of the thread details screen.
    detail: [DetailLine; DETAIL_ROWS],
    detail_count: usize,
    /// The rows of the recipient picker.
    picker: [PickerLine; RECIP_ROWS],
    picker_count: usize,
    contacts: [messages::contacts::SavedContact; messages::contacts::MAX_SAVED_CONTACTS],
    contacts_count: usize,
    clock: [u8; 5],
    clock_ok: bool,
}

impl Frame {
    pub fn resolve_contact_name<'a>(&'a self, peer: &'a [u8]) -> &'a [u8] {
        if let Some(c) = messages::contacts::find_contact_by_number(&self.contacts, self.contacts_count, peer) {
            c.name()
        } else {
            peer
        }
    }
}

/// The header line of the current view, before it is cut to the pill.
///
/// WHY it takes the contacts table instead of `&Frame`: the title is measured
/// against the pill, and the pill is built before the frame exists.
fn header_title<'a>(
    messages: &'a MessagesApp,
    contacts: &'a [messages::contacts::SavedContact],
    contacts_count: usize,
) -> &'a [u8] {
    match messages.view_mode {
        ViewMode::ThreadList => TITLE_TEXT,
        ViewMode::Conversation => {
            if let Some(tid) = messages.active_thread_id {
                if let Some(tr) = messages.ledger.get_thread(tid) {
                    if let Some(c) =
                        messages::contacts::find_contact_by_number(contacts, contacts_count, tr.peer())
                    {
                        c.name()
                    } else {
                        tr.peer()
                    }
                } else {
                    TITLE_TEXT
                }
            } else {
                TITLE_TEXT
            }
        }
        ViewMode::Details => b"Thread details",
        ViewMode::Recipients => b"Choose a recipient",
    }
}

fn build_frame(messages: &MessagesApp) -> Frame {
    let mut clock = [b' '; 5];
    let mut clock_ok = false;
    if let Some(now) = std::rtc::Reading::now() {
        clock = now.clock_text();
        clock_ok = true;
    }
    let search_y = (SEARCH_FIELD.1 + SEARCH_FIELD.3 / 2)
        .saturating_sub(std::design::font::line_height(SEARCH_TEXT_SCALE) / 2);
    let recipient_rect = recipient_field(messages);
    let compose_rect = compose_field(messages);
    let recipient_y = (recipient_rect.1 + recipient_rect.3 / 2)
        .saturating_sub(std::design::font::line_height(RECIPIENT_TEXT_SCALE) / 2);
    let compose_y = (compose_rect.1 + compose_rect.3 / 2)
        .saturating_sub(std::design::font::line_height(COMPOSE_TEXT_SCALE) / 2);
    let filtered = if messages.search_len > 0 && messages.match_count() == 0 {
        None
    } else {
        Some(messages.active_thread_idx)
    };
    let mut threat = [gsm::ThreatLevel::Safe; messages::MAX_THREADS];
    for i in 0..messages.ledger.thread_count {
        if i >= threat.len() {
            break;
        }
        if let Some(last) = messages.ledger.latest_message(messages.ledger.threads[i].id) {
            threat[i] = gsm::analyze_threat(last.text()).level;
        }
    }
    let mut detail = [DetailLine {
        label: b"",
        value: [0; 72],
        value_len: 0,
        action: DetailAction::None,
    }; DETAIL_ROWS];
    let detail_count = build_detail_lines(messages, &mut detail);
    let mut picker = [PickerLine::EMPTY; RECIP_ROWS];
    let picker_count = build_picker_lines(messages, &mut picker);
    let mut contacts = [messages::contacts::SavedContact::EMPTY; messages::contacts::MAX_SAVED_CONTACTS];
    let contacts_count = messages::contacts::load_saved_contacts(&mut contacts);
    let pill = build_pill(messages);
    let mut title = [0u8; TITLE_CAP];
    let title_len = std::design::font::ellipsize(
        header_title(messages, &contacts, contacts_count),
        TITLE_SCALE,
        pill.left.saturating_sub(TITLE_X + 12),
        &mut title,
    );
    Frame {
        pill,
        title,
        title_len,
        search: build_editor(
            &messages.search,
            messages.search_len,
            SEARCH_PLACEHOLDER,
            SEARCH_TEXT_SCALE,
            SEARCH_TEXT_MAX_W,
            SEARCH_TEXT_X,
            search_y,
            messages.keyboard.is_focused(Field::Search),
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
            messages.keyboard.is_focused(Field::Recipient),
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
            messages.keyboard.is_focused(Field::Body),
            messages.compose_select_all,
        ),
        filtered,
        threat,
        detail,
        detail_count,
        picker,
        picker_count,
        contacts,
        contacts_count,
        clock,
        clock_ok,
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
    THEME.muted // Slate
}

fn thread_matches(tr: &messages::ThreadRecord, query: &[u8]) -> bool {
    messages::thread_matches(tr, query)
}

/// Master pixel scanout pipeline (zero heap allocations).
const STATUS_BAR_HEIGHT: usize = interos_std::design::viewport::STATUS_BAR_HEIGHT;

pub fn status_band() -> (usize, usize) {
    (0, STATUS_BAR_HEIGHT)
}

#[inline(never)]
fn get_messages_pixel(x: usize, y: usize, messages: &MessagesApp, f: &Frame) -> u32 {
    // Status Band (y < STATUS_BAR_HEIGHT)
    if y < STATUS_BAR_HEIGHT {
        let band = interos_std::design::components::StatusBand::new(W);
        let clock = if f.clock_ok { Some(&f.clock[..]) } else { None };
        let cellular = if messages.modem_present {
            interos_std::design::components::Reading::Measured(100)
        } else {
            interos_std::design::components::Reading::Unbound
        };
        let instruments = interos_std::design::components::Instruments {
            cellular,
            wifi: interos_std::design::components::Reading::Unbound,
            battery: interos_std::design::components::Reading::Unbound,
        };
        return band
            .pixel(x, y, clock, &instruments, &THEME)
            .unwrap_or(THEME.surface);
    }
    // 0. The on-screen keyboard, docked at the bottom of the screen.
    //
    // WHY the keyboard is drawn here and not under the bands: the keyboard
    // is the topmost thing on the screen while it is up, and the bands it
    // would otherwise cover have moved above it.
    if messages.keyboard.is_visible() && y >= KB_Y0 && y < KB_Y1 {
        return messages
            .keyboard
            .widget()
            .render_pixel(x, y, &THEME)
            .unwrap_or(THEME.surface);
    }

    // 1. Header (y 0..96)
    if y < HEADER_Y1 {
        if y >= 94 {
            return THEME.border;
        }

        // Standardized back button squircle (24..72, 24..72)
        if let Some(c) = std::design::back_button::pixel(x, y, THEME.on_surface, THEME.border, THEME.surface_container_high) {
            return c;
        }

        // Electric red dot accent at (96, 48)
        let dot_dx = x as i32 - 96;
        let dot_dy = y as i32 - 48;
        if dot_dx * dot_dx + dot_dy * dot_dy <= 16 {
            return THEME.error;
        }

        // Header Title: already cut to the space left of the status pill.
        if y >= TITLE_Y && y < TITLE_Y + std::design::font::line_height(TITLE_SCALE) {
            if let Some(c) = test_text(
                x,
                y,
                TITLE_X,
                TITLE_Y,
                TITLE_SCALE,
                &f.title[..f.title_len],
                THEME.on_surface,
                THEME.surface_container,
            ) {
                return c;
            }
        }

        // Status pill, top-right: truthful NO TRANSPORT / outcome
        if x >= f.pill.left && x < PILL_RIGHT && y >= PILL_TOP && y < PILL_BOTTOM {
            let rect = primitives::Rect::new(f.pill.left, PILL_TOP, PILL_RIGHT - f.pill.left, PILL_BOTTOM - PILL_TOP);
            if let Some(c) = test_text_bold(
                x,
                y,
                f.pill.text_x,
                f.pill.text_y,
                f.pill.scale,
                f.pill.label,
                THEME.on_surface_variant,
                THEME.surface_container_high,
            ) {
                return c;
            }
            if let Some(c) = monochrome::chip_px(x, y, rect, THEME.surface_container_high, Some(THEME.border)) {
                return c;
            }
            return THEME.surface_container;
        }

        return THEME.surface_container;
    }

    // -------------------------------------------------------------------------
    // ViewMode 1: ThreadListView
    // -------------------------------------------------------------------------
    if messages.view_mode == ViewMode::ThreadList {
        // 2. Search & Filter Bar (y 104..156)
        if y >= SEARCH_Y0 && y < SEARCH_Y1 {
            if in_rect(SEARCH_FIELD, x, y) {
                let rect = primitives::Rect::new(SEARCH_FIELD.0, SEARCH_FIELD.1, SEARCH_FIELD.2, SEARCH_FIELD.3);

                // Vector magnifying glass icon at (cx: 56, cy: 130)
                let cx = 56;
                let cy = 130;
                let dx = (x as isize - cx as isize).abs();
                let dy = (y as isize - cy as isize).abs();
                let d_sq = dx * dx + dy * dy;
                if d_sq >= 20 && d_sq <= 48 {
                    return THEME.on_surface_variant;
                }
                let hx = x as isize - (cx + 5) as isize;
                let hy = y as isize - (cy + 5) as isize;
                if hx >= 0 && hx <= 8 && hy >= 0 && hy <= 8 && (hx - hy).abs() <= 1 {
                    return THEME.on_surface_variant;
                }

                if messages.search_len > 0 && x >= 656 && x < 688 && y >= 116 && y < 144 {
                    let cx = 672isize;
                    let cy = 130isize;
                    let dx = (x as isize - cx).abs();
                    let dy = (y as isize - cy).abs();
                    if dx <= 4 && dy <= 4 && dx == dy {
                        return THEME.on_surface_variant;
                    }
                }
                if let Some(c) = editor_pixel(x, y, &f.search, messages.keyboard.is_focused(Field::Search), THEME.surface_container) {
                    return c;
                }

                if let Some(c) = monochrome::card_px(x, y, rect, THEME.surface_container, Some(THEME.border)) {
                    return c;
                }
            }
            return THEME.surface;
        }

        // 3. Thread List / Empty State (y 168..1288)
        //
        // WHY the list band starts at the first card and not at 168: the row
        // math below subtracts CARD_Y0, so a pixel in the 12-row gap above the
        // first card underflowed and killed the task on the first paint of a
        // list that had a thread in it.
        if y >= CARD_Y0 && y < recipient_band_y0(messages) {
            // Filter matched nothing
            if f.filtered.is_none() {
                if in_rect(EMPTY_CARD, x, y) {
                    // WHY the platform's card: this app drew the empty state's own
                    // outline -- a corner cut that ate the hairline, and a rule drawn
                    // as "the nearest edge is within two pixels". A rule drawn that
                    // way has no corners, so the outline broke at every corner, and
                    // it was a second implementation of a card the platform already
                    // draws. The platform's card has one outline that follows the
                    // corner.

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
                        THEME.on_surface,
                        THEME.surface_container,
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
                        THEME.on_surface_variant,
                        THEME.surface_container,
                    ) {
                        return c;
                    }
                    return monochrome::card_px(
                        x,
                        y,
                        primitives::Rect::new(
                            EMPTY_CARD.0,
                            EMPTY_CARD.1,
                            EMPTY_CARD.2,
                            EMPTY_CARD.3,
                        ),
                        CARD.fill,
                        CARD.border,
                    )
                    .unwrap_or(THEME.surface);
                }
                return THEME.surface;
            }

            // Zero conversation threads: Honest Empty State Card
            if messages.ledger.thread_count == 0 {
                if in_rect(EMPTY_CARD, x, y) {
                    // WHY the platform's card: this app drew the empty state's own
                    // outline -- a corner cut that ate the hairline, and a rule drawn
                    // as "the nearest edge is within two pixels". A rule drawn that
                    // way has no corners, so the outline broke at every corner, and
                    // it was a second implementation of a card the platform already
                    // draws. The platform's card has one outline that follows the
                    // corner.

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
                                return THEME.primary;
                            }
                            if y >= cy - 8 && y < cy - 2 && x >= cx - 40 && x < cx + 16 {
                                return THEME.border;
                            }
                            if y >= cy + 6 && y < cy + 12 && x >= cx - 40 && x < cx - 4 {
                                return THEME.border;
                            }
                            let is_rim = bx < 2 || by < 2 || (in_corner && {
                                let cdx = (18 - bx) as isize;
                                let cdy = (18 - by) as isize;
                                cdx * cdx + cdy * cdy >= 256
                            });
                            if is_rim {
                                return THEME.border;
                            }
                            return THEME.surface_container_high;
                        }
                    }

                    // Speech bubble pointer tail at bottom-left
                    if x >= cx - 44 && x < cx - 18 && y >= cy + 30 && y < cy + 48 {
                        let rel_x = (x - (cx - 44)) as isize;
                        let rel_y = (y - (cy + 30)) as isize;
                        if rel_x >= (rel_y * 24) / 18 {
                            if rel_x == (rel_y * 24) / 18 || x == cx - 19 || y == cy + 47 {
                                return THEME.border;
                            }
                            return THEME.surface_container_high;
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
                        THEME.on_surface,
                        THEME.surface_container,
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
                            THEME.warning,
                        ),
                        store::StoreState::Invalid => (
                            b"Stored conversations failed validation",
                            THEME.error,
                        ),
                        store::StoreState::Full => (
                            b"Conversation store is full; older threads were not kept",
                            THEME.warning,
                        ),
                        _ => (b"Messaging unavailable: no transport", THEME.on_surface_variant),
                    };
                    if let Some(c) = test_text(
                        x,
                        y,
                        EMPTY_CARD.0 + std::design::font::center_x(subline, 1, EMPTY_CARD.2),
                        810,
                        1,
                        subline,
                        subline_color,
                        THEME.surface_container,
                    ) {
                        return c;
                    }

                    return monochrome::card_px(
                        x,
                        y,
                        primitives::Rect::new(
                            EMPTY_CARD.0,
                            EMPTY_CARD.1,
                            EMPTY_CARD.2,
                            EMPTY_CARD.3,
                        ),
                        THEME.surface_container,
                        Some(THEME.border),
                    )
                    .unwrap_or(THEME.surface);
                }
            } else {
                // Render conversation thread rows
                //
                // WHY the offset is added back: the rows are drawn at their
                // scrolled position, so the row under the finger is the one
                // whose scrolled band contains it.
                let scroll = messages.list_scroll.offset;
                let rel = (y - CARD_Y0) + scroll;
                let idx = rel / CARD_H;
                let card_top = CARD_Y0 + idx * CARD_H - scroll;
                let card_offset_y = rel % CARD_H;
                if card_offset_y < CARD_BODY_H && x >= CARD_X0 && x < CARD_X1 {
                    let card_rect = primitives::Rect::new(CARD_X0, card_top, CARD_X1 - CARD_X0, CARD_BODY_H);
                    if let Some(ledger_idx) = messages.filtered_index(idx) {
                        if let Some(tr) = messages.ledger.get_thread_by_idx(ledger_idx) {
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
                                    THEME.ink_on_dark,
                                    avatar_color(tr.peer()),
                                ) {
                                    return c;
                                }
                                return avatar_color(tr.peer());
                            }

                            // 2. Peer handle / name (Scale 1 Bold with 10px spacing above snippet)
                            let display_peer = f.resolve_contact_name(tr.peer());
                            let mut peer_name = [0u8; PEER_NAME_CAP];
                            let peer_len = std::design::font::ellipsize(
                                display_peer,
                                2,
                                PEER_NAME_MAX_W,
                                &mut peer_name,
                            );
                            if let Some(c) = test_text_bold(
                                x,
                                y,
                                CARD_X0 + 84,
                                card_top + 22,
                                2,
                                &peer_name[..peer_len],
                                THEME.on_surface,
                                THEME.surface_container,
                            ) {
                                return c;
                            }

                            // 3. Last message preview
                            //
                            // WHY a hidden thread shows a fixed word: the
                            // privacy setting exists so that the message is
                            // not on the screen, and a preview that was
                            // merely shortened would still be the message.
                            let mut snippet = [0u8; 48];
                            let slen = if messages.hidden_previews[ledger_idx] {
                                copy_ascii(HIDDEN_PREVIEW_LABEL, &mut snippet)
                            } else if let Some(last_msg) = messages.ledger.latest_message(tr.id) {
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

                            if let Some(c) = test_text_bold(
                                x,
                                y,
                                CARD_X0 + 84,
                                card_top + 56,
                                1,
                                &snippet[..slen],
                                THEME.on_surface_variant,
                                THEME.surface_container,
                            ) {
                                return c;
                            }

                            // 4. Security indicator badge dot
                            //
                            // WHY the level comes from the frame and not
                            // from the drawn snippet: the snippet is
                            // truncated and, for a thread whose previews are
                            // hidden, it is not the message at all. The badge
                            // describes the message.
                            //
                            // WHY only a non-clean message is badged: a green
                            // dot in the avatar's corner is read as "online"
                            // or "unread", so marking every clean thread with
                            // one made the safe case the noise and the warning
                            // case invisible. A clean thread carries no dot;
                            // the details view still reports VERIFIED CLEAN.
                            let threat = f.threat[ledger_idx];
                            if threat != gsm::ThreatLevel::Safe {
                                let s_dx = x as isize - (CARD_X0 + 20) as isize;
                                let s_dy = y as isize - (card_top + 24) as isize;
                                if s_dx * s_dx + s_dy * s_dy <= 16 {
                                    return threat.color();
                                }
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
                                    if let Some(c) = test_text_bold(
                                        x,
                                        y,
                                        px0 + 12,
                                        py0 + 5,
                                        1,
                                        &digit[..dn],
                                        THEME.ink_on_dark,
                                        THEME.primary,
                                    ) {
                                        return c;
                                    }
                                    return THEME.primary;
                                }
                            }

                            // 6. Delete affordance 'x' glyph
                            let dxc = CARD_X1 - 38;
                            let dyc = card_top + 50;
                            let gx = x as isize - dxc as isize;
                            let gy = y as isize - dyc as isize;
                            // WHY both diagonals: a single stroke is a slash,
                            // not an X, and a slash at the end of a row reads
                            // as a stray mark rather than a delete control.
                            let on_stroke = (gx - gy).abs() <= 2 || (gx + gy).abs() <= 2;
                            if gx.abs() <= 6 && gy.abs() <= 6 && on_stroke {
                                return THEME.primary;
                            }

                            return monochrome::card_px(x, y, card_rect, CARD.fill, CARD.border).unwrap_or(THEME.surface);
                        }
                    }
                }
            }
            return THEME.surface;
        }

        // 3b. Recipient editor band (directly above the compose band)
        if y >= recipient_band_y0(messages) && y < recipient_band_y1(messages) {
            let rfield = recipient_field(messages);
            if in_rect(rfield, x, y) {
                let rect = primitives::Rect::new(rfield.0, rfield.1, rfield.2, rfield.3);
                if let Some(c) = test_text(x, y, 40, f.recipient.y, 1, b"To:", THEME.on_surface_variant, THEME.surface_container) {
                    return c;
                }
                if let Some(c) = editor_pixel(x, y, &f.recipient, messages.keyboard.is_focused(Field::Recipient), THEME.surface_container) {
                    return c;
                }

                // Clear 'x' indicator when recipient has text
                if messages.recip_len > 0 && x >= 574 && x < 606 && y >= recipient_band_y0(messages) + 6 && y < recipient_band_y1(messages) - 6 {
                    let cx = 590isize;
                    let cy = (recipient_band_y0(messages) + 22) as isize;
                    let dx = (x as isize - cx).abs();
                    let dy = (y as isize - cy).abs();
                    if dx <= 4 && dy <= 4 && dx == dy {
                        return THEME.on_surface_variant;
                    }
                }

                if let Some(c) = monochrome::card_px(x, y, rect, THEME.surface_container, Some(THEME.border)) {
                    return c;
                }
            }
            if let Some(c) = pick_control_pixel(x, y, pick_control(messages), PICK_LABEL) {
                return c;
            }
            return THEME.surface;
        }

        // 4. Compose Input Bar (directly below the recipient band)
        if y >= compose_band_y0(messages) && y < compose_band_y1(messages) {
            let cfield = compose_field(messages);
            if in_rect(cfield, x, y) {
                let rect = primitives::Rect::new(cfield.0, cfield.1, cfield.2, cfield.3);
                // Clear 'x' indicator when compose has text
                if messages.compose_len > 0 && x >= 574 && x < 606 && y >= cfield.1 + 10 && y < cfield.1 + 50 {
                    let cx = 590isize;
                    let cy = (cfield.1 + 30) as isize;
                    let dx = (x as isize - cx).abs();
                    let dy = (y as isize - cy).abs();
                    if dx <= 4 && dy <= 4 && dx == dy {
                        return THEME.on_surface_variant;
                    }
                }
                if let Some(c) = editor_pixel(x, y, &f.compose, messages.keyboard.is_focused(Field::Body), THEME.surface_container) {
                    return c;
                }
                // The count belongs to the composer it counts, not to the
                // recipient row above it, and belongs to nothing while the
                // composer is empty.
                if messages.compose_len > 0 {
                    let mut count_buf = [0u8; 8];
                    let cn = format_char_count(messages.compose_len, &mut count_buf);
                    let cw = std::design::font::measure(&count_buf[..cn], 1);
                    if let Some(c) = test_text(
                        x,
                        y,
                        COMPOSE_COUNT_RIGHT.saturating_sub(cw),
                        f.compose.y,
                        1,
                        &count_buf[..cn],
                        THEME.on_surface_variant,
                        THEME.surface_container,
                    ) {
                        return c;
                    }
                }
                if let Some(c) = monochrome::card_px(x, y, rect, THEME.surface_container, Some(THEME.border)) {
                    return c;
                }
            }

            // Send control circle
            if x >= 630 && x < 694 && y >= compose_band_y0(messages) + 14 && y < compose_band_y1(messages) - 10 {
                let dx = (x as isize) - SEND_CX as isize;
                let dy = (y as isize) - send_cy(messages) as isize;
                if dx * dx + dy * dy <= (SEND_R * SEND_R) as isize {
                    let live = messages.compose_len > 0;
                    let bg = if live { THEME.primary } else { THEME.surface_container_low };
                    let ink = if live { THEME.ink_on_dark } else { THEME.on_surface_variant };
                    if send_arrow_ink(dx, dy) {
                        return ink;
                    }
                    return bg;
                }
                return THEME.surface;
            }
            return THEME.surface;
        }
    } else if messages.view_mode == ViewMode::Conversation {
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
                    THEME.error_container,
                    THEME.error,
                    // WHY it names a local keyword list: the analyzer holds
                    // a fixed list of phishing words and a fixed list of
                    // link prefixes and matches the text against them. There
                    // is no database of known scam scripts on this device,
                    // and a badge that implies one would claim more
                    // evidence than the code has.
                    b"Caution: text matches local phishing keywords" as &[u8],
                )
            } else if flagged_urgency {
                (
                    THEME.warning_container,
                    THEME.warning,
                    b"Caution: text matches urgency language" as &[u8],
                )
            } else {
                // WHY this is no longer "Verified": the banner sat next to
                // a refusal status and a "no transport" state, and no
                // handshake, key exchange or peer contact ever happened.
                // The thread is local and unsent; that is what it can say.
                (THEME.surface_container, THEME.border, b"Local thread - not delivered" as &[u8])
            };

            if in_rect(SEARCH_FIELD, x, y) {
                let rect = primitives::Rect::new(SEARCH_FIELD.0, SEARCH_FIELD.1, SEARCH_FIELD.2, SEARCH_FIELD.3);

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
                    if show_threat { THEME.ink_on_dark } else { THEME.on_surface_variant },
                    bar_bg,
                ) {
                    return c;
                }

                if let Some(c) = monochrome::card_px(x, y, rect, bar_bg, Some(bar_border)) {
                    return c;
                }
            }
            return THEME.surface;
        }

        // Chat Message Bubbles Area (from below the info bar to the bands)
        if y >= 168 && y < recipient_band_y0(messages) {
            let active_id = messages.active_thread_id.unwrap_or(0);
            let mut msgs = [MessageRecord::EMPTY; messages::MAX_MESSAGES_PER_THREAD];
            let count = messages.ledger.get_thread_messages(active_id, &mut msgs);

            if count == 0 {
                // Empty conversation info banner
                let line1 = b"No messages in this chat yet";
                let line2 = b"Type a draft below to compose";
                let l1_w = std::design::font::measure_bold(line1, 1);
                let l2_w = std::design::font::measure(line2, 1);
                if let Some(c) = test_text_bold(x, y, (W.saturating_sub(l1_w)) / 2, 676, 1, line1, THEME.on_surface, THEME.surface) {
                    return c;
                }
                if let Some(c) = test_text(x, y, (W.saturating_sub(l2_w)) / 2, 716, 1, line2, THEME.on_surface_variant, THEME.surface) {
                    return c;
                }
            } else {
                // The bubbles are laid out from the top of the band and the
                // scroll model moves them.
                //
                // WHY they are not bottom-aligned any more: a thread holds
                // sixteen messages and the band holds twelve, so the oldest
                // four were drawn off the top of the screen with no way to
                // reach them. The offset is the model's, so the band, the
                // thumb and the tap targets cannot disagree.
                let scroll = messages.convo_scroll.offset;
                for i in 0..count {
                    let b_top = CONVO_TOP + i * BUBBLE_STEP - scroll;
                    let b_bot = b_top + BUBBLE_H;
                    if y >= b_top && y < b_bot {
                        let is_incoming = msgs[i].direction == MessageDirection::Incoming;
                        let (fill_color, border_color, ink_color) = if is_incoming {
                            (THEME.surface_container_high, THEME.border, THEME.on_surface)
                        } else {
                            (THEME.inverted_surface(), THEME.inverted_surface(), THEME.ink_on_dark)
                        };

                        // Subline / Status indicator
                        let (status_label, status_color) = if is_incoming {
                            (INCOMING_LOCAL_LABEL, THEME.on_surface_variant)
                        } else {
                            match msgs[i].status {
                                DeliveryStatus::RefusedNoTransport => {
                                    (b"Refused - No Transport" as &[u8], THEME.warning)
                                }
                                DeliveryStatus::Sent => (b"Sent" as &[u8], THEME.success),
                                DeliveryStatus::Failed => (b"Failed" as &[u8], THEME.error),
                                DeliveryStatus::LocalOnly => {
                                    (b"Local only - not sent" as &[u8], THEME.muted)
                                }
                                DeliveryStatus::Pending => (b"Not sent" as &[u8], THEME.muted),
                            }
                        };

                        // WHY the bubble hugs its content: a fixed 488 px slab
                        // turned a five-letter message into a full-width black
                        // bar, so the thread read as bars instead of a
                        // conversation. The bubble is the wider of the body
                        // and the status line, clamped to the band.
                        let mut bubble_w = std::design::font::measure_bold(msgs[i].text(), 2) + 32;
                        let status_w = std::design::font::measure_bold(status_label, 1) + 32;
                        if status_w > bubble_w {
                            bubble_w = status_w;
                        }
                        bubble_w = bubble_w.clamp(BUBBLE_MIN_W, BUBBLE_MAX_W);
                        let (bx0, bx1) = if is_incoming {
                            (32usize, 32 + bubble_w)
                        } else {
                            (688 - bubble_w, 688usize)
                        };

                        if x >= bx0 && x < bx1 {
                            let bubble_rect = primitives::Rect::new(bx0, b_top, bx1 - bx0, BUBBLE_H);

                            // Message body text (scale 2)
                            let mut buf = [0u8; 96];
                            let n = std::design::font::ellipsize(
                                msgs[i].text(),
                                2,
                                bx1 - bx0 - 32,
                                &mut buf,
                            );
                            if let Some(c) = test_text_bold(
                                x,
                                y,
                                bx0 + 16,
                                b_top + 10,
                                2,
                                &buf[..n],
                                ink_color,
                                fill_color,
                            ) {
                                return c;
                            }

                            if let Some(c) = test_text_bold(
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

                            if let Some(c) = primitives::draw_rounded_rect(
                                x,
                                y,
                                bubble_rect,
                                R_SOFT,
                                fill_color,
                                Some(border_color),
                                1,
                            ) {
                                return c;
                            }
                            return fill_color;
                        }
                    }
                }
            }
            return THEME.surface;
        }

        // Draft Bar: Recipient row (directly above the compose band)
        if y >= recipient_band_y0(messages) && y < recipient_band_y1(messages) {
            let rfield = recipient_field(messages);
            if in_rect(rfield, x, y) {
                let rect = primitives::Rect::new(rfield.0, rfield.1, rfield.2, rfield.3);
                if let Some(c) = test_text(x, y, 40, f.recipient.y, 1, b"To:", THEME.on_surface_variant, THEME.surface_container) {
                    return c;
                }
                if let Some(c) = editor_pixel(x, y, &f.recipient, messages.keyboard.is_focused(Field::Recipient), THEME.surface_container) {
                    return c;
                }

                // Clear 'x' indicator when recipient has text
                if messages.recip_len > 0 && x >= 574 && x < 606 && y >= recipient_band_y0(messages) + 6 && y < recipient_band_y1(messages) - 6 {
                    let cx = 590isize;
                    let cy = (recipient_band_y0(messages) + 22) as isize;
                    let dx = (x as isize - cx).abs();
                    let dy = (y as isize - cy).abs();
                    if dx <= 4 && dy <= 4 && dx == dy {
                        return THEME.on_surface_variant;
                    }
                }

                if let Some(c) = monochrome::card_px(x, y, rect, THEME.surface_container, Some(THEME.border)) {
                    return c;
                }
            }
            if let Some(c) = pick_control_pixel(x, y, pick_control(messages), INFO_LABEL) {
                return c;
            }
            return THEME.surface;
        }

        // Compose input field (directly below the recipient band)
        if y >= compose_band_y0(messages) && y < compose_band_y1(messages) {
            let cfield = compose_field(messages);
            if in_rect(cfield, x, y) {
                let rect = primitives::Rect::new(cfield.0, cfield.1, cfield.2, cfield.3);
                // Clear 'x' indicator when compose has text
                if messages.compose_len > 0 && x >= 574 && x < 606 && y >= cfield.1 + 10 && y < cfield.1 + 50 {
                    let cx = 590isize;
                    let cy = (cfield.1 + 30) as isize;
                    let dx = (x as isize - cx).abs();
                    let dy = (y as isize - cy).abs();
                    if dx <= 4 && dy <= 4 && dx == dy {
                        return THEME.on_surface_variant;
                    }
                }
                if let Some(c) = editor_pixel(x, y, &f.compose, messages.keyboard.is_focused(Field::Body), THEME.surface_container) {
                    return c;
                }
                if messages.compose_len > 0 {
                    let mut count_buf = [0u8; 8];
                    let cn = format_char_count(messages.compose_len, &mut count_buf);
                    let cw = std::design::font::measure(&count_buf[..cn], 1);
                    if let Some(c) = test_text(
                        x,
                        y,
                        COMPOSE_COUNT_RIGHT.saturating_sub(cw),
                        f.compose.y,
                        1,
                        &count_buf[..cn],
                        THEME.on_surface_variant,
                        THEME.surface_container,
                    ) {
                        return c;
                    }
                }
                if let Some(c) = monochrome::card_px(x, y, rect, THEME.surface_container, Some(THEME.border)) {
                    return c;
                }
            }

            // Send control circle
            if x >= 630 && x < 694 && y >= compose_band_y0(messages) + 14 && y < compose_band_y1(messages) - 10 {
                let dx = (x as isize) - SEND_CX as isize;
                let dy = (y as isize) - send_cy(messages) as isize;
                if dx * dx + dy * dy <= (SEND_R * SEND_R) as isize {
                    let live = messages.compose_len > 0;
                    let bg = if live { THEME.primary } else { THEME.surface_container_low };
                    let ink = if live { THEME.ink_on_dark } else { THEME.on_surface_variant };
                    if send_arrow_ink(dx, dy) {
                        return ink;
                    }
                    return bg;
                }
                return THEME.surface;
            }
            return THEME.surface;
        }
    } else if messages.view_mode == ViewMode::Details {
        // ---------------------------------------------------------------------
        // ViewMode 3: ThreadDetailsView
        //
        // WHY the rows are cards and not a form: the screen is a report of
        // what the app knows about the thread, and the platform's card
        // treatment is what a report is drawn on.
        // ---------------------------------------------------------------------
        if y >= DETAILS_TOP && y < recipient_band_y0(messages) {
            for row in 0..f.detail_count {
                let top = DETAILS_TOP + row * DETAIL_ROW_H;
                if y >= top && y < top + DETAIL_ROW_H {
                    let line = &f.detail[row];
                    let rect = (CARD_X0, top, CARD_X1 - CARD_X0, DETAIL_ROW_H - 8);
                    if let Some(c) = card_row_pixel(
                        x,
                        y,
                        rect,
                        line.label,
                        &line.value[..line.value_len],
                    ) {
                        return c;
                    }
                }
            }
            return THEME.surface;
        }
        if y >= recipient_band_y0(messages) && y < compose_band_y1(messages) {
            // The notice band: what the last action on this screen did.
            let rect = (CARD_X0, recipient_band_y0(messages) + 16, CARD_X1 - CARD_X0, 96);
            if x >= rect.0 && x < rect.0 + rect.2 && y >= rect.1 && y < rect.1 + rect.3 {
                let card_rect = primitives::Rect::new(rect.0, rect.1, rect.2, rect.3);
                let mut buf = [0u8; 96];
                let n = if messages.notice_len > 0 {
                    std::design::font::ellipsize(
                        &messages.notice[..messages.notice_len],
                        1,
                        rect.2 - 48,
                        &mut buf,
                    )
                } else {
                    std::design::font::ellipsize(
                        b"Nothing has been published or shared yet.",
                        1,
                        rect.2 - 48,
                        &mut buf,
                    )
                };
                if let Some(c) = test_text(
                    x,
                    y,
                    rect.0 + 24,
                    rect.1 + 40,
                    1,
                    &buf[..n],
                    THEME.on_surface_variant,
                    CARD.fill,
                ) {
                    return c;
                }
                return monochrome::card_px(
                    x,
                    y,
                    card_rect,
                    CARD.fill,
                    CARD.border,
                )
                .unwrap_or(THEME.surface);
            }
            return THEME.surface;
        }
    } else {
        // ---------------------------------------------------------------------
        // ViewMode 4: RecipientPickerView
        //
        // WHY the rows are the recipients the platform can produce: the
        // device has no contacts store, so the picker offers the dialer's
        // hand-off and the peers of the conversations this device holds, and
        // says that no other source exists.
        // ---------------------------------------------------------------------
        if y >= RECIP_ROW_TOP && y < recipient_band_y0(messages) {
            for row in 0..f.picker_count {
                let top = RECIP_ROW_TOP + row * RECIP_ROW_H;
                if y >= top && y < top + RECIP_ROW_H {
                    let line = &f.picker[row];
                    let rect = (CARD_X0, top, CARD_X1 - CARD_X0, RECIP_ROW_H - 8);
                    if let Some(c) = card_row_pixel(
                        x,
                        y,
                        rect,
                        line.title(),
                        &line.value[..line.value_len],
                    ) {
                        return c;
                    }
                }
            }
            return THEME.surface;
        }
        if y >= recipient_band_y0(messages) && y < compose_band_y1(messages) {
            // The chosen address stays on screen, so the user can see what
            // the row they tapped filled in.
            let rect = (CARD_X0, recipient_band_y0(messages) + 16, CARD_X1 - CARD_X0, 96);
            if x >= rect.0 && x < rect.0 + rect.2 && y >= rect.1 && y < rect.1 + rect.3 {
                let card_rect = primitives::Rect::new(rect.0, rect.1, rect.2, rect.3);
                let mut buf = [0u8; 64];
                let n = if messages.recip_len > 0 {
                    std::design::font::ellipsize(
                        &messages.recip[..messages.recip_len],
                        1,
                        rect.2 - 48,
                        &mut buf,
                    )
                } else {
                    copy_ascii(b"No recipient chosen", &mut buf)
                };
                if let Some(c) = test_text(
                    x,
                    y,
                    rect.0 + 24,
                    rect.1 + 40,
                    1,
                    &buf[..n],
                    THEME.on_surface,
                    CARD.fill,
                ) {
                    return c;
                }
                return monochrome::card_px(
                    x,
                    y,
                    card_rect,
                    CARD.fill,
                    CARD.border,
                )
                .unwrap_or(THEME.surface);
            }
            return THEME.surface;
        }
    }

    // 5. Home Gesture Pill (x: 240..480, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= HOME_X0 && x < HOME_X1 {
        return THEME.on_surface_variant;
    }

    THEME.surface
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
    /// The on-screen keyboard's own pixels: the shift state changed.
    Keyboard,
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

/// Puts the composer in front of the user.
///
/// WHY the keyboard comes up with the focus: the field takes focus and
/// nothing else appeared, so there was no way to type a message at all. A
/// focused field and a visible keyboard are the same state.
fn focus_compose(app: &mut MessagesApp) {
    if !app.keyboard.is_focused(Field::Body) {
        app.compose_select_all = app.compose_len > 0;
    }
    app.keyboard.focus(Field::Body);
    app.search_select_all = false;
    app.recip_select_all = false;
    sync_scroll_extents(app);
}

/// Puts the thread list's search field in front of the user.
fn focus_search(app: &mut MessagesApp) {
    if !app.keyboard.is_focused(Field::Search) {
        app.search_select_all = app.search_len > 0;
    }
    app.keyboard.focus(Field::Search);
    app.compose_select_all = false;
    app.recip_select_all = false;
    sync_scroll_extents(app);
}

/// Puts the recipient field in front of the user.
fn focus_recipient(app: &mut MessagesApp) {
    if !app.keyboard.is_focused(Field::Recipient) {
        app.recip_select_all = app.recip_len > 0;
    }
    app.keyboard.focus(Field::Recipient);
    app.compose_select_all = false;
    app.search_select_all = false;
    sync_scroll_extents(app);
}

/// Takes the keyboard off the screen and gives the field its caret back.
///
/// WHY the focus goes with it: a field that is still focused with no
/// keyboard on the screen is the dead end this keyboard was added to end.
fn dismiss_keyboard(app: &mut MessagesApp) {
    app.keyboard.dismiss();
    app.compose_select_all = false;
    app.search_select_all = false;
    app.recip_select_all = false;
    sync_scroll_extents(app);
}

/// Re-derives the scroll extents after the keyboard changed the visible band.
///
/// WHY the extents are re-derived and not left as they were: the keyboard
/// takes the bottom of the screen while it is up, so the visible band is
/// shorter then, and a scroll extent built for the tall band would let the
/// list scroll under the keyboard.
fn sync_scroll_extents(app: &mut MessagesApp) {
    app.sync_list_extent();
    app.sync_convo_extent();
}

/// Applies a key of the on-screen keyboard to the field it types into.
///
/// WHY the send button is not the keyboard's enter key: sending is the one
/// action in this app that asks a transport whether it happened, and it
/// belongs to the control that says so out loud.
fn apply_keyboard_key(app: &mut MessagesApp, action: KeyAction) -> Dirty {
    match action {
        KeyAction::Shift => {
            app.keyboard.toggle_shift();
            Dirty::Keyboard
        }
        KeyAction::Hide | KeyAction::Enter => {
            dismiss_keyboard(app);
            Dirty::All
        }
        KeyAction::Char(_) | KeyAction::Backspace => {
            // WHY the row count is taken before the fields are borrowed:
            // the search field's value decides how many rows the list
            // draws, and the list cannot be read while the field is
            // borrowed.
            let searching = app.keyboard.is_focused(Field::Search);
            let before = if searching { app.match_count() } else { 0 };
            let changed = {
                let mut editors = Editors {
                    search: Editor {
                        buf: &mut app.search,
                        len: &mut app.search_len,
                        select_all: &mut app.search_select_all,
                    },
                    recipient: Editor {
                        buf: &mut app.recip,
                        len: &mut app.recip_len,
                        select_all: &mut app.recip_select_all,
                    },
                    body: Editor {
                        buf: &mut app.compose,
                        len: &mut app.compose_len,
                        select_all: &mut app.compose_select_all,
                    },
                };
                app.keyboard.apply_to(action, &mut editors)
            };
            if !changed {
                return Dirty::None;
            }
            if searching {
                // WHY the extent is re-derived: the number of rows the list
                // draws changed, and a scroll extent that is stale would
                // let the list scroll past its last row.
                if app.match_count() != before {
                    app.sync_list_extent();
                }
                // WHY the search field redraws the whole screen and not the
                // keyboard's band: the search field sits at the top of the
                // screen, far from the dock, and the list under it changes
                // with what was typed.
                return Dirty::All;
            }
            // WHY the other two fields redraw the keyboard's band: the
            // two editor bands sit directly above the dock, so one
            // band covers the field that changed and the key that was
            // pressed.
            Dirty::Keyboard
        }
    }
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

/// Publishes the thread as a document in the shared storage area.
///
/// WHY the app publishes the thread: the sealed copy lives in a vault
/// sector, and a vault sector is a secret no other app can read. The
/// design document asks for the sealed thread to be readable, so the thread
/// is also written into the shared storage area, where the file manager
/// lists it and the launcher's search finds it.
fn publish_thread(app: &mut MessagesApp, tid: u32) {
    let mut msgs = [MessageRecord::EMPTY; messages::MAX_MESSAGES_PER_THREAD];
    let count = app.ledger.get_thread_messages(tid, &mut msgs);
    let mut peer = [0u8; messages::MAX_PEER_HANDLE_LEN];
    let mut peer_len = 0usize;
    if let Some(tr) = app.ledger.get_thread(tid) {
        peer_len = tr.peer().len().min(peer.len());
        peer[..peer_len].copy_from_slice(&tr.peer()[..peer_len]);
    }
    let (outcome, path, path_len) = publish::publish(tid, &peer[..peer_len], &msgs[..count]);
    app.published_path[..path_len].copy_from_slice(&path[..path_len]);
    app.published_path_len = path_len;
    match outcome {
        publish::PublishOutcome::Published => {
            app.set_notice(b"Published: the file manager and the launcher can find this thread.");
        }
        publish::PublishOutcome::IndexRefused => {
            app.set_notice(b"Published, but the device refused to refresh the document index.");
        }
        publish::PublishOutcome::WriteRefused => {
            app.set_notice(b"The device refused the write; the thread was not published.");
        }
    }
}

/// Hands the published document to the app that handles its media type.
fn hand_off_thread(app: &mut MessagesApp) {
    if app.published_path_len == 0 {
        if let Some(tid) = app.active_thread_id {
            publish_thread(app, tid);
        }
    }
    if app.published_path_len == 0 {
        app.set_notice(b"Publish the thread before handing it to another app.");
        return;
    }
    let Ok(path) = core::str::from_utf8(&app.published_path[..app.published_path_len]) else {
        app.set_notice(b"The published path is not readable; nothing was handed over.");
        return;
    };
    match publish::hand_to_handler(path) {
        publish::HandOutcome::Handed => {
            app.set_notice(b"Handed to the app that declared this document type.");
        }
        publish::HandOutcome::NoHandler => {
            app.set_notice(b"No app on this device declares a handler for this document.");
        }
        publish::HandOutcome::DispatchRefused => {
            app.set_notice(b"The platform refused the hand-over.");
        }
    }
}

/// Turns the open thread's previews in the thread list on or off.
fn toggle_previews(app: &mut MessagesApp) {
    let Some(tid) = app.active_thread_id else {
        return;
    };
    let mut peer = [0u8; messages::MAX_PEER_HANDLE_LEN];
    let mut peer_len = 0usize;
    if let Some(tr) = app.ledger.get_thread(tid) {
        peer_len = tr.peer().len().min(peer.len());
        peer[..peer_len].copy_from_slice(&tr.peer()[..peer_len]);
    }
    let idx = app.thread_ledger_index(tid);
    let hidden = idx.map(|i| app.hidden_previews[i]).unwrap_or(false);
    let now_hidden = !hidden;
    if privacy::set_previews_hidden(&peer[..peer_len], now_hidden) {
        if let Some(i) = idx {
            app.hidden_previews[i] = now_hidden;
        }
        app.set_notice(if now_hidden {
            b"Previews hidden in the thread list."
        } else {
            b"Previews shown in the thread list."
        });
    } else {
        app.set_notice(b"The device refused to store the privacy setting.");
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

    // 6. Publish the thread so the file manager and the launcher can find it.
    if let Some(tid) = app.active_thread_id {
        publish_thread(app, tid);
    }

    // 7. Transition to conversation view
    app.view_mode = ViewMode::Conversation;
    app.compose_len = 0;
    app.compose_select_all = false;
    app.recip_select_all = false;
    // WHY the keyboard goes away with the composer: the message is gone, and
    // a keyboard over a conversation with nothing to type into is a dead
    // control covering the thread.
    dismiss_keyboard(app);

    // WHY the conversation is scrolled to its newest message: the message
    // the user just sent is at the end of the thread, and a view that
    // stayed where it was would not show it.
    app.scroll_convo_to_newest();

    Dirty::All
}

fn handle_tap(app: &mut MessagesApp, px: u32, py: u32) -> Tap<'static> {
    // WHY the pressed key is released before the touch is handled: the
    // kernel's input pipeline reports a tap, not a press and a release, so
    // a key that is drawn pressed would stay pressed until the next touch.
    // The key that was touched last goes back up when the next touch
    // arrives, and the keyboard is redrawn so the highlight goes with it.
    let had_pressed = app.keyboard.pressed().is_some();
    app.keyboard.set_pressed(None);
    let tap = handle_tap_inner(app, px, py);
    if !had_pressed {
        return tap;
    }
    match tap {
        Tap::None => Tap::Redraw(Dirty::Keyboard),
        Tap::Redraw(Dirty::All) => Tap::Redraw(Dirty::All),
        Tap::Redraw(d) => Tap::Redraw(merge(d, Dirty::Keyboard)),
        Tap::Leave(msg) => Tap::Leave(msg),
    }
}

fn handle_tap_inner(app: &mut MessagesApp, px: u32, py: u32) -> Tap<'static> {
    let (x, y) = (px as usize, py as usize);

    // System home gesture: leave to launcher
    if x >= HOME_X0 && x < HOME_X1 && y >= HOME_Y0 {
        return Tap::Leave("[MESSAGES APP] Home gesture pressed. Exiting to launcher.");
    }

    // Header Back button (24..72, 24..72)
    if std::design::back_button::contains(x, y) || (x >= 24 && x < 100 && y >= 28 && y < 68) {
        match app.view_mode {
            ViewMode::Conversation => {
                // Return to conversation directory
                app.view_mode = ViewMode::ThreadList;
                dismiss_keyboard(app);
                std::log("[MESSAGES APP] Back pressed: Navigating to ThreadListView.").ok();
                return Tap::Redraw(Dirty::All);
            }
            ViewMode::Details => {
                app.view_mode = ViewMode::Conversation;
                std::log("[MESSAGES APP] Back pressed: Returning to the conversation.").ok();
                return Tap::Redraw(Dirty::All);
            }
            ViewMode::Recipients => {
                app.view_mode = app.picker_return;
                std::log("[MESSAGES APP] Back pressed: Leaving the recipient picker.").ok();
                return Tap::Redraw(Dirty::All);
            }
            ViewMode::ThreadList => {
                return Tap::Leave("[MESSAGES APP] Back control pressed. Exiting to launcher.");
            }
        }
    }

    // The on-screen keyboard, docked at the bottom of the screen.
    //
    // WHY it is handled before everything below it: the keyboard is the
    // topmost thing on the screen while it is up, and a press that fell
    // through to the screen behind it would type into a field the user was
    // not looking at.
    if app.keyboard.is_visible() && y >= KB_Y0 && y < KB_Y1 {
        let Some(action) = app.keyboard.action_at(x, y) else {
            // A press in the dock's margin is not a key.
            return Tap::Redraw(Dirty::Keyboard);
        };
        app.keyboard.set_pressed(Some(action));
        let dirty = match apply_keyboard_key(app, action) {
            // WHY a key that changed nothing still repaints the dock: the
            // key is drawn pressed, and a press that repainted nothing
            // would leave the key highlighted until the next touch.
            Dirty::None => Dirty::Keyboard,
            other => other,
        };
        let _ = std::audio_haptic_click();
        return Tap::Redraw(dirty);
    }

    // The thread details screen's own rows.
    if app.view_mode == ViewMode::Details && y >= DETAILS_TOP && y < recipient_band_y0(app) {
        let mut detail = [DetailLine {
            label: b"",
            value: [0; 72],
            value_len: 0,
            action: DetailAction::None,
        }; DETAIL_ROWS];
        let count = build_detail_lines(app, &mut detail);
        let row = (y - DETAILS_TOP) / DETAIL_ROW_H;
        if row < count {
            match detail[row].action {
                DetailAction::None => {
                    // The recipient row is the one that opens the picker,
                    // and every other row is a fact about the thread.
                    if detail[row].label == b"Recipient" {
                        app.picker_return = ViewMode::Details;
                        app.view_mode = ViewMode::Recipients;
                        return Tap::Redraw(Dirty::All);
                    }
                }
                DetailAction::Publish => {
                    if let Some(tid) = app.active_thread_id {
                        publish_thread(app, tid);
                    } else {
                        app.set_notice(b"There is no thread to publish.");
                    }
                    return Tap::Redraw(Dirty::All);
                }
                DetailAction::HandOff => {
                    hand_off_thread(app);
                    return Tap::Redraw(Dirty::All);
                }
                DetailAction::CallContact => {
                    let peer = if app.recip_len > 0 {
                        &app.recip[..app.recip_len]
                    } else if let Some(tid) = app.active_thread_id {
                        app.ledger.get_thread(tid).map(|tr| tr.peer()).unwrap_or(&[])
                    } else {
                        &[]
                    };
                    if !peer.is_empty() {
                        if messages::contacts::hand_off_call_to_dialer(peer) {
                            app.set_notice(b"Calling via Dialer...");
                            std::log("[MESSAGES APP] Handed phone number to Dialer and launched it.").ok();
                        } else {
                            // The helper reports false for a refused number,
                            // a failed settings write or a refused launch.
                            app.set_notice(b"Could not open Dialer for this contact.");
                            std::log("[MESSAGES APP] Dialer hand-off was refused.").ok();
                        }
                    }
                    return Tap::Redraw(Dirty::All);
                }
                DetailAction::TogglePreviews => {
                    toggle_previews(app);
                    return Tap::Redraw(Dirty::All);
                }
            }
        }
    }

    // The recipient picker's rows.
    if app.view_mode == ViewMode::Recipients && y >= RECIP_ROW_TOP && y < recipient_band_y0(app) {
        let mut picker = [PickerLine::EMPTY; RECIP_ROWS];
        let count = build_picker_lines(app, &mut picker);
        let row = (y - RECIP_ROW_TOP) / RECIP_ROW_H;
        if row < count && picker[row].selectable {
            let n = picker[row].value_len.min(RECIP_CAP);
            app.recip[..n].copy_from_slice(&picker[row].value[..n]);
            app.recip_len = n;
            dismiss_keyboard(app);
            app.view_mode = app.picker_return;
            let _ = std::audio_haptic_click();
            std::log("[MESSAGES APP] A recipient was chosen from the picker.").ok();
            return Tap::Redraw(Dirty::All);
        }
    }

    // The control at the right of the recipient band: it opens the recipient
    // picker from the thread list and the thread's details from a
    // conversation.
    if in_rect(pick_control(app), x, y)
        && (app.view_mode == ViewMode::ThreadList || app.view_mode == ViewMode::Conversation)
    {
        if app.view_mode == ViewMode::ThreadList {
            app.picker_return = ViewMode::ThreadList;
            app.view_mode = ViewMode::Recipients;
        } else {
            app.view_mode = ViewMode::Details;
        }
        // WHY the keyboard goes away with the screen: the picker and the
        // details screen have no field to type into, and a keyboard with
        // nothing to type into is a dead control.
        dismiss_keyboard(app);
        let _ = std::audio_haptic_click();
        return Tap::Redraw(Dirty::All);
    }

    // Send button circle
    let sdx = x as isize - SEND_CX as isize;
    let sdy = y as isize - send_cy(app) as isize;
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
    if in_rect(compose_field(app), x, y) {
        if app.compose_len > 0 && x >= 570 && x < 610 {
            app.compose = [0; COMPOSE_CAP];
            app.compose_len = 0;
            app.compose_select_all = false;
            let _ = std::audio_haptic_click();
            std::log("[MESSAGES APP] Compose field cleared.").ok();
            return Tap::Redraw(Dirty::Compose);
        }
        focus_compose(app);
        let _ = std::audio_haptic_click();
        std::log("[MESSAGES APP] Compose field focused for keyboard input.").ok();
        // WHY the whole screen is redrawn: the keyboard coming up moves the
        // two bands and the content band with them.
        return Tap::Redraw(Dirty::All);
    }

    // Recipient text field
    if in_rect(recipient_field(app), x, y) {
        if app.recip_len > 0 && x >= 570 && x < 610 {
            app.recip = [0; RECIP_CAP];
            app.recip_len = 0;
            app.recip_select_all = false;
            let _ = std::audio_haptic_click();
            std::log("[MESSAGES APP] Recipient cleared.").ok();
            return Tap::Redraw(Dirty::All);
        }
        focus_recipient(app);
        let _ = std::audio_haptic_click();
        std::log("[MESSAGES APP] Recipient field focused for keyboard input.").ok();
        return Tap::Redraw(Dirty::All);
    }

    // ThreadListView specific touches
    if app.view_mode == ViewMode::ThreadList {
        // Search field
        if in_rect(SEARCH_FIELD, x, y) {
            if app.search_len > 0 && x >= 650 && x < 696 {
                app.search = [0; SEARCH_CAP];
                app.search_len = 0;
                app.search_select_all = false;
                let _ = std::audio_haptic_click();
                std::log("[MESSAGES APP] Search cleared.").ok();
                return Tap::Redraw(Dirty::All);
            }
            focus_search(app);
            let _ = std::audio_haptic_click();
            std::log("[MESSAGES APP] Search field focused for keyboard input.").ok();
            return Tap::Redraw(Dirty::All);
        }

        // Tapping a conversation row opens ConversationView
        //
        // WHY the offset is added back: the rows are drawn at their
        // scrolled position, so the row under the finger is the one whose
        // scrolled band contains it.
        if x >= CARD_X0 && x < CARD_X1 && y >= CARD_Y0 {
            let scroll = app.list_scroll.offset;
            let rel = (y - CARD_Y0) + scroll;
            let idx = rel / CARD_H;
            let coff = rel % CARD_H;
            if coff < CARD_BODY_H {
                if let Some(ledger_idx) = app.filtered_index(idx) {
                    if let Some(tr) = app.ledger.get_thread_by_idx(ledger_idx) {
                        // Delete affordance tap on card's right edge
                        //
                        // WHY the target is a 44 px square around a 6 px glyph:
                        // the glyph is the only sign of the action and the old
                        // 33 px box was under the platform's minimum. The part
                        // of the square over the unread pill is left to the row,
                        // so a tap on the counter still opens the thread.
                        let ddx = x as isize - (CARD_X1 - 38) as isize;
                        let ddy = coff as isize - 50;
                        let reach = (MIN_TOUCH_TARGET / 2) as isize;
                        let on_unread_pill = tr.unread_count > 0
                            && x >= CARD_X1 - 92
                            && x < CARD_X1 - 56
                            && coff >= 34
                            && coff < 68;
                        if ddx.abs() <= reach && ddy.abs() <= reach && !on_unread_pill {
                            let tid = tr.id;
                            app.ledger.remove_thread(tid);
                            app.store.remove(ledger_idx);
                            app.refresh_privacy();
                            app.sync_list_extent();
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
                        // WHY the keyboard goes away with the search field:
                        // the search field belongs to the thread list, so
                        // a keyboard still typing into it would type into
                        // a field the conversation does not draw.
                        dismiss_keyboard(app);
                        app.scroll_convo_to_newest();
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
        if app.keyboard.is_visible() {
            dismiss_keyboard(app);
            return Dirty::All;
        } else {
            match app.view_mode {
                ViewMode::Conversation => {
                    app.view_mode = ViewMode::ThreadList;
                    return Dirty::All;
                }
                ViewMode::Details => {
                    app.view_mode = ViewMode::Conversation;
                    return Dirty::All;
                }
                ViewMode::Recipients => {
                    app.view_mode = app.picker_return;
                    return Dirty::All;
                }
                ViewMode::ThreadList => {}
            }
        }
        return Dirty::None;
    }

    if !app.keyboard.is_visible() {
        return Dirty::None;
    }

    if app.keyboard.is_focused(Field::Recipient) {
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

    if app.keyboard.is_focused(Field::Body) {
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
        // WHY the extent is re-derived: the number of rows the list draws
        // changed, and a scroll extent that is stale would let the list
        // scroll past its last row.
        app.sync_list_extent();
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

    // The per-thread privacy settings, and the scroll extents that follow
    // from what the ledger now holds.
    messages.refresh_privacy();
    messages.sync_list_extent();
    messages.sync_convo_extent();

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
        match std::event_poll(interest_mask, 30) {
            Ok(events) => {
                if events & (std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK) != 0 {
                    let prev_recip_len = messages.recip_len;
                    let prev_keyboard = messages.keyboard.is_visible();
                    messages.take_handoff_recipient();
                    // WHY the keyboard is part of the condition: the
                    // hand-off focuses the recipient field, and a focused
                    // field raises the keyboard and moves the bands, so a
                    // screen that was not repainted would keep the bands
                    // where they were.
                    if (messages.recip_len != prev_recip_len
                        || messages.keyboard.is_visible() != prev_keyboard)
                        && surface > 0
                    {
                        render_messages_ui(surface, &messages);
                        let _ = std::surface_flip();
                    }
                }
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
                            // A drag that ends over a scrolling band is a
                            // scroll gesture; anything else is the system's
                            // navigation gesture.
                            //
                            // WHY the band needs the gesture: a thread holds
                            // sixteen messages and the visible band holds
                            // twelve, so the oldest four were drawn off the
                            // top of the screen with no way to reach them.
                            let over_conversation = messages.view_mode == ViewMode::Conversation
                                && input.y >= CONVO_TOP as u32
                                && (input.y as usize) < recipient_band_y0(&messages);
                            let over_list = messages.view_mode == ViewMode::ThreadList
                                && input.y >= CARD_Y0 as u32
                                && (input.y as usize) < recipient_band_y0(&messages);
                            // WHY the dock swallows the gesture: a pan that
                            // starts on the keys is a pan on a control that
                            // does not scroll, and a gesture the app does
                            // not recognise is a gesture that closes the
                            // app, so a thumb sliding over the keyboard
                            // would throw the user out of the compose
                            // screen.
                            let over_keyboard = messages.keyboard.is_visible()
                                && input.y >= KB_Y0 as u32
                                && input.y < KB_Y1 as u32;
                            if (input.kind == 4 || input.kind == 10) && over_keyboard {
                                continue;
                            }
                            if (input.kind == 4 || input.kind == 10) && (over_conversation || over_list) {
                                if over_conversation {
                                    messages.convo_scroll.apply_drag(input.dy);
                                } else {
                                    messages.list_scroll.apply_drag(input.dy);
                                }
                                dirty = merge(dirty, Dirty::All);
                                continue;
                            }
                            if input.kind == 6 {
                                // Back gesture walks one screen up.
                                match messages.view_mode {
                                    ViewMode::Conversation => {
                                        messages.view_mode = ViewMode::ThreadList;
                                        dirty = merge(dirty, Dirty::All);
                                        continue;
                                    }
                                    ViewMode::Details => {
                                        messages.view_mode = ViewMode::Conversation;
                                        dirty = merge(dirty, Dirty::All);
                                        continue;
                                    }
                                    ViewMode::Recipients => {
                                        messages.view_mode = messages.picker_return;
                                        dirty = merge(dirty, Dirty::All);
                                        continue;
                                    }
                                    ViewMode::ThreadList => {}
                                }
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
                                render_messages_band(
                                    surface,
                                    &messages,
                                    recipient_band_y0(&messages),
                                    recipient_band_y1(&messages),
                                );
                            }
                            Dirty::Compose => {
                                render_messages_band(
                                    surface,
                                    &messages,
                                    compose_band_y0(&messages),
                                    compose_band_y1(&messages),
                                );
                            }
                            Dirty::Keyboard => {
                                // WHY the region holds the dock and both
                                // positions of the bands: the bands move
                                // when the keyboard comes up and when it
                                // goes down, and a redraw that covered
                                // only the position they are in now would
                                // leave the pixels of the other one on
                                // the screen.
                                let (y0, y1) =
                                    messages::keyboard::dirty_region(&messages.keyboard);
                                render_messages_band(surface, &messages, y0, y1);
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
