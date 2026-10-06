//! System Space: device and account settings.
//!
//! WHY `pub`: `render_view` in the crate root dispatches here by view mode.
//! Geometry and colour come from `launcher::theme`, `launcher::layout` and
//! `launcher::text`, never from literals local to this file.
//!
//! WHY this surface is a list of the nineteen standard category namespaces and not
//! seven category tiles: the previous version drew Device, Network, Personal, Apps,
//! Power, Security and System as a fixed two-column grid over a decorative sphere,
//! and the grid did not change when the thing it claimed to describe did. Three of
//! its subtitles described a fuel gauge, a bound radio and a thermal reading that
//! this platform has no sensor for and the shell is never handed. A tile that says
//! "Battery" and a tile that says "Battery: 87%" look the same to the person reading
//! them, and only one of them is true.
//!
//! So the surface is a settings *list*, grouped the way a phone groups settings, and
//! every row is one of the nineteen namespaces. Four of them report something -- the
//! compositor's surface size, the notification count, the installed package count and
//! the kernel RTC -- and the other fifteen render as unavailable with the specific
//! absence named, because a settings list that quietly dropped half its categories
//! reads as broken while one that says "no fuel gauge bound" reads as a device.
//! Nothing here is drawn as a toggle: there is no switch on this platform that this
//! shell can read, and a switch with no reader behind it is either inert or a lie.
//!
//! WHY the rows come from state in groups rather than one flat stack: the categories
//! describe the radio, the display, the software and the machine, and a list that
//! mixed them in one alphabetical run would be longer than this screen and would say
//! less. The Power entry is pinned to a rectangle the shell's touch handler owns, so
//! the groups are flowed in two passes around it.
//!
//! WHY the pixel path is a plan and not arithmetic: the previous version of this file
//! called `run_left` and `cell_pixel`, and `cell_pixel` calls `text::ellipsize`,
//! which returns a 96-byte array by value, once for every pixel of every row. That is
//! the regression this shell already paid for once. Everything this change paints
//! through is resolved once per frame below; `run_left`, `run_right`, `run_center`
//! and `cell_pixel` are still here, unchanged, because `live_widgets` and `power`
//! call them.

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

/// The scale every shared row's label and value are drawn at.
///
/// WHY it is derived and not read from `theme::type_scale`: a row has to hold one
/// whole glyph cell, and this face's cell is `GLYPH_H` = 24 rows per scale step
/// rather than the 8 the row heights were sized against. A standalone row is 52 px,
/// so 24 * s <= 52 gives s = 2. At `type_scale::BODY` the cell is 72, which is 20
/// px taller than the row: `run_left` pins the run to the row's top edge and the
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
/// control rather than a named one. "Power" plus the caption under it says the same
/// thing with room to spare.
const POWER_LABEL: &[u8] = b"Power";

/// WHY the value column has a ceiling at all: a reason string long enough to fill
/// the row would push the label off it, and a value that needs the whole row is
/// a caption rather than a value. Below the ceiling the column is exactly as wide
/// as the value needs, so a one-character value cannot squeeze the label it
/// belongs to -- which is what a fixed split did to the narrow entry card.
const VALUE_COLUMN_MAX_PCT: usize = 50;

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
    ///
    /// WHY `allow(dead_code)`: no screen calls this today -- `live_widgets` uses
    /// [`Cell::scaled`] because its rows are a different height -- but it is the
    /// constructor that documents the shared row height, and removing it would take
    /// the only statement of what a "standard" row is with it.
    #[allow(dead_code)]
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
// Runs and the per-frame plan
//
// WHY this layer exists at all: `run_left`, `run_right`, `run_center` and
// `cell_pixel` above are the shell's shared painters and they are kept byte for
// byte, because `live_widgets` and `power` call them. But `cell_pixel` calls
// `text::ellipsize`, which returns a 96-byte array by value, once for every pixel
// of a row -- and `run_left` measures its run on every pixel it is asked about.
// That is the regression this shell already paid for once: it cost two orders of
// magnitude in frame time. So nothing on the two surfaces this change owns is
// painted through those four functions any more. Everything below settles the
// ellipsis decision, the column split, the baseline and the advance width once
// per frame, and the pixel walk does integer comparisons and nothing else.
// -----------------------------------------------------------------------------

/// Capacity of one run's byte buffer.
///
/// WHY it matches `text::ellipsize`: a run resolved once per frame has to be
/// byte-for-byte the run the shared per-pixel painter would have produced, or the
/// two painters disagree about where a truncated label ends.
pub(super) const RUN_CAP: usize = 96;

/// Rows one grouped card can hold.
///
/// WHY eight: the largest group below has six categories, and the card that names
/// the categories a frame could not fit holds up to eight of them.
const ROWS_CAP: usize = 8;

/// Cards one frame can hold: the groups above the pinned band, the band's two
/// cells, the groups below, and the overflow card.
const CARD_CAP: usize = 8;

/// Height of one row inside a grouped card.
///
/// WHY [`MIN_TOUCH_TARGET`] and not the 52 the standalone rows above use: a row in
/// a group has no gutter of its own -- the group supplies the separation -- and at
/// the `BODY` scale this face's cell is exactly `GLYPH_H * 2` = 48 px, which is
/// also exactly one touch target. A 52 px row here would buy nothing and would cost
/// the four pixels that decide whether all nineteen categories fit on the screen.
pub(super) const GROUP_ROW_H: usize = MIN_TOUCH_TARGET;

