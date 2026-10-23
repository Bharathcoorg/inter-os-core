//! Live Widgets & System Dashboard.
//!
//! Production-grade widget dashboard:
//! - Uniform system status bar (bold clock, signal bars, Wi-Fi fan, battery status).
//! - Large, bold flagship typography across all widgets and cards.
//! - Weather & Climate widget: high-contrast glyphs, large temperature, condition.
//! - Device Health & Battery widget: charge level, detailed gauge bar, status chip.
//! - Live Media Player widget: album art squircle, disc glyph, progress timeline, controls.
//! - Calendar & Upcoming Events widget: vibrant accent indicator bar, time chip, location.
//! - Security Vault widget: sovereign status, encryption badge.
//! - System Readings dashboard cards with large legible type and zero descender clipping.
//! - Zero heap allocations in rendering loops.

#![allow(dead_code)]

use crate::{test_text_aa, LauncherState};
use launcher::layout::Rect;
use launcher::text;
use launcher::theme::{space, type_scale, radius, Theme, SCREEN_HEIGHT, SCREEN_WIDTH, STATUS_BAR_HEIGHT, CONTENT_PADDING};

const THEME: Theme = launcher::theme::SHELL;

// -----------------------------------------------------------------------------
// The five entry widgets (coordinates match touch handler in main.rs)
// -----------------------------------------------------------------------------
const WIDGET_WEATHER: Rect = Rect::new(40, 135, 305, 180);
const WIDGET_HEALTH: Rect = Rect::new(375, 135, 305, 180);
const WIDGET_MEDIA: Rect = Rect::new(40, 335, 640, 150);
const WIDGET_CALENDAR: Rect = Rect::new(40, 505, 305, 180);
const WIDGET_VAULT: Rect = Rect::new(375, 505, 305, 180);

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

#[derive(Clone, Copy)]
struct DashboardReadingCard {
    rect: Rect,
    label: LabelRun,
    sublabel: LabelRun,
    value: LabelRun,
    accent_color: u32,
}

impl DashboardReadingCard {
    const EMPTY: DashboardReadingCard = DashboardReadingCard {
        rect: Rect::new(0, 0, 0, 0),
        label: LabelRun::EMPTY,
        sublabel: LabelRun::EMPTY,
        value: LabelRun::EMPTY,
        accent_color: 0,
    };
}

struct WidgetsPlan {
    status: StatusPlan,
    page_title: LabelRun,
    page_subtitle: LabelRun,
    status_chip_rect: Rect,
    status_chip_label: LabelRun,

    // Widget 0: Weather
    weather_rect: Rect,
    weather_label: LabelRun,
    weather_temp: LabelRun,
    weather_condition: LabelRun,
    weather_details: LabelRun,

    // Widget 1: Health & Battery
    health_rect: Rect,
    health_label: LabelRun,
    health_pct: LabelRun,
    health_gauge_rect: Rect,
    health_status: LabelRun,

    // Widget 2: Media Player
    media_rect: Rect,
    media_art_rect: Rect,
    media_title: LabelRun,
    media_artist: LabelRun,
    media_progress_rect: Rect,
    media_time_left: LabelRun,
    media_time_right: LabelRun,
    media_btn_prev: LabelRun,
    media_btn_play: Rect,
    media_btn_play_glyph: LabelRun,
    media_btn_next: LabelRun,

    // Widget 3: Calendar
    cal_rect: Rect,
    cal_accent_bar: Rect,
    cal_time_chip: Rect,
    cal_time_label: LabelRun,
    cal_title: LabelRun,
    cal_location: LabelRun,
    cal_next: LabelRun,

    // Widget 4: Security Vault
    vault_rect: Rect,
    vault_label: LabelRun,
    vault_status: LabelRun,
    vault_specs: LabelRun,
    vault_badge_rect: Rect,
    vault_badge_label: LabelRun,

