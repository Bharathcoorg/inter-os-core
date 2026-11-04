// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The global search surface: one list, three sources, honest verdicts.
//!
//! WHY this surface exists: the home surface has a search field that says it
//! searches "apps, settings & files", and it did not. Tapping it opened a
//! launcher-drawn copy of the Settings app's own page -- a second surface with
//! a second name for the same thing -- and there was no search anywhere in the
//! shell. This is the search that field promised.
//!
//! WHY one list and not three: the user types one thing. A screen with an app
//! list, a settings list and a file list side by side is three answers to one
//! question, and the user cannot tell which of them the query reached. So
//! every result carries a chip saying what it is, and the three sources each
//! report below the list whether they matched, found nothing, or could not be
//! read at all.

use crate::LauncherState;
use launcher::icons::{self, IconKind};
use launcher::layout::Rect;
use launcher::search::{CategoryStatus, ListRows, SearchKind};
use launcher::text::Label;
use launcher::theme::{radius, space, type_scale, Theme, STATUS_BAR_HEIGHT};

/// The theme the surface is drawn with.
const THEME: Theme = launcher::theme::SHELL;

/// The bar's three marks, in the one ink this flat surface has.
///
/// WHY an array and not a colour: the home surface samples each mark out of its
/// own wallpaper, so the shared painter takes one ink per control. Here the panel
/// is flat, so all three slots carry the same ink -- and the geometry behind each
/// slot is still [`launcher::nav`]'s, not this file's.
const NAV_INK: [u32; launcher::nav::MARK_SLOTS] = [THEME.on_surface_variant; launcher::nav::MARK_SLOTS];

/// How many results the list shows.
///
/// WHY read from the list model and not from the layout directly: the model sizes
/// the rows it can hand back by this number, and a painter that inked more slots
/// than the list holds would be back to answering taps on rows it never drew.
const MAX_ROWS: usize = launcher::search::MAX_LIST_ROWS;

/// The query field and the box the hint is centred in.
///
/// WHY these are read from `launcher::layout` and not typed here: the rows a
/// keystroke repaints ([`query_band`]) are computed from the same rectangles, and
/// `apps/launcher/src/tests.rs` checks the two against each other. A copy in this
/// file would be a number no host test could tie to the band.
///
/// WHY one row's height is not among them: the row's rectangle is the list model's
/// to hand out ([`launcher::search::row_rect`]), because the painter and the tap
/// path have to be told the same geometry about the same slot.
const FIELD: Rect = launcher::layout::SEARCH_FIELD;
const FIELD_X: usize = FIELD.x;
const FIELD_Y: usize = FIELD.y;
const FIELD_W: usize = FIELD.w;
const FIELD_H: usize = FIELD.h;

/// The rows the query field's text reserves on the outline face at its rung.
///
/// WHY the ladder and not `text::line_height`: the query and its placeholder
/// draw with the outline face, whose line box at the BODY rung is
/// `launcher::font::line_height` (39 rows), not the bitmap cell's 48. Centring the
/// taller bitmap box inside the 64-row field left the ink high, the gap below twice
/// the gap above.
const FIELD_TEXT_BOX_H: usize = launcher::font::line_height(type_scale::BODY);
const _: () = assert!(
    FIELD_TEXT_BOX_H <= FIELD_H,
    "the query field's label box must fit inside the field it is centred in",
);

/// The card the per-source verdicts are drawn in.
const STATUS_CARD: Rect = launcher::layout::SEARCH_STATUS_CARD;
const STATUS_X: usize = STATUS_CARD.x;

/// One result row, resolved.
#[derive(Clone, Copy)]
struct RowPlan {
    rect: Rect,
    icon: Option<IconKind>,
    /// The acronym drawn when the package has no pictogram of its own.
    badge: [u8; 4],
    badge_len: usize,
    title: Label,
    subtitle: Label,
    chip: Label,
}

/// One source's verdict, resolved.
#[derive(Clone, Copy)]
struct StatusPlan {
    name: Label,
    verdict: Label,
}