/// Height of a group's header band: one caption line and a gap under it.
///
/// WHY `space::XS` and not `space::SM`: this is the whole vertical budget for a
/// band that separates a heading from the cap of the row beneath it, and it is
/// multiplied by five groups. `space::XS` is the smallest gap in the scale that
/// still reads as a gap.
pub(super) fn header_h() -> usize {
    text::line_height(type_scale::CAPTION) + space::XS
}

/// Horizontal inset from a card's edge to its rows' text.
///
/// WHY `space::LG` and not `space::MD`, which is what the standalone rows above
/// use: a grouped card is a container the reader's eye is already inside, and its
/// text has to clear the card's own rounded edge plus the header's inset to read
/// as part of the same object rather than as a row that drifted inwards.
const ROW_PAD_X: usize = space::LG;

/// The same inset, for the sibling surface that shares this row renderer.
#[allow(dead_code)]
pub(super) fn row_pad_x() -> usize {
    ROW_PAD_X
}

/// Gap between one grouped card and the next.
///
/// WHY `space::XS`: a gap between two grouped cards is read as part of the list's
/// rhythm rather than as separation between two objects, and the five headers
/// already say where a group ends. This is the value that buys the vertical room
/// the nineteen namespaces need.
pub(super) const CARD_GAP: usize = space::XS;

/// Air between the flow of groups and the pinned Power band.
const BAND_GAP: usize = space::SM;

/// The caption under the Power entry's label.
///
/// WHY it says what the tap does rather than naming a power state: the shell holds
/// no power state -- there is no governor, no battery and no thermal reading in
/// `LauncherState` -- so a value here would be invented. Naming the destination is
/// true, and it is the thing a reader of a settings list is trying to find out.
const POWER_ENTRY_CAPTION: &[u8] = b"Open power menu";

/// One run of text, placed and measured once per frame.
///
/// WHY the box is stored and not just the origin: `test_text` walks the pen of
/// every glyph before it can answer "no", so a pixel nowhere near a run still costs
/// a pen walk. The stored box is exactly `y .. y + line_height(scale)` by
/// `x .. x + measure(run)`, which is the region `test_text` can return `Some` for.
#[derive(Clone, Copy)]
pub(super) struct Run {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    scale: usize,
    len: usize,
    ink: u32,
    b: [u8; RUN_CAP],
}

#[allow(dead_code)]
impl Run {
    /// A run that never inks anything.
    pub(super) const EMPTY: Run =
        Run { x: 0, y: 0, w: 0, h: 0, scale: 1, len: 0, ink: 0, b: [0; RUN_CAP] };

    /// `bytes` at (`x`, `y`), drawn whole with no truncation step.
    pub(super) fn whole(x: usize, y: usize, scale: usize, bytes: &[u8], ink: u32) -> Run {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        Run::placed(x, y, scale, buf, len, ink)
    }

    /// As [`Run::whole`], but nothing is drawn if the run is wider than `centre`.
    ///
    /// WHY the guard: this is what `run_center` does, and a line wider than its
    /// box has to be dropped rather than drawn flush left, which would be a
    /// different screen.
    pub(super) fn centred(x: usize, y: usize, scale: usize, centre: usize, bytes: &[u8], ink: u32) -> Run {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        if text::measure(&buf[..len], scale) > centre {
            return Run::EMPTY;
        }
        Run::placed(x + text::center_x(&buf[..len], scale, centre), y, scale, buf, len, ink)
    }

    /// As [`Run::whole`], truncated with an ellipsis if wider than `fit`.
    pub(super) fn fitted(x: usize, y: usize, scale: usize, fit: usize, bytes: &[u8], ink: u32) -> Run {
        let (buf, len) = text::ellipsize(bytes, scale, fit);
        Run::placed(x, y, scale, buf, len, ink)
    }

    /// As [`Run::fitted`], but with its right edge at `right`.
    pub(super) fn right_edge(right: usize, y: usize, scale: usize, fit: usize, bytes: &[u8], ink: u32) -> Run {
        let (buf, len) = text::ellipsize(bytes, scale, fit);
        let w = text::measure(&buf[..len], scale);
        if w > fit {
            return Run::EMPTY;
        }
        Run::placed(right - w, y, scale, buf, len, ink)
    }

    /// A run from an already-resolved buffer.
    fn placed(x: usize, y: usize, scale: usize, buf: [u8; RUN_CAP], len: usize, ink: u32) -> Run {
        Run { x, y, w: text::measure(&buf[..len], scale), h: text::line_height(scale), scale, len, ink, b: buf }
    }

    /// Whether this run will ink nothing, which is how a caller's "does this fit"
    /// question is answered without looking at the bytes.
    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The run's left edge, for a caller positioning something beside it.
    pub(super) fn left(&self) -> usize {
        self.x
    }

    /// The run's top edge.
    #[allow(dead_code)]
    pub(super) fn top(&self) -> usize {
        self.y
    }

    /// The run's scale.
    #[allow(dead_code)]
    pub(super) fn scale(&self) -> usize {
        self.scale
    }

    /// The run's bytes, for a caller recolouring it in place.
    #[allow(dead_code)]
    pub(super) fn bytes(&self) -> &[u8] {
        &self.b[..self.len]
    }

    /// The run's advance width.
    pub(super) fn width(&self) -> usize {
        self.w
    }

    /// The run's cell height.
    pub(super) fn height(&self) -> usize {
        self.h
    }

    /// The ink at (`px`, `py`) if this run puts any there.
    pub(super) fn pixel(&self, px: usize, py: usize) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if px < self.x || px - self.x >= self.w || py < self.y || py - self.y >= self.h {
            return None;
        }
        test_text(px, py, self.x, self.y, self.scale, &self.b[..self.len], self.ink)
    }
}

