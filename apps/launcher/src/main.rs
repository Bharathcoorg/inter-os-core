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
const DANGER_CRIMSON: u32 = 0xFFFF6B6B;

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
fn test_text(x: usize, y: usize, x0: usize, y0: usize, scale: usize, text: &[u8], color: u32) -> Option<u32> {
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
    AiCompanion,
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
    pub ai_listening: bool,
    pub app_drawer_tab: usize,
    pub notif_tab: usize,
    pub recent_apps: [&'static str; 3],
    pub active_child: Option<u64>,
}

impl LauncherState {
    pub const fn new() -> Self {
        Self {
            current_view: ViewMode::HomeScreen,
            task_done: [false, false, false],
            wifi_on: true,
            bt_on: true,
            dark_mode: true,
            media_playing: true,
            flashlight_on: false,
            ai_listening: false,
            app_drawer_tab: 0,
            notif_tab: 0,
            recent_apps: [
                "org.interos.calculator",
                "org.interos.settings",
                "org.interos.terminal",
            ],
            active_child: None,
        }
    }

    pub fn push_recent(&mut self, app: &'static str) {
        if self.recent_apps[0] == app { return; }
        if self.recent_apps[1] == app {
            self.recent_apps.swap(0, 1);
            return;
        }
        if self.recent_apps[2] == app {
            let temp = self.recent_apps[2];
            self.recent_apps[2] = self.recent_apps[1];
            self.recent_apps[1] = self.recent_apps[0];
            self.recent_apps[0] = temp;
            return;
        }
        self.recent_apps[2] = self.recent_apps[1];
        self.recent_apps[1] = self.recent_apps[0];
        self.recent_apps[0] = app;
    }

    pub fn dismiss_recent(&mut self, index: usize) {
        if index == 0 {
            self.recent_apps[0] = self.recent_apps[1];
            self.recent_apps[1] = self.recent_apps[2];
            self.recent_apps[2] = "org.interos.calculator";
        } else if index == 1 {
            self.recent_apps[1] = self.recent_apps[2];
            self.recent_apps[2] = "org.interos.settings";
        } else if index == 2 {
            self.recent_apps[2] = "org.interos.terminal";
        }
    }
}

// -----------------------------------------------------------------------------
// Pixel-Accurate Screen Renderers
// -----------------------------------------------------------------------------

#[inline(never)]
fn get_home_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 { return BORDER_SUBTLE; }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, b"--:--", TEXT_PRIMARY) { return c; }
        }
        // 4-Bar Cellular Signal
        if x >= 590 && x <= 608 && y >= 16 && y <= 32 {
            if x <= 593 && y >= 28 { return TEXT_PRIMARY; }
            if x >= 595 && x <= 598 && y >= 24 { return TEXT_PRIMARY; }
            if x >= 600 && x <= 603 && y >= 20 { return TEXT_PRIMARY; }
            if x >= 605 && x <= 608 && y >= 16 { return TEXT_PRIMARY; }
        }
        // Wi-Fi 3-arcs
        if x >= 626 && x <= 644 && y >= 18 && y <= 32 {
            let dx = (x as i32) - 635;
            let dy = (y as i32) - 30;
            let d2 = dx * dx + dy * dy;
            if (d2 >= 100 && d2 <= 144 && dy <= -2) || (d2 >= 36 && d2 <= 64 && dy <= -1) || d2 <= 9 {
                return TEXT_PRIMARY;
            }
        }
        // Battery outline only until real telemetry is available
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 { return TEXT_PRIMARY; }
            if x >= 685 && x <= 686 && y >= 22 && y <= 28 { return TEXT_PRIMARY; }
            // Synthetic battery fill removed; hardware telemetry must supply the level.
        }
        return BG_DARK;
    }

    // 2. Scenic Mountain Wallpaper & Header (y: 48..235)
    if y < 235 {
        // Date & Tabular Clock
        if y >= 64 && y < 80 {
            if let Some(c) = test_text(x, y, 48, 64, 2, b"Date unavailable", TEXT_MUTED) { return c; }
        }
        if y >= 90 && y < 122 {
            if let Some(c) = test_text(x, y, 48, 90, 4, b"--:--", TEXT_PRIMARY) { return c; }
        }

        // Weather Widget Right
        if x >= 460 && x <= 680 && y >= 80 && y < 135 {
            let dx = (x as i32) - 490;
            let dy = (y as i32) - 96;
            if dx * dx + dy * dy <= 81 { return ACCENT_AMBER; }
            if y >= 84 && y < 92 {
                if let Some(c) = test_text(x, y, 520, 84, 1, b"Location unavailable", TEXT_MUTED) { return c; }
            }
            if y >= 98 && y < 114 {
                if let Some(c) = test_text(x, y, 520, 98, 2, b"24*", TEXT_PRIMARY) { return c; }
            }
            if y >= 118 && y < 126 {
                if let Some(c) = test_text(x, y, 520, 118, 1, b"Cloudy", TEXT_MUTED) { return c; }
            }
        }

        // Contextual Greeting
        if y >= 145 && y < 169 {
            if let Some(c) = test_text(x, y, 48, 145, 3, b"Good morning,", TEXT_PRIMARY) { return c; }
        }
        if y >= 175 && y < 199 {
            if let Some(c) = test_text(x, y, 48, 175, 3, b"Bharath", TEXT_PRIMARY) { return c; }
        }
        if y >= 208 && y < 224 {
            if let Some(c) = test_text(x, y, 48, 208, 2, b"Let's make a productive day", TEXT_MUTED) { return c; }
        }

        // Mountain gradient background
        let ridge = 210 + (((x * 13) % 29) as usize);
        if y >= ridge { return 0xFF141D2D; }
        return BG_DARK;
    }

    // 3. Interactive Contextual "Today" Card (x: 40..680, y: 240..470)
    if x >= 40 && x < 680 && y >= 240 && y < 470 {
        if y >= 466 { return 0xFFE2E8F0; }

        if y >= 258 && y < 274 {
            if let Some(c) = test_text(x, y, 68, 258, 2, b"Today", TEXT_DARK) { return c; }
            if let Some(c) = test_text(x, y, 640, 258, 2, b">", 0xFF64748B) { return c; }
        }

        // Task 1: "Finish InterOS build"
        if y >= 305 && y < 330 {
            let dx1 = (x as i32) - 78;
            let dy1 = (y as i32) - 318;
            let d1_sq = dx1 * dx1 + dy1 * dy1;
            if d1_sq <= 64 {
                if state.task_done[0] { return ACCENT_SKY; }
                if d1_sq >= 36 { return 0xFF64748B; }
            }
            if y >= 310 && y < 326 {
                if let Some(c) = test_text(x, y, 100, 310, 2, b"Finish InterOS build", TEXT_DARK) { return c; }
            }
        }

        // Task 2: "2 messages need reply"
        if y >= 355 && y < 380 {
            let dx2 = (x as i32) - 78;
            let dy2 = (y as i32) - 368;
            let d2_sq = dx2 * dx2 + dy2 * dy2;
            if d2_sq <= 64 {
                if state.task_done[1] { return ACCENT_SKY; }
                if d2_sq >= 36 { return 0xFF64748B; }
            }
            if y >= 360 && y < 376 {
                if let Some(c) = test_text(x, y, 100, 360, 2, b"2 messages need reply", TEXT_DARK) { return c; }
            }
        }

        // Task 3: "Call at 11:00 AM"
        if y >= 405 && y < 430 {
            let dx3 = (x as i32) - 78;
            let dy3 = (y as i32) - 418;
            let d3_sq = dx3 * dx3 + dy3 * dy3;
            if d3_sq <= 64 {
                if state.task_done[2] { return ACCENT_SKY; }
                if d3_sq >= 36 { return 0xFF64748B; }
            }
            if y >= 410 && y < 426 {
                if let Some(c) = test_text(x, y, 100, 410, 2, b"Call at 11:00 AM", TEXT_DARK) { return c; }
            }
        }

        return SURFACE_LIGHT;
    }

    // 4. Primary Applications Grid (Rows 0 to 4: y: 480..1110)
    // Row 1 (y: 490..590)
    if y >= 490 && y < 590 {
        if x >= 56 && x < 306 {
            if y >= 522 && y < 538 {
                if let Some(c) = test_text(x, y, 80, 522, 2, b"Calculator", TEXT_PRIMARY) { return c; }
            }
            if y >= 505 && y < 545 && x >= 240 && x < 280 { return ACCENT_SKY; }
            if y >= 586 || x < 60 || x >= 302 { return BORDER_SUBTLE; }
            return SURFACE_RAISED;
        }
        if x >= 360 && x < 610 {
            if y >= 522 && y < 538 {
                if let Some(c) = test_text(x, y, 384, 522, 2, b"Vault", TEXT_PRIMARY) { return c; }
            }
            if y >= 505 && y < 545 && x >= 544 && x < 584 { return SUCCESS_EMERALD; }
            if y >= 586 || x < 364 || x >= 606 { return BORDER_SUBTLE; }
            return SURFACE_RAISED;
        }
        return BG_DARK;
    }
    // Row 2 (y: 620..760)
    if y >= 620 && y < 760 {
        if x >= 56 && x < 306 {
            if y >= 674 && y < 690 {
                if let Some(c) = test_text(x, y, 80, 674, 2, b"Camera", TEXT_PRIMARY) { return c; }
            }
            if y >= 670 && y < 734 && x >= 149 && x < 213 { return ACCENT_SKY; }
            if y >= 756 || x < 60 || x >= 302 { return BORDER_SUBTLE; }
            return SURFACE_RAISED;
        }
        if x >= 360 && x < 610 {
            if y >= 674 && y < 690 {
                if let Some(c) = test_text(x, y, 384, 674, 2, b"Dialer", TEXT_PRIMARY) { return c; }
            }
            if y >= 670 && y < 734 && x >= 453 && x < 517 { return SUCCESS_EMERALD; }
            if y >= 756 || x < 364 || x >= 606 { return BORDER_SUBTLE; }
            return SURFACE_RAISED;
        }
        return BG_DARK;
    }
    // Row 3 (y: 790..930)
    if y >= 790 && y < 930 {
        if x >= 56 && x < 306 {
            if y >= 844 && y < 860 {
                if let Some(c) = test_text(x, y, 80, 844, 2, b"Browser", TEXT_PRIMARY) { return c; }
            }
            if y >= 840 && y < 904 && x >= 149 && x < 213 { return TEXT_MUTED; }
            if y >= 926 || x < 60 || x >= 302 { return BORDER_SUBTLE; }
            return SURFACE_RAISED;
        }
        if x >= 360 && x < 610 {
            if y >= 844 && y < 860 {
                if let Some(c) = test_text(x, y, 384, 844, 2, b"Messages", TEXT_PRIMARY) { return c; }
            }
            if y >= 840 && y < 904 && x >= 453 && x < 517 { return ACCENT_AMBER; }
            if y >= 926 || x < 364 || x >= 606 { return BORDER_SUBTLE; }
            return SURFACE_RAISED;
        }
        return BG_DARK;
    }
    // Row 4 (y: 960..1100)
    if y >= 960 && y < 1100 {
        if x >= 56 && x < 306 {
            if y >= 1014 && y < 1030 {
                if let Some(c) = test_text(x, y, 80, 1014, 2, b"Calendar", TEXT_PRIMARY) { return c; }
            }
            if y >= 1010 && y < 1074 && x >= 149 && x < 213 { return ACCENT_SKY; }
            if y >= 1096 || x < 60 || x >= 302 { return BORDER_SUBTLE; }
            return SURFACE_RAISED;
        }
        if x >= 360 && x < 610 {
            if y >= 1014 && y < 1030 {
                if let Some(c) = test_text(x, y, 384, 1014, 2, b"Music", TEXT_PRIMARY) { return c; }
            }
            if y >= 1010 && y < 1074 && x >= 453 && x < 517 { return SUCCESS_EMERALD; }
            if y >= 1096 || x < 364 || x >= 606 { return BORDER_SUBTLE; }
            return SURFACE_RAISED;
        }
        return BG_DARK;
    }

    // 5. Tactile Quick Action Dock (x: 40..680, y: 1130..1260)
    if y >= 1130 && y < 1260 {
        let is_card0 = x >= 48 && x < 183;
        let is_card1 = x >= 201 && x < 336;
        let is_card2 = x >= 354 && x < 489;
        let is_card3 = x >= 507 && x < 642;

        if is_card0 || is_card1 || is_card2 || is_card3 {
            if y >= 1216 && y < 1232 {
                if is_card0 {
                    if let Some(c) = test_text(x, y, 76, 1216, 2, b"Chat", TEXT_DARK) { return c; }
                } else if is_card1 {
                    if let Some(c) = test_text(x, y, 225, 1216, 2, b"Camera", TEXT_DARK) { return c; }
                } else if is_card2 {
                    if let Some(c) = test_text(x, y, 375, 1216, 2, b"Gallery", TEXT_DARK) { return c; }
                } else if is_card3 {
                    if let Some(c) = test_text(x, y, 535, 1216, 2, b"Files", TEXT_DARK) { return c; }
                }
            }
            if y >= 1155 && y < 1195 {
                if is_card0 {
                    let dx = (x as i32) - 115;
                    let dy = (y as i32) - 1175;
                    if dx * dx + dy * dy <= 256 { return ACCENT_SKY; }
                } else if is_card1 {
                    let dx = (x as i32) - 268;
                    let dy = (y as i32) - 1175;
                    if dx * dx + dy * dy <= 256 { return SUCCESS_EMERALD; }
                } else if is_card2 {
                    let dx = (x as i32) - 421;
                    let dy = (y as i32) - 1175;
                    if dx * dx + dy * dy <= 256 { return ACCENT_AMBER; }
                } else if is_card3 {
                    let dx = (x as i32) - 574;
                    let dy = (y as i32) - 1175;
                    if dx * dx + dy * dy <= 256 { return DANGER_CRIMSON; }
                }
            }
            return SURFACE_LIGHT;
        }
        return BG_DARK;
    }

    // 6. Universal "Ask anything..." Command Bar (x: 40..680, y: 1290..1365)
    if x >= 40 && x < 680 && y >= 1290 && y < 1365 {
        if y == 1290 || y == 1364 || x == 40 || x == 679 { return BORDER_SUBTLE; }

        // Search magnifying glass icon
        let dx_s = (x as i32) - 68;
        let dy_s = (y as i32) - 1327;
        let d_s = dx_s * dx_s + dy_s * dy_s;
        if (d_s >= 36 && d_s <= 64) || (dx_s >= 6 && dx_s <= 10 && dx_s == dy_s) {
            return TEXT_MUTED;
        }

        // Placeholder text
        if y >= 1320 && y < 1336 {
            if let Some(c) = test_text(x, y, 96, 1320, 2, b"Ask anything...", TEXT_MUTED) { return c; }
        }

        // Cyan microphone icon right
        let dx_m = (x as i32) - 644;
        let dy_m = (y as i32) - 1327;
        if dx_m >= -4 && dx_m <= 4 && dy_m >= -8 && dy_m <= 4 { return ACCENT_SKY; }
        if dy_m >= 6 && dy_m <= 8 && dx_m >= -6 && dx_m <= 6 { return ACCENT_SKY; }
        return SURFACE_RAISED;
    }

    // 7. Bottom Home Gesture Bar (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

