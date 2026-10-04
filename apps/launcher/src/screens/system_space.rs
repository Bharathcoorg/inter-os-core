//! System Space: device and account settings.
//!
//! WHY `pub`: `render_view` in the crate root dispatches here by view mode.
//! Geometry and colour come from `launcher::theme`, `launcher::layout` and
//! `launcher::text`, never from literals local to this file.
//!
//! WHY this surface is a list of readings instead of seven category tiles: the
//! previous version drew Device, Network, Personal, Apps, Power, Security and
//! System as a fixed two-column grid over a decorative sphere, and the grid did
//! not change when the thing it claimed to describe did. Three of its subtitles
//! described a fuel gauge, a bound radio and a thermal reading that this
//! platform has no sensor for and the shell is never handed. A tile that says
//! "Battery" and a tile that says "Battery: 87%" look the same to the person
//! reading them, and only one of them is true.
//!
//! So every row here comes from state the shell actually holds, and a reading
//! it does not hold renders as unavailable with the reason spelled out. The
//! rows are the clock and the date when the kernel's RTC answered, the
//! notification count, the three readings this platform cannot make, and one row
//! per package in the live registry. Nothing here is a control except the one
//! entry point the shell's touch handler actually knows how to open.

#![allow(unused_imports)]

use crate::screens::lock::LOCK_DATE_TEXT_LEN;
use crate::{test_text, LauncherState};
use launcher::layout::{self, Rect};
use launcher::text;
use launcher::theme::*;

/// WHY `Theme::LIGHT`: the reference design splits its palette by context, and
/// System Space is one of the light surfaces -- a near-white page with white
/// cards and dark ink -- sitting beside a near-black Power drawer. Binding the
/// palette per screen rather than inheriting one global default is what lets
/// the two read as one system.
///
/// WHY a `const` and not a `static`: a theme is a value, so it belongs where a
/// screen can choose it. A mutable global "current theme" would be the same
/// coupling with none of the type checking.
const THEME: Theme = Theme::LIGHT;

/// Row geometry, in spacing tokens: a body label with a caption beside it, the
/// proportions the reference's settings list uses.
const ROW_H: usize = space::XXL + space::XS;
const ROW_GUTTER: usize = space::SM;
const ROW_PITCH: usize = ROW_H + ROW_GUTTER;

/// The scale every row's label and value are drawn at.
///
/// WHY it is derived from `ROW_H` and not read from `theme::type_scale`: a row has
/// to hold one whole glyph cell, and this face's cell is `GLYPH_H` = 24 rows per
/// scale step rather than the 8 the row heights were sized against. `ROW_H` is 52,
/// so 24 * s <= 52 gives s = 2. At `type_scale::BODY` (3) the cell is 72, which is
/// 20 px taller than the row: `run_left` pins the run to the row's top edge and the
/// descenders of any label with a `g` or a `p` land in the gutter below it.
///
/// WHY 2 and not 1: at 1 the cap is 17 px on a 720 px surface, which is a caption,
/// not a list label.
const ROW_TEXT_SCALE: usize = 2;

/// WHY the entry card's label is one word: this card is a third of the width of
/// every other row on the surface, so the label column it leaves is about 240 px,
/// and "Power menu" measured 240 px under the fixed cell the shell used to have --
/// one pixel of slack. The moment glyphs stopped being a fixed 8 px wide the same
/// label ellipsized to "Power Â·Â·Â·" in the screendump, which reads as a truncated
/// control rather than a named one. "Power" plus the chevron says the same thing
/// with room to spare.
const POWER_LABEL: &[u8] = b"Power";

/// WHY the value column has a ceiling at all: a reason string long enough to fill
/// the row would push the label off it, and a value that needs the whole row is
/// a caption rather than a value. Below the ceiling the column is exactly as wide
/// as the value needs, so a one-character value cannot squeeze the label it
/// belongs to -- which is what a fixed split did to the narrow entry card.
const VALUE_COLUMN_MAX_PCT: usize = 50;

