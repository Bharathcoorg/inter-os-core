//! Activity Space: Sovereign Recents Tray & Interactive Multitasking Deck.
//!
//! Visualizes running applications with authentic miniature screen previews,
//! real-time process metadata, and responsive touch controls.
//! Fully compliant with the InterOS design system and zero heap allocation contract.

#![allow(dead_code)]

use crate::{test_text, test_text_aa, LauncherState};
use launcher::icons::{self, IconKind};
use launcher::layout::Rect;
use launcher::text;
use launcher::theme::{
    type_scale, SCREEN_HEIGHT, SCREEN_WIDTH,
    STATUS_BAR_HEIGHT,
};

// -----------------------------------------------------------------------------
// Constants & Colors: Flagship Dark Acrylic & Translucent Glass System
// -----------------------------------------------------------------------------

/// Deep OLED black / dark acrylic backdrop (#0B0F17)
pub const BG_COLOR: u32 = 0xFF0B0F17;

/// Card translucent dark glass surface
pub const CARD_BG: u32 = 0xFF141923;
/// Subtle translucent glass border
pub const CARD_BORDER: u32 = 0xFF2A3446;
/// Top edge specular reflection highlight for glass illusion
pub const CARD_BORDER_HIGHLIGHT: u32 = 0xFF3E4D64;

pub const TEXT_WHITE: u32 = 0xFFF8FAFC;
pub const TEXT_MUTED: u32 = 0xFF94A3B8;

// Squircle Icon Badge
pub const SQUIRCLE_SIZE: usize = 48;
pub const SQUIRCLE_RADIUS: usize = 14;
pub const SQUIRCLE_BG: u32 = 0xFF1C2433;
pub const SQUIRCLE_BORDER: u32 = 0xFF2F3D52;

// State Badge Colors (Platform Truth compliant: no forbidden traffic-light tones)
pub const STATE_ACTIVE_DOT: u32 = 0xFF22C55E; // Emerald green
pub const STATE_ACTIVE_TEXT: u32 = 0xFF4ADE80;
pub const STATE_RUNNING_DOT: u32 = 0xFF38BDF8; // Electric sky blue
pub const STATE_RUNNING_TEXT: u32 = 0xFF7DD3FC;
pub const STATE_RECENT_DOT: u32 = 0xFF64748B; // Muted silver
pub const STATE_RECENT_TEXT: u32 = 0xFF94A3B8;

pub const STATE_BADGE_BG: u32 = 0xFF18202D;
pub const STATE_BADGE_BORDER: u32 = 0xFF283446;

// Preview Well (Authentic Surface Canvas)
pub const PREVIEW_BG: u32 = 0xFF0D111A;
pub const PREVIEW_BORDER: u32 = 0xFF1E2636;
pub const PREVIEW_HEADER_BG: u32 = 0xFF121722;
pub const PREVIEW_HEADER_BORDER: u32 = 0xFF1A2230;
pub const PREVIEW_WATERMARK_INK: u32 = 0xFF334155;
pub const PREVIEW_NOTE_INK: u32 = 0xFF7C8BA1;
pub const PREVIEW_NOTE: &[u8] = b"No live preview";

// Elevated Dismiss Circle [×]
pub const BADGE_SIZE: usize = 48;
pub const BADGE_RADIUS: usize = BADGE_SIZE / 2;
pub const BADGE_X: usize = 586;
pub const CARD0_DISMISS: Rect = Rect::new(BADGE_X, 206, BADGE_SIZE, BADGE_SIZE);
pub const CARD1_DISMISS: Rect = Rect::new(BADGE_X, 526, BADGE_SIZE, BADGE_SIZE);
pub const CARD2_DISMISS: Rect = Rect::new(BADGE_X, 846, BADGE_SIZE, BADGE_SIZE);

