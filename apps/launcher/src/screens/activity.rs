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
    space, type_scale, SCREEN_HEIGHT, SCREEN_WIDTH,
    STATUS_BAR_HEIGHT,
};

// -----------------------------------------------------------------------------
// Constants & Colors
// -----------------------------------------------------------------------------

const BG_COLOR: u32 = 0xFF111214;
const CARD_BG: u32 = 0xFF1E222A;
const CARD_BORDER: u32 = 0xFF353B46;
const TEXT_WHITE: u32 = 0xFFF8FAFC;
const TEXT_MUTED: u32 = 0xFF94A3B8;

const CARD_X: usize = 32;
const CARD_W: usize = SCREEN_WIDTH - 64; // 656

const CARD_RADIUS: usize = 28;

// Card rectangles:
// Card 0 (recent slot 0): 120 .. 390  (h=270, contains y=230 dismiss badge)
// Card 1 (recent slot 1): 420 .. 690  (h=270, contains y=550 dismiss badge)
// Card 2 (recent slot 2): 720 .. 990  (h=270, contains y=870 dismiss badge)
// Card 3:                  1020 .. 1200
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

const CARD_TEXT_X: usize = 96;
const CARD_TITLE_Y: usize = 26;
const CARD_SUBTITLE_Y: usize = 54;

const CARD_ICON_SIZE: usize = 56;
const CARD_ICON_RADIUS: usize = 14;

const CARD_ICON_BG: u32 = 0xFF334155;
const CARD_ICON_BORDER: u32 = 0xFF475569;

// Dismiss badges: 48x48 rounded circles centered at test coordinates
// Card 0: center (610, 230) -> x: 586..634, y: 206..254
// Card 1: center (610, 550) -> x: 586..634, y: 526..574
// Card 2: center (610, 870) -> x: 586..634, y: 846..894
const BADGE_SIZE: usize = 48;
const BADGE_RADIUS: usize = BADGE_SIZE / 2;
const BADGE_X: usize = 586;
pub const CARD0_DISMISS: Rect = Rect::new(BADGE_X, 206, BADGE_SIZE, BADGE_SIZE);
pub const CARD1_DISMISS: Rect = Rect::new(BADGE_X, 526, BADGE_SIZE, BADGE_SIZE);
pub const CARD2_DISMISS: Rect = Rect::new(BADGE_X, 846, BADGE_SIZE, BADGE_SIZE);

/// Prominent "Close All" pill button centered below the recents deck
pub const CLOSE_ALL_BTN: Rect = Rect::new(SCREEN_WIDTH / 2 - 110, 1220, 220, 48);

const HOME_PILL: Rect = Rect::new(SCREEN_WIDTH / 2 - 90, SCREEN_HEIGHT - 20, 180, 5);

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
// Text Run Handling
// -----------------------------------------------------------------------------

const RUN_CAP: usize = 96;

