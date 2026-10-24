//! The Recents multi-tasking app switcher screen: a horizontal card carousel
//! with snapshot previews, swipe gestures, and snap-to-card motion physics.
//!
//! # Architecture & Guarantees
//! - Horizontal carousel layout displaying running app snapshot cards side-by-side.
//! - Shows app icon, title, running/last-frame state, and close ("X") button to terminate.
//! - Smooth horizontal swipe gesture handling (touch down, move, up) with rubber-band
//!   edge resistance and snap-to-card physics using `interos_std::design::motion`.
//! - Strict `#![no_std]` and zero heap allocation in interactive rendering and event loops.
//! - Screen raster renderer `get_recents_color(x, y, state) -> u32` conforming to
//!   the InterOS launcher ScreenPixelFn contract.

#![allow(dead_code)]

use crate::LauncherState;
use launcher::icons::{self, IconKind};
use launcher::layout::Rect;
use launcher::recents::{self, Preview, MAX_CARDS};
use launcher::text::Label;
use launcher::theme::{radius, type_scale, Theme, SCREEN_WIDTH, STATUS_BAR_HEIGHT};
use interos_std::design::motion::{self, MotionClock, SpringConfig, SpringState};

/// The theme the recents surface is drawn with.
const THEME: Theme = launcher::theme::SHELL;

// ============================================================================
// Layout Constants
// ============================================================================

/// Width of an individual task card in the horizontal carousel.
pub const CARD_W: usize = 480;

/// Height of an individual task card.
pub const CARD_H: usize = 860;

/// Top vertical coordinate of the card carousel.
pub const CARD_Y: usize = 160;

/// Horizontal gap between adjacent cards.
pub const CARD_GAP: usize = 28;

/// Pitch (stride) between card center positions.
pub const PITCH: usize = CARD_W + CARD_GAP; // 508

/// Dimensions of the live preview snapshot inside the card.
pub const SNAPSHOT_W: usize = 360;
pub const SNAPSHOT_H: usize = 720;

/// Close ("X") touch target size (meets 44px platform minimum).
pub const CLOSE_BTN_SIZE: usize = 44;

/// Position of the "Clear All" / "Close All" bottom pill button.
pub const CLOSE_ALL_X: usize = 248;
pub const CLOSE_ALL_Y: usize = 1090;
pub const CLOSE_ALL_W: usize = 224;
pub const CLOSE_ALL_H: usize = 52;

// ============================================================================
// Hit Testing & Interaction Actions
// ============================================================================

/// Interaction action determined by hit testing a tap or gesture on Recents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecentsAction {
    /// No interactive element was touched.
    None,
    /// The user tapped the close ("X") button on card at `slot`.
    CloseTask { slot: usize, task_id: u64 },
    /// The user tapped card at `slot` to resume / focus that app.
    FocusTask { slot: usize, task_id: u64 },
    /// The user tapped the "Close All" / "Clear All" pill button.
    CloseAll,
    /// The user completed a swipe that changed the active card.
    Scrolled { target_index: usize },
}

// ============================================================================
// Carousel Motion Controller
// ============================================================================

/// Motion controller managing horizontal swipe gestures, scroll offsets,
/// rubber-band resistance, and snap-to-card physics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarouselMotion {
    /// Continuous scroll offset in pixels (0 corresponds to card 0 centered).
    pub scroll_offset: i32,
    /// Target snap position in pixels.
    pub target_offset: i32,
    /// The currently selected / focused card index.
    pub selected_index: usize,
    /// Touch X where current drag gesture started.
    pub drag_start_x: i32,
    /// Scroll offset at the start of current drag gesture.
    pub drag_start_offset: i32,
    /// Whether touch is currently down / dragging.
    pub is_dragging: bool,
    /// Spring state for physics-based snapping.
    pub spring: SpringState,
    /// Spring configuration (stiffness, damping, mass).
    pub spring_config: SpringConfig,
    /// Clock for easing-based animation fallback.
    pub clock: MotionClock,
}

impl CarouselMotion {
    /// Create a new carousel motion controller initialized at card 0.
    pub const fn new() -> Self {
        Self {
            scroll_offset: 0,
            target_offset: 0,
            selected_index: 0,
            drag_start_x: 0,
            drag_start_offset: 0,
            is_dragging: false,
            spring: SpringState::new(0),
            spring_config: SpringConfig::snappy(),
            clock: MotionClock::new(0, motion::NORMAL),
        }
    }

