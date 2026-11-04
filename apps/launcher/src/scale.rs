// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The person's text size, and the shell geometry that follows it.
//!
//! WHY this module exists: every box in this crate was authored as the platform's
//! type ladder read at one hundred per cent, so a reader who raised "Text size" in
//! Settings kept getting the same pixels here -- the shell never asked the store for
//! the level, and no face below could answer at a rung it had never been handed.
//! Settings writes the level, `DynamicType::publish` hands it to this process's
//! renderer, and the faces here give it the geometry back. A layout that moves its
//! ink and keeps its plate is the defect this exists to prevent, so the rung is read
//! from the one cell the ink is drawn from ([`draw_level`]) and every plate a run
//! sits in is derived from that same number.
//!
//! The authored numbers stay as the anchor of each derivation rather than being
//! replaced by it: every `_at` face below answers the number the shell was drawn
//! with at [`PLATFORM_LEVEL`], and the sweeps beside them refuse the day it stops
//! being true. That is what lets the shell carry the setting without moving one
//! pixel for the reader who never raised it.
//!
//! SCOPE, stated rather than hidden: the app drawer's catalogue names are the first
//! surface wired through here. The chips, the icon badges and the search field still
//! answer the authored ladder, because each sits in a pill sized to the run inside it
//! and moving one means moving the pill row, its band and its tap test in the same
//! stroke. A screen joins by deriving its own plate beside the authored constant and
//! keeping [`PLATFORM_LEVEL`]'s answer identical to the number it was drawn with.

use crate::font;
use crate::theme::type_scale;
use interos_std::design::outline_text;
use interos_std::design::spacing::GRID_20;
use interos_std::design::typeface;

/// The text-size level every box in this crate was authored against.
///
/// WHY it is named: each box is derived twice -- once as a `const fn` of a level, so
/// the compile-time gates can sweep all four selectable levels, and once at run time
/// through the published rung, so the pixels a person gets follow the setting they
/// chose.
pub const PLATFORM_LEVEL: u8 = font::TEXT_SCALE_NORMAL;

/// The highest level the ladder offers, and therefore the level the gates sweep to.
/// `UserTextScale::from_u8` clamps anything above it, so a sweep past this rung would
/// be a sweep over a level no renderer can be handed.
pub const MAX_LEVEL: u8 = 3;

/// The rung an app name is drawn at, and the numbers a tile is measured in.
///
/// WHY they are repeated here rather than read from the screen: the drawer lives in
/// the binary, and a binary cannot be imported by the library that has to test it. The
/// drawer asserts that each of these equals its own constant, so the clamp and the
/// layout cannot drift apart.
const NAME_SCALE: usize = type_scale::BODY;
pub const DRAWER_ICON_D: usize = 80;
pub const DRAWER_ICON_NAME_GAP: usize = 10;
pub const DRAWER_ROWS: usize = 5;
pub const DRAWER_GRID_H: usize = 895;
pub const DRAWER_GUTTER: usize = GRID_20;

/// The later of two layout edges.
///
/// WHY a helper rather than `Ord::max`: `max` is not callable in a `const` context on
/// this toolchain, and every edge a plate is built from has to be a compile-time
/// constant, because a pixel function compares against it once per pixel.
pub const fn later(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}

/// The cap height a rung inks at on a level.
pub const fn cap_at(scale: usize, level: u8) -> u32 {
    font::cap_for_user_scale(scale, level)
}

/// The line box a rung reserves on a level.
pub const fn line_at(scale: usize, level: u8) -> usize {
    font::line_height_for_user_scale(scale, level)
}

/// The ascent a rung's baseline sits above its box's top on a level.
pub const fn ascent_at(scale: usize, level: u8) -> usize {
    font::ascent_for_user_scale(scale, level)
}

/// The pixels-per-em a rung is rasterised at on a level.
pub const fn em_at(scale: usize, level: u8) -> u32 {
    font::em_for_user_scale(scale, level)
}

