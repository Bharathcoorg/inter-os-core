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
//! WHY the drawing primitives come from `system_space`: they are `pub(super)`
//! precisely so the sibling surfaces that share this visual language share one
//! implementation of "rounded rectangle" and "centre this run of text". See the
//! note at the top of that file about when they move to a `screens/primitives.rs`.

use super::system_space::{
    cell_pixel, counted, decimal, push, round_contains, run_center, run_left, run_right, Cell, Reading,
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
    Entry { rect: Rect::new(40, 135, 305, 180), package: "org.interos.sensor_dashboard" },
    Entry { rect: Rect::new(375, 135, 305, 180), package: "org.interos.calculator" },
    Entry { rect: Rect::new(40, 335, 640, 150), package: "org.interos.music" },
    Entry { rect: Rect::new(40, 505, 305, 180), package: "org.interos.calendar" },
    Entry { rect: Rect::new(375, 505, 305, 180), package: "org.interos.health" },
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
/// [`ROW_MIN`]: 24 * 2 = 48 is the largest cell that fits it. At
/// `type_scale::BODY` (3) the cell is 72 px, which is taller than most rows the
/// list computes here, so `run_left` pins the run to the row's top edge and drops
/// the descenders into the gutter below.
const ROW_TEXT_SCALE: usize = 2;
/// Row gutter, on the spacing scale.
const ROW_GUTTER: usize = space::SM;
/// Row height bounds. WHY the list does not simply use one token for its rows: the
/// list is as tall as the content area allows, so a fixed row height leaves a dead
/// band above the home indicator that grows every time a reading is added. The
/// ceiling stops a short list becoming slabs; the floor is one whole cell at
/// [`ROW_TEXT_SCALE`], so no row can ever be shorter than the type inside it.
///
/// WHY the parentheses in the expression that uses this: written as a method chain
/// ending in `.clamp(..)`, `a / ROWS.len().clamp(..)` divides by the clamped count
/// instead of clamping the quotient, because a method call binds tighter than `/`.
/// That is not a typo risk, it is what the surface rendered: ten reading rows ten
/// pixels tall, eighteen pixels apart, with no room for a line of type and no text
/// in any of them.
const ROW_MIN: usize = text::GLYPH_H * ROW_TEXT_SCALE;
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
///
/// WHY look it up instead of naming it: the tile says what opens, and what opens
/// is a package in the live registry whose label and presence both change with
/// what is installed. A name written here would survive the package being
/// uninstalled, and the tile would go on offering to open it.
fn installed_label<'a>(state: &'a LauncherState, package: &str) -> Option<&'a [u8]> {
    for i in 0..state.app_count {
        let id_len = state.app_id_lens[i] as usize;
        if id_len == 0 || &state.app_ids[i][..id_len] != package.as_bytes() {
            continue;
        }
        let label_len = state.app_label_lens[i] as usize;
        if label_len > 0 {
            return Some(&state.app_labels[i][..label_len]);
        }
    }
    None
}

/// Paints one entry point: the app's registry name, centred, with a chevron that
/// is only drawn when the package is actually installed.
///
/// WHY an installed tile is tinted and an absent one is recessed: the chevron and
/// the accent wash are what say "this one opens something", and `main.rs` can only
/// open a package the registry knows. A tile that keeps its chevron after the
/// package is uninstalled is a control that presses and does nothing.
fn entry_pixel(
    x: usize,
    y: usize,
    theme: &Theme,
    entry: &Entry,
    state: &LauncherState,
) -> Option<u32> {
    let rect = entry.rect;
    if !round_contains(&rect, radius::CARD, x, y) {
        return None;
    }
    let inner = rect.inset(1);
    if !round_contains(&inner, radius::CARD.saturating_sub(1), x, y) {
        return Some(theme.border);
    }

    let installed = installed_label(state, entry.package);
    let pad = rect.inset(space::MD);
    let name = match installed {
        Some(label) => label,
        // WHY the id rather than a blank: the user should be able to see which
        // package is missing, and an empty tile tells them nothing.
        None => entry.package.as_bytes(),
    };

    let caption_h = if installed.is_none() { text::line_height(type_scale::CAPTION) } else { 0 };
    let name_rect = Rect::new(pad.x, pad.y, pad.w, pad.h.saturating_sub(caption_h + space::SM));

    if installed.is_some() {
        if let Some(c) = run_center(
            x,
            y,
            &name_rect,
            type_scale::BODY,
            name,
            theme.text,
        ) {
            return Some(c);
        }
        // The chevron every settings and launcher list uses to say "this opens
        // something", in the accent the tinted card is washed with.
        let chevron = Rect::new(
            pad.x + pad.w.saturating_sub(text::measure(b">", type_scale::BODY)),
            name_rect.y,
            text::measure(b">", type_scale::BODY),
            name_rect.h,
        );
        if let Some(c) = run_right(x, y, &chevron, type_scale::BODY, b">", theme.accent) {
            return Some(c);
        }
        return Some(Theme::blend(theme.surface, theme.accent, 22));
    }

    if let Some(c) = run_center(x, y, &name_rect, type_scale::BODY, name, theme.text_muted) {
        return Some(c);
    }
    let caption = Rect::new(pad.x, name_rect.y + name_rect.h + space::SM, pad.w, caption_h);
    if let Some(c) = run_center(
        x,
        y,
        &caption,
        type_scale::CAPTION,
        b"Not installed",
        theme.text_muted,
    ) {
        return Some(c);
    }
    Some(theme.surface_sunken)
}

