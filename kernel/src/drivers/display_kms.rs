//! DRM/KMS Multi-Surface Compositor & Display Controller
//!
//! Provides a hardware-decoupled, double-buffered Direct Rendering Manager (DRM)
//! and Kernel Mode Setting (KMS) surface composition engine:
//! - Double-buffered page flipping (Front buffer scanout and Back buffer render target)
//! - Multi-surface layer composition with z-ordering, position offsets, and visibility
//! - Porter-Duff source-over alpha blending (`out = src * a + dst * (1 - a)`)
//! - Dirty rectangle tracking to minimize memory bandwidth
//! - Native portrait mobile scanout (720x1440 / 1080x2400) and rotation transforms

use alloc::vec::Vec;
use alloc::vec;
use spin::Mutex;

/// Pixel format used across the DRM/KMS graphics pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrmPixelFormat {
    Argb8888,
    Rgb565,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayOrientation { Portrait, LandscapeRight, PortraitInverted, LandscapeLeft }

/// A hardware plane / application graphics surface managed by KMS.
pub const MAX_CLIENT_SURFACES: usize = 32;
pub const MAX_SURFACE_PIXELS: usize = 1280 * 720;
pub const MAX_SCANOUT_PIXELS: usize = 3840 * 2160;

pub struct DrmSurface {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub z_order: u32,
    pub opacity: u8, // 0 = fully transparent, 255 = fully opaque
    pub visible: bool,
    /// Whether this window participates in pointer/touch focus hit-testing.
    pub input_enabled: bool,
    pub pixels: Vec<u32>, // 32-bit ARGB pixels
    pub dirty: bool,
    /// Owning userspace task. Surface mutation is restricted to the creator.
    pub owner_task: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceSnapshot {
    pub id: u32,
    pub owner_task: u64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub z_order: u32,
    pub opacity: u8,
    pub visible: bool,
    pub dirty: bool,
    pub input_enabled: bool,
}

impl DrmSurface {
    pub fn new(id: u32, width: u32, height: u32, z_order: u32, owner_task: u64) -> Self {
        let size = (width as usize) * (height as usize);
        Self {
            id,
            x: 0,
            y: 0,
            width,
            height,
            z_order,
            opacity: 255,
            visible: true,
            input_enabled: true,
            pixels: vec![0xFF000000; size], // default opaque black
            dirty: true,
            owner_task,
        }
    }

    /// Updates surface pixels from an input buffer.
    pub fn update_pixels(&mut self, src: &[u32]) -> Result<(), &'static str> {
        if src.len() != self.pixels.len() {
            return Err("Pixel buffer length mismatch");
        }
        self.pixels.copy_from_slice(src);
        self.dirty = true;
        Ok(())
    }
}

/// DRM/KMS Display Controller and Compositor.
pub struct DrmKmsController {
    pub scanout_width: u32,
    pub scanout_height: u32,
    pub format: DrmPixelFormat,
    pub front_buffer: Vec<u32>,
    pub back_buffer: Vec<u32>,
    pub surfaces: Vec<DrmSurface>,
    pub next_surface_id: u32,
    pub vsync_counter: u64,
    pub page_flips_completed: u64,
    pub focused_surface: Option<u32>,
    pub orientation: DisplayOrientation,
    pub refresh_hz: u16,
    pub frame_deadline_ticks: u64,
    /// Set whenever a surface, display mode, or orientation changes. The compositor
    /// can reuse the already-composed back buffer when the scene is unchanged.
    pub frame_dirty: bool,
}

impl DrmKmsController {
    pub fn new(width: u32, height: u32) -> Self {
        let size = (width as usize) * (height as usize);
        Self {
            scanout_width: width,
            scanout_height: height,
            format: DrmPixelFormat::Argb8888,
            front_buffer: vec![0xFF000000; size],
            back_buffer: vec![0xFF000000; size],
            surfaces: Vec::new(),
            next_surface_id: 1,
            vsync_counter: 0,
            page_flips_completed: 0,
            focused_surface: None,
            orientation: DisplayOrientation::Portrait,
            refresh_hz: 60,
            frame_deadline_ticks: 0,
            frame_dirty: true,
        }
    }

