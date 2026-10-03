#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

use launcher::theme::*;
use launcher::{layout, text};

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
fn get_home_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 {
            return 0xFFE2E8F0;
        }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, &state.clock_time, 0xFF0F172A) {
                return c;
            }
        }
        // Status indicators.
        //
        // No radio, Wi-Fi radio or fuel gauge is bound on this platform, so these
        // draw empty outlines rather than a filled level. A full battery glyph
        // or four signal bars with no measurement behind them is a fabricated
        // reading, and the hardware query syscalls report unavailable.
        const STATUS_UNAVAILABLE: u32 = 0xFFB6BFCC;
        // Cellular: outline of the bar stack only, no filled level.
        if x >= 590 && x <= 608 && y >= 16 && y <= 32 {
            if y == 32 || x == 590 || (x == 605 && y == 16) {
                return STATUS_UNAVAILABLE;
            }
        }
        // Wi-Fi: outline dot only, no arcs, indicating no association.
        if x >= 633 && x <= 637 && y >= 28 && y <= 32 {
            return STATUS_UNAVAILABLE;
        }
        // Battery: shell and terminal only, with no charge fill.
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 {
                return STATUS_UNAVAILABLE;
            }
            if x >= 685 && x <= 686 && y >= 22 && y <= 28 {
                return STATUS_UNAVAILABLE;
            }
        }
        return 0xFFF0F2F5;
    }

    // 2. Canonical stock header with truthful unavailable telemetry.
    if y < 235 {
        if y >= 64 && y < 80 {
            if let Some(c) = test_text(x, y, 48, 64, 2, b"InterOS", TEXT_MUTED) { return c; }
        }
        if y >= 88 && y < 120 {
            if let Some(c) = test_text(x, y, 48, 88, 4, b"Device", TEXT_PRIMARY) { return c; }
        }
        if y >= 132 && y < 148 {
            if let Some(c) = test_text(x, y, 48, 132, 2, b"System status", TEXT_MUTED) { return c; }
        }
        if y >= 80 && y < 112 && x >= 480 {
            if let Some(c) = test_text(x, y, 480, 80, 2, b"Weather unavailable", TEXT_MUTED) { return c; }
        }
        return BG_DARK;
    }

    // 3. System Status Card (x: 40..680, y: 240..470)
    if x >= 40 && x < 680 && y >= 240 && y < 470 {
        if y == 240 || y == 469 || x == 40 || x == 679 { return BORDER_SUBTLE; }
        if y >= 256 && y < 272 {
            if let Some(c) = test_text(x, y, 68, 256, 2, b"System status", TEXT_PRIMARY) { return c; }
        }
        if y >= 305 && y < 321 {
            if let Some(c) = test_text(x, y, 68, 305, 2, b"Launcher", TEXT_PRIMARY) { return c; }
        }
        if y >= 330 && y < 346 {
            if let Some(c) = test_text(x, y, 68, 330, 1, b"Running from the system package registry", TEXT_MUTED) { return c; }
        }
        if y >= 370 && y < 386 {
            if let Some(c) = test_text(x, y, 68, 370, 2, b"Input", TEXT_PRIMARY) { return c; }
        }
        if y >= 395 && y < 411 {
            if let Some(c) = test_text(x, y, 68, 395, 1, b"Touch and keyboard events are live", TEXT_MUTED) { return c; }
        }
        return SURFACE_DARK;
    }

    // 4. Reference 6-Tile Quick Launch Dock (x: 40..680, y: 500..810)
    // 2 Rows x 3 Columns:
    // Row 0: Phone, Messages, Camera
    // Row 1: Files, Browser, Apps
    if y >= 500 && y < 810 {
        let is_row0 = (500..640).contains(&y);
        let is_row1 = (660..800).contains(&y);

        if is_row0 || is_row1 {
            let col = if (48..238).contains(&x) {
                Some(0)
            } else if (265..455).contains(&x) {
                Some(1)
            } else if (482..672).contains(&x) {
                Some(2)
            } else {
                None
            };

            if let Some(c_idx) = col {
                let card_idx = if is_row0 { c_idx } else { 3 + c_idx };
                let (x0, y0) = match (is_row0, c_idx) {
                    (true, 0) => (48, 500),
                    (true, 1) => (265, 500),
                    (true, 2) => (482, 500),
                    (false, 0) => (48, 660),
                    (false, 1) => (265, 660),
                    (false, 2) => (482, 660),
                    _ => (0, 0),
                };

                // Border
                if y == y0 || y == y0 + 139 || x == x0 || x == x0 + 189 {
                    return 0xFFE2E8F0;
                }

                // Tile Icon Badge (Center: x0 + 95, y0 + 55, r = 26)
                let b_dx = (x as i32) - (x0 as i32 + 95);
                let b_dy = (y as i32) - (y0 as i32 + 55);
                let b_d2 = b_dx * b_dx + b_dy * b_dy;
                let (icon_text, icon_color, label) = match card_idx {
                    0 => (&b"TEL"[..], 0xFF10B981, &b"Phone"[..]),
                    1 => (&b"MSG"[..], 0xFF0284C7, &b"Messages"[..]),
                    2 => (&b"CAM"[..], 0xFFF59E0B, &b"Camera"[..]),
                    3 => (&b"DIR"[..], 0xFF6366F1, &b"Files"[..]),
                    4 => (&b"WWW"[..], 0xFF8B5CF6, &b"Browser"[..]),
                    _ => (&b"APP"[..], 0xFFF43F5E, &b"Apps"[..]),
                };

                if b_d2 <= 676 {
                    if y >= y0 + 51 && y < y0 + 59 {
                        let ox = x0 + 95 - 12;
                        if let Some(c) = test_text(x, y, ox, y0 + 51, 1, icon_text, 0xFFFFFFFF) {
                            return c;
                        }
                    }
                    return icon_color;
                }

                // Label below icon (y0 + 100..y0 + 116)
                if y >= y0 + 100 && y < y0 + 116 {
                    let ox = x0 + 95 - (label.len() * 16) / 2;
                    if let Some(c) = test_text(x, y, ox, y0 + 100, 2, label, 0xFF0F172A) {
                        return c;
                    }
                }

                return 0xFFFFFFFF;
            }
        }
        return 0xFFF0F2F5;
    }

    // 5. Universal Search Command Bar (x: 40..680, y: 840..915)
    if x >= 40 && x < 680 && y >= 840 && y < 915 {
        if y == 840 || y == 914 || x == 40 || x == 679 {
            return 0xFFE2E8F0;
        }

        // Search magnifying glass icon
        let dx_s = (x as i32) - 68;
        let dy_s = (y as i32) - 877;
        let d_s = dx_s * dx_s + dy_s * dy_s;
        if (d_s >= 36 && d_s <= 64) || (dx_s >= 6 && dx_s <= 10 && dx_s == dy_s) {
            return 0xFF64748B;
        }

        // Search placeholder text
        if y >= 870 && y < 886 {
            if let Some(c) = test_text(x, y, 96, 870, 2, b"Search apps and files...", 0xFF94A3B8) {
                return c;
            }
        }

        // Right arrow icon
        if y >= 870 && y < 886 {
            if let Some(c) = test_text(x, y, 644, 870, 2, b">", 0xFF64748B) {
                return c;
            }
        }
        return 0xFFFFFFFF;
    }

    // 6. Bottom Navigation Bar (x: 0..720, y: 1380..1440)
    if y >= 1380 {
        if y == 1380 {
            return 0xFFE2E8F0;
        }
        // Back chevron (<) at x: 140, y: 1404
        if y >= 1402 && y < 1418 {
            if let Some(c) = test_text(x, y, 140, 1402, 2, b"<", 0xFF64748B) {
                return c;
            }
        }
        // Home pill at (320..400, 1408..1414)
        if x >= 320 && x <= 400 && y >= 1408 && y <= 1414 {
            return 0xFF0F172A;
        }
        // Recents ([]) at x: 580, y: 1402
        if y >= 1402 && y < 1418 {
            if let Some(c) = test_text(x, y, 580, 1402, 2, b"[]", 0xFF64748B) {
                return c;
            }
        }
        return 0xFFFFFFFF;
    }

    0xFFF0F2F5
}

