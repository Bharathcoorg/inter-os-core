//! Mobile Multi-Touch Contact Tracking & Gesture Recognition Engine
//!
//! Provides a hardware-decoupled multi-touch event processing pipeline:
//! - Tracks up to 5 concurrent touch contacts with unique tracking IDs
//! - Calibrates and transforms raw digitizer coordinates to screen viewport
//! - High-level gesture recognition: Tap, Double-Tap, Long-Press (>=500ms), Pan/Drag, and Two-Finger Pinch
//! - Thread-safe event queue dispatching to microkernel input listeners

use alloc::collections::VecDeque;
use spin::Mutex;

/// Maximum number of concurrent hardware touch contact slots.
pub const MAX_SLOTS: usize = 5;

/// Maximum travel, in raw digitizer units, still classified as a tap.
const TAP_MAX_TRAVEL: u32 = 15;

/// Maximum travel between the two contacts of a double tap.
const TAP_SLOP: u32 = 20;

/// Maximum gap between the two taps of a double tap, in system ticks.
const DOUBLE_TAP_WINDOW_TICKS: u64 = 35;

/// Minimum contact duration, in system ticks, classified as a long press.
const LONG_PRESS_TICKS: u64 = 50;

/// Dead zone, in raw digitizer units, below which a single-contact move is not
/// reported as a pan.
const PAN_DEADZONE: i32 = 4;

/// Maximum number of recognised gestures retained for the consumer.
///
/// WHY bounded: `TOUCH_ENGINE.gesture_queue` is drained only when a user task
/// calls `TouchPoll`. Without a producer-side bound, a stream that is never
/// polled keeps the engine growing the kernel heap until allocation fails, and
/// allocation failure in this kernel is an allocation-error panic rather than a
/// recoverable error. The oldest entries are evicted because a consumer wants
/// the most recent contacts, not the backlog.
pub const MAX_QUEUED_GESTURES: usize = 64;

/// Squared Euclidean distance between two digitizer coordinates.
///
/// WHY unsigned and widened: coordinates arrive from user space as `u32`, so a
/// single squared axis delta reaches `(u32::MAX)^2 < 2^64` and the sum of two
/// can exceed it. Computing this in `i32` (as an `i32 * i32` pair) overflowed
/// for any separation above 46340 units, and the kernel builds with
/// `overflow-checks` enabled, so that was a live panic on ordinary touch input
/// rather than an unreachable arithmetic edge case.
#[inline]
fn squared_distance(ax: u32, ay: u32, bx: u32, by: u32) -> u64 {
    let dx = ax.abs_diff(bx) as u64;
    let dy = ay.abs_diff(by) as u64;
    dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy))
}

/// Absolute difference between two digitizer coordinates.
///
/// WHY unsigned: `(a as i32 - b as i32).abs()` wrapped for any coordinate pair
/// spanning `i32::MIN`, and `i32::abs` of `i32::MIN` is itself an overflow, so
/// the distance of two perfectly valid `u32` coordinates could panic.
#[inline]
fn coord_distance(a: u32, b: u32) -> u64 {
    a.abs_diff(b) as u64
}

/// High-level mobile touch gesture types recognized by the engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecognizedGesture {
    Tap { x: u32, y: u32 },
    DoubleTap { x: u32, y: u32 },
    LongPress { x: u32, y: u32 },
    Pan { start_x: u32, start_y: u32, current_x: u32, current_y: u32, dx: i32, dy: i32 },
    Pinch { center_x: u32, center_y: u32, scale_factor: f32 },
}

/// State of an active touch contact slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContactSlot {
    pub active: bool,
    pub id: u8,
    pub start_x: u32,
    pub start_y: u32,
    pub current_x: u32,
    pub current_y: u32,
    pub start_ticks: u64,
    pub last_ticks: u64,
}

impl ContactSlot {
    pub const fn empty() -> Self {
        Self {
            active: false,
            id: 0,
            start_x: 0,
            start_y: 0,
            current_x: 0,
            current_y: 0,
            start_ticks: 0,
            last_ticks: 0,
        }
    }
}

