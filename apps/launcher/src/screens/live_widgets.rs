// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
use launcher::theme::{type_scale, radius, Theme, SCREEN_WIDTH, STATUS_BAR_HEIGHT};

const THEME: Theme = launcher::theme::SHELL;

/// The home mark this screen paints, taken from the platform instead of from
/// a measurement of this screen.
///
/// WHY it was wrong: the dashboard drew the mark 180 columns by FIVE rows at
/// (270, 1420) while the shade beside it drew the same mark 180 by SIX at
/// (270, 1416), the drawer drew it 192 by six at (264, 1405) and the power sheet
/// filled 201 columns by seven at (260, 1410) -- four objects for one gesture,
/// painted by the shell every application is told to copy, while the box that
/// answered the gesture on each of those surfaces was a cut across the panel's
/// whole width rather than the mark's neighbourhood. Five rows is also under a
/// seventh of the 44-row touch floor the platform promises a thumb.
///
/// WHAT THIS CHANGE DID NOT MOVE: the control centre's own 160 x 8 indicator at
/// (280, 1428), which is the anchor the whole shade panel is centred from, and the
/// home surface's three-control bar in `launcher::nav`, whose 88 x 11 home capsule
/// sits inside committed 240 x 60 thirds. Both are named here so that the number of
/// home marks on this device is stated rather than implied.
///
/// WHAT it is now: `design::home_band::PILL`, the platform's mark -- 200 columns
/// by six, centred, at rows 1418..=1423 -- and the box that answers it,
/// `design::home_band::HIT`, 240 columns by 48 reaching the panel's last row.
/// The painter below asks the platform's own raster for the ink, so the shape
/// cannot drift from the shape the rest of the tree draws; this rectangle is left
/// for the fit gate under it, which has to know the row the card column stops
/// above.
///
/// WHY the launcher's `Rect` and not the SDK's: the frame and the gates on this
/// surface are typed in `launcher::layout::Rect`, so the four fields are copied
/// rather than a second rectangle type threaded through the painter.
const HOME_PILL: Rect = Rect::new(
    interos_std::design::home_band::PILL.x,
    interos_std::design::home_band::PILL.y,
    interos_std::design::home_band::PILL.w,
    interos_std::design::home_band::PILL.h,
);

/// The mark on this screen is the platform's, to the row and the column.
///
/// WHY restated rather than assumed: the gate below keeps the card column above
/// `HOME_PILL.y`, so it only means what it says if `HOME_PILL` still is
/// `home_band::PILL`. The old rectangle satisfied neither, and no check on this
/// surface could tell.
const _: () = assert!(
    HOME_PILL.y == interos_std::design::home_band::PILL.y
        && HOME_PILL.h == interos_std::design::home_band::PILL_H
        && HOME_PILL.w == interos_std::design::home_band::PILL_W,
    "the dashboard's home mark must be the platform's mark, not a rectangle of its own",
);

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
/// WHY the cards and the row pitch are derived from the same ladder: a stack of
/// whole line boxes is only as tall as the boxes are, and the raised ladder grew
/// a caption from 22 rows to 27, a body from 32 to 36, a section from 44 to 48
/// and a title from 59 to 63. A card whose height was its old stack plus its old
/// padding therefore had its last line running past the bottom edge, so every
/// card height here is solved from the boxes it carries and each row is placed
/// off the bottom of the row above rather than from a remembered y. The whole
/// dashboard still closes above the home gesture pill, which the last gate
/// below checks.

/// Vertical inset from a card's top edge to the first line box it draws.
const CARD_INSET_Y: usize = 16;
/// Breathing room between a card's last line box and its bottom edge.
const CARD_BOTTOM_PAD: usize = 9;
/// Gap between one dashboard row's bottom edge and the next row's top edge.
const ROW_GAP: usize = 14;

/// The page header: the title run, and its subtitle a whole title box below it.
///
/// WHY the dashboard starts below this box: a run's ink reaches the bottom of
/// the box it reserves, and the first card row began inside the subtitle's.
const HEADER_Y: usize = 60;
const HEADER_STACK_H: usize = lh(type_scale::TITLE) + lh(type_scale::CAPTION);

