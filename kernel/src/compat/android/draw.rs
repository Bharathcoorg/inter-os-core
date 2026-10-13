//! Bounded Android Drawing Runtime
//!
//! Provides a validated, strictly bounded drawing path for Android activity surfaces.
//! Android activity surfaces are allocated through KMS via `AndroidWindowManager`.
//! Drawing commands emitted by untrusted APK/DEX workloads or Binder clients are
//! validated against surface bounds, arithmetic overflow, command count ceilings,
//! and a cumulative per-frame area budget to prevent denial-of-service rasterization.

use alloc::vec::Vec;
use crate::drivers::display_kms::DRM_KMS;
use crate::task::TaskId;
use super::window::{ANDROID_SURFACE_WIDTH, ANDROID_SURFACE_HEIGHT};

/// Maximum drawing operations allowed in a single DrawList.
pub const MAX_DRAW_OPS_PER_LIST: usize = 256;

/// Cumulative pixel area budget per frame (2 full surface equivalents = 2 * 720 * 1320).
pub const MAX_FRAME_AREA_PIXELS: u64 = (ANDROID_SURFACE_WIDTH as u64) * (ANDROID_SURFACE_HEIGHT as u64) * 2;

/// Maximum dimension allowed for a single draw primitive.
pub const MAX_PRIMITIVE_DIMENSION: u32 = 4096;

/// Errors returned during drawing command validation and rasterization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawError {
    InvalidSurface,
    OwnershipDenied,
    OutOfBounds,
    IntegerOverflow,
    CommandCountExceeded,
    AreaBudgetExceeded,
    InvalidDimensions,
    MalformedCommand,
}

/// Primitive drawing operations supported on Android activity surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawOp {
    /// Clears the entire surface with an ARGB8888 color.
    Clear { color: u32 },
    /// Fills an axis-aligned rectangle with clipping to surface bounds.
    FillRect { x: i32, y: i32, width: u32, height: u32, color: u32 },
    /// Draws a 1-pixel horizontal line span.
    DrawHLine { x: i32, y: i32, width: u32, color: u32 },
    /// Draws a 1-pixel vertical line span.
    DrawVLine { x: i32, y: i32, height: u32, color: u32 },
}

/// A validated list of drawing operations to execute on an Android activity surface.
#[derive(Debug, Clone)]
pub struct DrawList {
    ops: Vec<DrawOp>,
}

impl DrawList {
    /// Creates an empty DrawList.
    pub const fn new() -> Self {
        Self { ops: Vec::new() }
    }

    /// Adds an operation to the list, verifying operation count, geometry, and area.
    pub fn push(&mut self, op: DrawOp) -> Result<(), DrawError> {
        if self.ops.len() >= MAX_DRAW_OPS_PER_LIST {
            return Err(DrawError::CommandCountExceeded);
        }

        match op {
            DrawOp::Clear { .. } => {}
            DrawOp::FillRect { width, height, .. } => {
                if width == 0 || height == 0 || width > MAX_PRIMITIVE_DIMENSION || height > MAX_PRIMITIVE_DIMENSION {
                    return Err(DrawError::InvalidDimensions);
                }
            }
            DrawOp::DrawHLine { width, .. } => {
                if width == 0 || width > MAX_PRIMITIVE_DIMENSION {
                    return Err(DrawError::InvalidDimensions);
                }
            }
            DrawOp::DrawVLine { height, .. } => {
                if height == 0 || height > MAX_PRIMITIVE_DIMENSION {
                    return Err(DrawError::InvalidDimensions);
                }
            }
        }

        self.ops.push(op);
        Ok(())
    }

    /// Returns a slice of the contained operations.
    pub fn ops(&self) -> &[DrawOp] {
        &self.ops
    }

    /// Number of operations in the list.
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// Whether the draw list contains no operations.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Calculates the cumulative pixel area demanded by all operations.
    pub fn total_area(&self) -> Result<u64, DrawError> {
        let mut total: u64 = 0;
        let surf_area = (ANDROID_SURFACE_WIDTH as u64) * (ANDROID_SURFACE_HEIGHT as u64);

        for op in &self.ops {
            let area = match *op {
                DrawOp::Clear { .. } => surf_area,
                DrawOp::FillRect { width, height, .. } => {
                    (width as u64).checked_mul(height as u64).ok_or(DrawError::IntegerOverflow)?
                }
                DrawOp::DrawHLine { width, .. } => width as u64,
                DrawOp::DrawVLine { height, .. } => height as u64,
            };
            total = total.checked_add(area).ok_or(DrawError::IntegerOverflow)?;
        }
        Ok(total)
    }
}

