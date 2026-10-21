//! Live Widgets dashboard.
//!
//! WHY `pub`: `render_view` in the crate root dispatches here by view mode.
//! Geometry and colour come from `launcher::theme`, `launcher::layout` and
//! `launcher::text`, never from literals local to this file.
//!
//! WHY this surface is five entry points and a list of readings: the previous
//! version drew a weather tile, a build-progress bar, a media player, a meeting
//! card, activity rings, a hardware gauge and an appearance card, all with their
//! own hand-typed rectangles, and five of the eight were showing a plausible
//! number for something this platform cannot measure. A "Build Status" bar at 60%
//! and a "Steps" ring at three quarters are indistinguishable, on screen, from a
//! build that is running and a pedometer that is counting.
//!
//! Worse, the five cards were the only things on this screen that could be
//! pressed, and none of them said what pressing did: the tile labelled "Weather"
//! opened the sensor dashboard, and the tile labelled "Build Status" opened the
//! calculator. So the shell now says what each of them actually does -- they are
//! the five apps `main.rs` opens for a tap in that region -- and everything below
//! them is a reading, present or absent, derived from `LauncherState`.
//!
//! WHY the drawing primitives come from `draw`: they are `pub(super)`
//! precisely so the sibling surfaces that share this visual language share one
//! implementation of "rounded rectangle" and "centre this run of text". See the
//! note at the top of that file about where they live.

use core::cell::UnsafeCell;

use super::draw::{
    counted, decimal, push, round_contains, run_center, run_left, run_right, Reading,
    VALUE_MAX,
};
use crate::screens::lock::LOCK_DATE_TEXT_LEN;
use crate::LauncherState;
use launcher::layout::{self, Rect};
use launcher::text;
use launcher::theme::*;

/// WHY `Theme::LIGHT`: this is a glanceable summary of what the device knows, and
/// the reference's is a light page of white cards on near-white, sitting between
/// the dark Power drawer and the near-black lock surface.
const THEME: Theme = Theme::LIGHT;

// -----------------------------------------------------------------------------
// The five things a tap on this surface actually opens
// -----------------------------------------------------------------------------
// WHY these rectangles are typed and are the only typed geometry here: the
// shell's touch handler lives in `main.rs`, it hit-tests these five regions, and
// it opens the package named beside each one. A card drawn anywhere else would
// be a control that cannot be pressed, and a card drawn here under a different
// name would be a control that lies about what it does. `main.rs` is not part of
// this change, so the drawn card and the hit region have to be the same rectangle
// until it is. Exporting these next to the handler is the change that would let
// them be computed, exactly as the lock screen exports its disc geometry.

/// One wired entry point: the rectangle `main.rs` tests, and what it opens.
struct Entry {
    rect: Rect,
    package: &'static str,
}

const ENTRIES: [Entry; 5] = [
    Entry { rect: Rect::new(40, 135, 305, 180), package: "org.interos.settings" },
    Entry { rect: Rect::new(375, 135, 305, 180), package: "org.interos.calculator" },
    Entry { rect: Rect::new(40, 335, 640, 150), package: "org.interos.music" },
    Entry { rect: Rect::new(40, 505, 305, 180), package: "org.interos.calendar" },
    Entry { rect: Rect::new(375, 505, 305, 180), package: "org.interos.vault" },
];

/// One step below the last entry point. WHY it is written in terms of that
/// rectangle rather than as its own number: this is the one place where a typed
/// hit region and a computed layout meet, and it should move when the region does.
const LIST_TOP: usize = ENTRIES[4].rect.y + ENTRIES[4].rect.h + space::LG;
/// Row label and value scale.
///
/// WHY 2: a row has to hold one whole glyph cell, and this face's cell is
/// `GLYPH_H` = 24 rows per scale step, not the 8 the row heights were sized
/// against. The row height here is computed from the space the list is given, so
/// the scale is fixed by the smallest row the list will ever be handed, which is
/// [`ROW_MIN`]. At `type_scale::BODY` (2) the cell is 48 px, which is what
/// [`ROW_MIN`] holds for.
const ROW_TEXT_SCALE: usize = 2;