/// A chip: a caption label inside a filled pill, so the pill reserves the
/// caption's own line box plus its vertical padding.
///
/// WHY the pill is no longer a 24-row band: 24 rows is the bitmap cell's line,
/// and a caption run drawn with the outline face at the raised rung puts ink
/// below the pill's bottom edge, which reads as a clipped control.
const CHIP_PAD_Y: usize = 6;
const CHIP_H: usize = CHIP_PAD_Y + lh(type_scale::CAPTION) + CHIP_PAD_Y;

/// The cards whose figures are device readings.
///
/// WHY they are consts and not literals inside the painter: the shell repaints a
/// band named by the screen that draws it, and a band written down separately
/// from the rows it covers is a band that can drift away from them.
///
/// Row 1 (Storage & Physical Memory): label, the figure, a body line, a caption
/// detail. Both cards share the same vertical ladder.
const ROW1_STACK_H: usize = CARD_INSET_Y
    + lh(type_scale::CAPTION)
    + lh(type_scale::TITLE)
    + lh(type_scale::BODY)
    + lh(type_scale::CAPTION);
const ROW1_CARD_H: usize = ROW1_STACK_H + CARD_BOTTOM_PAD;
const ROW1_Y: usize = HEADER_Y + HEADER_STACK_H + ROW_GAP;
const STORAGE_ROW: Rect = Rect::new(40, ROW1_Y, 305, ROW1_CARD_H);
const RAM_ROW: Rect = Rect::new(375, ROW1_Y, 305, ROW1_CARD_H);

const ROW1_LABEL_Y: usize = ROW1_Y + CARD_INSET_Y;
const ROW1_FIGURE_Y: usize = ROW1_LABEL_Y + lh(type_scale::CAPTION);
const ROW1_SECONDARY_Y: usize = ROW1_FIGURE_Y + lh(type_scale::TITLE);
const ROW1_DETAIL_Y: usize = ROW1_SECONDARY_Y + lh(type_scale::BODY);

/// Audio card: the title, then the device line a whole body box below it, then
/// the footer row - the volume reading on the left, the mixer tag on the right -
/// under the gauge band.
///
/// WHY the gauge band moved: the old gate was the caption box closing on the
/// literal `y + 74`, which was the proof the device line sat above the divider.
/// With the raised boxes the line ends below 74, so the divider is now placed
/// off the line box and the footer row off the divider.
const AUDIO_GAUGE_GAP: usize = 4;
const AUDIO_GAUGE_H: usize = 8;
const AUDIO_FOOT_GAP: usize = 10;
const AUDIO_FOOT_DY: usize = CARD_INSET_Y
    + lh(type_scale::BODY)
    + lh(type_scale::CAPTION)
    + AUDIO_GAUGE_GAP
    + AUDIO_GAUGE_H
    + AUDIO_FOOT_GAP;
const AUDIO_CARD_H: usize = AUDIO_FOOT_DY + lh(type_scale::CAPTION) + CARD_BOTTOM_PAD;
const AUDIO_ROW_Y: usize = ROW1_Y + ROW1_CARD_H + ROW_GAP;
const AUDIO_ROW: Rect = Rect::new(40, AUDIO_ROW_Y, 640, AUDIO_CARD_H);
const AUDIO_TITLE_Y: usize = AUDIO_ROW_Y + CARD_INSET_Y;
const AUDIO_SUB_Y: usize = AUDIO_TITLE_Y + lh(type_scale::BODY);
const AUDIO_GAUGE_Y: usize = AUDIO_SUB_Y + lh(type_scale::CAPTION) + AUDIO_GAUGE_GAP;
const AUDIO_FOOT_Y: usize = AUDIO_GAUGE_Y + AUDIO_GAUGE_H + AUDIO_FOOT_GAP;

/// Row 3: the calendar card (label, the RTC date line, the source chip, the
/// detail) and the security card (label, the status figure, the architecture
/// line, the capability chip). The pair takes the height the taller stack needs
/// so the two cards still read as one row.
const CAL_CHIP_DY: usize = CARD_INSET_Y + lh(type_scale::CAPTION) + lh(type_scale::SECTION);
const CAL_DETAIL_DY: usize = CAL_CHIP_DY + CHIP_H + 4;
const VAULT_CHIP_DY: usize =
    CARD_INSET_Y + lh(type_scale::CAPTION) + lh(type_scale::TITLE) + lh(type_scale::BODY);