/// The rows a run at `scale` can ink on `level`: [`outline_text::row_band`]'s union of
/// the outline line box and the bitmap cell, asked at a level rather than at the ladder.
///
/// WHY a restatement and not a call: `row_band` is the platform's own answer and it is
/// a `const fn` of the ladder, so a plate sized from it can never see the setting. The
/// walk below is the same arithmetic -- the lower of the two faces' baselines, the
/// deeper of the two faces' bottoms -- with every metric read through the
/// `*_user_scale` faces, and the identity gate beside the derivation of
/// [`DRAWER_CELL_H`] refuses the day the two answers disagree at the level they were
/// authored at.
pub const fn rows_at(scale: usize, level: u8) -> usize {
    if scale == 0 {
        return 0;
    }
    let em = em_at(scale, level);
    let (outline_ascent, outline_descent) = typeface::line_metrics(em);
    let outline_ascent = outline_ascent as usize;
    let outline_descent = outline_descent as usize;
    let line_ascent = ascent_at(scale, level);
    let baseline = later(outline_ascent, line_ascent);
    later(baseline + outline_descent, line_at(scale, level))
}

/// An authored vertical stride between two stacked runs, grown by the rows the rung's
/// box gained above the level the stride was authored at.
///
/// WHY this form: a stride is the space one run's box plus its air, and the only
/// honest way to keep the authored pixels at Normal *and* carry a raised rung is to add
/// exactly the growth the box took. It never shrinks a stride, and at the authored
/// level it is the stride itself.
pub const fn grown(stride: usize, scale: usize, level: u8) -> usize {
    stride + rows_at(scale, level).saturating_sub(rows_at(scale, PLATFORM_LEVEL))
}

/// A plate that reserves a rung's box plus its authored air, never below the height it
/// was drawn with.
///
/// WHY the floor: several of the shell's band heights are numbers its own fit gates
/// read, so a plate may only ever grow -- at the authored level this is `authored`
/// exactly, and the air is solved from the authored pair rather than invented here.
pub const fn plate_at(authored: usize, scale: usize, level: u8) -> usize {
    let air = authored.saturating_sub(rows_at(scale, PLATFORM_LEVEL));
    later(authored, rows_at(scale, level) + air)
}

// -----------------------------------------------------------------------------
// The rung the painter and the layout both ask for
// -----------------------------------------------------------------------------

/// Whether the rung `level` names is one the shell's binding surface can carry.
///
/// WHY the app drawer is the binding surface: a tile centres an icon disc above a
/// catalogue name, and the disc does not grow with the type. The drawer has no scroll
/// inside a page either, so the question is whether the tallest name box the level
/// answers still leaves the icon-and-name group inside the shortest cell its grid can
/// build.
pub const fn drawer_level_holds(level: u8) -> bool {
    let name_box = line_at(NAME_SCALE, level);
    let group = DRAWER_ICON_D + DRAWER_ICON_NAME_GAP + name_box;
    group <= DRAWER_CELL_H
}

/// The shortest tile cell the drawer's grid can build: five rows in the drawer's
/// grid band, less the gutters between them.
///
/// WHY a face of this module: [`drawer_level_holds`] measures a tile against it, and a
/// screen module cannot be reached from here. `layout::grid_fixed_columns` divides the
/// band by the rows it actually needs, so a half-empty page gets *taller* cells -- five
/// rows is the tightest case, and the one a name's rung has to fit inside.
pub const DRAWER_CELL_H: usize =
    (DRAWER_GRID_H - (DRAWER_ROWS - 1) * GRID_20) / DRAWER_ROWS;

/// The rung this shell draws at, given the rung the person asked for.
///
/// WHY a clamp and not the published rung itself: growing every box to a rung the
/// panel cannot carry walks the last row of tiles out of the grid band, which is the
/// same failure the drawer's own fit gates refuse at compile time. The clamp hands
/// back the largest rung that fits rather than the one that does not.
///
/// WHY a downward walk: the binding box grows with the rung and never shrinks, so the
/// largest carrying rung is found by stepping down from the one asked for -- and at the
/// authored level the walk never moves, which the sweep below proves.
pub const fn draw_level_at(want: u8) -> u8 {
    let mut level = want;
    while level > 0 && !drawer_level_holds(level) {
        level -= 1;
    }
    level
}

