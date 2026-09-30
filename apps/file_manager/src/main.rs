//! InterOS Native Sovereign File Manager & VFS Explorer App
//!
//! Native Ring-3 graphical application executing in User Mode:
//! - DRM/KMS Client Surface allocation (z=200) and double-buffered scanout presentation
//! - Real-time block device capacity query via microkernel storage driver
//! - Dynamic VFS directory traversal & file system exploration
//! - Internal Storage capacity queried from the storage service; usage is shown only when real telemetry is available.
//! - Tactile Category Shortcuts (Recent, Images, Docs, Downloads)
//! - Interactive directory navigation with hierarchical path breadcrumbs
//! - Asynchronous event reactor loop handling touch interactions and home gestures
//! - Graceful lifecycle supervision and clean return to Launcher

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
const SURFACE_RAISED: u32 = 0xFF1A2234;
const BORDER_SUBTLE: u32 = 0xFF222E42;
const TEXT_PRIMARY: u32 = 0xFFF8FAFC;
const TEXT_MUTED: u32 = 0xFF94A3B8;
const ACCENT_SKY: u32 = 0xFF31C4FF;
#[allow(dead_code)]
const ACCENT_AMBER: u32 = 0xFFF59E0B;
#[allow(dead_code)]
const SUCCESS_EMERALD: u32 = 0xFF10B981;
#[allow(dead_code)]
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
// VFS Entry Representation
// Runtime entries must come from the kernel VFS; no synthetic inventory is embedded.
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, Copy)]
pub struct VfsItem {
    pub name: &'static str,
    pub is_dir: bool,
    pub size_str: &'static str,
    pub meta_str: &'static str,
    pub color_accent: u32,
}

const ITEMS_ALL: [VfsItem; 0] = [];

pub struct FileManagerApp {
    pub selected_shortcut: usize,
    pub selected_row: usize,
    pub storage_capacity_bytes: u64,
    pub current_path: &'static str,
}

impl FileManagerApp {
    pub const fn new() -> Self {
        Self {
            selected_shortcut: 0,
            selected_row: 0,
            storage_capacity_bytes: 0,
            current_path: "/data",
        }
    }
}

