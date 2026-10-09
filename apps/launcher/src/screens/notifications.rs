//! Notifications and Events Shade (Reference Screen 5).
//!
//! Renders the canonical "Events / Notifications done right" screen from system specifications:
//! - Crisp light theme background with status bar clock, signals, and battery.
//! - Header: "Events" with right-side filter control.
//! - Filter pills: "All" (active), "Messages", "System", "Apps".
//! - "Today" section with reference notifications:
//!   - "New message" (Rama, 2 min ago, green chat bubble)
//!   - "Meeting reminder" (Team sync at 11:00 AM, 1 hr ago, red calendar)
//!   - "Download complete" (interos-sdk.zip, 2 hr ago, teal download)
//!   - "Photo saved" (IMG_20260924.jpg, 3 hr ago, amber gallery)
//! - "Yesterday" section:
//!   - "App updated" (Browser, 10:30 PM, blue browser/update)
//! - "Clear all" action pill and bottom swipe-home indicator.
//! - Dynamic integration with kernel notification subsystem when real alerts arrive.

#![allow(dead_code)]

use crate::{test_text, LauncherState};
use launcher::icons::{self, IconKind};
use launcher::layout::Rect;
use launcher::text;
use launcher::theme::{type_scale, SCREEN_HEIGHT, SCREEN_WIDTH, STATUS_BAR_HEIGHT};

// -----------------------------------------------------------------------------
// Constants & Colors
// -----------------------------------------------------------------------------

const BG_COLOR: u32 = 0xFFF7F6F2; // Canonical warm cream background
const CARD_BG: u32 = 0xFFFFFFFF; // Pure white surface
const CARD_BORDER: u32 = 0xFFE1E0DB; // Canonical border
const TEXT_MAIN: u32 = 0xFF111214; // Canonical near-black primary
const TEXT_MUTED: u32 = 0xFF777773; // Canonical muted secondary
const TEXT_HINT: u32 = 0xFF9CA3AF; // Muted hint

const PILL_ACTIVE_BG: u32 = 0xFF111214;
const PILL_ACTIVE_TEXT: u32 = 0xFFFFFFFF;
const PILL_INACTIVE_BG: u32 = 0xFFEFEEE9;
const PILL_INACTIVE_TEXT: u32 = 0xFF777773;

const CARD_X: usize = 32;
const CARD_W: usize = SCREEN_WIDTH - 64; // 656

// Clear-all pill, drawn below the last event card: x in 260..460, y in 708..752.
// WHY this y: the event cards occupy 206..700 (five 94px cards on a 100px
// pitch), so 708..752 is the first band that both clears the cards and contains
// the pinned clear tap at (360, 720). The shell hit-tests this exact rect.
pub const CLEAR_BTN: Rect = Rect::new(SCREEN_WIDTH / 2 - 100, 708, 200, 44);
const HOME_PILL: Rect = Rect::new(SCREEN_WIDTH / 2 - 90, SCREEN_HEIGHT - 20, 180, 5);

// -----------------------------------------------------------------------------
// Rounded Geometry Helpers
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
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    dx * dx + dy * dy <= (rad as i32) * (rad as i32)
}

#[inline]
fn rounded_outline(r: Rect, x: usize, y: usize, rad: usize) -> bool {
    rounded_contains(r, x, y, rad)
        && !rounded_contains(r.inset(1), x, y, rad.saturating_sub(1))
}

// -----------------------------------------------------------------------------
// Typography Helpers
// -----------------------------------------------------------------------------

const RUN_CAP: usize = 64;

#[derive(Clone, Copy)]
struct LabelRun {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    scale: usize,
    len: usize,
    ink: u32,
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
        bytes: [0u8; RUN_CAP],
    };

    fn new(x: usize, y: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        let w = text::measure(&buf[..len], scale);
        let h = text::line_height(scale);
        LabelRun {
            x,
            y,
            w,
            h,
            scale,
            len,
            ink,
            bytes: buf,
        }
    }

    fn centred(x: usize, y: usize, w: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        let measured = text::measure(&buf[..len], scale);
        let inset = text::center_x(&buf[..len], scale, w);
        LabelRun {
            x: x + inset,
            y,
            w: measured,
            h: text::line_height(scale),
            scale,
            len,
            ink,
            bytes: buf,
        }
    }

    #[inline]
    fn pixel(&self, px: usize, py: usize) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if px < self.x || px >= self.x + self.w || py < self.y || py >= self.y + self.h {
            return None;
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
}

