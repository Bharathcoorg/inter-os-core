// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Sovereign Mobile Display Refresh-Rate Governor & Ambient Dimming HAL (AArch64)
//!
//! **This module contains no display controller driver.** There are NO MMIO reads and NO MMIO writes
//! against `mmio_base` anywhere in this file, and no panel timing generator, VSYNC interrupt,
//! LTPO controller, OLED power rail, or DC dimming hardware is implemented or described. `mmio_base`
//! is retained only as a bind point a concrete controller driver would be written against; until
//! `set_hardware_present(true)` is called by such a driver, no controller exists.
//!
//! What this file actually is: a **software refresh-rate and brightness policy model**. It decides
//! which refresh rate and brightness a panel *should* be given under a given battery and ambient
//! light condition. Those are policy outputs, not measurements of a panel.
//!
//! Status of each layer, stated explicitly:
//! - **Unavailable**: panel resolution, refresh rate, brightness in nits, DC dimming, framebuffer
//!   scanout, VSYNC. `brightness_nits` is `None` and `current_hz` is `None` until a bound
//!   controller reports them.
//! - **Emulated**: none. `init_hardware` does not assert 400 nits, 60 Hz, or DC dimming.
//! - **Present**: none. Zero display hardware is bound at build time.
//!
//! Reusable software:
//! - Refresh-rate governor with battery conservation policy
//! - Ambient-light to brightness policy mapping
//! - Buffer slot bookkeeping for a future VSYNC-driven ping-pong compositor
//! - Zero Ambient Authority: display reconfiguration requires `CapabilityType::DisplayControllerDirect`.

use crate::capability::{CapabilityType, Rights, CapabilityTable};

/// Reserved offsets for a future concrete controller driver.
///
/// These are documented register offsets only. THIS MODULE NEVER READS OR WRITES THEM, and their
/// presence must not be read as evidence that a controller with this layout exists.
pub const DISP_REG_CTRL: usize = 0x00;
pub const DISP_REG_STATUS: usize = 0x04;
pub const DISP_REG_WIDTH: usize = 0x08;
pub const DISP_REG_HEIGHT: usize = 0x0C;
pub const DISP_REG_REFRESH_RATE: usize = 0x10;
pub const DISP_REG_BRIGHTNESS: usize = 0x14;
pub const DISP_REG_FB0_ADDR: usize = 0x18;
pub const DISP_REG_FB1_ADDR: usize = 0x1C;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayError {
    CapabilityDenied,
    HardwareDisabled,
    InvalidRefreshRate,
    ThrottledByBatteryGovernor,
    InvalidBrightness,
}

/// Refresh rates this policy model knows how to *request*.
///
/// Enum membership is not a claim that the bound panel supports these rates; a panel's supported
/// range must come from a bound controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshRateHz {
    Hz10 = 10,   // Always-on display / eBook static reading (ultra-low power)
    Hz30 = 30,   // Video playback
    Hz60 = 60,   // Standard mobile UI
    Hz120 = 120, // High-frequency touch interaction
}

pub struct MobileDisplayHal {
    /// Bind point for a concrete controller driver. THIS MODULE PERFORMS NO MMIO ACCESS AT IT.
    pub mmio_base: usize,
    /// True only when a bound controller has enabled the panel. Not set by `init_hardware`.
    pub enabled: bool,
    pub hardware_present: bool,
    /// Panel resolution, or `None` when no controller has reported it.
    pub resolution: Option<(u32, u32)>,
    /// Refresh rate currently requested, or `None` when nothing has been applied.
    pub current_hz: Option<RefreshRateHz>,
    /// Brightness currently requested in nits, or `None` when nothing has been applied.
    pub brightness_nits: Option<u16>,
    /// Whether DC dimming has been requested; `None` until a controller supports it.
    pub dc_dimming_active: Option<bool>,
    /// Ping-pong slot index tracked in software. A toggle here is NOT a frame present.
    pub active_buffer_slot: u8,
    pub frame_flips_count: u64,
}

impl MobileDisplayHal {
    pub const fn new(mmio_base: usize) -> Self {
        Self {
            mmio_base,
            enabled: false,
            hardware_present: false,
            // WHY None, not Some((1080, 2400)): the previous seed asserted a specific FHD+ panel
            // geometry that no controller reported. No controller, no resolution.
            resolution: None,
            // WHY None, not Some(Hz60): a refresh rate belongs to a panel. None means no rate has
            // been requested, which also makes "no VRR range 10Hz-120Hz" self-evident.
            current_hz: None,
            // WHY None, not 300: brightness in nits is a panel measurement/actuation, not a default.
            brightness_nits: None,
            dc_dimming_active: None,
            active_buffer_slot: 0,
            frame_flips_count: 0,
        }
    }

    /// Brings up the software policy model only.
    ///
    /// This performs no MMIO access and does NOT enable the display. `enabled` stays `false` and
    /// `hardware_present` stays `false` because no controller has been bound; the previous version
    /// set `enabled = true`, `brightness_nits = 400`, and `dc_dimming_active = true` while itself
    /// declaring `hardware_present = false`, which asserted a lit panel with a measured brightness.
    pub fn init_hardware(&mut self) {
        self.enabled = false;
        self.hardware_present = false;
        self.current_hz = None;
        self.brightness_nits = None;
        self.dc_dimming_active = None;
    }