/// The frame's plan.
#[derive(Clone, Copy)]
struct Plan {
    clock: Label,
    field: Rect,
    query: Label,
    placeholder: Label,
    headline: Label,
    rows: [RowPlan; MAX_ROWS],
    row_count: usize,
    status_card: Rect,
    status: [StatusPlan; 4],
    hint: Label,
    /// The state this plan was built from.
    cached_query_len: usize,
    /// The needle's bytes, not only their count.
    ///
    /// WHY both: the cache is read on a banded repaint, which never passes through
    /// `(0, 0)`, and a length alone lets `browel` and `browser` share a plan -- the
    /// rows under the field would then keep inking the previous needle's answer
    /// while the headline counts the new one. The drawer already carries its query
    /// bytes for this reason.
    cached_query: [u8; 32],
    cached_result_count: usize,
    cached_index_len: usize,
}

/// The frame's plan, built once per frame.
///
/// WHY `(0, 0)` is the boundary: the render loop walks the surface in raster
/// order from the top-left pixel, so `(0, 0)` is the first pixel of every
/// frame and the only pixel within a frame that repeats it. `LauncherState` is
/// borrowed immutably for the whole frame, so the plan can never describe a
/// state the frame was not drawn from.
static mut PLAN: Option<Plan> = None;

fn plan(x: usize, y: usize, state: &LauncherState) -> &'static Plan {
    let slot: *mut Option<Plan> = core::ptr::addr_of_mut!(PLAN);
    // SAFETY: the launcher is one bare-metal task and the paint loop is not
    // preemptible or reentrant, so this is the only reference to the slot at any
    // time, and the reference handed back cannot outlive the call that made it.
    let should_rebuild = unsafe {
        match (*slot).as_ref() {
            None => true,
            Some(p) => {
                // WHY the fingerprint and not only the top-left pixel: a banded
                // repaint never passes through (0, 0), so a plan built for the
                // last query would keep being drawn for the new one.
                (x == 0 && y == 0)
                    || p.cached_query_len != state.search_query_len
                    || p.cached_query != state.search_query
                    || p.cached_result_count != state.search_results.count
                    || p.cached_index_len != state.search_index.len()
            }
        }
    };
    if should_rebuild {
        // SAFETY: as above; this is the one mutation of the slot.
        unsafe { *slot = Some(build(state)) };
    }
    // SAFETY: the slot is `Some` here -- it was just filled when it was empty.
    unsafe { (*slot).as_ref() }.expect("WHY: the slot is filled when it is empty")
}

