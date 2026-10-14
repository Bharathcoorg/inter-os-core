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

    /// Computes scalar dot product with another 3D vector.
    pub fn dot(&self, other: Vector3D) -> i64 {
        (self.x as i64) * (other.x as i64)
            + (self.y as i64) * (other.y as i64)
            + (self.z as i64) * (other.z as i64)
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

    pub fn update_gyro(&mut self, new_gyro: Vector3D) {
        self.gyro = new_gyro;
    }

    pub fn sample(&self) -> (Vector3D, Vector3D) {
        (self.accel, self.gyro)
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

    /// Computes approximate pitch tilt angle in degrees (-90..=90).
    pub fn pitch_degrees(&self) -> i32 {
        (self.accel.y * 90 / 980).clamp(-90, 90)
    }

    /// Computes approximate roll tilt angle in degrees (-90..=90).
    pub fn roll_degrees(&self) -> i32 {
        (self.accel.x * 90 / 980).clamp(-90, 90)
    }

    /// Computes compass heading (0..=359 degrees) from magnetometer X (East) and Y (North) readings.
    pub fn compass_heading(&self, mag_x: i32, mag_y: i32) -> u16 {
        compass_heading_degrees(mag_x, mag_y)
    }
}

/// Computes integer degree arctangent approximation (-180..=180) using fixed-point octant reduction.
pub fn atan2_degrees_approx(y: i32, x: i32) -> i32 {
    if x == 0 && y == 0 {
        return 0;
    }
    if x == 0 {
        return if y > 0 { 90 } else { -90 };
    }
    if y == 0 {
        return if x > 0 { 0 } else { 180 };
    }

    let abs_y = y.abs() as i64;
    let abs_x = x.abs() as i64;

    let base_deg = if abs_y <= abs_x {
        let r_1000 = abs_y * 1000 / abs_x;
        (45 * r_1000 - r_1000 * (1000 - r_1000) * 14 / 1000) / 1000
    } else {
        let r_1000 = abs_x * 1000 / abs_y;
        90 - (45 * r_1000 - r_1000 * (1000 - r_1000) * 14 / 1000) / 1000
    } as i32;

    if x > 0 && y > 0 {
        base_deg
    } else if x < 0 && y > 0 {
        180 - base_deg
    } else if x < 0 && y < 0 {
        -180 + base_deg
    } else {
        -base_deg
    }
}

/// Computes compass heading (0..359 degrees, clockwise from North) from horizontal magnetic flux vectors (X=East, Y=North).
pub fn compass_heading_degrees(mag_x: i32, mag_y: i32) -> u16 {
    let deg = atan2_degrees_approx(mag_x, mag_y);
    let normalized = (deg + 360) % 360;
    normalized as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector3d_operations() {
        let v1 = Vector3D::new(0, 980, 0);
        let v2 = Vector3D::new(0, 980, 0);
        assert_eq!(v1.magnitude_sq(), 980 * 980);
        assert_eq!(v1.dot(v2), 980 * 980);

        let v3 = Vector3D::new(100, 0, 0);
        assert_eq!(v1.dot(v3), 0); // orthogonal
    }

    #[test]
    fn test_imu_tilt_angles_and_shake() {
        let mut imu = ImuSensor::new();
        assert_eq!(imu.pitch_degrees(), 90);
        assert_eq!(imu.roll_degrees(), 0);

        imu.update_accel(Vector3D::new(490, 0, 0));
        assert_eq!(imu.pitch_degrees(), 0);
        assert_eq!(imu.roll_degrees(), 45);

        // Shake detection test
        assert!(!imu.detect_shake());
        imu.update_accel(Vector3D::new(2000, 2000, 0));
        assert!(imu.detect_shake());
    }

    #[test]
    fn test_compass_cardinal_headings() {
        // North: mag_x = 0, mag_y = 100 -> 0 degrees
        assert_eq!(compass_heading_degrees(0, 100), 0);
        // East: mag_x = 100, mag_y = 0 -> 90 degrees
        assert_eq!(compass_heading_degrees(100, 0), 90);
        // South: mag_x = 0, mag_y = -100 -> 180 degrees
        assert_eq!(compass_heading_degrees(0, -100), 180);
        // West: mag_x = -100, mag_y = 0 -> 270 degrees
        assert_eq!(compass_heading_degrees(-100, 0), 270);

        // Northeast: mag_x = 100, mag_y = 100 -> ~45 degrees
        let ne = compass_heading_degrees(100, 100);
        assert!((44..=46).contains(&ne));
    }
}

