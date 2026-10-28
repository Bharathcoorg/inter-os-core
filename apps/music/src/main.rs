//! InterOS Core Native Music Player (Ring-3)
//!
//! Bare-metal user-mode media player on the canonical dark media surface:
//! - Playlist populated only from real media storage (`PLAYLIST` is honestly
//!   empty, so the player reports an idle transport and an empty track list)
//! - Every transport target is >= 56 px (previous/next 64 px, play/pause 88 px)
//! - The transport feeds the platform's AC'97 PCM stream; the elapsed position
//!   is the frame count the device reports, and no scrubber is drawn because
//!   the transport cannot seek
//! - Album artwork is a neutral placeholder glyph, never a fabricated cover
//! - All strings are measured with `interos_std::design::font::measure` /
//!   `ellipsize` at the coordinates they are drawn at
//! - Hardware Keystore Slot 1 media key derivation and the track record's
//!   authenticated form are preserved; the keystore refuses derivation to this
//!   app and the log says so rather than claiming a key
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
use music::playback::{PlaybackError, Player, PlayerState};
use store::TrackStore;
use music::library;
use music::SOURCE_MAX;
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
/// Preallocated static buffer for genuine embedded album art (raw JPEG/PNG bytes up to 32 KiB).
static mut ALBUM_ART_RAW: [u8; music::art::MAX_ART_BYTES] = [0; music::art::MAX_ART_BYTES];
/// Preallocated static buffer for decoded RGBA cover image pixels.
static mut ALBUM_ART_RGBA: [u8; 524_288] = [0; 524_288];
/// Temporary scratch buffer for scanning media file header/metadata.
static mut ART_SCAN_SCRATCH: [u8; 65536] = [0; 65536];


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
    /// What the platform's service registry last said about the audio service.
    ///
    /// WHY it is cached here: the registry is a syscall and the renderer runs
    /// once per pixel, so the answer is fetched on the timer and read from
    /// here when the surface state is derived.
    pub audio_service: music::ServiceReport,
    /// The engine that reads the selected track's file and feeds the device's
    /// PCM stream.
    pub player: Player,
    /// The whole second of playback the screen last drew.
    ///
    /// WHY the whole second: the position is a fact that changes every tick,
    /// and repainting the metadata band for a value the screen cannot show
    /// would spend the frame on nothing.
    pub shown_second: u64,
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
    /// The token of the last dispatched intent this app handled.
    pub last_intent_token: u64,
    /// Genuine embedded album art state.
    pub has_cover_art: bool,
    pub cover_art_w: u32,
    pub cover_art_h: u32,
    pub cover_art_len: usize,
    pub loop_queue: bool,
}

impl MusicApp {
    pub const fn new() -> Self {
        Self {
            state: PlaybackState::Stopped,
            current_track_idx: 0,
            waveform: AudioWaveform::Sine,
            volume_percent: None,
            audio_service: music::ServiceReport::Unknown,
            player: Player::EMPTY,
            shown_second: 0,
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
            last_intent_token: 0,
            has_cover_art: false,
            cover_art_w: 0,
            cover_art_h: 0,
            cover_art_len: 0,
            loop_queue: true,
        }
    }

    /// Opens the audio file a caller dispatched to this player.
    ///
    /// WHY the import runs first: the dispatched payload is a document in the
    /// shared storage area, and the library holds tracks rather than paths.
    /// Importing is this app's own way of bringing a document in, so a
    /// dispatched file and an imported one end up as the same kind of track.

    /// Extracts and decodes genuine embedded album art from the selected track.
    pub fn reload_current_art(&mut self) {
        self.has_cover_art = false;
        self.cover_art_w = 0;
        self.cover_art_h = 0;
        self.cover_art_len = 0;

        if self.store.is_empty() {
            return;
        }
        let mut path = [0u8; SOURCE_MAX];
        let Some(path_len) = library::resolve_source(&self.store, self.current_track_idx, &mut path) else {
            return;
        };
        let Ok(path_str) = core::str::from_utf8(&path[..path_len]) else {
            return;
        };
        let Ok(fd) = std::fs_open(path_str, std::FS_READ) else {
            return;
        };

        let scan_buf = unsafe { &mut *core::ptr::addr_of_mut!(ART_SCAN_SCRATCH) };
        let mut total_read = 0;
        while total_read < scan_buf.len() {
            match std::fs_read(fd, &mut scan_buf[total_read..]) {
                Ok(0) => break,
                Ok(n) => total_read += n,
                Err(_) => break,
            }
        }
        let _ = std::fs_close(fd);

        if total_read == 0 {
            return;
        }

        let raw_art = unsafe { &mut *core::ptr::addr_of_mut!(ALBUM_ART_RAW) };
        if let Ok(art_len) = music::art::extract_album_art(&scan_buf[..total_read], raw_art) {
            let rgba_buf = unsafe { &mut *core::ptr::addr_of_mut!(ALBUM_ART_RGBA) };
            let name = if music::art::is_png(&raw_art[..art_len]) {
                b"cover.png"
            } else {
                b"cover.jpg"
            };
            if let Ok((w, h, len)) = std::image::decode_image(&raw_art[..art_len], name, rgba_buf) {
                if len == (w as usize) * (h as usize) * 4 {
                    self.has_cover_art = true;
                    self.cover_art_w = w;
                    self.cover_art_h = h;
                    self.cover_art_len = len;
                    std::log("[MUSIC APP] Album cover art extracted and decoded.").ok();
                }
            }
        }
    }
    pub fn handle_pending_intent(&mut self) -> bool {
        let mut buf = [0u8; 320];
        let n = match std::content::intent_pending(&mut buf) {
            Ok(n) => n,
            Err(_) => return false,
        };
        if n == 0 {
            return false;
        }
        let Some(pending) = std::content::parse_pending(&buf[..n]) else {
            return false;
        };
        // WHY the token is remembered: the pending slot is peeked, not taken,
        // so it stays readable until another dispatch replaces it. Without
        // this the player would re-import and re-select on every pass.
        if pending.token == self.last_intent_token {
            return false;
        }
        self.last_intent_token = pending.token;
        if pending.kind != std::content::IntentKind::Mime {
            return false;
        }
        let payload = pending.payload;
        let name = payload
            .as_bytes()
            .rsplit(|b| *b == b'/')
            .next()
            .unwrap_or(b"");
        if name.is_empty() || !library::is_audio_name(name) {
            std::log("[MUSIC APP] A dispatched document is not audio this player can list.").ok();
            return false;
        }
        match library::import_from_storage(&mut self.store) {
            library::ImportOutcome::Imported(_) | library::ImportOutcome::PartiallyImported(_, _) => {}
            library::ImportOutcome::WriteRefused(_) => {
                std::log("[MUSIC APP] The dispatched audio file was found, but the settings write was refused, so it could not be added to the library.").ok();
                return false;
            }
            library::ImportOutcome::ListRefused => {
                std::log("[MUSIC APP] The device refused to list the shared storage, so the dispatched audio file could not be read.").ok();
                return false;
            }
            library::ImportOutcome::NoneFound => {
                std::log("[MUSIC APP] The dispatched audio file was not listed in the shared storage.").ok();
                return false;
            }
        }
        self.sync_list_extent();
        let mut title = [0u8; music::TITLE_MAX];
        let title_len = library::title_from_name(name, &mut title);
        for i in 0..self.store.len() {
            if let Some(track) = self.store.get(i) {
                if track.title() == &title[..title_len] {
                    self.current_track_idx = i;
                    self.reload_current_art();
                    self.clear_feedback();
                    if self.state == PlaybackState::Playing {
                        self.start_selected();
                    }
                    self.publish_now_playing();
                    std::log("[MUSIC APP] Opened the audio file a caller dispatched to this player.").ok();
                    return true;
                }
            }
        }
        std::log("[MUSIC APP] The dispatched audio file was not listed in the shared storage.").ok();
        false
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
        self.move_visible(n, -1)
    }

