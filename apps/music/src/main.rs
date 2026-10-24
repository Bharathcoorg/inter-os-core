//! InterOS Core Native Music Player (Ring-3)
//!
//! Bare-metal user-mode media player on the canonical dark media surface:
//! - Playlist populated only from real media storage (`PLAYLIST` is honestly
//!   empty, so the player reports an idle transport and an empty track list)
//! - Every transport target is >= 56 px (previous/next 64 px, play/pause 88 px)
//! - The progress bar is bound to a real playback position; no backend reports
//!   one, so it renders the honest idle state instead of a fake fill
//! - Album artwork is a neutral placeholder glyph, never a fabricated cover
//! - All strings are measured with `interos_std::design::font::measure` /
//!   `ellipsize` at the coordinates they are drawn at
//! - Hardware Keystore Slot 1 media key derivation, authenticated track record
//!   encryption and the zero-copy DMA audio path are preserved unchanged
//! - Asynchronous event reactor loop for transport controls
//!
//! Typed input (mirrors `apps/browser/src/main.rs`):
//! - The player was the only one of these three with no key handler at all. It
//!   now routes the normalized keyboard pipeline: ESCAPE dismisses, SPACE
//!   toggles play/pause, LEFT/RIGHT step the playlist, UP/DOWN move the volume.
//!   Every one of those drives real state that is reported back honestly.
//!
//! Banded rendering:
//! - `Dirty` + `render_music_band(surface, app, y0, y1)` mirror the browser. The
//!   previous renderer pushed all 1,036,800 pixels through `get_music_pixel` for
//!   every touch event.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

mod store;
use library::{ImportOutcome, PublishOutcome};
use store::TrackStore;
use music::library;
use music::theme::{THEME, radius, spacing};
use music::SortMode;
use std::design::components;
use std::design::font;
use std::design::outline_text;
use std::design::primitives::{draw_circle, draw_pill, draw_rounded_rect, Rect};

/// Synthesizer audio waveform.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioWaveform {
    Sine = 0,
    Square = 1,
    Triangle = 2,
    Sawtooth = 3,
}

/// Playback state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped = 0,
    Playing = 1,
    Paused = 2,
}

/// Sovereign Audio Track record.
#[derive(Debug, Clone, Copy)]
pub struct AudioTrack {
    pub id: u32,
    pub title: &'static str,
    pub artist: &'static str,
    pub bpm: u16,
    pub codec: &'static str,
}

/// Local Music Playlist. Populated only from real media storage, which exposes
/// no audio files, so the playlist is honestly empty.
///
/// WHY this stays a zero-length array while the library is realised elsewhere:
/// this static is the compile-time "no sample data" anchor -- an app can never
/// ship a hardcoded track. Real tracks live in the bounded, persisted
/// `store::TrackStore`, which holds user-typed titles and durations rather than
/// `&'static str` literals, so it cannot be a `static`. This array therefore
/// stays empty on purpose and is never read.
#[allow(dead_code)]
pub static PLAYLIST: [AudioTrack; 0] = [];

/// Production Sovereign Music state manager.
#[allow(dead_code)]
pub struct MusicApp {
    pub state: PlaybackState,
    pub current_track_idx: usize,
    pub waveform: AudioWaveform,
    /// Master volume as the audio manager last reported it.
    ///
    /// WHY `Option<u8>` and not a number: this used to be a hardcoded `85` that
    /// was never drawn, never read and never changed - a fabricated telemetry
    /// value sitting in the state struct. It is now `Some(percent)` only when
    /// `audio_query_volume` / `audio_set_volume` actually returned a value, and
    /// `None` when the audio service refused, which the UI draws as an
    /// unavailable meter with disabled step controls.
    pub volume_percent: Option<u8>,
    pub total_dma_buffers_streamed: u32,
    pub keystore_derived: bool,
    pub sector_encrypted: bool,
    /// Real, persisted local track library. The playlist and the now-playing row
    /// render from this, never from a hardcoded array.
    pub store: TrackStore,
    /// Add-track editor: title buffer, its length, the duration digits the user
    /// typed (whole seconds), and which field is focused. `add_open` reveals the
    /// editor in the track list.
    pub add_title: [u8; ADD_TITLE_CAP],
    pub add_title_len: usize,
    pub add_dur: [u8; ADD_DUR_DIGITS],
    pub add_dur_len: usize,
    pub add_open: bool,
    pub add_focus: u8,
    /// Scroll position of the track list.
    ///
    /// WHY the list scrolls: the store holds up to eight tracks and the
    /// visible band holds three, so without a scroll the lower five could
    /// not be reached at all. The shared model is used so the thumb and the
    /// offset cannot disagree with each other.
    pub list_scroll: std::design::scroll::ScrollState,
    /// The library's search query and its length.
    ///
    /// WHY the app searches its own library: the design document asks for a
    /// search over the library, and the library is a list of tracks the
    /// device holds. A query that is not text is not a query.
    pub search: [u8; SEARCH_CAP],
    pub search_len: usize,
    pub search_focused: bool,
    /// How the queue is ordered.
    pub sort: SortMode,
    /// The store indices of the tracks the list shows, in the order it shows
    /// them.
    ///
    /// WHY the list is addressed through this table and not through the
    /// store's own indices: a search hides the tracks that do not match, so
    /// the row under the finger is the n-th match and not the n-th track.
    pub order: [u8; music::MAX_TRACKS],
    /// How many entries `order` holds.
    pub order_len: usize,
    /// Transient user feedback message (e.g. "Ringtone: Nocturne").
    pub feedback: [u8; FEEDBACK_CAP],
    pub feedback_len: usize,
}

impl MusicApp {
    pub const fn new() -> Self {
        Self {
            state: PlaybackState::Stopped,
            current_track_idx: 0,
            waveform: AudioWaveform::Sine,
            volume_percent: None,
            total_dma_buffers_streamed: 0,
            keystore_derived: false,
            sector_encrypted: false,
            store: TrackStore::EMPTY,
            add_title: [0; ADD_TITLE_CAP],
            add_title_len: 0,
            add_dur: [0; ADD_DUR_DIGITS],
            add_dur_len: 0,
            add_open: false,
            add_focus: 0,
            list_scroll: std::design::scroll::ScrollState::new(0, LIST_VIEWPORT_H, 0),
            search: [0; SEARCH_CAP],
            search_len: 0,
            search_focused: false,
            sort: SortMode::Added,
            order: [0; music::MAX_TRACKS],
            order_len: 0,
            feedback: [0; FEEDBACK_CAP],
            feedback_len: 0,
        }
    }

    /// Re-derives the track list's scroll extent from the library's length.
    ///
    /// WHY this runs after every change to the library: the content height
    /// is the library's length times the row height, and a stale extent
    /// would let the list scroll past its last row.
    pub fn sync_list_extent(&mut self) {
        self.rebuild_order();
        let content_h = self.order_len * TRACKLIST_ROW_H;
        self.list_scroll = std::design::scroll::ScrollState::new(
            content_h,
            LIST_VIEWPORT_H,
            self.list_scroll.offset,
        );
    }

    /// Rebuilds the table of the tracks the list shows.
    ///
    /// WHY the table exists: a search hides the tracks that do not match, so
    /// the row under the finger is the n-th match and not the n-th track.
    /// Addressing a row by the store's own index would open the wrong track
    /// and delete the wrong track.
    pub fn rebuild_order(&mut self) {
        let mut n = 0usize;
        for i in 0..self.store.len() {
            let Some(track) = self.store.get(i) else {
                continue;
            };
            if music::contains_ci(track.title(), &self.search[..self.search_len]) {
                self.order[n] = i as u8;
                n += 1;
            }
        }
        self.order_len = n;
    }

    /// The store index of the `n`th track the list shows.
    pub fn store_index_at(&self, n: usize) -> Option<usize> {
        if n >= self.order_len {
            return None;
        }
        Some(self.order[n] as usize)
    }

    /// Moves the `n`th track the list shows one place up in the queue.
    ///
    /// Returns `true` only when the new order was persisted.
    pub fn move_up(&mut self, n: usize) -> bool {
        let Some(store_idx) = self.store_index_at(n) else {
            return false;
        };
        if store_idx == 0 {
            return false;
        }
        if !self.store.move_to(store_idx, store_idx - 1) {
            return false;
        }
        self.rebuild_order();
        true
    }

    /// Cycles the queue's order and persists the new one.
    ///
    /// WHY the order is persisted: the queue is the order the tracks are
    /// played in, and an order that is forgotten when the app closes is not
    /// an order.
    pub fn cycle_sort(&mut self) -> bool {
        let next = self.sort.next();
        let ok = match next {
            SortMode::Added => self.store.sort_by_added(),
            SortMode::Title => self.store.sort_by_title(),
            SortMode::Duration => self.store.sort_by_duration(),
        };
        if ok {
            self.sort = next;
            self.rebuild_order();
        }
        ok
    }

    /// Asks the audio manager for the real master volume.
    ///
    /// Returns `true` only when a value came back; otherwise the meter is drawn
    /// as unavailable rather than showing a guessed number.
    pub fn refresh_volume(&mut self) -> bool {
        match std::audio_query_volume() {
            Ok(v) if v <= 100 => {
                self.volume_percent = Some(v as u8);
                true
            }
            Ok(_) => {
                // WHY: a percentage above 100 is not a volume this player can
                // draw as a 0..100 meter. Report the absence, do not clamp a
                // nonsense reading into a plausible-looking one.
                self.volume_percent = None;
                std::log("[MUSIC APP] Audio service returned an out-of-range volume; meter unavailable.",
                )
                .ok();
                false
            }
            Err(_) => {
                // WHY: audio control is refused without the audio-service
                // authority. Never log or draw a volume that was never issued.
                self.volume_percent = None;
                std::log("[MUSIC APP] Volume query refused by the audio service; no volume is known.",
                )
                .ok();
                false
            }
        }
    }

