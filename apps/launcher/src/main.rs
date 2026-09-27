#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 512;

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
}

pub struct LauncherState {
    pub current_view: ViewMode,
    pub task_done: [bool; 3],
    pub wifi_on: bool,
    pub bt_on: bool,
    pub dark_mode: bool,
    pub media_playing: bool,
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
            if let Some(c) = test_text(x, y, 48, 16, 2, b"09:20", TEXT_PRIMARY) { return c; }
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
        // Battery (outline pill + 78% fill)
        if x >= 658 && x <= 686 && y >= 18 && y <= 32 {
            if x == 658 || x == 684 || y == 18 || y == 32 { return TEXT_PRIMARY; }
            if x >= 685 && x <= 686 && y >= 22 && y <= 28 { return TEXT_PRIMARY; }
            if x >= 661 && x <= 680 && y >= 21 && y <= 29 { return SUCCESS_EMERALD; }
        }
        return BG_DARK;
    }

    // 2. Scenic Mountain Wallpaper & Header (y: 48..235)
    if y < 235 {
        // Date & Tabular Clock
        if y >= 64 && y < 80 {
            if let Some(c) = test_text(x, y, 48, 64, 2, b"Mon, 12 Jan", TEXT_MUTED) { return c; }
        }
        if y >= 90 && y < 122 {
            if let Some(c) = test_text(x, y, 48, 90, 4, b"09:20", TEXT_PRIMARY) { return c; }
        }

        // Weather Widget Right
        if x >= 460 && x <= 680 && y >= 80 && y < 135 {
            let dx = (x as i32) - 490;
            let dy = (y as i32) - 96;
            if dx * dx + dy * dy <= 81 { return ACCENT_AMBER; }
            if y >= 84 && y < 92 {
                if let Some(c) = test_text(x, y, 520, 84, 1, b"Bangalore", TEXT_MUTED) { return c; }
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
        if (340..485).contains(&x) { return Some("org.interos.vault"); }
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
                    render_view(surface, &state);
                    let _ = std::surface_flip(4);
                    std::log(0, "[LAUNCHER] Foreground application exited and was reaped.").ok();
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
                        ViewMode::HomeScreen => {
                            // 1. Gesture Navigation
                            // Swipe up opens Activity Space
                            if input.kind == 4 && input.dy <= -60 && input.y >= 1200 {
                                state.current_view = ViewMode::ActivitySpace;
                                let mut win_buf = [0u8; 1024];
                                let _ = std::window_list(&mut win_buf);
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Overview / Recent Apps switcher opened via swipe-up gesture.").ok();
                                let mut drain = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                                while std::input_event_poll(&mut drain).is_ok() {}
                                break;
                            }
                            // Swipe down from top status bar opens Quick Controls
                            if input.kind == 4 && input.dy >= 60 && input.y <= 120 {
                                state.current_view = ViewMode::QuickControls;
                                render_view(surface, &state);
                                let _ = std::surface_flip(4);
                                std::log(0, "[LAUNCHER] Quick Controls shade opened via swipe-down gesture.").ok();
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

                                // Tapping top status bar opens Quick Controls
                                if y <= 48 {
                                    state.current_view = ViewMode::QuickControls;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] Quick Controls shade opened via top status bar tap.").ok();
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

                                // Tapping "Ask anything..." search pill opens System Space
                                if (1290..1365).contains(&y) && (40..680).contains(&x) {
                                    state.current_view = ViewMode::SystemSpace;
                                    render_view(surface, &state);
                                    let _ = std::surface_flip(4);
                                    std::log(0, "[LAUNCHER] System Space opened via universal search bar tap.").ok();
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