/// Row indices. Every row is one of these or a package row that follows them,
/// so a new reading is a case in [`row`] rather than another box of pixels.
const R_CLOCK: usize = 0;
const R_DATE: usize = 1;
const R_NOTIFICATIONS: usize = 2;
const R_BATTERY: usize = 3;
const R_THERMAL: usize = 4;
const R_RADIO: usize = 5;
const R_PACKAGES: usize = 6;
const FIRST_PACKAGE_ROW: usize = R_PACKAGES + 1;
/// Rows that exist whatever the registry holds.
const FIXED_ROWS: usize = R_PACKAGES + 1;
/// How many readings this surface describes -- every row above the package
/// heading. Derived from the indices so a new reading cannot drift out of step
/// with the headline that counts them.
const TOTAL_READINGS: usize = R_PACKAGES;

/// Longest value this screen prints.
pub(super) const VALUE_MAX: usize = 48;

/// The Power menu entry point.
///
/// WHY these four literals are the only geometry on this screen that is typed
/// rather than computed: the shell's touch handler lives in `main.rs`, hit-tests
/// `(40..345, 590..710)` there, and opens the Power menu for a tap inside it.
/// That file is not part of this change, and a drawn control whose rectangle is
/// not the rectangle the input layer tests is a control that lies about where
/// it can be pressed. Every other rectangle here is derived. Exporting this
/// constant next to the handler in the crate root -- the lock screen already
/// exports its button geometry that way -- is the change that would let it be
/// computed too.
const POWER_ENTRY: Rect = Rect::new(40, 590, 305, 120);

// -----------------------------------------------------------------------------
// Shared drawing primitives
// -----------------------------------------------------------------------------
// WHY these live in this file: `screens/mod.rs` owns the screen list and this
// change does not touch it, so there is no shared drawing module to add them
// to. The two sibling screens this change also owns (`power`, `notifications`)
// reach them through `super::system_space`, which keeps one implementation of
// "rounded rectangle" and "centre this run of text" instead of three that drift
// apart between saves. When the shell grows a `screens/primitives.rs`, these
// move there unchanged.

/// Whether `(x, y)` falls inside `rect` with its corners rounded to `radius`.
///
/// WHY a distance test rather than four rectangle tests: a card drawn with
/// square corners is the most obvious tell that a screen was assembled from
/// boxes instead of designed, and every surface in this shell shares one radius
/// scale for exactly that reason.
pub(super) fn round_contains(rect: &Rect, radius: usize, x: usize, y: usize) -> bool {
    if !rect.contains(x, y) {
        return false;
    }
    let r = radius.min(rect.w / 2).min(rect.h / 2);
    if r == 0 {
        return true;
    }
    let left = rect.x + r;
    let right = rect.x + rect.w - r;
    let top = rect.y + r;
    let bottom = rect.y + rect.h - r;
    let cx = if x < left {
        left
    } else if x >= right {
        right
    } else {
        x
    };
    let cy = if y < top {
        top
    } else if y >= bottom {
        bottom
    } else {
        y
    };
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    (dx * dx + dy * dy) as usize <= r * r
}

/// Colours the pixel if it falls on `run`, laid out from the left of `rect` and
/// centred vertically against it.
///
/// WHY vertical centring comes from `text::line_height` rather than a number:
/// the face rasterises at integer scales only, so the same label at another
/// scale is another height, and a layout that measured it once drifts every time
/// the type scale changes.
pub(super) fn run_left(
    x: usize,
    y: usize,
    rect: &Rect,
    scale: usize,
    run: &[u8],
    color: u32,
) -> Option<u32> {
    let h = text::line_height(scale);
    let y0 = rect.y + rect.h.saturating_sub(h) / 2;
    let w = text::measure(run, scale);
    if y < y0 || y >= y0 + h || x < rect.x || x >= rect.x + w {
        return None;
    }
    test_text(x, y, rect.x, y0, scale, run, color)
}

