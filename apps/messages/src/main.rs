// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
use interos_std::design::typeface;
use interos_std::design::scroll::ScrollState;
use interos_std::design::accessibility::{DynamicType, UserTextScale};

use messages::gsm;
use messages::keyboard::{
    Editor, Editors, Field, KeyAction, Keyboard, BANDS_H, BANDS_TOP_IDLE, BANDS_TOP_KEYBOARD,
    KB_Y0, KB_Y1,
};
use messages::layout;
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
///
/// WHY they are the library's: the character counter's widest line is the
/// compose capacity written out as `used/limit`, so the number a fit gate has
/// to measure is the same number the buffer is sized with, and a copy of it
/// beside the painter is a number no test can tie to the counter.
const COMPOSE_CAP: usize = layout::COMPOSE_CAP;
const SEARCH_CAP: usize = layout::SEARCH_CAP;
const RECIP_CAP: usize = layout::RECIP_CAP;

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
    /// Signal bars the baseband actually measured, 0..4; 0 means no reading.
    ///
    /// WHY this is separate from `modem_present`: presence says a radio is
    /// attached, it says nothing about the level, and a full set of bars from a
    /// boolean is a reading no instrument took.
    pub signal_bars: u8,
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
            signal_bars: 0,
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
            list_scroll: ScrollState::new(0, INITIAL_VIEWPORT_H, 0),
            convo_scroll: ScrollState::new(0, INITIAL_VIEWPORT_H, 0),
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
            rows.saturating_mul(card_h()),
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
            count.saturating_mul(bubble_step()),
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

    /// Probes Keystore Slot 3 for session key derivation.
    ///
    /// WHY this always reports refused: Under the Platform Truth Contract and kernel
    /// security policy (`sys_keystore_derive`), raw cryptographic key material never
    /// crosses the kernel/userspace boundary to Ring 3. The probe honestly records
    /// the kernel's refusal.
    pub fn derive_messaging_key(&mut self) -> bool {
        let mut key = [0u8; 32];
        match std::keystore_derive(3, &mut key) {
            Ok(32) => {
                self.keystore_derived = true;
                std::log("[MESSAGES APP] Hardware Keystore Slot 3: Ingested 256-bit P2P messaging key.").ok();
                true
            }
            Ok(_) => {
                std::log("[MESSAGES APP] Keystore Slot 3: short result; no messaging key available.").ok();
                false
            }
            Err(_) => {
                self.keystore_derived = false;
                std::log("[MESSAGES APP] Keystore Slot 3: derivation refused by kernel policy; raw keys never cross to Ring 3.").ok();
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

/// One unsigned integer out of a telemetry payload, keyed exactly as `key`.
///
/// WHY the app parses rather than assumes: the only honest cellular mark is
/// one the baseband actually measured. The dialer reads the same `"bars"` field
/// out of the same `telephony_query` payload, so both apps draw the radio's
/// level from the one number the HAL reported instead of the `present` boolean.
fn json_u8(json: &[u8], key: &str) -> Option<u8> {
    let pat = key.as_bytes();
    let mut i = 0usize;
    while i + pat.len() < json.len() {
        if &json[i..i + pat.len()] == pat
            && json[i + pat.len()] == b':'
            && i + pat.len() > 0
            && json[i + pat.len() - 1] == b'"'
        {
            let mut j = i + pat.len() + 1;
            let mut v: u32 = 0;
            let mut any = false;
            while j < json.len() && json[j].is_ascii_digit() {
                v = v.saturating_mul(10).saturating_add((json[j] - b'0') as u32);
                any = true;
                j += 1;
            }
            if any && v <= 255 {
                return Some(v as u8);
            }
        }
        i += 1;
    }
    None
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

// -----------------------------------------------------------------------------
// The person's text size, from the platform's store to this screen
//
// WHY this block exists beside the text faces rather than inside them:
// `font::cap_for_scale` is a `const fn` ladder, and a box written as
// `const X: usize = font::line_height(rung)` is folded at compile time and can
// therefore never see what the person chose. That is the whole of the defect
// this block closes -- a raised text size resized Settings and left every other
// application tiny. The fix has two halves and both are here: the rung is read
// once, before the first pixel is painted, and published into the renderer's
// own cell (`DynamicType::publish`); and every box this app lays out is
// re-derived through the `*_user` helpers beside the ladder, so the space a run
// reserves moves with the ink it draws.
//
// WHY the helpers answer the ladder's own number at the default rung: level one
// multiplies every cap by one hundred per cent, so an application that never
// published a rung -- and a person who never changed the setting -- draws and
// lays out exactly what this file drew before this block existed. Nothing here
// moves a pixel until the store names a different rung.
// -----------------------------------------------------------------------------

/// The settings key the platform's Settings app writes the text size to.
const TEXT_SCALE_KEY: &str = "display.text_scale";

/// Reads the person's text size and hands it to the renderer, answering whether
/// the rung actually moved.
///
/// WHY it is called before `surface_create` and again on every wake of the event
/// loop: the boxes below are derived from the published rung every time the
/// painter asks for one, so a rung that arrived mid-frame would move a row out
/// from under a finger already inside it -- and a rung read only at startup is a
/// screen that resizes on the next cold open, if ever, while the person changed
/// the size in Settings with this screen alive. Once per wake is also the only
/// place a settings query belongs: asking the store per glyph would put a syscall
/// inside the paint loop.
///
/// WHY `if let` and not a default: a value this app cannot read is not a level
/// the person chose, and publishing a rung the store never named would resize
/// the screen on a guess. The renderer's own default -- the ladder, at one
/// hundred per cent -- stands.
fn publish_user_text_scale() -> bool {
    let before = std::design::font::user_text_scale();
    let mut buf = [0u8; 32];
    if let Ok(n) = std::settings_get(TEXT_SCALE_KEY, &mut buf) {
        if let Some(level) = UserTextScale::from_settings_value(&buf[..n]) {
            let _ = DynamicType::new(level).publish();
        }
    }
    std::design::font::user_text_scale() != before
}

/// Whether the published rung is the ladder's own hundred per cent.
#[inline(always)]
fn draws_at_ladder() -> bool {
    std::design::font::user_text_scale() == std::design::font::TEXT_SCALE_NORMAL
}

/// The line box a rung reserves at the published level.
///
/// WHY every box below asks this instead of `font::line_height(rung)`: a run
/// drawn at a scaled cap inside an unscaled box is clipped text, which is the
/// class of defect the platform keeps being asked to fix. This is the box half
/// of the pair, and it is the ladder's own number at the default rung.
#[inline(always)]
fn line_h(scale: usize) -> usize {
    std::design::font::line_height_user(scale)
}

/// The rows from a run's box top to its baseline, at the published level.
#[inline(always)]
fn ascent_h(scale: usize) -> usize {
    std::design::font::ascent_for_scale_user(scale)
}

/// The cap height a rung draws at, at the published level.
#[inline(always)]
fn cap_h(scale: usize) -> u32 {
    std::design::font::cap_for_scale_user(scale)
}

/// The advance a run takes at the published level.
#[inline(always)]
fn run_w(text: &[u8], scale: usize, bold: bool) -> usize {
    if bold {
        std::design::font::measure_bold_user(text, scale)
    } else {
        std::design::font::measure_user(text, scale)
    }
}

/// The rows a rung's line box has gained over the ladder's default rung.
///
/// WHY a growth and not the box: the offsets inside a thread row, a bubble and
/// the unread pill are the designer's numbers -- 22, 56, 52, 34 -- and several of
/// them are not `pad + box + gap`, so rewriting them as a derived chain would
/// move ink on the default level. That is the one level every screenshot in the
/// review queue and every contract test's expected row was made against. This
/// answers nought there, and exactly the rows the raised rung adds everywhere
/// else, so the relationship between two runs of ink survives the person
/// choosing a bigger text size.
#[inline(always)]
fn line_growth(scale: usize) -> usize {
    line_h(scale).saturating_sub(std::design::font::line_height(scale))
}

/// The columns a run has gained over the ladder's default rung.
///
/// WHY it exists beside [`line_growth`]: the same rule in the other axis -- an
/// ink mark that is centred on a run's width keeps its shipped offset at the
/// default level and follows the published level from there.
#[inline(always)]
fn run_growth(text: &[u8], scale: usize, bold: bool) -> usize {
    let here = run_w(text, scale, bold);
    let ladder = if bold {
        std::design::font::measure_bold(text, scale)
    } else {
        std::design::font::measure(text, scale)
    };
    here.saturating_sub(ladder)
}

/// The widest `text` can be drawn inside `max_w` at the published level, with
/// `...` where it was cut. Writes the shown run into `out`.
///
/// WHY an application-side fit rather than `font::fit`: `font::fit` compares the
/// run against the lane at the *ladder's* advance, so at a raised rung a run it
/// judged to fit is in fact wider than the lane and paints over the mark or the
/// card edge the lane was measured against. The cut then happens where the ink
/// actually ends, and the run is marked rather than silently clipped.
///
/// WHY the ladder still hands the work to `font::ellipsize`: the shipped trims
/// are that function's arithmetic -- a budget spent in the outline face's
/// advances -- and re-deriving it here would move a cut on the default rung,
/// which is the one rung every contract test and every screenshot in the review
/// queue were made against. The scaled branch below is the same algorithm with
/// the em read from the published level.
fn fit_run(text: &[u8], scale: usize, max_w: usize, bold: bool, out: &mut [u8]) -> usize {
    if text.is_empty() || out.is_empty() || scale == 0 {
        return 0;
    }
    if draws_at_ladder() {
        return std::design::font::ellipsize(text, scale, max_w, out);
    }
    if run_w(text, scale, bold) <= max_w {
        let n = text.len().min(out.len());
        out[..n].copy_from_slice(&text[..n]);
        return n;
    }
    const DOTS: &[u8] = b"...";
    let dots = run_w(DOTS, scale, bold);
    if dots > max_w || out.len() < 3 {
        return 0;
    }
    // `measure_bold` folds the renderer's one-column dilation into the width it
    // returns, so the dots already carry it and the budget below needs no weight
    // term: the finished run is `sum + dots`, which the bound keeps inside `max_w`.
    //
    // WHY the mark's three bytes come off the plate before the walk spends it: the
    // copy below writes them at the end of whatever the lane took, so a bound that
    // left room for the dots alone lets one more glyph in and the mark lands past
    // the caller's end -- a panic in the painter, not a pixel in the wrong place.
    let em = std::design::font::em_for_scale_user(scale);
    let budget = max_w - dots;
    let fit_len = longest_fit(text, em, budget, out.len() - 3);
    out[..fit_len].copy_from_slice(&text[..fit_len]);
    out[fit_len..fit_len + 3].copy_from_slice(DOTS);
    fit_len + 3
}

/// The em the trim above spends its lane in: the ladder's at the default rung,
/// the published one everywhere else -- the same face `fit_run` reads.
fn trim_em(scale: usize) -> u32 {
    if draws_at_ladder() {
        std::design::font::em_for_scale(scale)
    } else {
        std::design::font::em_for_scale_user(scale)
    }
}

/// How many leading bytes of `text` a painter can ink inside `max_w` at `em`,
/// given `room` bytes of the caller's plate for them.
///
/// WHY the walk is the trim's own: the lane and the mark are the same width spent
/// twice, and a cut walked differently from the ellipsis disagrees with it about
/// what a glyph costs -- which is how a refused run ended up past its plate.
fn longest_fit(text: &[u8], em: u32, max_w: usize, room: usize) -> usize {
    let mut current_w = 0usize;
    let mut fit_len = 0usize;
    for (i, &c) in text.iter().enumerate() {
        let adv = typeface::advance_px(c, em) as usize;
        if current_w + adv > max_w || fit_len + 1 > room {
            break;
        }
        current_w += adv;
        fit_len = i + 1;
    }
    fit_len
}

/// The run a container hands its painter: `fit_run`'s answer when the lane took
/// the mark, and a hard cut to the lane when the lane refused it.
///
/// WHY a cut and not the label: `fit_run` answers `0` when even the ellipsis will
/// not sit in the lane, and handing the caller the untouched label then inks a run
/// past the plate that framed it -- the words a reader was promised in a card that
/// cannot hold them. The cut is the same answer the lane gives when it does have
/// room, minus the mark, and the bytes are copied into the caller's plate first:
/// a refusal is the one answer that writes nothing, so a cut read straight back out
/// of the buffer would be the stale zeros it was zeroed to.
fn fitted_run<'a>(
    text: &'a [u8],
    scale: usize,
    max_w: usize,
    bold: bool,
    out: &'a mut [u8],
) -> &'a [u8] {
    let len = fit_run(text, scale, max_w, bold, out);
    if len > 0 {
        return &out[..len];
    }
    if text.is_empty() || scale == 0 {
        return text;
    }
    let fit = longest_fit(text, trim_em(scale), max_w, out.len());
    out[..fit].copy_from_slice(&text[..fit]);
    &out[..fit]
}

/// The advance of the run `fit_run` would actually draw.
fn fitted_run_w(text: &[u8], scale: usize, max_w: usize, bold: bool) -> usize {
    let mut buf = [0u8; 96];
    let n = fit_run(text, scale, max_w, bold, &mut buf);
    run_w(&buf[..n], scale, bold)
}

/// A run of the outline face at the published level.
///
/// WHY a second face and not a change to [`test_text`]: the ladder face asks the
/// SDK's memoised `run_box` and `place_run`, both keyed on the rung and not on
/// the published level, so it answers the ladder's numbers whatever the person
/// chose. This face asks the same arithmetic the memo would have cached -- the
/// line box, the advance and the baseline at the published rung -- from the
/// `*_user` helpers beside the ladder.
fn user_ink(
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
    let h = line_h(scale);
    if y < y0 || y - y0 >= h || x < x0 || x >= W || x - x0 >= run_w(text, scale, bold) + 1 {
        return None;
    }
    let baseline = y0 + ascent_h(scale);
    let em = std::design::font::em_for_scale_user(scale);
    if let Some(run) = outline_text::OutlineRun::place_styled(x0, baseline, em, text, bold) {
        return run.pixel(x, y, color, bg);
    }
    std::design::font::test_text_aa_user(x, y, x0, y0, scale, text, color, bg, bold)
}

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
    // WHY the ladder path is kept and asked first: at the default rung the two
    // faces answer the same numbers, and only the ladder one runs through the
    // SDK's memoised box, which is what keeps a frame of this app's rows off a
    // per-pixel walk of the run. A raised rung takes [`user_ink`], whose
    // measurement follows the published level.
    if draws_at_ladder() {
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
        return std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, false);
    }
    user_ink(x, y, x0, y0, scale, text, color, bg, false)
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
    // Kept for the default rung for the reason given in [`test_text`].
    if draws_at_ladder() {
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
        return std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, true);
    }
    user_ink(x, y, x0, y0, scale, text, color, bg, true)
}