/// Splits a row's padded width into a label column and a value column.
///
/// WHY this is measured and not a fixed percentage: the shared painters above give
/// the value column a flat half of the row, which on this surface truncated eight
/// of the nineteen reason strings to "no boot state ...". A truncated reason is
/// worse than a short one -- it claims to name the missing thing and then does not
/// -- and the labels here are the shortest text on the surface, so reserving a third
/// of the row for one and letting the other two thirds go to the other is both
/// cheaper and the only split under which every reason on this page fits whole.
///
/// WHY the label is still capped: `avail * 45 / 100` is the guarantee that a long
/// label cannot squeeze the value out. The cap was originally a third, and that
/// truncated six labels on this surface instead of any value -- "Notifications" and
/// "Accessibility" are 208 px and 195 px at this face's body scale against a
/// 202 px third, so the value column was widened by starving the thing that was
/// already short. Measured, the longest label is 208 px and the longest value 384 px,
/// and 208 + 384 + `space::SM` is 600 of the 608 this surface has.
fn split_columns(avail: usize, label: &[u8], value: &[u8], scale: usize) -> (usize, usize) {
    if avail == 0 {
        return (0, 0);
    }
    let label_cap = avail * 45 / 100;
    let label_w = text::measure(label, scale).min(label_cap);
    let wanted = text::measure(value, scale).min(avail.saturating_sub(label_w + space::SM));
    (label_w, wanted)
}

/// One row, with its two columns and the split between them resolved.
///
/// WHY the split is two numbers rather than two rectangles: a pixel belongs to the
/// value column when it is at or after `split_x` and to the label when it is
/// before `label_end`, and those are the two edges the shared painter compares
/// against. Keeping them as numbers keeps the pixel walk two comparisons.
#[derive(Clone, Copy)]
pub(super) struct CellPlan {
    rect: Rect,
    inner: Rect,
    corner: usize,
    /// Whether to draw the one-pixel outline. A row inside a group does not get
    /// one -- the group has an outline and the rows are planes inside it.
    outlined: bool,
    fill: u32,
    split_x: usize,
    label_end: usize,
    label: Run,
    value: Run,
}

impl CellPlan {
    pub(super) const EMPTY: CellPlan = CellPlan {
        rect: Rect::new(0, 0, 0, 0),
        inner: Rect::new(0, 0, 0, 0),
        corner: 0,
        outlined: false,
        fill: 0,
        split_x: 0,
        label_end: 0,
        label: Run::EMPTY,
        value: Run::EMPTY,
    };

    /// A row at the shell's body scale: label left, value right, one baseline.
    ///
    /// WHY the vertical placement is `rect.y` and not an inset: at `BODY` this
    /// face's cell is 48 px, the same as the row, so the cell top *is* the row top
    /// and the ink lands at `CAP_TOP * 2 .. BASELINE * 2` inside it, which is
    /// optically centred in the row. The standalone rows above inset by
    /// `space::MD` because they are 52 px cards with a gutter under them; a row in
    /// a group has no gutter, so insetting it would push the ink 16 px down and out
    /// of the row.
    pub(super) fn row(
        rect: Rect,
        label: &[u8],
        value: &Reading,
        pad_x: usize,
        theme: &Theme,
    ) -> CellPlan {
        let scale = ROW_TEXT_SCALE;
        let (fill, value_ink) = value.skin(theme);
        let pad = Rect::new(rect.x + pad_x, rect.y, rect.w.saturating_sub(pad_x * 2), rect.h);
        let (label_w, value_w) = split_columns(pad.w, label, value.as_slice(), scale);
        CellPlan {
            rect,
            inner: rect.inset(1),
            corner: 0,
            outlined: false,
            fill,
            split_x: pad.x + label_w + space::SM,
            label_end: pad.x + label_w,
            label: Run::fitted(pad.x, pad.y, scale, label_w, label, theme.text),
            value: Run::right_edge(
                pad.x + pad.w,
                pad.y,
                scale,
                value_w,
                value.as_slice(),
                value_ink,
            ),
        }
    }

    /// A row for the 311 px cell beside the pinned Power entry, where even a caption
    /// label and its reason are the whole budget.
    ///
    /// WHY this exists and why `compact` was not enough: `compact` keeps the label at
    /// `BODY`, and "Battery" is 196 px at that scale against a 263 px padded width,
    /// so the label was ellipsized to "B...". Dropping the label to `CAPTION` puts it
    /// at 98 px and leaves 189 px for "no fuel gauge bound" at 152 px, so both fit
    /// whole. The cell is the band's quieter half and sits beside a feature card set
    /// at `TITLE`, so a caption row beside it reads as secondary rather than as
    /// small -- and a truncated label would read as broken, which is worse at any
    /// size.
    pub(super) fn fine(rect: Rect, label: &[u8], value: &Reading, pad_x: usize, theme: &Theme) -> CellPlan {
        let (fill, value_ink) = value.skin(theme);
        let pad = Rect::new(rect.x + pad_x, rect.y, rect.w.saturating_sub(pad_x * 2), rect.h);
        let (label_w, value_w) = split_columns(pad.w, label, value.as_slice(), type_scale::CAPTION);
        // WHY both cells are centred rather than top-aligned: at `CAPTION` the cell is
        // 24 px inside a 48 px row, and pinning it to the row's top edge drops it
        // against the row above.
        let cap = text::line_height(type_scale::CAPTION);
        let row_y = pad.y + (pad.h - cap) / 2;
        CellPlan {
            rect,
            inner: rect.inset(1),
            corner: 0,
            outlined: false,
            fill,
            split_x: pad.x + label_w + space::SM,
            label_end: pad.x + label_w,
            label: Run::fitted(pad.x, row_y, type_scale::CAPTION, label_w, label, theme.text),
            value: Run::right_edge(pad.x + pad.w, row_y, type_scale::CAPTION, value_w, value.as_slice(), value_ink),
        }
    }

