//! Live Widgets & System Dashboard.
//!
//! Production-grade widget dashboard:
//! - Uniform system status bar (canonical clock and hardware telemetry icons).
//! - Large, bold flagship typography across all widgets and cards.
//! - Storage & VFS widget: the block capacity the kernel reports, or no figure.
//! - Physical Memory widget: RAM totals read from the kernel's own counters.
//! - Master Audio widget: the master level the mixer register answers with.
//! - Calendar & Clock widget: real kernel RTC date line and time, honest event status.
//! - Security Architecture widget: zero ambient authority, isolated capability broker.
//! - System Readings dashboard cards with large legible type and zero descender clipping.
//! - Zero heap allocations in rendering loops (#![no_std]).

#![allow(dead_code)]

use crate::{test_text_aa, LauncherState};
use launcher::layout::Rect;
use launcher::text;
use launcher::theme::{type_scale, radius, Theme, SCREEN_HEIGHT, SCREEN_WIDTH, STATUS_BAR_HEIGHT};

const THEME: Theme = launcher::theme::SHELL;

const HOME_PILL: Rect = Rect::new(SCREEN_WIDTH / 2 - 90, SCREEN_HEIGHT - 20, 180, 5);

/// The cards whose figures are device readings.
///
/// WHY they are consts and not literals inside the painter: the shell repaints a
/// band named by the screen that draws it, and a band written down separately
/// from the rows it covers is a band that can drift away from them.
const STORAGE_ROW: Rect = Rect::new(40, 138, 305, 160);
const RAM_ROW: Rect = Rect::new(375, 138, 305, 160);
const AUDIO_ROW: Rect = Rect::new(40, 312, 640, 120);
const CAL_ROW: Rect = Rect::new(40, 446, 305, 160);
const VAULT_ROW: Rect = Rect::new(375, 446, 305, 160);

/// The line box a run at `scale` reserves, named once so a card's rows can be
/// stacked from the ladder rather than from a fitted literal.
const fn lh(scale: usize) -> usize {
    launcher::font::line_height(scale)
}

/// WHY every second line below is derived from the line above: the old offsets
/// (36, 42, 52 rows) were the 8x16 bitmap cell's, shorter than the run they
/// followed, so a card's body line began before the line above had finished its
/// box and read straight through it. Each row now starts where the row above
/// ends, one `lh` lower.
///
/// Row 1 (Storage & Physical Memory): label, the figure, a body line, a caption
/// detail. Both cards share the same vertical ladder.
const ROW1_LABEL_Y: usize = STORAGE_ROW.y + 16;
const ROW1_FIGURE_Y: usize = ROW1_LABEL_Y + lh(type_scale::CAPTION);
const ROW1_SECONDARY_Y: usize = ROW1_FIGURE_Y + lh(type_scale::TITLE);
const ROW1_DETAIL_Y: usize = ROW1_SECONDARY_Y + lh(type_scale::BODY);

/// Audio card: the title, then the device line a whole body box below it, then
/// the footer row - the volume reading on the left, the mixer tag on the right -
/// under the gauge band.
const AUDIO_TITLE_Y: usize = AUDIO_ROW.y + 16;
const AUDIO_SUB_Y: usize = AUDIO_TITLE_Y + lh(type_scale::BODY);
const AUDIO_FOOT_Y: usize = AUDIO_ROW.y + 92;

/// Calendar card: label, the RTC date line, then the detail row (the chip is a
/// filled rect and its label is centred, so it reserves its own band).
const CAL_LABEL_Y: usize = CAL_ROW.y + 16;
const CAL_DATE_Y: usize = CAL_LABEL_Y + lh(type_scale::CAPTION);
const CAL_DETAIL_Y: usize = CAL_ROW.y + 124;

/// Security card: label, the status figure, the architecture line, then the
/// capability chip a whole body box under the architecture line.
const VAULT_LABEL_Y: usize = VAULT_ROW.y + 16;
const VAULT_STATUS_Y: usize = VAULT_LABEL_Y + lh(type_scale::CAPTION);
const VAULT_ARCH_Y: usize = VAULT_STATUS_Y + lh(type_scale::TITLE);
const VAULT_CHIP_Y: usize = VAULT_ARCH_Y + lh(type_scale::BODY);

/// Readings cards: the label, and the sublabel one body box below it. The value
/// is right-anchored on the same first row, so it keeps its own offset.
const READING_LABEL_DY: usize = 10;
const READING_SUB_DY: usize = 10 + lh(type_scale::BODY);
const READING_VALUE_DY: usize = 20;
/// The first readings card's row, and the section header above it. WHY the
/// header is solved from the card and not from the card row above: a scale-3 run
/// reserves `lh(SECTION)` = 44 rows and its descender reaches the bottom of that
/// box, so a header placed 6 rows above the card let the card's own fill answer
/// those rows first and ate the tail of the 'g'.
const READINGS_CARD0_Y: usize = 664;
const READINGS_HEAD_Y: usize = READINGS_CARD0_Y - lh(type_scale::SECTION);