#[inline(never)]
fn get_activity_color(x: usize, y: usize, _state: &LauncherState) -> u32 {
    // Header
    if y < 120 {
        if y >= 64 && y < 88 {
            if let Some(c) = test_text(x, y, 48, 64, 3, b"Activity Space", TEXT_PRIMARY) { return c; }
        }
        let dx = (x as i32) - 644;
        let dy = (y as i32) - 76;
        let d = dx * dx + dy * dy;
        if (d >= 36 && d <= 64) || (dx >= 6 && dx <= 10 && dx == dy) { return TEXT_MUTED; }
        if y >= 116 { return BORDER_SUBTLE; }
        return BG_DARK;
    }

    // Card 0: InterOS Development (y in 200..480, x in 56..(W - 56))
    if y >= 200 && y < 480 && x >= 56 && x < (W - 56) {
        // Dismiss badge [x] top right (tested at 610, 230)
        if y >= 210 && y < 240 && x >= 590 && x < 644 {
            if y >= 218 && y < 226 {
                if let Some(c) = test_text(x, y, 610, 218, 1, b"x", TEXT_PRIMARY) { return c; }
            }
            return DANGER_CRIMSON;
        }
        if y >= 216 && y < 232 {
            if let Some(c) = test_text(x, y, 76, 216, 2, b"InterOS Development", TEXT_PRIMARY) { return c; }
        }
        if y >= 238 && y < 246 {
            if let Some(c) = test_text(x, y, 76, 238, 1, b"3 apps - 1h 14m", TEXT_MUTED) { return c; }
        }
        if y < 250 { return ACCENT_SKY; }
        if y >= 476 || x < 60 || x >= (W - 60) { return BORDER_SUBTLE; }

        // Window surface previews (Terminal, Docs, Browser)
        if y >= 270 && y < 450 {
            if x >= 76 && x < 240 {
                if y >= 310 && y < 318 {
                    if let Some(c) = test_text(x, y, 86, 310, 1, b">_ terminal", ACCENT_SKY) { return c; }
                }
                if y == 270 || y == 449 || x == 76 || x == 239 { return BORDER_SUBTLE; }
                return 0xFF05080E;
            }
            if x >= 260 && x < 424 {
                if y >= 310 && y < 318 {
                    if let Some(c) = test_text(x, y, 270, 310, 1, b"# Docs", TEXT_DARK) { return c; }
                }
                if y == 270 || y == 449 || x == 260 || x == 423 { return BORDER_SUBTLE; }
                return SURFACE_LIGHT;
            }
            if x >= 444 && x < 608 {
                if y >= 310 && y < 318 {
                    if let Some(c) = test_text(x, y, 454, 310, 1, b"Browser", TEXT_PRIMARY) { return c; }
                }
                if y == 270 || y == 449 || x == 444 || x == 607 { return BORDER_SUBTLE; }
                return SURFACE_RAISED;
            }
        }
        return SURFACE_DARK;
    }

    // Card 1: Research (y in 520..800, x in 56..(W - 56))
    if y >= 520 && y < 800 && x >= 56 && x < (W - 56) {
        if y >= 530 && y < 560 && x >= 590 && x < 644 {
            if y >= 538 && y < 546 {
                if let Some(c) = test_text(x, y, 610, 538, 1, b"x", TEXT_PRIMARY) { return c; }
            }
            return DANGER_CRIMSON;
        }
        if y >= 536 && y < 552 {
            if let Some(c) = test_text(x, y, 76, 536, 2, b"Research", TEXT_PRIMARY) { return c; }
        }
        if y >= 558 && y < 566 {
            if let Some(c) = test_text(x, y, 76, 558, 1, b"2 apps - 42m", TEXT_MUTED) { return c; }
        }
        if y < 570 { return SUCCESS_EMERALD; }
        if y >= 796 || x < 60 || x >= (W - 60) { return BORDER_SUBTLE; }
        if y >= 650 && y < 690 && x >= 100 && x < (W - 100) { return SURFACE_RAISED; }
        return SURFACE_DARK;
    }

    // Card 2: Communication (y in 840..1120, x in 56..(W - 56))
    if y >= 840 && y < 1120 && x >= 56 && x < (W - 56) {
        if y >= 850 && y < 880 && x >= 590 && x < 644 {
            if y >= 858 && y < 866 {
                if let Some(c) = test_text(x, y, 610, 858, 1, b"x", TEXT_PRIMARY) { return c; }
            }
            return DANGER_CRIMSON;
        }
        if y >= 856 && y < 872 {
            if let Some(c) = test_text(x, y, 76, 856, 2, b"Communication", TEXT_PRIMARY) { return c; }
        }
        if y >= 878 && y < 886 {
            if let Some(c) = test_text(x, y, 76, 878, 1, b"4 apps - 28m", TEXT_MUTED) { return c; }
        }
        if y < 890 { return ACCENT_AMBER; }
        if y >= 1116 || x < 60 || x >= (W - 60) { return BORDER_SUBTLE; }
        if y >= 970 && y < 1010 && x >= 100 && x < (W - 100) { return SURFACE_RAISED; }
        return SURFACE_DARK;
    }

    // Clear / Return / "+ New Activity" button: y in 1200..1280, x in 180..(W - 180)
    if y >= 1200 && y < 1280 && x >= 180 && x < (W - 180) {
        if y >= 1230 && y < 1246 {
            if let Some(c) = test_text(x, y, 260, 1230, 2, b"+ New Activity", TEXT_PRIMARY) { return c; }
        }
        if y >= 1276 || y <= 1204 || x < 184 || x >= (W - 184) { return ACCENT_SKY; }
        return SURFACE_RAISED;
    }

    // Bottom Home Gesture Pill
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
            if let Some(c) = test_text(x, y, 48, 64, 2, b"09:20  Mon, 12 Jan", TEXT_PRIMARY) { return c; }
        }
        let dx = (x as i32) - 640;
        let dy = (y as i32) - 76;
        let d = dx * dx + dy * dy;
        if (d >= 100 && d <= 196) || d <= 25 { return TEXT_MUTED; }
        if y >= 116 { return BORDER_SUBTLE; }
        return BG_DARK;
    }

    // Primary Connectivity Cards (y: 130..260, 2 Columns)
    if y >= 130 && y < 260 {
        // Wi-Fi
        if x >= 40 && x < 345 {
            if y >= 175 && y < 191 {
                if let Some(c) = test_text(x, y, 100, 175, 2, b"Wi-Fi", TEXT_PRIMARY) { return c; }
            }
            if y >= 205 && y < 213 {
                if let Some(c) = test_text(x, y, 100, 205, 1, b"Connected", SUCCESS_EMERALD) { return c; }
            }
            let dx = (x as i32) - 72;
            let dy = (y as i32) - 195;
            let d = dx * dx + dy * dy;
            if (d >= 64 && d <= 100) || (d >= 16 && d <= 36) || d <= 4 { return SUCCESS_EMERALD; }
            if y >= 256 || x == 40 || x == 344 { return BORDER_SUBTLE; }
            return SURFACE_RAISED;
        }
        // Bluetooth
        if x >= 375 && x < 680 {
            if y >= 175 && y < 191 {
                if let Some(c) = test_text(x, y, 435, 175, 2, b"Bluetooth", TEXT_PRIMARY) { return c; }
            }
            if y >= 205 && y < 213 {
                if let Some(c) = test_text(x, y, 435, 205, 1, b"On", SUCCESS_EMERALD) { return c; }
            }
            let dx = (x as i32) - 407;
            let dy = (y as i32) - 195;
            if (dx == 0 && dy >= -12 && dy <= 12) || (dx == dy && dy >= -8 && dy <= 8) { return ACCENT_SKY; }
            if y >= 256 || x == 375 || x == 679 { return BORDER_SUBTLE; }
            return SURFACE_RAISED;
        }
        return BG_DARK;
    }

    // Secondary Toggles (y: 280..380, 3 Columns)
    if y >= 280 && y < 380 {
        if (x >= 40 && x < 235) || (x >= 262 && x < 458) || (x >= 485 && x < 680) {
            if y >= 320 && y < 336 {
                if x < 235 {
                    if let Some(c) = test_text(x, y, 64, 320, 2, b"5G Mobile", TEXT_PRIMARY) { return c; }
                } else if x < 458 {
                    if let Some(c) = test_text(x, y, 290, 320, 2, b"Hotspot", TEXT_MUTED) { return c; }
                } else {
                    if let Some(c) = test_text(x, y, 508, 320, 2, b"Airplane", TEXT_MUTED) { return c; }
                }
            }
            if y >= 376 || y == 280 { return BORDER_SUBTLE; }
            return SURFACE_DARK;
        }
        return BG_DARK;
    }

    // Continuous Sliders (y: 400..570)
    // Brightness Slider
    if x >= 40 && x < 680 && y >= 400 && y < 465 {
        if x <= 520 { return SURFACE_LIGHT; }
        return SURFACE_DARK;
    }
    // Volume Slider
    if x >= 40 && x < 680 && y >= 485 && y < 550 {
        if x <= 424 { return ACCENT_SKY; }
        return SURFACE_DARK;
    }

    // Active Sovereign Media Player Card (x: 40..680, y: 720..890)
    if x >= 40 && x < 680 && y >= 720 && y < 890 {
        let dx = (x as i32) - 100;
        let dy = (y as i32) - 805;
        let d = dx * dx + dy * dy;
        if d <= 1764 {
            if d <= 144 { return ACCENT_AMBER; }
            if d >= 1600 || (d >= 400 && d <= 441) || (d >= 900 && d <= 961) { return 0xFF2A3448; }
            return 0xFF05070D;
        }
        if y >= 785 && y < 801 {
            if let Some(c) = test_text(x, y, 170, 785, 2, b"Interstellar", TEXT_PRIMARY) { return c; }
        }
        if y >= 815 && y < 831 {
            if let Some(c) = test_text(x, y, 170, 815, 2, b"Hans Zimmer", TEXT_MUTED) { return c; }
        }

        // Transport: |<<, ||, >>|
        if y >= 800 && y < 816 {
            if let Some(c) = test_text(x, y, 480, 800, 2, b"|<", TEXT_PRIMARY) { return c; }
            if let Some(c) = test_text(x, y, 540, 800, 2, b"||", ACCENT_SKY) { return c; }
            if let Some(c) = test_text(x, y, 600, 800, 2, b">|", TEXT_PRIMARY) { return c; }
        }

        if y >= 886 || x == 40 || x == 679 { return BORDER_SUBTLE; }
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
            if let Some(c) = test_text(x, y, 48, 50, 3, b"System", TEXT_PRIMARY) { return c; }
        }
        if y >= 96 { return BORDER_SUBTLE; }
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
            if s_dx * s_dx + s_dy * s_dy <= 120 { return 0xFFFFFFFF; }
            if s_dx * s_dx + s_dy * s_dy <= 300 { return ACCENT_SKY; }
            if d >= r_sq - 300 { return 0xFF235A7C; }
            return 0xFF121B2A;
        }
        if y >= 275 && y < 283 {
            if let Some(c) = test_text(x, y, 260, 275, 1, b"Everything in one place", TEXT_MUTED) { return c; }
        }
        return BG_DARK;
    }

    // 2-Column Visual Category Grid (y: 310..1180)
    if y >= 310 && y < 430 {
        if x >= 40 && x < 345 {
            if y >= 350 && y < 366 {
                if let Some(c) = test_text(x, y, 70, 350, 2, b"Device", TEXT_PRIMARY) { return c; }
            }
            if y >= 380 && y < 388 {
                if let Some(c) = test_text(x, y, 70, 380, 1, b"Display, Sound, Gestures", TEXT_MUTED) { return c; }
            }
            return SURFACE_RAISED;
        }
        if x >= 375 && x < 680 {
            if y >= 350 && y < 366 {
                if let Some(c) = test_text(x, y, 405, 350, 2, b"Network", TEXT_PRIMARY) { return c; }
            }
            if y >= 380 && y < 388 {
                if let Some(c) = test_text(x, y, 405, 380, 1, b"Wi-Fi, Mobile, Bluetooth", TEXT_MUTED) { return c; }
            }
            return SURFACE_RAISED;
        }
    }
    if y >= 450 && y < 570 {
        if x >= 40 && x < 345 {
            if y >= 490 && y < 506 {
                if let Some(c) = test_text(x, y, 70, 490, 2, b"Personal", TEXT_PRIMARY) { return c; }
            }
            if y >= 520 && y < 528 {
                if let Some(c) = test_text(x, y, 70, 520, 1, b"Accounts, Privacy, Language", TEXT_MUTED) { return c; }
            }
            return SURFACE_RAISED;
        }
        if x >= 375 && x < 680 {
            if y >= 490 && y < 506 {
                if let Some(c) = test_text(x, y, 405, 490, 2, b"Apps", TEXT_PRIMARY) { return c; }
            }
            if y >= 520 && y < 528 {
                if let Some(c) = test_text(x, y, 405, 520, 1, b"Permissions, Storage", TEXT_MUTED) { return c; }
            }
            return SURFACE_RAISED;
        }
    }
    if y >= 590 && y < 710 {
        if x >= 40 && x < 345 {
            if y >= 630 && y < 646 {
                if let Some(c) = test_text(x, y, 70, 630, 2, b"Power", TEXT_PRIMARY) { return c; }
            }
            if y >= 660 && y < 668 {
                if let Some(c) = test_text(x, y, 70, 660, 1, b"Battery, DVFS Governor", TEXT_MUTED) { return c; }
            }
            return SURFACE_RAISED;
        }
        if x >= 375 && x < 680 {
            if y >= 630 && y < 646 {
                if let Some(c) = test_text(x, y, 405, 630, 2, b"Security", TEXT_PRIMARY) { return c; }
            }
            if y >= 660 && y < 668 {
                if let Some(c) = test_text(x, y, 405, 660, 1, b"KeyStore, Encryption", TEXT_MUTED) { return c; }
            }
            return SURFACE_RAISED;
        }
    }

    if y >= 1418 && y < 1424 && x >= 260 && x < 460 { return TEXT_MUTED; }
    BG_DARK
}