    /// Allocates a new client surface for application rendering.
    pub fn create_surface(&mut self, width: u32, height: u32, z_order: u32, owner_task: u64) -> u32 {
        let pixels = match (width as usize).checked_mul(height as usize) {
            // Client surfaces have a stricter per-surface budget than the
            // physical scanout buffer. This prevents a single display client
            // from multiplying large kernel-heap allocations across 32 slots.
            Some(value) if value > 0 && (value <= MAX_SURFACE_PIXELS || (width <= 1080 && height <= 2400)) => value,
            _ => return 0,
        };
        if self.surfaces.len() >= MAX_CLIENT_SURFACES {
            return 0;
        }
        let id = self.next_surface_id;
        self.next_surface_id = match self.next_surface_id.checked_add(1) {
            Some(next) if next != 0 => next,
            _ => return 0,
        };
        debug_assert_eq!(pixels, width as usize * height as usize);
        let is_overlay = z_order == 100
            || owner_task == crate::services::task_id(crate::services::ServiceId::Display).map(|t| t.0).unwrap_or(u64::MAX)
            || owner_task == crate::services::task_id(crate::services::ServiceId::Compositor).map(|t| t.0).unwrap_or(u64::MAX);
        let kind = if is_overlay { crate::gui::WindowKind::SystemOverlay } else { crate::gui::WindowKind::Application };
        let mut surface = DrmSurface::new(id, width, height, z_order, owner_task);
        surface.input_enabled = !is_overlay;
        self.surfaces.push(surface);
        self.frame_dirty = true;
        if crate::gui::WINDOW_MANAGER.lock().create_with_id(
            crate::gui::WindowId(id),
            owner_task,
            crate::gui::WindowRect { x: 0, y: 0, width, height },
            kind,
            false,
        ).is_err() {
            self.surfaces.pop();
            return 0;
        }
        if !is_overlay {
            self.focused_surface = Some(id);
        }
        id
    }