fn activity_run_metrics(y: usize, scale: usize) -> (u32, usize, usize) {
    let em = launcher::outline_text::em_for_scale(scale);
    let (asc, desc) = interos_std::design::typeface::line_metrics(em);
    let baseline = y + launcher::font::BASELINE * scale;
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
        if self.scale >= 2 && self.len <= 24 {
            if let Some(run) = launcher::outline_text::OutlineRun::place(
                self.x,
                self.y + launcher::font::BASELINE * self.scale,
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
        if self.scale >= 2 && self.len <= 24 {
            if let Some(run) = launcher::outline_text::OutlineRun::place(
                self.x,
                self.y + launcher::font::BASELINE * self.scale,
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
// Card Plan & Mini Screen Preview
// -----------------------------------------------------------------------------

struct CardPlan {
    rect: Rect,
    icon_rect: Rect,
    icon: Option<IconKind>,
    badge: LabelRun,
    title: LabelRun,
    subtitle: LabelRun,
    dismiss_rect: Option<Rect>,
    pkg_id: [u8; 32],
    pkg_len: usize,
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

const SUB_CAP: usize = 32;
const SUB_RUNNING: &[u8] = b"Running \xb7 task ";
const SUB_ACTIVE: &[u8] = b"Running \xb7 active";
const SUB_NOT_RUNNING: &[u8] = b"Recent \xb7 ready";

const SUB_INK_RUNNING: u32 = 0xFF38BDF8;

fn card_subtitle(buf: &mut [u8; SUB_CAP], task: u64, focused: bool) -> &[u8] {
    if task == 0 {
        return SUB_NOT_RUNNING;
    }
    if focused {
        return SUB_ACTIVE;
    }
    let mut at = put(buf, 0, SUB_RUNNING);
    at = push_decimal(buf, at, task as usize);
    &buf[..at]
}

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

fn card_title_bytes<'a>(state: &'a LauncherState, id: &'a [u8]) -> &'a [u8] {
    match registry_index(state, id) {
        Some(index) => {
            let len = (state.app_label_lens[index] as usize).min(state.app_labels[index].len());
            if len > 0 {
                &state.app_labels[index][..len]
            } else {
                id
            }
        }
        None => id,
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

    let icon_rect = Rect::new(
        rect.x + (CARD_TEXT_X - CARD_ICON_SIZE - space::MD),
        rect.y + 24,
        CARD_ICON_SIZE,
        CARD_ICON_SIZE,
    );
    let title_x = rect.x + CARD_TEXT_X;
    let title_fit = rect.w.saturating_sub(CARD_TEXT_X + BADGE_SIZE + space::XL);

    let task = state.recent_task_ids.get(slot).copied().unwrap_or(0);
    let focused = task != 0 && state.active_child == Some(task);
    let mut sub_buf = [0u8; SUB_CAP];
    let subtitle = LabelRun::new(
        title_x,
        rect.y + CARD_SUBTITLE_Y,
        type_scale::CAPTION,
        card_subtitle(&mut sub_buf, task, focused),
        if task == 0 { TEXT_MUTED } else { SUB_INK_RUNNING },
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

    let mut pkg_id = [0u8; 32];
    pkg_id[..len].copy_from_slice(id);

    Some(CardPlan {
        rect,
        icon_rect,
        icon: icons::match_icon_kind(id),
        badge,
        title: LabelRun::fitted(
            title_x,
            rect.y + CARD_TITLE_Y,
            type_scale::BODY,
            title_fit,
            card_title_bytes(state, id),
            TEXT_WHITE,
        ),
        subtitle,
        dismiss_rect: dismiss_rect_for(slot),
        pkg_id,
        pkg_len: len,
    })
}

// -----------------------------------------------------------------------------
// Miniature Screen Preview Renderer
// -----------------------------------------------------------------------------

fn render_mini_preview(card: &CardPlan, px: usize, py: usize) -> Option<u32> {
    let preview_rect = Rect::new(card.rect.x + 20, card.rect.y + 86, card.rect.w - 40, 166);
    if !rounded_contains(preview_rect, px, py, 14) {
        return None;
    }
    if rounded_outline(preview_rect, px, py, 14) {
        return Some(0xFF333842);
    }

    let rx = px - preview_rect.x;
    let ry = py - preview_rect.y;

    // Window top title bar
    if ry < 24 {
        if ry == 23 {
            return Some(0xFF2E3440);
        }
        // Window dots on top left
        if rx >= 12 && rx < 44 && ry >= 8 && ry < 16 {
            let dot_idx = (rx - 12) / 12;
            let dot_x = 12 + dot_idx * 12 + 4;
            let dot_y = 12;
            let d2 = (px as i32 - (preview_rect.x + dot_x) as i32).pow(2)
                + (py as i32 - (preview_rect.y + dot_y) as i32).pow(2);
            if d2 <= 9 {
                return match dot_idx {
                    0 => Some(0xFFEF4444),
                    1 => Some(0xFFF59E0B),
                    _ => Some(0xFF10B981),
                };
            }
        }
        return Some(0xFF1E222A);
    }

    let pkg = &card.pkg_id[..card.pkg_len];

    // 1. Browser
    if pkg == b"org.interos.browser" || pkg == b"browser" {
        // Omnibox URL bar
        if ry >= 28 && ry < 48 && rx >= 16 && rx < 600 {
            if rx == 16 || rx == 599 || ry == 28 || ry == 47 {
                return Some(0xFF334155);
            }
            // Lock icon dot
            if rx >= 26 && rx <= 32 && ry >= 35 && ry <= 41 {
                return Some(0xFF10B981);
            }
            return Some(0xFF1E293B);
        }
        // Web page content body
        if ry >= 56 {
            // Search result card 1
            if ry >= 60 && ry < 104 && rx >= 16 && rx < 600 {
                if ry >= 66 && ry < 74 && rx >= 24 && rx < 320 {
                    return Some(0xFF38BDF8); // Result title
                }
                if ry >= 80 && ry < 86 && rx >= 24 && rx < 480 {
                    return Some(0xFF64748B); // Snippet line 1
                }
                if ry >= 90 && ry < 96 && rx >= 24 && rx < 380 {
                    return Some(0xFF475569); // Snippet line 2
                }
                return Some(0xFF151922);
            }
            // Search result card 2
            if ry >= 110 && ry < 156 && rx >= 16 && rx < 600 {
                if ry >= 116 && ry < 124 && rx >= 24 && rx < 280 {
                    return Some(0xFF38BDF8);
                }
                if ry >= 130 && ry < 136 && rx >= 24 && rx < 450 {
                    return Some(0xFF64748B);
                }
                if ry >= 140 && ry < 146 && rx >= 24 && rx < 340 {
                    return Some(0xFF475569);
                }
                return Some(0xFF151922);
            }
        }
        return Some(0xFF0F1218);
    }

    // 2. Camera
    if pkg == b"org.interos.camera" || pkg == b"camera" {
        // Viewfinder dark matrix
        if ry >= 30 && ry < 130 && rx >= 16 && rx < 600 {
            let cx = 308i32;
            let cy = 80i32;
            let dx = (rx as i32 - cx).abs();
            let dy = (ry as i32 - cy).abs();
            if (dx == 40 && dy <= 40) || (dy == 40 && dx <= 40) {
                if dx >= 28 || dy >= 28 {
                    return Some(0xFF38BDF8);
                }
            }
            return Some(0xFF07080B);
        }
        // Camera bottom controls (shutter pill)
        if ry >= 134 && ry < 160 {
            let scx = 308i32;
            let scy = 146i32;
            let d2 = (rx as i32 - scx).pow(2) + (ry as i32 - scy).pow(2);
            if d2 <= 81 {
                if d2 <= 36 {
                    return Some(0xFFEF4444);
                }
                return Some(0xFFFFFFFF);
            }
        }
        return Some(0xFF0A0C10);
    }

    // 3. Settings
    if pkg == b"org.interos.settings" || pkg == b"settings" {
        if ry >= 28 && ry < 66 && rx >= 16 && rx < 600 {
            if rx == 16 || rx == 599 || ry == 28 || ry == 65 {
                return Some(0xFF2E3544);
            }
            if rx >= 540 && rx <= 576 && ry >= 40 && ry <= 54 {
                if rx >= 558 {
                    return Some(0xFF38BDF8);
                }
                return Some(0xFF0284C7);
            }
            if ry >= 44 && ry < 50 && rx >= 32 && rx < 260 {
                return Some(0xFFE2E8F0);
            }
            return Some(0xFF1E232C);
        }
        if ry >= 72 && ry < 110 && rx >= 16 && rx < 600 {
            if rx == 16 || rx == 599 || ry == 72 || ry == 109 {
                return Some(0xFF2E3544);
            }
            if rx >= 520 && rx <= 576 && ry >= 84 && ry <= 98 {
                return Some(0xFF334155);
            }
            if ry >= 88 && ry < 94 && rx >= 32 && rx < 300 {
                return Some(0xFFE2E8F0);
            }
            return Some(0xFF1E232C);
        }
        if ry >= 116 && ry < 154 && rx >= 16 && rx < 600 {
            if rx == 16 || rx == 599 || ry == 116 || ry == 153 {
                return Some(0xFF2E3544);
            }
            if ry >= 132 && ry < 138 && rx >= 32 && rx < 280 {
                return Some(0xFFE2E8F0);
            }
            return Some(0xFF1E232C);
        }
        return Some(0xFF14171E);
    }

    // 4. File Manager
    if pkg == b"org.interos.file_manager" || pkg == b"files" {
        if ry >= 30 && ry < 42 && rx >= 16 && rx < 600 {
            if rx < 260 {
                return Some(0xFF38BDF8);
            }
            return Some(0xFF242A36);
        }
        if ry >= 50 && ry < 152 {
            let col = (rx.saturating_sub(16)) / 148;
            let col_rx = (rx.saturating_sub(16)) % 148;
            if col < 4 && col_rx < 136 {
                if col_rx == 0 || col_rx == 135 || ry == 50 || ry == 151 {
                    return Some(0xFF2D3545);
                }
                if ry >= 60 && ry < 84 && col_rx >= 16 && col_rx < 56 {
                    return match col {
                        0 => Some(0xFF38BDF8),
                        1 => Some(0xFF10B981),
                        2 => Some(0xFFA855F7),
                        _ => Some(0xFFF59E0B),
                    };
                }
                if ry >= 120 && ry < 126 && col_rx >= 16 && col_rx < 100 {
                    return Some(0xFFCBD5E1);
                }
                return Some(0xFF1B202A);
            }
        }
        return Some(0xFF12151B);
    }

    // 5. Dialer
    if pkg == b"org.interos.dialer" || pkg == b"dialer" {
        if ry >= 28 && ry < 50 && rx >= 24 && rx < 592 {
            if ry >= 36 && ry < 44 && rx >= 180 && rx < 436 {
                return Some(0xFFF8FAFC);
            }
            return Some(0xFF181C24);
        }
        if ry >= 56 && ry < 128 {
            let kcol = (rx.saturating_sub(160)) / 100;
            let krow = (ry - 56) / 36;
            let krx = (rx.saturating_sub(160)) % 100;
            let kry = (ry - 56) % 36;
            if kcol < 3 && krow < 2 && krx < 80 && kry < 30 {
                return Some(0xFF222938);
            }
        }
        if ry >= 134 && ry < 158 && rx >= 250 && rx < 366 {
            return Some(0xFF10B981);
        }
        return Some(0xFF101319);
    }

    // 6. Messages
    if pkg == b"org.interos.messages" || pkg == b"messages" {
        if ry >= 30 && ry < 62 && rx >= 20 && rx < 340 {
            if rx == 20 || rx == 339 || ry == 30 || ry == 61 {
                return Some(0xFF334155);
            }
            if ry >= 42 && ry < 48 && rx >= 36 && rx < 300 {
                return Some(0xFFE2E8F0);
            }
            return Some(0xFF1E293B);
        }
        if ry >= 70 && ry < 102 && rx >= 280 && rx < 596 {
            if rx == 280 || rx == 595 || ry == 70 || ry == 101 {
                return Some(0xFF0284C7);
            }
            if ry >= 82 && ry < 88 && rx >= 300 && rx < 550 {
                return Some(0xFFFFFFFF);
            }
            return Some(0xFF0369A1);
        }
        if ry >= 120 && ry < 154 && rx >= 20 && rx < 596 {
            return Some(0xFF1A1F29);
        }
        return Some(0xFF0F1217);
    }

    // 7. Music
    if pkg == b"org.interos.music" || pkg == b"music" {
        if ry >= 32 && ry < 132 && rx >= 24 && rx < 124 {
            let cx = 74i32;
            let cy = 82i32;
            let d2 = (rx as i32 - cx).pow(2) + (ry as i32 - cy).pow(2);
            if d2 <= 400 {
                return Some(0xFFEC4899);
            }
            return Some(0xFF8B5CF6);
        }
        if ry >= 44 && ry < 52 && rx >= 144 && rx < 380 {
            return Some(0xFFF8FAFC);
        }
        if ry >= 58 && ry < 64 && rx >= 144 && rx < 300 {
            return Some(0xFF94A3B8);
        }
        if ry >= 80 && ry < 124 && rx >= 144 && rx < 580 {
            let bar_idx = (rx - 144) / 28;
            let bar_rx = (rx - 144) % 28;
            if bar_rx < 14 {
                let h_val = 14 + ((bar_idx * 7) % 28);
                if ry >= (124 - h_val) {
                    return Some(0xFF38BDF8);
                }
            }
        }
        if ry >= 138 && ry < 144 && rx >= 24 && rx < 592 {
            if rx < 280 {
                return Some(0xFF38BDF8);
            }
            return Some(0xFF334155);
        }
        return Some(0xFF111319);
    }

    // 8. Calculator
    if pkg == b"org.interos.calculator" || pkg == b"calculator" {
        if ry >= 30 && ry < 64 && rx >= 20 && rx < 596 {
            if ry >= 42 && ry < 52 && rx >= 460 && rx < 580 {
                return Some(0xFF38BDF8);
            }
            return Some(0xFF0F172A);
        }
        if ry >= 72 && ry < 156 {
            let ccol = (rx.saturating_sub(20)) / 144;
            let crow = (ry - 72) / 28;
            let crx = (rx.saturating_sub(20)) % 144;
            let cry = (ry - 72) % 28;
            if ccol < 4 && crow < 3 && crx < 136 && cry < 24 {
                if ccol == 3 {
                    return Some(0xFFF97316);
                }
                return Some(0xFF1E293B);
            }
        }
        return Some(0xFF0F131C);
    }

    // Default application card
    if ry >= 32 && ry < 84 && rx >= 24 && rx < 592 {
        if rx == 24 || rx == 591 || ry == 32 || ry == 83 {
            return Some(0xFF334155);
        }
        if ry >= 52 && ry < 60 && rx >= 40 && rx < 340 {
            return Some(0xFFE2E8F0);
        }
        return Some(0xFF1E2430);
    }
    if ry >= 94 && ry < 146 && rx >= 24 && rx < 592 {
        if rx == 24 || rx == 591 || ry == 94 || ry == 145 {
            return Some(0xFF334155);
        }
        if ry >= 114 && ry < 122 && rx >= 40 && rx < 300 {
            return Some(0xFFE2E8F0);
        }
        return Some(0xFF1E2430);
    }

    Some(0xFF151821)
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

    // 4. Prominent "Close All" pill button under deck
    let nav_label_scale = type_scale::BODY;
    let close_all_label = LabelRun::centred(
        CLOSE_ALL_BTN.x,
        CLOSE_ALL_BTN.y
            + (CLOSE_ALL_BTN.h.saturating_sub(text::line_height(nav_label_scale))) / 2,
        CLOSE_ALL_BTN.w,
        nav_label_scale,
        b"Close All",
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

    // 1. Status Bar (y < 72)
    if y < STATUS_BAR_HEIGHT + 12 {
        if let Some(c) = f.clock.pixel_on(x, y, BG_COLOR) {
            return c;
        }
        return BG_COLOR;
    }

    // 2. Status band (y in STATUS_BAND_TOP .. WORKSPACE_CARDS[0].y: 60..120)
    if y < 120 {
        if y >= STATUS_BAND_TOP {
            if let Some(c) = f.status_head.pixel_on(x, y, BG_COLOR) {
                return c;
            }
            if let Some(c) = f.status_body.pixel_on(x, y, BG_COLOR) {
                return c;
            }
        }
        return BG_COLOR;
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
                if rounded_outline(card.rect, x, y, CARD_RADIUS) {
                    return CARD_BORDER;
                }

                // App icon
                if rounded_contains(card.icon_rect, x, y, CARD_ICON_RADIUS) {
                    if rounded_outline(card.icon_rect, x, y, CARD_ICON_RADIUS) {
                        return CARD_ICON_BORDER;
                    }
                    if let Some(kind) = card.icon {
                        let cx = card.icon_rect.center_x();
                        let cy = card.icon_rect.center_y();
                        if let Some(ic) = icons::rasterize_icon_pixel(kind, x, y, cx, cy, 0xFFFFFFFF) {
                            return ic;
                        }
                        return CARD_ICON_BG;
                    }
                    if let Some(c) = card.badge.pixel_on(x, y, CARD_ICON_BG) {
                        return c;
                    }
                    return CARD_ICON_BG;
                }

                // Title & Subtitle
                if let Some(c) = card.title.pixel_on(x, y, CARD_BG) {
                    return c;
                }
                if let Some(c) = card.subtitle.pixel_on(x, y, CARD_BG) {
                    return c;
                }

                // Mini preview inside the card
                if let Some(c) = render_mini_preview(card, x, y) {
                    // Elevated dismiss badge [X] on top of preview
                    if let Some(dismiss_r) = card.dismiss_rect {
                        if rounded_contains(dismiss_r, x, y, BADGE_RADIUS) {
                            if rounded_outline(dismiss_r, x, y, BADGE_RADIUS) {
                                return 0xFF475569;
                            }
                            let cx = dismiss_r.center_x() as i32;
                            let cy = dismiss_r.center_y() as i32;
                            let dx = (x as i32) - cx;
                            let dy = (y as i32) - cy;
                            if dx.abs() <= 6 && dy.abs() <= 6 && ((dx - dy).abs() <= 1 || (dx + dy).abs() <= 1) {
                                return 0xFFFFFFFF;
                            }
                            return 0xFF1E2532;
                        }
                    }
                    return c;
                }

                // Dismiss badge [X]
                if let Some(dismiss_r) = card.dismiss_rect {
                    if rounded_contains(dismiss_r, x, y, BADGE_RADIUS) {
                        if rounded_outline(dismiss_r, x, y, BADGE_RADIUS) {
                            return 0xFF475569;
                        }
                        let cx = dismiss_r.center_x() as i32;
                        let cy = dismiss_r.center_y() as i32;
                        let dx = (x as i32) - cx;
                        let dy = (y as i32) - cy;
                        if dx.abs() <= 6 && dy.abs() <= 6 && ((dx - dy).abs() <= 1 || (dx + dy).abs() <= 1) {
                            return 0xFFFFFFFF;
                        }
                        return 0xFF1E2532;
                    }
                }

                return CARD_BG;
            }
        }
        return BG_COLOR;
    }

    // 4. Bottom Controls: Prominent "Close All" pill and home indicator
    if y >= 1220 && y < 1270 {
        if f.close_all_btn.contains(x, y) {
            if rounded_contains(f.close_all_btn, x, y, 24) {
                if rounded_outline(f.close_all_btn, x, y, 24) {
                    return 0xFF475569;
                }
                if let Some(c) = f.close_all_label.pixel(x, y) {
                    return c;
                }
                return 0xFF1E2532;
            }
        }
    } else if y >= 1410 {
        if rounded_contains(f.home_pill, x, y, 2) {
            return 0xFF64748B;
        }
    }

    BG_COLOR
}

// -----------------------------------------------------------------------------
// Close control: hit test and termination
// -----------------------------------------------------------------------------

pub fn activity_close_hit(_state: &LauncherState, x: usize, y: usize) -> Option<usize> {
    if CARD0_DISMISS.contains(x, y) {
        Some(0)
    } else if CARD1_DISMISS.contains(x, y) {
        Some(1)
    } else if CARD2_DISMISS.contains(x, y) {
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