    /// Process touch down at screen coordinate (x, y).
    pub fn on_touch_down(&mut self, x: i32, _y: i32) {
        self.drag_start_x = x;
        self.drag_start_offset = self.scroll_offset;
        self.is_dragging = true;
        self.spring.velocity = 0;
    }

    /// Process touch movement during drag with rubber-band edge resistance.
    pub fn on_touch_move(&mut self, x: i32, _y: i32, card_count: usize) {
        if !self.is_dragging || card_count == 0 {
            return;
        }
        let dx = x - self.drag_start_x;
        let raw = self.drag_start_offset - dx;
        let max_offset = (card_count.saturating_sub(1) * PITCH) as i32;

        // Apply 3:1 rubber-band resistance when dragging beyond limits
        self.scroll_offset = if raw < 0 {
            raw / 3
        } else if raw > max_offset {
            max_offset + (raw - max_offset) / 3
        } else {
            raw
        };
        self.spring.position = self.scroll_offset;
        self.spring.sub_position = 0;
    }

    /// Process touch release, compute snap-to-card target index, and initiate spring physics.
    pub fn on_touch_up(&mut self, x: i32, _y: i32, card_count: usize) -> usize {
        self.is_dragging = false;
        if card_count == 0 {
            self.selected_index = 0;
            self.target_offset = 0;
            self.spring.snap_to(0);
            return 0;
        }

        let dx = x - self.drag_start_x;
        let max_idx = card_count.saturating_sub(1);
        let max_offset = (max_idx * PITCH) as i32;

        // Determine target card index based on swipe displacement / direction
        let target_idx = if dx < -50 && self.selected_index < max_idx {
            // Swiped left: advance to next card
            self.selected_index + 1
        } else if dx > 50 && self.selected_index > 0 {
            // Swiped right: move to previous card
            self.selected_index - 1
        } else {
            // Snap to nearest card based on current offset
            let nearest = (self.scroll_offset + (PITCH as i32 / 2)) / (PITCH as i32);
            (nearest.clamp(0, max_idx as i32)) as usize
        };

        self.selected_index = target_idx;
        self.target_offset = ((target_idx * PITCH) as i32).clamp(0, max_offset);

        // Impart initial velocity into spring based on gesture release speed
        let initial_velocity = (-dx * 20).clamp(-12000, 12000);
        self.spring.velocity = initial_velocity;
        self.clock.reset(0);
        target_idx
    }

    /// Advance physics spring simulation step by `dt_ms`. Returns true if settled.
    pub fn step_physics(&mut self, dt_ms: u32) -> bool {
        if self.is_dragging {
            return false;
        }
        let settled = self.spring_config.step(&mut self.spring, self.target_offset, dt_ms);
        self.scroll_offset = self.spring.position;
        settled
    }

    /// Snap directly to a specific card index without animation.
    pub fn snap_to_index(&mut self, index: usize, card_count: usize) {
        let max_idx = card_count.saturating_sub(1);
        let idx = index.min(max_idx);
        self.selected_index = idx;
        self.target_offset = (idx * PITCH) as i32;
        self.scroll_offset = self.target_offset;
        self.spring.snap_to(self.target_offset);
    }

    /// Return current card screen X coordinate for card at `index`.
    pub fn card_x(&self, index: usize) -> i32 {
        let base_x = ((SCREEN_WIDTH - CARD_W) / 2) as i32;
        base_x + (index as i32 * PITCH as i32) - self.scroll_offset
    }
}

/// Global motion instance for single-threaded bare-metal UI loop.
static mut CAROUSEL_MOTION: CarouselMotion = CarouselMotion::new();

/// Access the static carousel motion instance.
pub fn carousel_motion() -> &'static mut CarouselMotion {
    let ptr: *mut CarouselMotion = core::ptr::addr_of_mut!(CAROUSEL_MOTION);
    // SAFETY: Launcher is a single bare-metal task; the event loop is not re-entrant.
    unsafe { &mut *ptr }
}

// ============================================================================
// Geometry Helpers
// ============================================================================

/// Bounding rectangle for the "Clear All" pill button.
pub fn close_all_rect() -> Rect {
    Rect::new(CLOSE_ALL_X, CLOSE_ALL_Y, CLOSE_ALL_W, CLOSE_ALL_H)
}

