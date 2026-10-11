//! InterOS Core visual design tokens shared by native Rust applications.
//!
//! These are semantic tokens, not app-specific styling. Apps should compose
//! surfaces, spacing and typography from this module so the launcher, system
//! line and native applications retain one visual language.

pub mod walls;
pub mod face;
pub mod typeface;

pub mod color {
    // Canonical light system
    pub const BACKGROUND: u32 = 0xFFF7F6F2;
    pub const SURFACE: u32 = 0xFFFFFFFF;
    pub const SURFACE_SOFT: u32 = 0xFFEFEEE9;
    pub const SURFACE_RAISED: u32 = 0xFFEFEEE9;
    pub const BORDER: u32 = 0xFFE1E0DB;
    pub const TEXT: u32 = 0xFF111214;
    pub const TEXT_MUTED: u32 = 0xFF777773;

    // Canonical dark surfaces
    pub const DARK_BACKGROUND: u32 = 0xFF111214;
    pub const DARK_SURFACE: u32 = 0xFF25282A;
    pub const DARK_BORDER: u32 = 0xFF35383D;
    pub const DARK_TEXT: u32 = 0xFFF5F6F7;
    pub const DARK_TEXT_MUTED: u32 = 0xFF9AA0A6;

    // Restrained semantic accents
    pub const ACCENT: u32 = 0xFF2563EB;
    pub const SUCCESS: u32 = 0xFF2E8B57;
    pub const WARNING: u32 = 0xFFD97706;
    pub const DANGER: u32 = 0xFFDC2626;
    pub const EMERGENCY: u32 = 0xFFFF5555;
}

pub mod spacing {
    pub const GRID_4: usize = 4;
    pub const GRID_8: usize = 8;
    pub const GRID_12: usize = 12;
    pub const GRID_16: usize = 16;
    pub const GRID_20: usize = 20;
    pub const GRID_24: usize = 24;
    pub const GRID_32: usize = 32;
    pub const GRID_40: usize = 40;
    pub const GRID_48: usize = 48;
    pub const GRID_64: usize = 64;

    pub const XS: usize = 4;
    pub const SM: usize = 8;
    pub const MD: usize = 16;
    pub const LG: usize = 24;
    pub const XL: usize = 32;
    pub const XXL: usize = 48;
}

pub mod radius {
    pub const SMALL: usize = 8;
    pub const CONTROL: usize = 12;
    pub const CARD: usize = 18;
    pub const SHEET: usize = 24;
    pub const MAJOR: usize = 32;
    pub const PILL: usize = 999;

    pub const SM: u32 = 12;
    pub const MD: u32 = 18;
    pub const LG: u32 = 24;
}

pub mod touch {
    pub const MIN_TARGET: usize = 44;
    pub const PREFERRED_TARGET: usize = 48;
    pub const PRIMARY_ACTION: usize = 56;
}

pub mod motion {
    pub const PRESS_FEEDBACK_MS: u32 = 100;
    pub const QUICK_CONTROLS_MS: u32 = 150;
    pub const STANDARD_TRANSITION_MS: u32 = 200;
    pub const MAJOR_TRANSITION_MS: u32 = 300;
    pub const ACTIVITY_EXPANSION_MS: u32 = 280;

    pub const FAST: u32 = 120;
    pub const STANDARD: u32 = 220;
    pub const EMPHASIZED: u32 = 360;
}

pub mod viewport {
    /// Canonical portrait smartphone logical viewport dimensions (9:18 standard).
    pub const LOGICAL_WIDTH: usize = 720;
    pub const LOGICAL_HEIGHT: usize = 1440;

    /// Hardware / QEMU scanout dimensions.
    pub const SCANOUT_WIDTH: usize = LOGICAL_WIDTH;
    pub const SCANOUT_HEIGHT: usize = LOGICAL_HEIGHT;

    /// System safe areas and chrome metrics.
    pub const STATUS_BAR_HEIGHT: usize = 48;
    pub const NAV_BAR_HEIGHT: usize = 64;
    pub const GESTURE_BAR_HEIGHT: usize = 48;
    pub const CONTENT_PADDING: usize = 24;

