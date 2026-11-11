// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V 64 Display Engine & Bochs VBE MMIO Driver
//!
//! Provides the primary graphical display pipeline for InterOS RISC-V 64 smartphone targets.
//! Supports Bochs PCI display controller with hardware scanout at 720x1440 (32bpp ARGB).

extern crate alloc;
use alloc::vec::Vec;
use spin::Mutex;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SimpleFbFormat {
    Argb8888,
    Rgb565,
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

    pub fn buffer_size(&self) -> usize {
        (self.stride as usize) * (self.height as usize)
    }

    pub fn clear(&mut self, r: u8, g: u8, b: u8) {
        let argb = 0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
        for y in 0..self.height {
            for x in 0..self.width {
                self.write_pixel_argb(x, y, argb);
            }
        }
    }

    #[inline(always)]
    pub fn write_pixel_argb(&mut self, x: u32, y: u32, argb: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        let ptr = (self.base_vaddr + (y as usize) * (self.stride as usize) + (x as usize) * 4) as *mut u32;
        unsafe {
            core::ptr::write_volatile(ptr, argb);
        }
    }

    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, r: u8, g: u8, b: u8) {
        let argb = 0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);
        for cy in y..y_end {
            for cx in x..x_end {
                self.write_pixel_argb(cx, cy, argb);
            }
        }
    }
}

pub static FRAMEBUFFER: Mutex<Option<SimpleFramebuffer>> = Mutex::new(None);

pub fn register_framebuffer(fb: SimpleFramebuffer) {
    let mut guard = FRAMEBUFFER.lock();
    *guard = Some(fb);
}

#[derive(Clone)]
pub struct Surface {
    pub id: u32,
    pub owner_task: u32,
    pub width: u32,
    pub height: u32,
    pub z_order: u32,
    pub visible: bool,
    pub pixels: Vec<u32>,
}

pub struct SurfaceManager {
    pub surfaces: Vec<Surface>,
    pub next_id: u32,
    pub flip_count: u64,
}

impl SurfaceManager {
    pub const fn new() -> Self {
        Self {
            surfaces: Vec::new(),
            next_id: 1,
            flip_count: 0,
        }
    }

    pub fn create_surface(&mut self, owner_task: u32, width: u32, height: u32, z_order: u32) -> Result<u32, i64> {
        let width = width.min(1440);
        let height = height.min(2160);
        let pixels_count = (width as usize).saturating_mul(height as usize);
        if pixels_count == 0 || pixels_count > 1440 * 2160 {
            return Err(-22); // -EINVAL
        }
        let id = self.next_id;
        self.next_id += 1;
        let mut pixels = Vec::with_capacity(pixels_count);
        pixels.resize(pixels_count, 0); // Transparent black initially
        self.surfaces.push(Surface {
            id,
            owner_task,
            width,
            height,
            z_order,
            visible: true,
            pixels,
        });
        // Keep surfaces sorted by z_order at creation so flip() requires zero allocations
        self.surfaces.sort_by_key(|s| s.z_order);
        Ok(id)
    }

    pub fn write_surface(&mut self, surface_id: u32, caller_task: u32, offset: usize, src_pixels: &[u32]) -> Result<usize, i64> {
        let s = self.surfaces.iter_mut().find(|s| s.id == surface_id).ok_or(-2)?; // -ENOENT
        if s.owner_task != caller_task {
            return Err(-13); // -EACCES: caller does not own this surface
        }
        if offset >= s.pixels.len() {
            return Err(-22);
        }
        let copy_len = src_pixels.len().min(s.pixels.len() - offset);
        s.pixels[offset..offset + copy_len].copy_from_slice(&src_pixels[..copy_len]);
        Ok(copy_len)
    }

    pub fn destroy_surface(&mut self, surface_id: u32, caller_task: u32, is_admin: bool) -> Result<(), i64> {
        if let Some(pos) = self.surfaces.iter().position(|s| s.id == surface_id) {
            if self.surfaces[pos].owner_task != caller_task && !is_admin {
                return Err(-13); // -EACCES: caller does not own this surface and lacks ADMIN
            }
            self.surfaces.remove(pos);
            let _ = self.flip();
            Ok(())
        } else {
            Err(-2)
        }
    }

    pub fn destroy_surfaces_owned_by(&mut self, owner_pid: u32) -> usize {
        let before = self.surfaces.len();
        self.surfaces.retain(|s| s.owner_task != owner_pid);
        let removed = before - self.surfaces.len();
        if removed > 0 {
            let _ = self.flip();
        }
        removed
    }

    pub fn flip(&mut self) -> u64 {
        self.flip_count += 1;
        let mut fb_guard = FRAMEBUFFER.lock();
        if let Some(fb) = fb_guard.as_mut() {
            fb.clear(16, 21, 32);

            // In-place compositing across pre-sorted surfaces: ZERO heap allocations
            for surface in self.surfaces.iter() {
                if !surface.visible {
                    continue;
                }
                let w = surface.width.min(fb.width);
                let h = surface.height.min(fb.height);
                for y in 0..h {
                    let src_row = (y as usize) * (surface.width as usize);
                    for x in 0..w {
                        let argb = surface.pixels[src_row + (x as usize)];
                        if (argb >> 24) != 0 {
                            fb.write_pixel_argb(x, y, argb);
                        }
                    }
                }
            }
        }
        self.flip_count
    }

    pub fn top_visible_owner(&self) -> Option<u32> {
        self.surfaces
            .iter()
            .filter(|s| s.visible)
            .max_by_key(|s| s.z_order)
            .map(|s| s.owner_task)
    }
}

pub static SURFACE_MANAGER: Mutex<SurfaceManager> = Mutex::new(SurfaceManager::new());

pub fn focused_task_pid() -> u32 {
    if let Some(owner) = SURFACE_MANAGER.lock().top_visible_owner() {
        owner
    } else {
        let sched = crate::task::SCHEDULER.lock();
        for t in &sched.tasks {
            if t.name == "launcher" {
                return t.id;
            }
        }
        sched.current_pid()
    }
}

pub fn focused_task_idx() -> u32 {
    focused_task_pid()
}

pub fn init_bochs_display(bar0_paddr: usize, bar2_paddr: usize) {
    if bar0_paddr == 0 || bar2_paddr == 0 {
        return;
    }

    unsafe {
        let dispi = (bar2_paddr + 0x500) as *mut u16;
        core::ptr::write_volatile(dispi.add(1), 720);   // XRES = 720
        core::ptr::write_volatile(dispi.add(2), 1440);  // YRES = 1440
        core::ptr::write_volatile(dispi.add(3), 32);    // BPP = 32
        core::ptr::write_volatile(dispi.add(4), 0x41);  // ENABLE = VBE_DISPI_ENABLED | VBE_DISPI_LFB_ENABLED
        core::ptr::write_volatile(dispi.add(6), 720);   // VIRT_WIDTH = 720
        core::ptr::write_volatile(dispi.add(7), 1440);  // VIRT_HEIGHT = 1440
    }

    let mut fb = SimpleFramebuffer::new(
        bar0_paddr,
        bar0_paddr,
        720,
        1440,
        720 * 4,
        SimpleFbFormat::Argb8888,
    );
    fb.clear(16, 21, 32);
    register_framebuffer(fb);
    crate::println!("[OK] Display: Bochs PCI display bound @ 0x{:08X} (720x1440 32bpp ARGB scanout).", bar0_paddr);
}