#[inline(never)]
fn get_lock_screen_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 { return BORDER_SUBTLE; }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, b"--:--", TEXT_PRIMARY) { return c; }
        }
        // 4-Bar Cellular Signal
        if x >= 590 && x <= 608 && y >= 16 && y <= 32 {
            if x <= 593 && y >= 28 { return TEXT_PRIMARY; }
            if x >= 595 && x <= 598 && y >= 24 { return TEXT_PRIMARY; }
            if x >= 600 && x <= 603 && y >= 20 { return TEXT_PRIMARY; }
            if x >= 605 && x <= 608 && y >= 16 { return TEXT_PRIMARY; }
        }
        // Wi-Fi 3-arcs
        if x >= 626 && x <= 644 && y >= 18 && y <= 32 {
            let dx = (x as i32) - 635;
            let dy = (y as i32) - 30;
            let d2 = dx * dx + dy * dy;
            if (d2 >= 100 && d2 <= 144 && dy <= -2) || (d2 >= 36 && d2 <= 64 && dy <= -1) || d2 <= 9 {
                return TEXT_PRIMARY;
            }
        }
        // Battery outline only until real telemetry is available
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 { return TEXT_PRIMARY; }
            if x >= 685 && x <= 686 && y >= 22 && y <= 28 { return TEXT_PRIMARY; }
            // Synthetic battery fill removed; hardware telemetry must supply the level.
        }
        return BG_DARK;
    }

    // 2. Scenic Mountain Atmospheric Backdrop (y: 48..1280)
    let r1 = 620 + (((x as i32 - 360) * (x as i32 - 360)) / 1400) as usize;
    let r2 = 780 + (((x as i32 - 180) * (x as i32 - 540)) / 1800).abs() as usize;
    let r3 = 940 + (((x as i32 - 280) * (x as i32 - 440)) / 1200).abs() as usize;

    // Date & Tabular Clock Overlay
    if y >= 160 && y < 176 {
        if let Some(c) = test_text(x, y, 48, 160, 2, b"Wed, 24 Sep", TEXT_MUTED) { return c; }
    }
    if y >= 210 && y < 250 {
        if let Some(c) = test_text(x, y, 48, 210, 5, b"--:--", TEXT_PRIMARY) { return c; }
    }

    // Mountain Silhouette Layering
    if y >= r3 && y < 1280 {
        return 0xFF080B10; // Forefront mountain ridge
    } else if y >= r2 && y < 1280 {
        return 0xFF101726; // Mid mountain ridge
    } else if y >= r1 && y < 1280 {
        return 0xFF182236; // Distant mountain ridge
    } else if y >= 48 && y < 1280 {
        // Sky gradient
        let ratio = (y - 48) / 30;
        let blue = (0x18 + ratio).min(0x28) as u32;
        return 0xFF0B1000 | blue;
    }

    // 3. Bottom Shortcuts & Unlock Hint (y: 1280..1440)
    // Flashlight Circular Button at (72, 1330, r: 30)
    let f_dx = (x as i32) - 72;
    let f_dy = (y as i32) - 1330;
    let f_d2 = f_dx * f_dx + f_dy * f_dy;
    if f_d2 <= 900 {
        if f_d2 >= 841 { return BORDER_SUBTLE; }
        if y >= 1324 && y < 1332 {
            let label = if state.flashlight_on { &b"ON"[..] } else { &b"LGT"[..] };
            let fg = if state.flashlight_on { TEXT_DARK } else { TEXT_PRIMARY };
            let ox = if state.flashlight_on { 64 } else { 60 };
            if let Some(c) = test_text(x, y, ox, 1324, 1, label, fg) { return c; }
        }
        return if state.flashlight_on { ACCENT_AMBER } else { SURFACE_RAISED };
    }

    // Camera Circular Button at (648, 1330, r: 30)
    let c_dx = (x as i32) - 648;
    let c_dy = (y as i32) - 1330;
    let c_d2 = c_dx * c_dx + c_dy * c_dy;
    if c_d2 <= 900 {
        if c_d2 >= 841 { return BORDER_SUBTLE; }
        if y >= 1324 && y < 1332 {
            if let Some(c) = test_text(x, y, 636, 1324, 1, b"CAM", TEXT_PRIMARY) { return c; }
        }
        return SURFACE_RAISED;
    }

    // Centered "Swipe up to unlock" Hint
    if y >= 1374 && y < 1390 {
        if let Some(c) = test_text(x, y, 208, 1374, 2, b"Swipe up to unlock", TEXT_MUTED) { return c; }
    }

    // Home Gesture Indicator Pill
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    0xFF080B10
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
        if let Some(c) = test_text(x, y, 370, 95, 2, b"Power & Safety", TEXT_PRIMARY) { return c; }
    }
    if y >= 120 && y < 128 {
        if let Some(c) = test_text(x, y, 370, 120, 1, b"System actions & control", TEXT_MUTED) { return c; }
    }

    // Action Card 1: Lock device (y: 190..265, x: 365..695)
    if y >= 190 && y < 265 && x >= 365 && x < 695 {
        if y == 190 || y == 264 || x == 365 || x == 694 { return BORDER_SUBTLE; }
        if y >= 212 && y < 228 {
            if let Some(c) = test_text(x, y, 395, 212, 2, b"Lock Device", TEXT_PRIMARY) { return c; }
        }
        if y >= 236 && y < 244 {
            if let Some(c) = test_text(x, y, 395, 236, 1, b"Secure current session", TEXT_MUTED) { return c; }
        }
        return SURFACE_RAISED;
    }

    // Action Card 2: Restart (y: 285..360, x: 365..695)
    if y >= 285 && y < 360 && x >= 365 && x < 695 {
        if y == 285 || y == 359 || x == 365 || x == 694 { return BORDER_SUBTLE; }
        if y >= 307 && y < 323 {
            if let Some(c) = test_text(x, y, 395, 307, 2, b"Restart", TEXT_PRIMARY) { return c; }
        }
        if y >= 331 && y < 339 {
            if let Some(c) = test_text(x, y, 395, 331, 1, b"Reboot InterOS kernel", TEXT_MUTED) { return c; }
        }
        return SURFACE_RAISED;
    }

    // Action Card 3: Power off (y: 380..455, x: 365..695)
    if y >= 380 && y < 455 && x >= 365 && x < 695 {
        if y == 380 || y == 454 || x == 365 || x == 694 { return BORDER_SUBTLE; }
        if y >= 402 && y < 418 {
            if let Some(c) = test_text(x, y, 395, 402, 2, b"Power Off", TEXT_PRIMARY) { return c; }
        }
        if y >= 426 && y < 434 {
            if let Some(c) = test_text(x, y, 395, 426, 1, b"Shut down device power", TEXT_MUTED) { return c; }
        }
        return SURFACE_RAISED;
    }

    // Action Card 4: Emergency (y: 475..550, x: 365..695)
    if y >= 475 && y < 550 && x >= 365 && x < 695 {
        if y == 475 || y == 549 || x == 365 || x == 694 { return DANGER_CRIMSON; }
        if y >= 497 && y < 513 {
            if let Some(c) = test_text(x, y, 395, 497, 2, b"Emergency", DANGER_CRIMSON) { return c; }
        }
        if y >= 521 && y < 529 {
            if let Some(c) = test_text(x, y, 395, 521, 1, b"Direct dial emergency lines", TEXT_MUTED) { return c; }
        }
        return 0xFF221115;
    }

    // Dismiss Button (y: 580..635, x: 420..640)
    if y >= 580 && y < 635 && x >= 420 && x < 640 {
        if y == 580 || y == 634 || x == 420 || x == 639 { return BORDER_SUBTLE; }
        if y >= 600 && y < 616 {
            if let Some(c) = test_text(x, y, 490, 600, 2, b"Cancel", TEXT_PRIMARY) { return c; }
        }
        return SURFACE_RAISED;
    }

    SURFACE_DARK
}

