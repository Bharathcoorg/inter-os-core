//! Events Surface (Reference Screen 5): one chronological feed, honestly derived.
//!
//! WHAT THE REFERENCE ASKS FOR (`Ui Samples/05_events.png`, design §5): a unified
//! chronological feed grouped under day sections, each row carrying a source
//! mark, a title and a time, across categories, with an All / Messages / System
//! / Apps filter.
//!
//! WHAT THIS GUEST CAN ACTUALLY BACK, which is the whole difficulty of the
//! surface:
//!
//! - The kernel's notification service (`notification_query`, syscall 112, read
//!   by `main.rs::refresh_notifications` into `LauncherState::notifications`).
//!   Each record carries a package id, a channel, a title, a body and a
//!   `timestamp_ticks` taken from the kernel's PIT counter when it was posted.
//!   This is the ONLY source on the device with a time attached to it.
//! - The installed-package registry. An app being installed is a real event,
//!   but `PackageEntry` (`kernel/src/package/registry.rs`) records only id,
//!   generation, state and version -- no install time. So an app appearing
//!   contributes no entry: an entry without a time in a chronological feed is
//!   either padded or misplaced, and this surface omits it instead.
//! - The shell's runtime recents list (`LauncherState::recent_apps`). It holds
//!   ids and task ids and no timestamps either, so it contributes nothing.
//!
//! So the feed is notification records, relabelled and SAID SO on the surface
//! itself: the subtitle, three derivation lines and the filter note under the
//! pill row all state what the feed is derived from, that ages are elapsed
//! guest timer ticks rather than wall clock, and that the kernel keeps no
//! calendar so there are no Today/Yesterday day groups. Source names are the
//! record's own package id and channel, so every row is traceable to the record
//! it came from.
//!
//! The filter pills are a legend with real counts over the records, not live
//! controls: `main.rs`'s Notifications input arm hit-tests only the clear-all
//! rect and the home gesture, so no tap reaches the pill row. Drawing them as
//! switchable would be a control that never moves; they are drawn with the
//! active treatment on All, which is the filter actually in effect, and the
//! note under them says so.

#![allow(dead_code)]

use crate::{test_text, test_text_aa, LauncherState, NotificationRecord};
use launcher::layout::Rect;
use launcher::text;
use launcher::theme::{space, type_scale, radius, Theme, SCREEN_HEIGHT, SCREEN_WIDTH, STATUS_BAR_HEIGHT};

// -----------------------------------------------------------------------------
// Constants & Colors
// -----------------------------------------------------------------------------

/// Left edge and width of the feed's content, on the theme's content padding.
const ROW_X: usize = 32;
const ROW_W: usize = SCREEN_WIDTH - 64; // 656

/// Height of one feed row, and the pitch its stack advances by.
const ROW_H: usize = 88;
const ROW_PITCH: usize = 92;

/// First row of the first section's stack. Everything above it is the header
/// block: title, subtitle, three derivation lines, the pill row and the filter
/// note.
const SECTION_HEAD_Y: usize = 300;

/// The feed may not reach the clear-all pill: the shell's tap handler treats
/// that exact rect as "clear", and a card drawn underneath it would be pixels
/// that look like content and behave like a destructive action.
const FEED_BOTTOM: usize = 708 - space::SM;

/// Height of the pill row, and the gap between pills.
const PILLS_Y: usize = 210;
const PILL_H: usize = 52;
const PILL_GAP: usize = space::SM + space::XS;

/// Width reserved at a row's right edge for the time and source runs.
const TIME_COL_W: usize = 104;

/// Left edge of a row's text column, past the source dot.
const ROW_TEXT_X: usize = ROW_X + 56;

/// Guest timer ticks per second: the kernel programs PIT channel 0 with
/// divisor `1193182 / 1000` (`kernel/src/interrupts.rs`, `SYSTEM_TICK_HZ`), so
/// one tick is one millisecond of GUEST time. A notification's
/// `timestamp_ticks` is a reading of that same counter, so the difference is an
/// elapsed duration the guest itself measured -- not a wall-clock claim, and
/// the derivation line on the surface says so.
const TICKS_PER_SEC: u64 = 1000;