/// Multi-Touch Gesture Recognition Engine.
pub struct TouchGestureEngine {
    pub slots: [ContactSlot; MAX_SLOTS],
    pub last_tap_time: u64,
    pub last_tap_coords: (u32, u32),
    pub gesture_queue: VecDeque<RecognizedGesture>,
}

impl TouchGestureEngine {
    pub const fn new() -> Self {
        Self {
            slots: [ContactSlot::empty(); MAX_SLOTS],
            last_tap_time: 0,
            last_tap_coords: (0, 0),
            gesture_queue: VecDeque::new(),
        }
    }

    /// Appends a recognised gesture, keeping the queue within
    /// [`MAX_QUEUED_GESTURES`] by evicting the oldest entry.
    fn emit(&mut self, gesture: RecognizedGesture) {
        while self.gesture_queue.len() >= MAX_QUEUED_GESTURES {
            self.gesture_queue.pop_front();
        }
        self.gesture_queue.push_back(gesture);
    }

    /// Handles a touch contact down event on a specific slot.
    pub fn handle_down(&mut self, slot: usize, id: u8, x: u32, y: u32, current_ticks: u64) {
        if slot >= MAX_SLOTS { return; }
        self.slots[slot] = ContactSlot {
            active: true,
            id,
            start_x: x,
            start_y: y,
            current_x: x,
            current_y: y,
            start_ticks: current_ticks,
            last_ticks: current_ticks,
        };
    }

    /// Handles touch movement on an active slot.
    pub fn handle_move(&mut self, slot: usize, x: u32, y: u32, current_ticks: u64) {
        if slot >= MAX_SLOTS || !self.slots[slot].active { return; }

        let s = &mut self.slots[slot];
        // WHY wrapping/unsigned_abs: both coordinates are `u32`, so `x as i32` can
        // be negative and the subtraction can leave the `i32` range. The previous
        // `x as i32 - current as i32` and `dx.abs()` forms overflowed (and
        // therefore panicked, because the kernel builds with `overflow-checks`)
        // for perfectly valid `u32` coordinates.
        let dx = (x as i32).wrapping_sub(s.current_x as i32);
        let dy = (y as i32).wrapping_sub(s.current_y as i32);
        s.current_x = x;
        s.current_y = y;
        s.last_ticks = current_ticks;

        // If two fingers are active, compute pinch gesture
        //
        // WHY the manual slot scan: the previous form allocated a
        // `Vec<&ContactSlot>` on every two-finger move and then indexed it with
        // `active_slots[0]` and `active_slots[1]`. Those index expressions were
        // only safe because the collected length happened to agree with the
        // count taken from a separate pass over the same array; anything that
        // decoupled the two became an out-of-bounds index inside the kernel.
        // This walk cannot index out of bounds and allocates nothing on the
        // touch path.
        let mut first_active: Option<usize> = None;
        let mut second_active: Option<usize> = None;
        for idx in 0..self.slots.len() {
            if !self.slots[idx].active { continue; }
            if first_active.is_none() {
                first_active = Some(idx);
            } else {
                second_active = Some(idx);
                break;
            }
        }

        if let (Some(first), Some(second)) = (first_active, second_active) {
            let p1 = &self.slots[first];
            let p2 = &self.slots[second];

            let cx = p1.current_x / 2 + p2.current_x / 2 + (p1.current_x & p2.current_x & 1);
            let cy = p1.current_y / 2 + p2.current_y / 2 + (p1.current_y & p2.current_y & 1);

            let d0_sq = squared_distance(p1.start_x, p1.start_y, p2.start_x, p2.start_y);
            let d1_sq = squared_distance(p1.current_x, p1.current_y, p2.current_x, p2.current_y);

            if d0_sq > 25 && d1_sq > 25 {
                let d0 = isqrt(d0_sq);
                let d1 = isqrt(d1_sq);
                if d0 > 0 {
                    let scale = (d1 as f32) / (d0 as f32);
                    self.emit(RecognizedGesture::Pinch {
                        center_x: cx,
                        center_y: cy,
                        scale_factor: scale,
                    });
                    return;
                }
            }
        }

        // Single contact drag/pan
        if dx.unsigned_abs() > PAN_DEADZONE as u32 || dy.unsigned_abs() > PAN_DEADZONE as u32 {
            let start_x = self.slots[slot].start_x;
            let start_y = self.slots[slot].start_y;
            self.emit(RecognizedGesture::Pan {
                start_x,
                start_y,
                current_x: x,
                current_y: y,
                dx,
                dy,
            });
        }
    }