    // Lower readings section
    readings_head: LabelRun,
    readings: [DashboardReadingCard; 5],
    home_pill: Rect,
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

// -----------------------------------------------------------------------------
// Frame Builder
// -----------------------------------------------------------------------------

fn build_widgets_frame(state: &LauncherState) -> WidgetsPlan {
    let theme = THEME;

    // 1. Status Bar
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
    let page_title = LabelRun::new(40, 50, type_scale::HEADING, b"Live Widgets", theme.on_surface);
    let page_subtitle = LabelRun::new(
        40,
        96,
        type_scale::CAPTION,
        b"Live system overview & upcoming schedule",
        theme.on_surface_variant,
    );

    let status_chip_rect = Rect::new(SCREEN_WIDTH - 200, 52, 160, 32);
    let status_chip_label = LabelRun::centred(
        status_chip_rect.x,
        status_chip_rect.y + 7,
        status_chip_rect.w,
        type_scale::CAPTION,
        b"System Nominal",
        0xFF10B981, // Emerald green
    );

    // 3. Widget 0: Weather
    let weather_rect = WIDGET_WEATHER;
    let weather_label = LabelRun::new(weather_rect.x + 20, weather_rect.y + 18, type_scale::BODY, b"Weather", theme.on_surface_variant);
    let weather_temp = LabelRun::new(weather_rect.x + 20, weather_rect.y + 44, type_scale::DISPLAY, b"22\xc2\xb0C", theme.on_surface);
    let weather_condition = LabelRun::new(weather_rect.x + 20, weather_rect.y + 104, type_scale::BODY, b"Partly Cloudy", theme.on_surface);
    let weather_details = LabelRun::new(weather_rect.x + 20, weather_rect.y + 138, type_scale::CAPTION, b"H: 26\xc2\xb0  L: 18\xc2\xb0 \xe2\x80\xa2 Clear", theme.on_surface_variant);

    // 4. Widget 1: Health & Battery
    let health_rect = WIDGET_HEALTH;
    let health_label = LabelRun::new(health_rect.x + 20, health_rect.y + 18, type_scale::BODY, b"Device Health", theme.on_surface_variant);
    let health_pct = LabelRun::new(health_rect.x + 20, health_rect.y + 44, type_scale::DISPLAY, b"98%", 0xFF10B981);
    let health_gauge_rect = Rect::new(health_rect.x + 20, health_rect.y + 112, health_rect.w - 40, 8);
    let health_status = LabelRun::new(health_rect.x + 20, health_rect.y + 138, type_scale::CAPTION, b"Normal \xe2\x80\xa2 Battery Healthy", theme.on_surface_variant);

    // 5. Widget 2: Media Player
    let media_rect = WIDGET_MEDIA;
    let media_art_rect = Rect::new(media_rect.x + 20, media_rect.y + 20, 64, 64);
    let media_title = LabelRun::new(media_rect.x + 100, media_rect.y + 22, type_scale::BODY, b"Symphony No. 9 in D minor", theme.on_surface);
    let media_artist = LabelRun::new(media_rect.x + 100, media_rect.y + 52, type_scale::CAPTION, b"Ludwig van Beethoven \xe2\x80\xa2 InterOS Hi-Fi", theme.on_surface_variant);
    let media_progress_rect = Rect::new(media_rect.x + 20, media_rect.y + 100, media_rect.w - 40, 6);
    let media_time_left = LabelRun::new(media_rect.x + 20, media_rect.y + 116, type_scale::CAPTION, b"2:45", theme.on_surface_variant);
    let media_time_right = LabelRun::right(media_rect.x + media_rect.w - 20, media_rect.y + 116, type_scale::CAPTION, b"-1:27", theme.on_surface_variant);

    let media_btn_prev = LabelRun::centred(media_rect.x + media_rect.w - 140, media_rect.y + 36, 28, type_scale::BODY, b"|<", theme.on_surface);
    let media_btn_play = Rect::new(media_rect.x + media_rect.w - 100, media_rect.y + 30, 36, 36);
    let media_btn_play_glyph = LabelRun::centred(media_btn_play.x, media_btn_play.y + 6, media_btn_play.w, type_scale::BODY, b"||", 0xFFFFFFFF);
    let media_btn_next = LabelRun::centred(media_rect.x + media_rect.w - 52, media_rect.y + 36, 28, type_scale::BODY, b">|", theme.on_surface);

    // 6. Widget 3: Calendar
    let cal_rect = WIDGET_CALENDAR;
    let cal_accent_bar = Rect::new(cal_rect.x + 14, cal_rect.y + 18, 4, 144);
    let cal_time_chip = Rect::new(cal_rect.x + 26, cal_rect.y + 18, 96, 24);
    let cal_time_label = LabelRun::centred(cal_time_chip.x, cal_time_chip.y + 3, cal_time_chip.w, type_scale::CAPTION, b"10:30 AM", theme.on_surface);
    let cal_title = LabelRun::new(cal_rect.x + 26, cal_rect.y + 54, type_scale::BODY, b"Architecture Sync", theme.on_surface);
    let cal_location = LabelRun::new(cal_rect.x + 26, cal_rect.y + 84, type_scale::CAPTION, b"Room 402 \xe2\x80\xa2 Core Review", theme.on_surface_variant);
    let cal_next = LabelRun::new(cal_rect.x + 26, cal_rect.y + 124, type_scale::CAPTION, b"2:00 PM \xe2\x80\xa2 Kernel Verify", theme.on_surface_variant);

    // 7. Widget 4: Security Vault
    let vault_rect = WIDGET_VAULT;
    let vault_label = LabelRun::new(vault_rect.x + 20, vault_rect.y + 18, type_scale::BODY, b"Security Vault", theme.on_surface_variant);
    let vault_status = LabelRun::new(vault_rect.x + 20, vault_rect.y + 46, type_scale::HEADING, b"Protected", theme.on_surface);
    let vault_specs = LabelRun::new(vault_rect.x + 20, vault_rect.y + 92, type_scale::CAPTION, b"Zero Ambient Authority", theme.on_surface_variant);
    let vault_badge_rect = Rect::new(vault_rect.x + 20, vault_rect.y + 126, 114, 26);
    let vault_badge_label = LabelRun::centred(vault_badge_rect.x, vault_badge_rect.y + 4, vault_badge_rect.w, type_scale::CAPTION, b"Encrypted", 0xFFFFFFFF);

    // 8. Lower Readings section
    let readings_head = LabelRun::new(40, 715, type_scale::HEADING, b"System Readings", theme.on_surface);

    let mut pkg_buf = [0u8; 32];
    let mut n = push_dec(&mut pkg_buf, 0, state.app_count as u64);
    for &b in b" registered" {
        if n < pkg_buf.len() {
            pkg_buf[n] = b;
            n += 1;
        }
    }

    let mut notif_buf = [0u8; 32];
    let mut m = push_dec(&mut notif_buf, 0, state.notification_count as u64);
    for &b in b" active" {
        if m < notif_buf.len() {
            notif_buf[m] = b;
            m += 1;
        }
    }

    let card_w = 640;
    let card_h = 76;
    let readings = [
        DashboardReadingCard {
            rect: Rect::new(40, 760, card_w, card_h),
            label: LabelRun::new(64, 772, type_scale::BODY, b"Real-Time Clock", theme.on_surface),
            sublabel: LabelRun::new(64, 800, type_scale::CAPTION, b"Hardware PIT timer synchronization", theme.on_surface_variant),
            value: LabelRun::right(40 + card_w - 24, 782, type_scale::BODY, if state.clock_source_ok { &state.clock_time } else { b"No RTC" }, if state.clock_source_ok { 0xFF10B981 } else { theme.on_surface_variant }),
            accent_color: 0xFF3B82F6,
        },
        DashboardReadingCard {
            rect: Rect::new(40, 852, card_w, card_h),
            label: LabelRun::new(64, 864, type_scale::BODY, b"Applications Registry", theme.on_surface),
            sublabel: LabelRun::new(64, 892, type_scale::CAPTION, b"Signed verified package store", theme.on_surface_variant),
            value: LabelRun::right(40 + card_w - 24, 874, type_scale::BODY, &pkg_buf[..n], theme.on_surface),
            accent_color: 0xFF6366F1,
        },
        DashboardReadingCard {
            rect: Rect::new(40, 944, card_w, card_h),
            label: LabelRun::new(64, 956, type_scale::BODY, b"Notification Service", theme.on_surface),
            sublabel: LabelRun::new(64, 984, type_scale::CAPTION, b"Kernel IPC alert channel queue", theme.on_surface_variant),
            value: LabelRun::right(40 + card_w - 24, 966, type_scale::BODY, &notif_buf[..m], if state.notification_count > 0 { 0xFFF59E0B } else { theme.on_surface_variant }),
            accent_color: 0xFF10B981,
        },
        DashboardReadingCard {
            rect: Rect::new(40, 1036, card_w, card_h),
            label: LabelRun::new(64, 1048, type_scale::BODY, b"Appearance & Engine", theme.on_surface),
            sublabel: LabelRun::new(64, 1076, type_scale::CAPTION, b"Flagship portrait 720x1440 compositor", theme.on_surface_variant),
            value: LabelRun::right(40 + card_w - 24, 1058, type_scale::BODY, b"Light \xe2\x80\xa2 60 FPS", theme.on_surface),
            accent_color: 0xFF8B5CF6,
        },
        DashboardReadingCard {
            rect: Rect::new(40, 1128, card_w, card_h),
            label: LabelRun::new(64, 1140, type_scale::BODY, b"System Sandbox & Memory", theme.on_surface),
            sublabel: LabelRun::new(64, 1168, type_scale::CAPTION, b"Isolated page tables \xe2\x80\xa2 Ring 3 user mode", theme.on_surface_variant),
            value: LabelRun::right(40 + card_w - 24, 1150, type_scale::BODY, b"Enforced", 0xFF10B981),
            accent_color: 0xFF059669,
        },
    ];

    WidgetsPlan {
        status,
        page_title,
        page_subtitle,
        status_chip_rect,
        status_chip_label,
        weather_rect,
        weather_label,
        weather_temp,
        weather_condition,
        weather_details,
        health_rect,
        health_label,
        health_pct,
        health_gauge_rect,
        health_status,
        media_rect,
        media_art_rect,
        media_title,
        media_artist,
        media_progress_rect,
        media_time_left,
        media_time_right,
        media_btn_prev,
        media_btn_play,
        media_btn_play_glyph,
        media_btn_next,
        cal_rect,
        cal_accent_bar,
        cal_time_chip,
        cal_time_label,
        cal_title,
        cal_location,
        cal_next,
        vault_rect,
        vault_label,
        vault_status,
        vault_specs,
        vault_badge_rect,
        vault_badge_label,
        readings_head,
        readings,
        home_pill: HOME_PILL,
    }
}

// -----------------------------------------------------------------------------
// Vector Glyphs
// -----------------------------------------------------------------------------

#[inline]
fn render_sun_glyph(cx: usize, cy: usize, x: usize, y: usize) -> bool {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    let d2 = dx * dx + dy * dy;
    // Central disc
    if d2 <= 64 {
        return true;
    }
    // 8 radial rays
    if d2 >= 100 && d2 <= 169 {
        let is_cardinal = (dx.abs() <= 2 && dy.abs() >= 10) || (dy.abs() <= 2 && dx.abs() >= 10);
        let is_diagonal = (dx - dy).abs() <= 2 || (dx + dy).abs() <= 2;
        return is_cardinal || is_diagonal;
    }
    false
}

#[inline]
fn render_bolt_glyph(cx: usize, cy: usize, x: usize, y: usize) -> bool {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    if dy < 0 && dy >= -10 {
        return dx >= -dy / 2 - 2 && dx <= -dy / 2 + 2;
    }
    if dy >= 0 && dy <= 10 {
        return dx >= -dy / 2 && dx <= -dy / 2 + 4;
    }
    false
}

#[inline]
fn render_shield_glyph(cx: usize, cy: usize, x: usize, y: usize) -> bool {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    if dx.abs() > 10 || dy < -8 || dy > 12 {
        return false;
    }
    if dy <= 2 {
        return dx.abs() <= 10;
    }
    let bottom_limit = 12 - dx.abs();
    dy <= bottom_limit
}

// -----------------------------------------------------------------------------
// Per-Pixel Renderer
// -----------------------------------------------------------------------------

static mut FRAME_CACHE: Option<WidgetsPlan> = None;

pub fn get_live_widgets_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let f = unsafe {
        let slot: *mut Option<WidgetsPlan> = core::ptr::addr_of_mut!(FRAME_CACHE);
        if (x == 0 && y == 0) || (*slot).is_none() {
            *slot = Some(build_widgets_frame(state));
        }
        (*slot).as_ref().unwrap()
    };
    let theme = THEME;

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

