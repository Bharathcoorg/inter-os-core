//! Notifications & Events Shade: production-grade sovereign notification center.
//!
//! Visual hierarchy:
//! - Uniform system status bar (clock, signal bars, Wi-Fi fan, battery shell).
//! - Header: Large bold "Notifications" title, crisp "Clear all" pill button,
//!   and clean subtitle.
//! - Filter pills: All, Messages, System, Apps with live counts.
//! - Notification Cards:
//!   - Squircle app icon (32x32) with app brand color & clean vector glyph.
//!   - Header line: App name (Scale 2 Body) • timestamp ("Just now", "5m ago").
//!   - Individual card dismiss action (>= 48px touch target with vector 'x' glyph).
//!   - Bold title (Scale 2 Body) and clean body text (Scale 2 Body).
//!   - Action pills (>= 48px touch targets, Scale 2 Body, e.g. "Reply", "Details", "Dismiss").
//! - Empty state: Flagship "No new notifications", "All caught up" illustration.
//! - Home indicator bar.

#![allow(dead_code)]

use crate::{test_text_aa, LauncherState, NotificationRecord};
use launcher::layout::Rect;
use launcher::text;
use launcher::theme::{space, type_scale, radius, Theme, SCREEN_HEIGHT, SCREEN_WIDTH, STATUS_BAR_HEIGHT};

// -----------------------------------------------------------------------------
// Constants & Geometry
// -----------------------------------------------------------------------------

/// Left edge and width of the feed's content cards.
const ROW_X: usize = 32;
const ROW_W: usize = SCREEN_WIDTH - 64; // 656

/// Left edge of the header block.
const HEADER_X: usize = 36;

/// Tops of the header block's lines, in order.
///
/// WHY every one of these is an advance rather than a second literal: a
/// `LabelRun` reserves its whole line box from its top -- ascent plus descent
/// at the rung, which is what `launcher::font::line_height` answers -- and a
/// HEADING run's box is 80 rows, not the 48 the bitmap cell used to reserve.
/// The subtitle's old top of 134 sat two rows inside the title's box, so the
/// title's descenders were painted over by the line below them. Each line
/// starts at the line above it plus that line's own box plus one gap, so
/// changing a rung moves everything under it instead of colliding with it.
///
/// WHY the advances are literals pinned by an `assert!`: the events-header
/// contract reads TITLE_Y, SUBTITLE_Y and SOURCE_NOTE_Y back with a restricted
/// expression parser that resolves an integer, a name declared in this file, or
/// exactly two such terms joined by one operator. A call expression there makes
/// the contract fail to parse rather than fail on geometry, so the number is
/// written down and tied to the SDK's own metrics beside it.
pub const TITLE_Y: usize = 56;
const _: () = assert!(TITLE_Y == interos_std::design::viewport::HEADER_TOP);

/// `TITLE_Y` to `SUBTITLE_Y`: the HEADING title's box plus one GRID_8 gap.
const TITLE_ADVANCE: usize = 88;
const _: () = assert!(
    TITLE_ADVANCE == launcher::font::line_height(type_scale::HEADING) + space::GRID_8,
);
pub const SUBTITLE_Y: usize = TITLE_Y + TITLE_ADVANCE;

/// `SUBTITLE_Y` to `SOURCE_NOTE_Y`: the BODY subtitle's box plus one GRID_4 gap.
///
/// WHY 43 and not the 40 it carried: the step between two runs has to pay for the
/// whole box of the run above it, and the body rung rose with the ladder, so its
/// line box is `line_height(BODY)` rows rather than the 36 the step was solved
/// against; the GRID_4 of air on top of it is a design decision and did not move.
/// The gate beside it re-derives the number from the SDK's own metric, so this
/// literal is that arithmetic written down for the contract's parser.
const SUBTITLE_ADVANCE: usize = 43;
const _: () = assert!(
    SUBTITLE_ADVANCE == launcher::font::line_height(type_scale::BODY) + space::GRID_4,
);
pub const SOURCE_NOTE_Y: usize = SUBTITLE_Y + SUBTITLE_ADVANCE;