    /// The ink at (`px`, `py`), or `None` where this row does not reach.
    pub(super) fn pixel(&self, px: usize, py: usize, theme: &Theme) -> Option<u32> {
        if !round_contains(&self.rect, self.corner, px, py) {
            return None;
        }
        if self.outlined && !round_contains(&self.inner, self.corner.saturating_sub(1), px, py) {
            return Some(theme.border);
        }
        if px >= self.split_x {
            if let Some(c) = self.value.pixel(px, py) {
                return Some(c);
            }
        } else if px < self.label_end {
            if let Some(c) = self.label.pixel(px, py) {
                return Some(c);
            }
        }
        Some(self.fill)
    }
}

/// A grouped card: one rounded rectangle, an optional header band, and a run of
/// equal-height row planes.
///
/// WHY the rows are inside the card rather than being cards of their own: a
/// settings list is a list, and a wall of separate rounded rectangles reads as a
/// grid of tiles. Grouping is what makes nineteen categories read as one list, and
/// it is what lets [`Reading::skin`] carry the separation -- a row with a reading is
/// `surface`, a row without one is `surface_sunken` -- instead of 28 px of
/// whitespace per row.
#[derive(Clone, Copy)]
pub(super) struct CardPlan {
    rect: Rect,
    inner: Rect,
    corner: usize,
    fill: u32,
    /// The group's heading.
    header: Run,
    /// A heading and a caption under it, for the one card on the page that is a
    /// control rather than a list of settings.
    lead: Run,
    sub: Run,
    pub(super) rows_top: usize,
    pub(super) group_row_h: usize,
    count: usize,
    rows: [CellPlan; ROWS_CAP],
}

impl CardPlan {
    pub(super) const EMPTY: CardPlan = CardPlan {
        rect: Rect::new(0, 0, 0, 0),
        inner: Rect::new(0, 0, 0, 0),
        corner: 0,
        fill: 0,
        header: Run::EMPTY,
        lead: Run::EMPTY,
        sub: Run::EMPTY,
        rows_top: 0,
        group_row_h: GROUP_ROW_H,
        count: 0,
        rows: [CellPlan::EMPTY; ROWS_CAP],
    };

    /// A card with a header band and room for `ROWS_CAP` rows.
    ///
    /// WHY the header is set in capitals: a header and a row are drawn from the
    /// same two columns and at the same sizes, so without the capitals the only
    /// thing that says "this band is a heading, not a setting" is that it is the
    /// only row with no value.
    pub(super) fn with_header(rect: Rect, corner: usize, header: &[u8], theme: &Theme) -> CardPlan {
        let cap = text::line_height(type_scale::CAPTION);
        let mut caps = [0u8; 24];
        let mut n = 0usize;
        for &c in header.iter() {
            if n == caps.len() {
                break;
            }
            caps[n] = if c >= b'a' && c <= b'z' { c - 32 } else { c };
            n += 1;
        }
        CardPlan {
            rect,
            inner: rect.inset(1),
            corner,
            fill: theme.surface,
            header: Run::whole(
                rect.x + ROW_PAD_X,
                rect.y + (header_h() - cap) / 2,
                type_scale::CAPTION,
                &caps[..n],
                theme.text_muted,
            ),
            lead: Run::EMPTY,
            sub: Run::EMPTY,
            rows_top: rect.y + header_h(),
            group_row_h: GROUP_ROW_H,
            count: 0,
            rows: [CellPlan::EMPTY; ROWS_CAP],
        }
    }

    /// A card with no header band, for the cell beside the pinned Power entry.
    ///
    /// WHY no header there: that cell's height is fixed at 120 by the shell's touch
    /// handler, and a header plus two rows is 124. Two centred rows read as the
    /// band's second column rather than as a group that lost its heading.
    pub(super) fn bare(rect: Rect, corner: usize, theme: &Theme) -> CardPlan {
        CardPlan { rect, inner: rect.inset(1), corner, fill: theme.surface, ..CardPlan::EMPTY }
    }

    /// Adds a row. Rows are added top down, so the index is the row count so far.
    pub(super) fn push(&mut self, cell: CellPlan) {
        if self.count < ROWS_CAP {
            self.rows[self.count] = cell;
            self.count += 1;
        }
    }

    /// Recolours the card's plane, for a card that sits inside a sheet rather than
    /// on the page.
    #[allow(dead_code)]
    pub(super) fn set_fill(&mut self, fill: u32) {
        self.fill = fill;
    }

    /// Recolours the header, for the one card whose heading is a warning.
    #[allow(dead_code)]
    pub(super) fn set_header_ink(&mut self, ink: u32) {
        self.header = Run::whole(
            self.header.left(),
            self.header.top(),
            self.header.scale(),
            self.header.bytes(),
            ink,
        );
    }

    /// This card's top edge, for the pixel walk's ordering test.
    pub(super) fn top(&self) -> usize {
        self.rect.y
    }