pub const DISMISS_BG: u32 = 0xFF1F2838;
pub const DISMISS_BORDER: u32 = 0xFF43546D;
pub const DISMISS_HIGHLIGHT: u32 = 0xFF5B6E8C;
pub const DISMISS_SHADOW: u32 = 0xFF080B12;

// Modern Pill-Shaped "Close all" Control
pub const CLOSE_ALL_BTN: Rect = Rect::new(SCREEN_WIDTH / 2 - 100, 1220, 200, 48);
pub const CLOSE_ALL_BG: u32 = 0xFF18202D;
pub const CLOSE_ALL_BORDER: u32 = 0xFF364457;
pub const CLOSE_ALL_HIGHLIGHT: u32 = 0xFF4C5D76;

const HOME_PILL: Rect = Rect::new(SCREEN_WIDTH / 2 - 90, SCREEN_HEIGHT - 20, 180, 5);

// Card dimensions & geometry
const CARD_X: usize = 32;
const CARD_W: usize = SCREEN_WIDTH - 64; // 656
const CARD_RADIUS: usize = 28;

pub const WORKSPACE_CARDS: [Rect; 4] = [
    Rect::new(CARD_X, 120, CARD_W, 270),
    Rect::new(CARD_X, 420, CARD_W, 270),
    Rect::new(CARD_X, 720, CARD_W, 270),
    Rect::new(CARD_X, 1020, CARD_W, 180),
];

pub fn recents_band() -> (usize, usize) {
    let last = WORKSPACE_CARDS[2];
    (STATUS_BAND_TOP, last.y + last.h)
}

// Stub category definitions for backward compatibility
pub const ACTIVITY_NAMES: [&[u8]; 4] = [b"Work", b"Personal", b"Research", b"Media"];
pub fn activity_row_at(_x: usize, _y: usize) -> Option<usize> {
    None
}

// -----------------------------------------------------------------------------
// Status band
// -----------------------------------------------------------------------------

const STATUS_BAND_TOP: usize = STATUS_BAR_HEIGHT + 12;
const STATUS_BAND_H: usize = WORKSPACE_CARDS[0].y - STATUS_BAND_TOP;
const STATUS_CAP: usize = 48;

fn put(buf: &mut [u8], at: usize, src: &[u8]) -> usize {
    let mut end = at;
    for b in src {
        if end >= buf.len() {
            break;
        }
        buf[end] = *b;
        end += 1;
    }
    end
}

