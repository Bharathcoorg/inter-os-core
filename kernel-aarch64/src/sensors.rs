// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Sovereign Mobile Sensor Fusion Subsystem & Hardware IMU/Barometer/Proximity/Light HAL (AArch64)
//!
//! This module is an **orientation classifier, gesture engine, and capability boundary, not a sensor
//! driver**. No InvenSense/Bosch/ST/APDS/BMP sensor is described, addressed, or emulated here, and
//! this file performs no read or write against `mmio_base`. `mmio_base` is retained only as the bind
//! point a concrete driver would be written against; until `set_hardware_present(true)` is called by
//! such a driver, no sensor exists and every telemetry read fails closed.
//!
//! Status of each layer, stated explicitly:
//! - **Unavailable**: accelerometer, gyroscope, magnetometer, proximity, ambient light, barometer,
//!   and derived altitude. The hub reports *no reading* until a driver feeds one.
//! - **Emulated**: none. The constructor does not invent gravity, lux, or pressure.
//! - **Present**: none. Zero sensor hardware is bound at build time.
//!
//! Reusable software that is honest about its own status:
//! - 6-Axis IMU sample types (3-axis Accelerometer in milli-g, 3-axis Gyroscope in deci-degrees/sec)
//! - Device orientation classifier (PortraitUp, PortraitDown, LandscapeLeft, LandscapeRight, FaceUp, FaceDown)
//! - Proximity & Ambient Light (ALS) ear-detection for automatic screen blanking during voice calls
//! - Adaptive display backlight governor (lux-based PWM duty cycle)
//! - Barometric pressure (Pa) & integer hypsometric altimeter calculation
//! - Real-time gesture recognition (Flip to Mute, Double-Tap to Wake, Shake for Emergency SOS)
//! - Zero Ambient Authority: Reading sensor telemetry requires `CapabilityType::SensorTelemetryRead`.

extern crate alloc;
use crate::capability::{CapabilityTable, CapabilityType, Rights};

/// Reference sea-level pressure used by the hypsometric altimeter formula.
///
/// This is a physical reference constant, not a measurement: it is the P0 term of
/// delta_h = (P0 - P) * 84 / 1000 and is never reported as "this device's pressure".
pub const STANDARD_SEA_LEVEL_PRESSURE_PA: i32 = 101_325; // 1013.25 hPa

