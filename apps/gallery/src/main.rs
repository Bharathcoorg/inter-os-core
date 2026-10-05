//! InterOS Native Sovereign Gallery & Photo Studio App
//!
//! Native Ring-3 graphical application executing in User Mode:
//! - DRM/KMS Client Surface allocation (z=200) and double-buffered scanout presentation
//! - High-contrast responsive 2-column masonry photo tile layout
//! - Photo metadata and thumbnails are populated only from real media storage; no sample photos are injected.
//!   * No sample photos are embedded; media entries come from real storage.
//! - Interactive full-screen photo inspection modal with optical telemetry
//! - Segmented Filter Tabs (All, Albums, Favorites) & Bottom Navigation Bar
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
// Photo Record Definitions
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, Copy)]
pub struct PhotoItem {
    pub title: &'static str,
    pub date: &'static str,
    pub meta: &'static str,
    pub primary_color: u32,
    pub secondary_color: u32,
}

/// Media entries, populated only from real media storage.
///
/// This is empty by design: no photo backend is bound, and nothing may be
/// invented to fill the grid. A plausible title, date and EXIF string rendered
/// as a photograph would be fabricated media state, and the procedural
/// gradients previously drawn for these entries were not images at all.
/// With no entries the grid renders an explicit empty state instead.
pub static PHOTOS: [PhotoItem; 0] = [];

/// Returns true when no real media is available to show.
#[inline]
fn has_no_media() -> bool {
    PHOTOS.is_empty()
}

pub struct GalleryApp {
    pub selected_tab: usize, // 0: All, 1: Albums, 2: Favorites
    pub bottom_nav: usize,   // 0: Photos, 1: Albums, 2: Places
    pub inspecting_photo: Option<usize>,
}

impl GalleryApp {
    pub const fn new() -> Self {
        Self {
            selected_tab: 0,
            bottom_nav: 0,
            inspecting_photo: None,
        }
    }
}

// -----------------------------------------------------------------------------
// Vector Pictogram Engines
// -----------------------------------------------------------------------------

/// Empty state large landscape picture frame icon
fn test_empty_gallery_frame(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() <= 54 && dy.abs() <= 40 {
        // Outer frame border (thickness 2)
        if dx.abs() >= 52 || dy.abs() >= 38 {
            return true;
        }
        // Sun circle at (-20, -14), radius 7
        let sun_dx = dx + 20;
        let sun_dy = dy + 14;
        if sun_dx * sun_dx + sun_dy * sun_dy <= 49 {
            return true;
        }
        // Mountain peaks
        if dy >= -4 && dy <= 34 {
            let m1 = (dx + 10).abs() * 2 <= (dy + 8) * 3;
            let m2 = (dx - 22).abs() * 2 <= (dy - 4) * 3;
            if m1 || m2 {
                return true;
            }
        }
    }
    false
}

/// Vector icon for Photos tab (small picture frame)
fn test_tab_photos_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() <= 12 && dy.abs() <= 10 {
        if dx.abs() >= 10 || dy.abs() >= 8 {
            return true;
        }
        // Sun dot
        if (dx + 5).abs() <= 2 && (dy + 3).abs() <= 2 {
            return true;
        }
        // Mountain peak
        if dy >= 0 && dy <= 6 && (dx - 2).abs() <= dy {
            return true;
        }
    }
    false
}

/// Vector icon for Albums tab (stacked album cards)
fn test_tab_albums_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    let front = (dx >= -8 && dx <= 10 && (dy == -6 || dy == 10))
        || (dy >= -6 && dy <= 10 && (dx == -8 || dx == 10));
    let back = (dx >= -12 && dx <= 6 && dy == -10)
        || (dy >= -10 && dy <= 6 && dx == -12);
    front || back
}

/// Vector icon for Places tab (map pin marker)
fn test_tab_places_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dy <= 2 {
        let r2 = dx * dx + (dy + 3) * (dy + 3);
        if r2 <= 9 * 9 && r2 >= 5 * 5 {
            return true;
        }
    } else if dy <= 11 && dx.abs() <= (11 - dy) {
        return true;
    }
    false
}