    /// Moves the `n`th track the list shows one place down in the queue.
    pub fn move_down(&mut self, n: usize) -> bool {
        self.move_visible(n, 1)
    }

    /// Moves a visible row to its neighbour's place in the queue.
    ///
    /// WHY the move targets the neighbouring *visible* row and not the
    /// neighbouring store index: a search hides tracks, and swapping with a
    /// hidden track would persist an order the person cannot see change.
    fn move_visible(&mut self, n: usize, delta: isize) -> bool {
        let Some(from) = self.store_index_at(n) else {
            return false;
        };
        let target_n = if delta < 0 {
            if n == 0 {
                return false;
            }
            n - 1
        } else {
            if n + 1 >= self.order_len {
                return false;
            }
            n + 1
        };
        let Some(to) = self.store_index_at(target_n) else {
            return false;
        };
        let current_id = self.store.get(self.current_track_idx).map(|t| t.id);
        if !self.store.move_to(from, to) {
            return false;
        }
        self.relink_current(current_id);
        self.rebuild_order();
        true
    }

    /// Removes the `n`th track the list shows and keeps the selection honest.
    ///
    /// WHY the current track is stopped only when it was the one removed: the
    /// stream is named by a store index, and deleting a different track shifts
    /// that index. Retracking the player keeps the same file playing under its
    /// new position; deleting the playing track has no file left to keep.
    pub fn remove_at(&mut self, n: usize) -> bool {
        let Some(store_idx) = self.store_index_at(n) else {
            return false;
        };
        let current_id = self.store.get(self.current_track_idx).map(|t| t.id);
        let removed_id = self.store.get(store_idx).map(|t| t.id);
        if !self.store.remove(store_idx) {
            return false;
        }
        let removed_current = removed_id.is_some() && removed_id == current_id;
        if removed_current {
            self.player.stop();
            self.state = PlaybackState::Stopped;
            self.current_track_idx = if self.store.is_empty() {
                0
            } else {
                store_idx.min(self.store.len() - 1)
            };
        } else {
            self.relink_current(current_id);
        }
        self.sync_list_extent();
        self.publish_now_playing();
        true
    }

    /// Keeps the selection on the same track after the queue was reordered.
    ///
    /// WHY the id: the selection is a position, and a reorder moves tracks
    /// between positions. The id travels with the track, so it is what says
    /// which position holds the track that was selected.
    fn relink_current(&mut self, id: Option<u32>) {
        let Some(id) = id else {
            return;
        };
        for i in 0..self.store.len() {
            if self.store.get(i).map(|t| t.id) == Some(id) {
                self.current_track_idx = i;
                self.player.retrack(i);
                return;
            }
        }
    }