/// WHY this is an adjacency and not only a clearance: every step in the header
/// pays for the whole box of the run above it and then one grid gap of air, and
/// the note-to-pill-row step was the one line the file left as a bare `<=`. That
/// worked while the caption's box was smaller than the row's slack and stopped
/// working when the rung rose: the note's box reached three rows past the pill
/// row it was clearing. The equality states the rule the other steps already
/// follow -- box, then GRID_4 -- so the pill row moves with the ladder instead of
/// being nudged by hand until the old gate passes.
const _: () = assert!(
    SOURCE_NOTE_Y + launcher::font::line_height(type_scale::CAPTION) <= PILLS_Y,
    "the source note's line box must end above the filter pill row",
);
const _: () = assert!(
    PILLS_Y == SOURCE_NOTE_Y + launcher::font::line_height(type_scale::CAPTION) + space::GRID_4,
    "the pill row starts one caption box and a GRID_4 gap under the source note",
);

/// Filter note line placed below the pill row.
pub const FILTER_NOTE_Y: usize = PILLS_Y + PILL_H;

/// Height of one notification card, and the pitch its stack advances by.
///
/// WHY 193 and not the old 176: a card is the app name, the title, the body and
/// a row of action pills, each one whole line box under the last, and the raised
/// body rung lengthened every advance in that stack. The gates beside these two
/// literals re-derive both from the card stack solved below, so the numbers
/// stated here cannot drift from the boxes they contain -- they stay literals
/// because the contract parses `ROW_H` with the restricted expression reader.
const ROW_H: usize = 193;
const ROW_PITCH: usize = 201;

/// Rows from a card's top edge to each line of its stack: the name row, the
/// title a whole body box under it, the body a whole box under that, and the
/// action pills under the body.
///
/// WHY these are advances and not four hand-typed offsets: the old stack stepped
/// 14 / 48 / 82 / 120, which was the 32-row body box plus two rows of air. With
/// a 36-row box each line began inside the box above it and the two runs read
/// through one another.
const ROW_NAME_DY: usize = 14;
const ROW_TITLE_DY: usize = ROW_NAME_DY + BODY_LABEL_BOX_H;
const ROW_BODY_DY: usize = ROW_TITLE_DY + BODY_LABEL_BOX_H;
const ROW_ACTION_GAP: usize = 6;
const ROW_ACTION_H: usize = 48;
const ROW_ACTIONS_DY: usize = ROW_BODY_DY + BODY_LABEL_BOX_H + ROW_ACTION_GAP;
const ROW_BOTTOM_PAD: usize = 8;

const _: () = assert!(
    ROW_ACTIONS_DY + ROW_ACTION_H + ROW_BOTTOM_PAD == ROW_H,
    "the card must be exactly its stack: four body boxes, the pill row and its padding",
);
const _: () = assert!(
    ROW_PITCH == ROW_H + 8,
    "the feed advances one card plus the gutter between cards",
);

/// First row of the first section's stack: below the filter note's line box.
///
/// WHY 296 and not 312: the heading placed at this row owns a line box of its
/// own, and the cards used to start `space::MD` below the box's *top* -- which
/// is inside the caption's reserved rows. This surface has no clipper, so the
/// card a heading introduces simply claimed the pixels first and painted the
/// heading's bottom rows out: on the device "Last hour" arrived with its
/// baseline shaved off by the card it names.
///
/// WHY 311 now: the same rule, re-solved against the raised caption rung. The
/// filter note's box ends at `FILTER_NOTE_Y + SECTION_LABEL_BOX_H`, and this is
/// that plus one GRID_4 of air -- the gate beside it checks the arithmetic, so
/// the literal cannot drift from the box it is meant to clear.
pub const SECTION_HEAD_Y: usize = 311;
const _: () = assert!(
    SECTION_HEAD_Y == FILTER_NOTE_Y + SECTION_LABEL_BOX_H + space::GRID_4,
    "the first section head must start one caption box and a GRID_4 gap under the filter note"
);