fn build(state: &LauncherState) -> Plan {
    let theme = THEME;
    let mut plan = Plan {
        clock: Label::at(
            crate::screens::status::CLOCK_X,
            crate::screens::status::CLOCK_Y,
            crate::screens::status::CLOCK_SCALE,
            if state.clock_source_ok {
                &state.clock_time
            } else {
                b""
            },
            theme.on_surface,
        ),
        field: FIELD,
        query: Label::at(0, 0, type_scale::BODY, b"", theme.on_surface),
        placeholder: Label::at(0, 0, type_scale::BODY, b"", theme.on_surface_variant),
        headline: Label::at(0, 0, type_scale::BODY, b"", theme.on_surface),
        rows: [RowPlan {
            rect: Rect::new(0, 0, 0, 0),
            icon: None,
            badge: [0; 4],
            badge_len: 0,
            title: Label::EMPTY,
            subtitle: Label::EMPTY,
            chip: Label::EMPTY,
        }; MAX_ROWS],
        row_count: 0,
        status_card: STATUS_CARD,
        status: [
            StatusPlan {
                name: Label::EMPTY,
                verdict: Label::EMPTY,
            },
            StatusPlan {
                name: Label::EMPTY,
                verdict: Label::EMPTY,
            },
            StatusPlan {
                name: Label::EMPTY,
                verdict: Label::EMPTY,
            },
            StatusPlan {
                name: Label::EMPTY,
                verdict: Label::EMPTY,
            },
        ],
        hint: Label::EMPTY,
        cached_query_len: state.search_query_len,
        cached_query: state.search_query,
        cached_result_count: state.search_results.count,
        cached_index_len: state.search_index.len(),
    };

    // The query field's text: the needle, or the hint that says what the field is
    // for while nobody has typed one.
    //
    // WHY the hint comes from `launcher::search::field_hint`: the rule that keeps a
    // hint a hint -- no needle, and inside the field only -- is one rule, and the
    // surface cannot enforce it from a copy of the literal. Once a needle exists the
    // model answers `None` and the field's only ink is what was typed.
    let field_text_y = FIELD_Y + (FIELD_H - FIELD_TEXT_BOX_H) / 2;
    match launcher::search::field_hint(state.search_query_len) {
        None => {
            plan.query = Label::at(
                FIELD_X + 56,
                field_text_y,
                type_scale::BODY,
                &state.search_query[..state.search_query_len],
                theme.on_surface,
            );
        }
        Some(hint) => {
            plan.placeholder = Label::at(
                FIELD_X + 56,
                field_text_y,
                type_scale::BODY,
                hint,
                theme.on_surface_variant,
            );
        }
    }

    // The headline: how many results the query found.
    //
    // WHY an empty query does not read "No results": the per-source verdicts
    // below already refuse to report nothing-found for a query nobody typed,
    // and the headline would contradict them by claiming a search ran and came
    // back empty the moment the surface opens.
    let count = state.search_results.count;
    let headline = if state.search_query_len == 0 {
        b"Type to search".as_slice()
    } else if count == 0 {
        b"No results".as_slice()
    } else {
        b"Results".as_slice()
    };
    plan.headline = Label::at(
        FIELD_X,
        launcher::layout::SEARCH_HEADLINE_TOP,
        type_scale::BODY,
        headline,
        theme.on_surface,
    );

    // The result rows: every row the list holds for this answer, and nothing else.
    //
    // WHY the list is built by `launcher::search::ListRows` and walked here: the
    // rows painted, the count the tap path resolves and the slots the band repaints
    // are one question -- which results does this answer actually hold -- and the
    // surface used to answer it twice, once from the ranking's count and once from
    // the rows it had entries for. Whatever the two answers disagreed about was
    // inked, or tapped, over dead space.
    let rows = ListRows::build(&state.search_results, &state.search_index);
    let mut row = 0usize;
    while row < rows.len() {
        let Some(list_row) = rows.get(row) else {
            break;
        };
        let Some(entry) = state.search_index.get(list_row.entry) else {
            // Unreachable: the list only holds rows whose entry resolves. The slot
            // is left un-inked rather than filled from anything else, so a row that
            // cannot be named paints the panel and not a guess.
            row += 1;
            continue;
        };
        let y = launcher::layout::search_row_top(row);
        let mut badge = [0u8; 4];
        let mut badge_len = 0usize;
        let icon = match entry.kind {
            SearchKind::App => icons::match_icon_kind(entry.target()),
            SearchKind::Settings => Some(IconKind::Settings),
            SearchKind::File => Some(IconKind::Files),
        };
        if icon.is_none() {
            // WHY an acronym: a third-party package has no pictogram of its
            // own, and a blank square in the result list would read as a
            // rendering fault rather than as "this app draws its own icon".
            let title = entry.title();
            let mut at = 0usize;
            let mut in_word = false;
            for &c in title.iter() {
                if at >= 4 {
                    break;
                }
                let is_sep = c == b' ' || c == b'-' || c == b'_' || c == b'.';
                if !is_sep && !in_word {
                    badge[at] = c.to_ascii_uppercase();
                    at += 1;
                }
                in_word = !is_sep;
            }
            badge_len = at;
        }
        let chip_text = entry.kind.label();
        let chip_w = launcher::text::measure(chip_text, type_scale::CAPTION);
        let chip_x = FIELD_X + FIELD_W - 16 - chip_w;
        // WHY the title is fitted to the column: a package's display name can
        // be longer than the row, and a label that runs under the chip is two
        // labels on top of each other.
        let title_column = chip_x.saturating_sub(FIELD_X + 88 + 12);
        let (title, title_len) =
            launcher::text::ellipsize(entry.title(), type_scale::BODY, title_column);
        let (subtitle, subtitle_len) =
            launcher::text::ellipsize(entry.subtitle(), type_scale::CAPTION, title_column);
        plan.rows[row] = RowPlan {
            rect: result_rect(row),
            icon,
            badge,
            badge_len,
            title: Label::at(
                FIELD_X + 88,
                y + 18,
                type_scale::BODY,
                &title[..title_len],
                theme.on_surface,
            ),
            subtitle: Label::at(
                FIELD_X + 88,
                y + 52,
                type_scale::CAPTION,
                &subtitle[..subtitle_len],
                theme.on_surface_variant,
            ),
            chip: Label::at(
                chip_x,
                y + 30,
                type_scale::CAPTION,
                chip_text,
                theme.on_surface_variant,
            ),
        };
        row += 1;
    }
    plan.row_count = row;

    // The per-source verdicts.
    //
    // WHY an empty query says "type to search" rather than "nothing matches":
    // an empty query has not searched anything, and a category that says it
    // found nothing when nothing was asked of it is a category that is
    // answering a question nobody asked. Categories that could not be read
    // or whose service/weights are absent report their unavailable reason honestly.
    let waiting = state.search_query_len == 0;
    plan.status[0] = verdict_plan(
        b"Apps",
        state.search_results.apps,
        STATUS_X + space::MD,
        launcher::layout::search_status_line_top(0),
        &theme,
        waiting,
    );
    plan.status[1] = verdict_plan(
        b"Settings",
        state.search_results.settings,
        STATUS_X + space::MD,
        launcher::layout::search_status_line_top(1),
        &theme,
        waiting,
    );
    plan.status[2] = verdict_plan(
        b"Files",
        state.search_results.files,
        STATUS_X + space::MD,
        launcher::layout::search_status_line_top(2),
        &theme,
        waiting,
    );
    plan.status[3] = verdict_plan(
        b"AI",
        state.search_results.ai,
        STATUS_X + space::MD,
        launcher::layout::search_status_line_top(3),
        &theme,
        waiting,
    );

    // The line under the verdict card. It reads "Tap a result to open it. Backspace
    // edits the query." -- a promise about a row to tap and a query to edit -- so it
    // is inked only while the list actually holds both.
    //
    // WHY `Label::EMPTY` otherwise: this is the empty state, and the state the
    // surface is in is the state the surface says. Writing a different sentence for
    // the same region would be the same mistake in another font: an example of what
    // a list could do, standing in the space the answer left empty. The headline
    // above the rows already says the one thing that is true here -- "Type to search"
    // while nobody has typed, "No results" when a needle brought nothing back.
    //
    // WHY the repaint band still covers the box (see `query_band`): the line is gone
    // only if the rows it used to sit on are re-inked; a band that stopped above it
    // would leave the old promise standing there.
    plan.hint = if launcher::search::foot_line_applies(state.search_query_len, rows.len()) {
        launcher::layout::search_hint_label(theme.on_surface_variant)
    } else {
        Label::EMPTY
    };

    plan
}