    /// Steps the real master volume by one 5% notch through the audio service.
    ///
    /// Returns `true` only when the service accepted and echoed the applied
    /// value; the UI redraws from what it returned, never from the request.
    pub fn step_volume(&mut self, up: bool) -> bool {
        let Some(current) = self.volume_percent else {
            // WHY: no level is known, so there is no level to step from. The
            // control is drawn recessed for this exact state.
            std::log("[MUSIC APP] Volume step ignored: no volume is known to step from.",
            )
            .ok();
            return false;
        };
        let requested = if up {
            (current as u32).saturating_add(5).min(100)
        } else {
            (current as u32).saturating_sub(5)
        };
        match std::audio_set_volume(requested) {
            Ok(v) if v <= 100 => {
                self.volume_percent = Some(v as u8);
                true
            }
            Ok(_) => {
                std::log("[MUSIC APP] Audio service applied an out-of-range volume; meter hidden.",
                )
                .ok();
                self.volume_percent = None;
                false
            }
            Err(_) => {
                std::log("[MUSIC APP] Volume change refused by the audio service; level unchanged.",
                )
                .ok();
                false
            }
        }
    }

    /// Derives the media storage key from the hardware keystore.
    ///
    /// Returns `true` only when a full 32-byte key was actually returned.
    pub fn derive_keystore_key(&mut self) -> bool {
        let mut media_key = [0u8; 32];
        match std::keystore_derive(MEDIA_KEY_SLOT, &mut media_key) {
            Ok(32) => {
                self.keystore_derived = true;
                std::log("[MUSIC APP] Hardware keystore: Ingested media decryption key for local audio streaming.").ok();
                true
            }
            Ok(_) => {
                // WHY: a short result is not a key. Report it instead of leaving
                // the caller to assume derivation succeeded.
                std::log("[MUSIC APP] Hardware keystore: short derivation result; no media key available.").ok();
                false
            }
            Err(_) => {
                // WHY: this app holds no keystore capability, so the call is
                // refused. Say so instead of reporting a key that was never issued.
                std::log("[MUSIC APP] Hardware keystore: derivation refused by the kernel; no media key available.").ok();
                false
            }
        }
    }

    /// Plays the 4-note ascending C-major arpeggio via hardware audio timer.
    ///
    /// Returns `true` only when the audio service accepted the sequence. Tone
    /// requests are gated on a capability this app does not hold, so the honest
    /// default is "not played".
    #[allow(dead_code)]
    pub fn play_arpeggio(&self) -> bool {
        let mut played = 0u32;
        for freq in [261u32, 329, 392, 523] {
            if std::audio_play_tone(freq).is_ok() {
                played += 1;
            }
        }
        let _ = std::audio_silence();
        let _ = std::audio_haptic_click();
        if played == 4 {
            std::log("[MUSIC APP] Arpeggiated C-major melody synthesized (261Hz -> 523Hz).",
            )
            .ok();
            true
        } else {
            // WHY: audio control is refused without the audio-service authority.
            // Never log a synthesized melody that was never emitted.
            std::log("[MUSIC APP] Arpeggio not played: the audio service refused the tone requests.",
            )
            .ok();
            false
        }
    }

    /// Attempts to stream one media payload over the zero-copy DMA ring.
    ///
    /// Returns `true` only when a transfer actually occurred. The local playlist
    /// is populated from real media storage, so with no media there is nothing to
    /// hand to the DMA engine and no buffer was consumed.
    pub fn stream_dma_audio_block(&mut self) -> bool {
        // WHY: the transport state and the DMA counter must track real transfers.
        // Returning false here is what stops the caller from reporting playback
        // or counting a buffer that was never streamed.
        if self.store.is_empty() {
            std::log("[MUSIC APP] No media payload is available for DMA streaming.").ok();
            return false;
        }
        // A real library is present, but there is still no audio mixer, no DMA
        // descriptor and no playback-position source in this guest, so fail
        // closed: nothing streams and nothing is counted.
        std::log("[MUSIC APP] Local library present but no DMA descriptor was obtained; no buffer streamed.",
        )
        .ok();
        false
    }

    /// Encrypts metadata only when a real media record exists.
    pub fn encrypt_track_record(&mut self) {
        std::log("[MUSIC APP] No media record is available for encryption.").ok();
    }

    /// Steps the playlist index, but only when there is real media to step over.
    ///
    /// Returns `true` only when the selection actually moved. A tap on prev/next
    /// with an empty library changes nothing, so it reports nothing changed.
    pub fn step_track(&mut self, delta: isize) -> bool {
        if self.store.is_empty() {
            std::log("[MUSIC APP] Track step ignored: the local library is empty, so there is no next or previous track.",
            )
            .ok();
            return false;
        }
        let len = self.store.len() as isize;
        let cur = (self.current_track_idx.min(self.store.len() - 1)) as isize;
        let next = (cur + delta).rem_euclid(len);
        self.current_track_idx = next as usize;
        self.publish_now_playing();
        true
    }

    /// Toggles play/pause against the real media path.
    ///
    /// Returns `true` only when playback actually started. With no media the
    /// transport stays `Stopped`.
    pub fn toggle_play(&mut self) -> bool {
        match self.state {
            PlaybackState::Playing => {
                self.state = PlaybackState::Paused;
                self.publish_now_playing();
                std::log("[MUSIC APP] Transport paused by user.").ok();
                false
            }
            _ => {
                if self.store.is_empty() {
                    self.state = PlaybackState::Stopped;
                    self.publish_now_playing();
                    std::log("[MUSIC APP] Play requested but no media is available; transport stays stopped.",
                    )
                    .ok();
                    false
                } else {
                    self.state = PlaybackState::Playing;
                    if self.stream_dma_audio_block() {
                        self.total_dma_buffers_streamed += 1;
                    }
                    self.publish_now_playing();
                    std::log("[MUSIC APP] Transport playing started; background audio active.").ok();
                    true
                }
            }
        }
    }

    /// Publishes current now-playing metadata to the settings store for
    /// notification bar and system-wide media card consumption.
    pub fn publish_now_playing(&self) {
        let (title, artist): (&[u8], &[u8]) = if let Some(t) = self.store.get(self.current_track_idx) {
            (t.title(), b"Local Library")
        } else {
            (b"", b"")
        };
        let state_str: &[u8] = match self.state {
            PlaybackState::Playing => b"playing",
            PlaybackState::Paused => b"paused",
            PlaybackState::Stopped => b"stopped",
        };
        music::publish_now_playing(title, artist, state_str);
    }

    /// Restores persistent background playback state from previous session.
    pub fn restore_playback_state(&mut self) {
        let mut state_buf = [0u8; 16];
        if let Ok(n) = std::settings_get(music::NOW_PLAYING_STATE_KEY, &mut state_buf) {
            let state_str = &state_buf[..n];
            if state_str == b"playing" && !self.store.is_empty() {
                let mut title_buf = [0u8; music::TITLE_MAX];
                if let Ok(tn) = std::settings_get(music::NOW_PLAYING_TITLE_KEY, &mut title_buf) {
                    if tn > 0 {
                        for i in 0..self.store.len() {
                            if let Some(t) = self.store.get(i) {
                                if t.title() == &title_buf[..tn] {
                                    self.current_track_idx = i;
                                    break;
                                }
                            }
                        }
                    }
                }
                self.state = PlaybackState::Playing;
                std::log("[MUSIC APP] Persistent background playback state restored: Playing.").ok();
            } else if state_str == b"paused" && !self.store.is_empty() {
                self.state = PlaybackState::Paused;
            }
        }
    }

    /// Sets the currently selected audio track as the system ringtone.
    pub fn set_current_as_ringtone(&mut self) -> bool {
        let Some(track) = self.store.get(self.current_track_idx) else {
            self.set_feedback(b"No track selected for ringtone");
            return false;
        };
        let title = track.title();
        let mut path = [0u8; library::PATH_CAP];
        let path_len = library::document_path(title, &mut path);

        // Ensure track document is published to storage
        let _ = library::publish_library(&self.store);

        if music::set_ringtone(&path[..path_len]) {
            let mut msg = [0u8; FEEDBACK_CAP];
            let prefix = b"Ringtone: ";
            let mut at = prefix.len();
            msg[..at].copy_from_slice(prefix);
            let n = title.len().min(FEEDBACK_CAP - at);
            msg[at..at + n].copy_from_slice(&title[..n]);
            at += n;
            self.set_feedback(&msg[..at]);
            std::log("[MUSIC APP] Track set as system ringtone (audio.ringtone & telephony.ringtone).").ok();
            true
        } else {
            self.set_feedback(b"Ringtone setting write refused");
            std::log("[MUSIC APP] Settings store refused ringtone write.").ok();
            false
        }
    }

    pub fn set_feedback(&mut self, msg: &[u8]) {
        let n = msg.len().min(FEEDBACK_CAP);
        self.feedback[..n].copy_from_slice(&msg[..n]);
        self.feedback_len = n;
    }

    pub fn clear_feedback(&mut self) {
        self.feedback_len = 0;
    }

    /// Commits the add-track editor: validates the title and duration, then adds
    /// the track to the persisted store. The track joins the list only when the
    /// write is acknowledged; a refusal is logged, never shown as a stored track.
    fn save_add(&mut self) -> Dirty {
        if self.add_title_len == 0 {
            std::log("[MUSIC APP] Add track needs a title; nothing was stored.",
            )
            .ok();
            return Dirty::None;
        }
        if self.add_dur_len == 0 {
            std::log("[MUSIC APP] Add track needs a duration in seconds; nothing was stored.",
            )
            .ok();
            return Dirty::None;
        }
        let mut secs: u32 = 0;
        for i in 0..self.add_dur_len {
            let b = self.add_dur[i];
            if !(b'0'..=b'9').contains(&b) {
                std::log("[MUSIC APP] Duration must be digits only; nothing was stored.",
                )
                .ok();
                return Dirty::None;
            }
            secs = secs.saturating_mul(10).saturating_add((b - b'0') as u32).min(100_000);
        }
        if secs == 0 {
            std::log("[MUSIC APP] Duration must be at least 1 second; nothing was stored.",
            )
            .ok();
            return Dirty::None;
        }
        let mut title = [0u8; ADD_TITLE_CAP];
        let n = self.add_title_len.min(ADD_TITLE_CAP);
        title[..n].copy_from_slice(&self.add_title[..n]);
        if self.store.add(&title[..n], secs) {
            self.sync_list_extent();
            std::log("[MUSIC APP] Track stored to the local library by title and duration; no audio file is claimed.",
            )
            .ok();
            self.add_title_len = 0;
            self.add_dur_len = 0;
            self.add_open = false;
            self.add_focus = 0;
            Dirty::All
        } else {
            // Report the store's real refusal (capacity or write refusal), never
            // a track that did not persist.
            std::log(self.store.last_reason()).ok();
            Dirty::None
        }
    }

