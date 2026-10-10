//! Notifications and Events Shade (Reference Screen 5).
//!
//! Renders the canonical "Events / Notifications done right" screen from system specifications:
//! - Crisp light theme background with status bar clock, signals, and battery.
//! - Header: "Events" with right-side filter control.
//! - No filter pills: four were drawn here with no filter state behind them
//!   and removed rather than left as inert controls.
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
use launcher::theme::{space, type_scale, SCREEN_HEIGHT, SCREEN_WIDTH, STATUS_BAR_HEIGHT};

// -----------------------------------------------------------------------------
// Constants & Colors
// -----------------------------------------------------------------------------

const BG_COLOR: u32 = 0xFFF7F6F2; // Canonical warm cream background
const CARD_BG: u32 = 0xFFFFFFFF; // Pure white surface
const CARD_BORDER: u32 = 0xFFE1E0DB; // Canonical border
const TEXT_MAIN: u32 = 0xFF111214; // Canonical near-black primary
const TEXT_MUTED: u32 = 0xFF777773; // Canonical muted secondary


const CARD_X: usize = 32;
const CARD_W: usize = SCREEN_WIDTH - 64; // 656

/// Height of one event card, and the pitch its stack advances by.
const CARD_H: usize = 94;
const CARD_PITCH: usize = 100;
/// Top of the card stack when it is full. WHY it is named and not retyped per
/// card: the stack is anchored to the clear-all pill, and this is the ceiling that
/// anchor is clamped to, so it has to be one number.
const CARDS_TOP: usize = 206;
/// Gap between the card's text column and the card's right edge.
const CARD_TEXT_INSET: usize = space::MD;
/// Left edge of the card's text column, past the 52 px icon squircle.
const CARD_TEXT_X: usize = CARD_X + 86;

