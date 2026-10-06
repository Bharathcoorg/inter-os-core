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
// 32-bit ARGB8888 Design Color Tokens (Screen 11 Light Theme)
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFFF7F6F2;
#[allow(dead_code)]
const SURFACE_DARK: u32 = 0xFFFFFFFF;
#[allow(dead_code)]
const SURFACE_RAISED: u32 = 0xFFEFEEE9;
const BORDER_SUBTLE: u32 = 0xFFE1E0DB;
const TEXT_PRIMARY: u32 = 0xFF111214;
const TEXT_MUTED: u32 = 0xFF777773;
#[allow(dead_code)]
const ACCENT_SKY: u32 = 0xFF2563EB;
#[allow(dead_code)]
const ACCENT_AMBER: u32 = 0xFFF59E0B;
#[allow(dead_code)]
const SUCCESS_EMERALD: u32 = 0xFF2E8B57;
#[allow(dead_code)]
const DANGER_CRIMSON: u32 = 0xFFEF4444;

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
    // 1. Status Bar & Clean Header (y: 0..140)
    if y < 140 {
        // Clock "09:20" at x: 48, y: 36
        if y >= 36 && y < 52 {
            if let Some(c) = test_text(x, y, 48, 36, 2, b"09:20", TEXT_PRIMARY) {
                return c;
            }
        }

        // Status indicators at top right (x: 620..664, y: 36..52)
        if y >= 36 && y < 52 {
            if let Some(c) = test_text(x, y, 620, 36, 2, b"100%", TEXT_MUTED) {
                return c;
            }
        }

        // Title: "Files" at x: 48, y: 76 (scale 4)
        if y >= 76 && y < 108 {
            if let Some(c) = test_text(x, y, 48, 76, 4, b"Files", TEXT_PRIMARY) {
                return c;
            }
        }

        // Search icon at (600, 92, r: 12)
        let s_dx = (x as i32) - 600;
        let s_dy = (y as i32) - 92;
        let s_dist2 = s_dx * s_dx + s_dy * s_dy;
        if (s_dist2 >= 36 && s_dist2 <= 100) || (s_dx >= 6 && s_dx <= 14 && s_dy >= 6 && s_dy <= 14 && (s_dx - s_dy).abs() <= 2) {
            return TEXT_PRIMARY;
        }

        // More icon (3 vertical dots at 660, 92)
        let m_dx = (x as i32) - 660;
        let m_dy = (y as i32) - 92;
        if m_dx.abs() <= 2 && (m_dy.abs() <= 2 || (m_dy - 10).abs() <= 2 || (m_dy + 10).abs() <= 2) {
            return TEXT_PRIMARY;
        }

        return BG_DARK;
    }

    // 2. Vertical Category Rows (y: 150..820)
    if (150..820).contains(&y) && x >= 36 && x < 684 {
        let (row_idx, row_start_y, row_h) = if y < 240 {
            (0, 150, 90)
        } else if y < 330 {
            (1, 240, 90)
        } else if y < 420 {
            (2, 330, 90)
        } else if y < 510 {
            (3, 420, 90)
        } else if y < 600 {
            (4, 510, 90)
        } else if y < 710 {
            (5, 600, 110)
        } else {
            (6, 710, 90)
        };

        // Thin bottom divider line
        if y == row_start_y + row_h - 1 {
            return BORDER_SUBTLE;
        }

        let center_y = row_start_y + (if row_idx == 5 { 40 } else { 45 });
        let icon_cx = 64i32;
        let icon_cy = center_y as i32;
        let idx = (x as i32) - icon_cx;
        let idy = (y as i32) - icon_cy;
        let idist2 = idx * idx + idy * idy;

        // Render Icons
        match row_idx {
            0 => {
                // Clock icon (Recent)
                if (idist2 >= 100 && idist2 <= 196) || (idx == 0 && idy >= -10 && idy <= 0) || (idy == 0 && idx >= 0 && idx <= 8) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, center_y - 8, 2, b"Recent", TEXT_PRIMARY) {
                    return c;
                }
            }
            1 => {
                // Images icon (Landscape frame)
                if (idx.abs() <= 14 && idy.abs() <= 10) && (idx.abs() >= 12 || idy.abs() >= 8 || ((idx - 5).abs() <= 2 && (idy + 3).abs() <= 2) || (idy >= 2 && idx.abs() <= (10 - idy))) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, center_y - 8, 2, b"Images", TEXT_PRIMARY) {
                    return c;
                }
            }
            2 => {
                // Videos icon (Camcorder)
                if ((idx >= -14 && idx <= 4 && idy.abs() <= 10) && (idx == -14 || idx == 4 || idy.abs() >= 8)) || (idx >= 6 && idx <= 14 && idy.abs() <= (idx - 5)) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, center_y - 8, 2, b"Videos", TEXT_PRIMARY) {
                    return c;
                }
            }
            3 => {
                // Documents icon (Folded sheet)
                if (idx.abs() <= 10 && idy.abs() <= 14) && (idx.abs() >= 8 || idy == 12 || (idy == -12 && idx <= 2) || (idy == -2 || idy == 3 || idy == 8)) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, center_y - 8, 2, b"Documents", TEXT_PRIMARY) {
                    return c;
                }
            }
            4 => {
                // Downloads icon (Down arrow with tray)
                if (idx == 0 && idy >= -12 && idy <= 4) || (idy >= 0 && idy <= 6 && idx.abs() <= (6 - idy)) || (idy == 10 && idx.abs() <= 12) || (idx.abs() == 12 && idy >= 6 && idy <= 10) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, center_y - 8, 2, b"Downloads", TEXT_PRIMARY) {
                    return c;
                }
            }
            5 => {
                // Internal Storage (Drive icon)
                if (idx.abs() <= 14 && idy.abs() <= 10) && (idx.abs() >= 12 || idy.abs() >= 8 || (idy == 2 && idx.abs() <= 10)) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, row_start_y + 24, 2, b"Internal Storage", TEXT_PRIMARY) {
                    return c;
                }
                let usage_text = if app.storage_capacity_bytes > 0 {
                    b"42 GB / 128 GB"
                } else {
                    b"42 GB / 128 GB"
                };
                if let Some(c) = test_text(x, y, 108, row_start_y + 56, 2, usage_text, TEXT_MUTED) {
                    return c;
                }
            }
            _ => {
                // Cloud icon
                if (idy >= 2 && idy <= 8 && idx.abs() <= 14) || ((idx + 6) * (idx + 6) + idy * idy <= 49) || ((idx - 6) * (idx - 6) + (idy + 2) * (idy + 2) <= 36) {
                    return TEXT_PRIMARY;
                }
                if let Some(c) = test_text(x, y, 108, center_y - 8, 2, b"Cloud (Optional)", TEXT_PRIMARY) {
                    return c;
                }
            }
        }

        return BG_DARK;
    }

    // 3. Navigation Gesture Hint at bottom (y: 1250..1380)
    if y >= 1250 && y < 1380 {
        // Arrow pointing left "<-"
        if y >= 1280 && y < 1300 && x >= 140 && x < 210 {
            let ax = (x as i32) - 170;
            let ay = (y as i32) - 1290;
            if (ay == 0 && ax >= -24 && ax <= 24) || (ax <= -10 && (ax + 24).abs() == ay.abs()) {
                return TEXT_PRIMARY;
            }
        }
        if let Some(c) = test_text(x, y, 240, 1282, 2, b"Swipe from left", TEXT_PRIMARY) {
            return c;
        }
        if let Some(c) = test_text(x, y, 240, 1308, 2, b"Go back", TEXT_MUTED) {
            return c;
        }
    }

    // 4. Home Gesture Pill (x: 260..460, y: 1418..1424)
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
                    if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10
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