fn push_decimal(buf: &mut [u8], at: usize, value: usize) -> usize {
    let mut digits = [0u8; 20];
    let mut n = 0usize;
    let mut v = value;
    if v == 0 {
        digits[0] = b'0';
        n = 1;
    }
    while v > 0 && n < digits.len() {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let mut end = at;
    for i in (0..n).rev() {
        end = put(buf, end, &digits[i..i + 1]);
    }
    end
}

fn running_line(buf: &mut [u8; STATUS_CAP], n: usize) -> &[u8] {
    let mut at = push_decimal(buf, 0, n);
    at = put(buf, at, b" running - X closes an app");
    &buf[..at]
}

fn running_count(state: &LauncherState) -> usize {
    let mut n = 0usize;
    for task in state.recent_task_ids.iter() {
        if *task != 0 {
            n += 1;
        }
    }
    n
}

// -----------------------------------------------------------------------------
// Backdrop Frosted Glass Tint
// -----------------------------------------------------------------------------

#[inline(always)]
fn backdrop_pixel(x: usize, y: usize) -> u32 {
    if y < 320 {
        let cx = (SCREEN_WIDTH / 2) as i32;
        let dx = ((x as i32) - cx).abs();
        let glow = ((320 - y) * (360 - dx.min(360) as usize)) / 23040; // 0..5
        let r = 11 + (glow * 2 / 5) as u32;
        let g = 15 + (glow * 3 / 5) as u32;
        let b = 23 + glow as u32;
        0xFF000000 | (r << 16) | (g << 8) | b
    } else {
        BG_COLOR
    }
}

// -----------------------------------------------------------------------------
// Rounded Rectangle Helpers
// -----------------------------------------------------------------------------

#[inline]
fn rounded_contains(r: Rect, x: usize, y: usize, rad: usize) -> bool {
    if !r.contains(x, y) {
        return false;
    }
    let rad = rad.min(r.w / 2).min(r.h / 2);
    if rad == 0 {
        return true;
    }
    let left = r.x + rad;
    let right = r.x + r.w - 1 - rad;
    let top = r.y + rad;
    let bottom = r.y + r.h - 1 - rad;
    if (x >= left && x <= right) || (y >= top && y <= bottom) {
        return true;
    }
    let cx = if x < left { left } else { right };
    let cy = if y < top { top } else { bottom };
    let dx = (x as i64) - (cx as i64);
    let dy = (y as i64) - (cy as i64);
    let r2 = (rad as i64) * (rad as i64);
    let dx2 = dx * dx;
    let dy2 = dy * dy;
    // Continuous n=4 Lamé superellipse (flagship continuous squircle curvature)
    (dx2 * dx2) + (dy2 * dy2) <= r2 * r2
}

#[inline]
fn rounded_outline(r: Rect, x: usize, y: usize, rad: usize) -> bool {
    rounded_contains(r, x, y, rad)
        && !rounded_contains(r.inset(1), x, y, rad.saturating_sub(1))
}

// -----------------------------------------------------------------------------
// Text Run Handling
// -----------------------------------------------------------------------------

const RUN_CAP: usize = 96;

fn activity_run_metrics(y: usize, scale: usize) -> (u32, usize, usize) {
    let em = launcher::outline_text::em_for_scale(scale);
    let (asc, desc) = interos_std::design::typeface::line_metrics(em);
    let baseline = (y + launcher::font::BASELINE * scale).max(asc as usize);
    let top = baseline.saturating_sub(asc as usize);
    (em, top, (baseline + desc as usize).saturating_sub(top))
}

#[derive(Clone, Copy)]
struct LabelRun {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    scale: usize,
    len: usize,
    ink: u32,
    em: u32,
    top: usize,
    box_h: usize,
    bytes: [u8; RUN_CAP],
}

impl LabelRun {
    const EMPTY: LabelRun = LabelRun {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        scale: 1,
        len: 0,
        ink: 0,
        em: 0,
        top: 0,
        box_h: 0,
        bytes: [0u8; RUN_CAP],
    };

    fn new(x: usize, y: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        let (em, top, box_h) = activity_run_metrics(y, scale);
        let w = text::measure(&buf[..len], scale)
            .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize);
        LabelRun {
            x,
            y,
            w,
            h: text::line_height(scale),
            scale,
            len,
            ink,
            em,
            top,
            box_h,
            bytes: buf,
        }
    }

    fn fitted(x: usize, y: usize, scale: usize, fit: usize, bytes: &[u8], ink: u32) -> Self {
        let (buf, len) = text::ellipsize(bytes, scale, fit);
        let len = len.min(RUN_CAP);
        let (em, top, box_h) = activity_run_metrics(y, scale);
        LabelRun {
            x,
            y,
            w: text::measure(&buf[..len], scale)
                .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize),
            h: text::line_height(scale),
            scale,
            len,
            ink,
            em,
            top,
            box_h,
            bytes: buf,
        }
    }

    fn centred(x: usize, y: usize, w: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        let measured = text::measure(&buf[..len], scale);
        let inset = text::center_x(&buf[..len], scale, w);
        let (em, top, box_h) = activity_run_metrics(y, scale);
        LabelRun {
            x: x + inset,
            y,
            w: measured
                .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize),
            h: text::line_height(scale),
            scale,
            len,
            ink,
            em,
            top,
            box_h,
            bytes: buf,
        }
    }

    fn fitted_centred(
        x: usize,
        y: usize,
        scale: usize,
        fit: usize,
        centre: usize,
        bytes: &[u8],
        ink: u32,
    ) -> Self {
        let (buf, len) = text::ellipsize(bytes, scale, fit);
        let len = len.min(RUN_CAP);
        let inset = text::center_x(&buf[..len], scale, centre);
        let (em, top, box_h) = activity_run_metrics(y, scale);
        LabelRun {
            x: x + inset,
            y,
            w: text::measure(&buf[..len], scale)
                .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize),
            h: text::line_height(scale),
            scale,
            len,
            ink,
            em,
            top,
            box_h,
            bytes: buf,
        }
    }

    #[inline]
    fn pixel(&self, px: usize, py: usize) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if px < self.x || px >= self.x + self.w || py < self.top || py - self.top >= self.box_h {
            return None;
        }
        if self.scale >= 1 && self.len <= launcher::outline_text::KEY_MAX {
            let (asc, _) = interos_std::design::typeface::line_metrics(self.em);
            let baseline = (self.y + launcher::font::BASELINE * self.scale).max(asc as usize);
            if let Some(run) = launcher::outline_text::OutlineRun::place(
                self.x,
                baseline,
                self.em,
                &self.bytes[..self.len],
            ) {
                return run.pixel(px, py, self.ink, 0);
            }
        }
        test_text(
            px,
            py,
            self.x,
            self.y,
            self.scale,
            &self.bytes[..self.len],
            self.ink,
        )
    }

    #[inline]
    fn pixel_on(&self, px: usize, py: usize, bg: u32) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if px < self.x || px >= self.x + self.w || py < self.top || py - self.top >= self.box_h {
            return None;
        }
        if self.scale >= 1 && self.len <= launcher::outline_text::KEY_MAX {
            let (asc, _) = interos_std::design::typeface::line_metrics(self.em);
            let baseline = (self.y + launcher::font::BASELINE * self.scale).max(asc as usize);
            if let Some(run) = launcher::outline_text::OutlineRun::place(
                self.x,
                baseline,
                self.em,
                &self.bytes[..self.len],
            ) {
                return run.pixel(px, py, self.ink, bg);
            }
        }
        test_text_aa(
            px,
            py,
            self.x,
            self.y,
            self.scale,
            &self.bytes[..self.len],
            self.ink,
            bg,
        )
    }
}