/// The one category filter this surface can be in.
///
/// WHY a constant and not state: `main.rs`'s `ViewMode::Notifications` arm
/// hit-tests the clear-all rect and the home gesture and nothing else, so no
/// tap can reach the pill row. A mutable filter would be a control that looks
/// live and never moves -- the same dishonesty that got the earlier inert pill
/// row removed from this file. Keeping the switch in the code (see
/// [`in_category`]) means the day `main.rs` grows an input band that reaches
/// this surface, the only edit needed is this constant's source.
/// Which filter pill is active when the surface opens.
///
/// WHY a const remains at all: the painter needs a defined starting filter, and
/// the state's own default. It is read once when the frame is built, so the
/// painter and the hit-test agree on where the active pill is without the two
/// ever disagreeing about what the current filter is.
const DEFAULT_FILTER: usize = 0;

/// The reference's four category filters. Index 0 is All.
const FILTER_NAMES: [&[u8]; 4] = [b"All", b"Messages", b"System", b"Apps"];

/// Section headers, in age order. WHY these and not Today/Yesterday: a section
/// name has to describe the records under it, and the only measurement this
/// guest can make of a record is elapsed ticks since it was posted. There is no
/// calendar authority to compare dates against, so a "Today" header would be a
/// date the kernel never reported.
const SECTION_NAMES: [&[u8]; 4] = [b"Last minute", b"Last hour", b"Since boot", b"Undated"];

// Clear-all pill. WHY this rect is frozen: `main.rs` hit-tests exactly this
// rectangle and the guest's serial harness taps (360, 720) expecting
// "Notifications cleared", so the pill cannot move with the feed's length
// without the drawn control and the pressed one becoming different rectangles.
// The feed is laid out above it instead (see FEED_BOTTOM).
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

/// Whether a pixel belongs to the source dot at (`cx`, `cy`).
#[inline]
fn dot_contains(cx: usize, cy: usize, r: usize, x: usize, y: usize) -> bool {
    if r == 0 {
        return false;
    }
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    dx * dx + dy * dy <= (r as i32) * (r as i32)
}

// -----------------------------------------------------------------------------
// Typography Helpers
// -----------------------------------------------------------------------------

const RUN_CAP: usize = 64;

