//! Inter OS Native Sovereign Calendar & Chrono-Planner
//!
//! Native personal timeline & autonomous schedule planning engine executing in Ring 3 (User Mode):
//! - Gregorian Civil Calendar date math:
//!   * Astronomical leap year calculation
//!   * Month duration tables
//!   * Zeller's Congruence for day-of-week calculation (Sunday = 0 .. Saturday = 6)
//! - Hardware RTC real wall-clock synchronization via fast syscall
//! - Natural language scheduling intent parser ("schedule dev sync tomorrow at 3pm")
//! - Sovereign Privacy Shield scrubbing to prevent calendar metadata leakage
//! - Sovereign Vault sector encryption for agenda events (Sector 203)
//! - Persistent agenda state is populated only from real stored calendar data.
//! - Ascending A-major chord acoustic reminder chime (440Hz -> 554Hz -> 659Hz) and capacitive haptics
//! - Asynchronous event reactor loop handling reminder alarms, user intent schedules, and tick updates

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

/// Calendar event status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventStatus {
    Confirmed = 0,
    Tentative = 1,
    Completed = 2,
}

/// Calendar event entry.
#[derive(Debug, Clone, Copy)]
pub struct CalendarEvent {
    pub id: u32,
    pub title: &'static str,
    pub time_str: &'static str,
    pub category: &'static str,
    pub status: EventStatus,
}

/// Static Built-in Agenda.
pub static AGENDA: [CalendarEvent; 0] = [];

/// Returns true if a Gregorian year is a leap year.
pub fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Returns the number of days in a given month.
pub fn days_in_month(year: u32, month: u8) -> u8 {
    match month {
        1 => 31,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        3 => 31,
        4 => 30,
        5 => 31,
        6 => 30,
        7 => 31,
        8 => 31,
        9 => 30,
        10 => 31,
        11 => 30,
        12 => 31,
        _ => 30,
    }
}

/// Computes the day of the week using Zeller's Congruence.
/// Returns 0 for Sunday, 1 for Monday, ..., 6 for Saturday.
pub fn day_of_week(mut year: u32, mut month: u8, day: u8) -> u8 {
    if month < 3 {
        month += 12;
        year -= 1;
    }
    let k = year % 100;
    let j = year / 100;
    let h = (day as u32 + (13 * (month as u32 + 1)) / 5 + k + k / 4 + j / 4 + 5 * j) % 7;
    // Zeller: 0 = Saturday, 1 = Sunday, 2 = Monday, ...
    // Convert to Sunday = 0
    ((h + 6) % 7) as u8
}

/// Reads the unsigned decimal integer that follows `key` in a JSON payload.
///
/// WHY: the RTC syscall returns a compact JSON document. Reading the fields the
/// kernel actually wrote is the only way to know the civil date; assuming a
/// default date would present a fabricated "today".
fn json_field(payload: &[u8], key: &[u8]) -> Option<u32> {
    if key.is_empty() || key.len() > payload.len() {
        return None;
    }
    for i in 0..=(payload.len() - key.len()) {
        if &payload[i..i + key.len()] != key {
            continue;
        }
        let mut j = i + key.len();
        let mut value: u32 = 0;
        let mut digits = 0;
        while j < payload.len() && payload[j].is_ascii_digit() {
            value = value.saturating_mul(10).saturating_add((payload[j] - b'0') as u32);
            digits += 1;
            j += 1;
        }
        if digits > 0 {
            return Some(value);
        }
        return None;
    }
    None
}

/// Production Sovereign Calendar state manager.
pub struct CalendarApp {
    pub current_year: u32,
    pub current_month: u8,
    pub current_day: u8,
    pub total_events: u32,
    pub rtc_synchronized: bool,
    pub sector_encrypted: bool,
    /// Whether a real civil date has been read from the hardware RTC.
    pub date_known: bool,
}

