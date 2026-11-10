// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Shared scrolling model, boundary clamping, and thumb geometry.
//!
//! Provides an allocation-free scroll state manager used across browser reader,
//! notes editor, messages conversation view, and file manager.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollState {
    pub content_h: usize,
    pub viewport_h: usize,
    pub offset: usize,
    pub velocity_y: i32,
    pub subpixel_y: i32,
}

impl ScrollState {
    pub const fn new(content_h: usize, viewport_h: usize, offset: usize) -> Self {
        let max = if content_h > viewport_h { content_h - viewport_h } else { 0 };
        let clamped = if offset > max { max } else { offset };
        Self {
            content_h,
            viewport_h,
            offset: clamped,
            velocity_y: 0,
            subpixel_y: 0,
        }
    }

    #[inline]
    pub fn max_offset(&self) -> usize {
        self.content_h.saturating_sub(self.viewport_h)
    }

    #[inline]
    pub fn clamp(&mut self) {
        self.offset = self.offset.min(self.max_offset());
    }

    #[inline]
    pub fn apply_drag(&mut self, dy: i32) {
        if dy < 0 {
            self.offset = self.offset.saturating_add((-dy) as usize);
        } else {
            self.offset = self.offset.saturating_sub(dy as usize);
        }
        self.clamp();
    }

    #[inline]
    pub fn is_flinging(&self) -> bool {
        self.velocity_y != 0
    }

    #[inline]
    pub fn start_fling(&mut self, dy: i32) {
        self.velocity_y = dy.saturating_mul(1000);
        self.subpixel_y = 0;
    }

    #[inline]
    pub fn stop_fling(&mut self) {
        self.velocity_y = 0;
        self.subpixel_y = 0;
    }

    /// Advances inertial kinetic scrolling by one 240Hz tick (4.167ms interval).
    /// Decays velocity with subpixel friction and steps the scroll offset.
    /// Returns true if scroll offset changed.
    pub fn step_fling(&mut self) -> bool {
        if self.velocity_y == 0 {
            return false;
        }
        // Decays velocity with subpixel friction at 240Hz (4.167ms tick intervals).
        // 980 / 1000 retains ~92% per 16ms frame (0.98^4 ~= 0.922).
        self.velocity_y = (self.velocity_y * 980) / 1000;
        if self.velocity_y.abs() < 20 {
            self.velocity_y = 0;
            self.subpixel_y = 0;
            return false;
        }
        self.subpixel_y += self.velocity_y;
        let step_px = self.subpixel_y / 1000;
        self.subpixel_y %= 1000;
        if step_px != 0 {
            let prev_offset = self.offset;
            self.apply_drag(step_px);
            if self.offset == prev_offset {
                // Reached boundary, halt fling
                self.velocity_y = 0;
                self.subpixel_y = 0;
                return false;
            }
            true
        } else {
            false
        }
    }

    /// Computes scrollbar thumb geometry `(thumb_y, thumb_h)` along a track `[track_y, track_y + track_h)`.
    pub fn thumb_rect(&self, track_y: usize, track_h: usize, min_thumb_h: usize) -> Option<(usize, usize)> {
        if self.content_h <= self.viewport_h || self.content_h == 0 || track_h == 0 {
            return None;
        }
        let thumb_h = ((self.viewport_h * track_h) / self.content_h).max(min_thumb_h).min(track_h);
        let avail_track = track_h.saturating_sub(thumb_h);
        let max_off = self.max_offset();
        let thumb_y_rel = if max_off > 0 {
            (self.offset * avail_track) / max_off
        } else {
            0
        };
        Some((track_y + thumb_y_rel, thumb_h))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scroll_clamping_and_drag() {
        let mut scroll = ScrollState::new(1000, 400, 0);
        assert_eq!(scroll.max_offset(), 600);

        scroll.apply_drag(-200);
        assert_eq!(scroll.offset, 200);

        scroll.apply_drag(-1000);
        assert_eq!(scroll.offset, 600); // clamped to max_offset

        scroll.apply_drag(100);
        assert_eq!(scroll.offset, 500);

        scroll.apply_drag(1000);
        assert_eq!(scroll.offset, 0); // clamped to 0
    }

    #[test]
    fn test_thumb_geometry() {
        let scroll = ScrollState::new(1000, 500, 250);
        let track_y = 100;
        let track_h = 400;
        let (ty, th) = scroll.thumb_rect(track_y, track_h, 30).expect("thumb");
        assert_eq!(th, 200); // 500/1000 * 400 = 200
        assert_eq!(ty, 100 + 100); // offset halfway (250 / 500) -> 100px down
    }

    #[test]
    fn test_thumb_none_when_content_fits() {
        let scroll = ScrollState::new(300, 500, 0);
        assert_eq!(scroll.thumb_rect(0, 400, 20), None);
    }

    #[test]
    fn test_inertial_fling_240hz_decay() {
        let mut scroll = ScrollState::new(2000, 500, 0);
        assert!(!scroll.is_flinging());

        // Fling downwards (negative dy increases offset)
        scroll.start_fling(-30);
        assert!(scroll.is_flinging());

        let mut stepped = 0;
        while scroll.is_flinging() {
            if scroll.step_fling() {
                stepped += 1;
            }
        }
        assert!(stepped > 0, "Fling must step offset across ticks");
        assert!(scroll.offset > 0, "Fling must advance scroll offset");
        assert!(!scroll.is_flinging(), "Fling must eventually halt due to friction");
    }
}