/// Vertical and horizontal inset inside a reading row.
///
/// WHY the vertical inset is `space::SM` and not `space::MD`: the row has to hold
/// a 48 px cell plus its padding, and there are ten of them between the last entry
/// card and the home indicator. At a 16 px inset the floor is 80 px, ten rows plus
/// gutters need 872 px, and the band is 703 -- so the floor could be honoured and
/// two readings pushed off the bottom of the surface, which would be a worse lie
/// than the one it replaced (the heading counts "of 10 readings" whatever it
/// draws). At 8 px the floor is 64, ten rows land at 66 each and all ten are drawn.
/// The horizontal inset is unchanged, so no label moves sideways.
const ROW_PAD_Y: usize = space::SM;
const ROW_PAD_X: usize = space::MD;

/// Row gutter, on the spacing scale.
///
/// WHY `space::XS`: it is what makes ten rows fit the band once the rows are tall
/// enough to hold their own type. `space::SM` puts the last row's bottom edge one
/// row past the band, which clips the tail of its descenders -- the same defect
/// this floor exists to remove, one row up.
const ROW_GUTTER: usize = space::XS;
/// Row height bounds. WHY the list does not simply use one token for its rows: the
/// list is as tall as the content area allows, so a fixed row height leaves a dead
/// band above the home indicator that grows every time a reading is added. The
/// ceiling stops a short list becoming slabs; the floor is the row's own content,
/// so no row can ever be shorter than the type inside it.
///
/// WHY the floor is a *cell plus its padding* and not just the cell: a row is
/// inset by [`ROW_PAD_Y`] top and bottom before anything is drawn in it, so a row
/// that is only as tall as the 48 px cell has 16 px of usable height. `text_y0`
/// then saturates to the pad's own top edge and the painter's band test
/// (`y < text_y0 + text_h`) runs past the bottom of the row rectangle, so every row
/// lost its descender rows: "Packages" and "Appearance" rendered with the tails
/// sliced flat, and the last rows were cut off the surface entirely. The floor has
/// to be what the row actually has to contain.
///
/// WHY the parentheses in the expression that uses this: written as a method chain
/// ending in `.clamp(..)`, `a / ROWS.len().clamp(..)` divides by the clamped count
/// instead of clamping the quotient, because a method call binds tighter than `/`.
/// That is not a typo risk, it is what the surface rendered: ten reading rows ten
/// pixels tall, eighteen pixels apart, with no room for a line of type and no text
/// in any of them.
const ROW_MIN: usize = text::GLYPH_H * ROW_TEXT_SCALE + ROW_PAD_Y * 2;
const ROW_MAX: usize = space::XXL * 2;

// -----------------------------------------------------------------------------
// The readings
// -----------------------------------------------------------------------------

/// One reading this surface can report.
///
/// WHY an enum rather than a struct per card: the difference between the two
/// kinds of row on this screen is whether a number behind it exists, and an
/// enum makes that the only question a new row has to answer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    Clock,
    Date,
    Notifications,
    Packages,
    Appearance,
    Weather,
    Media,
    Calendar,
    Activity,
    Telemetry,
}

/// WHY `ALL` is a list and not a range: the surface shows every row it has, so
/// the order is the reading order and it has to be written down once.
const ROWS: [Row; 10] = [
    Row::Clock,
    Row::Date,
    Row::Notifications,
    Row::Packages,
    Row::Appearance,
    Row::Weather,
    Row::Media,
    Row::Calendar,
    Row::Activity,
    Row::Telemetry,
];

