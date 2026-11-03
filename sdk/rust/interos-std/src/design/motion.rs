// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! InterOS Core Motion Engine
//!
//! Provides deterministic, integer-approximated motion physics, cubic bezier
//! easing curves, spring dynamics, and an animation clock for `#![no_std]` userspace
//! applications with zero heap allocations.
//!
//! All progress values are normalized in permille (`0..=1000`), ensuring 0.1%
//! precision without floating-point emulation overhead.

use crate::design::display_list::Rect;

// ============================================================================
// Canonical Motion Durations
// ============================================================================

/// Instantaneous transition (0 ms). Used when motion is disabled or for layout snaps.
pub const INSTANT: u32 = 0;

/// Fast animation duration (120 ms). Micro-interactions, touch feedback, toggles, pill glide.
pub const FAST: u32 = 120;

/// Standard animation duration (220 ms). Layout shifts, page reorders, segmented tabs.
pub const NORMAL: u32 = 220;

/// Major UI transitions (360 ms). Sheet slide-ups, modal presentation, activity expands.
pub const SLOW: u32 = 360;

/// Gentle / large scale animation (500 ms). Full-screen modals, graceful background fades.
pub const GENTLE: u32 = 500;

// Semantic aliases matching InterOS legacy and system tokens
pub const STANDARD: u32 = NORMAL;
pub const EMPHASIZED: u32 = SLOW;

pub const PRESS_FEEDBACK_MS: u32 = 100;
pub const QUICK_CONTROLS_MS: u32 = 150;
pub const STANDARD_TRANSITION_MS: u32 = 200;
pub const MAJOR_TRANSITION_MS: u32 = 300;
pub const ACTIVITY_EXPANSION_MS: u32 = 280;

/// Strongly-typed duration token referencing the five load-bearing durations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionDuration {
    Instant,
    Fast,
    Normal,
    Slow,
    Gentle,
}

impl MotionDuration {
    #[inline]
    pub const fn ms(&self) -> u32 {
        match self {
            Self::Instant => INSTANT,
            Self::Fast => FAST,
            Self::Normal => NORMAL,
            Self::Slow => SLOW,
            Self::Gentle => GENTLE,
        }
    }
}

/// High-level interaction role mapped to canonical durations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionRole {
    TapFeedback,
    SelectionPill,
    LayoutShift,
    DrawerSlide,
    ModalScrim,
    FullScreenTransition,
}

impl MotionRole {
    pub const fn duration(&self) -> MotionDuration {
        match self {
            Self::TapFeedback => MotionDuration::Fast,
            Self::SelectionPill => MotionDuration::Fast,
            Self::LayoutShift => MotionDuration::Normal,
            Self::DrawerSlide => MotionDuration::Slow,
            Self::ModalScrim => MotionDuration::Slow,
            Self::FullScreenTransition => MotionDuration::Gentle,
        }
    }

    pub const fn ms(&self) -> u32 {
        self.duration().ms()
    }
}

// ============================================================================
// Monotonic Animation Clock
// ============================================================================

/// Monotonic animation timeline clock converting ticks or timestamps (ms) into
/// normalized progress `0..=1000` (permille) with zero heap allocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MotionClock {
    /// Starting timestamp in milliseconds.
    start_ms: u64,
    /// Duration of the animation timeline in milliseconds.
    duration_ms: u32,
    /// Monotonic current timestamp in milliseconds.
    current_ms: u64,
}

impl MotionClock {
    /// Create a new motion clock starting at `start_ms` with `duration_ms`.
    pub const fn new(start_ms: u64, duration_ms: u32) -> Self {
        Self {
            start_ms,
            duration_ms,
            current_ms: start_ms,
        }
    }

    /// Create a motion clock from a canonical duration token.
    pub const fn with_duration(duration: MotionDuration, start_ms: u64) -> Self {
        Self::new(start_ms, duration.ms())
    }

    /// Pure integer computation of normalized progress in permille (`0..=1000`).
    pub const fn compute_progress(start_ms: u64, duration_ms: u32, now_ms: u64) -> u32 {
        if duration_ms == 0 {
            return 1000;
        }
        if now_ms <= start_ms {
            return 0;
        }
        let elapsed = now_ms - start_ms;
        let dur = duration_ms as u64;
        if elapsed >= dur {
            return 1000;
        }
        ((elapsed * 1000) / dur) as u32
    }

