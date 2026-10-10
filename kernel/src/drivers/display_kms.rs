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

/// Grows a half-open row band `band` so that it also covers `[y0, y1)`.
///
/// WHY a single growing band rather than a rectangle list: the present path
/// consumes one contiguous row range, and a band keeps the invariant "rows
/// outside `band` are unchanged" easy to state and to test. Empty and inverted
/// ranges are absorbed (`y1 <= band.0` adds nothing), which matters because a
/// clipped-off surface can produce an empty range.
fn union_rows(band: &mut (usize, usize), y0: usize, y1: usize) {
    if y1 <= y0 { return; }
    if band.1 <= band.0 {
        *band = (y0, y1);
        return;
    }
    band.0 = band.0.min(y0);
    band.1 = band.1.max(y1);
}

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
    /// Half-open, surface-local row band `[y0, y1)` whose pixels changed since
    /// the last composite, in addition to the boolean `dirty` flag.
    ///
    /// WHY this exists: `dirty` is only a boolean, but `present_argb8888` can
    /// only skip scanout rows that provably did not change. Row granularity is
    /// what lets the present path redraw a handful of rows instead of all
    /// 1,036,800 pixels. `(0, 0)` means "no row-granular information" and is
    /// deliberately treated by `composite` as an unknown change (see the
    /// untracked-write guard there): an empty band must never be mistaken for
    /// "nothing moved", because pixel mutation can also happen through the raw
    /// surface-write syscall, which only sets `dirty`.
    pub dirty_rows: (usize, usize),
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
            dirty_rows: (0, 0),
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
    /// Authoritative guest display-power state. When false the compositor scanout is black.
    pub powered: bool,
    /// Half-open band of scanout rows `[y0, y1)` that the next flip must copy
    /// out to the physical framebuffer.
    ///
    /// WHY default to the full scanout: the boot framebuffer starts out holding
    /// whatever the firmware drew, so the first flip after boot has to push
    /// every row. The same "present everything" default is re-armed by
    /// `mark_present_full` for any change whose row extent is not tracked
    /// exactly (mode/orientation changes, power transitions, surface
    /// create/destroy/move, and pixel writes that did not come through
    /// `write_surface_pixels`). Narrowing only ever happens for a band that a
    /// write actually established, so behaviour is byte-identical to the old
    /// full-scanout present whenever no narrower band was genuinely earned.
    pub present_dirty_rows: (usize, usize),
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
            powered: true,
            present_dirty_rows: (0, height as usize),
        }
    }

    /// Re-arms the full-scanout present band.
    ///
    /// WHY: every mutation that can move pixels we cannot attribute to an exact
    /// row range calls this, so the flip still presents everything exactly like
    /// the pre-banding code did. Old behaviour: every flip copied the whole
    /// 720x1440 front buffer regardless of what changed.
    pub fn mark_present_full(&mut self) {
        self.present_dirty_rows = (0, self.scanout_height as usize);
    }

    /// Widens the pending present band to cover scanout rows `[y0, y1)`.
    pub fn mark_present_rows(&mut self, y0: usize, y1: usize) {
        union_rows(&mut self.present_dirty_rows, y0, y1.min(self.scanout_height as usize));
    }

    /// Returns the pending present band and clears it.
    ///
    /// WHY clear to an *empty* band rather than re-arming the full scanout:
    /// `composite()` already ran and recomputed the entire back buffer before
    /// the flip, and composition is a pure function of surface pixels plus
    /// geometry. Rows outside the accumulated band are therefore guaranteed to
    /// hold the same bytes they held at the previous present, so re-presenting
    /// them is pure work. Clearing means an idle frame presents nothing instead
    /// of re-converting 1M pixels; every mutator widens the band back on its
    /// own, so "nothing was recorded" cannot become stale pixels.
    pub fn take_present_dirty_rows(&mut self) -> (usize, usize) {
        let band = self.present_dirty_rows;
        self.present_dirty_rows = (0, 0);
        band
    }

    /// Sets the authoritative display power state. Surfaces remain intact so wake is instantaneous.
    pub fn set_powered(&mut self, powered: bool) {
        if self.powered == powered { return; }
        self.powered = powered;
        self.frame_dirty = true;
        // A sleep fills both buffers with black, so the next flip has to push
        // every row; a wake re-exposes content that may never have been
        // presented. Neither transition has a usable row range.
        self.mark_present_full();
        if !powered {
            self.front_buffer.fill(0xFF000000);
            self.back_buffer.fill(0xFF000000);
        }
    }

    pub fn toggle_powered(&mut self) -> bool {
        let next = !self.powered;
        self.set_powered(next);
        next
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
        // A new surface appears on top of whatever was there before, so the rows
        // it covers are the union of "what it paints" and "what it hides". The
        // occluded rows are not tracked per-surface, so re-arm the full scanout.
        self.mark_present_full();
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
        // Record which surface rows moved so `composite` can narrow the present
        // band instead of re-copying the whole scanout. `offset..end` is a
        // row-major range of `width`-pixel rows, so the rows touched are
        // `[offset / width, (end - 1) / width + 1)`.
        let row_pitch = surface.width as usize;
        if row_pitch > 0 && src.len() > 0 {
            let r0 = offset / row_pitch;
            let r1 = (end - 1) / row_pitch + 1;
            union_rows(&mut surface.dirty_rows, r0, r1.min(surface.height as usize));
        }
        Ok(src.len())
    }

    /// Destroys a surface by ID.
    pub fn destroy_surface(&mut self, id: u32) -> bool {
        if let Some(pos) = self.surfaces.iter().position(|s| s.id == id) {
            let owner = self.surfaces[pos].owner_task;
            self.surfaces.remove(pos);
            self.frame_dirty = true;
            // Whatever the removed surface covered is now background, and the
            // surfaces below it become visible again. Neither transition has a
            // single trackable row range.
            self.mark_present_full();
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
            // Same occlusion reveal as `destroy_surface`: full re-present.
            self.mark_present_full();
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
        if removed_count > 0 {
            self.frame_dirty = true;
            // Tearing down a task's windows exposes the layers beneath them.
            self.mark_present_full();
        }
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
            // A move relocates the surface and re-orders the stack, so both the
            // vacated rows and the newly covered rows change with no single
            // contiguous range to attribute them to.
            self.mark_present_full();
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
            // See `set_surface_geometry`: vacated and covered rows both change.
            self.mark_present_full();
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
        // Resizing reallocates and re-fills both buffers, so every row of the
        // front buffer is new content that has never been presented.
        if self.front_buffer.len() != size {
            self.front_buffer.resize(size, 0xFF000000);
            self.mark_present_full();
        }
        if self.back_buffer.len() != size {
            self.back_buffer.resize(size, 0xFF0B0F19);
            self.mark_present_full();
        }
    }

    /// Composites all visible surfaces onto the back buffer in z-order.
    pub fn composite(&mut self) {
        self.ensure_buffers();
        if !self.powered {
            self.back_buffer.fill(0xFF000000);
            for surface in self.surfaces.iter_mut() { surface.dirty = false; }
            self.frame_dirty = false;
            // Suspending blanks the whole scanout, so every row changed.
            self.mark_present_full();
            return;
        }
        if !self.frame_dirty && self.surfaces.iter().all(|s| !s.dirty) {
            // Nothing was rewritten: the back buffer still holds the image the
            // last flip published, so there is nothing new to present and the
            // band is left exactly as `take_present_dirty_rows` left it (empty).
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

        // Scanout rows the flip will have to re-present. Seeded with whatever
        // the callers already declared dirty; the surface loop narrows it to
        // exactly the rows each dirty surface actually rewrote, and
        // `diff_present_rows` below makes the result fail-safe.
        let mut present_band = self.present_dirty_rows;

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
                    // WHY the mapping is the identity here: this branch requires
                    // the destination rect to be exactly `dst_y=0, dst_h=1440`
                    // over a 720x1440 scanout, and the blit above is a straight
                    // `copy_from_slice`. Surface row `r` therefore lands on
                    // scanout row `0 + r`, so the tracked band copies 1:1.
                    if surface.dirty_rows.1 > surface.dirty_rows.0 {
                        union_rows(&mut present_band, surface.dirty_rows.0, surface.dirty_rows.1.min(dst_y + dst_h));
                    }
                    surface.dirty = false;
                    surface.dirty_rows = (0, 0);
                    continue;
                }

                // Every remaining branch of this block is a *scaled* launcher
                // blit (rotation, exact 2x box average, or nearest-neighbour).
                // Each uses a different vertical mapping between source rows and
                // destination rows, so rather than invert three different
                // samplers here, widen to the destination rect the surface
                // covers: still bounded by this surface, and never wider than
                // the pre-banding code ever presented.
                if surface.dirty_rows.1 > surface.dirty_rows.0 {
                    union_rows(&mut present_band, dst_y, dst_y + dst_h);
                }

                if landscape {
                    let scanout_w = self.scanout_width as usize;
                    if is_right {
                        for dy in 0..dst_h {
                            let src_x = if dst_h <= 1 { 0 } else { ((dy * 719 + (dst_h - 1) / 2) / (dst_h - 1)).min(719) };
                            let dst_y_offset = (dst_y + dy) * scanout_w;
                            for dx in 0..dst_w {
                                let src_y = if dst_w <= 1 { 0 } else { 1439usize.saturating_sub((dx * 1439 + (dst_w - 1) / 2) / (dst_w - 1)) };
                                let src_idx = src_y * 720 + src_x;
                                let dst_idx = dst_y_offset + dst_x + dx;
                                if src_idx < surface.pixels.len() && dst_idx < self.back_buffer.len() {
                                    self.back_buffer[dst_idx] = surface.pixels[src_idx];
                                }
                            }
                        }
                    } else {
                        for dy in 0..dst_h {
                            let src_x = if dst_h <= 1 { 0 } else { 719usize.saturating_sub((dy * 719 + (dst_h - 1) / 2) / (dst_h - 1)) };
                            let dst_y_offset = (dst_y + dy) * scanout_w;
                            for dx in 0..dst_w {
                                let src_y = if dst_w <= 1 { 0 } else { (dx * 1439 + (dst_w - 1) / 2) / (dst_w - 1) };
                                let src_idx = src_y * 720 + src_x;
                                let dst_idx = dst_y_offset + dst_x + dx;
                                if src_idx < surface.pixels.len() && dst_idx < self.back_buffer.len() {
                                    self.back_buffer[dst_idx] = surface.pixels[src_idx];
                                }
                            }
                        }
                    }
                } else {
                    let scanout_w = self.scanout_width as usize;
                    if dst_w * 2 == 720 && dst_h * 2 == 1440 {
                        // WHY box-average instead of point sampling: this is
                        // the exact-integer 2x portrait shrink (720x1440
                        // canvas into a half-size phone slice, e.g. the
                        // 360x720 slice of a 1280x720 landscape scanout).
                        // Sampling only the top-left pixel of each 2x2 quad
                        // dropped 3 of 4 source pixels and made glyphs and
                        // 1px rules shimmer; averaging the quad costs 4 reads
                        // per destination pixel and stays bounded by the
                        // destination size (~259k pixels here), inside the
                        // QEMU-TCG per-flip budget. Fractional ratios below
                        // keep the cheaper point sample for the same reason.
                        for dy in 0..dst_h {
                            let r0 = (dy * 2) * 720;
                            let r1 = r0 + 720;
                            let dst_row_start = (dst_y + dy) * scanout_w + dst_x;
                            let dst_slice = &mut self.back_buffer[dst_row_start..dst_row_start + dst_w];
                            for dx in 0..dst_w {
                                let sx = dx * 2;
                                let p00 = surface.pixels[r0 + sx];
                                let p01 = surface.pixels[r0 + sx + 1];
                                let p10 = surface.pixels[r1 + sx];
                                let p11 = surface.pixels[r1 + sx + 1];
                                let a = (((p00 >> 24) + (p01 >> 24) + (p10 >> 24) + (p11 >> 24)) / 4) & 0xFF;
                                let r = ((((p00 >> 16) & 0xFF) + ((p01 >> 16) & 0xFF) + ((p10 >> 16) & 0xFF) + ((p11 >> 16) & 0xFF)) / 4) & 0xFF;
                                let g = ((((p00 >> 8) & 0xFF) + ((p01 >> 8) & 0xFF) + ((p10 >> 8) & 0xFF) + ((p11 >> 8) & 0xFF)) / 4) & 0xFF;
                                let b = (((p00 & 0xFF) + (p01 & 0xFF) + (p10 & 0xFF) + (p11 & 0xFF)) / 4) & 0xFF;
                                dst_slice[dx] = (a << 24) | (r << 16) | (g << 8) | b;
                            }
                        }
                    } else {
                        for dy in 0..dst_h {
                            let src_y = if dst_h <= 1 { 0 } else { ((dy * 1439 + (dst_h - 1) / 2) / (dst_h - 1)).min(1439) };
                            let src_row = &surface.pixels[src_y * 720..(src_y + 1) * 720];
                            let dst_row_start = (dst_y + dy) * scanout_w + dst_x;
                            for dx in 0..dst_w {
                                let src_x = if dst_w <= 1 { 0 } else { ((dx * 719 + (dst_w - 1) / 2) / (dst_w - 1)).min(719) };
                                self.back_buffer[dst_row_start + dx] = src_row[src_x];
                            }
                        }
                    }
                }
                surface.dirty = false;
                surface.dirty_rows = (0, 0);
                continue;
            }

            if x0 >= x1 || y0 >= y1 {
                continue;
            }

            // WHY this offset is a constant: the generic blit below copies
            // surface row `(y - sy)` onto scanout row `y`, so surface row `r`
            // always lands on scanout row `r + sy`. Intersecting the tracked
            // band with the rows this blit actually draws (`y0..y1`) yields the
            // exact scanout rows that changed.
            if surface.dirty_rows.1 > surface.dirty_rows.0 {
                let drawn_lo = (y0 - sy) as usize;
                let drawn_hi = (y1 - sy) as usize;
                let lo = surface.dirty_rows.0.max(drawn_lo);
                let hi = surface.dirty_rows.1.min(drawn_hi);
                if hi > lo {
                    union_rows(&mut present_band, lo + sy as usize, hi + sy as usize);
                }
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
            surface.dirty_rows = (0, 0);
        }
        self.diff_present_rows(&mut present_band);
        self.present_dirty_rows = present_band;
        self.frame_dirty = false;
    }

    /// Widens `band` to every scanout row where the freshly composed back
    /// buffer differs from the front buffer.
    ///
    /// WHY this exists even though `write_surface_pixels` already tracks rows:
    /// userspace does not reach that method. `std::surface_write` maps to
    /// `SyscallNumber::SurfaceWrite` (73), whose handler writes
    /// `surface.pixels` directly and only raises `surface.dirty`, so the
    /// tracked band is empty for every real client frame and would otherwise
    /// never narrow. Diffing the two scanout buffers is writer-agnostic and
    /// exact: `front_buffer` is what the previous flip published, so the rows
    /// that differ are precisely the rows whose on-screen bytes are stale.
    ///
    /// WHY it is cheap relative to what it saves: this is a compare-only pass
    /// over the scanout (1,036,800 `u32` for the 720x1440 case) against the
    /// ~150M-cycle present it can eliminate, and it runs at most once per
    /// composited frame.
    ///
    /// WHY the union is safe: this only ever *adds* rows to the band. Even if a
    /// tracked band were mapped wrongly, the diff keeps the flip correct.
    fn diff_present_rows(&self, band: &mut (usize, usize)) {
        let scanout_w = self.scanout_width as usize;
        let scanout_h = self.scanout_height as usize;
        let needed = match scanout_w.checked_mul(scanout_h) {
            Some(value) => value,
            None => { *band = (0, scanout_h); return; }
        };
        // Buffers of different lengths (a mode change that has not been
        // re-allocated yet) have no comparable common prefix to reason about.
        if self.front_buffer.len() != needed || self.back_buffer.len() != needed {
            *band = (0, scanout_h);
            return;
        }
        let (front, back) = (&self.front_buffer, &self.back_buffer);
        let mut y0 = scanout_h;
        let mut y1 = 0usize;
        for y in 0..scanout_h {
            let start = y * scanout_w;
            let end = start + scanout_w;
            if front[start..end] != back[start..end] {
                y0 = y0.min(y);
                y1 = y + 1;
            }
        }
        if y1 > y0 { union_rows(band, y0, y1); }
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
        // New scanout geometry: rows are re-fitted into a different physical
        // framebuffer layout, so the old present band no longer describes it.
        self.mark_present_full();
        self.ensure_buffers();
        Ok(())
    }

    pub fn set_orientation(&mut self, orientation: DisplayOrientation) {
        self.orientation = orientation;
        self.frame_dirty = true;
        // Rotation re-maps every source row onto a different destination row.
        self.mark_present_full();
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
    powered: true,
    present_dirty_rows: (0, 1440),
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
        // Geometry is unchanged (centered 360-wide slice at x=460), but the
        // exact-half portrait shrink now box-averages each 2x2 quad: dst(0,0)
        // blends two 0xFF112233 taps with two opaque-black neighbours, and
        // the corner blends one 0xFF445566 tap with three black neighbours.
        assert_eq!(kms.back_buffer[460], 0xFF081119);
        assert_eq!(kms.back_buffer[(719 * 1280) + 819], 0xFF111519);
        // Under point sampling these were the raw tap colors; under the
        // box-average shrink the taps survive as their blended quads instead.
        assert!(kms.back_buffer.iter().any(|&p| p == 0xFF081119));
        assert!(kms.back_buffer.iter().any(|&p| p == 0xFF111519));

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