/// Close button rectangle for card `index` given screen card X.
pub fn card_close_button_rect(card_x: i32) -> Rect {
    let x = (card_x + CARD_W as i32 - 58).max(0) as usize;
    Rect::new(x, CARD_Y + 16, CLOSE_BTN_SIZE, CLOSE_BTN_SIZE)
}

/// Hit test a tap at (x, y) against the recents screen.
pub fn hit_test_recents(
    x: usize,
    y: usize,
    state: &LauncherState,
    motion: &CarouselMotion,
) -> RecentsAction {
    if state.recents_count == 0 {
        return RecentsAction::None;
    }

    // 1. Check "Clear All" pill button
    if close_all_rect().contains(x, y) {
        return RecentsAction::CloseAll;
    }

    let px = x as i32;
    let py = y as i32;

    // 2. Check cards in the carousel (front-to-back or in order)
    for slot in 0..state.recents_count.min(MAX_CARDS) {
        let card_x = motion.card_x(slot);
        let card_right = card_x + CARD_W as i32;

        if px < card_x || px >= card_right || py < CARD_Y as i32 || py >= (CARD_Y + CARD_H) as i32 {
            continue;
        }

        // Inside card: test close ("X") button first
        let close_x = card_x + CARD_W as i32 - 58;
        let close_y = CARD_Y as i32 + 16;
        if px >= close_x && px < close_x + (CLOSE_BTN_SIZE as i32) &&
           py >= close_y && py < close_y + (CLOSE_BTN_SIZE as i32)
        {
            let task_id = state.recents_cards[slot].task_id;
            return RecentsAction::CloseTask { slot, task_id };
        }

        // Otherwise: tapping the card body resumes / focuses that task
        let task_id = state.recents_cards[slot].task_id;
        return RecentsAction::FocusTask { slot, task_id };
    }

    RecentsAction::None
}

// ============================================================================
// Render Plan & Frame Cache
// ============================================================================

/// Resolved card layout data for the active frame.
#[derive(Clone, Copy)]
struct CardPlan {
    card_x: i32,
    picture_slot: Option<usize>,
    icon: Option<IconKind>,
    badge: [u8; 4],
    badge_len: usize,
    name: [u8; 24],
    name_len: usize,
    state_label: [u8; 16],
    state_len: usize,
    reason: [u8; 32],
    reason_len: usize,
}

/// Resolved frame plan for the recents switcher.
struct Plan {
    clock: Label,
    title: Label,
    count_buf: [u8; 32],
    count_len: usize,
    cards: [CardPlan; MAX_CARDS],
    card_count: usize,
    scroll_offset: i32,
    selected_index: usize,
    close_all: Rect,
    close_all_label: Label,
    empty_title: Label,
    empty_hint: Label,
    cached_fingerprint: u64,
}

static mut PLAN: Option<Plan> = None;

/// Invalidate cached plan so the next frame is recomputed.
pub fn invalidate_recents_frame() {
    let slot: *mut Option<Plan> = core::ptr::addr_of_mut!(PLAN);
    // SAFETY: Launcher is single bare-metal task; UI loop is non-reentrant.
    unsafe { *slot = None };
}

