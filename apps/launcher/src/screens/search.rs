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
use launcher::search::{CategoryStatus, SearchKind};
use launcher::text::Label;
use launcher::theme::{radius, space, type_scale, Theme, STATUS_BAR_HEIGHT};

/// The theme the surface is drawn with.
const THEME: Theme = launcher::theme::SHELL;

/// How many results the list shows.
const MAX_ROWS: usize = 6;

/// The query field.
const FIELD_X: usize = 32;
const FIELD_Y: usize = 76;
const FIELD_W: usize = 656;
const FIELD_H: usize = 64;

/// Where the result list starts.
const ROWS_Y: usize = 216;
/// The height of one result row.
const ROW_H: usize = 96;

/// The card the per-source verdicts are drawn in.
const STATUS_X: usize = 32;
const STATUS_Y: usize = 812;
const STATUS_W: usize = 656;
const STATUS_H: usize = 256;
/// The height of one verdict line.
const STATUS_LINE_H: usize = 56;

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
        field: Rect::new(FIELD_X, FIELD_Y, FIELD_W, FIELD_H),
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
        status_card: Rect::new(STATUS_X, STATUS_Y, STATUS_W, STATUS_H),
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
        cached_result_count: state.search_results.count,
        cached_index_len: state.search_index.len(),
    };

    // The query field's text.
    let field_text_y = FIELD_Y + (FIELD_H - launcher::text::line_height(type_scale::BODY)) / 2;
    if state.search_query_len > 0 {
        plan.query = Label::at(
            FIELD_X + 56,
            field_text_y,
            type_scale::BODY,
            &state.search_query[..state.search_query_len],
            theme.on_surface,
        );
    } else {
        plan.placeholder = Label::at(
            FIELD_X + 56,
            field_text_y,
            type_scale::BODY,
            b"Search apps, settings and files",
            theme.on_surface_variant,
        );
    }

    // The headline: how many results the query found.
    let count = state.search_results.count;
    let headline = if count == 0 {
        b"No results".as_slice()
    } else {
        b"Results".as_slice()
    };
    plan.headline = Label::at(FIELD_X, 168, type_scale::BODY, headline, theme.on_surface);

    // The result rows.
    let mut row = 0usize;
    while row < MAX_ROWS {
        let Some(hit) = state.search_results.hit(row) else {
            break;
        };
        let Some(entry) = state.search_index.get(hit.index as usize) else {
            break;
        };
        let y = ROWS_Y + row * ROW_H;
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
        STATUS_Y + space::MD,
        &theme,
        waiting,
    );
    plan.status[1] = verdict_plan(
        b"Settings",
        state.search_results.settings,
        STATUS_X + space::MD,
        STATUS_Y + space::MD + STATUS_LINE_H,
        &theme,
        waiting,
    );
    plan.status[2] = verdict_plan(
        b"Files",
        state.search_results.files,
        STATUS_X + space::MD,
        STATUS_Y + space::MD + STATUS_LINE_H * 2,
        &theme,
        waiting,
    );
    plan.status[3] = verdict_plan(
        b"AI",
        state.search_results.ai,
        STATUS_X + space::MD,
        STATUS_Y + space::MD + STATUS_LINE_H * 3,
        &theme,
        waiting,
    );

    plan.hint = Label::at(
        FIELD_X,
        STATUS_Y + STATUS_H + space::LG,
        type_scale::CAPTION,
        b"Tap a result to open it. Backspace edits the query.",
        theme.on_surface_variant,
    );

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

/// The row of the result at `rank`.
///
/// WHY the shell hit-tests with this and not with its own numbers: the row the
/// painter drew and the row the tap resolves have to be the same row, and
/// there is only one place that knows where a row is.
pub fn result_rect(rank: usize) -> Rect {
    Rect::new(FIELD_X, ROWS_Y + rank * ROW_H, FIELD_W, ROW_H - 8)
}

/// The result at (`x`, `y`).
pub fn result_at(x: usize, y: usize, count: usize) -> Option<usize> {
    for rank in 0..count.min(MAX_ROWS) {
        if result_rect(rank).contains(x, y) {
            return Some(rank);
        }
    }
    None
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

/// The magnifier in the query field.
fn magnifier(x: usize, y: usize, cx: usize, cy: usize, ink: u32) -> Option<u32> {
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    let d2 = dx * dx + dy * dy;
    if d2 >= 64 && d2 <= 144 && dx <= 0 && dy <= 0 {
        return Some(ink);
    }
    if dx >= 0 && dx <= 8 && dy >= 0 && dy <= 8 && (dx - 8).abs() + (dy - 8).abs() <= 10 {
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