// -----------------------------------------------------------------------------
// Notification Item Plan
// -----------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct EventCardPlan {
    rect: Rect,
    icon_rect: Rect,
    icon_bg: u32,
    icon_tint: u32,
    icon_kind: IconKind,
    title: LabelRun,
    subtitle: LabelRun,
    time: LabelRun,
}

#[derive(Clone, Copy)]
struct FilterPillPlan {
    rect: Rect,
    bg: u32,
    label: LabelRun,
}

struct NotificationsFrame {
    clock: LabelRun,
    title: LabelRun,
    today_label: LabelRun,
    yesterday_label: LabelRun,
    pills: [FilterPillPlan; 4],
    cards: [Option<EventCardPlan>; 5],
    card_count: usize,
    clear_btn: Rect,
    clear_label: LabelRun,
    home_pill: Rect,
}

fn build_notifications_frame(state: &LauncherState) -> NotificationsFrame {
    let clock_slice: &[u8] = if state.clock_source_ok && state.clock_time[0] != b' ' { &state.clock_time } else { b"" };
    let clock = LabelRun::new(48, 14, type_scale::CAPTION, clock_slice, TEXT_MAIN);
    let title = LabelRun::new(36, 64, type_scale::BODY, b"Notifications", TEXT_MAIN);

    let pill_y = 124;
    let pill_h = 36;
    let pills = [
        FilterPillPlan { rect: Rect::new(36, pill_y, 68, pill_h), bg: PILL_ACTIVE_BG,
            label: LabelRun::centred(36, pill_y + 6, 68, type_scale::CAPTION, b"All", PILL_ACTIVE_TEXT) },
        FilterPillPlan { rect: Rect::new(116, pill_y, 116, pill_h), bg: PILL_INACTIVE_BG,
            label: LabelRun::centred(116, pill_y + 6, 116, type_scale::CAPTION, b"Messages", PILL_INACTIVE_TEXT) },
        FilterPillPlan { rect: Rect::new(244, pill_y, 96, pill_h), bg: PILL_INACTIVE_BG,
            label: LabelRun::centred(244, pill_y + 6, 96, type_scale::CAPTION, b"System", PILL_INACTIVE_TEXT) },
        FilterPillPlan { rect: Rect::new(352, pill_y, 80, pill_h), bg: PILL_INACTIVE_BG,
            label: LabelRun::centred(352, pill_y + 6, 80, type_scale::CAPTION, b"Apps", PILL_INACTIVE_TEXT) },
    ];

    let count = state.notification_count.min(state.notifications.len()).min(5);
    let today_label = if count > 0 {
        LabelRun::new(36, 178, type_scale::CAPTION, b"Recent", TEXT_MAIN)
    } else {
        LabelRun::new(36, 210, type_scale::BODY, b"No notifications", TEXT_MUTED)
    };
    let yesterday_label = LabelRun::new(36, 650, type_scale::CAPTION, b"", TEXT_MAIN);

    let mut cards: [Option<EventCardPlan>; 5] = [None, None, None, None, None];
    for i in 0..count {
        let rec = state.notifications[i];
        // WHY a 100px pitch: five 94px cards starting at 206 end at 700, leaving
        // the drawn clear-all pill at 708..752 clear of the last card.
        let y = 206 + i * 100;
        let title_bytes: &[u8] = if rec.title_len > 0 { &rec.title[..rec.title_len as usize] } else { b"Notification" };
        let subtitle_bytes: &[u8] = if rec.body_len > 0 {
            &rec.body[..rec.body_len as usize]
        } else if rec.pkg_len > 0 {
            &rec.package_id[..rec.pkg_len as usize]
        } else if rec.chan_len > 0 {
            &rec.channel[..rec.chan_len as usize]
        } else {
            b"System event"
        };
        let (icon_kind, icon_bg, icon_tint) = match i % 4 {
            0 => (IconKind::Messages, 0xFFDCFCE7, 0xFF16A34A),
            1 => (IconKind::Calendar, 0xFFFEE2E2, 0xFFDC2626),
            2 => (IconKind::Files, 0xFFCCFBF1, 0xFF0D9488),
            _ => (IconKind::Apps, 0xFFDBEAFE, 0xFF2563EB),
        };
        cards[i] = Some(EventCardPlan {
            rect: Rect::new(CARD_X, y, CARD_W, 94),
            icon_rect: Rect::new(CARD_X + 18, y + 20, 52, 52),
            icon_bg, icon_tint, icon_kind,
            title: LabelRun::new(CARD_X + 86, y + 16, type_scale::CAPTION, title_bytes, TEXT_MAIN),
            subtitle: LabelRun::new(CARD_X + 86, y + 40, type_scale::CAPTION, subtitle_bytes, TEXT_MUTED),
            time: LabelRun::new(CARD_X + 86, y + 64, type_scale::CAPTION, b"Event", TEXT_HINT),
        });
    }

    let clear_label = LabelRun::centred(CLEAR_BTN.x, CLEAR_BTN.y + 10, CLEAR_BTN.w, type_scale::CAPTION, b"Clear all", TEXT_MUTED);
    NotificationsFrame {
        clock, title, today_label, yesterday_label, pills, cards, card_count: count,
        clear_btn: CLEAR_BTN, clear_label, home_pill: HOME_PILL,
    }
}