// -----------------------------------------------------------------------------
// Layout Geometry
// -----------------------------------------------------------------------------

/// The row the app's own chrome starts at.
///
/// WHY not the top of the panel: the platform band owns `0..48` and answers the
/// pixel function before any header content is reached.
const HEADER_TOP: usize = interos_std::design::viewport::HEADER_TOP;

/// The row the header's controls -- the back button and the status pill -- are
/// centred on.
const HEADER_ROW: usize = HEADER_TOP + std::design::back_button::SIZE / 2;

/// The later of two layout edges.
///
/// WHY a local helper rather than `Ord::max`: `max` is not callable in a `const`
/// context on this toolchain -- the platform's own `typeface::line_metrics`
/// records the same workaround -- and every edge below has to be a compile-time
/// constant, because the pixel function compares against it once per pixel.
const fn later(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}

const HEADER_Y0: usize = 0;
const TITLE_TEXT: &[u8] = b"Messages";
const TITLE_X: usize = layout::HEADER_TITLE_X;
const TITLE_SCALE: usize = type_scale::TITLE;

/// The rows the title's line box reserves: ascent + descent + leading.
///
/// WHY a function and not the constant this was: `const title_box_h() =
/// line_height(TITLE_SCALE)` is folded at compile time, so it was the one number
/// on this screen that could not see the person's text size -- the half of the
/// defect that left the ink growing inside an unchanged box (see
/// [`line_h`]).
#[inline(always)]
fn title_box_h() -> usize {
    line_h(TITLE_SCALE)
}

/// The title's line-box top.
///
/// WHY the cap height is centred on the control row instead of the box being
/// dropped at the top of the control band: `TITLE_SCALE` is rung 4, whose line
/// box -- ascent plus descent at the em the rung's cap implies -- is taller than
/// the 48-row back control beside it. The box used to start at `HEADER_TOP + 4`
/// and therefore ran past the header's hairline and under the search field,
/// which begins below it. That is the cut-off tail of the "g" in "Messages".
/// Centring the caps on the control row keeps the whole box inside the band, and
/// is the placement the dialer already documents and uses
/// (`apps/dialer/src/main.rs`, `TITLE_Y0`).
#[inline(always)]
fn title_y() -> usize {
    HEADER_ROW - ascent_h(TITLE_SCALE) + cap_h(TITLE_SCALE) as usize / 2
}

/// The air between the last row anything in the header may ink and its hairline.
const HEADER_PAD_BOTTOM: usize = 2;
/// The header's hairline, drawn on the band's last rows.
const HEADER_RULE_H: usize = 2;

/// The status pill's height: its own rung's line box, and never less than the
/// 40 rows it was designed with.
#[inline(always)]
fn pill_h() -> usize {
    later(PILL_H_ANCHOR, line_h(PILL_RUNG) + PILL_PAD_Y * 2)
}

const PILL_H_ANCHOR: usize = 40;
/// The rung the pill's label is inked at.
const PILL_RUNG: usize = type_scale::CAPTION;
/// The air the label keeps from the pill's own top and bottom edges.
const PILL_PAD_Y: usize = 4;

const PILL_RIGHT: usize = W - 24;
#[inline(always)]
fn pill_top() -> usize {
    HEADER_ROW - pill_h() / 2
}
#[inline(always)]
fn pill_bottom() -> usize {
    pill_top() + pill_h()
}
const PILL_PAD_X: usize = spacing::GRID_20;

/// Header band bottom, derived from the tallest thing standing in the band.
///
/// WHY derived and not typed as a literal: the band is only correct while it
/// contains the title's whole line box, the back control and the pill. The
/// literal held 112 after the title moved to rung 4 and grew past it, and
/// nothing complained -- the surface under the header simply painted over the
/// run's descent.
///
/// WHY a function and not the constant this was: every term of the sum is a
/// line box at the published rung, so a band folded at compile time is exactly
/// how a scaled title came to ink over the search field under it.
#[inline(always)]
fn header_content_bottom() -> usize {
    later(
        later(
            title_y() + title_box_h(),
            HEADER_TOP + std::design::back_button::SIZE,
        ),
        pill_bottom(),
    )
}

#[inline(always)]
fn header_y1() -> usize {
    header_content_bottom() + HEADER_PAD_BOTTOM + HEADER_RULE_H
}

#[inline]
fn pill_text(app: &MessagesApp) -> &'static [u8] {
    app.header_status.label()
}

/// The rung the pill's label is inked at.
///
/// WHY it asks the published level: the room beside the title is what the pill
/// has, and at a raised rung the title has taken more of it -- the loop that
/// used to live here started at rung one and tested `while scale > 1`, so it
/// never ran and the pill kept its rung whatever the room said.
#[inline]
fn pill_text_scale(label: &[u8]) -> usize {
    let title_end = TITLE_X + run_w(TITLE_TEXT, TITLE_SCALE, true);
    let avail = PILL_RIGHT.saturating_sub(title_end + 24);
    let mut scale = PILL_RUNG;
    while scale > 1 {
        let need = run_w(label, scale, false) + 2 * PILL_PAD_X;
        if need <= avail {
            break;
        }
        scale -= 1;
    }
    scale
}

#[inline]
fn pill_left_for(label: &[u8], scale: usize) -> usize {
    let w = run_w(label, scale, false) + 2 * PILL_PAD_X;
    PILL_RIGHT.saturating_sub(w)
}

/// The air between the header band and the first band under it.
///
/// WHY one named gap: the section below the header used to start on the header's
/// own bottom row -- a zero-row gap, so the header's hairline doubled as the
/// search field's top edge -- while the next section started 24 rows further
/// down. `spacing::GRID_8` is the step the dialer leaves between its header and
/// the first band under it (`TAB_BAND_Y0 = 120` against `HEADER_H = 112`).
const SECTION_GAP: usize = spacing::GRID_8;

/// Search / status band geometry (`search_y0()..search_y1()`).
///
/// WHY the band is named at all: the conversation view reuses this exact strip
/// for its threat banner, so the two share one edge instead of one of them
/// drifting.
///
/// WHY these are functions: the band carries a field, the field carries a run
/// at [`SEARCH_TEXT_SCALE`], and a plate that stays folded at the ladder while
/// its run grows is the clipped-text defect. The literals stay as anchors -- a
/// plate is never smaller than the box it was designed with -- and grows by the
/// rows the published rung needs.
const SEARCH_BAND_H_ANCHOR: usize = 52;
const SEARCH_FIELD_H_ANCHOR: usize = 48;

/// The field's margin inside its band: the platform's smallest grid step split
/// between the two sides, so the field's hairline is not the band's edge.
const SEARCH_FIELD_DY: usize = spacing::GRID_4 / 2;

/// The air a field's run keeps from its own top and bottom edges.
const FIELD_TEXT_PAD_Y: usize = 4;

/// A field's height at the published level: the rung it inks, plus its air,
/// never less than the anchor it shipped with.
#[inline(always)]
fn field_h(anchor: usize, rung: usize) -> usize {
    later(anchor, line_h(rung) + FIELD_TEXT_PAD_Y * 2)
}

#[inline(always)]
fn search_field_h() -> usize {
    field_h(SEARCH_FIELD_H_ANCHOR, SEARCH_TEXT_SCALE)
}

#[inline(always)]
fn search_band_h() -> usize {
    later(
        SEARCH_BAND_H_ANCHOR,
        search_field_h() + 2 * SEARCH_FIELD_DY,
    )
}

#[inline(always)]
fn search_y0() -> usize {
    header_y1() + SECTION_GAP
}

#[inline(always)]
fn search_y1() -> usize {
    search_y0() + search_band_h()
}

/// The field's edge: the band's top, the plate's inset and its height.
#[inline(always)]
fn search_field() -> (usize, usize, usize, usize) {
    (
        layout::SEARCH_FIELD_X,
        search_y0() + SEARCH_FIELD_DY,
        layout::SEARCH_FIELD_W,
        search_field_h(),
    )
}
/// The centre of the band's leading glyph lane -- the search view's magnifier
/// and the conversation view's threat mark share it.
///
/// WHY derived from the field: it was the literal `130`, which is eight rows
/// above the middle of the field it sits in, so the mark read as mis-hung
/// against the text beside it.
const SEARCH_ICON_CX: usize = 56;
#[inline(always)]
fn search_icon_cy() -> usize {
    let f = search_field();
    f.1 + f.3 / 2
}
/// The clear control's hit half-height; the mark itself is nine rows tall.
const SEARCH_CLEAR_HALF_H: usize = 14;
/// The centre of the clear mark that ends the search field.
const SEARCH_CLEAR_CX: usize = layout::SEARCH_CLEAR_CX;
/// The half-width of the band the clear mark answers taps in.
const SEARCH_CLEAR_HALF_W: usize = layout::SEARCH_CLEAR_HALF_W;
const SEARCH_TEXT_X: usize = layout::SEARCH_TEXT_X;
/// The columns the typed query may use.
///
/// WHY it stops short of the mark: the budget used to run to the field's own edge
/// minus 20, which is 676 -- inside the nine columns the clear mark paints. A
/// query long enough to reach it lost its last letters under the cross, and the
/// cross lost its right arm to them.
///
/// WHY it is the library's lane and not arithmetic written here: the lane is what
/// a fit gate has to be able to reach, and this app's binary is built with
/// `test = false`. The arithmetic now lives in `messages::layout`, derived from
/// the clear mark's own left arm, so moving the mark moves the lane with it.
const SEARCH_TEXT_MAX_W: usize = layout::SEARCH_TEXT_LANE;
const SEARCH_PLACEHOLDER: &[u8] = b"Search chats...";
const SEARCH_TEXT_SCALE: usize = layout::FIELD_TEXT_RUNG;

/// The rows a caption rung's box has to be pushed down to put its caps on the
/// same baseline as a body rung's box laid out at the same box top.
///
/// WHY a baseline and not a shared box top: a run laid out at box top `y0` sits
/// its ascent rows lower, so two runs that start at the same box top end on
/// lines eight rows apart. The composer's character count and the "To:" label
/// are both rung 1 and both drawn at the box top of the rung-2 text beside them,
/// so each hung above the words it belongs to.
#[inline(always)]
fn shared_baseline_shift() -> usize {
    ascent_h(type_scale::BODY) - ascent_h(type_scale::CAPTION)
}