/// As [`run_left`], aligned to the right edge of `rect`.
pub(super) fn run_right(
    x: usize,
    y: usize,
    rect: &Rect,
    scale: usize,
    run: &[u8],
    color: u32,
) -> Option<u32> {
    let w = text::measure(run, scale);
    if w > rect.w {
        return None;
    }
    let shifted = Rect::new(rect.x + rect.w - w, rect.y, w, rect.h);
    run_left(x, y, &shifted, scale, run, color)
}

/// As [`run_left`], horizontally centred in `rect`.
///
/// WHY this is a helper and not `x0 + width - label.len() * 16 / 2` at each call
/// site: that expression hardcodes both the glyph cell width and the scale, so
/// it is wrong the moment the type scale changes and nobody finds out until the
/// label is visibly off-centre. `text::center_x` measures instead.
pub(super) fn run_center(
    x: usize,
    y: usize,
    rect: &Rect,
    scale: usize,
    run: &[u8],
    color: u32,
) -> Option<u32> {
    let w = text::measure(run, scale);
    if w > rect.w {
        return None;
    }
    let placed = Rect::new(rect.x + text::center_x(run, scale, rect.w), rect.y, w, rect.h);
    run_left(x, y, &placed, scale, run, color)
}

/// A settings value: either bytes the shell holds, or the reason it holds none.
///
/// WHY there is no third case: a surface that answers every question with a
/// plausible line of copy is indistinguishable from one that measured them, and
/// that ambiguity is the whole reason this screen is being rewritten.
pub(super) struct Reading {
    bytes: [u8; VALUE_MAX],
    len: usize,
    available: bool,
}

impl Reading {
    /// A reading the shell actually holds.
    pub(super) fn of(run: &[u8]) -> Reading {
        let mut bytes = [0u8; VALUE_MAX];
        let len = run.len().min(VALUE_MAX);
        bytes[..len].copy_from_slice(&run[..len]);
        Reading { bytes, len, available: true }
    }

    /// No reading exists. `reason` says why, in the user's language.
    pub(super) fn unavailable(reason: &[u8]) -> Reading {
        let mut reading = Reading::of(reason);
        reading.available = false;
        reading
    }

    pub(super) fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    /// Fill and value ink for a row carrying this reading.
    ///
    /// WHY an unavailable row is recessed rather than merely grey: on a phone a
    /// dimmed value still reads as a value, and the one thing this screen must
    /// not leave behind is the idea that a missing sensor reported zero.
    pub(super) fn skin(&self, theme: &Theme) -> (u32, u32) {
        if self.available {
            (theme.surface, theme.text)
        } else {
            (theme.surface_sunken, theme.text_muted)
        }
    }
}

/// One drawn cell: a rounded card with a label on the left and a value on the
/// right.
///
/// WHY a struct rather than a long parameter list: three kinds of row share this
/// one painter -- a reading, a reading the shell could not make, and an entry
/// point that opens another surface -- and the fill and ink are what tell them
/// apart. Naming them keeps the call sites readable.
pub(super) struct Cell<'a> {
    pub rect: Rect,
    pub corner: usize,
    pub label: &'a [u8],
    pub value: &'a [u8],
    pub fill: u32,
    pub value_ink: u32,
    /// The scale both runs are drawn at. WHY it lives on the cell: a cell's text
    /// scale is a consequence of the cell's height, so the painter and its caller
    /// cannot be allowed to disagree about it.
    pub scale: usize,
}