/// Rows a section heading's line box reserves on the outline face -- the rung
/// `SectionPlan::label` draws at.
///
/// WHY written down and pinned beside it: the same reason `TITLE_ADVANCE` is.
/// A call expression here would make the contract fail to parse rather than
/// fail on geometry, so the number is stated and tied to the SDK's own metric.
/// The statement moves whenever the caption rung's box does, which since the
/// rungs rose is the outline face's caption box and not the bitmap cell's.
const SECTION_LABEL_BOX_H: usize = 30;
const _: () = assert!(
    SECTION_LABEL_BOX_H == launcher::font::line_height(type_scale::CAPTION),
    "the section heading's box must be the outline caption line box, not a remembered cell",
);

/// Air between a section heading's line box and the first card of its group.
const SECTION_HEAD_GAP: usize = 8;
const _: () = assert!(SECTION_HEAD_GAP == space::GRID_8);

/// Rows from a section heading's line-box top to the first card it names.
///
/// WHY the box plus the gap, and not the gap: an advance measured from the top
/// of a box has to pay for the box before it can start reserving air. The old
/// `space::MD` paid for neither, which is how the heading lost its baseline.
const SECTION_HEAD_ADVANCE: usize = SECTION_LABEL_BOX_H + SECTION_HEAD_GAP;

/// Bottom cutoff for feed rows.
///
/// WHY keyed to the home indicator and not the remembered 700: 700 was the top
/// of the second "Clear all" pill that used to sit in the middle of this
/// surface. With that control gone the feed was still stopping at its old edge,
/// so two cards arrived and then seven hundred rows of nothing before the
/// indicator -- a feed that reads as broken rather than as short. The cutoff is
/// the bottom chrome minus one card, so the stack ends where the surface does:
/// the card grew with the body rung, so the edge it stops at moves up to match.
const FEED_BOTTOM: usize = 1223;
const _: () = assert!(
    FEED_BOTTOM + ROW_H == HOME_PILL.y,
    "the feed must stop one card clear of the home indicator it is stacked above",
);

const _: () = assert!(
    SECTION_HEAD_Y + SECTION_HEAD_ADVANCE + ROW_H <= FEED_BOTTOM,
    "a section's first card must clear its own heading and still fit above the bottom chrome",
);

/// Top of the filter pill row.
///
/// WHY a literal at all: the events-header contract reads `PILLS_Y` with the
/// restricted expression parser, the same reason `TITLE_ADVANCE` and `ROW_H` are
/// written down. The gate above re-derives it from the source note's box plus one
/// GRID_4 of air, so the number below is that arithmetic's answer, not a remembered
/// row.
///
/// Height of the pill row, and the gap between pills.
pub const PILLS_Y: usize = 221;
pub const PILL_H: usize = 56;
const PILL_GAP: usize = space::SM + space::XS;

/// Left edge of a row's text column, past the app squircle icon.
const ROW_TEXT_X: usize = ROW_X + 60;

/// Guest timer ticks per second (1 kHz PIT counter).
const TICKS_PER_SEC: u64 = 1000;

/// Default active category filter (0 = All).
const DEFAULT_FILTER: usize = 0;

/// Category filter names.
const FILTER_NAMES: [&[u8]; 4] = [b"All", b"Messages", b"System", b"Apps"];

/// Section headers, in age order.
const SECTION_NAMES: [&[u8]; 4] = [b"Last minute", b"Last hour", b"Since boot", b"Undated"];

/// Header "Clear all" button (48px height touch target).
///
/// WHY its top is the platform's header edge and not a literal above it: the
/// status band owns `0..STATUS_BAR_HEIGHT` and answers the pixel function before
/// any header content is reached, so the pill's old top of 44 had its first four
/// rows -- the crown of its rounded arc -- painted over by a surface that was
/// never its own. Keyed to `viewport::HEADER_TOP` it starts where the app's
/// chrome is allowed to start, and moves if the band ever does.
///
/// WHY it is the only one: this pill and a second, wider one at y 702 both read
/// "Clear all" and both emptied the store, so the surface asked the same
/// destructive question twice and left the person to work out which answer the
/// machine meant. The second one sat below a feed that holds two cards, which
/// made it look like an object in the list rather than a control over it.
pub const HEADER_CLEAR_BTN: Rect = Rect::new(
    SCREEN_WIDTH - 168,
    interos_std::design::viewport::HEADER_TOP,
    136,
    48,
);

