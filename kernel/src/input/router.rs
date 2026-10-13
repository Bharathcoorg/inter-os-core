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
    pub scale_milli: u32,
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
    pub edge: u8,
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
            edge: 0,
            down_tick: 0,
        }
    }
}

const LONG_PRESS_TICKS: u64 = 500;
const DOUBLE_TAP_WINDOW_TICKS: u64 = 300;
const FLING_WINDOW_TICKS: u64 = 250;

/// WHY: one constant decides "the finger never really left this spot", shared
/// by tap classification and by the double-tap proximity test. A real finger
/// wobbles by more than a dozen pixels between contact and release, so a
/// release a couple of dozen pixels from the press is still a tap. This is
/// also why the slop is expressed as a named tolerance rather than folded into
/// the edge/fling thresholds: widening it must not make a real drag vanish.
const TAP_SLOP: i32 = 24;

/// WHY: how far a press must travel before a system-owned edge swipe is
/// recognised. It sits above `TAP_SLOP` so a press that merely wobbles off the
/// edge is consumed by the tap arm first, and below the 80px fling travel so
/// an edge swipe is not rejected for being short. Travel that is too short for
/// the touched edge's direction is `Cancel`, not a mislabelled Back/Recents.
const EDGE_SWIPE_MIN: i32 = 40;

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
    pub fn classify_owner(&self, position: Point, screen_width: u32, screen_height: u32, edge: u32) -> PointerOwner {
        if position.x < edge as i32
            || position.x >= screen_width.saturating_sub(edge) as i32
            || position.y >= screen_height.saturating_sub(edge) as i32
        {
            PointerOwner::System
        } else {
            PointerOwner::App
        }
    }

    fn classify_edge(position: Point, screen_width: u32, screen_height: u32, edge: u32) -> u8 {
        if position.x < edge as i32 { 1 }
        else if position.x >= screen_width.saturating_sub(edge) as i32 { 2 }
        else if position.y >= screen_height.saturating_sub(edge) as i32 { 3 }
        else { 0 }
    }

    /// Maps a completed swipe onto the system gesture owned by the edge it
    /// started on.
    ///
    /// WHY: which edge was touched is captured at `Down` and is the only thing
    /// that can disambiguate a horizontal swipe — direction alone cannot, since
    /// a rightward swipe on the left edge means Back while a leftward swipe on
    /// the right edge means Recents, and both are "horizontal". This lives in
    /// one function so the timed and untimed entry points cannot disagree
    /// about the same Down+Up pair.
    fn classify_edge_gesture(edge: u8, dx: i32, dy: i32) -> GestureKind {
        match edge {
            1 if dx >= EDGE_SWIPE_MIN => GestureKind::Back,
            2 if dx <= -EDGE_SWIPE_MIN => GestureKind::Recents,
            3 if dy <= -EDGE_SWIPE_MIN => GestureKind::Home,
            _ => GestureKind::Cancel,
        }
    }

    /// Dispatches a pointer using monotonic kernel ticks supplied by the caller.
    ///
    /// WHY: elapsed time is an argument, never a global read. The router
    /// classifies a gesture purely from the events handed to it plus the tick
    /// stamped on each one, so its timing is a property of the event stream
    /// rather than of a hardware counter that only the PIT handler advances.
    /// That is what makes long-press, double-tap, and fling decidable in a
    /// host test where no timer runs. The single production caller reads
    /// `interrupts::get_system_ticks()` once per posted event
    /// (`input::post_touch_event`) and passes that value through, so the
    /// stream is stamped at the boundary and stays immutable downstream.
    ///
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
                let owner = self.classify_owner(event.position, screen_width, 1440, edge);
                let edge_code = Self::classify_edge(event.position, screen_width, 1440, edge);
                #[cfg(not(test))]
                crate::serial_println!("[ROUTER] Down pos=({}, {}) owner={:?} edge={}", event.position.x, event.position.y, owner, edge_code);
                self.pointers[index] = PointerState {
                    active: true,
                    source: event.source,
                    space: event.space,
                    start: event.position,
                    current: event.position,
                    owner,
                    edge: edge_code,
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
                            scale_milli,
                        });
                    }
                } else {
                    self.pinch_initial_distance = None;
                }
                None
            }
            PointerPhase::Up => {
                let mut state = self.pointers[index];
                if !state.active { return None; }
                // WHY: the release event carries the authoritative release
                // position, and it is the only sample of the pointer's final
                // location. `current` otherwise only ever advances on Move, so
                // a stream whose Move samples were coalesced away or never
                // delivered would classify the gesture as a tap at the press
                // point. Adopting the release position here is what makes
                // Down+Up with no intermediate Move still resolve to the
                // movement gesture the user actually performed.
                state.current = event.position;
                self.pointers[index] = PointerState::empty();
                if self.pointers.iter().all(|p| !p.active) { self.pinch_initial_distance = None; }
                let dx = state.current.x - state.start.x;
                let dy = state.current.y - state.start.y;
                let distance = dx.abs().max(dy.abs());
                if distance <= TAP_SLOP {
                    let held = now_tick.saturating_sub(state.down_tick);
                    let kind = if held >= LONG_PRESS_TICKS {
                        GestureKind::LongPress
                    } else if self.last_tap_tick != 0
                        && now_tick.saturating_sub(self.last_tap_tick) <= DOUBLE_TAP_WINDOW_TICKS
                        && (state.current.x - self.last_tap_position.x).abs() <= TAP_SLOP
                        && (state.current.y - self.last_tap_position.y).abs() <= TAP_SLOP
                    {
                        GestureKind::DoubleTap
                    } else {
                        GestureKind::Tap
                    };
                    if kind == GestureKind::Tap || kind == GestureKind::DoubleTap {
                        self.last_tap_tick = now_tick;
                        self.last_tap_position = state.current;
                    }
                    return Some(GestureEvent {
                        kind,
                        position: state.current,
                        delta_x: dx,
                        delta_y: dy,
                        pointer_id: event.pointer_id,
                        scale_milli: 1000,
                    });
                }
                if state.owner == PointerOwner::System {
                    // WHY: `owner` and `edge` are latched at Down and are
                    // deliberately not re-derived at release. A back swipe
                    // necessarily starts on the left edge and finishes in the
                    // middle of the screen, so re-sampling the edge at the
                    // release point would classify every real edge swipe as
                    // Cancel and hand it to the app instead of the system.
                    let kind = Self::classify_edge_gesture(state.edge, dx, dy);
                    #[cfg(not(test))]
                    crate::serial_println!("[ROUTER] Up System-owned edge={} -> gesture={:?}", state.edge, kind);
                    return Some(GestureEvent {
                        kind,
                        position: state.current,
                        delta_x: dx,
                        delta_y: dy,
                        pointer_id: event.pointer_id,
                        scale_milli: 1000,
                    });
                }
                let held = now_tick.saturating_sub(state.down_tick);
                // WHY: fling is the fast-far case, scroll is everything else.
                // A drag held past the fling window stays a drag, so slowing
                // down can never be used to launder a movement into a fling.
                let kind = if held <= FLING_WINDOW_TICKS && distance >= 80 {
                    GestureKind::Fling
                } else {
                    GestureKind::Scroll
                };
                #[cfg(not(test))]
                crate::serial_println!("[ROUTER] Up App-owned held={} dist={} -> gesture={:?}", held, distance, kind);
                Some(GestureEvent {
                    kind,
                    position: state.current,
                    delta_x: dx,
                    delta_y: dy,
                    pointer_id: event.pointer_id,
                    scale_milli: 1000,
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
                        scale_milli: 1000,
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
                let owner = self.classify_owner(event.position, screen_width, 1440, edge);
                self.pointers[index] = PointerState {
                    active: true,
                    source: event.source,
                    space: event.space,
                    start: event.position,
                    current: event.position,
                    owner,
                    edge: Self::classify_edge(event.position, screen_width, 1440, edge),
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
                let mut state = self.pointers[index];
                if !state.active {
                    return None;
                }
                // WHY: same invariant as `dispatch_pointer_at` — the release
                // position is the authoritative final sample and must not be
                // dropped in the timing-free path either, otherwise the two
                // entry points would disagree about a single Down+Up pair.
                state.current = event.position;
                self.pointers[index] = PointerState::empty();
                let dx = state.current.x - state.start.x;
                let dy = state.current.y - state.start.y;
                let distance = dx.abs().max(dy.abs());

                if distance <= TAP_SLOP {
                    return Some(GestureEvent {
                        kind: GestureKind::Tap,
                        position: state.current,
                        delta_x: dx,
                        delta_y: dy,
                        pointer_id: event.pointer_id,
                        scale_milli: 1000,
                    });
                }

                if state.owner == PointerOwner::System {
                    // WHY: uses the latched edge, exactly like the timed path.
                    // The previous rule here compared only dx against dy and so
                    // reported Recents for *any* horizontal edge swipe — a
                    // left-edge swipe towards the right is Back, and nothing
                    // about the direction alone can tell the two apart.
                    let kind = Self::classify_edge_gesture(state.edge, dx, dy);
                    return Some(GestureEvent {
                        kind,
                        position: state.current,
                        delta_x: dx,
                        delta_y: dy,
                        pointer_id: event.pointer_id,
                        scale_milli: 1000,
                    });
                }

                Some(GestureEvent {
                    kind: GestureKind::Scroll,
                    position: state.current,
                    delta_x: dx,
                    delta_y: dy,
                    pointer_id: event.pointer_id,
                        scale_milli: 1000,
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
                        scale_milli: 1000,
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
            Some(GestureKind::Back)
        );
    }

    #[test]
    fn right_edge_swipe_left_is_recents() {
        let mut router = InputRouter::new();
        let down = PointerEvent::new(InputSource::Touch, CoordinateSpace::LogicalPortrait, 0, PointerPhase::Down, 716, 700);
        let up = PointerEvent::new(InputSource::Touch, CoordinateSpace::LogicalPortrait, 0, PointerPhase::Up, 620, 700);
        router.dispatch_pointer(down, 720, 24);
        assert_eq!(router.dispatch_pointer(up, 720, 24).map(|g| g.kind), Some(GestureKind::Recents));
    }

    #[test]
    fn bottom_edge_swipe_up_is_home() {
        let mut router = InputRouter::new();
        let down = PointerEvent::new(InputSource::Touch, CoordinateSpace::LogicalPortrait, 0, PointerPhase::Down, 360, 1438);
        let up = PointerEvent::new(InputSource::Touch, CoordinateSpace::LogicalPortrait, 0, PointerPhase::Up, 360, 1360);
        router.dispatch_pointer(down, 720, 24);
        assert_eq!(router.dispatch_pointer(up, 720, 24).map(|g| g.kind), Some(GestureKind::Home));
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
            router.dispatch_pointer_at(up, 720, 24, 601).map(|g| g.kind),
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

    // WHY: the two tests below are the same code path, the same source, the
    // same pointer id, and the same tick values. The only difference is
    // whether the release event carries a position away from the press. They
    // exist as a pair so a future change cannot make a stationary press and a
    // travelling one collapse into the same gesture kind: the router must
    // learn the shape of the gesture from the events, not from the clock.

    #[test]
    fn release_position_alone_distinguishes_a_swipe_from_a_tap() {
        let mut router = InputRouter::new();
        // No Move event is ever delivered: the release must still be honoured.
        let down = PointerEvent::new(
            InputSource::DeviceStudio,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Down,
            300,
            700,
        );
        let up = PointerEvent::new(
            InputSource::DeviceStudio,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Up,
            300,
            600,
        );
        router.dispatch_pointer_at(down, 720, 24, 100);
        let gesture = router
            .dispatch_pointer_at(up, 720, 24, 108)
            .expect("a Down+Up pair with no Move is still a gesture");
        assert_eq!(gesture.kind, GestureKind::Fling);
        assert_eq!(gesture.delta_y, -100, "reported travel is the release travel");
        assert_eq!(gesture.position, Point { x: 300, y: 600 });
    }

    #[test]
    fn release_at_the_press_point_with_no_elapsed_ticks_is_still_a_tap() {
        let mut router = InputRouter::new();
        let down = PointerEvent::new(
            InputSource::DeviceStudio,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Down,
            300,
            700,
        );
        let up = PointerEvent::new(
            InputSource::DeviceStudio,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Up,
            300,
            700,
        );
        router.dispatch_pointer_at(down, 720, 24, 100);
        // Zero elapsed ticks: not a long press, and last_tap_tick is still the
        // `0` sentinel, so no double-tap can be synthesised either.
        let gesture = router
            .dispatch_pointer_at(up, 720, 24, 100)
            .expect("a stationary press is a tap");
        assert_eq!(gesture.kind, GestureKind::Tap);
        assert_eq!(gesture.delta_x, 0);
        assert_eq!(gesture.delta_y, 0);
    }

    #[test]
    fn slow_drag_with_move_samples_is_a_drag_not_a_fling() {
        let mut router = InputRouter::new();
        let down = PointerEvent::new(
            InputSource::DeviceStudio,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Down,
            300,
            700,
        );
        let mid = PointerEvent::new(
            InputSource::DeviceStudio,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Move,
            300,
            640,
        );
        let up = PointerEvent::new(
            InputSource::DeviceStudio,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Up,
            300,
            600,
        );
        router.dispatch_pointer_at(down, 720, 24, 100);
        router.dispatch_pointer_at(mid, 720, 24, 200);
        // 400 ticks held, past the fling window: the same 100px of travel that
        // flung above must be an ordinary drag here, so distance alone cannot
        // decide the gesture and a slow drag cannot become a fling.
        let gesture = router
            .dispatch_pointer_at(up, 720, 24, 500)
            .expect("a drag is a gesture");
        assert_eq!(gesture.kind, GestureKind::Scroll);
        assert_eq!(gesture.delta_y, -100);
    }

    #[test]
    fn tap_slop_boundary_separates_a_tap_from_a_drag() {
        // WHY: pins the exact tolerance rather than "some large number", so
        // the tap/drag boundary cannot be widened until every gesture passes.
        let down = |x: i32, y: i32| PointerEvent::new(
            InputSource::DeviceStudio,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Down,
            x,
            y,
        );
        let up = |x: i32, y: i32| PointerEvent::new(
            InputSource::DeviceStudio,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Up,
            x,
            y,
        );

        let mut router = InputRouter::new();
        router.dispatch_pointer_at(down(300, 700), 720, 24, 100);
        assert_eq!(
            router
                .dispatch_pointer_at(up(300 + TAP_SLOP, 700), 720, 24, 110)
                .map(|g| g.kind),
            Some(GestureKind::Tap),
            "travel exactly at the slop is finger wobble, not a drag"
        );

        let mut router = InputRouter::new();
        router.dispatch_pointer_at(down(300, 700), 720, 24, 100);
        assert_eq!(
            router
                .dispatch_pointer_at(up(300 + TAP_SLOP + 1, 700), 720, 24, 110)
                .map(|g| g.kind),
            Some(GestureKind::Scroll),
            "one pixel past the slop is a real drag"
        );
    }

    #[test]
    fn edge_swipe_recognised_without_move_samples_loses_to_a_short_press() {
        // WHY: the same untimed entry point must judge a real edge swipe by
        // its release position while still refusing to turn a wobbling press
        // on the edge into a system gesture.
        let mut router = InputRouter::new();
        router.dispatch_pointer(
            PointerEvent::new(
                InputSource::Touch,
                CoordinateSpace::LogicalPortrait,
                0,
                PointerPhase::Down,
                4,
                700,
            ),
            720,
            24,
        );
        assert_eq!(
            router
                .dispatch_pointer(
                    PointerEvent::new(
                        InputSource::Touch,
                        CoordinateSpace::LogicalPortrait,
                        0,
                        PointerPhase::Up,
                        12,
                        700,
                    ),
                    720,
                    24,
                )
                .map(|g| g.kind),
            Some(GestureKind::Tap)
        );

        let mut router = InputRouter::new();
        router.dispatch_pointer(
            PointerEvent::new(
                InputSource::Touch,
                CoordinateSpace::LogicalPortrait,
                0,
                PointerPhase::Down,
                4,
                700,
            ),
            720,
            24,
        );
        assert_eq!(
            router
                .dispatch_pointer(
                    PointerEvent::new(
                        InputSource::Touch,
                        CoordinateSpace::LogicalPortrait,
                        0,
                        PointerPhase::Up,
                        120,
                        700,
                    ),
                    720,
                    24,
                )
                .map(|g| g.kind),
            Some(GestureKind::Back)
        );
    }
}

#[cfg(test)]
mod canonical_integration_tests {
    use super::*;

    #[test]
    fn timed_router_emits_fling_for_fast_app_swipe() {
        let mut router = InputRouter::new();
        let down = PointerEvent::new(
            InputSource::DeviceStudio,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Down,
            300,
            700,
        );
        let up = PointerEvent::new(
            InputSource::DeviceStudio,
            CoordinateSpace::LogicalPortrait,
            0,
            PointerPhase::Up,
            300,
            600,
        );
        router.dispatch_pointer_at(down, 720, 24, 100);
        assert_eq!(
            router.dispatch_pointer_at(up, 720, 24, 108).map(|g| g.kind),
            Some(GestureKind::Fling)
        );
    }

    #[test]
    fn timed_router_emits_pinch_with_scale() {
        let mut router = InputRouter::new();
        let d0 = PointerEvent::new(InputSource::Touch, CoordinateSpace::LogicalPortrait, 0, PointerPhase::Down, 200, 400);
        let d1 = PointerEvent::new(InputSource::Touch, CoordinateSpace::LogicalPortrait, 1, PointerPhase::Down, 400, 400);
        let m1 = PointerEvent::new(InputSource::Touch, CoordinateSpace::LogicalPortrait, 1, PointerPhase::Move, 450, 400);
        router.dispatch_pointer_at(d0, 720, 24, 10);
        router.dispatch_pointer_at(d1, 720, 24, 11);
        let gesture = router.dispatch_pointer_at(m1, 720, 24, 12).expect("pinch gesture");
        assert_eq!(gesture.kind, GestureKind::Pinch);
        assert!(gesture.scale_milli > 1000);
    }
}