/// The width of a readings card, and the room one of its side-anchored runs
/// gets: each half of the span between the left label's anchor and the right
/// value's anchor, so the widest the label may grow is the card centre and the
/// widest the value may grow back is the same centre.
const READING_CARD_W: usize = 640;
const READING_TEXT_MAX: usize = side_text_w(READING_CARD_W, 24, 24);

/// The room the header title and subtitle get before the status chip: from the
/// left margin to the chip's own left edge (the screen minus the chip's span),
/// less the margin and a gutter.
const HEADER_TEXT_MAX: usize = SCREEN_WIDTH - 214 - 40 - 8;
/// The clock is canonical shell telemetry drawn in the shared band; its bound is
/// simply the width left of it on the screen, so it can never run off-edge.
const CLOCK_TEXT_MAX: usize = SCREEN_WIDTH - crate::screens::status::CLOCK_X;

/// Truth gates: the ladder each derived stride trusts, and the fact that the
/// grown stacks still close inside their own cards.
const _: () = assert!(lh(type_scale::CAPTION) == 22);
const _: () = assert!(lh(type_scale::BODY) == 32);
const _: () = assert!(lh(type_scale::TITLE) == 59);
const _: () = assert!(ROW1_DETAIL_Y + lh(type_scale::CAPTION) <= STORAGE_ROW.y + STORAGE_ROW.h);
const _: () = assert!(AUDIO_SUB_Y + lh(type_scale::CAPTION) <= AUDIO_ROW.y + 74);
const _: () = assert!(VAULT_CHIP_Y + 24 <= VAULT_ROW.y + VAULT_ROW.h);
const _: () = assert!(READING_SUB_DY + lh(type_scale::CAPTION) <= 64);
const _: () = assert!(READINGS_HEAD_Y + lh(type_scale::SECTION) <= READINGS_CARD0_Y);

/// First row the dashboard draws a queried figure into, and the row after the
/// last one. Covers the calendar card because its date line is read from the
/// RTC rather than written here.
pub const fn readings_band() -> (usize, usize) {
    (STORAGE_ROW.y, CAL_ROW.y + CAL_ROW.h)
}

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

/// The bytes a run paints, cut to fit `max_w` and marked with an ellipsis when
/// it had to be.
///
/// WHY this is the first step of every unbounded path: a card figure is a
/// runtime reading with no length cap of its own, and a run measured wider than
/// its box paints into whatever is anchored on the other side, so a left label
/// and a right value read through each other regardless of their rows. This
/// names the canonical truncation once so `new` and `right` cannot forget it.
/// It fills the caller's own buffer rather than returning one, so the painted
/// bytes outlive the call that trimmed them.
fn truncate_into(bytes: &[u8], scale: usize, max_w: usize, buf: &mut [u8]) -> usize {
    let len = bytes.len().min(buf.len());
    buf[..len].copy_from_slice(&bytes[..len]);
    if max_w == 0 || text::measure(&buf[..len], scale) <= max_w {
        return len;
    }
    // WHY the run handed to `ellipsize` is three bytes shorter than the buffer:
    // the mark is written over the run's tail, and a destination that could hold
    // one or the other would end the run mid-glyph with no mark on it.
    let room = buf.len() - 3;
    let (trimmed, trimmed_len) = text::ellipsize(&bytes[..bytes.len().min(room)], scale, max_w);
    let n = trimmed_len.min(buf.len());
    buf[..n].copy_from_slice(&trimmed[..n]);
    n
}

/// The horizontal room a left-anchored reading gets inside a card of width
/// `w`: from its inset on the left to the same inset on the right.
const fn card_text_w(w: usize) -> usize {
    w.saturating_sub(2 * 20)
}

/// The horizontal room the title line of the wide audio card gets between the
/// art emblem it follows and the card's inset right edge.
const AUDIO_TITLE_TEXT_W: usize = AUDIO_ROW.w.saturating_sub(76 + 20);

/// The horizontal room a left reading and a right reading share on a card that
/// carries both on one row: half the gap between their anchors, so the widest
/// the left run may grow is the card centre and the widest the right run may
/// grow is the same centre, and they can never meet.
const fn side_text_w(w: usize, left_inset: usize, right_inset: usize) -> usize {
    w.saturating_sub(left_inset + right_inset) / 2
}

