//! Notifications & Events Shade: production-grade notification center.
//!
//! Visual hierarchy:
//! - Uniform system status bar (clock, signal bars, Wi-Fi fan, battery shell).
//! - Header: Large bold "Notifications" title, crisp "Clear all" pill button,
//!   and clean subtitle.
//! - Filter pills: All, Messages, System, Apps with live counts.
//! - Notification Cards:
//!   - Squircle app icon (28x28) with app brand color & clean vector glyph.
//!   - Header line: App name (bold) • timestamp ("Just now", "5m ago").
//!   - Bold title (18-20px) and clean body text.
//!   - Action pills (e.g. "Reply", "Mark read", "Details", "Dismiss").
//! - Empty state: Flagship "No new notifications", "All caught up" illustration.
//! - Bottom Clear All button & Home indicator bar.

#![allow(dead_code)]

use crate::{test_text_aa, LauncherState, NotificationRecord};
use launcher::layout::Rect;
use launcher::text;
use launcher::theme::{space, type_scale, radius, Theme, SCREEN_HEIGHT, SCREEN_WIDTH, STATUS_BAR_HEIGHT, CONTENT_PADDING};

// -----------------------------------------------------------------------------
// Constants & Geometry
// -----------------------------------------------------------------------------

/// Left edge and width of the feed's content cards.
const ROW_X: usize = 32;
const ROW_W: usize = SCREEN_WIDTH - 64; // 656

/// Left edge of the header block.
const HEADER_X: usize = 36;

/// Baselines of the header block, in order.
pub const TITLE_Y: usize = 56;
pub const SUBTITLE_Y: usize = 108;
pub const SOURCE_NOTE_Y: usize = 132;

/// Baseline pitch between consecutive caption lines.
const CAPTION_PITCH: usize = 24;

/// Filter note line placed below the pill row.
pub const FILTER_NOTE_Y: usize = PILLS_Y + PILL_H;

/// Height of one notification card, and the pitch its stack advances by.
const ROW_H: usize = 120;
const ROW_PITCH: usize = 126;

/// First row of the first section's stack.
pub const SECTION_HEAD_Y: usize = 300;

/// Bottom cutoff for feed rows above the bottom action bar.
const FEED_BOTTOM: usize = 708 - space::SM; // 700

/// Height of the pill row, and the gap between pills.
pub const PILLS_Y: usize = 210;
pub const PILL_H: usize = 52;
const PILL_GAP: usize = space::SM + space::XS;

/// Left edge of a row's text column, past the app squircle icon.
const ROW_TEXT_X: usize = ROW_X + 56;

/// Guest timer ticks per second (1 kHz PIT counter).
const TICKS_PER_SEC: u64 = 1000;

/// Default active category filter (0 = All).
const DEFAULT_FILTER: usize = 0;

/// Category filter names.
const FILTER_NAMES: [&[u8]; 4] = [b"All", b"Messages", b"System", b"Apps"];

/// Section headers, in age order.
const SECTION_NAMES: [&[u8]; 4] = [b"Last minute", b"Last hour", b"Since boot", b"Undated"];

/// Header "Clear all" button.
pub const HEADER_CLEAR_BTN: Rect = Rect::new(SCREEN_WIDTH - 156, 50, 120, 36);

/// Bottom Clear All pill: matches the hit-test and serial harness (360, 720).
pub const CLEAR_BTN: Rect = Rect::new(SCREEN_WIDTH / 2 - 100, 708, 200, 44);

/// Home gesture indicator pill.
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