impl<'a> Cell<'a> {
    /// A cell whose value came from a [`Reading`], raised or recessed by whether
    /// that reading exists.
    ///
    /// WHY the scale is not a parameter here: it is a consequence of the row's
    /// height, and the rows this constructor is shared with are all
    /// `space::XXL + space::XS` tall, which holds exactly one cell of this face at
    /// [`ROW_TEXT_SCALE`]. A caller whose rows are a different height uses
    /// [`Cell::scaled`].
    pub(super) fn reading(
        rect: Rect,
        corner: usize,
        label: &'a [u8],
        value: &'a Reading,
        theme: &Theme,
    ) -> Cell<'a> {
        Cell::scaled(rect, corner, label, value, ROW_TEXT_SCALE, theme)
    }

    /// [`Cell::reading`] at an explicit text scale.
    pub(super) fn scaled(
        rect: Rect,
        corner: usize,
        label: &'a [u8],
        value: &'a Reading,
        scale: usize,
        theme: &Theme,
    ) -> Cell<'a> {
        let (fill, value_ink) = value.skin(theme);
        Cell { rect, corner, label, value: value.as_slice(), fill, value_ink, scale }
    }

    /// Whether this pixel is on the cell at all.
    pub(super) fn contains(&self, x: usize, y: usize) -> bool {
        round_contains(&self.rect, self.corner, x, y)
    }
}

/// Paints one cell: card fill, hairline outline, label left, value right.
pub(super) fn cell_pixel(x: usize, y: usize, theme: &Theme, cell: &Cell) -> u32 {
    if !cell.contains(x, y) {
        return theme.bg;
    }
    let inner = cell.rect.inset(1);
    if !round_contains(&inner, cell.corner.saturating_sub(1), x, y) {
        return theme.border;
    }

    let pad = cell.rect.inset(space::MD);
    // WHY the column is measured and capped rather than a fixed share: two rows
    // on this surface differ in width by more than a factor of two, and a fixed
    // share that suits a full-width reading row ellipsized the entry card down
    // to "Pow...".
    let value_w =
        text::measure(cell.value, cell.scale).min(pad.w * VALUE_COLUMN_MAX_PCT / 100)
            + space::SM;
    let value_col = Rect::new(pad.x + pad.w - value_w, pad.y, value_w, pad.h);
    let label_col = Rect::new(pad.x, pad.y, pad.w - value_w - space::SM, pad.h);

    // WHY the columns are resolved separately: they cannot overlap, so a pixel
    // belongs to at most one and only that one needs measuring and ellipsizing.
    if x >= value_col.x {
        let (run, len) = text::ellipsize(cell.value, cell.scale, value_col.w);
        if let Some(c) =
            run_right(x, y, &value_col, cell.scale, &run[..len], cell.value_ink)
        {
            return c;
        }
    } else if x < label_col.x + label_col.w {
        let (run, len) = text::ellipsize(cell.label, cell.scale, label_col.w);
        if let Some(c) = run_left(x, y, &label_col, cell.scale, &run[..len], theme.text) {
            return c;
        }
    }
    cell.fill
}

/// Appends `run` to a fixed buffer, returning the new length.
///
/// WHY a hand-rolled builder: the crate is `no_std` with no allocator, and the
/// alternative would be the largest dependency in the shell for the sake of a
/// number and a word.
pub(super) fn push(buf: &mut [u8; VALUE_MAX], at: &mut usize, run: &[u8]) {
    for b in run {
        if *at >= VALUE_MAX {
            return;
        }
        buf[*at] = *b;
        *at += 1;
    }
}

/// Renders `n` in decimal.
///
/// WHY not `format!`: the same reason as [`push`].
pub(super) fn decimal(n: usize) -> ([u8; VALUE_MAX], usize) {
    let mut out = [b' '; VALUE_MAX];
    if n == 0 {
        out[0] = b'0';
        return (out, 1);
    }
    let mut v = n;
    let mut at = 0usize;
    while v > 0 && at < VALUE_MAX {
        out[at] = b'0' + (v % 10) as u8;
        at += 1;
        v /= 10;
    }
    out[..at].reverse();
    (out, at)
}