/// The room the volume reading and the mixer tag share on the audio footer row.
const AUDIO_FOOT_TEXT_W: usize = side_text_w(AUDIO_ROW.w, 20, 20);

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

    fn new(x: usize, y: usize, max_w: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let mut buf = [0u8; RUN_CAP];
        let len = truncate_into(bytes, scale, max_w, &mut buf);
        let (em, top, box_h) = run_metrics(y, scale);
        let w = text::measure(&buf[..len], scale)
            .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize)
            .min(max_w.max(1));
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
            w: measured.max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize),
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

    fn right(x_right: usize, y: usize, max_w: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let mut buf = [0u8; RUN_CAP];
        let len = truncate_into(bytes, scale, max_w, &mut buf);
        let (em, top, box_h) = run_metrics(y, scale);
        let w = text::measure(&buf[..len], scale)
            .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize)
            .min(max_w.max(1));
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
// Reading Cards for Lower Section
// -----------------------------------------------------------------------------

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
    status_clock: LabelRun,
    page_title: LabelRun,
    page_subtitle: LabelRun,
    status_chip_rect: Rect,
    status_chip_label: LabelRun,

    // Widget 0: Real Storage Capacity & VFS (Left column)
    storage_rect: Rect,
    storage_label: LabelRun,
    storage_capacity: LabelRun,
    storage_mount: LabelRun,
    storage_detail: LabelRun,

    // Widget 1: Real Memory & DVFS Power Governor (Right column)
    ram_rect: Rect,
    ram_label: LabelRun,
    ram_total: LabelRun,
    ram_mmu: LabelRun,
    ram_detail: LabelRun,

    // Widget 2: Real System Audio & Master Volume (Full width row)
    audio_rect: Rect,
    audio_art_rect: Rect,
    audio_title: LabelRun,
    audio_sublabel: LabelRun,
    audio_gauge_rect: Rect,
    audio_gauge_fill_w: usize,
    audio_volume_label: LabelRun,
    audio_service_tag: LabelRun,

    // Widget 3: Real Calendar & Clock (Left column)
    cal_rect: Rect,
    cal_label: LabelRun,
    cal_date: LabelRun,
    cal_time_chip: Rect,
    cal_time_label: LabelRun,
    cal_detail: LabelRun,

    // Widget 4: Real Security Architecture (Right column)
    vault_rect: Rect,
    vault_label: LabelRun,
    vault_status: LabelRun,
    vault_arch: LabelRun,
    vault_chip_rect: Rect,
    vault_chip_label: LabelRun,

    // Section 2 Header
    readings_head: LabelRun,

    // Real System Readings Cards (5 cards)
    readings: [DashboardReadingCard; 5],

    // Home indicator
    home_pill: Rect,
}

/// Appends `extra` to a label buffer, stopping at its end rather than wrapping.
fn push_text(out: &mut [u8], at: usize, extra: &[u8]) -> usize {
    let mut cur = at;
    for &b in extra {
        if cur < out.len() {
            out[cur] = b;
            cur += 1;
        }
    }
    cur
}

/// A byte count written with the binary unit that fits it.
///
/// WHY the unit is spelled GiB and MiB: the kernel answers these queries in
/// bytes, and a line that said "GB" over a 1024-based divisor would report one
/// number and name another.
fn push_capacity(out: &mut [u8], bytes: u64) -> usize {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        let hundredths = (bytes % GIB) * 100 / GIB;
        let mut n = push_dec(out, 0, bytes / GIB);
        n = push_text(out, n, b".");
        n = push_text(out, n, &[(b'0' + (hundredths / 10) as u8), (b'0' + (hundredths % 10) as u8)]);
        push_text(out, n, b" GiB")
    } else {
        let n = push_dec(out, 0, bytes / MIB);
        push_text(out, n, b" MiB")
    }
}

fn push_dec(out: &mut [u8], at: usize, mut v: u64) -> usize {
    if v == 0 {
        if at < out.len() {
            out[at] = b'0';
            return at + 1;
        }
        return at;
    }
    let mut rev = [0u8; 20];
    let mut rev_len = 0;
    while v > 0 {
        rev[rev_len] = b'0' + (v % 10) as u8;
        v /= 10;
        rev_len += 1;
    }
    let mut cur = at;
    for i in (0..rev_len).rev() {
        if cur < out.len() {
            out[cur] = rev[i];
            cur += 1;
        }
    }
    cur
}