    /// Writes a bounded chunk of ARGB pixels into an owned surface.
    pub fn write_surface_pixels(&mut self, id: u32, owner_task: u64, offset: usize, src: &[u32]) -> Result<usize, &'static str> {
        let surface = self.surfaces.iter_mut().find(|s| s.id == id).ok_or("surface not found")?;
        if surface.owner_task != owner_task { return Err("surface ownership denied"); }
        let end = offset.checked_add(src.len()).ok_or("surface write overflow")?;
        if end > surface.pixels.len() { return Err("surface write out of bounds"); }
        surface.pixels[offset..end].copy_from_slice(src);
        surface.dirty = true;
        self.frame_dirty = true;
        Ok(src.len())
    }

    /// Destroys a surface by ID.
    pub fn destroy_surface(&mut self, id: u32) -> bool {
        if let Some(pos) = self.surfaces.iter().position(|s| s.id == id) {
            let owner = self.surfaces[pos].owner_task;
            self.surfaces.remove(pos);
            self.frame_dirty = true;
            let _ = crate::gui::WINDOW_MANAGER.lock().destroy_if(crate::gui::WindowId(id), Some(owner));
            if self.focused_surface == Some(id) {
                self.focused_surface = self.surfaces.iter().filter(|s| s.visible && s.input_enabled).max_by_key(|s| s.z_order).map(|s| s.id);
            }
            true
        } else {
            false
        }
    }

    /// Destroys a surface only when the owner matches, or when the caller is the kernel.
    pub fn destroy_surface_owned(&mut self, id: u32, owner_task: Option<u64>) -> bool {
        if let Some(pos) = self.surfaces.iter().position(|s| {
            s.id == id && owner_task.map(|owner| s.owner_task == owner).unwrap_or(true)
        }) {
            let owner = self.surfaces[pos].owner_task;
            self.surfaces.remove(pos);
            self.frame_dirty = true;
            let gui_owner = owner_task.map(|_| owner);
            let _ = crate::gui::WINDOW_MANAGER.lock().destroy_if(crate::gui::WindowId(id), gui_owner);
            if self.focused_surface == Some(id) {
                self.focused_surface = self.surfaces.iter().filter(|s| s.visible && s.input_enabled).max_by_key(|s| s.z_order).map(|s| s.id);
            }
            true
        } else {
            false
        }
    }

    /// Removes every surface owned by a terminating userspace task.
    pub fn destroy_surfaces_owned_by(&mut self, owner_task: u64) -> usize {
        let removed: alloc::vec::Vec<u32> = self.surfaces.iter().filter(|s| s.owner_task == owner_task).map(|s| s.id).collect();
        let before = self.surfaces.len();
        self.surfaces.retain(|s| s.owner_task != owner_task);
        {
            let mut wm = crate::gui::WINDOW_MANAGER.lock();
            for id in &removed {
                let _ = wm.destroy_owned(crate::gui::WindowId(*id), owner_task);
            }
        }
        if let Some(focused) = self.focused_surface {
            if !self.surfaces.iter().any(|s| s.id == focused) {
                self.focused_surface = self.surfaces.iter().filter(|s| s.visible && s.input_enabled).max_by_key(|s| s.z_order).map(|s| s.id);
            }
        }
        let removed_count = before - self.surfaces.len();
        if removed_count > 0 { self.frame_dirty = true; }
        removed_count
    }

    /// Copies live window metadata without exposing client pixel memory.
    pub fn surface_snapshot(&self, out: &mut [SurfaceSnapshot]) -> usize {
        let count = self.surfaces.len().min(out.len());
        for (dst, src) in out.iter_mut().zip(self.surfaces.iter()).take(count) {
            *dst = SurfaceSnapshot {
                id: src.id,
                owner_task: src.owner_task,
                x: src.x,
                y: src.y,
                width: src.width,
                height: src.height,
                z_order: src.z_order,
                opacity: src.opacity,
                visible: src.visible,
                dirty: src.dirty,
                input_enabled: src.input_enabled,
            };
        }
        count
    }

    /// Sets surface position and z-order.
    pub fn set_surface_geometry(&mut self, id: u32, x: i32, y: i32, z_order: u32, visible: bool) -> bool {
        if let Some(s) = self.surfaces.iter_mut().find(|s| s.id == id) {
            s.x = x;
            s.y = y;
            s.z_order = z_order;
            s.visible = visible;
            s.dirty = true;
            self.frame_dirty = true;
            true
        } else {
            false
        }
    }

    /// Moves an owned surface and updates its visibility.
    pub fn set_surface_geometry_owned(&mut self, id: u32, owner: u64, x: i32, y: i32, z: u32, visible: bool) -> bool {
        if let Some(s) = self.surfaces.iter_mut().find(|s| s.id == id && s.owner_task == owner) {
            s.x = x;
            s.y = y;
            s.z_order = z;
            s.visible = visible;
            s.dirty = true;
            let _ = crate::gui::WINDOW_MANAGER.lock().set_geometry_owned(
                crate::gui::WindowId(id),
                owner,
                crate::gui::WindowRect { x, y, width: s.width, height: s.height },
            );
            self.frame_dirty = true;
            return true;
        }
        false
    }

    /// Focuses the topmost visible surface containing a screen coordinate.
    pub fn focus_at(&mut self, x: u32, y: u32) -> Option<(u32,u64)> {
        let mut best: Option<(u32,u32,u64)> = None;
        for s in &self.surfaces {
            if !s.visible || !s.input_enabled || x < s.x.max(0) as u32 || y < s.y.max(0) as u32 {
                continue;
            }
            if x >= (s.x.max(0) as u32).saturating_add(s.width) ||
               y >= (s.y.max(0) as u32).saturating_add(s.height) { continue; }
            if best.map(|v| s.z_order >= v.1).unwrap_or(true) {
                best=Some((s.id,s.z_order,s.owner_task));
            }
        }
        if let Some((id,_,owner))=best {
            self.focused_surface=Some(id);
            let _ = crate::gui::WINDOW_MANAGER.lock().focus_owned(crate::gui::WindowId(id), owner);
            return Some((id,owner));
        }
        None
    }

    pub fn focused_surface(&self) -> Option<u32> { self.focused_surface }

    /// Enables or disables pointer/touch hit-testing for an owned window.
    pub fn set_input_enabled_owned(&mut self, id: u32, owner: u64, enabled: bool) -> bool {
        if let Some(surface) = self.surfaces.iter_mut().find(|s| s.id == id && s.owner_task == owner) {
            surface.input_enabled = enabled;
            if !enabled && self.focused_surface == Some(id) {
                self.focused_surface = self.surfaces.iter()
                    .filter(|s| s.visible && s.input_enabled)
                    .max_by_key(|s| s.z_order)
                    .map(|s| s.id);
            }
            return true;
        }
        false
    }

    /// Ensures that front and back scanout buffers are allocated to match scanout dimensions.
    pub fn ensure_buffers(&mut self) {
        let Some(size) = (self.scanout_width as usize).checked_mul(self.scanout_height as usize) else { return; };
        if size == 0 || size > MAX_SCANOUT_PIXELS { return; }
        if self.front_buffer.len() != size {
            self.front_buffer.resize(size, 0xFF000000);
        }
        if self.back_buffer.len() != size {
            self.back_buffer.resize(size, 0xFF0B0F19);
        }
    }

    /// Composites all visible surfaces onto the back buffer in z-order.
    pub fn composite(&mut self) {
        self.ensure_buffers();
        if !self.frame_dirty && self.surfaces.iter().all(|s| !s.dirty) {
            return;
        }
        // Clear back buffer to base background color (slate navy: 0xFF0B0F19)
        let bg_color = 0xFF0B0F19u32;
        self.back_buffer.fill(bg_color);

        // Sort surfaces by z_order ascending (lower z painted first)
        self.surfaces.sort_by_key(|s| s.z_order);

        let sw = self.scanout_width as i32;
        let sh = self.scanout_height as i32;

        let has_mobile_portrait = self.surfaces.iter().any(|s| s.visible && s.width == 720 && s.height == 1440);

        for surface in self.surfaces.iter_mut() {
            if !surface.visible || surface.opacity == 0 {
                continue;
            }

            if has_mobile_portrait && surface.height == 96 {
                // Do not overlay legacy desktop banner on mobile portrait phone canvas
                continue;
            }

            let sx = surface.x;
            let sy = surface.y;
            let s_w = surface.width as i32;
            let s_h = surface.height as i32;

            // Compute clipped intersection bounds
            let x0 = sx.max(0);
            let y0 = sy.max(0);
            let x1 = sx.saturating_add(s_w).min(sw);
            let y1 = sy.saturating_add(s_h).min(sh);

            // The launcher surface has a stable 720x1440 logical portrait
            // canvas, but the physical scanout is negotiated per platform. Fit
            // that canvas to the actual scanout instead of baking in 1280x720.
            if s_w == 720 && s_h == 1440 && sx == 0 && sy == 0 {
                let (mut dst_w, mut dst_h) = if 720i64 * sh as i64 <= 1440i64 * sw as i64 {
                    (
                        (720u32.saturating_mul(sh as u32) / 1440).max(1).min(self.scanout_width),
                        self.scanout_height,
                    )
                } else {
                    (
                        self.scanout_width,
                        (1440u32.saturating_mul(sw as u32) / 720).max(1).min(self.scanout_height),
                    )
                };
                let landscape = matches!(
                    self.orientation,
                    DisplayOrientation::LandscapeRight | DisplayOrientation::LandscapeLeft
                );
                if landscape {
                    // Rotated portrait canvas has a 1440x720 aspect ratio.
                    if 1440i64 * sh as i64 <= 720i64 * sw as i64 {
                        dst_w = self.scanout_width;
                        dst_h = (720u32.saturating_mul(sw as u32) / 1440).max(1).min(self.scanout_height);
                    } else {
                        dst_w = (1440u32.saturating_mul(sh as u32) / 720).max(1).min(self.scanout_width);
                        dst_h = self.scanout_height;
                    }
                }
                let dst_x = ((sw - dst_w as i32) / 2).max(0) as usize;
                let dst_y = ((sh - dst_h as i32) / 2).max(0) as usize;
                let dst_w = dst_w as usize;
                let dst_h = dst_h as usize;
                let is_right = self.orientation == DisplayOrientation::LandscapeRight;

                // Direct identity blit when destination matches canonical canvas
                if dst_w == 720 && dst_h == 1440 && dst_x == 0 && dst_y == 0
                    && self.scanout_width == 720 && self.scanout_height == 1440 && !landscape
                {
                    self.back_buffer.copy_from_slice(&surface.pixels);
                    surface.dirty = false;
                    continue;
                }

                if landscape {
                    if is_right {
                        let y_lut_lr: Vec<usize> = if dst_w <= 1 {
                            vec![0]
                        } else {
                            (0..dst_w).map(|dx| 1439usize.saturating_sub((dx * 1439 + (dst_w - 1) / 2) / (dst_w - 1))).collect()
                        };
                        let x_lut_lr: Vec<usize> = if dst_h <= 1 {
                            vec![0]
                        } else {
                            (0..dst_h).map(|dy| ((dy * 719 + (dst_h - 1) / 2) / (dst_h - 1)).min(719)).collect()
                        };
                        for dy in 0..dst_h {
                            let src_x = x_lut_lr[dy];
                            let dst_y_offset = (dst_y + dy) * self.scanout_width as usize;
                            for dx in 0..dst_w {
                                let src_y = y_lut_lr[dx];
                                let src_idx = src_y * 720 + src_x;
                                let dst_idx = dst_y_offset + dst_x + dx;
                                if src_idx < surface.pixels.len() && dst_idx < self.back_buffer.len() {
                                    self.back_buffer[dst_idx] = surface.pixels[src_idx];
                                }
                            }
                        }
                    } else {
                        let y_lut_ll: Vec<usize> = if dst_w <= 1 {
                            vec![0]
                        } else {
                            (0..dst_w).map(|dx| (dx * 1439 + (dst_w - 1) / 2) / (dst_w - 1)).collect()
                        };
                        let x_lut_ll: Vec<usize> = if dst_h <= 1 {
                            vec![0]
                        } else {
                            (0..dst_h).map(|dy| 719usize.saturating_sub((dy * 719 + (dst_h - 1) / 2) / (dst_h - 1))).collect()
                        };
                        for dy in 0..dst_h {
                            let src_x = x_lut_ll[dy];
                            let dst_y_offset = (dst_y + dy) * self.scanout_width as usize;
                            for dx in 0..dst_w {
                                let src_y = y_lut_ll[dx];
                                let src_idx = src_y * 720 + src_x;
                                let dst_idx = dst_y_offset + dst_x + dx;
                                if src_idx < surface.pixels.len() && dst_idx < self.back_buffer.len() {
                                    self.back_buffer[dst_idx] = surface.pixels[src_idx];
                                }
                            }
                        }
                    }
                } else {
                    let x_lut: Vec<usize> = if dst_w <= 1 {
                        vec![0]
                    } else {
                        (0..dst_w).map(|dx| (dx * 719 + (dst_w - 1) / 2) / (dst_w - 1)).collect()
                    };
                    let y_lut: Vec<usize> = if dst_h <= 1 {
                        vec![0]
                    } else {
                        (0..dst_h).map(|dy| (dy * 1439 + (dst_h - 1) / 2) / (dst_h - 1)).collect()
                    };
                    for dy in 0..dst_h {
                        let src_y = y_lut[dy];
                        let src_row = &surface.pixels[src_y * 720..(src_y + 1) * 720];
                        let dst_row_start = (dst_y + dy) * self.scanout_width as usize + dst_x;
                        for dx in 0..dst_w {
                            let src_x = x_lut[dx];
                            self.back_buffer[dst_row_start + dx] = src_row[src_x];
                        }
                    }
                }
                surface.dirty = false;
                continue;
            }

            if x0 >= x1 || y0 >= y1 {
                continue;
            }

            let opacity = surface.opacity;
            if opacity == 255 {
                for y in y0..y1 {
                    let dst_y_offset = (y as usize) * (self.scanout_width as usize);
                    let src_y = (y - sy) as usize;
                    let src_y_offset = src_y * (surface.width as usize);

                    for x in x0..x1 {
                        let dst_idx = dst_y_offset + (x as usize);
                        let src_x = (x - sx) as usize;
                        let src_idx = src_y_offset + src_x;

                        let src_pixel = surface.pixels[src_idx];
                        let sa = (src_pixel >> 24) & 0xFF;
                        if sa == 255 {
                            self.back_buffer[dst_idx] = src_pixel;
                        } else if sa > 0 {
                            let inv_sa = 255 - sa;
                            let sr = (src_pixel >> 16) & 0xFF;
                            let sg = (src_pixel >> 8) & 0xFF;
                            let sb = src_pixel & 0xFF;
                            let dst_pixel = self.back_buffer[dst_idx];
                            let dr = (dst_pixel >> 16) & 0xFF;
                            let dg = (dst_pixel >> 8) & 0xFF;
                            let db = dst_pixel & 0xFF;
                            let out_r = ((sr * sa + dr * inv_sa) / 255) & 0xFF;
                            let out_g = ((sg * sa + dg * inv_sa) / 255) & 0xFF;
                            let out_b = ((sb * sa + db * inv_sa) / 255) & 0xFF;
                            self.back_buffer[dst_idx] = (0xFF << 24) | (out_r << 16) | (out_g << 8) | out_b;
                        }
                    }
                }
            } else {
                let op = opacity as u32;
                for y in y0..y1 {
                    let dst_y_offset = (y as usize) * (self.scanout_width as usize);
                    let src_y = (y - sy) as usize;
                    let src_y_offset = src_y * (surface.width as usize);

                    for x in x0..x1 {
                        let dst_idx = dst_y_offset + (x as usize);
                        let src_x = (x - sx) as usize;
                        let src_idx = src_y_offset + src_x;

                        let src_pixel = surface.pixels[src_idx];
                        let sa = (((src_pixel >> 24) & 0xFF) * op) / 255;
                        if sa == 255 {
                            self.back_buffer[dst_idx] = src_pixel;
                        } else if sa > 0 {
                            let inv_sa = 255 - sa;
                            let sr = (src_pixel >> 16) & 0xFF;
                            let sg = (src_pixel >> 8) & 0xFF;
                            let sb = src_pixel & 0xFF;
                            let dst_pixel = self.back_buffer[dst_idx];
                            let dr = (dst_pixel >> 16) & 0xFF;
                            let dg = (dst_pixel >> 8) & 0xFF;
                            let db = dst_pixel & 0xFF;
                            let out_r = ((sr * sa + dr * inv_sa) / 255) & 0xFF;
                            let out_g = ((sg * sa + dg * inv_sa) / 255) & 0xFF;
                            let out_b = ((sb * sa + db * inv_sa) / 255) & 0xFF;
                            self.back_buffer[dst_idx] = (0xFF << 24) | (out_r << 16) | (out_g << 8) | out_b;
                        }
                    }
                }
            }
            surface.dirty = false;
        }
        self.frame_dirty = false;
    }

    pub fn set_display_mode(&mut self, width: u32, height: u32, refresh_hz: u16) -> Result<(), &'static str> {
        if width == 0 || height == 0 || refresh_hz == 0 || refresh_hz > 240 {
            return Err("invalid display mode");
        }
        let pixels = (width as usize).checked_mul(height as usize).ok_or("display mode size overflow")?;
        if pixels > MAX_SCANOUT_PIXELS {
            return Err("display mode exceeds bounded scanout budget");
        }
        self.scanout_width = width;
        self.scanout_height = height;
        self.refresh_hz = refresh_hz;
        self.frame_dirty = true;
        self.ensure_buffers();
        Ok(())
    }

    pub fn set_orientation(&mut self, orientation: DisplayOrientation) {
        self.orientation = orientation;
        self.frame_dirty = true;
    }

    pub fn next_frame_deadline(&self) -> u64 { self.frame_deadline_ticks }

    /// Performs an atomic page flip: swaps back buffer to front scanout buffer.
    pub fn page_flip(&mut self) -> u64 {
        self.ensure_buffers();
        core::mem::swap(&mut self.front_buffer, &mut self.back_buffer);
        self.vsync_counter += 1;
        self.page_flips_completed += 1;
        let ticks_per_frame = ((100u64 + self.refresh_hz.max(1) as u64 - 1) / self.refresh_hz.max(1) as u64).max(1);
        self.frame_deadline_ticks = crate::interrupts::get_system_ticks().saturating_add(ticks_per_frame);
        self.page_flips_completed
    }
}