// WHY these guards run at startup instead of folding at compile time, as the
// `const _: () = assert!` pair here used to: the shifted caption's box and the
// two fields' heights are all properties of the published rung now, so the
// bound can only be checked against the rung the screen is about to draw with.
// The check is not weakened -- it is the same inequality, and it now answers for
// every level a person can select rather than for the one the ladder happened to
// ship with. `the_bands_hold_their_own_ink` in this file's library is where the
// same inequality is asserted for all four levels.
#[inline(always)]
fn fields_hold_shifted_caption() -> bool {
    let shift = shared_baseline_shift();
    shift + line_h(type_scale::CAPTION) <= recipient_field_h()
        && shift + line_h(type_scale::CAPTION) <= compose_field_h()
}

/// The rows a caption box is pushed down to sit on the body run's baseline.
///
/// WHY the guard decides it and the shift is not simply added: the shift exists
/// to bring the caption's caps onto the baseline of the body run beside it, and
/// it is only a whole box's worth of rows while the plate it lands in still
/// holds the shifted box. `fields_hold_shifted_caption` is that bound asked at
/// the rung the screen is about to paint with; where it fails, the caption stays
/// at the body run's own box top -- the two then read at the same top instead of
/// the same baseline, which is the older layout, rather than the caption's box
/// running out of the field it belongs to.
#[inline(always)]
fn caption_shift() -> usize {
    if fields_hold_shifted_caption() {
        shared_baseline_shift()
    } else {
        0
    }
}

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
const RECIPIENT_FIELD_H_ANCHOR: usize = MIN_TOUCH_TARGET;

/// The recipient field's height at the published level.
///
/// WHY the band grows upward with it: the two bands keep their shared bottom
/// edge -- that edge is the row the composer's ink stops at and the row the home
/// gesture claims from (`HOME_HIT`) -- so a taller field buys its rows from the
/// thread list above, which scrolls, and never from the gesture below.
#[inline(always)]
fn recipient_field_h() -> usize {
    field_h(RECIPIENT_FIELD_H_ANCHOR, RECIPIENT_TEXT_SCALE)
}

/// The rows the recipient band has to give the field beyond the anchor it
/// shipped with. Zero at the ladder, so the bands sit where the suite pins them.
#[inline(always)]
fn band_extra_rows() -> usize {
    recipient_field_h().saturating_sub(RECIPIENT_FIELD_H_ANCHOR)
}
const RECIPIENT_FIELD_W: usize = layout::RECIPIENT_FIELD_W;
const RECIPIENT_TEXT_X: usize = layout::RECIPIENT_TEXT_X;
/// The lane the typed recipient address owns.
///
/// WHY it was not left as `490 - RECIPIENT_TEXT_X`: the `490` named no box. It is
/// not the field's right edge (the field ends at `RECIPIENT_FIELD_X +
/// RECIPIENT_FIELD_W`, 610), not the clear mark's arm (574) and not the picker
/// control's column (616) -- it was a width remembered from the day the band was
/// laid out, and it cut the address 76 columns before the space the field
/// actually reserves. The lane now answers the mark the address is typed beside.
const RECIPIENT_TEXT_MAX_W: usize = layout::RECIPIENT_TEXT_LANE;
const RECIPIENT_PLACEHOLDER: &[u8] = b"Recipient (name or handle)";
const RECIPIENT_TEXT_SCALE: usize = layout::FIELD_TEXT_RUNG;

// Compose field geometry
const COMPOSE_FIELD_H_ANCHOR: usize = 60;

/// The compose field's height at the published level.
///
/// WHY it keeps its 60-row anchor at every rung up to extra large: the field's
/// own rung reserves 59 rows there, so the plate it shipped with already holds
/// it -- the anchor is a floor, not a ceiling.
#[inline(always)]
fn compose_field_h() -> usize {
    field_h(COMPOSE_FIELD_H_ANCHOR, COMPOSE_TEXT_SCALE)
}
const COMPOSE_FIELD_W: usize = layout::COMPOSE_FIELD_W;
const COMPOSE_TEXT_X: usize = layout::COMPOSE_TEXT_X;
// WHY the text stops before the field's right edge: the last ~60 px of the
// field are the character counter and the clear control, and text that ran
// underneath them made both unreadable. The room the counter is given is its
// own named constant now, and the fit gate below measures the counter's widest
// line against it -- an unnamed `56` is exactly how a counter outgrows its room.
const COMPOSE_COUNT_RIGHT: usize = layout::COMPOSE_COUNT_RIGHT;
const COMPOSE_TEXT_MAX_W: usize = layout::COMPOSE_TEXT_LANE;
const COMPOSE_PLACEHOLDER: &[u8] = b"Type a message...";
const COMPOSE_TEXT_SCALE: usize = layout::FIELD_TEXT_RUNG;
/// The composer field's inset from its band's top edge.
const COMPOSE_FIELD_DY: usize = 14;
/// The clear control both editors carry at the right end of their field.
///
/// WHY one band for the mark and the press: the painter clipped the mark to
/// `574..606` while the tap path clipped it to `570..610`, and `570` is inside
/// the character counter's own right edge at 572 -- so the two columns at the
/// end of the count cleared the draft instead of focusing the field.
const FIELD_CLEAR_CX: usize = layout::FIELD_CLEAR_CX;
const FIELD_CLEAR_X0: usize = layout::FIELD_CLEAR_X0;
const FIELD_CLEAR_X1: usize = layout::FIELD_CLEAR_X1;
/// WHY gated: the mark lives inside the field, and the field's edge is also the
/// lane the send control and the recipient picker are drawn in. A wider mark
/// would answer presses that belong to those controls.
const _: () = assert!(
    FIELD_CLEAR_X1 <= 24 + COMPOSE_FIELD_W && FIELD_CLEAR_X1 <= 24 + RECIPIENT_FIELD_W
);
/// The send disc, and the lane of the compose band it is drawn in.
///
/// WHY the lane is named at all: the painter clipped the disc to
/// `x 630..694, y band+14..band+74` while the tap path clipped it to nothing,
/// so the disc answered presses in every screen -- including the two that draw
/// a notice card over those very pixels.
const SEND_CX: usize = 662;
const SEND_R: usize = 29;
const SEND_LANE_X0: usize = 630;
const SEND_LANE_X1: usize = 694;

/// WHY checked at compile time: the disc has to sit inside the lane it is hit
/// as, and the lane inside the card column. The diameter against the field's
/// height is the fact that fails first if either control grows.
///
/// WHY the anchor and not the published height: `compose_field_h()` is the
/// anchor as a floor and only ever grows with the rung, so a disc that fits the
/// anchor fits the field at every level a person can select.
const _: () = assert!(SEND_R * 2 <= COMPOSE_FIELD_H_ANCHOR);
const _: () = assert!(SEND_LANE_X0 + SEND_R <= SEND_CX);
const _: () = assert!(SEND_CX + SEND_R <= SEND_LANE_X1);
const _: () = assert!(SEND_LANE_X1 <= CARD_X1);
/// The count's right edge stops before the lane, so the composer's number and
/// its send control cannot share a column.
const _: () = assert!(COMPOSE_COUNT_RIGHT < SEND_LANE_X0);

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
    bands_top(app) + messages::keyboard::recipient_band_h()
}

/// The top of the compose band.
#[inline(always)]
fn compose_band_y0(app: &MessagesApp) -> usize {
    bands_top(app) + messages::keyboard::recipient_band_h()
}

/// The bottom of the compose band.
#[inline(always)]
fn compose_band_y1(app: &MessagesApp) -> usize {
    bands_top(app) + messages::keyboard::bands_h()
}

/// The recipient field's rectangle.
#[inline(always)]
fn recipient_field(app: &MessagesApp) -> (usize, usize, usize, usize) {
    (
        24,
        recipient_band_y0(app),
        RECIPIENT_FIELD_W,
        recipient_field_h(),
    )
}

/// The compose field's rectangle.
#[inline(always)]
fn compose_field(app: &MessagesApp) -> (usize, usize, usize, usize) {
    (
        24,
        compose_band_y0(app) + COMPOSE_FIELD_DY,
        COMPOSE_FIELD_W,
        compose_field_h(),
    )
}

/// The lane the send control is drawn in: the strip of the compose band right
/// of the composer field, over the field's own rows.
///
/// WHY derived from the field: the disc is the field's action, so it is centred
/// on the field, and the two used to be expressed as separate literals
/// (`band + 14 .. band + 74`) that only happened to agree while the field was
/// 60 tall in a band of 84.
#[inline(always)]
fn send_lane(app: &MessagesApp) -> (usize, usize, usize, usize) {
    let field = compose_field(app);
    (
        SEND_LANE_X0,
        field.1,
        SEND_LANE_X1 - SEND_LANE_X0,
        field.3,
    )
}

/// Whether the screen being drawn paints the recipient and compose bands.
///
/// WHY the tap path asks the same question the painter does: the thread details
/// and the recipient picker paint a notice card across the rows the bands
/// would occupy, and a control that is not drawn there must not answer a press
/// there either -- above all the send control, whose press seals a message into
/// the vault and posts it to the thread.
#[inline(always)]
fn compose_controls_shown(app: &MessagesApp) -> bool {
    app.view_mode == ViewMode::ThreadList || app.view_mode == ViewMode::Conversation
}

/// The recipient picker's control, at the right of the recipient band.
///
/// WHY the box is the library's: the label inside it is centred with no trim, so
/// whether the word still reads inside its control is a question about this rect,
/// and the gate that answers it is in `messages::layout`.
#[inline(always)]
fn pick_control(app: &MessagesApp) -> (usize, usize, usize, usize) {
    (
        layout::PICK_CONTROL_X,
        recipient_band_y0(app),
        layout::PICK_CONTROL_W,
        MIN_TOUCH_TARGET,
    )
}

/// The centre of the send button.
///
/// WHY it is the lane's own centre: the disc is hit-tested as the circle it is
/// drawn as, and the lane is what the painter clips it to.
#[inline(always)]
fn send_cy(app: &MessagesApp) -> usize {
    let lane = send_lane(app);
    lane.1 + lane.3 / 2
}

/// The height of the thread list's visible band.
#[inline(always)]
fn list_viewport_h(app: &MessagesApp) -> usize {
    bands_top(app) - card_y0()
}

/// The height of the conversation's visible band.
#[inline(always)]
fn convo_viewport_h(app: &MessagesApp) -> usize {
    bands_top(app) - convo_top()
}

/// The home indicator this app draws, and the box that answers a press on it.
///
/// Both come from `std::design::home_band` now. They used to be authored here as
/// five numbers that had already drifted: the painter inked a mark 240 columns
/// wide while the shell draws 200, and the press answered the painter's own rows
/// from their top.
///
/// WHY the answering box is `hit_from` and not the SDK's default: the composer
/// paints down to row 1414 and the default box begins at 1392, inside it. The
/// box therefore claims every row the bands and the keyboard leave -- the
/// platform's centred 240 columns, from the composer's foot to the canvas -- and
/// the floor-sized escape on this screen is the header's back chip, which the
/// same press answers.
///
/// WHY the box does not move with the person's text size: the bands grow
/// upward from `BANDS_BOTTOM_IDLE`, because the row they buy from a raised rung
/// comes out of the scrolling list and not out of the gesture. The composer's
/// ink therefore ends on the same row at every level, and the answering box that
/// is measured from that row is level-independent -- which is what the pair of
/// gates below still proves at the default rung and what
/// `the_bands_stop_where_the_gesture_starts` in the library proves for all four.
const HOME_PILL: primitives::Rect = std::design::home_band::PILL;
const HOME_HIT: primitives::Rect = std::design::home_band::hit_from(BANDS_BOTTOM_IDLE);

/// The row the two editor bands end on, whatever rung the app is drawing at.
const BANDS_BOTTOM_IDLE: usize = BANDS_TOP_IDLE + BANDS_H;

/// WHY checked at compile time: the gesture may only own rows no band of this
/// app paints, and both positions of the compose bands and the keyboard dock sit
/// above the mark.
const _: () = assert!(HOME_PILL.y >= BANDS_TOP_IDLE + BANDS_H);
const _: () = assert!(HOME_HIT.y >= KB_Y1);
const _: () = assert!(HOME_HIT.y <= HOME_PILL.y);
const _: () = assert!(HOME_HIT.y + HOME_HIT.h == H);
const _: () = assert!(HOME_PILL.y + HOME_PILL.h <= H);
const _: () = assert!(
    HOME_HIT.x <= HOME_PILL.x && HOME_HIT.x + HOME_HIT.w >= HOME_PILL.x + HOME_PILL.w
);

