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

use super::system_space::{cell_pixel, counted, round_contains, run_center, run_left, Cell, Reading};
use crate::LauncherState;
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

/// The shade panel.
///
/// Paints the shade panel: a rounded sheet covering the area under the header,
/// with either the list of what the shell knows or the empty state.
///
/// WHY the panel fills that area instead of shrinking to its rows: a shade is a
/// sheet that covers the screen, and a card floating in the middle of an empty
/// page reads as a broken list. The rows inside it still come from the state, so
/// a shade with nothing to list is a large area that says so.
fn panel_pixel(
    x: usize,
    y: usize,
    theme: &Theme,
    bounds: &Rect,
    rows: usize,
    state: &LauncherState,
) -> u32 {
    if !round_contains(bounds, radius::SHEET, x, y) {
        return theme.bg;
    }
    let inner = bounds.inset(1);
    if !round_contains(&inner, radius::SHEET - 1, x, y) {
        return theme.border;
    }

    let pad = bounds.inset(space::XL);
    if rows == 0 {
        // WHY the empty state is two centred lines and not a greyed-out Clear
        // all button: there is nothing to clear, and a disabled control only
        // raises the question the user cannot act on.
        let title_rect = Rect::new(pad.x, pad.y, pad.w, text::line_height(type_scale::BODY));
        if let Some(c) =
            run_center(x, y, &title_rect, type_scale::BODY, b"No notifications", theme.text)
        {
            return c;
        }
        let body_rect = Rect::new(
            pad.x,
            title_rect.y + title_rect.h + space::SM,
            pad.w,
            text::line_height(type_scale::CAPTION),
        );
        if let Some(c) = run_center(
            x,
            y,
            &body_rect,
            type_scale::CAPTION,
            b"Nothing is active for this account",
            theme.text_muted,
        ) {
            return c;
        }
        return theme.surface;
    }

    // WHY the row index is only computed inside the padded area: the stack starts
    // at the panel's inner padding, so a pixel in the padding is above the first
    // row and subtracting would underflow.
    if pad.contains(x, y) {
        let Some(stack) = layout::list(pad, rows, ROW_H, ROW_GUTTER) else {
            return theme.surface;
        };
        let index = (y - stack.y) / ROW_PITCH;
        if let Some(cell_rect) = layout::list_row(stack, index, rows, ROW_H, ROW_GUTTER) {
            if cell_rect.contains(x, y) {
                let (label, value) = row(state, index);
                let cell = Cell::reading(cell_rect, radius::CARD, label, &value, theme);
                return cell_pixel(x, y, theme, &cell);
            }
        }
    }
    theme.surface
}

pub fn get_notifications_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let theme = THEME;

    // Status bar.
    let bar = Rect::new(0, 0, SCREEN_WIDTH, STATUS_BAR_HEIGHT);
    if y < STATUS_BAR_HEIGHT {
        if state.clock_source_ok {
            let area = Rect::new(bar.x + CONTENT_PADDING, bar.y, bar.w - CONTENT_PADDING * 2, bar.h);
            if let Some(c) = run_left(
                x,
                y,
                &area,
                type_scale::CAPTION,
                &state.clock_time,
                layout::ink_on(&theme, theme.overlay),
            ) {
                return c;
            }
        }
        return theme.overlay;
    }
    if y == STATUS_BAR_HEIGHT {
        return theme.border;
    }

    let content = layout::content_area(space::LG);
    let title_rect = Rect::new(content.x, content.y, content.w, text::line_height(type_scale::TITLE));
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
    if round_contains(&bounds, radius::SHEET, x, y) {
        return panel_pixel(x, y, &theme, &bounds, rows, state);
    }

    if let Some(c) = run_left(
        x,
        y,
        &title_rect,
        type_scale::TITLE,
        b"Notifications",
        theme.text,
    ) {
        return c;
    }
    // WHY the headline is the count and nothing else: the notification service
    // publishes a count, so any adjective beside it would be invented. It is
    // built only once the pixel is on that line, because this function runs for
    // every pixel of the surface.
    if y >= sub_rect.y && y < sub_rect.y + sub_rect.h {
        let (headline, headline_len) = counted(state.notification_count, b" active");
        if let Some(c) = run_left(
            x,
            y,
            &sub_rect,
            type_scale::CAPTION,
            &headline[..headline_len],
            theme.text_muted,
        ) {
            return c;
        }
    }

    // Home indicator.
    let indicator_w = space::XL * 4;
    let indicator_h = space::XS;
    let indicator = Rect::new(
        (SCREEN_WIDTH - indicator_w) / 2,
        SCREEN_HEIGHT - space::MD - indicator_h,
        indicator_w,
        indicator_h,
    );
    if round_contains(&indicator, indicator_h / 2, x, y) {
        return Theme::blend(theme.text_muted, theme.bg, 140);
    }

    theme.bg
}