// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! AArch64 Physical Smartphone Display Driver (Simple Framebuffer / SimpleFB)
//!
//! Provides bare-metal display scanout support on physical mobile ARM64 hardware
//! (PinePhone Allwinner A64, Qualcomm Snapdragon, MediaTek Dimensity, Raspberry Pi CM4).

use crate::fdt::ArmFdtParser;
use core::sync::atomic::{compiler_fence, Ordering};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimpleFbFormat {
    Rgb565,
    Bgr888,
    Rgb888,
    Argb8888,
    Xrgb8888,
}

impl SimpleFbFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb565 => 2,
            Self::Bgr888 | Self::Rgb888 => 3,
            Self::Argb8888 | Self::Xrgb8888 => 4,
        }
    }

    pub fn from_dtb_string(s: &str) -> Option<Self> {
        match s {
            "r5g6b5" => Some(Self::Rgb565),
            "b8g8r8" => Some(Self::Bgr888),
            "r8g8b8" => Some(Self::Rgb888),
            "a8r8g8b8" => Some(Self::Argb8888),
            "x8r8g8b8" => Some(Self::Xrgb8888),
            _ => None,
        }
    }
}

pub struct SimpleFramebuffer {
    pub base_paddr: usize,
    pub base_vaddr: usize,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: SimpleFbFormat,
}

impl SimpleFramebuffer {
    pub const fn new(
        base_paddr: usize,
        base_vaddr: usize,
        width: u32,
        height: u32,
        stride: u32,
        format: SimpleFbFormat,
    ) -> Self {
        Self {
            base_paddr,
            base_vaddr,
            width,
            height,
            stride,
            format,
        }
    }

    pub fn from_fdt(parser: &ArmFdtParser, node_name: &str, base_vaddr: usize) -> Result<Self, &'static str> {
        let node = parser.find_node_by_name(node_name).map_err(|_| "Framebuffer node not found in DTB")?;

        let w_bytes = parser.get_node_property(&node, "width").map_err(|_| "Missing width property")?;
        if w_bytes.len() < 4 { return Err("Invalid width in DTB"); }
        let width = u32::from_be_bytes([w_bytes[0], w_bytes[1], w_bytes[2], w_bytes[3]]);

        let h_bytes = parser.get_node_property(&node, "height").map_err(|_| "Missing height property")?;
        if h_bytes.len() < 4 { return Err("Invalid height in DTB"); }
        let height = u32::from_be_bytes([h_bytes[0], h_bytes[1], h_bytes[2], h_bytes[3]]);

        let s_bytes = parser.get_node_property(&node, "stride").map_err(|_| "Missing stride property")?;
        if s_bytes.len() < 4 { return Err("Invalid stride in DTB"); }
        let stride = u32::from_be_bytes([s_bytes[0], s_bytes[1], s_bytes[2], s_bytes[3]]);

        let fmt_bytes = parser.get_node_property(&node, "format").map_err(|_| "Missing format property")?;
        let len = if !fmt_bytes.is_empty() && fmt_bytes[fmt_bytes.len() - 1] == 0 {
            fmt_bytes.len() - 1
        } else {
            fmt_bytes.len()
        };
        let fmt_str = core::str::from_utf8(&fmt_bytes[..len]).map_err(|_| "Corrupted format string")?;
let format = SimpleFbFormat::from_dtb_string(fmt_str).ok_or("Unsupported SimpleFB format in DTB")?;

    // Validate the geometry before any surface is built from it. Every value
    // comes from a device tree supplied by the bootloader, so a zero or forged
    // dimension would otherwise turn into a divide-by-zero, a wrapped
    // allocation or an out-of-bounds pixel offset.
    if width == 0 || height == 0 {
        return Err("Framebuffer geometry is zero");
    }
    let bpp = format.bytes_per_pixel() as u64;
    let min_stride = (width as u64)
        .checked_mul(bpp)
        .ok_or("Framebuffer row size overflow")?;
    if (stride as u64) < min_stride {
        return Err("Framebuffer stride is smaller than one scanline");
    }
    let buffer_size = (stride as u64)
        .checked_mul(height as u64)
        .ok_or("Framebuffer size overflow")?;
    if buffer_size == 0 || buffer_size > usize::MAX as u64 {
        return Err("Framebuffer size is invalid");
    }

    // A display-backed surface needs a physical base address. Falling back to the
    // supplied virtual address would present a heap or identity-mapped range as
    // though it were a scanout.
    let base_paddr = parser
        .get_node_property(&node, "reg")
        .map_err(|_| "Missing framebuffer reg property")?;
    let base_paddr = if base_paddr.len() >= 8 {
        u64::from_be_bytes([
            base_paddr[0], base_paddr[1], base_paddr[2], base_paddr[3],
            base_paddr[4], base_paddr[5], base_paddr[6], base_paddr[7],
        ]) as usize
    } else if base_paddr.len() >= 4 {
        u32::from_be_bytes([base_paddr[0], base_paddr[1], base_paddr[2], base_paddr[3]]) as usize
    } else {
        return Err("Framebuffer reg property is too short");
    };
    if base_paddr == 0 {
        return Err("Framebuffer reg property has no base address");
    }