    // 2. Header
    if let Some(c) = f.page_title.pixel_on(x, y, theme.surface) {
        return c;
    }
    if let Some(c) = f.page_subtitle.pixel_on(x, y, theme.surface) {
        return c;
    }
    if f.status_chip_rect.contains(x, y) {
        let rad = f.status_chip_rect.h / 2;
        if rounded_contains(f.status_chip_rect, x, y, rad) {
            if rounded_outline(f.status_chip_rect, x, y, rad) {
                return 0x3310B981; // subtle green border
            }
            if let Some(c) = f.status_chip_label.pixel_on(x, y, theme.surface_container_low) {
                return c;
            }
            return theme.surface_container_low;
        }
    }

    // 3. Widget 0: Weather
    if f.weather_rect.contains(x, y) {
        if rounded_contains(f.weather_rect, x, y, radius::CARD) {
            if rounded_outline(f.weather_rect, x, y, radius::CARD) {
                return theme.border;
            }
            // Sun glyph at top right
            if render_sun_glyph(f.weather_rect.x + f.weather_rect.w - 36, f.weather_rect.y + 36, x, y) {
                return 0xFFF59E0B; // Amber gold
            }
            if let Some(c) = f.weather_label.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.weather_temp.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.weather_condition.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.weather_details.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            return theme.surface_container;
        }
        return theme.surface;
    }

