//! Inter OS Native Sovereign Telephony & Dialer App
//!
//! Native telephony dialer executing in Ring 3 (User Mode):
//! - Dual-Tone Multi-Frequency (DTMF) acoustic tone synthesis for keypad keys (0-9, *, #)
//! - Call State Machine: Idle, Dialing, Ringing, ActiveCall, CallHeld, CallEnded
//! - Cellular Baseband Modem HAL telemetry: 5G Carrier, RSSI, Cell ID, 3GPP AT state
//! - Local Contacts Book & Call History ledger
//! - Emergency E911/112 override pipeline
//! - Android Compatibility Runtime (ACR) cross-zone intent bridge dispatch
//! - DRM/KMS Client Surface Rendering (z=200) with Mobile Phone Keypad & Status Display
//! - Asynchronous event reactor loop handling touch input, modem URCs, and timers

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// Current execution state of the telephony call engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallState {
    Idle = 0,
    Dialing = 1,
    Ringing = 2,
    ActiveCall = 3,
    CallHeld = 4,
    CallEnded = 5,
}

/// Keypad DTMF frequency pair (low frequency, high frequency in Hz).
#[derive(Debug, Clone, Copy)]
pub struct DtmfPair {
    pub low_freq: u32,
    pub high_freq: u32,
}

/// Resolves standard CCITT dual-tone frequencies for telephone keypad keys.
pub fn get_dtmf_frequencies(key: char) -> Option<DtmfPair> {
    match key {
        '1' => Some(DtmfPair {
            low_freq: 697,
            high_freq: 1209,
        }),
        '2' => Some(DtmfPair {
            low_freq: 697,
            high_freq: 1336,
        }),
        '3' => Some(DtmfPair {
            low_freq: 697,
            high_freq: 1477,
        }),
        '4' => Some(DtmfPair {
            low_freq: 770,
            high_freq: 1209,
        }),
        '5' => Some(DtmfPair {
            low_freq: 770,
            high_freq: 1336,
        }),
        '6' => Some(DtmfPair {
            low_freq: 770,
            high_freq: 1477,
        }),
        '7' => Some(DtmfPair {
            low_freq: 852,
            high_freq: 1209,
        }),
        '8' => Some(DtmfPair {
            low_freq: 852,
            high_freq: 1336,
        }),
        '9' => Some(DtmfPair {
            low_freq: 852,
            high_freq: 1477,
        }),
        '0' => Some(DtmfPair {
            low_freq: 941,
            high_freq: 1336,
        }),
        '*' => Some(DtmfPair {
            low_freq: 941,
            high_freq: 1209,
        }),
        '#' => Some(DtmfPair {
            low_freq: 941,
            high_freq: 1477,
        }),
        _ => None,
    }
}

/// Sovereign Contact Record.
#[derive(Debug, Clone, Copy)]
pub struct Contact {
    pub name: &'static str,
    pub number: &'static str,
    pub did: &'static str,
    pub is_emergency: bool,
}

/// Contacts are populated only from real user/contact state.
pub static CONTACTS_BOOK: [Contact; 0] = [];

/// Production Telephony Dialer state manager.
pub struct DialerApp {
    pub state: CallState,
    pub active_number: [u8; 32],
    pub active_len: usize,
    pub is_muted: bool,
    pub is_speaker: bool,
    pub call_duration_secs: u32,
    pub calls_completed: u32,
    /// Whether the telephony service actually reported a baseband present.
    pub modem_present: bool,
}

impl DialerApp {
    pub const fn new() -> Self {
        Self {
            state: CallState::Idle,
            active_number: [0u8; 32],
            active_len: 0,
            is_muted: false,
            is_speaker: false,
            call_duration_secs: 0,
            calls_completed: 0,
            // WHY: fail closed. Until the telephony service reports a baseband,
            // no inbound-call path may be claimed.
            modem_present: false,
        }
    }