    /// Update the current timestamp monotonically and return the updated progress.
    pub fn tick(&mut self, now_ms: u64) -> u32 {
        if now_ms > self.current_ms {
            self.current_ms = now_ms;
        }
        self.progress()
    }

    /// Advance the clock forward by `delta_ms` and return the updated progress.
    pub fn step(&mut self, delta_ms: u32) -> u32 {
        self.current_ms = self.current_ms.saturating_add(delta_ms as u64);
        self.progress()
    }

    /// Current progress in permille (`0..=1000`).
    #[inline]
    pub fn progress(&self) -> u32 {
        Self::compute_progress(self.start_ms, self.duration_ms, self.current_ms)
    }

    /// Query progress at a specific timestamp without mutating clock state.
    #[inline]
    pub fn progress_at(&self, now_ms: u64) -> u32 {
        Self::compute_progress(self.start_ms, self.duration_ms, now_ms)
    }

    /// Returns `true` if the animation has completed.
    #[inline]
    pub fn is_finished(&self) -> bool {
        self.progress() >= 1000
    }

    /// Returns `true` if the animation has completed at `now_ms`.
    #[inline]
    pub fn is_finished_at(&self, now_ms: u64) -> bool {
        self.progress_at(now_ms) >= 1000
    }

    /// Milliseconds elapsed since `start_ms`.
    pub fn elapsed_ms(&self) -> u64 {
        if self.current_ms > self.start_ms {
            self.current_ms - self.start_ms
        } else {
            0
        }
    }

    /// Remaining milliseconds until duration completes.
    pub fn remaining_ms(&self) -> u32 {
        let elapsed = self.elapsed_ms();
        let dur = self.duration_ms as u64;
        if elapsed >= dur {
            0
        } else {
            (dur - elapsed) as u32
        }
    }

    /// Reset start and current timestamp to `now_ms` with existing duration.
    pub fn reset(&mut self, now_ms: u64) {
        self.start_ms = now_ms;
        self.current_ms = now_ms;
    }

    /// Restart the clock with a new duration.
    pub fn restart(&mut self, now_ms: u64, duration_ms: u32) {
        self.start_ms = now_ms;
        self.duration_ms = duration_ms;
        self.current_ms = now_ms;
    }

    pub const fn start_ms(&self) -> u64 {
        self.start_ms
    }

    pub const fn duration_ms(&self) -> u32 {
        self.duration_ms
    }

    pub const fn current_ms(&self) -> u64 {
        self.current_ms
    }
}

// ============================================================================
// Easing Curves & Cubic Bezier
// ============================================================================

/// Integer cubic ease-in curve.
///
/// Starts slow and accelerates.
/// At t=0 -> 0; at t=500 -> 125; at t=1000 -> 1000.
pub fn ease_in(t: u32) -> u32 {
    if t >= 1000 {
        return 1000;
    }
    let t64 = t as u64;
    ((t64 * t64 * t64) / 1_000_000) as u32
}

/// Integer cubic ease-out curve.
///
/// Starts fast and decelerates into destination.
/// At t=0 -> 0; at t=500 -> 875; at t=1000 -> 1000.
pub fn ease_out(t: u32) -> u32 {
    if t >= 1000 {
        return 1000;
    }
    let rem = 1000 - (t as u64);
    (1000 - (rem * rem * rem) / 1_000_000) as u32
}

/// Integer symmetric cubic ease-in-out curve.
///
/// Accelerates until midpoint and decelerates to endpoint.
/// At t=0 -> 0; at t=500 -> 500; at t=1000 -> 1000.
pub fn ease_in_out(t: u32) -> u32 {
    if t >= 1000 {
        return 1000;
    }
    if t < 500 {
        let t64 = t as u64;
        ((4 * t64 * t64 * t64) / 1_000_000) as u32
    } else {
        let rem = 1000 - (t as u64);
        (1000 - (4 * rem * rem * rem) / 1_000_000) as u32
    }
}

/// Canonical spring overshoot curve.
///
/// Yields an organic bounce that slightly exceeds target (~1099 permille at t=600)
/// before settling down precisely to 1000 at t=1000.
pub fn spring_overshoot(t: u32) -> i32 {
    if t == 0 {
        return 0;
    }
    if t >= 1000 {
        return 1000;
    }
    let tau = 1000i64 - (t as i64);
    let s = 1702i64; // Standard 1.70158 overshoot factor
    let a = s - ((s + 1000) * tau) / 1000;
    let term = (tau * tau * a) / 1_000_000;
    (1000 + term) as i32
}

