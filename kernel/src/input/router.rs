//! Canonical input routing above raw hardware events.
//!
//! The router is deliberately UI-agnostic: it converts declared coordinate
//! spaces, tracks pointer ownership, and emits semantic gestures. Applications
//! do not need to understand Device Studio, QEMU, or physical-device geometry.

use super::coordinate::{CoordinateSpace, CoordinateTransform, Point};
use super::event::{InputSource, PointerEvent, PointerPhase};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GestureKind {
    Tap,
    DoubleTap,
    LongPress,
    Scroll,
    Fling,
    Back,
    Home,
    Recents,
    Cancel,
    Pinch,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GestureEvent {
    pub kind: GestureKind,
    pub position: Point,
    pub delta_x: i32,
    pub delta_y: i32,
    pub pointer_id: u8,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PointerOwner {
    None,
    App,
    System,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PointerState {
    pub active: bool,
    pub source: InputSource,
    pub space: CoordinateSpace,
    pub start: Point,
    pub current: Point,
    pub owner: PointerOwner,
    pub down_tick: u64,
}

impl PointerState {
    pub const fn empty() -> Self {
        Self {
            active: false,
            source: InputSource::Touch,
            space: CoordinateSpace::LogicalPortrait,
            start: Point { x: 0, y: 0 },
            current: Point { x: 0, y: 0 },
            owner: PointerOwner::None,
            down_tick: 0,
        }
    }
}

pub struct InputRouter {
    pointers: [PointerState; 5],
    last_tap_tick: u64,
    last_tap_position: Point,
    pinch_initial_distance: Option<u32>,
}

fn isqrt_u32(n: u32) -> u32 {
    if n == 0 { return 0; }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

impl InputRouter {
    pub const fn new() -> Self {
        Self {
            pointers: [PointerState::empty(); 5],
            last_tap_tick: 0,
            last_tap_position: Point { x: 0, y: 0 },
            pinch_initial_distance: None,
        }
    }

    pub fn pointer(&self, id: u8) -> Option<PointerState> {
        self.pointers.get(id as usize).copied()
    }

    pub fn transform(
        &self,
        event: PointerEvent,
        transform: CoordinateTransform,
    ) -> Option<PointerEvent> {
        if event.space != transform.source {
            return None;
        }
        let position = transform.map(event.position)?;
        Some(PointerEvent {
            space: transform.target,
            position,
            ..event
        })
    }

    /// System-owned gestures are reserved only at the display edges.
    /// Everything else remains available to the focused application.
    pub fn classify_owner(&self, position: Point, screen_width: u32, edge: u32) -> PointerOwner {
        if position.x < edge as i32 || position.x >= screen_width.saturating_sub(edge) as i32 {
            PointerOwner::System
        } else {
            PointerOwner::App
        }
    }

    /// Dispatches a pointer using monotonic kernel ticks.
    /// This variant enables long-press and double-tap semantics while keeping
    /// the legacy convenience method deterministic for callers without timing.
    pub fn dispatch_pointer_at(
        &mut self,
        event: PointerEvent,
        screen_width: u32,
        edge: u32,
        now_tick: u64,
    ) -> Option<GestureEvent> {
        let index = event.pointer_id as usize;
        if index >= self.pointers.len() {
            return None;
        }
        match event.phase {
            PointerPhase::Down => {
                let owner = self.classify_owner(event.position, screen_width, edge);
                self.pointers[index] = PointerState {
                    active: true,
                    source: event.source,
                    space: event.space,
                    start: event.position,
                    current: event.position,
                    owner,
                    down_tick: now_tick,
                };
                None
            }
            PointerPhase::Move => {
                let state = &mut self.pointers[index];
                if !state.active { return None; }
                state.current = event.position;

                let mut active = [0usize; 2];
                let mut count = 0usize;
                for (id, pointer) in self.pointers.iter().enumerate() {
                    if pointer.active {
                        if count < 2 { active[count] = id; }
                        count += 1;
                    }
                }
                if count == 2 {
                    let a = self.pointers[active[0]];
                    let b = self.pointers[active[1]];
                    let dx0 = a.start.x - b.start.x;
                    let dy0 = a.start.y - b.start.y;
                    let dx1 = a.current.x - b.current.x;
                    let dy1 = a.current.y - b.current.y;
                    let initial = self.pinch_initial_distance.get_or_insert(
                        isqrt_u32((dx0 * dx0 + dy0 * dy0) as u32)
                    );
                    let current = isqrt_u32((dx1 * dx1 + dy1 * dy1) as u32);
                    if *initial > 12 && current > 12 {
                        let scale_milli = current.saturating_mul(1000) / *initial;
                        let center = Point {
                            x: (a.current.x + b.current.x) / 2,
                            y: (a.current.y + b.current.y) / 2,
                        };
                        return Some(GestureEvent {
                            kind: GestureKind::Pinch,
                            position: center,
                            delta_x: a.current.x - a.start.x,
                            delta_y: a.current.y - a.start.y,
                            pointer_id: event.pointer_id,
                        });
                    }
                } else {
                    self.pinch_initial_distance = None;
                }
                None
            }
            PointerPhase::Up => {
                let state = self.pointers[index];
                if !state.active { return None; }
                self.pointers[index] = PointerState::empty();
                if self.pointers.iter().all(|p| !p.active) { self.pinch_initial_distance = None; }
                let dx = state.current.x - state.start.x;
                let dy = state.current.y - state.start.y;
                let distance = dx.abs().max(dy.abs());
                if distance <= 12 {
                    let held = now_tick.saturating_sub(state.down_tick);
                    let kind = if held >= 50 {
                        GestureKind::LongPress
                    } else if self.last_tap_tick != 0
                        && now_tick.saturating_sub(self.last_tap_tick) <= 20
                        && (state.current.x - self.last_tap_position.x).abs() <= 24
                        && (state.current.y - self.last_tap_position.y).abs() <= 24
                    {
                        GestureKind::DoubleTap
                    } else {
                        GestureKind::Tap
                    };
                    self.last_tap_tick = now_tick;
                    self.last_tap_position = state.current;
                    return Some(GestureEvent {
                        kind,
                        position: state.current,
                        delta_x: dx,
                        delta_y: dy,
                        pointer_id: event.pointer_id,
                    });
                }
                if state.owner == PointerOwner::System {
                    let kind = if dy.abs() > dx.abs() {
                        if dy > 0 { GestureKind::Back } else { GestureKind::Home }
                    } else {
                        GestureKind::Recents
                    };
                    return Some(GestureEvent {
                        kind,
                        position: state.current,
                        delta_x: dx,
                        delta_y: dy,
                        pointer_id: event.pointer_id,
                    });
                }
                Some(GestureEvent {
                    kind: GestureKind::Scroll,
                    position: state.current,
                    delta_x: dx,
                    delta_y: dy,
                    pointer_id: event.pointer_id,
                })
            }
            PointerPhase::Cancel => {
                if self.pointers[index].active {
                    self.pointers[index] = PointerState::empty();
                    Some(GestureEvent {
                        kind: GestureKind::Cancel,
                        position: event.position,
                        delta_x: 0,
                        delta_y: 0,
                        pointer_id: event.pointer_id,
                    })
                } else { None }
            }
        }
    }

    pub fn dispatch_pointer(
        &mut self,
        event: PointerEvent,
        screen_width: u32,
        edge: u32,
    ) -> Option<GestureEvent> {
        let index = event.pointer_id as usize;
        if index >= self.pointers.len() {
            return None;
        }

        match event.phase {
            PointerPhase::Down => {
                let owner = self.classify_owner(event.position, screen_width, edge);
                self.pointers[index] = PointerState {
                    active: true,
                    source: event.source,
                    space: event.space,
                    start: event.position,
                    current: event.position,
                    owner,
                    down_tick: 0,
                };
                None
            }
            PointerPhase::Move => {
                let state = &mut self.pointers[index];
                if !state.active {
                    return None;
                }
                state.current = event.position;
                None
            }
            PointerPhase::Up => {
                let state = self.pointers[index];
                if !state.active {
                    return None;
                }
                self.pointers[index] = PointerState::empty();
                let dx = state.current.x - state.start.x;
                let dy = state.current.y - state.start.y;
                let distance = dx.abs().max(dy.abs());

                if distance <= 12 {
                    return Some(GestureEvent {
                        kind: GestureKind::Tap,
                        position: state.current,
                        delta_x: dx,
                        delta_y: dy,
                        pointer_id: event.pointer_id,
                    });
                }

                if state.owner == PointerOwner::System {
                    let kind = if dy.abs() > dx.abs() {
                        if dy > 0 { GestureKind::Back } else { GestureKind::Home }
                    } else {
                        GestureKind::Recents
                    };
                    return Some(GestureEvent {
                        kind,
                        position: state.current,
                        delta_x: dx,
                        delta_y: dy,
                        pointer_id: event.pointer_id,
                    });
                }

                Some(GestureEvent {
                    kind: GestureKind::Scroll,
                    position: state.current,
                    delta_x: dx,
                    delta_y: dy,
                    pointer_id: event.pointer_id,
                })
            }
            PointerPhase::Cancel => {
                if self.pointers[index].active {
                    self.pointers[index] = PointerState::empty();
                    Some(GestureEvent {
                        kind: GestureKind::Cancel,
                        position: event.position,
                        delta_x: 0,
                        delta_y: 0,
                        pointer_id: event.pointer_id,
                    })
                } else {
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::coordinate::CoordinateSpace;

    #[test]
    fn app_pointer_stays_with_app_until_release() {
        let mut router = InputRouter::new();
        let down = PointerEvent::new(
            InputSource::Mouse,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Down,
            360,
            700,
        );
        assert_eq!(router.dispatch_pointer(down, 720, 24), None);
        let up = PointerEvent::new(
            InputSource::Mouse,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Up,
            360,
            720,
        );
        assert_eq!(
            router.dispatch_pointer(up, 720, 24).map(|g| g.kind),
            Some(GestureKind::Tap)
        );
    }

    #[test]
    fn edge_release_becomes_system_gesture() {
        let mut router = InputRouter::new();
        let down = PointerEvent::new(
            InputSource::Touch,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Down,
            4,
            700,
        );
        router.dispatch_pointer(down, 720, 24);
        let up = PointerEvent::new(
            InputSource::Touch,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Up,
            160,
            700,
        );
        assert_eq!(
            router.dispatch_pointer(up, 720, 24).map(|g| g.kind),
            Some(GestureKind::Recents)
        );
    }

    #[test]
    fn timed_tap_and_double_tap_are_semantic() {
        let mut router = InputRouter::new();
        let down = PointerEvent::new(
            InputSource::Touch,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Down,
            200,
            300,
        );
        let up = PointerEvent::new(
            InputSource::Touch,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Up,
            200,
            300,
        );
        router.dispatch_pointer_at(down, 720, 24, 100);
        assert_eq!(
            router.dispatch_pointer_at(up, 720, 24, 110).map(|g| g.kind),
            Some(GestureKind::Tap)
        );
        router.dispatch_pointer_at(down, 720, 24, 115);
        assert_eq!(
            router.dispatch_pointer_at(up, 720, 24, 120).map(|g| g.kind),
            Some(GestureKind::DoubleTap)
        );
    }

    #[test]
    fn timed_hold_becomes_long_press() {
        let mut router = InputRouter::new();
        let down = PointerEvent::new(
            InputSource::Touch,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Down,
            200,
            300,
        );
        let up = PointerEvent::new(
            InputSource::Touch,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Up,
            200,
            300,
        );
        router.dispatch_pointer_at(down, 720, 24, 100);
        assert_eq!(
            router.dispatch_pointer_at(up, 720, 24, 151).map(|g| g.kind),
            Some(GestureKind::LongPress)
        );
    }

    #[test]
    fn cancel_releases_capture() {
        let mut router = InputRouter::new();
        let down = PointerEvent::new(
            InputSource::Qemu,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Down,
            100,
            100,
        );
        router.dispatch_pointer(down, 720, 24);
        let cancel = PointerEvent::new(
            InputSource::Qemu,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Cancel,
            100,
            100,
        );
        assert_eq!(
            router.dispatch_pointer(cancel, 720, 24).map(|g| g.kind),
            Some(GestureKind::Cancel)
        );
        assert!(!router.pointer(0).unwrap().active);
    }
}