/// Sentinel ambient-light value meaning "no ambient light reading has been taken yet".
pub const NO_LUX_READING: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorError {
    CapabilityDenied,
    HardwareDisabled,
    SensorFault,
    InvalidSamplingRate,
    CalibrationFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceOrientation {
    Unknown,
    PortraitUp,
    PortraitDown,
    LandscapeLeft,
    LandscapeRight,
    FaceUp,
    FaceDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenBlankState {
    Active,
    BlankedEarDetection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureEvent {
    None,
    FlipToMute,
    DoubleTapWake,
    ShakeEmergencySos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImuSample {
    pub accel_x_mg: i32,
    pub accel_y_mg: i32,
    pub accel_z_mg: i32,
    pub gyro_x_dps: i32,
    pub gyro_y_dps: i32,
    pub gyro_z_dps: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvironmentSample {
    pub proximity_mm: u16,
    pub ambient_lux: u32,
    pub pressure_pa: i32,
    pub altitude_meters: i32,
}

pub struct SensorHub {
    /// Bind point for a concrete sensor driver. THIS MODULE PERFORMS NO MMIO ACCESS AT IT.
    pub mmio_base: usize,
    pub enabled: bool,
    pub hardware_present: bool,
    pub imu_active: bool,
    pub prox_als_active: bool,
    pub barometer_active: bool,
    /// Most recent IMU sample, or `None` when no hardware has ever fed one.
    pub last_imu: Option<ImuSample>,
    /// Most recent environment sample, or `None` when no hardware has ever fed one.
    pub last_env: Option<EnvironmentSample>,
    /// Orientation derived from the most recent IMU sample, or `Unknown` when there has been none.
    pub orientation: DeviceOrientation,
    pub screen_state: ScreenBlankState,
    /// Backlight duty cycle in percent, or `None` until a lux reading has driven the governor.
    pub display_backlight_pwm: Option<u8>,
    pub last_gesture: GestureEvent,
    pub total_samples_read: u64,
    pub total_gestures_detected: u64,
}

impl SensorHub {
    pub const fn new(mmio_base: usize) -> Self {
        Self {
            mmio_base,
            enabled: false,
            hardware_present: false,
            imu_active: false,
            prox_als_active: false,
            barometer_active: false,
            // WHY None, not a fabricated sample: the constructor previously seeded
            // accel_y_mg = -1000 ("standard gravity in PortraitUp"), proximity 100 mm,
            // ambient_lux 350 ("standard indoor lighting"), sea-level pressure and altitude 0.
            // Those are plausible-looking measurements that were never taken, and they are exactly
            // what a fail-closed read path could leak. An unset sensor has no reading.
            last_imu: None,
            last_env: None,
            // WHY Unknown, not PortraitUp: orientation is a conclusion drawn from an accelerometer
            // sample. With no sample, no orientation may be asserted.
            orientation: DeviceOrientation::Unknown,
            screen_state: ScreenBlankState::Active,
            // WHY None, not 65: the PWM duty cycle is an output of the lux governor. No lux
            // reading means no governor output exists.
            display_backlight_pwm: None,
            last_gesture: GestureEvent::None,
            total_samples_read: 0,
            total_gestures_detected: 0,
        }
    }

    /// Initializes the sensor hub's software paths and sub-busse enable bits.
    ///
    /// This does NOT assert that a sensor exists: `hardware_present` stays `false` until a
    /// concrete driver binds, and `feed_hardware_telemetry` remains gated on it.
    pub fn init_hardware(&mut self) {
        self.enabled = true;
        self.imu_active = true;
        self.prox_als_active = true;
        self.barometer_active = true;
    }

    pub fn set_hardware_present(&mut self, present: bool) {
        self.hardware_present = present;
        if !present {
            self.imu_active = false;
            self.prox_als_active = false;
            self.barometer_active = false;
            // WHY: readings taken from a sensor that is no longer attached are not current
            // readings. Drop them so a stale sample can never be read back as live telemetry.
            self.last_imu = None;
            self.last_env = None;
            self.orientation = DeviceOrientation::Unknown;
            self.display_backlight_pwm = None;
        }
    }

    /// Ingests hardware sensor telemetry, enforces Zero Ambient Authority,
    /// and updates orientation, screen blanking, and gestures.
    pub fn feed_hardware_telemetry(
        &mut self,
        imu: ImuSample,
        env: EnvironmentSample,
        call_in_progress: bool,
        caps: &CapabilityTable,
    ) -> Result<GestureEvent, SensorError> {
        // Enforce Zero Ambient Authority
        caps.check(CapabilityType::SensorTelemetryRead, Rights::READ)
            .map_err(|_| SensorError::CapabilityDenied)?;

        if !self.enabled || !self.hardware_present {
            return Err(SensorError::HardwareDisabled);
        }

        let prev_orientation = self.orientation;

        // WHY: refuse a sample whose ambient-light field is the explicit "no reading" sentinel.
        // Treating it as 0 lux would drive the backlight governor to its floor on the strength of
        // a value that was never measured.
        if env.ambient_lux == NO_LUX_READING {
            return Err(SensorError::SensorFault);
        }

        self.total_samples_read += 1;

        // 1. Classify Device Orientation from Accelerometer Vector
        self.orientation = Self::classify_orientation(imu.accel_x_mg, imu.accel_y_mg, imu.accel_z_mg);

        // 2. Ear Detection & Screen Blanking State Machine
        // During an active voice call, if proximity <= 30 mm (3 cm), blank the screen
        if call_in_progress && env.proximity_mm <= 30 {
            self.screen_state = ScreenBlankState::BlankedEarDetection;
        } else {
            self.screen_state = ScreenBlankState::Active;
        }

        // 3. Ambient Light Display Backlight Governor
        let pwm = Self::calculate_backlight_pwm(env.ambient_lux);

        // 4. Barometric Hypsometric Altimeter (Integer math)
        // delta_h = (P0 - P) * 84 / 1000 meters, where P0 is the sea-level reference constant.
        let pressure_delta = STANDARD_SEA_LEVEL_PRESSURE_PA.saturating_sub(env.pressure_pa);
        let mut env = env;
        env.altitude_meters = (pressure_delta * 84) / 1000;

        // Commit the derived environment sample and the governor output together, so
        // `last_env` is only ever Some when its altitude was actually computed from a
        // real pressure reading.
        self.last_env = Some(env);
        self.last_imu = Some(imu);
        self.display_backlight_pwm = Some(pwm);

        // 5. Real-Time Gesture Engine
        let mut detected_gesture = GestureEvent::None;

        // Flip to Mute: transition to FaceDown while calling or ringing
        if prev_orientation != DeviceOrientation::FaceDown && self.orientation == DeviceOrientation::FaceDown {
            detected_gesture = GestureEvent::FlipToMute;
        }
        // Shake for Emergency SOS: high X-axis lateral acceleration >= 2000 mg (2.0g)
        else if imu.accel_x_mg.abs() >= 2000 {
            detected_gesture = GestureEvent::ShakeEmergencySos;
        }
        // Double-Tap to Wake: high Z-axis pulse >= 1500 mg
        else if imu.accel_z_mg.abs() >= 1500 {
            detected_gesture = GestureEvent::DoubleTapWake;
        }

        if detected_gesture != GestureEvent::None {
            self.last_gesture = detected_gesture;
            self.total_gestures_detected += 1;
        }

        Ok(detected_gesture)
    }

    /// Classifies device physical orientation based on acceleration vectors (milli-g).
    pub fn classify_orientation(ax: i32, ay: i32, az: i32) -> DeviceOrientation {
        if az < -700 {
            DeviceOrientation::FaceDown
        } else if az > 700 {
            DeviceOrientation::FaceUp
        } else if ay < -700 {
            DeviceOrientation::PortraitUp
        } else if ay > 700 {
            DeviceOrientation::PortraitDown
        } else if ax < -700 {
            DeviceOrientation::LandscapeLeft
        } else if ax > 700 {
            DeviceOrientation::LandscapeRight
        } else {
            DeviceOrientation::Unknown
        }
    }

    /// Computes display backlight PWM (10% - 100%) from ambient lux reading.
    pub fn calculate_backlight_pwm(lux: u32) -> u8 {
        if lux < 10 {
            10 // Pitch black: lowest brightness to prevent eye strain and save battery
        } else if lux <= 100 {
            30 // Dim indoor lighting
        } else if lux <= 1000 {
            65 // Standard office lighting
        } else {
            100 // Bright sunlight
        }
    }
}