/// The rung this process draws at.
#[inline]
pub fn draw_level() -> u8 {
    draw_level_at(font::user_text_scale())
}

/// The rows a run at `scale` can ink at the rung this process draws at.
#[inline]
pub fn run_rows(scale: usize) -> usize {
    rows_at(scale, draw_level())
}

/// The line box a run at `scale` reserves at the rung this process draws at.
#[inline]
pub fn run_line(scale: usize) -> usize {
    line_at(scale, draw_level())
}

/// The pixels-per-em a run at `scale` is rasterised at, at this process's rung.
#[inline]
pub fn run_em(scale: usize) -> u32 {
    em_at(scale, draw_level())
}

/// The cap height a run at `scale` inks at, at this process's rung.
#[inline]
pub fn run_cap(scale: usize) -> u32 {
    cap_at(scale, draw_level())
}

// -----------------------------------------------------------------------------
// Measurement at a rung
//
// WHY these mirror [`crate::text`] rather than replace it: the shell's measurement
// path and its paint path have to name the same size, and `text` is what every screen
// already calls. Each face below is `text`'s arithmetic with the em read at a level,
// and at [`PLATFORM_LEVEL`] it delegates to `text` itself, byte for byte -- so a run
// measured here and drawn by a painter at the same rung cannot disagree about its
// width.
// -----------------------------------------------------------------------------

/// Advance of one byte at `scale` on `level`.
pub fn advance_at(c: u8, scale: usize, level: u8) -> usize {
    if level == PLATFORM_LEVEL {
        return crate::text::advance(c, scale);
    }
    if scale >= 2 {
        let outline = typeface::text_advance_px(&[c], em_at(scale, level)) as usize;
        if outline > 0 {
            return outline;
        }
    }
    font::advance_of(c, false) * scale
}

/// The advance a run takes at `scale` on `level`.
///
/// WHY the wider of the two faces: a run has two renderers -- the outline the shell
/// places and the bitmap pen it falls back to -- and `crate::text::measure` follows
/// whichever the rung picks. Reserving only one of them is how a name's tail ended up
/// past the cell it was centred in.
pub fn measure_at(text: &[u8], scale: usize, level: u8) -> usize {
    if level == PLATFORM_LEVEL {
        return crate::text::measure(text, scale);
    }
    if scale == 0 {
        return 0;
    }
    let outline = typeface::text_advance_px(text, em_at(scale, level)) as usize;
    let mut pen = 0usize;
    for &c in text {
        pen += advance_at(c, scale, level);
    }
    later(pen, outline)
}

/// The width a run of `text` is painted at on `level`: the measured advance, plus the
/// column a bold stroke dilates.
pub fn painted_width_at(text: &[u8], scale: usize, bold: bool, level: u8) -> usize {
    if level == PLATFORM_LEVEL {
        return crate::text::painted_width(text, scale, bold);
    }
    let measured = measure_at(text, scale, level);
    if scale < 2 {
        return measured;
    }
    measured + usize::from(bold)
}

/// Left edge that centres `text` across `available` at `scale` on `level`.
pub fn center_x_painted_at(
    text: &[u8],
    scale: usize,
    available: usize,
    bold: bool,
    level: u8,
) -> usize {
    let w = painted_width_at(text, scale, bold, level);
    if w >= available {
        0
    } else {
        (available - w) / 2
    }
}