impl Row {
    /// The label on the left of the row.
    fn label(self) -> &'static [u8] {
        match self {
            Row::Clock => b"Clock",
            Row::Date => b"Date",
            Row::Notifications => b"Notifications",
            Row::Packages => b"Packages",
            Row::Appearance => b"Appearance",
            Row::Weather => b"Weather",
            Row::Media => b"Media",
            Row::Calendar => b"Calendar",
            Row::Activity => b"Activity",
            Row::Telemetry => b"Telemetry",
        }
    }

    /// Why there is no reading, in the user's language. None of these is "0".
    ///
    /// WHY each one names the missing thing: this platform has no location
    /// service, no media session published to the shell, no calendar store in
    /// `LauncherState`, no accelerometer to count steps from, and a fuel gauge
    /// and thermal sensor that both report themselves absent. A row that said
    /// "Unavailable" five times would say it once; a row that says "no fuel
    /// gauge" tells the user what is missing.
    fn absent(self) -> &'static [u8] {
        match self {
            Row::Weather => b"no location source",
            Row::Media => b"no media session",
            Row::Calendar => b"no event data",
            Row::Activity => b"no pedometer",
            Row::Telemetry => b"no sensor bound",
            _ => b"no RTC reading",
        }
    }

    /// Whether the shell holds a reading for this row right now.
    fn available(self, state: &LauncherState) -> bool {
        match self {
            Row::Clock | Row::Date => state.clock_source_ok,
            Row::Notifications | Row::Packages | Row::Appearance => true,
            _ => false,
        }
    }

    /// The value, or the reason there is none.
    ///
    /// WHY availability is decided here, next to the bytes that would be
    /// printed: the one rule this screen must not break is printing a number
    /// where the platform has no sensor to produce one, and the only reliable
    /// place to enforce that is the line that builds the number.
    fn value(self, state: &LauncherState) -> Reading {
        match self {
            Row::Clock => {
                if state.clock_source_ok {
                    Reading::of(&state.clock_time)
                } else {
                    Reading::unavailable(self.absent())
                }
            }
            Row::Date => {
                if state.clock_source_ok {
                    Reading::of(&state.date_line[..LOCK_DATE_TEXT_LEN])
                } else {
                    Reading::unavailable(self.absent())
                }
            }
            Row::Notifications => {
                let (buf, len) = counted(state.notification_count, b" active");
                Reading::of(&buf[..len])
            }
            Row::Packages => {
                let (buf, len) = counted(state.app_count, b" registered");
                Reading::of(&buf[..len])
            }
            // WHY the theme's own name: the appearance row is the one reading
            // this shell can always make, and it is made from the value every
            // other pixel on this screen is derived from. Spelling "light" out in
            // a literal would make it the one row that cannot follow a theme
            // change.
            Row::Appearance => Reading::of(THEME.name.as_bytes()),
            _ => Reading::unavailable(self.absent()),
        }
    }
}

/// `<available> of <total> readings` -- the honest headline for a dashboard whose
/// job is to say what the device knows.
fn reading_summary(state: &LauncherState) -> ([u8; VALUE_MAX], usize) {
    let mut held = 0usize;
    for row in ROWS {
        if row.available(state) {
            held += 1;
        }
    }
    let (mut buf, mut at) = decimal(held);
    push(&mut buf, &mut at, b" of ");
    let (total, total_len) = decimal(ROWS.len());
    push(&mut buf, &mut at, &total[..total_len]);
    push(&mut buf, &mut at, b" readings");
    (buf, at)
}

// -----------------------------------------------------------------------------
// Entry points
// -----------------------------------------------------------------------------

/// The registry's display name for `package`, or `None` if it is not installed.
fn installed_label<'a>(state: &'a LauncherState, package: &str) -> Option<&'a [u8]> {
    let pkg_bytes = package.as_bytes();
    for i in 0..state.app_count {
        let id_len = state.app_id_lens[i] as usize;
        if id_len != pkg_bytes.len() || &state.app_ids[i][..id_len] != pkg_bytes {
            continue;
        }
        let label_len = state.app_label_lens[i] as usize;
        if label_len > 0 {
            return Some(&state.app_labels[i][..label_len]);
        }
    }
    None
}

// -----------------------------------------------------------------------------
// Per-frame cached layout & metrics
// -----------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct EntryFrame {
    rect: Rect,
    inner: Rect,
    fill: u32,
    border: u32,
    name_rect: Rect,
    name_buf: [u8; 32],
    name_len: usize,
    name_ink: u32,
    chevron: Option<Rect>,
    caption: Option<Rect>,
}

#[derive(Clone, Copy)]
struct RowFrame {
    rect: Rect,
    inner: Rect,
    fill: u32,
    border: u32,
    text_y0: usize,
    text_h: usize,
    value_col: Rect,
    value_buf: [u8; VALUE_MAX],
    value_len: usize,
    value_ink: u32,
    label_col: Rect,
    label_buf: [u8; 32],
    label_len: usize,
    label_ink: u32,
}

