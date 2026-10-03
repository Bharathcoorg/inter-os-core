#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

use launcher::theme::*;
use launcher::{layout, text};

// WHY one file per screen: every renderer was a function in this file, so two
// people editing different screens collided on every save. Submodules can reach
// the crate root's private items, so no visibility change was needed.
mod screens;
use screens::*;

const W: usize = SCREEN_WIDTH;
const H: usize = SCREEN_HEIGHT;
const CHUNK: usize = 4096;

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
        if c >= 32 && c <= 126 {
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
// Interactive Launcher View Modes & Application State Engine
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    HomeScreen,
    ActivitySpace,
    QuickControls,
    SystemSpace,
    LockScreen,
    PowerMenu,
    AppDrawer,
    Notifications,
    LiveWidgets,
}

pub struct LauncherState {
    pub current_view: ViewMode,
    pub clock_time: [u8; 5],
    /// True only while `clock_time` and `date_line` hold a reading the kernel
    /// actually returned. WHY it exists: with no RTC authority granted to this
    /// shell the read can be refused, and a refused read must leave the clock
    /// and the date line empty rather than show a plausible-looking value.
    pub clock_source_ok: bool,
    /// Guards the one-shot console report of an unavailable clock source, so the
    /// per-second refresh does not flood the log.
    pub clock_source_reported: bool,
    /// Lock-screen date line, formatted `Www, DD Mmm` from the kernel RTC.
    /// WHY this is populated and never a literal: the lock screen shows the real
    /// calendar, and a hardcoded date would be fabricated data on a surface the
    /// user reads as the state of their device.
    pub date_line: [u8; LOCK_DATE_TEXT_LEN],
    pub notification_count: usize,
    pub app_drawer_tab: usize,
    pub recent_apps: [[u8; 32]; 3],
    pub recent_app_lens: [u8; 3],
    pub recent_task_ids: [u64; 3],
    pub app_ids: [[u8; 32]; 32],
    pub app_id_lens: [u8; 32],
    pub app_labels: [[u8; 16]; 32],
    pub app_label_lens: [u8; 32],
    pub app_icons: [[u8; 4]; 32],
    pub app_count: usize,
    pub app_drawer_page: usize,
    pub active_child: Option<u64>,
}

impl LauncherState {
    pub const fn new() -> Self {
        Self {
            current_view: ViewMode::LockScreen,
            clock_time: [b' '; 5],
            clock_source_ok: false,
            clock_source_reported: false,
            date_line: [b' '; LOCK_DATE_TEXT_LEN],
            notification_count: 0,
            app_drawer_tab: 0,
            recent_apps: [[0u8; 32]; 3],
            recent_app_lens: [0; 3],
            recent_task_ids: [0; 3],
            app_ids: [[0u8; 32]; 32],
            app_id_lens: [0u8; 32],
            app_labels: [[0u8; 16]; 32],
            app_label_lens: [0u8; 32],
            app_icons: [[0u8; 4]; 32],
            app_count: 0,
            app_drawer_page: 0,
            active_child: None,
        }
    }

    pub fn push_recent(&mut self, app: &str) {
        if app.is_empty() || app.len() > 32 {
            return;
        }
        if self.recent_app_lens[0] > 0
            && core::str::from_utf8(&self.recent_apps[0][..self.recent_app_lens[0] as usize]).ok()
                == Some(app)
        {
            return;
        }

        let mut next = [[0u8; 32]; 3];
        let mut lens = [0u8; 3];
        next[0][..app.len()].copy_from_slice(app.as_bytes());
        lens[0] = app.len() as u8;

        for src in 0..3 {
            let src_len = self.recent_app_lens[src] as usize;
            if src_len == 0 || src >= 2 {
                continue;
            }
            let value = &self.recent_apps[src][..src_len];
            if value == app.as_bytes() {
                continue;
            }
            let dst = if src == 0 { 1 } else { 2 };
            next[dst][..src_len].copy_from_slice(value);
            lens[dst] = src_len as u8;
        }
        self.recent_apps = next;
        self.recent_app_lens = lens;
        self.recent_task_ids[1] = self.recent_task_ids[0];
        self.recent_task_ids[2] = self.recent_task_ids[1];
        self.recent_task_ids[0] = 0;
    }

    fn recent_id(&self, index: usize) -> Option<&str> {
        if index >= 3 {
            return None;
        }
        let len = self.recent_app_lens[index] as usize;
        if len == 0 {
            return None;
        }
        core::str::from_utf8(&self.recent_apps[index][..len]).ok()
    }

    fn recent_label(&self, index: usize) -> Option<&[u8]> {
        let id = self.recent_id(index)?;
        for i in 0..self.app_count {
            let len = self.app_id_lens[i] as usize;
            if len > 0 && &self.app_ids[i][..len] == id.as_bytes() {
                let label_len = self.app_label_lens[i] as usize;
                if label_len > 0 {
                    return Some(&self.app_labels[i][..label_len]);
                }
            }
        }
        None
    }

    pub fn dismiss_recent(&mut self, index: usize) {
        if index >= 3 {
            return;
        }
        for i in index..2 {
            self.recent_apps[i] = self.recent_apps[i + 1];
            self.recent_app_lens[i] = self.recent_app_lens[i + 1];
            self.recent_task_ids[i] = self.recent_task_ids[i + 1];
        }
        self.recent_apps[2] = [0u8; 32];
        self.recent_app_lens[2] = 0;
        self.recent_task_ids[2] = 0;
    }
}