/// The row every scrollable section of the app starts at, below the search band.
///
/// WHY one edge for four screens: the thread list began at 188, the conversation
/// at 180, and the details and recipient-picker screens at 180 as well -- so the
/// bubbles were laid out from 180 inside a band the painter opened at 168, and
/// the list sat eight rows lower than the screen it scrolls past. `GRID_16` is
/// the step the notes app puts between its search band and the content under it
/// (`CHIP_Y0 = search_y1() + 16`).
///
/// WHY a function: it stands on the search band, and the search band stands on
/// the header, and both of those now hold a run at the published rung.
#[inline(always)]
fn content_top() -> usize {
    search_y1() + spacing::GRID_16
}

// Thread card geometry
//
// WHY the card's own column lives in the library: every lane in the thread list,
// the details report, the recipient picker and the notice card is derived from
// this box, and a fit gate can only reach a container the library exports.
const CARD_X0: usize = layout::LIST_CARD_X;
const CARD_X1: usize = layout::LIST_CARD_RIGHT;

/// The card a thread row owns at the published level.
///
/// WHY it is derived rather than the literal 104 it was: a row carries its
/// peer's name at the body rung and its snippet at the caption rung, one above
/// the other, and 104 rows is the box the ladder's two line boxes fitted. At a
/// raised rung the two boxes are taller, so the row grows by the rows they need
/// -- the alternative was the ink running past the card into the next row.
#[inline(always)]
fn card_body_h() -> usize {
    later(
        CARD_BODY_H_ANCHOR,
        line_h(layout::PEER_NAME_RUNG)
            + ROW_INK_GAP
            + line_h(layout::SNIPPET_RUNG)
            + CARD_TEXT_PAD_Y * 2,
    )
}

/// The anchor the row was designed with: the box the ladder's two runs fit.
const CARD_BODY_H_ANCHOR: usize = 104;
/// The air a row's two runs keep from the card's own top and bottom edges.
const CARD_TEXT_PAD_Y: usize = 12;

/// The pitch from one thread card's top to the next one's.
///
/// WHY the gap is kept out of the body: the body is what a press answers and
/// what the avatar, the pill and the delete mark are laid out inside.
const CARD_GAP: usize = 16;

#[inline(always)]
fn card_h() -> usize {
    card_body_h() + CARD_GAP
}

#[inline(always)]
fn card_y0() -> usize {
    content_top()
}

/// The rows the avatar initial's box sits left of and above the disc's centre.
///
/// WHY split from the growth terms: the initial is centred on the disc, so the
/// mark follows the run's own width and line box outward from these shipped
/// offsets -- `7` and `12` are what the ladder draws, and a raised rung adds
/// half of what the run gained to each.
const AVATAR_GLYPH_DX: usize = 7;
const AVATAR_GLYPH_DY: usize = 12;

/// The rows from a thread card's top to the peer name's box.
///
/// The name is the card's first run, so nothing above it changes height and the
/// offset is the designer's number at every published level.
#[inline(always)]
fn row_name_dy() -> usize {
    THREAD_NAME_DY_ANCHOR
}

/// The rows from a thread card's top to the snippet's box.
///
/// WHY it carries the name rung's growth: the two runs stack, so the rows the
/// name's box gains at a raised rung are the rows the snippet has to step down.
#[inline(always)]
fn row_snippet_dy() -> usize {
    THREAD_SNIPPET_DY_ANCHOR + line_growth(layout::PEER_NAME_RUNG)
}

const THREAD_NAME_DY_ANCHOR: usize = 22;
const THREAD_SNIPPET_DY_ANCHOR: usize = 56;

/// The width a thread row's bold peer name may use.
///
/// The run starts at the row's text column and has to clear the unread pill's
/// left edge; the slack absorbs bold advances being wider than the regular
/// advances the trim is measured with. Both numbers are the library's now, so a
/// pill that moves moves the lane, and the gate in `messages::layout` refuses a
/// lane that reaches past the pill.
const PEER_NAME_CAP: usize = 96;
const PEER_NAME_MAX_W: usize = layout::PEER_NAME_LANE;

/// The height of the thread list's visible band when the keyboard is down.
///
/// WHY it is derived from the band's edges and not typed in: the band is
/// bounded by the card row above it and the recipient field below it, and a
/// typed-in height drifts away from both the first time either moves.
#[inline(always)]
fn list_viewport_h_idle() -> usize {
    bands_top_idle() - card_y0()
}

/// The row the two editor bands sit at when the keyboard is down, at the
/// published level: the bands keep their bottom edge and buy their extra rows
/// from the top.
#[inline(always)]
fn bands_top_idle() -> usize {
    BANDS_BOTTOM_IDLE - messages::keyboard::bands_h()
}

/// The top of the conversation's visible band.
#[inline(always)]
fn convo_top() -> usize {
    content_top()
}

/// The height of a chat bubble at the published level.
///
/// WHY it is derived rather than the literal 76: a bubble carries its body line
/// at the body rung and its delivery state under it at the caption rung, and 76
/// is the box the ladder's two line boxes fitted. Same rule as the thread row:
/// the plate grows with the ink it holds, or the ink leaves the plate.
#[inline(always)]
fn bubble_h() -> usize {
    later(
        BUBBLE_H_ANCHOR,
        line_h(layout::BUBBLE_BODY_RUNG)
            + ROW_INK_GAP
            + line_h(layout::BUBBLE_STATUS_RUNG)
            + BUBBLE_PAD_Y * 2,
    )
}

/// The box the bubble shipped with, and the air its two runs keep from its edges.
const BUBBLE_H_ANCHOR: usize = 76;
const BUBBLE_PAD_Y: usize = 8;

/// The narrowest a bubble may be drawn, so a one-word message still reads as
/// a bubble and not as a floating chip.
const BUBBLE_MIN_W: usize = layout::BUBBLE_MIN_W;

/// The widest a bubble may be drawn. Long messages still wrap short of the
/// screen edge instead of touching it.
///
/// WHY it is the library's: the bubble hugs its content, so this clamp is the
/// widest lane a body line can ever be fitted to, and the notice that says a run
/// did not fit has to answer the same number.
const BUBBLE_MAX_W: usize = layout::BUBBLE_MAX_W;

/// The gap between two chat bubbles.
///
/// WHY the platform's step and not 14: 14 is on no grid this app otherwise
/// uses, and the thread list above it gaps its rows at `GRID_16`. Two lists in
/// one app that space the same thing two different ways read as one of them
/// being wrong.
const BUBBLE_GAP: usize = spacing::GRID_16;

/// The distance from one bubble's top to the next one's top.
#[inline(always)]
fn bubble_step() -> usize {
    bubble_h() + BUBBLE_GAP
}

/// WHY the gate runs at draw time and is not the compile-time `assert!` it was:
/// both terms are now properties of the published rung, and the conversation
/// clamps a scrolled-out bubble to row zero -- so the bound has to hold at the
/// rung the screen is about to paint with, not at the one the ladder shipped
/// with. `the_clamped_bubble_never_enters_the_search_band` in the library checks
/// it for all four levels; the layout below clamps a thread's oldest bubble to
/// `convo_top()`, which is never above the search band's foot at any level.
#[inline(always)]
fn convo_top_holds_a_bubble() -> bool {
    convo_top() >= bubble_h()
}

/// The longest line of notice text the app shows.
const NOTICE_CAP: usize = 96;

// Details and recipient-picker geometry
//
// WHY both screens start at `content_top()`: they are the same section as the
// thread list -- the scrollable body under the search/status band -- and a
// screen that started eight rows above the list printed its first row over the
// band's hairline while its siblings did not.
#[inline(always)]
fn details_top() -> usize {
    content_top()
}

// The report row both the details screen and the recipient picker lay out: a
// caption box over a body box, on a card, with the platform's grid between two
// cards.
//
// WHY every number here is derived: the pitch was the literal 96 and the card
// under it 88, and twelve of them from `details_top()` run to row 1340 --
// fifty-six rows past the band the screen paints inside, because the band ends
// where the recipient band begins. The last row of the details report was
// therefore cut on every visit: its label showed and the sentence under it
// ("Unavailable: no transport is bound") never did, so the screen printed a
// question with no answer.
const ROW_TOP_PAD: usize = 12;
const ROW_INK_GAP: usize = 4;
const ROW_BOTTOM_PAD: usize = 12;

/// The caption rung's box at the published level.
#[inline(always)]
fn row_caption_box() -> usize {
    line_h(type_scale::CAPTION)
}

/// The body rung's box at the published level.
#[inline(always)]
fn row_body_box() -> usize {
    line_h(type_scale::BODY)
}

/// The card a report row owns.
#[inline(always)]
fn row_card_h() -> usize {
    ROW_TOP_PAD + row_caption_box() + ROW_INK_GAP + row_body_box() + ROW_BOTTOM_PAD
}

/// The gap between two report cards.
const ROW_GAP: usize = spacing::GRID_8;

/// The distance from one report row's top to the next one's top.
#[inline(always)]
fn row_pitch() -> usize {
    row_card_h() + ROW_GAP
}

/// The row's side padding, and the two boxes' tops inside the card.
///
/// WHY the padding is the library's: it is the air every full-width card of this
/// app keeps from its own edge, and the lane a report row trims to is the card's
/// width less this padding twice. Named in one place, the painter and the fit gate
/// cannot disagree about it.
const ROW_PAD_X: usize = layout::CARD_PAD_X;
#[inline(always)]
fn row_caption_y() -> usize {
    ROW_TOP_PAD
}
#[inline(always)]
fn row_body_y() -> usize {
    ROW_TOP_PAD + row_caption_box() + ROW_INK_GAP
}

#[inline(always)]
fn detail_row_h() -> usize {
    row_pitch()
}

/// HOW MANY rows the detail band can show: the viewport divided by the stride.
///
/// WHY derived and not the twelve the arrays were typed at: the row grew when
/// the type ladder's caption and body rungs were re-measured, and a twelfth
/// row's card began below the band's bottom edge -- painted, and answered by
/// the recipient field drawn over it. The builders cap themselves at the array
/// length, so the capacity the band can actually hold is the capacity.
#[inline(always)]
fn detail_rows() -> usize {
    list_viewport_h_idle() / row_pitch()
}

#[inline(always)]
fn recip_row_top() -> usize {
    content_top()
}

#[inline(always)]
fn recip_row_h() -> usize {
    row_pitch()
}

#[inline(always)]
fn recip_rows() -> usize {
    list_viewport_h_idle() / row_pitch()
}

// --- The same layout, folded at compile time for every selectable level ------
//
// WHY this block exists beside the functions above: two things in this file have
// to be array lengths, and an array length cannot be asked at draw time. Rather
// than pin them to the ladder's rung -- which is the defect -- they are derived
// for all four levels a person can select and bounded by the widest answer, so
// no selectable level can make the builders write past a buffer. The arithmetic
// is the functions above with the level named as an argument instead of read from
// the published cell, and the library's gate asserts the two agree at every
// level.
const fn line_at(rung: usize, level: u8) -> usize {
    std::design::font::line_height_for_user_scale(rung, level)
}

const fn ascent_at(rung: usize, level: u8) -> usize {
    std::design::font::ascent_for_user_scale(rung, level)
}

const fn cap_at(rung: usize, level: u8) -> usize {
    std::design::font::cap_for_user_scale(rung, level) as usize
}

const fn pill_h_at(level: u8) -> usize {
    later(PILL_H_ANCHOR, line_at(PILL_RUNG, level) + PILL_PAD_Y * 2)
}

const fn pill_bottom_at(level: u8) -> usize {
    HEADER_ROW - pill_h_at(level) / 2 + pill_h_at(level)
}

const fn title_y_at(level: u8) -> usize {
    HEADER_ROW - ascent_at(TITLE_SCALE, level) + cap_at(TITLE_SCALE, level) / 2
}

const fn header_y1_at(level: u8) -> usize {
    later(
        later(
            title_y_at(level) + line_at(TITLE_SCALE, level),
            HEADER_TOP + std::design::back_button::SIZE,
        ),
        pill_bottom_at(level),
    ) + HEADER_PAD_BOTTOM
        + HEADER_RULE_H
}

const fn field_h_at(anchor: usize, rung: usize, level: u8) -> usize {
    later(anchor, line_at(rung, level) + FIELD_TEXT_PAD_Y * 2)
}

const fn search_y1_at(level: u8) -> usize {
    let band = later(
        SEARCH_BAND_H_ANCHOR,
        field_h_at(SEARCH_FIELD_H_ANCHOR, SEARCH_TEXT_SCALE_ANCHOR, level) + 2 * SEARCH_FIELD_DY,
    );
    header_y1_at(level) + SECTION_GAP + band
}