#[inline(never)]
fn get_app_drawer_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 { return BORDER_SUBTLE; }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, b"--:--", TEXT_PRIMARY) { return c; }
        }
        // Cellular
        if x >= 590 && x <= 608 && y >= 16 && y <= 32 {
            if x <= 593 && y >= 28 { return TEXT_PRIMARY; }
            if x >= 595 && x <= 598 && y >= 24 { return TEXT_PRIMARY; }
            if x >= 600 && x <= 603 && y >= 20 { return TEXT_PRIMARY; }
            if x >= 605 && x <= 608 && y >= 16 { return TEXT_PRIMARY; }
        }
        // Wi-Fi
        if x >= 626 && x <= 644 && y >= 18 && y <= 32 {
            let dx = (x as i32) - 635;
            let dy = (y as i32) - 30;
            let d2 = dx * dx + dy * dy;
            if (d2 >= 100 && d2 <= 144 && dy <= -2) || (d2 >= 36 && d2 <= 64 && dy <= -1) || d2 <= 9 {
                return TEXT_PRIMARY;
            }
        }
        // Battery
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 { return TEXT_PRIMARY; }
            if x >= 685 && x <= 686 && y >= 22 && y <= 28 { return TEXT_PRIMARY; }
            // Synthetic battery fill removed; hardware telemetry must supply the level.
        }
        return BG_DARK;
    }

    // 2. Header & Search (y: 50..130)
    if y >= 50 && y < 130 {
        if y >= 64 && y < 88 {
            if let Some(c) = test_text(x, y, 48, 64, 3, b"Apps", TEXT_PRIMARY) { return c; }
        }
        if y >= 98 && y < 106 {
            if let Some(c) = test_text(x, y, 48, 98, 1, b"Sovereign .rapp registry", TEXT_MUTED) { return c; }
        }
        // Search icon circle at (644, 76, r: 20)
        let dx = (x as i32) - 644;
        let dy = (y as i32) - 76;
        let d2 = dx * dx + dy * dy;
        if d2 <= 400 {
            if d2 >= 361 { return BORDER_SUBTLE; }
            if (d2 >= 16 && d2 <= 64) || (dx >= 4 && dx <= 8 && dx == dy) { return TEXT_PRIMARY; }
            return SURFACE_RAISED;
        }
        return BG_DARK;
    }

    // 3. Category Filter Tabs (y: 135..185)
    if y >= 135 && y < 185 {
        // Tab 0: All (x: 40..170)
        if x >= 40 && x < 170 {
            if y == 135 || y == 184 || x == 40 || x == 169 { return BORDER_SUBTLE; }
            if y >= 153 && y < 169 {
                let fg = if state.app_drawer_tab == 0 { TEXT_DARK } else { TEXT_MUTED };
                if let Some(c) = test_text(x, y, 82, 153, 2, b"All", fg) { return c; }
            }
            return if state.app_drawer_tab == 0 { SURFACE_LIGHT } else { SURFACE_RAISED };
        }
        // Tab 1: Recent (x: 185..315)
        if x >= 185 && x < 315 {
            if y == 135 || y == 184 || x == 185 || x == 314 { return BORDER_SUBTLE; }
            if y >= 153 && y < 169 {
                let fg = if state.app_drawer_tab == 1 { TEXT_DARK } else { TEXT_MUTED };
                if let Some(c) = test_text(x, y, 210, 153, 2, b"Recent", fg) { return c; }
            }
            return if state.app_drawer_tab == 1 { SURFACE_LIGHT } else { SURFACE_RAISED };
        }
        // Tab 2: System (x: 330..460)
        if x >= 330 && x < 460 {
            if y == 135 || y == 184 || x == 330 || x == 459 { return BORDER_SUBTLE; }
            if y >= 153 && y < 169 {
                let fg = if state.app_drawer_tab == 2 { TEXT_DARK } else { TEXT_MUTED };
                if let Some(c) = test_text(x, y, 355, 153, 2, b"System", fg) { return c; }
            }
            return if state.app_drawer_tab == 2 { SURFACE_LIGHT } else { SURFACE_RAISED };
        }
        // Tab 3: Tools (x: 475..605)
        if x >= 475 && x < 605 {
            if y == 135 || y == 184 || x == 475 || x == 604 { return BORDER_SUBTLE; }
            if y >= 153 && y < 169 {
                let fg = if state.app_drawer_tab == 3 { TEXT_DARK } else { TEXT_MUTED };
                if let Some(c) = test_text(x, y, 508, 153, 2, b"Tools", fg) { return c; }
            }
            return if state.app_drawer_tab == 3 { SURFACE_LIGHT } else { SURFACE_RAISED };
        }
        return BG_DARK;
    }

    // 4. 4-Column Sovereign App Grid (y: 205..705)
    let (row_idx, row_y0) = if (205..315).contains(&y) {
        (0, 205)
    } else if (335..445).contains(&y) {
        (1, 335)
    } else if (465..575).contains(&y) {
        (2, 465)
    } else if (595..705).contains(&y) {
        (3, 595)
    } else {
        (99, 0)
    };

    if row_idx < 4 {
        let (col_idx, col_x0) = if (40..180).contains(&x) {
            (0, 40)
        } else if (200..340).contains(&x) {
            (1, 200)
        } else if (360..500).contains(&x) {
            (2, 360)
        } else if (520..660).contains(&x) {
            (3, 520)
        } else {
            (99, 0)
        };

        if col_idx < 4 {
            let app_idx = row_idx * 4 + col_idx;
            if y == row_y0 || y == row_y0 + 109 || x == col_x0 || x == col_x0 + 139 {
                return BORDER_SUBTLE;
            }

            let (name, icon, accent): (&[u8], &[u8], u32) = match app_idx {
                0 => (b"Terminal", b">_", ACCENT_SKY),
                1 => (b"Browser", b"WWW", 0xFF4ADE80),
                2 => (b"Files", b"DIR", 0xFF60A5FA),
                3 => (b"Camera", b"CAM", 0xFFF472B6),
                4 => (b"Notes", b"NOT", ACCENT_AMBER),
                5 => (b"AI Studio", b"AI*", 0xFFA78BFA),
                6 => (b"Gallery", b"IMG", 0xFF38BDF8),
                7 => (b"Settings", b"SET", TEXT_MUTED),
                8 => (b"Vault", b"KEY", 0xFFEAB308),
                9 => (b"Health", b"FIT", SUCCESS_EMERALD),
                10 => (b"Calendar", b"CAL", 0xFFFB7185),
                11 => (b"Calc", b"+-=", 0xFFF97316),
                12 => (b"Phone", b"TEL", SUCCESS_EMERALD),
                13 => (b"Messages", b"MSG", ACCENT_SKY),
                14 => (b"Music", b"AUD", 0xFFA855F7),
                15 => (b"Sensors", b"SNS", 0xFF06B6D4),
                _ => (b"", b"", BG_DARK),
            };

            let b_dx = (x as i32) - (col_x0 as i32 + 70);
            let b_dy = (y as i32) - (row_y0 as i32 + 36);
            let b_d2 = b_dx * b_dx + b_dy * b_dy;
            if b_d2 <= 400 {
                if b_d2 >= 361 { return accent; }
                if y >= row_y0 + 28 && y < row_y0 + 44 {
                    let text_ox = (col_x0 + 70).saturating_sub(icon.len() * 8);
                    if let Some(c) = test_text(x, y, text_ox, row_y0 + 28, 2, icon, accent) { return c; }
                }
                return 0xFF0E1422;
            }

            if y >= row_y0 + 72 && y < row_y0 + 88 {
                let text_ox = (col_x0 + 70).saturating_sub((name.len() * 8) / 2);
                if let Some(c) = test_text(x, y, text_ox, row_y0 + 72, 1, name, TEXT_PRIMARY) { return c; }
            }

            return SURFACE_RAISED;
        }
        return BG_DARK;
    }

    // 5. Pagination Indicator (y: 840..856)
    if y >= 844 && y <= 852 {
        let dx_c = (x as i32) - 360;
        let dy_c = (y as i32) - 848;
        if dx_c * dx_c + dy_c * dy_c <= 16 { return TEXT_PRIMARY; }
        let dx_l = (x as i32) - 336;
        if dx_l * dx_l + dy_c * dy_c <= 9 { return TEXT_MUTED; }
        let dx_r = (x as i32) - 384;
        if dx_r * dx_r + dy_c * dy_c <= 9 { return TEXT_MUTED; }
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
        if y == 47 { return BORDER_SUBTLE; }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, b"--:--", TEXT_PRIMARY) { return c; }
        }
        // Cellular
        if x >= 590 && x <= 608 && y >= 16 && y <= 32 {
            if x <= 593 && y >= 28 { return TEXT_PRIMARY; }
            if x >= 595 && x <= 598 && y >= 24 { return TEXT_PRIMARY; }
            if x >= 600 && x <= 603 && y >= 20 { return TEXT_PRIMARY; }
            if x >= 605 && x <= 608 && y >= 16 { return TEXT_PRIMARY; }
        }
        // Wi-Fi
        if x >= 626 && x <= 644 && y >= 18 && y <= 32 {
            let dx = (x as i32) - 635;
            let dy = (y as i32) - 30;
            let d2 = dx * dx + dy * dy;
            if (d2 >= 100 && d2 <= 144 && dy <= -2) || (d2 >= 36 && d2 <= 64 && dy <= -1) || d2 <= 9 {
                return TEXT_PRIMARY;
            }
        }
        // Battery
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 { return TEXT_PRIMARY; }
            if x >= 685 && x <= 686 && y >= 22 && y <= 28 { return TEXT_PRIMARY; }
            // Synthetic battery fill removed; hardware telemetry must supply the level.
        }
        return BG_DARK;
    }

    // 2. Header (y: 50..130)
    if y >= 50 && y < 130 {
        if y >= 64 && y < 88 {
            if let Some(c) = test_text(x, y, 48, 64, 3, b"Notifications", TEXT_PRIMARY) { return c; }
        }
        if y >= 96 && y < 104 {
            if let Some(c) = test_text(x, y, 48, 96, 1, b"Grouped system and app events", TEXT_MUTED) { return c; }
        }
        // Clear icon circle at (644, 76, r: 18)
        let dx = (x as i32) - 644;
        let dy = (y as i32) - 76;
        let d2 = dx * dx + dy * dy;
        if d2 <= 324 {
            if d2 >= 289 { return BORDER_SUBTLE; }
            if y >= 72 && y < 80 {
                if let Some(c) = test_text(x, y, 640, 72, 1, b"X", TEXT_MUTED) { return c; }
            }
            return SURFACE_RAISED;
        }
        return BG_DARK;
    }

    // 3. Filter Tabs (y: 135..185)
    if y >= 135 && y < 185 {
        // Tab 0: All (x: 40..170)
        if x >= 40 && x < 170 {
            if y == 135 || y == 184 || x == 40 || x == 169 { return BORDER_SUBTLE; }
            if y >= 153 && y < 169 {
                let fg = if state.notif_tab == 0 { TEXT_DARK } else { TEXT_MUTED };
                if let Some(c) = test_text(x, y, 82, 153, 2, b"All", fg) { return c; }
            }
            return if state.notif_tab == 0 { SURFACE_LIGHT } else { SURFACE_RAISED };
        }
        // Tab 1: Messages (x: 185..315)
        if x >= 185 && x < 315 {
            if y == 135 || y == 184 || x == 185 || x == 314 { return BORDER_SUBTLE; }
            if y >= 153 && y < 169 {
                let fg = if state.notif_tab == 1 { TEXT_DARK } else { TEXT_MUTED };
                if let Some(c) = test_text(x, y, 202, 153, 2, b"Messages", fg) { return c; }
            }
            return if state.notif_tab == 1 { SURFACE_LIGHT } else { SURFACE_RAISED };
        }
        // Tab 2: System (x: 330..460)
        if x >= 330 && x < 460 {
            if y == 135 || y == 184 || x == 330 || x == 459 { return BORDER_SUBTLE; }
            if y >= 153 && y < 169 {
                let fg = if state.notif_tab == 2 { TEXT_DARK } else { TEXT_MUTED };
                if let Some(c) = test_text(x, y, 355, 153, 2, b"System", fg) { return c; }
            }
            return if state.notif_tab == 2 { SURFACE_LIGHT } else { SURFACE_RAISED };
        }
        // Tab 3: Apps (x: 475..605)
        if x >= 475 && x < 605 {
            if y == 135 || y == 184 || x == 475 || x == 604 { return BORDER_SUBTLE; }
            if y >= 153 && y < 169 {
                let fg = if state.notif_tab == 3 { TEXT_DARK } else { TEXT_MUTED };
                if let Some(c) = test_text(x, y, 514, 153, 2, b"Apps", fg) { return c; }
            }
            return if state.notif_tab == 3 { SURFACE_LIGHT } else { SURFACE_RAISED };
        }
        return BG_DARK;
    }

    // 4. Actionable Notification Cards (y: 205..670, x: 40..680)
    // Card 0 (y: 205..310)
    if y >= 205 && y < 310 && x >= 40 && x < 680 {
        if x < 48 { return SUCCESS_EMERALD; }
        if y == 205 || y == 309 || x == 40 || x == 679 { return BORDER_SUBTLE; }
        if y >= 222 && y < 238 {
            if let Some(c) = test_text(x, y, 65, 222, 2, b"Build Complete", TEXT_PRIMARY) { return c; }
            if let Some(c) = test_text(x, y, 570, 226, 1, b"5m ago", TEXT_MUTED) { return c; }
        }
        if y >= 252 && y < 260 {
            if let Some(c) = test_text(x, y, 65, 252, 1, b"InterOS kernel compiled successfully", TEXT_MUTED) { return c; }
        }
        if y >= 272 && y < 280 {
            if let Some(c) = test_text(x, y, 65, 272, 1, b"> Tap to launch terminal console", ACCENT_SKY) { return c; }
        }
        return SURFACE_RAISED;
    }

    // Card 1 (y: 325..430)
    if y >= 325 && y < 430 && x >= 40 && x < 680 {
        if x < 48 { return ACCENT_SKY; }
        if y == 325 || y == 429 || x == 40 || x == 679 { return BORDER_SUBTLE; }
        if y >= 342 && y < 358 {
            if let Some(c) = test_text(x, y, 65, 342, 2, b"2 new messages", TEXT_PRIMARY) { return c; }
            if let Some(c) = test_text(x, y, 570, 346, 1, b"7m ago", TEXT_MUTED) { return c; }
        }
        if y >= 372 && y < 380 {
            if let Some(c) = test_text(x, y, 65, 372, 1, b"Bharath, Team: Ready for device sync", TEXT_MUTED) { return c; }
        }
        if y >= 392 && y < 400 {
            if let Some(c) = test_text(x, y, 65, 392, 1, b"> Tap to view conversation threads", SUCCESS_EMERALD) { return c; }
        }
        return SURFACE_RAISED;
    }

    // Card 2 (y: 445..550)
    if y >= 445 && y < 550 && x >= 40 && x < 680 {
        if x < 48 { return ACCENT_AMBER; }
        if y == 445 || y == 549 || x == 40 || x == 679 { return BORDER_SUBTLE; }
        if y >= 462 && y < 478 {
            if let Some(c) = test_text(x, y, 65, 462, 2, b"Meeting at 11:00 AM", TEXT_PRIMARY) { return c; }
            if let Some(c) = test_text(x, y, 560, 466, 1, b"12m ago", TEXT_MUTED) { return c; }
        }
        if y >= 492 && y < 500 {
            if let Some(c) = test_text(x, y, 65, 492, 1, b"InterOS Sync (Google Meet)", TEXT_MUTED) { return c; }
        }
        if y >= 512 && y < 520 {
            if let Some(c) = test_text(x, y, 65, 512, 1, b"> Tap to open calendar schedule", ACCENT_AMBER) { return c; }
        }
        return SURFACE_RAISED;
    }

    // Card 3 (y: 565..670)
    if y >= 565 && y < 670 && x >= 40 && x < 680 {
        if x < 48 { return 0xFFA78BFA; }
        if y == 565 || y == 669 || x == 40 || x == 679 { return BORDER_SUBTLE; }
        if y >= 582 && y < 598 {
            if let Some(c) = test_text(x, y, 65, 582, 2, b"System Update", TEXT_PRIMARY) { return c; }
            if let Some(c) = test_text(x, y, 570, 586, 1, b"1h ago", TEXT_MUTED) { return c; }
        }
        if y >= 612 && y < 620 {
            if let Some(c) = test_text(x, y, 65, 612, 1, b"InterOS v0.2.1 available for deployment", TEXT_MUTED) { return c; }
        }
        if y >= 632 && y < 640 {
            if let Some(c) = test_text(x, y, 65, 632, 1, b"> Tap to review and install update", ACCENT_SKY) { return c; }
        }
        return SURFACE_RAISED;
    }

    // 5. "Clear All" Pill Button (y: 700..755, x: 260..460)
    if y >= 700 && y < 755 && x >= 260 && x < 460 {
        if y == 700 || y == 754 || x == 260 || x == 459 { return BORDER_SUBTLE; }
        if y >= 720 && y < 736 {
            if let Some(c) = test_text(x, y, 316, 720, 2, b"Clear All", TEXT_MUTED) { return c; }
        }
        return SURFACE_RAISED;
    }

    // 6. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn resolve_app_in_drawer(x: usize, y: usize) -> Option<&'static str> {
    let row_idx = if (205..315).contains(&y) {
        0
    } else if (335..445).contains(&y) {
        1
    } else if (465..575).contains(&y) {
        2
    } else if (595..705).contains(&y) {
        3
    } else {
        return None;
    };

    let col_idx = if (40..180).contains(&x) {
        0
    } else if (200..340).contains(&x) {
        1
    } else if (360..500).contains(&x) {
        2
    } else if (520..660).contains(&x) {
        3
    } else {
        return None;
    };

    let app_idx = row_idx * 4 + col_idx;
    match app_idx {
        0 => Some("org.interos.terminal"),
        1 => Some("org.interos.browser"),
        2 => Some("org.interos.file_manager"),
        3 => Some("org.interos.camera"),
        4 => Some("org.interos.test_app"),
        5 => Some("org.interos.aistudio"),
        6 => Some("org.interos.gallery"),
        7 => Some("org.interos.settings"),
        8 => Some("org.interos.vault"),
        9 => Some("org.interos.health"),
        10 => Some("org.interos.calendar"),
        11 => Some("org.interos.calculator"),
        12 => Some("org.interos.dialer"),
        13 => Some("org.interos.messages"),
        14 => Some("org.interos.music"),
        15 => Some("org.interos.sensor_dashboard"),
        _ => None,
    }
}