/// `base` darkened by `percent`, on the same 0..=256 fixed-point scale the
/// wallpaper ramp uses.
///
/// WHY this is written out rather than `theme::veil`: `Theme::veil(base, percent,
/// light)` picks the target's alpha from `light` and then ORs `0xFFFFFF` into the
/// colour, so `light = false` blends toward *white* with a transparent alpha
/// instead of toward black. Every darkening in this change that used it came out
/// as a lightening -- the Power drawer's scrim came out mid-grey and the
/// emergency pill came out pink. `theme.rs` is not part of this change, so the
/// correction is made here.
///
/// WHY the channels are scaled rather than blended toward a named shade: naming
/// the shade means writing the colour out, which is the one thing this function
/// exists to avoid. Scaling toward zero is the same arithmetic, and the alpha is
/// carried across by hand rather than by a mask literal.
pub(super) fn darken(base: u32, percent: u32) -> u32 {
    let keep = 256 - (percent.min(100) * 256) / 100;
    let mut out = ((base >> 24) & 255) << 24;
    for shift in [16u32, 8, 0] {
        let channel = (((base >> shift) & 255) * keep) >> 8;
        out |= channel.min(255) << shift;
    }
    out
}

/// Renders `<n><suffix>`, e.g. `3 registered`.
pub(super) fn counted(n: usize, suffix: &[u8]) -> ([u8; VALUE_MAX], usize) {
    let (mut buf, mut at) = decimal(n);
    push(&mut buf, &mut at, suffix);
    (buf, at)
}

// -----------------------------------------------------------------------------
// What this screen is allowed to say
// -----------------------------------------------------------------------------

/// Package rows the list can show without pushing the stack off the surface.
///
/// WHY `capacity` is passed in rather than read from a constant: the number of
/// rows the surface holds is a property of the geometry, which is computed, and
/// a constant beside it would be a second place to update when the geometry
/// moved. One row is held back for the note that says how many did not fit.
fn shown_packages(state: &LauncherState, capacity: usize) -> usize {
    state.app_count.min(capacity.saturating_sub(FIXED_ROWS + 1))
}

/// How many rows this surface has, derived from the registry and the geometry.
fn row_count(state: &LauncherState, capacity: usize) -> usize {
    if state.app_count == 0 {
        // The truthful-empty convention the rest of the shell uses: say plainly
        // that there is nothing, rather than leaving a blank panel that reads as
        // a list that failed to load.
        return FIXED_ROWS;
    }
    let shown = shown_packages(state, capacity);
    FIXED_ROWS + shown + if state.app_count > shown { 1 } else { 0 }
}

/// The label and the value of row `index`.
///
/// WHY availability is decided where the value is built: the one rule this
/// screen must not break is printing a number where the platform has no sensor
/// to produce one, and the only reliable place to enforce that is next to the
/// bytes that would be printed.
fn row(state: &LauncherState, index: usize, capacity: usize) -> (&[u8], Reading) {
    match index {
        R_CLOCK => {
            let value = if state.clock_source_ok {
                // WHY the shell's own bytes: with no RTC authority granted,
                // `clock_time` holds blanks, and a substituted value here would
                // be a fabrication on the surface the user trusts most.
                Reading::of(&state.clock_time[..5])
            } else {
                Reading::unavailable(b"no RTC reading")
            };
            (b"Clock", value)
        }
        R_DATE => {
            let value = if state.clock_source_ok {
                Reading::of(&state.date_line[..LOCK_DATE_TEXT_LEN])
            } else {
                Reading::unavailable(b"no RTC reading")
            };
            (b"Date", value)
        }
        R_NOTIFICATIONS => {
            // WHY the count and not a summary sentence: the notification service
            // publishes a number and nothing else, so a sentence about what the
            // notifications are would be invented. `0 active` is the same shape
            // as `3 active`, which is what makes the empty case read as an
            // answer rather than as a missing one.
            let (buf, len) = counted(state.notification_count, b" active");
            (b"Notifications", Reading::of(&buf[..len]))
        }
        R_BATTERY => (
            b"Battery",
            // WHY unavailable and not "0%": there is no fuel gauge on this
            // platform, and a zero would read as a flat battery, which is a
            // hardware fault rather than an absence.
            Reading::unavailable(b"no fuel gauge"),
        ),
        R_THERMAL => (b"Thermal", Reading::unavailable(b"no temp sensor")),
        R_RADIO => (b"Radio", Reading::unavailable(b"no radio bound")),
        R_PACKAGES => {
            // WHY "0 registered" rather than an absent heading: the registry read
            // is the thing being reported, and a heading that disappears when
            // the list is empty is indistinguishable from a list that failed.
            let (buf, len) = counted(state.app_count, b" registered");
            (b"Packages", Reading::of(&buf[..len]))
        }
        _ => {
            let index = index - FIRST_PACKAGE_ROW;
            if index >= shown_packages(state, capacity) {
                let hidden = state.app_count - shown_packages(state, capacity);
                let (buf, len) = counted(hidden, b" not shown");
                return (b"More packages", Reading::of(&buf[..len]));
            }
            // WHY the package id stands in for a missing label: the registry
            // requires a non-empty display name, but a row with no label would
            // be a row the user cannot act on, so the id beats a blank.
            let label_len = state.app_label_lens[index] as usize;
            let label: &[u8] = if label_len > 0 {
                &state.app_labels[index][..label_len]
            } else {
                &state.app_ids[index][..state.app_id_lens[index] as usize]
            };
            // WHY "active" rather than the registry's status string: the shell
            // only admits rows whose status is `active`, so the row is the proof
            // and the word restates it.
            (label, Reading::of(b"active"))
        }
    }
}