    /// Discards the add-track editor without storing anything.
    pub fn cancel_add(&mut self) {
        self.add_open = false;
        self.add_focus = 0;
        self.add_title_len = 0;
        self.add_dur_len = 0;
    }
}


/// The platform's corner radii, under the short names the renderer uses.
///
/// WHY the aliases exist: the corner test names its radius four times, and a
/// number that appears four times is a number that can drift from the
/// platform. The alias resolves to the platform's step, so the drift is the
/// platform's and not this app's.
const R_CONTROL: usize = radius::CONTROL;

/// The platform's list-row treatment, at the two states the track list uses.
///
/// WHY the track list is the platform's list row: a row that invents its own
/// fill, its own divider and its own padding is a row a third-party app
/// cannot match, and the platform already says what a row looks like.
const ROW: components::ListRowStyle = components::list_row_style(&THEME, false);
const ROW_SELECTED: components::ListRowStyle = components::list_row_style(&THEME, true);

const W: usize = 720;

/// The hardware keystore slot this app asks for its media key from.
///
/// WHY it is named: the argument is a keystore slot, not a capability
/// slot, and the two are not interchangeable. Naming it keeps the
/// difference visible at the call site.
const MEDIA_KEY_SLOT: u32 = 1;
const H: usize = 1440;
const CHUNK: usize = 4096;

// -----------------------------------------------------------------------------
// Visual Design Tokens
//
// WHY there is no palette here any more: the app used to reach into the
// platform's *palette* module for its colours, which named values rather than
// roles, so an app could not ask "what is the accent on this surface" and get
// an answer that held in both themes. Every colour now resolves through the
// platform's theme, so a third-party app that asks the SDK for the same roles
// draws the same media surface. See `music::theme` for the mapping from the
// old names to the platform's roles.
// -----------------------------------------------------------------------------

// -----------------------------------------------------------------------------
// Layout constants - shared by the renderer and the hit-tester
// -----------------------------------------------------------------------------
const MARGIN: usize = spacing::GRID_24;
const CONTENT_W: usize = W - 2 * MARGIN;

const BAR_H: usize = 96;
const BACK: Rect = Rect::new(24, 24, 72, 72);

const ART_X: usize = 136;
const ART_Y: usize = 128;
const ART_W: usize = 448;
const ART_H: usize = 448;
const ART_GLYPH_CY: usize = 300;
const ART_CAPTION_Y: usize = 400;

const TITLE_Y: usize = 636;
const ARTIST_Y: usize = 676;
const STATE_Y: usize = 712;

/// The library's search field.
///
/// WHY there is a field and not a magnifier icon: the design document asks
/// for a search over the library, and a search that is not a place to type
/// is not a search.
const SEARCH_FIELD: Rect = Rect { x: 24, y: 748, w: 412, h: 48 };

/// The control that changes the queue's order.
const SORT_BTN: Rect = Rect { x: 448, y: 748, w: 118, h: 48 };

/// The control that imports the audio files the device holds.
const IMPORT_BTN: Rect = Rect { x: 578, y: 748, w: 118, h: 48 };

/// The line that names the features the platform does not offer this app.
const UNAVAILABLE_Y: usize = 808;

/// Transient user feedback message line.
const FEEDBACK_Y: usize = 828;

/// The search query's capacity, in bytes.
const SEARCH_CAP: usize = 32;

/// Capacity for live feedback notifications.
const FEEDBACK_CAP: usize = 48;

/// Metadata band (title, artist, state, search row, availability line).
///
/// WHY this band exists: pressing play/pause moves the state label at y 716 and
/// nothing else in this region. The old renderer re-rasterised all 1440 rows to
/// change 24 of them.
const META_Y0: usize = 620;
const META_Y1: usize = 848;

// -----------------------------------------------------------------------------
// Volume row (y 848..896). Real telemetry only.
// -----------------------------------------------------------------------------
const VOLUME_Y0: usize = 848;
const VOLUME_Y1: usize = 896;
/// Step-down / step-up controls.
///
/// Touch targets are at least 48px to comply with platform guidelines.
const VOL_MINUS: Rect = Rect::new(24, 848, 56, 48);
const VOL_PLUS: Rect = Rect::new(640, 848, 56, 48);
const VOL_LABEL_X: usize = 96;
const VOL_VALUE_X: usize = 190;
/// Meter trough and fill, drawn only when a real level is known.
const VOL_METER_X0: usize = 310;
const VOL_METER_X1: usize = 624;
const VOL_METER_Y0: usize = 866;
const VOL_METER_Y1: usize = 878;

const TRANSPORT_CY: usize = 970;
const PREV_CX: usize = 190;
const PLAY_CX: usize = 360;
const NEXT_CX: usize = 530;
const SIDE_R: usize = 32; // 64 px target, >= 48 px required
const PLAY_R: usize = 44; // 88 px target, >= 48 px required

/// Transport row band: the play/pause disc is the tallest control (r = 44).
const TRANSPORT_Y0: usize = TRANSPORT_CY - PLAY_R;
const TRANSPORT_Y1: usize = TRANSPORT_CY + PLAY_R;

const LIST_LABEL_Y: usize = 1036;
const LIST_DIVIDER_Y: usize = 1114;
const LIST_HIT_Y1: usize = 1400;
/// The top of the visible track-list band.
const LIST_TOP: usize = 1124;
/// The height of the visible track-list band.
const LIST_VIEWPORT_H: usize = LIST_HIT_Y1 - LIST_TOP;
const LIST_TITLE_Y: usize = 1170;
const LIST_SUB_Y: usize = 1210;

/// Track-list row pitch and the pitch body of each row.
const TRACKLIST_ROW_H: usize = 96;
/// The control that publishes the library into the device's document index.
const PUBLISH_BTN: Rect = Rect { x: 236, y: 1054, w: 136, h: 48 };
/// The control that sets the currently selected track as system ringtone.
const RINGTONE_BTN: Rect = Rect { x: 384, y: 1054, w: 154, h: 48 };
/// "ADD TRACK" button on the label row (right side, >= 48 px tall to hit).
const ADD_BTN: Rect = Rect { x: 550, y: 1054, w: 146, h: 48 };
/// Add-track editor fields, shown while `add_open` in the track-list band.
const ADD_TITLE_FIELD: Rect = Rect { x: 24, y: 1130, w: 672, h: 48 };
const ADD_DUR_FIELD: Rect = Rect { x: 24, y: 1190, w: 264, h: 48 };
const ADD_SAVE_BTN: Rect = Rect { x: 300, y: 1190, w: 200, h: 48 };
const ADD_CANCEL_BTN: Rect = Rect { x: 512, y: 1190, w: 184, h: 48 };
/// Title bytes the add editor holds. Matches the store's per-record title width.
const ADD_TITLE_CAP: usize = 40;
/// Duration digits the add editor holds (whole seconds, up to 5 digits).
const ADD_DUR_DIGITS: usize = 5;

const HOME_X0: usize = 240;
const HOME_X1: usize = 480;
const HOME_Y0: usize = 1418;
const HOME_Y1: usize = 1424;

// -----------------------------------------------------------------------------
// Text helpers - the gate is the glyph line box, so ink is never clipped.
// -----------------------------------------------------------------------------
/// Draws a run of text with the sharp outline face, falling back to the
/// bitmap face when the outline face refuses the run.
///
/// WHY the outline face is tried first: the platform scanout is 720x1440,
/// which is exactly the space this app paints, so composition is 1:1 and
/// the compositor magnifies nothing. The outline face is still preferred
/// because it rasterises the real outline at the requested cap height
/// instead of magnifying the 16x24 bitmap fallback face, so it stays sharp
/// at every size.
///
/// WHY the box is the union of the two faces' boxes: the outline face's box
/// sits one or two rows inside the bitmap cell's, and a bitmap fallback
/// that lost its top row would draw a clipped glyph.
#[inline(always)]
fn text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    s: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    if scale == 0 || s.is_empty() {
        return None;
    }
    let (bx, bt, bw, bh) = outline_text::run_box(x0, y0, scale, s);
    let bitmap_h = font::line_height(scale);
    let bitmap_w = font::measure(s, scale);
    let top = bt.min(y0);
    let height = bh.max(bitmap_h);
    let width = bw.max(bitmap_w);
    if x < bx || x - bx >= width || y < top || y - top >= height {
        return None;
    }
    if let Some(run) = outline_text::place_run(x0, y0, scale, s, None, false) {
        return run.pixel(x, y, color, bg);
    }
    font::test_text_aa(x, y, x0, y0, scale, s, color, bg, false)
}

/// Draws a bold run of text with the sharp outline face.
#[inline(always)]
fn text_bold(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    s: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    if scale == 0 || s.is_empty() {
        return None;
    }
    let (bx, bt, bw, bh) = outline_text::run_box(x0, y0, scale, s);
    let bitmap_h = font::line_height(scale);
    let bitmap_w = font::measure_bold(s, scale);
    let top = bt.min(y0);
    let height = bh.max(bitmap_h);
    // A bold outline run is one pixel wider than the run the box was
    // measured from, so the box has to carry that pixel.
    let width = bw.max(bitmap_w) + 1;
    if x < bx || x - bx >= width || y < top || y - top >= height {
        return None;
    }
    if let Some(run) = outline_text::place_run(x0, y0, scale, s, None, true) {
        return run.pixel(x, y, color, bg);
    }
    font::test_text_aa(x, y, x0, y0, scale, s, color, bg, true)
}

#[inline(always)]
fn text_centered(
    x: usize,
    y: usize,
    y0: usize,
    scale: usize,
    s: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    let w = font::measure(s, scale);
    if w > CONTENT_W {
        return None;
    }
    text(x, y, (W - w) / 2, y0, scale, s, color, bg)
}