/// Compute state fingerprint for frame caching.
fn fingerprint(state: &LauncherState, scroll_offset: i32) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    hash ^= state.recents_count as u64;
    hash = hash.wrapping_mul(0x1000_0000_01b3);
    hash ^= (scroll_offset as u64) & 0xffff_ffff;
    hash = hash.wrapping_mul(0x1000_0000_01b3);
    for i in 0..state.recents_count.min(MAX_CARDS) {
        let card = &state.recents_cards[i];
        for &b in card.package() {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
        hash ^= card.task_id;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// Build plan for the current frame.
fn build_plan(state: &LauncherState, motion: &CarouselMotion, fp: u64) -> Plan {
    let theme = THEME;

    // Header labels
    let clock = Label::at(
        crate::screens::status::CLOCK_X,
        crate::screens::status::CLOCK_Y,
        crate::screens::status::CLOCK_SCALE,
        if state.clock_source_ok { &state.clock_time } else { b"" },
        theme.on_surface,
    );
    let title = Label::at(32, 64, type_scale::TITLE, b"Recent Apps", theme.on_surface);

    // Count label: e.g. "2 active tasks"
    let mut count_buf = [0u8; 32];
    let count_len = format_count(state.recents_count, &mut count_buf);

    let mut cards = [CardPlan {
        card_x: 0,
        picture_slot: None,
        icon: None,
        badge: [0; 4],
        badge_len: 0,
        name: [0; 24],
        name_len: 0,
        state_label: [0; 16],
        state_len: 0,
        reason: [0; 32],
        reason_len: 0,
    }; MAX_CARDS];

    let count = state.recents_count.min(MAX_CARDS);
    for index in 0..count {
        let card = &state.recents_cards[index];
        let card_x = motion.card_x(index);
        let icon = icons::match_icon_kind(card.package());

        // Name
        let mut name = [0u8; 24];
        let nlen = card.label().len().min(24);
        name[..nlen].copy_from_slice(&card.label()[..nlen]);

        // State label
        let mut state_label = [0u8; 16];
        let state_bytes: &[u8] = match card.preview {
            Preview::Live { .. } => if card.focused { b"Active" } else { b"Running" },
            Preview::LastFrame { .. } => b"Last frame",
            Preview::Unavailable(_) => b"Stopped",
        };
        let slen = state_bytes.len().min(16);
        state_label[..slen].copy_from_slice(&state_bytes[..slen]);

        // Reason (if unavailable)
        let mut reason = [0u8; 32];
        let rlen = if let Some(r) = card.preview.reason() {
            let b = r.as_bytes();
            let l = b.len().min(32);
            reason[..l].copy_from_slice(&b[..l]);
            l
        } else {
            0
        };

        // Fallback badge acronym if no icon
        let mut badge = [0u8; 4];
        let mut badge_len = 0;
        if icon.is_none() {
            let mut at = 0;
            let mut in_word = false;
            for &b in card.package() {
                let is_sep = b == b'.' || b == b'_' || b == b'-';
                if !in_word && !is_sep && at < 3 {
                    badge[at] = b.to_ascii_uppercase();
                    at += 1;
                }
                in_word = !is_sep;
            }
            badge_len = at;
        }

        cards[index] = CardPlan {
            card_x,
            picture_slot: crate::screens::activity::picture_slot(card.package()),
            icon,
            badge,
            badge_len,
            name,
            name_len: nlen,
            state_label,
            state_len: slen,
            reason,
            reason_len: rlen,
        };
    }

    let close_all = close_all_rect();
    let close_all_label = Label::at(
        close_all.x + 60,
        close_all.y + 16,
        type_scale::BODY,
        b"Clear All",
        theme.on_surface,
    );

    let empty_title = Label::at(
        SCREEN_WIDTH / 2 - 110,
        600,
        type_scale::TITLE,
        b"No Recent Apps",
        theme.on_surface,
    );
    let empty_hint = Label::at(
        SCREEN_WIDTH / 2 - 140,
        648,
        type_scale::CAPTION,
        b"Apps you open will appear here",
        theme.on_surface_variant,
    );

    Plan {
        clock,
        title,
        count_buf,
        count_len,
        cards,
        card_count: count,
        scroll_offset: motion.scroll_offset,
        selected_index: motion.selected_index,
        close_all,
        close_all_label,
        empty_title,
        empty_hint,
        cached_fingerprint: fp,
    }
}

fn format_count(count: usize, buf: &mut [u8; 32]) -> usize {
    if count == 0 {
        let text = b"No active tasks";
        buf[..text.len()].copy_from_slice(text);
        return text.len();
    }
    let mut at = 0;
    if count < 10 {
        buf[at] = b'0' + count as u8;
        at += 1;
    } else {
        buf[at] = b'0' + (count / 10) as u8;
        at += 1;
        buf[at] = b'0' + (count % 10) as u8;
        at += 1;
    }
    let suffix: &[u8] = if count == 1 { b" active task" } else { b" active tasks" };
    for &b in suffix {
        if at < buf.len() {
            buf[at] = b;
            at += 1;
        }
    }
    at
}

fn get_plan(x: usize, y: usize, state: &LauncherState, motion: &CarouselMotion) -> &'static Plan {
    let slot: *mut Option<Plan> = core::ptr::addr_of_mut!(PLAN);
    let fp = fingerprint(state, motion.scroll_offset);
    let should_rebuild = unsafe {
        match (*slot).as_ref() {
            None => true,
            Some(p) => (x == 0 && y == 0) || p.cached_fingerprint != fp,
        }
    };
    if should_rebuild {
        unsafe { *slot = Some(build_plan(state, motion, fp)) };
    }
    unsafe { (*slot).as_ref() }.expect("Recents plan must be initialized")
}

// ============================================================================
// Drawing & Pixel Rasterization
// ============================================================================

/// Check if point (px, py) is inside rounded rectangle.
fn inside_rounded_rect(px: i32, py: i32, rx: i32, ry: i32, rw: i32, rh: i32, rad: i32) -> bool {
    if px < rx || px >= rx + rw || py < ry || py >= ry + rh {
        return false;
    }
    let r = rad.min(rw / 2).min(rh / 2);
    if r <= 0 {
        return true;
    }
    let left = rx + r;
    let right = rx + rw - r;
    let top = ry + r;
    let bottom = ry + rh - r;

    let cx = if px < left { left } else if px >= right { right } else { px };
    let cy = if py < top { top } else if py >= bottom { bottom } else { py };

    let dx = px - cx;
    let dy = py - cy;
    (dx * dx + dy * dy) <= r * r
}

/// Render the circular close ("X") button on a card.
fn render_close_btn(px: i32, py: i32, card_x: i32, theme: &Theme) -> Option<u32> {
    let btn_x = card_x + CARD_W as i32 - 58;
    let btn_y = CARD_Y as i32 + 16;
    if px < btn_x || px >= btn_x + CLOSE_BTN_SIZE as i32 ||
       py < btn_y || py >= btn_y + CLOSE_BTN_SIZE as i32
    {
        return None;
    }

    let cx = btn_x + (CLOSE_BTN_SIZE as i32) / 2;
    let cy = btn_y + (CLOSE_BTN_SIZE as i32) / 2;
    let dx = px - cx;
    let dy = py - cy;
    let dist_sq = dx * dx + dy * dy;

    // Outer circle
    if dist_sq <= 256 {
        // Cross lines "X"
        if ((dx - dy).abs() <= 1 && dx.abs() <= 6) || ((dx + dy).abs() <= 1 && dx.abs() <= 6) {
            return Some(theme.on_surface);
        }
        return Some(theme.surface_container_high);
    }
    None
}

/// Render a single pixel of the Recents multi-tasking switcher screen.
pub fn get_recents_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let motion = carousel_motion();
    let plan = get_plan(x, y, state, motion);
    let theme = THEME;
    let px = x as i32;
    let py = y as i32;

    // 1. Status Bar & Clock
    if y < STATUS_BAR_HEIGHT {
        return crate::screens::status::pixel(x, y, Some(&plan.clock), &theme);
    }

    // 2. Header: Title & Count
    if let Some(c) = plan.title.pixel(x, y, theme.surface) {
        return c;
    }
    if py >= 112 && py < 136 && px >= 32 && px < 32 + (plan.count_len * 12) as i32 {
        if let Some(c) = crate::test_text_aa(
            x,
            y,
            32,
            112,
            type_scale::CAPTION,
            &plan.count_buf[..plan.count_len],
            theme.on_surface_variant,
            theme.surface,
        ) {
            return c;
        }
    }

    // 3. Horizontal Card Carousel
    for index in 0..plan.card_count {
        let card = &plan.cards[index];
        let card_x = card.card_x;
        let card_y = CARD_Y as i32;
        let card_w = CARD_W as i32;
        let card_h = CARD_H as i32;

        // Skip off-screen cards
        if px < card_x || px >= card_x + card_w || py < card_y || py >= card_y + card_h {
            continue;
        }

        // Card rounded boundary check (radius 24)
        if !inside_rounded_rect(px, py, card_x, card_y, card_w, card_h, radius::CARD as i32) {
            continue;
        }

        // 1px Border check
        if !inside_rounded_rect(px, py, card_x + 1, card_y + 1, card_w - 2, card_h - 2, (radius::CARD - 1) as i32) {
            return theme.border;
        }

        let card_fill = theme.surface_container;

        // A. Close button
        if let Some(c) = render_close_btn(px, py, card_x, &theme) {
            return c;
        }

        // B. App Icon / Acronym Badge
        let icon_cx = (card_x + 44) as usize;
        let icon_cy = (card_y + 44) as usize;
        if let Some(kind) = card.icon {
            if let Some(c) = icons::rasterize_icon_pixel_scaled(kind, x, y, icon_cx, icon_cy, theme.on_surface, 22) {
                return c;
            }
        } else if card.badge_len > 0 && px >= (card_x + 22) && px < (card_x + 66) && py >= (card_y + 22) && py < (card_y + 66) {
            if let Some(c) = crate::test_text_aa(
                x,
                y,
                (card_x + 28) as usize,
                (card_y + 32) as usize,
                type_scale::CAPTION,
                &card.badge[..card.badge_len],
                theme.on_surface,
                card_fill,
            ) {
                return c;
            }
        }

        // C. Title label
        if card.name_len > 0 && px >= (card_x + 78) && px < (card_x + 380) && py >= (card_y + 24) && py < (card_y + 50) {
            if let Some(c) = crate::test_text_aa(
                x,
                y,
                (card_x + 78) as usize,
                (card_y + 24) as usize,
                type_scale::BODY,
                &card.name[..card.name_len],
                theme.on_surface,
                card_fill,
            ) {
                return c;
            }
        }

        // D. Subtitle (state: "Active" / "Running" / "Last frame")
        if card.state_len > 0 && px >= (card_x + 78) && px < (card_x + 380) && py >= (card_y + 52) && py < (card_y + 74) {
            if let Some(c) = crate::test_text_aa(
                x,
                y,
                (card_x + 78) as usize,
                (card_y + 52) as usize,
                type_scale::CAPTION,
                &card.state_label[..card.state_len],
                theme.on_surface_variant,
                card_fill,
            ) {
                return c;
            }
        }

        // E. Live Snapshot Preview Well
        let snap_x = card_x + 20 + ((card_w - 40 - SNAPSHOT_W as i32) / 2);
        let snap_y = card_y + 84 + ((card_h - 104 - SNAPSHOT_H as i32) / 2);
        let snap_w = SNAPSHOT_W as i32;
        let snap_h = SNAPSHOT_H as i32;

        if px >= snap_x && px < snap_x + snap_w && py >= snap_y && py < snap_y + snap_h {
            if inside_rounded_rect(px, py, snap_x, snap_y, snap_w, snap_h, 16) {
                if let Some(slot) = card.picture_slot {
                    let src_x = (((px - snap_x) * recents::PREVIEW_W as i32) / snap_w) as usize;
                    let src_y = (((py - snap_y) * recents::PREVIEW_H as i32) / snap_h) as usize;
                    if let Some(c) = crate::screens::activity::picture_pixel(slot, src_x, src_y) {
                        return c;
                    }
                }

                // Placeholder well if no real snapshot frame
                let well_bg = theme.surface_container_low;
                if card.reason_len > 0 && py >= snap_y + snap_h / 2 - 12 && py < snap_y + snap_h / 2 + 16 {
                    let rx = (snap_x + 24) as usize;
                    let ry = (snap_y + snap_h / 2) as usize;
                    if let Some(c) = crate::test_text_aa(
                        x,
                        y,
                        rx,
                        ry,
                        type_scale::CAPTION,
                        &card.reason[..card.reason_len],
                        theme.on_surface_variant,
                        well_bg,
                    ) {
                        return c;
                    }
                }
                return well_bg;
            }
        }

        return card_fill;
    }

    // 4. Empty State
    if plan.card_count == 0 {
        if let Some(c) = plan.empty_title.pixel(x, y, theme.surface) {
            return c;
        }
        if let Some(c) = plan.empty_hint.pixel(x, y, theme.surface) {
            return c;
        }
    }

    // 5. Page Indicator Dots (below carousel at y = 1046)
    if plan.card_count > 1 && py >= 1040 && py <= 1052 {
        let total_w = (plan.card_count - 1) * 24;
        let start_x = (SCREEN_WIDTH / 2) as i32 - (total_w as i32 / 2);
        for i in 0..plan.card_count {
            let dot_x = start_x + (i as i32 * 24);
            let dot_y = 1046i32;
            let dx = px - dot_x;
            let dy = py - dot_y;
            let dist_sq = dx * dx + dy * dy;

            if i == plan.selected_index {
                // Active dot: pill or larger circle
                if dist_sq <= 25 {
                    return theme.primary;
                }
            } else {
                // Inactive dot
                if dist_sq <= 12 {
                    return theme.outline_variant;
                }
            }
        }
    }

    // 6. Bottom "Clear All" Pill Button
    if plan.card_count > 0 && inside_rounded_rect(px, py, CLOSE_ALL_X as i32, CLOSE_ALL_Y as i32, CLOSE_ALL_W as i32, CLOSE_ALL_H as i32, 26) {
        let pill_fill = theme.surface_container_high;
        if let Some(c) = plan.close_all_label.pixel(x, y, pill_fill) {
            return c;
        }
        return pill_fill;
    }

    theme.surface
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carousel_geometry_centering_and_pitch() {
        assert_eq!(CARD_W, 480);
        assert_eq!(CARD_H, 860);
        assert_eq!(PITCH, 508);

        let motion = CarouselMotion::new();
        // At offset 0, Card 0 must be centered on the 720px wide screen
        let card0_x = motion.card_x(0);
        assert_eq!(card0_x, (720 - 480) / 2); // 120
        let card0_center = card0_x + (CARD_W as i32 / 2);
        assert_eq!(card0_center, 360);

        // Card 1 is at card0_x + PITCH
        let card1_x = motion.card_x(1);
        assert_eq!(card1_x, 120 + 508); // 628
        // Card 1 peeks onto the screen by 720 - 628 = 92 px
        assert!(card1_x < 720);
    }

    #[test]
    fn carousel_drag_and_rubberbanding() {
        let mut motion = CarouselMotion::new();
        motion.on_touch_down(300, 500);
        assert!(motion.is_dragging);

        // Drag left by 100px (x moves 300 -> 200)
        motion.on_touch_move(200, 500, 3);
        assert_eq!(motion.scroll_offset, 100);

        // Drag beyond left bound (x moves to 400 => raw offset -100)
        motion.on_touch_move(400, 500, 3);
        // Rubber-band resistance: raw / 3 = -100 / 3 = -33
        assert_eq!(motion.scroll_offset, -33);
    }

    #[test]
    fn carousel_swipe_snap_to_card() {
        let mut motion = CarouselMotion::new();
        motion.on_touch_down(400, 500);

        // Swipe left by 120px (x from 400 to 280)
        motion.on_touch_move(280, 500, 3);
        let next_idx = motion.on_touch_up(280, 500, 3);
        assert_eq!(next_idx, 1);
        assert_eq!(motion.selected_index, 1);
        assert_eq!(motion.target_offset, PITCH as i32);
        assert!(!motion.is_dragging);

        // Step spring physics towards target
        let mut steps = 0;
        while !motion.step_physics(16) && steps < 100 {
            steps += 1;
        }
        assert_eq!(motion.scroll_offset, PITCH as i32);
    }

    #[test]
    fn hit_testing_actions() {
        let mut state = LauncherState::new();
        state.recents_count = 2;
        state.recents_cards[0].task_id = 42;
        state.recents_cards[1].task_id = 99;

        let motion = CarouselMotion::new();

        // 1. Hit test "Clear All" button
        let action = hit_test_recents(CLOSE_ALL_X + 20, CLOSE_ALL_Y + 10, &state, &motion);
        assert_eq!(action, RecentsAction::CloseAll);

        // 2. Hit test close button on Card 0
        let card0_x = motion.card_x(0);
        let close_rect = card_close_button_rect(card0_x);
        let action = hit_test_recents(close_rect.x + 5, close_rect.y + 5, &state, &motion);
        assert_eq!(action, RecentsAction::CloseTask { slot: 0, task_id: 42 });

        // 3. Hit test body of Card 0 (focus)
        let action = hit_test_recents(card0_x as usize + 100, (CARD_Y + 200), &state, &motion);
        assert_eq!(action, RecentsAction::FocusTask { slot: 0, task_id: 42 });

        // 4. Hit test background
        let action = hit_test_recents(10, 100, &state, &motion);
        assert_eq!(action, RecentsAction::None);
    }

    #[test]
    fn empty_state_safe() {
        let state = LauncherState::new();
        assert_eq!(state.recents_count, 0);

        let mut motion = CarouselMotion::new();
        motion.on_touch_down(300, 500);
        motion.on_touch_move(200, 500, 0);
        let target = motion.on_touch_up(200, 500, 0);
        assert_eq!(target, 0);

        let action = hit_test_recents(360, 500, &state, &motion);
        assert_eq!(action, RecentsAction::None);

        // Color raster test
        let color = get_recents_color(360, 600, &state);
        assert_ne!(color, 0);
    }
}