// Per-Pixel Renderer
// -----------------------------------------------------------------------------

static mut FRAME_CACHE: Option<NotificationsFrame> = None;

#[inline(never)]
pub fn get_notifications_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let f = unsafe {
        let slot: *mut Option<NotificationsFrame> = core::ptr::addr_of_mut!(FRAME_CACHE);
        if (x == 0 && y == 0) || (*slot).is_none() {
            *slot = Some(build_notifications_frame(state));
        }
        (*slot).as_ref().unwrap()
    };

    let mut color = BG_COLOR;

    // 1. Status Bar (y < 48)
    if y < STATUS_BAR_HEIGHT + 12 {
        if let Some(c) = f.clock.pixel(x, y) {
            return c;
        }
        return color;
    }

    // 2. Header
    if y < 120 {
        if let Some(c) = f.title.pixel(x, y) {
            return c;
        }
        // Filter / settings sliders icon at right (x: 644..676, y: 72..94)
        if x >= 644 && x <= 676 && y >= 72 && y <= 94 {
            let is_line0 = y == 76;
            let is_line1 = y == 83;
            let is_line2 = y == 90;
            let is_knob0 = (x >= 660 && x <= 664) && (y >= 74 && y <= 78);
            let is_knob1 = (x >= 648 && x <= 652) && (y >= 81 && y <= 85);
            let is_knob2 = (x >= 664 && x <= 668) && (y >= 88 && y <= 92);
            if is_line0 || is_line1 || is_line2 || is_knob0 || is_knob1 || is_knob2 {
                return TEXT_MAIN;
            }
        }
        return color;
    }

    // 3. Filter Pills
    for pill in &f.pills {
        if pill.rect.contains(x, y) {
            if rounded_contains(pill.rect, x, y, 18) {
                color = pill.bg;
                if let Some(c) = pill.label.pixel(x, y) {
                    return c;
                }
                return color;
            }
        }
    }

    // 4. Section Headers
    if let Some(c) = f.today_label.pixel(x, y) {
        return c;
    }
    if let Some(c) = f.yesterday_label.pixel(x, y) {
        return c;
    }

    // 5. Notification Cards
    for card_opt in &f.cards {
        if let Some(card) = card_opt {
            if card.rect.contains(x, y) {
                if rounded_contains(card.rect, x, y, 20) {
                    color = CARD_BG;
                    if rounded_outline(card.rect, x, y, 20) {
                        return CARD_BORDER;
                    }

                    // Icon Squircle
                    if rounded_contains(card.icon_rect, x, y, 14) {
                        color = card.icon_bg;
                        let cx = card.icon_rect.center_x();
                        let cy = card.icon_rect.center_y();
                        if let Some(ic) = icons::rasterize_icon_pixel(card.icon_kind, x, y, cx, cy, card.icon_tint) {
                            return ic;
                        }
                        return color;
                    }

                    // Labels
                    if let Some(c) = card.title.pixel(x, y) {
                        return c;
                    }
                    if let Some(c) = card.subtitle.pixel(x, y) {
                        return c;
                    }
                    if let Some(c) = card.time.pixel(x, y) {
                        return c;
                    }
                    return color;
                }
            }
        }
    }

    // 6. Clear All Pill (y: 708..752, x: 260..460 -- see CLEAR_BTN)
    if CLEAR_BTN.contains(x, y) {
        if rounded_contains(CLEAR_BTN, x, y, 22) {
            color = 0xFFF1F5F9;
            if rounded_outline(CLEAR_BTN, x, y, 22) {
                return 0xFFE2E8F0;
            }
            if let Some(c) = f.clear_label.pixel(x, y) {
                return c;
            }
            return color;
        }
    }

    // 7. Home Indicator Pill
    if rounded_contains(f.home_pill, x, y, 2) {
        return 0xFF94A3B8;
    }

    color
}