/// The rows a BODY label reserves on the outline face -- the rung every pill's
/// label on this surface is drawn at.
///
/// WHY one const and not three literals: the action and Clear pills centred
/// their label against a remembered `36`, which was neither the outline BODY
/// line box nor the old bitmap cell (48). A label centred in a box the
/// painter never fills puts the ink above the pill's middle and leaves the gap
/// below twice the gap above -- the header pill already fixed for exactly this.
const BODY_LABEL_BOX_H: usize = launcher::font::line_height(type_scale::BODY);

/// The rows the Clear all pill's own label reserves -- the BODY line box.
const CLEAR_LABEL_BOX_H: usize = BODY_LABEL_BOX_H;

/// Where the "Clear all" label starts: centred in the pill by its line box.
///
/// WHY derived and not the old hand-typed 36: the label was centred against a
/// box height that no longer matched the rung it draws at -- the pill's label is
/// a BODY run whose box is `line_height(BODY)` -- so the ink sat high and the
/// gap beneath it read as twice the gap above.
const HEADER_CLEAR_LABEL_Y: usize =
    HEADER_CLEAR_BTN.y + (HEADER_CLEAR_BTN.h - CLEAR_LABEL_BOX_H) / 2;

const _: () = assert!(
    CLEAR_LABEL_BOX_H <= HEADER_CLEAR_BTN.h,
    "the Clear all label's line box must fit the pill it is centred in",
);
const _: () = assert!(
    BODY_LABEL_BOX_H > launcher::font::line_height(type_scale::CAPTION),
    "a pill label sits a rung above the captions on this surface, so its box must reserve more",
);

/// Home gesture indicator pill.
const HOME_PILL: Rect = Rect::new(SCREEN_WIDTH / 2 - 90, SCREEN_HEIGHT - 24, 180, 6);

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
pub struct LabelRun {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub scale: usize,
    pub len: usize,
    pub ink: u32,
    pub em: u32,
    pub top: usize,
    pub box_h: usize,
    pub bytes: [u8; RUN_CAP],
    pub outline: Option<launcher::outline_text::OutlineRun>,
}