/// How many of the six readings the shell can make right now.
fn available_readings(state: &LauncherState) -> usize {
    // The clock and the date come from one refused-or-answered read; the
    // notification count comes from a different service and fails separately.
    usize::from(state.clock_source_ok) * 2 + 1
}

/// `<available> of <total> readings` -- the honest headline for a settings
/// screen whose job is to say what the device knows.
fn reading_summary(state: &LauncherState) -> ([u8; VALUE_MAX], usize) {
    let (mut buf, mut at) = decimal(available_readings(state));
    push(&mut buf, &mut at, b" of ");
    let (total, total_len) = decimal(TOTAL_READINGS);
    push(&mut buf, &mut at, &total[..total_len]);
    push(&mut buf, &mut at, b" readings");
    (buf, at)
}

/// Which stack a pixel belongs to, and which row of it.
///
/// WHY two stacks instead of one: the Power entry point sits between them
/// because the shell's touch handler looks for it there. The rows above and
/// below it are one logical list that happens to be drawn in two runs.
fn stack_row(
    x: usize,
    y: usize,
    bounds: Rect,
    count: usize,
    offset: usize,
) -> Option<(usize, Rect)> {
    if count == 0 || !bounds.contains(x, y) {
        return None;
    }
    let stack = layout::list(bounds, count, ROW_H, ROW_GUTTER)?;
    let index = (y - stack.y) / ROW_PITCH;
    let row = layout::list_row(stack, index, count, ROW_H, ROW_GUTTER)?;
    if row.contains(x, y) {
        Some((offset + index, row))
    } else {
        None
    }
}

/// The clock, when the kernel gave one.
///
/// WHY a blank status bar rather than a placeholder: the same refusal that
/// leaves it empty is logged once by the shell, and a `00:00` here would
/// contradict that log on the surface the user reads first.
fn clock_pixel(
    x: usize,
    y: usize,
    bar: &Rect,
    state: &LauncherState,
    theme: &Theme,
) -> Option<u32> {
    if !state.clock_source_ok {
        return None;
    }
    let area = Rect::new(bar.x + CONTENT_PADDING, bar.y, bar.w - CONTENT_PADDING * 2, bar.h);
    run_left(
        x,
        y,
        &area,
        type_scale::CAPTION,
        &state.clock_time,
        layout::ink_on(theme, theme.overlay),
    )
}

