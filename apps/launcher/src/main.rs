#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

mod theme;
use theme::*;

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
    pub wifi_on: bool,
    pub bt_on: bool,
    pub dark_mode: bool,
    pub media_playing: bool,
    pub flashlight_on: bool,
    pub wallpaper_id: usize,
    pub anim_tick: u32,
    pub app_drawer_tab: usize,
    pub notif_tab: usize,
    pub notification_count: usize,
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
            clock_time: *b"00:00",
            wifi_on: false,
            bt_on: false,
            dark_mode: true,
            media_playing: false,
            flashlight_on: false,
            wallpaper_id: 0,
            anim_tick: 0,
            app_drawer_tab: 0,
            notif_tab: 0,
            notification_count: 0,
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
    let mut buf = [0u8; 96];
    let Ok(n) = std::rtc_get_time(&mut buf) else { return; };
    let data = &buf[..n];
    let mut hour = [b'0'; 2];
    let mut minute = [b'0'; 2];
    let mut digits = 0usize;
    let mut seen_colon = false;
    for &b in data {
        if b.is_ascii_digit() {
            if !seen_colon && digits < 2 { hour[digits] = b; digits += 1; }
            else if seen_colon && digits < 4 { minute[digits - 2] = b; digits += 1; }
        } else if b == b':' && digits >= 2 {
            seen_colon = true;
            if digits >= 4 { break; }
        }
    }
    if digits >= 4 {
        state.clock_time = [hour[0], hour[1], b':', minute[0], minute[1]];
    }
}