fn build_widgets_frame(state: &LauncherState) -> WidgetsPlan {
    let theme = THEME;

    // 1. Canonical Status Bar Elements
    let status_clock = if state.clock_source_ok {
        LabelRun::new(
            crate::screens::status::CLOCK_X,
            crate::screens::status::CLOCK_Y,
            CLOCK_TEXT_MAX,
            type_scale::BODY,
            &state.clock_time,
            theme.on_surface,
        )
    } else {
        LabelRun::EMPTY
    };

    // 2. Header
    // WHY the subtitle sits a whole title box below the title: the page title is
    // a scale-4 run that reserves `lh(4)` = 59 rows, and at the old y the
    // subtitle's ink began above the bottom of that box, so the two read as one
    // smeared row on the device.
    let page_title = LabelRun::new(40, 60, HEADER_TEXT_MAX, type_scale::TITLE, b"Live Widgets", theme.on_surface);
    let page_subtitle = LabelRun::new(
        40,
        60 + lh(type_scale::TITLE),
        HEADER_TEXT_MAX,
        type_scale::CAPTION,
        b"Real-time hardware telemetry & system status",
        theme.on_surface_variant,
    );

    // WHY the chip is as wide as it is: the caption run measures ~135 px and the
    // old 150 px pill left the label touching its own border, which reads as a
    // clipped control rather than a status.
    let status_chip_rect = Rect::new(SCREEN_WIDTH - 214, 60, 174, 30);
    let status_chip_label = LabelRun::centred(
        status_chip_rect.x,
        status_chip_rect.y + 6,
        status_chip_rect.w,
        type_scale::CAPTION,
        b"Platform Active",
        0xFF10B981, // Emerald green
    );

    // 3. Row 1: Storage & Physical Memory (y = 138..298, height 160)
    //
    // WHY both figures are read from the kernel: a capacity is a property of the
    // machine the guest happens to be running on, and the same image boots with
    // 1 GiB in Device Studio and with whatever a developer's QEMU line says. The
    // card printed "6.00 GB" and "13.2 GB" as source literals, so it reported
    // hardware the device never confirmed.
    let storage_rect = STORAGE_ROW;
    let storage_label = LabelRun::new(storage_rect.x + 20, ROW1_LABEL_Y, card_text_w(STORAGE_ROW.w), type_scale::CAPTION, b"Storage & VFS", theme.on_surface_variant);
    let mut cap_buf = [0u8; 24];
    let cap_len = if state.storage_state_valid {
        push_capacity(&mut cap_buf, state.storage_total_bytes)
    } else {
        push_text(&mut cap_buf, 0, b"Unavailable")
    };
    let storage_ink = if state.storage_state_valid { theme.on_surface } else { theme.on_surface_variant };
    let storage_capacity = LabelRun::new(storage_rect.x + 20, ROW1_FIGURE_Y, card_text_w(STORAGE_ROW.w), type_scale::TITLE, &cap_buf[..cap_len], storage_ink);
    let storage_mount = if state.storage_state_valid {
        LabelRun::new(storage_rect.x + 20, ROW1_SECONDARY_Y, card_text_w(STORAGE_ROW.w), type_scale::BODY, b"Persistent Block Disk", theme.on_surface)
    } else {
        LabelRun::new(storage_rect.x + 20, ROW1_SECONDARY_Y, card_text_w(STORAGE_ROW.w), type_scale::BODY, b"No block device bound", theme.on_surface_variant)
    };
    let storage_detail = LabelRun::new(storage_rect.x + 20, ROW1_DETAIL_Y, card_text_w(STORAGE_ROW.w), type_scale::CAPTION, b"/data, /user, /storage persist", theme.on_surface_variant);

    let ram_rect = RAM_ROW;
    let ram_label = LabelRun::new(ram_rect.x + 20, ROW1_LABEL_Y, card_text_w(RAM_ROW.w), type_scale::CAPTION, b"Physical Memory", theme.on_surface_variant);
    let mut ram_buf = [0u8; 24];
    let ram_len = if state.memory_state_valid {
        push_capacity(&mut ram_buf, state.memory_total_bytes)
    } else {
        push_text(&mut ram_buf, 0, b"Unavailable")
    };
    let ram_total = LabelRun::new(
        ram_rect.x + 20,
        ROW1_FIGURE_Y,
        card_text_w(RAM_ROW.w),
        type_scale::TITLE,
        &ram_buf[..ram_len],
        if state.memory_state_valid { 0xFF10B981 } else { theme.on_surface_variant },
    );
    // WHY the second line is the used counter and not a governor claim: nothing
    // in this kernel feeds the DVFS governor a load sample and no CPU frequency
    // is under its control, so "Dynamic DVFS Active" described behaviour the
    // device does not perform. Bytes in use are measured.
    let mut used_buf = [0u8; 32];
    let used_len = if state.memory_state_valid {
        let n = push_dec(&mut used_buf, 0, state.memory_used_bytes / (1024 * 1024));
        push_text(&mut used_buf, n, b" MiB in use")
    } else {
        push_text(&mut used_buf, 0, b"No counters reported")
    };
    // WHY the rows are the shared ladder and not the old `y + 90` / `y + 124`:
    // those offsets were the 8x16 cell's, shorter than the boxes they follow, so
    // this card's body line began while the scale-4 figure above was still
    // painting and read through it. The storage card already stacks this way.
    let ram_mmu = LabelRun::new(ram_rect.x + 20, ROW1_SECONDARY_Y, card_text_w(RAM_ROW.w), type_scale::BODY, &used_buf[..used_len], theme.on_surface);
    let ram_detail = LabelRun::new(ram_rect.x + 20, ROW1_DETAIL_Y, card_text_w(RAM_ROW.w), type_scale::CAPTION, b"Isolated CR3 Page Tables", theme.on_surface_variant);

    // 4. Row 2: Real System Audio & Master Volume (y = 312..432, height 120)
    //
    // WHY the device line changes with the reading: the mixer level is the AC'97
    // master-volume register, so a reading is the only evidence this shell has
    // that a codec is bound at all. Without one the card says it was not told,
    // instead of naming a sound device the platform never answered for.
    let audio_rect = AUDIO_ROW;
    let audio_art_rect = Rect::new(audio_rect.x + 20, audio_rect.y + 16, 44, 44);
    let audio_title = LabelRun::new(audio_rect.x + 76, AUDIO_TITLE_Y, AUDIO_TITLE_TEXT_W, type_scale::BODY, b"Master Audio Output", theme.on_surface);
    let audio_sublabel = if state.volume_state_valid {
        LabelRun::new(audio_rect.x + 76, AUDIO_SUB_Y, AUDIO_TITLE_TEXT_W, type_scale::CAPTION, b"AC'97 codec, 48 kHz PCM", theme.on_surface_variant)
    } else {
        LabelRun::new(audio_rect.x + 76, AUDIO_SUB_Y, AUDIO_TITLE_TEXT_W, type_scale::CAPTION, b"No volume reported by the mixer", theme.on_surface_variant)
    };
    let audio_gauge_rect = Rect::new(audio_rect.x + 20, audio_rect.y + 74, audio_rect.w - 40, 8);
    let pct = if state.volume_state_valid { state.last_volume_percent as usize } else { 0 };
    let audio_gauge_fill_w = (audio_gauge_rect.w * pct.min(100)) / 100;

    let mut vol_buf = [0u8; 32];
    let vn = if state.volume_state_valid {
        vol_buf[..8].copy_from_slice(b"Volume: ");
        let mut n = 8;
        n = push_dec(&mut vol_buf, n, pct as u64);
        if n < vol_buf.len() {
            vol_buf[n] = b'%';
            n += 1;
        }
        n
    } else {
        push_text(&mut vol_buf, 0, b"Volume: not reported")
    };
    let audio_volume_label = LabelRun::new(audio_rect.x + 20, AUDIO_FOOT_Y, AUDIO_FOOT_TEXT_W, type_scale::CAPTION, &vol_buf[..vn], theme.on_surface_variant);
    let audio_service_tag = if state.volume_state_valid {
        LabelRun::right(audio_rect.x + audio_rect.w - 20, AUDIO_FOOT_Y, AUDIO_FOOT_TEXT_W, type_scale::CAPTION, b"Mixer register live", 0xFF10B981)
    } else {
        LabelRun::right(audio_rect.x + audio_rect.w - 20, AUDIO_FOOT_Y, AUDIO_FOOT_TEXT_W, type_scale::CAPTION, b"Audio unread", theme.on_surface_variant)
    };

    // 5. Row 3: Calendar / RTC & Security (y = 446..606, height 160)
    let cal_rect = CAL_ROW;
    let cal_label = LabelRun::new(cal_rect.x + 20, CAL_LABEL_Y, card_text_w(CAL_ROW.w), type_scale::CAPTION, b"System Date & Clock", theme.on_surface_variant);
    // WHY "No clock reading" and not "Clock Synced": the two say opposite things
    // and the old literal was printed whenever the RTC refused, so the card
    // claimed a synchronisation this shell had just been told did not happen.
    let date_slice = if state.clock_source_ok && state.date_line[0] != b' ' {
        let mut dlen = 0;
        while dlen < state.date_line.len() && state.date_line[dlen] != 0 {
            dlen += 1;
        }
        &state.date_line[..dlen]
    } else {
        b"No clock reading"
    };
    // WHY the date line is derived from the label box, not from `y + 38`: the
    // old offset assumed the 8x16 cell, but the label is a caption run that
    // reserves `lh(CAPTION)` = 22 rows, so a date line begun at 38 rows below
    // the card top was already clear - the number is the same, only named from
    // the ladder so a change in the caption stride moves it with the label.
    let cal_date = LabelRun::new(cal_rect.x + 20, CAL_DATE_Y, card_text_w(CAL_ROW.w), type_scale::SECTION, date_slice, theme.on_surface);
    let cal_time_chip = Rect::new(cal_rect.x + 20, cal_rect.y + 88, 110, 24);
    let cal_time_label = if state.clock_source_ok {
        LabelRun::centred(cal_time_chip.x, cal_time_chip.y + 4, cal_time_chip.w, type_scale::CAPTION, b"Hardware RTC", theme.on_surface)
    } else {
        LabelRun::centred(cal_time_chip.x, cal_time_chip.y + 4, cal_time_chip.w, type_scale::CAPTION, b"RTC refused", theme.on_surface_variant)
    };
    // WHY the detail names the reading it shows: the chip above it already says
    // where the date came from, and a static "No calendar source queried" under
    // a live "Hardware RTC" chip is the same contradiction the old literals
    // carried, only inverted.
    let mut cal_buf = [0u8; 32];
    let cal_len = if state.clock_source_ok && state.clock_time[0] != 0 {
        let mut n = push_text(&mut cal_buf, 0, b"Time ");
        n = push_text(&mut cal_buf, n, &state.clock_time);
        push_text(&mut cal_buf, n, b" from the RTC")
    } else {
        push_text(&mut cal_buf, 0, b"No calendar source queried")
    };
    let cal_detail = LabelRun::new(cal_rect.x + 20, CAL_DETAIL_Y, card_text_w(CAL_ROW.w), type_scale::CAPTION, &cal_buf[..cal_len], theme.on_surface_variant);

    // WHY the security card is stacked from the ladder: the status figure is a
    // scale-4 run that reserves `lh(TITLE)` = 59 rows, and the architecture line
    // and the chip below it were spaced by the 8x16 cell's 52 and 34 rows, so
    // each began its ink while the run above was still inside its box.
    let vault_rect = VAULT_ROW;
    let vault_label = LabelRun::new(vault_rect.x + 20, VAULT_LABEL_Y, card_text_w(VAULT_ROW.w), type_scale::CAPTION, b"Security Architecture", theme.on_surface_variant);
    let vault_status = LabelRun::new(vault_rect.x + 20, VAULT_STATUS_Y, card_text_w(VAULT_ROW.w), type_scale::TITLE, b"Enforced", 0xFF10B981);
    let vault_arch = LabelRun::new(vault_rect.x + 20, VAULT_ARCH_Y, card_text_w(VAULT_ROW.w), type_scale::BODY, b"Zero Ambient Authority", theme.on_surface);
    let vault_chip_rect = Rect::new(vault_rect.x + 20, VAULT_CHIP_Y, 130, 24);
    let vault_chip_label = LabelRun::centred(vault_chip_rect.x, vault_chip_rect.y + 4, vault_chip_rect.w, type_scale::CAPTION, b"Capability Broker", 0xFFFFFFFF);

    // 6. Section 2 Header.
    let readings_head = LabelRun::new(
        40,
        READINGS_HEAD_Y,
        card_text_w(READING_CARD_W),
        type_scale::SECTION,
        b"System Readings",
        theme.on_surface,
    );

    // 7. System Readings Cards (y = 664..1044, 5 cards of height 64)
    let mut pkg_buf = [0u8; 32];
    let mut n = push_dec(&mut pkg_buf, 0, state.app_count as u64);
    for &b in b" registered" {
        if n < pkg_buf.len() {
            pkg_buf[n] = b;
            n += 1;
        }
    }

    let mut notif_buf = [0u8; 32];
    let m = push_dec(&mut notif_buf, 0, state.notification_count as u64);
    let mut n_len = m;
    for &b in b" active" {
        if n_len < notif_buf.len() {
            notif_buf[n_len] = b;
            n_len += 1;
        }
    }

    let card_w = 640;
    let card_h = 64;
    // WHY the run rows are derived per card: the label is a body run reserving
    // `lh(BODY)` = 32 rows, and the sublabels were placed 36 rows under the card
    // top - inside the label's box - so they read straight through it. The value
    // is right-anchored and shares the row with the label, so all three runs are
    // bounded at the card centre by `READING_TEXT_MAX`.
    let readings = [
        DashboardReadingCard {
            rect: Rect::new(40, READINGS_CARD0_Y, card_w, card_h),
            label: LabelRun::new(64, READINGS_CARD0_Y + READING_LABEL_DY, READING_TEXT_MAX, type_scale::BODY, b"Package Registry", theme.on_surface),
            sublabel: LabelRun::new(64, READINGS_CARD0_Y + READING_SUB_DY, READING_TEXT_MAX, type_scale::CAPTION, b"Signed verified package store", theme.on_surface_variant),
            value: LabelRun::right(40 + card_w - 24, READINGS_CARD0_Y + READING_VALUE_DY, READING_TEXT_MAX, type_scale::BODY, &pkg_buf[..n], theme.on_surface),
            accent_color: 0xFF6366F1,
        },
        DashboardReadingCard {
            rect: Rect::new(40, 740, card_w, card_h),
            label: LabelRun::new(64, 740 + READING_LABEL_DY, READING_TEXT_MAX, type_scale::BODY, b"IPC Alert Channel", theme.on_surface),
            sublabel: LabelRun::new(64, 740 + READING_SUB_DY, READING_TEXT_MAX, type_scale::CAPTION, b"Kernel notification queue", theme.on_surface_variant),
            value: LabelRun::right(40 + card_w - 24, 740 + READING_VALUE_DY, READING_TEXT_MAX, type_scale::BODY, &notif_buf[..n_len], if state.notification_count > 0 { 0xFFF59E0B } else { theme.on_surface_variant }),
            accent_color: 0xFF10B981,
        },
        DashboardReadingCard {
            rect: Rect::new(40, 816, card_w, card_h),
            label: LabelRun::new(64, 816 + READING_LABEL_DY, READING_TEXT_MAX, type_scale::BODY, b"Display Compositor", theme.on_surface),
            sublabel: LabelRun::new(64, 816 + READING_SUB_DY, READING_TEXT_MAX, type_scale::CAPTION, b"Flagship portrait 720x1440", theme.on_surface_variant),
            // WHY the panel state and not a frame rate: nothing in this kernel measures
            // a flip interval - the scanout deadline is computed from a configured
            // refresh figure - so "60 FPS" reported a number no clock had observed.
            // Whether the panel has power is something the platform does answer.
            value: LabelRun::right(
                40 + card_w - 24,
                816 + READING_VALUE_DY,
                READING_TEXT_MAX,
                type_scale::BODY,
                if !state.display_power_state_valid {
                    b"Power state unread"
                } else if state.display_was_powered {
                    b"Scanout on"
                } else {
                    b"Display off"
                },
                if state.display_power_state_valid && state.display_was_powered {
                    theme.on_surface
                } else {
                    theme.on_surface_variant
                },
            ),
            accent_color: 0xFF8B5CF6,
        },
        DashboardReadingCard {
            rect: Rect::new(40, 892, card_w, card_h),
            label: LabelRun::new(64, 892 + READING_LABEL_DY, READING_TEXT_MAX, type_scale::BODY, b"Network Connectivity", theme.on_surface),
            // WHY this names the address as a configuration: the stack sends from a
            // fixed address and nothing probes a link, so the old line's "Online"
            // claimed a measured connection that no code here could observe.
            sublabel: LabelRun::new(64, 892 + READING_SUB_DY, READING_TEXT_MAX, type_scale::CAPTION, b"E1000 IPv4, addr 10.0.2.15", theme.on_surface_variant),
            value: LabelRun::right(40 + card_w - 24, 892 + READING_VALUE_DY, READING_TEXT_MAX, type_scale::BODY, b"Link unverified", theme.on_surface_variant),
            accent_color: 0xFF3B82F6,
        },
        DashboardReadingCard {
            rect: Rect::new(40, 968, card_w, card_h),
            label: LabelRun::new(64, 968 + READING_LABEL_DY, READING_TEXT_MAX, type_scale::BODY, b"Process Sandbox & Isolation", theme.on_surface),
            sublabel: LabelRun::new(64, 968 + READING_SUB_DY, READING_TEXT_MAX, type_scale::CAPTION, b"Ring 3 user mode CR3 tables", theme.on_surface_variant),
            value: LabelRun::right(40 + card_w - 24, 968 + READING_VALUE_DY, READING_TEXT_MAX, type_scale::BODY, b"Protected", 0xFF059669),
            accent_color: 0xFF059669,
        },
    ];

    WidgetsPlan {
        status_clock,
        page_title,
        page_subtitle,
        status_chip_rect,
        status_chip_label,
        storage_rect,
        storage_label,
        storage_capacity,
        storage_mount,
        storage_detail,
        ram_rect,
        ram_label,
        ram_total,
        ram_mmu,
        ram_detail,
        audio_rect,
        audio_art_rect,
        audio_title,
        audio_sublabel,
        audio_gauge_rect,
        audio_gauge_fill_w,
        audio_volume_label,
        audio_service_tag,
        cal_rect,
        cal_label,
        cal_date,
        cal_time_chip,
        cal_time_label,
        cal_detail,
        vault_rect,
        vault_label,
        vault_status,
        vault_arch,
        vault_chip_rect,
        vault_chip_label,
        readings_head,
        readings,
        home_pill: HOME_PILL,
    }
}