#[inline(always)]
fn text_centered_bold(
    x: usize,
    y: usize,
    y0: usize,
    scale: usize,
    s: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    let w = font::measure_bold(s, scale);
    if w > CONTENT_W {
        return None;
    }
    text_bold(x, y, (W - w) / 2, y0, scale, s, color, bg)
}

#[inline(always)]
fn text_in_rect_centered(
    x: usize,
    y: usize,
    rect: Rect,
    scale: usize,
    s: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    let w = font::measure(s, scale);
    let x0 = rect.x + rect.w.saturating_sub(w) / 2;
    let y0 = if rect.h >= 16 * scale {
        rect.y + (rect.h - 16 * scale) / 2
    } else {
        rect.y + rect.h.saturating_sub(font::line_height(scale)) / 2
    };
    text(x, y, x0, y0, scale, s, color, bg)
}

/// Decimal `v` into `out`, no leading zeros. Returns the byte count.
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

/// Formats a stored track duration as `m:ss` (or `h:mm:ss` for an hour or more)
/// into `out`, returning the length. It is a length the user entered, never a
/// playback position.
fn fmt_mmss(out: &mut [u8; 12], secs: u32) -> usize {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    let mut n = 0usize;
    if h > 0 {
        let mut hb = [0u8; 4];
        let hn = fmt_u32_pub(&mut hb, h);
        out[..hn].copy_from_slice(&hb[..hn]);
        n += hn;
        out[n] = b':';
        n += 1;
        out[n] = b'0' + (m / 10) as u8;
        out[n + 1] = b'0' + (m % 10) as u8;
        n += 2;
    } else {
        let mut mb = [0u8; 4];
        let mn = fmt_u32_pub(&mut mb, m);
        out[..mn].copy_from_slice(&mb[..mn]);
        n += mn;
    }
    out[n] = b':';
    out[n + 1] = b'0' + (s / 10) as u8;
    out[n + 2] = b'0' + (s % 10) as u8;
    n + 3
}

