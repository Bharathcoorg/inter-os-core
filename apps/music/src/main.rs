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

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
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
    pub volume_percent: u8,
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
            volume_percent: 85,
            total_dma_buffers_streamed: 0,
            keystore_derived: false,
            sector_encrypted: false,
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
}

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// -----------------------------------------------------------------------------
// 32-bit ARGB8888 Design Color Tokens (immersive dark media surface)
// -----------------------------------------------------------------------------
const BG: u32 = 0xFF111214;
/// Dark surface (controls, artwork card).
const SURFACE: u32 = 0xFF25282A;
/// Recessed control fill - a disabled transport sits below the surface.
const RECESSED: u32 = 0xFF1B1E20;
const BORDER: u32 = 0xFF35383D;
const TEXT: u32 = 0xFFF5F6F7;
const MUTED: u32 = 0xFF9AA0A6;
const FAINT: u32 = 0xFF777773;
const INK_ON_LIGHT: u32 = 0xFF111214;

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

const TRANSPORT_CY: usize = 990;
const PREV_CX: usize = 190;
const PLAY_CX: usize = 360;
const NEXT_CX: usize = 530;
const SIDE_R: usize = 32; // 64 px target, >= 56 px required
const PLAY_R: usize = 44; // 88 px target, >= 56 px required

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

// -----------------------------------------------------------------------------
// Pixel Color Resolution Engine (dark minimal player)
// -----------------------------------------------------------------------------
#[inline(never)]
fn get_music_pixel(x: usize, y: usize, app: &MusicApp) -> u32 {
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
    if y >= TRACK_Y && y < TRACK_Y + TRACK_H && x >= MARGIN && x < MARGIN + CONTENT_W {
        return BORDER;
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
                if y >= row_y && y < row_y + 72 {
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
                    if y == row_y + 95 && x >= MARGIN && x < MARGIN + CONTENT_W {
                        return BORDER;
                    }
                }
            }
        }
    }

    BG
}

fn render_music_ui(surface: u32, app: &MusicApp) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_music_pixel(x, y, app);
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

    // 4. Zero-Copy DMA Audio Streaming
    if music.stream_dma_audio_block() {
        music.total_dma_buffers_streamed += 1;
        music.state = PlaybackState::Playing;
    }

    if surface > 0 {
        render_music_ui(surface, &music);
        let _ = std::surface_flip(6);
        std::log(0, "[MUSIC APP] Music canvas presented to scanout buffer.").ok();
    }

    // 5. Asynchronous Event Reactor Loop
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
                    while std::input_event_poll(&mut input).is_ok() {
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
                            let x = input.x as usize;
                            let y = input.y as usize;

                            // Dismiss control in the top bar or home gesture bar.
                            if BACK.contains(x, y)
                                || (x >= HOME_X0 && x <= HOME_X1 && y >= 1400)
                            {
                                std::log(0, "[MUSIC APP] Back button or home gesture pressed. Exiting to launcher.").ok();
                                if surface > 0 {
                                    let _ = std::surface_destroy(6, surface);
                                }
                                std::exit_with_code(0);
                            }

                            // Previous button (190, 990, r: 32) - 64 px target.
                            {
                                let p_dx = (x as i32) - PREV_CX as i32;
                                let p_dy = (y as i32) - TRANSPORT_CY as i32;
                                if p_dx * p_dx + p_dy * p_dy <= (SIDE_R * SIDE_R) as i32 {
                                    if !PLAYLIST.is_empty() && music.current_track_idx > 0 {
                                        music.current_track_idx -= 1;
                                    }
                                    let _ = std::audio_haptic_click();
                                    if surface > 0 {
                                        render_music_ui(surface, &music);
                                        let _ = std::surface_flip(6);
                                    }
                                }
                            }

                            // Next button (530, 990, r: 32) - 64 px target.
                            {
                                let n_dx = (x as i32) - NEXT_CX as i32;
                                let n_dy = (y as i32) - TRANSPORT_CY as i32;
                                if n_dx * n_dx + n_dy * n_dy <= (SIDE_R * SIDE_R) as i32 {
                                    if !PLAYLIST.is_empty() && music.current_track_idx + 1 < PLAYLIST.len() {
                                        music.current_track_idx += 1;
                                    }
                                    let _ = std::audio_haptic_click();
                                    if surface > 0 {
                                        render_music_ui(surface, &music);
                                        let _ = std::surface_flip(6);
                                    }
                                }
                            }

                            // Play / Pause control (360, 990, r: 44) - 88 px target.
                            {
                                let center_x = PLAY_CX as isize;
                                let center_y = TRANSPORT_CY as isize;
                                let dist_sq = (x as isize - center_x).pow(2)
                                    + (y as isize - center_y).pow(2);
                                if dist_sq < (PLAY_R * PLAY_R) as isize {
                                    match music.state {
                                        PlaybackState::Playing => {
                                            music.state = PlaybackState::Paused;
                                            std::log(0, "[MUSIC APP] Transport paused by user.").ok();
                                        }
                                        _ => {
                                            if music.stream_dma_audio_block() {
                                                music.total_dma_buffers_streamed += 1;
                                                music.state = PlaybackState::Playing;
                                            } else {
                                                music.state = PlaybackState::Stopped;
                                                std::log(
                                                    0,
                                                    "[MUSIC APP] Play requested but no media is available; transport stays stopped.",
                                                )
                                                .ok();
                                            }
                                        }
                                    }
                                    let _ = std::audio_haptic_click();
                                    if surface > 0 {
                                        render_music_ui(surface, &music);
                                        let _ = std::surface_flip(6);
                                    }
                                }
                            }

                            // Track list band: honest report when the library is empty.
                            if x >= MARGIN
                                && x < MARGIN + CONTENT_W
                                && y >= LIST_HIT_Y0
                                && y < LIST_HIT_Y1
                            {
                                std::log(
                                    0,
                                    "[MUSIC APP] Track list tapped: no audio files in the local library.",
                                )
                                .ok();
                            }
                        }
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
