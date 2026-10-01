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
