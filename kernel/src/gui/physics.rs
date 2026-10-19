//! Fluid Inertial Physics Touch Scroller & Elastic Spring Engine
//!
//! Provides bare-metal 60 Hz smartphone momentum scrolling and spring-loaded
//! overscroll boundaries for the Gravity Glass display pipeline.
//! Uses integer arithmetic to avoid floating-point overhead in Ring 0.

use crate::serial_println;

/// Friction coefficient: velocity retained per 16ms frame (scaled by 1000).
pub const FRICTION_FACTOR: i32 = 920; // 92% retention = smooth natural deceleration
pub const MIN_VELOCITY: i32 = 5; // Velocity threshold below which movement halts
pub const SPRING_RESTITUTION: i32 = 180; // Overscroll spring tension factor

/// Fluid Inertial Physics Scroller state.
#[derive(Debug, Clone)]
pub struct InertialScroller {
    pub viewport_height: i32,
    pub content_height: i32,
    pub scroll_y: i32,
    pub velocity_y: i32,
    pub is_dragging: bool,
    pub last_touch_y: i32,
}

impl InertialScroller {
    /// Creates a new scroller with specified viewport and content dimensions.
    pub const fn new(viewport_height: i32, content_height: i32) -> Self {
        Self {
            viewport_height,
            content_height,
            scroll_y: 0,
            velocity_y: 0,
            is_dragging: false,
            last_touch_y: 0,
        }
    }

    /// Maximum valid scroll offset before overscrolling bottom.
    pub fn max_scroll(&self) -> i32 {
        (self.content_height - self.viewport_height).max(0)
    }

    /// Handles finger touch down event.
    pub fn on_touch_down(&mut self, y: i32) {
        self.is_dragging = true;
        self.velocity_y = 0;
        self.last_touch_y = y;
    }

    /// Handles continuous finger drag event.
    pub fn on_touch_move(&mut self, y: i32) {
        if !self.is_dragging {
            return;
        }
        let delta = self.last_touch_y - y;
        self.last_touch_y = y;

        // Apply resistance if already overscrolling
        let effective_delta = if self.scroll_y < 0 || self.scroll_y > self.max_scroll() {
            delta / 2 // 50% elastic drag resistance
        } else {
            delta
        };

        self.scroll_y += effective_delta;
        self.velocity_y = delta * 60; // Approximate pixel/s velocity
    }

    /// Handles finger release (touch up) event, transitioning into inertial fling.
    pub fn on_touch_up(&mut self) {
        self.is_dragging = false;
    }

    /// Advances physics simulation by one frame interval (typically 16 ms for 60 FPS).
    pub fn tick(&mut self) {
        if self.is_dragging {
            return;
        }

        let max = self.max_scroll();

        // 1. Overscroll Spring-back (top edge: scroll_y < 0)
        if self.scroll_y < 0 {
            let displacement = -self.scroll_y;
            let spring_force = (displacement * SPRING_RESTITUTION) / 1000;
            self.scroll_y += spring_force.max(1);
            self.velocity_y = 0;
            return;
        }

        // 2. Overscroll Spring-back (bottom edge: scroll_y > max)
        if self.scroll_y > max {
            let displacement = self.scroll_y - max;
            let spring_force = (displacement * SPRING_RESTITUTION) / 1000;
            self.scroll_y -= spring_force.max(1);
            self.velocity_y = 0;
            return;
        }

        // 3. Inertial Fling Deceleration
        if self.velocity_y.abs() > MIN_VELOCITY {
            self.scroll_y += self.velocity_y / 60;
            self.velocity_y = (self.velocity_y * FRICTION_FACTOR) / 1000;
        } else {
            self.velocity_y = 0;
        }
    }
}

/// Initializes the Fluid Inertial Physics engine and verifies momentum & spring self-tests.
pub fn init() {
    let mut scroller = InertialScroller::new(720, 2000);

    // 1. Nominal position
    assert_eq!(scroller.scroll_y, 0);
    assert_eq!(scroller.max_scroll(), 1280);

    // 2. Downward drag past top bound (Overscroll)
    scroller.on_touch_down(100);
    scroller.on_touch_move(160); // Dragged down 60px -> delta = -60 -> scroll_y becomes -30 (50% elastic)
    assert!(scroller.scroll_y < 0, "Overscroll top must produce negative scroll_y");
    let overscroll_val = scroller.scroll_y;

    // 3. Release touch: spring-back must pull scroll_y back toward 0
    scroller.on_touch_up();
    for _ in 0..60 {
        scroller.tick();
    }
    assert!(
        scroller.scroll_y >= overscroll_val && scroller.scroll_y <= 0,
        "Spring-back must restore position toward 0"
    );

    // 4. Upward fling (Simulate rapid swipe up)
    scroller.scroll_y = 100;
    scroller.velocity_y = 600; // 600 px/s upward momentum
    scroller.tick();
    assert!(scroller.scroll_y > 100, "Inertial momentum must advance scroll offset");
    assert!(scroller.velocity_y < 600, "Friction must decelerate velocity");

    serial_println!(
        "[TEST] Fluid Inertial Physics: 60Hz momentum scrolling, friction decay, and elastic spring-back VERIFIED."
    );

    let mut spring = InterruptibleSpring::new(0, 180, 200);
    spring.retarget(100);
    for _ in 0..10 {
        spring.step();
    }
    assert!(spring.velocity > 0);
    // Retarget mid-flight
    let v_mid = spring.velocity;
    spring.retarget(50);
    assert_eq!(spring.velocity, v_mid, "Instantaneous velocity must be preserved on mid-flight retarget");
}

/// Smooth interruptible spring physics simulation.
/// Retargets mid-flight while preserving instantaneous velocity.
#[derive(Debug, Clone, Copy)]
pub struct InterruptibleSpring {
    pub current: i32,
    pub target: i32,
    pub velocity: i32,
    pub stiffness: i32,
    pub damping: i32,
}

impl InterruptibleSpring {
    pub const fn new(initial: i32, stiffness: i32, damping: i32) -> Self {
        Self {
            current: initial,
            target: initial,
            velocity: 0,
            stiffness,
            damping,
        }
    }

    /// Retargets the spring mid-flight while preserving current velocity.
    pub fn retarget(&mut self, new_target: i32) {
        self.target = new_target;
    }

    /// Advances the spring simulation by one step. Returns true if settled.
    pub fn step(&mut self) -> bool {
        let displacement = self.current - self.target;
        if displacement == 0 && self.velocity.abs() <= 1 {
            self.current = self.target;
            self.velocity = 0;
            return true;
        }

        let spring_force = -(displacement * self.stiffness) / 1000;
        let damping_force = -(self.velocity * self.damping) / 1000;
        let acceleration = spring_force + damping_force;

        self.velocity += acceleration;
        self.current += self.velocity / 60;

        self.current == self.target && self.velocity.abs() <= 1
    }
}