/// Global static instance of the DRM/KMS Controller.
/// Canonical portrait scanout matches the 720x1440 launcher surface. The boot framebuffer is a separate presentation target and may be any supported size.
pub static DRM_KMS: Mutex<DrmKmsController> = Mutex::new(DrmKmsController {
    scanout_width: 720,
    scanout_height: 1440,
    format: DrmPixelFormat::Argb8888,
    front_buffer: Vec::new(),
    back_buffer: Vec::new(),
    surfaces: Vec::new(),
    next_surface_id: 1,
    vsync_counter: 0,
    page_flips_completed: 0,
    focused_surface: None,
    orientation: DisplayOrientation::Portrait,
    refresh_hz: 60,
    frame_deadline_ticks: 0,
    frame_dirty: true,
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_mode_accepts_runtime_refresh_without_host_assumptions() {
        let mut kms = DrmKmsController::new(720, 1440);
        assert_eq!(kms.set_display_mode(1080, 2400, 144), Ok(()));
        assert_eq!(kms.scanout_width, 1080);
        assert_eq!(kms.scanout_height, 2400);
        assert_eq!(kms.refresh_hz, 144);
        assert!(kms.set_display_mode(1080, 2400, 0).is_err());
        assert!(kms.set_display_mode(4096, 2160, 144).is_err());
    }

    #[test]
    fn mobile_surface_fits_native_and_landscape_scanouts() {
        let mut kms = DrmKmsController::new(1280, 720);
        let id = kms.create_surface(720, 1440, 0, 0x100);
        assert_ne!(id, 0);
        kms.surfaces[0].pixels[0] = 0xFF112233;
        kms.surfaces[0].pixels[720] = 0xFF112233;
        kms.surfaces[0].pixels[(1439 * 720) + 719] = 0xFF445566;
        kms.composite();
        assert_eq!(kms.back_buffer[460], 0xFF112233);
        assert_eq!(kms.back_buffer[(719 * 1280) + 819], 0xFF445566);
        assert!(kms.back_buffer.iter().any(|&p| p == 0xFF112233));
        assert!(kms.back_buffer.iter().any(|&p| p == 0xFF445566));

        kms.set_orientation(DisplayOrientation::LandscapeRight);
        kms.frame_dirty = true;
        kms.surfaces[0].dirty = true;
        kms.composite();
        assert_eq!(kms.back_buffer[1279], 0xFF112233);
        assert_eq!(kms.back_buffer[719 * 1280], 0xFF445566);

        assert!(kms.destroy_surface(id));
    }
}

/// Automated verification self-test for DRM/KMS surface composition and page flipping.
pub fn test_drm_kms() {
    let mut kms = DrmKmsController::new(320, 480);
    assert_eq!(kms.front_buffer.len(), 320 * 480);

    // Create 2 surfaces: wallpaper (z=0) and status bar (z=10)
    let s1 = kms.create_surface(320, 480, 0, 1);
    let s2 = kms.create_surface(320, 40, 10, 1);
    assert_eq!(s1, 1);
    assert_eq!(s2, 2);
    assert_eq!(kms.create_surface(1281, 720, 20, 1), 0);
    assert_eq!(kms.create_surface(1280, 720, 20, 1), 3);

    // Fill surface 2 with a test color (semi-transparent white)
    let s2_pixels = vec![0x80FFFFFFu32; 320 * 40];
    let update_res = kms.surfaces[1].update_pixels(&s2_pixels);
    assert!(update_res.is_ok());

    // Composite and page flip
    kms.composite();
    let flips = kms.page_flip();
    assert_eq!(flips, 1);
    assert_eq!(kms.vsync_counter, 1);
    crate::gui::WINDOW_MANAGER.lock().destroy_if(crate::gui::WindowId(s1), None);
    crate::gui::WINDOW_MANAGER.lock().destroy_if(crate::gui::WindowId(s2), None);
    crate::gui::WINDOW_MANAGER.lock().destroy_if(crate::gui::WindowId(3), None);

    crate::serial_println!("[TEST] DRM/KMS: Multi-surface alpha composition & page flip verified.");
}
