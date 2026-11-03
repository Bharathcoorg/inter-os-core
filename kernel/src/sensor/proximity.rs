// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Proximity and Ambient Light Sensors (ALS)
//!
//! Provides distance detection for display blanking during phone calls and
//! ambient illumination measurement for intelligent display brightness scaling.

/// Mobile Proximity Sensor
pub struct ProximitySensor {
    /// Distance in millimeters (0..100 mm)
    pub distance_mm: u16,
}

impl ProximitySensor {
    pub const fn new() -> Self {
        Self { distance_mm: 50 } // Default: 50 mm (Far)
    }

    /// Returns true if an object (e.g. user's ear) is near the screen (< 30 mm).
    pub fn is_near(&self) -> bool {
        self.distance_mm < 30
    }

    /// Updates the measured proximity distance.
    pub fn update(&mut self, distance_mm: u16) {
        self.distance_mm = distance_mm.min(100);
    }

    pub fn reading_mm(&self) -> u16 {
        self.distance_mm
    }
}

/// Ambient Light Sensor (ALS)
pub struct LightSensor {
    /// Illumination level in Lux (0..100,000 lx)
    pub lux: u32,
}

impl LightSensor {
    pub const fn new() -> Self {
        Self { lux: 450 } // Default: standard indoor ambient light (450 lux)
    }

    /// Computes the recommended display brightness percentage (10% to 100%).
    pub fn auto_brightness_percent(&self) -> u8 {
        if self.lux <= 10 {
            10 // Dim dark room
        } else if self.lux <= 100 {
            30 // Low indoor light
        } else if self.lux <= 500 {
            60 // Normal office / room
        } else if self.lux <= 2000 {
            80 // Bright daylight indoors
        } else {
            100 // Direct outdoor sunlight
        }
    }

    /// Updates the ambient lux reading.
    pub fn update(&mut self, lux: u32) {
        self.lux = lux.min(100_000);
    }

    pub fn reading_lux(&self) -> u32 {
        self.lux
    }
}