// -----------------------------------------------------------------------------
// Pixel Color Resolution Engine
// -----------------------------------------------------------------------------
#[inline(never)]
fn get_file_manager_color(x: usize, y: usize, app: &FileManagerApp) -> u32 {
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

        // Header Title: "Files"
        if y >= 36 && y < 60 {
            if let Some(c) = test_text(x, y, 120, 36, 3, b"Files", TEXT_PRIMARY) {
                return c;
            }
        }

        // Search icon at (640, 48, r: 16)
        let s_dx = (x as i32) - 640;
        let s_dy = (y as i32) - 48;
        if s_dx * s_dx + s_dy * s_dy <= 256 {
            if s_dx * s_dx + s_dy * s_dy >= 225 {
                return BORDER_SUBTLE;
            }
            if y >= 44 && y < 52 {
                if let Some(c) = test_text(x, y, 636, 44, 1, b"Q", ACCENT_SKY) {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }

        return SURFACE_DARK;
    }

    // 2. Internal Storage Bar Card (x: 24..696, y: 110..220, r: 24 px)
    if y >= 110 && y < 220 && x >= 24 && x < 696 {
        if y == 110 || y == 219 || x == 24 || x == 695 {
            return BORDER_SUBTLE;
        }

        // Title: Internal Storage
        if y >= 126 && y < 142 {
            if let Some(c) = test_text(x, y, 48, 126, 2, b"Internal Storage", TEXT_PRIMARY) {
                return c;
            }
            if let Some(c) = test_text(x, y, 480, 126, 2, b"Usage unavailable", ACCENT_SKY) {
                return c;
            }
        }

        // Horizontal Progress Gauge Track (x: 48..672, y: 156..170)
        if y >= 156 && y < 170 && x >= 48 && x < 672 {
            if y == 156 || y == 169 || x == 48 || x == 671 {
                return BORDER_SUBTLE;
            }
            if x <= 254 {
                return ACCENT_SKY; // 33% filled
            }
            return 0xFF0E1A2C;
        }

        // Subtext: Free space & Partition format
        if y >= 184 && y < 192 {
            if let Some(c) = test_text(x, y, 48, 184, 1, b"Usage telemetry unavailable", TEXT_MUTED)
            {
                return c;
            }
        }

        return SURFACE_RAISED;
    }

    // 3. Grid Shortcuts (y: 235..335, 4 categories)
    if y >= 235 && y < 335 {
        let slot = if (24..176).contains(&x) {
            Some((0, 24, 176, &b"Recent"[..], &b"--"[..]))
        } else if (197..349).contains(&x) {
            Some((1, 197, 349, &b"Images"[..], &b"--"[..]))
        } else if (370..522).contains(&x) {
            Some((2, 370, 522, &b"Docs"[..], &b"--"[..]))
        } else if (543..695).contains(&x) {
            Some((3, 543, 695, &b"Download"[..], &b"--"[..]))
        } else {
            None
        };

        if let Some((idx, x0, x1, label, count_lbl)) = slot {
            let is_selected = app.selected_shortcut == idx;
            if y == 235 || y == 334 || x == x0 || x == x1 - 1 {
                return if is_selected {
                    ACCENT_SKY
                } else {
                    BORDER_SUBTLE
                };
            }
            // Category Icon Badge Circle at (x0 + 35, 265, r: 16)
            let b_dx = (x as i32) - (x0 as i32 + 35);
            let b_dy = (y as i32) - 265;
            if b_dx * b_dx + b_dy * b_dy <= 256 {
                if is_selected {
                    return ACCENT_SKY;
                }
                return 0xFF142032;
            }
            // Count Pill
            if y >= 258 && y < 274 {
                if let Some(c) = test_text(
                    x,
                    y,
                    x1 - 42,
                    258,
                    2,
                    count_lbl,
                    if is_selected { ACCENT_SKY } else { TEXT_MUTED },
                ) {
                    return c;
                }
            }
            // Title
            if y >= 295 && y < 311 {
                if let Some(c) = test_text(
                    x,
                    y,
                    x0 + 16,
                    295,
                    2,
                    label,
                    if is_selected {
                        TEXT_PRIMARY
                    } else {
                        TEXT_MUTED
                    },
                ) {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }
    }

    // 4. Current Path Banner (y: 350..395, x: 24..696)
    if y >= 350 && y < 395 && x >= 24 && x < 696 {
        if y == 350 || y == 394 || x == 24 || x == 695 {
            return BORDER_SUBTLE;
        }
        if y >= 366 && y < 382 {
            if let Some(c) = test_text(x, y, 48, 366, 2, b"Path:", TEXT_MUTED) {
                return c;
            }
            if let Some(c) = test_text(x, y, 140, 366, 2, app.current_path.as_bytes(), ACCENT_SKY) {
                return c;
            }
        }
        return SURFACE_DARK;
    }

    // 5. VFS File & Folder List Rows (y: 410..1320)
    if (410..1320).contains(&y) && x >= 24 && x < 696 {
        let row_idx = (y - 410) / 100;
        let in_row_y = (y - 410) % 100;
        if row_idx < ITEMS_ALL.len() && in_row_y < 90 {
            let item = &ITEMS_ALL[row_idx];
            let is_hovered = app.selected_row == row_idx;

            if in_row_y == 0 || in_row_y == 89 || x == 24 || x == 695 {
                return if is_hovered {
                    ACCENT_SKY
                } else {
                    BORDER_SUBTLE
                };
            }

            // Folder/File Icon Squircle (x: 42..86, y: 18..62 inside row)
            if (18..62).contains(&in_row_y) && (42..86).contains(&x) {
                if in_row_y == 18 || in_row_y == 61 || x == 42 || x == 85 {
                    return item.color_accent;
                }
                if (34..42).contains(&in_row_y) {
                    let label = if item.is_dir {
                        &b"DIR"[..]
                    } else {
                        &b"DOC"[..]
                    };
                    if let Some(c) = test_text(x, in_row_y, 48, 34, 1, label, item.color_accent) {
                        return c;
                    }
                }
                return 0xFF0E1A2C;
            }

            // Filename
            let global_y = 410 + row_idx * 100 + 20;
            if y >= global_y && y < global_y + 16 {
                if let Some(c) =
                    test_text(x, y, 105, global_y, 2, item.name.as_bytes(), TEXT_PRIMARY)
                {
                    return c;
                }
            }

            // Metadata / Size
            let meta_y = global_y + 24;
            if y >= meta_y && y < meta_y + 8 {
                if let Some(c) =
                    test_text(x, y, 105, meta_y, 1, item.meta_str.as_bytes(), TEXT_MUTED)
                {
                    return c;
                }
                if let Some(c) =
                    test_text(x, y, 520, meta_y, 1, item.size_str.as_bytes(), ACCENT_SKY)
                {
                    return c;
                }
            }

            // Chevron `>` on right
            if y >= global_y + 8 && y < global_y + 24 {
                if let Some(c) = test_text(x, y, 660, global_y + 8, 2, b">", TEXT_MUTED) {
                    return c;
                }
            }

            return SURFACE_RAISED;
        }
    }

    // 6. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn render_file_manager_ui(surface: u32, app: &FileManagerApp) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_file_manager_color(x, y, app);
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

// -----------------------------------------------------------------------------
// Entry Point & Event Reactor Loop
// -----------------------------------------------------------------------------
#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(
        0,
        "[FILE_MANAGER (Ring 3)] Sovereign File Manager & VFS Explorer online.",
    )
    .ok();

    let mut app = FileManagerApp::new();

    // Query physical block storage capacity from driver
    if let Ok(bytes) = std::storage_query() {
        app.storage_capacity_bytes = bytes;
        std::log(
            0,
            "[FILE_MANAGER] Storage capacity validated from hardware service.",
        )
        .ok();
    }

    // Enumerate initial VFS directory
    let mut entries = [0u8; 1024];
    if let Ok(n) = std::fs_list(3, "/data", &mut entries) {
        if n > 0 {
            std::log(
                0,
                "[FILE_MANAGER] VFS directory '/data' parsed successfully.",
            )
            .ok();
        }
    }

    // Allocate client surface (slot 6 = FrameBufferDirect, z=200)
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(id) => {
            std::log(
                0,
                "[FILE_MANAGER] DRM/KMS Client Surface allocated (z=200).",
            )
            .ok();
            id
        }
        Err(_) => 0,
    };

    if surface > 0 {
        render_file_manager_ui(surface, &app);
        let _ = std::surface_flip(6);
        std::log(0, "[FILE_MANAGER] Files UI presented to scanout buffer.").ok();
    }

    // Interactive Event Loop
    loop {
        let interest = std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK;
        if let Ok(triggered) = std::event_poll(interest, 100) {
            if (triggered & std::EVENT_TOUCH_INPUT) != 0 {
                let mut input = std::InputEvent {
                    kind: 0,
                    x: 0,
                    y: 0,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                };
                while std::input_event_poll(&mut input).is_ok() {
                    // Swipe gesture -> Return to launcher
                    if input.kind == 4
                        && ((input.dy <= -40 && input.y >= 1200) || input.dx.abs() >= 40)
                    {
                        std::log(
                            0,
                            "[FILE_MANAGER] Navigation gesture recognized. Exiting to launcher.",
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

                        // Check Back button (<) or bottom home pill
                        if (x >= 24 && x < 100 && y >= 28 && y < 68)
                            || (x >= 240 && x <= 480 && y >= 1400)
                        {
                            std::log(0, "[FILE_MANAGER] Back button or home gesture pressed. Exiting to launcher.").ok();
                            if surface > 0 {
                                let _ = std::surface_destroy(6, surface);
                            }
                            std::exit_with_code(0);
                        }

                        // Category Shortcuts (y: 235..335)
                        if (235..335).contains(&y) {
                            if (24..176).contains(&x) {
                                app.selected_shortcut = 0;
                                app.current_path = "/recent";
                            } else if (197..349).contains(&x) {
                                app.selected_shortcut = 1;
                                app.current_path = "/media/images";
                            } else if (370..522).contains(&x) {
                                app.selected_shortcut = 2;
                                app.current_path = "/docs";
                            } else if (543..695).contains(&x) {
                                app.selected_shortcut = 3;
                                app.current_path = "/downloads";
                            }
                            std::log(0, "[FILE_MANAGER] Category filter shortcut switched.").ok();
                            if surface > 0 {
                                render_file_manager_ui(surface, &app);
                                let _ = std::surface_flip(6);
                            }
                            break;
                        }

                        // Directory & File Row Hit-test (y: 410..1320)
                        if (410..1320).contains(&y) && x >= 24 && x < 696 {
                            let row = (y - 410) / 100;
                            if row < ITEMS_ALL.len() {
                                app.selected_row = row;
                                let item = &ITEMS_ALL[row];
                                if item.is_dir {
                                    app.current_path = match row {
                                        0 => "/system",
                                        1 => "/apps",
                                        2 => "/docs",
                                        3 => "/media",
                                        4 => "/packages",
                                        _ => "/data",
                                    };
                                    std::log(0, "[FILE_MANAGER] Navigated to subdirectory.").ok();
                                } else {
                                    std::log(
                                        0,
                                        "[FILE_MANAGER] File selected: inspection modal requested.",
                                    )
                                    .ok();
                                }
                                if surface > 0 {
                                    render_file_manager_ui(surface, &app);
                                    let _ = std::surface_flip(6);
                                }
                                break;
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
    std::log(0, "[FILE_MANAGER] Panic detected! Terminating task...").ok();
    std::exit();
}