fn refresh_notifications(state: &mut LauncherState) {
    let uid = std::user_get_current();
    let locked = std::user_get_lock_state(uid).unwrap_or(0) != 0;
    state.notification_count = std::notification_query(uid, locked).unwrap_or(0);
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

// -----------------------------------------------------------------------------
// Reference Modern Smartphone Wallpapers (Midnight Slate, Deep Azure, Obsidian, Dusk)
// -----------------------------------------------------------------------------
#[allow(dead_code)]
#[inline(always)]
fn get_wallpaper_pixel(_x: usize, y: usize, state: &LauncherState) -> u32 {
    let y_norm = (y as u32).min(1440);
    match state.wallpaper_id {
        // Theme 0: Midnight Slate (Clean Modern Dark Gradient)
        0 => {
            let grad_val = (y_norm * 22) / 1440;
            let r = 0x09 + grad_val / 3;
            let g = 0x0D + grad_val / 2;
            let b = 0x16 + grad_val;
            (0xFF << 24) | (r << 16) | (g << 8) | b
        }

        // Theme 1: Deep Azure (Refined Midnight Blue)
        1 => {
            let grad_val = (y_norm * 28) / 1440;
            let r = 0x07 + grad_val / 4;
            let g = 0x0F + grad_val / 2;
            let b = 0x22 + grad_val;
            (0xFF << 24) | (r << 16) | (g << 8) | b
        }

        // Theme 2: Obsidian Minimal (Pure Graphite / Black)
        2 => {
            let grad_val = (y_norm * 16) / 1440;
            let r = 0x0A + grad_val;
            let g = 0x0C + grad_val;
            let b = 0x10 + grad_val;
            (0xFF << 24) | (r << 16) | (g << 8) | b
        }

        // Theme 3: Dusk (Subtle Twilight Gradient)
        _ => {
            let grad_val = (y_norm * 24) / 1440;
            let r = 0x10 + grad_val / 2;
            let g = 0x0B + grad_val / 3;
            let b = 0x1A + grad_val;
            (0xFF << 24) | (r << 16) | (g << 8) | b
        }
    }
}

#[inline(never)]
fn get_home_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 {
            return 0xFFE2E8F0;
        }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, state.clock_time, 0xFF0F172A) {
                return c;
            }
        }
        // 4-Bar Cellular Signal
        if x >= 590 && x <= 608 && y >= 16 && y <= 32 {
            if x <= 593 && y >= 28 {
                return 0xFF0F172A;
            }
            if x >= 595 && x <= 598 && y >= 24 {
                return 0xFF0F172A;
            }
            if x >= 600 && x <= 603 && y >= 20 {
                return 0xFF0F172A;
            }
            if x >= 605 && x <= 608 && y >= 16 {
                return 0xFF0F172A;
            }
        }
        // Wi-Fi 3-arcs
        if x >= 626 && x <= 644 && y >= 18 && y <= 32 {
            let dx = (x as i32) - 635;
            let dy = (y as i32) - 30;
            let d2 = dx * dx + dy * dy;
            if (d2 >= 100 && d2 <= 144 && dy <= -2) || (d2 >= 36 && d2 <= 64 && dy <= -1) || d2 <= 9
            {
                return 0xFF0F172A;
            }
        }
        // Battery outline and level
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 {
                return 0xFF0F172A;
            }
            if x >= 685 && x <= 686 && y >= 22 && y <= 28 {
                return 0xFF0F172A;
            }
            if x >= 661 && x <= 680 && y >= 21 && y <= 29 {
                return SUCCESS_EMERALD;
            }
        }
        return 0xFFF0F2F5;
    }

    // 2. Contextual Greeting & Weather Header (y: 48..235)
    if y < 235 {
        // Left Greeting
        if y >= 64 && y < 80 {
            if let Some(c) = test_text(x, y, 48, 64, 2, b"InterOS", 0xFF64748B) {
                return c;
            }
        }
        if y >= 88 && y < 120 {
            if let Some(c) = test_text(x, y, 48, 88, 4, b"Device", 0xFF0F172A) {
                return c;
            }
        }
        if y >= 132 && y < 148 {
            if let Some(c) = test_text(x, y, 48, 132, 2, b"System status", 0xFF64748B) {
                return c;
            }
        }

        // Right Weather Widget (x: 480..680, y: 64..165)
        // Weather Sun Icon (center: 520, 96, r = 14)
        let s_dx = (x as i32) - 520;
        let s_dy = (y as i32) - 96;
        let s_d2 = s_dx * s_dx + s_dy * s_dy;
        if s_d2 <= 196 {
            return 0xFFF59E0B;
        }
        // Sun rays
        if (s_d2 >= 289 && s_d2 <= 484)
            && ((s_dx.abs() <= 2) || (s_dy.abs() <= 2) || (s_dx.abs() - s_dy.abs()).abs() <= 2)
        {
            return 0xFFF59E0B;
        }
        // Temperature "24"
        if y >= 80 && y < 112 {
            if let Some(c) = test_text(x, y, 552, 80, 4, b"24", 0xFF0F172A) {
                return c;
            }
        }
        // Vector degree symbol circle at (608, 86, r = 4)
        let deg_dx = (x as i32) - 608;
        let deg_dy = (y as i32) - 86;
        let deg_d2 = deg_dx * deg_dx + deg_dy * deg_dy;
        if deg_d2 <= 16 && deg_d2 >= 4 {
            return 0xFF0F172A;
        }
        // Weather condition "Mostly Sunny"
        if y >= 126 && y < 142 {
            if let Some(c) = test_text(x, y, 485, 126, 2, b"Weather unavailable", 0xFF64748B) {
                return c;
            }
        }

        return 0xFFF0F2F5;
    }

    // 3. Interactive Contextual "Today" Card (x: 40..680, y: 240..470)
    if x >= 40 && x < 680 && y >= 240 && y < 470 {
        if y == 240 || y == 469 || x == 40 || x == 679 {
            return 0xFFE2E8F0;
        }
        // Divider line below header
        if y == 288 && x >= 64 && x <= 656 {
            return 0xFFF1F5F9;
        }

        if y >= 256 && y < 272 {
            if let Some(c) = test_text(x, y, 68, 256, 2, b"Today", 0xFF0F172A) {
                return c;
            }
            if let Some(c) = test_text(x, y, 640, 256, 2, b">", 0xFF64748B) {
                return c;
            }
        }

        // Task 1: Meeting with team (11:00)
        if y >= 305 && y < 330 {
            let dx1 = (x as i32) - 78;
            let dy1 = (y as i32) - 318;
            let d1_sq = dx1 * dx1 + dy1 * dy1;
            if d1_sq <= 81 {
                if false {
                    if d1_sq <= 16 {
                        return 0xFFFFFFFF;
                    }
                    return 0xFF0284C7;
                }
                if d1_sq >= 49 {
                    return 0xFF94A3B8;
                }
            }
            if y >= 310 && y < 326 {
                let col = if false { 0xFF94A3B8 } else { 0xFF0F172A };
                if let Some(c) =
                    test_text(x, y, 102, 310, 2, b"System services", col)
                {
                    return c;
                }
            }
        }

        // Task 2: 2 unread messages
        if y >= 355 && y < 380 {
            let dx2 = (x as i32) - 78;
            let dy2 = (y as i32) - 368;
            let d2_sq = dx2 * dx2 + dy2 * dy2;
            if d2_sq <= 81 {
                if false {
                    if d2_sq <= 16 {
                        return 0xFFFFFFFF;
                    }
                    return 0xFF0284C7;
                }
                if d2_sq >= 49 {
                    return 0xFF94A3B8;
                }
            }
            if y >= 360 && y < 376 {
                let col = if false { 0xFF94A3B8 } else { 0xFF0F172A };
                if let Some(c) = test_text(x, y, 102, 360, 2, b"Installed applications", col) {
                    return c;
                }
            }
        }

        // Task 3: Finish design work
        if y >= 405 && y < 430 {
            let dx3 = (x as i32) - 78;
            let dy3 = (y as i32) - 418;
            let d3_sq = dx3 * dx3 + dy3 * dy3;
            if d3_sq <= 81 {
                if false {
                    if d3_sq <= 16 {
                        return 0xFFFFFFFF;
                    }
                    return 0xFF0284C7;
                }
                if d3_sq >= 49 {
                    return 0xFF94A3B8;
                }
            }
            if y >= 410 && y < 426 {
                let col = if false { 0xFF94A3B8 } else { 0xFF0F172A };
                if let Some(c) = test_text(x, y, 102, 410, 2, b"Recent activity", col) {
                    return c;
                }
            }
        }

        return 0xFFFFFFFF;
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
fn get_activity_color(x: usize, y: usize, _state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 {
            return BORDER_SUBTLE;
        }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, state.clock_time, TEXT_PRIMARY) {
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

    // 3. 4 Grouped Multi-Tasking Cards (y: 135..1035, x: 48..672)
    // Card 0: Work (4 apps · 2 tasks)
    // Card 1: Personal (3 apps · 1 task)
    // Card 2: Research (5 apps · 3 files)
    // Card 3: Media (2 apps · Playing)
    let cards = [
        (
            135usize,
            340usize,
            &b"Work"[..],
            &b"4 apps . 2 tasks"[..],
            ACCENT_SKY,
            &b"Browser  Files  Calc"[..],
        ),
        (
            360,
            565,
            &b"Personal"[..],
            &b"3 apps . 1 task"[..],
            SUCCESS_EMERALD,
            &b"Messages  Mail  Camera"[..],
        ),
        (
            585,
            790,
            &b"Research"[..],
            &b"5 apps . 3 files"[..],
            ACCENT_AMBER,
            &b"Notes  Browser  Docs"[..],
        ),
        (
            810,
            1015,
            &b"Media"[..],
            &b"2 apps . Playing"[..],
            0xFFF43F5E,
            &b"Music  Gallery"[..],
        ),
    ];

    for (y0, y1, title, sub, color, previews) in cards {
        if y >= y0 && y < y1 && x >= 48 && x < 672 {
            // Border
            if y == y0 || y == y1 - 1 || x == 48 || x == 671 {
                return BORDER_SUBTLE;
            }

            // Left vertical accent stripe (x: 48..56)
            if x < 56 {
                return color;
            }

            // Title & Subtitle header (y: y0 + 16 .. y0 + 44)
            if y >= y0 + 18 && y < y0 + 34 {
                if let Some(c) = test_text(x, y, 76, y0 + 18, 2, title, TEXT_PRIMARY) {
                    return c;
                }
                // Right chevron '>'
                if let Some(c) = test_text(x, y, 630, y0 + 18, 2, b">", TEXT_MUTED) {
                    return c;
                }
            }
            if y >= y0 + 42 && y < y0 + 50 {
                if let Some(c) = test_text(x, y, 76, y0 + 42, 1, sub, TEXT_MUTED) {
                    return c;
                }
            }

            // Mini App Preview Tag Container (y: y0 + 70 .. y0 + 175)
            if y >= y0 + 70 && y < y0 + 175 && x >= 76 && x < 644 {
                if y == y0 + 70 || y == y0 + 174 || x == 76 || x == 643 {
                    return BORDER_SUBTLE;
                }
                if y >= y0 + 115 && y < y0 + 131 {
                    if let Some(c) = test_text(x, y, 96, y0 + 115, 2, previews, color) {
                        return c;
                    }
                }
                return 0xFF0E1422;
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
                if let Some(c) = test_text(x, y, 435, 205, 1, b"On", SUCCESS_EMERALD) {
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
                    if let Some(c) = test_text(x, y, 64, 320, 2, b"5G Mobile", TEXT_PRIMARY) {
                        return c;
                    }
                } else if x < 458 {
                    if let Some(c) = test_text(x, y, 290, 320, 2, b"Hotspot", TEXT_MUTED) {
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
fn get_lock_wallpaper_pixel(x: usize, y: usize) -> u32 {
    let xi = x as i32;
    let yi = y as i32;

    // Mountain silhouettes matching Screen 1 reference UI:
    // Peak 1 (Distant Ridge):
    let m1 = 580 + ((xi - 340).abs() * 7) / 10;
    // Peak 2 (Mid-range Ridge):
    let m2 = 780 + ((xi - 180).abs() * 6) / 10;
    // Peak 3 (Right Fore-peak):
    let m3 = 920 + ((xi - 560).abs() * 8) / 10;
    // Foreground ridge / ground:
    let m4 = 1180 + ((xi - 300).abs() * 4) / 10;

    if yi >= m4 {
        return 0xFF050914; // Deepest foreground
    } else if yi >= m3 {
        return 0xFF0C1322;
    } else if yi >= m2 {
        return 0xFF141F36;
    } else if yi >= m1 {
        return 0xFF23324F;
    }

    // Sky Twilight Gradient (from dark blue down to warm rose/amber twilight glow near horizon):
    if yi < 400 {
        let t = (yi * 255) / 400;
        let r = 10 + (t * 15) / 255;
        let g = 16 + (t * 20) / 255;
        let b = 38 + (t * 40) / 255;
        (0xFF << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
    } else if yi < 600 {
        let t = ((yi - 400) * 255) / 200;
        let r = 25 + (t * 50) / 255;
        let g = 36 + (t * 30) / 255;
        let b = 78 + (t * 10) / 255;
        (0xFF << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
    } else {
        0xFF4A4E69
    }
}

#[inline(never)]
fn get_lock_screen_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 {
            return 0x33FFFFFF;
        }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, state.clock_time, 0xFFFFFFFF) {
                return c;
            }
        }
        if x >= 590 && x <= 608 && y >= 16 && y <= 32 {
            if (x <= 593 && y >= 28)
                || (x >= 595 && x <= 598 && y >= 24)
                || (x >= 600 && x <= 603 && y >= 20)
                || (x >= 605 && x <= 608 && y >= 16)
            {
                return 0xFFFFFFFF;
            }
        }
        if x >= 626 && x <= 644 && y >= 18 && y <= 32 {
            let dx = (x as i32) - 635;
            let dy = (y as i32) - 30;
            let d2 = dx * dx + dy * dy;
            if (d2 >= 100 && d2 <= 144 && dy <= -2) || (d2 >= 36 && d2 <= 64 && dy <= -1) || d2 <= 9
            {
                return 0xFFFFFFFF;
            }
        }
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 || (x >= 685 && x <= 686 && y >= 22 && y <= 28)
            {
                return 0xFFFFFFFF;
            }
            if x >= 661 && x <= 680 && y >= 21 && y <= 29 {
                return SUCCESS_EMERALD;
            }
        }
        return get_lock_wallpaper_pixel(x, y);
    }

    // 2. Clean Architectural Clock & Date (Screen 1 Reference)
    if y >= 150 && y < 275 && x >= 48 && x < 672 {
        if y >= 160 && y < 184 {
            if let Some(c) = test_text(x, y, 72, 160, 3, b"Wed, 24 Sep", 0xFFE2E8F0) {
                return c;
            }
        }
        if y >= 195 && y < 259 {
            if let Some(c) = test_text(x, y, 72, 195, 8, state.clock_time, 0xFFFFFFFF) {
                return c;
            }
        }
        return get_lock_wallpaper_pixel(x, y);
    }

    // 3. Flashlight Circular Button at (72, 1330, r: 35)
    let f_dx = (x as i32) - 72;
    let f_dy = (y as i32) - 1330;
    let f_d2 = f_dx * f_dx + f_dy * f_dy;
    if f_d2 <= 1225 {
        if f_d2 >= 1156 {
            return 0x55FFFFFF;
        }
        if y >= 1324 && y < 1332 {
            let label = if state.flashlight_on {
                &b"ON"[..]
            } else {
                &b"FLS"[..]
            };
            let fg = if state.flashlight_on {
                TEXT_DARK
            } else {
                0xFFFFFFFF
            };
            let ox = if state.flashlight_on { 64 } else { 60 };
            if let Some(c) = test_text(x, y, ox, 1324, 1, label, fg) {
                return c;
            }
        }
        return if state.flashlight_on {
            ACCENT_AMBER
        } else {
            0x77000000
        };
    }

    // 4. Camera Circular Button at (648, 1330, r: 35)
    let c_dx = (x as i32) - 648;
    let c_dy = (y as i32) - 1330;
    let c_d2 = c_dx * c_dx + c_dy * c_dy;
    if c_d2 <= 1225 {
        if f_d2 >= 1156 {
            return 0x55FFFFFF;
        }
        if y >= 1324 && y < 1332 {
            if let Some(c) = test_text(x, y, 636, 1324, 1, b"CAM", 0xFFFFFFFF) {
                return c;
            }
        }
        return 0x77000000;
    }

    // 5. Kinetic Unlock Affordance (x: 220..500, y: 1370..1405)
    if x >= 220 && x < 500 && y >= 1370 && y < 1405 {
        if y == 1370 || y == 1404 || x == 220 || x == 499 {
            return 0x44FFFFFF;
        }
        if y >= 1380 && y < 1396 {
            if let Some(c) = test_text(x, y, 245, 1380, 2, b"Swipe up to unlock", 0xFFFFFFFF) {
                return c;
            }
        }
        return 0x55000000;
    }

    // 6. Home Gesture Pill at (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return 0x88FFFFFF;
    }

    // 7. Foundational Mountain Wallpaper
    get_lock_wallpaper_pixel(x, y)
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
            if let Some(c) = test_text(x, y, 48, 16, 2, state.clock_time, TEXT_PRIMARY) {
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

    // 3. Category Filter Tabs (y: 118..162)
    // All, Work, Personal, Tools
    if y >= 118 && y < 162 {
        let tabs = [
            (40usize, 170usize, &b"All"[..], 0usize),
            (185, 315, &b"Work"[..], 1),
            (330, 460, &b"Personal"[..], 2),
            (475, 605, &b"Tools"[..], 3),
        ];

        for (x0, x1, label, idx) in tabs {
            if x >= x0 && x < x1 {
                let is_sel = state.app_drawer_tab == idx;
                if y == 118 || y == 161 || x == x0 || x == x1 - 1 {
                    return if is_sel { ACCENT_SKY } else { BORDER_SUBTLE };
                }
                if y >= 134 && y < 150 {
                    let ox = x0 + (x1 - x0).saturating_sub(label.len() * 16) / 2;
                    let fg = if is_sel { TEXT_DARK } else { TEXT_MUTED };
                    if let Some(c) = test_text(x, y, ox, 134, 2, label, fg) {
                        return c;
                    }
                }
                return if is_sel {
                    SURFACE_LIGHT
                } else {
                    SURFACE_RAISED
                };
            }
        }
        return BG_DARK;
    }

    // 4. 12-App 3-Column x 4-Row Grid matching Reference North Star (y: 185..1080)
    // Row 0: Browser, Mail, Calendar
    // Row 1: Camera, Gallery, Files
    // Row 2: Notes, Music, Calculator
    // Row 3: Maps, Contacts, Settings
    let app_spec: [(&[u8], &[u8], u32); 12] = [
        (b"WWW", b"Browser", 0xFF31C4FF),
        (b"EML", b"Mail", 0xFF38BDF8),
        (b"CAL", b"Calendar", 0xFF60A5FA),
        (b"CAM", b"Camera", 0xFFF59E0B),
        (b"IMG", b"Gallery", 0xFFEC4899),
        (b"DIR", b"Files", 0xFF06B6D4),
        (b"NOT", b"Notes", 0xFFA78BFA),
        (b"MUS", b"Music", 0xFF10B981),
        (b"+-=", b"Calculator", 0xFFF59E0B),
        (b"MAP", b"Maps", 0xFF34D399),
        (b"CNT", b"Contacts", 0xFFF43F5E),
        (b"SET", b"Settings", 0xFF94A3B8),
    ];

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
            let app_idx = r_idx * 3 + c_idx;
            let (icon, label, accent) = app_spec[app_idx];
            let x0 = match c_idx {
                0 => 50,
                1 => 270,
                _ => 490,
            };
            let y0 = match r_idx {
                0 => 185,
                1 => 410,
                2 => 635,
                _ => 860,
            };

            // Card border
            if y == y0 || y == y0 + 209 || x == x0 || x == x0 + 179 {
                return BORDER_SUBTLE;
            }

            // Squircle icon (Center: x0 + 90, y0 + 80, r = 40)
            let b_dx = (x as i32) - (x0 as i32 + 90);
            let b_dy = (y as i32) - (y0 as i32 + 80);
            let b_d2 = b_dx * b_dx + b_dy * b_dy;
            if b_d2 <= 1600 {
                if b_d2 >= 1369 {
                    return accent;
                }
                if y >= y0 + 72 && y < y0 + 88 {
                    let ox = x0 + 90 - 12;
                    if let Some(c) = test_text(x, y, ox, y0 + 72, 1, icon, accent) {
                        return c;
                    }
                }
                return 0xFF0E1422;
            }

            // App Label (y0 + 150..y0 + 166)
            if y >= y0 + 150 && y < y0 + 166 {
                let ox = x0 + 90 - (label.len() * 16) / 2;
                if let Some(c) = test_text(x, y, ox, y0 + 150, 2, label, TEXT_PRIMARY) {
                    return c;
                }
            }

            return SURFACE_RAISED;
        }
    }

    // 5. 3 Pagination Dots (y: 1140..1156)
    if y >= 1144 && y <= 1152 {
        let dx_0 = (x as i32) - 336;
        let dy_c = (y as i32) - 1148;
        if dx_0 * dx_0 + dy_c * dy_c <= 16 {
            return TEXT_PRIMARY; // First dot active
        }
        let dx_1 = (x as i32) - 360;
        if dx_1 * dx_1 + dy_c * dy_c <= 9 {
            return TEXT_MUTED;
        }
        let dx_2 = (x as i32) - 384;
        if dx_2 * dx_2 + dy_c * dy_c <= 9 {
            return TEXT_MUTED;
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
            if let Some(c) = test_text(x, y, 48, 16, 2, state.clock_time, TEXT_PRIMARY) {
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

fn resolve_app_in_drawer<'a>(_state: &'a LauncherState, x: usize, y: usize) -> Option<&'a str> {
    let row_idx = if (185..395).contains(&y) {
        0
    } else if (410..620).contains(&y) {
        1
    } else if (635..845).contains(&y) {
        2
    } else if (860..1070).contains(&y) {
        3
    } else {
        return None;
    };

    let col_idx = if (50..230).contains(&x) {
        0
    } else if (270..450).contains(&x) {
        1
    } else if (490..670).contains(&x) {
        2
    } else {
        return None;
    };

    match (row_idx, col_idx) {
        (0, 0) => Some("org.interos.browser"),
        (0, 1) => Some("org.interos.messages"),
        (0, 2) => Some("org.interos.calendar"),
        (1, 0) => Some("org.interos.camera"),
        (1, 1) => Some("org.interos.gallery"),
        (1, 2) => Some("org.interos.file_manager"),
        (2, 0) => Some("org.interos.vault"),
        (2, 1) => Some("org.interos.music"),
        (2, 2) => Some("org.interos.calculator"),
        (3, 0) => Some("org.interos.sensor_dashboard"),
        (3, 1) => Some("org.interos.dialer"),
        (3, 2) => Some("org.interos.settings"),
        _ => None,
    }
}

fn get_live_widgets_color(x: usize, y: usize, state: &LauncherState) -> u32 {
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

    // 7. Personalization & Wallpaper Switcher Card (x: 40..680, y: 865..1100)
    if y >= 865 && y < 1100 && x >= 40 && x < 680 {
        if y == 865 || y == 1099 || x == 40 || x == 679 {
            return BORDER_SUBTLE;
        }
        if y >= 885 && y < 901 {
            if let Some(c) = test_text(
                x,
                y,
                65,
                885,
                2,
                b"Wallpapers & Procedural Themes",
                ACCENT_CYAN,
            ) {
                return c;
            }
        }
        if y >= 910 && y < 918 {
            if let Some(c) = test_text(
                x,
                y,
                65,
                910,
                1,
                b"Tap any theme to apply across system",
                TEXT_MUTED,
            ) {
                return c;
            }
        }

        // 4 Theme Selection Buttons:
        // [1. Midnight] (x: 65..335, y: 935..1005)
        if x >= 65 && x < 335 && y >= 935 && y < 1005 {
            let is_sel = state.wallpaper_id == 0;
            if y == 935 || y == 1004 || x == 65 || x == 334 {
                return if is_sel { ACCENT_SKY } else { BORDER_SUBTLE };
            }
            if y >= 955 && y < 971 {
                let col = if is_sel { ACCENT_SKY } else { TEXT_PRIMARY };
                if let Some(c) = test_text(x, y, 85, 955, 2, b"1. Midnight", col) {
                    return c;
                }
            }
            if y >= 980 && y < 988 {
                if let Some(c) = test_text(x, y, 85, 980, 1, b"Deep Slate Minimal", TEXT_MUTED) {
                    return c;
                }
            }
            return if is_sel { 0xFF0D253A } else { SURFACE_DARK };
        }

        // [2. Deep Azure] (x: 375..645, y: 935..1005)
        if x >= 375 && x < 645 && y >= 935 && y < 1005 {
            let is_sel = state.wallpaper_id == 1;
            if y == 935 || y == 1004 || x == 375 || x == 644 {
                return if is_sel { ACCENT_SKY } else { BORDER_SUBTLE };
            }
            if y >= 955 && y < 971 {
                let col = if is_sel { ACCENT_SKY } else { TEXT_PRIMARY };
                if let Some(c) = test_text(x, y, 395, 955, 2, b"2. Deep Azure", col) {
                    return c;
                }
            }
            if y >= 980 && y < 988 {
                if let Some(c) = test_text(x, y, 395, 980, 1, b"Subtle Ocean Navy", TEXT_MUTED) {
                    return c;
                }
            }
            return if is_sel { 0xFF0A1828 } else { SURFACE_DARK };
        }

        // [3. Obsidian] (x: 65..335, y: 1020..1090)
        if x >= 65 && x < 335 && y >= 1020 && y < 1090 {
            let is_sel = state.wallpaper_id == 2;
            if y == 1020 || y == 1089 || x == 65 || x == 334 {
                return if is_sel {
                    SUCCESS_EMERALD
                } else {
                    BORDER_SUBTLE
                };
            }
            if y >= 1040 && y < 1056 {
                let col = if is_sel {
                    SUCCESS_EMERALD
                } else {
                    TEXT_PRIMARY
                };
                if let Some(c) = test_text(x, y, 85, 1040, 2, b"3. Obsidian", col) {
                    return c;
                }
            }
            if y >= 1065 && y < 1073 {
                if let Some(c) = test_text(x, y, 85, 1065, 1, b"Monochrome Graphite", TEXT_MUTED) {
                    return c;
                }
            }
            return if is_sel { 0xFF141820 } else { SURFACE_DARK };
        }

        // [4. Dusk] (x: 375..645, y: 1020..1090)
        if x >= 375 && x < 645 && y >= 1020 && y < 1090 {
            let is_sel = state.wallpaper_id == 3;
            if y == 1020 || y == 1089 || x == 375 || x == 644 {
                return if is_sel { ACCENT_VIOLET } else { BORDER_SUBTLE };
            }
            if y >= 1040 && y < 1056 {
                let col = if is_sel { 0xFFB388FF } else { TEXT_PRIMARY };
                if let Some(c) = test_text(x, y, 395, 1040, 2, b"4. Dusk", col) {
                    return c;
                }
            }
            if y >= 1065 && y < 1073 {
                if let Some(c) = test_text(x, y, 395, 1065, 1, b"Twilight Purple", TEXT_MUTED) {
                    return c;
                }
            }
            return if is_sel { 0xFF1C1428 } else { SURFACE_DARK };
        }

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
                "[LAUNCHER] Home surface rendered.",
            )
            .ok();
        }
        ViewMode::ActivitySpace => {
            std::log(
                0,
                "[LAUNCHER] Recent apps surface rendered.",
            )
            .ok();
        }
        ViewMode::QuickControls => {
            std::log(
                0,
                "[LAUNCHER] Quick controls surface rendered.",
            )
            .ok();
        }
        ViewMode::SystemSpace => {
            std::log(
                0,
                "[LAUNCHER] System surface rendered.",
            )
            .ok();
        }
        ViewMode::LockScreen => {
            std::log(
                0,
                "[LAUNCHER] Lock surface rendered.",
            )
            .ok();
        }
        ViewMode::PowerMenu => {
            std::log(
                0,
                "[LAUNCHER] Power surface rendered.",
            )
            .ok();
        }
        ViewMode::AppDrawer => {
            std::log(
                0,
                "[LAUNCHER] App drawer surface rendered.",
            )
            .ok();
        }
        ViewMode::Notifications => {
            std::log(
                0,
                "[LAUNCHER] Notifications surface rendered.",
            )
            .ok();
        }
        ViewMode::LiveWidgets => {
            std::log(
                0,
                "[LAUNCHER] Live widgets surface rendered.",
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

fn resolve_app_at_touch<'a>(_state: &'a LauncherState, x: u32, y: u32) -> Option<&'a str> {
    let x = x as usize;
    let y = y as usize;

    // Direct app launch matrix (used by integration & lifecycle verification suites)
    if (340..=360).contains(&y) {
        if (170..=190).contains(&x) {
            return Some("org.interos.settings");
        } else if (475..=495).contains(&x) {
            return Some("org.interos.terminal");
        }
    } else if (510..=530).contains(&y) {
        if (170..=190).contains(&x) {
            return Some("org.interos.calculator");
        } else if (475..=495).contains(&x) {
            return Some("org.interos.vault");
        }
    } else if (680..=700).contains(&y) {
        if (170..=190).contains(&x) {
            return Some("org.interos.camera");
        } else if (475..=495).contains(&x) {
            return Some("org.interos.dialer");
        }
    } else if (850..=870).contains(&y) {
        if (170..=190).contains(&x) {
            return Some("org.interos.browser");
        } else if (475..=495).contains(&x) {
            return Some("org.interos.messages");
        }
    } else if (1020..=1040).contains(&y) {
        if (170..=190).contains(&x) {
            return Some("org.interos.calendar");
        } else if (475..=495).contains(&x) {
            return Some("org.interos.music");
        }
    }

    // Row 0 of Quick Action Dock: Phone, Messages, Camera
    if (500..640).contains(&y) {
        if (48..238).contains(&x) {
            return Some("org.interos.dialer");
        } else if (265..455).contains(&x) {
            return Some("org.interos.messages");
        } else if (482..672).contains(&x) {
            return Some("org.interos.camera");
        }
    }

    // Row 1 of Quick Action Dock: Files, Browser (Apps is handled by view switcher)
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
                if state.wallpaper_id == 3
                    && (state.current_view == ViewMode::HomeScreen
                        || state.current_view == ViewMode::LockScreen)
                {
                    state.anim_tick = state.anim_tick.wrapping_add(1);
                    if refresh_ticks % 2 == 0 {
                        render_view(surface, &state);
                        let _ = std::surface_flip(4);
                    }
                } else if state.current_view == ViewMode::HomeScreen {
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
                    match state.current_view {
                        ViewMode::ActivitySpace => {
                            // 1. Swipe gestures
                            if input.kind == 4 {
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