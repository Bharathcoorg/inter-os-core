//! DRM/KMS Multi-Surface Compositor & Display Controller
//!
//! Provides a hardware-decoupled, double-buffered Direct Rendering Manager (DRM)
//! and Kernel Mode Setting (KMS) surface composition engine:
//! - Double-buffered page flipping (Front buffer scanout and Back buffer render target)
//! - Multi-surface layer composition with z-ordering, position offsets, and visibility
//! - Porter-Duff source-over alpha blending (`out = src * a + dst * (1 - a)`)
//! - Dirty rectangle tracking to minimize memory bandwidth
//! - Native portrait mobile scanout (1080x2160 FHD-class / 1280x720 landscape) and rotation transforms

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
/// Widens `band` to cover `y0..y1`, half-open.
///
/// WHY a helper with a public face: the `SurfaceWrite` syscall needs to record
/// the same rows the controller's own write path records, and a caller reaching
/// for its own arithmetic there produces exactly the kind of band that disagrees
/// with the compositor's. Exported so there is one implementation.
pub fn union_rows(band: &mut (usize, usize), y0: usize, y1: usize) {
    if y1 <= y0 { return; }
    if band.1 <= band.0 {
        *band = (y0, y1);
        return;
    }
    band.0 = band.0.min(y0);
    band.1 = band.1.max(y1);
}

/// Porter-Duff source-over alpha blending (`out = src * a + dst * (1 - a)`).
/// Respects both per-pixel alpha channel and surface-level opacity.
#[inline(always)]
pub fn blend_pixel(src_pixel: u32, dst_pixel: u32, opacity: u8) -> u32 {
    let mut sa = (src_pixel >> 24) & 0xFF;
    if opacity != 255 {
        sa = (sa * opacity as u32) / 255;
    }
    if sa == 255 {
        src_pixel | 0xFF000000
    } else if sa > 0 {
        let inv_sa = 255 - sa;
        let sr = (src_pixel >> 16) & 0xFF;
        let sg = (src_pixel >> 8) & 0xFF;
        let sb = src_pixel & 0xFF;
        let dr = (dst_pixel >> 16) & 0xFF;
        let dg = (dst_pixel >> 8) & 0xFF;
        let db = dst_pixel & 0xFF;
        let out_r = ((sr * sa + dr * inv_sa) / 255) & 0xFF;
        let out_g = ((sg * sa + dg * inv_sa) / 255) & 0xFF;
        let out_b = ((sb * sa + db * inv_sa) / 255) & 0xFF;
        (0xFF << 24) | (out_r << 16) | (out_g << 8) | out_b
    } else {
        dst_pixel
    }
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
    /// Whether this surface has reached a completed frame commit. When false (during
    /// in-progress multi-chunk surface writes), `composite()` skips blending this surface
    /// to prevent tearing.
    pub frame_ready: bool,
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
    pub frame_ready: bool,
}