// -----------------------------------------------------------------------------
// Card Plan & Layout
// -----------------------------------------------------------------------------

struct CardPlan {
    rect: Rect,
    icon_rect: Rect,
    icon: Option<IconKind>,
    badge: LabelRun,
    title: LabelRun,
    state_pill_rect: Rect,
    state_label: LabelRun,
    state_dot_color: u32,
    dismiss_rect: Option<Rect>,
    preview_rect: Rect,
    preview_note: LabelRun,
}

struct ActivityFrame {
    clock: LabelRun,
    status_head: LabelRun,
    status_body: LabelRun,
    cards: [Option<CardPlan>; 4],
    close_all_btn: Rect,
    close_all_label: LabelRun,
    home_pill: Rect,
}

const TITLE_CAP: usize = RUN_CAP;

fn registry_index(state: &LauncherState, id: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i < state.app_count {
        let len = (state.app_id_lens[i] as usize).min(state.app_ids[i].len());
        if len == id.len() && &state.app_ids[i][..len] == id {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn registry_badge(state: &LauncherState, index: usize) -> &[u8] {
    let mut len = 0usize;
    while len < state.app_icons[index].len() && state.app_icons[index][len] != b' ' {
        len += 1;
    }
    &state.app_icons[index][..len]
}

fn card_title_bytes<'a>(
    state: &'a LauncherState,
    id: &'a [u8],
    scratch: &'a mut [u8; TITLE_CAP],
) -> &'a [u8] {
    match registry_index(state, id) {
        Some(index) => {
            let len = (state.app_label_lens[index] as usize).min(state.app_labels[index].len());
            if len > 0 {
                &state.app_labels[index][..len]
            } else {
                text::humanise_package_id(id, scratch)
            }
        }
        None => text::humanise_package_id(id, scratch),
    }
}

fn dismiss_rect_for(slot: usize) -> Option<Rect> {
    match slot {
        0 => Some(CARD0_DISMISS),
        1 => Some(CARD1_DISMISS),
        2 => Some(CARD2_DISMISS),
        _ => None,
    }
}

fn build_card(state: &LauncherState, slot: usize) -> Option<CardPlan> {
    let rect = *WORKSPACE_CARDS.get(slot)?;
    let raw = state.recent_apps.get(slot)?;
    let len = (*state.recent_app_lens.get(slot)? as usize).min(raw.len());
    if len == 0 {
        return None;
    }
    let id = &raw[..len];

    // First-party HD vector icon inside a 48x48 squircle badge (radius 14px)
    let icon_rect = Rect::new(rect.x + 20, rect.y + 16, SQUIRCLE_SIZE, SQUIRCLE_SIZE);
    let title_x = rect.x + 80;
    let title_fit = rect.w.saturating_sub(80 + 120);

    let task = state.recent_task_ids.get(slot).copied().unwrap_or(0);
    let focused = task != 0 && state.active_child == Some(task);

    // State badge pill (e.g. "Active", "Running", "Recent") aligned cleanly on top-right before dismiss [x]
    let (state_text, dot_color, text_color, badge_w): (&[u8], u32, u32, usize) = if task != 0 && focused {
        (b"Active", STATE_ACTIVE_DOT, STATE_ACTIVE_TEXT, 70)
    } else if task != 0 {
        (b"Running", STATE_RUNNING_DOT, STATE_RUNNING_TEXT, 78)
    } else {
        (b"Recent", STATE_RECENT_DOT, STATE_RECENT_TEXT, 72)
    };
    let state_x = rect.x + rect.w - 64 - badge_w;
    let state_y = rect.y + 20;
    let state_pill_rect = Rect::new(state_x, state_y, badge_w, 24);
    let state_label = LabelRun::new(
        state_x + 18,
        state_y + 4,
        type_scale::CAPTION,
        state_text,
        text_color,
    );

    let index = registry_index(state, id);
    let badge_bytes: &[u8] = match index {
        Some(i) => registry_badge(state, i),
        None => &[],
    };
    let badge = if badge_bytes.is_empty() {
        LabelRun::EMPTY
    } else {
        LabelRun::fitted_centred(
            icon_rect.x,
            icon_rect.y
                + (icon_rect.h.saturating_sub(text::line_height(type_scale::CAPTION))) / 2,
            type_scale::CAPTION,
            icon_rect.w,
            icon_rect.w,
            badge_bytes,
            TEXT_WHITE,
        )
    };

    let mut title_scratch = [0u8; TITLE_CAP];
    let preview_rect = Rect::new(
        rect.x + 16,
        rect.y + 60,
        rect.w - 32,
        194,
    );
    let preview_note = LabelRun::fitted_centred(
        preview_rect.x,
        preview_rect.center_y() + 24,
        type_scale::CAPTION,
        preview_rect.w,
        preview_rect.w,
        PREVIEW_NOTE,
        PREVIEW_NOTE_INK,
    );

    Some(CardPlan {
        rect,
        icon_rect,
        icon: icons::match_icon_kind(id),
        badge,
        title: LabelRun::fitted(
            title_x,
            rect.y + 20,
            type_scale::BODY,
            state_x.saturating_sub(title_x + 12),
            card_title_bytes(state, id, &mut title_scratch),
            TEXT_WHITE,
        ),
        state_pill_rect,
        state_label,
        state_dot_color: dot_color,
        dismiss_rect: dismiss_rect_for(slot),
        preview_rect,
        preview_note,
    })
}

// -----------------------------------------------------------------------------
// Card Preview Renderer: Authentic Clean Application Canvas
// -----------------------------------------------------------------------------

fn render_mini_preview(card: &CardPlan, px: usize, py: usize) -> Option<u32> {
    if !rounded_contains(card.preview_rect, px, py, 14) {
        return None;
    }
    if rounded_outline(card.preview_rect, px, py, 14) {
        return Some(PREVIEW_BORDER);
    }

    // Surface canvas mini header strip at top of preview
    if py < card.preview_rect.y + 24 {
        if py == card.preview_rect.y + 23 {
            return Some(PREVIEW_HEADER_BORDER);
        }
        let cy = card.preview_rect.y as i32 + 11;
        for &dot_x in &[card.preview_rect.x + 16, card.preview_rect.x + 24, card.preview_rect.x + 32] {
            let dx = (px as i32) - dot_x as i32;
            let dy = (py as i32) - cy;
            if dx * dx + dy * dy <= 4 {
                return Some(0xFF2A3648);
            }
        }
        return Some(PREVIEW_HEADER_BG);
    }

    // Authentic platform note sits in lower half
    if let Some(c) = card.preview_note.pixel_on(px, py, PREVIEW_BG) {
        return Some(c);
    }

    // Authentic HD vector watermark icon centered in preview canvas
    let cx = card.preview_rect.center_x();
    let cy = card.preview_rect.center_y() - 12;
    if let Some(kind) = card.icon {
        if let Some(ic) = icons::rasterize_icon_pixel_scaled_aa(
            kind,
            px,
            py,
            cx,
            cy,
            PREVIEW_WATERMARK_INK,
            PREVIEW_BG,
            20,
        ) {
            return Some(ic);
        }
    }

    Some(PREVIEW_BG)
}

fn build_activity_frame(state: &LauncherState) -> ActivityFrame {
    // 1. Status Bar Clock
    let clock_slice: &[u8] = if state.clock_time[0] != b' ' {
        &state.clock_time
    } else {
        b""
    };
    let clock = LabelRun::new(48, 14, type_scale::CAPTION, clock_slice, TEXT_WHITE);

    // 2. Cards
    let cards = [
        build_card(state, 0),
        build_card(state, 1),
        build_card(state, 2),
        build_card(state, 3),
    ];

    // 3. Status band
    let running = running_count(state);
    let card_count = state.recent_app_lens.iter().filter(|&&len| len > 0).count();
    let head_scale = type_scale::CAPTION;
    let mut status_head_buf = [0u8; STATUS_CAP];
    let (status_head, status_body) = if card_count == 0 && running == 0 {
        (
            LabelRun::centred(
                CARD_X,
                STATUS_BAND_TOP,
                CARD_W,
                head_scale,
                b"No recent apps",
                TEXT_WHITE,
            ),
            LabelRun::centred(
                CARD_X,
                STATUS_BAND_TOP + text::line_height(head_scale),
                CARD_W,
                head_scale,
                b"Swipe up from any app to switch between tasks",
                TEXT_MUTED,
            ),
        )
    } else {
        let label = if running > 0 {
            running_line(&mut status_head_buf, running)
        } else {
            b"Recent Applications"
        };
        (
            LabelRun::centred(
                CARD_X,
                STATUS_BAND_TOP
                    + STATUS_BAND_H.saturating_sub(text::line_height(head_scale)) / 2,
                CARD_W,
                head_scale,
                label,
                TEXT_WHITE,
            ),
            LabelRun::EMPTY,
        )
    };

    // 4. Modern pill-shaped "Close all" button centered under deck
    let nav_label_scale = type_scale::BODY;
    let close_all_label = LabelRun::centred(
        CLOSE_ALL_BTN.x,
        CLOSE_ALL_BTN.y
            + (CLOSE_ALL_BTN.h.saturating_sub(text::line_height(nav_label_scale))) / 2,
        CLOSE_ALL_BTN.w,
        nav_label_scale,
        b"Close all",
        TEXT_WHITE,
    );

    ActivityFrame {
        clock,
        status_head,
        status_body,
        cards,
        close_all_btn: CLOSE_ALL_BTN,
        close_all_label,
        home_pill: HOME_PILL,
    }
}

// -----------------------------------------------------------------------------
// Per-Pixel Renderer
// -----------------------------------------------------------------------------

static mut FRAME_CACHE: Option<ActivityFrame> = None;

#[inline(never)]
pub fn get_activity_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let cached = unsafe {
        let slot: *mut Option<ActivityFrame> = core::ptr::addr_of_mut!(FRAME_CACHE);
        if (x == 0 && y == 0) || (*slot).is_none() {
            *slot = Some(build_activity_frame(state));
        }
        (*slot).as_ref()
    };
    let Some(f) = cached else {
        return BG_COLOR;
    };

    // 1. Status Bar Clock (y < 72)
    if y < STATUS_BAR_HEIGHT + 12 {
        if let Some(c) = f.clock.pixel_on(x, y, backdrop_pixel(x, y)) {
            return c;
        }
        return backdrop_pixel(x, y);
    }

    // 2. Status band (y in STATUS_BAND_TOP .. 120)
    if y < 120 {
        if y >= STATUS_BAND_TOP {
            if let Some(c) = f.status_head.pixel_on(x, y, backdrop_pixel(x, y)) {
                return c;
            }
            if let Some(c) = f.status_body.pixel_on(x, y, backdrop_pixel(x, y)) {
                return c;
            }
        }
        return backdrop_pixel(x, y);
    }

    // 3. Deck Cards (y in 120..990)
    if y < 990 {
        let card_opt = if y < 390 {
            f.cards[0].as_ref()
        } else if y >= 420 && y < 690 {
            f.cards[1].as_ref()
        } else if y >= 720 {
            f.cards[2].as_ref()
        } else {
            None
        };

        if let Some(card) = card_opt {
            if rounded_contains(card.rect, x, y, CARD_RADIUS) {
                // Subtle translucent glass border with top edge specular reflection
                if rounded_outline(card.rect, x, y, CARD_RADIUS) {
                    if y <= card.rect.y + 1 {
                        return CARD_BORDER_HIGHLIGHT;
                    }
                    return CARD_BORDER;
                }

                // Sleek elevated dismiss circle [×] at top-right of the card
                // Checked with priority so it cleanly floats above the preview well
                if let Some(dismiss_r) = card.dismiss_rect {
                    let cx = dismiss_r.center_x() as i32;
                    let cy = dismiss_r.center_y() as i32;
                    let dx = (x as i32) - cx;
                    let dy = (y as i32) - cy;
                    let dist2 = dx * dx + dy * dy;

                    // Outer drop-shadow halo for floating elevation
                    if dist2 <= 23 * 23 && dist2 > 19 * 19 {
                        return DISMISS_SHADOW;
                    }
                    if dist2 <= 19 * 19 {
                        // Glass circle rim with specular highlight on top
                        if dist2 >= 17 * 17 {
                            if dy < -4 {
                                return DISMISS_HIGHLIGHT;
                            }
                            return DISMISS_BORDER;
                        }
                        // Crisp white [×] cross glyph
                        if dx.abs() <= 5 && dy.abs() <= 5 && ((dx - dy).abs() <= 1 || (dx + dy).abs() <= 1) {
                            return 0xFFFFFFFF;
                        }
                        return DISMISS_BG;
                    }
                }

                // First-party HD vector icon inside squircle badge
                if rounded_contains(card.icon_rect, x, y, SQUIRCLE_RADIUS) {
                    if rounded_outline(card.icon_rect, x, y, SQUIRCLE_RADIUS) {
                        return SQUIRCLE_BORDER;
                    }
                    let cx = card.icon_rect.center_x();
                    let cy = card.icon_rect.center_y();
                    if let Some(kind) = card.icon {
                        if let Some(ic) = icons::rasterize_icon_pixel_scaled_aa(
                            kind,
                            x,
                            y,
                            cx,
                            cy,
                            0xFFFFFFFF,
                            SQUIRCLE_BG,
                            18,
                        ) {
                            return ic;
                        }
                    } else if let Some(c) = card.badge.pixel_on(x, y, SQUIRCLE_BG) {
                        return c;
                    }
                    return SQUIRCLE_BG;
                }

                // App Name Title in crisp anti-aliased font
                if let Some(c) = card.title.pixel_on(x, y, CARD_BG) {
                    return c;
                }

                // State Badge Pill ("Active", "Running", "Recent")
                if rounded_contains(card.state_pill_rect, x, y, 10) {
                    if rounded_outline(card.state_pill_rect, x, y, 10) {
                        return STATE_BADGE_BORDER;
                    }
                    // Status indicator dot
                    let dot_cx = card.state_pill_rect.x as i32 + 9;
                    let dot_cy = card.state_pill_rect.center_y() as i32;
                    let dot_dx = (x as i32) - dot_cx;
                    let dot_dy = (y as i32) - dot_cy;
                    if dot_dx * dot_dx + dot_dy * dot_dy <= 7 {
                        return card.state_dot_color;
                    }
                    if let Some(c) = card.state_label.pixel_on(x, y, STATE_BADGE_BG) {
                        return c;
                    }
                    return STATE_BADGE_BG;
                }

                // Mini preview well showing authentic preview canvas
                if let Some(c) = render_mini_preview(card, x, y) {
                    return c;
                }

                return CARD_BG;
            }
        }
        return backdrop_pixel(x, y);
    }

    // 4. Bottom Controls: Modern pill-shaped "Close all" control and home indicator
    if y >= 1220 && y < 1270 {
        if rounded_contains(f.close_all_btn, x, y, 24) {
            if rounded_outline(f.close_all_btn, x, y, 24) {
                if y <= f.close_all_btn.y + 1 {
                    return CLOSE_ALL_HIGHLIGHT;
                }
                return CLOSE_ALL_BORDER;
            }
            if let Some(c) = f.close_all_label.pixel_on(x, y, CLOSE_ALL_BG) {
                return c;
            }
            return CLOSE_ALL_BG;
        }
    } else if y >= 1410 {
        if rounded_contains(f.home_pill, x, y, 2) {
            return 0xFF475569;
        }
    }

    backdrop_pixel(x, y)
}