#[derive(Clone, Copy)]
struct WidgetsFrame {
    status_time: [u8; 8],
    status_time_len: usize,
    title_rect: Rect,
    summary_rect: Rect,
    summary_buf: [u8; VALUE_MAX],
    summary_len: usize,
    entries: [EntryFrame; 5],
    rows: [RowFrame; 10],
    stack_y: usize,
    row_stride: usize,
    bounds: Rect,
}

impl WidgetsFrame {
    const fn empty() -> Self {
        const EMPTY_ENTRY: EntryFrame = EntryFrame {
            rect: Rect::new(0, 0, 0, 0),
            inner: Rect::new(0, 0, 0, 0),
            fill: 0,
            border: 0,
            name_rect: Rect::new(0, 0, 0, 0),
            name_buf: [0u8; 32],
            name_len: 0,
            name_ink: 0,
            chevron: None,
            caption: None,
        };
        const EMPTY_ROW: RowFrame = RowFrame {
            rect: Rect::new(0, 0, 0, 0),
            inner: Rect::new(0, 0, 0, 0),
            fill: 0,
            border: 0,
            text_y0: 0,
            text_h: 0,
            value_col: Rect::new(0, 0, 0, 0),
            value_buf: [0u8; VALUE_MAX],
            value_len: 0,
            value_ink: 0,
            label_col: Rect::new(0, 0, 0, 0),
            label_buf: [0u8; 32],
            label_len: 0,
            label_ink: 0,
        };
        WidgetsFrame {
            status_time: [0u8; 8],
            status_time_len: 0,
            title_rect: Rect::new(0, 0, 0, 0),
            summary_rect: Rect::new(0, 0, 0, 0),
            summary_buf: [0u8; VALUE_MAX],
            summary_len: 0,
            entries: [EMPTY_ENTRY; 5],
            rows: [EMPTY_ROW; 10],
            stack_y: 0,
            row_stride: 0,
            bounds: Rect::new(0, 0, 0, 0),
        }
    }