fn run_metrics(y: usize, scale: usize) -> (u32, usize, usize) {
    let em = launcher::outline_text::em_for_scale(scale);
    let (asc, desc) = interos_std::design::typeface::line_metrics(em);
    let baseline = (y + launcher::font::ascent_for_scale(scale)).max(asc as usize);
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
    outline: Option<launcher::outline_text::OutlineRun>,
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
        outline: None,
    };

    fn new(x: usize, y: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        let (em, top, box_h) = run_metrics(y, scale);
        let w = text::measure(&buf[..len], scale)
            .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize);
        let (asc, _) = interos_std::design::typeface::line_metrics(em);
        let baseline = (y + launcher::font::ascent_for_scale(scale)).max(asc as usize);
        let outline = if scale >= 2 && len <= launcher::outline_text::KEY_MAX {
            launcher::outline_text::OutlineRun::place(
                x,
                baseline,
                em,
                &buf[..len],
            )
        } else {
            None
        };
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
            outline,
        }
    }

    fn centred(x: usize, y: usize, w: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        let measured = text::measure(&buf[..len], scale);
        let inset = text::center_x(&buf[..len], scale, w);
        let (em, top, box_h) = run_metrics(y, scale);
        let rx = x + inset;
        let (asc, _) = interos_std::design::typeface::line_metrics(em);
        let baseline = (y + launcher::font::ascent_for_scale(scale)).max(asc as usize);
        let outline = if scale >= 2 && len <= launcher::outline_text::KEY_MAX {
            launcher::outline_text::OutlineRun::place(
                rx,
                baseline,
                em,
                &buf[..len],
            )
        } else {
            None
        };
        LabelRun {
            x: rx,
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
            outline,
        }
    }

    fn right(x_right: usize, y: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        let (em, top, box_h) = run_metrics(y, scale);
        let w = text::measure(&buf[..len], scale)
            .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize);
        let rx = x_right.saturating_sub(w);
        let (asc, _) = interos_std::design::typeface::line_metrics(em);
        let baseline = (y + launcher::font::ascent_for_scale(scale)).max(asc as usize);
        let outline = if scale >= 2 && len <= launcher::outline_text::KEY_MAX {
            launcher::outline_text::OutlineRun::place(
                rx,
                baseline,
                em,
                &buf[..len],
            )
        } else {
            None
        };
        LabelRun {
            x: rx,
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
            outline,
        }
    }

    fn fitted(x: usize, y: usize, scale: usize, fit: usize, bytes: &[u8], ink: u32) -> Self {
        let (em, top, box_h) = run_metrics(y, scale);
        let mut held = [0u8; RUN_CAP + 3];
        let src_len = bytes.len().min(RUN_CAP);
        let whole = bytes.len() <= RUN_CAP
            && text::measure(&bytes[..src_len], scale) <= fit
            && interos_std::design::typeface::text_advance_px(&bytes[..src_len], em) as usize <= fit;
        let n = if whole {
            held[..src_len].copy_from_slice(&bytes[..src_len]);
            src_len
        } else {
            let mut take = src_len.min(RUN_CAP - 3);
            loop {
                let m = take + 3;
                held[..take].copy_from_slice(&bytes[..take]);
                held[take..m].copy_from_slice(b"...");
                if text::measure(&held[..m], scale) <= fit
                    && interos_std::design::typeface::text_advance_px(&held[..m], em) as usize <= fit
                {
                    break m;
                }
                if take == 0 {
                    break 0;
                }
                take -= 1;
            }
        };
        let mut bytes_buf = [0u8; RUN_CAP];
        bytes_buf[..n].copy_from_slice(&held[..n]);
        let (asc, _) = interos_std::design::typeface::line_metrics(em);
        let baseline = (y + launcher::font::ascent_for_scale(scale)).max(asc as usize);
        let outline = if scale >= 2 && n <= launcher::outline_text::KEY_MAX {
            launcher::outline_text::OutlineRun::place(
                x,
                baseline,
                em,
                &held[..n],
            )
        } else {
            None
        };
        LabelRun {
            x,
            y,
            w: text::measure(&held[..n], scale)
                .max(interos_std::design::typeface::text_advance_px(&held[..n], em) as usize),
            h: text::line_height(scale),
            scale,
            len: n,
            ink,
            em,
            top,
            box_h,
            bytes: bytes_buf,
            outline,
        }
    }

    #[inline]
    fn pixel_on(&self, px: usize, py: usize, bg: u32) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if px < self.x || px >= self.x + self.w || py < self.top || py - self.top >= self.box_h {
            return None;
        }
        if let Some(ref run) = self.outline {
            return run.pixel(px, py, self.ink, bg);
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

fn push_dec(out: &mut [u8], at: usize, mut v: u64) -> usize {
    let mut digits = [0u8; 20];
    let mut n = 0usize;
    if v == 0 {
        digits[0] = b'0';
        n = 1;
    } else {
        while v > 0 && n < digits.len() {
            digits[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
    }
    let mut cursor = at;
    while n > 0 {
        n -= 1;
        if cursor < out.len() {
            out[cursor] = digits[n];
        }
        cursor += 1;
    }
    cursor
}

fn write_age(out: &mut [u8; 16], posted: u64, now: Option<u64>) -> usize {
    let Some(now) = now else { return 0 };
    let secs = now.saturating_sub(posted) / TICKS_PER_SEC;
    if secs < 60 {
        out[..8].copy_from_slice(b"Just now");
        return 8;
    }
    let (value, suffix): (u64, &[u8]) = if secs < 3600 {
        (secs / 60, b"m ago")
    } else if secs < 86400 {
        (secs / 3600, b"h ago")
    } else {
        (secs / 86400, b"d ago")
    };
    let mut n = push_dec(out, 0, value);
    for &b in suffix {
        if n < out.len() {
            out[n] = b;
            n += 1;
        }
    }
    n
}

// -----------------------------------------------------------------------------
// Feed Model
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    Messages,
    System,
    Apps,
    Unlisted,
}

fn in_category(cat: Category, filter: usize) -> bool {
    match filter {
        0 => true,
        1 => cat == Category::Messages,
        2 => cat == Category::System,
        3 => cat == Category::Apps,
        _ => true,
    }
}

fn ends_with(hay: &[u8], needle: &[u8]) -> bool {
    hay.len() >= needle.len() && &hay[hay.len() - needle.len()..] == needle
}

fn package_known(state: &LauncherState, pkg: &[u8]) -> bool {
    for i in 0..state.app_count.min(state.app_ids.len()) {
        let len = state.app_id_lens[i] as usize;
        if len > 0 && &state.app_ids[i][..len] == pkg {
            return true;
        }
    }
    false
}

fn category_of(rec: &NotificationRecord, state: &LauncherState) -> Category {
    let pkg = &rec.package_id[..rec.pkg_len as usize];
    let chan = &rec.channel[..rec.chan_len as usize];
    if pkg == b"system" || (pkg.is_empty() && chan == b"system") {
        return Category::System;
    }
    if chan == b"messages" {
        return Category::Messages;
    }
    if pkg.is_empty() {
        return Category::Unlisted;
    }
    if !package_known(state, pkg) {
        return Category::Unlisted;
    }
    if ends_with(pkg, b"messages") {
        Category::Messages
    } else {
        Category::Apps
    }
}

/// Vibrant brand squircle color for notifications.
fn squircle_color(cat: Category) -> u32 {
    match cat {
        Category::Messages => 0xFF10B981, // Emerald Green
        Category::System => 0xFF2563EB,   // Royal Blue
        Category::Apps => 0xFF6366F1,     // Indigo
        Category::Unlisted => 0xFFF59E0B, // Amber
    }
}

fn category_slot(cat: Category) -> usize {
    match cat {
        Category::Messages => 1,
        Category::System => 2,
        Category::Apps => 3,
        Category::Unlisted => 0,
    }
}

fn bucket_of(posted: u64, now: Option<u64>) -> usize {
    let Some(now) = now else { return 3 };
    let secs = now.saturating_sub(posted) / TICKS_PER_SEC;
    if secs < 60 {
        0
    } else if secs < 3600 {
        1
    } else {
        2
    }
}

// -----------------------------------------------------------------------------
// Status Bar & Frame Plan
// -----------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct StatusPlan {
    clock: LabelRun,
    battery: Rect,
    battery_dash: Rect,
    tip: Rect,
    wifi_centre: (i32, i32),
    bars: [Rect; 4],
}

impl StatusPlan {
    const EMPTY: StatusPlan = StatusPlan {
        clock: LabelRun::EMPTY,
        battery: Rect::new(0, 0, 0, 0),
        battery_dash: Rect::new(0, 0, 0, 0),
        tip: Rect::new(0, 0, 0, 0),
        wifi_centre: (0, 0),
        bars: [Rect::new(0, 0, 0, 0); 4],
    };
}

struct PillPlan {
    rect: Rect,
    label: LabelRun,
    active: bool,
}

impl PillPlan {
    const EMPTY: PillPlan = PillPlan {
        rect: Rect::new(0, 0, 0, 0),
        label: LabelRun::EMPTY,
        active: false,
    };
}

struct SectionPlan {
    label: LabelRun,
}

impl SectionPlan {
    const EMPTY: SectionPlan = SectionPlan {
        label: LabelRun::EMPTY,
    };
}

#[derive(Clone, Copy)]
struct RowPlan {
    rect: Rect,
    squircle: Rect,
    squircle_color: u32,
    icon_kind: usize, // 0 = messages, 1 = system, 2 = apps, 3 = unlisted
    app_name: LabelRun,
    time: LabelRun,
    title: LabelRun,
    body: LabelRun,
    action1: Rect,
    action1_label: LabelRun,
    action2: Rect,
    action2_label: LabelRun,
}

impl RowPlan {
    const EMPTY: RowPlan = RowPlan {
        rect: Rect::new(0, 0, 0, 0),
        squircle: Rect::new(0, 0, 0, 0),
        squircle_color: 0,
        icon_kind: 0,
        app_name: LabelRun::EMPTY,
        time: LabelRun::EMPTY,
        title: LabelRun::EMPTY,
        body: LabelRun::EMPTY,
        action1: Rect::new(0, 0, 0, 0),
        action1_label: LabelRun::EMPTY,
        action2: Rect::new(0, 0, 0, 0),
        action2_label: LabelRun::EMPTY,
    };
}

struct EventsFrame {
    status: StatusPlan,
    title: LabelRun,
    subtitle: LabelRun,
    source_note: LabelRun,
    filter_note: LabelRun,
    header_clear_btn: Rect,
    header_clear_label: LabelRun,
    pills: [PillPlan; 4],
    sections: [SectionPlan; 4],
    section_count: usize,
    rows: [RowPlan; 8],
    row_count: usize,
    more_note: LabelRun,
    hidden: usize,
    empty_head: LabelRun,
    empty_body: LabelRun,
    clear_btn: Rect,
    clear_label: LabelRun,
    home_pill: Rect,
}

// -----------------------------------------------------------------------------
// Row Layout
// -----------------------------------------------------------------------------

fn row_plan(rec: &NotificationRecord, y: usize, cat: Category, now: Option<u64>, theme: &Theme) -> RowPlan {
    let rect = Rect::new(ROW_X, y, ROW_W, ROW_H);
    let pkg = &rec.package_id[..rec.pkg_len as usize];
    let chan = &rec.channel[..rec.chan_len as usize];

    // Determine readable App Name
    let app_name_bytes: &[u8] = match cat {
        Category::Messages => b"Messages",
        Category::System => b"System",
        Category::Apps => {
            if ends_with(pkg, b"dialer") {
                b"Phone"
            } else if ends_with(pkg, b"calendar") {
                b"Calendar"
            } else if ends_with(pkg, b"vault") {
                b"Vault"
            } else if ends_with(pkg, b"music") {
                b"Music"
            } else if ends_with(pkg, b"settings") {
                b"Settings"
            } else if rec.pkg_len > 0 {
                pkg
            } else {
                b"Application"
            }
        }
        Category::Unlisted => {
            if rec.pkg_len > 0 {
                pkg
            } else if rec.chan_len > 0 {
                chan
            } else {
                b"Alert"
            }
        }
    };

    let title_bytes: &[u8] = if rec.title_len > 0 {
        &rec.title[..rec.title_len as usize]
    } else {
        b"Notification"
    };

    let body_bytes: &[u8] = if rec.body_len > 0 {
        &rec.body[..rec.body_len as usize]
    } else if rec.chan_len > 0 {
        chan
    } else {
        b"No additional details"
    };

    let mut age = [0u8; 16];
    let age_len = write_age(&mut age, rec.timestamp_ticks, now);

    let squircle = Rect::new(ROW_X + 16, y + 14, 28, 28);
    let icon_kind = match cat {
        Category::Messages => 0,
        Category::System => 1,
        Category::Apps => 2,
        Category::Unlisted => 3,
    };

    let app_name = LabelRun::new(
        ROW_TEXT_X,
        y + 16,
        type_scale::BODY,
        app_name_bytes,
        theme.on_surface,
    );

    let time = LabelRun::right(
        ROW_X + ROW_W - 16,
        y + 18,
        type_scale::CAPTION,
        &age[..age_len],
        theme.on_surface_variant,
    );

    let content_w = ROW_W - 72;
    let title = LabelRun::fitted(
        ROW_TEXT_X,
        y + 42,
        type_scale::BODY,
        content_w,
        title_bytes,
        theme.on_surface,
    );

    let body = LabelRun::fitted(
        ROW_TEXT_X,
        y + 66,
        type_scale::CAPTION,
        content_w,
        body_bytes,
        theme.on_surface_variant,
    );

    // Action buttons
    let (act1_name, act2_name): (&[u8], &[u8]) = match cat {
        Category::Messages => (b"Reply", b"Mark read"),
        Category::System => (b"Details", b"Dismiss"),
        _ => (b"Open", b"Dismiss"),
    };

    let action1 = Rect::new(ROW_TEXT_X, y + 88, 68, 24);
    let action1_label = LabelRun::centred(
        action1.x,
        action1.y + 3,
        action1.w,
        type_scale::CAPTION,
        act1_name,
        theme.on_surface,
    );

    let action2 = Rect::new(ROW_TEXT_X + 76, y + 88, 80, 24);
    let action2_label = LabelRun::centred(
        action2.x,
        action2.y + 3,
        action2.w,
        type_scale::CAPTION,
        act2_name,
        theme.on_surface_variant,
    );

    RowPlan {
        rect,
        squircle,
        squircle_color: squircle_color(cat),
        icon_kind,
        app_name,
        time,
        title,
        body,
        action1,
        action1_label,
        action2,
        action2_label,
    }
}

// -----------------------------------------------------------------------------
// Frame Builder
// -----------------------------------------------------------------------------

fn build_events_frame(state: &LauncherState) -> EventsFrame {
    let theme = launcher::theme::SHELL;

    // 1. Status Bar Plan (uniform with app_drawer & home)
    let mid = STATUS_BAR_HEIGHT / 2;
    let battery = Rect::new(
        SCREEN_WIDTH - CONTENT_PADDING - space::XXL - space::SM,
        mid - 7,
        space::XXL + space::SM,
        14,
    );
    let tip = Rect::new(battery.x + battery.w + 2, mid - 3, 4, 6);
    let wifi = Rect::new(battery.x - space::XL - space::SM, mid - 10, space::XXL, 20);
    let bars_right = wifi.x - space::MD;
    let mut bars = [Rect::new(0, 0, 0, 0); 4];
    for (i, slot) in bars.iter_mut().enumerate() {
        let h = 6 + i * 4;
        *slot = Rect::new(bars_right - (4 - i) * 4 - (3 - i) * 4, mid + 8 - h, 4, h);
    }
    let battery_dash = Rect::new(
        battery.x + (battery.w - 6) / 2,
        battery.y + (battery.h - 2) / 2,
        6,
        2,
    );

    let clock_slice: &[u8] = if state.clock_source_ok && state.clock_time[0] != b' ' {
        &state.clock_time
    } else {
        b""
    };
    let status_clock = LabelRun::new(
        CONTENT_PADDING + space::SM,
        (STATUS_BAR_HEIGHT - text::line_height(type_scale::BODY)) / 2,
        type_scale::BODY,
        clock_slice,
        theme.on_surface,
    );

    let status = StatusPlan {
        clock: status_clock,
        battery,
        battery_dash,
        tip,
        wifi_centre: (wifi.center_x() as i32, wifi.y as i32 + 15),
        bars,
    };

    // 2. Header
    let now = interos_std::cap::timer_slot().and_then(|_| interos_std::get_ticks().ok());

    let title = LabelRun::new(HEADER_X, TITLE_Y, type_scale::HEADING, b"Notifications", theme.on_surface);
    let subtitle = LabelRun::new(
        HEADER_X,
        SUBTITLE_Y,
        type_scale::CAPTION,
        b"Everything in one feed",
        theme.on_surface_variant,
    );
    let source_note = LabelRun::new(
        HEADER_X,
        SOURCE_NOTE_Y,
        type_scale::CAPTION,
        b"Recent activity from your apps and system.",
        theme.on_surface_variant,
    );
    let filter_note = LabelRun::new(
        HEADER_X,
        FILTER_NOTE_Y,
        type_scale::CAPTION,
        b"Filter the feed by its real source.",
        theme.on_surface_variant,
    );

    let header_clear_label = LabelRun::centred(
        HEADER_CLEAR_BTN.x,
        HEADER_CLEAR_BTN.y + (HEADER_CLEAR_BTN.h - text::line_height(type_scale::CAPTION)) / 2,
        HEADER_CLEAR_BTN.w,
        type_scale::CAPTION,
        b"Clear all",
        theme.on_surface,
    );

    // 3. Category Filter Counts
    let count = state.notification_count.min(state.notifications.len());
    let mut order = [0usize; 8];
    let mut total = 0usize;
    for i in (0..count).rev() {
        order[total] = i;
        total += 1;
    }

    let mut counts = [0usize; 4];
    counts[0] = total;
    for k in 0..total {
        match category_of(&state.notifications[order[k]], state) {
            Category::Unlisted => {}
            cat => counts[category_slot(cat)] += 1,
        }
    }

    let filter = if state.feed_filter < FILTER_NAMES.len() {
        state.feed_filter
    } else {
        DEFAULT_FILTER
    };

    let mut pills = [PillPlan::EMPTY; 4];
    let mut pill_x = ROW_X;
    for i in 0..4 {
        let mut label = [0u8; 24];
        let mut n = FILTER_NAMES[i].len().min(label.len());
        label[..n].copy_from_slice(&FILTER_NAMES[i][..n]);
        if n < label.len() {
            label[n] = b' ';
            n += 1;
        }
        n = push_dec(&mut label, n, counts[i] as u64).min(label.len());
        let w = text::measure(&label[..n], type_scale::CAPTION) + space::XXL;
        let rect = Rect::new(pill_x, PILLS_Y, w, PILL_H);
        pills[i] = PillPlan {
            rect,
            label: LabelRun::centred(
                rect.x,
                rect.center_y() - text::line_height(type_scale::CAPTION) / 2,
                rect.w,
                type_scale::CAPTION,
                &label[..n],
                if filter == i { theme.surface } else { theme.on_surface_variant },
            ),
            active: filter == i,
        };
        pill_x += w + PILL_GAP;
    }

    let mut matched = 0usize;
    for k in 0..total {
        let cat = category_of(&state.notifications[order[k]], state);
        if in_category(cat, filter) {
            matched += 1;
        }
    }

    // 4. Notification Rows
    let mut rows = [RowPlan::EMPTY; 8];
    let mut sections = [SectionPlan::EMPTY; 4];
    let mut row_count = 0usize;
    let mut section_count = 0usize;
    let mut y = SECTION_HEAD_Y;
    let mut current_bucket = usize::MAX;

    for k in 0..total {
        let rec = state.notifications[order[k]];
        let cat = category_of(&rec, state);
        if !in_category(cat, filter) {
            continue;
        }
        let bucket = bucket_of(rec.timestamp_ticks, now);
        let row_y = if bucket != current_bucket {
            y + space::XL
        } else {
            y
        };
        if row_y + ROW_H > FEED_BOTTOM {
            break;
        }
        if bucket != current_bucket {
            if section_count < sections.len() {
                sections[section_count] = SectionPlan {
                    label: LabelRun::new(36, y, type_scale::CAPTION, SECTION_NAMES[bucket], theme.on_surface),
                };
                section_count += 1;
            }
            current_bucket = bucket;
        }
        if row_count < rows.len() {
            rows[row_count] = row_plan(&rec, row_y, cat, now, &theme);
            row_count += 1;
        }
        y = row_y + ROW_PITCH;
    }

    let hidden = matched.saturating_sub(row_count);
    let mut more = [0u8; 32];
    let more_len = if hidden > 0 {
        let mut n = 1usize;
        more[0] = b'+';
        n = push_dec(&mut more, n, hidden as u64).min(more.len());
        for &b in b" more records not shown" {
            if n < more.len() {
                more[n] = b;
                n += 1;
            }
        }
        n
    } else {
        0
    };

    let last_bottom = if row_count > 0 {
        y - (ROW_PITCH - ROW_H)
    } else {
        0
    };
    let note_y = if last_bottom + text::line_height(type_scale::CAPTION) + space::SM <= FEED_BOTTOM {
        last_bottom + space::MD
    } else {
        CLEAR_BTN.y + CLEAR_BTN.h + space::MD
    };
    let more_note = LabelRun::new(36, note_y, type_scale::CAPTION, &more[..more_len], theme.on_surface_variant);

    // 5. Empty State
    let (empty_head, empty_body) = if total == 0 {
        (
            LabelRun::centred(ROW_X, 420, ROW_W, type_scale::HEADING, b"No new notifications", theme.on_surface),
            LabelRun::centred(
                ROW_X,
                420 + text::line_height(type_scale::HEADING) + space::MD,
                ROW_W,
                type_scale::BODY,
                b"All caught up with apps and system updates.",
                theme.on_surface_variant,
            ),
        )
    } else {
        (LabelRun::EMPTY, LabelRun::EMPTY)
    };

    let clear_label = LabelRun::centred(
        CLEAR_BTN.x,
        CLEAR_BTN.y + (CLEAR_BTN.h - text::line_height(type_scale::BODY)) / 2,
        CLEAR_BTN.w,
        type_scale::BODY,
        b"Clear all",
        theme.on_surface,
    );

    EventsFrame {
        status,
        title,
        subtitle,
        source_note,
        filter_note,
        header_clear_btn: HEADER_CLEAR_BTN,
        header_clear_label,
        pills,
        sections,
        section_count,
        rows,
        row_count,
        more_note,
        hidden,
        empty_head,
        empty_body,
        clear_btn: CLEAR_BTN,
        clear_label,
        home_pill: HOME_PILL,
    }
}

// -----------------------------------------------------------------------------
// Vector Icon Renderers (inside squircle)
// -----------------------------------------------------------------------------

#[inline]
fn render_icon_glyph(cx: usize, cy: usize, kind: usize, x: usize, y: usize) -> bool {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    match kind {
        0 => {
            // Messages: Chat bubble
            let bubble = (dx >= -7 && dx <= 7 && dy >= -6 && dy <= 3)
                && !(dx.abs() >= 6 && (dy == -6 || dy == 3));
            let tail = (dy >= 4 && dy <= 6) && (dx >= -6 && dx <= -3) && (dx + dy <= 0);
            bubble || tail
        }
        1 => {
            // System: Gear / Settings
            let d2 = dx * dx + dy * dy;
            let ring = d2 <= 36 && d2 >= 9;
            let teeth = (dx.abs() <= 2 && dy.abs() <= 8) || (dy.abs() <= 2 && dx.abs() <= 8);
            ring || (teeth && d2 <= 64)
        }
        2 => {
            // Apps: 4 squircle app dots (2x2 grid)
            let in_tl = (dx >= -6 && dx <= -2) && (dy >= -6 && dy <= -2);
            let in_tr = (dx >= 2 && dx <= 6) && (dy >= -6 && dy <= -2);
            let in_bl = (dx >= -6 && dx <= -2) && (dy >= 2 && dy <= 6);
            let in_br = (dx >= 2 && dx <= 6) && (dy >= 2 && dy <= 6);
            in_tl || in_tr || in_bl || in_br
        }
        _ => {
            // Alert bell / Star
            let d2 = dx * dx + dy * dy;
            d2 <= 25
        }
    }
}

// -----------------------------------------------------------------------------
// Per-Pixel Renderer
// -----------------------------------------------------------------------------

static mut FRAME_CACHE: Option<EventsFrame> = None;

#[inline(never)]
pub fn filter_band() -> (usize, usize) {
    (
        PILLS_Y,
        PILLS_Y.saturating_add(PILL_H).max(FEED_BOTTOM),
    )
}

pub fn filter_pill_at(x: usize, y: usize, state: &LauncherState) -> Option<usize> {
    if y < PILLS_Y || y >= PILLS_Y + PILL_H {
        return None;
    }
    let f = build_events_frame(state);
    f.pills
        .iter()
        .position(|pill| pill.rect.contains(x, y))
}

pub fn get_notifications_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let f = unsafe {
        let slot: *mut Option<EventsFrame> = core::ptr::addr_of_mut!(FRAME_CACHE);
        if (x == 0 && y == 0) || (*slot).is_none() {
            *slot = Some(build_events_frame(state));
        }
        (*slot).as_ref().unwrap()
    };
    let theme = launcher::theme::SHELL;

    // 1. Status Bar (y < STATUS_BAR_HEIGHT)
    if y < STATUS_BAR_HEIGHT {
        if y == STATUS_BAR_HEIGHT - 1 {
            return theme.border;
        }
        let s = &f.status;
        if let Some(c) = s.clock.pixel_on(x, y, theme.surface) {
            return c;
        }
        if rounded_contains(s.battery, x, y, radius::CHIP) {
            if rounded_outline(s.battery, x, y, radius::CHIP) {
                return theme.on_surface;
            }
            if s.battery_dash.contains(x, y) {
                return theme.on_surface_variant;
            }
            return theme.surface_container_low;
        }
        if s.tip.contains(x, y) {
            return theme.on_surface;
        }
        let wdx = x as i32 - s.wifi_centre.0;
        let wdy = y as i32 - s.wifi_centre.1;
        let wd2 = wdx * wdx + wdy * wdy;
        if wdy <= 0 && ((36..=64).contains(&wd2) || (100..=144).contains(&wd2)) {
            return theme.on_surface;
        }
        if wd2 <= 9 {
            return theme.on_surface;
        }
        for bar in s.bars.iter() {
            if bar.contains(x, y) {
                return theme.on_surface;
            }
        }
        return theme.surface;
    }

    // 2. Header block: Title, Subtitle, Notes, and Header Clear All pill
    if let Some(c) = f.title.pixel_on(x, y, theme.surface) {
        return c;
    }
    if let Some(c) = f.subtitle.pixel_on(x, y, theme.surface) {
        return c;
    }
    if let Some(c) = f.source_note.pixel_on(x, y, theme.surface) {
        return c;
    }
    if let Some(c) = f.filter_note.pixel_on(x, y, theme.surface) {
        return c;
    }

    // Header "Clear all" button
    if f.header_clear_btn.contains(x, y) {
        let rad = f.header_clear_btn.h / 2;
        if rounded_contains(f.header_clear_btn, x, y, rad) {
            if rounded_outline(f.header_clear_btn, x, y, rad) {
                return theme.border;
            }
            if let Some(c) = f.header_clear_label.pixel_on(x, y, theme.surface_container_low) {
                return c;
            }
            return theme.surface_container_low;
        }
        return theme.surface;
    }

    // 3. Category Filter Pills
    if y >= PILLS_Y && y < PILLS_Y + PILL_H {
        for pill in &f.pills {
            if pill.rect.contains(x, y) {
                let rad = pill.rect.h / 2;
                if rounded_contains(pill.rect, x, y, rad) {
                    if pill.active {
                        if rounded_outline(pill.rect, x, y, rad) {
                            return theme.on_surface;
                        }
                        if let Some(c) = pill.label.pixel_on(x, y, theme.on_surface) {
                            return c;
                        }
                        return theme.on_surface;
                    }
                    if rounded_outline(pill.rect, x, y, rad) {
                        return theme.border;
                    }
                    if let Some(c) = pill.label.pixel_on(x, y, theme.surface_container_low) {
                        return c;
                    }
                    return theme.surface_container_low;
                }
                return theme.surface;
            }
        }
    }

    // 4. Section headers
    for i in 0..f.section_count {
        if let Some(c) = f.sections[i].label.pixel_on(x, y, theme.surface) {
            return c;
        }
    }

    // 5. Notification Cards
    for i in 0..f.row_count {
        let row = &f.rows[i];
        if row.rect.contains(x, y) {
            if rounded_contains(row.rect, x, y, radius::CARD) {
                if rounded_outline(row.rect, x, y, radius::CARD) {
                    return theme.border;
                }

                // App squircle icon (28x28)
                if rounded_contains(row.squircle, x, y, 7) {
                    if render_icon_glyph(
                        row.squircle.center_x(),
                        row.squircle.center_y(),
                        row.icon_kind,
                        x,
                        y,
                    ) {
                        return 0xFFFFFFFF; // Crisp white glyph
                    }
                    return row.squircle_color;
                }

                // App name & time
                if let Some(c) = row.app_name.pixel_on(x, y, theme.surface_container) {
                    return c;
                }
                if let Some(c) = row.time.pixel_on(x, y, theme.surface_container) {
                    return c;
                }

                // Title & body
                if let Some(c) = row.title.pixel_on(x, y, theme.surface_container) {
                    return c;
                }
                if let Some(c) = row.body.pixel_on(x, y, theme.surface_container) {
                    return c;
                }

                // Action button 1
                if rounded_contains(row.action1, x, y, 12) {
                    if rounded_outline(row.action1, x, y, 12) {
                        return theme.border;
                    }
                    if let Some(c) = row.action1_label.pixel_on(x, y, theme.surface_container_high) {
                        return c;
                    }
                    return theme.surface_container_high;
                }

                // Action button 2
                if rounded_contains(row.action2, x, y, 12) {
                    if rounded_outline(row.action2, x, y, 12) {
                        return theme.border;
                    }
                    if let Some(c) = row.action2_label.pixel_on(x, y, theme.surface_container_low) {
                        return c;
                    }
                    return theme.surface_container_low;
                }

                return theme.surface_container;
            }
            return theme.surface;
        }
    }

    // 6. Notes: +N more records, or empty state
    if let Some(c) = f.more_note.pixel_on(x, y, theme.surface) {
        return c;
    }

    if f.empty_head.len > 0 {
        // Empty state bell illustration
        let cx = SCREEN_WIDTH / 2;
        let cy = 370;
        let dx = x as i32 - cx as i32;
        let dy = y as i32 - cy as i32;
        let d2 = dx * dx + dy * dy;
        if d2 <= 900 { // 30px radius circle
            let bell_body = (dx.abs() <= 12 && dy >= -10 && dy <= 6)
                || (dx.abs() <= 16 && dy >= 6 && dy <= 10);
            let clapper = dx.abs() <= 4 && dy >= 11 && dy <= 14;
            if bell_body || clapper {
                return theme.on_surface_variant;
            }
        }

        if let Some(c) = f.empty_head.pixel_on(x, y, theme.surface) {
            return c;
        }
        if let Some(c) = f.empty_body.pixel_on(x, y, theme.surface) {
            return c;
        }
    }

    // 7. Bottom Clear All Button (CLEAR_BTN)
    if f.clear_btn.contains(x, y) {
        if rounded_contains(f.clear_btn, x, y, 22) {
            if rounded_outline(f.clear_btn, x, y, 22) {
                return theme.border;
            }
            if let Some(c) = f.clear_label.pixel_on(x, y, theme.surface_container_low) {
                return c;
            }
            return theme.surface_container_low;
        }
        return theme.surface;
    }

    // 8. Home Indicator Pill
    if rounded_contains(f.home_pill, x, y, 2) {
        return theme.on_surface_variant;
    }

    theme.surface
}