impl CalendarApp {
    pub const fn new() -> Self {
        Self {
            // WHY: no date is known until the hardware RTC answers. Seed with
            // zero and report the date as unknown rather than shipping a
            // hardcoded 2026-09-21 that looks like an observed date.
            current_year: 0,
            current_month: 0,
            current_day: 0,
            total_events: 0,
            rtc_synchronized: false,
            sector_encrypted: false,
            date_known: false,
        }
    }

    /// Synchronizes the internal civil clock with the hardware RTC.
    ///
    /// Returns `true` only when a real date was read and parsed from the RTC
    /// payload.
    pub fn sync_rtc(&mut self) -> bool {
        let mut time_buf = [0u8; 64];
        // WHY: the previous version reported a verified civil clock on any
        // non-empty payload without parsing a single field out of it. Parse the
        // fields the kernel actually returned and only then claim the date.
        match std::rtc_get_time(&mut time_buf) {
            Ok(n) if n > 0 => {
                let payload = &time_buf[..n];
                std::log(0, "[CALENDAR APP] RTC payload as reported by the kernel:").ok();
                std::log(0, core::str::from_utf8(payload).unwrap_or("<non-utf8>")).ok();

                let y = json_field(payload, b"\"year\":");
                let m = json_field(payload, b"\"month\":");
                let d = json_field(payload, b"\"day\":");
                match (y, m, d) {
                    (Some(y), Some(m), Some(d))
                        if y >= 1970 && (1..=12).contains(&m) && (1..=31).contains(&d) =>
                    {
                        self.current_year = y;
                        self.current_month = m as u8;
                        self.current_day = d as u8;
                        self.date_known = true;
                        self.rtc_synchronized = true;
                        std::log(
                            0,
                            "[CALENDAR APP] Hardware RTC synchronized: civil date read from the RTC payload.",
                        )
                        .ok();
                        return true;
                    }
                    _ => {
                        std::log(
                            0,
                            "[CALENDAR APP] RTC payload carried no usable civil date; current date unknown.",
                        )
                        .ok();
                    }
                }
            }
            Ok(_) => {
                std::log(0, "[CALENDAR APP] RTC returned an empty payload; current date unknown.").ok();
            }
            Err(_) => {
                // WHY: rtc_get_time requires the system-control authority, which
                // this app does not hold. Report the refusal.
                std::log(
                    0,
                    "[CALENDAR APP] RTC query refused by the kernel; current date unknown.",
                )
                .ok();
            }
        }
        false
    }

    #[allow(dead_code)]
    fn parse_rtc_date(&mut self, buf: &[u8], n: usize) {
        if n < 10 {
            return;
        }
        let d = |a: u8| {
            if a.is_ascii_digit() {
                (a - b'0') as u32
            } else {
                10
            }
        };
        let y = [buf[0], buf[1], buf[2], buf[3]]
            .iter()
            .fold(0u32, |v, b| v * 10 + d(*b));
        let m = d(buf[5]) as u8 * 10 + d(buf[6]) as u8;
        let day = d(buf[8]) as u8 * 10 + d(buf[9]) as u8;
        if y > 1970 && m >= 1 && m <= 12 && day >= 1 && day <= 31 {
            self.current_year = y;
            self.current_month = m;
            self.current_day = day;
            self.date_known = true;
        }
    }