    fn measure(state: &LauncherState) -> Self {
        let theme = THEME;
        let mut status_time = [0u8; 8];
        let status_time_len = state.clock_time.len().min(8);
        status_time[..status_time_len].copy_from_slice(&state.clock_time[..status_time_len]);

        let content = layout::content_area(CONTENT_PADDING);

        // Entries
        let mut entries = [Self::empty().entries[0]; 5];
        for (i, entry) in ENTRIES.iter().enumerate() {
            let rect = entry.rect;
            let inner = rect.inset(1);
            let installed = installed_label(state, entry.package);
            let pad = rect.inset(space::MD);
            let name_slice = match installed {
                Some(label) => label,
                None => entry.package.as_bytes(),
            };

            let caption_h = if installed.is_none() { text::line_height(type_scale::CAPTION) } else { 0 };

            // WHY the chevron's width is reserved before the name is measured: the
            // chevron is drawn at the right edge of the same pad the name is
            // centred in, so a name fitted to the whole padded width runs straight
            // underneath it. Reserving it here is what makes `name_rect` -- which
            // is also the centring width -- stop short of the chevron, and the
            // ellipsize is what turns "too long" into an ellipsised name instead of
            // a name `run_center` silently refuses to draw.
            let chevron_w = text::measure(b">", type_scale::BODY);
            let name_fit = if installed.is_some() {
                pad.w.saturating_sub(chevron_w + space::SM)
            } else {
                pad.w
            };
            let name_rect = Rect::new(pad.x, pad.y, name_fit, pad.h.saturating_sub(caption_h + space::SM));

            let (name_fitted, name_fitted_len) =
                text::ellipsize(name_slice, type_scale::BODY, name_fit);
            let mut name_buf = [0u8; 32];
            let name_len = name_fitted_len.min(32);
            name_buf[..name_len].copy_from_slice(&name_fitted[..name_len]);

            let (fill, name_ink, chevron, caption) = if installed.is_some() {
                let fill = Theme::blend(theme.surface_container, theme.primary, 22);
                let chevron = Rect::new(
                    pad.x + pad.w.saturating_sub(chevron_w),
                    name_rect.y,
                    chevron_w,
                    name_rect.h,
                );
                (fill, theme.on_surface, Some(chevron), None)
            } else {
                let caption = Rect::new(pad.x, name_rect.y + name_rect.h + space::SM, pad.w, caption_h);
                (theme.surface_container_low, theme.on_surface_variant, None, Some(caption))
            };

            entries[i] = EntryFrame {
                rect,
                inner,
                fill,
                border: theme.border,
                name_rect,
                name_buf,
                name_len,
                name_ink,
                chevron,
                caption,
            };
        }

        // Title and summary
        //
        // WHY the summary sits under the title and not beside it: they are drawn
        // with `run_left`/`run_right` at different scales, and two runs of
        // different scales can only share rows if the smaller one is placed from
        // the larger one's baseline. It was -- `heading_baseline -
        // text::BASELINE * ROW_TEXT_SCALE` -- and with a 3x title and a 2x summary
        // that put both runs' ink in 82..115, so the summary's leading "10 " was
        // drawn under the title's ink and the screen read "of 10 readings".
        // Stacked line boxes cannot overlap at any pair of scales.
        let title_rect = Rect::new(
            content.x,
            HEADER_TOP,
            content.w,
            text::line_height(WIDGETS_TITLE_SCALE),
        );
        let summary_rect = Rect::new(
            content.x,
            title_rect.y + title_rect.h + WIDGETS_HEADING_GAP,
            content.w,
            text::line_height(WIDGETS_SUMMARY_SCALE),
        );
        let (summary_raw, summary_len) = reading_summary(state);
        let mut summary_buf = [0u8; VALUE_MAX];
        summary_buf[..summary_len].copy_from_slice(&summary_raw[..summary_len]);

        // Rows
        //
        // WHY the band reaches to the home indicator rather than to the content
        // area's own padding: the band is what decides how tall a row may be, so
        // stopping it short would make every row shorter than the type inside it.
        let bottom = SCREEN_HEIGHT - space::MD - INDICATOR_H - space::SM;
        let bounds = Rect::new(content.x, LIST_TOP, content.w, bottom.saturating_sub(LIST_TOP));
        let row_h = (bounds.h.saturating_sub(ROW_GUTTER * (ROWS.len() - 1)) / ROWS.len())
            .clamp(ROW_MIN, ROW_MAX);
        let stack = layout::list(bounds, ROWS.len(), row_h, ROW_GUTTER).unwrap_or(bounds);
        let row_stride = row_h + ROW_GUTTER;
        let mut rows = [Self::empty().rows[0]; 10];

        for (i, row_item) in ROWS.iter().enumerate() {
            if let Some(rect) = layout::list_row(stack, i, ROWS.len(), row_h, ROW_GUTTER) {
                let inner = rect.inset(1);
                let value_reading = row_item.value(state);
                let (fill, value_ink) = value_reading.skin(&theme);

                // WHY the pad is built from the two insets and not `rect.inset(..)`:
                // the vertical inset is what the row's height budget has to carry,
                // and it is a different token from the horizontal one. See
                // [`ROW_PAD_Y`].
                let pad = Rect::new(
                    rect.x + ROW_PAD_X,
                    rect.y + ROW_PAD_Y,
                    rect.w.saturating_sub(ROW_PAD_X * 2),
                    rect.h.saturating_sub(ROW_PAD_Y * 2),
                );
                let h = text::line_height(ROW_TEXT_SCALE);
                let text_y0 = pad.y + pad.h.saturating_sub(h) / 2;

                let value_w = text::measure(value_reading.as_slice(), ROW_TEXT_SCALE).min(pad.w * 50 / 100) + space::SM;
                let value_col = Rect::new(pad.x + pad.w - value_w, pad.y, value_w, pad.h);
                let label_col = Rect::new(pad.x, pad.y, pad.w - value_w - space::SM, pad.h);

                let (v_run, v_len) = text::ellipsize(value_reading.as_slice(), ROW_TEXT_SCALE, value_col.w);
                let (l_run, l_len) = text::ellipsize(row_item.label(), ROW_TEXT_SCALE, label_col.w);

                let mut value_buf = [0u8; VALUE_MAX];
                let value_len = v_len.min(VALUE_MAX);
                value_buf[..value_len].copy_from_slice(&v_run[..value_len]);

                let mut label_buf = [0u8; 32];
                let label_len = l_len.min(32);
                label_buf[..label_len].copy_from_slice(&l_run[..label_len]);

                rows[i] = RowFrame {
                    rect,
                    inner,
                    fill,
                    border: theme.border,
                    text_y0,
                    text_h: h,
                    value_col,
                    value_buf,
                    value_len,
                    value_ink,
                    label_col,
                    label_buf,
                    label_len,
                    label_ink: theme.on_surface,
                };
            }
        }

        WidgetsFrame {
            status_time,
            status_time_len,
            title_rect,
            summary_rect,
            summary_buf,
            summary_len,
            entries,
            rows,
            stack_y: stack.y,
            row_stride,
            bounds,
        }
    }
}

