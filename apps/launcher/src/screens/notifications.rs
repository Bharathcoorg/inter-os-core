//! Notifications shade.
//!
//! WHY `pub`: `render_view` in the crate root dispatches here by view mode.
//! Geometry and colour come from `launcher::theme`, `launcher::layout` and
//! `launcher::text`, never from literals local to this file.
//!
//! WHY there are no Today and Yesterday headings: the reference design groups
//! entries under those two headings, and an entry can only be placed in either
//! of them if it carries a timestamp. The kernel's notification manager stores
//! `timestamp_ticks`, package, title and body for every record -- and its
//! `NotificationQuery` syscall returns `active.len()`, a number, and nothing
//! else. The shell therefore holds a count and not one entry, so it cannot place
//! an entry in any group. Drawing the two headings anyway would put two empty
//! sections on a device that may have six notifications waiting, which reads as
//! "nothing arrived today" when the truth is "nothing is published to me". So
//! the shade reports what it was given and names what is missing.
//!
//! WHY there is no Clear all control: clearing needs an id to cancel, the shell
//! has no ids, and the handler in `main.rs` resets its local counter and
//! navigates away. A button that empties a number on this screen while the
//! notifications stay on the device is a control that reports work it did not
//! do.

#![allow(unused_imports)]

use super::system_space::{counted, round_contains, Reading};
use crate::{test_text, LauncherState};
use launcher::layout::{self, Rect};
use launcher::text;
use launcher::theme::*;

/// WHY `Theme::LIGHT`: the reference design splits its palette by context, and
/// the notifications shade is one of the light surfaces -- a near-white page,
/// white cards, dark ink -- rather than the near-black the Power drawer uses.
const THEME: Theme = Theme::LIGHT;

/// Row geometry, in spacing tokens: a label with a caption beside it.
const ROW_H: usize = space::XXL + space::XS;
const ROW_GUTTER: usize = space::SM;
const ROW_PITCH: usize = ROW_H + ROW_GUTTER;

/// Row indices for the rows that exist when there is something to describe.
const R_ENTRIES: usize = 0;
const R_GROUPS: usize = 1;
const R_CLEAR: usize = 2;
const GROUP_ROWS: usize = R_CLEAR + 1;

/// How many day groups the shade can honestly show.
///
/// WHY this is a function of the state and not two hardcoded headings: a group
/// is a set of entries that share a day, and an entry belongs to a day only if
/// it carries a time. `LauncherState` publishes `notification_count` and no
/// per-entry record, so there is nothing to group and the answer is zero. Two
/// changes would make it non-zero -- a syscall that returns the active records
/// rather than their length, and fields on `LauncherState` to hold them -- and
/// neither is in this screen's gift, so the shade says so instead of inventing
/// entries to fill the headings.
fn day_groups(_state: &LauncherState) -> usize {
    0
}

/// How many rows the shade has. An empty shade has none: it draws the empty
/// state, which is a different thing to show and not a list of zeroes.
fn row_count(state: &LauncherState) -> usize {
    if state.notification_count == 0 {
        return 0;
    }
    GROUP_ROWS
}

/// The label and the value of row `index`.
fn row(state: &LauncherState, index: usize) -> (&[u8], Reading) {
    match index {
        R_ENTRIES => (
            b"Entries",
            // WHY unavailable rather than a dash: the service answers with a
            // count, so there is no entry list to show, and saying which half of
            // the pipe is missing is more useful than a mark that could mean
            // "hidden" or "none".
            Reading::unavailable(b"not published"),
        ),
        R_GROUPS => {
            let groups = day_groups(state);
            let value = if groups == 0 {
                // WHY unavailable and not "0 groups": a zero there would read as
                // "nothing arrived today", which is a claim about the world. The
                // truth is that the shell cannot see a single entry, so there is
                // no today to be empty.
                Reading::unavailable(b"no entries")
            } else {
                let (shown, shown_len) = counted(groups, b" shown");
                Reading::of(&shown[..shown_len])
            };
            (b"Day groups", value)
        }
        R_CLEAR => (
            b"Clear all",
            // WHY this row exists at all: the reference has a Clear all control
            // and this shell has the tap handler for one, so a user who knows the
            // reference will look for it. Naming it and saying why it cannot work
            // is more honest than leaving a gap they will read as a missing
            // button -- and the handler behind it only zeroes a local counter
            // while the notifications stay on the device.
            Reading::unavailable(b"no entry ids"),
        ),
        _ => (b"", Reading::unavailable(b"")),
    }
}

