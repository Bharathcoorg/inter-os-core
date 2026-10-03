//! Inter OS Mobile Device GUI & Framebuffer Compositor
//!
//! Renders a sleek, modern smartphone device viewport, ambient AI dynamic island,
//! intent search bar, fluid capability cards (Android MicroVM, .rapp Engine),
//! system telemetry, and complete native app views directly into the 1280x720 video framebuffer.

use bootloader_api::info::{FrameBuffer, FrameBufferInfo, PixelFormat};
use spin::Mutex;

// Color Palette (RGB)
pub const COLOR_OBSIDIAN: (u8, u8, u8) = (11, 15, 25);    // #0B0F19 (Deep Obsidian)
pub const COLOR_FRAME_BG: (u8, u8, u8) = (16, 22, 34);    // #101622 (Phone Body Canvas)
pub const COLOR_BEZEL: (u8, u8, u8) = (38, 48, 68);       // #263044 (Titanium Bezel)
pub const COLOR_CARD_BG: (u8, u8, u8) = (24, 32, 48);     // #182030 (Glass Card BG)
pub const COLOR_CARD_BORDER: (u8, u8, u8) = (45, 58, 82); // #2D3A52 (Card Border)

pub const COLOR_CYAN: (u8, u8, u8) = (0, 229, 255);       // #00E5FF (Neon Cyan)
pub const COLOR_GREEN: (u8, u8, u8) = (16, 185, 129);     // #10B981 (Emerald Green)
pub const COLOR_PURPLE: (u8, u8, u8) = (168, 85, 247);    // #A855F7 (Electric Violet)
pub const COLOR_AMBER: (u8, u8, u8) = (245, 158, 11);     // #F59E0B (Amber Gold)
pub const COLOR_WHITE: (u8, u8, u8) = (248, 250, 252);    // #F8FAFC (Pure White)
pub const COLOR_MUTED: (u8, u8, u8) = (148, 163, 184);    // #94A3B8 (Slate Gray)
pub const COLOR_DARK_SLATE: (u8, u8, u8) = (5, 8, 15);    // #05080F (Terminal Drawer)
pub const COLOR_RED: (u8, u8, u8) = (239, 68, 68);        // #EF4444 (Alert Red)
pub const COLOR_BLUE: (u8, u8, u8) = (59, 130, 246);      // #3B82F6 (Electric Blue)

// Phone Frame Geometry (Centered on 1280x720 display)
pub const PHONE_X: usize = 420;
pub const PHONE_Y: usize = 12;
pub const PHONE_W: usize = 440;
pub const PHONE_H: usize = 696;

/// Active full-screen mobile application view in the bare-metal framebuffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveScreen {
    HomeScreen,
    Dialer,
    Camera,
    Settings,
    Terminal,
    Messages,
    Vault,
    Calculator,
    Browser,
    Calendar,
    Music,
}