    /// Display orientation mode.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum DisplayOrientation {
        Portrait,
        LandscapeLeft,
        LandscapeRight,
    }

    /// Logical safe area insets for mobile applications.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Insets {
        pub top: usize,
        pub bottom: usize,
        pub left: usize,
        pub right: usize,
    }

    impl Insets {
        pub const STANDARD: Insets = Insets {
            top: STATUS_BAR_HEIGHT,
            bottom: GESTURE_BAR_HEIGHT,
            left: 0,
            right: 0,
        };

        pub const FULLSCREEN: Insets = Insets {
            top: 0,
            bottom: 0,
            left: 0,
            right: 0,
        };
    }

    /// Unified display coordinate transformer between logical mobile canvas and scanout.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct DisplayTransform {
        pub orientation: DisplayOrientation,
        pub scanout_w: usize,
        pub scanout_h: usize,
    }

    impl DisplayTransform {
        pub const fn new(orientation: DisplayOrientation) -> Self {
            Self {
                orientation,
                scanout_w: SCANOUT_WIDTH,
                scanout_h: SCANOUT_HEIGHT,
            }
        }

        /// Map logical portrait (lx, ly) in 720x1440 to scanout destination (sx, sy).
        pub fn logical_to_scanout(&self, lx: usize, ly: usize) -> Option<(usize, usize)> {
            if lx >= LOGICAL_WIDTH || ly >= LOGICAL_HEIGHT || self.scanout_w == 0 || self.scanout_h == 0 {
                return None;
            }

            match self.orientation {
                DisplayOrientation::Portrait => {
                    let (dst_w, dst_h) = if LOGICAL_WIDTH * self.scanout_h <= LOGICAL_HEIGHT * self.scanout_w {
                        (
                            (LOGICAL_WIDTH * self.scanout_h / LOGICAL_HEIGHT).max(1).min(self.scanout_w),
                            self.scanout_h,
                        )
                    } else {
                        (
                            self.scanout_w,
                            (LOGICAL_HEIGHT * self.scanout_w / LOGICAL_WIDTH).max(1).min(self.scanout_h),
                        )
                    };
                    let off_x = (self.scanout_w - dst_w) / 2;
                    let off_y = (self.scanout_h - dst_h) / 2;
                    let sx = off_x + lx * dst_w / LOGICAL_WIDTH;
                    let sy = off_y + ly * dst_h / LOGICAL_HEIGHT;
                    Some((sx.min(self.scanout_w - 1), sy.min(self.scanout_h - 1)))
                }
                DisplayOrientation::LandscapeRight => {
                    let sx = ly * self.scanout_w / LOGICAL_HEIGHT;
                    let sy = self.scanout_h - 1 - lx * self.scanout_h / LOGICAL_WIDTH;
                    Some((sx.min(self.scanout_w - 1), sy.min(self.scanout_h - 1)))
                }
                DisplayOrientation::LandscapeLeft => {
                    let sx = self.scanout_w - 1 - ly * self.scanout_w / LOGICAL_HEIGHT;
                    let sy = lx * self.scanout_h / LOGICAL_WIDTH;
                    Some((sx.min(self.scanout_w - 1), sy.min(self.scanout_h - 1)))
                }
            }
        }
    }
}

pub mod primitives {
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Rect {
        pub x: usize,
        pub y: usize,
        pub w: usize,
        pub h: usize,
    }

    impl Rect {
        pub const fn new(x: usize, y: usize, w: usize, h: usize) -> Self {
            Self { x, y, w, h }
        }

        pub fn contains(&self, px: usize, py: usize) -> bool {
            px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
        }

        pub fn inset(&self, pad: usize) -> Self {
            Self {
                x: self.x + pad,
                y: self.y + pad,
                w: self.w.saturating_sub(pad * 2),
                h: self.h.saturating_sub(pad * 2),
            }
        }
    }

    /// Anti-aliased rounded rectangle rasterizer with 1px border support.
    #[inline]
    pub fn draw_rounded_rect(
        x: usize,
        y: usize,
        rect: Rect,
        radius: usize,
        fill: u32,
        border: Option<u32>,
        border_w: usize,
    ) -> Option<u32> {
        if !rect.contains(x, y) {
            return None;
        }
        let r = radius.min(rect.w / 2).min(rect.h / 2);
        if r == 0 {
            if let Some(b_color) = border {
                if border_w > 0
                    && (x < rect.x + border_w
                        || x >= rect.x + rect.w - border_w
                        || y < rect.y + border_w
                        || y >= rect.y + rect.h - border_w)
                {
                    return Some(b_color);
                }
            }
            return Some(fill);
        }

        let in_left = x < rect.x + r;
        let in_right = x >= rect.x + rect.w - r;
        let in_top = y < rect.y + r;
        let in_bottom = y >= rect.y + rect.h - r;

        if (in_left || in_right) && (in_top || in_bottom) {
            let cx = if in_left { rect.x + r } else { rect.x + rect.w - r - 1 } as i32;
            let cy = if in_top { rect.y + r } else { rect.y + rect.h - r - 1 } as i32;
            let dx = (x as i32) - cx;
            let dy = (y as i32) - cy;
            let dist_sq = dx * dx + dy * dy;
            let r_i32 = r as i32;
            let r_sq = r_i32 * r_i32;

            if dist_sq > r_sq {
                return None;
            }

            if let Some(b_color) = border {
                if border_w > 0 {
                    let inner_r = r_i32.saturating_sub(border_w as i32);
                    if dist_sq >= inner_r * inner_r {
                        return Some(b_color);
                    }
                }
            }
            return Some(fill);
        }

        if let Some(b_color) = border {
            if border_w > 0
                && (x < rect.x + border_w
                    || x >= rect.x + rect.w - border_w
                    || y < rect.y + border_w
                    || y >= rect.y + rect.h - border_w)
            {
                return Some(b_color);
            }
        }
        Some(fill)
    }

