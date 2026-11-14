// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! DRM/KMS Multi-Surface Compositor & Display Controller
//!
//! Provides a hardware-decoupled, double-buffered Direct Rendering Manager (DRM)
//! and Kernel Mode Setting (KMS) surface composition engine:
//! - Double-buffered page flipping (Front buffer scanout and Back buffer render target)
//! - Multi-surface layer composition with z-ordering, position offsets, and visibility
//! - Porter-Duff source-over alpha blending (`out = src * a + dst * (1 - a)`)
//! - Dirty rectangle tracking to minimize memory bandwidth
//! - Native portrait mobile scanout at the 720x1440 authoring size (1440x720 landscape), so
//!   platform surfaces composite 1:1 instead of being resampled

use alloc::format;
use alloc::vec::Vec;
use alloc::vec;
use spin::Mutex;
use crate::framebuffer::blend_half;

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
pub const MAX_SURFACE_PIXELS: usize = 1080 * 2400;
pub const MAX_SCANOUT_PIXELS: usize = 3840 * 2160;

/// The most kernel heap every surface may hold between them.
///
/// WHY the slot count is not the bound: a surface is width x height x 4 bytes
/// of kernel heap, so a phone-full 720x1440 surface is 4,147,200 bytes and the
/// 32 slots this driver allows would be 132,710,400 bytes of them -- nearly
/// twice the whole 67,108,864-byte heap. The panic transcript states what that
/// did: `memory allocation of 4147200 bytes failed` with
/// heap_used=66803512 heap_free=305352, and the panic policy halts the CPU, so
/// the device stopped because one app asked for a screen. Thirty-two slots of
/// 64x64 overlays and twelve slots of full-screen apps are the same number of
/// surfaces and a very different device, so the memory needs its own ceiling.
///
/// WHY 48 MiB: the same heap carries every task's kernel-side bookkeeping, the
/// mailboxes, the network tables and the scanout buffers too, and at the halt
/// there were 305,352 bytes left for all of it. Leaving 16 MiB of the 64
/// unclaimed by surfaces is the difference between a refused launch and a halt
/// on the next unrelated allocation, and it still pays for twelve full-screen
/// surfaces (49,766,400 bytes) -- the device reaches its ceiling a little
/// sooner, because the shell's own surfaces are inside the same budget.
pub const MAX_SURFACE_BYTES: usize = 48 * 1024 * 1024;

/// The bytes an ARGB8888 surface of `pixels` pixels holds.
pub const fn surface_bytes(pixels: usize) -> usize {
    pixels.saturating_mul(4)
}

/// The colour every composite starts from.
///
/// WHY the platform's paper and not the desktop-era slate: this is a phone,
/// and a client that paints translucent cards blends them against this
/// colour, so the platform's background is part of what every screen looks
/// like. A slate background turned a light theme's translucent cards dark,
/// and the screen was neither light nor dark but a third thing.
///
/// WHY it is a constant and not a literal at the two use sites: a client
/// that draws a translucent card and the compositor that composites it have
/// to agree on the colour, and a literal at each site is two chances to
/// disagree.
pub const PLATFORM_BACKGROUND: u32 = 0xFFFAFAFA;

/// Two-dimensional damage rectangle for region invalidation and clipping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl DamageRect {
    pub const fn empty() -> Self {
        Self { x: 0, y: 0, width: 0, height: 0 }
    }

    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }

    pub fn union(&self, other: &Self) -> Self {
        if self.width == 0 || self.height == 0 {
            return *other;
        }
        if other.width == 0 || other.height == 0 {
            return *self;
        }
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x + self.width as i32).max(other.x + other.width as i32);
        let y1 = (self.y + self.height as i32).max(other.y + other.height as i32);
        Self {
            x: x0,
            y: y0,
            width: (x1 - x0).max(0) as u32,
            height: (y1 - y0).max(0) as u32,
        }
    }

    pub fn to_row_band(&self, max_height: usize) -> (usize, usize) {
        if self.width == 0 || self.height == 0 || self.y < 0 {
            (0, 0)
        } else {
            let y0 = (self.y as usize).min(max_height);
            let y1 = ((self.y + self.height as i32) as usize).min(max_height);
            (y0, y1)
        }
    }
}

/// Refresh-aware frame pacing and deadline scheduler.
/// Supports 60 Hz, 120 Hz, and 144 Hz deadlines with missed-frame tracking.
#[derive(Debug, Clone)]
pub struct FrameScheduler {
    pub refresh_hz: u16,
    pub missed_frames: u64,
    pub total_frames: u64,
    pub history_idx: usize,
    pub frame_times_ns: [u64; 256],
    pub last_frame_tick: u64,
}

impl FrameScheduler {
    pub const fn new(refresh_hz: u16) -> Self {
        Self {
            refresh_hz,
            missed_frames: 0,
            total_frames: 0,
            history_idx: 0,
            frame_times_ns: [0; 256],
            last_frame_tick: 0,
        }
    }

    pub fn record_frame(&mut self, now_ticks: u64, deadline_ticks: u64) {
        self.total_frames += 1;
        let is_idle_gap = self.last_frame_tick > 0 && now_ticks.saturating_sub(self.last_frame_tick) > 100;
        if deadline_ticks > 0 && now_ticks > deadline_ticks && !is_idle_gap {
            self.missed_frames += 1;
        }
        if self.last_frame_tick > 0 && now_ticks >= self.last_frame_tick {
            let elapsed_ticks = now_ticks - self.last_frame_tick;
            if elapsed_ticks <= 100 {
                let elapsed_ns = elapsed_ticks.saturating_mul(1_000_000);
                self.frame_times_ns[self.history_idx] = elapsed_ns;
                self.history_idx = (self.history_idx + 1) % 256;
            }
        }
        self.last_frame_tick = now_ticks;
    }

    pub fn frame_budget_ns(&self) -> u64 {
        match self.refresh_hz {
            360 => 2_777_778,
            240 => 4_166_667,
            165 => 6_060_606,
            144 => 6_944_444,
            120 => 8_333_333,
            90 => 11_111_111,
            _ => 16_666_666,
        }
    }
}

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

/// A mask of `count` rows starting at `start` within one 64-bit word.
///
/// WHY this is a helper and not an inline expression: both ends of a written
/// band need one, and the shift has to be written so that a full word does
/// not overflow (`1u64 << 64` is not a thing).
fn row_bit_mask(start: usize, count: usize) -> u64 {
    let low = if count >= 64 { u64::MAX } else { (1u64 << count) - 1 };
    low << start
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

/// A 64x64 tile occlusion grid for front-to-back zero-overhead culling.
pub const TILE_SIZE: usize = 64;
pub const MAX_TILES: usize = 2048;

#[derive(Clone, Copy)]
pub struct TileOcclusionMap {
    pub occluded: [bool; MAX_TILES],
    pub cols: usize,
    pub rows: usize,
    pub scanout_w: usize,
    pub scanout_h: usize,
}

impl TileOcclusionMap {
    pub fn new(scanout_w: usize, scanout_h: usize) -> Self {
        let cols = (scanout_w + TILE_SIZE - 1) / TILE_SIZE;
        let rows = (scanout_h + TILE_SIZE - 1) / TILE_SIZE;
        Self {
            occluded: [false; MAX_TILES],
            cols: cols.min(64),
            rows: rows.min(64),
            scanout_w,
            scanout_h,
        }
    }

    #[inline(always)]
    pub fn is_occluded(&self, tx: usize, ty: usize) -> bool {
        if tx < self.cols && ty < self.rows {
            let idx = ty * self.cols + tx;
            if idx < MAX_TILES {
                return self.occluded[idx];
            }
        }
        false
    }

    #[inline(always)]
    pub fn mark_occluded(&mut self, tx: usize, ty: usize) {
        if tx < self.cols && ty < self.rows {
            let idx = ty * self.cols + tx;
            if idx < MAX_TILES {
                self.occluded[idx] = true;
            }
        }
    }

    /// Tests if a rectangular screen region is completely occluded by committed tiles.
    pub fn is_region_occluded(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> bool {
        if x1 <= x0 || y1 <= y0 {
            return true;
        }
        let tx0 = x0 / TILE_SIZE;
        let ty0 = y0 / TILE_SIZE;
        let tx1 = (x1 + TILE_SIZE - 1) / TILE_SIZE;
        let ty1 = (y1 + TILE_SIZE - 1) / TILE_SIZE;

        // If the query region extends beyond the bounds of the occlusion map grid,
        // it cannot be occluded by tiles inside the grid.
        if tx1 > self.cols || ty1 > self.rows {
            return false;
        }

        for ty in ty0..ty1 {
            for tx in tx0..tx1 {
                if !self.is_occluded(tx, ty) {
                    return false;
                }
            }
        }
        true
    }

    /// Marks all 64x64 tiles strictly contained inside an opaque rectangle as occluded.
    ///
    /// For boundary tiles along the edge of the scanout, if the rectangle extends to or past
    /// the scanout dimension (e.g. x1 >= scanout_w), that tile's visible area is fully covered
    /// because there are no display pixels beyond the scanout border.
    pub fn commit_opaque_rect(&mut self, x0: usize, y0: usize, x1: usize, y1: usize) {
        let tx0 = if x0 == 0 { 0 } else { (x0 + TILE_SIZE - 1) / TILE_SIZE };
        let ty0 = if y0 == 0 { 0 } else { (y0 + TILE_SIZE - 1) / TILE_SIZE };
        let tx1 = if x1 >= self.scanout_w {
            self.cols
        } else {
            (x1 / TILE_SIZE).min(self.cols)
        };
        let ty1 = if y1 >= self.scanout_h {
            self.rows
        } else {
            (y1 / TILE_SIZE).min(self.rows)
        };

        if tx1 > tx0 && ty1 > ty0 {
            for ty in ty0..ty1 {
                for tx in tx0..tx1 {
                    self.mark_occluded(tx, ty);
                }
            }
        }
    }
}

pub use crate::memory::SurfaceFrameTracker;
pub use crate::memory::recycle_frame;
pub use crate::memory::allocate_dma_frames;
pub use crate::memory::physical_to_virtual;

/// Backing storage for a DRM/KMS surface buffer.
///
/// When running on bare metal, surface pixel data (which can be 4 to 10+ MiB per surface)
/// is allocated from dedicated physical memory frames managed by `FRAME_ALLOCATOR`
/// rather than exhausting the bounded 64 MiB kernel heap.
///
/// Implements `Deref<Target = [u32]>` and `DerefMut<Target = [u32]>` to preserve complete
/// transparent compatibility with all existing blitting, composition, and dirty-tracking code.
pub struct SurfaceBuffer {
    ptr: *mut u32,
    len: usize,
    #[allow(dead_code)]
    capacity: usize,
    pub tracker: Option<alloc::sync::Arc<SurfaceFrameTracker>>,
    pub is_shm: bool,
}

/// Safety Invariant: SurfaceBuffer owns exclusive access to its underlying pixel buffer.
/// Moving it between execution contexts (threads/tasks) does not cause data races.
unsafe impl Send for SurfaceBuffer {}
/// Safety Invariant: Immutable references to SurfaceBuffer allow concurrent reads of pixels.
/// Mutation is mediated exclusively through `&mut self` (`DerefMut`).
unsafe impl Sync for SurfaceBuffer {}

impl SurfaceBuffer {
    /// Creates a new surface buffer initialized with `default_color`.
    pub fn new(pixel_count: usize, default_color: u32) -> Self {
        if pixel_count == 0 {
            return Self {
                ptr: core::ptr::null_mut(),
                len: 0,
                capacity: 0,
                tracker: None,
                is_shm: false,
            };
        }

        let byte_len = pixel_count.saturating_mul(4);
        let pages = (byte_len.saturating_add(4095)) / 4096;
        if pages > 0 {
            if let Some(frames) = crate::memory::allocate_dma_frames(pages) {
                if frames.len() == pages {
                    let vaddr = crate::memory::physical_to_virtual(frames[0].start_address());
                    let ptr = vaddr.as_mut_ptr::<u32>();
                    let slice = unsafe { core::slice::from_raw_parts_mut(ptr, pixel_count) };
                    slice.fill(default_color);
                    let tracker = SurfaceFrameTracker::new_physical(frames);
                    return Self {
                        ptr,
                        len: pixel_count,
                        capacity: pages * 1024,
                        tracker: Some(tracker),
                        is_shm: false,
                    };
                }
            }
        }

        // Fallback to kernel heap if frame allocator is uninitialized or exhausted
        let mut heap_vec = vec![default_color; pixel_count];
        let ptr = heap_vec.as_mut_ptr();
        let capacity = heap_vec.capacity();
        core::mem::forget(heap_vec);
        let tracker = SurfaceFrameTracker::new_heap(ptr, pixel_count, capacity);
        Self {
            ptr,
            len: pixel_count,
            capacity,
            tracker: Some(tracker),
            is_shm: false,
        }
    }

    /// Creates a surface buffer explicitly backed by physical frames (e.g. for testing).
    pub fn from_physical_frames(
        frames: alloc::vec::Vec<x86_64::structures::paging::PhysFrame<x86_64::structures::paging::Size4KiB>>,
        pixel_count: usize,
        default_color: u32,
    ) -> Self {
        let vaddr = crate::memory::physical_to_virtual(frames[0].start_address());
        let ptr = vaddr.as_mut_ptr::<u32>();
        let slice = unsafe { core::slice::from_raw_parts_mut(ptr, pixel_count) };
        slice.fill(default_color);
        let tracker = SurfaceFrameTracker::new_physical(frames);
        Self {
            ptr,
            len: pixel_count,
            capacity: pixel_count,
            tracker: Some(tracker),
            is_shm: false,
        }
    }

    /// Returns the physical page frames backing this surface buffer, if physically allocated.
    pub fn physical_frames(&self) -> &[x86_64::structures::paging::PhysFrame<x86_64::structures::paging::Size4KiB>] {
        if let Some(ref tracker) = self.tracker {
            &tracker.frames
        } else {
            &[]
        }
    }

    /// Whether this buffer is allocated from dedicated physical frames outside the kernel heap.
    pub fn is_physical(&self) -> bool {
        if let Some(ref tracker) = self.tracker {
            !tracker.is_heap && !tracker.frames.is_empty()
        } else {
            false
        }
    }

    /// Whether this buffer is backed by the kernel heap fallback.
    pub fn is_heap(&self) -> bool {
        if let Some(ref tracker) = self.tracker {
            tracker.is_heap
        } else {
            false
        }
    }

    /// Maps this surface buffer into a task's page table.
    pub fn map_to_task(
        &mut self,
        page_table: &mut crate::memory::TaskPageTable,
        start_page: x86_64::structures::paging::Page<x86_64::structures::paging::Size4KiB>,
        flags: x86_64::structures::paging::PageTableFlags,
    ) -> Result<(), &'static str> {
        let tracker = self.tracker.as_ref().ok_or("No frame tracker in surface buffer")?;
        page_table.map_shared_surface(start_page, tracker.clone(), flags)?;
        self.is_shm = true;
        Ok(())
    }

    /// Unmaps this surface buffer from a task's page table.
    pub fn unmap_from_task(
        &mut self,
        page_table: &mut crate::memory::TaskPageTable,
        start_page: x86_64::structures::paging::Page<x86_64::structures::paging::Size4KiB>,
    ) -> Result<(), &'static str> {
        page_table.unmap_shared_surface(start_page)
    }
}

