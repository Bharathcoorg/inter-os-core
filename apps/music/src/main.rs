//! InterOS Core Native Music Player (Ring-3)
//!
//! Bare-metal user-mode media player:
//! - Static playlist populated only from real media storage (currently empty).
//! - Hardware Keystore Slot 1 media key derivation.
//! - Authenticated track record encryption.
//! - Zero-copy DMA audio block streaming.
//! - 4-note major arpeggio synthesis via hardware audio timer.
//! - Capacitive tactile haptics.
//! - Asynchronous event reactor loop for transport controls.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

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

/// Local Music Playlist.
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
// 32-bit ARGB8888 Design Color Tokens (immersive dark surface)
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFF111214;
#[allow(dead_code)]
const SURFACE_RAISED: u32 = 0xFF25282A;
const BORDER_SUBTLE: u32 = 0xFF3A3D40;
const TEXT_PRIMARY: u32 = 0xFFF7F6F2;
const TEXT_MUTED: u32 = 0xFF777773;
#[allow(dead_code)]
const ACCENT: u32 = TEXT_PRIMARY;

// -----------------------------------------------------------------------------
// Embedded 8x8 ASCII Font (Characters 32 ' ' through 126 '~')
// -----------------------------------------------------------------------------
const FONT_8X8: [[u8; 8]; 95] = [
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // 32 ' '
    [0x18, 0x18, 0x18, 0x18, 0x00, 0x00, 0x18, 0x00], // 33 '!'
    [0x66, 0x66, 0x24, 0x00, 0x00, 0x00, 0x00, 0x00], // 34 '"'
    [0x6c, 0x6c, 0xfe, 0x6c, 0xfe, 0x6c, 0x6c, 0x00], // 35 '#'
    [0x18, 0x3e, 0x60, 0x3c, 0x06, 0x7c, 0x18, 0x00], // 36 '$'
    [0x00, 0xc6, 0xcc, 0x18, 0x30, 0x66, 0xc6, 0x00], // 37 '%'
    [0x38, 0x6c, 0x38, 0x76, 0xdc, 0xcc, 0x76, 0x00], // 38 '&'
    [0x18, 0x18, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00], // 39 '''
    [0x0c, 0x18, 0x30, 0x30, 0x30, 0x18, 0x0c, 0x00], // 40 '('
    [0x30, 0x18, 0x0c, 0x0c, 0x0c, 0x18, 0x30, 0x00], // 41 ')'
    [0x00, 0x66, 0x3c, 0xff, 0x3c, 0x66, 0x00, 0x00], // 42 '*'
    [0x00, 0x18, 0x18, 0x7e, 0x18, 0x18, 0x00, 0x00], // 43 '+'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x30], // 44 ','
    [0x00, 0x00, 0x00, 0x7e, 0x00, 0x00, 0x00, 0x00], // 45 '-'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00], // 46 '.'
    [0x06, 0x0c, 0x18, 0x30, 0x60, 0xc0, 0x80, 0x00], // 47 '/'
    [0x7c, 0xc6, 0xce, 0xd6, 0xe6, 0xc6, 0x7c, 0x00], // 48 '0'
    [0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x7e, 0x00], // 49 '1'
    [0x7c, 0xc6, 0x06, 0x1c, 0x30, 0x66, 0xfe, 0x00], // 50 '2'
    [0x7c, 0xc6, 0x06, 0x3c, 0x06, 0xc6, 0x7c, 0x00], // 51 '3'
    [0x1c, 0x3c, 0x6c, 0xcc, 0xfe, 0x0c, 0x1e, 0x00], // 52 '4'
    [0xfe, 0xc0, 0xfc, 0x06, 0x06, 0xc6, 0x7c, 0x00], // 53 '5'
    [0x78, 0x0c, 0xc0, 0xfc, 0xc6, 0xc6, 0x7c, 0x00], // 54 '6'
    [0xfe, 0xc6, 0x0c, 0x18, 0x30, 0x30, 0x30, 0x00], // 55 '7'
    [0x7c, 0xc6, 0xc6, 0x7c, 0xc6, 0xc6, 0x7c, 0x00], // 56 '8'
    [0x7c, 0xc6, 0xc6, 0x7e, 0x06, 0x0c, 0x78, 0x00], // 57 '9'
    [0x00, 0x18, 0x18, 0x00, 0x18, 0x18, 0x00, 0x00], // 58 ':'
    [0x00, 0x18, 0x18, 0x00, 0x18, 0x18, 0x30, 0x00], // 59 ';'
    [0x06, 0x0c, 0x18, 0x30, 0x18, 0x0c, 0x06, 0x00], // 60 '<'
    [0x00, 0x7e, 0x00, 0x7e, 0x00, 0x00, 0x00, 0x00], // 61 '='
    [0x60, 0x30, 0x18, 0x0c, 0x18, 0x30, 0x60, 0x00], // 62 '>'
    [0x7c, 0xc6, 0x0c, 0x18, 0x18, 0x00, 0x18, 0x00], // 63 '?'
    [0x7c, 0xc6, 0xde, 0xde, 0xdc, 0xc0, 0x7c, 0x00], // 64 '@'
    [0x38, 0x6c, 0xc6, 0xfe, 0xc6, 0xc6, 0xc6, 0x00], // 65 'A'
    [0xfc, 0x66, 0x66, 0x7c, 0x66, 0x66, 0xfc, 0x00], // 66 'B'
    [0x3c, 0x66, 0xc0, 0xc0, 0xc0, 0x66, 0x3c, 0x00], // 67 'C'
    [0xf8, 0x6c, 0x66, 0x66, 0x66, 0x6c, 0xf8, 0x00], // 68 'D'
    [0xfe, 0x62, 0x68, 0x78, 0x68, 0x62, 0xfe, 0x00], // 69 'E'
    [0xfe, 0x62, 0x68, 0x78, 0x68, 0x60, 0xf0, 0x00], // 70 'F'
    [0x3c, 0x66, 0xc0, 0xc0, 0xce, 0x66, 0x3e, 0x00], // 71 'G'
    [0xc6, 0xc6, 0xc6, 0xfe, 0xc6, 0xc6, 0xc6, 0x00], // 72 'H'
    [0x7e, 0x18, 0x18, 0x18, 0x18, 0x18, 0x7e, 0x00], // 73 'I'
    [0x1e, 0x0c, 0x0c, 0x0c, 0xcc, 0xcc, 0x78, 0x00], // 74 'J'
    [0xe6, 0x66, 0x6c, 0x78, 0x6c, 0x66, 0xe6, 0x00], // 75 'K'
    [0xf0, 0x60, 0x60, 0x60, 0x62, 0x66, 0xfe, 0x00], // 76 'L'
    [0xc6, 0xee, 0xfe, 0xfe, 0xd6, 0xc6, 0xc6, 0x00], // 77 'M'
    [0xc6, 0xe6, 0xf6, 0xde, 0xce, 0xc6, 0xc6, 0x00], // 78 'N'
    [0x7c, 0xc6, 0xc6, 0xc6, 0xc6, 0xc6, 0x7c, 0x00], // 79 'O'
    [0xfc, 0x66, 0x66, 0x7c, 0x60, 0x60, 0xf0, 0x00], // 80 'P'
    [0x7c, 0xc6, 0xc6, 0xc6, 0xd6, 0xcc, 0x76, 0x00], // 81 'Q'
    [0xfc, 0x66, 0x66, 0x7c, 0x6c, 0x66, 0xe6, 0x00], // 82 'R'
    [0x7c, 0xc6, 0x60, 0x38, 0x0c, 0xc6, 0x7c, 0x00], // 83 'S'
    [0x7e, 0x7e, 0x5a, 0x18, 0x18, 0x18, 0x3c, 0x00], // 84 'T'
    [0xc6, 0xc6, 0xc6, 0xc6, 0xc6, 0xc6, 0x7c, 0x00], // 85 'U'
    [0xc6, 0xc6, 0xc6, 0xc6, 0x6c, 0x38, 0x10, 0x00], // 86 'V'
    [0xc6, 0xc6, 0xd6, 0xfe, 0xfe, 0xee, 0xc6, 0x00], // 87 'W'
    [0xc6, 0x6c, 0x38, 0x38, 0x6c, 0xc6, 0xc6, 0x00], // 88 'X'
    [0x66, 0x66, 0x66, 0x3c, 0x18, 0x18, 0x3c, 0x00], // 89 'Y'
    [0xfe, 0xc6, 0x0c, 0x18, 0x30, 0x62, 0xfe, 0x00], // 90 'Z'
    [0x3c, 0x30, 0x30, 0x30, 0x30, 0x30, 0x3c, 0x00], // 91 '['
    [0xc0, 0x60, 0x30, 0x18, 0x0c, 0x06, 0x02, 0x00], // 92 '\'
    [0x3c, 0x0c, 0x0c, 0x0c, 0x0c, 0x0c, 0x3c, 0x00], // 93 ']'
    [0x10, 0x38, 0x6c, 0xc6, 0x00, 0x00, 0x00, 0x00], // 94 '^'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff], // 95 '_'
    [0x30, 0x18, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00], // 96 '`'
    [0x00, 0x00, 0x7c, 0x06, 0x7e, 0xc6, 0x7e, 0x00], // 97 'a'
    [0xe0, 0x60, 0x7c, 0x66, 0x66, 0x66, 0xdc, 0x00], // 98 'b'
    [0x00, 0x00, 0x7c, 0xc6, 0xc0, 0xc6, 0x7c, 0x00], // 99 'c'
    [0x1c, 0x0c, 0x7c, 0xcc, 0xcc, 0xcc, 0x76, 0x00], // 100 'd'
    [0x00, 0x00, 0x7c, 0xc6, 0xfe, 0xc0, 0x7c, 0x00], // 101 'e'
    [0x38, 0x6c, 0x60, 0xf0, 0x60, 0x60, 0xf0, 0x00], // 102 'f'
    [0x00, 0x00, 0x76, 0xcc, 0xcc, 0x7c, 0x0c, 0xf8], // 103 'g'
    [0xe0, 0x60, 0x6c, 0x76, 0x66, 0x66, 0xe6, 0x00], // 104 'h'
    [0x18, 0x00, 0x38, 0x18, 0x18, 0x18, 0x3c, 0x00], // 105 'i'
    [0x06, 0x00, 0x0e, 0x06, 0x06, 0x66, 0x66, 0x3c], // 106 'j'
    [0xe0, 0x60, 0x66, 0x6c, 0x78, 0x6c, 0xe6, 0x00], // 107 'k'
    [0x38, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3c, 0x00], // 108 'l'
    [0x00, 0x00, 0xe6, 0xff, 0xdb, 0xc9, 0xc9, 0x00], // 109 'm'
    [0x00, 0x00, 0xdc, 0x66, 0x66, 0x66, 0x66, 0x00], // 110 'n'
    [0x00, 0x00, 0x7c, 0xc6, 0xc6, 0xc6, 0x7c, 0x00], // 111 'o'
    [0x00, 0x00, 0xdc, 0x66, 0x66, 0x7c, 0x60, 0xf0], // 112 'p'
    [0x00, 0x00, 0x76, 0xcc, 0xcc, 0x7c, 0x0c, 0x1e], // 113 'q'
    [0x00, 0x00, 0xdc, 0x66, 0x60, 0x60, 0xf0, 0x00], // 114 'r'
    [0x00, 0x00, 0x7c, 0xc0, 0x7c, 0x06, 0xfc, 0x00], // 115 's'
    [0x30, 0x30, 0xfc, 0x30, 0x30, 0x34, 0x18, 0x00], // 116 't'
    [0x00, 0x00, 0xcc, 0xcc, 0xcc, 0xcc, 0x76, 0x00], // 117 'u'
    [0x00, 0x00, 0xc6, 0xc6, 0xc6, 0x6c, 0x38, 0x00], // 118 'v'
    [0x00, 0x00, 0xc6, 0xd6, 0xfe, 0xfe, 0x6c, 0x00], // 119 'w'
    [0x00, 0x00, 0xc6, 0x6c, 0x38, 0x6c, 0xc6, 0x00], // 120 'x'
    [0x00, 0x00, 0xc6, 0xc6, 0xc6, 0x7e, 0x06, 0xfc], // 121 'y'
    [0x00, 0x00, 0x7e, 0x4c, 0x18, 0x32, 0x7e, 0x00], // 122 'z'
    [0x0e, 0x18, 0x18, 0x70, 0x18, 0x18, 0x0e, 0x00], // 123 '{'
    [0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00], // 124 '|'
    [0x70, 0x18, 0x18, 0x0e, 0x18, 0x18, 0x70, 0x00], // 125 '}'
    [0x76, 0xdc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // 126 '~'
];

#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
) -> Option<u32> {
    let char_w = 8 * scale;
    let char_h = 8 * scale;
    let total_w = text.len() * char_w;
    if y >= y0 && y < y0 + char_h && x >= x0 && x < x0 + total_w {
        let char_idx = (x - x0) / char_w;
        let c = text[char_idx];
        if (32..=126).contains(&c) {
            let glyph_idx = (c - 32) as usize;
            let col = ((x - x0) % char_w) / scale;
            let row = (y - y0) / scale;
            let row_byte = FONT_8X8[glyph_idx][row];
            if ((row_byte >> (7 - col)) & 1) == 1 {
                return Some(color);
            }
        }
    }
    None
}