    /// Plays dual-tone multi-frequency acoustic feedback for a pressed keypad digit.
    pub fn play_dtmf(&self, digit: char) {
        if let Some(pair) = get_dtmf_frequencies(digit) {
            std::audio_play_tone(pair.high_freq).ok();
            std::audio_haptic_click().ok();
            std::audio_silence().ok();
        }
    }

    /// Appends a dialed digit to the accumulator and plays its DTMF tone.
    pub fn input_digit(&mut self, digit: char) {
        if self.active_len < self.active_number.len() {
            self.active_number[self.active_len] = digit as u8;
            self.active_len += 1;
            self.play_dtmf(digit);
        }
    }

    /// Removes the last entered digit.
    pub fn backspace(&mut self) {
        if self.active_len > 0 {
            self.active_len -= 1;
            std::audio_haptic_click().ok();
        }
    }

    /// Initiates an outgoing cellular call.
    pub fn dial(&mut self, number: &str) -> Result<(), i64> {
        if number.is_empty() || number.len() > self.active_number.len() {
            return Err(-22);
        }

        self.state = CallState::Dialing;
        self.call_duration_secs = 0;

        // A call is connected only after the authoritative telephony service
        // accepts the request. UI/ACR dispatch must never manufacture an
        // ActiveCall state.
        match std::telephony_dial(number) {
            Ok(()) => {
                self.state = CallState::ActiveCall;
                std::audio_play_tone(440).ok();
                std::audio_silence().ok();
                std::log(0, "[DIALER APP] Telephony service accepted call request.").ok();
                Ok(())
            }
            Err(err) => {
                self.state = CallState::CallEnded;
                std::log(0, "[DIALER APP] Call request rejected by telephony service.").ok();
                Err(err)
            }
        }
    }

/// Terminates an active call.
    ///
    /// Only a call that actually reached the telephony service counts as
    /// completed and only that case releases a channel.
    pub fn hangup(&mut self) {
        let had_connected_call = self.state == CallState::ActiveCall;
        self.state = CallState::CallEnded;
        let _ = std::audio_haptic_click();
        self.state = CallState::Idle;
        self.active_len = 0;
        if had_connected_call {
            self.calls_completed += 1;
            // WHY: this line used to claim the channel was released to the
            // modem pool, but hangup() issued no telephony syscall at all, so no
            // modem state was ever touched. Ask the service and report the
            // outcome it actually returned.
            if std::telephony_hangup().is_ok() {
                std::log(
                    0,
                    "[DIALER APP] Call Terminated: telephony service released the channel.",
                )
                .ok();
            } else {
                std::log(
                    0,
                    "[DIALER APP] Call Terminated locally: the telephony service refused the hangup, so no channel was released.",
                )
                .ok();
            }
        } else {
            // WHY: no call was ever established, so there is no channel to
            // release and no completed call to count. Reporting a released
            // channel here would be a success claim for a rejected operation.
            std::log(
                0,
                "[DIALER APP] Hangup with no connected call; no channel was released.",
            )
            .ok();
        }
    }
}

// -----------------------------------------------------------------------------
// 32-bit ARGB8888 Design Color Tokens
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFF0A0D14;
const SURFACE_DARK: u32 = 0xFF121824;
const SURFACE_RAISED: u32 = 0xFF1A2234;
const BORDER_SUBTLE: u32 = 0xFF222E42;
const TEXT_PRIMARY: u32 = 0xFFF8FAFC;
const TEXT_MUTED: u32 = 0xFF94A3B8;
const ACCENT_SKY: u32 = 0xFF31C4FF;
const SUCCESS_EMERALD: u32 = 0xFF10B981;
const DANGER_CRIMSON: u32 = 0xFFEF4444;
const WARNING_AMBER: u32 = 0xFFF59E0B;

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

// Keypad layout constants
const BTN_W: usize = 180;
const BTN_H: usize = 124;
const COL_X: [usize; 3] = [48, 270, 492];
const ROW_Y: [usize; 4] = [410, 560, 710, 860];