    Ok(Self::new(base_paddr, base_vaddr, width, height, stride, format))
}

    pub const fn buffer_size(&self) -> usize {
        (self.stride as usize) * (self.height as usize)
    }

    /// Reads a pixel back from the surface in its native packed encoding.
    ///
    /// Used to prove that rasterisation actually reached the backing store
    /// instead of merely reporting success. Returns `None` for out-of-range
    /// coordinates.
    pub fn get_pixel(&self, x: u32, y: u32) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }

        let bpp = self.format.bytes_per_pixel();
        let offset = (y as usize * self.stride as usize) + (x as usize * bpp);
        let ptr = (self.base_vaddr + offset) as *const u8;

        compiler_fence(Ordering::SeqCst);
        let value = unsafe {
            match self.format {
                SimpleFbFormat::Rgb565 => core::ptr::read_volatile(ptr as *const u16) as u32,
                SimpleFbFormat::Bgr888 | SimpleFbFormat::Rgb888 => {
                    let b = core::ptr::read_volatile(ptr) as u32;
                    let g = core::ptr::read_volatile(ptr.add(1)) as u32;
                    let r = core::ptr::read_volatile(ptr.add(2)) as u32;
                    (r << 16) | (g << 8) | b
                }
                SimpleFbFormat::Argb8888 | SimpleFbFormat::Xrgb8888 => {
                    let b = core::ptr::read_volatile(ptr) as u32;
                    let g = core::ptr::read_volatile(ptr.add(1)) as u32;
                    let r = core::ptr::read_volatile(ptr.add(2)) as u32;
                    let a = core::ptr::read_volatile(ptr.add(3)) as u32;
                    (a << 24) | (r << 16) | (g << 8) | b
                }
            }
        };
        compiler_fence(Ordering::SeqCst);
        Some(value)
    }

    /// Packs an 8-bit RGB triple into the surface's native encoding.
    /// Exposed so callers can assert on read-back without duplicating the layout.
    pub fn pack_rgb(&self, r: u8, g: u8, b: u8) -> u32 {
        match self.format {
            SimpleFbFormat::Rgb565 => {
                let r5 = ((r as u32) >> 3) & 0x1F;
                let g6 = ((g as u32) >> 2) & 0x3F;
                let b5 = ((b as u32) >> 3) & 0x1F;
                (r5 << 11) | (g6 << 5) | b5
            }
            _ => (r as u32) << 16 | (g as u32) << 8 | b as u32,
        }
    }

    #[inline]
    pub fn set_pixel(&mut self, x: u32, y: u32, r: u8, g: u8, b: u8) {
        if x >= self.width || y >= self.height {
            return;
        }

        let bpp = self.format.bytes_per_pixel();
        let offset = (y as usize * self.stride as usize) + (x as usize * bpp);
        let ptr = (self.base_vaddr + offset) as *mut u8;

        compiler_fence(Ordering::SeqCst);
        unsafe {
            match self.format {
                SimpleFbFormat::Rgb565 => {
                    let r5 = ((r as u16) >> 3) & 0x1F;
                    let g6 = ((g as u16) >> 2) & 0x3F;
                    let b5 = ((b as u16) >> 3) & 0x1F;
                    let rgb16 = (r5 << 11) | (g6 << 5) | b5;
                    core::ptr::write_volatile(ptr as *mut u16, rgb16);
                }
                SimpleFbFormat::Bgr888 => {
                    core::ptr::write_volatile(ptr, b);
                    core::ptr::write_volatile(ptr.add(1), g);
                    core::ptr::write_volatile(ptr.add(2), r);
                }
                SimpleFbFormat::Rgb888 => {
                    core::ptr::write_volatile(ptr, r);
                    core::ptr::write_volatile(ptr.add(1), g);
                    core::ptr::write_volatile(ptr.add(2), b);
                }
                SimpleFbFormat::Argb8888 => {
                    core::ptr::write_volatile(ptr, b);
                    core::ptr::write_volatile(ptr.add(1), g);
                    core::ptr::write_volatile(ptr.add(2), r);
                    core::ptr::write_volatile(ptr.add(3), 0xFF);
                }
                SimpleFbFormat::Xrgb8888 => {
                    core::ptr::write_volatile(ptr, b);
                    core::ptr::write_volatile(ptr.add(1), g);
                    core::ptr::write_volatile(ptr.add(2), r);
                    core::ptr::write_volatile(ptr.add(3), 0x00);
                }
            }
        }
        compiler_fence(Ordering::SeqCst);
    }

    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, r: u8, g: u8, b: u8) {
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);

        for py in y..y_end {
            for px in x..x_end {
                self.set_pixel(px, py, r, g, b);
            }
        }
    }

    pub fn clear(&mut self, r: u8, g: u8, b: u8) {
        self.fill_rect(0, 0, self.width, self.height, r, g, b);
    }

    pub fn draw_char(&mut self, x: u32, y: u32, c: char, fg: (u8, u8, u8), bg: (u8, u8, u8)) {
        let ascii = c as usize;
        if ascii < 32 || ascii > 126 {
            return;
        }

        let glyph = FONT_8X8[ascii - 32];
        for row in 0..8 {
            let row_byte = glyph[row];
            for col in 0..8 {
                let is_fg = (row_byte & (1 << (7 - col))) != 0;
                let color = if is_fg { fg } else { bg };
                self.set_pixel(x + col as u32, y + row as u32, color.0, color.1, color.2);
            }
        }
    }

    pub fn draw_string(&mut self, mut x: u32, y: u32, text: &str, fg: (u8, u8, u8), bg: (u8, u8, u8)) {
        for c in text.chars() {
            if c == '\n' {
                break;
            }
            self.draw_char(x, y, c, fg, bg);
            x += 8;
            if x + 8 > self.width {
                break;
            }
        }
    }
}