    /// The ink at (`px`, `py`), or `None` where this card does not reach.
    pub(super) fn pixel(&self, px: usize, py: usize, theme: &Theme) -> Option<u32> {
        if !round_contains(&self.rect, self.corner, px, py) {
            return None;
        }
        if !round_contains(&self.inner, self.corner.saturating_sub(1), px, py) {
            return Some(theme.border);
        }
        if self.count > 0 {
            let first = self.rows_top;
            let last = first + self.group_row_h * self.count;
            if py >= first && py < last {
                let row = &self.rows[(py - first) / self.group_row_h];
                if let Some(ink) = row.pixel(px, py, theme) {
                    return Some(ink);
                }
            }
        }
        // WHY the three runs are tested in this order: a card is either rows under
        // a heading, or the Power card's heading and caption and nothing else, and
        // the two shapes never both carry ink, so the order only has to be cheap.
        if let Some(ink) = self.header.pixel(px, py) {
            return Some(ink);
        }
        if let Some(ink) = self.lead.pixel(px, py) {
            return Some(ink);
        }
        if let Some(ink) = self.sub.pixel(px, py) {
            return Some(ink);
        }
        Some(self.fill)
    }
}

/// A count chip in the status bar.
pub(super) struct Chip {
    rect: Rect,
    fill: u32,
    text: Run,
}

#[allow(dead_code)]
impl Chip {
    /// A chip of `fill` carrying `text`, with `space::SM` of air on the text's left.
    pub(super) fn new(text: Run, fill: u32) -> Chip {
        Chip {
            rect: Rect::new(text.left() - space::SM, space::XS, text.width() + space::SM, text.height()),
            fill,
            text,
        }
    }

    pub(super) fn pixel(&self, px: usize, py: usize) -> Option<u32> {
        if !round_contains(&self.rect, radius::CHIP, px, py) {
            return None;
        }
        Some(self.text.pixel(px, py).unwrap_or(self.fill))
    }
}

// -----------------------------------------------------------------------------
// The category table
// -----------------------------------------------------------------------------

/// Where a category's state comes from, or the fact that nothing on this platform
/// publishes it.
#[derive(Clone, Copy, PartialEq)]
enum Source {
    /// No wireless radio of any kind is bound.
    Radio,
    /// No Bluetooth adapter is bound.
    Adapter,
    /// No baseband modem is bound.
    Modem,
    /// No fuel gauge is bound. What the guest has is a PIT, which counts guest CPU
    /// time and says nothing about a cell.
    FuelGauge,
    /// The compositor is presenting a guest surface, and this surface is it.
    Surface,
    /// The notification service publishes a count for the calling account.
    Notifications,
    /// The package registry answered, and the answer is the installed count.
    Registry,
    /// The kernel RTC answered, and this is the calendar date it returned.
    Clock,
    /// A service is built and running, but the shell reads none of it, so there is
    /// nothing here to report without inventing a value.
    Unread,
}

struct Category {
    label: &'static [u8],
    /// What is missing, used verbatim as the row's value when the source cannot
    /// answer.
    ///
    /// WHY every one of these names the specific thing rather than saying
    /// "unavailable": "unavailable" alone tells the reader nothing about whether
    /// the row is a fault, and on a settings screen the reader's first question
    /// about a category is exactly that. None of these strings is a number, and
    /// none is a toggle: a switch drawn as off with no reader behind it is a
    /// control that cannot be operated, and one drawn as on would be a lie.
    reason: &'static [u8],
    source: Source,
}

/// Whether this category has a reading, without building one.
///
/// WHY this is separate from [`reading`]: the page caption counts how many
/// categories can answer, and counting must not build a value string for each of
/// the nineteen only to keep four of them.
fn can_answer(cat: &Category, state: &LauncherState) -> bool {
    match cat.source {
        Source::Surface | Source::Notifications | Source::Registry => true,
        Source::Clock => state.clock_source_ok,
        Source::Radio | Source::Adapter | Source::Modem | Source::FuelGauge | Source::Unread => false,
    }
}

/// The value column entry for one category.
///
/// WHY the whole rule of this screen lives in this function: the one thing it must
/// never do is print a number, a percentage or a level where the platform has
/// nothing to produce one. Availability is decided here, next to the bytes that
/// would be printed, because that is the only place the decision cannot be
/// forgotten later.
fn reading(cat: &Category, state: &LauncherState) -> Reading {
    match cat.source {
        Source::Radio | Source::Adapter | Source::Modem | Source::FuelGauge | Source::Unread => {
            Reading::unavailable(cat.reason)
        }
        // WHY the surface size is a fact about this platform and not an invented
        // panel reading: it is the geometry the compositor is scaling onto the
        // scanout as this frame is drawn, and the word "surface" keeps it from
        // being read as an EDID the guest never read.
        Source::Surface => {
            let (mut buf, mut at) = decimal(SCREEN_WIDTH);
            push(&mut buf, &mut at, b"x");
            let (h, h_len) = decimal(SCREEN_HEIGHT);
            push(&mut buf, &mut at, &h[..h_len]);
            push(&mut buf, &mut at, b" surface");
            Reading::of(&buf[..at])
        }
        Source::Notifications => {
            let (buf, len) = counted(state.notification_count, b" active");
            Reading::of(&buf[..len])
        }
        Source::Registry => {
            let (buf, len) = counted(state.app_count, b" installed");
            Reading::of(&buf[..len])
        }
        Source::Clock => {
            if state.clock_source_ok {
                // WHY the shell's own bytes: with no RTC authority granted,
                // `date_line` holds blanks, and a substituted value here would be a
                // fabrication on the surface the user trusts most.
                Reading::of(&state.date_line[..LOCK_DATE_TEXT_LEN])
            } else {
                Reading::unavailable(cat.reason)
            }
        }
    }
}