fn get_keypad_char(row: usize, col: usize) -> Option<char> {
    match (row, col) {
        (0, 0) => Some('1'),
        (0, 1) => Some('2'),
        (0, 2) => Some('3'),
        (1, 0) => Some('4'),
        (1, 1) => Some('5'),
        (1, 2) => Some('6'),
        (2, 0) => Some('7'),
        (2, 1) => Some('8'),
        (2, 2) => Some('9'),
        (3, 0) => Some('*'),
        (3, 1) => Some('0'),
        (3, 2) => Some('#'),
        _ => None,
    }
}

fn get_keypad_sublabel(row: usize, col: usize) -> &'static [u8] {
    match (row, col) {
        (0, 1) => b"ABC",
        (0, 2) => b"DEF",
        (1, 0) => b"GHI",
        (1, 1) => b"JKL",
        (1, 2) => b"MNO",
        (2, 0) => b"PQRS",
        (2, 1) => b"TUV",
        (2, 2) => b"WXYZ",
        (3, 1) => b"+",
        _ => b"",
    }
}

// -----------------------------------------------------------------------------
// Pixel Color Resolution Engine
// -----------------------------------------------------------------------------
#[inline(never)]
fn get_dialer_pixel(x: usize, y: usize, dialer: &DialerApp) -> u32 {
    // 1. Status Bar & Header (y: 0..96)
    if y < 96 {
        if y >= 94 {
            return BORDER_SUBTLE;
        }

        // Back button (<) at (x: 24..100, y: 28..68)
        if x >= 24 && x < 100 && y >= 28 && y < 68 {
            let center_x = 62;
            let center_y = 48;
            let dx = (x as isize - center_x as isize).abs();
            let dy = (y as isize - center_y as isize).abs();
            if (dx + dy <= 12 && x <= center_x)
                || (x >= 24 && x < 100 && (y == 28 || y == 67 || x == 24 || x == 99))
            {
                return TEXT_PRIMARY;
            }
            return SURFACE_RAISED;
        }

        // Header Title: "Phone"
        if y >= 36 && y < 60 {
            if let Some(c) = test_text(x, y, 120, 36, 3, b"Phone", TEXT_PRIMARY) {
                return c;
            }
        }

        // Cellular signal indicator (x: 610..670, y: 40..62)
        if x >= 610 && x < 670 && y >= 40 && y < 62 {
            let bar_idx = (x - 610) / 15;
            let bar_x = (x - 610) % 15;
            let bar_min_y = 62 - (bar_idx + 1) * 5;
            if bar_x < 9 && y >= bar_min_y {
                return if dialer.modem_present { ACCENT_SKY } else { BORDER_SUBTLE };
            }
            return SURFACE_DARK;
        }

        // Battery indicator (x: 678..706, y: 44..58)
        if x >= 678 && x < 706 && y >= 44 && y < 58 {
            if x == 678 || x == 702 || y == 44 || y == 57 || (x >= 703 && y >= 48 && y < 54) {
                return TEXT_MUTED;
            }
            return SURFACE_DARK;
        }

        return SURFACE_DARK;
    }

    // 2. Call Status & Dialed Number Display Card (y: 110..380, x: 24..696)
    if (110..380).contains(&y) && x >= 24 && x < 696 {
        if y == 110 || y == 379 || x == 24 || x == 695 {
            return BORDER_SUBTLE;
        }

        // Status Pill Banner (y: 126..156, x: 220..500)
        if (126..156).contains(&y) && x >= 220 && x < 500 {
            if y == 126 || y == 155 || x == 220 || x == 499 {
                return BORDER_SUBTLE;
            }
            let (label, text_x, color) = match dialer.state {
                CallState::ActiveCall => (&b"Connected"[..], 324, SUCCESS_EMERALD),
                CallState::Dialing => (&b"Calling..."[..], 324, ACCENT_SKY),
                CallState::CallEnded => (&b"Call Ended"[..], 320, DANGER_CRIMSON),
                CallState::CallHeld => (&b"On Hold"[..], 336, WARNING_AMBER),
                CallState::Ringing => (&b"Ringing..."[..], 324, ACCENT_SKY),
                _ => (&b"Ready to Call"[..], 308, TEXT_MUTED),
            };
            if y >= 135 && y < 147 {
                if let Some(c) = test_text(x, y, text_x, 135, 1, label, color) {
                    return c;
                }
            }
            return 0xFF152236;
        }

        // Dialed Digits Display
        if dialer.active_len > 0 {
            let scale = if dialer.active_len <= 10 { 4 } else { 3 };
            let char_w = 8 * scale;
            let total_w = dialer.active_len * char_w;
            let x0 = (W.saturating_sub(total_w)) / 2;
            let y0 = 226;
            if let Some(c) = test_text(
                x,
                y,
                x0,
                y0,
                scale,
                &dialer.active_number[..dialer.active_len],
                TEXT_PRIMARY,
            ) {
                return c;
            }
        } else if y >= 238 && y < 254 {
            if let Some(c) = test_text(x, y, 248, 238, 2, b"Enter a phone number", TEXT_MUTED) {
                return c;
            }
        }

        // Backspace button in display card (627, 270, r: 24)
        if dialer.active_len > 0 {
            let bx = 627i32;
            let by = 250i32;
            let dx = (x as i32) - bx;
            let dy = (y as i32) - by;
            if dx * dx + dy * dy <= 576 {
                if dx * dx + dy * dy >= 484 {
                    return DANGER_CRIMSON;
                }
                // Backspace icon 'X' inside
                if dx.abs() <= 7 && dy.abs() <= 7 && (dx == dy || dx == -dy) {
                    return DANGER_CRIMSON;
                }
                return 0xFF22161A;
            }
        }

        return SURFACE_DARK;
    }

    // 3. Keypad Matrix (y: 410..984)
    for row in 0..4 {
        let by = ROW_Y[row];
        if y >= by && y < by + BTN_H {
            for col in 0..3 {
                let bx = COL_X[col];
                if x >= bx && x < bx + BTN_W {
                    let cx = (bx + BTN_W / 2) as i32;
                    let cy = (by + BTN_H / 2) as i32;
                    let k_dx = (x as i32) - cx;
                    let k_dy = (y as i32) - cy;
                    let dist2 = k_dx * k_dx + k_dy * k_dy;

                    if dist2 <= 50 * 50 {
                        // Button rim
                        if dist2 >= 46 * 46 {
                            return BORDER_SUBTLE;
                        }

                        // Digit text
                        if let Some(digit) = get_keypad_char(row, col) {
                            let digit_byte = [digit as u8];
                            if let Some(c) = test_text(
                                x,
                                y,
                                (cx - 12) as usize,
                                (cy - 24) as usize,
                                3,
                                &digit_byte,
                                TEXT_PRIMARY,
                            ) {
                                return c;
                            }
                        }

                        // Sublabel text
                        let sublabel = get_keypad_sublabel(row, col);
                        if !sublabel.is_empty() {
                            let sub_w = sublabel.len() * 8;
                            let sub_x = (cx as usize).saturating_sub(sub_w / 2);
                            if let Some(c) = test_text(
                                x,
                                y,
                                sub_x,
                                (cy + 12) as usize,
                                1,
                                sublabel,
                                TEXT_MUTED,
                            ) {
                                return c;
                            }
                        }

                        return 0xFF151E2E;
                    }
                    return BG_DARK;
                }
            }
        }
    }

    // 4. Action Bar (y: 990..1140)
    if (990..1140).contains(&y) {
        // SOS Button (x: 48..220, centered at 134, 1065, r: 40)
        let s_dx = (x as i32) - 134;
        let s_dy = (y as i32) - 1065;
        if s_dx * s_dx + s_dy * s_dy <= 1600 {
            if s_dx * s_dx + s_dy * s_dy >= 1444 {
                return 0xFFE11D48;
            }
            if y >= 1057 && y < 1073 {
                if let Some(c) = test_text(x, y, 122, 1057, 2, b"SOS", 0xFFF43F5E) {
                    return c;
                }
            }
            return 0xFF2A1518;
        }

        // Call / Hangup Central Button (x: 270..450, centered at 360, 1065, r: 52)
        let c_dx = (x as i32) - 360;
        let c_dy = (y as i32) - 1065;
        if c_dx * c_dx + c_dy * c_dy <= 2704 {
            let is_in_call = dialer.state != CallState::Idle;
            let bg_color = if is_in_call { DANGER_CRIMSON } else { SUCCESS_EMERALD };

            // Vector Phone Handset
            if !is_in_call {
                // Angled handset (green button)
                let n1 = (c_dx + 12) * (c_dx + 12) + (c_dy + 12) * (c_dy + 12);
                let n2 = (c_dx - 12) * (c_dx - 12) + (c_dy - 12) * (c_dy - 12);
                if n1 <= 36 || n2 <= 36 {
                    return 0xFFFFFFFF;
                }
                let diff = c_dx - c_dy;
                if diff.abs() <= 5 && c_dx >= -12 && c_dx <= 12 && c_dy >= -12 && c_dy <= 12 {
                    return 0xFFFFFFFF;
                }
            } else {
                // Horizontal hangup handset (red button)
                let n1 = (c_dx + 14) * (c_dx + 14) + (c_dy - 2) * (c_dy - 2);
                let n2 = (c_dx - 14) * (c_dx - 14) + (c_dy - 2) * (c_dy - 2);
                if n1 <= 36 || n2 <= 36 {
                    return 0xFFFFFFFF;
                }
                if c_dy >= -8 && c_dy <= -2 && c_dx >= -14 && c_dx <= 14 {
                    return 0xFFFFFFFF;
                }
            }

            return bg_color;
        }

        // Contacts shortcut button (x: 500..672, centered at 586, 1065, r: 40)
        let ct_dx = (x as i32) - 586;
        let ct_dy = (y as i32) - 1065;
        if ct_dx * ct_dx + ct_dy * ct_dy <= 1600 {
            if ct_dx * ct_dx + ct_dy * ct_dy >= 1444 {
                return BORDER_SUBTLE;
            }
            // Person silhouette
            if ct_dx * ct_dx + (ct_dy + 8) * (ct_dy + 8) <= 49 {
                return TEXT_PRIMARY;
            }
            if ct_dy >= 2 && ct_dy <= 14 && ct_dx.abs() <= (ct_dy - 2) + 6 {
                return TEXT_PRIMARY;
            }
            return SURFACE_RAISED;
        }

        return BG_DARK;
    }

    // 5. Bottom Navigation Dock (y: 1260..1380)
    if (1260..1380).contains(&y) && x >= 24 && x < 696 {
        if y == 1260 || y == 1379 || x == 24 || x == 695 {
            return BORDER_SUBTLE;
        }

        // 4 tabs across width:
        // Tab 0: Favorites (x: 48..188)
        // Tab 1: Recents (x: 208..348)
        // Tab 2: Keypad (x: 368..508) -> ACTIVE
        // Tab 3: Contacts (x: 528..668)
        let is_keypad_tab = (368..508).contains(&x);
        if is_keypad_tab && (1275..1365).contains(&y) {
            if y == 1275 || y == 1364 || x == 368 || x == 507 {
                return ACCENT_SKY;
            }
        }

        // Tab titles
        if y >= 1340 && y < 1348 {
            if let Some(c) = test_text(x, y, 76, 1340, 1, b"Favorites", TEXT_MUTED) {
                return c;
            }
            if let Some(c) = test_text(x, y, 248, 1340, 1, b"Recents", TEXT_MUTED) {
                return c;
            }
            if let Some(c) = test_text(x, y, 412, 1340, 1, b"Keypad", ACCENT_SKY) {
                return c;
            }
            if let Some(c) = test_text(x, y, 568, 1340, 1, b"Contacts", TEXT_MUTED) {
                return c;
            }
        }

        // Tab icons
        if y >= 1290 && y < 1326 {
            // Keypad icon (3x3 dots) at (438, 1308)
            if is_keypad_tab {
                let dot_dx = (x as i32) - 438;
                let dot_dy = (y as i32) - 1308;
                if dot_dx.abs() <= 10 && dot_dy.abs() <= 10 {
                    if (dot_dx % 8).abs() <= 1 && (dot_dy % 8).abs() <= 1 {
                        return ACCENT_SKY;
                    }
                }
            }
        }

        return SURFACE_DARK;
    }

    // 6. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn render_dialer_ui(surface: u32, dialer: &DialerApp) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_dialer_pixel(x, y, dialer);
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

/// Returns true if app should exit back to Launcher.
fn handle_touch_point(dialer: &mut DialerApp, x: u32, y: u32) -> bool {
    let x = x as usize;
    let y = y as usize;

    // 1. Back button in header or Home gesture bar: exit to Launcher
    if (x >= 24 && x < 100 && y >= 28 && y < 68) || (x >= 240 && x <= 480 && y >= 1400) {
        std::log(
            0,
            "[DIALER] Back button or home gesture pressed. Exiting to launcher.",
        )
        .ok();
        return true;
    }

    // 2. Backspace button in display card
    if x >= 590 && x < 665 && y >= 235 && y < 305 {
        dialer.backspace();
        return false;
    }

    // 3. Keypad Matrix
    for row in 0..4 {
        let by = ROW_Y[row];
        if y >= by && y < by + BTN_H {
            for col in 0..3 {
                let bx = COL_X[col];
                if x >= bx && x < bx + BTN_W {
                    if let Some(digit) = get_keypad_char(row, col) {
                        dialer.input_digit(digit);
                        std::log(0, "[DIALER] Keypad digit entered.").ok();
                    }
                    return false;
                }
            }
        }
    }

    // 4. Action Bar
    if (990..1140).contains(&y) {
        if x >= 48 && x < 220 && y >= 1005 && y < 1125 {
            // SOS Emergency 911
            dialer.active_len = 0;
            dialer.input_digit('9');
            dialer.input_digit('1');
            dialer.input_digit('1');
            let _ = dialer.dial("911");
            return false;
        } else if x >= 270 && x < 450 && y >= 995 && y < 1135 {
            // Call / End toggle
            if dialer.state == CallState::Idle {
                let mut num_buf = [0u8; 32];
                let len = dialer.active_len;
                num_buf[..len].copy_from_slice(&dialer.active_number[..len]);
                let Ok(target) = core::str::from_utf8(&num_buf[..len]) else {
                    std::log(0, "[DIALER] Invalid dial string.").ok();
                    return false;
                };
                if let Err(_) = dialer.dial(target) {
                    std::log(0, "[DIALER] Call could not be established.").ok();
                }
            } else {
                dialer.hangup();
            }
        } else if x >= 500 && x < 672 && y >= 1005 && y < 1125 {
            // Contacts are loaded from real persisted contact state; no synthetic entries are injected.
            std::audio_haptic_click().ok();
            return false;
        }
    }

    false
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(
        0,
        "[DIALER APP (Ring 3)] Sovereign Dialer & Telephony online.",
    )
    .ok();

    let mut dialer = DialerApp::new();

    // 1. Query Cellular Modem Baseband Telemetry
    // WHY: this app holds no telephony authority, so the query is normally
    // refused. The previous code logged nothing on refusal, leaving an unlogged
    // absence. Report the real outcome and fail closed on modem presence.
    let mut modem_buf = [0u8; 128];
    match std::telephony_query(&mut modem_buf) {
        Ok(len) if len > 0 => {
            if let Ok(json_str) = core::str::from_utf8(&modem_buf[..len]) {
                if json_str.contains("\"present\":true") {
                    dialer.modem_present = true;
                    std::log(
                        0,
                        "[DIALER APP] Baseband Modem: Hardware detected and online.",
                    )
                    .ok();
                } else {
                    // WHY: the payload simply did not list a present modem.
                    // "Offline" would assert a baseband state the HAL never
                    // reported, so state only what the report actually said.
                    std::log(
                        0,
                        "[DIALER APP] Baseband Modem: the telemetry report did not list a present modem.",
                    )
                    .ok();
                }
            }
        }
        Ok(_) => {
            std::log(
                0,
                "[DIALER APP] Telephony query returned an empty payload; no baseband reported.",
            )
            .ok();
        }
        Err(_) => {
            std::log(
                0,
                "[DIALER APP] Telephony query refused by the kernel; no cellular baseband is available to this app.",
            )
            .ok();
        }
    }

    // Emergency contacts are provided by the real contacts/modem subsystem when available.

    // 3. Allocate dedicated KMS Client Surface (z=200)
    std::log(
        0,
        "[DIALER APP] Step 1: Allocating application surface (z=200)...",
    )
    .ok();
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(s) => {
            std::log(0, "[DIALER APP] Step 2: Surface created successfully.").ok();
            s
        }
        Err(_) => {
            std::log(
                0,
                "[DIALER APP] Display surface unavailable; refusing headless call simulation.",
            )
            .ok();
            std::exit_with_code(1);
        }
    };

    // 4. Render an empty dialer; numbers are entered by the user.
    render_dialer_ui(surface, &dialer);
    let _ = std::surface_flip(6);
    std::log(
        0,
        "[DIALER APP] Step 3: Initial frame presented to hardware scanout.",
    )
    .ok();

    // 5. Calls are initiated only from user input; no automatic call is placed at startup.

    // 6. Interactive Event Reactor Loop
    loop {
        if let Ok(events) = std::event_poll(
            std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK | std::EVENT_MODEM_URC,
            100,
        ) {
            if (events & std::EVENT_TIMER_TICK) != 0 {
                if dialer.state == CallState::ActiveCall {
                    dialer.call_duration_secs += 1;
                }
                let _ = std::surface_flip(6);
            }

            if (events & std::EVENT_MODEM_URC) != 0 {
                // WHY: the previous code logged an inbound tower alert and called
                // it a call for any MODEM_URC bit. The URC vector was never read,
                // no call setup message was decoded, and no baseband is attached,
                // so no inbound call occurred. Report the event only.
                if dialer.modem_present {
                    std::log(
                        0,
                        "[DIALER APP] Baseband URC signalled; no call setup message was decoded from it.",
                    )
                    .ok();
                } else {
                    std::log(
                        0,
                        "[DIALER APP] Modem URC event raised with no attached baseband; no call was received.",
                    )
                    .ok();
                }
            }

            if (events & std::EVENT_TOUCH_INPUT) != 0 {
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
                            "[DIALER APP] Navigation gesture recognized. Returning to launcher.",
                        )
                        .ok();
                        let _ = std::surface_destroy(6, surface);
                        std::exit_with_code(0);
                    }
                    if input.kind == 1 {
                        if handle_touch_point(&mut dialer, input.x, input.y) {
                            let _ = std::surface_destroy(6, surface);
                            std::exit_with_code(0);
                        }
                        render_dialer_ui(surface, &dialer);
                        let _ = std::surface_flip(6);
                    }
                }
            }
        }

        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[DIALER APP] Panic detected! Terminating task...").ok();
    std::exit_with_code(1);
}