/// SEARCH_TEXT_SCALE is named after the rung the fields ink at; the const chain
/// needs the same number as a literal the compiler can fold.
const SEARCH_TEXT_SCALE_ANCHOR: usize = layout::FIELD_TEXT_RUNG;

const fn content_top_at(level: u8) -> usize {
    search_y1_at(level) + spacing::GRID_16
}

const fn band_extra_at(level: u8) -> usize {
    let grown = field_h_at(RECIPIENT_FIELD_H_ANCHOR, RECIPIENT_TEXT_SCALE_ANCHOR, level);
    grown.saturating_sub(RECIPIENT_FIELD_H_ANCHOR)
}

const RECIPIENT_TEXT_SCALE_ANCHOR: usize = layout::FIELD_TEXT_RUNG;

const fn list_viewport_at(level: u8) -> usize {
    (BANDS_TOP_IDLE - band_extra_at(level)).saturating_sub(content_top_at(level))
}

/// The smallest scroll band any selectable text level can produce.
///
/// WHY a `const` at all: `MessagesApp::new()` is a `const fn`, so it cannot call
/// `list_viewport_h(self)` -- and the person's level is not even published at
/// that point. The seed therefore takes the narrowest band the four levels can
/// yield, which is the conservative direction: a viewport that is too small only
/// clamps the scroll offset harder, it can never let content scroll out of view.
/// Every path that changes the row or bubble count calls `sync_list_extent` or
/// `sync_convo_extent`, and both rebuild the viewport from the published level
/// before the screen is drawn, so the seed never survives to a paint.
const fn narrowest_viewport() -> usize {
    let mut best = list_viewport_at(0);
    let mut level = 1u8;
    while level <= 3 {
        let candidate = list_viewport_at(level);
        if candidate < best {
            best = candidate;
        }
        level += 1;
    }
    best
}

const INITIAL_VIEWPORT_H: usize = narrowest_viewport();
const _: () = assert!(INITIAL_VIEWPORT_H > 0);

const fn row_pitch_at(level: u8) -> usize {
    ROW_TOP_PAD
        + line_at(type_scale::CAPTION, level)
        + ROW_INK_GAP
        + line_at(type_scale::BODY, level)
        + ROW_BOTTOM_PAD
        + ROW_GAP
}

const fn rows_at(level: u8) -> usize {
    list_viewport_at(level) / row_pitch_at(level)
}

/// The widest the report arrays ever have to be: the row count of the level that
/// fits the most of them.
const fn widest_rows() -> usize {
    let mut best = rows_at(0);
    let mut level = 1u8;
    while level <= 3 {
        let candidate = rows_at(level);
        if candidate > best {
            best = candidate;
        }
        level += 1;
    }
    best
}

const DETAIL_ROWS: usize = widest_rows();
const RECIP_ROWS: usize = widest_rows();

/// WHY checked at compile time: a card has to outlast the ink it carries, and
/// the band has to hold a whole row rather than the top of one. Each of these
/// is the cut-off row coming back.
///
/// WHY the check now runs over the four levels instead of the ladder's one box:
/// the two boxes and the pitch are properties of the published rung, so the
/// bound is only honest if it holds for every rung a person can select. The
/// inequality is the same one the removed `assert!` pair asked -- it is not
/// weakened, it is asked four times.
const fn rows_hold_their_ink(level: u8) -> bool {
    let caption = line_at(type_scale::CAPTION, level);
    let body = line_at(type_scale::BODY, level);
    let card = ROW_TOP_PAD + caption + ROW_INK_GAP + body + ROW_BOTTOM_PAD;
    ROW_TOP_PAD + caption + ROW_INK_GAP + body < card && rows_at(level) >= 1
}

const _: () = assert!(
    rows_hold_their_ink(0)
        && rows_hold_their_ink(1)
        && rows_hold_their_ink(2)
        && rows_hold_their_ink(3),
    "a report row's card has to outlast its two runs, and the band has to hold a whole row",
);
const PICK_LABEL: &[u8] = layout::PICK_LABEL;
const INFO_LABEL: &[u8] = layout::INFO_LABEL;

// Empty-state card
//
// WHY its foot is the keyboard-up band: the card was a remembered 340 rows tall,
// which put its bottom edge at 900 while the list band ends at 850 the moment the
// keyboard is up. The empty state then lost its own bottom border and both bottom
// corners -- a card with no bottom reads as a painting cut off rather than as a
// list with nothing in it, and it happened on the screen that appears only when
// there is nothing else to look at.
const EMPTY_CARD_TOP: usize = 560;

/// The row the bands sit at when the keyboard is up, at the published level.
///
/// WHY it moves with the level: the bands keep their own height above the dock,
/// so a taller recipient field lifts them -- and the empty card, whose foot is
/// the band's top, has to be told or it paints under the recipient field.
#[inline(always)]
fn bands_top_keyboard() -> usize {
    BANDS_TOP_KEYBOARD - messages::keyboard::band_extra_rows()
}

#[inline(always)]
fn empty_card() -> (usize, usize, usize, usize) {
    (
        layout::EMPTY_CARD_X,
        EMPTY_CARD_TOP,
        layout::EMPTY_CARD_W,
        bands_top_keyboard() - spacing::GRID_8 - EMPTY_CARD_TOP,
    )
}