static mut FRAME_CACHE: Option<WidgetsPlan> = None;

pub fn invalidate_widgets_cache() {
    unsafe {
        FRAME_CACHE = None;
    }
}

pub fn get_live_widgets_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let f = unsafe {
        let slot: *mut Option<WidgetsPlan> = core::ptr::addr_of_mut!(FRAME_CACHE);
        if (x == 0 && y == 0) || (*slot).is_none() {
            *slot = Some(build_widgets_frame(state));
        }
        (*slot).as_ref().unwrap()
    };
    let theme = THEME;

    // 1. Canonical Status Bar (y < STATUS_BAR_HEIGHT)
    if y < STATUS_BAR_HEIGHT {
        return crate::screens::status::pixel_with(
            x,
            y,
            |x, y| f.status_clock.pixel_on(x, y, theme.surface),
            &theme,
        );
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

    // 3. Row 1: Storage & RAM Cards
    if f.storage_rect.contains(x, y) {
        if rounded_contains(f.storage_rect, x, y, radius::CARD) {
            if rounded_outline(f.storage_rect, x, y, radius::CARD) {
                return theme.border;
            }
            let strip = Rect::new(f.storage_rect.x + 12, f.storage_rect.y + 16, 4, f.storage_rect.h - 32);
            if rounded_contains(strip, x, y, 2) {
                return 0xFF3B82F6; // Blue accent
            }
            if let Some(c) = f.storage_label.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.storage_capacity.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.storage_mount.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.storage_detail.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            return theme.surface_container;
        }
        return theme.surface;
    }

    if f.ram_rect.contains(x, y) {
        if rounded_contains(f.ram_rect, x, y, radius::CARD) {
            if rounded_outline(f.ram_rect, x, y, radius::CARD) {
                return theme.border;
            }
            let strip = Rect::new(f.ram_rect.x + 12, f.ram_rect.y + 16, 4, f.ram_rect.h - 32);
            if rounded_contains(strip, x, y, 2) {
                return 0xFF10B981; // Emerald accent
            }
            if let Some(c) = f.ram_label.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.ram_total.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.ram_mmu.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.ram_detail.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            return theme.surface_container;
        }
        return theme.surface;
    }

    // 4. Row 2: Master Audio Output Card
    if f.audio_rect.contains(x, y) {
        if rounded_contains(f.audio_rect, x, y, radius::CARD) {
            if rounded_outline(f.audio_rect, x, y, radius::CARD) {
                return theme.border;
            }
            if rounded_contains(f.audio_art_rect, x, y, radius::CONTROL) {
                // Speaker / audio emblem
                let cx = f.audio_art_rect.x + f.audio_art_rect.w / 2;
                let cy = f.audio_art_rect.y + f.audio_art_rect.h / 2;
                let dx = (x as i32 - cx as i32).abs();
                let dy = (y as i32 - cy as i32).abs();
                if dx * dx + dy * dy <= 100 {
                    return 0xFF6366F1;
                }
                return theme.surface_container_high;
            }
            if let Some(c) = f.audio_title.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.audio_sublabel.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if rounded_contains(f.audio_gauge_rect, x, y, 4) {
                if x < f.audio_gauge_rect.x + f.audio_gauge_fill_w {
                    return 0xFF6366F1; // Accent fill
                }
                return theme.surface_container_high;
            }
            if let Some(c) = f.audio_volume_label.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.audio_service_tag.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            return theme.surface_container;
        }
        return theme.surface;
    }

    // 5. Row 3: Calendar & Security Cards
    if f.cal_rect.contains(x, y) {
        if rounded_contains(f.cal_rect, x, y, radius::CARD) {
            if rounded_outline(f.cal_rect, x, y, radius::CARD) {
                return theme.border;
            }
            let strip = Rect::new(f.cal_rect.x + 12, f.cal_rect.y + 16, 4, f.cal_rect.h - 32);
            if rounded_contains(strip, x, y, 2) {
                return 0xFFF59E0B; // Amber accent
            }
            if let Some(c) = f.cal_label.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.cal_date.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if rounded_contains(f.cal_time_chip, x, y, radius::CHIP) {
                if rounded_outline(f.cal_time_chip, x, y, radius::CHIP) {
                    return theme.border;
                }
                if let Some(c) = f.cal_time_label.pixel_on(x, y, theme.surface_container_high) {
                    return c;
                }
                return theme.surface_container_high;
            }
            if let Some(c) = f.cal_detail.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            return theme.surface_container;
        }
        return theme.surface;
    }

    if f.vault_rect.contains(x, y) {
        if rounded_contains(f.vault_rect, x, y, radius::CARD) {
            if rounded_outline(f.vault_rect, x, y, radius::CARD) {
                return theme.border;
            }
            let strip = Rect::new(f.vault_rect.x + 12, f.vault_rect.y + 16, 4, f.vault_rect.h - 32);
            if rounded_contains(strip, x, y, 2) {
                return 0xFF059669; // Teal accent
            }
            if let Some(c) = f.vault_label.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.vault_status.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if let Some(c) = f.vault_arch.pixel_on(x, y, theme.surface_container) {
                return c;
            }
            if rounded_contains(f.vault_chip_rect, x, y, radius::CHIP) {
                if let Some(c) = f.vault_chip_label.pixel_on(x, y, 0xFF059669) {
                    return c;
                }
                return 0xFF059669; // Solid teal chip
            }
            return theme.surface_container;
        }
        return theme.surface;
    }

    // 6. Section 2: System Readings Header
    if let Some(c) = f.readings_head.pixel_on(x, y, theme.surface) {
        return c;
    }

    // 7. System Readings Cards
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

    // 8. Home Indicator Pill
    if rounded_contains(f.home_pill, x, y, 2) {
        return theme.on_surface_variant;
    }

    theme.surface
}