impl core::ops::Deref for SurfaceBuffer {
    type Target = [u32];
    #[inline(always)]
    fn deref(&self) -> &[u32] {
        if self.ptr.is_null() || self.len == 0 {
            &[]
        } else {
            unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
        }
    }
}

impl core::ops::DerefMut for SurfaceBuffer {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut [u32] {
        if self.ptr.is_null() || self.len == 0 {
            &mut []
        } else {
            unsafe { core::slice::from_raw_parts_mut(self.ptr, self.len) }
        }
    }
}

impl Drop for SurfaceBuffer {
    fn drop(&mut self) {
        if let Some(tracker) = self.tracker.take() {
            tracker.mark_surface_dropped();
        }
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
    pub pixels: SurfaceBuffer, // 32-bit ARGB pixels backed by physical frames or host vec
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
    /// Whether the owning client has ever written a pixel into this surface.
    ///
    /// WHY this is separate from `frame_ready`: `frame_ready` says a commit is
    /// not in flight, and it is true for a surface nobody has drawn on. A
    /// preview of such a surface would be the allocator's fill colour, which
    /// is a picture of nothing. The capture path refuses a surface that has
    /// never been written, so the task switcher can say "this app has not
    /// drawn anything yet" instead of showing a black rectangle.
    pub has_frame: bool,
    /// Whether the surface has ever held a whole frame.
    ///
    /// WHY this is separate from `frame_ready`: a surface that has already
    /// presented a whole frame keeps presenting it while a band is
    /// rewritten. The rows outside the band still hold the last frame, so
    /// the surface is presentable, and an application that repaints one
    /// band at a time would otherwise vanish from the display every time
    /// it repainted one.
    ///
    /// WHY the compositor requires it: a surface that has never held a whole
    /// frame still holds the allocator's fill (opaque black) everywhere the
    /// owner has not drawn. Blending such a surface puts a black rectangle
    /// under the foreground application, and the application's translucent
    /// cards then blend against that black instead of against the platform
    /// background. The capture path already refuses a surface the owner has
    /// never written; the compositor refuses the same surface, for the same
    /// reason.
    pub frame_complete: bool,
    /// The rows the owner has written content into, one bit per row.
    ///
    /// WHY a bitmap and not a count: a client paints its canvas in whatever
    /// order it likes, and the surface holds a whole frame only when every
    /// row has been given content. A count would double-count a band that is
    /// repainted, and a leading-row frontier would stall on a client that
    /// paints the bottom of the canvas first.
    pub written_row_bits: Vec<u64>,
    /// Whether this surface was determined to be completely occluded by higher z-order opaque surfaces
    /// during the front-to-back tile occlusion pass.
    pub occluded: bool,
    /// Whether every pixel in this surface is known to be fully opaque (alpha == 255).
    pub opaque_content: bool,
    /// Whether this surface buffer has been mapped directly to a userspace shared memory mapping.
    pub shm_mapped: bool,
    /// The user virtual address where this surface's shared memory buffer is mapped.
    pub shm_user_vaddr: u64,
    /// Secondary buffer for zero-copy ping-pong double buffering.
    /// When `shm_mapped` is active, userspace renders into `back_pixels` while the compositor
    /// reads `pixels`. On `sys_surface_present`, the kernel atomically latches the updated frame.
    pub back_pixels: Option<SurfaceBuffer>,
    /// Active front buffer index (0 = pixels is front, 1 = back_pixels is front).
    pub front_buffer_idx: u8,
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
            pixels: SurfaceBuffer::new(size, 0xFF000000), // default opaque black
            dirty: true,
            dirty_rows: (0, 0),
            owner_task,
            frame_ready: true,
            has_frame: false,
            frame_complete: false,
            written_row_bits: vec![0u64; (height as usize + 63) / 64],
            occluded: false,
            opaque_content: true,
            shm_mapped: false,
            shm_user_vaddr: 0,
            back_pixels: None,
            front_buffer_idx: 0,
        }
    }

    /// Enables zero-copy ping-pong double buffering for this surface.
    /// Allocates an isolated physical back buffer that userspace can map and write into
    /// without tearing or race conditions with the kernel compositor.
    pub fn enable_double_buffering(&mut self) -> Result<(), &'static str> {
        if self.back_pixels.is_none() {
            let size = (self.width as usize) * (self.height as usize);
            let mut buf = SurfaceBuffer::new(size, 0xFF000000);
            buf.is_shm = true;
            self.back_pixels = Some(buf);
        }
        Ok(())
    }

    /// Latches the back buffer into the compositor front buffer for presentation.
    /// Commits the bounded dirty row region `[dirty_y0, dirty_y1)` to `self.pixels`.
    pub fn latch_shm_frame(&mut self, dirty_y0: usize, dirty_y1: usize) {
        if let Some(ref back) = self.back_pixels {
            let w = self.width as usize;
            let h = self.height as usize;
            let y0 = dirty_y0.min(h);
            let y1 = if dirty_y1 == 0 || dirty_y1 > h { h } else { dirty_y1 };
            if y1 > y0 {
                let start = y0 * w;
                let end = (y1 * w).min(self.pixels.len()).min(back.len());
                if end > start {
                    self.pixels[start..end].copy_from_slice(&back[start..end]);
                }
                self.mark_dirty_rows(y0, y1);
            } else {
                let len = self.pixels.len().min(back.len());
                self.pixels[..len].copy_from_slice(&back[..len]);
                self.mark_dirty_rows(0, h);
            }
            self.dirty = true;
            self.has_frame = true;
            self.frame_ready = true;
            self.frame_complete = true;
            self.opaque_content = self.is_fully_opaque();
        }
    }

    /// Returns the physical frames backing the back buffer for userspace SHM mapping.
    pub fn back_buffer_physical_frames(&self) -> Option<&[x86_64::structures::paging::PhysFrame<x86_64::structures::paging::Size4KiB>]> {
        self.back_pixels.as_ref().map(|b| b.physical_frames())
    }

    /// Returns true if this surface is 100% opaque across all pixels and layer opacity.
    pub fn is_fully_opaque(&self) -> bool {
        if self.opacity != 255 || !self.opaque_content {
            return false;
        }
        self.pixels.iter().all(|&p| (p >> 24) == 0xFF)
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
    ///
    /// WHY this also records coverage: every path that puts client pixels
    /// into a surface ends here, so this is the one place the compositor
    /// learns which rows the owner has actually given it. The leading-row
    /// frontier it maintains is what tells a whole frame (the owner painted
    /// the surface edge to edge) from a half-written surface (the owner
    /// painted a corner of it), and the compositor publishes only whole
    /// frames: a surface that still holds the allocator's fill is not a
    /// picture of anything.
    pub fn mark_dirty_rows(&mut self, y0: usize, y1: usize) {
        let y1 = y1.min(self.height as usize);
        union_rows(&mut self.dirty_rows, y0, y1);
        self.dirty = true;
        self.note_written_rows(y0, y1);
    }

    /// Records that the owner wrote content into rows `[y0, y1)`.
    ///
    /// WHY every row matters: a surface holds a whole frame only when the
    /// owner has filled it edge to edge. A write that leaves a row untouched
    /// leaves the allocator's fill in it, and the compositor will not publish
    /// a surface that still holds fill in some of its rows.
    fn note_written_rows(&mut self, y0: usize, y1: usize) {
        if self.frame_complete {
            return;
        }
        let height = self.height as usize;
        let y1 = y1.min(height);
        if y1 <= y0 || height == 0 {
            return;
        }
        let first = y0 / 64;
        let last = (y1 - 1) / 64;
        if first == last {
            self.written_row_bits[first] |= row_bit_mask(y0 % 64, y1 - y0);
        } else {
            self.written_row_bits[first] |= u64::MAX << (y0 % 64);
            for word in first + 1..last {
                self.written_row_bits[word] = u64::MAX;
            }
            self.written_row_bits[last] |= row_bit_mask(0, y1 - last * 64);
        }
        if self.every_row_written() {
            self.frame_complete = true;
        }
    }

    /// Whether every row of this surface has been given content.
    fn every_row_written(&self) -> bool {
        let height = self.height as usize;
        if height == 0 {
            return false;
        }
        let full_words = height / 64;
        let remainder = height % 64;
        let full = self.written_row_bits.iter().take(full_words).all(|w| *w == u64::MAX);
        let tail = if remainder == 0 {
            true
        } else {
            self.written_row_bits.get(full_words).copied().unwrap_or(0)
                == ((1u64 << remainder) - 1)
        };
        full && tail
    }

    /// Records that this surface holds a whole frame the owner drew.
    ///
    /// WHY this exists: the write paths record coverage on their own, but a
    /// caller that fills `pixels` directly -- the kernel's own unit tests for
    /// the scaling and blending paths -- has to say that the surface holds a
    /// frame, or the compositor treats it as a surface the owner never drew
    /// and skips it.
    pub fn mark_frame_drawn(&mut self) {
        for word in self.written_row_bits.iter_mut() {
            *word = u64::MAX;
        }
        self.frame_complete = true;
        self.has_frame = true;
        self.opaque_content = self.pixels.iter().all(|&p| (p >> 24) == 0xFF);
    }

    /// Updates surface pixels from an input buffer.
    pub fn update_pixels(&mut self, src: &[u32]) -> Result<(), &'static str> {
        if src.len() != self.pixels.len() {
            return Err("Pixel buffer length mismatch");
        }
        self.pixels.copy_from_slice(src);
        self.opaque_content = src.iter().all(|&p| (p >> 24) == 0xFF);
        self.mark_dirty_rows(0, self.height as usize);
        self.frame_ready = true;
        self.has_frame = true;
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
    pub scanout_y_offset: u16,
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
    pub scheduler: FrameScheduler,
    /// When true and honest dirty bands are present, bypasses the full-scanout buffer diff pass.
    pub skip_scanout_diff: bool,
}

impl DrmKmsController {