/// WHY the gate is asked at draw time: the card's foot is the band's top, which
/// is now a property of the published rung. `the_empty_card_holds_its_last_line`
/// in the library checks the same inequality the removed literal asserted, for
/// every level a person can select, and the painter refuses a line the card
/// cannot show rather than letting it run past the border.
#[inline(always)]
fn empty_card_holds_last_line() -> bool {
    832 < empty_card().1 + empty_card().3
}

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
    let lh = line_h(scale);
    let cy = (pill_top() + pill_bottom()) / 2;
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
        f.len = fit_run(placeholder, scale, max_w, false, &mut f.buf);
    } else {
        f.color = THEME.on_surface;
        f.len = fit_run(&raw[..raw_len], scale, max_w, false, &mut f.buf);
    }
    let w = run_w(&f.buf[..f.len], scale, false);
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
    let lh = line_h(f.scale);
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
    // WHY the band's own capacity and not the array's length: the array is
    // sized for the level that fits the most rows, so at any other level the
    // array can outlast the band. A row whose card begins below the band's foot
    // is painted nowhere and answered by no press, and a row that begins inside
    // it and ends below it is a label with no sentence under it -- the cut-off
    // row this file's own comment records. The band shows whole rows or none.
    n.min(detail_rows())
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
    // WHY the band's own capacity: the same reason as the details report -- the
    // array is sized for the level that fits the most rows, so a row can begin
    // inside the picker's band and end below it, and the address the user means
    // to choose would then be offered as a label with nothing under it. The
    // picker answers only rows it can show whole.
    n.min(recip_rows())
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
    // WHY the published advance and box: the label is centred in the control on
    // both axes, and a centring that measures at the ladder while the ink draws
    // at the person's rung puts the word off its own pill -- closer to one border
    // than the other, and higher than the plate's middle. At the default level
    // both numbers are the ladder's, which is where `PICK` and `INFO` sit today.
    let label_w = run_w(label, layout::CONTROL_LABEL_RUNG, false);
    let label_x = rect.x + (rect.w.saturating_sub(label_w)) / 2;
    let label_y = rect.y + (rect.h.saturating_sub(line_h(layout::CONTROL_LABEL_RUNG))) / 2;
    if let Some(c) = test_text(
        x,
        y,
        label_x,
        label_y,
        layout::CONTROL_LABEL_RUNG,
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
        let max_w = layout::card_row_lane(rect.2);
        // WHY the app's own fit and not `font::ellipsize`: the lane is measured
        // at the ladder, so at a raised rung a run `ellipsize` judged to fit is
        // wider than the card it was cut for and inks over the hairline. The cut
        // now happens where the ink ends; at the default rung this hands the work
        // to `font::ellipsize` itself, which is the trim every report row and
        // every picker row was screenshot with.
        let mut lbl_buf = [0u8; 64];
        let lbl_disp =
            fitted_run(label, layout::ROW_CAPTION_RUNG, max_w, false, &mut lbl_buf);
        if let Some(t) = test_text(
            x,
            y,
            fx + ROW_PAD_X,
            fy + row_caption_y(),
            layout::ROW_CAPTION_RUNG,
            lbl_disp,
            THEME.on_surface_variant,
            CARD.fill,
        ) {
            return Some(t);
        }
        let mut val_buf = [0u8; 96];
        let val_disp = fitted_run(value, layout::ROW_BODY_RUNG, max_w, false, &mut val_buf);
        if let Some(t) = test_text(
            x,
            y,
            fx + ROW_PAD_X,
            fy + row_body_y(),
            layout::ROW_BODY_RUNG,
            val_disp,
            THEME.on_surface,
            CARD.fill,
        ) {
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
    // WHY the published line box for all three: each field's own height already
    // grows with the level (`later(MIN_TOUCH_TARGET, line_h(rung) + pad)`), so a
    // run centred on the ladder's box drifts towards the top of a plate that has
    // grown below it. At the default level these are the boxes the fields were
    // screenshot with.
    let search_y =
        (search_field().1 + search_field().3 / 2).saturating_sub(line_h(SEARCH_TEXT_SCALE) / 2);
    let recipient_rect = recipient_field(messages);
    let compose_rect = compose_field(messages);
    let recipient_y = (recipient_rect.1 + recipient_rect.3 / 2)
        .saturating_sub(line_h(RECIPIENT_TEXT_SCALE) / 2);
    let compose_y =
        (compose_rect.1 + compose_rect.3 / 2).saturating_sub(line_h(COMPOSE_TEXT_SCALE) / 2);
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
    let title_len = fit_run(
        header_title(messages, &contacts, contacts_count),
        TITLE_SCALE,
        layout::header_title_lane(pill.left),
        false,
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
        let cellular = if messages.signal_bars > 0 {
            interos_std::design::components::Reading::Measured(
                (messages.signal_bars as u32) * 25,
            )
        } else {
            // No bars measured: the radio's presence is not a level, so the mark
            // stays an outline that says so.
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

    // 1. Header (y 0..header_y1())
    if y < header_y1() {
        if y >= header_y1() - HEADER_RULE_H {
            return THEME.border;
        }

        // Standardized back button squircle (x 24..72, y HEADER_TOP..+48)
        if let Some(c) = std::design::back_button::pixel_for_theme(x, y, &THEME) {
            return c;
        }

        // Electric red dot accent, on the row the controls and the title caps
        // are centred on (`HEADER_ROW`), not on the box's top.
        let dot_dx = x as i32 - 96;
        let dot_dy = y as i32 - HEADER_ROW as i32;
        if dot_dx * dot_dx + dot_dy * dot_dy <= 16 {
            return THEME.error;
        }

        // Header Title: already cut to the space left of the status pill.
        //
        // WHY the gate is the box (`title_box_h()`) and not the run's caps: the
        // rasteriser answers for the whole line box, descent included, and the
        // band above now reserves exactly that many rows.
        if y >= title_y() && y < title_y() + title_box_h() {
            if let Some(c) = test_text(
                x,
                y,
                TITLE_X,
                title_y(),
                TITLE_SCALE,
                &f.title[..f.title_len],
                THEME.on_surface,
                THEME.surface_container,
            ) {
                return c;
            }
        }

        // Status pill, top-right: truthful NO TRANSPORT / outcome
        if x >= f.pill.left && x < PILL_RIGHT && y >= pill_top() && y < pill_bottom() {
            let rect = primitives::Rect::new(f.pill.left, pill_top(), PILL_RIGHT - f.pill.left, pill_bottom() - pill_top());
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
        // 2. Search & Filter Bar (y search_y0()..search_y1())
        if y >= search_y0() && y < search_y1() {
            if in_rect(search_field(), x, y) {
                let rect = primitives::Rect::new(search_field().0, search_field().1, search_field().2, search_field().3);

                // Vector magnifying glass, centred on the field's leading lane.
                let cx = SEARCH_ICON_CX;
                let cy = search_icon_cy();
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

                if messages.search_len > 0
                    && x >= SEARCH_CLEAR_CX - SEARCH_CLEAR_HALF_W
                    && x < SEARCH_CLEAR_CX + SEARCH_CLEAR_HALF_W
                    && y >= search_icon_cy() - SEARCH_CLEAR_HALF_H
                    && y < search_icon_cy() + SEARCH_CLEAR_HALF_H
                {
                    let ccx = SEARCH_CLEAR_CX as isize;
                    let cy = search_icon_cy() as isize;
                    let dx = (x as isize - ccx).abs();
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

        // 3. Thread List / Empty State (y content_top()..the recipient band)
        //
        // WHY the list band starts at the first card and not at `search_y1()`: the
        // row math below subtracts card_y0(), so a pixel in the gap above the
        // first card underflowed and killed the task on the first paint of a
        // list that had a thread in it.
        if y >= card_y0() && y < recipient_band_y0(messages) {
            // Filter matched nothing
            if f.filtered.is_none() {
                if in_rect(empty_card(), x, y) {
                    // WHY the platform's card: this app drew the empty state's own
                    // outline -- a corner cut that ate the hairline, and a rule drawn
                    // as "the nearest edge is within two pixels". A rule drawn that
                    // way has no corners, so the outline broke at every corner, and
                    // it was a second implementation of a card the platform already
                    // draws. The platform's card has one outline that follows the
                    // corner.

                    let line = layout::EMPTY_FILTER_LINE;
                    let sub = &messages.search[..messages.search_len];
                    let mut buf = [0u8; COMPOSE_CAP];
                    // WHY the lane is the card's and not the remembered `624`:
                    // both lines of this card are centred inside it, so the room a
                    // run owns is the card's width less the air it keeps on either
                    // side -- the same arithmetic every other full-width card of
                    // this app answers. `624` was that number written out with no
                    // constant naming the card it came from, so the day the card
                    // moved, the trim would have stayed and the query would have run
                    // past the card's own edge.
                    let n = fit_run(
                        sub,
                        layout::EMPTY_QUERY_RUNG,
                        layout::EMPTY_CARD_LANE,
                        false,
                        &mut buf,
                    );
                    let sub_line: &[u8] = if n == 0 {
                        layout::EMPTY_FILTER_FALLBACK
                    } else {
                        &buf[..n]
                    };
                    // WHY the published advance for both centring widths: each
                    // line is centred on the width it is inked with, and a rung
                    // that inks the line wider than the ladder measured it left
                    // the line standing off-centre inside its own card -- the
                    // centring number and the ink number have to be the same
                    // number, whatever rung the person chose. At the default
                    // level `run_w` answers the ladder's advance, so both lines
                    // hold the columns they shipped in.
                    let line_w = run_w(line, layout::EMPTY_HEADLINE_RUNG, true);
                    let sub_w = run_w(sub_line, layout::EMPTY_QUERY_RUNG, false);
                    if let Some(c) = test_text_bold(
                        x,
                        y,
                        empty_card().0 + (empty_card().2.saturating_sub(line_w)) / 2,
                        676,
                        layout::EMPTY_HEADLINE_RUNG,
                        line,
                        THEME.on_surface,
                        THEME.surface_container,
                    ) {
                        return c;
                    }
                    if let Some(c) = test_text(
                        x,
                        y,
                        empty_card().0 + (empty_card().2.saturating_sub(sub_w)) / 2,
                        716,
                        layout::EMPTY_QUERY_RUNG,
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
                            empty_card().0,
                            empty_card().1,
                            empty_card().2,
                            empty_card().3,
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
                if in_rect(empty_card(), x, y) {
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
                    //
                    // WHY the rungs are named: both lines are centred inside the
                    // empty card, so the card's own fit gate (see
                    // `messages::layout`) has to measure them at the rung this
                    // branch inks them at.
                    //
                    // WHY the fitted advance and not `font::center_x`: this line
                    // is centred on the width its run takes, and `center_x`
                    // measures it at the ladder. A rung that inks the title
                    // wider than the card's lane made `center_x` answer zero for
                    // a reserve it had to divide, which put the title's first
                    // glyph on the card's own hairline. The fitted width is the
                    // advance of the run the lane can show -- the whole title at
                    // every selectable level, which
                    // `the_empty_card_holds_its_centred_lines` in the library
                    // checks at the default level, the one the contract's 740 and
                    // 810 were drawn against.
                    let title_w = fitted_run_w(
                        layout::EMPTY_THREADS_TITLE,
                        layout::EMPTY_TITLE_RUNG,
                        layout::EMPTY_CARD_LANE,
                        false,
                    );
                    if let Some(c) = test_text(
                        x,
                        y,
                        empty_card().0 + (empty_card().2.saturating_sub(title_w)) / 2,
                        740,
                        layout::EMPTY_TITLE_RUNG,
                        layout::EMPTY_THREADS_TITLE,
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
                            layout::EMPTY_STORE_REFUSED_SUBLINE,
                            THEME.warning,
                        ),
                        store::StoreState::Invalid => (
                            layout::EMPTY_STORE_INVALID_SUBLINE,
                            THEME.error,
                        ),
                        store::StoreState::Full => (
                            layout::EMPTY_STORE_FULL_SUBLINE,
                            THEME.warning,
                        ),
                        _ => (layout::EMPTY_TRANSPORT_SUBLINE, THEME.on_surface_variant),
                    };
                    // WHY the subline is cut to the card before it is centred:
                    // the four states the store can answer with have four
                    // different lengths, and the longest of them is inked at the
                    // caption rung, whose box the published level multiplies. Cut
                    // to the card's own lane the line stays a line the card holds;
                    // centred on the run that is actually inked, it keeps the
                    // columns it shipped with at the default level, where every
                    // one of the four fits the lane whole.
                    let mut sub_buf = [0u8; 72];
                    let sn = fit_run(
                        subline,
                        layout::EMPTY_SUBLINE_RUNG,
                        layout::EMPTY_CARD_LANE,
                        false,
                        &mut sub_buf,
                    );
                    let sub_disp: &[u8] = if sn == 0 { subline } else { &sub_buf[..sn] };
                    let sub_w = run_w(sub_disp, layout::EMPTY_SUBLINE_RUNG, false);
                    // WHY the guard stands in front of this draw: the card's foot
                    // is the band's top, which is a property of the published
                    // rung, and this line's box ends at row 832 -- the row the
                    // guard's own bound names. A rung that lifts the bands past
                    // 832 leaves the card no room for the line at all, and the
                    // painter then refuses it rather than inking it over the
                    // card's bottom border and under the recipient field. At the
                    // compact and the default levels the card does hold the line,
                    // which is every pixel the review queue has a screenshot of.
                    if empty_card_holds_last_line() {
                        if let Some(c) = test_text(
                            x,
                            y,
                            empty_card().0 + (empty_card().2.saturating_sub(sub_w)) / 2,
                            810,
                            layout::EMPTY_SUBLINE_RUNG,
                            sub_disp,
                            subline_color,
                            THEME.surface_container,
                        ) {
                            return c;
                        }
                    }

                    return monochrome::card_px(
                        x,
                        y,
                        primitives::Rect::new(
                            empty_card().0,
                            empty_card().1,
                            empty_card().2,
                            empty_card().3,
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
                let rel = (y - card_y0()) + scroll;
                let idx = rel / card_h();
                let card_top = card_y0() + idx * card_h() - scroll;
                let card_offset_y = rel % card_h();
                if card_offset_y < card_body_h() && x >= CARD_X0 && x < CARD_X1 {
                    let card_rect = primitives::Rect::new(CARD_X0, card_top, CARD_X1 - CARD_X0, card_body_h());
                    if let Some(ledger_idx) = messages.filtered_index(idx) {
                        if let Some(tr) = messages.ledger.get_thread_by_idx(ledger_idx) {
                            // 1. Avatar glyph with colored backdrop
                            let acx = layout::AVATAR_CX;
                            let acy = card_top + card_body_h() / 2;
                            let adx = x as isize - acx as isize;
                            let ady = y as isize - acy as isize;
                            if adx * adx + ady * ady <= (layout::AVATAR_R * layout::AVATAR_R) as isize {
                                let initial = if tr.handle_len > 0 { tr.peer_handle[0] } else { b'?' };
                                let glyph = [initial];
                                if let Some(c) = test_text_bold(
                                    x,
                                    y,
                                    acx.saturating_sub(
                                        AVATAR_GLYPH_DX + run_growth(&glyph, layout::SNIPPET_RUNG, true) / 2
                                    ),
                                    acy.saturating_sub(
                                        AVATAR_GLYPH_DY + line_growth(layout::SNIPPET_RUNG) / 2
                                    ),
                                    layout::SNIPPET_RUNG,
                                    &glyph,
                                    THEME.ink_on_dark,
                                    avatar_color(tr.peer()),
                                ) {
                                    return c;
                                }
                                return avatar_color(tr.peer());
                            }

                            // 2. Peer handle / name, on the row's text column, cut
                            // to the lane the unread pill leaves it.
                            let display_peer = f.resolve_contact_name(tr.peer());
                            let mut peer_name = [0u8; PEER_NAME_CAP];
                            let peer_len = fit_run(
                                display_peer,
                                layout::PEER_NAME_RUNG,
                                PEER_NAME_MAX_W,
                                true,
                                &mut peer_name,
                            );
                            if let Some(c) = test_text_bold(
                                x,
                                y,
                                layout::ROW_TEXT_X,
                                card_top + row_name_dy(),
                                layout::PEER_NAME_RUNG,
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
                            //
                            // WHY the lane is the pill-derived one: the painter
                            // trimmed this run to `CARD_X1 - CARD_X0 - 180`, a
                            // remembered number that stopped it four columns short
                            // of the unread pill and eight columns past the peer
                            // name's own bound -- and the run is inked bold, so the
                            // width the trim measured was never the width painted.
                            // It now answers the same edge the name does.
                            let mut snippet = [0u8; 48];
                            let slen = if messages.hidden_previews[ledger_idx] {
                                copy_ascii(HIDDEN_PREVIEW_LABEL, &mut snippet)
                            } else if let Some(last_msg) = messages.ledger.latest_message(tr.id) {
                                fit_run(
                                    last_msg.text(),
                                    layout::SNIPPET_RUNG,
                                    layout::SNIPPET_LANE,
                                    true,
                                    &mut snippet,
                                )
                            } else {
                                let placeholder = b"(no messages)";
                                copy_ascii(placeholder, &mut snippet)
                            };

                            if let Some(c) = test_text_bold(
                                x,
                                y,
                                layout::ROW_TEXT_X,
                                card_top + row_snippet_dy(),
                                layout::SNIPPET_RUNG,
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
                                let px0 = layout::UNREAD_PILL_X;
                                let px1 = layout::UNREAD_PILL_RIGHT;
                                let py0 = card_top + 34;
                                let py1 = card_top + 68;
                                if x >= px0 && x < px1 && y >= py0 && y < py1 {
                                    let mut digit = [0u8; 4];
                                    let dn = put_u8(&mut digit, tr.unread_count);
                                    if let Some(c) = test_text_bold(
                                        x,
                                        y,
                                        px0 + layout::UNREAD_COUNT_INSET,
                                        py0 + 5,
                                        layout::UNREAD_COUNT_RUNG,
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
                if let Some(c) = test_text(
                    x,
                    y,
                    layout::RECIPIENT_PREFIX_X,
                    f.recipient.y + caption_shift(),
                    layout::FIELD_PREFIX_RUNG,
                    layout::RECIPIENT_PREFIX_LABEL,
                    THEME.on_surface_variant,
                    THEME.surface_container,
                ) {
                    return c;
                }
                if let Some(c) = editor_pixel(x, y, &f.recipient, messages.keyboard.is_focused(Field::Recipient), THEME.surface_container) {
                    return c;
                }

                // Clear 'x' indicator when recipient has text
                if messages.recip_len > 0 && x >= FIELD_CLEAR_X0 && x < FIELD_CLEAR_X1
                    && y >= recipient_band_y0(messages) + 6
                    && y < recipient_band_y1(messages) - 6
                {
                    let cx = FIELD_CLEAR_CX as isize;
                    let cy = (rfield.1 + rfield.3 / 2) as isize;
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
                if messages.compose_len > 0
                    && x >= FIELD_CLEAR_X0
                    && x < FIELD_CLEAR_X1
                    && y >= cfield.1 + 10
                    && y < cfield.1 + 50
                {
                    let cx = FIELD_CLEAR_CX as isize;
                    let cy = (cfield.1 + cfield.3 / 2) as isize;
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
                    // WHY the published advance: the counter is right-aligned on
                    // this very width, so at a raised rung a ladder measurement
                    // slid the digits left into the text the field is typing over.
                    // At the default level it is `font::measure`'s own number.
                    let cw = run_w(&count_buf[..cn], layout::COUNTER_RUNG, false);
                    if let Some(c) = test_text(
                        x,
                        y,
                        COMPOSE_COUNT_RIGHT.saturating_sub(cw),
                        f.compose.y + caption_shift(),
                        layout::COUNTER_RUNG,
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
            if in_rect(send_lane(messages), x, y) {
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

        // Threat Warning Banner or clean header info bar (y search_y0()..search_y1())
        if y >= search_y0() && y < search_y1() {
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

            if in_rect(search_field(), x, y) {
                let rect = primitives::Rect::new(search_field().0, search_field().1, search_field().2, search_field().3);

                if show_threat {
                    // Warning icon, in the band's leading glyph lane.
                    let cx = SEARCH_ICON_CX;
                    let cy = search_icon_cy();
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
                // WHY the line box and not a literal: the run is drawn at scale
                // 1, so it centres on that rung's line box -- the same rule the
                // search field's own text follows in `build_frame`. The box is
                // asked at the published level, or a raised rung centres the banner
                // on a box shorter than the ink it stands in.
                let text_y =
                    (search_field().1 + search_field().3 / 2).saturating_sub(line_h(1) / 2);
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
        //
        // WHY the gate is `content_top()` and not the literal 168 it was: the
        // bubbles are laid out from `convo_top()`, and a gate eight rows above
        // that painted a dead strip of surface between the band's hairline and
        // the first bubble.
        if y >= content_top() && y < recipient_band_y0(messages) {
            let active_id = messages.active_thread_id.unwrap_or(0);
            let mut msgs = [MessageRecord::EMPTY; messages::MAX_MESSAGES_PER_THREAD];
            let count = messages.ledger.get_thread_messages(active_id, &mut msgs);

            if count == 0 {
                // Empty conversation info banner
                let line1 = b"No messages in this chat yet";
                let line2 = b"Type a draft below to compose";
                // WHY the published advance and not `font::measure_bold`: both
                // lines are centred on the whole canvas with no lane to trim
                // them to, so the only arithmetic that has to follow the rung is
                // the width the ink actually takes. Measuring at the ladder
                // centred a run that is wider than the number it was centred
                // with, which is a few columns off to the left at a raised rung.
                let l1_w = run_w(line1, 1, true);
                let l2_w = run_w(line2, 1, false);
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
                    // WHY the subtraction saturates: a thread holds sixteen
                    // bubbles and the band shows twelve, so the scroll extent
                    // runs to 16 * 92 - 1096 = 376 rows -- further than
                    // `convo_top()` itself. The top of the oldest bubble is then
                    // above row zero, and `usize` has no such number: in the
                    // checked build it aborted the task as soon as the user
                    // scrolled a long thread, and unchecked it wrapped to a
                    // top no pixel could reach, so the bubble silently stopped
                    // existing. Clamping is exact here because of the gate on
                    // `convo_top() >= bubble_h()` above.
                    let b_top = (convo_top() + i * bubble_step()).saturating_sub(scroll);
                    // WHY the guard is asked per bubble and not folded away:
                    // the clamp hides a scrolled-out bubble entire only while the
                    // band's top stands below one bubble's whole height -- the
                    // clamped bubble then ends above the band and the band's own
                    // gate paints none of it. At a rung whose bubble is taller
                    // than that, the clamped bubble would show its foot inside
                    // the band and its head nowhere, so it leaves the screen
                    // instead of arriving cut.
                    if b_top < convo_top() && !convo_top_holds_a_bubble() {
                        continue;
                    }
                    let b_bot = b_top + bubble_h();
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
                        //
                        // WHY the hug spends the same air the text lane removes:
                        // the body run is trimmed to `bubble - 2 * BUBBLE_PAD_X`
                        // and the bubble is grown by the same room, so a message
                        // that fits the bubble is a message that fits its lane.
                        //
                        // WHY both advances come from the published level: the hug
                        // is the only thing that decides which column the bubble
                        // ends at, and the lane the body is cut to is derived from
                        // it -- a ladder measurement at a raised rung built a plate
                        // narrower than its own ink, so the message ran out of the
                        // bubble and the trim cut a run the plate had no room for
                        // anyway. At the default level `run_w` is `measure_bold`,
                        // so every bubble keeps the columns it shipped with.
                        let mut bubble_w =
                            run_w(msgs[i].text(), layout::BUBBLE_BODY_RUNG, true)
                                + layout::BUBBLE_HUG_PAD;
                        let status_w = run_w(status_label, layout::BUBBLE_STATUS_RUNG, true)
                            + layout::BUBBLE_HUG_PAD;
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
                            let bubble_rect =
                                primitives::Rect::new(bx0, b_top, bx1 - bx0, bubble_h());

                            // Message body text, cut to the lane the bubble itself
                            // leaves it. WHY the app's fit and not `font::ellipsize`:
                            // the lane is a published-level box, and a run the ladder
                            // judged to fit it is wider than it at a raised rung -- it
                            // would then ink past the bubble's own edge. At the
                            // default level this hands the work to `ellipsize`, so the
                            // cut lands exactly where the shipped threads were cut.
                            let mut buf = [0u8; 96];
                            let n = fit_run(
                                msgs[i].text(),
                                layout::BUBBLE_BODY_RUNG,
                                layout::bubble_text_lane(bx1 - bx0),
                                true,
                                &mut buf,
                            );
                            if let Some(c) = test_text_bold(
                                x,
                                y,
                                bx0 + layout::BUBBLE_PAD_X,
                                b_top + 10,
                                layout::BUBBLE_BODY_RUNG,
                                &buf[..n],
                                ink_color,
                                fill_color,
                            ) {
                                return c;
                            }

                            // WHY the body's growth is added to the shipped offset:
                            // the status line sits a fixed 34 rows under the body's
                            // box top, and the bubble plate grows by both boxes. A
                            // rung that lifts the body's box without lifting the
                            // status with it puts the delivery state under the
                            // message's own descenders.
                            if let Some(c) = test_text_bold(
                                x,
                                y,
                                bx0 + layout::BUBBLE_PAD_X,
                                b_top + 44 + line_growth(layout::BUBBLE_BODY_RUNG),
                                layout::BUBBLE_STATUS_RUNG,
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
                if let Some(c) = test_text(
                    x,
                    y,
                    layout::RECIPIENT_PREFIX_X,
                    f.recipient.y + caption_shift(),
                    layout::FIELD_PREFIX_RUNG,
                    layout::RECIPIENT_PREFIX_LABEL,
                    THEME.on_surface_variant,
                    THEME.surface_container,
                ) {
                    return c;
                }
                if let Some(c) = editor_pixel(x, y, &f.recipient, messages.keyboard.is_focused(Field::Recipient), THEME.surface_container) {
                    return c;
                }

                // Clear 'x' indicator when recipient has text
                if messages.recip_len > 0 && x >= FIELD_CLEAR_X0 && x < FIELD_CLEAR_X1
                    && y >= recipient_band_y0(messages) + 6
                    && y < recipient_band_y1(messages) - 6
                {
                    let cx = FIELD_CLEAR_CX as isize;
                    let cy = (rfield.1 + rfield.3 / 2) as isize;
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
                if messages.compose_len > 0
                    && x >= FIELD_CLEAR_X0
                    && x < FIELD_CLEAR_X1
                    && y >= cfield.1 + 10
                    && y < cfield.1 + 50
                {
                    let cx = FIELD_CLEAR_CX as isize;
                    let cy = (cfield.1 + cfield.3 / 2) as isize;
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
                    // WHY the published advance: the counter is right-aligned on
                    // this very width, so at a raised rung a ladder measurement
                    // slid the digits left into the text the field is typing over.
                    // At the default level it is `font::measure`'s own number.
                    let cw = run_w(&count_buf[..cn], layout::COUNTER_RUNG, false);
                    if let Some(c) = test_text(
                        x,
                        y,
                        COMPOSE_COUNT_RIGHT.saturating_sub(cw),
                        f.compose.y + caption_shift(),
                        layout::COUNTER_RUNG,
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
            if in_rect(send_lane(messages), x, y) {
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
        if y >= details_top() && y < recipient_band_y0(messages) {
            for row in 0..f.detail_count {
                let top = details_top() + row * detail_row_h();
                if y >= top && y < top + detail_row_h() {
                    let line = &f.detail[row];
                    let rect = (CARD_X0, top, CARD_X1 - CARD_X0, row_card_h());
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
                    fit_run(
                        &messages.notice[..messages.notice_len],
                        layout::NOTICE_RUNG,
                        layout::card_row_lane(rect.2),
                        false,
                        &mut buf,
                    )
                } else {
                    fit_run(
                        b"Nothing has been published or shared yet.",
                        layout::NOTICE_RUNG,
                        layout::card_row_lane(rect.2),
                        false,
                        &mut buf,
                    )
                };
                if let Some(c) = test_text(
                    x,
                    y,
                    rect.0 + ROW_PAD_X,
                    rect.1 + 40,
                    layout::NOTICE_RUNG,
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
        if y >= recip_row_top() && y < recipient_band_y0(messages) {
            for row in 0..f.picker_count {
                let top = recip_row_top() + row * recip_row_h();
                if y >= top && y < top + recip_row_h() {
                    let line = &f.picker[row];
                    let rect = (CARD_X0, top, CARD_X1 - CARD_X0, row_card_h());
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
                    fit_run(
                        &messages.recip[..messages.recip_len],
                        layout::NOTICE_RUNG,
                        layout::card_row_lane(rect.2),
                        false,
                        &mut buf,
                    )
                } else {
                    copy_ascii(b"No recipient chosen", &mut buf)
                };
                if let Some(c) = test_text(
                    x,
                    y,
                    rect.0 + ROW_PAD_X,
                    rect.1 + 40,
                    layout::NOTICE_RUNG,
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

    // 5. The platform's home mark, on the rows no band of this app paints.
    if let Some(ink) = std::design::home_band::pixel(x, y, THEME.on_surface_variant) {
        return ink;
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
        std::log("[MESSAGES APP] The recipient is not a dialable number, so nothing was handed to the telephony service.").ok();
        return TransportOutcome::NotDialable;
    }
    let dest = match core::str::from_utf8(recipient) {
        Ok(s) => s,
        Err(_) => {
            std::log("[MESSAGES APP] The recipient is not a dialable number, so nothing was handed to the telephony service.").ok();
            return TransportOutcome::NotDialable;
        }
    };
    let text = match core::str::from_utf8(body) {
        Ok(s) => s,
        Err(_) => "",
    };
    match std::telephony_send_sms(dest, text) {
        Ok(_) => {
            std::log("[MESSAGES APP] The telephony service accepted the composed message.").ok();
            TransportOutcome::Accepted
        }
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
    if HOME_HIT.contains(x, y) {
        return Tap::Leave("[MESSAGES APP] Home gesture pressed. Exiting to launcher.");
    }

    // Header Back control: the platform chip, answered by the box the painter
    // inks and nothing else.
    //
    // WHY the old union is gone: the header paints the chip through
    // `back_button::pixel_for_theme`, whose plate is 24..72, while this press
    // also answered a hand-written 24..100 band. The columns between the plate
    // and the band's edge were bare `THEME.surface_container` -- the accent dot
    // at x 96 is a status mark, not a control -- so the old band let a tap on
    // dead space beside the button pop the view stack anyway. `contains` is the
    // SDK's own square for the same plate, and at 48x48 it still clears the
    // platform's touch floor, so matching the picture to the target costs no
    // reachability.
    if std::design::back_button::contains(x, y) {
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

    // The thread details screen's own rows. The card column bounds them, as
    // the painter bounds them: a press outside the card is not on a row.
    if app.view_mode == ViewMode::Details
        && x >= CARD_X0
        && x < CARD_X1
        && y >= details_top()
        && y < recipient_band_y0(app)
    {
        let mut detail = [DetailLine {
            label: b"",
            value: [0; 72],
            value_len: 0,
            action: DetailAction::None,
        }; DETAIL_ROWS];
        let count = build_detail_lines(app, &mut detail);
        let row = (y - details_top()) / detail_row_h();
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

    // The recipient picker's rows, inside the same card column.
    if app.view_mode == ViewMode::Recipients
        && x >= CARD_X0
        && x < CARD_X1
        && y >= recip_row_top()
        && y < recipient_band_y0(app)
    {
        let mut picker = [PickerLine::EMPTY; RECIP_ROWS];
        let count = build_picker_lines(app, &mut picker);
        let row = (y - recip_row_top()) / recip_row_h();
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
    if in_rect(pick_control(app), x, y) && compose_controls_shown(app) {
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

    // Send control: the disc the compose band draws, in the lane it draws it
    // in, on the screens that draw it at all.
    //
    // WHY the band is checked here and not only in the painter: the press used
    // to be tested against the circle alone, with no view and no rectangle, so
    // on the thread details and the recipient picker -- which paint a notice
    // card over exactly those rows -- an invisible send control sealed the
    // draft into the vault and posted it to the thread.
    if compose_controls_shown(app) && in_rect(send_lane(app), x, y) {
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
            // WHY there is no summary line here: the send used to print "Message
            // composed and processed." whatever had happened, so a press with no
            // recipient - which the app had just announced it was storing nothing
            // for - still read as a completed send. The transport, the vault and
            // the thread store each report their own answer on every path.
            return Tap::Redraw(dirty);
        }
    }

    // Composer text field -- drawn only on the two screens that carry the
    // bands, so only they may focus it.
    //
    // WHY it matters here: focusing it raises the keyboard, and the keyboard
    // moves the bands up to 850, which is where the details screen's rows are
    // still being laid out: a press on a notice card used to clip half of that
    // screen's report.
    if compose_controls_shown(app) && in_rect(compose_field(app), x, y) {
        if app.compose_len > 0 && x >= FIELD_CLEAR_X0 && x < FIELD_CLEAR_X1 {
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

    // Recipient text field, same band as the composer.
    if compose_controls_shown(app) && in_rect(recipient_field(app), x, y) {
        if app.recip_len > 0 && x >= FIELD_CLEAR_X0 && x < FIELD_CLEAR_X1 {
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
        if in_rect(search_field(), x, y) {
            if app.search_len > 0
                && x >= SEARCH_CLEAR_CX - SEARCH_CLEAR_HALF_W
                && x < SEARCH_CLEAR_CX + SEARCH_CLEAR_HALF_W
            {
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
        //
        // WHY the band's bottom edge is the painter's own edge: the row math
        // ran to the bottom of the screen, so a press in the recipient and
        // compose bands -- rows the list does not fill -- was fed to it as if
        // it were a row. The drag handler above already bounded the list to
        // `recipient_band_y0`; the tap was the one place that did not.
        if x >= CARD_X0 && x < CARD_X1 && y >= card_y0() && y < recipient_band_y0(app) {
            let scroll = app.list_scroll.offset;
            let rel = (y - card_y0()) + scroll;
            let idx = rel / card_h();
            let coff = rel % card_h();
            if coff < card_body_h() {
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

// -----------------------------------------------------------------------------
// Sovereign AppMCP Tool Dispatcher (Zero-Allocation & Bounded)
// -----------------------------------------------------------------------------

fn format_u32_str(mut val: u32, out: &mut [u8]) -> usize {
    if val == 0 {
        if !out.is_empty() {
            out[0] = b'0';
            return 1;
        }
        return 0;
    }
    let mut temp = [0u8; 10];
    let mut n = 0;
    while val > 0 && n < temp.len() {
        temp[n] = b'0' + (val % 10) as u8;
        val /= 10;
        n += 1;
    }
    let len = n.min(out.len());
    for i in 0..len {
        out[i] = temp[n - 1 - i];
    }
    len
}

fn parse_message_payload(payload: &[u8]) -> (&[u8], &[u8]) {
    if payload.is_empty() {
        return (b"", b"");
    }
    let mut delim = None;
    for (i, &b) in payload.iter().enumerate() {
        if b == b'|' || b == 0 {
            delim = Some(i);
            break;
        }
    }
    if let Some(i) = delim {
        let recip = &payload[..i];
        let body = if i + 1 < payload.len() {
            &payload[i + 1..]
        } else {
            b""
        };
        (recip, body)
    } else {
        (payload, b"")
    }
}

fn handle_appmcp_request(app: &mut MessagesApp, raw: &[u8], sender: u64, msg_type: u32) -> bool {
    if msg_type != std::appmcp::APPMCP_MSG_INVOKE {
        return false;
    }
    let Some(req) = std::appmcp::AppMcpRequest::from_bytes(raw) else {
        return false;
    };
    if req.cap_handle == 0 {
        let resp = std::appmcp::AppMcpResponse::error(
            std::appmcp::APPMCP_STATUS_PERMISSION_DENIED,
            req.tool_id,
            req.token,
        );
        let _ = std::ipc_send(sender, std::appmcp::APPMCP_MSG_RESPONSE, &resp.to_bytes());
        return true;
    }

    match req.tool_id {
        std::appmcp::TOOL_ID_MESSAGES_SEND_SMS => {
            let (recip, body) = parse_message_payload(req.payload());
            if recip.is_empty() || body.is_empty() {
                let resp = std::appmcp::AppMcpResponse::error(
                    std::appmcp::APPMCP_STATUS_INVALID_ARGUMENTS,
                    req.tool_id,
                    req.token,
                );
                let _ = std::ipc_send(sender, std::appmcp::APPMCP_MSG_RESPONSE, &resp.to_bytes());
                return true;
            }
            if app.store.append(recip, body) {
                app.sync_store_to_ledger();
                let resp = std::appmcp::AppMcpResponse::success(req.tool_id, req.token)
                    .with_payload(b"SMS saved to thread");
                let _ = std::ipc_send(sender, std::appmcp::APPMCP_MSG_RESPONSE, &resp.to_bytes());
            } else {
                let resp = std::appmcp::AppMcpResponse::error(
                    std::appmcp::APPMCP_STATUS_STORAGE_FULL,
                    req.tool_id,
                    req.token,
                );
                let _ = std::ipc_send(sender, std::appmcp::APPMCP_MSG_RESPONSE, &resp.to_bytes());
            }
            true
        }
        std::appmcp::TOOL_ID_MESSAGES_LIST => {
            let mut summary = [0u8; 32];
            let mut pos = 0usize;
            for &b in b"threads:" {
                summary[pos] = b;
                pos += 1;
            }
            pos += format_u32_str(app.store.len() as u32, &mut summary[pos..]);
            let resp = std::appmcp::AppMcpResponse::success(req.tool_id, req.token)
                .with_payload(&summary[..pos]);
            let _ = std::ipc_send(sender, std::appmcp::APPMCP_MSG_RESPONSE, &resp.to_bytes());
            true
        }
        std::appmcp::TOOL_ID_MESSAGES_READ => {
            let (recip, _) = parse_message_payload(req.payload());
            let thread_opt = if !recip.is_empty() {
                app.store.find(recip).and_then(|idx| app.store.get(idx))
            } else {
                app.store.get(0)
            };
            if let Some(thread) = thread_opt {
                let snippet = thread.last_snippet();
                let resp = std::appmcp::AppMcpResponse::success(req.tool_id, req.token)
                    .with_payload(snippet);
                let _ = std::ipc_send(sender, std::appmcp::APPMCP_MSG_RESPONSE, &resp.to_bytes());
            } else {
                let resp = std::appmcp::AppMcpResponse::error(
                    std::appmcp::APPMCP_STATUS_NOT_FOUND,
                    req.tool_id,
                    req.token,
                );
                let _ = std::ipc_send(sender, std::appmcp::APPMCP_MSG_RESPONSE, &resp.to_bytes());
            }
            true
        }
        std::appmcp::TOOL_ID_MESSAGES_DELETE => {
            let (recip, _) = parse_message_payload(req.payload());
            if let Some(idx) = app.store.find(recip) {
                if app.store.remove(idx) {
                    app.sync_store_to_ledger();
                    let resp = std::appmcp::AppMcpResponse::success(req.tool_id, req.token);
                    let _ = std::ipc_send(sender, std::appmcp::APPMCP_MSG_RESPONSE, &resp.to_bytes());
                } else {
                    let resp = std::appmcp::AppMcpResponse::error(
                        std::appmcp::APPMCP_STATUS_INTERNAL_ERROR,
                        req.tool_id,
                        req.token,
                    );
                    let _ = std::ipc_send(sender, std::appmcp::APPMCP_MSG_RESPONSE, &resp.to_bytes());
                }
            } else {
                let resp = std::appmcp::AppMcpResponse::error(
                    std::appmcp::APPMCP_STATUS_INVALID_ARGUMENTS,
                    req.tool_id,
                    req.token,
                );
                let _ = std::ipc_send(sender, std::appmcp::APPMCP_MSG_RESPONSE, &resp.to_bytes());
            }
            true
        }
        _ => {
            let resp = std::appmcp::AppMcpResponse::error(
                std::appmcp::APPMCP_STATUS_TOOL_NOT_FOUND,
                req.tool_id,
                req.token,
            );
            let _ = std::ipc_send(sender, std::appmcp::APPMCP_MSG_RESPONSE, &resp.to_bytes());
            true
        }
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

    // 0. The person's own text size, before a single pixel is placed.
    //
    // WHY here and not inside `MessagesApp::new`: `new` is a `const fn`, so it
    // cannot ask the store, and every box this file lays out reads the cell the
    // publish below writes. Handing the keyboard module the rows the recipient
    // band has to give its field is the same fact in the second place it is
    // needed -- the bands' edges are the keyboard module's own constants, and a
    // dock and a field that disagree about the band's height overlap.
    //
    // WHY the order: `band_extra_rows()` measures the field at the published
    // rung, so it has to be read after the publish and before the first
    // `bands_top` is asked.
    publish_user_text_scale();
    messages::keyboard::set_band_extra_rows(band_extra_rows());

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
            let payload = &modem_buf[..modem_len];
            let present = core::str::from_utf8(payload)
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
            // WHY the level comes from `"bars"` and not from `present`: the
            // status band used to draw four filled bars off the presence
            // boolean, a full-strength reading for a radio that may be idle.
            // This is the same field the dialer's meter reads.
            match json_u8(payload, "\"bars\"") {
                Some(bars) => {
                    messages.signal_bars = if bars > 4 { 4 } else { bars };
                }
                None => {
                    messages.signal_bars = 0;
                    std::log("[MESSAGES APP] Modem telemetry carried no signal bar count; the band draws no bars.",
                    )
                    .ok();
                }
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
    } else {
        let mut rx = [0u8; 64];
        if let Ok((sender, msg_type, len)) = std::ipc::receive_any(&mut rx) {
            if handle_appmcp_request(&mut messages, &rx[..len], sender, msg_type) {
                std::log("[MESSAGES APP] Headless AppMCP tool invocation handled successfully.").ok();
                std::exit_with_code(0);
            }
        }
        std::log("[MESSAGES APP] Display surface unavailable; refusing execution.").ok();
        std::exit_with_code(1);
    }

    // 4. Sovereign Asynchronous Event Reactor Loop
    let interest_mask =
        std::EVENT_TOUCH_INPUT | std::EVENT_IPC_MSG | std::EVENT_MODEM_URC | std::EVENT_TIMER_TICK;
    loop {
        // WHY on every wake rather than once at launch: the person changes the
        // text size in Settings while this screen is alive, and a rung read only
        // at startup resizes nothing until the app is opened cold again. This is
        // one settings query per wake, ahead of the blocking poll -- never one
        // per glyph, which is the cost the note beside `publish_user_text_scale`
        // refuses to pay.
        let rung_moved = publish_user_text_scale();
        if rung_moved {
            // The dock's rows are the recipient band's height at the rung, so the
            // keyboard module has to be told again before anything measures a
            // band on this wake.
            messages::keyboard::set_band_extra_rows(band_extra_rows());
        }
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
                                && input.y >= convo_top() as u32
                                && (input.y as usize) < recipient_band_y0(&messages);
                            let over_list = messages.view_mode == ViewMode::ThreadList
                                && input.y >= card_y0() as u32
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
                    // WHY a moved rung is a whole-screen repaint: every band on
                    // this screen -- search row, recipient field, thread list,
                    // conversation, dock -- is derived from the rung the publish
                    // above just wrote, so a repaint of one band would leave the
                    // others sized for the size the person left.
                    let dirty = if rung_moved { Dirty::All } else { dirty };
                    if dirty != Dirty::None && surface > 0 {
                        match dirty {
                            Dirty::None => {}
                            Dirty::Search => {
                                render_messages_band(surface, &messages, search_y0(), search_y1());
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
                    let mut rx = [0u8; 64];
                    if let Ok((sender, msg_type, len)) = std::ipc::receive_any(&mut rx) {
                        if handle_appmcp_request(&mut messages, &rx[..len], sender, msg_type) {
                            if surface > 0 {
                                messages.sync_list_extent();
                                messages.sync_convo_extent();
                                render_messages_ui(surface, &messages);
                                let _ = std::surface_flip();
                            }
                        } else {
                            std::log("[MESSAGES APP] IPC message received (non-AppMCP).").ok();
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
    std::log("[MESSAGES APP] Panic detected! Terminating task...").ok();
    std::exit();
}