struct FrameSlot {
    state: usize,
    at_x: usize,
    at_y: usize,
    measured: bool,
    frame: WidgetsFrame,
}

impl FrameSlot {
    fn holds_next(&self, x: usize, y: usize, state: usize) -> bool {
        self.measured
            && self.state == state
            && ((x == self.at_x + 1 && y == self.at_y) || (x == 0 && y == self.at_y + 1))
    }
}

struct FrameMemo(UnsafeCell<FrameSlot>);
unsafe impl Sync for FrameMemo {}

static FRAME_MEMO: FrameMemo = FrameMemo(UnsafeCell::new(FrameSlot {
    state: 0,
    at_x: 0,
    at_y: 0,
    measured: false,
    frame: WidgetsFrame::empty(),
}));

fn frame_for(x: usize, y: usize, state: &LauncherState) -> &WidgetsFrame {
    let addr = state as *const LauncherState as usize;
    let slot = unsafe { &mut *FRAME_MEMO.0.get() };
    if !slot.holds_next(x, y, addr) {
        slot.frame = WidgetsFrame::measure(state);
        slot.state = addr;
        slot.measured = true;
    }
    slot.at_x = x;
    slot.at_y = y;
    &slot.frame
}

// -----------------------------------------------------------------------------
// The surface
// -----------------------------------------------------------------------------

/// Top of the heading band.
const HEADER_TOP: usize = STATUS_BAR_HEIGHT + space::SM;

/// The heading's scale.
///
/// WHY `type_scale::BODY` and not `type_scale::TITLE`: the heading owns the band
/// from [`HEADER_TOP`] to the top of the first entry card, and that band is 79
/// rows tall -- the entry rectangles are the shell's hit regions and cannot move.
/// A `TITLE` cell is 72 rows, so a title-sized heading leaves 7 rows for the
/// summary that has to sit under it, and a summary needs 24. The heading therefore
/// takes the rung below and both lines fit with room to spare.
///
/// WHY the two lines are stacked and not sharing a baseline: a shared baseline is
/// what put them on top of each other in the first place. Two runs of different
/// scales only share ink rows safely when they are the same size; at 3 and 2 the
/// summary's cell started 40 rows above the title's baseline, so its ink ran
/// across the title's and the leading "10 " of "6 of 10 readings" was unreadable.
const WIDGETS_TITLE_SCALE: usize = type_scale::BODY;

/// The heading's summary line: what the dashboard knows, as dense metadata.
///
/// WHY the rung below the heading and not its own size: this line is a count of
/// what is present and what is not, which is metadata, and `type_scale::CAPTION` is
/// the rung the theme reserves for exactly that.
const WIDGETS_SUMMARY_SCALE: usize = type_scale::CAPTION;

/// Gap between the heading's baseline block and its summary block.
const WIDGETS_HEADING_GAP: usize = space::XS;

/// Width and height of the home indicator drawn at the foot of the surface.
///
/// WHY they are named: the painter drew this pill from literals inline while the
/// layout decided where the reading rows may end from a different number, so the
/// last row and the pill were two separate guesses about the same edge.
const INDICATOR_W: usize = space::XL * 4;
const INDICATOR_H: usize = space::XS;