/// One source's verdict line, resolved.
fn verdict_plan(
    name: &[u8],
    status: CategoryStatus,
    x: usize,
    y: usize,
    theme: &Theme,
    waiting: bool,
) -> StatusPlan {
    let mut verdict = [0u8; 72];
    let (len, ink) = match status {
        CategoryStatus::Unavailable(reason) => {
            let text = b"unavailable: ";
            verdict[..text.len()].copy_from_slice(text);
            let n = text.len() + reason_text(reason, &mut verdict[text.len()..]);
            (n, theme.error)
        }
        _ if waiting => {
            let text = b"type to search";
            verdict[..text.len()].copy_from_slice(text);
            (text.len(), theme.on_surface_variant)
        }
        CategoryStatus::Matched(count) => {
            let n = count_text(count, &mut verdict);
            (n, theme.on_surface)
        }
        CategoryStatus::NoMatch => {
            let text = b"nothing matches";
            verdict[..text.len()].copy_from_slice(text);
            (text.len(), theme.on_surface_variant)
        }
    };
    // WHY the verdict is placed after the name's own width: the three
    // category names are different lengths, and a fixed column puts the
    // longest one under the verdict beside it.
    let name_w = launcher::text::measure(name, type_scale::BODY);
    StatusPlan {
        name: Label::at(x, y, type_scale::BODY, name, theme.on_surface),
        verdict: Label::at(
            x + name_w + space::MD,
            y + 4,
            type_scale::CAPTION,
            &verdict[..len],
            ink,
        ),
    }
}