pub const FONT_8X8: [[u8; 8]; 95] = [
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

pub struct FramebufferWriter {
    buffer_ptr: Option<*mut u8>,
    info: Option<FrameBufferInfo>,
    intent_buffer: [char; 40],
    intent_len: usize,
    last_action: &'static str,
    pub active_screen: ActiveScreen,
    pub dialed_digits: [char; 16],
    pub dialed_len: usize,
    pub in_call: bool,
    pub camera_filter: u8,
    pub camera_mode: u8,
    pub shutter_count: u32,
    pub settings_tab: u8,
    pub dvfs_governor: u8,
    pub wifi_on: bool,
    pub term_lines: [[char; 42]; 10],
    pub term_line_lens: [usize; 10],
    pub term_line_count: usize,
    pub vault_pin: [char; 4],
    pub vault_pin_len: usize,
    pub vault_unlocked: bool,
    pub music_playing: bool,
    pub music_track: u8,
}

unsafe impl Send for FramebufferWriter {}
unsafe impl Sync for FramebufferWriter {}

impl FramebufferWriter {
    pub const fn new() -> Self {
        Self {
            buffer_ptr: None,
            info: None,
            intent_buffer: ['\0'; 40],
            intent_len: 0,
            last_action: "IDLE (Ring 0 Ready)",
            active_screen: ActiveScreen::HomeScreen,
            dialed_digits: ['\0'; 16],
            dialed_len: 0,
            in_call: false,
            camera_filter: 0,
            camera_mode: 0,
            shutter_count: 0,
            settings_tab: 0,
            dvfs_governor: 0,
            wifi_on: true,
            term_lines: [['\0'; 42]; 10],
            term_line_lens: [0; 10],
            term_line_count: 0,
            vault_pin: ['\0'; 4],
            vault_pin_len: 0,
            vault_unlocked: false,
            music_playing: false,
            music_track: 0,
        }
    }

    pub fn init(&mut self, framebuffer: &'static mut FrameBuffer) {
        self.info = Some(framebuffer.info());
        self.buffer_ptr = Some(framebuffer.buffer_mut().as_mut_ptr());
        self.clear(COLOR_OBSIDIAN);
    }

    #[inline]
    pub fn set_pixel(&mut self, x: usize, y: usize, color: (u8, u8, u8)) {
        if let (Some(ptr), Some(info)) = (self.buffer_ptr, self.info) {
            if x >= info.width || y >= info.height {
                return;
            }
            let pixel_offset = (y * info.stride + x) * info.bytes_per_pixel;
            unsafe {
                let pixel_ptr = ptr.add(pixel_offset);
                match info.pixel_format {
                    PixelFormat::Rgb => {
                        *pixel_ptr = color.0;
                        *pixel_ptr.add(1) = color.1;
                        *pixel_ptr.add(2) = color.2;
                    }
                    PixelFormat::Bgr => {
                        *pixel_ptr = color.2;
                        *pixel_ptr.add(1) = color.1;
                        *pixel_ptr.add(2) = color.0;
                    }
                    PixelFormat::U8 => {
                        *pixel_ptr = (color.0 as u16 + color.1 as u16 + color.2 as u16) as u8 / 3;
                    }
                    _ => {
                        *pixel_ptr = color.2;
                        *pixel_ptr.add(1) = color.1;
                        *pixel_ptr.add(2) = color.0;
                    }
                }
            }
        }
    }

    pub fn clear(&mut self, color: (u8, u8, u8)) {
        if let Some(info) = self.info {
            self.draw_rect(0, 0, info.width, info.height, color);
        }
    }

    pub fn draw_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: (u8, u8, u8)) {
        if let Some(info) = self.info {
            let max_x = (x + w).min(info.width);
            let max_y = (y + h).min(info.height);

            for cy in y..max_y {
                for cx in x..max_x {
                    self.set_pixel(cx, cy, color);
                }
            }
        }
    }

    pub fn draw_rect_outline(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        thickness: usize,
        color: (u8, u8, u8),
    ) {
        self.draw_rect(x, y, w, thickness, color);
        self.draw_rect(x, y + h.saturating_sub(thickness), w, thickness, color);
        self.draw_rect(x, y, thickness, h, color);
        self.draw_rect(x + w.saturating_sub(thickness), y, thickness, h, color);
    }

    pub fn draw_char_scaled(
        &mut self,
        x: usize,
        y: usize,
        c: char,
        scale: usize,
        fg_color: (u8, u8, u8),
        bg_color: Option<(u8, u8, u8)>,
    ) {
        let ascii_val = c as usize;
        let glyph_idx = if (32..=126).contains(&ascii_val) {
            ascii_val - 32
        } else {
            0
        };
        let glyph = FONT_8X8[glyph_idx];

        for font_row in 0..8 {
            let row_byte = glyph[font_row];
            for font_col in 0..8 {
                let is_fg = ((row_byte >> (7 - font_col)) & 1) == 1;

                if is_fg {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            self.set_pixel(x + font_col * scale + dx, y + font_row * scale + dy, fg_color);
                        }
                    }
                } else if let Some(bg) = bg_color {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            self.set_pixel(x + font_col * scale + dx, y + font_row * scale + dy, bg);
                        }
                    }
                }
            }
        }
    }

    pub fn draw_string(
        &mut self,
        x: usize,
        y: usize,
        text: &str,
        scale: usize,
        fg: (u8, u8, u8),
        bg: Option<(u8, u8, u8)>,
    ) {
        let mut cur_x = x;
        let char_w = 8 * scale;
        for c in text.chars() {
            self.draw_char_scaled(cur_x, y, c, scale, fg, bg);
            cur_x += char_w;
        }
    }

    /// Renders the complete, photorealistic Smartphone Shell UI
    pub fn draw_mobile_shell(&mut self) {
        self.clear(COLOR_OBSIDIAN);

        // Outer Titanium Bezel
        self.draw_rect(PHONE_X - 6, PHONE_Y - 6, PHONE_W + 12, PHONE_H + 12, COLOR_BEZEL);

        self.redraw_phone();
    }

    /// Fast refresh of the smartphone inner display canvas
    pub fn redraw_phone(&mut self) {
        // Inner Display Canvas
        self.draw_rect(PHONE_X, PHONE_Y, PHONE_W, PHONE_H, COLOR_FRAME_BG);

        // Status Bar & Dynamic Island
        self.draw_status_bar();
        self.draw_dynamic_island();

        // Render Active Screen
        match self.active_screen {
            ActiveScreen::HomeScreen => self.draw_home_contents(),
            ActiveScreen::Dialer => self.draw_dialer_contents(),
            ActiveScreen::Camera => self.draw_camera_contents(),
            ActiveScreen::Settings => self.draw_settings_contents(),
            ActiveScreen::Terminal => self.draw_terminal_contents(),
            ActiveScreen::Messages => self.draw_messages_contents(),
            ActiveScreen::Vault => self.draw_vault_contents(),
            ActiveScreen::Calculator => self.draw_calculator_contents(),
            ActiveScreen::Browser => self.draw_browser_contents(),
            ActiveScreen::Calendar => self.draw_calendar_contents(),
            ActiveScreen::Music => self.draw_music_contents(),
        }

        // Bottom Gesture Home Bar
        let bar_w = 120;
        let bar_x = PHONE_X + (PHONE_W - bar_w) / 2;
        let bar_y = PHONE_Y + PHONE_H - 14;
        self.draw_rect(bar_x, bar_y, bar_w, 4, COLOR_MUTED);
    }

    #[allow(dead_code)]
    fn draw_left_sidebar(&mut self) {
        let x = 30;
        let mut y = 40;

        self.draw_string(x, y, "INTER OS v0.1", 2, COLOR_CYAN, None);
        y += 24;
        self.draw_string(x, y, "AI-Native Sovereign Microkernel", 1, COLOR_MUTED, None);
        y += 28;

        self.draw_rect(x, y, 340, 1, COLOR_CARD_BORDER);
        y += 20;

        self.draw_string(x, y, "[ CORE ARCHITECTURE ]", 1, COLOR_WHITE, None);
        y += 22;
        self.draw_string(x, y, "Target:     Smartphones & Edge", 1, COLOR_MUTED, None);
        y += 18;
        self.draw_string(x, y, "Privilege:  Ring 0 Minimal TCB", 1, COLOR_MUTED, None);
        y += 18;
        self.draw_string(x, y, "Memory:     4-Level Paging Guard", 1, COLOR_MUTED, None);
        y += 18;
        self.draw_string(x, y, "Heap:       1 MiB Dynamic Linked-List", 1, COLOR_MUTED, None);
        y += 18;
        self.draw_string(x, y, "Exceptions: Dedicated IST Stack", 1, COLOR_MUTED, None);
        y += 28;

        self.draw_rect(x, y, 340, 1, COLOR_CARD_BORDER);
        y += 20;

        self.draw_string(x, y, "[ COMPATIBILITY & APPS ]", 1, COLOR_WHITE, None);
        y += 22;
        self.draw_string(x, y, "Android:    Isolated MicroVM (ACR)", 1, COLOR_GREEN, None);
        y += 18;
        self.draw_string(x, y, "Workflows:  Gravity Store Engine", 1, COLOR_PURPLE, None);
        y += 18;
        self.draw_string(x, y, "Sandboxing: WASI + Zero-Copy IPC", 1, COLOR_CYAN, None);
        y += 18;
        self.draw_string(x, y, "Identity:   DID Keystore Vault", 1, COLOR_AMBER, None);
        y += 18;
        self.draw_string(x, y, "Network:    Per-App Zero-Trust Token", 1, COLOR_MUTED, None);
        y += 28;

        self.draw_rect(x, y, 340, 1, COLOR_CARD_BORDER);
        y += 20;

        self.draw_string(x, y, "Status: KERNEL ONLINE & VERIFIED", 1, COLOR_GREEN, None);
        y += 18;
        self.draw_string(x, y, "Raw Logs: UART COM1 (0x3F8)", 1, COLOR_MUTED, None);
    }

    #[allow(dead_code)]
    fn draw_right_sidebar(&mut self) {
        let x = 890;
        let mut y = 40;

        self.draw_string(x, y, "DEVICE CONTROLS", 2, COLOR_WHITE, None);
        y += 24;
        self.draw_string(x, y, "Interactive Hardware & Intent Bridge", 1, COLOR_MUTED, None);
        y += 28;

        self.draw_rect(x, y, 350, 1, COLOR_CARD_BORDER);
        y += 20;

        self.draw_string(x, y, "[ NATIVE APP SHORTCUTS ]", 1, COLOR_CYAN, None);
        y += 22;
        self.draw_string(x, y, "1: Settings     2: Terminal", 1, COLOR_WHITE, None);
        y += 18;
        self.draw_string(x, y, "3: Phone Dialer 4: Storage Vault", 1, COLOR_WHITE, None);
        y += 18;
        self.draw_string(x, y, "5: Sony Camera  6: Astro AI", 1, COLOR_WHITE, None);
        y += 18;
        self.draw_string(x, y, "7: Browser      8: Messages", 1, COLOR_WHITE, None);
        y += 18;
        self.draw_string(x, y, "9: Calendar     0: Music Studio", 1, COLOR_WHITE, None);
        y += 18;
        self.draw_string(x, y, "ESC / 'home':   Return Home", 1, COLOR_AMBER, None);
        y += 28;

        self.draw_rect(x, y, 350, 1, COLOR_CARD_BORDER);
        y += 20;

        self.draw_string(x, y, "[ RUNTIME TELEMETRY ]", 1, COLOR_WHITE, None);
        y += 22;
        self.draw_string(x, y, "CPU Interrupts: Enabled (IF=1)", 1, COLOR_GREEN, None);
        y += 18;
        self.draw_string(x, y, "PIC Remapping:  IRQs 32..47 Active", 1, COLOR_GREEN, None);
        y += 18;
        self.draw_string(x, y, "PIT Timer:      100 Hz Clock Online", 1, COLOR_GREEN, None);
        y += 18;
        self.draw_string(x, y, "Resolution:     1280x720 TrueColor", 1, COLOR_CYAN, None);
    }

    fn draw_status_bar(&mut self) {
        let y = PHONE_Y + 14;
        self.draw_string(PHONE_X + 22, y, "12:45", 1, COLOR_WHITE, Some(COLOR_FRAME_BG));
        self.draw_string(PHONE_X + PHONE_W - 130, y, "5G  TCB  98%", 1, COLOR_MUTED, Some(COLOR_FRAME_BG));
    }

    fn draw_dynamic_island(&mut self) {
        let island_w = 200;
        let island_h = 24;
        let island_x = PHONE_X + (PHONE_W - island_w) / 2;
        let island_y = PHONE_Y + 10;

        self.draw_rect(island_x, island_y, island_w, island_h, (18, 24, 38));
        self.draw_rect_outline(island_x, island_y, island_w, island_h, 1, COLOR_BEZEL);

        self.draw_rect(island_x + 10, island_y + 8, 8, 8, COLOR_CYAN);
        self.draw_string(island_x + 24, island_y + 8, "INTEROS * SECURE", 1, COLOR_CYAN, Some((18, 24, 38)));
    }

    pub fn draw_intent_bar(&mut self) {
        let bar_x = PHONE_X + 20;
        let bar_y = PHONE_Y + 54;
        let bar_w = PHONE_W - 40;
        let bar_h = 44;

        self.draw_rect(bar_x, bar_y, bar_w, bar_h, COLOR_CARD_BG);
        self.draw_rect_outline(bar_x, bar_y, bar_w, bar_h, 2, COLOR_CYAN);

        let text_x = bar_x + 14;
        let text_y = bar_y + 16;

        if self.intent_len == 0 {
            self.draw_string(text_x, text_y, "> Search apps, settings (1-9)...", 1, COLOR_MUTED, Some(COLOR_CARD_BG));
        } else {
            let mut prompt_str = ['\0'; 44];
            prompt_str[0] = '>';
            prompt_str[1] = ' ';
            let mut pos = 2;
            for i in 0..self.intent_len {
                prompt_str[pos] = self.intent_buffer[i];
                pos += 1;
            }
            prompt_str[pos] = '_';
            pos += 1;

            let mut char_buf = [0u8; 4];
            let mut cur_x = text_x;
            for &c in &prompt_str[..pos] {
                let s = c.encode_utf8(&mut char_buf);
                self.draw_string(cur_x, text_y, s, 1, COLOR_WHITE, Some(COLOR_CARD_BG));
                cur_x += 8;
            }
            if cur_x < bar_x + bar_w - 4 {
                self.draw_rect(cur_x, text_y, (bar_x + bar_w - 4).saturating_sub(cur_x), 12, COLOR_CARD_BG);
            }
        }
    }

    fn draw_app_icon(
        &mut self,
        col_x: usize,
        row_y: usize,
        name: &str,
        glyph: &str,
        icon_bg: (u8, u8, u8),
    ) {
        let icon_size = 46;
        let icon_x = col_x + (100 - icon_size) / 2;
        let icon_y = row_y;

        // Rounded Squircle Icon Badge
        self.draw_rect(icon_x, icon_y, icon_size, icon_size, icon_bg);
        self.draw_rect_outline(icon_x, icon_y, icon_size, icon_size, 1, COLOR_WHITE);

        // Center glyph inside icon badge
        let glyph_w = glyph.len() * 8;
        let glyph_x = icon_x + (icon_size.saturating_sub(glyph_w)) / 2;
        let glyph_y = icon_y + (icon_size - 8) / 2;
        self.draw_string(glyph_x, glyph_y, glyph, 1, COLOR_WHITE, Some(icon_bg));

        // App Name Label below icon
        let label_w = name.len() * 8;
        let label_x = col_x + (100usize.saturating_sub(label_w)) / 2;
        let label_y = icon_y + icon_size + 5;
        self.draw_string(label_x, label_y, name, 1, COLOR_WHITE, Some(COLOR_FRAME_BG));
    }

    fn draw_home_contents(&mut self) {
        self.draw_intent_bar();

        let start_x = PHONE_X + 20;
        let start_y = PHONE_Y + 114;
        let col_w = 100;
        let card_w = PHONE_W - 40;

        // Row 1: Core Apps (Settings, Phone, Messages, Camera)
        self.draw_app_icon(start_x, start_y, "Settings", "SET", COLOR_CYAN);
        self.draw_app_icon(start_x + col_w, start_y, "Phone", "TEL", COLOR_GREEN);
        self.draw_app_icon(start_x + 2 * col_w, start_y, "Messages", "MSG", COLOR_BLUE);
        self.draw_app_icon(start_x + 3 * col_w, start_y, "Camera", "CAM", COLOR_PURPLE);

        // Row 2: Media & Daily (Browser, Music, Calendar, Clock)
        let r1_y = start_y + 76;
        self.draw_app_icon(start_x, r1_y, "Browser", "WEB", COLOR_CYAN);
        self.draw_app_icon(start_x + col_w, r1_y, "Music", "SND", COLOR_AMBER);
        self.draw_app_icon(start_x + 2 * col_w, r1_y, "Calendar", "CAL", COLOR_RED);
        self.draw_app_icon(start_x + 3 * col_w, r1_y, "Clock", "CLK", (70, 80, 100));

        // Row 3: Tools & Security (Files, Calculator, Terminal, Vault)
        let r2_y = start_y + 152;
        self.draw_app_icon(start_x, r2_y, "Files", "DIR", COLOR_AMBER);
        self.draw_app_icon(start_x + col_w, r2_y, "Calc", "NUM", COLOR_CYAN);
        self.draw_app_icon(start_x + 2 * col_w, r2_y, "Terminal", "CLI", COLOR_GREEN);
        self.draw_app_icon(start_x + 3 * col_w, r2_y, "Vault", "SEC", COLOR_PURPLE);

        // Row 4: System Tools & Compatibility (Updates, Android ACR, Packages, Verified TCB)
        let r3_y = start_y + 228;
        self.draw_app_icon(start_x, r3_y, "Updates", "SYS", COLOR_CYAN);
        self.draw_app_icon(start_x + col_w, r3_y, "Android", "APK", COLOR_GREEN);
        self.draw_app_icon(start_x + 2 * col_w, r3_y, "Store", "RAPP", COLOR_PURPLE);
        self.draw_app_icon(start_x + 3 * col_w, r3_y, "Security", "TCB", (30, 41, 59));

        // Microkernel Status Card
        let status_y = start_y + 308;
        self.draw_rect(start_x, status_y, card_w, 58, COLOR_CARD_BG);
        self.draw_rect_outline(start_x, status_y, card_w, 58, 1, COLOR_CARD_BORDER);
        self.draw_rect(start_x, status_y, 4, 58, COLOR_GREEN);
        self.draw_string(start_x + 14, status_y + 10, "[ INTEROS CORE MICROKERNEL ]", 1, COLOR_GREEN, Some(COLOR_CARD_BG));
        self.draw_string(start_x + 14, status_y + 24, "Zero Ambient Authority * CR3 Isolated", 1, COLOR_MUTED, Some(COLOR_CARD_BG));
        self.draw_string(start_x + 14, status_y + 38, self.last_action, 1, COLOR_AMBER, Some(COLOR_CARD_BG));

        // Bottom Frosted Glass Dock Shelf
        let dock_y = PHONE_Y + PHONE_H - 96;
        let dock_h = 74;
        self.draw_rect(start_x, dock_y, card_w, dock_h, (20, 28, 42));
        self.draw_rect_outline(start_x, dock_y, card_w, dock_h, 1, COLOR_CARD_BORDER);

        // 4 Quick-Launch Dock Icons
        self.draw_app_icon(start_x, dock_y + 6, "Phone", "TEL", COLOR_GREEN);
        self.draw_app_icon(start_x + col_w, dock_y + 6, "Messages", "MSG", COLOR_BLUE);
        self.draw_app_icon(start_x + 2 * col_w, dock_y + 6, "Browser", "WEB", COLOR_CYAN);
        self.draw_app_icon(start_x + 3 * col_w, dock_y + 6, "Camera", "CAM", COLOR_PURPLE);
    }

    fn draw_dialer_contents(&mut self) {
        let x = PHONE_X + 20;
        let w = PHONE_W - 40;
        let mut y = PHONE_Y + 54;

        self.draw_string(x, y, "PHONE & DIALER", 1, COLOR_GREEN, Some(COLOR_FRAME_BG));
        self.draw_string(x + w - 144, y, "5G VoNR | RSSI 31", 1, COLOR_MUTED, Some(COLOR_FRAME_BG));
        y += 24;

        let box_h = 44;
        self.draw_rect(x, y, w, box_h, COLOR_CARD_BG);
        self.draw_rect_outline(x, y, w, box_h, 2, if self.in_call { COLOR_GREEN } else { COLOR_CYAN });

        let mut digits_str = ['\0'; 20];
        let mut dpos = 0;
        if self.dialed_len == 0 && !self.in_call {
            self.draw_string(x + 14, y + 16, "> Type digits or tap keypad...", 1, COLOR_MUTED, Some(COLOR_CARD_BG));
        } else if self.in_call {
            self.draw_string(x + 14, y + 16, "[ CALL CONNECTED: 5G VoNR ]", 1, COLOR_GREEN, Some(COLOR_CARD_BG));
        } else {
            for i in 0..self.dialed_len {
                digits_str[dpos] = self.dialed_digits[i];
                dpos += 1;
            }
            digits_str[dpos] = '_';
            dpos += 1;

            let mut char_buf = [0u8; 4];
            let mut cur_x = x + 14;
            for &c in &digits_str[..dpos] {
                let s = c.encode_utf8(&mut char_buf);
                self.draw_string(cur_x, y + 16, s, 1, COLOR_WHITE, Some(COLOR_CARD_BG));
                cur_x += 10;
            }
        }
        y += 56;

        let key_labels = [
            ["1", "2 ABC", "3 DEF"],
            ["4 GHI", "5 JKL", "6 MNO"],
            ["7 PQRS", "8 TUV", "9 WXYZ"],
            ["*", "0 +", "#"],
        ];

        let kw = (w - 24) / 3;
        let kh = 42;
        for row in 0..4 {
            let ry = y + row * (kh + 8);
            for col in 0..3 {
                let kx = x + col * (kw + 12);
                self.draw_rect(kx, ry, kw, kh, COLOR_CARD_BG);
                self.draw_rect_outline(kx, ry, kw, kh, 1, COLOR_CARD_BORDER);
                let label = key_labels[row][col];
                let lx = kx + (kw - label.len() * 8) / 2;
                self.draw_string(lx, ry + 16, label, 1, COLOR_WHITE, Some(COLOR_CARD_BG));
            }
        }
        y += 4 * (kh + 8) + 10;

        let btn_w = (w - 16) / 2;
        let btn_h = 44;

        self.draw_rect(x, y, btn_w, btn_h, COLOR_GREEN);
        self.draw_string(x + (btn_w - 64) / 2, y + 16, "[ CALL ]", 1, COLOR_WHITE, Some(COLOR_GREEN));

        let end_x = x + btn_w + 16;
        self.draw_rect(end_x, y, btn_w, btn_h, COLOR_RED);
        self.draw_string(end_x + (btn_w - 80) / 2, y + 16, "[ END CALL ]", 1, COLOR_WHITE, Some(COLOR_RED));
        y += btn_h + 16;

        self.draw_rect(x, y, w, 110, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w, 110, 1, COLOR_CARD_BORDER);
        let mut cy = y + 10;
        self.draw_string(x + 12, cy, "[ VERIFIED CONTACTS DIRECTORY ]", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
        cy += 20;
        self.draw_string(x + 12, cy, "* Alice (Agent Swarm)     +1 555-0101", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
        cy += 18;
        self.draw_string(x + 12, cy, "* Bob (Kernel Security)    +1 555-0102", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
        cy += 18;
        self.draw_string(x + 12, cy, "* Emergency E911           911 Direct Fast-Path", 1, COLOR_AMBER, Some(COLOR_DARK_SLATE));
    }

    fn draw_camera_contents(&mut self) {
        let x = PHONE_X + 20;
        let w = PHONE_W - 40;
        let mut y = PHONE_Y + 54;

        self.draw_string(x, y, "SONY IMX CAMERA & NEURAL ISP", 1, COLOR_CYAN, Some(COLOR_FRAME_BG));
        self.draw_string(x + w - 104, y, "30 FPS | 1080p", 1, COLOR_MUTED, Some(COLOR_FRAME_BG));
        y += 24;

        let vf_h = 240;
        self.draw_rect(x, y, w, vf_h, (8, 12, 20));
        self.draw_rect_outline(x, y, w, vf_h, 2, COLOR_BEZEL);

        let gx1 = x + w / 3;
        let gx2 = x + (2 * w) / 3;
        let gy1 = y + vf_h / 3;
        let gy2 = y + (2 * vf_h) / 3;
        self.draw_rect(gx1, y + 4, 1, vf_h - 8, (30, 42, 60));
        self.draw_rect(gx2, y + 4, 1, vf_h - 8, (30, 42, 60));
        self.draw_rect(x + 4, gy1, w - 8, 1, (30, 42, 60));
        self.draw_rect(x + 4, gy2, w - 8, 1, (30, 42, 60));

        let cx = x + w / 2;
        let cy = y + vf_h / 2;
        self.draw_rect(cx - 16, cy - 16, 32, 2, COLOR_CYAN);
        self.draw_rect(cx - 16, cy + 14, 32, 2, COLOR_CYAN);
        self.draw_rect(cx - 16, cy - 16, 2, 32, COLOR_CYAN);
        self.draw_rect(cx + 14, cy - 16, 2, 32, COLOR_CYAN);

        let filter_names = ["NORMAL", "MONOCHROME", "SEPIA", "NEURAL HEATMAP", "HI-CONTRAST"];
        let fname = filter_names[(self.camera_filter as usize) % filter_names.len()];
        self.draw_string(x + 12, y + vf_h - 22, fname, 1, COLOR_CYAN, Some((8, 12, 20)));

        let mode_names = ["PHOTO", "VIDEO", "NIGHT", "DOC SCAN", "QR CODE"];
        let mname = mode_names[(self.camera_mode as usize) % mode_names.len()];
        self.draw_string(x + w - mname.len() * 8 - 14, y + vf_h - 22, mname, 1, COLOR_AMBER, Some((8, 12, 20)));
        y += vf_h + 14;

        self.draw_string(x, y, "EXP: 1/120s   ISO: 100   f/1.8   AF-LOCK: OK", 1, COLOR_MUTED, Some(COLOR_FRAME_BG));
        y += 24;

        self.draw_string(x, y, "MODES: [Photo] [Video] [Night] [Doc] [QR]", 1, COLOR_WHITE, Some(COLOR_FRAME_BG));
        y += 24;

        let sbtn_w = 160;
        let sbtn_h = 44;
        let sbtn_x = x + (w - sbtn_w) / 2;
        self.draw_rect(sbtn_x, y, sbtn_w, sbtn_h, COLOR_WHITE);
        self.draw_string(sbtn_x + (sbtn_w - 72) / 2, y + 16, "[ SHUTTER ]", 1, (10, 15, 25), Some(COLOR_WHITE));
        y += sbtn_h + 16;

        self.draw_rect(x, y, w, 86, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w, 86, 1, COLOR_CARD_BORDER);
        let mut sy = y + 10;
        self.draw_string(x + 12, sy, "[ HARDWARE SENSOR & STORAGE ]", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
        sy += 20;
        let mut cnt_buf = [b'0'; 6];
        let mut val = self.shutter_count;
        for i in (0..6).rev() {
            cnt_buf[i] = b'0' + (val % 10) as u8;
            val /= 10;
        }
        let cnt_str = core::str::from_utf8(&cnt_buf).unwrap_or("000000");
        let mut line_buf = [0u8; 40];
        let pfx = b"Photos Captured: ";
        line_buf[..pfx.len()].copy_from_slice(pfx);
        line_buf[pfx.len()..pfx.len() + 6].copy_from_slice(cnt_str.as_bytes());
        let sfx = b" -> Sector 201";
        line_buf[pfx.len() + 6..pfx.len() + 6 + sfx.len()].copy_from_slice(sfx);
        let photo_line = core::str::from_utf8(&line_buf[..pfx.len() + 6 + sfx.len()]).unwrap_or("");
        self.draw_string(x + 12, sy, photo_line, 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
        sy += 18;
        self.draw_string(x + 12, sy, "Zero-Copy DMA: Stream online via SHM Pipe 0", 1, COLOR_MUTED, Some(COLOR_DARK_SLATE));
    }

    fn draw_settings_contents(&mut self) {
        let x = PHONE_X + 20;
        let w = PHONE_W - 40;
        let mut y = PHONE_Y + 54;

        self.draw_string(x, y, "SYSTEM PREFERENCES & HAL", 1, COLOR_CYAN, Some(COLOR_FRAME_BG));
        self.draw_string(x + w - 112, y, "Inter OS v3.4", 1, COLOR_MUTED, Some(COLOR_FRAME_BG));
        y += 24;

        let tabs = ["Disp", "Net", "Power", "Batt", "Sec"];
        let tw = (w - 16) / 5;
        for i in 0..5 {
            let tx = x + i * (tw + 4);
            let is_active = self.settings_tab == (i as u8);
            self.draw_rect(tx, y, tw, 28, if is_active { COLOR_CYAN } else { COLOR_CARD_BG });
            let label = tabs[i];
            let lx = tx + (tw - label.len() * 8) / 2;
            self.draw_string(lx, y + 8, label, 1, if is_active { (10, 15, 25) } else { COLOR_WHITE }, Some(if is_active { COLOR_CYAN } else { COLOR_CARD_BG }));
        }
        y += 40;

        self.draw_rect(x, y, w, 440, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w, 440, 1, COLOR_CARD_BORDER);

        let mut py = y + 16;
        match self.settings_tab {
            0 => {
                self.draw_string(x + 14, py, "[ 1. DISPLAY & AUDIO PREFERENCES ]", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
                py += 26;
                self.draw_string(x + 14, py, "Screen Brightness: 85%", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
                py += 18;
                self.draw_string(x + 14, py, "[=============================---]", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
                py += 26;
                self.draw_string(x + 14, py, "Media Audio Volume: 70%", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
                py += 18;
                self.draw_string(x + 14, py, "[========================--------]", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
                py += 26;
                self.draw_string(x + 14, py, "* Refresh Rate: 120Hz ProMotion [ACTIVE]", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
                py += 20;
                self.draw_string(x + 14, py, "* Color Profile: OLED TrueBlack Native", 1, COLOR_MUTED, Some(COLOR_DARK_SLATE));
                py += 20;
                self.draw_string(x + 14, py, "* Auto-Rotate: Portrait Locked", 1, COLOR_MUTED, Some(COLOR_DARK_SLATE));
            }
            1 => {
                self.draw_string(x + 14, py, "[ 2. WIRELESS & CONNECTIVITY ]", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
                py += 26;
                self.draw_string(x + 14, py, if self.wifi_on { "Wi-Fi 6E: [ENABLED]" } else { "Wi-Fi 6E: [DISABLED]" }, 1, if self.wifi_on { COLOR_GREEN } else { COLOR_AMBER }, Some(COLOR_DARK_SLATE));
                py += 20;
                self.draw_string(x + 14, py, "  SSID: InterSovereignMesh-5G", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
                py += 18;
                self.draw_string(x + 14, py, "Wi-Fi: no wireless hardware discovered", 1, COLOR_MUTED, Some(COLOR_DARK_SLATE));
                py += 26;
                self.draw_string(x + 14, py, "Bluetooth: no controller discovered", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
                py += 20;
                self.draw_string(x + 14, py, "Cellular: no modem discovered", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
                py += 18;
                self.draw_string(x + 14, py, "  Signal: unavailable", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
            }
            2 => {
                self.draw_string(x + 14, py, "[ 3. MOBILE DVFS POWER GOVERNOR ]", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
                py += 26;
                let g0 = self.dvfs_governor == 0;
                let g1 = self.dvfs_governor == 1;
                let g2 = self.dvfs_governor == 2;
                self.draw_string(x + 14, py, if g0 { "[*] Performance Mode: 2400 MHz (Max Compute)" } else { "[ ] Performance Mode: 2400 MHz (Max Compute)" }, 1, if g0 { COLOR_GREEN } else { COLOR_MUTED }, Some(COLOR_DARK_SLATE));
                py += 24;
                self.draw_string(x + 14, py, if g1 { "[*] Balanced Mode:    1200 MHz (Dynamic Clock)" } else { "[ ] Balanced Mode:    1200 MHz (Dynamic Clock)" }, 1, if g1 { COLOR_GREEN } else { COLOR_MUTED }, Some(COLOR_DARK_SLATE));
                py += 24;
                self.draw_string(x + 14, py, if g2 { "[*] Power Saver Mode:  600 MHz (Extended Battery)" } else { "[ ] Power Saver Mode:  600 MHz (Extended Battery)" }, 1, if g2 { COLOR_GREEN } else { COLOR_MUTED }, Some(COLOR_DARK_SLATE));
                py += 28;
                self.draw_string(x + 14, py, "Dynamic Frequency Scaling: Hardware P-States Active", 1, COLOR_MUTED, Some(COLOR_DARK_SLATE));
            }
            3 => {
                self.draw_string(x + 14, py, "[ 4. BATTERY FUEL GAUGE & THERMALS ]", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
                py += 26;
                self.draw_string(x + 14, py, "State of Charge: 94% (4200 mAh Capacity)", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
                py += 20;
                self.draw_string(x + 14, py, "Battery Voltage: 3850 mV | Drain: 2990 mW", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
                py += 20;
                self.draw_string(x + 14, py, "Health: Excellent (100% Nominal Cycles)", 1, COLOR_MUTED, Some(COLOR_DARK_SLATE));
                py += 26;
                self.draw_string(x + 14, py, "SoC Temperature: 42 C (Zone: Normal)", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
                py += 20;
                self.draw_string(x + 14, py, "Thermal Throttling: Inactive (Headroom: 38C)", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
            }
            _ => {
                self.draw_string(x + 14, py, "[ 5. HARDWARE ROOT OF TRUST & KEYSTORE ]", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
                py += 26;
                self.draw_string(x + 14, py, "* Keystore Slot 0: Root Master Key [ONLINE]", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
                py += 20;
                self.draw_string(x + 14, py, "* Keystore Slot 1: Storage Key (AES-256-XTS)", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
                py += 20;
                self.draw_string(x + 14, py, "* Keystore Slot 3: Wire P2P Session Key", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
                py += 24;
                self.draw_string(x + 14, py, "W3C DID: did:inter:sovereign:node01", 1, COLOR_AMBER, Some(COLOR_DARK_SLATE));
                py += 20;
                self.draw_string(x + 14, py, "Merkle State Root: Attestation Verified", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
            }
        }

        let fy = y + 400;
        self.draw_string(x + 14, fy, "Press 1-5 to switch tabs | 'w' wifi | 'g' dvfs", 1, COLOR_MUTED, Some(COLOR_DARK_SLATE));
    }

    fn draw_terminal_contents(&mut self) {
        let x = PHONE_X + 20;
        let w = PHONE_W - 40;
        let mut y = PHONE_Y + 54;

        self.draw_string(x, y, "SOVEREIGN INTENT TERMINAL", 1, COLOR_PURPLE, Some(COLOR_FRAME_BG));
        self.draw_string(x + w - 104, y, "Ring 3 Shell", 1, COLOR_MUTED, Some(COLOR_FRAME_BG));
        y += 24;

        let box_h = 440;
        self.draw_rect(x, y, w, box_h, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w, box_h, 1, COLOR_CARD_BORDER);

        let mut ty = y + 12;
        self.draw_string(x + 14, ty, "Inter OS Sovereign Shell v3.4 (x86_64)", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
        ty += 18;
        self.draw_string(x + 14, ty, "Type 'help' for commands, 'exit' for home.", 1, COLOR_MUTED, Some(COLOR_DARK_SLATE));
        ty += 24;

        let start_idx = if self.term_line_count > 6 { self.term_line_count - 6 } else { 0 };
        for i in start_idx..self.term_line_count {
            let line_len = self.term_line_lens[i % 10];
            let mut char_buf = [0u8; 4];
            let mut cur_x = x + 14;
            for j in 0..line_len {
                let c = self.term_lines[i % 10][j];
                let s = c.encode_utf8(&mut char_buf);
                self.draw_string(cur_x, ty, s, 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
                cur_x += 8;
            }
            ty += 18;
        }

        let mut prompt_str = ['\0'; 44];
        let pfx = b"interos@sovereign:~$ ";
        let mut pos = 0;
        for &b in pfx {
            prompt_str[pos] = b as char;
            pos += 1;
        }
        for i in 0..self.intent_len {
            prompt_str[pos] = self.intent_buffer[i];
            pos += 1;
        }
        prompt_str[pos] = '_';
        pos += 1;

        let mut char_buf = [0u8; 4];
        let mut cur_x = x + 14;
        for &c in &prompt_str[..pos] {
            let s = c.encode_utf8(&mut char_buf);
            self.draw_string(cur_x, ty, s, 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
            cur_x += 8;
        }

        let ref_y = y + box_h + 16;
        self.draw_string(x, ref_y, "Commands: help, status, intent, plan, ai, dma, reboot", 1, COLOR_MUTED, Some(COLOR_FRAME_BG));
    }

    fn draw_messages_contents(&mut self) {
        let x = PHONE_X + 20;
        let w = PHONE_W - 40;
        let mut y = PHONE_Y + 54;

        self.draw_string(x, y, "SOVEREIGN P2P MESSAGES", 1, COLOR_GREEN, Some(COLOR_FRAME_BG));
        self.draw_string(x + w - 144, y, "ChaCha20-Poly1305", 1, COLOR_MUTED, Some(COLOR_FRAME_BG));
        y += 24;

        self.draw_rect(x, y, w, 36, COLOR_CARD_BG);
        self.draw_string(x + 14, y + 12, "Conversation: Alice (Agent Swarm Mesh)", 1, COLOR_WHITE, Some(COLOR_CARD_BG));
        y += 46;

        self.draw_rect(x, y, w - 40, 52, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w - 40, 52, 1, COLOR_CARD_BORDER);
        self.draw_string(x + 12, y + 10, "Alice [10:14 UTC]:", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
        self.draw_string(x + 12, y + 28, "Sovereign sector 100 verified intact.", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
        y += 64;

        let out_x = x + 40;
        self.draw_rect(out_x, y, w - 40, 52, (20, 48, 36));
        self.draw_rect_outline(out_x, y, w - 40, 52, 1, COLOR_GREEN);
        self.draw_string(out_x + 12, y + 10, "You [10:15 UTC]:", 1, COLOR_GREEN, Some((20, 48, 36)));
        self.draw_string(out_x + 12, y + 28, "Microkernel status check complete.", 1, COLOR_WHITE, Some((20, 48, 36)));
        y += 64;

        self.draw_rect(x, y, w - 40, 52, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w - 40, 52, 1, COLOR_CARD_BORDER);
        self.draw_string(x + 12, y + 10, "Alice [10:15 UTC]:", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
        self.draw_string(x + 12, y + 28, "All 26 Ring 3 tasks responsive.", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
        y += 74;

        self.draw_rect(x, y, w, 68, COLOR_CARD_BG);
        self.draw_rect_outline(x, y, w, 68, 1, COLOR_CARD_BORDER);
        self.draw_string(x + 12, y + 12, "[ ZERO-TRUST HARDWARE ENCRYPTION ]", 1, COLOR_CYAN, Some(COLOR_CARD_BG));
        self.draw_string(x + 12, y + 30, "* Hardware Keystore Slot 3 Session Key: Active", 1, COLOR_GREEN, Some(COLOR_CARD_BG));
        self.draw_string(x + 12, y + 48, "* Zero Cloud Relay: Direct Peer-to-Peer Wire", 1, COLOR_MUTED, Some(COLOR_CARD_BG));
        y += 84;

        self.draw_rect(x, y, w - 70, 42, COLOR_CARD_BG);
        self.draw_rect_outline(x, y, w - 70, 42, 1, COLOR_CARD_BORDER);
        self.draw_string(x + 12, y + 16, "Type encrypted message...", 1, COLOR_MUTED, Some(COLOR_CARD_BG));

        let send_x = x + w - 60;
        self.draw_rect(send_x, y, 60, 42, COLOR_GREEN);
        self.draw_string(send_x + 12, y + 16, "Send", 1, COLOR_WHITE, Some(COLOR_GREEN));
    }

    fn draw_vault_contents(&mut self) {
        let x = PHONE_X + 20;
        let w = PHONE_W - 40;
        let mut y = PHONE_Y + 54;

        self.draw_string(x, y, "SOVEREIGN STORAGE VAULT", 1, COLOR_AMBER, Some(COLOR_FRAME_BG));
        self.draw_string(x + w - 128, y, "6 Sectors Sealed", 1, COLOR_MUTED, Some(COLOR_FRAME_BG));
        y += 24;

        self.draw_rect(x, y, w, 180, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w, 180, 1, COLOR_CARD_BORDER);
        let mut sy = y + 12;
        self.draw_string(x + 14, sy, "[ AUTHENTICATED STORAGE SECTORS ]", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
        sy += 22;
        self.draw_string(x + 14, sy, "* Sector 100: Root Keystore       [AUTHENTICATED]", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
        sy += 18;
        self.draw_string(x + 14, sy, "* Sector 200: Media Containers    [AUTHENTICATED]", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
        sy += 18;
        self.draw_string(x + 14, sy, "* Sector 201: Camera Photos       [AUTHENTICATED]", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
        sy += 18;
        self.draw_string(x + 14, sy, "* Sector 202: Encrypted Messages  [AUTHENTICATED]", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
        sy += 18;
        self.draw_string(x + 14, sy, "* Sector 203: Calendar Events     [AUTHENTICATED]", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
        sy += 18;
        self.draw_string(x + 14, sy, "* Sector 204: Music Audio Tracks  [AUTHENTICATED]", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
        y += 194;

        self.draw_rect(x, y, w, 120, COLOR_CARD_BG);
        self.draw_rect_outline(x, y, w, 120, 1, COLOR_CARD_BORDER);
        let mut py = y + 14;
        self.draw_string(x + 14, py, "[ BIOMETRIC & PIN AUTHENTICATION ]", 1, COLOR_AMBER, Some(COLOR_CARD_BG));
        py += 24;
        self.draw_string(x + 14, py, if self.vault_unlocked { "Vault Status: UNLOCKED (Keys Accessible)" } else { "Vault Status: SEALED (Enter PIN 1234)" }, 1, if self.vault_unlocked { COLOR_GREEN } else { COLOR_AMBER }, Some(COLOR_CARD_BG));
        py += 24;
        self.draw_string(x + 14, py, "PIN Input: [ * * * * ] (Press 1-4 to unlock)", 1, COLOR_WHITE, Some(COLOR_CARD_BG));
        y += 134;

        self.draw_rect(x, y, w, 100, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w, 100, 1, COLOR_CARD_BORDER);
        let mut by = y + 12;
        self.draw_string(x + 14, by, "[ SOVEREIGN BACKUP EXPORT ]", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
        by += 22;
        self.draw_string(x + 14, by, "Frame: 'INTER_SOVEREIGN_VAULT_BACKUP'", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
        by += 18;
        self.draw_string(x + 14, by, "Fletcher-32 Integrity Checksum: VALID [OK]", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
        by += 18;
        self.draw_string(x + 14, by, "Encryption: ChaCha20 + AES-256-XTS", 1, COLOR_MUTED, Some(COLOR_DARK_SLATE));
    }

    fn draw_calculator_contents(&mut self) {
        let x = PHONE_X + 20;
        let w = PHONE_W - 40;
        let mut y = PHONE_Y + 54;

        self.draw_string(x, y, "TOUCH CALCULATOR", 1, COLOR_CYAN, Some(COLOR_FRAME_BG));
        self.draw_string(x + w - 112, y, "v1.0.0 Native", 1, COLOR_MUTED, Some(COLOR_FRAME_BG));
        y += 24;

        // Display Area (formula + result readout)
        self.draw_rect(x, y, w, 70, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w, 70, 1, COLOR_CARD_BORDER);
        self.draw_string(x + 14, y + 12, "1,024.50 + 256.00", 1, COLOR_MUTED, Some(COLOR_DARK_SLATE));
        self.draw_string(x + 14, y + 36, "= 1,280.50", 2, COLOR_WHITE, Some(COLOR_DARK_SLATE));
        y += 82;

        // Keypad Grid: 4 columns x 5 rows
        let btn_w = (w - 18) / 4;
        let btn_h = 36;
        let gap = 6;

        let rows = [
            [("C", COLOR_RED), ("+/-", (40, 52, 75)), ("%", (40, 52, 75)), ("/", COLOR_AMBER)],
            [("7", COLOR_CARD_BG), ("8", COLOR_CARD_BG), ("9", COLOR_CARD_BG), ("*", COLOR_AMBER)],
            [("4", COLOR_CARD_BG), ("5", COLOR_CARD_BG), ("6", COLOR_CARD_BG), ("-", COLOR_AMBER)],
            [("1", COLOR_CARD_BG), ("2", COLOR_CARD_BG), ("3", COLOR_CARD_BG), ("+", COLOR_AMBER)],
            [("0", COLOR_CARD_BG), (".", COLOR_CARD_BG), ("DEL", (40, 52, 75)), ("=", COLOR_GREEN)],
        ];

        for row in &rows {
            let mut bx = x;
            for &(label, bg) in row {
                self.draw_rect(bx, y, btn_w, btn_h, bg);
                self.draw_rect_outline(bx, y, btn_w, btn_h, 1, COLOR_CARD_BORDER);
                let lw = label.len() * 8;
                let lx = bx + (btn_w.saturating_sub(lw)) / 2;
                let ly = y + (btn_h.saturating_sub(8)) / 2;
                self.draw_string(lx, ly, label, 1, COLOR_WHITE, Some(bg));
                bx += btn_w + gap;
            }
            y += btn_h + gap;
        }

        y += 12;
        self.draw_rect(x, y, w, 84, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w, 84, 1, COLOR_CARD_BORDER);
        let mut iy = y + 10;
        self.draw_string(x + 14, iy, "[ FLOATING-POINT ARITHMETIC ENGINE ]", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
        iy += 22;
        self.draw_string(x + 14, iy, "* IEEE 754 64-bit Precision | Ring 3 User Space", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
        iy += 18;
        self.draw_string(x + 14, iy, "* Zero Cloud Telemetry | 100% Offline Math", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
        iy += 18;
        self.draw_string(x + 14, iy, "* Package: org.interos.calculator (Attested)", 1, COLOR_MUTED, Some(COLOR_DARK_SLATE));
    }

    fn draw_browser_contents(&mut self) {
        let x = PHONE_X + 20;
        let w = PHONE_W - 40;
        let mut y = PHONE_Y + 54;

        self.draw_string(x, y, "DECENTRALIZED BROWSER", 1, COLOR_CYAN, Some(COLOR_FRAME_BG));
        self.draw_string(x + w - 120, y, "Privacy Shield", 1, COLOR_MUTED, Some(COLOR_FRAME_BG));
        y += 24;

        self.draw_rect(x, y, w, 40, COLOR_CARD_BG);
        self.draw_rect_outline(x, y, w, 40, 1, COLOR_CYAN);
        self.draw_string(x + 12, y + 14, "interlayer://mesh.sovereign.net/docs", 1, COLOR_WHITE, Some(COLOR_CARD_BG));
        y += 52;

        self.draw_rect(x, y, w, 90, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w, 90, 1, COLOR_CARD_BORDER);
        let mut py = y + 12;
        self.draw_string(x + 14, py, "[ PRIVACY SHIELD ENFORCEMENT ]", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
        py += 22;
        self.draw_string(x + 14, py, "* Trackers Stripped: 14 blocked | PII Leaks: 0", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
        py += 18;
        self.draw_string(x + 14, py, "* Encrypted DNS: DoH Active (DNSSEC Verified)", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
        py += 18;
        self.draw_string(x + 14, py, "* Fingerprint Defense: Noise Injection Active", 1, COLOR_MUTED, Some(COLOR_DARK_SLATE));
        y += 104;

        self.draw_rect(x, y, w, 320, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w, 320, 1, COLOR_CARD_BORDER);
        let mut vy = y + 16;
        self.draw_string(x + 14, vy, "INTER OS BROWSER SANDBOX", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
        vy += 26;
        self.draw_string(x + 14, vy, "Decentralized peer-to-peer web runtime.", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
        vy += 20;
        self.draw_string(x + 14, vy, "Hardware capabilities isolated from DOM.", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
        vy += 20;
        self.draw_string(x + 14, vy, "Gravity Glass physics & fluid scrolling.", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
        vy += 20;
        self.draw_string(x + 14, vy, "No third-party tracking scripts permitted.", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
        vy += 30;
        self.draw_string(x + 14, vy, "* Protocol: InterOS IPC Bus", 1, COLOR_MUTED, Some(COLOR_DARK_SLATE));
        vy += 20;
        self.draw_string(x + 14, vy, "* Content Digest: SHA-256 Validated", 1, COLOR_MUTED, Some(COLOR_DARK_SLATE));
    }

    fn draw_calendar_contents(&mut self) {
        let x = PHONE_X + 20;
        let w = PHONE_W - 40;
        let mut y = PHONE_Y + 54;

        self.draw_string(x, y, "CHRONO-PLANNER & CALENDAR", 1, COLOR_PURPLE, Some(COLOR_FRAME_BG));
        self.draw_string(x + w - 120, y, "September 2026", 1, COLOR_MUTED, Some(COLOR_FRAME_BG));
        y += 24;

        self.draw_rect(x, y, w, 220, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w, 220, 1, COLOR_CARD_BORDER);

        let mut gy = y + 14;
        self.draw_string(x + 18, gy, "Su   Mo   Tu   We   Th   Fr   Sa", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
        gy += 24;

        let cal_rows = [
            "      1    2    3    4    5    6",
            " 7    8    9   10   11   12   13",
            "14   15   16   17   18   19 [20]",
            "21   22   23   24   25   26   27",
            "28   29   30",
        ];
        for row in cal_rows {
            self.draw_string(x + 18, gy, row, 1, if row.contains("[20]") { COLOR_CYAN } else { COLOR_WHITE }, Some(COLOR_DARK_SLATE));
            gy += 22;
        }
        y += 234;

        self.draw_rect(x, y, w, 52, COLOR_CARD_BG);
        self.draw_rect_outline(x, y, w, 52, 1, COLOR_CARD_BORDER);
        self.draw_string(x + 14, y + 12, "Hardware RTC: 2026-09-20 18:53:29 UTC", 1, COLOR_GREEN, Some(COLOR_CARD_BG));
        self.draw_string(x + 14, y + 30, "Date Math: Gregorian civil leap year (Zeller OK)", 1, COLOR_MUTED, Some(COLOR_CARD_BG));
        y += 66;

        self.draw_rect(x, y, w, 180, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w, 180, 1, COLOR_CARD_BORDER);
        let mut ay = y + 14;
        self.draw_string(x + 14, ay, "[ TODAY'S VERIFIED AGENDA ]", 1, COLOR_AMBER, Some(COLOR_DARK_SLATE));
        ay += 24;
        self.draw_string(x + 14, ay, "* 09:00 UTC - Microkernel Verification & Ring 3", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
        ay += 20;
        self.draw_string(x + 14, ay, "* 14:30 UTC - System Package Check", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
        ay += 20;
        self.draw_string(x + 14, ay, "* 18:00 UTC - Sovereign P2P Mesh Synchronization", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
        ay += 24;
        self.draw_string(x + 14, ay, "Agenda Sector 203: Authenticated & Sealed [OK]", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
    }

    fn draw_music_contents(&mut self) {
        let x = PHONE_X + 20;
        let w = PHONE_W - 40;
        let mut y = PHONE_Y + 54;

        self.draw_string(x, y, "SOVEREIGN MUSIC & SYNTHESIZER", 1, COLOR_CYAN, Some(COLOR_FRAME_BG));
        self.draw_string(x + w - 104, y, "FLAC 24-bit", 1, COLOR_MUTED, Some(COLOR_FRAME_BG));
        y += 24;

        self.draw_rect(x, y, w, 160, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w, 160, 1, COLOR_CARD_BORDER);

        let cx = x + w / 2;
        let cy = y + 80;
        self.draw_rect(cx - 50, cy - 50, 100, 100, (15, 20, 30));
        self.draw_rect_outline(cx - 50, cy - 50, 100, 100, 2, COLOR_BEZEL);
        self.draw_rect(cx - 30, cy - 30, 60, 60, (25, 35, 50));
        self.draw_rect(cx - 10, cy - 10, 20, 20, COLOR_CYAN);
        y += 174;

        self.draw_string(x, y, "Mesh Harmony", 2, COLOR_WHITE, Some(COLOR_FRAME_BG));
        y += 24;
        self.draw_string(x, y, "Ring 3 Synthesizer - 130 BPM - Hi-Fi", 1, COLOR_MUTED, Some(COLOR_FRAME_BG));
        y += 24;

        self.draw_rect(x, y, w, 64, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w, 64, 1, COLOR_CARD_BORDER);
        let bar_heights = [18, 34, 48, 26, 54, 40, 22, 60, 44, 30, 52, 38, 20, 46, 28, 14];
        let bar_w = (w - 36) / 16;
        for i in 0..16 {
            let bh = bar_heights[i];
            let bx = x + 14 + i * (bar_w + 2);
            let by = y + 64 - bh - 6;
            self.draw_rect(bx, by, bar_w, bh, if self.music_playing { COLOR_CYAN } else { COLOR_MUTED });
        }
        y += 78;

        let btn_w = (w - 20) / 3;
        self.draw_rect(x, y, btn_w, 42, COLOR_CARD_BG);
        self.draw_string(x + (btn_w - 48) / 2, y + 16, "[ PREV ]", 1, COLOR_WHITE, Some(COLOR_CARD_BG));

        let play_x = x + btn_w + 10;
        self.draw_rect(play_x, y, btn_w, 42, if self.music_playing { COLOR_GREEN } else { COLOR_CYAN });
        self.draw_string(play_x + (btn_w - 64) / 2, y + 16, if self.music_playing { "[ PAUSE ]" } else { "[ PLAY ]" }, 1, (10, 15, 25), Some(if self.music_playing { COLOR_GREEN } else { COLOR_CYAN }));

        let next_x = play_x + btn_w + 10;
        self.draw_rect(next_x, y, btn_w, 42, COLOR_CARD_BG);
        self.draw_string(next_x + (btn_w - 48) / 2, y + 16, "[ NEXT ]", 1, COLOR_WHITE, Some(COLOR_CARD_BG));
        y += 56;

        self.draw_rect(x, y, w, 90, COLOR_DARK_SLATE);
        self.draw_rect_outline(x, y, w, 90, 1, COLOR_CARD_BORDER);
        let mut sy = y + 12;
        self.draw_string(x + 14, sy, "[ ZERO-COPY DMA AUDIO STREAMING ]", 1, COLOR_CYAN, Some(COLOR_DARK_SLATE));
        sy += 22;
        self.draw_string(x + 14, sy, "* SHM Pipe 1: 44.1 kHz Stereo Stream Active", 1, COLOR_GREEN, Some(COLOR_DARK_SLATE));
        sy += 18;
        self.draw_string(x + 14, sy, "* Waveforms: Sine, Square, Triangle, Sawtooth", 1, COLOR_WHITE, Some(COLOR_DARK_SLATE));
        sy += 18;
        self.draw_string(x + 14, sy, "* Sector 204: Audio Media Container Sealed", 1, COLOR_MUTED, Some(COLOR_DARK_SLATE));
    }

    pub fn handle_key(&mut self, c: char) {
        if c == '\x1b' {
            self.active_screen = ActiveScreen::HomeScreen;
            self.intent_len = 0;
            self.last_action = "NAV: Returned to Home Canvas";
            self.redraw_phone();
            return;
        }

        match self.active_screen {
            ActiveScreen::HomeScreen => {
                match c {
                    '1' => { self.active_screen = ActiveScreen::Settings; self.redraw_phone(); return; }
                    '2' => { self.active_screen = ActiveScreen::Terminal; self.redraw_phone(); return; }
                    '3' => { self.active_screen = ActiveScreen::Dialer; self.redraw_phone(); return; }
                    '4' => { self.active_screen = ActiveScreen::Vault; self.redraw_phone(); return; }
                    '5' => { self.active_screen = ActiveScreen::Camera; self.redraw_phone(); return; }
                    '6' => { self.active_screen = ActiveScreen::Calculator; self.redraw_phone(); return; }
                    '7' => { self.active_screen = ActiveScreen::Browser; self.redraw_phone(); return; }
                    '8' => { self.active_screen = ActiveScreen::Messages; self.redraw_phone(); return; }
                    '9' => { self.active_screen = ActiveScreen::Calendar; self.redraw_phone(); return; }
                    '0' => { self.active_screen = ActiveScreen::Music; self.redraw_phone(); return; }
                    _ => {}
                }

                match c {
                    '\x08' => {
                        if self.intent_len > 0 {
                            self.intent_len -= 1;
                            self.intent_buffer[self.intent_len] = '\0';
                            self.draw_intent_bar();
                        }
                    }
                    '\n' | '\r' => {
                        if self.intent_len > 0 {
                            let mut cmd = [0u8; 40];
                            let mut len = 0;
                            for i in 0..self.intent_len {
                                let ch = self.intent_buffer[i];
                                if ch.is_ascii() {
                                    cmd[len] = (ch as u8).to_ascii_lowercase();
                                    len += 1;
                                }
                            }

                            let cmd_str = core::str::from_utf8(&cmd[..len]).unwrap_or("");
                            if cmd_str.contains("dial") || cmd_str.contains("phone") || cmd_str.contains("call") {
                                self.active_screen = ActiveScreen::Dialer;
                            } else if cmd_str.contains("cam") || cmd_str.contains("photo") {
                                self.active_screen = ActiveScreen::Camera;
                            } else if cmd_str.contains("set") || cmd_str.contains("pref") {
                                self.active_screen = ActiveScreen::Settings;
                            } else if cmd_str.contains("term") || cmd_str.contains("shell") {
                                self.active_screen = ActiveScreen::Terminal;
                            } else if cmd_str.contains("vault") || cmd_str.contains("key") {
                                self.active_screen = ActiveScreen::Vault;
                            } else if cmd_str.contains("calc") || cmd_str.contains("math") {
                                self.active_screen = ActiveScreen::Calculator;
                            } else if cmd_str.contains("web") || cmd_str.contains("browser") {
                                self.active_screen = ActiveScreen::Browser;
                            } else if cmd_str.contains("msg") || cmd_str.contains("message") {
                                self.active_screen = ActiveScreen::Messages;
                            } else if cmd_str.contains("cal") || cmd_str.contains("plan") {
                                self.active_screen = ActiveScreen::Calendar;
                            } else if cmd_str.contains("music") || cmd_str.contains("song") {
                                self.active_screen = ActiveScreen::Music;
                            } else {
                                self.last_action = "DISPATCHED: Intent processed";
                            }

                            self.intent_len = 0;
                            self.redraw_phone();
                        }
                    }
                    printable if printable.is_ascii_graphic() || printable == ' ' => {
                        if self.intent_len < 36 {
                            self.intent_buffer[self.intent_len] = printable;
                            self.intent_len += 1;
                            self.draw_intent_bar();
                        }
                    }
                    _ => {}
                }
            }
            ActiveScreen::Dialer => {
                match c {
                    d if d.is_ascii_digit() || d == '*' || d == '#' => {
                        if self.dialed_len < 15 {
                            self.dialed_digits[self.dialed_len] = d;
                            self.dialed_len += 1;
                            self.redraw_phone();
                        }
                    }
                    '\x08' => {
                        if self.dialed_len > 0 {
                            self.dialed_len -= 1;
                            self.dialed_digits[self.dialed_len] = '\0';
                            self.redraw_phone();
                        }
                    }
                    'c' | '\n' | '\r' => {
                        self.in_call = !self.in_call;
                        self.redraw_phone();
                    }
                    _ => {}
                }
            }
            ActiveScreen::Camera => {
                match c {
                    ' ' | '\n' | '\r' => {
                        self.shutter_count = self.shutter_count.saturating_add(1);
                        self.redraw_phone();
                    }
                    '1'..='5' => {
                        self.camera_filter = c as u8 - b'1';
                        self.redraw_phone();
                    }
                    'm' => {
                        self.camera_mode = (self.camera_mode + 1) % 5;
                        self.redraw_phone();
                    }
                    _ => {}
                }
            }
            ActiveScreen::Settings => {
                match c {
                    '1'..='5' => {
                        self.settings_tab = c as u8 - b'1';
                        self.redraw_phone();
                    }
                    'w' => {
                        self.wifi_on = !self.wifi_on;
                        self.redraw_phone();
                    }
                    'g' => {
                        self.dvfs_governor = (self.dvfs_governor + 1) % 3;
                        self.redraw_phone();
                    }
                    _ => {}
                }
            }
            ActiveScreen::Terminal => {
                match c {
                    '\x08' => {
                        if self.intent_len > 0 {
                            self.intent_len -= 1;
                            self.intent_buffer[self.intent_len] = '\0';
                            self.redraw_phone();
                        }
                    }
                    '\n' | '\r' => {
                        if self.intent_len > 0 {
                            let slot = self.term_line_count % 10;
                            let mut len = 0;
                            for i in 0..self.intent_len {
                                self.term_lines[slot][i] = self.intent_buffer[i];
                                len += 1;
                            }
                            self.term_line_lens[slot] = len;
                            self.term_line_count += 1;

                            if self.intent_len >= 4 && &self.intent_buffer[..4] == &['e','x','i','t'] {
                                self.active_screen = ActiveScreen::HomeScreen;
                            }
                            self.intent_len = 0;
                            self.redraw_phone();
                        }
                    }
                    printable if printable.is_ascii_graphic() || printable == ' ' => {
                        if self.intent_len < 38 {
                            self.intent_buffer[self.intent_len] = printable;
                            self.intent_len += 1;
                            self.redraw_phone();
                        }
                    }
                    _ => {}
                }
            }
            ActiveScreen::Vault => {
                if c.is_ascii_digit() {
                    if self.vault_pin_len < 4 {
                        self.vault_pin[self.vault_pin_len] = c;
                        self.vault_pin_len += 1;
                        if self.vault_pin_len == 4 && &self.vault_pin == &['1','2','3','4'] {
                            self.vault_unlocked = true;
                        }
                        self.redraw_phone();
                    }
                }
            }
            ActiveScreen::Music => {
                match c {
                    ' ' => {
                        self.music_playing = !self.music_playing;
                        self.redraw_phone();
                    }
                    'n' => {
                        self.music_track = (self.music_track + 1) % 3;
                        self.redraw_phone();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    pub fn handle_touch(&mut self, x: usize, y: usize) {
        if y >= PHONE_Y + PHONE_H - 24 {
            self.active_screen = ActiveScreen::HomeScreen;
            self.redraw_phone();
            return;
        }

        if self.active_screen == ActiveScreen::HomeScreen {
            let start_x = PHONE_X + 20;
            let start_y = PHONE_Y + 114;
            let col_w = 100;

            if x >= start_x && x < start_x + 4 * col_w {
                let col = (x - start_x) / col_w;

                // Row 1: Settings, Phone, Messages, Camera
                if y >= start_y && y < start_y + 70 {
                    match col {
                        0 => { self.active_screen = ActiveScreen::Settings; self.redraw_phone(); return; }
                        1 => { self.active_screen = ActiveScreen::Dialer; self.redraw_phone(); return; }
                        2 => { self.active_screen = ActiveScreen::Messages; self.redraw_phone(); return; }
                        3 => { self.active_screen = ActiveScreen::Camera; self.redraw_phone(); return; }
                        _ => {}
                    }
                }
                // Row 2: Browser, Music, Calendar, Clock
                if y >= start_y + 76 && y < start_y + 146 {
                    match col {
                        0 => { self.active_screen = ActiveScreen::Browser; self.redraw_phone(); return; }
                        1 => { self.active_screen = ActiveScreen::Music; self.redraw_phone(); return; }
                        2 => { self.active_screen = ActiveScreen::Calendar; self.redraw_phone(); return; }
                        3 => { self.active_screen = ActiveScreen::Settings; self.redraw_phone(); return; }
                        _ => {}
                    }
                }
                // Row 3: Files, Calc, Terminal, Vault
                if y >= start_y + 152 && y < start_y + 222 {
                    match col {
                        0 => { self.active_screen = ActiveScreen::Settings; self.redraw_phone(); return; }
                        1 => { self.active_screen = ActiveScreen::Calculator; self.redraw_phone(); return; }
                        2 => { self.active_screen = ActiveScreen::Terminal; self.redraw_phone(); return; }
                        3 => { self.active_screen = ActiveScreen::Vault; self.redraw_phone(); return; }
                        _ => {}
                    }
                }
                // Row 4: Updates, Android, Packages, Security
                if y >= start_y + 228 && y < start_y + 298 {
                    match col {
                        0 => { self.active_screen = ActiveScreen::Settings; self.redraw_phone(); return; }
                        1 => { self.active_screen = ActiveScreen::Settings; self.redraw_phone(); return; }
                        2 => { self.active_screen = ActiveScreen::Terminal; self.redraw_phone(); return; }
                        3 => { self.active_screen = ActiveScreen::Vault; self.redraw_phone(); return; }
                        _ => {}
                    }
                }
                // Bottom Dock: Phone, Messages, Browser, Camera
                let dock_y = PHONE_Y + PHONE_H - 96;
                if y >= dock_y && y < dock_y + 74 {
                    match col {
                        0 => { self.active_screen = ActiveScreen::Dialer; self.redraw_phone(); return; }
                        1 => { self.active_screen = ActiveScreen::Messages; self.redraw_phone(); return; }
                        2 => { self.active_screen = ActiveScreen::Browser; self.redraw_phone(); return; }
                        3 => { self.active_screen = ActiveScreen::Camera; self.redraw_phone(); return; }
                        _ => {}
                    }
                }
            }
        } else if self.active_screen == ActiveScreen::Dialer {
            let bx = PHONE_X + 20;
            let bw = (PHONE_W - 56) / 2;
            let by = PHONE_Y + 348;
            if x >= bx && x <= bx + bw && y >= by && y <= by + 44 {
                self.in_call = true;
                self.redraw_phone();
            } else if x >= bx + bw + 16 && x <= bx + 2 * bw + 16 && y >= by && y <= by + 44 {
                self.in_call = false;
                self.redraw_phone();
            }
        } else if self.active_screen == ActiveScreen::Camera {
            let sbtn_w = 160;
            let sbtn_h = 44;
            let sbtn_x = PHONE_X + 20 + (PHONE_W - 40 - sbtn_w) / 2;
            let sbtn_y = PHONE_Y + 380;
            if x >= sbtn_x && x <= sbtn_x + sbtn_w && y >= sbtn_y && y <= sbtn_y + sbtn_h {
                self.shutter_count = self.shutter_count.saturating_add(1);
                self.redraw_phone();
            }
        } else if self.active_screen == ActiveScreen::Music {
            let btn_w = (PHONE_W - 60) / 3;
            let play_x = PHONE_X + 20 + btn_w + 10;
            let by = PHONE_Y + 376;
            if x >= play_x && x <= play_x + btn_w && y >= by && y <= by + 42 {
                self.music_playing = !self.music_playing;
                self.redraw_phone();
            }
        }
    }
}

pub static FRAMEBUFFER_WRITER: Mutex<FramebufferWriter> = Mutex::new(FramebufferWriter::new());

/// Initializes the framebuffer console with the bootloader framebuffer.
pub fn init(framebuffer: &'static mut FrameBuffer) {
    FRAMEBUFFER_WRITER.lock().init(framebuffer);
}

/// Draws the complete phone device shell on screen
/// Presents an ARGB8888 scanout buffer to the bootloader-provided physical framebuffer.
/// This is the hardware-backed handoff used by the display driver on platforms where
/// the bootloader exposes a linear framebuffer. No userspace pointer is retained.
pub fn present_argb8888(pixels: &[u32], width: u32, height: u32) {
    crate::arch::with_interrupts_disabled(|| {
        let mut fb = FRAMEBUFFER_WRITER.lock();
        let info = match fb.info { Some(info) => info, None => return };
        let src_w = width as usize;
        let src_h = height as usize;
        if src_w == 0 || src_h == 0 || pixels.len() < src_w.saturating_mul(src_h) { return; }

        // Fit the compositor's logical scanout into the real boot framebuffer while
        // preserving aspect ratio. This keeps the portrait 720x1440 pipeline usable
        // on landscape QEMU framebuffers without exposing hardware memory to clients.
        let (dst_w, dst_h) = if src_w.saturating_mul(info.height) <= src_h.saturating_mul(info.width) {
            let h = info.height;
            ((src_w * h / src_h).max(1).min(info.width), h)
        } else {
            let w = info.width;
            (w, (src_h * w / src_w).max(1).min(info.height))
        };
        let off_x = (info.width - dst_w) / 2;
        let off_y = (info.height - dst_h) / 2;

        // Clear letterbox borders outside the active viewport only
        if off_x > 0 {
            fb.draw_rect(0, 0, off_x, info.height, COLOR_OBSIDIAN);
            fb.draw_rect(off_x + dst_w, 0, info.width.saturating_sub(off_x + dst_w), info.height, COLOR_OBSIDIAN);
        }
        if off_y > 0 {
            fb.draw_rect(off_x, 0, dst_w, off_y, COLOR_OBSIDIAN);
            fb.draw_rect(off_x, off_y + dst_h, dst_w, info.height.saturating_sub(off_y + dst_h), COLOR_OBSIDIAN);
        }
        for dy in 0..dst_h {
            let sy = dy * src_h / dst_h;
            for dx in 0..dst_w {
                let sx = dx * src_w / dst_w;
                let argb = pixels[sy * src_w + sx];
                fb.set_pixel(off_x + dx, off_y + dy, ((argb >> 16) as u8, (argb >> 8) as u8, argb as u8));
            }
        }
    });
}


pub fn draw_mobile_shell() {
    crate::arch::with_interrupts_disabled(|| {
        FRAMEBUFFER_WRITER.lock().draw_mobile_shell();
    });
}

/// Dispatches a keypress into the on-screen intent bar
pub fn on_key_press(c: char) {
    crate::arch::with_interrupts_disabled(|| {
        FRAMEBUFFER_WRITER.lock().handle_key(c);
    });
}

/// Dispatches a touch event into the active screen
pub fn on_touch(x: usize, y: usize) {
    crate::arch::with_interrupts_disabled(|| {
        FRAMEBUFFER_WRITER.lock().handle_touch(x, y);
    });
}

/// Programmatically switches the active screen view
pub fn switch_screen(screen: ActiveScreen) {
    crate::arch::with_interrupts_disabled(|| {
        let mut fb = FRAMEBUFFER_WRITER.lock();
        fb.active_screen = screen;
        fb.redraw_phone();
    });
}

// -----------------------------------------------------------------------------
// DisplayHal Implementation for FramebufferWriter
// -----------------------------------------------------------------------------

use crate::display_hal::{DisplayHal, DisplayInfo, DisplayPixelFormat};

impl DisplayHal for FramebufferWriter {
    fn display_info(&self) -> Option<DisplayInfo> {
        self.info.map(|info| DisplayInfo {
            width: info.width,
            height: info.height,
            stride: info.stride,
            bytes_per_pixel: info.bytes_per_pixel,
            pixel_format: match info.pixel_format {
                PixelFormat::Rgb => DisplayPixelFormat::Rgb888,
                PixelFormat::Bgr => DisplayPixelFormat::Bgr888,
                PixelFormat::U8 => DisplayPixelFormat::Grayscale8,
                _ => DisplayPixelFormat::Bgr888,
            },
        })
    }

    fn set_pixel(&mut self, x: usize, y: usize, r: u8, g: u8, b: u8) {
        self.set_pixel(x, y, (r, g, b));
    }

    fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, r: u8, g: u8, b: u8) {
        self.draw_rect(x, y, w, h, (r, g, b));
    }

    fn draw_rect_outline(&mut self, x: usize, y: usize, w: usize, h: usize, thickness: usize, r: u8, g: u8, b: u8) {
        self.draw_rect_outline(x, y, w, h, thickness, (r, g, b));
    }

    fn clear(&mut self, r: u8, g: u8, b: u8) {
        self.clear((r, g, b));
    }

    fn draw_char(&mut self, x: usize, y: usize, ch: char, scale: usize, r: u8, g: u8, b: u8) {
        self.draw_char_scaled(x, y, ch, scale, (r, g, b), None);
    }

    fn draw_text(&mut self, x: usize, y: usize, text: &str, scale: usize, r: u8, g: u8, b: u8) {
        self.draw_string(x, y, text, scale, (r, g, b), None);
    }
}
