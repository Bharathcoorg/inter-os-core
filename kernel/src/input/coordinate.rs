//! Canonical input coordinate spaces and deterministic transforms.
//!
//! Every input producer must declare its coordinate space before events enter
//! the shared input pipeline. UI hit testing consumes logical portrait
//! coordinates; transport-specific scaling stays at the boundary.

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CoordinateSpace {
    DevicePhysical,
    StudioDisplay,
    GuestFramebuffer,
    LogicalPortrait,
    LogicalLandscape,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CoordinateTransform {
    pub source: CoordinateSpace,
    pub target: CoordinateSpace,
    pub source_width: u32,
    pub source_height: u32,
    pub target_width: u32,
    pub target_height: u32,
}

impl CoordinateTransform {
    pub const fn new(
        source: CoordinateSpace,
        target: CoordinateSpace,
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
    ) -> Self {
        Self { source, target, source_width, source_height, target_width, target_height }
    }

    pub fn map(&self, point: Point) -> Option<Point> {
        if self.source_width == 0
            || self.source_height == 0
            || point.x < 0
            || point.y < 0
            || point.x as u32 >= self.source_width
            || point.y as u32 >= self.source_height
        {
            return None;
        }

        let x = (point.x as u64 * self.target_width as u64 / self.source_width as u64) as i32;
        let y = (point.y as u64 * self.target_height as u64 / self.source_height as u64) as i32;
        Some(Point {
            x: x.min(self.target_width.saturating_sub(1) as i32),
            y: y.min(self.target_height.saturating_sub(1) as i32),
        })
    }
}

pub const fn studio_to_guest() -> CoordinateTransform {
    CoordinateTransform::new(
        CoordinateSpace::StudioDisplay,
        CoordinateSpace::GuestFramebuffer,
        720,
        1440,
        360,
        720,
    )
}

pub const fn guest_to_logical_portrait() -> CoordinateTransform {
    CoordinateTransform::new(
        CoordinateSpace::GuestFramebuffer,
        CoordinateSpace::LogicalPortrait,
        360,
        720,
        720,
        1440,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_corners_without_heuristics() {
        let t = studio_to_guest();
        assert_eq!(t.map(Point { x: 0, y: 0 }), Some(Point { x: 0, y: 0 }));
        assert_eq!(t.map(Point { x: 719, y: 1439 }), Some(Point { x: 359, y: 719 }));
        assert_eq!(t.map(Point { x: 720, y: 0 }), None);
    }

    #[test]
    fn maps_guest_to_logical_portrait() {
        let t = guest_to_logical_portrait();
        assert_eq!(t.map(Point { x: 0, y: 0 }), Some(Point { x: 0, y: 0 }));
        assert_eq!(t.map(Point { x: 359, y: 719 }), Some(Point { x: 718, y: 1438 }));
    }
}