/// Clamped spring overshoot in permille range `0..=1000`.
pub fn spring_overshoot_clamped(t: u32) -> u32 {
    spring_overshoot(t).clamp(0, 1000) as u32
}

/// Evaluate cubic bezier polynomial coordinate for parameter `s` in permille `0..=1000`.
pub fn eval_bezier_coord(s: u32, p1: i32, p2: i32) -> i32 {
    let s_i64 = s as i64;
    let om = 1000i64 - s_i64;
    let term1 = (3 * om * om * s_i64 * (p1 as i64)) / 1_000_000_000i64;
    let term2 = (3 * om * s_i64 * s_i64 * (p2 as i64)) / 1_000_000_000i64;
    let term3 = (s_i64 * s_i64 * s_i64 * 1000i64) / 1_000_000_000i64;
    (term1 + term2 + term3) as i32
}

/// Solve for parametric `s` given time `t` using integer bisection.
pub fn solve_cubic_bezier_s(t: u32, p1_x: u32, p2_x: u32) -> u32 {
    if t == 0 {
        return 0;
    }
    if t >= 1000 {
        return 1000;
    }
    let mut low = 0u32;
    let mut high = 1000u32;
    for _ in 0..12 {
        let mid = (low + high) / 2;
        let x = eval_bezier_coord(mid, p1_x as i32, p2_x as i32);
        if (x as u32) < t {
            low = mid;
        } else {
            high = mid;
        }
    }
    (low + high) / 2
}

/// Arbitrary cubic bezier curve evaluated using pure integer mathematics.
pub fn cubic_bezier(t: u32, p1_x: u32, p1_y: i32, p2_x: u32, p2_y: i32) -> i32 {
    if t == 0 {
        return 0;
    }
    if t >= 1000 {
        return 1000;
    }
    let s = solve_cubic_bezier_s(t, p1_x, p2_x);
    eval_bezier_coord(s, p1_y, p2_y)
}

// ============================================================================
// Spring Dynamics
// ============================================================================

/// Integer square root via Babylonian method.
pub const fn isqrt(val: u64) -> u64 {
    if val <= 1 {
        return val;
    }
    let mut x0 = val / 2;
    let mut x1 = (x0 + val / x0) / 2;
    while x1 < x0 {
        x0 = x1;
        x1 = (x0 + val / x0) / 2;
    }
    x0
}

/// Spring physical configuration parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpringConfig {
    /// Spring stiffness constant (typically 50..500).
    pub stiffness: u32,
    /// Damping ratio in permille (1000 = 1.0 critical damping, <1000 underdamped, >1000 overdamped).
    pub damping_ratio: u32,
    /// Mass in mass units (typically 1..10).
    pub mass: u32,
}

impl SpringConfig {
    pub const fn new(stiffness: u32, damping_ratio: u32, mass: u32) -> Self {
        Self {
            stiffness: if stiffness == 0 { 1 } else { stiffness },
            damping_ratio,
            mass: if mass == 0 { 1 } else { mass },
        }
    }

    /// Bouncy spring preset: noticeable oscillation and overshoot (damping ratio 0.50).
    pub const fn bouncy() -> Self {
        Self::new(200, 500, 1)
    }

    /// Snappy spring preset: quick responsiveness with minimal overshoot (damping ratio 0.80).
    pub const fn snappy() -> Self {
        Self::new(250, 800, 1)
    }

    /// Gentle spring preset: smooth and relaxed transition (damping ratio 0.95).
    pub const fn gentle() -> Self {
        Self::new(120, 950, 1)
    }

    /// Critically damped spring preset: fastest arrival with zero overshoot (damping ratio 1.0).
    pub const fn critically_damped() -> Self {
        Self::new(180, 1000, 1)
    }

    /// Effective damping coefficient scaled for integer arithmetic.
    pub fn damping_coeff(&self) -> u64 {
        let km = (self.stiffness as u64) * (self.mass as u64);
        let sqrt_km = isqrt(km * 1_000_000);
        (2 * (self.damping_ratio as u64) * sqrt_km) / 1000
    }