// -----------------------------------------------------------------------------
// Runs and the per-frame frame
// -----------------------------------------------------------------------------

/// Capacity of one run's byte buffer. WHY it matches `text::ellipsize`, so a run
/// built once per frame is byte-for-byte the run the per-pixel painter built.
const RUN_CAP: usize = 96;

/// The scale every row's label and value are drawn at.
///
/// WHY it is derived from [`ROW_H`] and not read from `theme::type_scale`: a row has
/// to hold one whole glyph cell, and this face's cell is `GLYPH_H` rows per scale
/// step, so the only scale that fits a 52 px row is 2. `system_space.rs` derives its
/// own copy of this the same way and keeps it private, so it is derived again here
/// rather than typed in as a number that could drift from the row it has to fit.
const ROW_TEXT_SCALE: usize = ROW_H / text::GLYPH_H;

/// The ceiling on the value column, as a share of the padded row width.
///
/// WHY it is repeated: `system_space.rs` keeps the same ceiling for the same reason
/// -- a reason string long enough to fill the row would push the label off it. This
/// screen has to reproduce that column arithmetic because it cannot call the shared
/// cell painter for the reason below.
const VALUE_COLUMN_MAX_PCT: usize = 50;

/// One label, placed and measured once per frame.
///
/// WHY this type: the shade's header measured "Notifications" for every pixel of its
/// title band, formatted the notification count into a fresh 48-byte buffer for
/// every pixel of the line under it, and `run_left`/`run_center` measured their run
/// again on every call. Everything that depends on the string rather than the pixel
/// -- the ellipsis decision, the centring inset, the advance width -- is settled
/// here, once per frame.
///
/// WHY the box is carried as well as the origin: `test_text` walks the pen of every
/// glyph before it can answer "no", so a pixel nowhere near a run still costs a pen
/// walk per label. The stored box is exactly
/// `y .. y + line_height(scale)` by `x .. x + measure(run)`, which is the region
/// `test_text` can return `Some` for.
struct Run {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    scale: usize,
    len: usize,
    ink: u32,
    bytes: [u8; RUN_CAP],
}

impl Run {
    /// A run that never inks anything.
    const EMPTY: Run = Run {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        scale: 1,
        len: 0,
        ink: 0,
        bytes: [0u8; RUN_CAP],
    };

    /// `bytes` at (`x`, `y`), drawn whole with no truncation step.
    fn whole(x: usize, y: usize, scale: usize, bytes: &[u8], ink: u32) -> Run {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        Run::placed(x, y, scale, buf, len, ink)
    }

    /// As [`Run::whole`], but nothing is drawn if the run is wider than `centre`.
    ///
    /// WHY the guard: this is what `system_space::run_center` does, and the shade's
    /// two empty-state lines reach it without ever being ellipsized. Without the
    /// guard a line wider than its box would be drawn flush left instead of not
    /// drawn at all, which is a different screen.
    fn centred_whole(
        x: usize,
        y: usize,
        scale: usize,
        centre: usize,
        bytes: &[u8],
        ink: u32,
    ) -> Run {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        if text::measure(&buf[..len], scale) > centre {
            return Run::EMPTY;
        }
        let inset = text::center_x(&buf[..len], scale, centre);
        Run::placed(x + inset, y, scale, buf, len, ink)
    }

    /// `bytes` at (`x`, `y`), truncated with an ellipsis if wider than `fit` --
    /// what `system_space::cell_pixel` did to both of a row's columns, per pixel.
    fn fitted(x: usize, y: usize, scale: usize, fit: usize, bytes: &[u8], ink: u32) -> Run {
        let (buf, len) = text::ellipsize(bytes, scale, fit);
        Run::placed(x, y, scale, buf, len, ink)
    }

    /// A run from an already-resolved buffer.
    fn placed(x: usize, y: usize, scale: usize, buf: [u8; RUN_CAP], len: usize, ink: u32) -> Run {
        Run {
            x,
            y,
            w: text::measure(&buf[..len], scale),
            h: text::line_height(scale),
            scale,
            len,
            ink,
            bytes: buf,
        }
    }