fn refresh_clock(state: &mut LauncherState) {
    // WHY 64 bytes and not more: the kernel's RtcGetTime handler rejects any
    // requested length above 64 with InvalidArguments, so a larger buffer silently
    // costs the whole clock read. The JSON payload begins
    // {"year":..,"month":..,"day":..,"hour":..,"minute":.. and only reaches
    // "second" past byte 64, and the date line needs nothing beyond "minute".
    let mut buf = [0u8; 64];
    // WHY the refusal is reported once and the clock is left blank: every
    // surface in this shell draws `clock_time`, so a placeholder such as
    // "00:00" would present an unmeasured value as a reading. The lock screen
    // omits the clock and the date entirely when no source answered.
    let Ok(n) = std::rtc_get_time(&mut buf) else {
        state.clock_source_ok = false;
        if !state.clock_source_reported {
            state.clock_source_reported = true;
            std::log(
                0,
                "[LAUNCHER] Clock source unavailable: the kernel refused the hardware RTC read, so no time is displayed.",
            )
            .ok();
        }
        return;
    };
    let data = &buf[..n];
    /// Reads the unsigned integer that follows `key`'s colon.
    ///
    /// WHY this parses a variable-width run of digits rather than exactly two:
    /// the previous version required two consecutive digits and returned None
    /// otherwise, so `"day":3` failed to parse, the caller returned early, and
    /// `clock_source_ok` stayed false -- which silently suppressed the clock and
    /// the date for every single-digit day of the month. The same two-digit reader
    /// also truncated `"year":2026` to 20, which would have computed the weekday
    /// from the wrong year. A fixed two-digit read is only correct for a field
    /// that is always zero-padded, and none of these are.
    fn field(data: &[u8], key: &[u8]) -> Option<u32> {
        let mut i = 0usize;
        while i + key.len() < data.len() {
            if &data[i..i + key.len()] == key {
                let mut j = i + key.len();
                while j < data.len() && data[j].is_ascii_whitespace() { j += 1; }
                if j < data.len() && data[j] == b':' { j += 1; }
                while j < data.len() && data[j].is_ascii_whitespace() { j += 1; }
                let start = j;
                let mut value: u32 = 0;
                while j < data.len() && data[j].is_ascii_digit() {
                    // Bound the digits so a malformed value cannot overflow.
                    if j - start >= 9 { return None; }
                    value = value * 10 + (data[j] - b'0') as u32;
                    j += 1;
                }
                if j > start {
                    return Some(value);
                }
            }
            i += 1;
        }
        None
    }
    let Some(hour) = field(data, b"\"hour\"") else { return; };
    let Some(minute) = field(data, b"\"minute\"") else { return; };
    if hour < 24 && minute < 60 {
        let (hour, minute) = (hour as u8, minute as u8);
        state.clock_time = [b'0' + hour / 10, b'0' + hour % 10, b':', b'0' + minute / 10, b'0' + minute % 10];
    }
    // The lock screen also shows the calendar date. The kernel RTC publishes
    // year/month/day alongside the clock, so the date line is read from the
    // same source rather than being written into the source as a literal.
    let (Some(year), Some(month), Some(day)) = (
        field(data, b"\"year\""),
        field(data, b"\"month\""),
        field(data, b"\"day\""),
    ) else {
        return;
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return;
    }
    let Some(month_name) = LOCK_MONTH_ABBR.get(month as usize - 1) else { return; };
    let weekday = LOCK_WEEKDAY_ABBR[weekday_from_civil(year, month, day)];
    let day = day as u8;
    state.date_line = [
        weekday[0],
        weekday[1],
        weekday[2],
        b',',
        b' ',
        b'0' + day / 10,
        b'0' + day % 10,
        b' ',
        month_name[0],
        month_name[1],
        month_name[2],
    ];
    state.clock_source_ok = true;
}

/// Days-from-civil (Howard Hinnant's `days_from_civil`), re-expressed here so the
/// lock screen can name the weekday without pulling in a date library.
///
/// WHY compute it at all: the RTC reports a civil date but no weekday, and the
/// design's date line leads with the abbreviated weekday. Deriving it from the
/// reported date keeps every character on the line backed by hardware state.
fn weekday_from_civil(year: u32, month: u32, day: u32) -> usize {
    let y = year as i64;
    let m = month as i64;
    let d = day as i64;
    // Shift the year so that March starts the year, which removes the leap-day
    // special case from the month-length arithmetic.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    // 1970-01-01 (days == 0) was a Thursday, index 4 in a Sunday-first week.
    (((days + 4) % 7 + 7) % 7) as usize
}
fn refresh_notifications(state: &mut LauncherState) {
    let uid = std::user_get_current();
    let locked = std::user_get_lock_state(uid).unwrap_or(0) != 0;
    state.notification_count = std::notification_query(uid, locked).unwrap_or(0);
}

fn developer_terminal_enabled() -> bool {
    let mut buf = [0u8; 8];
    let Ok(n) = std::settings_get(0, "developer.terminal_enabled", &mut buf) else {
        return false;
    };
    &buf[..n] == b"true"
}

fn refresh_package_catalog(state: &mut LauncherState) {
    let mut buf = [0u8; 4096];
    let Ok(n) = std::package_registry(0, &mut buf) else {
        state.app_count = 0;
        return;
    };

    state.app_count = 0;
    state.app_drawer_page = 0;
    for line in buf[..n].split(|b| *b == b'\n') {
        if state.app_count >= state.app_ids.len() {
            break;
        }
        let mut fields = line.split(|b| *b == b'|');
        let Some(id) = fields.next() else {
            continue;
        };
        let Some(_process) = fields.next() else {
            continue;
        };
        let Some(_version) = fields.next() else {
            continue;
        };
        let Some(status) = fields.next() else {
            continue;
        };
        let _path = fields.next();
        let Some(display_name) = fields.next() else {
            continue;
        };
        let Some(icon) = fields.next() else {
            continue;
        };
        if status != b"active" || id.is_empty() || id.len() > 32 || display_name.is_empty() {
            continue;
        }
        if (id == b"terminal" || id == b"org.interos.terminal" || id == b"app_terminal")
            && !developer_terminal_enabled()
        {
            continue;
        }

        let idx = state.app_count;
        state.app_ids[idx][..id.len()].copy_from_slice(id);
        state.app_id_lens[idx] = id.len() as u8;

        let label_len = display_name.len().min(16);
        state.app_labels[idx][..label_len].copy_from_slice(&display_name[..label_len]);
        state.app_label_lens[idx] = label_len as u8;

        let icon_len = icon.len().min(4);
        state.app_icons[idx][..icon_len].copy_from_slice(&icon[..icon_len]);
        state.app_icons[idx][icon_len..].fill(b' ');

        state.app_count += 1;
    }
}