    pub fn set_hardware_present(&mut self, present: bool) {
        self.hardware_present = present;
        self.enabled = present;
        if !present {
            // WHY: drop every applied value when the controller goes away, so nothing reads back
            // as a live panel state that is no longer driven by hardware.
            self.current_hz = None;
            self.brightness_nits = None;
            self.dc_dimming_active = None;
            self.resolution = None;
        }
    }

    /// Records the panel resolution and supported refresh range reported by a bound controller.
    ///
    /// Without this the model has no panel to reason about; the seeded values must not be guessed.
    pub fn set_panel_info(&mut self, width: u32, height: u32) -> Result<(), DisplayError> {
        if !self.hardware_present {
            return Err(DisplayError::HardwareDisabled);
        }
        if width == 0 || height == 0 {
            return Err(DisplayError::InvalidRefreshRate);
        }
        self.resolution = Some((width, height));
        Ok(())
    }

    /// Requests a display refresh rate, applying the battery conservation policy.
    ///
    /// Requires a bound controller.
    pub fn set_refresh_rate(
        &mut self,
        target_hz: RefreshRateHz,
        battery_pct: u8,
        caps: &CapabilityTable,
    ) -> Result<(), DisplayError> {
        caps.check(CapabilityType::DisplayControllerDirect, Rights::WRITE)
            .map_err(|_| DisplayError::CapabilityDenied)?;

        if !self.enabled || !self.hardware_present {
            return Err(DisplayError::HardwareDisabled);
        }

        // Battery Conservation Policy: When battery is low (<= 20%), cap to 60Hz max
        if battery_pct <= 20 && target_hz == RefreshRateHz::Hz120 {
            self.current_hz = Some(RefreshRateHz::Hz60);
            return Err(DisplayError::ThrottledByBatteryGovernor);
        }

        self.current_hz = Some(target_hz);
        Ok(())
    }

    /// Requests a panel brightness in nits, with hardware DC dimming as a policy option.
    ///
    /// Requires a bound controller. `nits` is a request, not a measurement.
    pub fn set_brightness(
        &mut self,
        nits: u16,
        dc_dimming: bool,
        caps: &CapabilityTable,
    ) -> Result<(), DisplayError> {
        caps.check(CapabilityType::DisplayControllerDirect, Rights::WRITE)
            .map_err(|_| DisplayError::CapabilityDenied)?;

        // WHY: previously this gated on `enabled` alone, so a brightness request "succeeded"
        // against no panel at all. Both conditions are required now.
        if !self.enabled || !self.hardware_present {
            return Err(DisplayError::HardwareDisabled);
        }

        if nits > 1200 {
            return Err(DisplayError::InvalidBrightness);
        }

        self.brightness_nits = Some(nits);
        self.dc_dimming_active = Some(dc_dimming);
        Ok(())
    }

    /// Maps an ambient-light measurement to a brightness request via policy.
    ///
    /// The returned nits are this model's policy target, not a measured panel luminance. Requires a
    /// bound controller because the request is ultimately delivered to a panel.
    pub fn auto_adjust_from_ambient(
        &mut self,
        lux: u32,
        caps: &CapabilityTable,
    ) -> Result<u16, DisplayError> {
        let target_nits = if lux < 10 {
            50 // Dark room
        } else if lux < 500 {
            300 // Normal indoor
        } else if lux < 2000 {
            600 // Bright office / daylight
        } else {
            1000 // Direct outdoor sunlight (High Brightness Mode)
        };

        self.set_brightness(target_nits, true, caps)?;
        Ok(target_nits)
    }

    /// Selects the next ping-pong buffer slot.
    ///
    /// Requires a bound controller. This is bookkeeping for a future VSYNC-driven compositor, NOT a
    /// frame present: there is no VSYNC interrupt and no scanout in this module, so the previous
    /// implementation's "flip on VSYNC" comment and its `frame_flips_count` increment both claimed
    /// hardware synchronisation that does not exist here.
    pub fn flip_buffer(&mut self, caps: &CapabilityTable) -> Result<u8, DisplayError> {
        caps.check(CapabilityType::DisplayControllerDirect, Rights::WRITE)
            .map_err(|_| DisplayError::CapabilityDenied)?;

        if !self.enabled || !self.hardware_present {
            return Err(DisplayError::HardwareDisabled);
        }

        self.active_buffer_slot = if self.active_buffer_slot == 0 { 1 } else { 0 };
        self.frame_flips_count += 1;
        Ok(self.active_buffer_slot)
    }
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
    /// Canonical flagship phone profile (1080x2160 FHD+ physical, 720x1440 dp logical, 480 DPI).
    pub const fn canonical_phone() -> Self {
        Self {
            physical_width_px: 1080,
            physical_height_px: 2160,
            logical_width_dp: 720,
            logical_height_dp: 1440,
            density_dpi: 480,
            scale_factor_permille: 1500,
            orientation: Orientation::Portrait,
            safe_insets_px: SafeAreaInsetsPx {
                top: Px(72),
                bottom: Px(72),
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
