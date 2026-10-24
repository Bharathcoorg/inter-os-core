//! Display Hardware Abstraction Layer (DisplayHal)
//!
//! Provides a clean, minimal trait interface for raw display scanout operations.
//! This HAL is the ONLY kernel-level display primitive. All consumer-facing UI
//! (home screen, app views, notification shade, etc.) should use this HAL
//! through the compositor/shell layer — never by directly writing pixels.
//!
//! The `FramebufferWriter` in `framebuffer.rs` implements this trait and currently
//! also contains bootstrap consumer UI rendering for development purposes.
//! In production, consumer rendering will migrate to userspace compositor processes.

/// Pixel format supported by the display hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayPixelFormat {
    Rgb888,
    Bgr888,
    Grayscale8,
}

/// Display hardware information.
#[derive(Debug, Clone, Copy)]
pub struct DisplayInfo {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bytes_per_pixel: usize,
    pub pixel_format: DisplayPixelFormat,
}

/// Display Hardware Abstraction Layer trait.
///
/// Kernel-space components and the compositor use this trait to interact with
/// the raw display scanout buffer. This is intentionally minimal — it provides
/// only primitive pixel and rectangle operations.
///
/// Consumer UI rendering (app chrome, status bars, animations) belongs in
/// the compositor shell layer, NOT in HAL implementations.
pub trait DisplayHal {
    /// Returns display hardware information.
    fn display_info(&self) -> Option<DisplayInfo>;

    /// Writes a single pixel at (x, y) with the given RGB color.
    fn set_pixel(&mut self, x: usize, y: usize, r: u8, g: u8, b: u8);

    /// Fills a rectangle with a solid color.
    fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, r: u8, g: u8, b: u8);

    /// Draws a 1px outline rectangle.
    fn draw_rect_outline(&mut self, x: usize, y: usize, w: usize, h: usize, thickness: usize, r: u8, g: u8, b: u8);

    /// Clears the entire display to a solid color.
    fn clear(&mut self, r: u8, g: u8, b: u8);

    /// Draws an 8x8 bitmap character at the given position with optional scaling.
    fn draw_char(&mut self, x: usize, y: usize, ch: char, scale: usize, r: u8, g: u8, b: u8);

    /// Draws a string of characters at the given position.
    fn draw_text(&mut self, x: usize, y: usize, text: &str, scale: usize, r: u8, g: u8, b: u8);
}

/// Canonical display unit: Logical Density-independent Pixels (dp).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Dp(pub i32);

impl core::ops::Add for Dp {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl core::ops::Sub for Dp {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

/// Canonical display unit: Physical Device Pixels (px).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Px(pub i32);

impl core::ops::Add for Px {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl core::ops::Sub for Px {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

/// Display physical orientation.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Orientation {
    #[default]
    Portrait = 0,
    LandscapeRight = 1,
    PortraitInverted = 2,
    LandscapeLeft = 3,
}

impl Orientation {
    #[inline]
    pub const fn is_portrait(self) -> bool {
        matches!(self, Orientation::Portrait | Orientation::PortraitInverted)
    }

    #[inline]
    pub const fn is_landscape(self) -> bool {
        matches!(self, Orientation::LandscapeRight | Orientation::LandscapeLeft)
    }
}

/// System safe area insets in physical pixels.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SafeAreaInsetsPx {
    pub top: Px,
    pub bottom: Px,
    pub left: Px,
    pub right: Px,
}

impl SafeAreaInsetsPx {
    pub const ZERO: Self = Self {
        top: Px(0),
        bottom: Px(0),
        left: Px(0),
        right: Px(0),
    };
}

/// Canonical Display Metrics Contract for InterOS.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayMetrics {
    pub physical_width_px: u32,
    pub physical_height_px: u32,
    pub logical_width_dp: u32,
    pub logical_height_dp: u32,
    pub density_dpi: u32,
    pub scale_factor_permille: u32,
    pub orientation: Orientation,
    pub safe_insets_px: SafeAreaInsetsPx,
    pub refresh_rate_hz: u16,
}

impl DisplayMetrics {
    /// Canonical QEMU phone profile: 720x1440 physical at 1:1 with the
    /// 720x1440 logical space (320 DPI). A higher-DPI scanout would need the
    /// apps to paint at that resolution; otherwise it is an upscale.
    pub const fn canonical_phone() -> Self {
        Self {
            physical_width_px: 720,
            physical_height_px: 1440,
            logical_width_dp: 720,
            logical_height_dp: 1440,
            density_dpi: 320,
            scale_factor_permille: 1000,
            orientation: Orientation::Portrait,
            // The status bar is 48 logical px and physical == logical, so the
            // inset is 48 px rather than the 48 x 1.5 = 72 the retired 1080x2160
            // scanout used.
            safe_insets_px: SafeAreaInsetsPx {
                top: Px(48),
                bottom: Px(48),
                left: Px(0),
                right: Px(0),
            },
            refresh_rate_hz: 60,
        }
    }