    /// The colour of (`px`, `py`) if this run inks it.
    fn pixel(&self, px: usize, py: usize) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if px < self.x || px - self.x >= self.w || py < self.y || py - self.y >= self.h {
            return None;
        }
        test_text(px, py, self.x, self.y, self.scale, &self.bytes[..self.len], self.ink)
    }
}

/// One row of the shade's list, with both columns and the column split resolved.
///
/// WHY the split is a number rather than two rectangles: the shared cell painter
/// decides which column owns a pixel by comparing `x` against the value column's
/// left edge and the label column's right edge, and those two edges are one gap
/// apart. Storing both edges keeps that decision as two comparisons.
struct CellPlan {
    rect: Rect,
    inner: Rect,
    corner: usize,
    fill: u32,
    /// The value column's left edge: at or after this the value owns the pixel.
    split_x: usize,
    /// The label column's right edge: before this the label owns the pixel.
    label_end: usize,
    label: Run,
    value: Run,
}

impl CellPlan {
    const EMPTY: CellPlan = CellPlan {
        rect: Rect::new(0, 0, 0, 0),
        inner: Rect::new(0, 0, 0, 0),
        corner: 0,
        fill: 0,
        split_x: 0,
        label_end: 0,
        label: Run::EMPTY,
        value: Run::EMPTY,
    };
}

/// The shade panel, resolved.
struct PanelPlan {
    bounds: Rect,
    inner: Rect,
    pad: Rect,
    rows: usize,
    stack: Option<Rect>,
    empty_head: Run,
    empty_body: Run,
    cells: [CellPlan; GROUP_ROWS],
}

/// Everything the shade needs that does not depend on the pixel.
struct Frame {
    clock: Option<Run>,
    title: Run,
    headline: Run,
    panel: PanelPlan,
    indicator: Rect,
}

/// Resolves one row of the list into its two columns.
///
/// WHY this repeats the shared cell painter's arithmetic instead of calling it: the
/// shared painter measures its value, ellipsizes both columns and re-centres
/// whichever one the pixel lands in, on every pixel of the row, and
/// `system_space.rs` is not this change's file to edit. The arithmetic below is that
/// painter's, in its order, with the two columns resolved in advance.
fn cell_plan(rect: Rect, label: &[u8], value: &Reading, theme: &Theme) -> CellPlan {
    let pad = rect.inset(space::MD);
    let (fill, value_ink) = value.skin(theme);
    let value_w = text::measure(value.as_slice(), ROW_TEXT_SCALE)
        .min(pad.w * VALUE_COLUMN_MAX_PCT / 100)
        + space::SM;
    let value_col = Rect::new(pad.x + pad.w - value_w, pad.y, value_w, pad.h);
    let label_col = Rect::new(pad.x, pad.y, pad.w - value_w - space::SM, pad.h);
    // Both columns are the padded row's full height, so both share one baseline.
    let row_y = pad.y + pad.h.saturating_sub(text::line_height(ROW_TEXT_SCALE)) / 2;
    let value_bytes = value.as_slice();
    let (value_run, value_len) = text::ellipsize(value_bytes, ROW_TEXT_SCALE, value_col.w);
    let value_w_measured = text::measure(&value_run[..value_len], ROW_TEXT_SCALE);
    CellPlan {
        rect,
        inner: rect.inset(1),
        corner: radius::CARD,
        fill,
        split_x: value_col.x,
        label_end: label_col.x + label_col.w,
        label: Run::fitted(
            label_col.x,
            row_y,
            ROW_TEXT_SCALE,
            label_col.w,
            label,
            theme.text,
        ),
        // WHY the value's origin is derived from its own measured width: the shared
        // painter right-aligns it by building a rectangle that ends at the column's
        // right edge and starting its run at that rectangle's left edge.
        value: if value_w_measured > value_col.w {
            Run::EMPTY
        } else {
            Run::placed(
                value_col.x + value_col.w - value_w_measured,
                row_y,
                ROW_TEXT_SCALE,
                value_run,
                value_len,
                value_ink,
            )
        },
    }
}