    /// Handles a touch release (up) event.
    pub fn handle_up(&mut self, slot: usize, current_ticks: u64) {
        if slot >= MAX_SLOTS || !self.slots[slot].active { return; }

        let s = self.slots[slot];
        self.slots[slot].active = false;

        let duration = current_ticks.saturating_sub(s.start_ticks);
        let dist_x = coord_distance(s.current_x, s.start_x);
        let dist_y = coord_distance(s.current_y, s.start_y);

        if dist_x <= TAP_MAX_TRAVEL as u64 && dist_y <= TAP_MAX_TRAVEL as u64 {
            if duration >= LONG_PRESS_TICKS { // >= 500 ms at 100 Hz
                self.emit(RecognizedGesture::LongPress {
                    x: s.current_x,
                    y: s.current_y,
                });
            } else {
                // Check for double-tap
                let time_since_last = current_ticks.saturating_sub(self.last_tap_time);
                let tap_dist_x = coord_distance(s.current_x, self.last_tap_coords.0);
                let tap_dist_y = coord_distance(s.current_y, self.last_tap_coords.1);

                if time_since_last <= DOUBLE_TAP_WINDOW_TICKS
                    && tap_dist_x <= TAP_SLOP as u64
                    && tap_dist_y <= TAP_SLOP as u64
                {
                    self.emit(RecognizedGesture::DoubleTap {
                        x: s.current_x,
                        y: s.current_y,
                    });
                    self.last_tap_time = 0;
                } else {
                    self.emit(RecognizedGesture::Tap {
                        x: s.current_x,
                        y: s.current_y,
                    });
                    self.last_tap_time = current_ticks;
                    self.last_tap_coords = (s.current_x, s.current_y);
                }
            }
        } else {
            self.emit(RecognizedGesture::Pan {
                start_x: s.start_x,
                start_y: s.start_y,
                current_x: s.current_x,
                current_y: s.current_y,
                dx: s.current_x.wrapping_sub(s.start_x) as i32,
                dy: s.current_y.wrapping_sub(s.start_y) as i32,
            });
        }
    }
}

/// Integer square root, returning `floor(sqrt(n))`.
///
/// WHY this algorithm: the previous Newton's-method form computed `x + 1` and
/// `x + n / x` in `u32` arithmetic. Both expressions overflow for large inputs
/// (`n == u32::MAX` overflows the very first statement), and the kernel builds
/// with `overflow-checks` enabled, so `isqrt` could panic on its own inputs.
/// This form only ever adds two values bounded by `2^31 + 2^62`, so it is
/// total for every `u64`.
fn isqrt(n: u64) -> u64 {
    let mut result: u64 = 0;
    let mut bit: u64 = 1 << 62;
    let mut rest = n;
    while bit > rest {
        bit >>= 2;
    }
    while bit != 0 {
        // `result` never exceeds `2^31` and `bit` never exceeds `2^62`, so this
        // sum is bounded well below `u64::MAX` and cannot overflow.
        let candidate = result + bit;
        if rest >= candidate {
            rest -= candidate;
            result = (result >> 1) + bit;
        } else {
            result >>= 1;
        }
        bit >>= 2;
    }
    result
}

/// Global static instance of the Touch Gesture Engine.
pub static TOUCH_ENGINE: Mutex<TouchGestureEngine> = Mutex::new(TouchGestureEngine::new());