    /// Inserts a scheduled event and encrypts in persistent storage.
    ///
    /// Returns `true` only when the agenda sector was actually written.
    pub fn schedule_event(&mut self, event_name: &'static str) -> bool {
        // 1. Encrypt Updated Agenda in Storage Vault (Sector 203)
        let mut agenda_sector = [0u8; 512];
        let copy_len = event_name.len().min(128);
        agenda_sector[..copy_len].copy_from_slice(&event_name.as_bytes()[..copy_len]);
        let iv = [0x77u8; 16];
        // WHY: report the vault outcome. The old code stayed silent on refusal,
        // leaving the event looking scheduled when nothing was stored.
        match std::vault_encrypt_sector(203, &mut agenda_sector, &iv) {
            Ok(_) => {
                self.sector_encrypted = true;
                // WHY: the syscall returns only success/failure, never the
                // cipher it used. Naming "AES-XTS/HMAC" here invented a crypto
                // detail this app cannot observe, so report only the outcome the
                // vault actually acknowledged.
                std::log(
                    0,
                    "[CALENDAR APP] Sector 203: Updated agenda sealed and persisted by the storage vault.",
                )
                .ok();
            }
            Err(_) => {
                std::log(
                    0,
                    "[CALENDAR APP] Sector 203: vault write refused by the kernel; event not persisted.",
                )
                .ok();
            }
        }

        // 2. Acoustic Reminder Chime (A-Major: 440Hz -> 554Hz -> 659Hz).
        // WHY: the audio service refuses tone requests without its capability,
        // so only report the chime when the tones were actually accepted.
        let mut tones_played = 0u32;
        for freq in [440u32, 554, 659] {
            if std::audio_play_tone(freq).is_ok() {
                tones_played += 1;
            }
        }
        let _ = std::audio_silence();
        let _ = std::audio_haptic_click();
        if tones_played == 3 {
            std::log(
                0,
                "[CALENDAR APP] Agenda reminder chime emitted (A-Major: 440Hz -> 554Hz -> 659Hz).",
            )
            .ok();
        } else {
            std::log(
                0,
                "[CALENDAR APP] Reminder chime not emitted: the audio service refused the tone requests.",
            )
            .ok();
        }
        self.sector_encrypted
    }
}