pub fn get_live_widgets_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let theme = THEME;
    let f = frame_for(x, y, state);

    // Status bar: the clock and only the clock.
    if y < STATUS_BAR_HEIGHT {
        if state.clock_source_ok {
            let area = Rect::new(CONTENT_PADDING, 0, SCREEN_WIDTH - CONTENT_PADDING * 2, STATUS_BAR_HEIGHT);
            if let Some(c) = run_left(
                x,
                y,
                &area,
                type_scale::CAPTION,
                &f.status_time[..f.status_time_len],
                layout::ink_on(&theme, theme.overlay),
            ) {
                return c;
            }
        }
        return theme.overlay;
    }
    if y == STATUS_BAR_HEIGHT {
        return theme.overlay_border;
    }

    // 5 Entry points.
    if (135..=685).contains(&y) {
        for entry in &f.entries {
            if entry.rect.contains(x, y) {
                if !round_contains(&entry.rect, radius::CARD, x, y) {
                    return theme.surface;
                }
                if !round_contains(&entry.inner, radius::CARD.saturating_sub(1), x, y) {
                    return entry.border;
                }
                if y >= entry.name_rect.y && y < entry.name_rect.y + entry.name_rect.h {
                    if let Some(c) = run_center(
                        x,
                        y,
                        &entry.name_rect,
                        type_scale::BODY,
                        &entry.name_buf[..entry.name_len],
                        entry.name_ink,
                    ) {
                        return c;
                    }
                    if let Some(ref chevron) = entry.chevron {
                        if let Some(c) = run_right(x, y, chevron, type_scale::BODY, b">", theme.primary) {
                            return c;
                        }
                    }
                }
                if let Some(ref caption) = entry.caption {
                    if y >= caption.y && y < caption.y + caption.h {
                        if let Some(c) = run_center(
                            x,
                            y,
                            caption,
                            type_scale::CAPTION,
                            b"Not installed",
                            theme.on_surface_variant,
                        ) {
                            return c;
                        }
                    }
                }
                return entry.fill;
            }
        }
    }

    // Title and summary
    if y >= HEADER_TOP && y < ENTRIES[0].rect.y {
        if let Some(c) = run_left(x, y, &f.title_rect, WIDGETS_TITLE_SCALE, b"Live Widgets", theme.on_surface) {
            return c;
        }
        if f.summary_rect.contains(x, y) {
            if let Some(c) = run_right(
                x,
                y,
                &f.summary_rect,
                WIDGETS_SUMMARY_SCALE,
                &f.summary_buf[..f.summary_len],
                theme.on_surface_variant,
            ) {
                return c;
            }
        }
    }

    // The readings rows
    if f.bounds.contains(x, y) {
        let offset_y = y.saturating_sub(f.stack_y);
        let index = offset_y / f.row_stride;
        if index < f.rows.len() {
            let row = &f.rows[index];
            if row.rect.contains(x, y) {
                if !round_contains(&row.rect, radius::CARD, x, y) {
                    return theme.surface;
                }
                if !round_contains(&row.inner, radius::CARD.saturating_sub(1), x, y) {
                    return row.border;
                }
                if y >= row.text_y0 && y < row.text_y0 + row.text_h {
                    if x >= row.value_col.x {
                        if let Some(c) = run_right(
                            x,
                            y,
                            &row.value_col,
                            ROW_TEXT_SCALE,
                            &row.value_buf[..row.value_len],
                            row.value_ink,
                        ) {
                            return c;
                        }
                    } else if x < row.label_col.x + row.label_col.w {
                        if let Some(c) = run_left(
                            x,
                            y,
                            &row.label_col,
                            ROW_TEXT_SCALE,
                            &row.label_buf[..row.label_len],
                            row.label_ink,
                        ) {
                            return c;
                        }
                    }
                }
                return row.fill;
            }
        }
    }

    // Home indicator, centred.
    let indicator = Rect::new(
        (SCREEN_WIDTH - INDICATOR_W) / 2,
        SCREEN_HEIGHT - space::MD - INDICATOR_H,
        INDICATOR_W,
        INDICATOR_H,
    );
    if round_contains(&indicator, INDICATOR_H / 2, x, y) {
        return Theme::blend(theme.on_surface_variant, theme.surface, 140);
    }

    theme.surface
}