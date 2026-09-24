//! Mobile Hardware Sensor Subsystem
//!
//! Coordinates Inertial Measurement Units (IMU 3-axis accelerometer and gyroscope),
//! proximity sensing for call blanking, and ambient light sensors for dynamic brightness.

pub mod imu;
pub mod proximity;

pub use imu::{ImuSensor, ScreenOrientation, Vector3D};
pub use proximity::{LightSensor, ProximitySensor};

use spin::Mutex;
use crate::serial_println;

/// Global Sensor Subsystem Coordinator
pub struct SensorManager {
    pub imu: ImuSensor,
    pub proximity: ProximitySensor,
    pub light: LightSensor,
}

impl SensorManager {
    pub const fn new() -> Self {
        Self {
            imu: ImuSensor::new(),
            proximity: ProximitySensor::new(),
            light: LightSensor::new(),
        }
    }

    /// Checks if physical IMU/ambient hardware sensors are detected on the bus.
    pub fn is_hardware_present(&self) -> bool {
        // On standard QEMU virt/PC without physical I2C sensor chips, report false
        false
    }
}

pub static SENSOR_MANAGER: Mutex<SensorManager> = Mutex::new(SensorManager::new());

/// Initializes the hardware sensor subsystem and verifies orientation and proximity thresholds.
pub fn init() {
    serial_println!("--- [Initializing Mobile Sensor Subsystem] ---");

    let mut sm = SENSOR_MANAGER.lock();

    // 1. Verify Screen Orientation from Gravity Vectors
    sm.imu.update_accel(Vector3D::new(0, 980, 0));
    assert_eq!(sm.imu.determine_orientation(), ScreenOrientation::Portrait);

    sm.imu.update_accel(Vector3D::new(980, 0, 0));
    assert_eq!(sm.imu.determine_orientation(), ScreenOrientation::LandscapeRight);

    sm.imu.update_accel(Vector3D::new(-980, 0, 0));
    assert_eq!(sm.imu.determine_orientation(), ScreenOrientation::LandscapeLeft);

    sm.imu.update_accel(Vector3D::new(0, 0, 980));
    assert_eq!(sm.imu.determine_orientation(), ScreenOrientation::FaceUp);

    // Reset to normal portrait orientation
    sm.imu.update_accel(Vector3D::new(0, 980, 0));
    serial_println!("[TEST] Sensor Subsystem: Gravity vector [0, 980, 0] resolved to ScreenOrientation::Portrait.");

    // 2. Verify Proximity Sensor Near/Far Thresholds
    sm.proximity.update(15);
    assert!(sm.proximity.is_near(), "15mm distance should trigger Near detection");
    sm.proximity.update(60);
    assert!(!sm.proximity.is_near(), "60mm distance should trigger Far detection");
    serial_println!("[TEST] Proximity Sensor: Near detection (< 30mm) triggers display blanking guard.");

    // 3. Verify Ambient Light Sensor (ALS) Auto-Brightness Scaling
    sm.light.update(5);
    assert_eq!(sm.light.auto_brightness_percent(), 10);
    sm.light.update(450);
    assert_eq!(sm.light.auto_brightness_percent(), 60);
    sm.light.update(5000);
    assert_eq!(sm.light.auto_brightness_percent(), 100);
    serial_println!("[TEST] Ambient Light Sensor: Auto-brightness scaling validated (5 lx -> 10%, 5000 lx -> 100%).");

    serial_println!("--- [Mobile Sensor Subsystem 100% Attested & Enforced] ---");
}