// -----------------------------------------------------------------------------
// The surface
// -----------------------------------------------------------------------------

/// Top of the heading band.
///
/// WHY one step below the status bar and not two: the entry rectangles below are
/// owned by the input layer and cannot move, so the heading has to fit in whatever
/// is left above them. See [`WIDGETS_TITLE_SCALE`].
const HEADER_TOP: usize = STATUS_BAR_HEIGHT + space::SM;

/// The heading's scale.
///
/// WHY derived from the space above the first entry rectangle rather than read from
/// `theme::type_scale`: that band is `ENTRIES[0].rect.y - HEADER_TOP` = 135 - 56 =
/// 79 px, and a run occupies its whole `GLYPH_H * scale` cell, so 24 * s <= 79
/// gives s = 3. `type_scale::TITLE` (5) asks for a 120 px cell: the heading is drawn
/// in full, and then the first entry card paints over its lower half, which is what
/// the device screendump showed.
const WIDGETS_TITLE_SCALE: usize = 3;

pub fn get_live_widgets_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let theme = THEME;

    // Status bar: the clock and only the clock. WHY no signal, wifi or battery
    // glyphs: `LauncherState` carries no reading for any of them, and the kernel's
    // wireless manager starts with `hardware_present: false`.
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
        return theme.overlay_border;
    }

    let content = layout::content_area(CONTENT_PADDING);

    // Entry points. WHY they are tested before everything else: they are controls,
    // and a reading row drawn over one of them would sit on top of something the
    // user can press.
    for entry in ENTRIES.iter() {
        if let Some(c) = entry_pixel(x, y, &theme, entry, state) {
            return c;
        }
    }

    // Title, and the count of readings behind it. WHY the count is built only once
    // the pixel is on that line: it walks every row, and this function runs for
    // every pixel of the surface.
    //
    // WHY the two lines share a baseline instead of being stacked: the band between
    // the status bar and the first entry rectangle is 79 px, which fits one line of
    // this face at heading size and nothing else. A caption stacked under the
    // heading needs another 48 px of cell plus a gap, which does not fit before the
    // first control, so the count is right-aligned on the heading's baseline.
    //
    // WHY both are guarded against `ENTRIES[0].rect.y`: those rectangles belong to
    // the input layer and cannot move, so a heading that outgrows its band is
    // dropped rather than drawn where the user can press something.
    let title_rect = Rect::new(
        content.x,
        HEADER_TOP,
        content.w,
        text::line_height(WIDGETS_TITLE_SCALE),
    );
    let heading_baseline = HEADER_TOP + text::BASELINE * WIDGETS_TITLE_SCALE;
    let summary_rect = Rect::new(
        content.x,
        heading_baseline - text::BASELINE * ROW_TEXT_SCALE,
        content.w,
        text::line_height(ROW_TEXT_SCALE),
    );
    if title_rect.y + title_rect.h <= ENTRIES[0].rect.y {
        if let Some(c) =
            run_left(x, y, &title_rect, WIDGETS_TITLE_SCALE, b"Live Widgets", theme.text)
        {
            return c;
        }
    }
    if summary_rect.y + summary_rect.h <= ENTRIES[0].rect.y && summary_rect.contains(x, y) {
        let (summary, summary_len) = reading_summary(state);
        if let Some(c) = run_right(
            x,
            y,
            &summary_rect,
            ROW_TEXT_SCALE,
            &summary[..summary_len],
            theme.text_muted,
        ) {
            return c;
        }
    }

    // The readings, as a list laid out from the number of rows rather than from a
    // set of boxes. WHY the bottom is the content area and not the surface: the
    // nav bar and the home indicator live below it, and a row that ran under them
    // would be a row the user could see but not read.
    let bottom = content.y + content.h - space::MD;
    let bounds = Rect::new(content.x, LIST_TOP, content.w, bottom.saturating_sub(LIST_TOP));
    let row_h = (bounds.h.saturating_sub(ROW_GUTTER * (ROWS.len() - 1)) / ROWS.len())
        .clamp(ROW_MIN, ROW_MAX);
    if let Some(stack) = layout::list(bounds, ROWS.len(), row_h, ROW_GUTTER) {
        for (index, row) in ROWS.iter().enumerate() {
            let Some(rect) = layout::list_row(stack, index, ROWS.len(), row_h, ROW_GUTTER) else {
                break;
            };
            if !rect.contains(x, y) {
                continue;
            }
            let value = row.value(state);
            let cell = Cell::scaled(rect, radius::CARD, row.label(), &value, ROW_TEXT_SCALE, &theme);
            return cell_pixel(x, y, &theme, &cell);
        }
    }

    // Home indicator, centred.
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