// -----------------------------------------------------------------------------
// Pixel Color Resolution Engine (light minimal player)
// -----------------------------------------------------------------------------
#[inline(never)]
fn get_music_pixel(x: usize, y: usize, app: &MusicApp) -> u32 {
    // 0. Home Gesture Pill (x: 260..460, y: 1418..1424) — checked early, no overlap.
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    // 1. Top bar (y: 0..120): dismiss chevron + queue icon.
    if y < 120 {
        // Dismiss down-chevron "v" at x56 y64, TEXT_PRIMARY two diagonal strokes.
        if y >= 56 && y < 76 {
            let yi = y as i32;
            let xi = x as i32;
            if xi >= 46 && xi <= 56 {
                let yy = yi - 56;
                let xx = xi - 46;
                if yy >= xx && yy < xx + 4 {
                    return TEXT_PRIMARY;
                }
            }
            if xi >= 56 && xi <= 66 {
                let yy = yi - 56;
                let xx = 66 - xi;
                if yy >= xx && yy < xx + 4 {
                    return TEXT_PRIMARY;
                }
            }
        }
        // Queue icon: 3 horizontal lines at x654 y64, TEXT_PRIMARY.
        if x >= 644 && x < 665 {
            if (y >= 55 && y < 58) || (y >= 62 && y < 65) || (y >= 69 && y < 72) {
                return TEXT_PRIMARY;
            }
        }
        return BG_DARK;
    }

    // 2. Album card x72..648 y140..716 radius32, decorative monochrome shading.
    if y >= 140 && y < 716 && x >= 72 && x < 648 {
        let xi = x as i32;
        let yi = y as i32;
        // Rounded-corner outside test.
        if xi < 104 && yi < 172 {
            let dx = xi - 104;
            let dy = yi - 172;
            let d2 = dx * dx + dy * dy;
            if d2 > 32 * 32 {
                return BG_DARK;
            }
            if d2 >= 30 * 30 {
                return BORDER_SUBTLE;
            }
        } else if xi >= 616 && yi < 172 {
            let dx = xi - 616;
            let dy = yi - 172;
            let d2 = dx * dx + dy * dy;
            if d2 > 32 * 32 {
                return BG_DARK;
            }
            if d2 >= 30 * 30 {
                return BORDER_SUBTLE;
            }
        } else if xi < 104 && yi >= 684 {
            let dx = xi - 104;
            let dy = yi - 684;
            let d2 = dx * dx + dy * dy;
            if d2 > 32 * 32 {
                return BG_DARK;
            }
            if d2 >= 30 * 30 {
                return BORDER_SUBTLE;
            }
        } else if xi >= 616 && yi >= 684 {
            let dx = xi - 616;
            let dy = yi - 684;
            let d2 = dx * dx + dy * dy;
            if d2 > 32 * 32 {
                return BG_DARK;
            }
            if d2 >= 30 * 30 {
                return BORDER_SUBTLE;
            }
        } else if x < 74 || x >= 646 || y < 142 || y >= 714 {
            return BORDER_SUBTLE;
        }

        // Layered horizontal shade bands (light top -> dark bottom).
        let mut base: u32 = if y < 222 {
            0xFFE8E6E1
        } else if y < 304 {
            0xFFD5D3CD
        } else if y < 386 {
            0xFFB8B6B1
        } else if y < 468 {
            0xFF8E8C88
        } else if y < 550 {
            0xFF5E5C5A
        } else if y < 632 {
            0xFF3E3C3B
        } else {
            0xFF2A2A2A
        };

        // Vignette darkening at edges.
        let ex0 = x - 72;
        let ex1 = 647 - x;
        let ey0 = y - 140;
        let ey1 = 715 - y;
        let mut edge = ex0;
        if ex1 < edge {
            edge = ex1;
        }
        if ey0 < edge {
            edge = ey0;
        }
        if ey1 < edge {
            edge = ey1;
        }
        if edge < 32 {
            let factor = (150 + edge * 3) as u32;
            let rr = ((base >> 16) & 0xFF) * factor / 255;
            let gg = ((base >> 8) & 0xFF) * factor / 255;
            let bb = (base & 0xFF) * factor / 255;
            base = 0xFF000000 | (rr << 16) | (gg << 8) | bb;
        }

        // Diagonal ridge / wave silhouettes (decorative shading only).
        let ridge1 = 300 + (xi - 72) * 120 / 576;
        if (yi - ridge1).abs() < 5 {
            return 0xFF2A2A2A;
        }
        let ridge2 = 460 + (xi - 72) * 100 / 576;
        if (yi - ridge2).abs() < 4 {
            return TEXT_PRIMARY;
        }
        let diag = (xi + yi * 2) % 110;
        let diag_norm = if diag < 0 { diag + 110 } else { diag };
        if diag_norm < 3 {
            return 0xFF2A2A2A;
        }

        // Star dots: sparse white pixels.
        if (x * 7 + y * 13) % 97 == 0 {
            return 0xFFFFFFFF;
        }

        return base;
    }

    // 3. Metadata: title "Interstellar" + artist "Hans Zimmer".
    if y >= 755 && y < 755 + 24 {
        if let Some(c) = test_text(x, y, 72, 755, 3, b"Interstellar", TEXT_PRIMARY) {
            return c;
        }
    }
    if y >= 795 && y < 795 + 16 {
        if let Some(c) = test_text(x, y, 72, 795, 2, b"Hans Zimmer", TEXT_MUTED) {
            return c;
        }
    }

    // 4. Scrubber y860..910: track x72..648 y870 height4 + knob + times.
    if y >= 870 && y < 874 && x >= 72 && x < 648 {
        let progress_end = 72 + (648 - 72) * 40 / 100;
        if x < progress_end {
            return TEXT_PRIMARY;
        }
        return BORDER_SUBTLE;
    }
    // Knob circle radius6 at progress end (302, 872).
    {
        let progress_end = (72 + (648 - 72) * 40 / 100) as i32;
        let kdx = x as i32 - progress_end;
        let kdy = y as i32 - 872;
        if kdx * kdx + kdy * kdy <= 36 {
            return TEXT_PRIMARY;
        }
    }
    if y >= 890 && y < 898 {
        if let Some(c) = test_text(x, y, 72, 890, 1, b"0:00", TEXT_MUTED) {
            return c;
        }
        if let Some(c) = test_text(x, y, 570, 890, 1, b"-04:32", TEXT_MUTED) {
            return c;
        }
    }

    // 5. Transport controls y930..1060.
    // Previous button center (190, 990) r32: bar + left triangle.
    {
        let pdx = x as i32 - 190;
        let pdy = y as i32 - 990;
        let pd2 = pdx * pdx + pdy * pdy;
        if pd2 <= 32 * 32 {
            if pd2 >= 30 * 30 {
                return BORDER_SUBTLE;
            }
            if pdx >= -18 && pdx <= -14 && pdy.abs() <= 10 {
                return TEXT_PRIMARY;
            }
            if pdx >= -8 && pdx <= 12 {
                let allow = (pdx + 8) * 12 / 20;
                if pdy.abs() <= allow {
                    return TEXT_PRIMARY;
                }
            }
            return SURFACE_RAISED;
        }
    }
    // Play button center (360, 990) radius ~44: solid fill + inner glyph.
    {
        let cdx = x as i32 - 360;
        let cdy = y as i32 - 990;
        let cd2 = cdx * cdx + cdy * cdy;
        if cd2 <= 44 * 44 {
            if app.state == PlaybackState::Playing {
                if ((cdx >= -12 && cdx <= -4) || (cdx >= 4 && cdx <= 12))
                    && cdy.abs() <= 14
                {
                    return BG_DARK;
                }
                return TEXT_PRIMARY;
            }
            if cdx >= -14 && cdx <= 16 {
                let allow = (16 - cdx) * 18 / 30;
                if cdy.abs() <= allow {
                    return BG_DARK;
                }
            }
            return TEXT_PRIMARY;
        }
    }
    // Next button center (530, 990) r32: right triangle + bar.
    {
        let ndx = x as i32 - 530;
        let ndy = y as i32 - 990;
        let nd2 = ndx * ndx + ndy * ndy;
        if nd2 <= 32 * 32 {
            if nd2 >= 30 * 30 {
                return BORDER_SUBTLE;
            }
            if ndx >= 14 && ndx <= 18 && ndy.abs() <= 10 {
                return TEXT_PRIMARY;
            }
            if ndx >= -12 && ndx <= 8 {
                let allow = (8 - ndx) * 12 / 20;
                // Mirror of prev: tip at +8, base at -12.
                // At ndx=8 allow=0, at ndx=-12 allow=12.
                if ndy.abs() <= allow {
                    return TEXT_PRIMARY;
                }
            }
            return SURFACE_RAISED;
        }
    }

    // 6. Secondary row y1080..1160.
    // Shuffle glyph at x140: X shape (two crossing strokes).
    if x >= 118 && x < 162 && y >= 1098 && y < 1142 {
        let sx = x as i32 - 140;
        let sy = y as i32 - 1120;
        if (sx - sy).abs() <= 2 || (sx + sy).abs() <= 2 {
            return TEXT_MUTED;
        }
    }
    // Heart at x360: two circles + triangle approx.
    {
        let h1dx = x as i32 - 350;
        let h1dy = y as i32 - 1112;
        if h1dx * h1dx + h1dy * h1dy <= 100 {
            return TEXT_MUTED;
        }
        let h2dx = x as i32 - 370;
        let h2dy = y as i32 - 1112;
        if h2dx * h2dx + h2dy * h2dy <= 100 {
            return TEXT_MUTED;
        }
        if y >= 1118 && y < 1138 {
            let half = 1138 - y as i32;
            if (x as i32 - 360).abs() <= half {
                return TEXT_MUTED;
            }
        }
    }
    // "..." at x580: three dots.
    {
        let d1x = x as i32 - 566;
        let d1y = y as i32 - 1120;
        if d1x * d1x + d1y * d1y <= 16 {
            return TEXT_MUTED;
        }
        let d2x = x as i32 - 580;
        let d2y = y as i32 - 1120;
        if d2x * d2x + d2y * d2y <= 16 {
            return TEXT_MUTED;
        }
        let d3x = x as i32 - 594;
        let d3y = y as i32 - 1120;
        if d3x * d3x + d3y * d3y <= 16 {
            return TEXT_MUTED;
        }
    }

    BG_DARK
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

                            // Dismiss chevron in top bar or home gesture bar: exit to launcher.
                            if (x >= 24 && x < 100 && y >= 28 && y < 100)
                                || (x >= 240 && x <= 480 && y >= 1400)
                            {
                                std::log(0, "[MUSIC APP] Back button or home gesture pressed. Exiting to launcher.").ok();
                                if surface > 0 {
                                    let _ = std::surface_destroy(6, surface);
                                }
                                std::exit_with_code(0);
                            }

                            // Previous button (190, 990, r: 32)
                            {
                                let p_dx = (x as i32) - 190;
                                let p_dy = (y as i32) - 990;
                                if p_dx * p_dx + p_dy * p_dy <= 1024 {
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

                            // Next button (530, 990, r: 32)
                            {
                                let n_dx = (x as i32) - 530;
                                let n_dy = (y as i32) - 990;
                                if n_dx * n_dx + n_dy * n_dy <= 1024 {
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

                            // Play / Pause center button (360, 990, r: 44)
                            {
                                let center_x = 360isize;
                                let center_y = 990isize;
                                let dist_sq = (x as isize - center_x).pow(2)
                                    + (y as isize - center_y).pow(2);
                                if dist_sq < 44 * 44 {
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