/// The seventeen categories that live in a group on the page.
static CATEGORIES: [Category; 17] = [
    Category { label: b"Wi-Fi", reason: b"no radio bound", source: Source::Radio },
    Category { label: b"Bluetooth", reason: b"no adapter bound", source: Source::Adapter },
    Category { label: b"Cellular", reason: b"no modem bound", source: Source::Modem },
    Category { label: b"Display", reason: b"", source: Source::Surface },
    Category { label: b"Sound", reason: b"no mixer reading", source: Source::Unread },
    Category { label: b"Notifications", reason: b"", source: Source::Notifications },
    Category { label: b"Apps", reason: b"", source: Source::Registry },
    Category { label: b"Permissions", reason: b"no grant reading", source: Source::Unread },
    Category { label: b"Security", reason: b"no boot state", source: Source::Unread },
    Category { label: b"Privacy", reason: b"no policy store", source: Source::Unread },
    Category { label: b"Users", reason: b"no account state", source: Source::Unread },
    Category { label: b"Accessibility", reason: b"no service bound", source: Source::Unread },
    Category { label: b"Developer", reason: b"no terminal flag", source: Source::Unread },
    Category { label: b"Updates", reason: b"no update state", source: Source::Unread },
    Category { label: b"Date & time", reason: b"no RTC reading", source: Source::Clock },
    Category { label: b"Language", reason: b"no locale", source: Source::Unread },
    Category { label: b"About", reason: b"no build id", source: Source::Unread },
];

/// The two device resources in the cell beside the pinned Power entry.
///
/// WHY these two and not others: they are the categories whose absence is a
/// statement about the machine rather than about the shell, so they belong beside
/// Power -- the control that answers "what can this device do about its own power"
/// -- rather than in a list of software settings.
static RESOURCES: [Category; 2] = [
    Category { label: b"Battery", reason: b"no fuel gauge bound", source: Source::FuelGauge },
    Category { label: b"Storage", reason: b"no volume reading", source: Source::Unread },
];

struct Group {
    header: &'static [u8],
    first: usize,
    count: usize,
}

/// WHY these groups and this order: the three above the pinned band are the ones a
/// reader looks at first, and they are short enough to fit above a rectangle that
/// cannot move; the band interrupts them because the shell's touch handler pins the
/// Power entry there; and the two groups below carry the categories that describe
/// the software rather than the hardware.
///
/// WHY nothing is dropped: the nineteen standard namespaces appear once each --
/// seventeen across these five groups and two in the band -- and the frame fits all
/// nineteen on this geometry. The arithmetic is written out in [`build`].
static GROUPS: [Group; 5] = [
    Group { header: b"CONNECTIVITY", first: 0, count: 3 },
    Group { header: b"DISPLAY & SOUND", first: 3, count: 2 },
    Group { header: b"ALERTS", first: 5, count: 1 },
    Group { header: b"APPS & SECURITY", first: 6, count: 5 },
    Group { header: b"SYSTEM", first: 11, count: 6 },
];

fn group_h(group: &Group) -> usize {
    header_h() + group.count * GROUP_ROW_H
}

/// One resolved frame of the settings surface.
struct Frame {
    clock: Option<Run>,
    badge: Option<Chip>,
    title: Run,
    caption: Run,
    cards: [CardPlan; CARD_CAP],
    count: usize,
    indicator: Rect,
}

impl Frame {
    const EMPTY: Frame = Frame {
        clock: None,
        badge: None,
        title: Run::EMPTY,
        caption: Run::EMPTY,
        cards: [CardPlan::EMPTY; CARD_CAP],
        count: 0,
        indicator: Rect::new(0, 0, 0, 0),
    };
}

/// The area the groups above the band may occupy.
///
/// `main.rs` hits `(590..710).contains(&y)` for the Power entry, so the band above
/// and below stop one `BAND_GAP` short of its edges.
fn upper_bottom() -> usize {
    POWER_ENTRY.y - BAND_GAP
}

fn lower_top() -> usize {
    POWER_ENTRY.y + POWER_ENTRY.h + BAND_GAP
}

fn lower_bottom() -> usize {
    let c = layout::content_area(CONTENT_PADDING);
    c.y + c.h
}

/// Places one group at `y`. Returns false when there is no room for it.
fn place_group(
    cards: &mut [CardPlan; CARD_CAP],
    count: &mut usize,
    group: &Group,
    y: usize,
    state: &LauncherState,
    theme: &Theme,
) -> bool {
    if *count >= CARD_CAP || group.count > ROWS_CAP {
        return false;
    }
    let content = layout::content_area(CONTENT_PADDING);
    let rect = Rect::new(content.x, y, content.w, group_h(group));
    let mut card = CardPlan::with_header(rect, radius::CARD, group.header, theme);
    for i in 0..group.count {
        let cat = &CATEGORIES[group.first + i];
        let row_rect = Rect::new(rect.x, card.rows_top + i * GROUP_ROW_H, rect.w, GROUP_ROW_H);
        card.push(CellPlan::row(row_rect, cat.label, &reading(cat, state), ROW_PAD_X, theme));
    }
    cards[*count] = card;
    *count += 1;
    true
}