/// Clips an input axis-aligned box against the target surface bounds `[0, 0, surface_w, surface_h]`.
/// Returns `Some((clip_x, clip_y, clip_w, clip_h))` if there is a visible intersection, or `None` if fully offscreen.
pub fn clip_rect(x: i32, y: i32, width: u32, height: u32, surface_w: u32, surface_h: u32) -> Option<(usize, usize, usize, usize)> {
    if width == 0 || height == 0 {
        return None;
    }

    let end_x = (x as i64).checked_add(width as i64)?;
    let end_y = (y as i64).checked_add(height as i64)?;

    if end_x <= 0 || end_y <= 0 {
        return None;
    }

    let sw = surface_w as i64;
    let sh = surface_h as i64;

    if x as i64 >= sw || y as i64 >= sh {
        return None;
    }

    let clamped_x0 = (x as i64).max(0).min(sw) as usize;
    let clamped_y0 = (y as i64).max(0).min(sh) as usize;
    let clamped_x1 = end_x.max(0).min(sw) as usize;
    let clamped_y1 = end_y.max(0).min(sh) as usize;

    if clamped_x1 <= clamped_x0 || clamped_y1 <= clamped_y0 {
        return None;
    }

    Some((clamped_x0, clamped_y0, clamped_x1 - clamped_x0, clamped_y1 - clamped_y0))
}

/// Renders a validated `DrawList` onto an activity surface owned by `owner`.
///
/// Uses `DRM_KMS.write_surface_pixels` row-by-row, ensuring contiguous span writes
/// and real kernel dirty-row tracking.
pub fn render_draw_list(surface_id: u32, owner: TaskId, list: &DrawList) -> Result<usize, DrawError> {
    if list.ops.len() > MAX_DRAW_OPS_PER_LIST {
        return Err(DrawError::CommandCountExceeded);
    }

    let total_area = list.total_area()?;
    if total_area > MAX_FRAME_AREA_PIXELS {
        return Err(DrawError::AreaBudgetExceeded);
    }

    let mut kms = DRM_KMS.lock();
    let surf_w = ANDROID_SURFACE_WIDTH as usize;
    let surf_h = ANDROID_SURFACE_HEIGHT as usize;
    let mut total_pixels_written = 0;

    // Fixed chunk buffer for row-span writing without dynamic allocation.
    const ROW_BUFFER_CAP: usize = 720;
    let mut row_buf = [0u32; ROW_BUFFER_CAP];

    for op in &list.ops {
        match *op {
            DrawOp::Clear { color } => {
                row_buf.fill(color);
                for y in 0..surf_h {
                    let offset = y * surf_w;
                    let written = kms.write_surface_pixels(surface_id, owner.0, offset, &row_buf[..surf_w])
                        .map_err(map_kms_error)?;
                    total_pixels_written += written;
                }
            }
            DrawOp::FillRect { x, y, width, height, color } => {
                if let Some((cx, cy, cw, ch)) = clip_rect(x, y, width, height, ANDROID_SURFACE_WIDTH, ANDROID_SURFACE_HEIGHT) {
                    let span_w = cw.min(ROW_BUFFER_CAP);
                    row_buf[..span_w].fill(color);
                    for row in cy..(cy + ch) {
                        let offset = row * surf_w + cx;
                        let written = kms.write_surface_pixels(surface_id, owner.0, offset, &row_buf[..span_w])
                            .map_err(map_kms_error)?;
                        total_pixels_written += written;
                    }
                }
            }
            DrawOp::DrawHLine { x, y, width, color } => {
                if let Some((cx, cy, cw, _)) = clip_rect(x, y, width, 1, ANDROID_SURFACE_WIDTH, ANDROID_SURFACE_HEIGHT) {
                    let span_w = cw.min(ROW_BUFFER_CAP);
                    row_buf[..span_w].fill(color);
                    let offset = cy * surf_w + cx;
                    let written = kms.write_surface_pixels(surface_id, owner.0, offset, &row_buf[..span_w])
                        .map_err(map_kms_error)?;
                    total_pixels_written += written;
                }
            }
            DrawOp::DrawVLine { x, y, height, color } => {
                if let Some((cx, cy, _, ch)) = clip_rect(x, y, 1, height, ANDROID_SURFACE_WIDTH, ANDROID_SURFACE_HEIGHT) {
                    let pixel_val = [color];
                    for row in cy..(cy + ch) {
                        let offset = row * surf_w + cx;
                        let written = kms.write_surface_pixels(surface_id, owner.0, offset, &pixel_val)
                            .map_err(map_kms_error)?;
                        total_pixels_written += written;
                    }
                }
            }
        }
    }

    Ok(total_pixels_written)
}