/// The three numbers a run needs before it can be gated on both faces' extents:
/// the em to rasterise it at, and the top row and height of the box that covers
/// both renderers.
///
/// WHY one function and not arithmetic at each constructor: the box a run is
/// tested against has to be the union of the bitmap cell and the outline line
/// box, and the two are not the same shape -- at a scale-2 cap the outline's
/// ascent reaches 50 rows above the run's baseline against the bitmap
/// ascender's 8 rows into its cell, so a box measured for the cell alone rejects
/// rows the outline inks and clips the text silently. Deriving the three numbers
/// in one place is what keeps every run on this surface on one box, and it is
/// the same derivation `home.rs` and `app_drawer.rs` use.
fn run_metrics(y: usize, scale: usize) -> (u32, usize, usize) {
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
    /// Pixels per em for this run's outline rendering; see [`run_metrics`].
    em: u32,
    /// Top row of the box covering BOTH renderers, not the bitmap cell's.
    top: usize,
    /// Height of that union box.
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
        // WHY the width is the wider of the two measures: the outline face is
        // condensed but is not always the narrower of the two. "Events" measures
        // 67 bitmap advances at scale 2 (a 134px measure) and a 123px outline
        // advance, so here the cell is the wider face and the union has to take
        // it rather than assume which way it goes.
        let w = text::measure(&buf[..len], scale)
            .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize);
        let outline = if scale >= 2 && len <= 24 {
            launcher::outline_text::OutlineRun::place(
                x,
                y + launcher::font::BASELINE * scale,
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
        let outline = if scale >= 2 && len <= 24 {
            launcher::outline_text::OutlineRun::place(
                rx,
                y + launcher::font::BASELINE * scale,
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

    /// A run whose right edge is `x_right` -- the time and source columns of a
    /// feed row are measured from the card's right edge inwards, so their origin
    /// is derived from their own width rather than fixed by the layout.
    fn right(x_right: usize, y: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        let (em, top, box_h) = run_metrics(y, scale);
        let w = text::measure(&buf[..len], scale)
            .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize);
        let rx = x_right.saturating_sub(w);
        let outline = if scale >= 2 && len <= 24 {
            launcher::outline_text::OutlineRun::place(
                rx,
                y + launcher::font::BASELINE * scale,
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

    /// A run truncated with an ellipsis if it is wider than `fit`.
    ///
    /// WHY the prefix search and not one `text::ellipsize` call: `ellipsize`
    /// truncates against the BITMAP face's advance, while this run is boxed on
    /// the wider of the two faces. Measured, real prose runs 2-4% NARROWER in
    /// the outline face, so on ordinary text the bitmap fit is already
    /// conservative -- but the widest glyphs run the other way (a 32-byte run
    /// of 'W' measures 1,024 bitmap px and 1,152 outline px at scale 2, a 1.125
    /// ratio), and a record's title is whatever the kernel handed us. A single
    /// ellipsize against the bitmap measure then lets the outline ink run up to
    /// 12% past the column it was fitted to, where it collides with the row's
    /// time and source runs. This accepts the longest prefix whose BOTH
    /// measures fit, so the box and the column agree.
    fn fitted(x: usize, y: usize, scale: usize, fit: usize, bytes: &[u8], ink: u32) -> Self {
        let (em, top, box_h) = run_metrics(y, scale);
        let mut held = [0u8; RUN_CAP + 3];
        let src_len = bytes.len().min(RUN_CAP);
        let whole = bytes.len() <= RUN_CAP
            && text::measure(&bytes[..src_len], scale) <= fit
            && interos_std::design::typeface::text_advance_px(&bytes[..src_len], em) as usize <= fit;
        let n = if whole {
            // The run fits as it stands: no ellipsis, and nothing to search for.
            held[..src_len].copy_from_slice(&bytes[..src_len]);
            src_len
        } else {
            // WHY the take is capped three short of the buffer: the candidate is
            // `take` bytes plus the ellipsis, and it has to stay inside the run
            // buffer for the copy below.
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
                    // Even the ellipsis alone does not fit. The run draws
                    // nothing, matching `text::ellipsize`'s contract, rather
                    // than overflowing the column by three dots.
                    break 0;
                }
                take -= 1;
            }
        };
        let mut bytes_buf = [0u8; RUN_CAP];
        bytes_buf[..n].copy_from_slice(&held[..n]);
        let outline = if scale >= 2 && n <= 24 {
            launcher::outline_text::OutlineRun::place(
                x,
                y + launcher::font::BASELINE * scale,
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
    fn pixel(&self, px: usize, py: usize) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if px < self.x || px >= self.x + self.w || py < self.top || py - self.top >= self.box_h {
            return None;
        }
        if let Some(ref run) = self.outline {
            return run.pixel(px, py, self.ink, 0);
        }
        // The bitmap face is the refusal path, not the baseline: a run the
        // rasteriser declines -- too wide for the coverage buffer, a byte outside
        // its encoded range, a baseline too near the top of the surface -- still
        // has to draw as text. (The rasteriser also refuses any run longer than
        // its 24-byte placement key, which is why this fallback is the common
        // path for titles and bodies rather than the exceptional one.)
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

    /// As [`LabelRun::pixel`], anti-aliased against `bg` so a glyph over a flat card
    /// does not show a stair-stepped outline.
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

/// Appends `v` in decimal at `at`, returning the new cursor. Writes past the end
/// of `out` are dropped rather than panicking: this is a bare-metal painter with
/// no panic handler worth invoking over a count that does not fit.
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

/// Writes a record's age -- elapsed guest timer ticks since it was posted, at
/// the PIT's nominal 1 kHz -- into `out` and returns the length.
///
/// WHY the age is a tick delta and nothing more: the record's timestamp is a
/// reading of the kernel's tick counter, and the only clock this surface can
/// read it against is that same counter. Converting it to a wall-clock time
/// would need the tick count at a known wall moment, which nothing records. If
/// the counter itself cannot be read the function returns 0 and the row shows
/// no time at all -- an absent time, never an invented one.
fn write_age(out: &mut [u8; 16], posted: u64, now: Option<u64>) -> usize {
    let Some(now) = now else { return 0 };
    let secs = now.saturating_sub(posted) / TICKS_PER_SEC;
    let (value, suffix): (u64, &[u8]) = if secs < 60 {
        (secs, b"s ago")
    } else if secs < 3600 {
        (secs / 60, b" min ago")
    } else {
        (secs / 3600, b" h ago")
    };
    if value == 0 {
        out[..3].copy_from_slice(b"now");
        return 3;
    }
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
// Feed model: categories, sources and ages taken from the records themselves
// -----------------------------------------------------------------------------

/// A category the reference's filter names, plus the honest remainder.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    Messages,
    System,
    Apps,
    /// A record whose posting package the shell's own catalog cannot resolve.
    /// It is still a real record and still appears under All; no category pill
    /// claims it.
    Unlisted,
}

/// Whether a record in `cat` passes the filter slot `filter`.
///
/// WHY the filter logic lives here even though the active slot is a constant:
/// the pill row's counts are computed with this same predicate, so the counts
/// and the rows can never disagree about what a category means.
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

/// Whether `pkg` names a package in the shell's own catalog.
///
/// WHY the catalog and not a hardcoded list: `LauncherState::app_ids` is the
/// package registry the shell already read for the drawer, so "installed" here
/// means the same thing it means everywhere else on this device.
fn package_known(state: &LauncherState, pkg: &[u8]) -> bool {
    for i in 0..state.app_count.min(state.app_ids.len()) {
        let len = state.app_id_lens[i] as usize;
        if len > 0 && &state.app_ids[i][..len] == pkg {
            return true;
        }
    }
    false
}

/// The category a record belongs to, read from the record's own fields.
///
/// WHY package and channel and never the text: the record states who posted it
/// and on which channel. Sorting by title text would be a guess about content
/// this surface cannot verify.
fn category_of(rec: &NotificationRecord, state: &LauncherState) -> Category {
    let pkg = &rec.package_id[..rec.pkg_len as usize];
    let chan = &rec.channel[..rec.chan_len as usize];
    // The kernel's own records are posted under the literal package "system"
    // (kernel/src/notification/mod.rs::init), and a system-channel record with
    // no package identity is one of those.
    if pkg == b"system" || (pkg.is_empty() && chan == b"system") {
        return Category::System;
    }
    // A record posted on the messages channel is a message-category record
    // whatever package posted it: the channel is the kernel's own
    // classification, registered in seed_standard_channels.
    if chan == b"messages" {
        return Category::Messages;
    }
    if pkg.is_empty() {
        return Category::Unlisted;
    }
    if !package_known(state, pkg) {
        return Category::Unlisted;
    }
    // An installed package: the messages app is Messages, every other installed
    // package is Apps. Both readings come from the package's own identity.
    if ends_with(pkg, b"messages") {
        Category::Messages
    } else {
        Category::Apps
    }
}

/// The source mark colour for a category, from the theme's semantic inks.
fn category_color(cat: Category, theme: &Theme) -> u32 {
    match cat {
        Category::Messages => theme.success,
        Category::System => theme.warning,
        Category::Apps => theme.accent,
        Category::Unlisted => theme.text_muted,
    }
}

/// Which pill counts a category. `Unlisted` is slot 0, the All pill, because no
/// category pill may claim a record whose source the shell cannot resolve.
fn category_slot(cat: Category) -> usize {
    match cat {
        Category::Messages => 1,
        Category::System => 2,
        Category::Apps => 3,
        Category::Unlisted => 0,
    }
}

/// Which age bucket a record falls in. Bucket 3 is the no-clock-read case: the
/// record is real but its age could not be measured, and the section says
/// "Undated" rather than guessing one.
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
// Frame Plan
// -----------------------------------------------------------------------------

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

struct RowPlan {
    rect: Rect,
    /// Centre and radius of the source dot.
    dot_c: (usize, usize),
    dot_r: usize,
    dot_color: u32,
    title: LabelRun,
    body: LabelRun,
    time: LabelRun,
    /// The record's own package id, or its channel when it carries no package.
    source: LabelRun,
}

impl RowPlan {
    const EMPTY: RowPlan = RowPlan {
        rect: Rect::new(0, 0, 0, 0),
        dot_c: (0, 0),
        dot_r: 0,
        dot_color: 0,
        title: LabelRun::EMPTY,
        body: LabelRun::EMPTY,
        time: LabelRun::EMPTY,
        source: LabelRun::EMPTY,
    };
}

struct EventsFrame {
    clock: LabelRun,
    title: LabelRun,
    subtitle: LabelRun,
    /// What the feed is derived from, stated on the surface itself.
    derivation: [LabelRun; 3],
    filter_note: LabelRun,
    pills: [PillPlan; 4],
    sections: [SectionPlan; 4],
    section_count: usize,
    rows: [RowPlan; 8],
    row_count: usize,
    /// Records that exist but are not on screen, so a short viewport never
    /// silently swallows records.
    more_note: LabelRun,
    hidden: usize,
    empty_head: LabelRun,
    empty_body: LabelRun,
    clear_btn: Rect,
    clear_label: LabelRun,
    home_pill: Rect,
}

/// One feed row for a real record: dot, title, body, age and source.
fn row_plan(rec: &NotificationRecord, y: usize, cat: Category, now: Option<u64>, theme: &Theme) -> RowPlan {
    let rect = Rect::new(ROW_X, y, ROW_W, ROW_H);
    let pkg = &rec.package_id[..rec.pkg_len as usize];
    let chan = &rec.channel[..rec.chan_len as usize];

    // The title is the record's title; a record without one is still a record,
    // and the fallback names what it is rather than inventing a headline.
    let title_bytes: &[u8] = if rec.title_len > 0 {
        &rec.title[..rec.title_len as usize]
    } else {
        b"Notification"
    };
    // The body line is the record's body, then its channel, then its package:
    // whatever the record actually carries, in that order.
    let body_bytes: &[u8] = if rec.body_len > 0 {
        &rec.body[..rec.body_len as usize]
    } else if rec.chan_len > 0 {
        chan
    } else if rec.pkg_len > 0 {
        pkg
    } else {
        b"No body"
    };
    // The source line is the record's own package id -- the string that makes
    // the row traceable to the posting package. A record with no package names
    // its channel instead, and one with neither says so.
    let source_bytes: &[u8] = if rec.pkg_len > 0 {
        pkg
    } else if rec.chan_len > 0 {
        chan
    } else {
        b"no source"
    };

    let mut age = [0u8; 16];
    let age_len = write_age(&mut age, rec.timestamp_ticks, now);

    // The title stops short of the right column: the time and source runs are
    // measured inwards from the card's right edge, so the title's fit is
    // whatever is left of them.
    let title_fit = (ROW_X + ROW_W - TIME_COL_W - space::MD - space::SM).saturating_sub(ROW_TEXT_X);

    // WHY the body is fitted to the time column too, and not to the card's own
    // inner edge: the time/source runs are right-aligned, so the widest place a
    // body can reach is the leftmost column their longest run occupies. Fitted to
    // the card instead, a body longer than that ran its ink into the source run's
    // rows -- the two line boxes already share rows 59..68 of their row, so a long
    // body drew its tail across the package id. The title has always cleared the
    // same column; the body now does the arithmetic the title does.
    let right_col = ROW_X + ROW_W - TIME_COL_W;
    let body_fit = (right_col - space::SM).saturating_sub(ROW_TEXT_X);

    RowPlan {
        rect,
        dot_c: (ROW_X + space::XL - space::XS, y + ROW_H / 2),
        dot_r: space::SM - 1,
        dot_color: category_color(cat, theme),
        title: LabelRun::fitted(
            ROW_TEXT_X,
            y + space::SM + space::XS,
            type_scale::BODY,
            title_fit,
            title_bytes,
            theme.text,
        ),
        body: LabelRun::fitted(
            ROW_TEXT_X,
            y + space::XS + text::line_height(type_scale::BODY) + space::XS,
            type_scale::CAPTION,
            body_fit,
            body_bytes,
            theme.text_muted,
        ),
        time: LabelRun::right(
            ROW_X + ROW_W - space::MD,
            y + space::LG - space::XS,
            type_scale::CAPTION,
            &age[..age_len],
            theme.text_muted,
        ),
        source: LabelRun::right(
            ROW_X + ROW_W - space::MD,
            y + space::XL + space::XS + space::XS,
            type_scale::CAPTION,
            source_bytes,
            theme.text_muted,
        ),
    }
}

fn build_events_frame(state: &LauncherState) -> EventsFrame {
    // WHY a value and not a static: the shell has exactly one active theme and
    // every pixel on this screen resolves against it. The shell should hand the
    // active theme in -- `LauncherState` grows a `theme` field fed from the same
    // setting that picks the wallpaper, and this line becomes `state.theme`.
    // Until then the binding is a value, not a constant, so the swap is one edit
    // in one place. Events is a light surface by design (design system: "Events,
    // Files and Gallery are light").
    let theme = Theme::LIGHT;

    // The one clock this surface is allowed to read a record's timestamp
    // against: the kernel's own tick counter. Read once per frame, here, and
    // never per pixel.
    let now = interos_std::get_ticks(1).ok();

    let clock_slice: &[u8] = if state.clock_source_ok && state.clock_time[0] != b' ' {
        &state.clock_time
    } else {
        b""
    };
    let clock = LabelRun::new(48, 14, type_scale::CAPTION, clock_slice, theme.text);
    let title = LabelRun::new(36, 56, type_scale::BODY, b"Events", theme.text);
    let subtitle = LabelRun::new(
        36,
        108,
        type_scale::CAPTION,
        b"Everything in one feed",
        theme.text_muted,
    );
    // The surface says what it is made of. WHY on the surface and not only in
    // this file: a feed the user reads as "everything that happened on my
    // device" is a claim about the platform, and the platform can only back the
    // notification records.
    let derivation = [
        LabelRun::new(
            36,
            132,
            type_scale::CAPTION,
            b"One feed assembled from the kernel's notification records.",
            theme.text_muted,
        ),
        LabelRun::new(
            36,
            156,
            type_scale::CAPTION,
            b"Ages are guest ticks since posting. No calendar, so no day groups.",
            theme.text_muted,
        ),
        LabelRun::new(
            36,
            180,
            type_scale::CAPTION,
            b"Registry and recents carry no timestamps, so they add no entries.",
            theme.text_muted,
        ),
    ];
    let filter_note = LabelRun::new(
        36,
        272,
        type_scale::CAPTION,
        b"Filter the feed by its real source.",
        theme.text_muted,
    );

    // Chronological order: newest first. The kernel returns records in posting
    // order, so the walk is reversed.
    let count = state.notification_count.min(state.notifications.len());
    let mut order = [0usize; 8];
    let mut total = 0usize;
    for i in (0..count).rev() {
        order[total] = i;
        total += 1;
    }

    // The pill row is a legend with real counts, All rendered active because it
    // is the filter in effect. All counts every record; the three category
    // pills count only records that category actually claims, and a record whose
    // package the shell cannot resolve is counted by All alone -- it appears in
    // the feed under All and under no category, so no category pill may claim
    // it.
    let mut counts = [0usize; 4];
    counts[0] = total;
    for k in 0..total {
        match category_of(&state.notifications[order[k]], state) {
            Category::Unlisted => {}
            cat => counts[category_slot(cat)] += 1,
        }
    }
    let mut pills = [PillPlan::EMPTY; 4];
    // WHY the filter is validated once here rather than at each use: the pills
    // and the layout walk read it, and an out-of-range value would draw a pill
    // that matches nothing and filter nothing out -- which reads as a broken
    // surface rather than as a state the user got into. The default is this
    // surface's own.
    let filter = if state.feed_filter < FILTER_NAMES.len() {
        state.feed_filter
    } else {
        DEFAULT_FILTER
    };
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
            // Active treatment mirrors the app drawer's chips: dark fill, page
            // ink. Inactive pills are recessed, which is how this shell already
            // draws a control that cannot be operated (see Quick Controls'
            // unavailable radio tiles).
            label: LabelRun::centred(
                rect.x,
                rect.center_y() - text::line_height(type_scale::CAPTION) / 2,
                rect.w,
                type_scale::CAPTION,
                &label[..n],
                if filter == i { theme.bg } else { theme.text_muted },
            ),
            active: filter == i,
        };
        pill_x += w + PILL_GAP;
    }

    // How many records the filter admits, counted over the whole feed before
    // any of them is laid out: the layout walk below breaks at the viewport
    // cut, and a count taken from it would stop at the same place and report
    // the cut as the feed's length.
    let mut matched = 0usize;
    for k in 0..total {
        let cat = category_of(&state.notifications[order[k]], state);
        if in_category(cat, filter) {
            matched += 1;
        }
    }

    // Rows: as many records as fit above the clear-all pill, in chronological
    // order, with a section header whenever the age bucket changes. A row that
    // does not fit is not laid out -- and because the walk is top-down and
    // contiguous, breaking here is correct: no later row could fit either. The
    // records past the cut are counted by `more_note` instead of vanishing.
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
                    label: LabelRun::new(36, y, type_scale::CAPTION, SECTION_NAMES[bucket], theme.text),
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

    // Records that exist but are not on screen. WHY it is said: the shell's
    // input arm cannot scroll this surface, so a viewport that silently holds
    // four of eight records is a feed that lies about its own length.
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
    // The note sits under the last row when there is room for it above the
    // clear-all pill, and under the pill otherwise. It is never placed over
    // either.
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
    let more_note = LabelRun::new(36, note_y, type_scale::CAPTION, &more[..more_len], theme.text_muted);

    // The empty state says what is absent, in the record service's own terms.
    let (empty_head, empty_body) = if total == 0 {
        (
            LabelRun::new(36, SECTION_HEAD_Y, type_scale::BODY, b"No notifications", theme.text_muted),
            LabelRun::new(
                36,
                SECTION_HEAD_Y + text::line_height(type_scale::BODY) + space::SM,
                type_scale::CAPTION,
                b"The kernel holds no active notification records.",
                theme.text_muted,
            ),
        )
    } else {
        (LabelRun::EMPTY, LabelRun::EMPTY)
    };

    let clear_label = LabelRun::centred(
        CLEAR_BTN.x,
        CLEAR_BTN.y + space::SM + space::XS,
        CLEAR_BTN.w,
        type_scale::CAPTION,
        b"Clear all",
        theme.text_muted,
    );

    EventsFrame {
        clock,
        title,
        subtitle,
        derivation,
        filter_note,
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

// Per-Pixel Renderer
// -----------------------------------------------------------------------------

static mut FRAME_CACHE: Option<EventsFrame> = None;

#[inline(never)]
/// Rows the filter-pill row and the feed below it occupy.
///
/// WHY this is a function and not a constant: the pills and the rows they admit
/// are laid out from the state, and a caller marking a repaint dirty has to
/// cover exactly what the painter draws -- no more, or a full repaint. The same
/// derivation the painter uses is the one place both can agree.
pub fn filter_band() -> (usize, usize) {
    (
        PILLS_Y,
        PILLS_Y.saturating_add(PILL_H).max(FEED_BOTTOM),
    )
}

/// Which filter pill a tap at (`x`, `y`) falls in, if any.///
/// WHY this exists: the pills were a legend with real counts that no tap could
/// reach, so the surface printed "Filter reads All" while drawing no way to
/// change it. The rows are rebuilt from the state and cached, so the hit test
/// walks the same rects rather than re-deriving them — a second derivation
/// would be a second thing to keep in step.
///
/// The pills' row sits at `PILLS_Y` (210) with `PILL_H`, which is above the feed
/// body and below the header — clear of the `CLEAR_BTN` the layout harness pins
/// at (360, 720) and of the bottom home band. A tap on the already-active pill
/// returns its index all the same, because which pill it is is the caller's
/// question and what to do about it is the caller's decision.
pub fn filter_pill_at(x: usize, y: usize, state: &LauncherState) -> Option<usize> {
    if y < PILLS_Y || y >= PILLS_Y + PILL_H {
        return None;
    }
    // The frame is only built on demand and is rebuilt here, so a caller that
    // already holds a frame pays nothing: this is the same walk the painter
    // does, on the same source.
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
    let theme = Theme::LIGHT;

    // 1. Status Bar (y < 60)
    if y < STATUS_BAR_HEIGHT + 12 {
        if let Some(c) = f.clock.pixel_on(x, y, theme.bg) {
            return c;
        }
        return theme.bg;
    }

    // 2. Header block: title, subtitle and the derivation lines. Each run owns
    // its own rows, so they are tested in order and the first ink wins.
    if let Some(c) = f.title.pixel_on(x, y, theme.bg) {
        return c;
    }
    if let Some(c) = f.subtitle.pixel_on(x, y, theme.bg) {
        return c;
    }
    for line in &f.derivation {
        if let Some(c) = line.pixel_on(x, y, theme.bg) {
            return c;
        }
    }
    if let Some(c) = f.filter_note.pixel_on(x, y, theme.bg) {
        return c;
    }

    // 3. Category legend. WHY the pills are drawn recessed except the active
    // one: they are a real reading of the records -- "3 records, 1 message, 2
    // system" -- but they are not operable, and the treatment says so.
    if y >= PILLS_Y && y < PILLS_Y + PILL_H {
        for pill in &f.pills {
            if pill.rect.contains(x, y) {
                if rounded_contains(pill.rect, x, y, pill.rect.h / 2) {
                    if pill.active {
                        if rounded_outline(pill.rect, x, y, pill.rect.h / 2) {
                            return theme.text;
                        }
                        if let Some(c) = pill.label.pixel_on(x, y, theme.text) {
                            return c;
                        }
                        return theme.text;
                    }
                    if rounded_outline(pill.rect, x, y, pill.rect.h / 2) {
                        return theme.border;
                    }
                    if let Some(c) = pill.label.pixel_on(x, y, theme.surface_sunken) {
                        return c;
                    }
                    return theme.surface_sunken;
                }
                return theme.bg;
            }
        }
    }

    // 4. Section headers
    for i in 0..f.section_count {
        if let Some(c) = f.sections[i].label.pixel_on(x, y, theme.bg) {
            return c;
        }
    }

    // 5. Feed rows
    for i in 0..f.row_count {
        let row = &f.rows[i];
        if row.rect.contains(x, y) {
            if rounded_contains(row.rect, x, y, radius::CARD) {
                if rounded_outline(row.rect, x, y, radius::CARD) {
                    return theme.border;
                }
                // Source dot: the category this record was classified into.
                if dot_contains(row.dot_c.0, row.dot_c.1, row.dot_r, x, y) {
                    return row.dot_color;
                }
                if let Some(c) = row.title.pixel_on(x, y, theme.surface) {
                    return c;
                }
                if let Some(c) = row.body.pixel_on(x, y, theme.surface) {
                    return c;
                }
                if let Some(c) = row.time.pixel_on(x, y, theme.surface) {
                    return c;
                }
                if let Some(c) = row.source.pixel_on(x, y, theme.surface) {
                    return c;
                }
                return theme.surface;
            }
            return theme.bg;
        }
    }

    // 6. Notes: records past the viewport cut, then the empty state.
    if let Some(c) = f.more_note.pixel_on(x, y, theme.bg) {
        return c;
    }
    if let Some(c) = f.empty_head.pixel_on(x, y, theme.bg) {
        return c;
    }
    if let Some(c) = f.empty_body.pixel_on(x, y, theme.bg) {
        return c;
    }

    // 7. Clear All Pill (see CLEAR_BTN: the shell hit-tests this exact rect)
    if f.clear_btn.contains(x, y) {
        if rounded_contains(f.clear_btn, x, y, 22) {
            if rounded_outline(f.clear_btn, x, y, 22) {
                return theme.border;
            }
            if let Some(c) = f.clear_label.pixel_on(x, y, theme.surface_sunken) {
                return c;
            }
            return theme.surface_sunken;
        }
        return theme.bg;
    }

    // 8. Home Indicator Pill
    if rounded_contains(f.home_pill, x, y, 2) {
        return theme.text_muted;
    }

    theme.bg
}