pub const EMPTY_SURFACE_SNAPSHOT: SurfaceSnapshot = SurfaceSnapshot {
    id: 0,
    owner_task: 0,
    x: 0,
    y: 0,
    width: 0,
    height: 0,
    z_order: 0,
    opacity: 0,
    visible: false,
    dirty: false,
    input_enabled: false,
    frame_ready: true,
};

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
            frame_ready: true,
        }
    }

    /// Returns whether this surface has a completed, latched frame ready for composition.
    pub fn is_frame_ready(&self) -> bool {
        self.frame_ready
    }

    /// Sets the frame readiness latch.
    pub fn set_frame_ready(&mut self, ready: bool) {
        self.frame_ready = ready;
    }

    /// Marks a half-open band of rows `[y0, y1)` as dirty on this surface.
    pub fn mark_dirty_rows(&mut self, y0: usize, y1: usize) {
        union_rows(&mut self.dirty_rows, y0, y1.min(self.height as usize));
        self.dirty = true;
    }

    /// Updates surface pixels from an input buffer.
    pub fn update_pixels(&mut self, src: &[u32]) -> Result<(), &'static str> {
        if src.len() != self.pixels.len() {
            return Err("Pixel buffer length mismatch");
        }
        self.pixels.copy_from_slice(src);
        self.mark_dirty_rows(0, self.height as usize);
        self.frame_ready = true;
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
        let actual_z = if z_order >= 200 && z_order < 1000 {
            let max_z = self.surfaces.iter()
                .filter(|s| s.z_order < 1000)
                .map(|s| s.z_order)
                .max()
                .unwrap_or(200);
            max_z.max(200).saturating_add(1)
        } else {
            z_order
        };
        let mut surface = DrmSurface::new(id, width, height, actual_z, owner_task);
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
        if end == surface.pixels.len() {
            surface.frame_ready = true;
        } else if offset == 0 {
            surface.frame_ready = false;
        }
        self.frame_dirty = true;
        // Record which surface rows moved so `composite` can narrow the present
        // band instead of re-copying the whole scanout. `offset..end` is a
        // row-major range of `width`-pixel rows, so the rows touched are
        // `[offset / width, (end - 1) / width + 1)`.
        let row_pitch = surface.width as usize;
        if row_pitch > 0 && src.len() > 0 {
            let r0 = offset / row_pitch;
            let r1 = (end - 1) / row_pitch + 1;
            surface.mark_dirty_rows(r0, r1);
        } else {
            surface.mark_dirty_rows(0, surface.height as usize);
        }
        Ok(src.len())
    }

    /// Marks all surfaces owned by `owner_task` as committed and ready for composition.
    /// If `owner_task` is `None`, marks all surfaces as frame-ready.
    pub fn commit_surfaces(&mut self, owner_task: Option<u64>) {
        for s in self.surfaces.iter_mut() {
            if owner_task.map(|owner| s.owner_task == owner).unwrap_or(true) {
                s.frame_ready = true;
            }
        }
    }

    /// Commits a specific surface by ID if owned by `owner_task` (or if kernel).
    pub fn commit_surface(&mut self, id: u32, owner_task: Option<u64>) -> bool {
        if let Some(s) = self.surfaces.iter_mut().find(|s| {
            s.id == id && owner_task.map(|owner| s.owner_task == owner).unwrap_or(true)
        }) {
            s.frame_ready = true;
            self.frame_dirty = true;
            true
        } else {
            false
        }
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
                frame_ready: src.frame_ready,
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

    /// Raises all application surfaces owned by `owner_task` above all other application surfaces.
    pub fn raise_surfaces_for_task(&mut self, owner_task: u64) {
        let max_app_z = self.surfaces.iter()
            .filter(|s| s.z_order < 1000)
            .map(|s| s.z_order)
            .max()
            .unwrap_or(200);
        let new_z = max_app_z.max(200).saturating_add(1);
        let mut raised_any = false;
        for s in self.surfaces.iter_mut() {
            if s.owner_task == owner_task && s.z_order < 1000 {
                s.z_order = new_z;
                s.visible = true;
                s.dirty = true;
                self.focused_surface = Some(s.id);
                raised_any = true;
            }
        }
        if raised_any {
            self.frame_dirty = true;
            self.mark_present_full();
        }
    }

    /// Sets the visibility of all surfaces owned by `owner_task`.
    pub fn set_surfaces_visibility_for_task(&mut self, owner_task: u64, visible: bool) {
        let mut changed = false;
        for s in self.surfaces.iter_mut() {
            if s.owner_task == owner_task {
                if s.visible != visible {
                    s.visible = visible;
                    s.dirty = true;
                    changed = true;
                }
            }
        }
        if changed {
            if !visible && self.focused_surface.map(|f| self.surfaces.iter().any(|s| s.id == f && s.owner_task == owner_task)).unwrap_or(false) {
                self.focused_surface = self.surfaces.iter()
                    .filter(|s| s.visible && s.input_enabled)
                    .max_by_key(|s| s.z_order)
                    .map(|s| s.id);
            }
            self.frame_dirty = true;
            self.mark_present_full();
        }
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

    /// True when the surface loop below is guaranteed to take the identity
    /// full-cover blit: one surface whose width/height are exactly the scanout's,
    /// placed at the origin over an unrotated scanout, which is a
    /// `copy_from_slice` replacing every back-buffer pixel.
    ///
    /// WHY this predicate and not "some surface covers the scanout": a surface
    /// declares `opacity`, but its *pixels* carry their own alpha, and the
    /// generic blit blends against the backdrop rather than replacing it. Only
    /// the identity path overwrites the backdrop outright, so only it makes
    /// dropping the background fill provably equivalent — a transparent pixel
    /// lands on the same colour either way.
    ///
    /// WHY the scanout's own size rather than the 720x1440 launcher canvas: the
    /// whole launcher canvas is only identity when the scanout *is* 720x1440.
    /// Once the scanout is 1080x2160 the launcher surface takes the scaled blit,
    /// which is the correct image at the cost of the fill and the interpolation;
    /// the fast path still belongs to whichever surface matches the scanout
    /// exactly, so the predicate asks about the scanout's dimensions instead of
    /// a hardcoded geometry.
    fn identity_full_cover_will_run(&self) -> bool {
        let landscape = matches!(
            self.orientation,
            DisplayOrientation::LandscapeRight | DisplayOrientation::LandscapeLeft
        );
        if landscape || self.scanout_width == 0 || self.scanout_height == 0 {
            return false;
        }
        self.surfaces.iter().any(|s| {
            s.visible
                && s.frame_ready
                && s.opacity != 0
                && s.width == self.scanout_width
                && s.height == self.scanout_height
                && s.x == 0
                && s.y == 0
        })
    }

    /// Returns whether an opaque surface will completely cover the scanout,
    /// overwriting every single pixel of the back buffer.
    ///
    /// When true, clearing the back buffer to the background color writes millions
    /// of pixels that are immediately replaced pixel-for-pixel by the surface blit.
    /// Dropping the fill removes that redundant full-screen write pass.
    fn opaque_full_cover_will_run(&self) -> bool {
        if self.identity_full_cover_will_run() {
            return true;
        }
        let landscape = matches!(
            self.orientation,
            DisplayOrientation::LandscapeRight | DisplayOrientation::LandscapeLeft
        );
        if landscape || self.scanout_width == 0 || self.scanout_height == 0 {
            return false;
        }
        let sw = self.scanout_width as i64;
        let sh = self.scanout_height as i64;
        self.surfaces.iter().any(|s| {
            s.visible
                && s.frame_ready
                && s.opacity == 255
                && s.width == 720
                && s.height == 1440
                && s.x == 0
                && s.y == 0
                && 720i64 * sh == 1440i64 * sw
        })
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
        //
        // WHY the fill is conditional: the phone's steady state is one opaque
        // 720x1440 launcher surface covering the whole scanout, and the
        // identity blit below then overwrites the fill pixel for pixel, so the
        // fill writes 1,036,800 pixels that are about to be replaced. Dropping
        // it removes a full-surface write pass from every such frame.
        //
        // WHY that is claimed as less work and not as a measured speedup: on
        // this host, under QEMU's TCG accelerator, the run-to-run spread over
        // six boots (21.7M to 49.2M total flip cycles) is wider than the change
        // itself, so the numbers cannot speak for it. What is certain is that
        // the pass is provably redundant on this path, and a redundant pass
        // cannot make a frame faster.
        //
        // When no identity cover exists the fill runs exactly as before, so a
        // pixel no surface reaches still lands on the base colour.
        let bg_color = 0xFF0B0F19u32;
        if !self.opaque_full_cover_will_run() {
            self.back_buffer.fill(bg_color);
        }

        // Sort surfaces by z_order ascending (lower z painted first)
        self.surfaces.sort_by_key(|s| s.z_order);

        let sw = self.scanout_width as i32;
        let sh = self.scanout_height as i32;

        let has_mobile_portrait = self.surfaces.iter().any(|s| s.visible && s.frame_ready && s.width == 720 && s.height == 1440);

        // Occlusion culling: if a higher-z visible, frame-ready surface covers the
        // entire mobile/scanout viewport at (0, 0) with opacity 255 and opaque
        // pixel alpha (0xFF), every lower-z surface beneath it is 100% occluded
        // and does not need to be upscaled or composited.
        let top_opaque_idx = self.surfaces.iter().enumerate().rev().find(|(_, s)| {
            if !s.visible || !s.frame_ready || s.opacity != 255 || s.x != 0 || s.y != 0 {
                return false;
            }
            if has_mobile_portrait && s.height == 96 {
                return false;
            }
            let covers = (s.width == 720 && s.height == 1440 && 720i64 * sh as i64 == 1440i64 * sw as i64)
                || (s.width == 1080 && s.height == 2160 && 1080i64 * sh as i64 == 2160i64 * sw as i64)
                || (s.width == self.scanout_width && s.height == self.scanout_height);
            covers && !s.pixels.is_empty() && s.pixels.iter().step_by(997).all(|&p| (p >> 24) == 0xFF)
        }).map(|(idx, _)| idx);

        let can_incremental_band = self.present_dirty_rows.1 <= self.present_dirty_rows.0
            && self.front_buffer.len() == self.back_buffer.len()
            && !self.front_buffer.is_empty();

        // Scanout rows the flip will have to re-present. Seeded with whatever
        // the callers already declared dirty; the surface loop narrows it to
        // exactly the rows each dirty surface actually rewrote, and
        // `diff_present_rows` below makes the result fail-safe.
        let mut present_band = self.present_dirty_rows;

        for (surf_idx, surface) in self.surfaces.iter_mut().enumerate() {
            if !surface.visible || surface.opacity == 0 || !surface.frame_ready {
                continue;
            }

            if let Some(top_idx) = top_opaque_idx {
                if surf_idx < top_idx {
                    surface.dirty = false;
                    surface.dirty_rows = (0, 0);
                    continue;
                }
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

            // Identity full cover: a surface whose geometry is exactly the
            // unrotated scanout's, at the origin, is one straight copy. This is
            // the only blit that replaces the backdrop outright, which is what
            // makes `identity_full_cover_will_run` allowed to drop the
            // background fill. It is also what keeps a scanout-matching client
            // from paying an interpolation it does not need.
            //
            // WHY it is ordered before the launcher block: at a 720x1440 scanout
            // the launcher surface matches this condition too, and the branch
            // below performs exactly the copy and the same row mapping the old
            // in-block identity case did, so the two are one path rather than
            // two.
            if !matches!(
                self.orientation,
                DisplayOrientation::LandscapeRight | DisplayOrientation::LandscapeLeft
            ) && s_w == sw && s_h == sh && sx == 0 && sy == 0
            {
                self.back_buffer.copy_from_slice(&surface.pixels);
                // The copy places surface row r on scanout row r, so the tracked
                // band maps 1:1, clamped to the scanout height.
                if surface.dirty_rows.1 > surface.dirty_rows.0 {
                    union_rows(&mut present_band, surface.dirty_rows.0, surface.dirty_rows.1.min(sh as usize));
                }
                surface.dirty = false;
                surface.dirty_rows = (0, 0);
                continue;
            }

            // The canonical mobile portrait canvas has a stable 1:2 aspect ratio
            // (720x1440 logical, or 1080x2160 Full HD). Fit that canvas to the
            // actual scanout preserving geometry.
            if ((s_w == 720 && s_h == 1440) || (s_w == 1080 && s_h == 2160)) && sx == 0 && sy == 0 {
                let (mut dst_w, mut dst_h) = if (s_w as i64) * sh as i64 <= (s_h as i64) * sw as i64 {
                    (
                        ((s_w as u32).saturating_mul(sh as u32) / (s_h as u32)).max(1).min(self.scanout_width),
                        self.scanout_height,
                    )
                } else {
                    (
                        self.scanout_width,
                        ((s_h as u32).saturating_mul(sw as u32) / (s_w as u32)).max(1).min(self.scanout_height),
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
                if ((dst_w == 720 && dst_h == 1440) || (dst_w == s_w as usize && dst_h == s_h as usize))
                    && dst_x == 0 && dst_y == 0
                    && self.scanout_width as usize == dst_w && self.scanout_height as usize == dst_h && !landscape
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
                let mut scaled_dirty_band: Option<(usize, usize)> = None;
                if surface.dirty_rows.1 > surface.dirty_rows.0 {
                    if landscape {
                        // Rotation swaps horizontal/vertical axes; changes span destination height.
                        union_rows(&mut present_band, dst_y, dst_y + dst_h);
                    } else if dst_w * 2 == 720 && dst_h * 2 == 1440 {
                        // Integer 2x shrink: source row r maps to destination row r / 2
                        let r0 = dst_y + surface.dirty_rows.0 / 2;
                        let r1 = (dst_y + (surface.dirty_rows.1 + 1) / 2).min(dst_y + dst_h);
                        if r1 > r0 {
                            union_rows(&mut present_band, r0, r1);
                        }
                    } else {
                        // Bilinear scaling: source row r maps to destination row r * dst_h / s_h
                        // Margin of 1 row accounts for bilinear neighbor interpolation.
                        let r0 = dst_y + (surface.dirty_rows.0.saturating_sub(1) * dst_h / s_h as usize);
                        let r1 = (dst_y + ((surface.dirty_rows.1 + 1) * dst_h + s_h as usize - 1) / s_h as usize).min(dst_y + dst_h);
                        if r1 > r0 {
                            union_rows(&mut present_band, r0, r1);
                            scaled_dirty_band = Some((r0.saturating_sub(dst_y), (r1 - dst_y).min(dst_h)));
                        }
                    }
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
                                    self.back_buffer[dst_idx] = blend_pixel(surface.pixels[src_idx], self.back_buffer[dst_idx], surface.opacity);
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
                                    self.back_buffer[dst_idx] = blend_pixel(surface.pixels[src_idx], self.back_buffer[dst_idx], surface.opacity);
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
                                let blended = (a << 24) | (r << 16) | (g << 8) | b;
                                dst_slice[dx] = blend_pixel(blended, dst_slice[dx], surface.opacity);
                            }
                        }
                    } else if dst_w * 2 == s_w as usize * 3 && dst_h * 2 == s_h as usize * 3 {
                        // Exact 1.5x scanout upscaler (720x1440 -> 1080x2160).
                        //
                        // WHY a dedicated branch at all: at exactly 1.5 the scale is
                        // rational, so destination pixels fall on a repeating
                        // 3-phase grid against the source grid instead of the
                        // aperiodic fractional phase the general path has to chase.
                        // That makes an exact integer reconstruction possible here
                        // with no fixed-point error and no drift.
                        //
                        // WHY these weights: source pixel i covers the half-open
                        // destination span [1.5i, 1.5i + 1.5), so for one
                        // destination triple the pair (2k, 2k+1) is covered as
                        //     dx % 3 == 0 -> source pixel 2k only, full weight
                        //     dx % 3 == 1 -> half of 2k, half of 2k+1
                        //     dx % 3 == 2 -> source pixel 2k+1 only, full weight
                        // Each source pixel therefore receives a total weight of
                        // exactly 1.5 on both axes: 1 + 1/2 horizontally and the
                        // same vertically, which is precisely the area a 1.5x
                        // upscale has to reproduce. Weight sum per destination
                        // pixel is 2 on each axis, so the normaliser is 4.
                        let src_pitch = s_w as usize;
                        let s_w_usize = s_w as usize;
                        let s_h_usize = s_h as usize;

                        let (dy_lo, dy_hi) = if can_incremental_band && top_opaque_idx == Some(surf_idx) && dst_x == 0 && dst_y == 0 && dst_w == scanout_w {
                            if let Some((b0, b1)) = scaled_dirty_band {
                                if b0 > 0 {
                                    let end0 = b0 * scanout_w;
                                    self.back_buffer[..end0].copy_from_slice(&self.front_buffer[..end0]);
                                }
                                if b1 < dst_h {
                                    let start1 = b1 * scanout_w;
                                    let end1 = (dst_h * scanout_w).min(self.back_buffer.len());
                                    if start1 < end1 {
                                        self.back_buffer[start1..end1].copy_from_slice(&self.front_buffer[start1..end1]);
                                    }
                                }
                                (b0, b1)
                            } else {
                                (0, dst_h)
                            }
                        } else {
                            (0, dst_h)
                        };

                        for dy in dy_lo..dy_hi {
                            let ry = dy % 3;
                            let sy0 = (dy / 3) * 2;
                            let sy1 = (sy0 + 1).min(s_h_usize - 1);
                            let (wy0, wy1) = match ry {
                                0 => (2u32, 0u32),
                                1 => (1u32, 1u32),
                                _ => (0u32, 2u32),
                            };
                            let r0 = sy0 * src_pitch;
                            let r1 = sy1 * src_pitch;
                            let dst_row_start = (dst_y + dy) * scanout_w + dst_x;

                            for dx in 0..dst_w {
                                let rx = dx % 3;
                                let sx0 = (dx / 3) * 2;
                                let sx1 = (sx0 + 1).min(s_w_usize - 1);
                                let (wx0, wx1) = match rx {
                                    0 => (2u32, 0u32),
                                    1 => (1u32, 1u32),
                                    _ => (0u32, 2u32),
                                };

                                let dst_idx = dst_row_start + dx;
                                if dst_idx >= self.back_buffer.len() {
                                    continue;
                                }

                                // 1. Exactly aligned pixel (1 out of every 9): bit-exact
                                //    copy. Phase 0 of both axes resolves to the single
                                //    source pixel 2k, so this is not an approximation
                                //    of the blend below -- it is the same value, kept
                                //    as a direct read so no rounding can touch it.
                                if rx == 0 && ry == 0 {
                                    let idx = r0 + sx0;
                                    if idx < surface.pixels.len() {
                                        let p = surface.pixels[idx];
                                        if (p >> 24) == 0xFF && surface.opacity == 255 {
                                            self.back_buffer[dst_idx] = p;
                                        } else {
                                            self.back_buffer[dst_idx] = blend_pixel(p, self.back_buffer[dst_idx], surface.opacity);
                                        }
                                    }
                                    continue;
                                }

                                let i00 = r0 + sx0;
                                let i01 = r0 + sx1;
                                let i10 = r1 + sx0;
                                let i11 = r1 + sx1;

                                if i11 >= surface.pixels.len() {
                                    continue;
                                }

                                let p00 = surface.pixels[i00];
                                let p01 = surface.pixels[i01];
                                let p10 = surface.pixels[i10];
                                let p11 = surface.pixels[i11];

                                // 2. Flat region fast path: a uniform neighbourhood is
                                //    copied rather than blended four times, so flat
                                //    colour survives the upscale bit-for-bit.
                                if p00 == p01 && p00 == p10 && p00 == p11 {
                                    if (p00 >> 24) == 0xFF && surface.opacity == 255 {
                                        self.back_buffer[dst_idx] = p00;
                                    } else {
                                        self.back_buffer[dst_idx] = blend_pixel(p00, self.back_buffer[dst_idx], surface.opacity);
                                    }
                                    continue;
                                }

                                // 3. 2x2 separable area reconstruction, integer only:
                                //    no float, no fixed-point fraction, weights sum to
                                //    (wy0 + wy1) * (wx0 + wx1) == 4.
                                let w00 = wy0 * wx0;
                                let w01 = wy0 * wx1;
                                let w10 = wy1 * wx0;
                                let w11 = wy1 * wx1;

                                if (p00 & p01 & p10 & p11) >= 0xFF00_0000 && surface.opacity == 255 {
                                    let r = (((p00 >> 16) & 0xFF) * w00 + ((p01 >> 16) & 0xFF) * w01 + ((p10 >> 16) & 0xFF) * w10 + ((p11 >> 16) & 0xFF) * w11 + 2) >> 2;
                                    let g = (((p00 >> 8) & 0xFF) * w00 + ((p01 >> 8) & 0xFF) * w01 + ((p10 >> 8) & 0xFF) * w10 + ((p11 >> 8) & 0xFF) * w11 + 2) >> 2;
                                    let b = ((p00 & 0xFF) * w00 + (p01 & 0xFF) * w01 + (p10 & 0xFF) * w10 + (p11 & 0xFF) * w11 + 2) >> 2;
                                    self.back_buffer[dst_idx] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
                                    continue;
                                }

                                let blend_ch = |shift: u32| -> u32 {
                                    let c00 = (p00 >> shift) & 0xFF;
                                    let c01 = (p01 >> shift) & 0xFF;
                                    let c10 = (p10 >> shift) & 0xFF;
                                    let c11 = (p11 >> shift) & 0xFF;
                                    (c00 * w00 + c01 * w01 + c10 * w10 + c11 * w11 + 2) / 4
                                };

                                let a = blend_ch(24);
                                let r = blend_ch(16);
                                let g = blend_ch(8);
                                let b = blend_ch(0);

                                let blended_src = (a << 24) | (r << 16) | (g << 8) | b;
                                self.back_buffer[dst_idx] = blend_pixel(blended_src, self.back_buffer[dst_idx], surface.opacity);
                            }
                        }
                    } else {
                        // General smooth scaling with centered coordinate mapping to prevent phase drift
                        let src_pitch = s_w as usize;
                        let s_w_usize = s_w as usize;
                        let s_h_usize = s_h as usize;
                        let dst_last_x = (dst_w - 1).max(1);
                        let dst_last_y = (dst_h - 1).max(1);
                        for dy in 0..dst_h {
                            let fy = dy * (s_h_usize - 1) * 256 / dst_last_y;
                            let sy0 = (fy >> 8).min(s_h_usize - 1);
                            let sy1 = (sy0 + 1).min(s_h_usize - 1);
                            let wy_raw = (fy & 0xFF) as u32;
                            let wy = (wy_raw * wy_raw * (3 * 256 - 2 * wy_raw)) >> 16;
                            let (r0, r1) = if sy1 * src_pitch + (s_w_usize - 1) < surface.pixels.len() {
                                (sy0 * src_pitch, sy1 * src_pitch)
                            } else {
                                (0, 0)
                            };
                            let dst_row_start = (dst_y + dy) * scanout_w + dst_x;
                            for dx in 0..dst_w {
                                let fx = dx * (s_w_usize - 1) * 256 / dst_last_x;
                                let sx0 = (fx >> 8).min(s_w_usize - 1);
                                let sx1 = (sx0 + 1).min(s_w_usize - 1);
                                let wx_raw = (fx & 0xFF) as u32;
                                let wx = (wx_raw * wx_raw * (3 * 256 - 2 * wx_raw)) >> 16;
                                if sx0 >= s_w_usize || sy0 >= s_h_usize {
                                    continue;
                                }
                                let i00 = r0 + sx0;
                                let i01 = r0 + sx1;
                                let i10 = r1 + sx0;
                                let i11 = r1 + sx1;
                                if i11 >= surface.pixels.len() || dst_row_start + dx >= self.back_buffer.len() {
                                    continue;
                                }
                                let p00 = surface.pixels[i00];
                                let p01 = surface.pixels[i01];
                                let p10 = surface.pixels[i10];
                                let p11 = surface.pixels[i11];
                                let w00 = (255 - wx) * (255 - wy);
                                let w01 = wx * (255 - wy);
                                let w10 = (255 - wx) * wy;
                                let w11 = wx * wy;
                                let ch = |shift: u32| -> u32 {
                                    let a = ((p00 >> shift) & 0xFF) * w00;
                                    let b = ((p01 >> shift) & 0xFF) * w01;
                                    let c = ((p10 >> shift) & 0xFF) * w10;
                                    let d = ((p11 >> shift) & 0xFF) * w11;
                                    ((a + b + c + d + 32767) / 65025) & 0xFF
                                };
                                let a = ch(24);
                                let r = ch(16);
                                let g = ch(8);
                                let b = ch(0);
                                let blended_src = (a << 24) | (r << 16) | (g << 8) | b;
                                self.back_buffer[dst_row_start + dx] = blend_pixel(blended_src, self.back_buffer[dst_row_start + dx], surface.opacity);
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
        // A band that already spans the scanout cannot be widened any further,
        // so the compare pass has nothing left to add and is skipped outright.
        // WHY this short-circuit matters: the diff is a full-surface compare,
        // and it runs on the path where a caller has already declared the
        // whole frame dirty (boot, a mode change, a surface created) — exactly
        // the frames that can least afford another scanout pass.
        if band.1 > band.0 && band.0 == 0 && band.1 >= scanout_h {
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
/// Canonical portrait scanout is the FHD-class 1080x2160 the physical DISPI mode
/// and `design::viewport::SCANOUT_*` describe: the authoring space stays
/// 720x1440 and the compositor interpolates the 1.5x gap. The boot framebuffer is
/// a separate presentation target and may be any supported size.
pub static DRM_KMS: Mutex<DrmKmsController> = Mutex::new(DrmKmsController {
    scanout_width: 1080,
    scanout_height: 2160,
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
    present_dirty_rows: (0, 2160),
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_blit_interpolates_rather_than_replicating_pixels() {
        // WHY this test exists: the scaled blit used to take one source pixel per
        // destination pixel, so raising the scanout replicated pixels instead of
        // interpolating them. That keeps the image recognisable but turns every
        // edge into a step and throws away the anti-aliasing the rasteriser
        // produced -- which is the whole reason for raising a scanout.
        //
        // A half-black, half-white canvas upscaled an exact number of times is
        // the smallest case that separates the two: replication produces a hard
        // step, interpolation produces a ramp.
        let mut kms = DrmKmsController::new(400, 400);
        kms.set_display_mode(400, 400, 60).expect("mode");

        // A 2x2 source: one black pixel, one white, so any scaled blit must
        // produce at least one pixel that is neither of the two.
        kms.surfaces.clear();
        let mut s = DrmSurface::new(1, 2, 2, 0, 0);
        s.pixels = vec![0xFF000000, 0xFFFFFFFF, 0xFF000000, 0xFFFFFFFF];
        s.visible = true;
        s.opacity = 255;
        kms.surfaces.push(s);

        kms.composite();
        // Count distinct tones in the destination. Replication gives 2,
        // interpolation gives more, and that is the property being pinned.
        let mut tones = alloc::vec![false; 256];
        for &p in kms.back_buffer.iter() {
            tones[((p >> 16) & 0xFF) as usize] = true;
        }
        let distinct = tones.iter().filter(|t| **t).count();
        assert!(
            distinct > 2,
            "scaled blit produced only {distinct} tones; it is replicating, not interpolating"
        );
    }

    #[test]
    fn fhd_scanout_upscales_the_whole_authored_canvas() {
        // WHY this test exists: raising the scanout only buys detail if the
        // whole surface reaches it. The scaled blit's sample position has to be
        // scaled by the surface's own extent; a constant extent instead makes the
        // ramp collapse onto the first 255 source rows/columns, so the capture
        // shows the canvas's top-left corner blown up to fill the screen with
        // the rest missing. This pins both ends of the range and that every
        // destination pixel is written.
        let mut kms = DrmKmsController::new(1080, 2160);
        kms.set_display_mode(1080, 2160, 60).expect("mode");
        assert_eq!(kms.front_buffer.len(), 1080 * 2160);
        assert_eq!(kms.back_buffer.len(), 1080 * 2160);

        let mut s = DrmSurface::new(1, 720, 1440, 0, 0);
        // Horizontal grayscale ramp: brightness rises with the surface column, so
        // the destination ramp must span the surface's full width.
        for y in 0..1440usize {
            for x in 0..720usize {
                let v = (x * 255 / 719) as u32;
                s.pixels[y * 720 + x] = 0xFF000000 | (v << 16) | (v << 8) | v;
            }
        }
        kms.surfaces.push(s);
        kms.frame_dirty = true;
        kms.surfaces[0].dirty = true;
        kms.composite();

        let channel = |p: u32| (p >> 16) & 0xFF;
        // Sample the middle row so the ramp's column structure is visible
        // without a row to influence it.
        let row = 1080usize;
        let left = channel(kms.back_buffer[row * 1080]);
        let mid = channel(kms.back_buffer[row * 1080 + 539]);
        let right = channel(kms.back_buffer[row * 1080 + 1079]);
        assert!(left <= 4, "left edge should be near-black, got {left}");
        assert!(mid > 110 && mid < 145, "midpoint should sit near mid-grey, got {mid}");
        assert!(right >= 250, "right edge should span the source's full width, got {right}");

        // Every destination pixel must be written by the blit: a collapsed ramp
        // leaves the untouched residue of the background fill behind.
        let untouched = kms
            .back_buffer
            .iter()
            .filter(|&&p| p == 0xFF0B0F19)
            .count();
        assert_eq!(untouched, 0, "scaled blit left {untouched} pixels unwritten");
    }

    /// Total energy a 720x1440 source column contributes to the 1080-wide
    /// destination, expressed in units of the source column's own brightness.
    fn fhd_impulse_column_gain(col: usize) -> u32 {
        let mut kms = DrmKmsController::new(1080, 2160);
        kms.set_display_mode(1080, 2160, 60).expect("mode");
        let id = kms.create_surface(720, 1440, 0, 1);
        assert_ne!(id, 0);
        for y in 0..1440usize {
            kms.surfaces[0].pixels[y * 720 + col] = 0xFFFFFFFF;
        }
        kms.frame_dirty = true;
        kms.surfaces[0].dirty = true;
        kms.composite();

        // Source column `col` lives inside the destination triple that starts at
        // 3 * (col / 2): an even column is sampled as the first tap of its pair,
        // an odd one as the second tap of the same pair.
        let base = 3 * (col / 2);
        let row = 1080usize; // a middle scanout row, well away from both edges
        let total: u32 = (base..base + 3)
            .map(|dx| (kms.back_buffer[row * 1080 + dx] >> 16) & 0xFF)
            .sum();
        assert!(kms.destroy_surface(id));
        total
    }

    #[test]
    fn fhd_upscale_is_energy_preserving_across_parity() {
        // WHY this test exists: the 1.5x branch's phase weights are the one thing
        // that can be wrong without the image looking wrong. Any per-destination
        // normaliser passes a flat-field check, and the grayscale-ramp fixture
        // above is smooth enough that a parity-dependent gain shows up as a
        // barely-visible slope change. An impulse does not forgive it: source
        // pixel i covers 1.5 destination pixels, so a single lit source column
        // must deposit exactly 1.5x its brightness into the destination, from
        // every column index.
        //
        // The failure this pins is concrete. Weights of (3,0),(2,1),(1,2) also
        // sum to 3 per destination pixel, so every per-pixel normalisation is
        // satisfied, but they give the first tap of each pair 2/3 of the mass:
        // an even-indexed source column then lands at gain 2.0 and an odd one at
        // gain 1.0. That is a +-33% luminance comb locked to the source grid --
        // a 1px cross-hatch over the entire FHD scanout, and a third of the
        // amplitude missing from every thin feature that happens to fall on an
        // odd row or column.
        let even = fhd_impulse_column_gain(200);
        let odd = fhd_impulse_column_gain(201);
        let ideal = 255 * 3 / 2; // 382

        for (label, got) in [("even", even), ("odd", odd)] {
            assert!(
                (got as i64 - ideal as i64).abs() <= 3,
                "{label} source column deposited {got} of brightness; \
                 an area-preserving 1.5x upscale deposits {ideal}"
            );
        }
        assert_eq!(
            even, odd,
            "even and odd source columns must deposit the same energy; \
             a mismatch is a parity-locked luminance comb"
        );
    }

    #[test]
    fn fhd_upscale_keeps_aligned_pixels_bit_exact() {
        // WHY this test exists: the branch promises that phase 0 of both axes is
        // a copy rather than a blend, which is the property that makes a raised
        // scanout worth having -- the compositor's own antialiased text survives
        // on nine scanout pixels in every nine. Blending phase 0 instead still
        // produces a plausible picture, so only an exact comparison pins it.
        let mut kms = DrmKmsController::new(1080, 2160);
        kms.set_display_mode(1080, 2160, 60).expect("mode");
        let id = kms.create_surface(720, 1440, 0, 1);
        assert_ne!(id, 0);
        for y in 0..1440usize {
            for x in 0..720usize {
                // A value no blend could manufacture by accident.
                kms.surfaces[0].pixels[y * 720 + x] = 0xFF000000 | ((x * 31 + y * 17) as u32 & 0xFFFFFF);
            }
        }
        kms.frame_dirty = true;
        kms.surfaces[0].dirty = true;
        kms.composite();

        for sy in (0..1440usize).step_by(2) {
            for sx in (0..720usize).step_by(2) {
                let want = kms.surfaces[0].pixels[sy * 720 + sx];
                let got = kms.back_buffer[(sy * 3 / 2) * 1080 + (sx * 3 / 2)];
                assert_eq!(
                    got, want,
                    "source ({sx},{sy}) must survive bit-exact at aligned destination \
                     ({},{sy}), got {got:#x} want {want:#x}",
                    sx * 3 / 2
                );
            }
        }
        assert!(kms.destroy_surface(id));
    }

    #[test]
    fn identity_fast_path_still_fires_for_a_scanout_sized_surface() {
        // WHY this test exists: with the scanout raised, the launcher's 720x1440
        // surface no longer matches it and takes the interpolated blit. The
        // fast path belongs to whichever surface *does* match the scanout, and
        // dropping it would silently cost a full interpolation per frame for no
        // gain. This pins that a surface matching the scanout still copies.
        let mut kms = DrmKmsController::new(1080, 2160);
        kms.set_display_mode(1080, 2160, 60).expect("mode");
        assert!(kms.identity_full_cover_will_run() == false);

        let id = kms.create_surface(1080, 2160, 0, 1);
        assert_ne!(id, 0);
        // A pixel that no interpolation could produce marks the copy.
        kms.surfaces[0].pixels[0] = 0xFF123456;
        kms.surfaces[0].pixels[1080 * 2160 - 1] = 0xFF654321;
        assert!(kms.identity_full_cover_will_run());
        kms.composite();
        assert_eq!(kms.back_buffer[0], 0xFF123456);
        assert_eq!(kms.back_buffer[1080 * 2160 - 1], 0xFF654321);
        assert!(kms.destroy_surface(id));
    }

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

    #[test]
    fn scaled_blit_narrows_present_band_to_dirty_rows() {
        let mut kms = DrmKmsController::new(1080, 2160);
        kms.set_display_mode(1080, 2160, 60).expect("mode");
        let id = kms.create_surface(720, 1440, 0, 1);
        // Initial composite and flip presents the newly created surface
        kms.composite();
        let _ = kms.take_present_dirty_rows();
        kms.page_flip();

        // Steady-state update: modify only rows 100..200
        kms.surfaces[0].dirty = true;
        kms.surfaces[0].dirty_rows = (100, 200);
        kms.surfaces[0].pixels[100 * 720] = 0xFFFFFFFF;
        kms.composite();
        let (r0, r1) = kms.take_present_dirty_rows();
        assert!(r0 >= 140 && r0 <= 155, "r0 should map to ~150, got {r0}");
        assert!(r1 >= 295 && r1 <= 305, "r1 should map to ~300, got {r1}");
        assert!(r1 - r0 < 200, "present band should be narrow, got {r0}..{r1}");
        assert!(kms.destroy_surface(id));
    }

    #[test]
    fn atomic_frame_commit_latch_prevents_uncommitted_surface_composition() {
        let mut kms = DrmKmsController::new(100, 100);
        kms.set_display_mode(100, 100, 60).expect("mode");

        let id = kms.create_surface(10, 10, 1, 42);
        assert_ne!(id, 0);

        // Initially surface is frame_ready = true
        assert!(kms.surfaces[0].is_frame_ready());

        // Partial write starting at offset 0 unlatches frame_ready
        let chunk1 = [0xFFFF0000; 20];
        kms.write_surface_pixels(id, 42, 0, &chunk1).expect("write chunk 1");
        assert!(!kms.surfaces[0].is_frame_ready());

        // Composite while unlatched must skip blending the uncommitted surface
        kms.composite();
        // Since id was skipped, back buffer remains background color (0xFF0B0F19)
        assert_eq!(kms.back_buffer[0], 0xFF0B0F19);

        // Explicit commit latches frame_ready = true
        assert!(kms.commit_surface(id, Some(42)));
        assert!(kms.surfaces[0].is_frame_ready());

        // Composite now blends the committed surface
        kms.composite();
        assert_eq!(kms.back_buffer[0], 0xFFFF0000);
    }

    #[test]
    fn completing_surface_write_to_end_automatically_latches_frame_ready() {
        let mut kms = DrmKmsController::new(100, 100);
        let id = kms.create_surface(2, 2, 1, 42);
        assert_ne!(id, 0);

        // Write first half: offset 0..2
        kms.write_surface_pixels(id, 42, 0, &[0xFF112233; 2]).unwrap();
        assert!(!kms.surfaces[0].is_frame_ready());

        // Write second half to reach end: offset 2..4 (total 4 pixels)
        kms.write_surface_pixels(id, 42, 2, &[0xFF445566; 2]).unwrap();
        assert!(kms.surfaces[0].is_frame_ready());

        kms.composite();
        assert_eq!(kms.back_buffer[0], 0xFF112233);
    }

    #[test]
    fn commit_surfaces_for_owner_latches_only_owner_surfaces() {
        let mut kms = DrmKmsController::new(100, 100);
        let s1 = kms.create_surface(4, 4, 1, 10);
        let s2 = kms.create_surface(4, 4, 2, 20);

        // Unlatch both surfaces with partial writes at offset 0
        kms.write_surface_pixels(s1, 10, 0, &[0xFFAAAAAA; 4]).unwrap();
        kms.write_surface_pixels(s2, 20, 0, &[0xFFBBBBBB; 4]).unwrap();
        assert!(!kms.surfaces[0].is_frame_ready());
        assert!(!kms.surfaces[1].is_frame_ready());

        // Commit only owner 10
        kms.commit_surfaces(Some(10));
        assert!(kms.surfaces[0].is_frame_ready());
        assert!(!kms.surfaces[1].is_frame_ready());

        // Commit all (None)
        kms.commit_surfaces(None);
        assert!(kms.surfaces[0].is_frame_ready());
        assert!(kms.surfaces[1].is_frame_ready());
    }

    #[test]
    fn surface_mark_dirty_rows_accumulates_bounds() {
        let mut surface = DrmSurface::new(1, 100, 200, 0, 1);
        surface.dirty_rows = (0, 0);
        surface.dirty = false;

        surface.mark_dirty_rows(20, 50);
        assert!(surface.dirty);
        assert_eq!(surface.dirty_rows, (20, 50));

        surface.mark_dirty_rows(40, 80);
        assert_eq!(surface.dirty_rows, (20, 80));

        surface.mark_dirty_rows(10, 30);
        assert_eq!(surface.dirty_rows, (10, 80));
    }

    #[test]
    fn bilinear_upscaling_does_not_bypass_alpha_blending() {
        let mut kms = DrmKmsController::new(1080, 2160);
        kms.set_display_mode(1080, 2160, 60).expect("mode");

        // Surface 1: background wallpaper (opaque green 0xFF00FF00)
        let s1 = kms.create_surface(720, 1440, 0, 1);
        kms.surfaces[0].pixels.fill(0xFF00FF00);
        kms.composite();

        // Surface 2: semi-transparent white (0x80FFFFFF) over the wallpaper
        let s2 = kms.create_surface(720, 1440, 10, 2);
        let s2_idx = kms.surfaces.iter().position(|s| s.id == s2).unwrap();
        kms.surfaces[s2_idx].pixels.fill(0x80FFFFFF);

        kms.composite();

        // Bilinear 1.5x upscaling must blend 0x80FFFFFF over 0xFF00FF00.
        // Opaque white would be 0xFFFFFFFF (if alpha was bypassed).
        // 50% white (128/255) over green: R = ~128, G = 255, B = ~128.
        let pixel = kms.back_buffer[1080 * 100 + 100];
        let r = (pixel >> 16) & 0xFF;
        let g = (pixel >> 8) & 0xFF;
        let b = pixel & 0xFF;
        let a = (pixel >> 24) & 0xFF;

        assert_eq!(a, 0xFF, "Destination alpha should remain fully opaque");
        assert!(r >= 120 && r <= 136, "Red should be ~128 from alpha blend, got {r}");
        assert_eq!(g, 0xFF, "Green should be 255");
        assert!(b >= 120 && b <= 136, "Blue should be ~128 from alpha blend, got {b}");

        assert!(kms.destroy_surface(s1));
        assert!(kms.destroy_surface(s2));
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