/// Automated verification self-test for touch contact tracking and gestures.
pub fn test_touch_gestures() {
    let mut engine = TouchGestureEngine::new();

    // 1. Test Tap
    engine.handle_down(0, 1, 100, 200, 10);
    engine.handle_up(0, 15);
    assert_eq!(engine.gesture_queue.pop_front(), Some(RecognizedGesture::Tap { x: 100, y: 200 }));

    // 2. Test Double-Tap
    engine.handle_down(0, 1, 102, 201, 30);
    engine.handle_up(0, 35);
    assert_eq!(engine.gesture_queue.pop_front(), Some(RecognizedGesture::DoubleTap { x: 102, y: 201 }));

    // 3. Test Long-Press
    engine.handle_down(0, 1, 150, 300, 50);
    engine.handle_up(0, 110); // duration 60 ticks >= 50
    assert_eq!(engine.gesture_queue.pop_front(), Some(RecognizedGesture::LongPress { x: 150, y: 300 }));

    // 4. Test Pan
    engine.handle_down(0, 1, 200, 400, 150);
    engine.handle_move(0, 250, 410, 155);
    assert!(matches!(engine.gesture_queue.pop_front(), Some(RecognizedGesture::Pan { dx: 50, .. })));

    // 5. Test Swipe Up
    engine.handle_down(0, 1, 360, 1420, 200);
    engine.handle_move(0, 360, 1100, 205);
    engine.handle_up(0, 210);
    assert!(matches!(engine.gesture_queue.pop_front(), Some(RecognizedGesture::Pan { dy, .. }) if dy < 0));

    crate::serial_println!("[TEST] Touch Gestures: Tap, DoubleTap, LongPress, and Pan recognition verified.");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two contacts 100_000 units apart, then moved to 200_000 apart.
    ///
    /// The assertion that pins the pinch regression is the `scale_factor`:
    /// the recogniser must measure 200_000 over 100_000 and report exactly
    /// 2.0. Before the fix this input panicked with "attempt to multiply with
    /// overflow", because the squared separation was computed as
    /// `(dy0 * dy0) as u32` in `i32` arithmetic and `100_000 * 100_000` does not
    /// fit in an `i32`. Nothing about the coordinate pair is degenerate: any two
    /// contacts more than 46340 units apart on one axis reached it.
    #[test]
    fn pinch_is_recognised_for_separations_beyond_the_i32_square_range() {
        let mut engine = TouchGestureEngine::new();
        engine.handle_down(0, 0, 0, 0, 10);
        engine.handle_down(1, 1, 0, 100_000, 10);
        engine.handle_move(1, 0, 200_000, 15);

        match engine.gesture_queue.pop_front() {
            Some(RecognizedGesture::Pinch { center_x, center_y, scale_factor }) => {
                assert_eq!(center_x, 0);
                assert_eq!(center_y, 100_000);
                assert_eq!(scale_factor, 2.0);
            }
            other => panic!("expected a pinch, got {:?}", other),
        }
        assert!(engine.gesture_queue.is_empty());
    }

    /// The release path must measure travel exactly across the whole `u32`
    /// coordinate range.
    ///
    /// This is the assertion that pins the travel-distance regression: the old
    /// `(|current| - |start|) as i32).abs()` form is `i32::MIN.abs()` for
    /// `current = 0x8000_0000`, which overflows and panics. The gesture that is
    /// reported must stay a `Pan` carrying the wrapped `i32` delta the public
    /// `RecognizedGesture::Pan` type has always exposed.
    #[test]
    fn release_travel_is_exact_across_the_full_u32_coordinate_range() {
        let mut engine = TouchGestureEngine::new();
        engine.handle_down(0, 0, 0, 0, 5);
        engine.handle_move(0, 0x8000_0000, 0, 10);
        engine.handle_up(0, 20);

        match engine.gesture_queue.pop_front() {
            Some(RecognizedGesture::Pan { start_x, start_y, current_x, current_y, dx, dy }) => {
                assert_eq!((start_x, start_y), (0, 0));
                assert_eq!((current_x, current_y), (0x8000_0000, 0));
                assert_eq!(dx, i32::MIN);
                assert_eq!(dy, 0);
            }
            other => panic!("expected a pan, got {:?}", other),
        }
    }

    /// The pinch branch must select the two active contacts without assuming
    /// they are adjacent slots, and without an indexed collection.
    ///
    /// The old code built `Vec<&ContactSlot>` from a filtered scan and then
    /// indexed `active_slots[0]` / `active_slots[1]`; correctness depended on
    /// the collected length agreeing with the separately computed count. This
    /// pins the slot selection for the worst case that shape mishandles: two
    /// contacts in the first and last slot.
    #[test]
    fn pinch_selection_does_not_assume_adjacent_active_slots() {
        let mut engine = TouchGestureEngine::new();
        engine.handle_down(0, 0, 100, 100, 10);
        engine.handle_down(MAX_SLOTS - 1, 4, 100, 300, 10);
        engine.handle_move(MAX_SLOTS - 1, 100, 500, 15);

        match engine.gesture_queue.pop_front() {
            Some(RecognizedGesture::Pinch { center_x, center_y, scale_factor }) => {
                assert_eq!(center_x, 100);
                assert_eq!(center_y, 300);
                assert_eq!(scale_factor, 2.0);
            }
            other => panic!("expected a pinch, got {:?}", other),
        }
    }

    /// The producer-side bound must hold and must evict the oldest entries.
    ///
    /// `TOUCH_ENGINE` is drained only when a user task calls `TouchPoll`, so an
    /// unbounded queue turns an unpolled touch stream into unbounded kernel heap
    /// growth. The bound must be enforced in `emit`, not at the consumer.
    #[test]
    fn gesture_queue_never_exceeds_the_documented_bound() {
        let mut engine = TouchGestureEngine::new();
        let total = MAX_QUEUED_GESTURES + 8;
        for i in 0..total {
            engine.emit(RecognizedGesture::Tap { x: i as u32, y: 0 });
        }

        assert_eq!(engine.gesture_queue.len(), MAX_QUEUED_GESTURES);
        assert_eq!(engine.gesture_queue.front(), Some(&RecognizedGesture::Tap { x: 8, y: 0 }));
        assert_eq!(engine.gesture_queue.back(), Some(&RecognizedGesture::Tap { x: (total - 1) as u32, y: 0 }));
    }

    /// `isqrt` must be correct for every input and must not overflow on any of
    /// them. The old Newton form began with `x + 1` on `x == n`, so `isqrt`
    /// panicked for `n == u32::MAX` before it ever looked at the caller's data.
    #[test]
    fn isqrt_is_total_and_correct() {
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(1), 1);
        assert_eq!(isqrt(3), 1);
        assert_eq!(isqrt(4), 2);
        assert_eq!(isqrt(10_000_000_000), 100_000);
        assert_eq!(isqrt(u32::MAX as u64), 65_535);
        assert_eq!(isqrt(u64::MAX), 4_294_967_295);

        for n in 0..4096u64 {
            let root = isqrt(n);
            assert!(
                root * root <= n && n < (root + 1) * (root + 1),
                "isqrt({}) returned {}",
                n,
                root
            );
        }
    }

    /// Squared distance must not wrap for any `u32` coordinate pair.
    #[test]
    fn squared_distance_does_not_wrap_for_any_u32_pair() {
        assert_eq!(squared_distance(0, 0, 0, 0), 0);
        assert_eq!(squared_distance(0, 0, 3, 4), 25);
        assert_eq!(squared_distance(0, 0, 100_000, 100_000), 20_000_000_000);
        assert_eq!(squared_distance(u32::MAX, 0, 0, 0), u32::MAX as u64 * u32::MAX as u64);
        assert_eq!(squared_distance(0, 0, u32::MAX, u32::MAX), u64::MAX);

        assert_eq!(coord_distance(0, u32::MAX), u32::MAX as u64);
        assert_eq!(coord_distance(u32::MAX, 0), u32::MAX as u64);
    }
}