    // 4. Widget 1: Health & Battery
    if f.health_rect.contains(x, y) {
        if rounded_contains(f.health_rect, x, y, radius::CARD) {
            if rounded_outline(f.health_rect, x, y, radius::CARD) {
                return theme.border;
            }
            // Bolt glyph at top right
            if render_bolt_glyph(f.health_rect.x + f.health_rect.w - 36, f.health_rect.y + 36, x, y) {
                return 0xFF10B981;
            }
            if let Some(c) = f.health_label.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.health_pct.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            // Battery progress track
            if rounded_contains(f.health_gauge_rect, x, y, 4) {
                let fill_w = f.health_gauge_rect.w * 98 / 100;
                if x < f.health_gauge_rect.x + fill_w {
                    return 0xFF10B981;
                }
                return theme.surface_container_high;
            }
            if let Some(c) = f.health_status.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            return theme.surface_container;
        }
        return theme.surface;
    }

    // 5. Widget 2: Media Player
    if f.media_rect.contains(x, y) {
        if rounded_contains(f.media_rect, x, y, radius::CARD) {
            if rounded_outline(f.media_rect, x, y, radius::CARD) {
                return theme.border;
            }
            // Album art squircle
            if rounded_contains(f.media_art_rect, x, y, 14) {
                let cx = f.media_art_rect.center_x() as i32;
                let cy = f.media_art_rect.center_y() as i32;
                let dx = x as i32 - cx;
                let dy = y as i32 - cy;
                let d2 = dx * dx + dy * dy;
                // Vinyl disc
                if d2 <= 324 && (d2 >= 256 || (d2 >= 100 && d2 <= 144) || d2 <= 25) {
                    return 0xFFFFFFFF;
                }
                return 0xFF7C3AED; // Violet gradient base
            }
            if let Some(c) = f.media_title.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.media_artist.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            // Progress bar
            if rounded_contains(f.media_progress_rect, x, y, 3) {
                let fill_w = f.media_progress_rect.w * 65 / 100;
                if x < f.media_progress_rect.x + fill_w {
                    return 0xFF8B5CF6;
                }
                return theme.surface_container_high;
            }
            if let Some(c) = f.media_time_left.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.media_time_right.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            // Play button circle
            if rounded_contains(f.media_btn_play, x, y, 18) {
                if let Some(c) = f.media_btn_play_glyph.pixel_on(x, y, 0xFF8B5CF6) {
                    return c;
                }
                return 0xFF8B5CF6;
            }
            if let Some(c) = f.media_btn_prev.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.media_btn_next.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            return theme.surface_container;
        }
        return theme.surface;
    }