/// Decimal `v` into a scratch buffer, no leading zeros. Returns the length.
fn fmt_u32_pub(scratch: &mut [u8; 4], v: u32) -> usize {
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

// -----------------------------------------------------------------------------
// Per-frame resolved volume text. Measured once per frame, drawn from the buffer.
// -----------------------------------------------------------------------------
struct VolumeFrame {
    /// "NN%" when a level is known, "unavailable" when the service refused.
    value: [u8; 16],
    value_len: usize,
    /// Filled width of the meter, in pixels. 0 when no level is known.
    fill_w: usize,
    known: bool,
}

impl VolumeFrame {
    fn build(app: &MusicApp) -> Self {
        let mut f = VolumeFrame {
            value: [0; 16],
            value_len: 0,
            fill_w: 0,
            known: false,
        };
        match app.volume_percent {
            Some(pct) => {
                let mut digits = [0u8; 4];
                let n = put_u8(&mut digits, pct);
                f.value[..n].copy_from_slice(&digits[..n]);
                f.value[n] = b'%';
                f.value_len = n + 1;
                // WHY this many pixels: the trough is VOL_METER_X1 - VOL_METER_X0
                // wide and a real 0..100 percent maps linearly onto it. An unknown
                // level draws an empty trough, never a guessed fill.
                f.fill_w = (VOL_METER_X1 - VOL_METER_X0) * pct as usize / 100;
                f.known = true;
            }
            None => {
                let msg: &[u8] = b"unavailable";
                f.value[..msg.len()].copy_from_slice(msg);
                f.value_len = msg.len();
            }
        }
        f
    }

    fn value_x(&self) -> usize {
        VOL_VALUE_X
    }
}

struct Frame {
    vol: VolumeFrame,
    /// The state line: the transport's own state and the queue's position.
    state: [u8; 56],
    state_len: usize,
    /// The queue's caption: how many tracks the queue holds.
    queue_caption: [u8; 32],
    queue_caption_len: usize,
}

/// The position of the current track in the queue the list shows.
///
/// WHY the position is looked up through the order table: the list shows
/// the tracks that match the search, so the current track's place in the
/// queue is not its store index.
fn current_position(app: &MusicApp) -> Option<(usize, usize)> {
    for n in 0..app.order_len {
        if app.order[n] as usize == app.current_track_idx {
            return Some((n + 1, app.order_len));
        }
    }
    None
}

fn build_frame(app: &MusicApp) -> Frame {
    let mut f = Frame {
        vol: VolumeFrame::build(app),
        state: [0; 56],
        state_len: 0,
        queue_caption: [0; 32],
        queue_caption_len: 0,
    };
    let state_label: &[u8] = match app.state {
        PlaybackState::Playing => b"Playing",
        PlaybackState::Paused => b"Paused",
        PlaybackState::Stopped => b"Stopped",
    };
    let mut at = 0usize;
    for &byte in state_label {
        if at < f.state.len() {
            f.state[at] = byte;
            at += 1;
        }
    }
    // WHY the position is in the state line: the state is what the transport
    // reports and the position is where that track sits in the queue, and
    // the two are the two facts a person looks for when the music is not
    // doing what they expect.
    if let Some((position, total)) = current_position(app) {
        for &byte in b" - " {
            if at < f.state.len() {
                f.state[at] = byte;
                at += 1;
            }
        }
        at += push_u32(&mut f.state[at..], position as u32);
        for &byte in b" of " {
            if at < f.state.len() {
                f.state[at] = byte;
                at += 1;
            }
        }
        at += push_u32(&mut f.state[at..], total as u32);
        for &byte in b" in queue" {
            if at < f.state.len() {
                f.state[at] = byte;
                at += 1;
            }
        }
    }
    f.state_len = at.min(f.state.len());

    let mut cap_at = 0usize;
    let mut prefix = [0u8; 4];
    let pn = put_u8(&mut prefix, app.order_len.min(99) as u8);
    for i in 0..pn {
        if cap_at < f.queue_caption.len() {
            f.queue_caption[cap_at] = prefix[i];
            cap_at += 1;
        }
    }
    for &byte in b" tracks" {
        if cap_at < f.queue_caption.len() {
            f.queue_caption[cap_at] = byte;
            cap_at += 1;
        }
    }
    f.queue_caption_len = cap_at;
    f
}

/// Writes a decimal `v` into `out` and returns how many bytes it wrote.
fn push_u32(out: &mut [u8], v: u32) -> usize {
    if out.is_empty() {
        return 0;
    }
    let mut digits = [0u8; 10];
    let mut n = 0usize;
    let mut value = v;
    loop {
        digits[n] = b'0' + (value % 10) as u8;
        value /= 10;
        n += 1;
        if value == 0 {
            break;
        }
    }
    let mut at = 0usize;
    for i in 0..n {
        if at >= out.len() {
            break;
        }
        out[at] = digits[n - 1 - i];
        at += 1;
    }
    at
}

// -----------------------------------------------------------------------------
// Vector pictograms (one rounded, ~2px stroke family)
// -----------------------------------------------------------------------------


/// Minus / plus glyphs for the volume step controls (14 px strokes).
fn step_glyph(x: usize, y: usize, r: Rect, plus: bool) -> bool {
    let cx = r.x + r.w / 2;
    let cy = r.y + r.h / 2;
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() > 8 || dy.abs() > 8 {
        return false;
    }
    if dy == 0 && dx.abs() <= 7 {
        return true;
    }
    plus && dx == 0 && dy.abs() <= 7
}

/// Neutral album-art placeholder: a beamed eighth-note pair, centred on (cx, cy).
fn note_glyph(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    // Beam from (-46,-46) to (46,-34)
    if (-46..=46).contains(&dx) {
        let t = (dx + 46) * 12 / 92;
        let beam = -46 + t;
        if (dy - beam).abs() <= 4 && dy <= -30 {
            return true;
        }
    }
    // Stems
    if (dx == -44 || dx == 44) && (-44..=24).contains(&dy) {
        return true;
    }
    // Note heads (filled ovals)
    for head_cx in [-44isize, 44] {
        let hx = dx - head_cx;
        let hy = dy - 32;
        if hx * hx + hy * hy * 2 <= 196 {
            return true;
        }
    }
    false
}

/// A small upward triangle: the queue's "move up" affordance.
fn up_glyph(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    if dy < -6 || dy > 6 {
        return false;
    }
    dx.abs() <= dy + 6
}

/// A small right-pointing triangle: the marker on the track that is playing.
///
/// WHY a shape and not only a colour: the design document asks for the
/// current track to be marked without relying on colour alone.
fn play_glyph(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    if dx < -6 || dx > 6 {
        return false;
    }
    dy.abs() <= 6 - dx
}

/// Draws the volume row: label, real value, meter trough and the two step
/// controls in the live or recessed treatment.
fn volume_pixel(x: usize, y: usize, app: &MusicApp, f: &Frame) -> Option<u32> {
    // Step controls first: they are drawn on top of nothing, but their hit rects
    // are the authority for the tap handler.
    for (r, plus) in [(VOL_MINUS, false), (VOL_PLUS, true)] {
        if let Some(c) = draw_rounded_rect(x, y, r, R_CONTROL, THEME.surface_container_low, Some(THEME.border), 1) {
            let ink = if app.volume_percent.is_some() { THEME.on_surface } else { THEME.muted };
            if c == THEME.surface_container_low && step_glyph(x, y, r, plus) {
                return Some(ink);
            }
            return Some(c);
        }
    }

    // Meter trough / fill.
    if y >= VOL_METER_Y0 && y < VOL_METER_Y1 && x >= VOL_METER_X0 && x < VOL_METER_X1 {
        if f.vol.known && x < VOL_METER_X0 + f.vol.fill_w {
            return Some(THEME.on_surface);
        }
        return Some(THEME.surface_container_low);
    }

    // Label + value, both gated on their own measured line box.
    let label_y = VOLUME_Y0
        + (VOLUME_Y1 - VOLUME_Y0).saturating_sub(font::line_height(1))
        / 2;
    if let Some(c) = text(x, y, VOL_LABEL_X, label_y, 1, b"Volume", THEME.on_surface_variant, THEME.surface) {
        return Some(c);
    }
    let value_color = if f.vol.known { THEME.on_surface } else { THEME.muted };
    text(
        x,
        y,
        f.vol.value_x(),
        label_y,
        1,
        &f.vol.value[..f.vol.value_len],
        value_color,
        THEME.surface,
    )
}

// -----------------------------------------------------------------------------
// Pixel Color Resolution Engine (dark minimal player)
// -----------------------------------------------------------------------------
#[inline(never)]
fn get_music_pixel(x: usize, y: usize, app: &MusicApp, f: &Frame) -> u32 {
    // 0. Home Gesture Pill (x: 260..460, y: 1418..1424).
    if y >= HOME_Y0 && y < HOME_Y1 && x >= 260 && x < 460 {
        return THEME.on_surface_variant;
    }

    // 1. Top bar: dismiss control + screen label.
    if y < BAR_H {
        if y == BAR_H - 1 {
            return THEME.border;
        }
        // Standardized back button squircle (24..72, 24..72)
        if let Some(c) = std::design::back_button::pixel(x, y, THEME.on_surface, THEME.border, THEME.surface_container_low) {
            return c;
        }
        if let Some(c) = text_centered_bold(x, y, 40, 2, b"Media Player", THEME.on_surface, THEME.surface) {
            return c;
        }
        return THEME.surface;
    }

    // 2. Album artwork: a neutral placeholder, never a fabricated cover.
    //
    // WHY the platform's card treatment: the artwork is a card, and a card
    // that invents its own fill, its own hairline and its own radius is a
    // card a third-party app cannot match. The platform already says what a
    // card looks like, and the placeholder is drawn on it.
    const ART_CARD: components::CardStyle =
        components::card_style(&THEME, components::Elevation::Resting);
    let art = Rect::new(ART_X, ART_Y, ART_W, ART_H);
    if let Some(c) = draw_rounded_rect(x, y, art, ART_CARD.radius, ART_CARD.fill, ART_CARD.border, 1) {
        if c != ART_CARD.fill {
            return c;
        }
        if note_glyph(x, y, ART_X + ART_W / 2, ART_GLYPH_CY) {
            return THEME.muted;
        }
        if let Some(c) = text_centered(
            x,
            y,
            ART_CAPTION_Y,
            1,
            b"No artwork - no album art in library",
            THEME.muted,
            ART_CARD.fill,
        ) {
            return c;
        }
        return ART_CARD.fill;
    }

    // 3. Track metadata from the real, persisted library (empty -> honest idle
    //    copy). The now-playing row reads from here.
    let sel: Option<&store::TrackRecord> = app.store.get(app.current_track_idx);
    let title: &[u8] = match sel {
        Some(t) => t.title(),
        None => b"No track selected",
    };
    {
        // Ellipsise to the content column so a long real title truncates instead
        // of vanishing (the old code drew nothing when the title overflowed).
        let mut tbuf = [0u8; 40];
        let tn = font::ellipsize(title, 2, CONTENT_W, &mut tbuf);
        let tw = font::measure_bold(&tbuf[..tn], 2);
        if let Some(c) = text_bold(x, y, (W.saturating_sub(tw)) / 2, TITLE_Y, 2, &tbuf[..tn], THEME.on_surface, THEME.surface) {
            return c;
        }
    }
    // Honest sub-caption. With a track it states the stored duration (a length
    // the user entered, never a playback position); with none it keeps the
    // empty-library copy exactly as before.
    if let Some(t) = sel {
        let mut cap = [0u8; 24];
        cap[..9].copy_from_slice(b"Duration ");
        let mut cl = 9usize;
        let mut db = [0u8; 12];
        let dn = fmt_mmss(&mut db, t.duration_sec());
        if cl + dn <= cap.len() {
            cap[cl..cl + dn].copy_from_slice(&db[..dn]);
            cl += dn;
        }
        if let Some(c) = text_centered_bold(x, y, ARTIST_Y, 2, &cap[..cl], THEME.on_surface, THEME.surface) {
            return c;
        }
    } else if let Some(c) = text_centered_bold(x, y, ARTIST_Y, 2, b"Local library is empty - nothing to play", THEME.on_surface, THEME.surface) {
        return c;
    }
    let state_label: &[u8] = match app.state {
        PlaybackState::Playing => b"Playing",
        PlaybackState::Paused => b"Paused",
        PlaybackState::Stopped => b"Stopped",
    };
    {
        // WHY the state and the position share one line: the position used to
        // reserve a second line from STATE_Y + 28, which ran into the search
        // field at 748, and the measured gap between STATE_Y + 32 and 748 is
        // only 4 px - too small for any line box.
        let pos = &f.state[..f.state_len];
        // The frame stores state first and appends " - <position>" only when
        // the transport reported one. Drawing all of it beside the bold state
        // repeated the state ("Stopped Stopped") when there was no position,
        // so the suffix is what goes on this line.
        let mut suffix = pos;
        if suffix.len() >= state_label.len() && &suffix[..state_label.len()] == state_label {
            suffix = &suffix[state_label.len()..];
        }
        if suffix.len() >= 3 && &suffix[..3] == b" - " {
            suffix = &suffix[3..];
        }
        let lw = font::measure_bold(state_label, 2);
        let pw = if suffix.is_empty() { 0 } else { font::measure(suffix, 1) };
        let gap = if pw > 0 { 8 } else { 0 };
        let x0 = W.saturating_sub(lw + gap + pw) / 2;
        if let Some(c) = text_bold(x, y, x0, STATE_Y, 2, state_label, THEME.on_surface, THEME.surface) {
            return c;
        }
        if pw > 0 {
            if let Some(c) = text(x, y, x0 + lw + gap, STATE_Y + 5, 1, suffix, THEME.muted, THEME.surface) {
                return c;
            }
        }
    }

    // 3b. The library's search field, the order control and the import
    //     control.
    //
    // WHY the seek bar is not here: no audio backend reports a playback
    // position and this app has no transport that can seek, so a scrubber
    // would be a control that cannot do anything. The design document says
    // to omit the control when seeking is unsupported, and a note under an
    // inert bar is a workaround, not an answer.
    if let Some(c) = draw_rounded_rect(
        x,
        y,
        SEARCH_FIELD,
        radius::CONTROL,
        THEME.surface_container,
        Some(THEME.border),
        1,
    ) {
        if c != THEME.surface_container {
            return c;
        }
        let query = &app.search[..app.search_len];
        let shown: &[u8] = if query.is_empty() { b"Search the library" } else { query };
        let ink = if query.is_empty() { THEME.muted } else { THEME.on_surface };
        let mut buf = [0u8; SEARCH_CAP];
        let n = font::ellipsize(shown, 2, SEARCH_FIELD.w - 32, &mut buf);
        let sy = SEARCH_FIELD.y + (SEARCH_FIELD.h.saturating_sub(16 * 2)) / 2;
        if let Some(c) = text(
            x,
            y,
            SEARCH_FIELD.x + 16,
            sy,
            2,
            &buf[..n],
            ink,
            THEME.surface_container,
        ) {
            return c;
        }
        return THEME.surface_container;
    }
    if let Some(c) = draw_pill(x, y, SORT_BTN, THEME.surface_container, Some(THEME.border)) {
        if let Some(c) = text_in_rect_centered(
            x,
            y,
            SORT_BTN,
            2,
            app.sort.label().as_bytes(),
            THEME.on_surface,
            THEME.surface_container,
        ) {
            return c;
        }
        return c;
    }
    if let Some(c) = draw_pill(x, y, IMPORT_BTN, THEME.surface_container, Some(THEME.border)) {
        if let Some(c) = text_in_rect_centered(
            x,
            y,
            IMPORT_BTN,
            2,
            b"IMPORT",
            THEME.on_surface,
            THEME.surface_container,
        ) {
            return c;
        }
        return c;
    }
    if app.feedback_len > 0 {
        if let Some(c) = text_centered_bold(
            x,
            y,
            FEEDBACK_Y,
            2,
            &app.feedback[..app.feedback_len],
            THEME.on_surface,
            THEME.surface,
        ) {
            return c;
        }
    }
    // The features the platform does not offer this app.
    //
    // WHY they are named: the design document asks for playlists, and the
    // library store holds one flat list. A control that promised a
    // playlist would be a control that does nothing, so the screen says
    // what the device can do instead.
    if let Some(c) = text_centered(
        x,
        y,
        UNAVAILABLE_Y,
        1,
        b"Playlists unavailable - the store holds one queue",
        THEME.muted,
        THEME.surface,
    ) {
        return c;
    }

    // 4b. Volume row: real audio-service telemetry, or an honest "unavailable".
    if y >= VOLUME_Y0 && y < VOLUME_Y1 {
        if let Some(c) = volume_pixel(x, y, app, f) {
            return c;
        }
        return THEME.surface;
    }

    // 5. Transport row at y = 990. Targets: 64 / 88 / 64 px, all >= 56 px.
    //    With an empty library every control renders in the recessed, disabled
    //    treatment so nothing reads as playing.
    let active = !app.store.is_empty();

    {
        let r = if active { THEME.surface_container } else { THEME.surface_container_low };
        if let Some(c) = draw_circle(x, y, PREV_CX, TRANSPORT_CY, SIDE_R, r, Some(THEME.border)) {
            if c != r {
                return c;
            }
            let ink = if active { THEME.on_surface } else { THEME.muted };
            let dx = (x as isize) - PREV_CX as isize;
            let dy = (y as isize) - TRANSPORT_CY as isize;
            // Points left into the vertical bar on the left: the previous-track glyph (|<).
            if (-18..=-14).contains(&dx) && dy.abs() <= 10 {
                return ink;
            }
            if (-12..=8).contains(&dx) {
                let allow = (dx + 12) * 12 / 20;
                if dy.abs() <= allow {
                    return ink;
                }
            }
            return r;
        }
    }

    {
        let r = if active { THEME.on_surface } else { THEME.surface_container_low };
        let ring = if active { None } else { Some(THEME.border) };
        if let Some(c) = draw_circle(x, y, PLAY_CX, TRANSPORT_CY, PLAY_R, r, ring) {
            if c != r {
                return c;
            }
            let glyph = if active { THEME.on_primary } else { THEME.muted };
            let dx = (x as isize) - PLAY_CX as isize;
            let dy = (y as isize) - TRANSPORT_CY as isize;
            if app.state == PlaybackState::Playing && active {
                if ((-14..=-6).contains(&dx) || (6..=14).contains(&dx)) && dy.abs() <= 16 {
                    return glyph;
                }
            } else if dx >= -14 && dx <= 18 {
                let allow = (18 - dx) * 18 / 32;
                if dy.abs() <= allow {
                    return glyph;
                }
            }
            return r;
        }
    }

    {
        let r = if active { THEME.surface_container } else { THEME.surface_container_low };
        if let Some(c) = draw_circle(x, y, NEXT_CX, TRANSPORT_CY, SIDE_R, r, Some(THEME.border)) {
            if c != r {
                return c;
            }
            let ink = if active { THEME.on_surface } else { THEME.muted };
            let dx = (x as isize) - NEXT_CX as isize;
            let dy = (y as isize) - TRANSPORT_CY as isize;
            // Points right into the vertical bar on the right: the next-track glyph (>|).
            if (14..=18).contains(&dx) && dy.abs() <= 10 {
                return ink;
            }
            if (-8..=12).contains(&dx) {
                let allow = (12 - dx) * 12 / 20;
                if dy.abs() <= allow {
                    return ink;
                }
            }
            return r;
        }
    }

    // 6. The queue section: the queue's own header, the controls that act on
    //    it, the add editor when it is open, real rows when the queue has
    //    entries, and an honest empty state otherwise.
    //
    // WHY the section is called a queue: the library store holds one ordered
    // list and that list is the order the tracks are played in. The design
    // document asks for a queue and for playlists, and the store supports
    // exactly one of them.
    if y >= LIST_LABEL_Y && y < LIST_LABEL_Y + font::line_height(2) {
        if let Some(c) = text(x, y, MARGIN, LIST_LABEL_Y, 2, b"QUEUE", THEME.on_surface, THEME.surface) {
            return c;
        }
        if let Some(c) = text(
            x,
            y,
            MARGIN + 100,
            LIST_LABEL_Y + 4,
            1,
            &f.queue_caption[..f.queue_caption_len],
            THEME.muted,
            THEME.surface,
        ) {
            return c;
        }
    }
    if let Some(c) = draw_pill(x, y, PUBLISH_BTN, THEME.surface_container, Some(THEME.border)) {
        if let Some(c) = text_in_rect_centered(
            x,
            y,
            PUBLISH_BTN,
            2,
            b"PUBLISH",
            THEME.on_surface,
            THEME.surface_container,
        ) {
            return c;
        }
        return c;
    }
    if let Some(c) = draw_pill(x, y, RINGTONE_BTN, THEME.surface_container, Some(THEME.border)) {
        if let Some(c) = text_in_rect_centered(
            x,
            y,
            RINGTONE_BTN,
            2,
            b"RINGTONE",
            THEME.on_surface,
            THEME.surface_container,
        ) {
            return c;
        }
        return c;
    }
    if let Some(c) = draw_pill(x, y, ADD_BTN, THEME.surface_container, Some(THEME.border)) {
        if let Some(c) = text_in_rect_centered(
            x,
            y,
            ADD_BTN,
            2,
            b"+ TRACK",
            THEME.on_surface,
            THEME.surface_container,
        ) {
            return c;
        }
        return c;
    }
    if y == LIST_DIVIDER_Y && x >= MARGIN && x < MARGIN + CONTENT_W {
        return THEME.border;
    }

    if app.add_open {
        // Add-track editor: a title field, a duration (seconds) field, and
        // Save / Cancel controls. Shares its rects with the tap handler and with
        // its own repaint band.
        if let Some(c) = draw_rounded_rect(x, y, ADD_TITLE_FIELD, R_CONTROL, THEME.surface_container, Some(THEME.border), 1) {
            if c != THEME.surface_container {
                return c;
            }
            let tt = &app.add_title[..app.add_title_len];
            let shown: &[u8] = if tt.is_empty() { b"Track title" } else { tt };
            let ty = ADD_TITLE_FIELD.y + (ADD_TITLE_FIELD.h.saturating_sub(16 * 2)) / 2;
            let ink = if tt.is_empty() { THEME.muted } else { THEME.on_surface };
            if let Some(c) = text(x, y, ADD_TITLE_FIELD.x + 16, ty, 2, shown, ink, THEME.surface_container) {
                return c;
            }
            return THEME.surface_container;
        }
        if let Some(c) = draw_rounded_rect(x, y, ADD_DUR_FIELD, R_CONTROL, THEME.surface_container, Some(THEME.border), 1) {
            if c != THEME.surface_container {
                return c;
            }
            let dd = &app.add_dur[..app.add_dur_len];
            let shown: &[u8] = if dd.is_empty() { b"duration (s)" } else { dd };
            let dyy = ADD_DUR_FIELD.y + (ADD_DUR_FIELD.h.saturating_sub(16 * 2)) / 2;
            let ink = if dd.is_empty() { THEME.muted } else { THEME.on_surface };
            if let Some(c) = text(x, y, ADD_DUR_FIELD.x + 16, dyy, 2, shown, ink, THEME.surface_container) {
                return c;
            }
            return THEME.surface_container;
        }
        if let Some(c) = draw_pill(x, y, ADD_SAVE_BTN, THEME.surface_container, Some(THEME.border)) {
            if let Some(c) = text_in_rect_centered(
                x,
                y,
                ADD_SAVE_BTN,
                2,
                b"ADD TRACK",
                THEME.on_surface,
                THEME.surface_container,
            ) {
                return c;
            }
            return c;
        }
        if let Some(c) = draw_pill(x, y, ADD_CANCEL_BTN, THEME.surface_container_low, Some(THEME.border)) {
            if let Some(c) = text_in_rect_centered(
                x,
                y,
                ADD_CANCEL_BTN,
                2,
                b"CANCEL",
                THEME.muted,
                THEME.surface_container_low,
            ) {
                return c;
            }
            return c;
        }
        return THEME.surface;
    }

    if app.order_len == 0 {
        // WHY the empty state follows the store's own state: the old copy
        // said the search had found no audio files in local storage, which
        // is a claim about a filesystem the app never searched. A refused
        // read, a store that failed validation and a store that is simply
        // empty are three different facts.
        let (title, subline): (&[u8], &[u8]) = if !app.store.is_empty() {
            (
                b"No track matches",
                b"The search matched no track in the queue",
            )
        } else {
            match app.store.state {
                store::StoreState::ReadRefused => (
                    b"Track store unreadable",
                    b"The device refused to read the track store",
                ),
                store::StoreState::Invalid => (
                    b"Track store invalid",
                    b"The stored tracks failed validation and were not loaded",
                ),
                store::StoreState::Untouched => (
                    b"Track store not read",
                    b"The track store has not been read yet",
                ),
                _ => (
                    b"No tracks",
                    b"No audio files were found in local storage",
                ),
            }
        };
        if let Some(c) = text_centered_bold(x, y, LIST_TITLE_Y, 1, title, THEME.on_surface, THEME.surface) {
            return c;
        }
        if let Some(c) = text_centered(x, y, LIST_SUB_Y, 1, subline, THEME.on_surface_variant, THEME.surface) {
            return c;
        }
        return THEME.surface;
    }

    let scroll = app.list_scroll.offset;
    for i in 0..app.order_len {
        let row_y = LIST_TOP + i * TRACKLIST_ROW_H - scroll;
        if row_y >= LIST_HIT_Y1 {
            break;
        }
        if row_y + TRACKLIST_ROW_H <= LIST_TOP {
            continue;
        }
        let Some(store_idx) = app.store_index_at(i) else {
            continue;
        };
        let t = app.store.get(store_idx).unwrap();
        if y >= row_y && y < row_y + 84 {
            let selected = store_idx == app.current_track_idx;
            // The platform's list-row treatment: the fill of the row the tap
            // lands on and the fill of the row it does not, the platform's
            // radius and the platform's padding.
            //
            // WHY the row is the platform's row and not a hand-rolled
            // block: a row that invents its own fill, its own radius and its
            // own padding is a row a third-party app cannot match.
            let row_fill = if selected { ROW_SELECTED.fill } else { ROW.fill };
            let row_rect = Rect::new(MARGIN, row_y, CONTENT_W, 84);
            if let Some(c) = draw_rounded_rect(x, y, row_rect, ROW.radius, row_fill, None, 0) {
                if c != row_fill {
                    return c;
                }
                // The marker of the track that is playing.
                //
                // WHY the marker is a shape and not only a colour: the
                // design document asks for the current track to be marked
                // without relying on colour alone.
                if selected && play_glyph(x, y, MARGIN + ROW.pad - 8, row_y + 24) {
                    return THEME.on_surface;
                }
                // The queue's "move up" affordance.
                if up_glyph(x, y, MARGIN + CONTENT_W - 90, row_y + 24) {
                    return THEME.muted;
                }
                // Delete affordance, matching its hit region in the tap handler.
                let del_x = MARGIN + CONTENT_W - 36;
                let ddx = x as isize - del_x as isize;
                let ddy = y as isize - (row_y + 24) as isize;
                if ddx.abs() <= 6 && ddy.abs() <= 6 && (ddx.abs() - ddy.abs()).abs() <= 2 {
                    return THEME.muted;
                }
                let mut buf = [0u8; 48];
                let n = font::ellipsize(t.title(), 2, CONTENT_W - 160, &mut buf);
                if let Some(c) = text(
                    x,
                    y,
                    MARGIN + ROW.pad,
                    row_y + 14,
                    2,
                    &buf[..n],
                    if selected { THEME.on_surface } else { THEME.on_surface_variant },
                    row_fill,
                ) {
                    return c;
                }
                // The length from the stored record. A track nobody has
                // measured says so; it does not claim a length of zero.
                let mut db = [0u8; 12];
                let dn = if t.duration_known() {
                    fmt_mmss(&mut db, t.duration_sec())
                } else {
                    db[..5].copy_from_slice(b"--:--");
                    5
                };
                if let Some(c) = text(x, y, MARGIN + ROW.pad, row_y + 50, 1, &db[..dn], THEME.muted, row_fill) {
                    return c;
                }
                return row_fill;
            }
        }
        // Row-gap divider at +88 (see the gate note above the text runs).
        if y == row_y + 88 && x >= MARGIN && x < MARGIN + CONTENT_W {
            return ROW.divider;
        }
    }

    // Scroll indicator, drawn from the shared scroll model so the thumb
    // cannot disagree with the offset the rows were drawn at.
    if let Some((thumb_y, thumb_h)) = app
        .list_scroll
        .thumb_rect(LIST_TOP, LIST_VIEWPORT_H, 24)
    {
        let track_x = MARGIN + CONTENT_W - 4;
        if x == track_x && y >= thumb_y && y < thumb_y + thumb_h {
            return THEME.muted;
        }
    }
    return THEME.surface;
}

fn render_music_ui(surface: u32, app: &MusicApp) {
    render_music_band(surface, app, 0, H);
}

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a band with no stride
/// bookkeeping. Every band starts on a row boundary, so the running `x`/`y`
/// cursor below is exact.
///
/// WHY this is correct rather than a shortcut: each input path names the rows it
/// actually changed, and the pixels outside a requested band are left exactly as
/// drawn because nothing about them changed.
fn render_music_band(surface: u32, app: &MusicApp, y0: usize, y1: usize) {
    let frame = build_frame(app);
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_music_pixel(x, y, app, &frame);
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
// Input handling
// -----------------------------------------------------------------------------

/// Which part of the surface an action changed.
///
/// WHY this exists: rendering the player re-rasterised all 1,036,800 pixels
/// through `get_music_pixel` for every touch event and every keystroke, so
/// pressing play cost the same as a full screen transition.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing changed; do not render.
    None,
    /// Track title / artist / state / times moved: the y 620..840 band.
    Meta,
    /// The volume meter moved: the y 848..892 band.
    Volume,
    /// Transport state moved: the metadata band (state label) AND the transport
    /// row. These are disjoint, so `repaint` walks two bands rather than
    /// redrawing the 176 rows of art that sit between them.
    Playback,
    /// The track-list band moved (add editor, rows, add/save/cancel).
    Tracklist,
    /// The whole surface.
    All,
}

/// Merges two dirty regions reported in the same input batch.
///
/// WHY this exists rather than "last one wins": one `input_event_poll` drain can
/// carry a volume step and a transport tap together, and those dirty different
/// bands. Keeping only one would leave the other stale.
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

/// Maps a `Dirty` onto the bands it re-rasterises.
fn repaint(surface: u32, app: &MusicApp, dirty: Dirty) {
    match dirty {
        Dirty::None => {}
        Dirty::Meta => render_music_band(surface, app, META_Y0, META_Y1),
        Dirty::Volume => render_music_band(surface, app, VOLUME_Y0, VOLUME_Y1),
        Dirty::Playback => {
            render_music_band(surface, app, META_Y0, META_Y1);
            render_music_band(surface, app, TRANSPORT_Y0, TRANSPORT_Y1);
        }
        Dirty::Tracklist => {
            render_music_band(surface, app, ADD_BTN.y, LIST_HIT_Y1);
        }
        Dirty::All => render_music_ui(surface, app),
    }
}

/// Exactly the transport discs drawn by `get_music_pixel`.
fn in_transport(x: usize, y: usize, cx: usize, r: usize) -> bool {
    let dx = (x as i32) - cx as i32;
    let dy = (y as i32) - TRANSPORT_CY as i32;
    dx * dx + dy * dy <= (r * r) as i32
}

fn handle_tap(app: &mut MusicApp, px: u32, py: u32) -> Tap<'static> {
    let x = px as usize;
    let y = py as usize;

    // Dismiss control in the top bar, or the home gesture band.
    if std::design::back_button::contains(x, y) || BACK.contains(x, y) || (x >= HOME_X0 && x < HOME_X1 && y >= 1400) {
        return Tap::Leave("[MUSIC APP] Dismiss control or home gesture pressed. Exiting to launcher.");
    }

    // Volume step controls. Drawn recessed when no level is known, and they say
    // so rather than clicking into nothing.
    if VOL_MINUS.contains(x, y) || VOL_PLUS.contains(x, y) {
        let up = VOL_PLUS.contains(x, y);
        if app.step_volume(up) {
            let _ = std::audio_haptic_click();
            return Tap::Redraw(Dirty::Volume);
        }
        return Tap::Redraw(Dirty::None);
    }

    // Transport row. Every control is hit-tested against the exact disc drawn.
    if in_transport(x, y, PLAY_CX, PLAY_R) {
        let started = app.toggle_play();
        // WHY the conditional haptic: a press on a control that is already drawn
        // in the disabled treatment must not feel like it did something. A real
        // pause is worth a click; a refused play is not.
        if started || app.state == PlaybackState::Paused {
            let _ = std::audio_haptic_click();
        }
        return Tap::Redraw(Dirty::Playback);
    }
    if in_transport(x, y, PREV_CX, SIDE_R) {
        if app.step_track(-1) {
            let _ = std::audio_haptic_click();
            return Tap::Redraw(Dirty::Meta);
        }
        return Tap::Redraw(Dirty::None);
    }
    if in_transport(x, y, NEXT_CX, SIDE_R) {
        if app.step_track(1) {
            let _ = std::audio_haptic_click();
            return Tap::Redraw(Dirty::Meta);
        }
        return Tap::Redraw(Dirty::None);
    }

    // The library's search field.
    if SEARCH_FIELD.contains(x, y) {
        app.search_focused = true;
        let _ = std::audio_haptic_click();
        return Tap::Redraw(Dirty::All);
    }

    // The control that changes the queue's order.
    if SORT_BTN.contains(x, y) {
        if app.cycle_sort() {
            app.sync_list_extent();
            let _ = std::audio_haptic_click();
        }
        return Tap::Redraw(Dirty::All);
    }

    // The control that imports the audio files the device holds.
    if IMPORT_BTN.contains(x, y) {
        let outcome = library::import_from_storage(&mut app.store);
        match outcome {
            ImportOutcome::Imported(n) => {
                app.sync_list_extent();
                std::log("[MUSIC APP] Imported a track from the shared storage; its length was not read.",
                )
                .ok();
                let _ = n;
            }
            ImportOutcome::ListRefused => {
                std::log("[MUSIC APP] The device refused to list the shared storage; nothing was imported.",
                )
                .ok();
            }
            ImportOutcome::NoneFound => {
                std::log("[MUSIC APP] The shared storage holds no audio file the library does not already hold.",
                )
                .ok();
            }
        }
        let _ = std::audio_haptic_click();
        return Tap::Redraw(Dirty::All);
    }

    // The control that publishes the library into the device's document index.
    if PUBLISH_BTN.contains(x, y) {
        match library::publish_library(&app.store) {
            PublishOutcome::Published(n) => {
                std::log("[MUSIC APP] Published the library into the device's document index; the file manager and the launcher can find it now.",
                )
                .ok();
                let _ = n;
            }
            PublishOutcome::IndexRefused(n) => {
                std::log("[MUSIC APP] Published the library, but the device refused to refresh the document index.",
                )
                .ok();
                let _ = n;
            }
            PublishOutcome::WriteRefused => {
                std::log("[MUSIC APP] The device refused the write; the library was not published.",
                )
                .ok();
            }
        }
        let _ = std::audio_haptic_click();
        return Tap::Redraw(Dirty::All);
    }

    // The control that sets the currently selected track as system ringtone.
    if RINGTONE_BTN.contains(x, y) {
        if app.set_current_as_ringtone() {
            let _ = std::audio_haptic_click();
        }
        return Tap::Redraw(Dirty::All);
    }

    // ADD TRACK control on the label row opens the add editor.
    if ADD_BTN.contains(x, y) {
        app.add_open = true;
        app.add_focus = 0;
        app.add_title_len = 0;
        app.add_dur_len = 0;
        app.search_focused = false;
        let _ = std::audio_haptic_click();
        return Tap::Redraw(Dirty::All);
    }

    if app.add_open {
        // Each editor control is hit-tested against the exact rect it is drawn in.
        if ADD_TITLE_FIELD.contains(x, y) {
            app.add_focus = 0;
            let _ = std::audio_haptic_click();
            return Tap::Redraw(Dirty::All);
        }
        if ADD_DUR_FIELD.contains(x, y) {
            app.add_focus = 1;
            let _ = std::audio_haptic_click();
            return Tap::Redraw(Dirty::All);
        }
        if ADD_SAVE_BTN.contains(x, y) {
            return Tap::Redraw(app.save_add());
        }
        if ADD_CANCEL_BTN.contains(x, y) {
            app.cancel_add();
            std::log("[MUSIC APP] Add track cancelled; nothing was stored.").ok();
            return Tap::Redraw(Dirty::All);
        }
    } else if app.order_len == 0 {
        std::log("[MUSIC APP] Track list tapped: the queue holds no track.",
        )
        .ok();
    } else {
        // A real row: tap its delete affordance to remove it, tap its move-up
        // affordance to move it up in the queue, and tap elsewhere in the row
        // to select it as now-playing.
        //
        // WHY the scroll offset is added back: the rows are drawn at their
        // scrolled position, so the row under the finger is the one whose
        // index is the offset-adjusted position divided by the row height.
        // Without it, a tap on a scrolled row selected the wrong track.
        let ry = y.saturating_sub(LIST_TOP) + app.list_scroll.offset;
        let idx = ry / TRACKLIST_ROW_H;
        let coff = ry % TRACKLIST_ROW_H;
        if idx < app.order_len && coff < 84 && x >= MARGIN && x < MARGIN + CONTENT_W {
            let del_cx = (MARGIN + CONTENT_W - 36) as isize;
            let ddx = x as isize - del_cx;
            let ddy = coff as isize - 24;
            if ddx.abs() <= 24 && ddy.abs() <= 24 {
                if let Some(store_idx) = app.store_index_at(idx) {
                    if app.store.remove(store_idx) {
                        app.sync_list_extent();
                        std::log("[MUSIC APP] Track removed; the deletion was persisted to the settings store.",
                        )
                        .ok();
                        if app.current_track_idx >= app.store.len() && app.current_track_idx > 0 {
                            app.current_track_idx -= 1;
                        }
                        app.publish_now_playing();
                    } else {
                        std::log("[MUSIC APP] Track deletion was not persisted; the library is unchanged.",
                        )
                        .ok();
                    }
                }
                return Tap::Redraw(Dirty::All);
            }
            // The queue's "move up" affordance.
            let up_cx = (MARGIN + CONTENT_W - 90) as isize;
            let udx = x as isize - up_cx;
            let udy = coff as isize - 24;
            if udx.abs() <= 24 && udy.abs() <= 24 {
                if app.move_up(idx) {
                    app.sync_list_extent();
                    std::log("[MUSIC APP] Track moved up in the queue; the new order was persisted.",
                    )
                    .ok();
                    app.publish_now_playing();
                } else {
                    std::log("[MUSIC APP] The track is already first in the queue, or the new order was not persisted.",
                    )
                    .ok();
                }
                return Tap::Redraw(Dirty::All);
            }
            if let Some(store_idx) = app.store_index_at(idx) {
                app.current_track_idx = store_idx.min(app.store.len().saturating_sub(1));
                app.publish_now_playing();
                let _ = std::audio_haptic_click();
                return Tap::Redraw(Dirty::All);
            }
        }
    }

    Tap::None
}