#[inline(never)]
fn get_ai_companion_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 { return BORDER_SUBTLE; }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, b"--:--", TEXT_PRIMARY) { return c; }
        }
        // Cellular
        if x >= 590 && x <= 608 && y >= 16 && y <= 32 {
            if x <= 593 && y >= 28 { return TEXT_PRIMARY; }
            if x >= 595 && x <= 598 && y >= 24 { return TEXT_PRIMARY; }
            if x >= 600 && x <= 603 && y >= 20 { return TEXT_PRIMARY; }
            if x >= 605 && x <= 608 && y >= 16 { return TEXT_PRIMARY; }
        }
        // Wi-Fi
        if x >= 626 && x <= 644 && y >= 18 && y <= 32 {
            let dx = (x as i32) - 635;
            let dy = (y as i32) - 30;
            let d2 = dx * dx + dy * dy;
            if (d2 >= 100 && d2 <= 144 && dy <= -2) || (d2 >= 36 && d2 <= 64 && dy <= -1) || d2 <= 9 {
                return TEXT_PRIMARY;
            }
        }
        // Battery
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 { return TEXT_PRIMARY; }
            if x >= 685 && x <= 686 && y >= 22 && y <= 28 { return TEXT_PRIMARY; }
            // Synthetic battery fill removed; hardware telemetry must supply the level.
        }
        return BG_DARK;
    }

    // 2. Header (y: 50..120)
    if y >= 50 && y < 120 {
        if y >= 64 && y < 88 {
            if let Some(c) = test_text(x, y, 48, 64, 3, b"InterOS AI", TEXT_PRIMARY) { return c; }
        }
        if y >= 94 && y < 102 {
            if let Some(c) = test_text(x, y, 48, 94, 1, b"Sovereign Intelligence & Companion", TEXT_MUTED) { return c; }
        }
        // Options circle at (644, 76, r: 18)
        let dx = (x as i32) - 644;
        let dy = (y as i32) - 76;
        let d2 = dx * dx + dy * dy;
        if d2 <= 324 {
            if d2 >= 289 { return BORDER_SUBTLE; }
            if y >= 72 && y < 80 {
                if let Some(c) = test_text(x, y, 640, 72, 1, b"*", ACCENT_SKY) { return c; }
            }
            return SURFACE_RAISED;
        }
        return BG_DARK;
    }

    // 3. 3D Procedural Raytraced Dynamic Sphere / Orb (y: 130..320)
    if y >= 130 && y < 320 {
        let dx = (x as i32) - 360;
        let dy = (y as i32) - 225;
        let d2 = dx * dx + dy * dy;
        let r_sq = 72 * 72;
        if d2 <= r_sq {
            let s1_dx = dx + 24;
            let s1_dy = dy + 24;
            if s1_dx * s1_dx + s1_dy * s1_dy <= 120 {
                return 0xFFFFFFFF;
            }
            if s1_dx * s1_dx + s1_dy * s1_dy <= 380 {
                return ACCENT_SKY;
            }
            if state.ai_listening && d2 >= r_sq - 500 {
                return SUCCESS_EMERALD;
            } else if d2 >= r_sq - 320 {
                return 0xFF24608A;
            }
            return 0xFF0D1829;
        }
        return BG_DARK;
    }

    // 4. Greeting text (y: 335..375)
    if y >= 335 && y < 375 {
        if y >= 345 && y < 361 {
            let label = if state.ai_listening { &b"Listening to prompt..."[..] } else { &b"How can I help you?"[..] };
            let fg = if state.ai_listening { SUCCESS_EMERALD } else { TEXT_PRIMARY };
            let ox = 360usize.saturating_sub((label.len() * 16) / 2);
            if let Some(c) = test_text(x, y, ox, 345, 2, label, fg) { return c; }
        }
        return BG_DARK;
    }

    // 5. 4 Full-Width Action Cards (y: 390..735, x: 40..680)
    // Card 0: Summarize (y: 390..465)
    if y >= 390 && y < 465 && x >= 40 && x < 680 {
        if y == 390 || y == 464 || x == 40 || x == 679 { return BORDER_SUBTLE; }
        let b_dx = (x as i32) - 75;
        let b_dy = (y as i32) - 427;
        if b_dx * b_dx + b_dy * b_dy <= 324 {
            if y >= 423 && y < 431 {
                if let Some(c) = test_text(x, y, 63, 423, 1, b"DOC", ACCENT_SKY) { return c; }
            }
            return 0xFF0E1A2C;
        }
        if y >= 408 && y < 424 {
            if let Some(c) = test_text(x, y, 110, 408, 2, b"Summarize", TEXT_PRIMARY) { return c; }
            if let Some(c) = test_text(x, y, 645, 408, 2, b">", TEXT_MUTED) { return c; }
        }
        if y >= 432 && y < 440 {
            if let Some(c) = test_text(x, y, 110, 432, 1, b"Analyze active screen, document or link", TEXT_MUTED) { return c; }
        }
        return SURFACE_RAISED;
    }

    // Card 1: Create (y: 480..555)
    if y >= 480 && y < 555 && x >= 40 && x < 680 {
        if y == 480 || y == 554 || x == 40 || x == 679 { return BORDER_SUBTLE; }
        let b_dx = (x as i32) - 75;
        let b_dy = (y as i32) - 517;
        if b_dx * b_dx + b_dy * b_dy <= 324 {
            if y >= 513 && y < 521 {
                if let Some(c) = test_text(x, y, 63, 513, 1, b"GEN", 0xFFA78BFA) { return c; }
            }
            return 0xFF181228;
        }
        if y >= 498 && y < 514 {
            if let Some(c) = test_text(x, y, 110, 498, 2, b"Create", TEXT_PRIMARY) { return c; }
            if let Some(c) = test_text(x, y, 645, 498, 2, b">", TEXT_MUTED) { return c; }
        }
        if y >= 522 && y < 530 {
            if let Some(c) = test_text(x, y, 110, 522, 1, b"Draft notes, code tasks or ideas", TEXT_MUTED) { return c; }
        }
        return SURFACE_RAISED;
    }

    // Card 2: Find (y: 570..645)
    if y >= 570 && y < 645 && x >= 40 && x < 680 {
        if y == 570 || y == 644 || x == 40 || x == 679 { return BORDER_SUBTLE; }
        let b_dx = (x as i32) - 75;
        let b_dy = (y as i32) - 607;
        if b_dx * b_dx + b_dy * b_dy <= 324 {
            if y >= 603 && y < 611 {
                if let Some(c) = test_text(x, y, 63, 603, 1, b"FND", SUCCESS_EMERALD) { return c; }
            }
            return 0xFF0E2218;
        }
        if y >= 588 && y < 604 {
            if let Some(c) = test_text(x, y, 110, 588, 2, b"Find", TEXT_PRIMARY) { return c; }
            if let Some(c) = test_text(x, y, 645, 588, 2, b">", TEXT_MUTED) { return c; }
        }
        if y >= 612 && y < 620 {
            if let Some(c) = test_text(x, y, 110, 612, 1, b"Locate files, apps, settings and history", TEXT_MUTED) { return c; }
        }
        return SURFACE_RAISED;
    }

    // Card 3: Do (y: 660..735)
    if y >= 660 && y < 735 && x >= 40 && x < 680 {
        if y == 660 || y == 734 || x == 40 || x == 679 { return BORDER_SUBTLE; }
        let b_dx = (x as i32) - 75;
        let b_dy = (y as i32) - 697;
        if b_dx * b_dx + b_dy * b_dy <= 324 {
            if y >= 693 && y < 701 {
                if let Some(c) = test_text(x, y, 63, 693, 1, b"ACT", ACCENT_AMBER) { return c; }
            }
            return 0xFF241C0E;
        }
        if y >= 678 && y < 694 {
            if let Some(c) = test_text(x, y, 110, 678, 2, b"Do", TEXT_PRIMARY) { return c; }
            if let Some(c) = test_text(x, y, 645, 678, 2, b">", TEXT_MUTED) { return c; }
        }
        if y >= 702 && y < 710 {
            if let Some(c) = test_text(x, y, 110, 702, 1, b"Run system actions with capability consent", TEXT_MUTED) { return c; }
        }
        return SURFACE_RAISED;
    }

    // 6. Bottom Prompt Input Pill (x: 40..680, y: 1290..1365)
    if y >= 1290 && y < 1365 && x >= 40 && x < 680 {
        if y == 1290 || y == 1364 || x == 40 || x == 679 { return BORDER_SUBTLE; }
        let dx_s = (x as i32) - 68;
        let dy_s = (y as i32) - 1327;
        let d_s = dx_s * dx_s + dy_s * dy_s;
        if (d_s >= 25 && d_s <= 64) || (dx_s >= 5 && dx_s <= 9 && dx_s == dy_s) { return TEXT_MUTED; }

        if y >= 1320 && y < 1336 {
            let label = if state.ai_listening { &b"Listening..."[..] } else { &b"Ask anything..."[..] };
            let fg = if state.ai_listening { SUCCESS_EMERALD } else { TEXT_MUTED };
            if let Some(c) = test_text(x, y, 104, 1320, 2, label, fg) { return c; }
        }

        let dx_m = (x as i32) - 644;
        let dy_m = (y as i32) - 1327;
        let mic_fg = if state.ai_listening { SUCCESS_EMERALD } else { ACCENT_SKY };
        if dx_m >= -4 && dx_m <= 4 && dy_m >= -8 && dy_m <= 4 { return mic_fg; }
        if dy_m >= 6 && dy_m <= 8 && dx_m >= -6 && dx_m <= 6 { return mic_fg; }
        return SURFACE_RAISED;
    }

    // 7. Bottom Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