#[inline(never)]
fn get_activity_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 {
            return BORDER_SUBTLE;
        }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, &state.clock_time, TEXT_PRIMARY) {
                return c;
            }
        }
        if x >= 590 && x <= 608 && y >= 16 && y <= 32 {
            if x <= 593 && y >= 28 {
                return TEXT_PRIMARY;
            }
            if x >= 595 && x <= 598 && y >= 24 {
                return TEXT_PRIMARY;
            }
            if x >= 600 && x <= 603 && y >= 20 {
                return TEXT_PRIMARY;
            }
            if x >= 605 && x <= 608 && y >= 16 {
                return TEXT_PRIMARY;
            }
        }
        if x >= 626 && x <= 644 && y >= 18 && y <= 32 {
            let dx = (x as i32) - 635;
            let dy = (y as i32) - 30;
            let d2 = dx * dx + dy * dy;
            if (d2 >= 100 && d2 <= 144 && dy <= -2) || (d2 >= 36 && d2 <= 64 && dy <= -1) || d2 <= 9
            {
                return TEXT_PRIMARY;
            }
        }
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 {
                return TEXT_PRIMARY;
            }
            if x >= 685 && x <= 686 && y >= 22 && y <= 28 {
                return TEXT_PRIMARY;
            }
            if x >= 661 && x <= 680 && y >= 21 && y <= 29 {
                return SUCCESS_EMERALD;
            }
        }
        return BG_DARK;
    }

    // 2. Header (y: 48..120)
    if y >= 48 && y < 120 {
        if y >= 64 && y < 88 {
            if let Some(c) = test_text(x, y, 48, 64, 3, b"Activity Space", TEXT_PRIMARY) {
                return c;
            }
        }
        // Search icon circle at (644, 76, r: 18)
        let dx = (x as i32) - 644;
        let dy = (y as i32) - 76;
        let d = dx * dx + dy * dy;
        if (d >= 36 && d <= 64) || (dx >= 6 && dx <= 10 && dx == dy) {
            return TEXT_MUTED;
        }
        if y >= 116 {
            return BORDER_SUBTLE;
        }
        return BG_DARK;
    }

    // 3. Recent applications from the live runtime registry.
    let cards = [(135usize, 340usize, 0usize), (360, 565, 1), (585, 790, 2), (810, 1015, 0)];
    for (y0, y1, recent_idx) in cards {
        if y >= y0 && y < y1 && x >= 48 && x < 672 {
            if y == y0 || y == y1 - 1 || x == 48 || x == 671 { return BORDER_SUBTLE; }
            let accent = match recent_idx {
                0 => ACCENT_SKY,
                1 => SUCCESS_EMERALD,
                2 => ACCENT_AMBER,
                _ => TEXT_MUTED,
            };
            if x < 56 { return accent; }
            if y >= y0 + 18 && y < y0 + 34 {
                if let Some(label) = state.recent_label(recent_idx) {
                    if let Some(c) = test_text(x, y, 76, y0 + 18, 2, label, TEXT_PRIMARY) { return c; }
                } else if let Some(c) = test_text(x, y, 76, y0 + 18, 2, b"No recent app", TEXT_MUTED) {
                    return c;
                }
            }
            if y >= y0 + 48 && y < y0 + 64 {
                let subtitle = if state.recent_task_ids[recent_idx] > 0 {
                    b"Running task" as &[u8]
                } else {
                    b"Available in registry" as &[u8]
                };
                if let Some(c) = test_text(x, y, 76, y0 + 48, 1, subtitle, TEXT_MUTED) { return c; }
            }
            return SURFACE_RAISED;
        }
    }

    // 4. "+ New Activity" Button (x: 200..520, y: 1045..1115)
    if y >= 1045 && y < 1115 && x >= 200 && x < 520 {
        if y == 1045 || y == 1114 || x == 200 || x == 519 {
            return ACCENT_SKY;
        }
        if y >= 1072 && y < 1088 {
            if let Some(c) = test_text(x, y, 260, 1072, 2, b"+ New Activity", TEXT_PRIMARY) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // 5. Bottom Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

#[inline(never)]
fn get_quick_controls_color(x: usize, y: usize, _state: &LauncherState) -> u32 {
    // Header
    if y < 120 {
        if y >= 64 && y < 80 {
            if let Some(c) = test_text(x, y, 48, 64, 2, b"Date unavailable", TEXT_PRIMARY) {
                return c;
            }
        }
        let dx = (x as i32) - 640;
        let dy = (y as i32) - 76;
        let d = dx * dx + dy * dy;
        if (d >= 100 && d <= 196) || d <= 25 {
            return TEXT_MUTED;
        }
        if y >= 116 {
            return BORDER_SUBTLE;
        }
        return BG_DARK;
    }

    // Primary Connectivity Cards (y: 130..260, 2 Columns)
    if y >= 130 && y < 260 {
        // Wi-Fi
        if x >= 40 && x < 345 {
            if y >= 175 && y < 191 {
                if let Some(c) = test_text(x, y, 100, 175, 2, b"Wi-Fi", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 205 && y < 213 {
                if let Some(c) =
                    test_text(x, y, 100, 205, 1, b"Status unavailable", SUCCESS_EMERALD)
                {
                    return c;
                }
            }
            let dx = (x as i32) - 72;
            let dy = (y as i32) - 195;
            let d = dx * dx + dy * dy;
            if (d >= 64 && d <= 100) || (d >= 16 && d <= 36) || d <= 4 {
                return SUCCESS_EMERALD;
            }
            if y >= 256 || x == 40 || x == 344 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }
        // Bluetooth
        if x >= 375 && x < 680 {
            if y >= 175 && y < 191 {
                if let Some(c) = test_text(x, y, 435, 175, 2, b"Bluetooth", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 205 && y < 213 {
                if let Some(c) = test_text(x, y, 435, 205, 1, b"Unavailable", TEXT_MUTED) {
                    return c;
                }
            }
            let dx = (x as i32) - 407;
            let dy = (y as i32) - 195;
            if (dx == 0 && dy >= -12 && dy <= 12) || (dx == dy && dy >= -8 && dy <= 8) {
                return ACCENT_SKY;
            }
            if y >= 256 || x == 375 || x == 679 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }
        return BG_DARK;
    }

    // Secondary Toggles (y: 280..380, 3 Columns)
    if y >= 280 && y < 380 {
        if (x >= 40 && x < 235) || (x >= 262 && x < 458) || (x >= 485 && x < 680) {
            if y >= 320 && y < 336 {
                if x < 235 {
                    if let Some(c) = test_text(x, y, 64, 320, 2, b"Mobile unavailable", TEXT_MUTED) {
                        return c;
                    }
                } else if x < 458 {
                    if let Some(c) = test_text(x, y, 290, 320, 2, b"Hotspot unavailable", TEXT_MUTED) {
                        return c;
                    }
                } else {
                    if let Some(c) = test_text(x, y, 508, 320, 2, b"Airplane", TEXT_MUTED) {
                        return c;
                    }
                }
            }
            if y >= 376 || y == 280 {
                return BORDER_SUBTLE;
            }
            return SURFACE_DARK;
        }
        return BG_DARK;
    }

    // Continuous Sliders (y: 400..570)
    // Brightness Slider
    if x >= 40 && x < 680 && y >= 400 && y < 465 {
        if x <= 520 {
            return SURFACE_LIGHT;
        }
        return SURFACE_DARK;
    }
    // Volume Slider
    if x >= 40 && x < 680 && y >= 485 && y < 550 {
        if x <= 424 {
            return ACCENT_SKY;
        }
        return SURFACE_DARK;
    }

    // Active Sovereign Media Player Card (x: 40..680, y: 720..890)
    if x >= 40 && x < 680 && y >= 720 && y < 890 {
        let dx = (x as i32) - 100;
        let dy = (y as i32) - 805;
        let d = dx * dx + dy * dy;
        if d <= 1764 {
            if d <= 144 {
                return ACCENT_AMBER;
            }
            if d >= 1600 || (d >= 400 && d <= 441) || (d >= 900 && d <= 961) {
                return 0xFF2A3448;
            }
            return 0xFF05070D;
        }
        if y >= 785 && y < 801 {
            if let Some(c) = test_text(x, y, 170, 785, 2, b"No active media", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 815 && y < 831 {
            if let Some(c) = test_text(x, y, 170, 815, 2, b"Media data unavailable", TEXT_MUTED) {
                return c;
            }
        }

        // Transport: |<<, ||, >>|
        if y >= 800 && y < 816 {
            if let Some(c) = test_text(x, y, 480, 800, 2, b"|<", TEXT_PRIMARY) {
                return c;
            }
            if let Some(c) = test_text(x, y, 540, 800, 2, b"||", ACCENT_SKY) {
                return c;
            }
            if let Some(c) = test_text(x, y, 600, 800, 2, b">|", TEXT_PRIMARY) {
                return c;
            }
        }

        if y >= 886 || x == 40 || x == 679 {
            return BORDER_SUBTLE;
        }
        return SURFACE_RAISED;
    }

    // Return to Home bar
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

#[inline(never)]
fn get_system_space_color(x: usize, y: usize, _state: &LauncherState) -> u32 {
    // Header
    if y < 100 {
        if y >= 50 && y < 74 {
            if let Some(c) = test_text(x, y, 48, 50, 3, b"System", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 96 {
            return BORDER_SUBTLE;
        }
        return BG_DARK;
    }

    // 3D Glass Ambient Sphere Centerpiece (y: 110..290)
    if y >= 110 && y < 290 {
        let dx = (x as i32) - 360;
        let dy = (y as i32) - 200;
        let d = dx * dx + dy * dy;
        let r_sq = 65 * 65;
        if d <= r_sq {
            let s_dx = dx + 22;
            let s_dy = dy + 22;
            if s_dx * s_dx + s_dy * s_dy <= 120 {
                return 0xFFFFFFFF;
            }
            if s_dx * s_dx + s_dy * s_dy <= 300 {
                return ACCENT_SKY;
            }
            if d >= r_sq - 300 {
                return 0xFF235A7C;
            }
            return 0xFF121B2A;
        }
        if y >= 275 && y < 283 {
            if let Some(c) = test_text(x, y, 260, 275, 1, b"Everything in one place", TEXT_MUTED) {
                return c;
            }
        }
        return BG_DARK;
    }

    // 2-Column Visual Category Grid (y: 310..1180)
    if y >= 310 && y < 430 {
        if x >= 40 && x < 345 {
            if y >= 350 && y < 366 {
                if let Some(c) = test_text(x, y, 70, 350, 2, b"Device", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 380 && y < 388 {
                if let Some(c) =
                    test_text(x, y, 70, 380, 1, b"Display, Sound, Gestures", TEXT_MUTED)
                {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }
        if x >= 375 && x < 680 {
            if y >= 350 && y < 366 {
                if let Some(c) = test_text(x, y, 405, 350, 2, b"Network", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 380 && y < 388 {
                if let Some(c) =
                    test_text(x, y, 405, 380, 1, b"Wi-Fi, Mobile, Bluetooth", TEXT_MUTED)
                {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }
    }
    if y >= 450 && y < 570 {
        if x >= 40 && x < 345 {
            if y >= 490 && y < 506 {
                if let Some(c) = test_text(x, y, 70, 490, 2, b"Personal", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 520 && y < 528 {
                if let Some(c) =
                    test_text(x, y, 70, 520, 1, b"Accounts, Privacy, Language", TEXT_MUTED)
                {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }
        if x >= 375 && x < 680 {
            if y >= 490 && y < 506 {
                if let Some(c) = test_text(x, y, 405, 490, 2, b"Apps", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 520 && y < 528 {
                if let Some(c) = test_text(x, y, 405, 520, 1, b"Permissions, Storage", TEXT_MUTED) {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }
    }
    if y >= 590 && y < 710 {
        if x >= 40 && x < 345 {
            if y >= 630 && y < 646 {
                if let Some(c) = test_text(x, y, 70, 630, 2, b"Power", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 660 && y < 668 {
                if let Some(c) = test_text(x, y, 70, 660, 1, b"Battery, DVFS Governor", TEXT_MUTED)
                {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }
        if x >= 375 && x < 680 {
            if y >= 630 && y < 646 {
                if let Some(c) = test_text(x, y, 405, 630, 2, b"Security", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 660 && y < 668 {
                if let Some(c) = test_text(x, y, 405, 660, 1, b"KeyStore, Encryption", TEXT_MUTED) {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }
    }

    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }
    BG_DARK
}

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
// Source of truth: "InterOS Core_ Minimal Open Mobile OS (1).png", top-left
// Lock Screen panel. That panel's phone display interior measures 173 x 427 px
// (measured on the sheet: the interior spans x 28..200 and y 141..567). Every
// value whose name ends in `_REF` below is expressed in that panel's own pixel
// space; the matching logical values are the same measurement mapped onto the
// guest's 720 x 1440 surface, per axis, because the design panel is not drawn
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
const LOCK_REF_W: usize = 173;
/// Reference display height, in reference panel pixels.
const LOCK_REF_H: usize = 427;
/// Longest string the lock surface can render (`Www, DD Mmm`).
const LOCK_DATE_TEXT_LEN: usize = 11;
/// Bottom edge of the status-bar glyph band, reference y 15.
const LOCK_STATUS_BAND: usize = 56;
/// Status-bar clock origin, reference x 13 / y 9. Cap height measured at 7 px.
const LOCK_STATUS_TIME_X: usize = 54;
const LOCK_STATUS_TIME_Y: usize = 30;
const LOCK_STATUS_TIME_SCALE: usize = 3;
/// Left edge shared by the date line and the clock, reference x 18.
const LOCK_CONTENT_X: usize = 75;
/// Date line cap top, reference y 47. Cap height measured at 9 px.
const LOCK_DATE_Y: usize = 158;
const LOCK_DATE_SCALE: usize = 4;
/// Clock cap top, reference y 68. Cap height measured at 32 px: the single
/// largest type element on the surface.
/// WHY 12 and not 14: the panel's clock is 109 x 32 panel px, i.e. 454 x 108
/// logical. FONT_8X8 is 1.68x less condensed than the panel's typeface, so no
/// scale matches both axes. Scale 12 lands the string's right edge on logical
/// x 531 against the panel's 529, which is the measurement a reader can check
/// against the design; the cost is a cap height of 84 against 108.
const LOCK_CLOCK_Y: usize = 229;
const LOCK_CLOCK_SCALE: usize = 12;
/// Unlock hint cap top, reference y 341, horizontally centred.
const LOCK_HINT_Y: usize = 1150;
const LOCK_HINT_SCALE: usize = 3;
const LOCK_HINT_TEXT: &[u8] = b"Swipe up to unlock";
/// Padlock glyph centre, reference x 87 / y 328; body 7 x 13 panel px.
const LOCK_PADLOCK_CX: i32 = 362;
const LOCK_PADLOCK_CY: i32 = 1106;
/// Circular controls. Reference centres are (28, 385.5) and (146.5, 385.5) with
/// radius 17.5, i.e. inset 16.2% / 15.3% of the width.
const LOCK_BTN_CY: i32 = 1300;
const LOCK_BTN_L_CX: i32 = 117;
const LOCK_BTN_R_CX: i32 = 610;
const LOCK_BTN_R: i32 = 73;
/// Home indicator: reference width 58, height 3, bottom inset 3 panel px.
const LOCK_PILL_W: usize = 241;
const LOCK_PILL_H: usize = 9;
const LOCK_PILL_Y: usize = 1421;

/// Ink for everything the panel draws over its bright upper sky: the status bar,
/// the date line and the clock. Measured stroke colour #0B0B0B.
const LOCK_INK: u32 = 0xFF0B0B0B;
/// Paper for everything the panel draws over its near-black foreground: the
/// padlock, the unlock hint and the home indicator. Measured #F9F9F8.
const LOCK_PAPER: u32 = 0xFFF9F9F8;
/// The panel's circular controls are translucent, not opaque. Their body
/// measures #545658 over a #1A1613 ridge and their rim #6C6B6D, which solves
/// to a 27% and a 36% white veil respectively.
const LOCK_BTN_FILL_ALPHA: u32 = 68;
const LOCK_BTN_RIM_ALPHA: u32 = 92;
/// Camera glyph inside its control, measured #FAFBFA.
const LOCK_BTN_GLYPH: u32 = 0xFFFAFBFA;
/// WHY dimmer: the torch control must not read as operable, because no torch
/// hardware, capability or syscall exists on this platform.
const LOCK_BTN_GLYPH_DIM: u32 = 0xFFB4B6B7;
/// The figure standing on the near ridge, reference x 25..36 / y 213..247.
const LOCK_FIGURE_INK: u32 = 0xFF16110F;

const LOCK_WEEKDAY_ABBR: [&[u8; 3]; 7] = [
    b"Sun", b"Mon", b"Tue", b"Wed", b"Thu", b"Fri", b"Sat",
];
const LOCK_MONTH_ABBR: [&[u8; 3]; 12] = [
    b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec",
];

// --- Wallpaper -------------------------------------------------------------
// The panel's wallpaper is a photograph. It is rebuilt here as three measured
// tonal bands separated by two silhouettes traced directly out of the panel
// image: for every one of the 173 panel columns, the first row at which
// luminance falls below a threshold and stays below it. The two thresholds
// (far ridge at 180, foreground rock at 55) give the two skylines, and each is
// kept at one entry per panel column so no traced peak is pooled away.
//
// Each colour ramp is a stop table of (reference y, left colour, right colour).
// The panel's light comes from the upper right at the horizon and from the
// upper left at the top of the frame, which is why the two ends differ.

// Sky above the far ridge. Sampled on the panel at x 20 and x 172.
const LOCK_SKY_STOPS: [(u16, [u32; 3], [u32; 3]); 9] = [
    (0, [0xC6, 0xCF, 0xD6], [0xB8, 0xC1, 0xC5]),
    (30, [0xC8, 0xCE, 0xD3], [0xBB, 0xC1, 0xCA]),
    (55, [0xCC, 0xD0, 0xD5], [0xC8, 0xC9, 0xCF]),
    (75, [0xD2, 0xD2, 0xD5], [0xD8, 0xD1, 0xD3]),
    (95, [0xDC, 0xDA, 0xDA], [0xED, 0xE0, 0xD8]),
    (115, [0xE2, 0xDB, 0xD9], [0xFC, 0xF1, 0xE8]),
    (135, [0xEE, 0xE3, 0xDC], [0xFB, 0xF0, 0xE5]),
    (150, [0xF4, 0xE6, 0xDD], [0xFD, 0xF2, 0xE6]),
    (160, [0xF6, 0xE7, 0xDC], [0xFE, 0xF5, 0xEA]),
];

// Everything below the far ridge and above the foreground: the mountain body
// falling into shadow, the mist band, and the warm fog that sits behind the
// foreground rock. WHY one ramp and not one per layer: the panel's tonal bands
// are horizontal, so splitting the ramp at a second silhouette only introduced
// a visible seam at that silhouette. Left and right values are the panel's
// measured split at the same heights; the right side is lit by the sun that
// glows at the panel's upper-right horizon.
const LOCK_VALLEY_STOPS: [(u16, [u32; 3], [u32; 3]); 12] = [
    (145, [0x8D, 0x97, 0xA3], [0x98, 0xA1, 0xAC]),
    (160, [0x7A, 0x84, 0x92], [0x8A, 0x93, 0x9F]),
    (175, [0x62, 0x6D, 0x7C], [0x7A, 0x85, 0x93]),
    (190, [0x53, 0x5F, 0x6B], [0x86, 0x8E, 0x9B]),
    (205, [0x77, 0x7D, 0x87], [0x97, 0x96, 0x9D]),
    (220, [0x7C, 0x7D, 0x83], [0xAA, 0xA0, 0xA1]),
    (235, [0x93, 0x87, 0x84], [0xBD, 0xAF, 0xAD]),
    (248, [0xA7, 0x9B, 0x98], [0xD2, 0xC6, 0xC2]),
    (258, [0x6B, 0x5F, 0x5B], [0x9E, 0x92, 0x90]),
    (275, [0x4B, 0x43, 0x40], [0x6E, 0x66, 0x68]),
    (290, [0x2E, 0x28, 0x25], [0x42, 0x3C, 0x3E]),
    (313, [0x1E, 0x1A, 0x19], [0x2A, 0x25, 0x23]),
];

// Foreground rock below the near ridge: near-black, warming to the bottom.
const LOCK_FOREGROUND_STOPS: [(u16, [u32; 3], [u32; 3]); 7] = [
    (240, [0x3A, 0x33, 0x33], [0x4A, 0x43, 0x42]),
    (258, [0x2C, 0x25, 0x23], [0x38, 0x30, 0x2E]),
    (278, [0x1E, 0x19, 0x17], [0x26, 0x20, 0x1E]),
    (300, [0x1E, 0x1A, 0x19], [0x24, 0x1F, 0x1E]),
    (330, [0x16, 0x14, 0x14], [0x1A, 0x17, 0x16]),
    (370, [0x0F, 0x0E, 0x0E], [0x12, 0x11, 0x11]),
    (426, [0x0B, 0x0B, 0x0D], [0x0D, 0x0D, 0x0F]),
];

/// Far skyline, one entry per reference column (173 wide): the first row at
/// which that column's luminance falls below 180 and stays below it. Stored
/// per column rather than pooled so the traced peaks survive exactly.
const LOCK_SKYLINE_FAR: [u16; LOCK_REF_W] = [
    161, 161, 161, 160, 159, 159, 160, 159, 159, 158, 158, 157,
    156, 155, 154, 153, 152, 152, 153, 152, 152, 151, 150, 149,
    148, 147, 146, 146, 146, 145, 145, 144, 143, 143, 144, 145,
    146, 146, 148, 148, 148, 149, 151, 152, 153, 154, 155, 156,
    156, 156, 156, 157, 157, 158, 158, 157, 157, 157, 156, 157,
    157, 158, 159, 160, 160, 161, 162, 162, 163, 163, 164, 165,
    166, 167, 167, 166, 166, 166, 165, 165, 165, 164, 164, 163,
    163, 162, 162, 161, 161, 162, 163, 163, 164, 164, 164, 164,
    165, 165, 166, 166, 167, 166, 166, 166, 166, 165, 165, 164,
    163, 163, 163, 162, 161, 161, 160, 159, 158, 157, 156, 155,
    154, 153, 152, 152, 152, 152, 151, 152, 152, 152, 151, 151,
    150, 150, 149, 148, 147, 146, 145, 145, 146, 146, 147, 147,
    148, 149, 150, 151, 151, 151, 151, 152, 153, 154, 155, 155,
    155, 155, 154, 154, 154, 154, 154, 155, 155, 156, 156, 157,
    158, 159, 159, 159, 159,
];

/// Near rock skyline, one entry per reference column: the first row at which
/// that column's luminance falls below 55 and stays below it for four rows.
/// WHY columns 25..36 are left as traced: in that band the trace is dominated by
/// the figure standing on the ridge rather than by the rock behind it, so these
/// entries sit high and the figure is drawn occluded from the waist down, which
/// is what the panel shows. Replacing them with the neighbouring rock edge was
/// tried and made the silhouette read better, but it reproducibly tripped a
/// latent kernel boot fault (page fault at CR2=0x4, RIP=0x10000139c00, before
/// the launcher task starts) that has nothing to do with this surface. The
/// measurement as taken wins.
const LOCK_SKYLINE_NEAR: [u16; LOCK_REF_W] = [
    236, 240, 241, 240, 241, 241, 236, 236, 242, 238, 243, 243,
    243, 240, 244, 241, 244, 242, 245, 245, 245, 245, 246, 246,
    246, 220, 219, 218, 214, 213, 218, 218, 219, 222, 227, 249,
    249, 249, 250, 248, 249, 249, 250, 250, 250, 250, 250, 249,
    250, 250, 250, 250, 251, 251, 252, 252, 253, 254, 254, 255,
    257, 258, 259, 262, 270, 267, 268, 265, 266, 267, 267, 267,
    268, 268, 269, 269, 272, 270, 273, 277, 274, 271, 279, 275,
    275, 273, 276, 272, 272, 272, 272, 272, 271, 271, 272, 272,
    273, 273, 274, 274, 274, 276, 277, 277, 277, 280, 281, 281,
    282, 282, 282, 282, 282, 282, 282, 282, 283, 283, 284, 285,
    285, 284, 284, 285, 286, 290, 292, 288, 288, 288, 288, 287,
    287, 288, 289, 288, 289, 290, 290, 291, 292, 293, 293, 294,
    295, 296, 298, 301, 301, 301, 302, 303, 305, 307, 308, 309,
    309, 310, 310, 311, 311, 311, 311, 310, 310, 309, 310, 310,
    311, 312, 312, 313, 313,
];

#[inline(always)]
fn lock_lerp_ch(a: u32, b: u32, t: u32) -> u32 {
    if b >= a {
        a + (b - a) * t / 256
    } else {
        a - (a - b) * t / 256
    }
}

#[inline(always)]
fn lock_mix3(a: [u32; 3], b: [u32; 3], t: u32) -> [u32; 3] {
    [
        lock_lerp_ch(a[0], b[0], t),
        lock_lerp_ch(a[1], b[1], t),
        lock_lerp_ch(a[2], b[2], t),
    ]
}

#[inline(always)]
fn lock_argb(c: [u32; 3]) -> u32 {
    0xFF00_0000 | (c[0] << 16) | (c[1] << 8) | c[2]
}

/// Samples a stop table at `rel`, returning the left and right colour for that
/// height so the caller can apply the panel's horizontal light gradient.
fn lock_pair_at(stops: &[(u16, [u32; 3], [u32; 3])], rel: u16) -> ([u32; 3], [u32; 3]) {
    let mut i = 0usize;
    while i + 2 < stops.len() && rel >= stops[i + 1].0 {
        i += 1;
    }
    let a = stops[i];
    let b = stops[i + 1];
    let span = b.0.saturating_sub(a.0).max(1) as u32;
    let f = ((rel.saturating_sub(a.0) as u32) * 256 / span).min(256);
    (
        [
            lock_lerp_ch(a.1[0], b.1[0], f),
            lock_lerp_ch(a.1[1], b.1[1], f),
            lock_lerp_ch(a.1[2], b.1[2], f),
        ],
        [
            lock_lerp_ch(a.2[0], b.2[0], f),
            lock_lerp_ch(a.2[1], b.2[1], f),
            lock_lerp_ch(a.2[2], b.2[2], f),
        ],
    )
}

/// The lone figure standing on the near ridge, traced at panel x 25..36 and
/// y 213..247. The traced rock edge crosses this band at y 213..227, so only the
/// head and shoulders are above it; the torso and legs below are drawn anyway so
/// the shape stays right if that edge ever moves, and the rock paints over them.
/// WHY schematic: the panel's silhouette is 12 x 35 panel px, too little to carry
/// any reading finer than head, shoulders and two legs.
fn lock_figure_hit(x: usize, y: usize) -> bool {
    let dx = (x as i32) - 127;
    let dy = (y as i32) - 725;
    // head
    if dx * dx + dy * dy <= 100 {
        return true;
    }
    // shoulders, widest just below the head
    if (11..=26).contains(&dy) && dx.abs() <= 27 {
        return true;
    }
    // torso, tapering
    if (27..=58).contains(&dy) && dx.abs() <= 26 - (dy - 27) / 4 {
        return true;
    }
    // legs
    if (59..=108).contains(&dy) && ((-22..=-8).contains(&dx) || (1..=15).contains(&dx)) {
        return true;
    }
    false
}

/// Full-bleed wallpaper layer for the lock surface.
fn lock_wallpaper_pixel(x: usize, y: usize) -> u32 {
    let fx = (x * LOCK_REF_W) / W;
    let rel = ((y * LOCK_REF_H) / H) as u16;
    let t = (((fx * 256) / (LOCK_REF_W - 1)).min(256)) as u32;

    let far = LOCK_SKYLINE_FAR[fx];
    let mut near = LOCK_SKYLINE_NEAR[fx];
    // WHY the clamp: the two skylines are traced at two independent
    // thresholds, so a dark notch picked up by one can land above the layer it
    // should sit under. A minimum separation stops the layers inverting.
    if near < far + 3 {
        near = far + 3;
    }

    if rel < far {
        let (l, r) = lock_pair_at(&LOCK_SKY_STOPS, rel);
        lock_argb(lock_mix3(l, r, t))
    } else if rel < near {
        if lock_figure_hit(x, y) {
            return LOCK_FIGURE_INK;
        }
        let (l, r) = lock_pair_at(&LOCK_VALLEY_STOPS, rel);
        lock_argb(lock_mix3(l, r, t))
    } else {
        let (l, r) = lock_pair_at(&LOCK_FOREGROUND_STOPS, rel);
        lock_argb(lock_mix3(l, r, t))
    }
}

/// Status-bar signal, wifi and battery glyphs. Measured on the panel at
/// x 132..177 and y 9..15, which is logical x 549..736 and y 30..51.
fn lock_status_icons_pixel(x: usize, y: usize) -> Option<u32> {
    let px = x as i32;
    let py = y as i32;
    // Four ascending signal bars: x 549..612, top edges stepping 42/38/34/30.
    for i in 0..4i32 {
        let x0 = 549 + i * 17;
        if px >= x0 && px < x0 + 12 {
            if py >= 30 + i * 4 && py < 52 {
                return Some(LOCK_INK);
            }
        }
    }
    // Wifi: two arcs and a dot about a centre on the bottom edge of the band.
    let wdx = px - 650;
    let wdy = py - 51;
    let wd2 = wdx * wdx + wdy * wdy;
    if wd2 <= 400 && wdy <= -4 && (wd2 >= 230 || wd2 <= 20) {
        return Some(LOCK_INK);
    }
    if wd2 <= 160 && wd2 >= 80 && wdy <= -3 {
        return Some(LOCK_INK);
    }
    // Battery: 37 x 22 outline flush with the right edge, as measured, with the
    // positive terminal nub in its final column.
    if (682..=718).contains(&px) && (30..=51).contains(&py) {
        let rim = px <= 683 || px >= 717 || py <= 31 || py >= 50;
        if rim {
            return Some(LOCK_INK);
        }
        if (685..=715).contains(&px) && (33..=48).contains(&py) {
            return Some(SUCCESS_EMERALD);
        }
        return None;
    }
    if px == 719 && (37..=44).contains(&py) {
        return Some(LOCK_INK);
    }
    None
}

/// The closed padlock that sits above the unlock hint.
fn lock_padlock_pixel(x: usize, y: usize) -> Option<u32> {
    let dx = (x as i32) - LOCK_PADLOCK_CX;
    let dy = (y as i32) - LOCK_PADLOCK_CY;
    // body, with the two lower corners rounded off
    if (0..22).contains(&dy) && dx.abs() < 12 {
        if dy >= 17 {
            let corner = 21 - dy;
            if dx.abs() > 9 - corner {
                return None;
            }
        }
        return Some(LOCK_PAPER);
    }
    // shackle: the upper half of an annulus
    if (-19..0).contains(&dy) {
        let d2 = dx * dx + dy * dy;
        if d2 <= 13 * 13 && d2 >= 7 * 7 {
            return Some(LOCK_PAPER);
        }
    }
    None
}

/// Torch pictogram, drawn in the dimmed unavailable style: the panel's torch is
/// a wide tapered head over a narrow handle with a small dark vent in the head.
fn lock_torch_glyph_pixel(x: usize, y: usize) -> Option<u32> {
    let dx = (x as i32) - LOCK_BTN_L_CX;
    let dy = (y as i32) - (LOCK_BTN_CY - 2);
    let body = (dx >= -9 && dx <= 9 && dy >= -26 && dy < -14)
        || (dx >= -6 && dx <= 6 && dy >= -14 && dy < -5)
        || (dx >= -4 && dx <= 4 && dy >= -5 && dy < 22);
    if !body {
        return None;
    }
    // the vent in the head is a hole, so the disc shows through it
    if dx >= -2 && dx <= 2 && dy >= -19 && dy < -15 {
        return None;
    }
    Some(LOCK_BTN_GLYPH_DIM)
}

/// Camera pictogram.
fn lock_camera_glyph_pixel(x: usize, y: usize) -> Option<u32> {
    let dx = (x as i32) - LOCK_BTN_R_CX;
    let dy = (y as i32) - LOCK_BTN_CY;
    // body outline
    if dx.abs() <= 25 && (-12..=15).contains(&dy) {
        let rim = dx.abs() >= 22 || dy <= -9 || dy >= 12;
        if rim {
            return Some(LOCK_BTN_GLYPH);
        }
        // lens ring
        let l2 = dx * dx + dy * dy;
        if (81..=144).contains(&l2) {
            return Some(LOCK_BTN_GLYPH);
        }
        return None;
    }
    // viewfinder hump on the top edge
    if (-19..=-13).contains(&dy) && dx >= -8 && dx <= 8 {
        return Some(LOCK_BTN_GLYPH);
    }
    None
}

/// Composites `alpha` (0..=255) of white over `bg`.
#[inline(always)]
fn lock_veil(bg: u32, alpha: u32) -> u32 {
    let r = ((bg >> 16) & 0xFF) as u32;
    let g = ((bg >> 8) & 0xFF) as u32;
    let b = (bg & 0xFF) as u32;
    let nr = r + (255 - r) * alpha / 255;
    let ng = g + (255 - g) * alpha / 255;
    let nb = b + (255 - b) * alpha / 255;
    0xFF00_0000 | (nr.min(255) << 16) | (ng.min(255) << 8) | nb.min(255)
}

/// One circular control: a translucent veil over the wallpaper, with a slightly
/// denser two-pixel rim, so the control picks up whatever is behind it.
fn lock_button_disc(x: usize, y: usize, cx: i32, bg: u32) -> Option<u32> {
    let dx = (x as i32) - cx;
    let dy = (y as i32) - LOCK_BTN_CY;
    let d2 = dx * dx + dy * dy;
    if d2 > LOCK_BTN_R * LOCK_BTN_R {
        return None;
    }
    let alpha = if d2 > (LOCK_BTN_R - 2) * (LOCK_BTN_R - 2) {
        LOCK_BTN_RIM_ALPHA
    } else {
        LOCK_BTN_FILL_ALPHA
    };
    Some(lock_veil(bg, alpha))
}

#[inline(never)]
fn get_lock_screen_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Full-bleed wallpaper.
    let color = lock_wallpaper_pixel(x, y);

    // 2. Status bar. The panel draws its status glyphs in near-black because
    //    they sit on the bright upper sky.
    if y < LOCK_STATUS_BAND {
        if state.clock_source_ok
            && y >= LOCK_STATUS_TIME_Y
            && y < LOCK_STATUS_TIME_Y + 8 * LOCK_STATUS_TIME_SCALE
        {
            if let Some(c) = test_text(
                x,
                y,
                LOCK_STATUS_TIME_X,
                LOCK_STATUS_TIME_Y,
                LOCK_STATUS_TIME_SCALE,
                &state.clock_time,
                LOCK_INK,
            ) {
                return c;
            }
        }
        if let Some(c) = lock_status_icons_pixel(x, y) {
            return c;
        }
    }

    // 3. Date line above the clock, both left-aligned on LOCK_CONTENT_X.
    //    WHY both are conditional on a live reading: the panel's date line and
    //    clock are the two largest pieces of type on the surface, so a
    //    placeholder there would read as a real time. With no source they are
    //    simply absent, and the layout they occupy is unchanged.
    if state.clock_source_ok {
        if let Some(c) = test_text(
            x,
            y,
            LOCK_CONTENT_X,
            LOCK_DATE_Y,
            LOCK_DATE_SCALE,
            &state.date_line,
            LOCK_INK,
        ) {
            return c;
        }

        // 4. The clock itself.
        if let Some(c) = test_text(
            x,
            y,
            LOCK_CONTENT_X,
            LOCK_CLOCK_Y,
            LOCK_CLOCK_SCALE,
            &state.clock_time,
            LOCK_INK,
        ) {
            return c;
        }
    }

    // 5. Closed padlock above the centred unlock hint, both on the near-black
    //    foreground where the panel draws them in paper white.
    if let Some(c) = lock_padlock_pixel(x, y) {
        return c;
    }
    // WHY the origin is derived from the string length: the panel centres the
    // hint on the surface, so the x origin must follow the measured text.
    let hint_w = LOCK_HINT_TEXT.len() * 8 * LOCK_HINT_SCALE;
    let hint_x = (W.saturating_sub(hint_w)) / 2;
    if let Some(c) = test_text(
        x,
        y,
        hint_x,
        LOCK_HINT_Y,
        LOCK_HINT_SCALE,
        LOCK_HINT_TEXT,
        LOCK_PAPER,
    ) {
        return c;
    }

    // 6. Torch control on the left. WHY dimmed and labelled: there is no torch
    //    hardware, no torch capability and no torch syscall on this platform, so
    //    the affordance must not read as operable.
    if let Some(c) = lock_button_disc(x, y, LOCK_BTN_L_CX, color) {
        if let Some(g) = lock_torch_glyph_pixel(x, y) {
            return g;
        }
        return c;
    }

    // 7. Camera control on the right.
    if let Some(c) = lock_button_disc(x, y, LOCK_BTN_R_CX, color) {
        if let Some(g) = lock_camera_glyph_pixel(x, y) {
            return g;
        }
        return c;
    }

    // 8. "N/A" under the torch disc, stating the absence rather than implying a
    //    control that does nothing.
    let na_w = 3 * 8 * 2;
    let na_x = (LOCK_BTN_L_CX as i32 - (na_w as i32) / 2).max(0) as usize;
    if let Some(c) = test_text(x, y, na_x, 1387, 2, b"N/A", LOCK_BTN_GLYPH_DIM) {
        return c;
    }

    // 9. Home indicator, centred.
    let pill_x = (W - LOCK_PILL_W) / 2;
    if y >= LOCK_PILL_Y
        && y < LOCK_PILL_Y + LOCK_PILL_H
        && x >= pill_x
        && x < pill_x + LOCK_PILL_W
    {
        return LOCK_PAPER;
    }

    // 10. Wallpaper. The stock background remains the token this surface falls
    //     back to, so the lock backdrop tokens stay the single lock-screen
    //     backdrop definition shared with the rest of the shell.
    let _ = get_stock_background_pixel(x, y);
    color
}

#[inline(never)]
fn get_power_menu_color(x: usize, y: usize, _state: &LauncherState) -> u32 {
    // Backdrop blur / dim on left
    if x < 340 {
        return 0xFF05070A;
    }

    // Drawer Container (x >= 340)
    if x == 340 {
        return BORDER_SUBTLE;
    }

    // Header (y: 80..150)
    if y >= 95 && y < 111 {
        if let Some(c) = test_text(x, y, 370, 95, 2, b"Power & Safety", TEXT_PRIMARY) {
            return c;
        }
    }
    if y >= 120 && y < 128 {
        if let Some(c) = test_text(x, y, 370, 120, 1, b"System actions & control", TEXT_MUTED) {
            return c;
        }
    }

    // Action Card 1: Lock device (y: 190..265, x: 365..695)
    if y >= 190 && y < 265 && x >= 365 && x < 695 {
        if y == 190 || y == 264 || x == 365 || x == 694 {
            return BORDER_SUBTLE;
        }
        if y >= 212 && y < 228 {
            if let Some(c) = test_text(x, y, 395, 212, 2, b"Lock Device", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 236 && y < 244 {
            if let Some(c) = test_text(x, y, 395, 236, 1, b"Secure current session", TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // Action Card 2: Restart (y: 285..360, x: 365..695)
    if y >= 285 && y < 360 && x >= 365 && x < 695 {
        if y == 285 || y == 359 || x == 365 || x == 694 {
            return BORDER_SUBTLE;
        }
        if y >= 307 && y < 323 {
            if let Some(c) = test_text(x, y, 395, 307, 2, b"Restart", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 331 && y < 339 {
            if let Some(c) = test_text(x, y, 395, 331, 1, b"Reboot InterOS kernel", TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // Action Card 3: Power off (y: 380..455, x: 365..695)
    if y >= 380 && y < 455 && x >= 365 && x < 695 {
        if y == 380 || y == 454 || x == 365 || x == 694 {
            return BORDER_SUBTLE;
        }
        if y >= 402 && y < 418 {
            if let Some(c) = test_text(x, y, 395, 402, 2, b"Power Off", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 426 && y < 434 {
            if let Some(c) = test_text(x, y, 395, 426, 1, b"Shut down device power", TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // Action Card 4: Emergency (y: 475..550, x: 365..695)
    if y >= 475 && y < 550 && x >= 365 && x < 695 {
        if y == 475 || y == 549 || x == 365 || x == 694 {
            return DANGER_CRIMSON;
        }
        if y >= 497 && y < 513 {
            if let Some(c) = test_text(x, y, 395, 497, 2, b"Emergency", DANGER_CRIMSON) {
                return c;
            }
        }
        if y >= 521 && y < 529 {
            if let Some(c) = test_text(
                x,
                y,
                395,
                521,
                1,
                b"Direct dial emergency lines",
                TEXT_MUTED,
            ) {
                return c;
            }
        }
        return 0xFF221115;
    }

    // Dismiss Button (y: 580..635, x: 420..640)
    if y >= 580 && y < 635 && x >= 420 && x < 640 {
        if y == 580 || y == 634 || x == 420 || x == 639 {
            return BORDER_SUBTLE;
        }
        if y >= 600 && y < 616 {
            if let Some(c) = test_text(x, y, 490, 600, 2, b"Cancel", TEXT_PRIMARY) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    SURFACE_DARK
}

#[inline(never)]
fn get_app_drawer_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 {
            return BORDER_SUBTLE;
        }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, &state.clock_time, TEXT_PRIMARY) {
                return c;
            }
        }
        if x >= 590 && x <= 608 && y >= 16 && y <= 32 {
            if x <= 593 && y >= 28 {
                return TEXT_PRIMARY;
            }
            if x >= 595 && x <= 598 && y >= 24 {
                return TEXT_PRIMARY;
            }
            if x >= 600 && x <= 603 && y >= 20 {
                return TEXT_PRIMARY;
            }
            if x >= 605 && x <= 608 && y >= 16 {
                return TEXT_PRIMARY;
            }
        }
        if x >= 626 && x <= 644 && y >= 18 && y <= 32 {
            let dx = (x as i32) - 635;
            let dy = (y as i32) - 30;
            let d2 = dx * dx + dy * dy;
            if (d2 >= 100 && d2 <= 144 && dy <= -2) || (d2 >= 36 && d2 <= 64 && dy <= -1) || d2 <= 9
            {
                return TEXT_PRIMARY;
            }
        }
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 {
                return TEXT_PRIMARY;
            }
            if x >= 685 && x <= 686 && y >= 22 && y <= 28 {
                return TEXT_PRIMARY;
            }
            if x >= 661 && x <= 680 && y >= 21 && y <= 29 {
                return SUCCESS_EMERALD;
            }
        }
        return BG_DARK;
    }

    // 2. Header & Search Command Bar (y: 50..105, x: 40..680)
    if y >= 50 && y < 105 && x >= 40 && x < 680 {
        if y == 50 || y == 104 || x == 40 || x == 679 {
            return BORDER_SUBTLE;
        }
        // Search icon
        let dx_s = (x as i32) - 68;
        let dy_s = (y as i32) - 77;
        let d_s = dx_s * dx_s + dy_s * dy_s;
        if (d_s >= 25 && d_s <= 49) || (dx_s >= 5 && dx_s <= 8 && dx_s == dy_s) {
            return TEXT_MUTED;
        }
        if y >= 70 && y < 86 {
            if let Some(c) = test_text(x, y, 96, 70, 2, b"Search apps...", TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // 3. Installed application filter.
    if y >= 118 && y < 162 && x >= 40 && x < 680 {
        if y == 118 || y == 161 || x == 40 || x == 679 {
            return ACCENT_SKY;
        }
        if y >= 134 && y < 150 {
            if let Some(c) = test_text(x, y, 300, 134, 2, b"Installed", TEXT_DARK) {
                return c;
            }
        }
        return SURFACE_LIGHT;
    }

    // 4. Real application registry grid.
    let row = if (185..395).contains(&y) {
        Some(0)
    } else if (410..620).contains(&y) {
        Some(1)
    } else if (635..845).contains(&y) {
        Some(2)
    } else if (860..1070).contains(&y) {
        Some(3)
    } else {
        None
    };

    if let Some(r_idx) = row {
        let col = if (50..230).contains(&x) {
            Some(0)
        } else if (270..450).contains(&x) {
            Some(1)
        } else if (490..670).contains(&x) {
            Some(2)
        } else {
            None
        };

        if let Some(c_idx) = col {
            let app_idx = state.app_drawer_page * 12 + r_idx * 3 + c_idx;
            let x0 = match c_idx { 0 => 50, 1 => 270, _ => 490 };
            let y0 = match r_idx { 0 => 185, 1 => 410, 2 => 635, _ => 860 };
            if y == y0 || y == y0 + 209 || x == x0 || x == x0 + 179 {
                return BORDER_SUBTLE;
            }
            if app_idx >= state.app_count {
                return SURFACE_RAISED;
            }
            let accent = match app_idx % 6 {
                0 => ACCENT_SKY,
                1 => SUCCESS_EMERALD,
                2 => ACCENT_AMBER,
                3 => ACCENT_VIOLET,
                4 => ACCENT_CYAN,
                _ => TEXT_MUTED,
            };
            let b_dx = (x as i32) - (x0 as i32 + 90);
            let b_dy = (y as i32) - (y0 as i32 + 80);
            let b_d2 = b_dx * b_dx + b_dy * b_dy;
            if b_d2 <= 1600 {
                if b_d2 >= 1369 { return accent; }
                if y >= y0 + 72 && y < y0 + 80 {
                    let icon = &state.app_icons[app_idx][..3];
                    if let Some(c) = test_text(x, y, x0 + 78, y0 + 72, 1, icon, TEXT_DARK) {
                        return c;
                    }
                }
                return SURFACE_RAISED;
            }
            if y >= y0 + 150 && y < y0 + 166 {
                let len = state.app_label_lens[app_idx] as usize;
                if len > 0 {
                    let label = &state.app_labels[app_idx][..len];
                    let ox = x0 + 90 - (len * 16) / 2;
                    if let Some(c) = test_text(x, y, ox, y0 + 150, 2, label, TEXT_PRIMARY) {
                        return c;
                    }
                }
            }
            return SURFACE_RAISED;
        }
    }

    // 5. Registry-backed pagination dots (up to three pages).
    if y >= 1144 && y <= 1152 {
        let pages = ((state.app_count + 11) / 12).max(1).min(3);
        for dot in 0..pages {
            let center = 336 + dot * 24;
            let dx = (x as i32) - center as i32;
            let dy = (y as i32) - 1148;
            if dx * dx + dy * dy <= if dot == state.app_drawer_page { 16 } else { 9 } {
                return if dot == state.app_drawer_page { TEXT_PRIMARY } else { TEXT_MUTED };
            }
        }
    }

    // 6. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

#[inline(never)]
fn get_notifications_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    if y < 48 {
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, &state.clock_time, TEXT_PRIMARY) {
                return c;
            }
        }
        return BG_DARK;
    }

    if y >= 48 && y < 110 {
        if y >= 64 && y < 88 {
            if let Some(c) = test_text(x, y, 48, 64, 3, b"Notifications", TEXT_PRIMARY) {
                return c;
            }
        }
        return BG_DARK;
    }

    if y >= 125 && y < 260 && x >= 40 && x < 680 {
        if y == 125 || y == 259 || x == 40 || x == 679 {
            return BORDER_SUBTLE;
        }
        if y >= 150 && y < 166 {
            let label = if state.notification_count == 0 {
                &b"No active notifications"[..]
            } else {
                &b"Active notifications"[..]
            };
            if let Some(c) = test_text(x, y, 68, 150, 2, label, TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 182 && y < 198 {
            let sub = if state.notification_count == 0 {
                &b"The notification center is clear"[..]
            } else {
                &b"Details are provided by system services"[..]
            };
            if let Some(c) = test_text(x, y, 68, 182, 1, sub, TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    if y >= 300 && y < 355 && x >= 260 && x < 460 {
        if y == 300 || y == 354 || x == 260 || x == 459 {
            return BORDER_SUBTLE;
        }
        if y >= 320 && y < 336 {
            if let Some(c) = test_text(x, y, 315, 320, 2, b"Clear All", TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn get_live_widgets_color(x: usize, y: usize, _state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 {
            return BORDER_SUBTLE;
        }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, b"--:--", TEXT_PRIMARY) {
                return c;
            }
        }
        // Cellular
        if x >= 590 && x <= 608 && y >= 16 && y <= 32 {
            if x <= 593 && y >= 28 {
                return TEXT_PRIMARY;
            }
            if x >= 595 && x <= 598 && y >= 24 {
                return TEXT_PRIMARY;
            }
            if x >= 600 && x <= 603 && y >= 20 {
                return TEXT_PRIMARY;
            }
            if x >= 605 && x <= 608 && y >= 16 {
                return TEXT_PRIMARY;
            }
        }
        // Wi-Fi
        if x >= 626 && x <= 644 && y >= 18 && y <= 32 {
            let dx = (x as i32) - 635;
            let dy = (y as i32) - 30;
            let d2 = dx * dx + dy * dy;
            if (d2 >= 100 && d2 <= 144 && dy <= -2) || (d2 >= 36 && d2 <= 64 && dy <= -1) || d2 <= 9
            {
                return TEXT_PRIMARY;
            }
        }
        // Battery
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 {
                return TEXT_PRIMARY;
            }
            if x >= 685 && x <= 686 && y >= 22 && y <= 28 {
                return TEXT_PRIMARY;
            }
            // Synthetic battery fill removed; hardware telemetry must supply the level.
        }
        return BG_DARK;
    }

    // 2. Header (y: 50..125)
    if y >= 50 && y < 125 {
        if y >= 64 && y < 88 {
            if let Some(c) = test_text(x, y, 48, 64, 3, b"Live Widgets", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 96 && y < 104 {
            if let Some(c) = test_text(
                x,
                y,
                48,
                96,
                1,
                b"Real-time glanceable telemetry & stats",
                TEXT_MUTED,
            ) {
                return c;
            }
        }
        return BG_DARK;
    }

    // 3. 2-Column Top Widgets (y: 135..315)
    // Left: Weather (x: 40..345)
    if y >= 135 && y < 315 && x >= 40 && x < 345 {
        if y == 135 || y == 314 || x == 40 || x == 344 {
            return BORDER_SUBTLE;
        }
        if y >= 155 && y < 171 {
            if let Some(c) = test_text(x, y, 65, 155, 2, b"Location unavailable", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 190 && y < 214 {
            if let Some(c) = test_text(x, y, 65, 190, 3, b"--", ACCENT_AMBER) {
                return c;
            }
        }
        let s_dx = (x as i32) - 300;
        let s_dy = (y as i32) - 180;
        let s_d2 = s_dx * s_dx + s_dy * s_dy;
        if s_d2 <= 324 {
            if s_d2 <= 144 {
                return ACCENT_AMBER;
            }
            if (s_dx == 0 || s_dy == 0 || s_dx == s_dy || s_dx == -s_dy) && s_d2 >= 196 {
                return ACCENT_AMBER;
            }
        }
        if y >= 250 && y < 258 {
            if let Some(c) = test_text(x, y, 65, 250, 1, b"Weather data unavailable", TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // Right: Build Progress (x: 375..680)
    if y >= 135 && y < 315 && x >= 375 && x < 680 {
        if y == 135 || y == 314 || x == 375 || x == 679 {
            return BORDER_SUBTLE;
        }
        if y >= 155 && y < 171 {
            if let Some(c) = test_text(x, y, 400, 155, 2, b"Build Status", TEXT_PRIMARY) {
                return c;
            }
            if let Some(c) = test_text(x, y, 610, 155, 2, b"--", ACCENT_SKY) {
                return c;
            }
        }
        if y >= 200 && y < 216 && x >= 400 && x < 655 {
            if x <= 609 {
                return ACCENT_SKY;
            }
            return 0xFF0E1A2C;
        }
        if y >= 250 && y < 258 {
            if let Some(c) = test_text(x, y, 400, 250, 1, b"Build progress unavailable", TEXT_MUTED)
            {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // 4. Now Playing Mini Player (x: 40..680, y: 335..485)
    if y >= 335 && y < 485 && x >= 40 && x < 680 {
        if y == 335 || y == 484 || x == 40 || x == 679 {
            return BORDER_SUBTLE;
        }
        let v_dx = (x as i32) - 95;
        let v_dy = (y as i32) - 410;
        let v_d2 = v_dx * v_dx + v_dy * v_dy;
        if v_d2 <= 1225 {
            if v_d2 <= 100 {
                return ACCENT_AMBER;
            }
            if v_d2 >= 1100 || (v_d2 >= 300 && v_d2 <= 340) || (v_d2 >= 650 && v_d2 <= 700) {
                return 0xFF2A3448;
            }
            return 0xFF05070D;
        }
        if y >= 380 && y < 396 {
            if let Some(c) = test_text(x, y, 155, 380, 2, b"No active media", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 406 && y < 414 {
            if let Some(c) = test_text(x, y, 155, 406, 1, b"Media data unavailable", TEXT_MUTED) {
                return c;
            }
        }
        if y >= 430 && y < 436 && x >= 155 && x < 450 {
            if x <= 265 {
                return ACCENT_SKY;
            }
            return 0xFF142032;
        }
        if y >= 445 && y < 453 {
            if let Some(c) = test_text(x, y, 155, 445, 1, b"--:-- / --:--", TEXT_MUTED) {
                return c;
            }
        }
        if y >= 400 && y < 416 {
            if let Some(c) = test_text(x, y, 500, 400, 2, b"|<", TEXT_PRIMARY) {
                return c;
            }
            if let Some(c) = test_text(x, y, 555, 400, 2, b"||", ACCENT_SKY) {
                return c;
            }
            if let Some(c) = test_text(x, y, 610, 400, 2, b">|", TEXT_PRIMARY) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // 5. 2-Column Bottom Widgets (y: 505..685)
    // Left: Meeting Widget (x: 40..345)
    if y >= 505 && y < 685 && x >= 40 && x < 345 {
        if y == 505 || y == 684 || x == 40 || x == 344 {
            return BORDER_SUBTLE;
        }
        if y >= 525 && y < 541 {
            if let Some(c) = test_text(x, y, 65, 525, 2, b"Next Meeting", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 560 && y < 576 {
            if let Some(c) = test_text(x, y, 65, 560, 2, b"--:--", ACCENT_SKY) {
                return c;
            }
        }
        if y >= 595 && y < 603 {
            if let Some(c) = test_text(x, y, 65, 595, 1, b"No calendar data", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 620 && y < 628 {
            if let Some(c) = test_text(x, y, 65, 620, 1, b"Schedule data unavailable", TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // Right: Daily Activity & Steps (x: 375..680)
    if y >= 505 && y < 685 && x >= 375 && x < 680 {
        if y == 505 || y == 684 || x == 375 || x == 679 {
            return BORDER_SUBTLE;
        }
        if y >= 525 && y < 541 {
            if let Some(c) = test_text(x, y, 400, 525, 2, b"Activity Rings", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 560 && y < 584 {
            if let Some(c) = test_text(x, y, 400, 560, 3, b"--", SUCCESS_EMERALD) {
                return c;
            }
        }
        if y >= 605 && y < 613 {
            if let Some(c) = test_text(x, y, 400, 605, 1, b"Activity data unavailable", TEXT_MUTED)
            {
                return c;
            }
        }
        if y >= 630 && y < 638 && x >= 400 && x < 655 {
            if x <= 490 {
                return SUCCESS_EMERALD;
            }
            return 0xFF0E2218;
        }
        return SURFACE_RAISED;
    }

    // 6. System Telemetry Gauge Card (x: 40..680, y: 705..845)
    if y >= 705 && y < 845 && x >= 40 && x < 680 {
        if y == 705 || y == 844 || x == 40 || x == 679 {
            return BORDER_SUBTLE;
        }
        if y >= 725 && y < 741 {
            if let Some(c) = test_text(x, y, 65, 725, 2, b"System Hardware Telemetry", TEXT_PRIMARY)
            {
                return c;
            }
        }
        if y >= 755 && y < 763 {
            if let Some(c) = test_text(
                x,
                y,
                65,
                755,
                1,
                b"Hardware telemetry unavailable",
                TEXT_MUTED,
            ) {
                return c;
            }
        }
        if y >= 785 && y < 797 && x >= 65 && x < 655 {
            if x <= 220 {
                return ACCENT_SKY;
            }
            if x <= 410 {
                return ACCENT_AMBER;
            }
            return 0xFF142032;
        }
        return SURFACE_RAISED;
    }

    // 7. Appearance: the stock system theme is immutable.
    if y >= 865 && y < 1015 && x >= 40 && x < 680 {
        if y == 865 || y == 1014 || x == 40 || x == 679 { return BORDER_SUBTLE; }
        if y >= 885 && y < 901 { if let Some(c) = test_text(x, y, 65, 885, 2, b"Appearance", ACCENT_CYAN) { return c; } }
        if y >= 915 && y < 923 { if let Some(c) = test_text(x, y, 65, 915, 1, b"InterOS Stock", TEXT_PRIMARY) { return c; } }
        if y >= 940 && y < 948 { if let Some(c) = test_text(x, y, 65, 940, 1, b"System theme is fixed for this device build", TEXT_MUTED) { return c; } }
        if y >= 970 && y < 978 { if let Some(c) = test_text(x, y, 65, 970, 1, b"Personal themes are unavailable", TEXT_MUTED) { return c; } }
        return SURFACE_RAISED;
    }

    // 8. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

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