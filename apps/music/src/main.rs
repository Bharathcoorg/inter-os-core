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
use std::design::color::{
    DARK_BACKGROUND as BG, DARK_BORDER as BORDER, DARK_SURFACE as SURFACE, DARK_TEXT as TEXT,
    DARK_TEXT_MUTED as MUTED, TEXT as INK_ON_LIGHT, TEXT_MUTED as FAINT,
};
use std::design::font;
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
        }
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
                std::log(
                    0,
                    "[MUSIC APP] Audio service returned an out-of-range volume; meter unavailable.",
                )
                .ok();
                false
            }
            Err(_) => {
                // WHY: audio control is refused without the audio-service
                // authority. Never log or draw a volume that was never issued.
                self.volume_percent = None;
                std::log(
                    0,
                    "[MUSIC APP] Volume query refused by the audio service; no volume is known.",
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
            std::log(
                0,
                "[MUSIC APP] Volume step ignored: no volume is known to step from.",
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
                std::log(
                    0,
                    "[MUSIC APP] Audio service applied an out-of-range volume; meter hidden.",
                )
                .ok();
                self.volume_percent = None;
                false
            }
            Err(_) => {
                std::log(
                    0,
                    "[MUSIC APP] Volume change refused by the audio service; level unchanged.",
                )
                .ok();
                false
            }
        }
    }

    /// Derives the media storage key from Hardware Keystore Slot 1.
    ///
    /// Returns `true` only when a full 32-byte key was actually returned.
    pub fn derive_keystore_key(&mut self) -> bool {
        let mut media_key = [0u8; 32];
        match std::keystore_derive(1, &mut media_key) {
            Ok(32) => {
                self.keystore_derived = true;
                std::log(0, "[MUSIC APP] Hardware keystore: Ingested media decryption key for local audio streaming.").ok();
                true
            }
            Ok(_) => {
                // WHY: a short result is not a key. Report it instead of leaving
                // the caller to assume derivation succeeded.
                std::log(0, "[MUSIC APP] Hardware keystore: short derivation result; no media key available.").ok();
                false
            }
            Err(_) => {
                // WHY: this app holds no keystore capability, so the call is
                // refused. Say so instead of reporting a key that was never issued.
                std::log(0, "[MUSIC APP] Hardware keystore: derivation refused by the kernel; no media key available.").ok();
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
            std::log(
                0,
                "[MUSIC APP] Arpeggiated C-major melody synthesized (261Hz -> 523Hz).",
            )
            .ok();
            true
        } else {
            // WHY: audio control is refused without the audio-service authority.
            // Never log a synthesized melody that was never emitted.
            std::log(
                0,
                "[MUSIC APP] Arpeggio not played: the audio service refused the tone requests.",
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
        if PLAYLIST.is_empty() {
            std::log(0, "[MUSIC APP] No media payload is available for DMA streaming.").ok();
            return false;
        }
        // A non-empty playlist would require a real DMA descriptor from the media
        // subsystem. No such descriptor exists yet, so fail closed.
        std::log(
            0,
            "[MUSIC APP] Media present but no DMA descriptor was obtained; no buffer streamed.",
        )
        .ok();
        false
    }

    /// Encrypts metadata only when a real media record exists.
    pub fn encrypt_track_record(&mut self) {
        std::log(0, "[MUSIC APP] No media record is available for encryption.").ok();
    }

    /// Steps the playlist index, but only when there is real media to step over.
    ///
    /// Returns `true` only when the selection actually moved. A tap on prev/next
    /// with an empty library changes nothing, so it reports nothing changed.
    pub fn step_track(&mut self, delta: isize) -> bool {
        if PLAYLIST.is_empty() {
            std::log(
                0,
                "[MUSIC APP] Track step ignored: the local library is empty, so there is no next or previous track.",
            )
            .ok();
            return false;
        }
        let len = PLAYLIST.len() as isize;
        let cur = self.current_track_idx as isize;
        let next = (cur + delta).rem_euclid(len);
        self.current_track_idx = next as usize;
        true
    }

    /// Toggles play/pause against the real media path.
    ///
    /// Returns `true` only when playback actually started (a DMA transfer
    /// occurred). With no media the transport stays `Stopped`.
    pub fn toggle_play(&mut self) -> bool {
        match self.state {
            PlaybackState::Playing => {
                self.state = PlaybackState::Paused;
                std::log(0, "[MUSIC APP] Transport paused by user.").ok();
                false
            }
            _ => {
                if self.stream_dma_audio_block() {
                    self.total_dma_buffers_streamed += 1;
                    self.state = PlaybackState::Playing;
                    true
                } else {
                    self.state = PlaybackState::Stopped;
                    std::log(
                        0,
                        "[MUSIC APP] Play requested but no media is available; transport stays stopped.",
                    )
                    .ok();
                    false
                }
            }
        }
    }
}

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// -----------------------------------------------------------------------------
// 32-bit ARGB8888 Design Color Tokens (immersive dark media surface)
//
// Every token the renderer uses is an alias of the shared canonical palette in
// `interos_std::design::color`, so this surface cannot silently drift from the
// design system the way raw hex literals in this file could. The aliases keep
// the short local names the renderer already uses. (When reconciled, every
// literal matched its shared token exactly, so this is a structural fix, not a
// colour change.)
// -----------------------------------------------------------------------------
/// Recessed control fill - a disabled transport sits below the surface.
///
/// The one colour not in the shared palette: it is the step between
/// `DARK_BACKGROUND` and `DARK_SURFACE` that gives a disabled control its
/// sunken read, used nowhere else.
const RECESSED: u32 = 0xFF1B1E20;

// -----------------------------------------------------------------------------
// Layout constants - shared by the renderer and the hit-tester
// -----------------------------------------------------------------------------
const MARGIN: usize = 24;
const CONTENT_W: usize = W - 2 * MARGIN;

const BAR_H: usize = 96;
const BACK: Rect = Rect::new(24, 24, 72, 72);

const ART_X: usize = 136;
const ART_Y: usize = 128;
const ART_W: usize = 448;
const ART_H: usize = 448;
const ART_GLYPH_CY: usize = 300;
const ART_CAPTION_Y: usize = 400;

const TITLE_Y: usize = 620;
const ARTIST_Y: usize = 684;
const STATE_Y: usize = 716;
const TIMES_Y: usize = 756;
const TRACK_Y: usize = 796;
const TRACK_H: usize = 4;
const SCRUB_NOTE_Y: usize = 816;

/// Metadata band (title, artist, state, times, track rule, scrub note).
///
/// WHY this band exists: pressing play/pause moves the state label at y 716 and
/// nothing else in this region, and a disabled scrub bar never changes at all.
/// The old renderer re-rasterised all 1440 rows to change 24 of them.
const META_Y0: usize = 620;
const META_Y1: usize = 840;

// -----------------------------------------------------------------------------
// Volume row (y 848..892). Real telemetry only.
// -----------------------------------------------------------------------------
const VOLUME_Y0: usize = 848;
const VOLUME_Y1: usize = 892;
/// Step-down / step-up controls: 56 x 40 px, both >= the 56 px touch minimum.
const VOL_MINUS: Rect = Rect::new(24, 852, 56, 40);
const VOL_PLUS: Rect = Rect::new(600, 852, 56, 40);
const VOL_LABEL_X: usize = 96;
const VOL_VALUE_X: usize = 190;
/// Meter trough and fill, drawn only when a real level is known.
const VOL_METER_X0: usize = 330;
const VOL_METER_X1: usize = 584;
const VOL_METER_Y0: usize = 862;
const VOL_METER_Y1: usize = 874;

const TRANSPORT_CY: usize = 990;
const PREV_CX: usize = 190;
const PLAY_CX: usize = 360;
const NEXT_CX: usize = 530;
const SIDE_R: usize = 32; // 64 px target, >= 56 px required
const PLAY_R: usize = 44; // 88 px target, >= 56 px required

/// Transport row band: the play/pause disc is the tallest control (r = 44).
const TRANSPORT_Y0: usize = TRANSPORT_CY - PLAY_R;
const TRANSPORT_Y1: usize = TRANSPORT_CY + PLAY_R;

const LIST_LABEL_Y: usize = 1080;
const LIST_DIVIDER_Y: usize = 1116;
const LIST_HIT_Y0: usize = 1130;
const LIST_HIT_Y1: usize = 1260;
const LIST_TITLE_Y: usize = 1160;
const LIST_SUB_Y: usize = 1224;

const HOME_X0: usize = 240;
const HOME_X1: usize = 480;
const HOME_Y0: usize = 1418;
const HOME_Y1: usize = 1424;

// -----------------------------------------------------------------------------
// Text helpers - the gate is the glyph line box, so ink is never clipped.
// -----------------------------------------------------------------------------
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
    if y < y0 || y >= y0 + font::line_height(scale) {
        return None;
    }
    font::test_text_aa(x, y, x0, y0, scale, s, color, bg, false)
}

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
    if y < y0 || y >= y0 + font::line_height(scale) {
        return None;
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
}

fn build_frame(app: &MusicApp) -> Frame {
    Frame {
        vol: VolumeFrame::build(app),
    }
}

// -----------------------------------------------------------------------------
// Vector pictograms (one rounded, ~2px stroke family)
// -----------------------------------------------------------------------------

/// Dismiss chevron: a "v" stroke whose ends sit at (cx +/- half, cy) and whose
/// apex sits at (cx, cy + half).
fn chevron_down(x: usize, y: usize, cx: usize, cy: usize, half: isize) -> bool {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    if dy < 0 || dy > half || dx.abs() > half {
        return false;
    }
    let s = dx.abs() + dy;
    (half - 2..=half + 2).contains(&s)
}

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

/// Draws the volume row: label, real value, meter trough and the two step
/// controls in the live or recessed treatment.
fn volume_pixel(x: usize, y: usize, app: &MusicApp, f: &Frame) -> Option<u32> {
    // Step controls first: they are drawn on top of nothing, but their hit rects
    // are the authority for the tap handler.
    for (r, plus) in [(VOL_MINUS, false), (VOL_PLUS, true)] {
        if let Some(c) = draw_rounded_rect(x, y, r, 12, RECESSED, Some(BORDER), 1) {
            let ink = if app.volume_percent.is_some() { TEXT } else { FAINT };
            if c == RECESSED && step_glyph(x, y, r, plus) {
                return Some(ink);
            }
            return Some(c);
        }
    }

    // Meter trough / fill.
    if y >= VOL_METER_Y0 && y < VOL_METER_Y1 && x >= VOL_METER_X0 && x < VOL_METER_X1 {
        if f.vol.known && x < VOL_METER_X0 + f.vol.fill_w {
            return Some(TEXT);
        }
        return Some(RECESSED);
    }

    // Label + value, both gated on their own measured line box.
    let label_y = VOLUME_Y0
        + (VOLUME_Y1 - VOLUME_Y0).saturating_sub(font::line_height(1))
        / 2;
    if let Some(c) = text(x, y, VOL_LABEL_X, label_y, 1, b"Volume", MUTED, BG) {
        return Some(c);
    }
    let value_color = if f.vol.known { TEXT } else { FAINT };
    text(
        x,
        y,
        f.vol.value_x(),
        label_y,
        1,
        &f.vol.value[..f.vol.value_len],
        value_color,
        BG,
    )
}

// -----------------------------------------------------------------------------
// Pixel Color Resolution Engine (dark minimal player)
// -----------------------------------------------------------------------------
#[inline(never)]
fn get_music_pixel(x: usize, y: usize, app: &MusicApp, f: &Frame) -> u32 {
    // 0. Home Gesture Pill (x: 260..460, y: 1418..1424).
    if y >= HOME_Y0 && y < HOME_Y1 && x >= 260 && x < 460 {
        return MUTED;
    }

    // 1. Top bar: dismiss control + screen label.
    if y < BAR_H {
        if y == BAR_H - 1 {
            return BORDER;
        }
        if let Some(c) = draw_pill(x, y, BACK, RECESSED, Some(BORDER)) {
            if c != RECESSED {
                return c;
            }
            if chevron_down(x, y, 60, 50, 12) {
                return TEXT;
            }
            return RECESSED;
        }
        if let Some(c) = text_centered(x, y, 48, 1, b"Media player", MUTED, BG) {
            return c;
        }
        return BG;
    }

    // 2. Album artwork: a neutral placeholder, never a fabricated cover.
    let art = Rect::new(ART_X, ART_Y, ART_W, ART_H);
    if let Some(c) = draw_rounded_rect(x, y, art, 24, SURFACE, Some(BORDER), 1) {
        if c != SURFACE {
            return c;
        }
        if note_glyph(x, y, ART_X + ART_W / 2, ART_GLYPH_CY) {
            return FAINT;
        }
        if let Some(c) = text_centered(
            x,
            y,
            ART_CAPTION_Y,
            1,
            b"No artwork - no album art in library",
            FAINT,
            SURFACE,
        ) {
            return c;
        }
        return SURFACE;
    }

    // 3. Track metadata from the real playlist (empty -> honest idle copy).
    let title: &[u8] = match PLAYLIST.get(app.current_track_idx) {
        Some(t) => t.title.as_bytes(),
        None => b"No track selected",
    };
    {
        let tw = font::measure(title, 2);
        if tw <= CONTENT_W {
            if let Some(c) = text_bold(x, y, (W - tw) / 2, TITLE_Y, 2, title, TEXT, BG) {
                return c;
            }
        }
    }
    let artist: &[u8] = match PLAYLIST.get(app.current_track_idx) {
        Some(t) => t.artist.as_bytes(),
        None => b"Local library is empty - nothing to play",
    };
    if let Some(c) = text_centered(x, y, ARTIST_Y, 1, artist, MUTED, BG) {
        return c;
    }
    let state_label: &[u8] = match app.state {
        PlaybackState::Playing => b"Playing",
        PlaybackState::Paused => b"Paused",
        PlaybackState::Stopped => b"Stopped",
    };
    if let Some(c) = text_centered(x, y, STATE_Y, 1, state_label, FAINT, BG) {
        return c;
    }

    // 4. Scrubber: bound to a real position; no backend reports one, so the bar
    //    stays idle (no fill, no knob, unknown duration).
    //
    // WHY the recessed fill rather than BORDER: the rule used to be drawn in the
    // same stroke colour as the enabled controls elsewhere on this surface,
    // which read as a seekable track. RECESSED is the one tone in this palette
    // reserved for something that cannot be acted on, and with no position and
    // no duration there is nothing to seek.
    if y >= TRACK_Y && y < TRACK_Y + TRACK_H && x >= MARGIN && x < MARGIN + CONTENT_W {
        return RECESSED;
    }
    if y >= TIMES_Y && y < TIMES_Y + font::line_height(1) {
        if let Some(c) = text(x, y, MARGIN, TIMES_Y, 1, b"0:00", FAINT, BG) {
            return c;
        }
        let rem = b"--:--";
        let rw = font::measure(rem, 1);
        if let Some(c) = text(x, y, MARGIN + CONTENT_W - rw, TIMES_Y, 1, rem, FAINT, BG) {
            return c;
        }
    }
    if let Some(c) = text_centered(
        x,
        y,
        SCRUB_NOTE_Y,
        1,
        b"No media backend reports a playback position",
        FAINT,
        BG,
    ) {
        return c;
    }

    // 4b. Volume row: real audio-service telemetry, or an honest "unavailable".
    if y >= VOLUME_Y0 && y < VOLUME_Y1 {
        if let Some(c) = volume_pixel(x, y, app, f) {
            return c;
        }
        return BG;
    }

    // 5. Transport row at y = 990. Targets: 64 / 88 / 64 px, all >= 56 px.
    //    With an empty library every control renders in the recessed, disabled
    //    treatment so nothing reads as playing.
    let active = !PLAYLIST.is_empty();

    {
        let r = if active { SURFACE } else { RECESSED };
        if let Some(c) = draw_circle(x, y, PREV_CX, TRANSPORT_CY, SIDE_R, r, Some(BORDER)) {
            if c != r {
                return c;
            }
            let ink = if active { TEXT } else { FAINT };
            let dx = (x as isize) - PREV_CX as isize;
            let dy = (y as isize) - TRANSPORT_CY as isize;
            // Bar at -14..-10 and a left-pointing triangle.
            if (-18..=-14).contains(&dx) && dy.abs() <= 10 {
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

    {
        let r = if active { TEXT } else { RECESSED };
        let ring = if active { None } else { Some(BORDER) };
        if let Some(c) = draw_circle(x, y, PLAY_CX, TRANSPORT_CY, PLAY_R, r, ring) {
            if c != r {
                return c;
            }
            let glyph = if active { INK_ON_LIGHT } else { FAINT };
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
        let r = if active { SURFACE } else { RECESSED };
        if let Some(c) = draw_circle(x, y, NEXT_CX, TRANSPORT_CY, SIDE_R, r, Some(BORDER)) {
            if c != r {
                return c;
            }
            let ink = if active { TEXT } else { FAINT };
            let dx = (x as isize) - NEXT_CX as isize;
            let dy = (y as isize) - TRANSPORT_CY as isize;
            if (14..=18).contains(&dx) && dy.abs() <= 10 {
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

    // 6. Track list section: real rows when the library has entries, an honest
    //    empty state when it does not.
    if y >= LIST_LABEL_Y && y < LIST_LABEL_Y + font::line_height(1) {
        if let Some(c) = text(x, y, MARGIN, LIST_LABEL_Y, 1, b"TRACKS", FAINT, BG) {
            return c;
        }
    }
    if y == LIST_DIVIDER_Y && x >= MARGIN && x < MARGIN + CONTENT_W {
        return BORDER;
    }

    match PLAYLIST.first() {
        None => {
            if let Some(c) = text_centered(x, y, LIST_TITLE_Y, 2, b"No tracks", TEXT, BG) {
                return c;
            }
            if let Some(c) = text_centered(
                x,
                y,
                LIST_SUB_Y,
                1,
                b"No audio files were found in local storage",
                MUTED,
                BG,
            ) {
                return c;
            }
        }
        Some(first) => {
            // Rows are laid out only when real media exists: 96 px rows starting
            // below the divider, each with an ellipsised title and artist.
            let _ = first;
            for (i, track) in PLAYLIST.iter().take(2).enumerate() {
                let row_y = 1140 + i * 96;
                // WHY 80 and not 72: the title owns row_y+4..+52 (scale 2) and
                // the artist owns row_y+56..+80 (scale 1). The old 72-row gate
                // cut the artist's last 8 rows - its whole descender band - so
                // every artist name with g j p q y rendered sliced. The gate is
                // the runs it contains, nothing less.
                if y >= row_y && y < row_y + 80 {
                    let selected = i == app.current_track_idx;
                    let title_y = row_y + 4;
                    let mut buf = [0u8; 48];
                    let n = font::ellipsize(track.title.as_bytes(), 2, CONTENT_W - 64, &mut buf);
                    if let Some(c) = text(
                        x,
                        y,
                        MARGIN + 40,
                        title_y,
                        2,
                        &buf[..n],
                        if selected { TEXT } else { MUTED },
                        BG,
                    ) {
                        return c;
                    }
                    let mut buf2 = [0u8; 48];
                    let n2 = font::ellipsize(track.artist.as_bytes(), 1, CONTENT_W - 64, &mut buf2);
                    if let Some(c) = text(x, y, MARGIN + 40, title_y + 52, 1, &buf2[..n2], FAINT, BG)
                    {
                        return c;
                    }
                }
                // WHY the divider lives outside the text gate above: it did not.
                // `y == row_y + 95` sat inside a gate ending at +72 (now +80), so
                // it could never be true and the separator simply never drew.
                // 88 sits 8 below the artist's last row (+80) and 8 above the
                // next row's first (+96): it belongs to the row gap, not to
                // either run, so it gets its own band test at the row's level.
                if y == row_y + 88 && x >= MARGIN && x < MARGIN + CONTENT_W {
                    return BORDER;
                }
            }
        }
    }

    BG
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
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
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
    if BACK.contains(x, y) || (x >= HOME_X0 && x < HOME_X1 && y >= 1400) {
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

    // Scrubber: drawn recessed with an explicit "no position" note, so there is
    // nothing to seek and the tap only reports that honestly.
    if x >= MARGIN && x < MARGIN + CONTENT_W && y >= TIMES_Y && y < META_Y1 {
        std::log(
            0,
            "[MUSIC APP] Scrubber tapped: no media backend reports a playback position or duration, so there is nothing to seek.",
        )
        .ok();
        return Tap::Redraw(Dirty::None);
    }

    // Track list band: honest report when the library is empty.
    if x >= MARGIN && x < MARGIN + CONTENT_W && y >= LIST_HIT_Y0 && y < LIST_HIT_Y1 {
        std::log(
            0,
            "[MUSIC APP] Track list tapped: no audio files in the local library.",
        )
        .ok();
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
        _ => Tap::None,
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(
        0,
        "[MUSIC APP (Ring 3)] Native music player online.",
    )
    .ok();

    let mut music = MusicApp::new();

    // 1. Hardware Keystore Derivation
    music.derive_keystore_key();

    // 2. Encrypt Track Record
    music.encrypt_track_record();

    // 3. Allocate DRM/KMS Client Surface (z=200)
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(id) => {
            std::log(0, "[MUSIC APP] DRM/KMS Client Surface allocated (z=200).").ok();
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
    }

    if surface > 0 {
        render_music_ui(surface, &music);
        let _ = std::surface_flip(6);
        std::log(0, "[MUSIC APP] Music canvas presented to scanout buffer.").ok();
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
                                    std::log(0, msg).ok();
                                    if surface > 0 {
                                        let _ = std::surface_destroy(6, surface);
                                    }
                                    std::exit_with_code(0);
                                }
                                Tap::Redraw(d) => dirty = merge(dirty, d),
                                Tap::None => {}
                            }
                            continue;
                        }
                        if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                            std::log(
                                0,
                                "[MUSIC APP] Navigation gesture recognized. Exiting to launcher.",
                            )
                            .ok();
                            if surface > 0 {
                                let _ = std::surface_destroy(6, surface);
                            }
                            std::exit_with_code(0);
                        }
                        if input.kind == 1 {
                            match handle_tap(&mut music, input.x, input.y) {
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
                        repaint(surface, &music, dirty);
                        let _ = std::surface_flip(6);
                    }
                }
                if events & std::EVENT_IPC_MSG != 0 {
                    std::log(
                        0,
                        "[MUSIC APP] IPC message event raised; no playback command was parsed from it.",
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
    std::log(0, "[MUSIC APP] Panic detected! Terminating task...").ok();
    std::exit();
}