    /// Converts a logical DP measurement to physical pixels using rounding half-up.
    #[inline]
    pub const fn dp_to_px(&self, dp: Dp) -> Px {
        let permille = self.scale_factor_permille as i64;
        let v = dp.0 as i64;
        let sign = if v >= 0 { 1 } else { -1 };
        let abs_v = if v >= 0 { v } else { -v };
        let px = (abs_v * permille + 500) / 1000;
        Px((sign * px) as i32)
    }

    /// Converts physical pixels to logical DP using rounding half-up.
    #[inline]
    pub const fn px_to_dp(&self, px: Px) -> Dp {
        let permille = self.scale_factor_permille as i64;
        if permille == 0 {
            return Dp(0);
        }
        let v = px.0 as i64;
        let sign = if v >= 0 { 1 } else { -1 };
        let abs_v = if v >= 0 { v } else { -v };
        let dp = (abs_v * 1000 + (permille / 2)) / permille;
        Dp((sign * dp) as i32)
    }

    /// Returns the exact physical pixel hairline width (always 1 physical pixel).
    #[inline]
    pub const fn hairline_px(&self) -> Px {
        Px(1)
    }

    /// Frame budget in nanoseconds for the configured refresh rate.
    #[inline]
    pub const fn frame_budget_ns(&self) -> u64 {
        Self::frame_budget_ns_for_hz(self.refresh_rate_hz)
    }

    /// Frame budget in nanoseconds for a specified refresh rate (e.g. 60, 120, 144 Hz).
    #[inline]
    pub const fn frame_budget_ns_for_hz(hz: u16) -> u64 {
        if hz == 0 {
            16_666_666
        } else {
            1_000_000_000 / (hz as u64)
        }
    }

    /// Returns a new `DisplayMetrics` adapted for the given `Orientation`.
    #[inline]
    pub fn with_orientation(&self, new_orientation: Orientation) -> Self {
        let was_portrait = self.orientation.is_portrait();
        let will_be_portrait = new_orientation.is_portrait();

        let (new_pw, new_ph, new_lw, new_lh) = if was_portrait != will_be_portrait {
            (
                self.physical_height_px,
                self.physical_width_px,
                self.logical_height_dp,
                self.logical_width_dp,
            )
        } else {
            (
                self.physical_width_px,
                self.physical_height_px,
                self.logical_width_dp,
                self.logical_height_dp,
            )
        };

        let new_insets = match new_orientation {
            Orientation::Portrait => SafeAreaInsetsPx {
                top: self.safe_insets_px.top,
                bottom: self.safe_insets_px.bottom,
                left: self.safe_insets_px.left,
                right: self.safe_insets_px.right,
            },
            Orientation::LandscapeRight => SafeAreaInsetsPx {
                top: self.safe_insets_px.left,
                bottom: self.safe_insets_px.right,
                left: self.safe_insets_px.bottom,
                right: self.safe_insets_px.top,
            },
            Orientation::PortraitInverted => SafeAreaInsetsPx {
                top: self.safe_insets_px.bottom,
                bottom: self.safe_insets_px.top,
                left: self.safe_insets_px.right,
                right: self.safe_insets_px.left,
            },
            Orientation::LandscapeLeft => SafeAreaInsetsPx {
                top: self.safe_insets_px.right,
                bottom: self.safe_insets_px.left,
                left: self.safe_insets_px.top,
                right: self.safe_insets_px.bottom,
            },
        };

        Self {
            physical_width_px: new_pw,
            physical_height_px: new_ph,
            logical_width_dp: new_lw,
            logical_height_dp: new_lh,
            density_dpi: self.density_dpi,
            scale_factor_permille: self.scale_factor_permille,
            orientation: new_orientation,
            safe_insets_px: new_insets,
            refresh_rate_hz: self.refresh_rate_hz,
        }
    }
}