/// Places the pinned Power entry.
///
/// WHY it is a featured card and not a row: it is the only thing on this surface
/// that can be pressed, and `main.rs` opens the Power drawer from it. A list whose
/// one interactive element looks identical to the eighteen that do nothing invites
/// the wrong tap. WHY a 13 percent accent wash and not a filled chip: a saturated
/// fill at 305x120 would be the loudest object on a page whose loudest object should
/// still be its title.
fn place_power(cards: &mut [CardPlan; CARD_CAP], count: &mut usize, theme: &Theme) -> bool {
    if *count >= CARD_CAP {
        return false;
    }
    let title = text::line_height(type_scale::TITLE);
    let cap = text::line_height(type_scale::CAPTION);
    let wash = Theme::blend(theme.surface, theme.accent, 34);
    let mut card = CardPlan::bare(POWER_ENTRY, radius::CARD, theme);
    card.fill = wash;
    // WHY the accent wash and not a filled chip: a saturated fill at 305x120 would
    // be the loudest object on a page whose loudest object should still be its
    // title. The wash separates the one control from the reading cards without
    // competing with the page.
    //
    // WHY `POWER_LABEL` and a caption and not a value column: this card is a third
    // of the page wide, so a 50 percent value column is 139 px, and the caption
    // below it is the thing that says what the tap does. A value column here would
    // leave room for two characters -- which is why the previous version of this
    // file ellipsized it to "Pow..." and read as a truncated control.
    let top = POWER_ENTRY.y + (POWER_ENTRY.h - title - space::XS - cap) / 2;
    card.lead = Run::whole(
        POWER_ENTRY.x + ROW_PAD_X,
        top,
        type_scale::TITLE,
        POWER_LABEL,
        layout::ink_on(theme, wash),
    );
    card.sub = Run::whole(
        POWER_ENTRY.x + ROW_PAD_X,
        top + title + space::XS,
        type_scale::CAPTION,
        POWER_ENTRY_CAPTION,
        layout::ink_on(theme, wash),
    );
    cards[*count] = card;
    *count += 1;
    true
}

/// Places the cell beside the pinned entry: the two device resources.
fn place_resources(cards: &mut [CardPlan; CARD_CAP], count: &mut usize, state: &LauncherState, theme: &Theme) -> bool {
    if *count >= CARD_CAP {
        return false;
    }
    let content = layout::content_area(CONTENT_PADDING);
    let x = POWER_ENTRY.x + POWER_ENTRY.w + space::LG;
    let rect = Rect::new(x, POWER_ENTRY.y, content.x + content.w - x, POWER_ENTRY.h);
let mut card = CardPlan::bare(rect, radius::CARD, theme);
    // WHY the rows are centred rather than started at the top: this card has no
    // header band, and a cell whose two rows start hard against its top edge reads
    // as a card whose heading was cut off rather than as a card that never had one.
    let top = rect.y + (rect.h - RESOURCES.len() * GROUP_ROW_H) / 2;
    card.rows_top = top;
    for (i, cat) in RESOURCES.iter().enumerate() {
        let row_rect = Rect::new(rect.x, top + i * GROUP_ROW_H, rect.w, GROUP_ROW_H);
        card.push(CellPlan::fine(row_rect, cat.label, &reading(cat, state), space::SM, theme));
    }
    cards[*count] = card;
    *count += 1;
    true
}

/// Places the card that names the categories a frame could not fit.
///
/// WHY it names them rather than counting them: a card headed "below the fold" that
/// lists each category reads as the list being longer than the screen, which is
/// true, where a card that stops half way through a group reads as a rendering
/// fault, which is not.
fn place_overflow(
    cards: &mut [CardPlan; CARD_CAP],
    count: &mut usize,
    pending: &[usize; ROWS_CAP],
    len: usize,
    y: usize,
    state: &LauncherState,
    theme: &Theme,
) -> bool {
    if *count >= CARD_CAP || len == 0 || len > ROWS_CAP {
        return false;
    }
    let content = layout::content_area(CONTENT_PADDING);
    let rect = Rect::new(content.x, y, content.w, header_h() + len * GROUP_ROW_H);
    let mut card = CardPlan::with_header(rect, radius::CARD, b"below the fold", theme);
    card.header = Run::whole(
        rect.x + ROW_PAD_X,
        rect.y + (header_h() - text::line_height(type_scale::CAPTION)) / 2,
        type_scale::CAPTION,
        b"BELOW THE FOLD",
        theme.warning,
    );
    for i in 0..len {
        let cat = &CATEGORIES[pending[i]];
        let row_rect = Rect::new(rect.x, card.rows_top + i * GROUP_ROW_H, rect.w, GROUP_ROW_H);
        card.push(CellPlan::row(row_rect, cat.label, &reading(cat, state), ROW_PAD_X, theme));
    }
    cards[*count] = card;
    *count += 1;
    true
}