/// Writes `count` as a number of matches.
fn count_text(count: usize, buf: &mut [u8]) -> usize {
    let mut digits = [0u8; 20];
    let mut n = 0usize;
    let mut v = count;
    while v > 0 && n < digits.len() {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let mut at = 0usize;
    for i in (0..n).rev() {
        if at + 1 < buf.len() {
            buf[at] = digits[i];
            at += 1;
        }
    }
    let tail: &[u8] = if count == 1 { b" match" } else { b" matches" };
    let tail_len = tail.len().min(buf.len().saturating_sub(at));
    buf[at..at + tail_len].copy_from_slice(&tail[..tail_len]);
    at + tail_len
}

/// Copies a reason into a buffer.
fn reason_text(reason: &str, buf: &mut [u8]) -> usize {
    let bytes = reason.as_bytes();
    let len = bytes.len().min(buf.len());
    buf[..len].copy_from_slice(&bytes[..len]);
    len
}

/// The row of the result at `slot`.
///
/// WHY the shell hit-tests with this and not with its own numbers: the row the
/// painter drew and the row the tap resolves have to be the same row, and
/// there is only one place that knows where a row is -- `launcher::search`, which
/// both this surface and [`result_at`] read.
pub fn result_rect(slot: usize) -> Rect {
    launcher::search::row_rect(slot)
}

/// The rank of the result at (`x`, `y`) among the rows this surface would ink for
/// the state in front of it.
///
/// WHY the list and not the ranking's count: the shell used to hit-test
/// `0..state.search_results.count` slots while the painter stopped at the rows it
/// could name, so a tap below the last inked row resolved a rank the person had
/// never seen and opened a result that was not on the panel. Both halves now read
/// the one list, and a slot the list does not hold answers nothing.
pub fn result_at(x: usize, y: usize, state: &LauncherState) -> Option<usize> {
    ListRows::build(&state.search_results, &state.search_index).row_at(x, y)
}

/// Rows a keystroke in the query field can move.
///
/// WHY an accessor here and not a band typed in the shell: the field's text, the
/// headline counting the results, the result rows and the per-source verdict
/// card are the four things `build` resolves from the query, and the shell has to
/// repaint what they claim. The rectangles themselves live in
/// [`launcher::layout`], because the band's bottom is the hint line's own claimed
/// box and a host gate reads that number -- nothing under `screens/` is reachable
/// from `cargo test`, so geometry a test has to see has to live in the library.
///
/// WHY the hint line is INSIDE it, against the note that used to sit here: the old
/// band stopped at the verdict card's edge, `STATUS_Y + STATUS_H` = row 1,068, while
/// the hint this screen plans claims rows 1,092..1,122 as its own box -- and
/// [`launcher::text::Label::pixel`] ink nothing outside the box it claims. Every
/// `Dirty::SearchQuery` frame therefore re-inked the field, the rows and the card
/// and never once visited the hint's lower 30 rows, so whatever those rows held from
/// the last full-surface paint is what stayed on the panel. That is the whole
/// "descenders went missing on the search screen" report, and the band was the wrong
/// side of the arithmetic, not the clip: `apps/launcher/src/tests.rs` now pins
/// `query_band()` against every box `build` plans.
///
/// WHY the status bar and the navigation band stay outside it: the clock in the bar
/// is a reading this surface did not change, and the bar below row
/// [`launcher::nav::BAND_TOP`] is painted from `Dirty::NavBar`, which is its own
/// 60 rows. A keystroke may not re-rasterise either -- this band is 1,046 rows, not
/// the 1,440-row walk that `3d4af947` removed.
pub fn query_band() -> (usize, usize) {
    launcher::layout::search_content_band()
}

/// The colour of (`x`, `y`) on the search surface.
pub fn get_search_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let p = plan(x, y, state);
    let theme = THEME;

    if y < STATUS_BAR_HEIGHT {
        return crate::screens::status::pixel(x, y, Some(&p.clock), &theme);
    }

    // The query field.
    if inside_rounded(p.field, radius::CONTROL, x, y) {
        let fill = theme.surface_container_high;
        if !inside_rounded(p.field.inset(1), radius::CONTROL.saturating_sub(1), x, y) {
            return theme.border;
        }
        // The magnifier.
        if let Some(c) = magnifier(x, y, p.field.x + 30, p.field.y + 32, theme.on_surface_variant) {
            return c;
        }
        if let Some(c) = p.query.pixel(x, y, fill) {
            return c;
        }
        if let Some(c) = p.placeholder.pixel(x, y, fill) {
            return c;
        }
        return fill;
    }

    if let Some(c) = p.headline.pixel(x, y, theme.surface) {
        return c;
    }

    // The result rows.
    for index in 0..p.row_count {
        let row = &p.rows[index];
        if !row.rect.contains(x, y) {
            continue;
        }
        let fill = theme.surface_container;
        if !inside_rounded(row.rect.inset(1), radius::CARD.saturating_sub(1), x, y) {
            return theme.border;
        }
        if let Some(c) = row_icon(x, y, row) {
            return c;
        }
        if let Some(c) = row.chip.pixel(x, y, fill) {
            return c;
        }
        if let Some(c) = row.title.pixel(x, y, fill) {
            return c;
        }
        if let Some(c) = row.subtitle.pixel(x, y, fill) {
            return c;
        }
        return fill;
    }

    // The per-source verdicts.
    if inside_rounded(p.status_card, radius::CARD, x, y) {
        let fill = theme.surface_container;
        if !inside_rounded(p.status_card.inset(1), radius::CARD.saturating_sub(1), x, y) {
            return theme.border;
        }
        for line in p.status.iter() {
            if let Some(c) = line.name.pixel(x, y, fill) {
                return c;
            }
            if let Some(c) = line.verdict.pixel(x, y, fill) {
                return c;
            }
        }
        return fill;
    }

    if let Some(c) = p.hint.pixel(x, y, theme.surface) {
        return c;
    }

    // The bottom bar's three controls.
    //
    // WHY this surface inks them now: the shell answers a tap here from
    // `launcher::nav::target_at`, and it used to answer the middle third only,
    // from a platform box this screen never drew -- so a person typing a query saw
    // an empty strip of panel and had to aim at nothing. Paint and tap are the same
    // two calls on the same geometry as on every other surface with a bar.
    if let Some(c) = launcher::nav::mark_ink(x, y, launcher::nav::Offered::PANEL, NAV_INK) {
        return c;
    }

    theme.surface
}