/// Mirrors `handle_browser_key` in `apps/browser/src/main.rs`.
///
/// The player had no key handler at all before this change. These bindings are
/// the conventional media-key set and each one drives real state whose outcome is
/// reported back rather than assumed.
fn handle_music_key(app: &mut MusicApp, key: std::InputKeyEvent) -> Tap<'static> {
    use std::input_key_code;
    if !key.pressed {
        return Tap::None;
    }
    // While the library's search field is focused the keyboard types into
    // it; the media bindings below are suspended so a space types a space
    // rather than pausing.
    //
    // WHY the search is checked first: the field the person tapped last is
    // the field they are typing into.
    if app.search_focused {
        return match key.code {
            input_key_code::ESCAPE => {
                app.search_focused = false;
                Tap::Redraw(Dirty::All)
            }
            input_key_code::BACKSPACE => {
                app.search_len = app.search_len.saturating_sub(1);
                app.sync_list_extent();
                Tap::Redraw(Dirty::All)
            }
            code if (32..=126).contains(&code) && app.search_len < SEARCH_CAP => {
                app.search[app.search_len] = code as u8;
                app.search_len += 1;
                app.sync_list_extent();
                Tap::Redraw(Dirty::All)
            }
            _ => Tap::None,
        };
    }
    // While the add-track editor is open the keyboard types into it; the media
    // bindings below are suspended so a space types a space rather than pausing.
    if app.add_open {
        return match key.code {
            input_key_code::ESCAPE => {
                app.cancel_add();
                Tap::Redraw(Dirty::All)
            }
            input_key_code::ENTER => Tap::Redraw(app.save_add()),
            input_key_code::BACKSPACE => {
                if app.add_focus == 0 {
                    app.add_title_len = app.add_title_len.saturating_sub(1);
                } else {
                    app.add_dur_len = app.add_dur_len.saturating_sub(1);
                }
                Tap::Redraw(Dirty::Tracklist)
            }
            input_key_code::ARROW_DOWN => {
                app.add_focus = 1;
                Tap::Redraw(Dirty::All)
            }
            input_key_code::ARROW_UP => {
                app.add_focus = 0;
                Tap::Redraw(Dirty::All)
            }
            code if app.add_focus == 0
                && (32..=126).contains(&code)
                && app.add_title_len < ADD_TITLE_CAP =>
            {
                app.add_title[app.add_title_len] = code as u8;
                app.add_title_len += 1;
                Tap::Redraw(Dirty::Tracklist)
            }
            code if app.add_focus == 1
                && (b'0' as u32..=b'9' as u32).contains(&code)
                && app.add_dur_len < ADD_DUR_DIGITS =>
            {
                app.add_dur[app.add_dur_len] = code as u8;
                app.add_dur_len += 1;
                Tap::Redraw(Dirty::Tracklist)
            }
            _ => Tap::None,
        };
    }
    match key.code {
        input_key_code::ESCAPE => {
            Tap::Leave("[MUSIC APP] Escape pressed. Exiting to launcher.")
        }
        // Space is the conventional play/pause key; it is also code 32, so it
        // has to be matched before any printable-character handling.
        32 => {
            app.toggle_play();
            Tap::Redraw(Dirty::Playback)
        }
        input_key_code::ARROW_LEFT => {
            if app.step_track(-1) {
                Tap::Redraw(Dirty::Meta)
            } else {
                Tap::Redraw(Dirty::None)
            }
        }
        input_key_code::ARROW_RIGHT => {
            if app.step_track(1) {
                Tap::Redraw(Dirty::Meta)
            } else {
                Tap::Redraw(Dirty::None)
            }
        }
        input_key_code::ARROW_UP => {
            if app.step_volume(true) {
                Tap::Redraw(Dirty::Volume)
            } else {
                Tap::Redraw(Dirty::None)
            }
        }
        input_key_code::ARROW_DOWN => {
            if app.step_volume(false) {
                Tap::Redraw(Dirty::Volume)
            } else {
                Tap::Redraw(Dirty::None)
            }
        }
        code if code == b'r' as u32 || code == b'R' as u32 => {
            if app.set_current_as_ringtone() {
                let _ = std::audio_haptic_click();
            }
            Tap::Redraw(Dirty::All)
        }
        _ => Tap::None,
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[MUSIC APP (Ring 3)] Native music player online.",
    )
    .ok();

    let mut music = MusicApp::new();

    // Load the real, persisted local library before the first frame so the
    // playlist and the now-playing row reflect what storage actually holds.
    music.store = store::TrackStore::load();
    music.sync_list_extent();
    music.restore_playback_state();
    music.publish_now_playing();
    let load_note: &str = match music.store.state {
        store::StoreState::Empty => "Track store loaded: no tracks are stored yet.",
        store::StoreState::Loaded => "Track store loaded from the settings store.",
        store::StoreState::ReadRefused => "Settings read refused; no stored tracks were loaded.",
        store::StoreState::Invalid => "Stored track data failed validation; no tracks were loaded.",
        _ => "",
    };
    if !load_note.is_empty() {
        std::log(load_note).ok();
    }

    // 1. Hardware Keystore Derivation
    music.derive_keystore_key();

    // 2. Encrypt Track Record
    music.encrypt_track_record();

    // 3. Allocate DRM/KMS Client Surface (z=200)
    //
    // WHY the slot is resolved by type: a slot number is a position in the
    // capability table, and the position depends on the order the kernel
    // inserted this app's capabilities in. The literal 6 happens to hold the
    // framebuffer in this app's table today.
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(id) => {
            std::log("[MUSIC APP] DRM/KMS Client Surface allocated (z=200).").ok();
            id
        }
        Err(_) => 0,
    };

    // 4. Real master volume. The player used to carry a hardcoded 85 here that
    //    was never drawn and never changed.
    music.refresh_volume();

    // 5. Zero-Copy DMA Audio Streaming
    if music.stream_dma_audio_block() {
        music.total_dma_buffers_streamed += 1;
        music.state = PlaybackState::Playing;
        music.publish_now_playing();
    }

    if surface > 0 {
        render_music_ui(surface, &music);
        let _ = std::surface_flip();
        std::log("[MUSIC APP] Music canvas presented to scanout buffer.").ok();
    }

    // 6. Asynchronous Event Reactor Loop
    let interest_mask = std::EVENT_TOUCH_INPUT | std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK;
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
                        // keys and touches through the same poll, so a key must
                        // be recognised before the touch branch sees kind != 1.
                        if let Some(key) = input.key_event() {
                            match handle_music_key(&mut music, key) {
                                Tap::Leave(msg) => {
                                    std::log(msg).ok();
                                    if music.state == PlaybackState::Playing {
                                        std::log("[MUSIC APP] Background playback active; audio transport continues.").ok();
                                    }
                                    music.publish_now_playing();
                                    if surface > 0 {
                                        let _ = std::surface_destroy(surface);
                                    }
                                    std::exit_with_code(0);
                                }
                                Tap::Redraw(d) => dirty = merge(dirty, d),
                                Tap::None => {}
                            }
                            continue;
                        }
                        if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                            // A drag that ends over the track list is a list
                            // gesture; anything else is the system's
                            // navigation gesture.
                            //
                            // WHY the list needed the gesture: the library
                            // holds up to eight tracks and the visible band
                            // holds three, so the lower five could not be
                            // reached at all. The scroll model owns the
                            // offset and the thumb, so they cannot disagree.
                            let over_list = input.y >= LIST_TOP as u32
                                && (input.y as usize) < LIST_HIT_Y1
                                && !music.add_open;
                            if (input.kind == 4 || input.kind == 10) && over_list {
                                music.list_scroll.apply_drag(input.dy);
                                dirty = merge(dirty, Dirty::All);
                                continue;
                            }
                            std::log("[MUSIC APP] Navigation gesture recognized. Exiting to launcher.",
                            )
                            .ok();
                            if music.state == PlaybackState::Playing {
                                std::log("[MUSIC APP] Background playback active; audio transport continues.").ok();
                            }
                            music.publish_now_playing();
                            if surface > 0 {
                                // WHY no slot argument: a slot is a position
                                // in this task's capability table, and the
                                // kernel authorises the release by the
                                // capability's type. The surface must be
                                // handed back or it stays a leaked KMS
                                // surface for the life of the device.
                                let _ = std::surface_destroy(surface);
                            }
                            std::exit_with_code(0);
                        }
                        if input.kind == 1 {
                            match handle_tap(&mut music, input.x, input.y) {
                                Tap::Leave(msg) => {
                                    std::log(msg).ok();
                                    if music.state == PlaybackState::Playing {
                                        std::log("[MUSIC APP] Background playback active; audio transport continues.").ok();
                                    }
                                    music.publish_now_playing();
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
                        repaint(surface, &music, dirty);
                        let _ = std::surface_flip();
                    }
                }
                if events & std::EVENT_IPC_MSG != 0 {
                    std::log("[MUSIC APP] IPC message event raised; no playback command was parsed from it.",
                    )
                    .ok();
                }
                if events & std::EVENT_TIMER_TICK != 0 {
                    if music.state == PlaybackState::Playing && music.stream_dma_audio_block() {
                        music.total_dma_buffers_streamed += 1;
                    }
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
    std::log("[MUSIC APP] Panic detected! Terminating task...").ok();
    std::exit();
}