    /// Step the spring physics simulation forward by `dt_ms`.
    ///
    /// Updates `state` in place and returns `true` if the spring has settled at `target`,
    /// or `false` if it is still in motion.
    pub fn step(&self, state: &mut SpringState, target: i32, dt_ms: u32) -> bool {
        if dt_ms == 0 {
            return state.position == target && state.velocity == 0;
        }

        let cur_milli = (state.position as i64) * 1000 + (state.sub_position as i64);
        let tgt_milli = (target as i64) * 1000;
        let disp = cur_milli - tgt_milli;

        let f_spring = -(self.stiffness as i64) * disp;
        let f_damping = -((self.damping_coeff() as i64) * (state.velocity as i64)) / 1000;
        let accel = (f_spring + f_damping) / (self.mass as i64);

        let vel = (state.velocity as i64) + (accel * (dt_ms as i64)) / 1000;
        let new_milli = cur_milli + (vel * (dt_ms as i64)) / 1000;

        let disp_new = if tgt_milli >= new_milli {
            tgt_milli - new_milli
        } else {
            new_milli - tgt_milli
        };
        let vel_abs = if vel >= 0 { vel } else { -vel };

        if disp_new < 2000 && vel_abs < 300 {
            state.position = target;
            state.sub_position = 0;
            state.velocity = 0;
            return true;
        }

        let pos = new_milli / 1000;
        let sub = new_milli % 1000;
        state.position = pos as i32;
        state.sub_position = sub as i32;
        state.velocity = vel as i32;
        false
    }
}

/// Stateful representation of a spring oscillator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpringState {
    /// Integer position (e.g. permille 0..=1000 or screen coordinate).
    pub position: i32,
    /// Velocity in milliunits / second.
    pub velocity: i32,
    /// Internal sub-unit fractional position in milliunits (-999..=999).
    pub sub_position: i32,
}

impl SpringState {
    pub const fn new(position: i32) -> Self {
        Self {
            position,
            velocity: 0,
            sub_position: 0,
        }
    }

    pub const fn with_velocity(position: i32, velocity: i32) -> Self {
        Self {
            position,
            velocity,
            sub_position: 0,
        }
    }

    #[inline]
    pub const fn value(&self) -> i32 {
        self.position
    }

    #[inline]
    pub const fn is_at_rest(&self, target: i32) -> bool {
        self.position == target && self.velocity == 0 && self.sub_position == 0
    }

    pub fn snap_to(&mut self, target: i32) {
        self.position = target;
        self.sub_position = 0;
        self.velocity = 0;
    }
}

// ============================================================================
// Load-Bearing UI Interpolation Helpers
// ============================================================================

/// Linear interpolation for 1D integer coordinates by permille progress (0..=1000).
#[inline]
pub fn lerp_position(start: i32, end: i32, progress_permille: u32) -> i32 {
    let p = progress_permille.min(1000) as i64;
    let s = start as i64;
    let e = end as i64;
    (s + ((e - s) * p) / 1000) as i32
}

/// Linear interpolation for unsigned 2D dimensions / coordinates by permille progress (0..=1000).
#[inline]
pub fn lerp_usize(start: usize, end: usize, progress_permille: u32) -> usize {
    let p = progress_permille.min(1000);
    if end >= start {
        start + ((end - start) * p as usize) / 1000
    } else {
        start - ((start - end) * p as usize) / 1000
    }
}

/// Interpolate a layout position using a specific `MotionDuration` and `ease_in_out` curve.
pub fn interpolate_layout_position(start: i32, end: i32, elapsed_ms: u32, duration: MotionDuration) -> i32 {
    let progress = MotionClock::compute_progress(0, duration.ms(), elapsed_ms as u64);
    let curved = ease_in_out(progress);
    lerp_position(start, end, curved)
}

/// Standard layout position interpolation using `MotionDuration::Normal` (220 ms) and `ease_in_out`.
pub fn interpolate_position(start: i32, end: i32, elapsed_ms: u32) -> i32 {
    interpolate_layout_position(start, end, elapsed_ms, MotionDuration::Normal)
}

/// Calculate sheet drawer slide-up vertical position from `closed_y` to `open_y`.
///
/// Uses `MotionDuration::Slow` (360 ms) and `ease_out` deceleration curve, providing
/// tactile, responsive sheet entrances.
pub fn sheet_drawer_slide_up(closed_y: i32, open_y: i32, elapsed_ms: u32) -> i32 {
    sheet_drawer_slide_position(closed_y, open_y, elapsed_ms, MotionDuration::Slow)
}