const ROW3_CARD_H: usize = VAULT_CHIP_DY + CHIP_H + CARD_BOTTOM_PAD;
const ROW3_Y: usize = AUDIO_ROW_Y + AUDIO_CARD_H + ROW_GAP;

/// Calendar card: label, the RTC date line, the source chip a whole section box
/// under it, then the detail row.
const CAL_ROW: Rect = Rect::new(40, ROW3_Y, 305, ROW3_CARD_H);
const CAL_LABEL_Y: usize = CAL_ROW.y + CARD_INSET_Y;
const CAL_DATE_Y: usize = CAL_LABEL_Y + lh(type_scale::CAPTION);
const CAL_CHIP_Y: usize = CAL_ROW.y + CAL_CHIP_DY;
const CAL_DETAIL_Y: usize = CAL_ROW.y + CAL_DETAIL_DY;

/// Security card: label, the status figure, the architecture line, then the
/// capability chip a whole body box under the architecture line.
const VAULT_ROW: Rect = Rect::new(375, ROW3_Y, 305, ROW3_CARD_H);
const VAULT_LABEL_Y: usize = VAULT_ROW.y + CARD_INSET_Y;
const VAULT_STATUS_Y: usize = VAULT_LABEL_Y + lh(type_scale::CAPTION);
const VAULT_ARCH_Y: usize = VAULT_STATUS_Y + lh(type_scale::TITLE);
const VAULT_CHIP_Y: usize = VAULT_ARCH_Y + lh(type_scale::BODY);

/// Readings cards: the label, and the sublabel one body box below it. The value
/// is right-anchored on the same first row, so it keeps its own offset.
const READING_LABEL_DY: usize = 10;
const READING_SUB_DY: usize = READING_LABEL_DY + lh(type_scale::BODY);
const READING_VALUE_DY: usize = 20;
/// Height of one readings card, and the pitch between two of them: the card
/// reserves the sublabel's whole box, the pitch keeps the 12-row gutter the
/// 64x76 pair had.
const READING_CARD_H: usize = READING_SUB_DY + lh(type_scale::CAPTION) + 3;
const READING_GUTTER: usize = 12;
const READING_PITCH: usize = READING_CARD_H + READING_GUTTER;
const READINGS_CARD_COUNT: usize = 5;
/// The five readings cards' top edges, one pitch apart.
const READING_CARD1_Y: usize = READINGS_CARD0_Y + READING_PITCH;
const READING_CARD2_Y: usize = READINGS_CARD0_Y + 2 * READING_PITCH;
const READING_CARD3_Y: usize = READINGS_CARD0_Y + 3 * READING_PITCH;
const READING_CARD4_Y: usize = READINGS_CARD0_Y + 4 * READING_PITCH;
/// The first readings card's row, and the section header above it. WHY the
/// header is solved from the card and not from the card row above: a section run
/// reserves `lh(SECTION)` rows and its descender reaches the bottom of that box,
/// so a header placed a few rows above the card let the card's own fill answer
/// those rows first and ate the tail of the 'g'.
const READINGS_HEAD_GAP: usize = 10;
const READINGS_CARD0_Y: usize = ROW3_Y + ROW3_CARD_H + READINGS_HEAD_GAP + lh(type_scale::SECTION);
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