    /// Renders a capsule / pill (fully rounded ends).
    #[inline]
    pub fn draw_pill(
        x: usize,
        y: usize,
        rect: Rect,
        fill: u32,
        border: Option<u32>,
    ) -> Option<u32> {
        let r = rect.h / 2;
        draw_rounded_rect(x, y, rect, r, fill, border, 1)
    }

    /// Renders a circular button or badge.
    #[inline]
    pub fn draw_circle(
        x: usize,
        y: usize,
        cx: usize,
        cy: usize,
        radius: usize,
        fill: u32,
        border: Option<u32>,
    ) -> Option<u32> {
        let dx = (x as i32) - (cx as i32);
        let dy = (y as i32) - (cy as i32);
        let dist_sq = dx * dx + dy * dy;
        let r_i32 = radius as i32;
        let r_sq = r_i32 * r_i32;
        if dist_sq > r_sq {
            return None;
        }
        if let Some(b_color) = border {
            let inner_r = r_i32.saturating_sub(1);
            if dist_sq >= inner_r * inner_r {
                return Some(b_color);
            }
        }
        Some(fill)
    }

    /// Renders a crisp 1px divider.
    #[inline]
    pub fn draw_divider(
        x: usize,
        y: usize,
        x0: usize,
        y0: usize,
        len: usize,
        is_horizontal: bool,
        color: u32,
    ) -> Option<u32> {
        if is_horizontal {
            if y == y0 && x >= x0 && x < x0 + len {
                Some(color)
            } else {
                None
            }
        } else {
            if x == x0 && y >= y0 && y < y0 + len {
                Some(color)
            } else {
                None
            }
        }
    }
}

pub use crate::font;



#[cfg(test)]
mod tests {
    use super::viewport::{DisplayOrientation, DisplayTransform, LOGICAL_HEIGHT, LOGICAL_WIDTH};

    #[test]
    fn portrait_transform_uses_the_actual_scanout_dimensions() {
        let mut transform = DisplayTransform::new(DisplayOrientation::Portrait);
        transform.scanout_w = 1280;
        transform.scanout_h = 720;

        assert_eq!(transform.logical_to_scanout(0, 0), Some((460, 0)));
        assert_eq!(transform.logical_to_scanout(719, 1439), Some((819, 719)));
    }

    #[test]
    fn native_portrait_scanout_has_no_letterbox() {
        let transform = DisplayTransform::new(DisplayOrientation::Portrait);
        assert_eq!(transform.logical_to_scanout(0, 0), Some((0, 0)));
        assert_eq!(
            transform.logical_to_scanout(LOGICAL_WIDTH - 1, LOGICAL_HEIGHT - 1),
            Some((LOGICAL_WIDTH - 1, LOGICAL_HEIGHT - 1))
        );
    }

    #[test]
    fn landscape_rotation_maps_logical_corners_to_scanout_corners() {
        let mut right = DisplayTransform::new(DisplayOrientation::LandscapeRight);
        right.scanout_w = 1280;
        right.scanout_h = 720;
        assert_eq!(right.logical_to_scanout(0, 0), Some((0, 719)));
        assert_eq!(right.logical_to_scanout(719, 1439), Some((1279, 0)));

        let mut left = DisplayTransform::new(DisplayOrientation::LandscapeLeft);
        left.scanout_w = 1280;
        left.scanout_h = 720;
        assert_eq!(left.logical_to_scanout(0, 0), Some((1279, 0)));
        assert_eq!(left.logical_to_scanout(719, 1439), Some((0, 719)));
    }

    #[test]
    fn invalid_points_and_zero_scanout_are_rejected() {
        let mut transform = DisplayTransform::new(DisplayOrientation::Portrait);
        transform.scanout_w = 0;
        assert_eq!(transform.logical_to_scanout(0, 0), None);

        transform.scanout_w = 720;
        transform.scanout_h = 1440;
        assert_eq!(transform.logical_to_scanout(720, 0), None);
        assert_eq!(transform.logical_to_scanout(0, 1440), None);
    }
}
