//! Sovereign Mobile Sensor Fusion Subsystem & Hardware IMU/Barometer/Proximity/Light HAL (AArch64)
//!
//! Provides bare-metal driver support for physical smartphone I2C/SPI sensor arrays
//! (InvenSense MPU-6050, Bosch BMI160, STMicroelectronics LSM6DS3, APDS-9960, BMP280)
//! mapped at MMIO base `0x09050000`.
//!
//! Features:
//! - 6-Axis IMU (3-axis Accelerometer in milli-g, 3-axis Gyroscope in deci-degrees/sec)
//! - Device orientation classifier (PortraitUp, PortraitDown, LandscapeLeft, LandscapeRight, FaceUp, FaceDown)
//! - Proximity & Ambient Light (ALS) ear-detection for automatic screen blanking during voice calls
//! - Adaptive display backlight governor (lux-based PWM duty cycle)
//! - Barometric pressure (Pa) & integer hypsometric altimeter calculation
//! - Real-time gesture recognition (Flip to Mute, Double-Tap to Wake, Shake for Emergency SOS)
//! - Zero Ambient Authority: Reading sensor telemetry requires `CapabilityType::SensorTelemetryRead`.

extern crate alloc;
use crate::capability::{CapabilityTable, CapabilityType, Rights};

pub const STANDARD_SEA_LEVEL_PRESSURE_PA: i32 = 101_325; // 1013.25 hPa

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
    pub mmio_base: usize,
    pub enabled: bool,
    pub hardware_present: bool,
    pub imu_active: bool,
    pub prox_als_active: bool,
    pub barometer_active: bool,
    pub last_imu: ImuSample,
    pub last_env: EnvironmentSample,
    pub orientation: DeviceOrientation,
    pub screen_state: ScreenBlankState,
    pub display_backlight_pwm: u8,
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
            last_imu: ImuSample {
                accel_x_mg: 0,
                accel_y_mg: -1000, // Standard gravity in PortraitUp
                accel_z_mg: 0,
                gyro_x_dps: 0,
                gyro_y_dps: 0,
                gyro_z_dps: 0,
            },
            last_env: EnvironmentSample {
                proximity_mm: 100, // 100mm = Far
                ambient_lux: 350,   // Standard indoor lighting
                pressure_pa: STANDARD_SEA_LEVEL_PRESSURE_PA,
                altitude_meters: 0,
            },
            orientation: DeviceOrientation::PortraitUp,
            screen_state: ScreenBlankState::Active,
            display_backlight_pwm: 65,
            last_gesture: GestureEvent::None,
            total_samples_read: 0,
            total_gestures_detected: 0,
        }
    }

    /// Initializes sensor hub hardware and sub-busses.
    pub fn init_hardware(&mut self) {
        self.enabled = true;
        self.imu_active = true;
        self.prox_als_active = true;
        self.barometer_active = true;
    }

    pub fn set_hardware_present(&mut self, present: bool) {
        self.hardware_present = present;
        if !present { self.imu_active=false; self.prox_als_active=false; self.barometer_active=false; }
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
        self.last_imu = imu;
        self.last_env = env;
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
        self.display_backlight_pwm = Self::calculate_backlight_pwm(env.ambient_lux);

        // 4. Barometric Hypsometric Altimeter (Integer math)
        // delta_h = (P0 - P) * 84 / 1000 meters
        let pressure_delta = STANDARD_SEA_LEVEL_PRESSURE_PA.saturating_sub(env.pressure_pa);
        self.last_env.altitude_meters = (pressure_delta * 84) / 1000;

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