/// Generalized sheet drawer slide position supporting custom durations (e.g. `FAST` for dismiss).
pub fn sheet_drawer_slide_position(closed_y: i32, open_y: i32, elapsed_ms: u32, duration: MotionDuration) -> i32 {
    let progress = MotionClock::compute_progress(0, duration.ms(), elapsed_ms as u64);
    let curved = ease_out(progress);
    lerp_position(closed_y, open_y, curved)
}

/// Interpolate selection pill bounding box from `start` to `end` over elapsed time.
///
/// Uses `MotionDuration::Fast` (120 ms) and `ease_in_out` for snappy tab and segment switching.
pub fn selection_pill_bounds(start: Rect, end: Rect, elapsed_ms: u32) -> Rect {
    selection_pill_bounds_with_duration(start, end, elapsed_ms, MotionDuration::Fast)
}

/// Interpolate selection pill bounding box with custom duration.
pub fn selection_pill_bounds_with_duration(start: Rect, end: Rect, elapsed_ms: u32, duration: MotionDuration) -> Rect {
    let progress = MotionClock::compute_progress(0, duration.ms(), elapsed_ms as u64);
    let curved = ease_in_out(progress);
    Rect {
        x: lerp_usize(start.x, end.x, curved),
        y: lerp_usize(start.y, end.y, curved),
        w: lerp_usize(start.w, end.w, curved),
        h: lerp_usize(start.h, end.h, curved),
    }
}

/// Interpolate selection pill bounding box with spring overshoot dynamics for tactile bounce.
pub fn selection_pill_spring_bounds(start: Rect, end: Rect, elapsed_ms: u32) -> Rect {
    let progress = MotionClock::compute_progress(0, FAST, elapsed_ms as u64);
    let curved = spring_overshoot(progress);
    let interpolate_dim = |s: usize, e: usize| -> usize {
        let diff = (e as i64) - (s as i64);
        let val = (s as i64) + (diff * (curved as i64)) / 1000;
        if val < 0 {
            0
        } else {
            val as usize
        }
    };
    Rect {
        x: interpolate_dim(start.x, end.x),
        y: interpolate_dim(start.y, end.y),
        w: interpolate_dim(start.w, end.w),
        h: interpolate_dim(start.h, end.h),
    }
}

/// Compute alpha channel (0..=255) for background scrim fade-in to `target_alpha`.
///
/// Uses `MotionDuration::Slow` (360 ms) and `ease_out` curve matching sheet drawer entrance.
pub fn alpha_scrim(target_alpha: u8, elapsed_ms: u32) -> u8 {
    alpha_scrim_with_duration(0, target_alpha, elapsed_ms, MotionDuration::Slow)
}