// Clear-all pill, drawn below the last event card: x in 260..460, y in 708..752.
// WHY this y: the shell's tap handler in `main.rs` hit-tests this exact rect and
// nothing else, so the pill cannot move with the number of notifications without
// the drawn control and the pressed one becoming different rectangles -- a tap on
// visible pixels that clears nothing, or a clear from pixels that draw nothing.
// The card stack is anchored to the pill instead (see `build_notifications_frame`),
// so the pill sits directly under the last card for every notification count.
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

    /// A run truncated with an ellipsis if it is wider than `fit`.
    ///
    /// WHY this exists here and not only on the centred runs: `new` copies up to
    /// `RUN_CAP` (64) bytes and draws every one of them, and `pixel` only rejects
    /// pixels outside the *measured* box -- which is however wide the string turned
    /// out to be. A 64-byte notification body at the body scale measures about
    /// 700 px from x 118, which is 100 px past the right edge of a 720 px screen,
    /// and nothing on the surface noticed.
    fn fitted(x: usize, y: usize, scale: usize, fit: usize, bytes: &[u8], ink: u32) -> Self {
        let (buf, len) = text::ellipsize(bytes, scale, fit);
        let n = len.min(RUN_CAP);
        let mut held = [0u8; RUN_CAP];
        held[..n].copy_from_slice(&buf[..n]);
        LabelRun {
            x,
            y,
            w: text::measure(&held[..n], scale),
            h: text::line_height(scale),
            scale,
            len: n,
            ink,
            bytes: held,
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
}

struct NotificationsFrame {
    clock: LabelRun,
    title: LabelRun,
    today_label: LabelRun,
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

    // WHY there are no filter pills on this surface: four pills were drawn here
    // (All / Messages / System / Apps) with an active treatment on All, but no
    // filter state exists on the frame and the shell's only Notifications tap
    // handler tests the clear-all rect. Three pills were inert and one was
    // permanently active. Wiring them would mean inventing a
    // package-to-category taxonomy the records do not carry, so they are removed
    // rather than replaced with a different fiction - the file manager set this
    // precedent for its own unhandled header glyphs. The section label moves up
    // into the space the pills occupied.
    let count = state.notification_count.min(state.notifications.len()).min(5);
    let today_label = if count > 0 {
        LabelRun::new(36, 124, type_scale::CAPTION, b"Recent", TEXT_MAIN)
    } else {
        LabelRun::new(36, 124, type_scale::BODY, b"No notifications", TEXT_MUTED)
    };

    let mut cards: [Option<EventCardPlan>; 5] = [None, None, None, None, None];
    // WHY the stack hangs off the clear-all pill instead of starting at a fixed y:
    // the pill is the only region the shell treats as "clear", and it cannot move
    // without the drawn control and the tapped one becoming different rectangles.
    // Pinning the stack's TOP instead left the pill stranded: with one notification
    // the last card ended at 300 and the pill sat at 708, so the surface showed a
    // clear control 400 px under anything it acted on. Anchoring the stack's BOTTOM
    // to the pill keeps the same layout for a full stack -- five cards at 206, 306,
    // 406, 506 and 606, the last ending at 700 -- and closes the gap for every
    // shorter one.
    let stack_h = count * CARD_H + count.saturating_sub(1) * (CARD_PITCH - CARD_H);
    let stack_top = (CLEAR_BTN.y - space::SM - stack_h).max(CARDS_TOP);
    let text_fit = CARD_X + CARD_W - CARD_TEXT_X - CARD_TEXT_INSET;
    for i in 0..count {
        let rec = state.notifications[i];
        let y = stack_top + i * CARD_PITCH;
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
            rect: Rect::new(CARD_X, y, CARD_W, CARD_H),
            icon_rect: Rect::new(CARD_X + 18, y + 20, 52, 52),
            icon_bg, icon_tint, icon_kind,
            // WHY the title is a rung above the body and there is no third line: at
            // one scale for all three, the only thing separating a notification's
            // name from its text was their colour, and the third line was the
            // literal "Event" in every card -- a word that is not a time and never
            // will be. The record's `timestamp_ticks` is a monotonic count from
            // boot, and nothing in this shell can turn that into a clock reading:
            // converting it needs an RTC epoch, and the RTC read is exactly what
            // `clock_source_ok` says was refused. So the row is dropped rather than
            // filled with a placeholder that looks like a timestamp.
            //
            // WHY the title is `type_scale::BODY` and not `TITLE`: two lines of
            // `type_scale::BODY` and `type_scale::CAPTION` are 48 + 24 = 72 rows,
            // which is what fits a 94-row card. A `TITLE` title alone is 72, and
            // there is no room for the body beneath it.
            title: LabelRun::fitted(CARD_TEXT_X, y + 12, type_scale::BODY, text_fit, title_bytes, TEXT_MAIN),
            subtitle: LabelRun::fitted(CARD_TEXT_X, y + 12 + text::line_height(type_scale::BODY) + space::XS, type_scale::CAPTION, text_fit, subtitle_bytes, TEXT_MUTED),
        });
    }

    let clear_label = LabelRun::centred(CLEAR_BTN.x, CLEAR_BTN.y + 10, CLEAR_BTN.w, type_scale::CAPTION, b"Clear all", TEXT_MUTED);
    NotificationsFrame {
        clock, title, today_label, cards, card_count: count,
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
        // WHY no header icon is drawn at the right: a three-line sliders mark
        // sat here and looked like notification settings, but it had no handler
        // and opened nothing. It went with the filter pills below.
        return color;
    }

    // 4. Section Headers
    //
    // WHY there is only one: the "Yesterday" header this used to draw was pinned at
    // y 650 and always held zero bytes, so it never inked a pixel -- and 650 is
    // inside the card stack now that the stack is anchored to the clear-all pill.
    // An always-empty header at a hardcoded y is a header waiting to become a lie.
    if let Some(c) = f.today_label.pixel(x, y) {
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
                    return color;
                }
            }
        }
    }

    // 6. Clear All Pill (see CLEAR_BTN)
    if f.clear_btn.contains(x, y) {
        if rounded_contains(f.clear_btn, x, y, 22) {
            color = 0xFFF1F5F9;
            if rounded_outline(f.clear_btn, x, y, 22) {
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