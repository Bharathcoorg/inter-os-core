//! Inertial Measurement Unit (IMU) & Screen Orientation Engine
//!
//! Provides 3-axis accelerometer and gyroscope sampling, screen orientation
//! determination, tilt angle calculations, and shake gesture recognition.

/// 3-Dimensional Vector (units: milli-g for acceleration, milli-deg/s for gyro)
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Vector3D {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Vector3D {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Computes approximate magnitude squared: x^2 + y^2 + z^2
    pub fn magnitude_sq(&self) -> u64 {
        ((self.x as i64) * (self.x as i64)
            + (self.y as i64) * (self.y as i64)
            + (self.z as i64) * (self.z as i64)) as u64
    }
}

/// Mobile Screen Orientation
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScreenOrientation {
    Portrait,
    LandscapeLeft,
    LandscapeRight,
    PortraitInverted,
    FaceUp,
    FaceDown,
}

/// Hardware IMU Accelerometer and Gyroscope Sensor
pub struct ImuSensor {
    pub accel: Vector3D,
    pub gyro: Vector3D,
    last_accel: Vector3D,
}

impl ImuSensor {
    pub const fn new() -> Self {
        Self {
            // Default nominal gravity pointing down in portrait orientation (1g ≈ 980 milli-g on Y axis)
            accel: Vector3D::new(0, 980, 0),
            gyro: Vector3D::new(0, 0, 0),
            last_accel: Vector3D::new(0, 980, 0),
        }
    }

    /// Updates current accelerometer vector and evaluates motion.
    pub fn update_accel(&mut self, new_accel: Vector3D) {
        self.last_accel = self.accel;
        self.accel = new_accel;
    }

    /// Determines the phone's physical screen orientation from the gravity vector.
    pub fn determine_orientation(&self) -> ScreenOrientation {
        let abs_x = self.accel.x.abs();
        let abs_y = self.accel.y.abs();
        let abs_z = self.accel.z.abs();

        // Check if phone is resting flat on a surface (Z axis dominates)
        if abs_z > abs_x && abs_z > abs_y {
            if self.accel.z > 0 {
                return ScreenOrientation::FaceUp;
            } else {
                return ScreenOrientation::FaceDown;
            }
        }

        // Check if Y axis dominates (Portrait or Inverted Portrait)
        if abs_y >= abs_x {
            if self.accel.y >= 0 {
                ScreenOrientation::Portrait
            } else {
                ScreenOrientation::PortraitInverted
            }
        } else {
            // X axis dominates (Landscape Left or Right)
            if self.accel.x > 0 {
                ScreenOrientation::LandscapeRight
            } else {
                ScreenOrientation::LandscapeLeft
            }
        }
    }

    /// Detects a sudden shake gesture (delta acceleration jerk exceeding 1500 milli-g).
    pub fn detect_shake(&self) -> bool {
        let dx = (self.accel.x - self.last_accel.x).abs();
        let dy = (self.accel.y - self.last_accel.y).abs();
        let dz = (self.accel.z - self.last_accel.z).abs();
        dx + dy + dz > 1500
    }
}