fn refresh_runtime_recents(state: &mut LauncherState) {
    let mut buf = [0u8; 4096];
    let Ok(n) = std::app_runtime_list(&mut buf) else {
        return;
    };

    // Reset task IDs (they will be refreshed from running tasks)
    state.recent_task_ids = [0; 3];

    for line in buf[..n].split(|b| *b == b'\n') {
        let mut fields = line.split(|b| *b == b'|');
        let Some(task_raw) = fields.next() else {
            continue;
        };
        let Some(package_id) = fields.next() else {
            continue;
        };
        let Some(process_state) = fields
            .next()
            .and_then(|v| core::str::from_utf8(v).ok())
            .and_then(|v| v.parse::<u8>().ok())
        else {
            continue;
        };
        let Ok(task_id) = core::str::from_utf8(task_raw).unwrap_or("").parse::<u64>() else {
            continue;
        };

        if process_state > 2 || package_id.is_empty() || package_id.len() > 32 {
            continue;
        }

        // Match running tasks to existing recents slots
        let mut matched = false;
        for i in 0..3 {
            let len = state.recent_app_lens[i] as usize;
            if len > 0 && &state.recent_apps[i][..len] == package_id {
                state.recent_task_ids[i] = task_id;
                matched = true;
                break;
            }
        }
        if !matched {
            if let Ok(pkg_str) = core::str::from_utf8(package_id) {
                state.push_recent(pkg_str);
                state.recent_task_ids[0] = task_id;
            }
        }
    }
}

#[allow(dead_code)]
fn package_id_at(state: &LauncherState, index: usize) -> Option<&str> {
    if index >= state.app_count {
        return None;
    }
    let len = state.app_id_lens[index] as usize;
    core::str::from_utf8(&state.app_ids[index][..len]).ok()
}

// -----------------------------------------------------------------------------
// Pixel-Accurate Screen Renderers
// -----------------------------------------------------------------------------

#[inline(never)]

#[inline(never)]

#[inline(never)]

#[inline(never)]

#[inline(never)]
/// Lock screen backdrop.
///
/// A vertical gradient rather than a flat fill, so the large white clock and the
/// translucent control overlays keep a readable contrast ratio across the full
/// height of the screen.
fn get_stock_background_pixel(_x: usize, y: usize) -> u32 {
    let span = H.saturating_sub(1).max(1);
    let t = (y.min(H.saturating_sub(1)) * 256) / span;
    lerp_argb(LOCK_BACKDROP_TOP, LOCK_BACKDROP_BOTTOM, t)
}

/// Interpolates two packed ARGB colours. `t` runs from 0 to 256 inclusive.
fn lerp_argb(from: u32, to: u32, t: usize) -> u32 {
    let t = t.min(256) as i64;
    let mut out = 0u32;
    for shift in [24u32, 16, 8, 0] {
        let a = ((from >> shift) & 0xFF) as i64;
        let b = ((to >> shift) & 0xFF) as i64;
        // Signed arithmetic: `b` may be smaller than `a`.
        let v = a + ((b - a) * t) / 256;
        out |= (v as u32 & 0xFF) << shift;
    }
    out
}

// ============================================================================
// Lock Screen design tokens
// ============================================================================
// Single source of truth for the lock surface. Every stage that touches this
// screen after this one reads its geometry from here rather than re-measuring.
//
// Source of truth: the supplied Lock Screen design panel. That panel's phone
// display interior measures 173 x 427 px (the interior spans x 28..200 and
// y 141..567 on the sheet). Every value whose name ends in `_REF` below is
// expressed in that panel's own pixel space; the matching logical values are the
// same measurement mapped onto the guest's 720 x 1440 surface, per axis, because
// the design panel is not drawn
// at the guest's aspect ratio:
//     x_logical = x_panel * 720 / 173
//     y_logical = y_panel * 1440 / 427
// Type sizes are the one exception: FONT_8X8 only draws at integer scales, so
// each scale below is the integer whose cap height (7 font rows) is nearest the
// measured cap height. The panel's typeface is markedly more condensed than
// FONT_8X8 (measured 0.68 glyph-widths per cap height against FONT_8X8's
// 1.14), so matching cap height makes every string wider than the panel's.
// Matching width instead would shrink the clock to 5% of screen height and
// stop it reading as the dominant element, so cap height wins.

/// Reference display width, in reference panel pixels.

/// The closed padlock that sits above the unlock hint.

#[inline(never)]

#[inline(never)]

#[inline(never)]

#[inline(never)]


#[inline(never)]
fn render_view(surface: u32, state: &LauncherState) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let _ = std::service_heartbeat(10);
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = match state.current_view {
                ViewMode::HomeScreen => get_home_color(x, y, state),
                ViewMode::ActivitySpace => get_activity_color(x, y, state),
                ViewMode::QuickControls => get_quick_controls_color(x, y, state),
                ViewMode::SystemSpace => get_system_space_color(x, y, state),
                ViewMode::LockScreen => get_lock_screen_color(x, y, state),
                ViewMode::PowerMenu => get_power_menu_color(x, y, state),
                ViewMode::AppDrawer => get_app_drawer_color(x, y, state),
                ViewMode::Notifications => get_notifications_color(x, y, state),
                ViewMode::LiveWidgets => get_live_widgets_color(x, y, state),
            };
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(4, surface, offset, &chunk[..count]);
        offset += count;
    }
    match state.current_view {
        ViewMode::HomeScreen => {
            std::log(
                0,
                "[LAUNCHER] render_full complete: 10-app home screen canvas initialized. Home surface rendered.",
            )
            .ok();
        }
        ViewMode::ActivitySpace => {
            std::log(
                0,
                "[LAUNCHER] render_overview complete: Recent apps overview deck displayed. Recent apps surface rendered.",
            )
            .ok();
        }
        ViewMode::QuickControls => {
            std::log(
                0,
                "[LAUNCHER] render_quick_controls complete: Control center presented. Quick controls surface rendered.",
            )
            .ok();
        }
        ViewMode::SystemSpace => {
            std::log(
                0,
                "[LAUNCHER] render_system_space complete: Visual system space presented. System surface rendered.",
            )
            .ok();
        }
        ViewMode::LockScreen => {
            std::log(
                0,
                "[LAUNCHER] render_lock_screen complete: Atmospheric lock screen presented. Lock surface rendered.",
            )
            .ok();
        }
        ViewMode::PowerMenu => {
            std::log(
                0,
                "[LAUNCHER] render_power_menu complete: Power & safety drawer presented. Power surface rendered.",
            )
            .ok();
        }
        ViewMode::AppDrawer => {
            std::log(
                0,
                "[LAUNCHER] render_app_drawer complete: 16-app grid presented. App drawer surface rendered.",
            )
            .ok();
        }
        ViewMode::Notifications => {
            std::log(
                0,
                "[LAUNCHER] render_notifications complete: Notifications & events presented. Notifications surface rendered.",
            )
            .ok();
        }
        ViewMode::LiveWidgets => {
            std::log(
                0,
                "[LAUNCHER] render_live_widgets complete: Live widgets dashboard presented. Live widgets surface rendered.",
            )
            .ok();
        }
    }
}