/// Truth gates: the ladder each derived stride trusts still orders the rungs the
/// way the stacks assume, every grown stack closes inside the card that carries
/// it, a chip's label inside its pill, and the whole dashboard above the home
/// gesture pill.
///
/// WHY these no longer read `lh(CAPTION) == 22`, `lh(BODY) == 32` and
/// `lh(TITLE) == 59`: those literals were the previous ladder's boxes, so they
/// pinned the ladder instead of this layout - the day the ladder grew they
/// failed for reasons the geometry here had nothing to do with, while the fact
/// the derivation actually depends on (that each rung's box is taller than the
/// rung above it, which is what makes a derived stride monotonic) went
/// unasserted.
const _: () = assert!(lh(type_scale::CAPTION) >= launcher::font::cap_for_scale(type_scale::CAPTION) as usize);
const _: () = assert!(
    lh(type_scale::BODY) > lh(type_scale::CAPTION),
    "a body line box must reserve more rows than the caption above it",
);
const _: () = assert!(
    lh(type_scale::TITLE) > lh(type_scale::BODY),
    "a title line box must reserve more rows than the body above it",
);
const _: () = assert!(ROW1_DETAIL_Y + lh(type_scale::CAPTION) <= STORAGE_ROW.y + STORAGE_ROW.h);
const _: () = assert!(ROW1_DETAIL_Y + lh(type_scale::CAPTION) <= RAM_ROW.y + RAM_ROW.h);
const _: () = assert!(AUDIO_SUB_Y + lh(type_scale::CAPTION) <= AUDIO_GAUGE_Y);
const _: () = assert!(AUDIO_FOOT_Y + lh(type_scale::CAPTION) <= AUDIO_ROW_Y + AUDIO_CARD_H);
const _: () = assert!(VAULT_CHIP_Y + CHIP_H <= VAULT_ROW.y + VAULT_ROW.h);
const _: () = assert!(CAL_CHIP_Y + CHIP_H <= CAL_DETAIL_Y);
const _: () = assert!(CAL_DETAIL_Y + lh(type_scale::CAPTION) <= CAL_ROW.y + CAL_ROW.h);
const _: () = assert!(READING_SUB_DY + lh(type_scale::CAPTION) <= READING_CARD_H);
const _: () = assert!(READINGS_HEAD_Y + lh(type_scale::SECTION) <= READINGS_CARD0_Y);
const _: () = assert!(AUDIO_ROW_Y >= STORAGE_ROW.y + STORAGE_ROW.h);
const _: () = assert!(ROW3_Y >= AUDIO_ROW.y + AUDIO_ROW.h);
/// The card column's last row against the mark's first row.
///
/// WHY this needed re-solving rather than a passing glance: the gate is written
/// against `HOME_PILL.y`, and the mark moved two rows UP with this change -- from
/// the 1420 this screen typed for itself to the platform's 1418 -- so the
/// clearance it guards shrank by two rows and had to be re-measured, not
/// re-asserted from the old number. The column solves to 789 + 4 x 94 + 82, so the
/// fifth readings card closes at row 1247 and sits 171 rows clear of the mark:
/// the gate holds with the platform's row exactly as it held with this screen's
/// own, because every row in the column is derived from the type ladder above it
/// and none was ever measured off the pill.
const _: () = assert!(
    READINGS_CARD0_Y + (READINGS_CARD_COUNT - 1) * READING_PITCH + READING_CARD_H <= HOME_PILL.y,
    "the dashboard's last readings card must stay above the home gesture pill",
);

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
    // a rung-4 run that reserves `lh(TITLE)` rows, and at the old y the
    // subtitle's ink began above the bottom of that box, so the two read as one
    // smeared row on the device.
    let page_title = LabelRun::new(40, HEADER_Y, HEADER_TEXT_MAX, type_scale::TITLE, b"Live Widgets", theme.on_surface);
    let page_subtitle = LabelRun::new(
        40,
        HEADER_Y + lh(type_scale::TITLE),
        HEADER_TEXT_MAX,
        type_scale::CAPTION,
        b"Real-time hardware telemetry & system status",
        theme.on_surface_variant,
    );

    // WHY the chip is as wide as it is: the caption run measures ~135 px and the
    // old 150 px pill left the label touching its own border, which reads as a
    // clipped control rather than a status. WHY it is as tall as it is: the pill
    // reserves the caption run's whole line box plus its own padding.
    let status_chip_rect = Rect::new(SCREEN_WIDTH - 214, HEADER_Y, 174, CHIP_H);
    let status_chip_label = LabelRun::centred(
        status_chip_rect.x,
        status_chip_rect.y + CHIP_PAD_Y,
        status_chip_rect.w,
        type_scale::CAPTION,
        b"Platform Active",
        0xFF10B981, // Emerald green
    );

    // 3. Row 1: Storage & Physical Memory (STORAGE_ROW / RAM_ROW, one ladder high)
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

    // 4. Row 2: Real System Audio & Master Volume (AUDIO_ROW)
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
    let audio_gauge_rect = Rect::new(audio_rect.x + 20, AUDIO_GAUGE_Y, audio_rect.w - 40, AUDIO_GAUGE_H);
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

    // 5. Row 3: Calendar / RTC & Security (CAL_ROW / VAULT_ROW)
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
    // WHY the date line is derived from the label box and not from a `y + 38`:
    // the old offset assumed the 8x16 cell, but the label is a caption run that
    // reserves `lh(CAPTION)` rows, so the date line is one whole caption box
    // under the card's inset - named from the ladder so a change in the caption
    // stride moves it with the label.
    let cal_date = LabelRun::new(cal_rect.x + 20, CAL_DATE_Y, card_text_w(CAL_ROW.w), type_scale::SECTION, date_slice, theme.on_surface);
    let cal_time_chip = Rect::new(cal_rect.x + 20, CAL_CHIP_Y, 110, CHIP_H);
    let cal_time_label = if state.clock_source_ok {
        LabelRun::centred(cal_time_chip.x, cal_time_chip.y + CHIP_PAD_Y, cal_time_chip.w, type_scale::CAPTION, b"Hardware RTC", theme.on_surface)
    } else {
        LabelRun::centred(cal_time_chip.x, cal_time_chip.y + CHIP_PAD_Y, cal_time_chip.w, type_scale::CAPTION, b"RTC refused", theme.on_surface_variant)
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
    // rung-4 run that reserves `lh(TITLE)` rows, and the architecture line
    // and the chip below it were spaced by the 8x16 cell's 52 and 34 rows, so
    // each began its ink while the run above was still inside its box.
    let vault_rect = VAULT_ROW;
    let vault_label = LabelRun::new(vault_rect.x + 20, VAULT_LABEL_Y, card_text_w(VAULT_ROW.w), type_scale::CAPTION, b"Security Architecture", theme.on_surface_variant);
    let vault_status = LabelRun::new(vault_rect.x + 20, VAULT_STATUS_Y, card_text_w(VAULT_ROW.w), type_scale::TITLE, b"Enforced", 0xFF10B981);
    let vault_arch = LabelRun::new(vault_rect.x + 20, VAULT_ARCH_Y, card_text_w(VAULT_ROW.w), type_scale::BODY, b"Zero Ambient Authority", theme.on_surface);
    let vault_chip_rect = Rect::new(vault_rect.x + 20, VAULT_CHIP_Y, 130, CHIP_H);
    let vault_chip_label = LabelRun::centred(vault_chip_rect.x, vault_chip_rect.y + CHIP_PAD_Y, vault_chip_rect.w, type_scale::CAPTION, b"Capability Broker", 0xFFFFFFFF);

    // 6. Section 2 Header.
    let readings_head = LabelRun::new(
        40,
        READINGS_HEAD_Y,
        card_text_w(READING_CARD_W),
        type_scale::SECTION,
        b"System Readings",
        theme.on_surface,
    );

    // 7. System Readings Cards (READINGS_CARD0_Y, five cards a READING_PITCH apart)
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

    let card_w = READING_CARD_W;
    let card_h = READING_CARD_H;
    // WHY the run rows are derived per card: the label is a body run reserving
    // `lh(BODY)` rows, and the sublabels were placed inside that box, so they
    // read straight through it. The card is now as tall as the sublabel's whole
    // box, and the value is right-anchored and shares the label's row, so all
    // three runs are bounded at the card centre by `READING_TEXT_MAX`.
    let readings = [
        DashboardReadingCard {
            rect: Rect::new(40, READINGS_CARD0_Y, card_w, card_h),
            label: LabelRun::new(64, READINGS_CARD0_Y + READING_LABEL_DY, READING_TEXT_MAX, type_scale::BODY, b"Package Registry", theme.on_surface),
            sublabel: LabelRun::new(64, READINGS_CARD0_Y + READING_SUB_DY, READING_TEXT_MAX, type_scale::CAPTION, b"Signed verified package store", theme.on_surface_variant),
            value: LabelRun::right(40 + card_w - 24, READINGS_CARD0_Y + READING_VALUE_DY, READING_TEXT_MAX, type_scale::BODY, &pkg_buf[..n], theme.on_surface),
            accent_color: 0xFF6366F1,
        },
        DashboardReadingCard {
            rect: Rect::new(40, READING_CARD1_Y, card_w, card_h),
            label: LabelRun::new(64, READING_CARD1_Y + READING_LABEL_DY, READING_TEXT_MAX, type_scale::BODY, b"IPC Alert Channel", theme.on_surface),
            sublabel: LabelRun::new(64, READING_CARD1_Y + READING_SUB_DY, READING_TEXT_MAX, type_scale::CAPTION, b"Kernel notification queue", theme.on_surface_variant),
            value: LabelRun::right(40 + card_w - 24, READING_CARD1_Y + READING_VALUE_DY, READING_TEXT_MAX, type_scale::BODY, &notif_buf[..n_len], if state.notification_count > 0 { 0xFFF59E0B } else { theme.on_surface_variant }),
            accent_color: 0xFF10B981,
        },
        DashboardReadingCard {
            rect: Rect::new(40, READING_CARD2_Y, card_w, card_h),
            label: LabelRun::new(64, READING_CARD2_Y + READING_LABEL_DY, READING_TEXT_MAX, type_scale::BODY, b"Display Compositor", theme.on_surface),
            sublabel: LabelRun::new(64, READING_CARD2_Y + READING_SUB_DY, READING_TEXT_MAX, type_scale::CAPTION, b"Flagship portrait 720x1440", theme.on_surface_variant),
            // WHY the panel state and not a frame rate: nothing in this kernel measures
            // a flip interval - the scanout deadline is computed from a configured
            // refresh figure - so "60 FPS" reported a number no clock had observed.
            // Whether the panel has power is something the platform does answer.
            value: LabelRun::right(
                40 + card_w - 24,
                READING_CARD2_Y + READING_VALUE_DY,
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
            rect: Rect::new(40, READING_CARD3_Y, card_w, card_h),
            label: LabelRun::new(64, READING_CARD3_Y + READING_LABEL_DY, READING_TEXT_MAX, type_scale::BODY, b"Network Connectivity", theme.on_surface),
            // WHY this names the address as a configuration: the stack sends from a
            // fixed address and nothing probes a link, so the old line's "Online"
            // claimed a measured connection that no code here could observe.
            sublabel: LabelRun::new(64, READING_CARD3_Y + READING_SUB_DY, READING_TEXT_MAX, type_scale::CAPTION, b"E1000 IPv4, addr 10.0.2.15", theme.on_surface_variant),
            value: LabelRun::right(40 + card_w - 24, READING_CARD3_Y + READING_VALUE_DY, READING_TEXT_MAX, type_scale::BODY, b"Link unverified", theme.on_surface_variant),
            accent_color: 0xFF3B82F6,
        },
        DashboardReadingCard {
            rect: Rect::new(40, READING_CARD4_Y, card_w, card_h),
            label: LabelRun::new(64, READING_CARD4_Y + READING_LABEL_DY, READING_TEXT_MAX, type_scale::BODY, b"Process Sandbox & Isolation", theme.on_surface),
            sublabel: LabelRun::new(64, READING_CARD4_Y + READING_SUB_DY, READING_TEXT_MAX, type_scale::CAPTION, b"Ring 3 user mode CR3 tables", theme.on_surface_variant),
            value: LabelRun::right(40 + card_w - 24, READING_CARD4_Y + READING_VALUE_DY, READING_TEXT_MAX, type_scale::BODY, b"Protected", 0xFF059669),
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
    //
    // WHY the platform draws it and not this screen: the pixel function below had
    // its own rectangle and its own corner radius (2 on a five-row box), so the
    // mark grew square ends no other surface on the device had. `home_band::pixel`
    // is the raster every application is told to copy, so the ink here is that
    // shape by construction rather than by a comment claiming the two agree.
    if let Some(ink) = interos_std::design::home_band::pixel(x, y, theme.on_surface_variant) {
        return ink;
    }

    theme.surface
}