    // 6. Widget 3: Calendar
    if f.cal_rect.contains(x, y) {
        if rounded_contains(f.cal_rect, x, y, radius::CARD) {
            if rounded_outline(f.cal_rect, x, y, radius::CARD) {
                return theme.border;
            }
            // Vibrant accent bar
            if rounded_contains(f.cal_accent_bar, x, y, 2) {
                return 0xFFF59E0B; // Vibrant amber
            }
            // Time chip
            if rounded_contains(f.cal_time_chip, x, y, 12) {
                if let Some(c) = f.cal_time_label.pixel_on(x, y, theme.surface_container_high) {
                    return c;
                }
                return theme.surface_container_high;
            }
            if let Some(c) = f.cal_title.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.cal_location.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.cal_next.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            return theme.surface_container;
        }
        return theme.surface;
    }

    // 7. Widget 4: Security Vault
    if f.vault_rect.contains(x, y) {
        if rounded_contains(f.vault_rect, x, y, radius::CARD) {
            if rounded_outline(f.vault_rect, x, y, radius::CARD) {
                return theme.border;
            }
            // Shield glyph
            if render_shield_glyph(f.vault_rect.x + f.vault_rect.w - 36, f.vault_rect.y + 36, x, y) {
                return 0xFF10B981;
            }
            if let Some(c) = f.vault_label.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.vault_status.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.vault_specs.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            // Sovereign chip
            if rounded_contains(f.vault_badge_rect, x, y, 13) {
                if let Some(c) = f.vault_badge_label.pixel_on(x, y, 0xFF10B981) {
                    return c;
                }
                return 0xFF10B981;
            }
            return theme.surface_container;
        }
        return theme.surface;
    }

    // 8. Lower Readings section
    if let Some(c) = f.readings_head.pixel_on(x, y, theme.surface) {
        return c;
    }

    for card in &f.readings {
        if card.rect.contains(x, y) {
            if rounded_contains(card.rect, x, y, radius::CARD) {
                if rounded_outline(card.rect, x, y, radius::CARD) {
                    return theme.border;
                }
                // Vertical accent strip on left
                let strip = Rect::new(card.rect.x + 12, card.rect.y + 16, 4, card.rect.h - 32);
                if rounded_contains(strip, x, y, 2) {
                    return card.accent_color;
                }
                if let Some(c) = card.label.pixel_on(x, y, theme.surface_container) {
                    return c;
                }
                if let Some(c) = card.sublabel.pixel_on(x, y, theme.surface_container) {
                    return c;
                }
                if let Some(c) = card.value.pixel_on(x, y, theme.surface_container) {
                    return c;
                }
                return theme.surface_container;
            }
            return theme.surface;
        }
    }

    // 9. Home Indicator Pill
    if rounded_contains(f.home_pill, x, y, 2) {
        return theme.on_surface_variant;
    }

    theme.surface
}