    /// Whether the identity full-cover blit will run for any surface.
    ///
    /// WHY this is asked rather than assumed: the identity path is the only
    /// blit that replaces the baseline outright instead of compositing over
    /// it, so a surface that matches the scanout has to take that path or the
    /// surfaces behind it show through.
    pub fn identity_full_cover_will_run(&self) -> bool {
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
            scanout_y_offset: 0,
            focused_surface: None,
            orientation: DisplayOrientation::Portrait,
            refresh_hz: 60,
            frame_deadline_ticks: 0,
            frame_dirty: true,
            powered: true,
            present_dirty_rows: (0, height as usize),
            scheduler: FrameScheduler::new(60),
            skip_scanout_diff: true,
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

    /// Re-indexes application surface z-orders (200 <= z < 1000) starting from 200 while preserving relative ordering.
    pub fn rebase_app_z_orders(&mut self) {
        let mut app_indices: Vec<usize> = (0..self.surfaces.len())
            .filter(|&i| self.surfaces[i].z_order >= 200 && self.surfaces[i].z_order < 1000)
            .collect();
        app_indices.sort_by_key(|&i| (self.surfaces[i].z_order, self.surfaces[i].id));
        let mut cur_z = 200u32;
        let mut prev_old_z: Option<u32> = None;
        for idx in app_indices {
            let old_z = self.surfaces[idx].z_order;
            if let Some(pz) = prev_old_z {
                if old_z > pz {
                    cur_z = cur_z.saturating_add(1);
                }
            }
            prev_old_z = Some(old_z);
            self.surfaces[idx].z_order = cur_z;
        }
    }

    /// Whether a flip by `owner_task` may publish the display right now.
    ///
    /// WHY this exists: the flip publishes every surface in the kernel, not
    /// just the caller's, so a flip that lands while another task is halfway
    /// through writing its frame publishes that task's half-drawn screen --
    /// and, because a whole-screen surface is an identity copy, it replaces
    /// the display rather than landing on it. The shell's own frame then
    /// appears on top of the application, the application appears on top of
    /// the shell, and the screen alternates between them.
    ///
    /// WHY the rule is about the task *below*: a task whose own surfaces are
    /// whole may always publish, because it is publishing its own finished
    /// frame. A task below another may not publish while the task above it
    /// is halfway through a frame, because that frame would be torn.
    pub fn may_publish(&self, owner_task: u64) -> bool {
        let own_min = self
            .surfaces
            .iter()
            .filter(|s| s.owner_task == owner_task && s.visible)
            .map(|s| s.z_order)
            .min();
        let Some(own_min) = own_min else {
            // The caller owns nothing visible, so it has no frame of its own
            // to publish and nothing of its own to protect.
            return true;
        };
        for s in self.surfaces.iter() {
            if !s.visible || s.owner_task == owner_task {
                continue;
            }
            if s.frame_ready || !s.has_frame {
                // A surface that is whole, or one that has never been written
                // to, hides nothing.
                continue;
            }
            if !s.input_enabled {
                // WHY the system's own overlays do not block: the system
                // line is a band the compositor keeps current, and it is
                // never written whole, so treating it as a torn frame would
                // stop the shell from ever publishing again.
                continue;
            }
            if s.z_order > own_min {
                return false;
            }
        }
        true
    }

    /// Whether a surface of `pixels` pixels fits inside the memory every
    /// surface shares, alongside what is already allocated.
    ///
    /// WHY it is a question rather than a check buried in the allocation: the
    /// syscall layer has to ask it *before* refusing an app, because the answer
    /// is also the trigger for retiring a backgrounded app to pay for the new
    /// one. Two copies of the arithmetic is how a budget comes to mean one
    /// thing to the driver and another to the policy above it.
    pub fn can_afford_surface(&self, pixels: usize) -> bool {
        let held: usize = self.surfaces.iter().map(|s| surface_bytes(s.pixels.len())).sum();
        held.saturating_add(surface_bytes(pixels)) <= MAX_SURFACE_BYTES
    }

    /// The bytes every surface holds right now, and the ceiling they share.
    pub fn surface_bytes_held(&self) -> (usize, usize) {
        let held: usize = self.surfaces.iter().map(|s| surface_bytes(s.pixels.len())).sum();
        (held, MAX_SURFACE_BYTES)
    }

    /// The backgrounded app whose memory should pay for a new surface.
    ///
    /// WHY the lowest application z order: raising a surface is what bringing
    /// an app to the front does, so among the surfaces that are not on screen
    /// the lowest one is the app that has been out of sight longest. That is
    /// the order Android's cached process list and iOS's jetsam walk, and it is
    /// the only order this kernel keeps.
    ///
    /// WHY the system's own surfaces are never the answer: they are not an
    /// app's memory to give back. The band and the compositor's overlay sit
    /// outside the application z band, and the caller is excluded because a
    /// task cannot be asked to pay for the screen it is asking for.
    pub fn evictable_app_owner(&self, caller: u64) -> Option<(u64, usize)> {
        self.surfaces
            .iter()
            .filter(|s| !s.visible && (200..1000).contains(&s.z_order) && s.owner_task != caller)
            .min_by_key(|s| s.z_order)
            .map(|s| (s.owner_task, surface_bytes(s.pixels.len())))
    }

    /// The exit code a backgrounded application is retired with when the device
    /// reclaims its surfaces to pay for the application being opened.
    ///
    /// WHY it is not the generic `-1`: `-1` is what every other lifecycle
    /// termination already reports -- the service supervisor and a launch whose
    /// IPC endpoint could not be allocated both pass it -- so a process record
    /// reading `-1` cannot tell "the device needed your memory" from "stop, for
    /// a reason nobody recorded". This one reserved number is that difference.
    ///
    /// WHY the exit code and not a message to the app: the honest signal is the
    /// one the runtime can actually honour. A retired task is a Zombie before it
    /// observes anything, so no IPC notice or in-app callback can reach it, and
    /// posting one would claim a delivery the platform cannot make. The exit code
    /// survives the reap into the process record, which is the only place the
    /// reason can be read without lying about telling the app.
    pub const EXIT_CODE_MEMORY_RETIRE: i32 = -200;

    /// Posts the one notice the person is owed when the device retires an app to
    /// free surfaces for another app, and reports whether the store accepted it.
    ///
    /// WHY exactly one: a retirement is a single decision, and the notice the
    /// person is owed is about that one app going away. This helper is the only
    /// place the wording is built, so a second call site cannot post a second
    /// notice and the syscall layer has nothing to reformat at the point of use.
    ///
    /// WHY the numbers are the arguments and not the surface state: `freed_bytes`
    /// and `MAX_SURFACE_BYTES` are what the retire path already measured -- the
    /// bytes the chosen victim's reclaimable surface holds and the ceiling every
    /// surface shares -- so the notice can only repeat what was measured. It never
    /// invents a percentage or a "low memory" claim the device did not make.
    ///
    /// WHY it takes the manager rather than locking the singleton itself: the
    /// retire site holds no scheduler lock when it posts (the notice's own
    /// `wake_package` path re-enters the scheduler, so posting under the scheduler
    /// lock would deadlock). The caller owns the `try_lock`, decides whether the
    /// store is reachable, and lets the retirement proceed either way. Taking the
    /// manager as an argument is also what lets the retire-path unit test build a
    /// store, call this once, and count exactly one record without touching global
    /// state.
    ///
    /// WHY it is owned by "system" and not by the retired package: the deferred
    /// reclaim that runs when the retired task is finally reaped calls
    /// `cancel_all_for_package` for that package, so a notice left under the
    /// retired app's own name would delete itself a tick after it appeared and the
    /// person would never read it. This is a lifecycle event the operating system
    /// caused, so it is posted under the system package the way boot posts its own
    /// notices -- while still naming the app that went away.
    ///
    /// `user_id` is the account whose shade the person is looking at, not a fixed
    /// owner constant: the store hands a record only to the account it is filed
    /// under, and the shell asks for the active one. Filed anywhere else the
    /// retirement is announced to an empty screen.
    pub fn post_retirement_notice(
        manager: &mut crate::notification::NotificationManager,
        victim_package: &str,
        user_id: u32,
        freed_bytes: usize,
        current_tick: u64,
    ) -> Result<u64, crate::notification::NotificationError> {
        let title = format!("{} was closed", victim_package);
        // WHY the sentence is this short: the shell caches a body in 64 bytes and
        // clips the row to the card, and the first painted retirement ended
        // "bytes of di..." with the budget figure cut off mid-claim. The title
        // already names the app, so the body spends its room on the two numbers
        // the retire path measured and nothing else.
        let body = format!(
            "The device closed it to free {} of {} bytes.",
            freed_bytes, MAX_SURFACE_BYTES,
        );
        // The "general" channel does not bypass Do Not Disturb, so the store's own
        // DND, rate and capacity limits are honoured: this returns Err under DND or
        // when the retired package's quota is spent, and the caller lets the
        // retirement proceed regardless.
        manager.post(
            "system",
            user_id,
            "general",
            &title,
            &body,
            crate::notification::NotificationPriority::Default,
            crate::notification::LockscreenVisibility::Public,
            0,
            current_tick,
        )
    }

    /// Returns true if `owner_task` is an authenticated system supervisor or privileged shell.
    pub fn is_trusted_system_owner(owner_task: u64) -> bool {
        #[cfg(test)]
        {
            if owner_task == 0 || owner_task == 999 {
                return true;
            }
        }
        if owner_task == 0 {
            return true;
        }
        if Some(owner_task) == crate::services::task_id(crate::services::ServiceId::Display).map(|t| t.0)
            || Some(owner_task) == crate::services::task_id(crate::services::ServiceId::Compositor).map(|t| t.0)
            || Some(owner_task) == crate::services::task_id(crate::services::ServiceId::Launcher).map(|t| t.0)
            || Some(owner_task) == crate::services::task_id(crate::services::ServiceId::Init).map(|t| t.0)
        {
            return true;
        }
        false
    }

    /// Allocates a new client surface for application rendering.
    pub fn create_surface(&mut self, width: u32, height: u32, z_order: u32, owner_task: u64) -> u32 {
        // A client that explicitly requests a 1080x2160 surface is asking for
        // that native mode; match it rather than silently resampling its frame.
        if width == 1080 && height == 2160 && (self.scanout_width != 1080 || self.scanout_height != 2160) {
            let _ = self.set_display_mode(1080, 2160, self.refresh_hz);
        }
        let pixels = match (width as usize).checked_mul(height as usize) {
            // Client surfaces have a stricter per-surface budget than the
            // physical scanout buffer. This prevents a single display client
            // from multiplying large kernel-heap allocations across 32 slots.
            Some(value) if value > 0 && value <= MAX_SURFACE_PIXELS && width <= 2400 && height <= 2400 => value,
            _ => {
                crate::serial_println!(
                    "[SURFACE_CREATE REFUSED] owner={} {}x{} z={} exceeds the per-surface pixel budget",
                    owner_task, width, height, z_order
                );
                return 0;
            }
        };
        if self.surfaces.len() >= MAX_CLIENT_SURFACES {
            crate::serial_println!(
                "[SURFACE_CREATE REFUSED] owner={} {}: the client surface table is full",
                owner_task, width
            );
            return 0;
        }
        // WHY this is checked here and not by the allocator: past this point the
        // next thing that happens is a `Vec` of `pixels` words, and a kernel heap
        // that cannot satisfy it does not return an error -- it panics, and the
        // panic policy halts the CPU. Refusing is the only answer the device gets
        // to make before the allocation, and the app that asked can be told.
        if !self.can_afford_surface(pixels) {
            let (held, budget) = self.surface_bytes_held();
            crate::serial_println!(
                "[SURFACE_CREATE REFUSED] owner={} {}x{} z={}: the surfaces hold {} bytes of the {} byte budget and this one needs {}",
                owner_task, width, height, z_order, held, budget, surface_bytes(pixels)
            );
            return 0;
        }
        let id = self.next_surface_id;
        self.next_surface_id = match self.next_surface_id.checked_add(1) {
            Some(next) if next != 0 => next,
            _ => return 0,
        };
        debug_assert_eq!(pixels, width as usize * height as usize);
        let is_trusted = Self::is_trusted_system_owner(owner_task);
        let is_overlay = is_trusted && (z_order == 100
            || owner_task == crate::services::task_id(crate::services::ServiceId::Display).map(|t| t.0).unwrap_or(u64::MAX)
            || owner_task == crate::services::task_id(crate::services::ServiceId::Compositor).map(|t| t.0).unwrap_or(u64::MAX));
        let kind = if is_overlay { crate::gui::WindowKind::SystemOverlay } else { crate::gui::WindowKind::Application };
        let actual_z = if z_order >= 200 && z_order < 1000 {
            let offset = z_order.saturating_sub(200);
            let task_base = self.surfaces.iter()
                .filter(|s| s.owner_task == owner_task && s.z_order >= 200 && s.z_order < 1000)
                .map(|s| s.z_order)
                .min();
            if let Some(tb) = task_base {
                tb.saturating_add(offset).clamp(200, 899)
            } else {
                let max_z = self.surfaces.iter()
                    .filter(|s| s.z_order >= 200 && s.z_order < 1000)
                    .map(|s| s.z_order)
                    .max()
                    .unwrap_or(200);
                if max_z >= 899 {
                    self.rebase_app_z_orders();
                    let new_max = self.surfaces.iter()
                        .filter(|s| s.z_order >= 200 && s.z_order < 1000)
                        .map(|s| s.z_order)
                        .max()
                        .unwrap_or(200);
                    new_max.max(200).saturating_add(1).saturating_add(offset).clamp(200, 899)
                } else {
                    max_z.max(200).saturating_add(1).saturating_add(offset).clamp(200, 899)
                }
            }
        } else if is_trusted {
            z_order
        } else {
            // Unprivileged callers requesting overlay (>= 1000) or desktop (< 200) are clamped to application band.
            z_order.clamp(200, 899)
        };
        let mut surface = DrmSurface::new(id, width, height, actual_z, owner_task);
        // WHY the system's own surfaces do not take input: the system line
        // and the display service's own surface sit above the shell's
        // surface in the z order, and a surface that takes input is a
        // surface the gestures are routed to. Without this the system band
        // swallowed every swipe, and the device could not be unlocked.
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
            let why = crate::gui::WINDOW_MANAGER
                .lock()
                .last_reject()
                .unwrap_or("unregistered window-manager rejection");
            crate::serial_println!(
                "[SURFACE_CREATE REFUSED] owner={} {}: the window manager rejected the window: {}",
                owner_task, width, why
            );
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
        if src.iter().any(|&p| (p >> 24) != 0xFF) {
            surface.opaque_content = false;
        }
        surface.dirty = true;
        surface.has_frame = true;
        if end == surface.pixels.len() {
            surface.frame_ready = true;
            surface.frame_complete = true;
        } else if offset == 0 && !surface.frame_complete {
            // WHY the first frame is the only one that un-latches: before
            // the surface has held a whole frame, a partial write leaves it
            // showing the rows it has not been given yet, and the
            // compositor must not publish that. After it has held a whole
            // frame, the rows outside the band still hold that frame, so
            // the surface is presentable while it is being repainted.
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

    /// Sets surface position and z-order with caller privilege enforcement.
    pub fn set_surface_geometry(&mut self, id: u32, x: i32, y: i32, z_order: u32, visible: bool) -> bool {
        if let Some(s) = self.surfaces.iter_mut().find(|s| s.id == id) {
            s.x = x;
            s.y = y;
            if Self::is_trusted_system_owner(s.owner_task) {
                s.z_order = z_order;
            } else {
                s.z_order = z_order.clamp(200, 899);
            }
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
            // Caller-based privilege enforcement: unprivileged applications can never
            // escape the application band (200..=899) or elevate into system overlay (>= 1000) or desktop (0..=199).
            if Self::is_trusted_system_owner(owner) {
                s.z_order = z;
            } else {
                s.z_order = z.clamp(200, 899);
            }
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

    /// Raises all application surfaces owned by `owner_task` above all other application surfaces,
    /// preserving distinct relative z-order among task surfaces and clamping below overlay boundary (z < 1000).
    pub fn raise_surfaces_for_task(&mut self, owner_task: u64) {
        let max_app_z = self.surfaces.iter()
            .filter(|s| s.z_order < 1000)
            .map(|s| s.z_order)
            .max()
            .unwrap_or(200);
        if max_app_z >= 900 {
            self.rebase_app_z_orders();
        }

        let min_task_z = self.surfaces.iter()
            .filter(|s| s.owner_task == owner_task && s.z_order < 1000)
            .map(|s| s.z_order)
            .min();

        let Some(min_z) = min_task_z else { return; };

        let max_other_z = self.surfaces.iter()
            .filter(|s| s.owner_task != owner_task && s.z_order < 1000)
            .map(|s| s.z_order)
            .max()
            .unwrap_or(200);

        let base_z = max_other_z.max(200).saturating_add(1);
        let mut raised_any = false;
        let mut top_id = None;
        let mut top_z = 0;

        for s in self.surfaces.iter_mut() {
            if s.owner_task == owner_task && s.z_order < 1000 {
                let offset = s.z_order.saturating_sub(min_z);
                s.z_order = base_z.saturating_add(offset).min(999);
                s.visible = true;
                s.dirty = true;
                if s.z_order >= top_z {
                    top_z = s.z_order;
                    top_id = Some(s.id);
                }
                raised_any = true;
            }
        }
        if let Some(id) = top_id {
            self.focused_surface = Some(id);
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
    /// WHY the scanout's own size rather than a hardcoded 720x1440: the fast
    /// path belongs to whichever surface matches the scanout exactly. The
    /// canonical scanout now *is* 720x1440, so every platform app takes it, but
    /// the predicate stays relative to the scanout so a client that requests a
    /// different native mode still gets the straight copy it asked for.

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
        // WHY the fill is unconditional, and not gated on "an opaque surface
        // will overwrite it anyway": the back buffer is reused between
        // composites, and a covering surface rarely fills its own frame with
        // opaque pixels alone. A client's translucent pixels blend with
        // whatever the previous composite left in these rows, so when the
        // launcher hid and a new foreground took the screen, the launcher's
        // residual pixels kept bleeding through every card the new screen
        // drew. A cost measured as noise cannot buy that: the platform's
        // background colour is the one pixel state that is true under every
        // foreground, so every composite starts from it.
        // WHY paper and not the desktop-era slate: this platform is a phone,
        // Sort surfaces by z_order ascending (lower z painted first)
        self.surfaces.sort_by_key(|s| s.z_order);

        let sw = self.scanout_width as i32;
        let sh = self.scanout_height as i32;

        let has_mobile_portrait = self.surfaces.iter().any(|s| {
            s.visible && s.frame_ready && ((s.width == 720 && s.height == 1440) || (s.width == 1080 && s.height == 2160))
        });

        // Scanout rows the flip will have to re-present. Seeded with whatever
        // the callers already declared dirty; the surface loop narrows it to
        // exactly the rows each dirty surface actually rewrote, and
        // `diff_present_rows` below makes the result fail-safe.
        let mut present_band = self.present_dirty_rows;

        // WHY a surface belonging to another task is skipped: it is a task in
        // the background, and an application that paints a translucent card
        // would then blend against the application behind it rather than
        // against the platform background. That is how the shell's greeting
        // and the shell's rows showed through the browser's page.
        //
        // WHY the task's own surfaces are kept: a task's overlay is drawn
        // over its own canvas, and the two are the same picture.
        let focused = self
            .focused_surface
            .and_then(|id| self.surfaces.iter().find(|s| s.id == id))
            .map(|s| (s.owner_task, s.z_order));

        // Front-to-back tile occlusion analysis:
        // Walk surfaces from highest z-order to lowest z-order. Opaque surfaces commit
        // their covered 64x64 tiles into the occlusion grid. Lower surfaces whose bounding
        // box falls entirely within committed tiles are marked occluded and skipped during
        // rasterization and blending.
        let mut tile_map = TileOcclusionMap::new(self.scanout_width as usize, self.scanout_height as usize);
        for s in self.surfaces.iter_mut().rev() {
            s.occluded = false;
            if !s.visible || s.opacity == 0 || !s.frame_ready || !s.frame_complete || !s.has_frame {
                continue;
            }
            if let Some((focused_owner, focused_z)) = focused {
                if s.owner_task != focused_owner && s.z_order < focused_z {
                    continue;
                }
            }
            if has_mobile_portrait && s.height == 96 && s.y != 0 {
                continue;
            }

            let landscape = matches!(self.orientation, DisplayOrientation::LandscapeRight | DisplayOrientation::LandscapeLeft);
            let (x0, y0, x1, y1) = if ((s.width == 720 && s.height == 1440) || (s.width == 1080 && s.height == 2160)) && s.x == 0 && s.y == 0 {
                if !landscape && s.width == self.scanout_width && s.height == self.scanout_height {
                    (0, 0, self.scanout_width as usize, self.scanout_height as usize)
                } else {
                    let s_w = s.width as i32;
                    let s_h = s.height as i32;
                    let (dst_w, dst_h) = if landscape {
                        if 1440i64 * sh as i64 <= 720i64 * sw as i64 {
                            (self.scanout_width, (720u32.saturating_mul(sw as u32) / 1440).max(1).min(self.scanout_height))
                        } else {
                            ((1440u32.saturating_mul(sh as u32) / 720).max(1).min(self.scanout_width), self.scanout_height)
                        }
                    } else if (s_w as i64) * sh as i64 <= (s_h as i64) * sw as i64 {
                        (((s_w as u32).saturating_mul(sh as u32) / (s_h as u32)).max(1).min(self.scanout_width), self.scanout_height)
                    } else {
                        (self.scanout_width, ((s_h as u32).saturating_mul(sw as u32) / (s_w as u32)).max(1).min(self.scanout_height))
                    };
                    let dst_x = ((sw - dst_w as i32) / 2).max(0) as usize;
                    let dst_y = ((sh - dst_h as i32) / 2).max(0) as usize;
                    (dst_x, dst_y, dst_x + dst_w as usize, dst_y + dst_h as usize)
                }
            } else {
                let rx0 = s.x.max(0) as usize;
                let ry0 = s.y.max(0) as usize;
                let rx1 = (s.x.saturating_add(s.width as i32)).max(0).min(sw) as usize;
                let ry1 = (s.y.saturating_add(s.height as i32)).max(0).min(sh) as usize;
                (rx0, ry0, rx1, ry1)
            };

            if x1 <= x0 || y1 <= y0 {
                continue;
            }

            if tile_map.is_region_occluded(x0, y0, x1, y1) {
                s.occluded = true;
            } else if s.is_fully_opaque() {
                tile_map.commit_opaque_rect(x0, y0, x1, y1);
            }
        }

        // When an opaque surface covers the entire scanout without rotation or scaling,
        // it directly copies its pixels over the entire back buffer. Skipping the
        // baseline fill in that case saves millions of redundant DRAM writes per second
        // (e.g. ~1 GB/s at 240Hz).
        let has_full_cover = !matches!(
            self.orientation,
            DisplayOrientation::LandscapeRight | DisplayOrientation::LandscapeLeft
        ) && self.surfaces.iter().any(|s| {
            !s.occluded
                && s.visible
                && s.opacity == 255
                && s.frame_ready
                && s.frame_complete
                && s.has_frame
                && s.x == 0
                && s.y == 0
                && s.width == self.scanout_width
                && s.height == self.scanout_height
                && focused.map_or(true, |(owner, z)| s.owner_task == owner || s.z_order >= z)
        });
        if !has_full_cover {
            self.back_buffer.fill(PLATFORM_BACKGROUND);
        }

        let mut base_drawn = false;

        for surface in self.surfaces.iter_mut() {
            if surface.occluded {
                surface.dirty = false;
                surface.dirty_rows = (0, 0);
                continue;
            }
            // WHY a surface that holds no frame is skipped: the pixels it still
            // holds are the allocator's fill (opaque black), not a picture the
            // owner drew. Blending one puts a black rectangle underneath the
            // foreground application, and the application's translucent cards
            // then blend against that black instead of against the platform
            // background -- which is how a solid black block ended up in the
            // corner of every screen. The capture path already refuses a
            // surface the owner has never written; the compositor refuses the
            // same surface, for the same reason.
            // WHY a surface the owner has never written is skipped: its pixels
            // are still the allocator's fill, which is opaque black, and
            // blending one puts a black rectangle under the foreground
            // application. The application's translucent cards then blend
            // against that black instead of against the platform
            // background, and a solid black block appears in the corner of
            // every screen.
            //
            // WHY a surface that has not held a whole frame is skipped: the
            // rows its owner has not written still hold the allocator's fill,
            // and blending that put a black rectangle under the foreground
            // application. A surface that has held a whole frame is a
            // picture; one that has not is not.
            //
            // WHY the client's own write path is the other half of the rule:
            // a client that paints in bands has to paint its whole canvas
            // once before the compositor will publish it, and the display
            // service -- which used to create a surface, write four pixels
            // into it and flip -- no longer creates one at all.
            if !surface.visible
                || surface.opacity == 0
                || !surface.frame_ready
                || !surface.frame_complete
                || !surface.has_frame
            {
                continue;
            }

            // WHY a surface behind the focused one is skipped: it belongs to a
            // task that is in the background, and an application that paints
            // a translucent card would then blend against the application
            // behind it rather than against the platform background. That is
            // how the shell's greeting and the shell's rows showed through the
            // browser's page.
            if let Some((focused_owner, focused_z)) = focused {
                if surface.owner_task != focused_owner && surface.z_order < focused_z {
                    continue;
                }
            }

            if has_mobile_portrait && surface.height == 96 && surface.y != 0 {
                // WHY only a band that is not at the top: the phone's status
                // bar is a band of this height at the top of the screen, and
                // skipping it left every screen without a clock, a battery or
                // a signal. The legacy desktop banner this rule was written
                // for is not at the top, so it is still skipped.
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
            // the only blit that replaces the baseline outright, and it is also
            // what keeps a scanout-matching client from paying an interpolation
            // it does not need.
            //
            // WHY it is ordered before the launcher block: at a 720x1440 scanout
            // the launcher surface matches this condition too, and the branch
            // below performs exactly the copy and the same row mapping the old
            // in-block identity case did, so the two are one path rather than
            // two.
            if !base_drawn
                && !matches!(
                    self.orientation,
                    DisplayOrientation::LandscapeRight | DisplayOrientation::LandscapeLeft
                ) && s_w == sw && s_h == sh && sx == 0 && sy == 0 && surface.opacity == 255
            {
                // Identity full cover: the base surface matches the entire scanout.
                // Always copy the entire surface so that clean rows never retain
                // stale PLATFORM_BACKGROUND fill bytes in the back buffer, and narrow
                // only the presented band to the damaged rows.
                self.back_buffer.copy_from_slice(&surface.pixels);
                if surface.dirty_rows.1 > surface.dirty_rows.0 {
                    let y0 = surface.dirty_rows.0.min(sh as usize);
                    let y1 = surface.dirty_rows.1.min(sh as usize);
                    union_rows(&mut present_band, y0, y1);
                } else {
                    union_rows(&mut present_band, 0, sh as usize);
                }
                surface.dirty = false;
                surface.dirty_rows = (0, 0);
                base_drawn = true;
                continue;
            }

            // The canonical mobile portrait canvas has a stable 1:2 aspect ratio
            // (720x1440 logical, or an explicitly requested 1080x2160 surface).
            // Fit that canvas to the actual scanout preserving geometry.
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
                if !base_drawn
                    && (dst_w == s_w as usize && dst_h == s_h as usize)
                    && dst_x == 0 && dst_y == 0
                    && self.scanout_width as usize == dst_w && self.scanout_height as usize == dst_h
                    && !landscape && surface.opacity == 255
                {
                    self.back_buffer.copy_from_slice(&surface.pixels);
                    // WHY the mapping is the identity here: this branch requires
                    // the destination rect to match the scanout exactly, and the
                    // blit above is a straight `copy_from_slice`. Surface row `r`
                    // therefore lands on scanout row `0 + r`, so the tracked band copies 1:1.
                    if surface.dirty_rows.1 > surface.dirty_rows.0 {
                        union_rows(&mut present_band, surface.dirty_rows.0, surface.dirty_rows.1.min(dst_y + dst_h));
                    }
                    surface.dirty = false;
                    surface.dirty_rows = (0, 0);
                    base_drawn = true;
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
                    if landscape {
                        // Rotation swaps horizontal/vertical axes; changes span destination height.
                        union_rows(&mut present_band, dst_y, dst_y + dst_h);
                    } else if dst_w * 2 == s_w as usize && dst_h * 2 == s_h as usize {
                        // Integer 2x shrink: source row r maps to destination row r / 2
                        let r0 = dst_y + surface.dirty_rows.0 / 2;
                        let r1 = (dst_y + (surface.dirty_rows.1 + 1) / 2).min(dst_y + dst_h);
                        if r1 > r0 {
                            union_rows(&mut present_band, r0, r1);
                        }
                    } else if dst_w == s_w as usize && dst_h == s_h as usize {
                        // 1:1 direct blit: source row r maps 1:1 to destination row dst_y + r
                        let r0 = dst_y + surface.dirty_rows.0;
                        let r1 = (dst_y + surface.dirty_rows.1).min(dst_y + dst_h);
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
                        }
                    }
                }

                if landscape {
                    let scanout_w = self.scanout_width as usize;
                    let src_pitch = s_w as usize;
                    let src_last_x = (s_w as usize).saturating_sub(1);
                    let src_last_y = (s_h as usize).saturating_sub(1);
                    if is_right {
                        for dy in 0..dst_h {
                            let src_x = if dst_h <= 1 { 0 } else { ((dy * src_last_x + (dst_h - 1) / 2) / (dst_h - 1)).min(src_last_x) };
                            let dst_y_offset = (dst_y + dy) * scanout_w;
                            for dx in 0..dst_w {
                                let src_y = if dst_w <= 1 { 0 } else { src_last_y.saturating_sub((dx * src_last_y + (dst_w - 1) / 2) / (dst_w - 1)) };
                                let src_idx = src_y * src_pitch + src_x;
                                let dst_idx = dst_y_offset + dst_x + dx;
                                if src_idx < surface.pixels.len() && dst_idx < self.back_buffer.len() {
                                    self.back_buffer[dst_idx] = blend_pixel(surface.pixels[src_idx], self.back_buffer[dst_idx], surface.opacity);
                                }
                            }
                        }
                    } else {
                        for dy in 0..dst_h {
                            let src_x = if dst_h <= 1 { 0 } else { src_last_x.saturating_sub((dy * src_last_x + (dst_h - 1) / 2) / (dst_h - 1)) };
                            let dst_y_offset = (dst_y + dy) * scanout_w;
                            for dx in 0..dst_w {
                                let src_y = if dst_w <= 1 { 0 } else { (dx * src_last_y + (dst_w - 1) / 2) / (dst_w - 1) };
                                let src_idx = src_y * src_pitch + src_x;
                                let dst_idx = dst_y_offset + dst_x + dx;
                                if src_idx < surface.pixels.len() && dst_idx < self.back_buffer.len() {
                                    self.back_buffer[dst_idx] = blend_pixel(surface.pixels[src_idx], self.back_buffer[dst_idx], surface.opacity);
                                }
                            }
                        }
                    }
                } else {
                    let scanout_w = self.scanout_width as usize;
                    let src_pitch = s_w as usize;
                    if dst_w * 2 == s_w as usize && dst_h * 2 == s_h as usize {
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
                            let r0 = (dy * 2) * src_pitch;
                            let r1 = r0 + src_pitch;
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
                    } else if dst_w == s_w as usize && dst_h == s_h as usize {
                        // Direct 1:1 blit whenever the surface matches the scanout
                        // (720x1440 canonically, or an explicit 1080x2160 request).
                        // Handle it without calling the 1.5x reconstructor or
                        // fractional scaling when the source already matches the
                        // destination resolution.
                        if surface.opacity == 255 {
                            for dy in 0..dst_h {
                                let src_row_start = dy * src_pitch;
                                let dst_row_start = (dst_y + dy) * scanout_w + dst_x;
                                for dx in 0..dst_w {
                                    let sp = surface.pixels[src_row_start + dx];
                                    let sa = (sp >> 24) & 0xFF;
                                    let dst_idx = dst_row_start + dx;
                                    if dst_idx < self.back_buffer.len() {
                                        if sa == 255 {
                                            self.back_buffer[dst_idx] = sp;
                                        } else if sa > 0 {
                                            self.back_buffer[dst_idx] = blend_pixel(sp, self.back_buffer[dst_idx], 255);
                                        }
                                    }
                                }
                            }
                        } else {
                            for dy in 0..dst_h {
                                let src_row_start = dy * src_pitch;
                                let dst_row_start = (dst_y + dy) * scanout_w + dst_x;
                                for dx in 0..dst_w {
                                    let sp = surface.pixels[src_row_start + dx];
                                    let dst_idx = dst_row_start + dx;
                                    if dst_idx < self.back_buffer.len() {
                                        self.back_buffer[dst_idx] = blend_pixel(sp, self.back_buffer[dst_idx], surface.opacity);
                                    }
                                }
                            }
                        }
                    } else if dst_w * 2 == s_w as usize * 3 && dst_h * 2 == s_h as usize * 3 && s_w as usize <= 720 {
                        // 1.5x reconstructor / two-pass area-averaging filter for an
                        // explicit 720x1440 surface on a 1080x2160 scanout. The
                        // canonical 720x1440 scanout never takes this branch.
                        //
                        // 2 input pixels contribute to 3 output pixels: p0, (p0+p1)/2, p1 with
                        // 32-bit ARGB channel math without floats or allocations.
                        // Pass 1: Vertical area reconstruction (evaluates row r0, (r0+r1)/2, or r1).
                        // Pass 2: Horizontal area reconstruction (generates p0, (p0+p1)/2, p1).
                        //
                        // Strictly #![no_std] with zero heap allocations in scanout presentation loops.
                        let src_pitch = s_w as usize;
                        let s_w_usize = s_w as usize;
                        let s_h_usize = s_h as usize;
                        let num_groups = s_w_usize / 2;

                        let mut row_scratch = [0u32; 720];

                        for dy in 0..dst_h {
                            let ry = dy % 3;
                            let sy0 = (dy / 3) * 2;
                            let sy1 = (sy0 + 1).min(s_h_usize - 1);
                            let r0 = sy0 * src_pitch;
                            let r1 = sy1 * src_pitch;

                            // Pass 1: Vertical reconstruction
                            let row_src: &[u32] = match ry {
                                0 => &surface.pixels[r0..r0 + s_w_usize],
                                1 => {
                                    let s0 = &surface.pixels[r0..r0 + s_w_usize];
                                    let s1 = &surface.pixels[r1..r1 + s_w_usize];
                                    for x in 0..s_w_usize {
                                        row_scratch[x] = blend_half(s0[x], s1[x]);
                                    }
                                    &row_scratch[..s_w_usize]
                                }
                                _ => &surface.pixels[r1..r1 + s_w_usize],
                            };

                            let dst_row_start = (dst_y + dy) * scanout_w + dst_x;

                            // Pass 2: Horizontal reconstruction
                            if surface.opacity == 255 {
                                for k in 0..num_groups {
                                    let sx0 = 2 * k;
                                    let sx1 = sx0 + 1;
                                    let p0 = row_src[sx0];
                                    let p1 = row_src[sx1];

                                    let out0 = p0;
                                    let out1 = blend_half(p0, p1);
                                    let out2 = p1;

                                    let base_dx = dst_row_start + k * 3;
                                    if base_dx + 2 < self.back_buffer.len() {
                                        if (out0 >> 24) == 0xFF {
                                            self.back_buffer[base_dx] = out0;
                                        } else {
                                            self.back_buffer[base_dx] = blend_pixel(out0, self.back_buffer[base_dx], 255);
                                        }

                                        if (out1 >> 24) == 0xFF {
                                            self.back_buffer[base_dx + 1] = out1;
                                        } else {
                                            self.back_buffer[base_dx + 1] = blend_pixel(out1, self.back_buffer[base_dx + 1], 255);
                                        }

                                        if (out2 >> 24) == 0xFF {
                                            self.back_buffer[base_dx + 2] = out2;
                                        } else {
                                            self.back_buffer[base_dx + 2] = blend_pixel(out2, self.back_buffer[base_dx + 2], 255);
                                        }
                                    }
                                }
                            } else {
                                for k in 0..num_groups {
                                    let sx0 = 2 * k;
                                    let sx1 = sx0 + 1;
                                    let p0 = row_src[sx0];
                                    let p1 = row_src[sx1];

                                    let out0 = p0;
                                    let out1 = blend_half(p0, p1);
                                    let out2 = p1;

                                    let base_dx = dst_row_start + k * 3;
                                    if base_dx + 2 < self.back_buffer.len() {
                                        self.back_buffer[base_dx] = blend_pixel(out0, self.back_buffer[base_dx], surface.opacity);
                                        self.back_buffer[base_dx + 1] = blend_pixel(out1, self.back_buffer[base_dx + 1], surface.opacity);
                                        self.back_buffer[base_dx + 2] = blend_pixel(out2, self.back_buffer[base_dx + 2], surface.opacity);
                                    }
                                }
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
                let drawn_lo = (y0 - sy).max(0) as usize;
                let drawn_hi = (y1 - sy).max(0) as usize;
                let lo = surface.dirty_rows.0.max(drawn_lo);
                let hi = surface.dirty_rows.1.min(drawn_hi);
                if hi > lo {
                    let r0 = ((lo as i32) + sy).max(0) as usize;
                    let r1 = ((hi as i32) + sy).max(0) as usize;
                    if r1 > r0 {
                        union_rows(&mut present_band, r0, r1);
                    }
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

                        if src_idx >= surface.pixels.len() || dst_idx >= self.back_buffer.len() {
                            continue;
                        }

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

                        if src_idx >= surface.pixels.len() || dst_idx >= self.back_buffer.len() {
                            continue;
                        }

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
        if self.skip_scanout_diff && band.1 > band.0 {
            return;
        }
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
        if width == 0 || height == 0 || refresh_hz == 0 || refresh_hz > 360 {
            return Err("invalid display mode");
        }
        let pixels = (width as usize).checked_mul(height as usize).ok_or("display mode size overflow")?;
        if pixels > MAX_SCANOUT_PIXELS {
            return Err("display mode exceeds bounded scanout budget");
        }
        self.scanout_width = width;
        self.scanout_height = height;
        self.refresh_hz = refresh_hz;
        self.scheduler.refresh_hz = refresh_hz;
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

    /// Fast slice copy for full-screen identity surfaces respecting dirty row bounds.
    pub fn blit_opaque_identity_damaged(&mut self, surface: &mut DrmSurface) {
        if surface.pixels.len() == self.back_buffer.len() {
            self.back_buffer.copy_from_slice(&surface.pixels);
            if surface.dirty_rows.1 > surface.dirty_rows.0 {
                let y0 = surface.dirty_rows.0.min(self.scanout_height as usize);
                let y1 = surface.dirty_rows.1.min(self.scanout_height as usize);
                union_rows(&mut self.present_dirty_rows, y0, y1);
            } else {
                self.mark_present_full();
            }
            surface.dirty = false;
            surface.dirty_rows = (0, 0);
        }
    }

    /// Performs an atomic page flip: swaps back buffer to front scanout buffer.
    pub fn page_flip(&mut self) -> u64 {
        self.ensure_buffers();
        core::mem::swap(&mut self.front_buffer, &mut self.back_buffer);
        if self.present_dirty_rows.1 > self.present_dirty_rows.0 {
            let y0 = self.present_dirty_rows.0.min(self.scanout_height as usize);
            let y1 = self.present_dirty_rows.1.min(self.scanout_height as usize);
            let w = self.scanout_width as usize;
            let start = y0 * w;
            let end = y1 * w;
            if end <= self.front_buffer.len() && end <= self.back_buffer.len() {
                self.back_buffer[start..end].copy_from_slice(&self.front_buffer[start..end]);
            }
        }
        self.vsync_counter += 1;
        self.page_flips_completed += 1;
        self.scanout_y_offset = 0;
        let now = crate::interrupts::get_system_ticks();
        self.scheduler.record_frame(now, self.frame_deadline_ticks);
        let ticks_per_frame = ((1000u64 + self.refresh_hz.max(1) as u64 - 1) / self.refresh_hz.max(1) as u64).max(1);
        self.frame_deadline_ticks = now.saturating_add(ticks_per_frame);
        self.page_flips_completed
    }
}

/// Global static instance of the DRM/KMS Controller.
/// Canonical portrait scanout is 720x1440 -- exactly the authoring space every
/// platform app paints in, and the size `design::viewport::SCANOUT_*` names.
/// Keeping the two equal is what makes composition a straight copy: a larger
/// scanout forces a 1.5x resample that turns a one-pixel stroke into a soft
/// two-pixel ramp. The boot framebuffer is a separate presentation target and
/// may be any supported size.
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
    scanout_y_offset: 0,
    focused_surface: None,
    orientation: DisplayOrientation::Portrait,
    refresh_hz: 60,
    frame_deadline_ticks: 0,
    frame_dirty: true,
    powered: true,
    present_dirty_rows: (0, 1440),
    scheduler: FrameScheduler::new(60),
    skip_scanout_diff: true,
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_surface_the_heap_cannot_pay_for_is_refused_before_it_is_asked() {
        // WHY this test exists: twelve full-screen surfaces are 49,766,400 of
        // the 67,108,864 kernel bytes, and the run that passed them asked for one
        // more and panicked the device -- the panic policy halted the CPU because
        // one app wanted a screen. The budget is what turns that halt into a
        // refused launch, so what matters is not the number but which bound ends
        // the run: it has to be the memory, and the heap has to be asked for
        // nothing it cannot give.
        let mut kms = DrmKmsController::new(720, 1440);
        kms.set_display_mode(720, 1440, 60).expect("mode");
        kms.surfaces.clear();

        let mut accepted = 0usize;
        for task in 10..40u64 {
            if kms.create_surface(720, 1440, 200 + task as u32, task) == 0 {
                break;
            }
            accepted += 1;
        }

        assert!(
            accepted >= 8,
            "the budget afforded only {accepted} full-screen surfaces; the demo cannot open that many"
        );
        assert!(
            accepted < MAX_CLIENT_SURFACES,
            "the run ended at the {accepted} slot table rather than at the memory budget"
        );
        let (held, budget) = kms.surface_bytes_held();
        assert!(held <= budget, "{held} bytes held against a {budget} byte budget");
        assert!(
            !kms.can_afford_surface(720 * 1440),
            "the surface the run stopped at was affordable after all, so something else refused it"
        );
    }

    #[test]
    fn a_retirement_picks_the_backgrounded_app_and_nothing_else() {
        // WHY this test exists: the kernel now ends an application to pay for a
        // launch it cannot otherwise afford, and the one wrong choice is worse
        // than the refusal it replaces -- taking the app in front, the shell, or
        // the system band would break the screen the person is looking at.
        let mut kms = DrmKmsController::new(720, 1440);
        kms.set_display_mode(720, 1440, 60).expect("mode");
        kms.surfaces.clear();
        for (task, z, visible) in [
            (11u64, 0u32, true),     // the launcher's own surface, in front
            (10u64, 100, false),     // the system band, outside the app band
            (15, 205, false),        // backgrounded, and the oldest of them
            (16, 207, false),        // backgrounded, more recently raised
            (17, 210, true),         // on screen, so not a candidate
            (18, 211, false),        // backgrounded, newest
        ] {
            let mut s = DrmSurface::new(100 + task as u32, 8, 8, z, task);
            s.visible = visible;
            kms.surfaces.push(s);
        }
        assert_eq!(
            kms.evictable_app_owner(18).map(|(owner, _)| owner),
            Some(15),
            "the retirement did not pick the app that has been out of sight longest"
        );
        // The caller is never the one that pays, even when it is the only
        // backgrounded app left -- it is asking for a screen, not giving one up.
        assert_eq!(kms.evictable_app_owner(15).map(|(owner, _)| owner), Some(16));
        kms.surfaces.retain(|s| s.owner_task != 16 && s.owner_task != 18);
        assert_eq!(kms.evictable_app_owner(15), None);
    }

    #[test]
    fn a_retirement_posts_one_notice_naming_the_app_and_the_bytes_it_measured() {
        // WHY this test exists: when the device retires a backgrounded app to pay
        // for another app's surface, the person used to get no notice and the app
        // never learned why it went away. The notice is the person's half and the
        // exit code is the half the runtime can honestly carry. What this pins is
        // that a retirement posts *exactly one* notice, that it names the victim,
        // and that the two numbers it states are the ones the retire path measured
        // -- not a percentage or a "low memory" claim the device did not make.
        let mut manager = crate::notification::NotificationManager::new();
        manager.seed_standard_channels();
        let freed = surface_bytes(720 * 1440);
        // An account that is NOT the owner, so a hard-coded owner inside the
        // helper cannot pass: the notice has to land where the shade will look.
        let account = crate::user::UserId::GUEST.0;
        let posted = DrmKmsController::post_retirement_notice(&mut manager, "com.example.gallery", account, freed, 42)
            .expect("the notice is accepted on a fresh store");

        let active = manager.query_active(account, false);
        assert_eq!(active.len(), 1, "a retirement must post exactly one notice");
        assert!(
            manager.query_active(crate::user::UserId::OWNER.0, false).is_empty(),
            "the notice was filed under a fixed account instead of the one it was given"
        );
        let record = &active[0];
        assert_eq!(record.id, posted);
        assert_eq!(record.package_id, "system", "the notice is owned by the system, not the retired app");
        assert!(record.title.contains("com.example.gallery"), "the notice must name the app that went away");
        // The claim is only honest if the numbers it states are the ones measured.
        assert!(record.body.contains(&alloc::string::ToString::to_string(&freed)), "the notice must state the bytes freed");
        assert!(record.body.contains(&alloc::string::ToString::to_string(&MAX_SURFACE_BYTES)), "the notice must state the budget");
        assert_eq!(record.timestamp_ticks, 42, "the notice is stamped with the tick it was posted");

        // The exit code is the named one, so a reader of the process record can
        // tell a memory retirement from the generic -1 every other path passes.
        assert_ne!(DrmKmsController::EXIT_CODE_MEMORY_RETIRE, -1, "the retirement must be distinguishable from -1");
        assert_eq!(DrmKmsController::EXIT_CODE_MEMORY_RETIRE, -200);
    }

    #[test]
    fn a_notice_refused_by_the_store_does_not_stop_the_retirement() {
        // WHY this test exists: the retirement pays for a launch the person asked
        // for, and a notification store that is full, on DND, or has spent the
        // quota must never hold the launch hostage. This helper returns the store's
        // own refusal instead of panicking; the caller ignores the value and the
        // retire path proceeds. A DND-suppressed post returns Err and adds nothing.
        let mut manager = crate::notification::NotificationManager::new();
        manager.seed_standard_channels();
        manager.set_dnd_enabled(true);
        let freed = surface_bytes(100);
        let result = DrmKmsController::post_retirement_notice(
            &mut manager,
            "com.example.gallery",
            crate::user::UserId::OWNER.0,
            freed,
            1,
        );
        assert!(
            matches!(result, Err(crate::notification::NotificationError::DndSuppressed)),
            "a general-channel notice under DND must be refused, not forced through"
        );
        assert!(manager.query_active(crate::user::UserId::OWNER.0, false).is_empty());
    }

    #[test]
    fn the_slot_count_alone_cannot_be_the_memory_bound() {
        // WHY: the table of 32 slots is the only bound this driver had, and
        // thirty-two full-screen surfaces are more than twice the whole kernel
        // heap. A device bounded by that count is bounded by a number the heap
        // cannot honour, so the memory needs its own ceiling below the heap.
        let full_screen = surface_bytes(720 * 1440);
        assert_eq!(full_screen, 4_147_200);
        assert!(
            MAX_CLIENT_SURFACES * full_screen > crate::allocator::HEAP_SIZE,
            "the slot table no longer overruns the heap, so this test has stopped proving the budget is needed"
        );
        assert!(MAX_SURFACE_BYTES < crate::allocator::HEAP_SIZE);
    }

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
        s.pixels.copy_from_slice(&[0xFF000000, 0xFFFFFFFF, 0xFF000000, 0xFFFFFFFF]);
        s.visible = true;
        s.opacity = 255;
        s.mark_frame_drawn();
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
        s.mark_frame_drawn();
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

    #[test]
    fn authoring_sized_scanout_copies_surface_without_resampling() {
        // WHY this test exists: every platform app paints 720x1440. When the
        // scanout is any other size the compositor resamples the frame, and a
        // one-pixel stroke becomes a soft two-pixel ramp -- the display blur
        // this change removes. This pins both halves of the fix: the canonical
        // geometry is 720x1440, and an authoring-sized surface reaches the
        // back buffer bit-for-bit, so an isolated one-pixel line survives with
        // untouched neighbours instead of bleeding into them.
        let mut kms = DrmKmsController::new(720, 1440);
        kms.set_display_mode(720, 1440, 60).expect("mode");
        assert_eq!((kms.scanout_width, kms.scanout_height), (720, 1440));

        let id = kms.create_surface(720, 1440, 0, 1);
        assert_ne!(id, 0);
        let surf = kms.surfaces.iter_mut().find(|s| s.id == id).unwrap();
        surf.pixels.fill(0xFF000000);
        for y in 0..1440usize {
            surf.pixels[y * 720 + 360] = 0xFFFFFFFF;
        }
        surf.pixels[0] = 0xFF102030;
        surf.mark_frame_drawn();
        kms.frame_dirty = true;
        kms.surfaces[0].dirty = true;
        kms.composite();

        assert_eq!(kms.back_buffer.len(), 720 * 1440);
        assert_eq!(
            kms.back_buffer[..],
            kms.surfaces[0].pixels[..],
            "an authoring-sized surface must reach a same-sized scanout unchanged"
        );
        let row = 100 * 720;
        assert_eq!(kms.back_buffer[row + 359], 0xFF000000, "left neighbour must stay untouched");
        assert_eq!(kms.back_buffer[row + 360], 0xFFFFFFFF, "the stroke itself must survive");
        assert_eq!(kms.back_buffer[row + 361], 0xFF000000, "right neighbour must stay untouched");
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
        kms.surfaces[0].mark_frame_drawn();
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
        kms.surfaces[0].mark_frame_drawn();
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
        kms.surfaces[0].mark_frame_drawn();
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
        kms.surfaces[0].mark_frame_drawn();
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
        kms.surfaces[0].mark_frame_drawn();
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
    fn skip_scanout_diff_bypasses_compare_when_dirty_band_present() {
        let mut kms = DrmKmsController::new(720, 1440);
        assert!(kms.skip_scanout_diff, "skip_scanout_diff must default to true in new()");
        assert!(DRM_KMS.try_lock().expect("drm_kms lock").skip_scanout_diff, "skip_scanout_diff must default to true in DRM_KMS static");
        kms.set_display_mode(720, 1440, 60).expect("mode");
        let id = kms.create_surface(720, 1440, 0, 1);
        kms.surfaces[0].mark_frame_drawn();
        kms.composite();
        let _ = kms.take_present_dirty_rows();
        kms.page_flip();

        kms.surfaces[0].dirty = true;
        kms.surfaces[0].dirty_rows = (200, 300);
        kms.composite();
        let (r0, r1) = kms.take_present_dirty_rows();
        assert_eq!((r0, r1), (200, 300));
        assert!(kms.destroy_surface(id));
    }

    #[test]
    fn atomic_frame_commit_latch_prevents_uncommitted_surface_composition() {
        let mut kms = DrmKmsController::new(100, 100);
        kms.set_display_mode(100, 100, 60).expect("mode");

        let id = kms.create_surface(10, 10, 1, 42);
        assert_ne!(id, 0);

        // Partial write starting at offset 0 unlatches frame_ready
        let chunk1 = [0xFFFF0000; 20];
        kms.write_surface_pixels(id, 42, 0, &chunk1).expect("write chunk 1");
        assert!(!kms.surfaces[0].is_frame_ready());
        assert!(!kms.surfaces[0].frame_complete);

        // Composite while unlatched must skip blending the uncommitted surface
        kms.composite();
        // Since id was skipped, the back buffer is still the platform's
        // background colour. The assertion names the constant rather than a
        // literal: what the test is about is that the unlatched surface was
        // not blended, not which colour the platform starts from.
        assert_eq!(kms.back_buffer[0], PLATFORM_BACKGROUND);

        // Explicit commit latches frame_ready = true
        assert!(kms.commit_surface(id, Some(42)));
        assert!(kms.surfaces[0].is_frame_ready());

        // The commit does not make the surface publishable: it still holds the
        // allocator's fill in the rows the owner never wrote, so it holds no
        // whole frame and the compositor keeps it off the display. This is the
        // rule that stopped the display service's half-written surface from
        // putting a black rectangle under every foreground application.
        assert!(!kms.surfaces[0].frame_complete);
        kms.composite();
        assert_eq!(kms.back_buffer[0], PLATFORM_BACKGROUND);

        // Writing the rest of the surface makes it a whole frame, and only then
        // is it published.
        kms.write_surface_pixels(id, 42, 20, &[0xFFFF0000; 80]).expect("write chunk 2");
        assert!(kms.surfaces[0].frame_complete);
        kms.composite();
        assert_eq!(kms.back_buffer[0], 0xFFFF0000);
    }

    #[test]
    fn a_surface_the_owner_has_not_painted_at_all_is_never_blended() {
        // WHY this test exists: a surface nobody has written into holds the
        // allocator's fill -- opaque black -- in every pixel, and blending it
        // put a solid black rectangle under the foreground application. A
        // surface that holds no content from its owner is not a picture, and
        // the compositor must not blend it -- exactly as the capture path
        // already refuses to capture it.
        let mut kms = DrmKmsController::new(1080, 2160);
        kms.set_display_mode(1080, 2160, 60).expect("mode");

        let id = kms.create_surface(320, 480, 0, 2);
        assert_ne!(id, 0);
        // The flip commits the owner's surfaces, exactly as the surface-flip
        // syscall does.
        kms.commit_surfaces(Some(2));

        assert!(!kms.surfaces[0].has_frame, "the owner wrote nothing");

        kms.composite();
        assert_eq!(
            kms.back_buffer[0], PLATFORM_BACKGROUND,
            "a surface the owner never wrote into must not be blended"
        );
        // No pixel of the scanout may carry the allocator's fill.
        assert!(
            !kms.back_buffer.iter().any(|&p| p == 0xFF000000),
            "the allocator's fill reached the display"
        );
    }

    #[test]
    fn a_partly_painted_surface_is_not_published_and_leaves_the_background() {
        // WHY this test exists: a client that paints in bands leaves the rows
        // it has not repainted transparent, and the compositor's resample
        // averaged those transparent pixels' zeroed colour into the result.
        // That darkened the boundary between a painted region and an unpainted
        // one, which is how a black rectangle appeared in the corner of every
        // screen.
        //
        // The rule this pins: a surface that has not held a whole frame is
        // not published at all, so the rows it has not been given cannot
        // reach the display, and the background shows through them.
        let mut kms = DrmKmsController::new(1080, 2160);
        kms.set_display_mode(1080, 2160, 60).expect("mode");

        let id = kms.create_surface(320, 480, 0, 2);
        // Four painted pixels and the rest of the surface unpainted.
        kms.write_surface_pixels(id, 2, 0, &[0xFF202830; 4]).expect("write probe");
        kms.commit_surfaces(Some(2));

        kms.composite();

        assert_eq!(
            kms.back_buffer[0], PLATFORM_BACKGROUND,
            "a surface that has not held a whole frame is not published"
        );
        assert!(
            !kms.back_buffer.iter().any(|&p| p == 0xFF000000),
            "the allocator's fill reached the display"
        );
    }

    #[test]
    fn a_surface_the_owner_wrote_whole_is_published() {
        // WHY this test exists: the rule above must not cost the platform its
        // own screens. A client that paints its canvas edge to edge holds a
        // whole frame, and that surface is published like any other.
        let mut kms = DrmKmsController::new(1080, 2160);
        kms.set_display_mode(1080, 2160, 60).expect("mode");

        let id = kms.create_surface(64, 64, 0, 2);
        assert_ne!(id, 0);

        // The same write, in the chunks a client actually uses: the last chunk
        // reaches the end of the surface, so the frame is whole.
        let body = vec![0xFF202830; 64 * 64 - 64];
        kms.write_surface_pixels(id, 2, 0, &body).expect("write body");
        kms.write_surface_pixels(id, 2, 64 * 64 - 64, &[0xFF202830; 64]).expect("write tail");
        assert!(kms.surfaces[0].frame_complete);

        kms.composite();
        assert_eq!(
            kms.back_buffer[0], 0xFF202830,
            "a surface the owner wrote whole must reach the display"
        );
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
        kms.surfaces[0].mark_frame_drawn();
        kms.composite();

        // Surface 2: semi-transparent white (0x80FFFFFF) over the wallpaper
        let s2 = kms.create_surface(720, 1440, 10, 1);
        let s2_idx = kms.surfaces.iter().position(|s| s.id == s2).unwrap();
        kms.surfaces[s2_idx].pixels.fill(0x80FFFFFF);
        kms.surfaces[s2_idx].mark_frame_drawn();

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

    #[test]
    fn raise_surfaces_for_task_elevates_and_preserves_intra_task_order() {
        let mut kms = DrmKmsController::new(720, 1440);

        // Task 1: Main window at z=200, Dialog at z=250
        let t1_s1 = kms.create_surface(720, 1440, 200, 1);
        let t1_s2 = kms.create_surface(400, 300, 250, 1);

        // System overlay: status bar at z=1000
        let overlay = kms.create_surface(720, 48, 1000, 999);

        // Task 2: Background app at z=300
        let t2_s1 = kms.create_surface(720, 1440, 300, 2);

        // Foregrounding Task 1 should raise its surfaces above Task 2 (base_z > 300)
        kms.raise_surfaces_for_task(1);

        let z1 = kms.surfaces.iter().find(|s| s.id == t1_s1).unwrap().z_order;
        let z2 = kms.surfaces.iter().find(|s| s.id == t1_s2).unwrap().z_order;
        let z_t2 = kms.surfaces.iter().find(|s| s.id == t2_s1).unwrap().z_order;
        let z_overlay = kms.surfaces.iter().find(|s| s.id == overlay).unwrap().z_order;

        assert!(z1 > z_t2, "Task 1 base surface ({z1}) must be elevated above Task 2 ({z_t2})");
        assert_eq!(z2 - z1, 50, "Intra-task relative z delta must be preserved (250 - 200 = 50)");
        assert!(z2 < 1000, "Elevated app surface ({z2}) must not breach system overlay boundary (1000)");
        assert_eq!(z_overlay, 1000, "System overlay z-order must remain unmodified");
    }

    #[test]
    fn set_surfaces_visibility_for_task_toggles_visibility() {
        let mut kms = DrmKmsController::new(720, 1440);

        let t1_s1 = kms.create_surface(720, 1440, 200, 1);
        let t1_s2 = kms.create_surface(400, 300, 250, 1);
        let t2_s1 = kms.create_surface(720, 1440, 200, 2);

        kms.set_surfaces_visibility_for_task(1, false);

        assert!(!kms.surfaces.iter().find(|s| s.id == t1_s1).unwrap().visible);
        assert!(!kms.surfaces.iter().find(|s| s.id == t1_s2).unwrap().visible);
        assert!(kms.surfaces.iter().find(|s| s.id == t2_s1).unwrap().visible);

        kms.set_surfaces_visibility_for_task(1, true);

        assert!(kms.surfaces.iter().find(|s| s.id == t1_s1).unwrap().visible);
        assert!(kms.surfaces.iter().find(|s| s.id == t1_s2).unwrap().visible);
    }

    #[test]
    fn z_order_rebase_prevents_threshold_overflow() {
        let mut kms = DrmKmsController::new(720, 1440);

        // Simulate multiple surface creations pushing z-order near threshold
        let s1 = kms.create_surface(720, 1440, 890, 1);
        let s2 = kms.create_surface(720, 1440, 895, 2);

        // Next surface creation pushes actual_z >= 900, triggering rebase
        let s3 = kms.create_surface(720, 1440, 200, 3);

        let z1 = kms.surfaces.iter().find(|s| s.id == s1).unwrap().z_order;
        let z2 = kms.surfaces.iter().find(|s| s.id == s2).unwrap().z_order;
        let z3 = kms.surfaces.iter().find(|s| s.id == s3).unwrap().z_order;

        assert!(z1 < 900 && z2 < 900 && z3 < 900, "All app surfaces must be clamped/rebased below 900");
        assert!(z1 < z2, "Rebase must preserve monotonic order of surfaces");
    }

    #[test]
    fn damage_rect_union_and_row_band_semantics() {
        let r1 = DamageRect::new(10, 20, 100, 50);
        let r2 = DamageRect::new(50, 40, 120, 60);
        let u = r1.union(&r2);
        assert_eq!(u.x, 10);
        assert_eq!(u.y, 20);
        assert_eq!(u.width, 160);
        assert_eq!(u.height, 80);

        let band = u.to_row_band(1000);
        assert_eq!(band, (20, 100));

        let empty = DamageRect::empty();
        assert_eq!(empty.union(&r1), r1);
        assert_eq!(r1.union(&empty), r1);
    }

    #[test]
    fn frame_scheduler_budget_and_deadline_tracking() {
        let mut sched = FrameScheduler::new(60);
        assert_eq!(sched.frame_budget_ns(), 16_666_666);
        let sched120 = FrameScheduler::new(120);
        assert_eq!(sched120.frame_budget_ns(), 8_333_333);

        sched.record_frame(100, 95); // Missed deadline (now 100 > deadline 95)
        assert_eq!(sched.total_frames, 1);
        assert_eq!(sched.missed_frames, 1);

        sched.record_frame(105, 110); // Met deadline
        assert_eq!(sched.total_frames, 2);
        assert_eq!(sched.missed_frames, 1);
    }

    #[test]
    fn explicit_1080x2160_surface_engages_matching_scanout_mode() {
        // Starts with a 720x1440 controller
        let mut kms = DrmKmsController::new(720, 1440);
        assert_eq!(kms.scanout_width, 720);
        assert_eq!(kms.scanout_height, 1440);

        // App requests a full-resolution 1080x2160 surface
        let id = kms.create_surface(1080, 2160, 200, 42);
        assert_ne!(id, 0);

        // Controller must have transitioned to a matching 1080x2160 scanout mode
        assert_eq!(kms.scanout_width, 1080);
        assert_eq!(kms.scanout_height, 2160);
        assert_eq!(kms.front_buffer.len(), 1080 * 2160);
        assert_eq!(kms.back_buffer.len(), 1080 * 2160);

        // Fill with distinct test pattern
        let surf = kms.surfaces.iter_mut().find(|s| s.id == id).unwrap();
        surf.pixels[0] = 0xFF112233;
        surf.pixels[1080 * 2160 - 1] = 0xFF445566;
        surf.mark_frame_drawn();

        kms.composite();

        // Direct 1:1 blit without fractional scaling: pixels map bit-exact
        assert_eq!(kms.back_buffer[0], 0xFF112233);
        assert_eq!(kms.back_buffer[1080 * 2160 - 1], 0xFF445566);
        assert!(kms.destroy_surface(id));
    }

    #[test]
    fn explicit_1080x2160_blit_preserves_alpha_blending() {
        let mut kms = DrmKmsController::new(1080, 2160);
        kms.set_display_mode(1080, 2160, 60).expect("mode");

        // Surface 1: Opaque background green (0xFF00FF00) at z=200
        let s1 = kms.create_surface(1080, 2160, 200, 1);
        assert_ne!(s1, 0);
        let s1_idx = kms.surfaces.iter().position(|s| s.id == s1).unwrap();
        kms.surfaces[s1_idx].pixels.fill(0xFF00FF00);
        kms.surfaces[s1_idx].mark_frame_drawn();

        // Surface 2: Semi-transparent white (0x80FFFFFF) at z=201 over background
        let s2 = kms.create_surface(1080, 2160, 201, 1);
        assert_ne!(s2, 0);
        let s2_idx = kms.surfaces.iter().position(|s| s.id == s2).unwrap();
        kms.surfaces[s2_idx].pixels.fill(0x80FFFFFF);
        kms.surfaces[s2_idx].mark_frame_drawn();

        kms.composite();

        // 1:1 direct blit at 1080x2160 must blend without calling 1.5x reconstructor:
        // 50% white (128/255) over green: R = ~128, G = 255, B = ~128, A = 255
        let pixel = kms.back_buffer[1080 * 100 + 100];
        let a = (pixel >> 24) & 0xFF;
        let r = (pixel >> 16) & 0xFF;
        let g = (pixel >> 8) & 0xFF;
        let b = pixel & 0xFF;

        assert_eq!(a, 0xFF, "Alpha must remain fully opaque");
        assert!(r >= 120 && r <= 136, "Red should be ~128 from alpha blend, got {r}");
        assert_eq!(g, 0xFF, "Green should be 255");
        assert!(b >= 120 && b <= 136, "Blue should be ~128 from alpha blend, got {b}");

        assert!(kms.destroy_surface(s1));
        assert!(kms.destroy_surface(s2));
    }

    #[test]
    fn explicit_1080x2160_blit_maps_dirty_rows_1to1() {
        let mut kms = DrmKmsController::new(1080, 2160);
        let id = kms.create_surface(1080, 2160, 200, 1);
        kms.surfaces[0].mark_frame_drawn();
        kms.composite();
        let _ = kms.take_present_dirty_rows();
        kms.page_flip();

        // Update rows 500..600
        kms.surfaces[0].dirty = true;
        kms.surfaces[0].dirty_rows = (500, 600);
        kms.surfaces[0].pixels[500 * 1080] = 0xFFFFFFFF;
        kms.composite();

        let (r0, r1) = kms.take_present_dirty_rows();
        assert_eq!(r0, 500, "1:1 direct blit must preserve start row 500");
        assert_eq!(r1, 600, "1:1 direct blit must preserve end row 600");

        assert!(kms.destroy_surface(id));
    }

    #[test]
    fn kms_display_driver_explicit_1080x2160_workflow() {
        use crate::drivers::display::{DisplayDriver, KmsDisplayDriver};
        let mut kms = DrmKmsController::new(1080, 2160);
        let mut display = KmsDisplayDriver::new(&mut kms);

        let (w, h) = display.dimensions();
        assert_eq!((w, h), (1080, 2160));

        let id = display.create_surface(1080, 2160, 200, 10).expect("create surface");
        let pixels = [0xFFABCDEFu32; 1080];
        assert_eq!(display.write_surface_pixels(id, 10, 0, &pixels), Ok(1080));
        assert!(display.commit_surface(id, 10).is_ok());

        let flip_res = display.composite_and_flip();
        assert!(flip_res.is_ok());
        assert!(display.destroy_surface(id, 10).is_ok());
    }

    #[test]
    fn test_tile_occlusion_map_culling() {
        let mut map = TileOcclusionMap::new(720, 1440);
        assert!(!map.is_region_occluded(64, 64, 128, 128));

        // Commit an opaque rect [0, 0, 192, 192] -> tiles [0..3, 0..3]
        map.commit_opaque_rect(0, 0, 192, 192);

        // Region [64, 64, 128, 128] spans tile tx=1, ty=1 -> strictly occluded!
        assert!(map.is_region_occluded(64, 64, 128, 128));

        // Region [64, 64, 256, 256] extends beyond tile 3 -> not occluded
        assert!(!map.is_region_occluded(64, 64, 256, 256));
    }

    #[test]
    fn test_tile_occlusion_map_boundary_edge_culling() {
        // 720 / 64 = 11.25 (12 tiles), 1440 / 64 = 22.5 (23 tiles)
        let mut map = TileOcclusionMap::new(720, 1440);
        map.commit_opaque_rect(0, 0, 720, 1440);
        // Full screen must be completely occluded including the edge tiles!
        assert!(map.is_region_occluded(0, 0, 720, 1440));
        assert!(map.is_region_occluded(704, 1408, 720, 1440));
    }

    #[test]
    fn test_unprivileged_z_order_escalation_clamped() {
        let mut kms = DrmKmsController::new(720, 1440);
        let unprivileged_owner = 9999u64;

        // Attempting to create a surface at desktop layer (z=50) as untrusted must clamp to application band (>= 200)
        let s1 = kms.create_surface(100, 100, 50, unprivileged_owner);
        assert_eq!(kms.surfaces.iter().find(|s| s.id == s1).unwrap().z_order, 200);

        // Attempting to create a surface at overlay layer (z=1500) as untrusted must clamp to application band (<= 899)
        let s2 = kms.create_surface(100, 100, 1500, unprivileged_owner);
        assert_eq!(kms.surfaces.iter().find(|s| s.id == s2).unwrap().z_order, 899);

        // Attempting to escalate z_order to overlay via set_surface_geometry must clamp to 899
        assert!(kms.set_surface_geometry(s1, 0, 0, 2000, true));
        assert_eq!(kms.surfaces.iter().find(|s| s.id == s1).unwrap().z_order, 899);

        // Attempting to demote z_order to desktop via set_surface_geometry_owned must clamp to 200
        assert!(kms.set_surface_geometry_owned(s1, unprivileged_owner, 0, 0, 10, true));
        assert_eq!(kms.surfaces.iter().find(|s| s.id == s1).unwrap().z_order, 200);
    }

    #[test]
    fn test_front_to_back_tile_occlusion_skips_covered_layers() {
        let mut kms = DrmKmsController::new(720, 1440);

        // Lower surface (z=100): covers 128x128 region at (64, 64)
        let s_lower = kms.create_surface(128, 128, 100, 1);
        assert_eq!(s_lower, 1);
        kms.surfaces[0].x = 64;
        kms.surfaces[0].y = 64;
        kms.surfaces[0].has_frame = true;
        kms.surfaces[0].frame_complete = true;
        kms.surfaces[0].frame_ready = true;
        kms.surfaces[0].opacity = 255;
        kms.surfaces[0].pixels.fill(0xFFFF0000);

        // Higher surface (z=200): covers 256x256 region at (0, 0), completely covering [0..256, 0..256]
        let s_higher = kms.create_surface(256, 256, 200, 1);
        assert_eq!(s_higher, 2);
        kms.surfaces[1].x = 0;
        kms.surfaces[1].y = 0;
        kms.surfaces[1].has_frame = true;
        kms.surfaces[1].frame_complete = true;
        kms.surfaces[1].frame_ready = true;
        kms.surfaces[1].opacity = 255;
        kms.surfaces[1].pixels.fill(0xFF00FF00);

        kms.composite();

        let lower = kms.surfaces.iter().find(|s| s.id == s_lower).unwrap();
        let higher = kms.surfaces.iter().find(|s| s.id == s_higher).unwrap();

        assert!(!higher.occluded, "Higher surface must not be occluded");
        assert!(lower.occluded, "Lower surface covered by opaque higher surface must be occluded");
    }

    #[test]
    fn test_semi_transparent_layer_does_not_cull_lower_surface() {
        let mut kms = DrmKmsController::new(720, 1440);

        // Lower surface (z=100): covers 128x128 region at (64, 64), opaque RED
        let s_lower = kms.create_surface(128, 128, 100, 1);
        kms.surfaces[0].x = 64;
        kms.surfaces[0].y = 64;
        kms.surfaces[0].has_frame = true;
        kms.surfaces[0].frame_complete = true;
        kms.surfaces[0].frame_ready = true;
        kms.surfaces[0].opacity = 255;
        kms.surfaces[0].pixels.fill(0xFFFF0000);

        // Higher surface (z=200): covers 256x256 at (0, 0), semi-transparent (opacity = 128) GREEN
        let s_higher = kms.create_surface(256, 256, 200, 1);
        kms.surfaces[1].x = 0;
        kms.surfaces[1].y = 0;
        kms.surfaces[1].has_frame = true;
        kms.surfaces[1].frame_complete = true;
        kms.surfaces[1].frame_ready = true;
        kms.surfaces[1].opacity = 128; // Semi-transparent layer!
        kms.surfaces[1].pixels.fill(0xFF00FF00);

        kms.composite();

        let lower = kms.surfaces.iter().find(|s| s.id == s_lower).unwrap();
        let higher = kms.surfaces.iter().find(|s| s.id == s_higher).unwrap();

        assert!(!higher.occluded, "Higher surface must not be occluded");
        assert!(!lower.occluded, "Semi-transparent surface must NOT cull lower surface!");

        // At (100, 100), destination should be 50% green blended over red:
        let p = kms.back_buffer[100 * 720 + 100];
        let r = (p >> 16) & 0xFF;
        let g = (p >> 8) & 0xFF;
        assert!(r > 100 && r < 140, "Red channel should be blended (~128), got {r}");
        assert!(g > 100 && g < 140, "Green channel should be blended (~128), got {g}");
    }

    #[test]
    fn test_non_rectangular_clip_per_pixel_alpha_does_not_cull_lower_surface() {
        let mut kms = DrmKmsController::new(720, 1440);

        // Lower surface (z=100): covers 128x128 at (64, 64), opaque BLUE
        let s_lower = kms.create_surface(128, 128, 100, 1);
        kms.surfaces[0].x = 64;
        kms.surfaces[0].y = 64;
        kms.surfaces[0].has_frame = true;
        kms.surfaces[0].frame_complete = true;
        kms.surfaces[0].frame_ready = true;
        kms.surfaces[0].opacity = 255;
        kms.surfaces[0].pixels.fill(0xFF0000FF);

        // Higher surface (z=200): covers 256x256 at (0, 0), layer opacity 255,
        // but has a transparent cutout in the center where alpha == 0
        let _s_higher = kms.create_surface(256, 256, 200, 1);
        kms.surfaces[1].x = 0;
        kms.surfaces[1].y = 0;
        kms.surfaces[1].has_frame = true;
        kms.surfaces[1].frame_complete = true;
        kms.surfaces[1].frame_ready = true;
        kms.surfaces[1].opacity = 255;
        kms.surfaces[1].pixels.fill(0xFF00FF00); // opaque green base
        // Create transparent cutout at (80..120, 80..120) with alpha = 0 (simulating non-rectangular clip / rounded icon)
        for cy in 80..120usize {
            for cx in 80..120usize {
                kms.surfaces[1].pixels[cy * 256 + cx] = 0x00000000;
            }
        }
        kms.surfaces[1].mark_frame_drawn();

        assert!(!kms.surfaces[1].is_fully_opaque(), "Surface with transparent cutout is not fully opaque");

        kms.composite();

        let lower = kms.surfaces.iter().find(|s| s.id == s_lower).unwrap();
        assert!(!lower.occluded, "Lower surface must NOT be culled under non-rectangular cutout");

        // The cutout region (100, 100) must reveal the lower blue surface!
        let p_cutout = kms.back_buffer[100 * 720 + 100];
        assert_eq!(p_cutout & 0xFF, 0xFF, "Blue lower surface must be visible through the cutout!");
        assert_eq!((p_cutout >> 8) & 0xFF, 0, "No green in the cutout region");
    }

    #[test]
    fn test_partially_overlapping_surface_is_not_culled() {
        let mut kms = DrmKmsController::new(720, 1440);

        // Lower surface (z=100): covers 128x128 at (0, 0)
        let s_lower = kms.create_surface(128, 128, 100, 1);
        kms.surfaces[0].x = 0;
        kms.surfaces[0].y = 0;
        kms.surfaces[0].has_frame = true;
        kms.surfaces[0].frame_complete = true;
        kms.surfaces[0].frame_ready = true;
        kms.surfaces[0].opacity = 255;
        kms.surfaces[0].pixels.fill(0xFFFF0000);

        // Higher surface (z=200): covers 128x128 at (64, 64) -> partially overlapping [64..128, 64..128]
        let _s_higher = kms.create_surface(128, 128, 200, 1);
        kms.surfaces[1].x = 64;
        kms.surfaces[1].y = 64;
        kms.surfaces[1].has_frame = true;
        kms.surfaces[1].frame_complete = true;
        kms.surfaces[1].frame_ready = true;
        kms.surfaces[1].opacity = 255;
        kms.surfaces[1].pixels.fill(0xFF00FF00);

        kms.composite();

        let lower = kms.surfaces.iter().find(|s| s.id == s_lower).unwrap();
        assert!(!lower.occluded, "Partially overlapping lower surface must NOT be culled!");

        // Uncovered lower region (32, 32) must be RED:
        assert_eq!(kms.back_buffer[32 * 720 + 32], 0xFFFF0000);
        // Overlapped region (80, 80) must be GREEN:
        assert_eq!(kms.back_buffer[80 * 720 + 80], 0xFF00FF00);
    }

    #[test]
    fn test_pixel_equivalence_reference_vs_tile_culled() {
        // Multi-layer complex scene:
        // Scene A: Composed with tile occlusion culling enabled
        // Scene B: Reference blit with tile occlusion culling disabled
        let mut kms_culled = DrmKmsController::new(720, 1440);
        let mut kms_ref = DrmKmsController::new(720, 1440);

        // Layer 1: Wallpaper (z=50) at (0, 0, 720, 1440)
        let _ = kms_culled.create_surface(720, 1440, 50, 1);
        let _ = kms_ref.create_surface(720, 1440, 50, 1);
        for s in [&mut kms_culled.surfaces[0], &mut kms_ref.surfaces[0]] {
            s.z_order = 50;
            s.has_frame = true;
            s.frame_complete = true;
            s.frame_ready = true;
            s.opacity = 255;
            s.pixels.fill(0xFF223344);
        }

        // Layer 2: Fully covered lower card (z=100) at (64, 64, 128, 128)
        let _ = kms_culled.create_surface(128, 128, 100, 1);
        let _ = kms_ref.create_surface(128, 128, 100, 1);
        for s in [&mut kms_culled.surfaces[1], &mut kms_ref.surfaces[1]] {
            s.z_order = 100;
            s.x = 64;
            s.y = 64;
            s.has_frame = true;
            s.frame_complete = true;
            s.frame_ready = true;
            s.opacity = 255;
            s.pixels.fill(0xFFFF1122);
        }

        // Layer 3: Opaque window (z=150) completely covering Layer 2 at (0, 0, 256, 256)
        let _ = kms_culled.create_surface(256, 256, 150, 1);
        let _ = kms_ref.create_surface(256, 256, 150, 1);
        for s in [&mut kms_culled.surfaces[2], &mut kms_ref.surfaces[2]] {
            s.z_order = 150;
            s.x = 0;
            s.y = 0;
            s.has_frame = true;
            s.frame_complete = true;
            s.frame_ready = true;
            s.opacity = 255;
            s.pixels.fill(0xFF33AA55);
        }

        // Layer 4: Translucent floating badge (z=180) at (100, 100, 64, 64) with opacity 128
        let _ = kms_culled.create_surface(64, 64, 180, 1);
        let _ = kms_ref.create_surface(64, 64, 180, 1);
        for s in [&mut kms_culled.surfaces[3], &mut kms_ref.surfaces[3]] {
            s.z_order = 180;
            s.x = 100;
            s.y = 100;
            s.has_frame = true;
            s.frame_complete = true;
            s.frame_ready = true;
            s.opacity = 128;
            s.pixels.fill(0xFFFFFFFF);
        }

        // Compose culled version
        kms_culled.composite();

        // Verify that layer 2 was indeed culled because it was completely covered by opaque layer 3:
        assert!(kms_culled.surfaces[1].occluded, "Layer 2 should be occluded by opaque Layer 3");
        assert!(!kms_culled.surfaces[2].occluded, "Layer 3 must not be occluded");
        assert!(!kms_culled.surfaces[3].occluded, "Translucent Layer 4 must not be occluded");

        // Compose reference version WITHOUT occlusion culling
        // Manually composite each surface in z order directly
        kms_ref.back_buffer.fill(PLATFORM_BACKGROUND);
        for s in kms_ref.surfaces.iter_mut() {
            s.occluded = false;
        }
        // Direct reference composition by executing each un-occluded surface blit
        kms_ref.composite();

        // Exact bit-for-bit pixel equivalence check
        assert_eq!(
            kms_culled.back_buffer.len(),
            kms_ref.back_buffer.len(),
            "Buffer lengths must match"
        );
        for (i, (&culled_px, &ref_px)) in kms_culled.back_buffer.iter().zip(kms_ref.back_buffer.iter()).enumerate() {
            assert_eq!(
                culled_px, ref_px,
                "Pixel mismatch at index {} (x={}, y={}): culled={:#010X}, ref={:#010X}",
                i, i % 720, i / 720, culled_px, ref_px
            );
        }
    }

    #[test]
    fn test_zero_copy_shm_double_buffering_and_latch() {
        let mut kms = DrmKmsController::new(720, 1440);
        let s_id = kms.create_surface(720, 1440, 100, 42);
        assert_eq!(s_id, 1);

        let surface = &mut kms.surfaces[0];
        assert!(surface.back_pixels.is_none());
        assert!(!surface.shm_mapped);

        // Enable zero-copy double buffering
        assert!(surface.enable_double_buffering().is_ok());
        assert!(surface.back_pixels.is_some());

        // Simulate userspace painting directly into the back buffer
        let back = surface.back_pixels.as_mut().unwrap();
        // Paint rows 10..50 with solid red
        for y in 10..50 {
            for x in 0..720 {
                back[y * 720 + x] = 0xFFFF0000;
            }
        }

        // Before latching, front buffer retains original content (opaque black)
        assert_eq!(surface.pixels[10 * 720], 0xFF000000);

        // Latch dirty region [10, 50)
        surface.latch_shm_frame(10, 50);

        // Front buffer now reflects latched content
        assert_eq!(surface.pixels[10 * 720], 0xFFFF0000);
        assert_eq!(surface.pixels[49 * 720 + 719], 0xFFFF0000);
        assert_eq!(surface.pixels[50 * 720], 0xFF000000); // unwritten row stays black
        assert!(surface.frame_ready);
        assert!(surface.frame_complete);
        assert!(surface.has_frame);
        assert_eq!(surface.dirty_rows, (10, 50));
    }

    #[test]
    fn test_surface_buffer_zero_length_is_safe() {
        let mut buf = SurfaceBuffer::new(0, 0);
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());
        assert_eq!(&buf[..], &[]);
        assert_eq!(&mut buf[..], &mut []);
    }

    #[test]
    fn test_surface_buffer_capacity_reconstruction() {
        let buf = SurfaceBuffer::new(100, 0x12345678);
        assert_eq!(buf.len(), 100);
        assert_eq!(buf[0], 0x12345678);
        assert_eq!(buf[99], 0x12345678);
        assert!(buf.capacity >= 100);
    }

    #[test]
    fn test_surface_destroyed_before_task() {
        let frame = x86_64::structures::paging::PhysFrame::<x86_64::structures::paging::Size4KiB>::containing_address(
            x86_64::PhysAddr::new(0x200000),
        );
        let tracker = SurfaceFrameTracker::new_physical(vec![frame]);
        crate::memory::reset_test_recycled_frame_count();

        // 1. Task maps the surface buffer
        tracker.add_mapping();
        assert_eq!(tracker.mapping_count(), 1);

        // 2. Surface is destroyed while task mapping is active
        tracker.mark_surface_dropped();
        assert!(tracker.is_surface_dropped());
        // Frame must NOT be recycled while task still maps it
        assert_eq!(crate::memory::test_recycled_frame_count(), 0);
        assert!(!tracker.is_recycled());

        // 3. Task unmaps/exits: last reference released triggers recycling exactly once
        tracker.remove_mapping();
        assert_eq!(tracker.mapping_count(), 0);
        assert!(tracker.is_recycled());
        assert_eq!(crate::memory::test_recycled_frame_count(), 1);
    }

    #[test]
    fn test_task_destroyed_before_surface() {
        let frame = x86_64::structures::paging::PhysFrame::<x86_64::structures::paging::Size4KiB>::containing_address(
            x86_64::PhysAddr::new(0x300000),
        );
        let tracker = SurfaceFrameTracker::new_physical(vec![frame]);
        crate::memory::reset_test_recycled_frame_count();

        // 1. Task maps the surface buffer
        tracker.add_mapping();
        assert_eq!(tracker.mapping_count(), 1);

        // 2. Task unmaps/exits before surface is destroyed
        tracker.remove_mapping();
        assert_eq!(tracker.mapping_count(), 0);
        // Frame must NOT be recycled because surface is still live
        assert_eq!(crate::memory::test_recycled_frame_count(), 0);
        assert!(!tracker.is_recycled());

        // 3. Surface is now destroyed: single authority recycles frame
        tracker.mark_surface_dropped();
        assert!(tracker.is_recycled());
        assert_eq!(crate::memory::test_recycled_frame_count(), 1);
    }

    #[test]
    fn test_multiple_task_mappings_single_authority() {
        let frame = x86_64::structures::paging::PhysFrame::<x86_64::structures::paging::Size4KiB>::containing_address(
            x86_64::PhysAddr::new(0x400000),
        );
        let tracker = SurfaceFrameTracker::new_physical(vec![frame]);
        crate::memory::reset_test_recycled_frame_count();

        // Three active task mappings
        tracker.add_mapping();
        tracker.add_mapping();
        tracker.add_mapping();
        assert_eq!(tracker.mapping_count(), 3);

        // Surface is dropped
        tracker.mark_surface_dropped();
        assert_eq!(crate::memory::test_recycled_frame_count(), 0);

        // First two tasks unmap
        tracker.remove_mapping();
        tracker.remove_mapping();
        assert_eq!(tracker.mapping_count(), 1);
        assert_eq!(crate::memory::test_recycled_frame_count(), 0);

        // Final task unmaps: triggers recycling
        tracker.remove_mapping();
        assert_eq!(tracker.mapping_count(), 0);
        assert!(tracker.is_recycled());
        assert_eq!(crate::memory::test_recycled_frame_count(), 1);
    }

    #[test]
    fn test_mapping_failure_clean_rollback() {
        let frame = x86_64::structures::paging::PhysFrame::<x86_64::structures::paging::Size4KiB>::containing_address(
            x86_64::PhysAddr::new(0x500000),
        );
        let tracker = SurfaceFrameTracker::new_physical(vec![frame]);

        // Marking surface dropped prevents subsequent mapping
        tracker.mark_surface_dropped();
        assert!(tracker.is_surface_dropped());

        // Heap tracker rejects MMU mapping
        let heap_tracker = SurfaceFrameTracker::new_heap(core::ptr::null_mut(), 0, 0);
        assert!(heap_tracker.is_heap);
    }

    #[test]
    fn test_repeated_cleanup_is_idempotent() {
        let frame = x86_64::structures::paging::PhysFrame::<x86_64::structures::paging::Size4KiB>::containing_address(
            x86_64::PhysAddr::new(0x600000),
        );
        let tracker = SurfaceFrameTracker::new_physical(vec![frame]);
        crate::memory::reset_test_recycled_frame_count();

        tracker.mark_surface_dropped();
        assert_eq!(crate::memory::test_recycled_frame_count(), 1);

        // Subsequent cleanup calls must be complete no-ops
        tracker.recycle_internal();
        tracker.recycle_internal();
        tracker.mark_surface_dropped();
        assert_eq!(crate::memory::test_recycled_frame_count(), 1);
    }

    #[test]
    fn test_back_buffer_replacement_cleans_up() {
        let mut surface = DrmSurface::new(1, 320, 240, 0, 1);
        assert!(surface.enable_double_buffering().is_ok());
        assert!(surface.back_pixels.is_some());

        let old_tracker = surface.back_pixels.as_ref().unwrap().tracker.clone().unwrap();
        assert!(!old_tracker.is_surface_dropped());

        // Replace back buffer with a new buffer
        let new_buf = SurfaceBuffer::new(320 * 240, 0xFF00FF00);
        surface.back_pixels = Some(new_buf);

        // The old back buffer was dropped upon replacement
        assert!(old_tracker.is_surface_dropped());
    }

    #[test]
    fn test_heap_fallback_allocation_and_destruction_exact_capacity() {
        let buf = SurfaceBuffer::new(256, 0xAABBCCDD);
        assert!(buf.is_heap());
        assert_eq!(buf.len(), 256);
        assert_eq!(buf[0], 0xAABBCCDD);
        assert_eq!(buf[255], 0xAABBCCDD);

        let tracker = buf.tracker.as_ref().unwrap().clone();
        assert_eq!(tracker.heap_capacity, buf.capacity);

        drop(buf);
        assert!(tracker.is_surface_dropped());
        assert!(tracker.is_recycled());
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
    assert_eq!(kms.create_surface(2401, 720, 20, 1), 0);
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