/// Compute alpha channel interpolation between `start_alpha` and `target_alpha` with explicit duration.
pub fn alpha_scrim_with_duration(start_alpha: u8, target_alpha: u8, elapsed_ms: u32, duration: MotionDuration) -> u8 {
    let progress = MotionClock::compute_progress(0, duration.ms(), elapsed_ms as u64);
    let curved = ease_out(progress);
    let range = (target_alpha as i32) - (start_alpha as i32);
    let current = (start_alpha as i32) + (range * (curved as i32)) / 1000;
    current.clamp(0, 255) as u8
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_duration_tokens_and_roles() {
        assert_eq!(INSTANT, 0);
        assert_eq!(FAST, 120);
        assert_eq!(NORMAL, 220);
        assert_eq!(SLOW, 360);
        assert_eq!(GENTLE, 500);

        assert_eq!(STANDARD, NORMAL);
        assert_eq!(EMPHASIZED, SLOW);

        assert_eq!(MotionDuration::Instant.ms(), 0);
        assert_eq!(MotionDuration::Fast.ms(), 120);
        assert_eq!(MotionDuration::Normal.ms(), 220);
        assert_eq!(MotionDuration::Slow.ms(), 360);
        assert_eq!(MotionDuration::Gentle.ms(), 500);

        assert_eq!(MotionRole::TapFeedback.ms(), 120);
        assert_eq!(MotionRole::SelectionPill.ms(), 120);
        assert_eq!(MotionRole::LayoutShift.ms(), 220);
        assert_eq!(MotionRole::DrawerSlide.ms(), 360);
        assert_eq!(MotionRole::ModalScrim.ms(), 360);
        assert_eq!(MotionRole::FullScreenTransition.ms(), 500);
    }

    #[test]
    fn test_motion_clock_progress_computation() {
        let clock = MotionClock::new(1000, 200);

        assert_eq!(clock.progress_at(900), 0);
        assert_eq!(clock.progress_at(1000), 0);
        assert_eq!(clock.progress_at(1050), 250);
        assert_eq!(clock.progress_at(1100), 500);
        assert_eq!(clock.progress_at(1150), 750);
        assert_eq!(clock.progress_at(1200), 1000);
        assert_eq!(clock.progress_at(1300), 1000);

        assert!(!clock.is_finished_at(1100));
        assert!(clock.is_finished_at(1200));
        assert!(clock.is_finished_at(1300));

        // Instant duration always 1000
        let instant = MotionClock::with_duration(MotionDuration::Instant, 500);
        assert_eq!(instant.progress_at(500), 1000);
        assert_eq!(instant.progress_at(1000), 1000);
        assert!(instant.is_finished());
    }

    #[test]
    fn test_motion_clock_monotonic_tick_and_step() {
        let mut clock = MotionClock::with_duration(MotionDuration::Normal, 1000);
        assert_eq!(clock.progress(), 0);

        // Advance to 1110 (halfway of 220ms)
        let p = clock.tick(1110);
        assert_eq!(p, 500);
        assert_eq!(clock.elapsed_ms(), 110);
        assert_eq!(clock.remaining_ms(), 110);

        // Monotonic guard: backwards tick does not decrease current_ms
        let p2 = clock.tick(1050);
        assert_eq!(p2, 500);
        assert_eq!(clock.current_ms(), 1110);

        // Step forward by 110ms
        let p3 = clock.step(110);
        assert_eq!(p3, 1000);
        assert!(clock.is_finished());
        assert_eq!(clock.remaining_ms(), 0);

        // Reset
        clock.reset(2000);
        assert_eq!(clock.progress(), 0);
        assert_eq!(clock.start_ms(), 2000);
    }

    #[test]
    fn test_ease_curves() {
        // Endpoints
        assert_eq!(ease_in(0), 0);
        assert_eq!(ease_in(1000), 1000);
        assert_eq!(ease_out(0), 0);
        assert_eq!(ease_out(1000), 1000);
        assert_eq!(ease_in_out(0), 0);
        assert_eq!(ease_in_out(500), 500);
        assert_eq!(ease_in_out(1000), 1000);

        // Curvature characteristics
        assert!(ease_in(500) < 500);
        assert_eq!(ease_in(500), 125);

        assert!(ease_out(500) > 500);
        assert_eq!(ease_out(500), 875);

        // Symmetry of ease_in_out
        for t in [100, 200, 300, 400] {
            let left = ease_in_out(t);
            let right = ease_in_out(1000 - t);
            assert_eq!(left + right, 1000);
        }

        // Monotonicity
        let mut prev_in = 0;
        let mut prev_out = 0;
        let mut prev_in_out = 0;
        for t in (0..=1000).step_by(50) {
            let cur_in = ease_in(t);
            let cur_out = ease_out(t);
            let cur_in_out = ease_in_out(t);
            assert!(cur_in >= prev_in);
            assert!(cur_out >= prev_out);
            assert!(cur_in_out >= prev_in_out);
            prev_in = cur_in;
            prev_out = cur_out;
            prev_in_out = cur_in_out;
        }
    }

    #[test]
    fn test_spring_overshoot() {
        assert_eq!(spring_overshoot(0), 0);
        assert_eq!(spring_overshoot(1000), 1000);

        // Overshoots 1000 between t=400 and t=800
        let peak = spring_overshoot(600);
        assert!(peak > 1050, "Peak should overshoot 1050, was {}", peak);
        assert!(peak < 1150, "Peak should be reasonable, was {}", peak);

        // Lands back at 1000
        assert_eq!(spring_overshoot(1000), 1000);
        assert_eq!(spring_overshoot_clamped(600), 1000);
    }

    #[test]
    fn test_cubic_bezier_solver() {
        // Linear bezier (p1=(0,0), p2=(1000,1000)) matches identity
        for t in [0, 250, 500, 750, 1000] {
            let y = cubic_bezier(t, 0, 0, 1000, 1000);
            assert!((y - (t as i32)).abs() <= 5, "Expected approx {}, got {}", t, y);
        }

        // Standard CSS ease-in-out: (420, 0, 580, 1000)
        assert_eq!(cubic_bezier(0, 420, 0, 580, 1000), 0);
        let mid = cubic_bezier(500, 420, 0, 580, 1000);
        assert!((mid - 500).abs() <= 5, "Midpoint should be ~500, was {}", mid);
        assert_eq!(cubic_bezier(1000, 420, 0, 580, 1000), 1000);
    }

    #[test]
    fn test_isqrt() {
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(1), 1);
        assert_eq!(isqrt(4), 2);
        assert_eq!(isqrt(9), 3);
        assert_eq!(isqrt(16), 4);
        assert_eq!(isqrt(100), 10);
        assert_eq!(isqrt(1_000_000), 1000);
    }

    #[test]
    fn test_spring_dynamics_snappy_and_settling() {
        let spring = SpringConfig::snappy();
        let mut state = SpringState::new(0);

        let mut settled = false;
        for _ in 0..100 {
            settled = spring.step(&mut state, 1000, 16);
            if settled {
                break;
            }
        }

        assert!(settled, "Spring should settle within 100 steps (1.6s)");
        assert_eq!(state.position, 1000);
        assert_eq!(state.velocity, 0);
        assert!(state.is_at_rest(1000));
    }

    #[test]
    fn test_spring_dynamics_bouncy_overshoot() {
        let spring = SpringConfig::bouncy();
        let mut state = SpringState::new(0);

        let mut overshot = false;
        let mut settled = false;
        for _ in 0..120 {
            if state.position > 1050 {
                overshot = true;
            }
            settled = spring.step(&mut state, 1000, 16);
            if settled {
                break;
            }
        }

        assert!(overshot, "Bouncy spring should overshoot > 1050");
        assert!(settled, "Bouncy spring should eventually settle");
        assert_eq!(state.position, 1000);
    }

    #[test]
    fn test_interpolate_layout_position() {
        // Normal duration (220 ms)
        assert_eq!(interpolate_position(100, 500, 0), 100);
        assert_eq!(interpolate_position(100, 500, 110), 300); // 50% ease_in_out midpoint
        assert_eq!(interpolate_position(100, 500, 220), 500);
        assert_eq!(interpolate_position(100, 500, 300), 500); // Clamped

        // Instant duration snaps immediately
        assert_eq!(
            interpolate_layout_position(100, 500, 0, MotionDuration::Instant),
            500
        );
    }

    #[test]
    fn test_sheet_drawer_slide_up() {
        let closed_y = 1440;
        let open_y = 600;

        assert_eq!(sheet_drawer_slide_up(closed_y, open_y, 0), 1440);
        let mid = sheet_drawer_slide_up(closed_y, open_y, 180);
        // ease_out at 50% time is 87.5% progress
        // 1440 - (840 * 875 / 1000) = 1440 - 735 = 705
        assert_eq!(mid, 705);
        assert_eq!(sheet_drawer_slide_up(closed_y, open_y, 360), 600);
        assert_eq!(sheet_drawer_slide_up(closed_y, open_y, 500), 600);
    }

    #[test]
    fn test_selection_pill_bounds() {
        let start = Rect::new(20, 100, 80, 44);
        let end = Rect::new(120, 100, 100, 44);

        // At t=0
        assert_eq!(selection_pill_bounds(start, end, 0), start);

        // At t=60 (halfway of 120ms FAST duration)
        let mid = selection_pill_bounds(start, end, 60);
        assert_eq!(mid.x, 70);
        assert_eq!(mid.y, 100);
        assert_eq!(mid.w, 90);
        assert_eq!(mid.h, 44);

        // At t=120
        assert_eq!(selection_pill_bounds(start, end, 120), end);

        // Spring bounds overshoot test
        let spring_pill = selection_pill_spring_bounds(start, end, 72); // 600 permille
        assert!(spring_pill.x > 120, "Spring pill x should overshoot end x, was {}", spring_pill.x);
    }

    #[test]
    fn test_alpha_scrim() {
        assert_eq!(alpha_scrim(180, 0), 0);
        let mid = alpha_scrim(180, 180);
        // At 50% time (180ms of 360ms), ease_out is 875 permille -> 180 * 875 / 1000 = 157
        assert_eq!(mid, 157);
        assert_eq!(alpha_scrim(180, 360), 180);
        assert_eq!(alpha_scrim(180, 500), 180);

        // Custom start and duration
        let fade_out = alpha_scrim_with_duration(200, 0, 120, MotionDuration::Fast);
        assert_eq!(fade_out, 0);
    }
}