impl LabelRun {
    pub const EMPTY: LabelRun = LabelRun {
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

    pub fn new(x: usize, y: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
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

    pub fn centred(x: usize, y: usize, w: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
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

    pub fn right(x_right: usize, y: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
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

    pub fn fitted(x: usize, y: usize, scale: usize, fit: usize, bytes: &[u8], ink: u32) -> Self {
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
    pub fn pixel_on(&self, px: usize, py: usize, bg: u32) -> Option<u32> {
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
pub enum Category {
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

#[derive(Clone, Copy)]
struct SectionPlan {
    label: LabelRun,
}

impl SectionPlan {
    const EMPTY: SectionPlan = SectionPlan {
        label: LabelRun::EMPTY,
    };
}

#[derive(Clone, Copy)]
pub struct RowPlan {
    pub id: u64,
    pub rect: Rect,
    pub squircle: Rect,
    pub squircle_color: u32,
    pub icon_kind: usize, // 0 = messages, 1 = system, 2 = apps, 3 = unlisted
    pub dismiss_btn: Rect,
    pub app_name: LabelRun,
    pub time: LabelRun,
    pub title: LabelRun,
    pub body: LabelRun,
    pub action1: Rect,
    pub action1_label: LabelRun,
    pub action2: Rect,
    pub action2_label: LabelRun,
    pub pkg: [u8; 32],
    pub pkg_len: usize,
}

impl RowPlan {
    pub const EMPTY: RowPlan = RowPlan {
        id: 0,
        rect: Rect::new(0, 0, 0, 0),
        squircle: Rect::new(0, 0, 0, 0),
        squircle_color: 0,
        icon_kind: 0,
        dismiss_btn: Rect::new(0, 0, 0, 0),
        app_name: LabelRun::EMPTY,
        time: LabelRun::EMPTY,
        title: LabelRun::EMPTY,
        body: LabelRun::EMPTY,
        action1: Rect::new(0, 0, 0, 0),
        action1_label: LabelRun::EMPTY,
        action2: Rect::new(0, 0, 0, 0),
        action2_label: LabelRun::EMPTY,
        pkg: [0u8; 32],
        pkg_len: 0,
    };
}

#[derive(Clone, Copy)]
struct EventsFrame {
    /// The shell's canonical status clock. The telemetry marks are the
    /// platform's own and are drawn from the SDK, so this frame carries no
    /// second copy of their geometry.
    clock: LabelRun,
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
            } else if ends_with(pkg, b"file_manager") {
                b"Files"
            } else if ends_with(pkg, b"gallery") {
                b"Gallery"
            } else if ends_with(pkg, b"camera") {
                b"Camera"
            } else if ends_with(pkg, b"calculator") {
                b"Calculator"
            } else if ends_with(pkg, b"browser") {
                b"Browser"
            } else if ends_with(pkg, b"notes") {
                b"Notes"
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

    let squircle = Rect::new(ROW_X + 16, y + ROW_NAME_DY, 32, 32);
    let icon_kind = match cat {
        Category::Messages => 0,
        Category::System => 1,
        Category::Apps => 2,
        Category::Unlisted => 3,
    };

    // 48x48 Dismiss button at top right of card (>= 48px touch target)
    let dismiss_btn = Rect::new(ROW_X + ROW_W - 48 - 10, y + 8, 48, 48);

    let app_name = LabelRun::fitted(
        ROW_TEXT_X,
        y + ROW_NAME_DY,
        type_scale::BODY,
        240,
        app_name_bytes,
        theme.on_surface,
    );

    let time = LabelRun::right(
        dismiss_btn.x - 12,
        y + ROW_NAME_DY,
        type_scale::BODY,
        &age[..age_len],
        theme.on_surface_variant,
    );

    let content_w = ROW_W - 84;
    let title = LabelRun::fitted(
        ROW_TEXT_X,
        y + ROW_TITLE_DY,
        type_scale::BODY,
        content_w,
        title_bytes,
        theme.on_surface,
    );

    let body = LabelRun::fitted(
        ROW_TEXT_X,
        y + ROW_BODY_DY,
        type_scale::BODY,
        content_w,
        body_bytes,
        theme.on_surface_variant,
    );

    // Action buttons (>= 48px touch targets, Scale 2 Body typography)
    let (act1_name, act2_name): (&[u8], &[u8]) = match cat {
        // WHY the second action says "Dismiss" rather than "Mark read": the
        // platform's notification record has no read flag, and this button has
        // always cancelled the notification. A label the handler does not
        // perform is worse than a smaller action.
        Category::Messages => (b"Reply", b"Dismiss"),
        Category::System => (b"Details", b"Dismiss"),
        _ => (b"Open", b"Dismiss"),
    };

    let action1 = Rect::new(ROW_TEXT_X, y + ROW_ACTIONS_DY, 130, ROW_ACTION_H);
    let action1_label = LabelRun::centred(
        action1.x,
        action1.y + (action1.h.saturating_sub(BODY_LABEL_BOX_H)) / 2,
        action1.w,
        type_scale::BODY,
        act1_name,
        theme.on_surface,
    );

    let action2 = Rect::new(ROW_TEXT_X + 142, y + ROW_ACTIONS_DY, 130, ROW_ACTION_H);
    let action2_label = LabelRun::centred(
        action2.x,
        action2.y + (action2.h.saturating_sub(BODY_LABEL_BOX_H)) / 2,
        action2.w,
        type_scale::BODY,
        act2_name,
        theme.on_surface_variant,
    );

    let mut pkg_buf = [0u8; 32];
    let plen = (rec.pkg_len as usize).min(32);
    if plen > 0 {
        pkg_buf[..plen].copy_from_slice(&rec.package_id[..plen]);
    }

    RowPlan {
        id: rec.id,
        rect,
        squircle,
        squircle_color: squircle_color(cat),
        icon_kind,
        dismiss_btn,
        app_name,
        time,
        title,
        body,
        action1,
        action1_label,
        action2,
        action2_label,
        pkg: pkg_buf,
        pkg_len: plen,
    }
}

// -----------------------------------------------------------------------------
// Frame Builder
// -----------------------------------------------------------------------------

fn build_events_frame(state: &LauncherState) -> EventsFrame {
    let theme = launcher::theme::SHELL;

    // 1. Status Bar Plan: the canonical shell clock at (40, 8) on the body
    // line and the home screen's telemetry glyphs. WHY reused rather than
    // redrawn: this screen used to paint its own filled signal bars, Wi-Fi
    // fan and battery, which disagreed with both the home screen's geometry
    // and the platform's honesty rule (no radio, no fuel gauge).
    let clock_slice: &[u8] = if state.clock_source_ok && state.clock_time[0] != b' ' {
        &state.clock_time
    } else {
        b""
    };
    let status_clock = LabelRun::new(
        40,
        8,
        type_scale::BODY,
        clock_slice,
        theme.on_surface,
    );

    // 2. Header
    let now = interos_std::cap::timer_slot().and_then(|_| interos_std::get_ticks().ok());

    let title = LabelRun::new(HEADER_X, TITLE_Y, type_scale::HEADING, b"Notifications", theme.on_surface);
    let subtitle = LabelRun::new(
        HEADER_X,
        SUBTITLE_Y,
        type_scale::BODY,
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
        HEADER_CLEAR_LABEL_Y,
        HEADER_CLEAR_BTN.w,
        type_scale::BODY,
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
        let w = (text::measure(&label[..n], type_scale::CAPTION) + space::XXL).max(64);
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
    let mut feed_end = SECTION_HEAD_Y;
    let mut current_bucket = usize::MAX;

    for k in 0..total {
        let rec = state.notifications[order[k]];
        let cat = category_of(&rec, state);
        if !in_category(cat, filter) {
            continue;
        }
        let bucket = bucket_of(rec.timestamp_ticks, now);
        let row_y = if bucket != current_bucket {
            y + SECTION_HEAD_ADVANCE
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
            feed_end = row_y + ROW_H;
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

    let note_y = feed_end + SECTION_HEAD_GAP;
    let more_note = LabelRun::new(36, note_y, type_scale::CAPTION, &more[..more_len], theme.on_surface_variant);

    // 5. Empty State
    let (empty_head, empty_body) = if total == 0 {
        (
            LabelRun::centred(ROW_X, 420, ROW_W, type_scale::HEADING, b"No new notifications", theme.on_surface),
            // WHY the outline box and not `text::line_height`: the heading draws
            // with the outline face at rung HEADING, which reserves 80 rows here
            // while the bitmap cell the second line was advanced by reserves 120.
            // The pair is one statement, and it read as two because the second
            // line started 56 rows below the box the first one actually owns.
            LabelRun::centred(
                ROW_X,
                420 + launcher::font::line_height(type_scale::HEADING) + space::MD,
                ROW_W,
                type_scale::BODY,
                b"All caught up with apps and system updates.",
                theme.on_surface_variant,
            ),
        )
    } else {
        (LabelRun::EMPTY, LabelRun::EMPTY)
    };

    EventsFrame {
        clock: status_clock,
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
// Per-Pixel Renderer & Touch Target Hit Testing
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

/// Identifies if a tap hit an individual notification's dismiss target
/// (either the top-right 48x48 'x' dismiss button or the Dismiss action pill).
pub fn card_dismiss_at(x: usize, y: usize, state: &LauncherState) -> Option<u64> {
    let f = build_events_frame(state);
    for i in 0..f.row_count {
        let row = &f.rows[i];
        if row.dismiss_btn.contains(x, y) || row.action2.contains(x, y) {
            return Some(row.id);
        }
    }
    None
}

/// Identifies if a tap hit an individual notification's primary action button
/// (e.g. "Open", "Reply", "Details").
pub fn card_open_at(x: usize, y: usize, state: &LauncherState) -> Option<(&'static str, u64)> {
    let f = build_events_frame(state);
    for i in 0..f.row_count {
        let row = &f.rows[i];
        if row.action1.contains(x, y) {
            let pkg_str = match &row.pkg[..row.pkg_len] {
                b"org.interos.messages" | b"messages" => "org.interos.messages",
                b"org.interos.dialer" | b"dialer" => "org.interos.dialer",
                b"org.interos.calendar" | b"calendar" => "org.interos.calendar",
                b"org.interos.vault" | b"vault" => "org.interos.vault",
                b"org.interos.music" | b"music" => "org.interos.music",
                b"org.interos.settings" | b"settings" => "org.interos.settings",
                b"org.interos.file_manager" | b"file_manager" => "org.interos.file_manager",
                b"org.interos.gallery" | b"gallery" => "org.interos.gallery",
                b"org.interos.camera" | b"camera" => "org.interos.camera",
                b"org.interos.calculator" | b"calculator" => "org.interos.calculator",
                b"org.interos.browser" | b"browser" => "org.interos.browser",
                b"org.interos.notes" | b"notes" => "org.interos.notes",
                _ => "",
            };
            return Some((pkg_str, row.id));
        }
    }
    None
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
        return crate::screens::status::pixel_with(
            x,
            y,
            |x, y| f.clock.pixel_on(x, y, theme.surface),
            &theme,
        );
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

    // Header "Clear all" button (48px height, radius 24)
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

                // App squircle icon (32x32)
                if rounded_contains(row.squircle, x, y, 8) {
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

                // Individual Dismiss button (48x48 touch target, circular chip with 'x' vector icon)
                if row.dismiss_btn.contains(x, y) {
                    let d_cx = row.dismiss_btn.center_x() as i32;
                    let d_cy = row.dismiss_btn.center_y() as i32;
                    let d_dx = x as i32 - d_cx;
                    let d_dy = y as i32 - d_cy;
                    let dist2 = d_dx * d_dx + d_dy * d_dy;
                    if dist2 <= 196 {
                        let cross = (d_dx.abs() - d_dy.abs()).abs() <= 1 && d_dx.abs() <= 6 && d_dy.abs() <= 6;
                        if cross {
                            return theme.on_surface;
                        }
                        return theme.surface_container_high;
                    }
                }

                // App name & timestamp
                if let Some(c) = row.app_name.pixel_on(x, y, theme.surface_container) {
                    return c;
                }
                if let Some(c) = row.time.pixel_on(x, y, theme.surface_container) {
                    return c;
                }

                // Title & body (Scale 2 Body)
                if let Some(c) = row.title.pixel_on(x, y, theme.surface_container) {
                    return c;
                }
                if let Some(c) = row.body.pixel_on(x, y, theme.surface_container) {
                    return c;
                }

                // Action button 1 (48px height, radius 24)
                if rounded_contains(row.action1, x, y, 24) {
                    if rounded_outline(row.action1, x, y, 24) {
                        return theme.border;
                    }
                    if let Some(c) = row.action1_label.pixel_on(x, y, theme.surface_container_high) {
                        return c;
                    }
                    return theme.surface_container_high;
                }

                // Action button 2 (48px height, radius 24)
                if rounded_contains(row.action2, x, y, 24) {
                    if rounded_outline(row.action2, x, y, 24) {
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

    // 7. Home Indicator Pill
    if rounded_contains(f.home_pill, x, y, 3) {
        return theme.on_surface_variant;
    }

    theme.surface
}