/// Builds this frame's plan.
fn build(state: &LauncherState) -> Frame {
    let theme = THEME;

    let bar = Rect::new(0, 0, SCREEN_WIDTH, STATUS_BAR_HEIGHT);
    let content = layout::content_area(space::LG);
    let title_rect = Rect::new(
        content.x,
        content.y,
        content.w,
        text::line_height(type_scale::TITLE),
    );
    let sub_rect = Rect::new(
        content.x,
        title_rect.y + title_rect.h + space::XS,
        content.w,
        text::line_height(type_scale::CAPTION),
    );
    let panel_top = sub_rect.y + sub_rect.h + space::MD;

    let rows = row_count(state);
    let bounds = Rect::new(
        content.x,
        panel_top,
        content.w,
        (content.y + content.h) - panel_top,
    );
    let pad = bounds.inset(space::XL);
    let stack = layout::list(pad, rows, ROW_H, ROW_GUTTER);

    // WHY the headline is formatted here: it is a count, so its text is a function of
    // the state, and the old painter rebuilt it out of a 48-byte buffer for every
    // pixel of the line under the title.
    let (headline, headline_len) = counted(state.notification_count, b" active");

    let mut cells = [CellPlan::EMPTY; GROUP_ROWS];
    if let Some(stack) = stack {
        for index in 0..rows.min(GROUP_ROWS) {
            let Some(rect) = layout::list_row(stack, index, rows, ROW_H, ROW_GUTTER) else {
                continue;
            };
            let (label, value) = row(state, index);
            cells[index] = cell_plan(rect, label, &value, &theme);
        }
    }

    // WHY the empty state's two lines are resolved even when they cannot be shown:
    // they are two tests of `rows == 0` either way, and keeping both branches in one
    // place is what stops the two states drifting apart.
    let empty_title = Rect::new(pad.x, pad.y, pad.w, text::line_height(type_scale::BODY));
    let empty_body = Rect::new(
        pad.x,
        empty_title.y + empty_title.h + space::SM,
        pad.w,
        text::line_height(type_scale::CAPTION),
    );

    let indicator_w = space::XL * 4;
    let indicator_h = space::XS;
    Frame {
        clock: if state.clock_source_ok {
            Some(Run::whole(
                bar.x + CONTENT_PADDING,
                bar.y + (bar.h - text::line_height(type_scale::CAPTION)) / 2,
                type_scale::CAPTION,
                &state.clock_time,
                layout::ink_on(&theme, theme.overlay),
            ))
        } else {
            None
        },
        title: Run::whole(
            title_rect.x,
            title_rect.y + title_rect.h.saturating_sub(text::line_height(type_scale::TITLE)) / 2,
            type_scale::TITLE,
            b"Notifications",
            theme.text,
        ),
        headline: Run::whole(
            sub_rect.x,
            sub_rect.y + sub_rect.h.saturating_sub(text::line_height(type_scale::CAPTION)) / 2,
            type_scale::CAPTION,
            &headline[..headline_len],
            theme.text_muted,
        ),
        panel: PanelPlan {
            bounds,
            inner: bounds.inset(1),
            pad,
            rows,
            stack,
            empty_head: Run::centred_whole(
                empty_title.x,
                empty_title.y,
                type_scale::BODY,
                empty_title.w,
                b"No notifications",
                theme.text,
            ),
            empty_body: Run::centred_whole(
                empty_body.x,
                empty_body.y,
                type_scale::CAPTION,
                empty_body.w,
                b"Nothing is active for this account",
                theme.text_muted,
            ),
            cells,
        },
        indicator: Rect::new(
            (SCREEN_WIDTH - indicator_w) / 2,
            SCREEN_HEIGHT - space::MD - indicator_h,
            indicator_w,
            indicator_h,
        ),
    }
}

/// This frame's plan.
///
/// WHY `(0, 0)` is the frame boundary: `render_view` walks the surface in raster
/// order from the top-left pixel, so `(0, 0)` is the first pixel of every frame and
/// the only pixel within a frame that repeats it. `LauncherState` is borrowed
/// immutably for the whole of a frame, so rebuilding there is once per frame and the
/// plan can never lag the state it describes.
static mut FRAME: Option<Frame> = None;