pub fn get_system_space_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let theme = THEME;

    // Status bar.
    let bar = Rect::new(0, 0, SCREEN_WIDTH, STATUS_BAR_HEIGHT);
    if y < STATUS_BAR_HEIGHT {
        if let Some(c) = clock_pixel(x, y, &bar, state, &theme) {
            return c;
        }
        return theme.overlay;
    }
    if y == STATUS_BAR_HEIGHT {
        return theme.border;
    }

    // WHY the entry point is tested before the list: it is a card, not a row,
    // and a list row drawn behind it would sit under a control.
    if round_contains(&POWER_ENTRY, radius::CONTROL, x, y) {
        let entry = Cell {
            rect: POWER_ENTRY,
            corner: radius::CONTROL,
            // WHY the chevron and not a sentence: the entry card is a third of
            // the screen wide, because the shell's touch handler owns that
            // rectangle. A caption there has to be two characters to survive the
            // row, and a row that reads "Pow... ope..." tells the user nothing.
            // The chevron is what every settings list uses to say "this opens
            // something", and it costs one glyph.
            label: POWER_LABEL,
            value: b">",
            // WHY a tinted card: this row is a third of the width of every other
            // row because the shell's touch handler owns that rectangle, and a
            // narrower plain card among full-width ones reads as a mistake. A
            // wash of the accent says "this one opens something" instead.
            fill: Theme::blend(theme.surface, theme.accent, 22),
            value_ink: theme.accent,
            scale: ROW_TEXT_SCALE,
        };
        return cell_pixel(x, y, &theme, &entry);
    }

    // WHY `CONTENT_PADDING` and not a spacing token: the Power card above is
    // positioned by the shell's touch handler and cannot move, so the rows either
    // line up with its left and right edges or they do not. A 24 px inset put
    // every row 16 px outside the one card on this surface that can be pressed.
    let content = layout::content_area(CONTENT_PADDING);
    let title_rect = Rect::new(content.x, content.y, content.w, text::line_height(type_scale::TITLE));
    let sub_rect = Rect::new(
        content.x,
        title_rect.y + title_rect.h + space::XS,
        content.w,
        text::line_height(type_scale::CAPTION),
    );
    let list_top = sub_rect.y + sub_rect.h + space::MD;

    let above = Rect::new(content.x, list_top, content.w, POWER_ENTRY.y - space::MD - list_top);
    let below_top = POWER_ENTRY.y + POWER_ENTRY.h + space::MD;
    let below = Rect::new(content.x, below_top, content.w, (content.y + content.h) - below_top);

    let count = row_count(state, above.h / ROW_PITCH + below.h / ROW_PITCH);
    let n_above = count.min(above.h / ROW_PITCH);
    let here = stack_row(x, y, above, n_above, 0)
        .or_else(|| stack_row(x, y, below, count - n_above, n_above));
    if let Some((index, rect)) = here {
        let capacity = above.h / ROW_PITCH + below.h / ROW_PITCH;
        let (label, value) = row(state, index, capacity);
        let cell = Cell::reading(rect, radius::CARD, label, &value, &theme);
        return cell_pixel(x, y, &theme, &cell);
    }

    // Title, and the reading count under it. WHY the count is built only once the
    // pixel is on that line: it walks the state, and this function runs for every
    // pixel of the surface.
    //
    // WHY these two bands are still correct on a 24-row cell, unlike the dashboard's
    // heading: stacking whole cells is the right way to stack two lines, and this
    // surface has room. `content.y` is 88, the title's cell is
    // `GLYPH_H * TITLE` = 120 and ends at 208, the caption's cell is 48 and ends at
    // 260, and the first row starts at 276 -- 314 px clear of the Power card at 590.
    // The dashboard's heading had 79 px for one cell, not 120, so that one had to be
    // rescaled; this one is measured, not guessed.
    if let Some(c) = run_left(x, y, &title_rect, type_scale::TITLE, b"System", theme.text) {
        return c;
    }
    if y >= sub_rect.y && y < sub_rect.y + sub_rect.h {
        let (summary, summary_len) = reading_summary(state);
        if let Some(c) = run_left(
            x,
            y,
            &sub_rect,
            type_scale::CAPTION,
            &summary[..summary_len],
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