    /// Cycles the queue's order and persists the new one.
    ///
    /// WHY the order is persisted: the queue is the order the tracks are
    /// played in, and an order that is forgotten when the app closes is not
    /// an order.
    pub fn cycle_sort(&mut self) -> bool {
        let next = self.sort.next();
        let current_id = self.store.get(self.current_track_idx).map(|t| t.id);
        let ok = match next {
            SortMode::Added => self.store.sort_by_added(),
            SortMode::Title => self.store.sort_by_title(),
            SortMode::Duration => self.store.sort_by_duration(),
        };
        if ok {
            self.sort = next;
            self.relink_current(current_id);
            self.rebuild_order();
            self.clear_feedback();
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
                self.player.set_volume(v as u8);
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
                self.player.set_volume(v as u8);
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

    /// Reads the platform's answer about the audio service's lifecycle.
    ///
    /// WHY it is fetched here and not in the renderer: the service registry is
    /// a syscall and the renderer runs once per pixel, so the answer is read
    /// on the timer and cached.
    pub fn refresh_audio_service(&mut self) {
        let mut record = [0u8; 12];
        self.audio_service = match std::service_query(music::AUDIO_SERVICE_ID, &mut record) {
            Ok(n) if n >= 2 => music::service_report(record[1]),
            _ => music::ServiceReport::Unknown,
        };
    }

    /// What the platform's audio path reported about the current stream.
    ///
    /// WHY the refusal and the finished track come first: a stream the
    /// platform refused can never be playing, and a player that closed its
    /// stream has already read the status that said the track was over. The
    /// status the kernel keeps for a closed stream is the last one the
    /// device gave, so it cannot answer for a track that was never opened.
    pub fn stream_report(&self) -> music::StreamReport {
        match self.player.error {
            PlaybackError::StreamUnavailable | PlaybackError::WriteRefused => {
                return music::StreamReport::Absent;
            }
            _ => {}
        }
        if self.player.state == PlayerState::Ended {
            return music::StreamReport::Ended;
        }
        match self.player.status {
            Some(status) => music::report_from_status(&status),
            None => music::StreamReport::Unknown,
        }
    }

    /// The state the now-playing surface may claim.
    ///
    /// WHY it is derived from the platform and not from this app's own state:
    /// the app's state says what the transport *meant* to do, and the screen
    /// used to show that intent. The device's own status says what the
    /// hardware is doing, so the screen shows that.
    pub fn surface_state(&self) -> music::SurfaceState {
        let intent = match self.state {
            PlaybackState::Playing => music::TransportIntent::Playing,
            PlaybackState::Paused => music::TransportIntent::Paused,
            PlaybackState::Stopped => music::TransportIntent::Stopped,
        };
        music::surface_state(intent, self.stream_report(), self.audio_service)
    }

    /// Starts the selected track on the device's PCM output.
    ///
    /// Returns `true` only when the device accepted the stream. A refusal is
    /// reported on the feedback line; the transport never claims a track is
    /// playing when no samples reached the device.
    pub fn start_selected(&mut self) -> bool {
        if self.store.is_empty() {
            self.state = PlaybackState::Stopped;
            self.set_feedback(b"Play refused: the library is empty");
            std::log("[MUSIC APP] Play requested but the library is empty; transport stays stopped.",
            )
            .ok();
            return false;
        }
        if self.player.start(&self.store, self.current_track_idx) {
            self.state = PlaybackState::Playing;
            // A track that starts playing must not leave the refusal of an
            // earlier, unplayable entry on the screen: the line names a failure
            // that is no longer the player's state.
            self.clear_feedback();
            if let Some(volume) = self.volume_percent {
                self.player.set_volume(volume);
            }
            self.reload_current_art();
            self.shown_second = u64::MAX;
            // WHY the log names the platform's answer and not the intent: the
            // stream was accepted, but the device may already have finished
            // the last batch it was given. Logging "playing" here is what
            // put "Playing" on the screen while the device had stopped.
            match self.surface_state() {
                music::SurfaceState::Playing => {
                    std::log("[MUSIC APP] Transport playing over the device's PCM output.",
                    )
                    .ok();
                }
                music::SurfaceState::Ended => {
                    std::log("[MUSIC APP] Transport opened; the device finished the track before the screen could show it playing.",
                    )
                    .ok();
                }
                music::SurfaceState::NotRunning => {
                    std::log("[MUSIC APP] Transport opened; the device is not running the stream.").ok();
                }
                music::SurfaceState::Unavailable => {
                    std::log("[MUSIC APP] Transport opened; the platform will not confirm any output.").ok();
                }
                _ => {
                    std::log("[MUSIC APP] Transport opened.").ok();
                }
            }
            true
        } else {
            self.state = PlaybackState::Stopped;
            let reason = self.player.error_reason();
            let mut msg = [0u8; FEEDBACK_CAP];
            let prefix = b"Cannot play: ";
            msg[..prefix.len()].copy_from_slice(prefix);
            let n = reason.len().min(FEEDBACK_CAP - prefix.len());
            msg[prefix.len()..prefix.len() + n].copy_from_slice(&reason.as_bytes()[..n]);
            self.set_feedback(&msg[..prefix.len() + n]);
            std::log("[MUSIC APP] Playback refused; the transport stays stopped.").ok();
            false
        }
    }

    /// Encrypts metadata only when a real media record exists.
    pub fn encrypt_track_record(&mut self) {
        std::log("[MUSIC APP] No media record is available for encryption.").ok();
    }

    /// Seeks to a position in milliseconds within the current track.
    pub fn seek(&mut self, target_ms: u64) -> bool {
        if self.store.is_empty() {
            return false;
        }
        let ok = self.player.seek(&self.store, target_ms);
        if ok {
            self.state = match self.player.state {
                PlayerState::Playing => PlaybackState::Playing,
                PlayerState::Paused => PlaybackState::Paused,
                _ => PlaybackState::Stopped,
            };
            self.shown_second = u64::MAX;
            std::log("[MUSIC APP] Repositioned playback timeline.").ok();
        }
        ok
    }

    /// Toggles queue looping behavior.
    pub fn toggle_loop_queue(&mut self) -> bool {
        self.loop_queue = !self.loop_queue;
        if self.loop_queue {
            std::log("[MUSIC APP] Queue looping enabled.").ok();
        } else {
            std::log("[MUSIC APP] Queue looping disabled.").ok();
        }
        true
    }

    /// Adds a new track to the playback queue.
    pub fn add_to_queue(&mut self, title: &[u8], duration_sec: u32) -> bool {
        let ok = self.store.add(title, duration_sec);
        if ok {
            self.sync_list_extent();
            self.publish_now_playing();
            std::log("[MUSIC APP] Track added to queue.").ok();
        }
        ok
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
        let next = if self.loop_queue {
            (cur + delta).rem_euclid(len)
        } else {
            let n = cur + delta;
            if n < 0 {
                0
            } else if n >= len {
                len - 1
            } else {
                n
            }
        };
        if !self.loop_queue && next == cur {
            std::log("[MUSIC APP] Track step reached queue boundary with looping disabled.").ok();
            return false;
        }
        self.current_track_idx = next as usize;
        self.reload_current_art();
        // WHY the line is cleared here: a refusal or an "ended" notice names
        // the track that was selected before this step, and leaving it under a
        // different title would describe a failure that is no longer current.
        self.clear_feedback();
        // WHY the stream is reopened: it carries the previous track's samples
        // and its position counter, so continuing it would play the rest of one
        // file under another file's name.
        match self.state {
            PlaybackState::Playing => {
                self.start_selected();
            }
            PlaybackState::Paused => {
                self.player.stop();
                self.state = PlaybackState::Stopped;
            }
            _ => {}
        }
        self.publish_now_playing();
        true
    }

    /// Moves on to the next track that can actually play.
    ///
    /// WHY it searches instead of taking the next entry: the library persists
    /// track records but their source paths are not guaranteed to survive a
    /// reboot, so one unplayable entry must not stop a queue that still holds
    /// playable tracks. Every entry is tried at most once, including a wrap
    /// around the end of the list.
    pub fn advance_after_end(&mut self) -> bool {
        let len = self.store.len();
        if len == 0 {
            self.state = PlaybackState::Stopped;
            return false;
        }
        if !self.loop_queue && self.current_track_idx + 1 >= len {
            self.state = PlaybackState::Stopped;
            // WHY the player is not stopped here: it has already closed its
            // stream and its file, and its state is the platform's last
            // answer about the track that just ended. Stopping it would
            // erase that answer and put "Stopped" on the screen.
            if self.player.state != PlayerState::Ended {
                self.player.stop();
            }
            let mut msg = [0u8; FEEDBACK_CAP];
            let n = music::ended_feedback(None, false, &mut msg);
            self.set_feedback(&msg[..n]);
            std::log("[MUSIC APP] Queue ended with looping disabled; transport stopped.").ok();
            return false;
        }
        let start = (self.current_track_idx + 1) % len;
        for step in 0..len {
            let next_idx = (start + step) % len;
            if !self.loop_queue && next_idx < self.current_track_idx {
                break;
            }
            self.current_track_idx = next_idx;
            if self.start_selected() {
                let mut msg = [0u8; FEEDBACK_CAP];
                let title = self
                    .store
                    .get(self.current_track_idx)
                    .map(|t| t.title())
                    .unwrap_or(b"");
                // WHY the device decides what the line says: the queue moved
                // on, but only the platform can say that the next track is
                // being streamed. The old wording claimed "playing" for a
                // track the device had already finished.
                let next_playing = self.surface_state() == music::SurfaceState::Playing;
                let n = music::ended_feedback(Some(title), next_playing, &mut msg);
                self.set_feedback(&msg[..n]);
                return true;
            }
        }
        self.state = PlaybackState::Stopped;
        let mut msg = [0u8; FEEDBACK_CAP];
        let n = music::ended_feedback(None, false, &mut msg);
        self.set_feedback(&msg[..n]);
        false
    }

    /// Toggles play/pause against the device's PCM stream.
    ///
    /// Returns `true` only when playback actually started. With no media or a
    /// refused stream the transport stays `Stopped`.
    pub fn toggle_play(&mut self) -> bool {
        // WHY the platform's state decides and not the app's: the control has
        // to act on what the device is doing. A transport that intended to
        // play while the device had already finished its track would pause a
        // stream that is not there, and the button would do nothing.
        match self.surface_state() {
            music::SurfaceState::Playing => {
                self.player.pause();
                self.player.poll();
                self.state = PlaybackState::Paused;
                self.clear_feedback();
                self.publish_now_playing();
                std::log("[MUSIC APP] Transport paused; the device keeps what it already holds.").ok();
                false
            }
            music::SurfaceState::Paused if self.player.holds(self.current_track_idx) => {
                self.player.resume();
                self.state = PlaybackState::Playing;
                self.clear_feedback();
                self.publish_now_playing();
                std::log("[MUSIC APP] Transport resumed from the file position it stopped at.").ok();
                true
            }
            _ => {
                let started = self.start_selected();
                self.publish_now_playing();
                started
            }
        }
    }

    /// Publishes current now-playing metadata to the settings store for
    /// notification bar and system-wide media card consumption.
    pub fn publish_now_playing(&self) {
        // WHY the artist can be empty: the library record keeps no artist and
        // the platform's decoders expose no ID3/FLAC text metadata, so the
        // player has none to name. The old code published "Local Library",
        // which named the store rather than the performer.
        let (title, artist): (&[u8], &[u8]) = if let Some(t) = self.store.get(self.current_track_idx) {
            (t.title(), t.artist())
        } else {
            (b"", b"")
        };
        // WHY the publication carries the platform's answer and not the app's
        // intent: the launcher and the notification shade read this key, and
        // a shade that says "playing" while the device is silent is the same
        // lie on another surface.
        let state_str: &[u8] = music::surface_state_name(self.surface_state());
        music::publish_now_playing(title, artist, state_str);
    }

    /// Restores the track the previous session selected.
    ///
    /// WHY the transport is not restored as playing: this app owns the PCM
    /// stream, and the process stopped the stream when it exited. Nothing is
    /// playing after a relaunch, so claiming otherwise would be a state the
    /// device cannot produce. What the previous session can still answer is
    /// which track was selected.
    pub fn restore_playback_state(&mut self) {
        let mut state_buf = [0u8; 16];
        if let Ok(n) = std::settings_get(music::NOW_PLAYING_STATE_KEY, &mut state_buf) {
            let state_str = &state_buf[..n];
            // WHY every state but "stopped" counts: the truth fix added
            // "ended", "unavailable" and "not_running", and a session that
            // ended still selected the track the screen should show. Only a
            // stopped session has no track to restore.
            if !state_str.is_empty() && state_str != b"stopped" && !self.store.is_empty() {
                let mut title_buf = [0u8; music::TITLE_MAX];
                if let Ok(tn) = std::settings_get(music::NOW_PLAYING_TITLE_KEY, &mut title_buf) {
                    if tn > 0 {
                        for i in 0..self.store.len() {
                            if let Some(t) = self.store.get(i) {
                                if t.title() == &title_buf[..tn] {
                                    self.current_track_idx = i;
                                    self.reload_current_art();
                                    break;
                                }
                            }
                        }
                    }
                }
                self.state = PlaybackState::Stopped;
                std::log("[MUSIC APP] The previous session selected a track; the transport starts stopped because its stream ended with the process.",
                )
                .ok();
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
        if library::publish_library(&self.store) == library::PublishOutcome::WriteRefused {
            // WHY this aborts: the ringtone setting stores a path to the
            // document, and a path to a document that was never written would
            // name a ringtone the device cannot play.
            self.set_feedback(b"Ringtone not set: track document was not saved");
            std::log("[MUSIC APP] The track document could not be written, so the ringtone was not set.").ok();
            return false;
        }

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
            self.clear_feedback();
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

/// The line that names the features the platform does not offer this app, and
/// carries transient feedback when there is any.
///
/// WHY one line and not two: the band between the search row (ends at 796) and
/// the volume row (starts at 860) holds a single caption. A second line at
/// y 828 ran into the volume controls, so feedback takes the caption's place
/// while it is shown.
///
/// WHY the caption sits at 800 and not at 808: a size-1 run draws its ink
/// from y0 + 5, so the caption at 808 put its ink on rows 813..823 while the
/// timeline bar was drawn on rows 824..829. The caption and the bar shared
/// the bottom of the band. At 800 the caption's ink ends on row 815 and the
/// bar's ink starts on row 827, so the two never touch.
const CAPTION_Y: usize = 800;

/// The rows the caption is drawn on, and the rows a touch on it reads.
const CAPTION_TOUCH_Y0: usize = 800;
const CAPTION_TOUCH_Y1: usize = 820;

/// The progress bar under the caption.
///
/// WHY it is at 828 and not at 824: the caption's ink ends on row 815, so
/// the bar and its thumb (rows 827..835) sit clear of it.
const TIMELINE_RECT: Rect = Rect { x: 24, y: 828, w: 672, h: 6 };

/// The thumb's centre row on the bar.
const TIMELINE_THUMB_CY: usize = 831;

/// The rows a touch on the bar is read from.
///
/// WHY forty-four of them: the bar is drawn six rows tall and its thumb reaches
/// nine, and a band of twenty-one rows (824..844) asked a person to hit a strip
/// thinner than the floor the platform sets for any control you can move. The
/// band now runs from below the caption's ink to the row above the volume
/// controls, which is the whole space the bar owns.
const TIMELINE_TOUCH_Y0: usize = 816;
const TIMELINE_TOUCH_Y1: usize = 859;

/// The search query's capacity, in bytes.
const SEARCH_CAP: usize = 32;

/// Capacity for live feedback notifications.
///
/// WHY 64: the longest refusal message is "Cannot play: " plus a reason of at
/// most 45 bytes. A shorter buffer used to truncate the reason mid-word.
const FEEDBACK_CAP: usize = music::FEEDBACK_MAX;

/// Metadata band (title, artist, state, search row, availability line).
///
/// WHY this band exists: pressing play/pause moves the state label at y 716 and
/// nothing else in this region. The old renderer re-rasterised all 1440 rows to
/// change 24 of them.
const META_Y0: usize = 620;
const META_Y1: usize = 860;

// -----------------------------------------------------------------------------
// Volume row (y 860..908). Real telemetry only.
//
// WHY it starts at 860 and not 848: the seek bar's touch band needs the platform
// minimum of forty-four rows and the bar sits at 828, so the band reaches to row
// 859. At 848 the row below answered taps that belonged to the bar.
// -----------------------------------------------------------------------------
const VOLUME_Y0: usize = 860;
const VOLUME_Y1: usize = 908;
/// Step-down / step-up controls.
///
/// Touch targets are at least 48px to comply with platform guidelines.
const VOL_MINUS: Rect = Rect::new(24, 860, 56, 48);
const VOL_PLUS: Rect = Rect::new(640, 860, 56, 48);
const VOL_LABEL_X: usize = 96;
const VOL_VALUE_X: usize = 190;
/// Meter trough and fill, drawn only when a real level is known.
const VOL_METER_X0: usize = 310;
const VOL_METER_X1: usize = 624;
const VOL_METER_Y0: usize = 878;
const VOL_METER_Y1: usize = 890;

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
    let y0 = rect.y + rect.h.saturating_sub(font::line_height(scale)) / 2;
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
    /// The state the platform reported, resolved once per frame.
    ///
    /// WHY it is resolved once: the renderer runs once per pixel, and the
    /// state has to be the same answer on every pixel of the frame.
    surface: music::SurfaceState,
    /// The state line: the transport's own state and the queue's position.
    state: [u8; 56],
    state_len: usize,
    /// The queue's caption: how many tracks the queue holds.
    queue_caption: [u8; 32],
    queue_caption_len: usize,
    clock: [u8; 5],
    clock_ok: bool,
}

/// The position of the current track in the queue the list shows.
///
/// WHY the position is looked up through the order table: the list shows
/// the tracks that match the search, so the current track's place in the
/// queue is not its store index.
fn current_position(app: &MusicApp) -> Option<(usize, usize)> {
    music::queue_position(&app.order, app.order_len, app.current_track_idx)
}

fn build_frame(app: &MusicApp) -> Frame {
    let mut clock = [b' '; 5];
    let mut clock_ok = false;
    if let Some(now) = std::rtc::Reading::now() {
        clock = now.clock_text();
        clock_ok = true;
    }
    let mut f = Frame {
        vol: VolumeFrame::build(app),
        surface: app.surface_state(),
        state: [0; 56],
        state_len: 0,
        queue_caption: [0; 32],
        queue_caption_len: 0,
        clock,
        clock_ok,
    };
    // WHY the label comes from the platform's report and not from the app's
    // own state: the screen must say what the device is doing. The old
    // mapping read the app's intent, so the screen said "Playing" while the
    // device had already finished the track.
    let state_label: &[u8] = music::surface_label(f.surface);
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
    } else if app.search_len > 0 && !app.store.is_empty() {
        // WHY: the visible queue holds only the matches, so a current track
        // the search filtered out has no position to report. Saying it is
        // hidden is better than dropping the position without explanation.
        for &byte in b" - hidden by search" {
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

/// A small downward triangle: the queue's "move down" affordance.
fn down_glyph(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    if dy < -6 || dy > 6 {
        return false;
    }
    dx.abs() <= 6 - dy
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
const STATUS_BAR_HEIGHT: usize = interos_std::design::viewport::STATUS_BAR_HEIGHT;

pub fn status_band() -> (usize, usize) {
    (0, STATUS_BAR_HEIGHT)
}

#[inline(never)]
fn get_music_pixel(x: usize, y: usize, app: &MusicApp, f: &Frame) -> u32 {
    // Status Band (y < STATUS_BAR_HEIGHT)
    if y < STATUS_BAR_HEIGHT {
        let band = interos_std::design::components::StatusBand::new(W);
        let clock = if f.clock_ok { Some(&f.clock[..]) } else { None };
        return band
            .pixel(x, y, clock, &interos_std::design::components::Instruments::unmeasured(), &THEME)
            .unwrap_or(THEME.surface);
    }
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

    // 2. Album artwork.
    //
    // WHAT is drawn: the platform's card treatment with a neutral note glyph.
    // WHY there is never a cover: the platform's WAV/FLAC/MP3 decoders expose
    // no ID3 APIC or FLAC PICTURE block, so this app has no embedded artwork
    // it could honestly show. The caption names the missing fact instead of
    // inventing a cover.
    let sel: Option<&store::TrackRecord> = app.store.get(app.current_track_idx);
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
        if app.has_cover_art {
            let rgba = unsafe { &ALBUM_ART_RGBA[..app.cover_art_len] };
            let bitmap = std::image::Bitmap {
                w: app.cover_art_w,
                h: app.cover_art_h,
                rgba,
            };
            return bitmap.pixel(x - ART_X, y - ART_Y, ART_W, ART_H);
        }
        if note_glyph(x, y, ART_X + ART_W / 2, ART_GLYPH_CY) {
            return THEME.muted;
        }
        let caption: &[u8] = if sel.is_none() {
            b"No track selected"
        } else {
            b"No cover art"
        };
        if !caption.is_empty() {
            if let Some(c) = text_centered(
                x,
                y,
                ART_CAPTION_Y,
                1,
                caption,
                THEME.muted,
                ART_CARD.fill,
            ) {
                return c;
            }
        }
        return ART_CARD.fill;
    }

    // 3. Track metadata from the real, persisted library (empty -> honest idle
    //    copy). The now-playing row reads from here.
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
    // Honest sub-caption. While a stream is open it shows the device's frame
    // count against the decoder's own total; stopped, it shows the length the
    // record holds or says the length was never read. An artist is prefixed
    // only when one is actually known.
    if let Some(t) = sel {
        let mut cap = [0u8; music::NOW_PLAYING_MAX];
        let holds = app.player.holds(app.current_track_idx);
        let position = if holds { app.player.position_ms() } else { None };
        let total = if holds { app.player.duration_ms() } else { None };
        let cl = music::now_playing_line(
            music::known_artist(t.artist()),
            position,
            total,
            t.duration_sec(),
            t.duration_known(),
            &mut cap,
        );
        let mut ebuf = [0u8; music::NOW_PLAYING_MAX];
        let en = font::ellipsize(&cap[..cl], 2, CONTENT_W, &mut ebuf);
        if let Some(c) = text_centered_bold(x, y, ARTIST_Y, 2, &ebuf[..en], THEME.on_surface, THEME.surface) {
            return c;
        }
    } else if let Some(c) = text_centered_bold(x, y, ARTIST_Y, 2, b"Local library is empty - nothing to play", THEME.on_surface, THEME.surface) {
        return c;
    }
    // WHY the label comes from the frame: the state is resolved once per
    // frame from the platform's report, so every pixel of the line shows the
    // same answer and the label can never disagree with the transport glyph.
    let state_label: &[u8] = music::surface_label(f.surface);
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
            CAPTION_Y,
            1,
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
    // what the device can do instead. Feedback temporarily replaces this
    // line rather than sharing a band too short for two of them.
    if app.feedback_len == 0 {
        let loop_text: &[u8] = if app.loop_queue {
            b"Playlists unavailable - queue loop active; touch bar to seek"
        } else {
            b"Playlists unavailable - queue loop off; touch bar to seek"
        };
        if let Some(c) = text_centered(
            x,
            y,
            CAPTION_Y,
            1,
            loop_text,
            THEME.muted,
            THEME.surface,
        ) {
            return c;
        }
    }

    // WHY the band is 826..836: the bar is drawn on rows 828..833 and its
    // thumb reaches rows 827..835, so the band has to cover both of them.
    if y >= 826 && y <= 836 && x >= TIMELINE_RECT.x && x < TIMELINE_RECT.x + TIMELINE_RECT.w {
        let dur = app.player.duration_ms().unwrap_or_else(|| {
            app.store.get(app.current_track_idx).map(|t| (t.duration_sec() as u64) * 1000).unwrap_or(0)
        });
        let pos = app.player.position_ms().unwrap_or(0);
        let fill_w = if dur > 0 {
            ((pos.min(dur) * (TIMELINE_RECT.w as u64)) / dur) as usize
        } else {
            0
        };
        let thumb_cx = TIMELINE_RECT.x + fill_w;
        let dx = (x as isize) - (thumb_cx as isize);
        let dy = (y as isize) - (TIMELINE_THUMB_CY as isize);
        if dx * dx + dy * dy <= 16 {
            return THEME.on_surface;
        }
        if y >= TIMELINE_RECT.y && y < TIMELINE_RECT.y + TIMELINE_RECT.h {
            if x < TIMELINE_RECT.x + fill_w {
                return THEME.primary;
            } else {
                return THEME.surface_container_high;
            }
        }
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
            // WHY the glyph follows the platform and not the app's intent:
            // the pause bars mean "the device is streaming right now", and
            // drawing them while nothing was playing was the other half of
            // the same lie the state label told.
            if f.surface == music::SurfaceState::Playing && active {
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
                store::StoreState::WriteRefused => (
                    b"Tracks not saved",
                    b"Audio was found, but the settings write was refused",
                ),
                store::StoreState::Full => (
                    b"Library full",
                    b"Audio was found, but the library holds no free slot",
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
                // The queue's reorder affordances. The three controls sit on
                // 44 px centres, so each hit region is a 44 px target with no
                // overlap (the platform's minimum is 44 px).
                if up_glyph(x, y, MARGIN + CONTENT_W - 116, row_y + 24) {
                    return THEME.muted;
                }
                if down_glyph(x, y, MARGIN + CONTENT_W - 72, row_y + 24) {
                    return THEME.muted;
                }
                // Delete affordance, matching its hit region in the tap handler.
                let del_x = MARGIN + CONTENT_W - 28;
                let ddx = x as isize - del_x as isize;
                let ddy = y as isize - (row_y + 24) as isize;
                if ddx.abs() <= 6 && ddy.abs() <= 6 && (ddx.abs() - ddy.abs()).abs() <= 2 {
                    return THEME.muted;
                }
                let mut buf = [0u8; 48];
                let n = font::ellipsize(t.title(), 2, CONTENT_W - 220, &mut buf);
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
                    music::format_mmss(&mut db, t.duration_sec())
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
    /// The volume meter moved: the VOLUME_Y0..VOLUME_Y1 band.
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
            app.clear_feedback();
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

    // Timeline seeking hit-test
    if y >= TIMELINE_TOUCH_Y0 && y <= TIMELINE_TOUCH_Y1 && x >= TIMELINE_RECT.x && x < TIMELINE_RECT.x + TIMELINE_RECT.w {
        let dur = app.player.duration_ms().unwrap_or_else(|| {
            app.store.get(app.current_track_idx).map(|t| (t.duration_sec() as u64) * 1000).unwrap_or(0)
        });
        if dur > 0 {
            let offset_x = (x - TIMELINE_RECT.x) as u64;
            let target_ms = (offset_x * dur) / (TIMELINE_RECT.w as u64);
            if app.seek(target_ms) {
                let _ = std::audio_haptic_click();
                return Tap::Redraw(Dirty::Meta);
            }
        }
        return Tap::Redraw(Dirty::None);
    }

    // Toggle queue looping hit-test on the caption's band.
    //
    // WHY the band is the caption's own rows: the caption line doubles as
    // the queue loop control, so the hit band has to be the rows the caption
    // is drawn on. It ends above the timeline's band, so the two controls
    // cannot both answer the same touch.
    if y >= CAPTION_TOUCH_Y0 && y < CAPTION_TOUCH_Y1 {
        if app.toggle_loop_queue() {
            let _ = std::audio_haptic_click();
            return Tap::Redraw(Dirty::Meta);
        }
    }

    // The library's search field.
    if SEARCH_FIELD.contains(x, y) {
        app.search_focused = true;
        // The search is a new interaction; a refusal or import result from
        // the last one must not sit under it as though it were current.
        app.clear_feedback();
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
        app.sync_list_extent();
        match outcome {
            ImportOutcome::Imported(n) => {
                std::log("[MUSIC APP] Imported audio files from the shared storage; their lengths were not read.").ok();
                let _ = n;
            }
            ImportOutcome::PartiallyImported(n, refused) => {
                std::log("[MUSIC APP] Imported some audio files, but the settings store refused the rest.").ok();
                let _ = (n, refused);
            }
            ImportOutcome::WriteRefused(found) => {
                std::log("[MUSIC APP] Audio files were found, but the settings write was refused, so none were added.").ok();
                let _ = found;
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
        // WHY the result is put on the screen: the import used to report only
        // in the log, so a person saw the library unchanged and could not tell
        // an empty search from a refused write.
        let mut msg = [0u8; FEEDBACK_CAP];
        let n = music::import_feedback(outcome, &mut msg);
        app.set_feedback(&msg[..n]);
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
        app.clear_feedback();
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
        app.clear_feedback();
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
        // A real row: three 44 px controls on its right reorder or remove the
        // entry, and a tap elsewhere selects it as now-playing.
        //
        // WHY the scroll offset is added back: the rows are drawn at their
        // scrolled position, so the row under the finger is the one whose
        // index is the offset-adjusted position divided by the row height.
        // Without it, a tap on a scrolled row selected the wrong track.
        let ry = y.saturating_sub(LIST_TOP) + app.list_scroll.offset;
        let idx = ry / TRACKLIST_ROW_H;
        let coff = ry % TRACKLIST_ROW_H;
        if idx < app.order_len && coff < 84 && x >= MARGIN && x < MARGIN + CONTENT_W {
            let control_y = coff as isize - 24;
            let in_control = |centre: usize| {
                (x as isize - centre as isize).abs() <= 22 && control_y.abs() <= 22
            };
            // Delete: the rightmost control.
            if in_control(MARGIN + CONTENT_W - 28) {
                if app.remove_at(idx) {
                    app.clear_feedback();
                    std::log("[MUSIC APP] Track removed; the deletion was persisted to the settings store.",
                    )
                    .ok();
                } else {
                    std::log("[MUSIC APP] Track deletion was not persisted; the library is unchanged.",
                    )
                    .ok();
                }
                return Tap::Redraw(Dirty::All);
            }
            // Move down: toward the end of the queue.
            if in_control(MARGIN + CONTENT_W - 72) {
                if app.move_down(idx) {
                    app.sync_list_extent();
                    app.clear_feedback();
                    std::log("[MUSIC APP] Track moved down in the queue; the new order was persisted.",
                    )
                    .ok();
                    app.publish_now_playing();
                } else {
                    std::log("[MUSIC APP] The track is already last in the queue, or the new order was not persisted.",
                    )
                    .ok();
                }
                return Tap::Redraw(Dirty::All);
            }
            // Move up: toward the front of the queue.
            if in_control(MARGIN + CONTENT_W - 116) {
                if app.move_up(idx) {
                    app.sync_list_extent();
                    app.clear_feedback();
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
                // WHY the line is cleared: the previous refusal or "ended"
                // notice belongs to the track that was selected before this
                // tap, not to the one the screen now names.
                app.clear_feedback();
                // WHY the stream follows the selection: the transport is
                // playing a file, and leaving it on the previous file while
                // the screen names the new one would make the play control
                // act on a track the person did not select.
                match app.state {
                    PlaybackState::Playing => {
                        app.start_selected();
                    }
                    PlaybackState::Paused => {
                        app.player.stop();
                        app.state = PlaybackState::Stopped;
                    }
                    _ => {}
                }
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
                app.clear_feedback();
                app.sync_list_extent();
                Tap::Redraw(Dirty::All)
            }
            code if (32..=126).contains(&code) && app.search_len < SEARCH_CAP => {
                app.search[app.search_len] = code as u8;
                app.search_len += 1;
                app.clear_feedback();
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
                app.clear_feedback();
                Tap::Redraw(Dirty::Volume)
            } else {
                Tap::Redraw(Dirty::None)
            }
        }
        input_key_code::ARROW_DOWN => {
            if app.step_volume(false) {
                app.clear_feedback();
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
    // A dispatch that launched this app is already waiting, so it is taken
    // before the first frame is drawn.
    music.handle_pending_intent();
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

    // 5. The transport starts stopped. Playback needs a file and the device's
    //    PCM stream, so it begins when the person presses play and the
    //    engine reports whether the stream was accepted.

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
                                        std::log("[MUSIC APP] App moved to background; audio playback continues.").ok();
                                        let _ = std::app_background();
                                    } else {
                                        music.player.stop();
                                        music.state = PlaybackState::Stopped;
                                        music.publish_now_playing();
                                        if surface > 0 {
                                            let _ = std::surface_destroy(surface);
                                        }
                                        std::exit_with_code(0);
                                    }
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
                            std::log("[MUSIC APP] Navigation gesture recognized. Leaving to launcher.",
                            )
                            .ok();
                            if music.state == PlaybackState::Playing {
                                std::log("[MUSIC APP] App moved to background; audio playback continues.").ok();
                                let _ = std::app_background();
                            } else {
                                music.player.stop();
                                music.state = PlaybackState::Stopped;
                                music.publish_now_playing();
                                if surface > 0 {
                                    let _ = std::surface_destroy(surface);
                                }
                                std::exit_with_code(0);
                            }
                            continue;
                        }
                        if input.kind == 1 {
                            match handle_tap(&mut music, input.x, input.y) {
                                Tap::Leave(msg) => {
                                    std::log(msg).ok();
                                    if music.state == PlaybackState::Playing {
                                        std::log("[MUSIC APP] App moved to background; audio playback continues.").ok();
                                        let _ = std::app_background();
                                    } else {
                                        music.player.stop();
                                        music.state = PlaybackState::Stopped;
                                        music.publish_now_playing();
                                        if surface > 0 {
                                            let _ = std::surface_destroy(surface);
                                        }
                                        std::exit_with_code(0);
                                    }
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
                    // A dispatch may reach this app while it is already
                    // running, so the slot is polled rather than pushed.
                    if music.handle_pending_intent() {
                        repaint(surface, &music, Dirty::All);
                        let _ = std::surface_flip();
                    }
                    if music.state == PlaybackState::Playing {
                        music.player.tick();
                        if music.player.state == PlayerState::Ended {
                            std::log("[MUSIC APP] Track ended; the device finished its last PCM batch.",
                            )
                            .ok();
                            // WHY the next playable track starts here: the
                            // stream reported that the device finished every
                            // sample it was given, so the queue's next entry is
                            // what follows. Entries that cannot play are
                            // skipped rather than stopping the whole queue.
                            music.advance_after_end();
                            music.publish_now_playing();
                            if surface > 0 {
                                repaint(surface, &music, Dirty::All);
                                let _ = std::surface_flip();
                            }
                        } else if music.player.state == PlayerState::Stopped || music.player.state == PlayerState::Paused {
                            music.state = if music.player.state == PlayerState::Paused {
                                PlaybackState::Paused
                            } else {
                                PlaybackState::Stopped
                            };
                            music.publish_now_playing();
                            if surface > 0 {
                                repaint(surface, &music, Dirty::Playback);
                                let _ = std::surface_flip();
                            }
                        } else {
                            let second = music.player.position_ms().unwrap_or(0) / 1000;
                            if second != music.shown_second {
                                music.shown_second = second;
                                if surface > 0 {
                                    repaint(surface, &music, Dirty::Playback);
                                    let _ = std::surface_flip();
                                }
                            }
                        }
                    } else {
                        // A paused stream still drains what the device already
                        // holds; polling keeps the shown position the device's
                        // own count rather than the count at the pause.
                        music.player.poll();
                    }
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
    std::log("[MUSIC APP] Panic detected! Terminating task...").ok();
    std::exit();
}