/// [`crate::text::ellipsize`] at `level`: the same budget spent with the advances the
/// level's em carries.
///
/// WHY a restatement and not a call: `text::ellipsize` trims against `text::measure`,
/// which walks the ladder's em. A run trimmed at one rung and inked at another is an
/// ellipsis past the edge of the plate it was fitted to. At the authored level this is
/// `text::ellipsize`, byte for byte.
pub fn ellipsize_at(text: &[u8], scale: usize, max_width: usize, level: u8) -> ([u8; 96], usize) {
    let mut out = [0u8; 96];
    if level == PLATFORM_LEVEL {
        return crate::text::ellipsize(text, scale, max_width);
    }
    if scale == 0 || max_width == 0 {
        return (out, 0);
    }
    if measure_at(text, scale, level) <= max_width {
        let n = text.len().min(out.len());
        out[..n].copy_from_slice(&text[..n]);
        return (out, n);
    }
    let dots = measure_at(b"...", scale, level);
    if dots > max_width {
        return (out, 0);
    }
    let budget = max_width - dots;
    let mut keep = 0usize;
    let mut w = 0usize;
    for (i, &c) in text.iter().enumerate() {
        if keep >= out.len() - 3 {
            break;
        }
        let a = advance_at(c, scale, level);
        if w + a > budget {
            break;
        }
        w += a;
        keep = i + 1;
    }
    out[..keep].copy_from_slice(&text[..keep]);
    out[keep..keep + 3].copy_from_slice(b"...");
    (out, keep + 3)
}

/// The top of the line box whose cap box is centred on `center_y`, at `scale` on
/// `level`.
pub fn cap_centred_top_at(scale: usize, center_y: usize, level: u8) -> usize {
    if level == PLATFORM_LEVEL {
        return crate::text::cap_centred_top(scale, center_y);
    }
    if scale < 2 {
        return center_y.saturating_sub(font::CAP_TOP + font::CAP_HEIGHT / 2);
    }
    let cap = cap_at(scale, level) as usize;
    let baseline = center_y + cap / 2;
    baseline.saturating_sub(ascent_at(scale, level))
}

/// WHY swept: the clamp only earns its place if every selectable level answers the
/// panel once the clamp has had it, and the authored level must be untouched by it --
/// otherwise the shell would resize the drawer for a reader who asked for nothing.
const fn draw_level_holds_at_every_level() -> bool {
    if draw_level_at(PLATFORM_LEVEL) != PLATFORM_LEVEL {
        return false;
    }
    let mut want = 0usize;
    while want <= MAX_LEVEL as usize {
        let chosen = draw_level_at(want as u8);
        if chosen > want as u8 {
            return false;
        }
        if !drawer_level_holds(chosen) {
            return false;
        }
        want += 1;
    }
    true
}
const _: () = assert!(draw_level_holds_at_every_level());

// WHY checked here and not in a screen: the derivations above are only a faithful
// restatement of the ladder if every rung answers the platform's own number at the
// platform's level. Each of these is the identity that lets the shell adopt the
// setting and still draw, at Normal, the pixels it drew before it read the setting.
const _: () = assert!(cap_at(NAME_SCALE, PLATFORM_LEVEL) == font::cap_for_scale(NAME_SCALE));
const _: () = assert!(line_at(NAME_SCALE, PLATFORM_LEVEL) == font::line_height(NAME_SCALE));
const _: () = assert!(ascent_at(NAME_SCALE, PLATFORM_LEVEL) == font::ascent_for_scale(NAME_SCALE));
const _: () = assert!(em_at(NAME_SCALE, PLATFORM_LEVEL) == outline_text::em_for_scale(NAME_SCALE));
const _: () = assert!(rows_at(NAME_SCALE, PLATFORM_LEVEL) == outline_text::row_band(0, NAME_SCALE).1);
const _: () = assert!(grown(44, NAME_SCALE, PLATFORM_LEVEL) == 44);
const _: () = assert!(plate_at(44, NAME_SCALE, PLATFORM_LEVEL) == 44);
const _: () = assert!(DRAWER_CELL_H == (DRAWER_GRID_H - (DRAWER_ROWS - 1) * GRID_20) / DRAWER_ROWS);

/// WHY swept two ways: a plate that grew at a rung the ink did not grow to, or an ink
/// that grew past a plate that did not, is the same defect seen from either side. The
/// walk therefore has to be forwards in the level for both faces at once.
const fn boxes_grow_with_the_rung() -> bool {
    let mut level = 1usize;
    while level <= MAX_LEVEL as usize {
        let l = level as u8;
        if rows_at(NAME_SCALE, l) < rows_at(NAME_SCALE, l - 1) {
            return false;
        }
        if line_at(NAME_SCALE, l) < line_at(NAME_SCALE, PLATFORM_LEVEL) {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(boxes_grow_with_the_rung());