/// The row's pictogram, or its acronym.
fn row_icon(x: usize, y: usize, row: &RowPlan) -> Option<u32> {
    let cx = row.rect.x + 16 + 20;
    let cy = row.rect.y + 20 + 20;
    if let Some(kind) = row.icon {
        return icons::rasterize_icon_pixel_scaled(kind, x, y, cx, cy, THEME.on_surface, 20);
    }
    if row.badge_len == 0 {
        return None;
    }
    // The acronym, drawn in the middle of the icon box.
    let scale = type_scale::CAPTION;
    let w = launcher::text::measure(&row.badge[..row.badge_len], scale);
    let h = launcher::text::line_height(scale);
    let x0 = cx.saturating_sub(w / 2);
    let y0 = cy.saturating_sub(h / 2);
    crate::test_text_aa(
        x,
        y,
        x0,
        y0,
        scale,
        &row.badge[..row.badge_len],
        THEME.on_surface,
        THEME.surface_container,
    )
}

/// The magnifier in the query field: a closed ring and one handle.
///
/// WHY the ring is every quadrant and not one: this test also asked for
/// `dx <= 0 && dy <= 0`, so three quarters of the lens were never painted and
/// the pictogram read as a stray arc sitting over a diamond. The handle was a
/// `|dx-8| + |dy-8| <= 10` band, which is a filled lozenge rather than a stem.
/// The home screen's field draws a closed ring over a narrow diagonal handle,
/// and the two fields open onto the same query, so they have to carry the same
/// pictogram -- an icon that changes shape between screens reads as a
/// rendering fault, not as a style choice.
fn magnifier(x: usize, y: usize, cx: usize, cy: usize, ink: u32) -> Option<u32> {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    let d2 = dx * dx + dy * dy;
    if (64..=144).contains(&d2) {
        return Some(ink);
    }
    if (9..=16).contains(&dx) && dy >= 9 && (dx - dy).abs() <= 2 {
        return Some(ink);
    }
    None
}

/// Whether (`x`, `y`) is inside a rounded rectangle.
fn inside_rounded(rect: Rect, radius: usize, x: usize, y: usize) -> bool {
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