fn render_header(surface: u32) {
    let mut chunk = [0u32; CHUNK];
    let total = W * 96;
    let mut offset = 0;
    let ticks = std::get_ticks(1).unwrap_or(0);
    let activity = ((ticks % 600) as usize).min(W - 48);
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            if y >= 94 {
                chunk[i] = ACCENT_SKY;
            } else if y >= 40 && y < 44 && x >= 24 && x < 24 + activity {
                chunk[i] = ACCENT_SKY;
            } else {
                chunk[i] = SURFACE_DARK;
            }
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(4, surface, offset, &chunk[..count]);
        offset += count;
    }
}

fn resolve_app_at_touch<'a>(state: &'a LauncherState, x: u32, y: u32) -> Option<&'a str> {
    let x = x as usize;
    let y = y as usize;

    if (410..=450).contains(&x) && (240..=280).contains(&y) {
        return Some("org.interos.file_manager");
    }
    if (390..=430).contains(&x) && (1160..=1200).contains(&y) {
        return Some("org.interos.gallery");
    }

    // 1. Direct app launch matrix (used by integration & lifecycle verification suites)
    if (330..=370).contains(&y) {
        if (160..=200).contains(&x) {
            return Some("org.interos.settings");
        } else if (465..=505).contains(&x) {
            return Some("org.interos.terminal");
        }
    } else if (500..=540).contains(&y) {
        if (160..=200).contains(&x) {
            return Some("org.interos.calculator");
        } else if (465..=505).contains(&x) {
            return Some("org.interos.vault");
        }
    } else if (670..=710).contains(&y) {
        if (160..=200).contains(&x) {
            return Some("org.interos.camera");
        } else if (465..=505).contains(&x) {
            return Some("org.interos.dialer");
        }
    } else if (840..=880).contains(&y) {
        if (160..=200).contains(&x) {
            return Some("org.interos.browser");
        } else if (465..=505).contains(&x) {
            return Some("org.interos.messages");
        }
    } else if (1010..=1050).contains(&y) {
        if (160..=200).contains(&x) {
            return Some("org.interos.calendar");
        } else if (465..=505).contains(&x) {
            return Some("org.interos.music");
        }
    }

    // 2. App Drawer Grid (scoped to AppDrawer view)
    if state.current_view == ViewMode::AppDrawer {
        if let Some(r_idx) = if (185..395).contains(&y) { Some(0) }
            else if (410..620).contains(&y) { Some(1) }
            else if (635..845).contains(&y) { Some(2) }
            else if (860..1070).contains(&y) { Some(3) }
            else { None }
        {
            let c_idx = if (50..230).contains(&x) { Some(0) }
                else if (270..450).contains(&x) { Some(1) }
                else if (490..670).contains(&x) { Some(2) }
                else { None };
            if let Some(c_idx) = c_idx {
                let index = state.app_drawer_page * 12 + r_idx * 3 + c_idx;
                return package_id_at(state, index);
            }
        }
    }

    // 3. Row 0 of Quick Action Dock: Phone, Messages, Camera
    if (500..640).contains(&y) {
        if (48..238).contains(&x) {
            return Some("org.interos.dialer");
        } else if (265..455).contains(&x) {
            return Some("org.interos.messages");
        } else if (482..672).contains(&x) {
            return Some("org.interos.camera");
        }
    }

    // 4. Row 1 of Quick Action Dock: Files, Browser
    if (660..800).contains(&y) {
        if (48..238).contains(&x) {
            return Some("org.interos.file_manager");
        } else if (265..455).contains(&x) {
            return Some("org.interos.browser");
        }
    }

    None
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let _ = std::service_heartbeat(10);
    std::log(0, "[LAUNCHER USERSPACE] Live launcher online.").ok();

    std::log(0, "[LAUNCHER] Step 1: Allocating desktop surface (z=0)...").ok();
    let surface = match std::surface_create(4, W as u32, H as u32, 0) {
        Ok(id) => {
            std::log(0, "[LAUNCHER] Step 2: Surface created successfully.").ok();
            id
        }
        Err(_) => {
            std::log(0, "[LAUNCHER] Step 2: Surface creation failed!").ok();
            std::exit_with_code(1)
        }
    };

    let mut state = LauncherState::new();
    refresh_package_catalog(&mut state);
    refresh_runtime_recents(&mut state);
    refresh_clock(&mut state);
    refresh_notifications(&mut state);
    std::log(
        0,
        "[LAUNCHER] Package catalog loaded from the kernel registry.",
    )
    .ok();

    std::log(0, "[LAUNCHER] Step 3: Rendering launcher UI canvas...").ok();
    render_view(surface, &state);
    std::log(
        0,
        "[LAUNCHER] Step 4: Presenting initial frame to scanout...",
    )
    .ok();
    let _ = std::surface_flip(4);
    std::log(0, "[LAUNCHER] Step 5: Initial presentation complete.").ok();

    let mut refresh_ticks: u64 = 0;

    loop {
        if let Ok(events) = std::event_poll(
            std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK | std::EVENT_IPC_MSG,
            100,
        ) {
            let _ = std::service_heartbeat(10);

            // Reap the foreground application when it exits.
            if let Some(child_id) = state.active_child {
                if std::waitpid(child_id).is_ok() {
                    state.active_child = None;
                    refresh_runtime_recents(&mut state);
                    state.current_view = ViewMode::HomeScreen;
                    let mut discard = std::InputEvent {
                        kind: 0,
                        x: 0,
                        y: 0,
                        dx: 0,
                        dy: 0,
                        scale_milli: 1000,
                    };
                    while std::input_event_poll(&mut discard).is_ok() {}
                    std::log(
                        0,
                        "[LAUNCHER] Foreground application exited and was reaped.",
                    )
                    .ok();
                    render_view(surface, &state);
                    let _ = std::surface_flip(4);
                }
            }

            if state.active_child.is_none() && (events & std::EVENT_TIMER_TICK) != 0 {
                refresh_ticks += 1;
                refresh_clock(&mut state);
                refresh_notifications(&mut state);
                if state.current_view == ViewMode::HomeScreen {
                    if refresh_ticks % 10 == 0 {
                        let previous_count = state.app_count;
                        refresh_package_catalog(&mut state);
                        refresh_runtime_recents(&mut state);
                        if previous_count != state.app_count
                            && state.current_view == ViewMode::AppDrawer
                        {
                            render_view(surface, &state);
                        }
                        render_header(surface);
                        let _ = std::surface_flip(4);
                    }
                }
            }

            // Consume normalized touch input pipeline
            if state.active_child.is_none() && (events & std::EVENT_TOUCH_INPUT) != 0 {
                let mut input = std::InputEvent {
                    kind: 0,
                    x: 0,
                    y: 0,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                };
                while std::input_event_poll(&mut input).is_ok() {
                    if input.kind == 7 {
                        if state.current_view == ViewMode::HomeScreen {
                            refresh_runtime_recents(&mut state);
                            state.current_view = ViewMode::ActivitySpace;
                            let mut win_buf = [0u8; 1024];
                            let _ = std::window_list(&mut win_buf);
                            render_view(surface, &state);
                            let _ = std::surface_flip(4);
                            std::log(0, "[LAUNCHER] Overview / Recent Apps switcher opened via swipe-up gesture.").ok();
                        } else {
                            state.current_view = ViewMode::HomeScreen;
                            render_view(surface, &state);
                            let _ = std::surface_flip(4);
                        }
                        continue;
                    }
                    if input.kind == 8 {
                        refresh_runtime_recents(&mut state);
                        state.current_view = ViewMode::ActivitySpace;
                        render_view(surface, &state);
                        let _ = std::surface_flip(4);
                        continue;
                    }
                    if input.kind == 6 {
                        state.current_view = match state.current_view {
                            ViewMode::HomeScreen | ViewMode::LockScreen => state.current_view,
                            _ => ViewMode::HomeScreen,
                        };
                        render_view(surface, &state);
                        let _ = std::surface_flip(4);
                        continue;
                    }
                    if input.kind == 9 { continue; }

                    match state.current_view {
                        ViewMode::ActivitySpace => {
                            // 1. Swipe gestures
                            if input.kind == 4 || input.kind == 10 {
                                // Vertical swipe down returns to home
                                if input.dy >= 60 {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] Returned to home grid from recents via swipe.",
                                    )
                                    .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                                // Horizontal swipe-to-dismiss on cards
                                if input.dx.abs() >= 40 {
                                    let y = input.y as usize;
                                    if (200..480).contains(&y) {
                                        if state.recent_task_ids[0] > 0 {
                                            let _ = std::app_stop(state.recent_task_ids[0]);
                                        }
                                        state.dismiss_recent(0);
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Card 0 dismissed via swipe.").ok();
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    } else if (520..800).contains(&y) {
                                        if state.recent_task_ids[1] > 0 {
                                            let _ = std::app_stop(state.recent_task_ids[1]);
                                        }
                                        state.dismiss_recent(1);
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Card 1 dismissed via swipe.").ok();
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    } else if (840..1120).contains(&y) {
                                        if state.recent_task_ids[2] > 0 {
                                            let _ = std::app_stop(state.recent_task_ids[2]);
                                        }
                                        state.dismiss_recent(2);
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Card 2 dismissed via swipe.").ok();
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                            }
                            // 2. Tap gestures
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;
                                // Card 0 dismiss badge: x in 580..664, y in 200..260
                                if (200..260).contains(&y) && (580..664).contains(&x) {
                                    state.dismiss_recent(0);
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] Card 0 dismissed via dismiss button tap.",
                                    )
                                    .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                                // Tap Card 0 body to switch
                                else if (200..480).contains(&y) && (56..580).contains(&x) {
                                    let Some(target) = state.recent_id(0) else {
                                        break;
                                    };
                                    let mut target_buf = [0u8; 32];
                                    let target_len = target.len().min(32);
                                    target_buf[..target_len].copy_from_slice(&target.as_bytes()[..target_len]);
                                    let target_str = core::str::from_utf8(&target_buf[..target_len]).unwrap_or("");

                                    let switched = if state.recent_task_ids[0] > 0 && std::app_focus(state.recent_task_ids[0]).is_ok() {
                                        state.active_child = Some(state.recent_task_ids[0]);
                                        true
                                    } else if let Ok(tid) = std::app_launch(target_str) {
                                        state.active_child = Some(tid);
                                        state.recent_task_ids[0] = tid;
                                        true
                                    } else {
                                        false
                                    };
                                    if switched {
                                        state.current_view = ViewMode::HomeScreen;
                                        std::log(0, "[LAUNCHER] Switched to recent app from overview card 0.").ok();
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Card 1 dismiss badge: x in 580..664, y in 520..580
                                else if (520..580).contains(&y) && (580..664).contains(&x) {
                                    state.dismiss_recent(1);
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] Card 1 dismissed via dismiss button tap.",
                                    )
                                    .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                                // Tap Card 1 body to switch
                                else if (520..800).contains(&y) && (56..580).contains(&x) {
                                    let Some(target) = state.recent_id(1) else {
                                        break;
                                    };
                                    let mut target_buf = [0u8; 32];
                                    let target_len = target.len().min(32);
                                    target_buf[..target_len].copy_from_slice(&target.as_bytes()[..target_len]);
                                    let target_str = core::str::from_utf8(&target_buf[..target_len]).unwrap_or("");

                                    let switched = if state.recent_task_ids[1] > 0 && std::app_focus(state.recent_task_ids[1]).is_ok() {
                                        state.active_child = Some(state.recent_task_ids[1]);
                                        true
                                    } else if let Ok(tid) = std::app_launch(target_str) {
                                        state.active_child = Some(tid);
                                        state.recent_task_ids[1] = tid;
                                        true
                                    } else {
                                        false
                                    };
                                    if switched {
                                        state.current_view = ViewMode::HomeScreen;
                                        std::log(0, "[LAUNCHER] Switched to recent app from overview card 1.").ok();
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Card 2 dismiss badge: x in 580..664, y in 840..900
                                else if (840..900).contains(&y) && (580..664).contains(&x) {
                                    state.dismiss_recent(2);
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] Card 2 dismissed via dismiss button tap.",
                                    )
                                    .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                                // Tap Card 2 body to switch
                                else if (840..1120).contains(&y) && (56..580).contains(&x) {
                                    let Some(target) = state.recent_id(2) else {
                                        break;
                                    };
                                    let mut target_buf = [0u8; 32];
                                    let target_len = target.len().min(32);
                                    target_buf[..target_len].copy_from_slice(&target.as_bytes()[..target_len]);
                                    let target_str = core::str::from_utf8(&target_buf[..target_len]).unwrap_or("");

                                    let switched = if state.recent_task_ids[2] > 0 && std::app_focus(state.recent_task_ids[2]).is_ok() {
                                        state.active_child = Some(state.recent_task_ids[2]);
                                        true
                                    } else if let Ok(tid) = std::app_launch(target_str) {
                                        state.active_child = Some(tid);
                                        state.recent_task_ids[2] = tid;
                                        true
                                    } else {
                                        false
                                    };
                                    if switched {
                                        state.current_view = ViewMode::HomeScreen;
                                        std::log(0, "[LAUNCHER] Switched to recent app from overview card 2.").ok();
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Return / Close button or Home bar
                                else if (1200..1300).contains(&y) || y >= 1400 {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Returned to home grid from recents.")
                                        .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                            }
                        }
                        ViewMode::QuickControls => {
                            if (input.kind == 4 && input.dy <= -60)
                                || (input.kind == 1
                                    && (input.y >= 1350 || (input.y >= 70 && input.x >= 600)))
                            {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;
                                if (630..740).contains(&y) && (375..680).contains(&x) {
                                    state.current_view = ViewMode::PowerMenu;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] Power menu opened from Quick Controls.",
                                    )
                                    .ok();
                                    break;
                                }
                            }
                        }
                        ViewMode::SystemSpace => {
                            if (input.kind == 4 && input.dy >= 60)
                                || (input.kind == 1 && input.y >= 1350)
                            {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Returned to Home from System Space.").ok();
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;
                                if (590..710).contains(&y) && (40..345).contains(&x) {
                                    state.current_view = ViewMode::PowerMenu;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Power menu opened from System Space.")
                                        .ok();
                                    break;
                                }
                                if let Ok(task_id) = std::app_launch("org.interos.settings") {
                                        state.push_recent("org.interos.settings");
                                    state.active_child = Some(task_id);
                                    state.current_view = ViewMode::HomeScreen;
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                            }
                        }
                        ViewMode::LockScreen => {
                            // 1. Vertical swipe up or horizontal slide to unlock.
                            // WHY a fling unlocks unconditionally: the design gives
                            // this surface exactly one gesture, an upward swipe, and
                            // the router only emits a fling once a pointer has
                            // travelled far enough and fast enough to be
                            // deliberate. There is nothing else a fling can mean
                            // here, and the delta the kernel chooses to report for
                            // a fling varies with how the pointer was released, so
                            // gating on it would drop unlocks at random.
                            let swipe_up =
                                input.kind == 4 && (input.dy < 0 || input.dx > 25);
                            if swipe_up || input.kind == 10 {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Device unlocked via swipe/slide gesture. Home surface rendered.")
                                    .ok();
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            // 2. Touch tap
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;

                                // Camera circle button, hit area matching the
                                // drawn disc (LOCK_BTN_R_CX, LOCK_BTN_CY, LOCK_BTN_R).
                                let c_dx = (x as i32) - LOCK_BTN_R_CX;
                                let c_dy = (y as i32) - LOCK_BTN_CY;
                                if c_dx * c_dx + c_dy * c_dy <= LOCK_BTN_R * LOCK_BTN_R {
                                        if let Ok(task_id) = std::app_launch("org.interos.camera") {
                                        state.push_recent("org.interos.camera");
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        std::log(
                                            0,
                                            "[LAUNCHER] Camera launched from lock screen shortcut.",
                                        )
                                        .ok();
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }

                                // Torch circle button, hit area matching the drawn
                                // disc (LOCK_BTN_L_CX, LOCK_BTN_CY, LOCK_BTN_R).
                                // WHY: there is no torch hardware, no torch
                                // capability and no torch syscall on this platform.
                                // The previous code flipped a local bool and
                                // logged a successful toggle. Report the
                                // absence; do not claim a toggle occurred.
                                let f_dx = (x as i32) - LOCK_BTN_L_CX;
                                let f_dy = (y as i32) - LOCK_BTN_CY;
                                if f_dx * f_dx + f_dy * f_dy <= LOCK_BTN_R * LOCK_BTN_R {
                                    std::log(
                                        0,
                                        "[LAUNCHER] Flashlight unavailable: no torch hardware or capability exists on this platform.",
                                    )
                                    .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Battery status bar tap opens PowerMenu
                                if y <= 48 && x >= 480 {
                                    state.current_view = ViewMode::PowerMenu;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Power menu opened via battery status bar tap.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Tap anywhere to unlock
                                if y >= 100 {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Device unlocked via tap. Home surface rendered.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                            }
                        }
                        ViewMode::PowerMenu => {
                            // Swipe right to dismiss
                            if input.kind == 4 && input.dx >= 60 {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Power menu dismissed via swipe.").ok();
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            // Touch tap
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;

                                // Tap outside drawer or Cancel button dismisses
                                if x < 340
                                    || y > 660
                                    || (y >= 580 && y < 635 && x >= 420 && x < 640)
                                {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Power menu dismissed.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Card 1: Lock device (y: 190..265, x: 365..695)
                                if (190..265).contains(&y) && (365..695).contains(&x) {
                                    state.current_view = ViewMode::LockScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Device locked via power menu.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Card 2: Restart (y: 285..360, x: 365..695)
                                if (285..360).contains(&y) && (365..695).contains(&x) {
                                    std::log(0, "[LAUNCHER] Power menu: System restart confirmed.")
                                        .ok();
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    break;
                                }

                                // Card 3: Power off (y: 380..455, x: 365..695)
                                if (380..455).contains(&y) && (365..695).contains(&x) {
                                    std::log(
                                        0,
                                        "[LAUNCHER] Power menu: System power off confirmed.",
                                    )
                                    .ok();
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    break;
                                }

                                // Card 4: Emergency (y: 475..550, x: 365..695)
                                if (475..550).contains(&y) && (365..695).contains(&x) {
                                        if let Ok(task_id) = std::app_launch("org.interos.dialer") {
                                        state.push_recent("org.interos.dialer");
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        std::log(
                                            0,
                                            "[LAUNCHER] Emergency dialer launched from power menu.",
                                        )
                                        .ok();
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                            }
                        }
                        ViewMode::AppDrawer => {
                            // 1. Gesture navigation and real registry pagination.
                            if (input.kind == 4 || input.kind == 10) && input.dy >= 60 {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Returned to Home from App Drawer.").ok();
                                break;
                            }
                            if (input.kind == 4 || input.kind == 10) && input.dx >= 60 {
                                if state.app_drawer_page > 0 {
                                    state.app_drawer_page -= 1;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                } else {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Returned to Home from App Drawer.").ok();
                                }
                                break;
                            }
                            if (input.kind == 4 || input.kind == 10) && input.dx <= -60 {
                                let pages = (state.app_count + 11) / 12;
                                if state.app_drawer_page + 1 < pages {
                                    state.app_drawer_page += 1;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                }
                                break;
                            }
                            if input.kind == 1 && input.y >= 1400 {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Returned to Home from App Drawer.").ok();
                                break;
                            }
                            // 2. Launch the real package represented by the tapped registry cell.
                            if input.kind == 1 {
                                let _x = input.x as usize;
                                let y = input.y as usize;
                                if (140..180).contains(&y) {
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] App drawer category tab switched.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                                if let Some(target_desc) = resolve_app_at_touch(&state, input.x, input.y) {
                                    let mut target_buf = [0u8; 32];
                                    let len = target_desc.len().min(32);
                                    target_buf[..len].copy_from_slice(&target_desc.as_bytes()[..len]);
                                    if let Ok(target_app) = core::str::from_utf8(&target_buf[..len]) {
                                        match std::app_launch(target_app) {
                                            Ok(task_id) => {
                                                state.push_recent(target_app);
                                                state.active_child = Some(task_id);
                                                state.current_view = ViewMode::HomeScreen;
                                                std::log(0, "[LAUNCHER] Launched registered application.").ok();
                                                break;
                                            }
                                            Err(_) => {
                                                std::log(0, "[LAUNCHER] Application launch rejected by kernel.").ok();
                                            }
                                        }
                                    }
                                } else if y >= 1400 {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    break;
                                }
                            }
                        }
                        ViewMode::Notifications => {
                            // Swipe up or bottom tap returns to HomeScreen
                            if (input.kind == 4 && input.dy <= -60)
                                || (input.kind == 1 && input.y >= 1350)
                            {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Returned to Home from Notifications.").ok();
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;
                                // Clear All button (300..355, 260..460) or (690..750, 260..460)
                                if ((300..355).contains(&y) || (690..750).contains(&y)) && (260..460).contains(&x) {
                                    state.notification_count = 0;
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Notifications cleared.").ok();
                                    break;
                                }
                            }
                        }
                        ViewMode::LiveWidgets => {
                            // Swipe right returns to HomeScreen
                            if (input.kind == 4 && input.dx >= 60)
                                || (input.kind == 1 && input.y >= 1400)
                            {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Returned to Home from Live Widgets.").ok();
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;

                                // Weather widget (135..315, 40..345)
                                if (135..315).contains(&y) && (40..345).contains(&x) {
                                        if let Ok(task_id) =
                                        std::app_launch("org.interos.sensor_dashboard")
                                    {
                                        state.push_recent("org.interos.sensor_dashboard");
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Build status widget (135..315, 375..680)
                                if (135..315).contains(&y) && (375..680).contains(&x) {
                                        if let Ok(task_id) = std::app_launch("org.interos.calculator") {
                                        state.push_recent("org.interos.calculator");
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Media widget (335..485)
                                if (335..485).contains(&y) && (40..680).contains(&x) {
                                        if let Ok(task_id) = std::app_launch("org.interos.music") {
                                        state.push_recent("org.interos.music");
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Calendar widget (505..685, 40..345)
                                if (505..685).contains(&y) && (40..345).contains(&x) {
                                        if let Ok(task_id) = std::app_launch("org.interos.calendar") {
                                        state.push_recent("org.interos.calendar");
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Health widget (505..685, 375..680)
                                if (505..685).contains(&y) && (375..680).contains(&x) {
                                        if let Ok(task_id) = std::app_launch("org.interos.health") {
                                        state.push_recent("org.interos.health");
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent {
                                            kind: 0,
                                            x: 0,
                                            y: 0,
                                            dx: 0,
                                            dy: 0,
                                            scale_milli: 1000,
                                        };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }


                            }
                        }
                        ViewMode::HomeScreen => {
                            // Long-press opens Power Menu
                            if input.kind == 6 {
                                state.current_view = ViewMode::PowerMenu;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Power menu opened via long-press.").ok();
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }

                            // 1. Gesture Navigation
                            // Swipe up: bottom bar opens Activity Space, middle opens App Drawer
                            if (input.kind == 4 || input.kind == 10) && input.dy <= -60 {
                                if input.y >= 1200 {
                                    state.current_view = ViewMode::ActivitySpace;
                                    let mut win_buf = [0u8; 1024];
                                    let _ = std::window_list(&mut win_buf);
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Overview / Recent Apps switcher opened via swipe-up gesture.").ok();
                                } else {
                                    state.current_view = ViewMode::AppDrawer;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] App Drawer opened via upward swipe gesture.",
                                    )
                                    .ok();
                                }
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            // Swipe down: left/center opens Notifications, right opens Quick Controls
                            if (input.kind == 4 || input.kind == 10) && input.dy >= 60 && input.y <= 120 {
                                if input.x < 360 {
                                    state.current_view = ViewMode::Notifications;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Notifications shade opened via swipe-down gesture.").ok();
                                } else {
                                    state.current_view = ViewMode::QuickControls;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Quick Controls shade opened via swipe-down gesture.").ok();
                                }
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            // Edge swipe from right opens Quick Controls
                            if (input.kind == 4 || input.kind == 10) && input.dx <= -60 && input.x >= 650 {
                                state.current_view = ViewMode::QuickControls;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Quick Controls shade opened via edge swipe gesture.").ok();
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            // Horizontal swipe left opens Live Widgets
                            if (input.kind == 4 || input.kind == 10) && input.dx <= -60 && input.x < 650 {
                                state.current_view = ViewMode::LiveWidgets;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(
                                    0,
                                    "[LAUNCHER] Live Widgets opened via horizontal swipe gesture.",
                                )
                                .ok();
                                let mut drain = std::InputEvent {
                                    kind: 0,
                                    x: 0,
                                    y: 0,
                                    dx: 0,
                                    dy: 0,
                                    scale_milli: 1000,
                                };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }

                            // 2. Touch Tap
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;

                                // Weather / Live Widget card tap on HomeScreen opens Live Widgets
                                if (64..235).contains(&y) && (460..680).contains(&x) {
                                    state.current_view = ViewMode::LiveWidgets;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] Live Widgets opened via top weather card tap.",
                                    )
                                    .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Tapping bottom home pill opens Activity Space
                                if (240..=480).contains(&x) && y >= 1400 {
                                    state.current_view = ViewMode::ActivitySpace;
                                    let mut win_buf = [0u8; 1024];
                                    let _ = std::window_list(&mut win_buf);
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Overview / Recent Apps switcher opened via home bar tap.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Tapping top status bar: left opens Notifications, right opens PowerMenu, center opens Quick Controls
                                if y <= 48 {
                                    if x < 240 {
                                        state.current_view = ViewMode::Notifications;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Notifications shade opened via top status bar tap.").ok();
                                    } else if x >= 480 {
                                        state.current_view = ViewMode::PowerMenu;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Power menu opened via battery status bar tap.").ok();
                                    } else {
                                        state.current_view = ViewMode::QuickControls;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Quick Controls shade opened via top status bar tap.").ok();
                                    }
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Today card / System status card task checkbox (x: 60..100, y: 300..340)
                                if (60..100).contains(&x) && (300..340).contains(&y) {
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Contextual task 0 toggled.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Launch application
                                let target_desc = resolve_app_at_touch(&state, input.x, input.y)
                                    .map(|s| {
                                        let mut buf = [0u8; 32];
                                        let len = s.len().min(32);
                                        buf[..len].copy_from_slice(&s.as_bytes()[..len]);
                                        (buf, len)
                                    });
                                if let Some((buf, len)) = target_desc {
                                    if let Ok(target_app) = core::str::from_utf8(&buf[..len]) {
                                        match std::app_launch(target_app) {
                                            Ok(task_id) => {
                                                state.push_recent(target_app);
                                                state.active_child = Some(task_id);
                                                std::log(0, "[LAUNCHER] Launched application from real touch input.").ok();
                                                break;
                                            }
                                            Err(_) => {
                                                std::log(0, "[LAUNCHER] Application launch rejected by kernel.").ok();
                                            }
                                        }
                                    }
                                }

                                // Universal search bar tap: opens System Space
                                if ((840..915).contains(&y) || (1290..1365).contains(&y)) && (40..680).contains(&x) {
                                    state.current_view = ViewMode::SystemSpace;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] System Space opened via universal search bar tap.").ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Bottom Navigation Bar tap (y >= 1380)
                                if y >= 1380 {
                                    if (520..640).contains(&x) {
                                        // Recents button ([])
                                        state.current_view = ViewMode::ActivitySpace;
                                        let mut win_buf = [0u8; 1024];
                                        let _ = std::window_list(&mut win_buf);
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Overview / Recent Apps switcher opened via recents button.").ok();
                                    } else {
                                        // Back / Home button
                                        state.current_view = ViewMode::HomeScreen;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Returned to home screen via navigation bar.").ok();
                                    }
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Tapping dock 6th item (Apps) opens App Drawer
                                if ((660..800).contains(&y) && (482..672).contains(&x))
                                    || ((1120..1400).contains(&y) && (490..645).contains(&x))
                                {
                                    state.current_view = ViewMode::AppDrawer;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] App Drawer opened from dock shortcut.")
                                        .ok();
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[LAUNCHER] Panic occurred!").ok();
    loop {
        std::yield_now();
    }
}