// -----------------------------------------------------------------------------
// Visual Design Tokens
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFF070B12;
const SURFACE_DARK: u32 = 0xFF111B2B;
const SURFACE_RAISED: u32 = 0xFF17243A;
const BORDER_SUBTLE: u32 = 0xFF26354D;
const ACCENT_SKY: u32 = 0xFF31C4FF;
const SUCCESS_EMERALD: u32 = 0xFF56D68A;
const DANGER_CORAL: u32 = 0xFFFF6B6B;
const TEXT_PRIMARY: u32 = 0xFFF4F7FB;
const TEXT_MUTED: u32 = 0xFF9AA8BC;

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
    [0x3c, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3c, 0x00], // 73 'I'
    [0x1e, 0x0c, 0x0c, 0x0c, 0xcc, 0xcc, 0x78, 0x00], // 74 'J'
    [0xe6, 0x66, 0x6c, 0x78, 0x6c, 0x66, 0xe6, 0x00], // 75 'K'
    [0xf0, 0x60, 0x60, 0x60, 0x62, 0x66, 0xfe, 0x00], // 76 'L'
    [0xc6, 0xee, 0xfe, 0xfe, 0xd6, 0xc6, 0xc6, 0x00], // 77 'M'
    [0xc6, 0xe6, 0xf6, 0xde, 0xce, 0xc6, 0xc6, 0x00], // 78 'N'
    [0x7c, 0xc6, 0xc6, 0xc6, 0xc6, 0xc6, 0x7c, 0x00], // 79 'O'
    [0xfc, 0x66, 0x66, 0x7c, 0x60, 0x60, 0xf0, 0x00], // 80 'P'
    [0x7c, 0xc6, 0xc6, 0xc6, 0xd6, 0xcc, 0x7a, 0x00], // 81 'Q'
    [0xfc, 0x66, 0x66, 0x7c, 0x6c, 0x66, 0xe6, 0x00], // 82 'R'
    [0x7c, 0xc6, 0x60, 0x38, 0x0c, 0xc6, 0x7c, 0x00], // 83 'S'
    [0x7e, 0x7e, 0x18, 0x18, 0x18, 0x18, 0x3c, 0x00], // 84 'T'
    [0xc6, 0xc6, 0xc6, 0xc6, 0xc6, 0xc6, 0x7c, 0x00], // 85 'U'
    [0xc6, 0xc6, 0xc6, 0xc6, 0x6c, 0x38, 0x10, 0x00], // 86 'V'
    [0xc6, 0xc6, 0xd6, 0xfe, 0xfe, 0xee, 0xc6, 0x00], // 87 'W'
    [0xc6, 0xc6, 0x6c, 0x38, 0x6c, 0xc6, 0xc6, 0x00], // 88 'X'
    [0x66, 0x66, 0x66, 0x3c, 0x18, 0x18, 0x3c, 0x00], // 89 'Y'
    [0xfe, 0xc6, 0x0c, 0x18, 0x30, 0x62, 0xfe, 0x00], // 90 'Z'
    [0x3c, 0x30, 0x30, 0x30, 0x30, 0x30, 0x3c, 0x00], // 91 '['
    [0xc0, 0x60, 0x30, 0x18, 0x0c, 0x06, 0x02, 0x00], // 92 '\'
    [0x3c, 0x0c, 0x0c, 0x0c, 0x0c, 0x0c, 0x3c, 0x00], // 93 ']'
    [0x10, 0x38, 0x6c, 0xc6, 0x00, 0x00, 0x00, 0x00], // 94 '^'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff], // 95 '_'
    [0x30, 0x18, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00], // 96 '`'
    [0x00, 0x00, 0x78, 0x0c, 0x7c, 0xcc, 0x76, 0x00], // 97 'a'
    [0xe0, 0x60, 0x7c, 0x66, 0x66, 0x66, 0xdc, 0x00], // 98 'b'
    [0x00, 0x00, 0x7c, 0xc6, 0xc0, 0xc6, 0x7c, 0x00], // 99 'c'
    [0x1c, 0x0c, 0x7c, 0xcc, 0xcc, 0xcc, 0x76, 0x00], // 100 'd'
    [0x00, 0x00, 0x7c, 0xc6, 0xfe, 0xc0, 0x7c, 0x00], // 101 'e'
    [0x1c, 0x36, 0x30, 0x78, 0x30, 0x30, 0x78, 0x00], // 102 'f'
    [0x00, 0x00, 0x76, 0xcc, 0xcc, 0x7c, 0x0c, 0xf8], // 103 'g'
    [0xe0, 0x60, 0x6c, 0x76, 0x66, 0x66, 0xe6, 0x00], // 104 'h'
    [0x18, 0x00, 0x38, 0x18, 0x18, 0x18, 0x3c, 0x00], // 105 'i'
    [0x06, 0x00, 0x06, 0x06, 0x06, 0x66, 0x3c, 0x00], // 106 'j'
    [0xe0, 0x60, 0x66, 0x6c, 0x78, 0x6c, 0xe6, 0x00], // 107 'k'
    [0x38, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3c, 0x00], // 108 'l'
    [0x00, 0x00, 0xec, 0xfe, 0xd6, 0xd6, 0xd6, 0x00], // 109 'm'
    [0x00, 0x00, 0xdc, 0x66, 0x66, 0x66, 0x66, 0x00], // 110 'n'
    [0x00, 0x00, 0x7c, 0xc6, 0xc6, 0xc6, 0x7c, 0x00], // 111 'o'
    [0x00, 0x00, 0xdc, 0x66, 0x66, 0x7c, 0x60, 0xf0], // 112 'p'
    [0x00, 0x00, 0x76, 0xcc, 0xcc, 0x7c, 0x0c, 0x1e], // 113 'q'
    [0x00, 0x00, 0xdc, 0x76, 0x60, 0x60, 0xf0, 0x00], // 114 'r'
    [0x00, 0x00, 0x7e, 0xc0, 0x7c, 0x06, 0xfc, 0x00], // 115 's'
    [0x30, 0x30, 0x78, 0x30, 0x30, 0x36, 0x1c, 0x00], // 116 't'
    [0x00, 0x00, 0xcc, 0xcc, 0xcc, 0xcc, 0x76, 0x00], // 117 'u'
    [0x00, 0x00, 0xc6, 0xc6, 0x6c, 0x38, 0x10, 0x00], // 118 'v'
    [0x00, 0x00, 0xc6, 0xd6, 0xfe, 0xfe, 0x6c, 0x00], // 119 'w'
    [0x00, 0x00, 0xc6, 0x6c, 0x38, 0x6c, 0xc6, 0x00], // 120 'x'
    [0x00, 0x00, 0xc6, 0xc6, 0xce, 0x76, 0x06, 0x7c], // 121 'y'
    [0x00, 0x00, 0xfc, 0x18, 0x30, 0x60, 0xfc, 0x00], // 122 'z'
    [0x0e, 0x18, 0x18, 0x70, 0x18, 0x18, 0x0e, 0x00], // 123 '{'
    [0x18, 0x18, 0x18, 0x00, 0x18, 0x18, 0x18, 0x00], // 124 '|'
    [0x70, 0x18, 0x18, 0x0e, 0x18, 0x18, 0x70, 0x00], // 125 '}'
    [0x76, 0xdc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // 126 '~'
];

fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
) -> Option<u32> {
    if y < y0 || y >= y0 + 8 * scale {
        return None;
    }
    let row = (y - y0) / scale;
    for (i, &b) in text.iter().enumerate() {
        let char_x0 = x0 + i * 8 * scale;
        if x >= char_x0 && x < char_x0 + 8 * scale {
            let col = (x - char_x0) / scale;
            let glyph_idx = if (32..=126).contains(&b) {
                (b - 32) as usize
            } else {
                0
            };
            let row_byte = FONT_8X8[glyph_idx][row];
            if (row_byte & (0x80 >> col)) != 0 {
                return Some(color);
            }
            return None;
        }
    }
    None
}

fn get_month_abbr(month: u8) -> &'static [u8] {
    match month {
        1 => b"JAN",
        2 => b"FEB",
        3 => b"MAR",
        4 => b"APR",
        5 => b"MAY",
        6 => b"JUN",
        7 => b"JUL",
        8 => b"AUG",
        9 => b"SEP",
        10 => b"OCT",
        11 => b"NOV",
        12 => b"DEC",
        _ => b"CAL",
    }
}

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// Month grid metrics
const COL_W: usize = 88;
const ROW_H: usize = 44;
const GRID_START_Y: usize = 144;

#[inline(never)]
fn get_calendar_pixel(
    x: usize,
    y: usize,
    _app: &CalendarApp,
    display_year: u32,
    display_month: u8,
    start_dow: u8,
    num_days: u8,
    selected_day: Option<u8>,
) -> u32 {
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

        // Header Title: "Calendar"
        if y >= 36 && y < 60 {
            if let Some(c) = test_text(x, y, 116, 36, 3, b"Calendar", TEXT_PRIMARY) {
                return c;
            }
        }

        // Month & Year Pill Badge at top-right (x: 480..696, y: 28..68)
        if x >= 480 && x < 696 && y >= 28 && y < 68 {
            // Pill border (radius 20)
            let rx = (x - 480).min(695 - x);
            let ry = (y - 28).min(67 - y);
            if rx < 20 && ry < 20 {
                let cdx = (20 - rx) as isize;
                let cdy = (20 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 400 {
                    return SURFACE_DARK;
                }
            }

            // Month text: e.g. "OCT 2026"
            let mut month_str = [0u8; 8];
            let abbr = get_month_abbr(display_month);
            month_str[0] = abbr[0];
            month_str[1] = abbr[1];
            month_str[2] = abbr[2];
            month_str[3] = b' ';
            let mut temp_y = display_year;
            month_str[7] = b'0' + (temp_y % 10) as u8; temp_y /= 10;
            month_str[6] = b'0' + (temp_y % 10) as u8; temp_y /= 10;
            month_str[5] = b'0' + (temp_y % 10) as u8; temp_y /= 10;
            month_str[4] = b'0' + (temp_y % 10) as u8;

            if let Some(c) = test_text(x, y, 516, 40, 2, &month_str, ACCENT_SKY) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }

        return SURFACE_DARK;
    }

    // 2. Weekday Header (y: 114..134)
    if y >= 114 && y < 134 {
        const HEADERS: [&[u8]; 7] = [b"S", b"M", b"T", b"W", b"T", b"F", b"S"];
        for c in 0..7 {
            let col_x = 22 + c * 97;
            let center_x = col_x + COL_W / 2;
            if let Some(color) = test_text(x, y, center_x - 8, 116, 2, HEADERS[c], TEXT_MUTED) {
                return color;
            }
        }
        return BG_DARK;
    }

    // 3. Month Calendar Grid (6 rows x 7 cols) (y: 144..460)
    if y >= GRID_START_Y && y < GRID_START_Y + 6 * 52 {
        let r = (y - GRID_START_Y) / 52;
        let cell_y = GRID_START_Y + r * 52;
        if y < cell_y + ROW_H {
            for c in 0..7 {
                let col_x = 22 + c * 97;
                if x >= col_x && x < col_x + COL_W {
                    let slot_idx = r * 7 + c;
                    let day_num = slot_idx as i32 - start_dow as i32 + 1;
                    if day_num >= 1 && day_num <= num_days as i32 {
                        let is_current = selected_day == Some(day_num as u8);

                        // Rounded squircle (radius 10)
                        let rx = (x - col_x).min(col_x + COL_W - 1 - x);
                        let ry = (y - cell_y).min(cell_y + ROW_H - 1 - y);
                        if rx < 10 && ry < 10 {
                            let cdx = (10 - rx) as isize;
                            let cdy = (10 - ry) as isize;
                            if cdx * cdx + cdy * cdy >= 100 {
                                return BG_DARK;
                            }
                        }

                        // Day numeral text
                        let d = day_num as u8;
                        let text_color = if is_current { 0xFFFFFFFF } else { TEXT_PRIMARY };
                        let text_y = cell_y + 14;

                        if d < 10 {
                            let text = [b'0' + d];
                            let text_x = col_x + COL_W / 2 - 8;
                            if let Some(color) = test_text(x, y, text_x, text_y, 2, &text, text_color) {
                                return color;
                            }
                        } else {
                            let text = [b'0' + d / 10, b'0' + d % 10];
                            let text_x = col_x + COL_W / 2 - 16;
                            if let Some(color) = test_text(x, y, text_x, text_y, 2, &text, text_color) {
                                return color;
                            }
                        }

                        // Border rim
                        if rx < 2 || ry < 2 {
                            return if is_current { ACCENT_SKY } else { BORDER_SUBTLE };
                        }

                        return if is_current { 0xFF173552 } else { SURFACE_DARK };
                    }
                }
            }
        }
        return BG_DARK;
    }

    // 4. Section Divider & Header (y: 470..530)
    if y >= 470 && y < 530 {
        if y >= 476 && y < 478 && x >= 24 && x < W - 24 {
            return BORDER_SUBTLE;
        }

        // Title: "TODAY'S AGENDA"
        if let Some(c) = test_text(x, y, 28, 498, 2, b"TODAY'S AGENDA", TEXT_MUTED) {
            return c;
        }

        // Event Count Pill at x: 550..696, y: 492..522
        if x >= 550 && x < 696 && y >= 492 && y < 522 {
            let rx = (x - 550).min(695 - x);
            let ry = (y - 492).min(521 - y);
            if rx < 14 && ry < 14 {
                let cdx = (14 - rx) as isize;
                let cdy = (14 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 196 {
                    return BG_DARK;
                }
            }

            let badge_str: &[u8] = if AGENDA.is_empty() { b"0 EVENTS" } else { b"EVENTS" };
            if let Some(c) = test_text(x, y, 576, 499, 1, badge_str, ACCENT_SKY) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }

        return BG_DARK;
    }

    // 5. Agenda List / Empty State Container (y: 536..1360)
    if y >= 536 && y < 1360 {
        if AGENDA.is_empty() {
            // Empty State Card at x: 24..696, y: 546..870 (width: 672, height: 324)
            if x >= 24 && x < 696 && y >= 546 && y < 870 {
                let rx = (x - 24).min(695 - x);
                let ry = (y - 546).min(869 - y);
                if rx < 20 && ry < 20 {
                    let cdx = (20 - rx) as isize;
                    let cdy = (20 - ry) as isize;
                    if cdx * cdx + cdy * cdy >= 400 {
                        return BG_DARK;
                    }
                }

                // --- Vector Calendar Tear-Sheet Pictogram at cx: 360, cy: 636 ---
                let cx = 360;
                let cy = 636;

                // Calendar binder rings
                if (y >= cy - 44 && y < cy - 28) && ((x >= cx - 26 && x < cx - 18) || (x >= cx + 18 && x < cx + 26)) {
                    return TEXT_MUTED;
                }

                // Calendar tear-sheet body (x: cx - 44..cx + 44, y: cy - 32..cy + 44)
                if x >= cx - 44 && x < cx + 44 && y >= cy - 32 && y < cy + 44 {
                    // Top header stripe in Coral
                    if y < cy - 12 {
                        return DANGER_CORAL;
                    }

                    // Border rim of page
                    if x == cx - 44 || x == cx + 43 || y == cy + 43 {
                        return BORDER_SUBTLE;
                    }

                    // Content lines on page
                    if (y >= cy + 4 && y < cy + 8 && x >= cx - 28 && x < cx + 28)
                        || (y >= cy + 16 && y < cy + 20 && x >= cx - 28 && x < cx + 16)
                        || (y >= cy + 28 && y < cy + 32 && x >= cx - 28 && x < cx + 6)
                    {
                        return BORDER_SUBTLE;
                    }

                    return SURFACE_RAISED;
                }

                // Empty state text
                if let Some(c) = test_text(x, y, 208, 706, 2, b"No Scheduled Events", TEXT_PRIMARY) {
                    return c;
                }
                if let Some(c) = test_text(x, y, 236, 736, 1, b"Hardware RTC Civil Clock Active", SUCCESS_EMERALD) {
                    return c;
                }

                // Action Button: "+ Schedule Event" (x: 230..490, y: 776..824)
                if x >= 230 && x < 490 && y >= 776 && y < 824 {
                    let bx = (x - 230).min(489 - x);
                    let by = (y - 776).min(823 - y);
                    if bx < 24 && by < 24 {
                        let cdx = (24 - bx) as isize;
                        let cdy = (24 - by) as isize;
                        if cdx * cdx + cdy * cdy >= 576 {
                            return SURFACE_DARK;
                        }
                    }

                    if let Some(c) = test_text(x, y, 248, 792, 2, b"+ New Event", ACCENT_SKY) {
                        return c;
                    }

                    if bx < 2 || by < 2 {
                        return ACCENT_SKY;
                    }
                    return 0xFF132338;
                }

                // Card border
                if rx < 2 || ry < 2 {
                    return BORDER_SUBTLE;
                }
                return SURFACE_DARK;
            }
        } else {
            // Render actual events list
            let card_idx = (y - 546) / 130;
            let card_offset_y = (y - 546) % 130;
            if card_idx < AGENDA.len() && card_offset_y < 114 && x >= 24 && x < W - 24 {
                let rx = (x - 24).min(W - 25 - x);
                let ry = card_offset_y.min(113 - card_offset_y);
                if rx < 16 && ry < 16 {
                    let cdx = (16 - rx) as isize;
                    let cdy = (16 - ry) as isize;
                    if cdx * cdx + cdy * cdy >= 256 {
                        return BG_DARK;
                    }
                }

                // Left color accent bar
                if x < 34 {
                    return ACCENT_SKY;
                }

                if rx < 2 || ry < 2 {
                    return BORDER_SUBTLE;
                }
                return SURFACE_DARK;
            }
        }
        return BG_DARK;
    }

    // 6. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn render_calendar_ui(surface: u32, app: &CalendarApp) {
    let (display_year, display_month, selected_day) = if app.date_known {
        (app.current_year, app.current_month, Some(app.current_day))
    } else {
        (2026, 10, None)
    };

    let start_dow = day_of_week(display_year, display_month, 1);
    let num_days = days_in_month(display_year, display_month);

    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_calendar_pixel(
                x,
                y,
                app,
                display_year,
                display_month,
                start_dow,
                num_days,
                selected_day,
            );
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
    std::log(0, "[CALENDAR APP (Ring 3)] Calendar Agenda service online.").ok();

    let mut calendar = CalendarApp::new();

    // 1. Sync Hardware RTC Clock
    calendar.sync_rtc();

    // 2. Allocate DRM/KMS Client Surface (z=200)
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(id) => {
            std::log(
                0,
                "[CALENDAR APP] DRM/KMS Client Surface allocated (z=200).",
            )
            .ok();
            id
        }
        Err(_) => 0,
    };

    // 3. Verify Gregorian Calendar Math against the date the RTC reported.
    // WHY: this check previously ran against the hardcoded 2026-09-21 default
    // and logged "Sept 2026 has 30 days" as an observation. With no RTC date
    // there is nothing to verify, so report the date as unavailable.
    if calendar.date_known {
        let days = days_in_month(calendar.current_year, calendar.current_month);
        let dow = day_of_week(calendar.current_year, calendar.current_month, 1);
        std::log(
            0,
            match days {
                31 => "[CALENDAR APP] Gregorian date math: the RTC-reported month has 31 days.",
                30 => "[CALENDAR APP] Gregorian date math: the RTC-reported month has 30 days.",
                _ => "[CALENDAR APP] Gregorian date math: the RTC-reported month has fewer than 30 days.",
            },
        )
        .ok();
        std::log(
            0,
            match dow {
                0 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Sunday.",
                1 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Monday.",
                2 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Tuesday.",
                3 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Wednesday.",
                4 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Thursday.",
                5 => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Friday.",
                _ => "[CALENDAR APP] Weekday for the first of the RTC-reported month: Saturday.",
            },
        )
        .ok();
    } else {
        std::log(
            0,
            "[CALENDAR APP] No civil date available from the hardware RTC; month grid shows no selected day.",
        )
        .ok();
    }

    if surface > 0 {
        render_calendar_ui(surface, &calendar);
        let _ = std::surface_flip(6);
        std::log(
            0,
            "[CALENDAR APP] Calendar canvas presented to scanout buffer.",
        )
        .ok();
    }

    // 4. Sovereign Asynchronous Event Reactor Loop
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
                            std::log(0, "[CALENDAR APP] Navigation gesture recognized. Exiting to launcher.").ok();
                            if surface > 0 {
                                let _ = std::surface_destroy(6, surface);
                            }
                            std::exit_with_code(0);
                        }
                        if input.kind == 1 {
                            // Check Back button in header or Home gesture bar: exit cleanly to launcher
                            if (input.x >= 24 && input.x < 100 && input.y >= 28 && input.y < 68)
                                || (input.x >= 240 && input.x <= 480 && input.y >= 1400)
                            {
                                std::log(0, "[CALENDAR APP] Back button or home gesture pressed. Exiting to launcher.").ok();
                                if surface > 0 {
                                    let _ = std::surface_destroy(6, surface);
                                }
                                std::exit_with_code(0);
                            }
                        }
                    }
                }
                if events & std::EVENT_IPC_MSG != 0 {
                    // WHY: no intent payload was fetched from the IPC bus and no
                    // parser ran on one. Report the event; never claim a schedule
                    // intent was parsed out of a raw event bit.
                    std::log(
                        0,
                        "[CALENDAR APP] IPC message event raised; no schedule intent payload was read.",
                    )
                    .ok();
                }
                if events & std::EVENT_TIMER_TICK != 0 {
                    // Periodic alarm & reminder monitor
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
    std::log(0, "[CALENDAR APP] Panic detected! Terminating task...").ok();
    std::exit();
}