fn frame(x: usize, y: usize, state: &LauncherState) -> &'static Frame {
    let slot: *mut Option<Frame> = core::ptr::addr_of_mut!(FRAME);
    // SAFETY: the launcher is one bare-metal task and the paint loop is neither
    // preemptible nor reentrant, so this is the only reference to the slot at any
    // moment, and the reference returned cannot outlive the call that made it.
    let empty = unsafe { (*slot).is_none() };
    if empty || (x == 0 && y == 0) {
        // SAFETY: as above; this is the one mutation of the slot.
        unsafe { *slot = Some(build(state)) };
    }
    // SAFETY: the slot is `Some` here -- it was just filled when it was empty.
    unsafe { (*slot).as_ref() }.expect("WHY: the slot is filled when it is empty")
}
/// Paints the shade panel: a rounded sheet covering the area under the header,
/// with either the list of what the shell knows or the empty state.
///
/// WHY the panel fills that area instead of shrinking to its rows: a shade is a
/// sheet that covers the screen, and a card floating in the middle of an empty
/// page reads as a broken list. The rows inside it still come from the state, so
/// a shade with nothing to list is a large area that says so.
///
/// WHY the row is found by arithmetic rather than by walking the stack: the stack
/// and each row's rectangle were rebuilt for every pixel of the panel, which is
/// most of the surface.
fn panel_pixel(x: usize, y: usize, theme: &Theme, plan: &PanelPlan) -> u32 {
    if !round_contains(&plan.bounds, radius::SHEET, x, y) {
        return theme.bg;
    }
    if !round_contains(&plan.inner, radius::SHEET - 1, x, y) {
        return theme.border;
    }

    if plan.rows == 0 {
        // WHY the empty state is two centred lines and not a greyed-out Clear
        // all button: there is nothing to clear, and a disabled control only
        // raises the question the user cannot act on.
        if let Some(c) = plan.empty_head.pixel(x, y) {
            return c;
        }
        if let Some(c) = plan.empty_body.pixel(x, y) {
            return c;
        }
        return theme.surface;
    }

    // WHY the row index is only computed inside the padded area: the stack starts
    // at the panel's inner padding, so a pixel in the padding is above the first
    // row and subtracting would underflow.
    if plan.pad.contains(x, y) {
        if let Some(stack) = plan.stack {
            let index = (y - stack.y) / ROW_PITCH;
            if index < plan.rows {
                return cell_pixel(&plan.cells[index], x, y, theme);
            }
        }
    }
    theme.surface
}

/// One row: card fill, hairline outline, label left, value right.
///
/// WHY this exists rather than a call to `system_space::cell_pixel`: that painter
/// measures its value, ellipsizes whichever of the two columns the pixel falls in,
/// and rebuilds the column rectangles for every pixel of the row. The rows this
/// shade draws are three, and their geometry is the same arithmetic in the same
/// order -- resolved once per frame in [`cell_plan`], and read here.
fn cell_pixel(plan: &CellPlan, x: usize, y: usize, theme: &Theme) -> u32 {
    if !round_contains(&plan.rect, plan.corner, x, y) {
        return theme.bg;
    }
    if !round_contains(&plan.inner, plan.corner.saturating_sub(1), x, y) {
        return theme.border;
    }

    // WHY the columns are resolved separately: they cannot overlap, so a pixel
    // belongs to at most one and only that one needs testing.
    if x >= plan.split_x {
        if let Some(c) = plan.value.pixel(x, y) {
            return c;
        }
    } else if x < plan.label_end {
        if let Some(c) = plan.label.pixel(x, y) {
            return c;
        }
    }
    plan.fill
}

pub fn get_notifications_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let theme = THEME;

    // WHY the plan is fetched before anything is drawn: it carries every rectangle,
    // every line and every derived ink for this frame, and fetching it is the only
    // place in this function where anything other than a pixel test happens.
    let f = frame(x, y, state);

    // Status bar.
    if y < STATUS_BAR_HEIGHT {
        if let Some(c) = f.clock.as_ref().and_then(|r| r.pixel(x, y)) {
            return c;
        }
        return theme.overlay;
    }
    if y == STATUS_BAR_HEIGHT {
        return theme.border;
    }

    if round_contains(&f.panel.bounds, radius::SHEET, x, y) {
        return panel_pixel(x, y, &theme, &f.panel);
    }

    if let Some(c) = f.title.pixel(x, y) {
        return c;
    }
    // WHY the headline is the count and nothing else: the notification service
    // publishes a count, so any adjective beside it would be invented. It is built
    // once per frame now, because this function runs for every pixel of the surface.
    if let Some(c) = f.headline.pixel(x, y) {
        return c;
    }

    // Home indicator.
    if round_contains(&f.indicator, f.indicator.h / 2, x, y) {
        return Theme::blend(theme.text_muted, theme.bg, 140);
    }

    theme.bg
}
