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
}

impl ScrollState {
    pub const fn new(content_h: usize, viewport_h: usize, offset: usize) -> Self {
        let max = if content_h > viewport_h { content_h - viewport_h } else { 0 };
        let clamped = if offset > max { max } else { offset };
        Self { content_h, viewport_h, offset: clamped }
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
}