#[inline(never)]
fn get_live_widgets_color(x: usize, y: usize, _state: &LauncherState) -> u32 {
    // 1. Status Bar (y: 0..48)
    if y < 48 {
        if y == 47 { return BORDER_SUBTLE; }
        if y >= 16 && y < 32 {
            if let Some(c) = test_text(x, y, 48, 16, 2, b"--:--", TEXT_PRIMARY) { return c; }
        }
        // Cellular
        if x >= 590 && x <= 608 && y >= 16 && y <= 32 {
            if x <= 593 && y >= 28 { return TEXT_PRIMARY; }
            if x >= 595 && x <= 598 && y >= 24 { return TEXT_PRIMARY; }
            if x >= 600 && x <= 603 && y >= 20 { return TEXT_PRIMARY; }
            if x >= 605 && x <= 608 && y >= 16 { return TEXT_PRIMARY; }
        }
        // Wi-Fi
        if x >= 626 && x <= 644 && y >= 18 && y <= 32 {
            let dx = (x as i32) - 635;
            let dy = (y as i32) - 30;
            let d2 = dx * dx + dy * dy;
            if (d2 >= 100 && d2 <= 144 && dy <= -2) || (d2 >= 36 && d2 <= 64 && dy <= -1) || d2 <= 9 {
                return TEXT_PRIMARY;
            }
        }
        // Battery
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 { return TEXT_PRIMARY; }
            if x >= 685 && x <= 686 && y >= 22 && y <= 28 { return TEXT_PRIMARY; }
            // Synthetic battery fill removed; hardware telemetry must supply the level.
        }
        return BG_DARK;
    }

    // 2. Header (y: 50..125)
    if y >= 50 && y < 125 {
        if y >= 64 && y < 88 {
            if let Some(c) = test_text(x, y, 48, 64, 3, b"Live Widgets", TEXT_PRIMARY) { return c; }
        }
        if y >= 96 && y < 104 {
            if let Some(c) = test_text(x, y, 48, 96, 1, b"Real-time glanceable telemetry & stats", TEXT_MUTED) { return c; }
        }
        return BG_DARK;
    }

    // 3. 2-Column Top Widgets (y: 135..315)
    // Left: Weather (x: 40..345)
    if y >= 135 && y < 315 && x >= 40 && x < 345 {
        if y == 135 || y == 314 || x == 40 || x == 344 { return BORDER_SUBTLE; }
        if y >= 155 && y < 171 {
            if let Some(c) = test_text(x, y, 65, 155, 2, b"Location unavailable", TEXT_PRIMARY) { return c; }
        }
        if y >= 190 && y < 214 {
            if let Some(c) = test_text(x, y, 65, 190, 3, b"24*C", ACCENT_AMBER) { return c; }
        }
        let s_dx = (x as i32) - 300;
        let s_dy = (y as i32) - 180;
        let s_d2 = s_dx * s_dx + s_dy * s_dy;
        if s_d2 <= 324 {
            if s_d2 <= 144 { return ACCENT_AMBER; }
            if (s_dx == 0 || s_dy == 0 || s_dx == s_dy || s_dx == -s_dy) && s_d2 >= 196 { return ACCENT_AMBER; }
        }
        if y >= 250 && y < 258 {
            if let Some(c) = test_text(x, y, 65, 250, 1, b"Cloudy - Humidity 64%", TEXT_MUTED) { return c; }
        }
        return SURFACE_RAISED;
    }

    // Right: Build Progress (x: 375..680)
    if y >= 135 && y < 315 && x >= 375 && x < 680 {
        if y == 135 || y == 314 || x == 375 || x == 679 { return BORDER_SUBTLE; }
        if y >= 155 && y < 171 {
            if let Some(c) = test_text(x, y, 400, 155, 2, b"Build Status", TEXT_PRIMARY) { return c; }
            if let Some(c) = test_text(x, y, 610, 155, 2, b"82%", ACCENT_SKY) { return c; }
        }
        if y >= 200 && y < 216 && x >= 400 && x < 655 {
            if x <= 609 { return ACCENT_SKY; }
            return 0xFF0E1A2C;
        }
        if y >= 250 && y < 258 {
            if let Some(c) = test_text(x, y, 400, 250, 1, b"Compiling kernel... 12m left", TEXT_MUTED) { return c; }
        }
        return SURFACE_RAISED;
    }

    // 4. Now Playing Mini Player (x: 40..680, y: 335..485)
    if y >= 335 && y < 485 && x >= 40 && x < 680 {
        if y == 335 || y == 484 || x == 40 || x == 679 { return BORDER_SUBTLE; }
        let v_dx = (x as i32) - 95;
        let v_dy = (y as i32) - 410;
        let v_d2 = v_dx * v_dx + v_dy * v_dy;
        if v_d2 <= 1225 {
            if v_d2 <= 100 { return ACCENT_AMBER; }
            if v_d2 >= 1100 || (v_d2 >= 300 && v_d2 <= 340) || (v_d2 >= 650 && v_d2 <= 700) { return 0xFF2A3448; }
            return 0xFF05070D;
        }
        if y >= 380 && y < 396 {
            if let Some(c) = test_text(x, y, 155, 380, 2, b"Interstellar", TEXT_PRIMARY) { return c; }
        }
        if y >= 406 && y < 414 {
            if let Some(c) = test_text(x, y, 155, 406, 1, b"Hans Zimmer - Soundtrack", TEXT_MUTED) { return c; }
        }
        if y >= 430 && y < 436 && x >= 155 && x < 450 {
            if x <= 265 { return ACCENT_SKY; }
            return 0xFF142032;
        }
        if y >= 445 && y < 453 {
            if let Some(c) = test_text(x, y, 155, 445, 1, b"02:15 / 04:32", TEXT_MUTED) { return c; }
        }
        if y >= 400 && y < 416 {
            if let Some(c) = test_text(x, y, 500, 400, 2, b"|<", TEXT_PRIMARY) { return c; }
            if let Some(c) = test_text(x, y, 555, 400, 2, b"||", ACCENT_SKY) { return c; }
            if let Some(c) = test_text(x, y, 610, 400, 2, b">|", TEXT_PRIMARY) { return c; }
        }
        return SURFACE_RAISED;
    }

    // 5. 2-Column Bottom Widgets (y: 505..685)
    // Left: Meeting Widget (x: 40..345)
    if y >= 505 && y < 685 && x >= 40 && x < 345 {
        if y == 505 || y == 684 || x == 40 || x == 344 { return BORDER_SUBTLE; }
        if y >= 525 && y < 541 {
            if let Some(c) = test_text(x, y, 65, 525, 2, b"Next Meeting", TEXT_PRIMARY) { return c; }
        }
        if y >= 560 && y < 576 {
            if let Some(c) = test_text(x, y, 65, 560, 2, b"11:00 AM", ACCENT_SKY) { return c; }
        }
        if y >= 595 && y < 603 {
            if let Some(c) = test_text(x, y, 65, 595, 1, b"InterOS Sync (Meet)", TEXT_PRIMARY) { return c; }
        }
        if y >= 620 && y < 628 {
            if let Some(c) = test_text(x, y, 65, 620, 1, b"Starts in 1h 40m", TEXT_MUTED) { return c; }
        }
        return SURFACE_RAISED;
    }

    // Right: Daily Activity & Steps (x: 375..680)
    if y >= 505 && y < 685 && x >= 375 && x < 680 {
        if y == 505 || y == 684 || x == 375 || x == 679 { return BORDER_SUBTLE; }
        if y >= 525 && y < 541 {
            if let Some(c) = test_text(x, y, 400, 525, 2, b"Activity Rings", TEXT_PRIMARY) { return c; }
        }
        if y >= 560 && y < 584 {
            if let Some(c) = test_text(x, y, 400, 560, 3, b"3,482", SUCCESS_EMERALD) { return c; }
        }
        if y >= 605 && y < 613 {
            if let Some(c) = test_text(x, y, 400, 605, 1, b"steps today / 10k goal", TEXT_MUTED) { return c; }
        }
        if y >= 630 && y < 638 && x >= 400 && x < 655 {
            if x <= 490 { return SUCCESS_EMERALD; }
            return 0xFF0E2218;
        }
        return SURFACE_RAISED;
    }

    // 6. System Telemetry Gauge Card (x: 40..680, y: 705..845)
    if y >= 705 && y < 845 && x >= 40 && x < 680 {
        if y == 705 || y == 844 || x == 40 || x == 679 { return BORDER_SUBTLE; }
        if y >= 725 && y < 741 {
            if let Some(c) = test_text(x, y, 65, 725, 2, b"Sovereign Hardware Telemetry", TEXT_PRIMARY) { return c; }
        }
        if y >= 755 && y < 763 {
            if let Some(c) = test_text(x, y, 65, 755, 1, b"RAM: 18/64 MB | InterFS: 42/128 GB | 2.4 GHz", TEXT_MUTED) { return c; }
        }
        if y >= 785 && y < 797 && x >= 65 && x < 655 {
            if x <= 220 { return ACCENT_SKY; }
            if x <= 410 { return ACCENT_AMBER; }
            return 0xFF142032;
        }
        return SURFACE_RAISED;
    }

    // 7. Home Gesture Pill (x: 260..460, y: 1418..1424)
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
                ViewMode::AiCompanion => get_ai_companion_color(x, y, state),
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
            std::log(0, "[LAUNCHER] render_full complete: 10-app home screen canvas initialized.").ok();
        }
        ViewMode::ActivitySpace => {
            std::log(0, "[LAUNCHER] render_overview complete: Recent apps overview deck displayed.").ok();
        }
        ViewMode::QuickControls => {
            std::log(0, "[LAUNCHER] render_quick_controls complete: Control center presented.").ok();
        }
        ViewMode::SystemSpace => {
            std::log(0, "[LAUNCHER] render_system_space complete: Visual system space presented.").ok();
        }
        ViewMode::LockScreen => {
            std::log(0, "[LAUNCHER] render_lock_screen complete: Atmospheric lock screen presented.").ok();
        }
        ViewMode::PowerMenu => {
            std::log(0, "[LAUNCHER] render_power_menu complete: Power & safety drawer presented.").ok();
        }
        ViewMode::AppDrawer => {
            std::log(0, "[LAUNCHER] render_app_drawer complete: 16-app grid presented.").ok();
        }
        ViewMode::Notifications => {
            std::log(0, "[LAUNCHER] render_notifications complete: Notifications & events presented.").ok();
        }
        ViewMode::AiCompanion => {
            std::log(0, "[LAUNCHER] render_ai_companion complete: Sovereign AI assistant presented.").ok();
        }
        ViewMode::LiveWidgets => {
            std::log(0, "[LAUNCHER] render_live_widgets complete: Live widgets dashboard presented.").ok();
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

fn resolve_app_at_touch(x: u32, y: u32) -> Option<&'static str> {
    let x = x as usize;
    let y = y as usize;
    // Row 0
    if (280..420).contains(&y) {
        if (56..306).contains(&x) { return Some("org.interos.settings"); }
        if (360..610).contains(&x) { return Some("org.interos.terminal"); }
    }
    // Row 1
    if (450..590).contains(&y) {
        if (56..306).contains(&x) { return Some("org.interos.calculator"); }
        if (360..610).contains(&x) { return Some("org.interos.vault"); }
    }
    // Row 2
    if (620..760).contains(&y) {
        if (56..306).contains(&x) { return Some("org.interos.camera"); }
        if (360..610).contains(&x) { return Some("org.interos.dialer"); }
    }
    // Row 3
    if (790..930).contains(&y) {
        if (56..306).contains(&x) { return Some("org.interos.browser"); }
        if (360..610).contains(&x) { return Some("org.interos.messages"); }
    }
    // Row 4
    if (960..1100).contains(&y) {
        if (56..306).contains(&x) { return Some("org.interos.calendar"); }
        if (360..610).contains(&x) { return Some("org.interos.music"); }
    }
    // Bottom Dock (y in 1120..1400)
    if (1120..1400).contains(&y) {
        if (40..185).contains(&x) { return Some("org.interos.messages"); }
        if (190..335).contains(&x) { return Some("org.interos.camera"); }
        if (340..485).contains(&x) { return Some("org.interos.gallery"); }
        if (490..645).contains(&x) { return Some("org.interos.vault"); }
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

    std::log(0, "[LAUNCHER] Step 3: Rendering launcher UI canvas...").ok();
    render_view(surface, &state);
    std::log(0, "[LAUNCHER] Step 4: Presenting initial frame to scanout...").ok();
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
                    std::log(0, "[LAUNCHER] Foreground application exited and was reaped.").ok();
                    render_view(surface, &state);
                    let _ = std::surface_flip(4);
                }
            }

            if state.active_child.is_none() && state.current_view == ViewMode::HomeScreen && (events & std::EVENT_TIMER_TICK) != 0 {
                refresh_ticks += 1;
                if refresh_ticks % 10 == 0 {
                    render_header(surface);
                    let _ = std::surface_flip(4);
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
                                    std::log(0, "[LAUNCHER] Returned to home grid from recents via swipe.").ok();
                                    let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                                // Horizontal swipe-to-dismiss on cards
                                if input.dx.abs() >= 60 {
                                    let y = input.y as usize;
                                    if (200..480).contains(&y) {
                                        state.dismiss_recent(0);
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Card 0 dismissed via swipe.").ok();
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    } else if (520..800).contains(&y) {
                                        state.dismiss_recent(1);
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Card 1 dismissed via swipe.").ok();
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    } else if (840..1120).contains(&y) {
                                        state.dismiss_recent(2);
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] Card 2 dismissed via swipe.").ok();
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                                    std::log(0, "[LAUNCHER] Card 0 dismissed via dismiss button tap.").ok();
                                    let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                                // Tap Card 0 body to switch
                                else if (200..480).contains(&y) && (56..580).contains(&x) {
                                    let target = state.recent_apps[0];
                                    state.push_recent(target);
                                    if let Ok(task_id) = std::app_launch(target) {
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        std::log(0, "[LAUNCHER] Switched to recent app from overview card 0.").ok();
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Card 1 dismiss badge: x in 580..664, y in 520..580
                                else if (520..580).contains(&y) && (580..664).contains(&x) {
                                    state.dismiss_recent(1);
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Card 1 dismissed via dismiss button tap.").ok();
                                    let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                                // Tap Card 1 body to switch
                                else if (520..800).contains(&y) && (56..580).contains(&x) {
                                    let target = state.recent_apps[1];
                                    state.push_recent(target);
                                    if let Ok(task_id) = std::app_launch(target) {
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        std::log(0, "[LAUNCHER] Switched to recent app from overview card 1.").ok();
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Card 2 dismiss badge: x in 580..664, y in 840..900
                                else if (840..900).contains(&y) && (580..664).contains(&x) {
                                    state.dismiss_recent(2);
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Card 2 dismissed via dismiss button tap.").ok();
                                    let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                                // Tap Card 2 body to switch
                                else if (840..1120).contains(&y) && (56..580).contains(&x) {
                                    let target = state.recent_apps[2];
                                    state.push_recent(target);
                                    if let Ok(task_id) = std::app_launch(target) {
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        std::log(0, "[LAUNCHER] Switched to recent app from overview card 2.").ok();
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Return / Close button or Home bar
                                else if (1200..1300).contains(&y) || y >= 1400 {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Returned to home grid from recents.").ok();
                                    let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                            }
                        }
                        ViewMode::QuickControls => {
                            if (input.kind == 4 && input.dy <= -60) || (input.kind == 1 && (input.y >= 1350 || (input.y >= 70 && input.x >= 600))) {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                                    std::log(0, "[LAUNCHER] Power menu opened from Quick Controls.").ok();
                                    break;
                                }
                            }
                        }
                        ViewMode::SystemSpace => {
                            if (input.kind == 4 && input.dy >= 60) || (input.kind == 1 && input.y >= 1350) {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                                    std::log(0, "[LAUNCHER] Power menu opened from System Space.").ok();
                                    break;
                                }
                                state.push_recent("org.interos.settings");
                                if let Ok(task_id) = std::app_launch("org.interos.settings") {
                                    state.active_child = Some(task_id);
                                    state.current_view = ViewMode::HomeScreen;
                                    let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }
                            }
                        }
                        ViewMode::LockScreen => {
                            // 1. Vertical swipe up to unlock
                            if input.kind == 4 && input.dy <= -60 {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Device unlocked via swipe-up gesture.").ok();
                                let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            // 2. Touch tap
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;

                                // Flashlight circle button at (72, 1330, r: 35)
                                let f_dx = (x as i32) - 72;
                                let f_dy = (y as i32) - 1330;
                                if f_dx * f_dx + f_dy * f_dy <= 1225 {
                                    state.flashlight_on = !state.flashlight_on;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Flashlight toggled on lock screen.").ok();
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
                                        std::log(0, "[LAUNCHER] Camera launched from lock screen shortcut.").ok();
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }

                                // Bottom home bar or hint area tap unlocks
                                if y >= 1370 {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Device unlocked via bottom tap.").ok();
                                    let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                                let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            // Touch tap
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;

                                // Tap outside drawer or Cancel button dismisses
                                if x < 340 || y > 660 || (y >= 580 && y < 635 && x >= 420 && x < 640) {
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Power menu dismissed.").ok();
                                    let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Card 1: Lock device (y: 190..265, x: 365..695)
                                if (190..265).contains(&y) && (365..695).contains(&x) {
                                    state.current_view = ViewMode::LockScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Device locked via power menu.").ok();
                                    let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Card 2: Restart (y: 285..360, x: 365..695)
                                if (285..360).contains(&y) && (365..695).contains(&x) {
                                    std::log(0, "[LAUNCHER] Power menu: System restart confirmed.").ok();
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    break;
                                }

                                // Card 3: Power off (y: 380..455, x: 365..695)
                                if (380..455).contains(&y) && (365..695).contains(&x) {
                                    std::log(0, "[LAUNCHER] Power menu: System power off confirmed.").ok();
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
                                        std::log(0, "[LAUNCHER] Emergency dialer launched from power menu.").ok();
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                            }
                        }
                        ViewMode::AppDrawer => {
                            // 1. Gesture Navigation
                            // Swipe down or swipe right returns to HomeScreen
                            if (input.kind == 4 && (input.dy >= 60 || input.dx >= 60)) || (input.kind == 1 && input.y >= 1400) {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Returned to Home from App Drawer.").ok();
                                let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                                        std::log(0, "[LAUNCHER] App drawer category tab switched.").ok();
                                        break;
                                    }
                                }

                                // Launch application from 16-app grid
                                if let Some(target_app) = resolve_app_in_drawer(x, y) {
                                    state.push_recent(target_app);
                                    match std::app_launch(target_app) {
                                        Ok(task_id) => {
                                            state.active_child = Some(task_id);
                                            state.current_view = ViewMode::HomeScreen;
                                            std::log(0, "[LAUNCHER] App launched from drawer.").ok();
                                            let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                        ViewMode::Notifications => {
                            // 1. Gesture Navigation
                            // Swipe up or swipe right returns to HomeScreen
                            if (input.kind == 4 && (input.dy <= -60 || input.dx >= 60)) || (input.kind == 1 && input.y >= 1400) {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Returned to Home from Notifications.").ok();
                                let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                                if (y >= 700 && y < 755 && x >= 260 && x < 460) || (dx_c * dx_c + dy_c * dy_c <= 324) {
                                    let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    state.current_view = ViewMode::HomeScreen;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Notifications cleared.").ok();
                                    break;
                                }

                                // Card 0: Terminal
                                if (205..310).contains(&y) && (40..680).contains(&x) {
                                    state.push_recent("org.interos.terminal");
                                    if let Ok(task_id) = std::app_launch("org.interos.terminal") {
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                            }
                        }
                        ViewMode::AiCompanion => {
                            // Swipe right or swipe down returns to HomeScreen
                            if (input.kind == 4 && (input.dx >= 60 || input.dy >= 60)) || (input.kind == 1 && input.y >= 1400) {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Returned to Home from AI Companion.").ok();
                                let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;

                                // Mic icon tap (x: 620..670, y: 1290..1365)
                                if (1290..1365).contains(&y) && x >= 620 {
                                    state.ai_listening = !state.ai_listening;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] AI voice listening toggled.").ok();
                                    break;
                                }

                                // Card 0: Summarize (390..465)
                                if (390..465).contains(&y) && (40..680).contains(&x) {
                                    state.push_recent("org.interos.browser");
                                    if let Ok(task_id) = std::app_launch("org.interos.browser") {
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }

                                // Card 1: Create (480..555)
                                if (480..555).contains(&y) && (40..680).contains(&x) {
                                    state.push_recent("org.interos.test_app");
                                    if let Ok(task_id) = std::app_launch("org.interos.test_app") {
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }

                                // Card 2: Find (570..645)
                                if (570..645).contains(&y) && (40..680).contains(&x) {
                                    state.push_recent("org.interos.file_manager");
                                    if let Ok(task_id) = std::app_launch("org.interos.file_manager") {
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }

                                // Card 3: Do (660..735)
                                if (660..735).contains(&y) && (40..680).contains(&x) {
                                    state.push_recent("org.interos.settings");
                                    if let Ok(task_id) = std::app_launch("org.interos.settings") {
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                            }
                        }
                        ViewMode::LiveWidgets => {
                            // Swipe right returns to HomeScreen
                            if (input.kind == 4 && input.dx >= 60) || (input.kind == 1 && input.y >= 1400) {
                                state.current_view = ViewMode::HomeScreen;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Returned to Home from Live Widgets.").ok();
                                let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;

                                // Weather widget (135..315, 40..345)
                                if (135..315).contains(&y) && (40..345).contains(&x) {
                                    state.push_recent("org.interos.sensor_dashboard");
                                    if let Ok(task_id) = std::app_launch("org.interos.sensor_dashboard") {
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                        while std::input_event_poll(&mut drain).is_ok() {}
                                        break;
                                    }
                                }
                                // Build status widget (135..315, 375..680)
                                if (135..315).contains(&y) && (375..680).contains(&x) {
                                    state.push_recent("org.interos.terminal");
                                    if let Ok(task_id) = std::app_launch("org.interos.terminal") {
                                        state.active_child = Some(task_id);
                                        state.current_view = ViewMode::HomeScreen;
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                                        let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                                let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                                    std::log(0, "[LAUNCHER] App Drawer opened via upward swipe gesture.").ok();
                                }
                                let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                                let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            // Edge swipe from right opens Quick Controls
                            if input.kind == 4 && input.dx <= -60 && input.x >= 650 {
                                state.current_view = ViewMode::QuickControls;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Quick Controls shade opened via edge swipe gesture.").ok();
                                let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            // Horizontal swipe left opens Live Widgets
                            if input.kind == 4 && input.dx <= -60 && input.x < 650 {
                                state.current_view = ViewMode::LiveWidgets;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Live Widgets opened via horizontal swipe gesture.").ok();
                                let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }

                            // 2. Touch Tap
                            if input.kind == 1 {
                                let x = input.x as usize;
                                let y = input.y as usize;

                                // Tapping bottom home pill opens Activity Space
                                if (240..=480).contains(&x) && y >= 1400 {
                                    state.current_view = ViewMode::ActivitySpace;
                                    let mut win_buf = [0u8; 1024];
                                    let _ = std::window_list(&mut win_buf);
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Overview / Recent Apps switcher opened via home bar tap.").ok();
                                    let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
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
                                    let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Tapping Today card chevron '>' opens Notifications
                                if (240..300).contains(&y) && x >= 600 {
                                    state.current_view = ViewMode::Notifications;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Notifications opened from Today card header.").ok();
                                    let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Tapping Today card radio buttons toggles task state
                                if (65..=95).contains(&x) {
                                    if (305..=330).contains(&y) {
                                        state.task_done[0] = !state.task_done[0];
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        break;
                                    } else if (355..=380).contains(&y) {
                                        state.task_done[1] = !state.task_done[1];
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        break;
                                    } else if (405..=430).contains(&y) {
                                        state.task_done[2] = !state.task_done[2];
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        break;
                                    }
                                }

                                // Tapping "Ask anything..." search pill: mic opens AI companion, body opens System Space
                                if (1290..1365).contains(&y) && (40..680).contains(&x) {
                                    if x >= 620 {
                                        state.current_view = ViewMode::AiCompanion;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] AI Companion opened via mic tap.").ok();
                                    } else {
                                        state.current_view = ViewMode::SystemSpace;
                                        render_view(surface, &state);
                                        let _ = std::surface_flip(4);
                                        std::log(0, "[LAUNCHER] System Space opened via universal search bar tap.").ok();
                                    }
                                    let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Tapping dock 4th item (490..645, 1120..1400) opens App Drawer
                                if (1120..1400).contains(&y) && (490..645).contains(&x) {
                                    state.current_view = ViewMode::AppDrawer;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] App Drawer opened from dock shortcut.").ok();
                                    let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                    while std::input_event_poll(&mut drain).is_ok() {}
                                    break;
                                }

                                // Launch application
                                if let Some(target_app) = resolve_app_at_touch(input.x, input.y) {
                                    state.push_recent(target_app);
                                    match std::app_launch(target_app) {
                                        Ok(task_id) => {
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