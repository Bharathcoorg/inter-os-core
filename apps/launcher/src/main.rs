#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// -----------------------------------------------------------------------------
// 32-bit ARGB8888 Design Color Tokens
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFF0A0D14;
const SURFACE_DARK: u32 = 0xFF121824;
const SURFACE_RAISED: u32 = 0xFF17243A;
const SURFACE_LIGHT: u32 = 0xFFFFFFFF;
const BORDER_SUBTLE: u32 = 0xFF222E42;
const TEXT_PRIMARY: u32 = 0xFFF8FAFC;
const TEXT_MUTED: u32 = 0xFF94A3B8;
const TEXT_DARK: u32 = 0xFF0F172A;
const ACCENT_SKY: u32 = 0xFF31C4FF;
const ACCENT_AMBER: u32 = 0xFFF59E0B;
const SUCCESS_EMERALD: u32 = 0xFF10B981;
const CLOCK_TIME: &[u8; 5] = &[b'0', b'9', b':', b'2', b'0'];
const DANGER_CRIMSON: u32 = 0xFFFF6B6B;
const ACCENT_CYAN: u32 = 0xFF00F2FE;
const ACCENT_VIOLET: u32 = 0xFF8A2BE2;

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
    pub task_done: [bool; 3],
    pub wifi_on: bool,
    pub bt_on: bool,
    pub dark_mode: bool,
    pub media_playing: bool,
    pub flashlight_on: bool,
    pub wallpaper_id: usize,
    pub anim_tick: u32,
    pub app_drawer_tab: usize,
    pub notif_tab: usize,
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
            task_done: [false, false, false],
            wifi_on: false,
            bt_on: false,
            dark_mode: true,
            media_playing: false,
            flashlight_on: false,
            wallpaper_id: 0,
            anim_tick: 0,
            app_drawer_tab: 0,
            notif_tab: 0,
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
        }
        self.recent_apps[2] = [0u8; 32];
        self.recent_app_lens[2] = 0;
        self.recent_task_ids[2] = 0;
        self.recent_task_ids[index] = 0;
    }
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

    state.recent_apps = [[0u8; 32]; 3];
    state.recent_app_lens = [0; 3];
    state.recent_task_ids = [0; 3];

    for line in buf[..n].split(|b| *b == b'\n') {
        if state.recent_app_lens.iter().all(|len| *len != 0) {
            break;
        }
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

        // Running/background/suspended applications are the live recent-task
        // set. Launcher/compositor are filtered by the kernel authority layer.
        if process_state > 2 || package_id.is_empty() || package_id.len() > 32 {
            continue;
        }
        if state.recent_app_lens.iter().any(|len| *len != 0) {
            if (0..3).any(|i| {
                let len = state.recent_app_lens[i] as usize;
                &state.recent_apps[i][..len] == package_id
            }) {
                continue;
            }
        }

        if let Some(slot) = state.recent_app_lens.iter().position(|len| *len == 0) {
            state.recent_apps[slot][..package_id.len()].copy_from_slice(package_id);
            state.recent_app_lens[slot] = package_id.len() as u8;
            state.recent_task_ids[slot] = task_id;
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
// Procedural Signature Wallpaper Engine (Quantum Horizon, Solar Corona, Cyber Matrix, Live Plasma)
// -----------------------------------------------------------------------------
#[inline(always)]
fn get_wallpaper_pixel(x: usize, y: usize, state: &LauncherState) -> u32 {
    match state.wallpaper_id {
        // Wallpaper 0: Quantum Horizon (Signature InterOS Cosmic Kinetic Lattice)
        0 => {
            let y_norm = (y as u32).min(1440);
            let grad_val = (y_norm * 24) / 1440;
            let base_r = 0x05 + grad_val / 3;
            let base_g = 0x08 + grad_val / 2;
            let base_b = 0x12 + grad_val;
            let base = (0xFF << 24) | (base_r << 16) | (base_g << 8) | base_b;

            // Primary Orbital Arc 1: Cyan resonance centered at (200, 760)
            let dx1 = (x as i32) - 200;
            let dy1 = (y as i32) - 760;
            let d1 = ((dx1 * dx1 + dy1 * dy1) as u32) >> 6;
            if d1 >= 2700 && d1 <= 2820 {
                return ACCENT_CYAN;
            }
            if d1 >= 2650 && d1 <= 2870 {
                return 0xFF0A2B3C;
            }

            // Secondary Orbital Arc 2: Violet resonance centered at (520, 640)
            let dx2 = (x as i32) - 520;
            let dy2 = (y as i32) - 640;
            let d2 = ((dx2 * dx2 + dy2 * dy2) as u32) >> 6;
            if d2 >= 2200 && d2 <= 2310 {
                return ACCENT_VIOLET;
            }
            if d2 >= 2160 && d2 <= 2350 {
                return 0xFF220D38;
            }

            // Horizon line
            if y == 940 && x >= 80 && x <= 640 {
                return 0xFF38BDF8;
            }

            // Starlight node coordinates
            if ((x * 37 + y * 73) % 1847) == 42 && y > 100 && y < 1300 {
                return 0xFFE0E7FF;
            }

            base
        }

        // Wallpaper 1: Solar Corona / Amber Aurora
        1 => {
            let y_norm = (y as u32).min(1440);
            let grad_val = (y_norm * 30) / 1440;
            let base_r = 0x14 + grad_val;
            let base_g = 0x0A + grad_val / 2;
            let base_b = 0x03;
            let base = (0xFF << 24) | (base_r << 16) | (base_g << 8) | base_b;

            let dx = (x as i32) - 360;
            let dy = (y as i32) - 380;
            let d = ((dx * dx + dy * dy) as u32) >> 6;
            if d >= 1800 && d <= 1920 {
                return ACCENT_AMBER;
            }
            if d >= 1700 && d <= 2020 {
                return 0xFF3D2305;
            }

            let d_flare = (((dx - 80) * (dx - 80) + (dy - 400) * (dy - 400)) as u32) >> 6;
            if d_flare >= 2400 && d_flare <= 2500 {
                return DANGER_CRIMSON;
            }

            base
        }

        // Wallpaper 2: Cyber Emerald Matrix
        2 => {
            let y_norm = (y as u32).min(1440);
            let grad_val = (y_norm * 20) / 1440;
            let base_r = 0x03;
            let base_g = 0x0D + grad_val;
            let base_b = 0x08 + grad_val / 2;
            let base = (0xFF << 24) | (base_r << 16) | (base_g << 8) | base_b;

            let sum = (x + y) % 180;
            let diff = ((x as i32) - (y as i32)).abs() as usize % 180;
            if (sum == 0 || diff == 0) && y >= 250 && y <= 1200 {
                return 0xFF0E3822;
            }
            if (sum == 0 && diff == 0) && y >= 250 && y <= 1200 {
                return SUCCESS_EMERALD;
            }

            base
        }

        // Wallpaper 3: Live Kinetic Plasma (Animated procedural energy rings)
        _ => {
            let dx = (x as i32) - 360;
            let dy = (y as i32) - 720;
            let dist_sq = ((dx * dx + dy * dy) as u32) >> 7;
            let phase = (dist_sq + state.anim_tick * 6) % 256;

            let r = 10 + (phase / 16);
            let g = 14 + (phase / 10);
            let b = 28 + (phase / 6);

            if phase >= 240 {
                ACCENT_CYAN
            } else if phase >= 220 {
                0xFF1E3A5F
            } else {
                (0xFF << 24) | (r << 16) | (g << 8) | b
            }
        }
    }
}

#[inline(never)]
fn get_home_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 {
            return BORDER_SUBTLE;
        }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, CLOCK_TIME, TEXT_PRIMARY) {
                return c;
            }
        }
        // 4-Bar Cellular Signal
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
        // Wi-Fi 3-arcs
        if x >= 626 && x <= 644 && y >= 18 && y <= 32 {
            let dx = (x as i32) - 635;
            let dy = (y as i32) - 30;
            let d2 = dx * dx + dy * dy;
            if (d2 >= 100 && d2 <= 144 && dy <= -2) || (d2 >= 36 && d2 <= 64 && dy <= -1) || d2 <= 9
            {
                return TEXT_PRIMARY;
            }
        }
        // Battery outline and level
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

    // 2. Wallpaper & Header (y: 48..235)
    if y < 235 {
        // Date & Tabular Clock
        if y >= 64 && y < 80 {
            if let Some(c) = test_text(x, y, 48, 64, 2, b"Wed, 24 Sep", TEXT_MUTED) {
                return c;
            }
        }
        if y >= 90 && y < 122 {
            if let Some(c) = test_text(x, y, 48, 90, 4, CLOCK_TIME, TEXT_PRIMARY) {
                return c;
            }
        }

        // Weather Widget Right
        if x >= 460 && x <= 680 && y >= 80 && y < 135 {
            let dx = (x as i32) - 490;
            let dy = (y as i32) - 96;
            if dx * dx + dy * dy <= 81 {
                return ACCENT_AMBER;
            }
            if y >= 84 && y < 92 {
                if let Some(c) = test_text(x, y, 520, 84, 1, b"Local", TEXT_MUTED) {
                    return c;
                }
            }
            if y >= 98 && y < 114 {
                if let Some(c) = test_text(x, y, 520, 98, 2, b"24*", TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 118 && y < 126 {
                if let Some(c) = test_text(x, y, 520, 118, 1, b"Sunny", TEXT_MUTED) {
                    return c;
                }
            }
        }

        // Contextual Greeting
        if y >= 145 && y < 169 {
            if let Some(c) = test_text(x, y, 48, 145, 3, b"Good morning,", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 175 && y < 199 {
            if let Some(c) = test_text(x, y, 48, 175, 3, b"Bharath", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 208 && y < 224 {
            if let Some(c) = test_text(x, y, 48, 208, 2, b"Let's make a good day.", TEXT_MUTED) {
                return c;
            }
        }

        return get_wallpaper_pixel(x, y, state);
    }

    // 3. Interactive Contextual "Today" Card (x: 40..680, y: 240..470)
    if x >= 40 && x < 680 && y >= 240 && y < 470 {
        if y >= 466 {
            return 0xFFE2E8F0;
        }

        if y >= 258 && y < 274 {
            if let Some(c) = test_text(x, y, 68, 258, 2, b"Today", TEXT_DARK) {
                return c;
            }
            if let Some(c) = test_text(x, y, 640, 258, 2, b">", 0xFF64748B) {
                return c;
            }
        }

        // Task 1: Meeting with team
        if y >= 305 && y < 330 {
            let dx1 = (x as i32) - 78;
            let dy1 = (y as i32) - 318;
            let d1_sq = dx1 * dx1 + dy1 * dy1;
            if d1_sq <= 64 {
                if state.task_done[0] {
                    return ACCENT_SKY;
                }
                if d1_sq >= 36 {
                    return 0xFF64748B;
                }
            }
            if y >= 310 && y < 326 {
                if let Some(c) =
                    test_text(x, y, 100, 310, 2, b"Meeting with team (11:00)", TEXT_DARK)
                {
                    return c;
                }
            }
        }

        // Task 2: "2 unread messages"
        if y >= 355 && y < 380 {
            let dx2 = (x as i32) - 78;
            let dy2 = (y as i32) - 368;
            let d2_sq = dx2 * dx2 + dy2 * dy2;
            if d2_sq <= 64 {
                if state.task_done[1] {
                    return ACCENT_SKY;
                }
                if d2_sq >= 36 {
                    return 0xFF64748B;
                }
            }
            if y >= 360 && y < 376 {
                if let Some(c) = test_text(x, y, 100, 360, 2, b"2 unread messages", TEXT_DARK) {
                    return c;
                }
            }
        }

        // Task 3: "Finish design work"
        if y >= 405 && y < 430 {
            let dx3 = (x as i32) - 78;
            let dy3 = (y as i32) - 418;
            let d3_sq = dx3 * dx3 + dy3 * dy3;
            if d3_sq <= 64 {
                if state.task_done[2] {
                    return ACCENT_SKY;
                }
                if d3_sq >= 36 {
                    return 0xFF64748B;
                }
            }
            if y >= 410 && y < 426 {
                if let Some(c) = test_text(x, y, 100, 410, 2, b"Finish design work", TEXT_DARK) {
                    return c;
                }
            }
        }

        return SURFACE_LIGHT;
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
                    return BORDER_SUBTLE;
                }

                // Tile Icon Badge (Center: x0 + 95, y0 + 55, r = 26)
                let b_dx = (x as i32) - (x0 as i32 + 95);
                let b_dy = (y as i32) - (y0 as i32 + 55);
                let b_d2 = b_dx * b_dx + b_dy * b_dy;
                let (icon_text, icon_color, label) = match card_idx {
                    0 => (&b"TEL"[..], ACCENT_SKY, &b"Phone"[..]),
                    1 => (&b"MSG"[..], SUCCESS_EMERALD, &b"Messages"[..]),
                    2 => (&b"CAM"[..], ACCENT_AMBER, &b"Camera"[..]),
                    3 => (&b"DIR"[..], 0xFF38BDF8, &b"Files"[..]),
                    4 => (&b"WWW"[..], 0xFFA78BFA, &b"Browser"[..]),
                    _ => (&b"APP"[..], 0xFFF43F5E, &b"Apps"[..]),
                };

                if b_d2 <= 676 {
                    if b_d2 >= 576 {
                        return icon_color;
                    }
                    if y >= y0 + 47 && y < y0 + 63 {
                        let ox = x0 + 95 - 12;
                        if let Some(c) = test_text(x, y, ox, y0 + 47, 1, icon_text, icon_color) {
                            return c;
                        }
                    }
                    return 0xFF0E1422;
                }

                // Label below icon (y0 + 100..y0 + 116)
                if y >= y0 + 100 && y < y0 + 116 {
                    let ox = x0 + 95 - (label.len() * 16) / 2;
                    if let Some(c) = test_text(x, y, ox, y0 + 100, 2, label, TEXT_PRIMARY) {
                        return c;
                    }
                }

                return SURFACE_RAISED;
            }
        }
        return BG_DARK;
    }

    // 5. Universal Search Command Bar (x: 40..680, y: 1290..1365)
    if x >= 40 && x < 680 && y >= 1290 && y < 1365 {
        if y == 1290 || y == 1364 || x == 40 || x == 679 {
            return BORDER_SUBTLE;
        }

        // Search magnifying glass icon
        let dx_s = (x as i32) - 68;
        let dy_s = (y as i32) - 1327;
        let d_s = dx_s * dx_s + dy_s * dy_s;
        if (d_s >= 36 && d_s <= 64) || (dx_s >= 6 && dx_s <= 10 && dx_s == dy_s) {
            return TEXT_MUTED;
        }

        // Search placeholder text
        if y >= 1320 && y < 1336 {
            if let Some(c) = test_text(x, y, 96, 1320, 2, b"Search apps and files...", TEXT_MUTED) {
                return c;
            }
        }

        // Right arrow icon
        if y >= 1320 && y < 1336 {
            if let Some(c) = test_text(x, y, 644, 1320, 2, b">", TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // 6. Bottom Home Gesture Bar (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    get_wallpaper_pixel(x, y, state)
}

#[inline(never)]
fn get_activity_color(x: usize, y: usize, _state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 {
            return BORDER_SUBTLE;
        }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, CLOCK_TIME, TEXT_PRIMARY) {
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
fn get_lock_screen_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 {
            return BORDER_SUBTLE;
        }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, CLOCK_TIME, TEXT_PRIMARY) {
                return c;
            }
        }
        if x >= 590 && x <= 608 && y >= 16 && y <= 32 {
            if x <= 593 && y >= 28 { return TEXT_PRIMARY; }
            if x >= 595 && x <= 598 && y >= 24 { return TEXT_PRIMARY; }
            if x >= 600 && x <= 603 && y >= 20 { return TEXT_PRIMARY; }
            if x >= 605 && x <= 608 && y >= 16 { return TEXT_PRIMARY; }
        }
        if x >= 626 && x <= 644 && y >= 18 && y <= 32 {
            let dx = (x as i32) - 635;
            let dy = (y as i32) - 30;
            let d2 = dx * dx + dy * dy;
            if (d2 >= 100 && d2 <= 144 && dy <= -2) || (d2 >= 36 && d2 <= 64 && dy <= -1) || d2 <= 9 {
                return TEXT_PRIMARY;
            }
        }
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 { return TEXT_PRIMARY; }
            if x >= 685 && x <= 686 && y >= 22 && y <= 28 { return TEXT_PRIMARY; }
            if x >= 661 && x <= 680 && y >= 21 && y <= 29 { return SUCCESS_EMERALD; }
        }
        return BG_DARK;
    }

    // 2. Sovereign Enclave Pill (y: 70..106, x: 48..440)
    if y >= 70 && y < 106 && x >= 48 && x < 440 {
        if y == 70 || y == 105 || x == 48 || x == 439 {
            return ACCENT_CYAN;
        }
        if y >= 80 && y < 96 {
            if let Some(c) = test_text(x, y, 68, 80, 2, b"INTEROS 2.0  SOVEREIGN", TEXT_PRIMARY) {
                return c;
            }
        }
        return 0xFF081826;
    }

    // 3. Distinct Architectural Large Clock Display (y: 125..275)
    if y >= 125 && y < 275 && x >= 48 && x < 672 {
        if x >= 48 && x <= 53 && y >= 135 && y <= 235 {
            return ACCENT_CYAN;
        }
        if y >= 135 && y < 183 {
            if let Some(c) = test_text(x, y, 72, 135, 6, CLOCK_TIME, TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 195 && y < 211 {
            if let Some(c) = test_text(x, y, 72, 195, 2, b"Wednesday, 24 Sep", 0xFF94A3B8) {
                return c;
            }
            if let Some(c) = test_text(x, y, 380, 195, 2, b"24 C  Sunny", 0xFFF59E0B) {
                return c;
            }
        }
        if y >= 220 && y < 228 {
            if let Some(c) = test_text(x, y, 72, 220, 1, b"DM-VERITY VERIFIED  ZERO-TRUST MICROKERNEL", 0xFF10B981) {
                return c;
            }
        }
    }

    // 4. Modular Lockscreen Widgets (y: 285..580)
    // Widget 1: System Vitals Capsule (x: 48..345, y: 285..465)
    if x >= 48 && x < 345 && y >= 285 && y < 465 {
        if y == 285 || y == 464 || x == 48 || x == 344 {
            return BORDER_SUBTLE;
        }
        if y >= 305 && y < 321 {
            if let Some(c) = test_text(x, y, 70, 305, 2, b"Core Vitals", ACCENT_CYAN) {
                return c;
            }
        }
        if y >= 335 && y < 343 {
            if let Some(c) = test_text(x, y, 70, 335, 1, b"CPU  2.4 GHz [4 Cores]", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 355 && y < 363 {
            if let Some(c) = test_text(x, y, 70, 355, 1, b"RAM  1024 MB [38% Used]", TEXT_MUTED) {
                return c;
            }
        }
        // 4-segment kinetic gauge bar (x: 70..320, y: 375..385)
        if y >= 375 && y <= 385 && x >= 70 && x <= 320 {
            let seg = (x - 70) / 64;
            let within = (x - 70) % 64;
            if within <= 56 {
                if seg < 2 {
                    return 0xFF10B981;
                } else if seg == 2 {
                    return 0xFF31C4FF;
                } else {
                    return 0xFF1E293B;
                }
            }
        }
        if y >= 405 && y < 413 {
            if let Some(c) = test_text(x, y, 70, 405, 1, b"Security: Hardware Root OK", 0xFF10B981) {
                return c;
            }
        }
        if y >= 425 && y < 433 {
            if let Some(c) = test_text(x, y, 70, 425, 1, b"IPC Latency: 1.2 us", TEXT_MUTED) {
                return c;
            }
        }
        return 0xEE0A1220;
    }

    // Widget 2: Ambient Horizon & Climate (x: 375..672, y: 285..465)
    if x >= 375 && x < 672 && y >= 285 && y < 465 {
        if y == 285 || y == 464 || x == 375 || x == 671 {
            return BORDER_SUBTLE;
        }
        if y >= 305 && y < 321 {
            if let Some(c) = test_text(x, y, 395, 305, 2, b"Atmosphere", 0xFFF59E0B) {
                return c;
            }
        }
        if y >= 335 && y < 343 {
            if let Some(c) = test_text(x, y, 395, 335, 1, b"Weather: 24 C  Sunny", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 355 && y < 363 {
            if let Some(c) = test_text(x, y, 395, 355, 1, b"Geomagnetic: Kp 1 Quiet", 0xFF10B981) {
                return c;
            }
        }
        // Solar elevation arc indicator (x: 395..645, y: 375..385)
        if y >= 375 && y <= 385 && x >= 395 && x <= 645 {
            let prog = (x - 395) * 100 / 250;
            if prog <= 65 {
                return 0xFFF59E0B;
            } else {
                return 0xFF1E293B;
            }
        }
        if y >= 405 && y < 413 {
            if let Some(c) = test_text(x, y, 395, 405, 1, b"Solar Zenith: 42 deg", TEXT_MUTED) {
                return c;
            }
        }
        if y >= 425 && y < 433 {
            if let Some(c) = test_text(x, y, 395, 425, 1, b"Sunset in: 4h 18m", TEXT_MUTED) {
                return c;
            }
        }
        return 0xEE0A1220;
    }

    // Widget 3: Kinetic Agenda Capsule (x: 48..672, y: 480..570)
    if x >= 48 && x < 672 && y >= 480 && y < 570 {
        if y == 480 || y == 569 || x == 48 || x == 671 {
            return BORDER_SUBTLE;
        }
        if y >= 500 && y < 516 {
            if let Some(c) = test_text(x, y, 70, 500, 2, b"Next: Capability Engine Review", TEXT_PRIMARY) {
                return c;
            }
        }
        if y >= 530 && y < 538 {
            if let Some(c) = test_text(x, y, 70, 530, 1, b"10:30 AM  Conf Room 4B  3 Tasks In Sandbox", TEXT_MUTED) {
                return c;
            }
        }
        return 0xEE0A1220;
    }

    // 5. Interactive Wallpaper Switcher Card (x: 80..640, y: 1120..1190)
    if x >= 80 && x < 640 && y >= 1120 && y < 1190 {
        if y == 1120 || y == 1189 || x == 80 || x == 639 {
            return ACCENT_CYAN;
        }
        if y >= 1135 && y < 1151 {
            let label = match state.wallpaper_id {
                0 => &b"THEME: QUANTUM HORIZON [TAP TO CYCLE]"[..],
                1 => &b"THEME: SOLAR CORONA   [TAP TO CYCLE]"[..],
                2 => &b"THEME: CYBER MATRIX   [TAP TO CYCLE]"[..],
                _ => &b"THEME: LIVE PLASMA    [TAP TO CYCLE]"[..],
            };
            if let Some(c) = test_text(x, y, 105, 1135, 2, label, ACCENT_CYAN) {
                return c;
            }
        }
        if y >= 1160 && y < 1168 {
            if let Some(c) = test_text(x, y, 160, 1160, 1, b"Procedural bare-metal graphics engine", TEXT_MUTED) {
                return c;
            }
        }
        return 0xEE0A1628;
    }

    // 6. Flashlight Circular Button at (72, 1330, r: 35)
    let f_dx = (x as i32) - 72;
    let f_dy = (y as i32) - 1330;
    let f_d2 = f_dx * f_dx + f_dy * f_dy;
    if f_d2 <= 1225 {
        if f_d2 >= 1156 { return BORDER_SUBTLE; }
        if y >= 1324 && y < 1332 {
            let label = if state.flashlight_on { &b"ON"[..] } else { &b"LGT"[..] };
            let fg = if state.flashlight_on { TEXT_DARK } else { TEXT_PRIMARY };
            let ox = if state.flashlight_on { 64 } else { 60 };
            if let Some(c) = test_text(x, y, ox, 1324, 1, label, fg) {
                return c;
            }
        }
        return if state.flashlight_on { ACCENT_AMBER } else { 0xEE142032 };
    }

    // Camera Circular Button at (648, 1330, r: 35)
    let c_dx = (x as i32) - 648;
    let c_dy = (y as i32) - 1330;
    let c_d2 = c_dx * c_dx + c_dy * c_dy;
    if c_d2 <= 1225 {
        if c_d2 >= 1156 { return BORDER_SUBTLE; }
        if y >= 1324 && y < 1332 {
            if let Some(c) = test_text(x, y, 636, 1324, 1, b"CAM", TEXT_PRIMARY) {
                return c;
            }
        }
        return 0xEE142032;
    }

    // 7. Kinetic Unlock Affordance (x: 200..520, y: 1330..1390)
    if x >= 200 && x < 520 && y >= 1330 && y < 1390 {
        if y >= 1340 && y < 1356 {
            if let Some(c) = test_text(x, y, 310, 1340, 2, b"^ ^ ^", ACCENT_CYAN) {
                return c;
            }
        }
        if y >= 1365 && y < 1381 {
            if let Some(c) = test_text(x, y, 240, 1365, 2, b"SWIPE UP TO UNLOCK", TEXT_PRIMARY) {
                return c;
            }
        }
    }

    // Home Gesture Indicator Pill at (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    // 8. Foundational Wallpaper Backdrop
    get_wallpaper_pixel(x, y, state)
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
            if let Some(c) = test_text(x, y, 48, 16, 2, CLOCK_TIME, TEXT_PRIMARY) {
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
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 {
            return BORDER_SUBTLE;
        }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, CLOCK_TIME, TEXT_PRIMARY) {
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

    // 2. Header (y: 48..110)
    if y >= 48 && y < 110 {
        if y >= 64 && y < 88 {
            if let Some(c) = test_text(x, y, 48, 64, 3, b"Events", TEXT_PRIMARY) {
                return c;
            }
        }
        // Filter icon at (644, 76)
        let dx = (x as i32) - 644;
        let dy = (y as i32) - 76;
        if dx.abs() <= 12 && (dy == -8 || dy == -2 || dy == 4) {
            return TEXT_MUTED;
        }
        if y >= 106 {
            return BORDER_SUBTLE;
        }
        return BG_DARK;
    }

    // 3. Category Tabs (y: 115..160)
    // All, Messages, System, Apps
    if y >= 115 && y < 160 {
        let tabs = [
            (40usize, 170usize, &b"All"[..], 0usize),
            (185, 315, &b"Messages"[..], 1),
            (330, 460, &b"System"[..], 2),
            (475, 605, &b"Apps"[..], 3),
        ];

        for (x0, x1, label, idx) in tabs {
            if x >= x0 && x < x1 {
                let is_sel = state.notif_tab == idx;
                if y == 115 || y == 159 || x == x0 || x == x1 - 1 {
                    return if is_sel { ACCENT_SKY } else { BORDER_SUBTLE };
                }
                if y >= 131 && y < 147 {
                    let ox = x0 + (x1 - x0).saturating_sub(label.len() * 16) / 2;
                    let fg = if is_sel { TEXT_DARK } else { TEXT_MUTED };
                    if let Some(c) = test_text(x, y, ox, 131, 2, label, fg) {
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

    // 4. Section: Today (y: 170..620)
    if y >= 170 && y < 190 && x >= 48 && x < 200 {
        if let Some(c) = test_text(x, y, 48, 170, 2, b"Today", TEXT_PRIMARY) {
            return c;
        }
    }

    let today_items: [(usize, usize, &[u8], &[u8], &[u8], u32); 4] = [
        (
            195,
            290,
            b"New message",
            b"Rama . 2 min ago",
            b"Got it, thanks!",
            SUCCESS_EMERALD,
        ),
        (
            300,
            395,
            b"Meeting reminder",
            b"Team sync (11:00) . 1 hr ago",
            b"Join voice room",
            ACCENT_SKY,
        ),
        (
            405,
            500,
            b"Download complete",
            b"interos-sdk.zip . 2 hr ago",
            b"Saved to /downloads",
            0xFF60A5FA,
        ),
        (
            510,
            605,
            b"Photo saved",
            b"IMG_20250924.jpg . 3 hr ago",
            b"Saved to Gallery",
            ACCENT_AMBER,
        ),
    ];

    for (y0, y1, title, sub, preview, color) in today_items {
        if y >= y0 && y < y1 && x >= 40 && x < 680 {
            if x < 48 {
                return color;
            }
            if y == y0 || y == y1 - 1 || x == 40 || x == 679 {
                return BORDER_SUBTLE;
            }
            if y >= y0 + 16 && y < y0 + 32 {
                if let Some(c) = test_text(x, y, 68, y0 + 16, 2, title, TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= y0 + 40 && y < y0 + 48 {
                if let Some(c) = test_text(x, y, 68, y0 + 40, 1, sub, TEXT_MUTED) {
                    return c;
                }
            }
            if y >= y0 + 58 && y < y0 + 66 {
                if let Some(c) = test_text(x, y, 68, y0 + 58, 1, preview, color) {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }
    }

    // 5. Section: Yesterday (y: 620..870)
    if y >= 625 && y < 645 && x >= 48 && x < 200 {
        if let Some(c) = test_text(x, y, 48, 625, 2, b"Yesterday", TEXT_PRIMARY) {
            return c;
        }
    }

    let yest_items: [(usize, usize, &[u8], &[u8], &[u8], u32); 2] = [
        (
            655,
            745,
            b"App updated",
            b"Browser . 10:20 PM",
            b"Version 1.4.0 active",
            0xFFA78BFA,
        ),
        (
            755,
            845,
            b"USB device connected",
            b"External storage . 9:15 PM",
            b"Mounted at /mnt/usb0",
            0xFF38BDF8,
        ),
    ];

    for (y0, y1, title, sub, preview, color) in yest_items {
        if y >= y0 && y < y1 && x >= 40 && x < 680 {
            if x < 48 {
                return color;
            }
            if y == y0 || y == y1 - 1 || x == 40 || x == 679 {
                return BORDER_SUBTLE;
            }
            if y >= y0 + 16 && y < y0 + 32 {
                if let Some(c) = test_text(x, y, 68, y0 + 16, 2, title, TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= y0 + 40 && y < y0 + 48 {
                if let Some(c) = test_text(x, y, 68, y0 + 40, 1, sub, TEXT_MUTED) {
                    return c;
                }
            }
            if y >= y0 + 58 && y < y0 + 66 {
                if let Some(c) = test_text(x, y, 68, y0 + 58, 1, preview, color) {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }
    }

    // 6. "Clear All" Pill (y: 880..935, x: 260..460)
    if y >= 880 && y < 935 && x >= 260 && x < 460 {
        if y == 880 || y == 934 || x == 260 || x == 459 {
            return BORDER_SUBTLE;
        }
        if y >= 900 && y < 916 {
            if let Some(c) = test_text(x, y, 316, 900, 2, b"Clear All", TEXT_MUTED) {
                return c;
            }
        }
        return SURFACE_RAISED;
    }

    // 7. Home Gesture Pill (x: 260..460, y: 1418..1424)
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
            if let Some(c) = test_text(
                x,
                y,
                65,
                725,
                2,
                b"Sovereign Hardware Telemetry",
                TEXT_PRIMARY,
            ) {
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
            if let Some(c) = test_text(x, y, 65, 885, 2, b"Wallpapers & Procedural Themes", ACCENT_CYAN) {
                return c;
            }
        }
        if y >= 910 && y < 918 {
            if let Some(c) = test_text(x, y, 65, 910, 1, b"Tap any theme to apply across system", TEXT_MUTED) {
                return c;
            }
        }

        // 4 Theme Selection Buttons:
        // [1. Quantum] (x: 65..335, y: 935..1005)
        if x >= 65 && x < 335 && y >= 935 && y < 1005 {
            let is_sel = state.wallpaper_id == 0;
            if y == 935 || y == 1004 || x == 65 || x == 334 {
                return if is_sel { ACCENT_CYAN } else { BORDER_SUBTLE };
            }
            if y >= 955 && y < 971 {
                let col = if is_sel { ACCENT_CYAN } else { TEXT_PRIMARY };
                if let Some(c) = test_text(x, y, 85, 955, 2, b"1. Quantum", col) { return c; }
            }
            if y >= 980 && y < 988 {
                if let Some(c) = test_text(x, y, 85, 980, 1, b"Cosmic Orbital Arcs", TEXT_MUTED) { return c; }
            }
            return if is_sel { 0xFF0D253A } else { SURFACE_DARK };
        }

        // [2. Solar Corona] (x: 375..645, y: 935..1005)
        if x >= 375 && x < 645 && y >= 935 && y < 1005 {
            let is_sel = state.wallpaper_id == 1;
            if y == 935 || y == 1004 || x == 375 || x == 644 {
                return if is_sel { ACCENT_AMBER } else { BORDER_SUBTLE };
            }
            if y >= 955 && y < 971 {
                let col = if is_sel { ACCENT_AMBER } else { TEXT_PRIMARY };
                if let Some(c) = test_text(x, y, 395, 955, 2, b"2. Solar Corona", col) { return c; }
            }
            if y >= 980 && y < 988 {
                if let Some(c) = test_text(x, y, 395, 980, 1, b"Amber Flare Radiance", TEXT_MUTED) { return c; }
            }
            return if is_sel { 0xFF28180A } else { SURFACE_DARK };
        }

        // [3. Cyber Matrix] (x: 65..335, y: 1020..1090)
        if x >= 65 && x < 335 && y >= 1020 && y < 1090 {
            let is_sel = state.wallpaper_id == 2;
            if y == 1020 || y == 1089 || x == 65 || x == 334 {
                return if is_sel { SUCCESS_EMERALD } else { BORDER_SUBTLE };
            }
            if y >= 1040 && y < 1056 {
                let col = if is_sel { SUCCESS_EMERALD } else { TEXT_PRIMARY };
                if let Some(c) = test_text(x, y, 85, 1040, 2, b"3. Cyber Matrix", col) { return c; }
            }
            if y >= 1065 && y < 1073 {
                if let Some(c) = test_text(x, y, 85, 1065, 1, b"Tactical Bus Grid", TEXT_MUTED) { return c; }
            }
            return if is_sel { 0xFF0A2416 } else { SURFACE_DARK };
        }

        // [4. Live Plasma] (x: 375..645, y: 1020..1090)
        if x >= 375 && x < 645 && y >= 1020 && y < 1090 {
            let is_sel = state.wallpaper_id == 3;
            if y == 1020 || y == 1089 || x == 375 || x == 644 {
                return if is_sel { 0xFF8A2BE2 } else { BORDER_SUBTLE };
            }
            if y >= 1040 && y < 1056 {
                let col = if is_sel { 0xFFB388FF } else { TEXT_PRIMARY };
                if let Some(c) = test_text(x, y, 395, 1040, 2, b"4. Live Plasma", col) { return c; }
            }
            if y >= 1065 && y < 1073 {
                if let Some(c) = test_text(x, y, 395, 1065, 1, b"Kinetic Animated Wave", TEXT_MUTED) { return c; }
            }
            return if is_sel { 0xFF220D38 } else { SURFACE_DARK };
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
                "[LAUNCHER] render_full complete: 10-app home screen canvas initialized.",
            )
            .ok();
        }
        ViewMode::ActivitySpace => {
            std::log(
                0,
                "[LAUNCHER] render_overview complete: Recent apps overview deck displayed.",
            )
            .ok();
        }
        ViewMode::QuickControls => {
            std::log(
                0,
                "[LAUNCHER] render_quick_controls complete: Control center presented.",
            )
            .ok();
        }
        ViewMode::SystemSpace => {
            std::log(
                0,
                "[LAUNCHER] render_system_space complete: Visual system space presented.",
            )
            .ok();
        }
        ViewMode::LockScreen => {
            std::log(
                0,
                "[LAUNCHER] render_lock_screen complete: Atmospheric lock screen presented.",
            )
            .ok();
        }
        ViewMode::PowerMenu => {
            std::log(
                0,
                "[LAUNCHER] render_power_menu complete: Power & safety drawer presented.",
            )
            .ok();
        }
        ViewMode::AppDrawer => {
            std::log(
                0,
                "[LAUNCHER] render_app_drawer complete: 16-app grid presented.",
            )
            .ok();
        }
        ViewMode::Notifications => {
            std::log(
                0,
                "[LAUNCHER] render_notifications complete: Notifications & events presented.",
            )
            .ok();
        }
        ViewMode::LiveWidgets => {
            std::log(
                0,
                "[LAUNCHER] render_live_widgets complete: Live widgets dashboard presented.",
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

            if state.active_child.is_none()
                && (events & std::EVENT_TIMER_TICK) != 0
            {
                refresh_ticks += 1;
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
                                if input.dx.abs() >= 60 {
                                    let y = input.y as usize;
                                    if (200..480).contains(&y) {
                                        let _ = std::app_stop(state.recent_task_ids[0]);
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
                                        let _ = std::app_stop(state.recent_task_ids[1]);
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
                                        let _ = std::app_stop(state.recent_task_ids[2]);
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
                                    let Some(_target) = state.recent_id(0) else {
                                        break;
                                    };
                                    if let Ok(()) = std::app_focus(state.recent_task_ids[0]) {
                                        state.active_child = Some(state.recent_task_ids[0]);
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
                                    let Some(_target) = state.recent_id(1) else {
                                        break;
                                    };
                                    if let Ok(()) = std::app_focus(state.recent_task_ids[1]) {
                                        state.active_child = Some(state.recent_task_ids[1]);
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
                                    let Some(_target) = state.recent_id(2) else {
                                        break;
                                    };
                                    if let Ok(()) = std::app_focus(state.recent_task_ids[2]) {
                                        state.active_child = Some(state.recent_task_ids[2]);
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
                                if (130..260).contains(&y) {
                                    if (40..345).contains(&x) {
                                        state.wifi_on = !state.wifi_on;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        break;
                                    } else if (375..680).contains(&x) {
                                        state.bt_on = !state.bt_on;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        break;
                                    }
                                }
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
                                state.push_recent("org.interos.settings");
                                if let Ok(task_id) = std::app_launch("org.interos.settings") {
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
                            // 1. Vertical swipe up to unlock
                            if input.kind == 4 && input.dy <= -25 {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Device unlocked via swipe-up gesture.")
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

                                // Wallpaper cycle pill (x: 80..640, y: 1120..1195)
                                if (1120..=1195).contains(&y) && (80..640).contains(&x) {
                                    state.wallpaper_id = (state.wallpaper_id + 1) % 4;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Wallpaper cycled from lock screen.")
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

                                // Flashlight circle button at (72, 1330, r: 35)
                                let f_dx = (x as i32) - 72;
                                let f_dy = (y as i32) - 1330;
                                if f_dx * f_dx + f_dy * f_dy <= 1225 {
                                    state.flashlight_on = !state.flashlight_on;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Flashlight toggled on lock screen.")
                                        .ok();
                                    break;
                                }

                                // Camera circle button at (648, 1330, r: 35)
                                let c_dx = (x as i32) - 648;
                                let c_dy = (y as i32) - 1330;
                                if c_dx * c_dx + c_dy * c_dy <= 1225 {
                                    state.push_recent("org.interos.camera");
                                    if let Ok(task_id) = std::app_launch("org.interos.camera") {
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

                                // Tap anywhere in middle or bottom to unlock
                                if y >= 250 {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Device unlocked via tap.").ok();
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
                                    state.push_recent("org.interos.dialer");
                                    if let Ok(task_id) = std::app_launch("org.interos.dialer") {
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
                            // 1. Gesture Navigation
                            // Swipe down or swipe right returns to HomeScreen
                            if (input.kind == 4 && (input.dy >= 60 || input.dx >= 60))
                                || (input.kind == 1 && input.y >= 1400)
                            {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Returned to Home from App Drawer.").ok();
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

                                // Filter tabs (y: 135..185)
                                if (135..185).contains(&y) {
                                    if (40..170).contains(&x) {
                                        state.app_drawer_tab = 0;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        break;
                                    } else if (185..315).contains(&x) {
                                        state.app_drawer_tab = 1;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        break;
                                    } else if (330..460).contains(&x) {
                                        state.app_drawer_tab = 2;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        break;
                                    } else if (475..605).contains(&x) {
                                        state.app_drawer_tab = 3;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] App drawer category tab switched.")
                                            .ok();
                                        break;
                                    }
                                }

                                // Page controls for registry-backed app catalog.
                                if (820..880).contains(&y) && (300..420).contains(&x) {
                                    let max_page = state.app_count.saturating_add(15) / 16;
                                    if max_page > 0 {
                                        if x < 360 {
                                            state.app_drawer_page =
                                                state.app_drawer_page.saturating_sub(1);
                                        } else if state.app_drawer_page + 1 < max_page {
                                            state.app_drawer_page += 1;
                                        }
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        break;
                                    }
                                }

                                // Launch application from the registry-backed grid.
                                let target_desc = resolve_app_in_drawer(&state, x, y).map(|s| {
                                    let mut buf = [0u8; 32];
                                    let len = s.len().min(32);
                                    buf[..len].copy_from_slice(&s.as_bytes()[..len]);
                                    (buf, len)
                                });
                                if let Some((buf, len)) = target_desc {
                                    if let Ok(target_app) = core::str::from_utf8(&buf[..len]) {
                                        state.push_recent(target_app);
                                        match std::app_launch(target_app) {
                                            Ok(task_id) => {
                                                state.active_child = Some(task_id);
                                                state.current_view = ViewMode::HomeScreen;
                                                std::log(0, "[LAUNCHER] App launched from drawer.")
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
                                            Err(_) => {
                                                std::log(0, "[LAUNCHER] Application launch rejected by kernel.").ok();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        ViewMode::Notifications => {
                            // 1. Gesture Navigation
                            // Swipe up or swipe right returns to HomeScreen
                            if (input.kind == 4 && (input.dy <= -60 || input.dx >= 60))
                                || (input.kind == 1 && input.y >= 1400)
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
                            // 2. Touch Tap
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;

                                // Filter tabs (y: 135..185)
                                if (135..185).contains(&y) {
                                    if (40..170).contains(&x) {
                                        state.notif_tab = 0;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        break;
                                    } else if (185..315).contains(&x) {
                                        state.notif_tab = 1;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        break;
                                    } else if (330..460).contains(&x) {
                                        state.notif_tab = 2;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        break;
                                    } else if (475..605).contains(&x) {
                                        state.notif_tab = 3;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        break;
                                    }
                                }

                                // Clear all button or header icon
                                let dx_c = (x as i32) - 644;
                                let dy_c = (y as i32) - 76;
                                if (y >= 700 && y < 755 && x >= 260 && x < 460)
                                    || (dx_c * dx_c + dy_c * dy_c <= 324)
                                {
                                    let mut drain = std::InputEvent {
                                        kind: 0,
                                        x: 0,
                                        y: 0,
                                        dx: 0,
                                        dy: 0,
                                        scale_milli: 1000,
                                    };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Notifications cleared.").ok();
                                    break;
                                }

                                // Card 0: Messages
                                if (205..310).contains(&y) && (40..680).contains(&x) {
                                    state.push_recent("org.interos.messages");
                                    if let Ok(task_id) = std::app_launch("org.interos.messages") {
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
                                // Card 1: Messages
                                if (325..430).contains(&y) && (40..680).contains(&x) {
                                    state.push_recent("org.interos.messages");
                                    if let Ok(task_id) = std::app_launch("org.interos.messages") {
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
                                // Card 2: Calendar
                                if (445..550).contains(&y) && (40..680).contains(&x) {
                                    state.push_recent("org.interos.calendar");
                                    if let Ok(task_id) = std::app_launch("org.interos.calendar") {
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
                                // Card 3: Settings
                                if (565..670).contains(&y) && (40..680).contains(&x) {
                                    state.push_recent("org.interos.settings");
                                    if let Ok(task_id) = std::app_launch("org.interos.settings") {
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
                                    state.push_recent("org.interos.sensor_dashboard");
                                    if let Ok(task_id) =
                                        std::app_launch("org.interos.sensor_dashboard")
                                    {
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
                                    state.push_recent("org.interos.calculator");
                                    if let Ok(task_id) = std::app_launch("org.interos.calculator") {
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
                                    state.push_recent("org.interos.music");
                                    if let Ok(task_id) = std::app_launch("org.interos.music") {
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
                                    state.push_recent("org.interos.calendar");
                                    if let Ok(task_id) = std::app_launch("org.interos.calendar") {
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
                                    state.push_recent("org.interos.health");
                                    if let Ok(task_id) = std::app_launch("org.interos.health") {
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

                                // Personalization & Wallpaper Switcher Theme Buttons:
                                // [1. Quantum] (65..335, 935..1005)
                                if (935..1005).contains(&y) && (65..335).contains(&x) {
                                    state.wallpaper_id = 0;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Wallpaper theme changed to Quantum Horizon.").ok();
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
                                // [2. Solar Corona] (375..645, 935..1005)
                                if (935..1005).contains(&y) && (375..645).contains(&x) {
                                    state.wallpaper_id = 1;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Wallpaper theme changed to Solar Corona.").ok();
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
                                // [3. Cyber Matrix] (65..335, 1020..1090)
                                if (1020..1090).contains(&y) && (65..335).contains(&x) {
                                    state.wallpaper_id = 2;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Wallpaper theme changed to Cyber Matrix.").ok();
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
                                // [4. Live Plasma] (375..645, 1020..1090)
                                if (1020..1090).contains(&y) && (375..645).contains(&x) {
                                    state.wallpaper_id = 3;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Wallpaper theme changed to Live Plasma.").ok();
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
                            if input.kind == 4 && input.dy <= -60 {
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
                            if input.kind == 4 && input.dy >= 60 && input.y <= 120 {
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
                            if input.kind == 4 && input.dx <= -60 && input.x >= 650 {
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
                            if input.kind == 4 && input.dx <= -60 && input.x < 650 {
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
                                    std::log(0, "[LAUNCHER] Live Widgets opened via top weather card tap.").ok();
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

                                // Tapping Today card chevron '>' opens Notifications
                                if (240..300).contains(&y) && x >= 600 {
                                    state.current_view = ViewMode::Notifications;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(
                                        0,
                                        "[LAUNCHER] Notifications opened from Today card header.",
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

                                // Tapping Today card radio buttons toggles task state
                                if (65..=95).contains(&x) {
                                    if (305..=330).contains(&y) {
                                        state.task_done[0] = !state.task_done[0];
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Contextual task 0 toggled.").ok();
                                        break;
                                    } else if (355..=380).contains(&y) {
                                        state.task_done[1] = !state.task_done[1];
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Contextual task 1 toggled.").ok();
                                        break;
                                    } else if (405..=430).contains(&y) {
                                        state.task_done[2] = !state.task_done[2];
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Contextual task 2 toggled.").ok();
                                        break;
                                    }
                                }

                                // Universal search bar tap: opens System Space
                                if (1290..1365).contains(&y) && (40..680).contains(&x) {
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

                                // Tapping dock 4th item (490..645, 1120..1400) opens App Drawer
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