// -----------------------------------------------------------------------------
// Pixel Color Resolution Engine
// -----------------------------------------------------------------------------
#[inline(never)]
fn get_gallery_color(x: usize, y: usize, app: &GalleryApp) -> u32 {
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

        // Header Title: "Gallery"
        if y >= 36 && y < 60 {
            if let Some(c) = test_text(x, y, 120, 36, 3, b"Gallery", TEXT_PRIMARY) {
                return c;
            }
        }

        // Options kebab menu icon at (640, 48, r: 18)
        let s_dx = (x as isize) - 640;
        let s_dy = (y as isize) - 48;
        let s_r2 = s_dx * s_dx + s_dy * s_dy;
        if s_r2 <= 18 * 18 {
            let is_dot = s_dx.abs() <= 2 && (s_dy.abs() <= 2 || (s_dy.abs() >= 6 && s_dy.abs() <= 10));
            if is_dot {
                return TEXT_PRIMARY;
            }
            if s_r2 >= 16 * 16 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }

        return SURFACE_DARK;
    }

    // Modal Fullscreen Inspection View
    if let Some(idx) = app.inspecting_photo {
        let Some(photo) = PHOTOS.get(idx) else {
            return BG_DARK;
        };

        // Header Bar (y: 110..180)
        if y >= 110 && y < 180 && x >= 30 && x < 690 {
            if y >= 125 && y < 141 {
                if let Some(c) = test_text(x, y, 50, 125, 2, photo.title.as_bytes(), TEXT_PRIMARY) {
                    return c;
                }
            }
            if y >= 150 && y < 158 {
                if let Some(c) = test_text(x, y, 50, 150, 1, photo.meta.as_bytes(), ACCENT_SKY) {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }

        // Large Photo Viewport (y: 200..1100, x: 30..690)
        if y >= 200 && y < 1100 && x >= 30 && x < 690 {
            if y == 200 || y == 1099 || x == 30 || x == 689 {
                return BORDER_SUBTLE;
            }

            // Procedural landscape shader for selected photo
            let rel_y = y - 200;
            let rel_x = x - 30;
            let ratio = rel_y * 100 / 900;

            if idx == 0 {
                // Mountain sunrise gradient with horizon
                if ratio < 50 {
                    return if ratio < 25 {
                        0xFF141A28
                    } else {
                        photo.primary_color
                    };
                } else {
                    let peak = 500 + (((rel_x as i32 - 330) * (rel_x as i32 - 330)) / 450) as usize;
                    if rel_y >= peak {
                        return 0xFF080D14;
                    }
                    return photo.secondary_color;
                }
            } else if idx == 1 {
                // Cyber city grid
                if rel_x % 40 < 2 || rel_y % 40 < 2 {
                    return ACCENT_SKY;
                }
                return photo.secondary_color;
            } else {
                // Smooth celestial gradient
                if rel_x * rel_x + rel_y * rel_y < 120000 {
                    return photo.primary_color;
                }
                return photo.secondary_color;
            }
        }

        // Inspection Bottom Close Pill (x: 230..490, y: 1140..1200)
        if y >= 1140 && y < 1200 && x >= 230 && x < 490 {
            if y == 1140 || y == 1199 || x == 230 || x == 489 {
                return BORDER_SUBTLE;
            }
            if y >= 1162 && y < 1178 {
                if let Some(c) = test_text(x, y, 310, 1162, 2, b"Close", TEXT_PRIMARY) {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }

        // Home Gesture Indicator Pill
        if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
            return TEXT_MUTED;
        }
        return BG_DARK;
    }

    // 2. Segmented Filter Tabs (y: 110..160)
    if y >= 110 && y < 160 {
        let tab: Option<(usize, usize, usize, &'static [u8])> = if (30..160).contains(&x) {
            Some((0, 30, 160, &b"All"[..]))
        } else if (180..310).contains(&x) {
            Some((1, 180, 310, &b"Albums"[..]))
        } else if (330..480).contains(&x) {
            Some((2, 330, 480, &b"Favorites"[..]))
        } else {
            None
        };

        if let Some((idx, x0, x1, label)) = tab {
            let is_sel = app.selected_tab == idx;
            if y == 110 || y == 159 || x == x0 || x == x1 - 1 {
                return if is_sel { ACCENT_SKY } else { BORDER_SUBTLE };
            }
            if y >= 126 && y < 142 {
                let ox = x0 + (x1 - x0).saturating_sub(label.len() * 16) / 2;
                if let Some(c) = test_text(
                    x,
                    y,
                    ox,
                    126,
                    2,
                    label,
                    if is_sel { TEXT_PRIMARY } else { TEXT_MUTED },
                ) {
                    return c;
                }
            }
            return if is_sel { 0xFF14243A } else { SURFACE_RAISED };
        }
    }

    // 3. 2-Column Photo Tile Grid (y: 180..1300)
    let grid_top = 180;
    let tile_h = 240;
    let tile_gap = 20;

    // Empty state: no media backend is bound, so say so rather than render
    // placeholders that would read as real photographs.
    if has_no_media() && y >= grid_top + 40 && y < grid_top + 340 {
        if test_empty_gallery_frame(x, y, 360, grid_top + 130) {
            return 0xFF2A3A54;
        }
        if y >= grid_top + 240 && y < grid_top + 256 {
            if let Some(c) = test_text(x, y, 176, grid_top + 240, 2, b"No media available", TEXT_MUTED) {
                return c;
            }
        }
        if y >= grid_top + 268 && y < grid_top + 280 {
            if let Some(c) = test_text(
                x,
                y,
                140,
                grid_top + 268,
                1,
                b"Connect a media source to browse photos",
                TEXT_MUTED,
            ) {
                return c;
            }
        }
        return BG_DARK;
    }

    for row in 0..3 {
        let row_y0 = grid_top + row * (tile_h + tile_gap);
        let row_y1 = row_y0 + tile_h;

        if y >= row_y0 && y < row_y1 {
            // Col 0: (x: 30..345), Col 1: (x: 375..690)
            let col = if (30..345).contains(&x) {
                Some((0, 30, 345))
            } else if (375..690).contains(&x) {
                Some((1, 375, 690))
            } else {
                None
            };

            if let Some((c_idx, x0, x1)) = col {
                let photo_idx = row * 2 + c_idx;
                if photo_idx < PHOTOS.len() {
                    let photo = &PHOTOS[photo_idx];

                    // Tile border
                    if y == row_y0 || y == row_y1 - 1 || x == x0 || x == x1 - 1 {
                        return BORDER_SUBTLE;
                    }

                    // Scenery thumbnail gradient
                    let in_tile_y = y - row_y0;
                    let in_tile_x = x - x0;

                    // Photo Scenery View (y: row_y0 .. row_y1 - 50)
                    if in_tile_y < tile_h - 50 {
                        if in_tile_y * in_tile_y + in_tile_x * in_tile_x < 15000 {
                            return photo.primary_color;
                        }
                        if in_tile_y > tile_h / 2 && in_tile_x % 20 < 2 {
                            return photo.primary_color;
                        }
                        return photo.secondary_color;
                    }

                    // Bottom info label banner (y: row_y1 - 50 .. row_y1)
                    if y >= row_y1 - 40 && y < row_y1 - 24 {
                        if let Some(c) = test_text(
                            x,
                            y,
                            x0 + 12,
                            row_y1 - 40,
                            2,
                            photo.title.as_bytes(),
                            TEXT_PRIMARY,
                        ) {
                            return c;
                        }
                    }
                    if y >= row_y1 - 20 && y < row_y1 - 12 {
                        if let Some(c) = test_text(
                            x,
                            y,
                            x0 + 12,
                            row_y1 - 20,
                            1,
                            photo.date.as_bytes(),
                            TEXT_MUTED,
                        ) {
                            return c;
                        }
                    }

                    return SURFACE_RAISED;
                }
            }
        }
    }

    // 4. Bottom Navigation Bar (y: 1330..1400)
    if y >= 1330 && y < 1400 {
        if y == 1330 {
            return BORDER_SUBTLE;
        }
        let nav: Option<(usize, usize, usize, &'static [u8])> = if (40..220).contains(&x) {
            Some((0, 40, 220, &b"Photos"[..]))
        } else if (270..450).contains(&x) {
            Some((1, 270, 450, &b"Albums"[..]))
        } else if (500..680).contains(&x) {
            Some((2, 500, 680, &b"Places"[..]))
        } else {
            None
        };

        if let Some((idx, x0, x1, label)) = nav {
            let is_sel = app.bottom_nav == idx;
            let icon_color = if is_sel { ACCENT_SKY } else { TEXT_MUTED };

            let cx = x0 + (x1 - x0) / 2;
            let is_icon = match idx {
                0 => test_tab_photos_icon(x, y, cx, 1350),
                1 => test_tab_albums_icon(x, y, cx, 1350),
                _ => test_tab_places_icon(x, y, cx, 1350),
            };
            if is_icon {
                return icon_color;
            }

            if y >= 1368 && y < 1376 {
                let ox = cx.saturating_sub(label.len() * 4);
                if let Some(c) = test_text(x, y, ox, 1368, 1, label, icon_color) {
                    return c;
                }
            }

            // Active indicator pill at bottom
            if is_sel && y >= 1392 && y < 1396 && x >= cx - 18 && x < cx + 18 {
                return ACCENT_SKY;
            }
        }
        return SURFACE_DARK;
    }

    // 5. Home Gesture Indicator Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn render_gallery_ui(surface: u32, app: &GalleryApp) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_gallery_color(x, y, app);
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
        "[GALLERY (Ring 3)] Sovereign Gallery & Photo Studio online.",
    )
    .ok();

    let mut app = GalleryApp::new();

    // Allocate client surface (slot 6 = FrameBufferDirect, z=200)
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(id) => {
            std::log(0, "[GALLERY] DRM/KMS Client Surface allocated (z=200).").ok();
            id
        }
        Err(_) => 0,
    };

    if surface > 0 {
        render_gallery_ui(surface, &app);
        let _ = std::surface_flip(6);
        std::log(0, "[GALLERY] Gallery UI presented to scanout buffer.").ok();
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
                    // Swipe up home gesture -> Return to launcher
                    if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10
                    {
                        std::log(
                            0,
                            "[GALLERY] Navigation gesture recognized. Exiting to launcher.",
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
                            if app.inspecting_photo.is_some() {
                                app.inspecting_photo = None;
                                std::log(0, "[GALLERY] Exited photo inspection modal.").ok();
                                if surface > 0 {
                                    render_gallery_ui(surface, &app);
                                    let _ = std::surface_flip(6);
                                }
                                break;
                            } else {
                                std::log(0, "[GALLERY] Back button or home gesture pressed. Exiting to launcher.").ok();
                                if surface > 0 {
                                    let _ = std::surface_destroy(6, surface);
                                }
                                std::exit_with_code(0);
                            }
                        }

                        // If in modal, tapping Close button exits modal
                        if app.inspecting_photo.is_some() {
                            if (1140..1200).contains(&y) && (230..490).contains(&x) {
                                app.inspecting_photo = None;
                                std::log(0, "[GALLERY] Closed photo inspection modal.").ok();
                                if surface > 0 {
                                    render_gallery_ui(surface, &app);
                                    let _ = std::surface_flip(6);
                                }
                                break;
                            }
                        }

                        // Category Tabs (y: 110..160)
                        if (110..160).contains(&y) {
                            if (30..160).contains(&x) {
                                app.selected_tab = 0;
                            } else if (180..310).contains(&x) {
                                app.selected_tab = 1;
                            } else if (330..480).contains(&x) {
                                app.selected_tab = 2;
                            }
                            std::log(0, "[GALLERY] Gallery category tab switched.").ok();
                            if surface > 0 {
                                render_gallery_ui(surface, &app);
                                let _ = std::surface_flip(6);
                            }
                            break;
                        }

                        // Photo Grid Tile Taps (y: 180..1300)
                        let grid_top = 180;
                        let tile_h = 240;
                        let tile_gap = 20;
                        for row in 0..3 {
                            let row_y0 = grid_top + row * (tile_h + tile_gap);
                            let row_y1 = row_y0 + tile_h;
                            if (row_y0..row_y1).contains(&y) {
                                let col = if (30..345).contains(&x) {
                                    Some(0)
                                } else if (375..690).contains(&x) {
                                    Some(1)
                                } else {
                                    None
                                };
                                if let Some(c) = col {
                                    let p_idx = row * 2 + c;
                                    if p_idx < PHOTOS.len() {
                                        app.inspecting_photo = Some(p_idx);
                                        std::log(
                                            0,
                                            "[GALLERY] Photo tapped: inspection modal opened.",
                                        )
                                        .ok();
                                        if surface > 0 {
                                            render_gallery_ui(surface, &app);
                                            let _ = std::surface_flip(6);
                                        }
                                        break;
                                    }
                                }
                            }
                        }

                        // Bottom Navigation Tabs (y: 1330..1400)
                        if (1330..1400).contains(&y) {
                            if (40..220).contains(&x) {
                                app.bottom_nav = 0;
                            } else if (270..450).contains(&x) {
                                app.bottom_nav = 1;
                            } else if (500..680).contains(&x) {
                                app.bottom_nav = 2;
                            }
                            std::audio_haptic_click().ok();
                            if surface > 0 {
                                render_gallery_ui(surface, &app);
                                let _ = std::surface_flip(6);
                            }
                            break;
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
    std::log(0, "[GALLERY] Panic detected! Terminating task...").ok();
    std::exit();
}