fn map_kms_error(err: &'static str) -> DrawError {
    match err {
        "surface not found" => DrawError::InvalidSurface,
        "surface ownership denied" => DrawError::OwnershipDenied,
        "surface write out of bounds" | "surface write overflow" => DrawError::OutOfBounds,
        _ => DrawError::MalformedCommand,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clip_rect_in_bounds() {
        let clipped = clip_rect(10, 20, 100, 50, 720, 1320);
        assert_eq!(clipped, Some((10, 20, 100, 50)));
    }

    #[test]
    fn test_clip_rect_negative_origin() {
        let clipped = clip_rect(-20, -10, 100, 50, 720, 1320);
        assert_eq!(clipped, Some((0, 0, 80, 40)));
    }

    #[test]
    fn test_clip_rect_fully_offscreen() {
        assert_eq!(clip_rect(-100, -100, 50, 50, 720, 1320), None);
        assert_eq!(clip_rect(800, 50, 50, 50, 720, 1320), None);
        assert_eq!(clip_rect(50, 1400, 50, 50, 720, 1320), None);
    }

    #[test]
    fn test_clip_rect_partial_bottom_right() {
        let clipped = clip_rect(700, 1300, 50, 50, 720, 1320);
        assert_eq!(clipped, Some((700, 1300, 20, 20)));
    }

    #[test]
    fn test_clip_rect_zero_dimensions() {
        assert_eq!(clip_rect(10, 10, 0, 10, 720, 1320), None);
        assert_eq!(clip_rect(10, 10, 10, 0, 720, 1320), None);
    }

    #[test]
    fn test_clip_rect_overflow_protection() {
        assert_eq!(clip_rect(i32::MAX, 10, 100, 100, 720, 1320), None);
        assert_eq!(clip_rect(10, i32::MAX, 100, 100, 720, 1320), None);
    }

    #[test]
    fn test_draw_list_command_count_limit() {
        let mut list = DrawList::new();
        for _ in 0..MAX_DRAW_OPS_PER_LIST {
            assert!(list.push(DrawOp::Clear { color: 0xFF000000 }).is_ok());
        }
        assert_eq!(list.len(), MAX_DRAW_OPS_PER_LIST);
        assert_eq!(list.push(DrawOp::Clear { color: 0xFFFFFFFF }), Err(DrawError::CommandCountExceeded));
    }

    #[test]
    fn test_draw_list_dimension_validation() {
        let mut list = DrawList::new();
        assert_eq!(
            list.push(DrawOp::FillRect { x: 0, y: 0, width: 0, height: 10, color: 0 }),
            Err(DrawError::InvalidDimensions)
        );
        assert_eq!(
            list.push(DrawOp::FillRect { x: 0, y: 0, width: 5000, height: 10, color: 0 }),
            Err(DrawError::InvalidDimensions)
        );
        assert_eq!(
            list.push(DrawOp::DrawHLine { x: 0, y: 0, width: 0, color: 0 }),
            Err(DrawError::InvalidDimensions)
        );
        assert_eq!(
            list.push(DrawOp::DrawVLine { x: 0, y: 0, height: 0, color: 0 }),
            Err(DrawError::InvalidDimensions)
        );
    }

    #[test]
    fn test_area_budget_enforcement() {
        let mut list = DrawList::new();
        // Area of ANDROID_SURFACE is 720 * 1320 = 950,400.
        // MAX_FRAME_AREA_PIXELS is 2 * 950,400 = 1,900,800.
        // Two clears consume exactly the budget.
        assert!(list.push(DrawOp::Clear { color: 0xFF000000 }).is_ok());
        assert!(list.push(DrawOp::Clear { color: 0xFF000000 }).is_ok());
        assert_eq!(list.total_area(), Ok(MAX_FRAME_AREA_PIXELS));

        // A third clear exceeds the budget.
        assert!(list.push(DrawOp::Clear { color: 0xFF000000 }).is_ok());
        assert!(list.total_area().unwrap() > MAX_FRAME_AREA_PIXELS);
    }
}