// -----------------------------------------------------------------------------
// Close control: hit test and termination
// -----------------------------------------------------------------------------

pub fn activity_close_hit(_state: &LauncherState, x: usize, y: usize) -> Option<usize> {
    if CARD0_DISMISS.contains(x, y) || ((120..390).contains(&y) && x >= 580 && x <= 670 && y <= 254) {
        Some(0)
    } else if CARD1_DISMISS.contains(x, y) || ((420..690).contains(&y) && x >= 580 && x <= 670 && y <= 574) {
        Some(1)
    } else if CARD2_DISMISS.contains(x, y) || ((720..990).contains(&y) && x >= 580 && x <= 670 && y <= 894) {
        Some(2)
    } else {
        None
    }
}

pub fn activity_close_slot(state: &mut LauncherState, slot: usize) -> Option<u64> {
    let task = *state.recent_task_ids.get(slot)?;
    if task != 0 {
        let _ = interos_std::app_stop(task);
        if state.active_child == Some(task) {
            state.active_child = None;
        }
    }
    state.dismiss_recent(slot);
    Some(task)
}

pub fn activity_close_at(state: &mut LauncherState, x: usize, y: usize) -> Option<u64> {
    let slot = activity_close_hit(state, x, y)?;
    activity_close_slot(state, slot)
}