/// Resolves one frame of the settings surface.
///
/// WHY the gaps are all `space::XS` and the title starts at `content_area`'s own top
/// edge: every pixel of vertical space below the caption belongs to the category
/// list. The arithmetic, once, so it can be checked against the geometry above:
///
/// ```text
///   content.y   = 48 + 40                   =   88
///   title       = 88 .. 88 + 72                      TITLE cell
///   caption     = 164 .. 188                            CAPTION cell
///   list_top    = 192
///   upper       = 192 .. 582                           590 - BAND_GAP
///     CONNECTIVITY     header_h + 3*48 = 172           192 .. 364
///     DISPLAY & SOUND  header_h + 2*48 = 124           368 .. 492
///     ALERTS            header_h + 1*48 =  76           496 .. 572     10 px spare
///   band        = 590 .. 710                            Power + two resources
///   lower       = 718 .. 1336
///     APPS & SECURITY  header_h + 5*48 = 268           718 .. 986
///     SYSTEM            header_h + 6*48 = 316           990 .. 1306   30 px spare
/// ```
fn build(state: &LauncherState) -> Frame {
    let theme = THEME;
    let content = layout::content_area(CONTENT_PADDING);
    let cap_h = text::line_height(type_scale::CAPTION);

    // ---- status bar ------------------------------------------------------
    // WHY the badge exists only when the count is non-zero: an empty chip saying
    // "0" is a notification the user has to read in order to learn there is
    // nothing, which is the opposite of what a badge is for.
    let badge = if state.notification_count > 0 {
        let (digits, dlen) = decimal(state.notification_count);
        let t = Run::right_edge(
            content.x + content.w - space::SM,
            (STATUS_BAR_HEIGHT - cap_h) / 2,
            type_scale::CAPTION,
            60,
            &digits[..dlen],
            theme.text_on_accent,
        );
        if t.is_empty() {
            None
        } else {
            Some(Chip {
                rect: Rect::new(t.x - space::SM, space::XS, t.w + space::SM, t.h),
                fill: theme.accent,
                text: t,
            })
        }
    } else {
        None
    };

    // ---- title block -----------------------------------------------------
    // WHY `TITLE` for the page and `CAPTION` for the line under it: that is the
    // whole type hierarchy in two lines, and the caption is the sentence that tells
    // the reader what this page is before they read a single row.
    let title_h = text::line_height(type_scale::TITLE);
    let title_y = content.y;
    let cap_y = title_y + title_h + space::XS;

    let mut caption_buf = [0u8; VALUE_MAX];
    let mut at = 0usize;
    // WHY the count walks both tables: the nineteen namespaces are seventeen rows in
    // groups plus the two in the band, and a headline that counted only the groups
    // would report a denominator the reader cannot check against the page.
    let readable = GROUPS
        .iter()
        .flat_map(|g| CATEGORIES[g.first..g.first + g.count].iter())
        .chain(RESOURCES.iter())
        .filter(|c| can_answer(c, state))
        .count();
    let (n, n_len) = decimal(readable);
    push(&mut caption_buf, &mut at, &n[..n_len]);
    push(&mut caption_buf, &mut at, b" of ");
    let total: usize = GROUPS.iter().map(|g| g.count).sum::<usize>() + RESOURCES.len();
    let (t, t_len) = decimal(total);
    push(&mut caption_buf, &mut at, &t[..t_len]);
    push(&mut caption_buf, &mut at, b" categories report a reading");

    // ---- flow the groups around the pinned band --------------------------
    // WHY two passes with the band between them and not one continuous cursor: the
    // Power entry's rectangle is fixed by the shell's touch handler, so a single
    // cursor would either draw the list under it or leave a hole. Two passes make
    // the hole the band itself.
    let mut frame = Frame::EMPTY;
    frame.clock = if state.clock_source_ok {
        Some(Run::whole(
            content.x,
            (STATUS_BAR_HEIGHT - cap_h) / 2,
            type_scale::CAPTION,
            &state.clock_time,
            layout::ink_on(&theme, theme.overlay),
        ))
    } else {
        None
    };
    frame.badge = badge;
    frame.title = Run::whole(content.x, title_y, type_scale::TITLE, b"System", theme.text);
    frame.caption = Run::whole(content.x, cap_y, type_scale::CAPTION, &caption_buf[..at], theme.text_muted);

    let list_top = cap_y + cap_h + space::XS;
    let mut cards = [CardPlan::EMPTY; CARD_CAP];
    let mut count = 0usize;

    let mut y = list_top;
    let mut g = 0usize;
    while g < GROUPS.len() {
        let group = &GROUPS[g];
        let h = group_h(group);
        if y + h > upper_bottom() {
            break;
        }
        if !place_group(&mut cards, &mut count, group, y, state, &theme) {
            break;
        }
        y += h + CARD_GAP;
        g += 1;
    }

    place_power(&mut cards, &mut count, &theme);
    place_resources(&mut cards, &mut count, state, &theme);

    y = lower_top();
    let mut pending = [0usize; ROWS_CAP];
    let mut pending_len = 0usize;
    while g < GROUPS.len() {
        let group = &GROUPS[g];
        let h = group_h(group);
        if count < CARD_CAP && y + h <= lower_bottom() && place_group(&mut cards, &mut count, group, y, state, &theme)
        {
            y += h + CARD_GAP;
        } else {
            for i in 0..group.count {
                if pending_len < ROWS_CAP {
                    pending[pending_len] = group.first + i;
                    pending_len += 1;
                }
            }
        }
        g += 1;
    }
    if pending_len > 0
        && y + header_h() + pending_len * GROUP_ROW_H <= lower_bottom()
        && place_overflow(&mut cards, &mut count, &pending, pending_len, y, state, &theme)
    {
        count += 1;
    }

    frame.cards = cards;
    frame.count = count;
    let indicator_w = space::XL * 4;
    frame.indicator = Rect::new(
        (SCREEN_WIDTH - indicator_w) / 2,
        SCREEN_HEIGHT - space::MD - space::XS,
        indicator_w,
        space::XS,
    );
    frame
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

/// Paints the page.
///
/// WHY the cards are walked in the order they appear down the screen: `count` is
/// filled top down and the band is in the middle, so a pixel above the first card
/// can stop the walk immediately instead of testing eight rectangles.
pub fn get_system_space_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let theme = THEME;
    let f = frame(x, y, state);

    if y < STATUS_BAR_HEIGHT {
        if let Some(c) = f.clock.as_ref().and_then(|r| r.pixel(x, y)) {
            return c;
        }
        if let Some(chip) = &f.badge {
            if let Some(c) = chip.pixel(x, y) {
                return c;
            }
        }
        return theme.overlay;
    }
    if y == STATUS_BAR_HEIGHT {
        return theme.border;
    }
    if let Some(c) = f.title.pixel(x, y) {
        return c;
    }
    if let Some(c) = f.caption.pixel(x, y) {
        return c;
    }
    let mut i = 0usize;
    while i < f.count {
        let card = &f.cards[i];
        if y < card.top() {
            break;
        }
        if let Some(c) = card.pixel(x, y, &theme) {
            return c;
        }
        i += 1;
    }
    if f.indicator.contains(x, y) {
        return Theme::blend(theme.text_muted, theme.bg, 140);
    }
    theme.bg
}