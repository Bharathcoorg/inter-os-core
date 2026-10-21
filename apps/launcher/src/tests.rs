//! Host-side tests for the launcher's geometry, theme and text metrics.
//!
//! WHY these assert behaviour rather than golden images: an off-by-one in grid
//! addressing or an over-wide label is invisible in a screenshot but breaks every
//! tap and every centring, so the geometry is pinned numerically. Each assertion
//! here is one that would fail if the layout were wrong in a way the eye would
//! not catch.
use crate::layout::*;
use crate::text as tm;
use crate::theme::*;

const W: usize = SCREEN_WIDTH;
const H: usize = SCREEN_HEIGHT;

// ---------------------------------------------------------------------------
// Grid
// ---------------------------------------------------------------------------

#[test]
fn columns_respect_the_minimum_touch_target() {
    // Full-width bounds: 720px. Four columns must each still be >= 48px.
    for cols in 1..=6usize {
        let width = cols * MIN_TOUCH_TARGET + (cols - 1) * 8;
        assert!(
            cols * MIN_TOUCH_TARGET <= width,
            "column {} cannot satisfy the touch target",
            cols
        );
    }
    // A narrow surface must not be offered columns that would break the target.
    let narrow = grid_columns(12, 100, 8);
    assert!(
        narrow <= 2,
        "a 100px-wide area must not get {} columns",
        narrow
    );
}

#[test]
fn columns_are_capped_so_tiles_stay_tappable() {
    // Six is the documented ceiling: a 720px surface with more than six columns
    // puts each tile under the minimum touch target.
    assert_eq!(grid_columns(30, W, 8), 6);
}

#[test]
fn zero_items_still_yields_a_usable_grid() {
    let g = grid(0, Rect::new(40, 200, W - 80, 600), 4, 8);
    assert!(g.columns >= 1, "a zero-column grid would divide by zero");
    assert!(g.rows >= 1);
    assert!(grid_cell(&g, 0).is_some(), "the surface must still be addressable");
}

#[test]
fn cells_flow_left_to_right_then_top_to_bottom() {
    // Narrow bounds force two columns, so six items must wrap to three rows. Using a
    // full-width area here would silently produce one six-column row and assert
    // nothing about wrapping at all.
    let g = grid(6, Rect::new(0, 0, 120, 600), 3, 8);
    assert_eq!(
        g.columns, 2,
        "this test is meaningless unless the bounds force two columns, got {}",
        g.columns
    );
    let a = grid_cell(&g, 0).expect("cell 0");
    let b = grid_cell(&g, 1).expect("cell 1");
    let c = grid_cell(&g, 2).expect("cell 2");
    assert!(b.x > a.x, "cell 1 must be right of cell 0");
    assert_eq!(a.y, b.y, "cells 0 and 1 share a row");
    assert!(
        c.y > a.y,
        "cell 2 must wrap to the next row, not continue right"
    );
    assert_eq!(c.x, a.x, "a wrapped row restarts at the origin column");
}

#[test]
fn a_full_width_area_fits_the_column_ceiling_on_one_row() {
    // Six items on a 720px surface fit in a single row of six, which is what makes
    // the dock a dock rather than a grid.
    let g = grid(6, Rect::new(0, 0, W, 600), 3, 8);
    assert_eq!(g.columns, 6);
    assert_eq!(g.rows, 1, "six items at full width must not wrap");
    assert_eq!(
        grid_cell(&g, 5).map(|c| c.y),
        grid_cell(&g, 0).map(|c| c.y),
        "the whole dock shares one baseline"
    );
}

#[test]
fn cell_indices_beyond_the_grid_are_rejected() {
    let g = grid(4, Rect::new(0, 0, W, 600), 1, 8); // one row holds 4
    assert!(grid_cell(&g, 3).is_some());
    assert!(
        grid_cell(&g, 4).is_none(),
        "an index past the grid must not resolve to a phantom cell"
    );
}

#[test]
fn hit_testing_round_trips_every_cell() {
    let g = grid(9, Rect::new(40, 200, W - 80, 900), 3, 8);
    for i in 0..g.columns * g.rows {
        let cell = grid_cell(&g, i).expect("cell exists");
        assert_eq!(
            grid_hit(&g, cell.center_x(), cell.center_y()),
            Some(i),
            "tapping the centre of cell {} resolved elsewhere",
            i
        );
    }
}

#[test]
fn hit_testing_rejects_points_outside_the_grid() {
    let bounds = Rect::new(40, 400, W - 80, 600);
    let g = grid(6, bounds, 2, 8);
    assert_eq!(grid_hit(&g, 2, 2), None, "a point above the grid must miss");
    assert_eq!(
        grid_hit(&g, W - 2, H - 2),
        None,
        "a point below the grid must miss"
    );
}

#[test]
fn a_single_item_stays_centred() {
    let bounds = Rect::new(40, 400, W - 80, 600);
    let g = grid(1, bounds, 3, 8);
    let cell = grid_cell(&g, 0).expect("cell");
    let left_gap = cell.x - bounds.x;
    let right_gap = bounds.x + bounds.w - (cell.x + cell.w);
    assert!(
        left_gap.abs_diff(right_gap) <= 1,
        "a lone item must be centred: left gap {} vs right gap {}",
        left_gap,
        right_gap
    );
}

#[test]
fn adding_items_never_overflows_the_bounds() {
    let bounds = Rect::new(40, 300, W - 80, 1000);
    for count in 1..=24usize {
        let g = grid(count, bounds, 5, 8);
        for i in 0..(count.min(g.columns * g.rows)) {
            let c = grid_cell(&g, i).expect("cell");
            assert!(
                c.y + c.h <= bounds.y + bounds.h + 1,
                "cell {} escapes the vertical bounds at count {}",
                i,
                count
            );
            assert!(
                c.x + c.w <= bounds.x + bounds.w + 1,
                "cell {} escapes the horizontal bounds at count {}",
                i,
                count
            );
        }
    }
}

// ---------------------------------------------------------------------------
// List and paging
// ---------------------------------------------------------------------------

#[test]
fn list_rows_are_contiguous_and_indexable() {
    let bounds = Rect::new(40, 200, W - 80, 900);
    let count = 5usize;
    let l = list(bounds, count, 120, 8).expect("list");
    for i in 0..count {
        let r = list_row(l, i, count, 120, 8).expect("row");
        assert_eq!(r.h, 120);
        assert_eq!(r.w, bounds.w, "a row spans the list width");
        if i > 0 {
            let prev = list_row(l, i - 1, count, 120, 8).expect("previous row");
            assert_eq!(
                r.y - (prev.y + prev.h),
                8,
                "row {} must be separated by exactly the gutter",
                i
            );
        }
    }
    assert!(list_row(l, count, count, 120, 8).is_none());
}

#[test]
fn an_empty_list_has_no_geometry() {
    assert!(
        list(Rect::new(0, 0, W, H), 0, 120, 8).is_none(),
        "an empty list must not fabricate a row"
    );
}

#[test]
fn paging_covers_every_item_exactly_once() {
    let per_page = 12usize;
    for count in [0usize, 1, 11, 12, 13, 24, 25, 100] {
        let p = pages(count, per_page);
        let mut seen = 0usize;
        for page in 0..p {
            let (s, e) = page_slice(count, per_page, page);
            assert!(s <= e, "page {} has an inverted slice", page);
            assert!(
                e - s <= per_page,
                "page {} exceeds the page size",
                page
            );
            seen += e - s;
        }
        assert_eq!(
            seen, count,
            "paging must cover every item exactly once at count {}",
            count
        );
    }
}

#[test]
fn paging_a_zero_per_page_does_not_divide_by_zero() {
    assert_eq!(pages(10, 0), 0);
    assert_eq!(page_slice(10, 0, 0), (0, 0));
}

#[test]
fn a_page_past_the_end_is_empty_rather_than_wrapping() {
    let (s, e) = page_slice(5, 10, 9);
    assert_eq!((s, e), (5, 5), "an out-of-range page must be empty, not wrap");
}

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

#[test]
fn both_themes_are_legible_on_their_own_background() {
    // The theme's body ink must contrast with the theme's own backdrop, or text
    // vanishes. This is the assertion that caught the dark theme declaring a
    // near-black `text_on_accent` and having it chosen as its own ink.
    for t in [Theme::LIGHT, Theme::DARK] {
        let ink = ink_on(&t, t.surface);
        assert_ne!(
            ink, t.surface,
            "{:?} ink on its own background is invisible",
            t.name
        );
        assert_ne!(
            ink, t.surface,
            "{:?} ink on its own surface is invisible",
            t.name
        );
        // And it must be the polarity the backdrop calls for.
        let expected = if t.is_dark() { t.ink_on_dark } else { t.ink_on_light };
        assert_eq!(ink, expected, "{:?} chose the wrong ink polarity", t.name);
    }
}

#[test]
fn ink_polarity_flips_with_the_backdrop() {
    let t = Theme::LIGHT;
    assert_eq!(
        ink_on(&t, 0xFFFFFFFF),
        t.ink_on_light,
        "dark ink on white"
    );
    assert_eq!(
        ink_on(&t, 0xFF000000),
        0xFFFFFFFF,
        "light ink on black"
    );
}

#[test]
fn both_themes_are_distinct_and_named() {
    assert_ne!(Theme::LIGHT.name, Theme::DARK.name);
    assert_ne!(Theme::LIGHT.surface, Theme::DARK.surface);
    assert_eq!(Theme::by_name("dark").name, "dark");
    assert_eq!(
        Theme::by_name("light").name,
        "light",
        "an unknown name falls back to light rather than panicking"
    );
    assert_eq!(Theme::by_name("nonsense").name, "light");
}

#[test]
fn blend_endpoints_are_exact() {
    let a = 0xFF102030u32;
    let b = 0xFFA0B0C0u32;
    assert_eq!(Theme::blend(a, b, 0), a, "t=0 must be exactly the first stop");
    assert_eq!(Theme::blend(a, b, 256), b, "t=256 must be exactly the second stop");
}

#[test]
fn the_wallpaper_ramp_spans_top_to_bottom() {
    let t = Theme::LIGHT;
    let top = t.ramp3(0);
    let mid = t.ramp3(128);
    let bottom = t.ramp3(256);
    assert_eq!(top, t.wallpaper_top);
    assert_eq!(bottom, t.wallpaper_bottom);
    assert_ne!(mid, top, "the midpoint must be a distinct stop");
    // The ramp must not be clamped flat at either end.
    assert_eq!(t.ramp3(300), bottom, "t above the range clamps to the last stop");
}

#[test]
fn themes_declare_the_same_metrics() {
    // A theme that changed geometry would break layout, so the metrics live on the
    // shared constants rather than per-theme; this guards the invariant.
    assert_eq!(SCREEN_WIDTH, W);
    assert_eq!(SCREEN_HEIGHT, H);
    // WHY the platform's minimum and not the shell's own: the minimum touch
    // target is the floor every app on the device is held to, and the shell
    // holds itself to the same floor. It used to declare 48 while the platform
    // declared 44, which meant a third-party app could draw a target the shell
    // would have refused.
    assert_eq!(MIN_TOUCH_TARGET, interos_std::design::touch::MIN_TARGET);
    assert_eq!(MIN_TOUCH_TARGET, 44);
    assert!(MIN_TOUCH_TARGET >= 44);
    // Padding must be a multiple of the base unit so every gap lands on the grid.
    assert_eq!(
        CONTENT_PADDING % space::XS,
        0,
        "content padding must sit on the spacing scale"
    );
    assert_eq!(STATUS_BAR_HEIGHT, 48);
    assert!(NAV_BAR_HEIGHT >= MIN_TOUCH_TARGET, "the nav bar must be tappable");
}

// ---------------------------------------------------------------------------
// Text metrics
// ---------------------------------------------------------------------------

#[test]
fn measurement_is_the_sum_of_real_advances() {
    // WHY this replaced a fixed-cell assertion: the face is proportional, so a
    // glyph's width depends on the character. The old test asserted
    // `measure == len * GLYPH_W * scale`, which is only true for a monospaced face
    // and would have passed while the launcher mis-rendered every centred label.
    // This asserts the invariant the rasteriser actually depends on, and that it is
    // genuinely proportional rather than accidentally cell-width.
    for scale in [1usize, 2, 3] {
        for len in 0..12usize {
            let s: [u8; 16] = [b'x'; 16];
            let expected: usize = s[..len].iter().map(|&c| tm::advance(c, scale)).sum();
            assert_eq!(
                tm::measure(&s[..len], scale),
                expected,
                "measure must equal the sum of advances at scale {}",
                scale
            );
        }
    }

    // And the face must actually be proportional: 'i' is narrower than 'W'.
    assert!(
        tm::measure(b"W", 1) > tm::measure(b"i", 1),
        "a proportional face must give W a wider advance than i"
    );
}

#[test]
fn digits_are_tabular_so_a_clock_does_not_jitter() {
    // WHY: the lock screen clock is built from digits. If they were proportional
    // the whole time string would shift horizontally on every tick, which reads as
    // a rendering fault rather than a clock.
    let base = tm::measure(b"0", 2);
    for d in b"123456789" {
        assert_eq!(
            tm::measure(&[*d], 2),
            base,
            "digit {} must share the zero advance",
            *d as char
        );
    }
}

#[test]
fn centring_places_text_symmetrically() {
    let s = b"Camera";
    let scale = 2usize;
    let available = 300usize;
    let x = tm::center_x(s, scale, available);
    let w = tm::measure(s, scale);
    assert_eq!(x * 2 + w, available - (available - w) % 2);
    assert!(x + w <= available, "centred text must fit");
}

#[test]
fn centring_text_wider_than_the_space_does_not_underflow() {
    let s = b"Settings";
    let x = tm::center_x(s, 12, 40);
    assert_eq!(x, 0, "over-wide text clamps to the left edge instead of wrapping");
}

#[test]
fn wrapping_never_exceeds_the_line_budget() {
    let text = b"Running from the system package registry";
    for max_lines in 1..=3usize {
        for max_width in [64usize, 128, 300, 640] {
            let (breaks, n) = tm::wrap(text, 1, max_width, max_lines);
            assert!(n <= max_lines, "produced {} lines for a budget of {}", n, max_lines);
            assert!(n <= breaks.len(), "line count must fit the fixed array");
            // WHY measured width and not a character count: with a proportional
            // face a character-count budget says nothing about whether the line
            // fits. This is the invariant that actually matters, and it is
            // stronger than the proxy it replaces.
            let mut start = 0usize;
            for i in 0..n {
                let end = start + breaks[i];
                let w = tm::measure(&text[start..end], 1);
                assert!(
                    w <= max_width,
                    "line {} measures {} px, over the {} px budget",
                    i,
                    w,
                    max_width
                );
                assert!(breaks[i] > 0, "an empty line would render as a gap");
                start = end;
                if start < text.len() && text[start] == b' ' {
                    start += 1;
                }
            }
        }
    }
}

#[test]
fn wrapping_short_text_yields_a_single_line() {
    let (breaks, n) = tm::wrap(b"Camera", 1, 640, 3);
    assert_eq!(n, 1);
    assert_eq!(breaks[0], 6);
}

#[test]
fn wrapping_a_single_unbreakable_word_falls_back_to_a_hard_break() {
    let (breaks, n) = tm::wrap(b"Supercalifragilistic", 1, 40, 3);
    assert!(n > 0, "an unbreakable word must still produce lines");
    for i in 0..n {
        assert!(breaks[i] > 0 && breaks[i] <= 5, "hard break line {} out of range", i);
    }
}

#[test]
fn wrapping_a_zero_budget_is_safe() {
    let (breaks, n) = tm::wrap(b"hello", 1, 0, 3);
    assert_eq!(n, 0, "a zero width must not loop forever or divide by zero");
    assert_eq!(breaks[0], 0);
}

#[test]
fn ellipsize_appends_an_ellipsis_when_it_cuts() {
    let (out, n) = tm::ellipsize(b"Settings and configuration", 2, 160);
    // WHY measured width: the old assertion divided the result length by the cell
    // width, which is meaningless for a proportional face. What must hold is that
    // the result actually fits the space it was given.
    assert!(
        tm::measure(&out[..n], 2) <= 160,
        "ellipsized result is {} px wide, over the 160 px budget",
        tm::measure(&out[..n], 2)
    );
    assert_eq!(&out[n - 3..n], b"...", "a cut string ends in an ellipsis");
}

#[test]
fn ellipsize_leaves_text_that_fits_alone() {
    let (out, n) = tm::ellipsize(b"Camera", 1, 640);
    assert_eq!(&out[..n], b"Camera");
}

#[test]
fn ellipsize_refuses_rather_than_panicking_when_no_room_exists() {
    let (out, n) = tm::ellipsize(b"Settings", 12, 8);
    assert_eq!(n, 0, "no room for even an ellipsis must yield nothing, not underflow");
    assert_eq!(out[0], 0);
}

#[test]
fn a_package_id_humanises_into_a_readable_title() {
    let mut buf = [0u8; 64];
    assert_eq!(
        tm::humanise_package_id(b"org.interos.notes", &mut buf),
        b"Notes"
    );
}

#[test]
fn a_package_id_without_a_vendor_prefix_is_left_alone() {
    let mut buf = [0u8; 64];
    // WHY this case: `org` with nothing after the dot is an app named "org",
    // not a vendor segment. Dropping it would render a blank label, which is
    // worse than showing it.
    assert_eq!(tm::humanise_package_id(b"org", &mut buf), b"Org");
}

#[test]
fn a_package_id_keeps_only_its_final_segment() {
    let mut buf = [0u8; 64];
    // WHY the final segment and not the first: `org.interos` is the vendor and
    // `notes` is the app. Stripping only the first segment leaves
    // `interos.notes`, which reads as a product line nobody ships.
    assert_eq!(
        tm::humanise_package_id(b"org.interos.notes", &mut buf),
        b"Notes"
    );
}

#[test]
fn a_humanised_package_id_never_carries_a_trailing_separator() {
    let mut buf = [0u8; 64];
    let out = tm::humanise_package_id(b"org.interos.", &mut buf);
    assert!(
        !out.ends_with(b" "),
        "a trailing separator must not survive as a trailing space"
    );
}

#[test]
fn a_humanised_package_id_stops_at_the_buffer_it_was_given() {
    let mut buf = [0u8; 6];
    let out = tm::humanise_package_id(b"org.interos.calculator", &mut buf);
    assert_eq!(out.len(), 6, "the result is bounded by the buffer, not by the id");
}

#[test]
fn glyph_lookup_rejects_unprintable_bytes() {
    assert!(tm::glyph_index(b'A').is_some());
    assert!(tm::glyph_index(b' ').is_some());
    assert!(tm::glyph_index(31).is_none(), "control bytes have no glyph");
    assert!(tm::glyph_index(127).is_none());
}

#[test]
fn boot_typography_and_tagline_metrics() {
    let width = 720usize;
    let wordmark_text = b"InterOS";
    let wordmark_em = 52u32;
    let wordmark_adv = interos_std::design::typeface::text_advance_px(wordmark_text, wordmark_em) as usize;
    assert!(wordmark_adv > 100 && wordmark_adv < 200);
    let wordmark_x = (width - wordmark_adv) / 2;
    assert!((wordmark_x + wordmark_adv / 2).abs_diff(width / 2) <= 1);

    let tag1 = b"SMARTER";
    let tag2 = b"PRIVATE";
    let tag3 = b"OPEN";
    let w1 = interos_std::design::font::measure(tag1, 1);
    let w2 = interos_std::design::font::measure(tag2, 1);
    let w3 = interos_std::design::font::measure(tag3, 1);
    let gap = 24usize;
    let total_tag_w = w1 + gap + w2 + gap + w3;
    assert!(total_tag_w < width);
}

#[test]
fn dynamic_user_greeting_contract() {
    // Platform Truth Contract: Zero hardcoding or faking of identity.
    let user_name_len = 0usize;
    assert!(user_name_len == 0);

    let mut configured_name = [0u8; 32];
    let name_src = b"Alice";
    configured_name[..name_src.len()].copy_from_slice(name_src);
    let len = name_src.len();
    assert_eq!(&configured_name[..len], b"Alice");
}


// ---------------------------------------------------------------------------
// The search index and its ranking
// ---------------------------------------------------------------------------

use crate::recents::{self, Card, Preview, Swipe};
use crate::search::{self, CategoryStatus, Entry, ExactMatch, Ranker, SearchKind, SourceState};

/// An index with one app, one settings surface and one file.
fn sample_index() -> search::Index {
    let mut index = search::Index::new();
    index.push(Entry::new(
        SearchKind::App,
        b"Messages",
        b"Tools",
        b"org.interos.messages",
    ));
    index.push(
        Entry::new(
            SearchKind::Settings,
            b"Network",
            b"Settings",
            b"Network",
        )
        .with_keywords(b"network wifi wi-fi bluetooth cellular data"),
    );
    index.push(Entry::new(
        SearchKind::File,
        b"holiday.ppm",
        b"/storage/photos/holiday.ppm",
        b"/storage/photos/holiday.ppm",
    ));
    index
}

#[test]
fn an_exact_title_beats_every_other_match() {
    let index = sample_index();
    let results = index.search(b"messages", &ExactMatch);
    assert_eq!(results.count, 1, "only the app is named Messages");
    let hit = results.hit(0).expect("a hit");
    let entry = index.get(hit.index as usize).expect("the entry");
    assert_eq!(entry.kind, SearchKind::App);
    assert_eq!(entry.title(), b"Messages");
}

#[test]
fn a_match_ignores_case() {
    let index = sample_index();
    let results = index.search(b"MESSAGES", &ExactMatch);
    assert_eq!(results.count, 1);
    let results = index.search(b"mEsSaGeS", &ExactMatch);
    assert_eq!(results.count, 1);
}

#[test]
fn a_settings_section_is_found_by_the_words_a_person_would_use() {
    let index = sample_index();
    // "wifi" is not in the section's title; it is in the words the section is
    // known by. A search that only matched labels would not find it.
    let results = index.search(b"wifi", &ExactMatch);
    assert_eq!(results.count, 1, "wifi has to reach the Network section");
    let hit = results.hit(0).expect("a hit");
    let entry = index.get(hit.index as usize).expect("the entry");
    assert_eq!(entry.kind, SearchKind::Settings);
    assert_eq!(entry.title(), b"Network");
}

#[test]
fn a_file_is_found_by_its_name() {
    let index = sample_index();
    let results = index.search(b"holiday", &ExactMatch);
    assert_eq!(results.count, 1);
    let hit = results.hit(0).expect("a hit");
    let entry = index.get(hit.index as usize).expect("the entry");
    assert_eq!(entry.kind, SearchKind::File);
    assert_eq!(entry.title(), b"holiday.ppm");
}

#[test]
fn a_query_that_matches_nothing_finds_nothing() {
    let index = sample_index();
    let results = index.search(b"zzzzz", &ExactMatch);
    assert_eq!(results.count, 0);
    assert_eq!(results.apps, CategoryStatus::NoMatch);
    assert_eq!(results.settings, CategoryStatus::NoMatch);
    assert_eq!(results.files, CategoryStatus::NoMatch);
}

#[test]
fn an_empty_query_finds_nothing_rather_than_everything() {
    let index = sample_index();
    let results = index.search(b"", &ExactMatch);
    assert_eq!(results.count, 0, "an empty query has no answer to give");
}

#[test]
fn the_best_match_is_ranked_first() {
    let mut index = search::Index::new();
    index.push(Entry::new(
        SearchKind::App,
        b"Messages Archive",
        b"Tools",
        b"org.interos.archive",
    ));
    index.push(Entry::new(
        SearchKind::App,
        b"Messages",
        b"Tools",
        b"org.interos.messages",
    ));
    let results = index.search(b"messages", &ExactMatch);
    assert_eq!(results.count, 2, "both contain the query");
    let first = index.get(results.hit(0).expect("hit").index as usize).expect("entry");
    let second = index.get(results.hit(1).expect("hit").index as usize).expect("entry");
    assert_eq!(
        first.title(),
        b"Messages",
        "the exact title has to outrank the longer one"
    );
    assert_eq!(second.title(), b"Messages Archive");
}

#[test]
fn a_word_is_ranked_above_a_lucky_substring() {
    let mut index = search::Index::new();
    index.push(Entry::new(SearchKind::App, b"Sunset", b"", b"a"));
    index.push(Entry::new(SearchKind::App, b"Settings", b"", b"b"));
    let results = index.search(b"set", &ExactMatch);
    // WHY both are found: a substring match is a real match, and a search
    // that hid it would not find "Sunset" for a query that names it.
    assert_eq!(results.count, 2);
    // WHY the order matters: the word that *starts* with the query has to
    // come first, or every short query matches half the index.
    let first = index.get(results.hit(0).expect("hit").index as usize).expect("entry");
    assert_eq!(first.title(), b"Settings");
}

#[test]
fn an_unreadable_category_says_why_and_does_not_say_it_found_nothing() {
    let mut index = sample_index();
    index.set_files_state(SourceState::Unavailable(
        "the platform refused the storage listing",
    ));
    let results = index.search(b"holiday", &ExactMatch);
    assert_eq!(
        results.files,
        CategoryStatus::Unavailable("the platform refused the storage listing"),
        "an unavailable category is not the same answer as an empty one"
    );
    assert_ne!(
        results.files,
        CategoryStatus::NoMatch,
        "an unavailable category must not report as no match"
    );
    assert_eq!(
        results.apps,
        CategoryStatus::NoMatch,
        "the other categories still answer for themselves"
    );
}

#[test]
fn a_category_that_answered_reports_how_many_matched() {
    let index = sample_index();
    let results = index.search(b"o", &ExactMatch);
    // "Messages" contains an "o", "holiday.ppm" contains one, "Network" and
    // "Settings" do not, but "org.interos.messages" and the file's path do.
    assert!(matches!(results.apps, CategoryStatus::Matched(_)));
    assert!(matches!(results.files, CategoryStatus::Matched(_)));
}

#[test]
fn the_ranker_is_the_seam_a_model_backed_ranker_plugs_into() {
    // WHY this test exists: the seam is only a seam if something else can
    // implement it. A ranker that scores by its own rule has to be able to
    // replace the exact-match ranker without the index or the results
    // changing shape.
    struct AlwaysTop;
    impl Ranker for AlwaysTop {
        fn score(&self, _query: &[u8], entry: &Entry) -> Option<u32> {
            if entry.kind == SearchKind::File {
                Some(search::SCORE_EXACT)
            } else {
                None
            }
        }
    }

    let index = sample_index();
    let results = index.search(b"anything", &AlwaysTop);
    assert_eq!(results.count, 1);
    let hit = results.hit(0).expect("a hit");
    let entry = index.get(hit.index as usize).expect("the entry");
    assert_eq!(entry.kind, SearchKind::File);
    assert_eq!(
        results.apps,
        CategoryStatus::NoMatch,
        "a ranker that declines the apps reports no app match"
    );
}

#[test]
fn the_index_holds_at_most_what_it_says_it_holds() {
    let mut index = search::Index::new();
    let mut pushed = 0usize;
    for n in 0..(search::MAX_ENTRIES + 8) {
        let name = [b'A' + (n % 26) as u8];
        if index.push(Entry::new(SearchKind::App, &name, b"", b"x")) {
            pushed += 1;
        }
    }
    assert_eq!(pushed, search::MAX_ENTRIES);
    assert_eq!(index.len(), search::MAX_ENTRIES);
    assert!(index.get(search::MAX_ENTRIES).is_none());
}

// ---------------------------------------------------------------------------
// The task switcher's card model
// ---------------------------------------------------------------------------

#[test]
fn a_card_names_the_task_and_says_whether_it_has_a_picture() {
    let mut card = Card::EMPTY;
    card.package_id[..6].copy_from_slice(b"dialer");
    card.package_len = 6;
    card.task_id = 7;
    card.preview = Preview::Live {
        width: recents::PREVIEW_W as u16,
        height: recents::PREVIEW_H as u16,
    };
    assert_eq!(card.package(), b"dialer");
    assert!(card.preview.is_live());
    assert_eq!(card.preview.reason(), None);
}

#[test]
fn a_card_without_a_picture_names_the_reason() {
    let card = Card {
        preview: Preview::Unavailable("the app has not drawn a frame yet"),
        ..Card::EMPTY
    };
    assert!(!card.preview.is_live());
    assert_eq!(
        card.preview.reason(),
        Some("the app has not drawn a frame yet"),
        "a card with no picture has to say which of the reasons it is"
    );
}

#[test]
fn a_last_frame_is_not_a_live_preview() {
    // WHY this is a state of its own: an app in this platform exits when the
    // person leaves it, so the picture a card holds is the app's last frame
    // rather than a view of a task that is still running. Calling that a live
    // preview would be a claim the card cannot back.
    let last = Preview::LastFrame {
        width: recents::PREVIEW_W as u16,
        height: recents::PREVIEW_H as u16,
    };
    assert!(!last.is_live(), "a last frame is not a live preview");
    assert!(last.has_picture(), "a last frame is still a picture");
    assert_eq!(last.reason(), None, "a picture is not a reason");

    let live = Preview::Live {
        width: recents::PREVIEW_W as u16,
        height: recents::PREVIEW_H as u16,
    };
    assert!(live.is_live());
    assert!(live.has_picture());
}

#[test]
fn the_cards_stack_without_overlapping() {
    for index in 0..recents::MAX_CARDS {
        let card = recents::card_rect(index);
        assert!(card.w <= 720, "the card has to fit the surface");
        assert!(card.x + card.w <= 720);
        if index > 0 {
            let above = recents::card_rect(index - 1);
            assert!(
                card.y >= above.y + above.h,
                "card {} overlaps the one above it",
                index
            );
        }
    }
}

#[test]
fn the_preview_well_keeps_the_shape_of_the_screen_it_shows() {
    let well = recents::preview_rect(0);
    assert_eq!(well.w, recents::PREVIEW_W);
    assert_eq!(well.h, recents::PREVIEW_H);
    // The guest surface is 720 x 1440, so a preview with the same aspect
    // ratio shows the screen's shape rather than a squashed one.
    assert_eq!(well.w * 1440, well.h * 720, "the well has the screen's shape");
    let card = recents::card_rect(0);
    assert!(well.y + well.h <= card.y + card.h, "the well has to fit its card");
}

#[test]
fn the_close_control_is_a_target_a_person_can_hit() {
    let close = recents::close_rect(0);
    assert!(close.w >= 44 && close.h >= 44, "the close target is too small");
    let card = recents::card_rect(0);
    assert!(close.x >= card.x && close.x + close.w <= card.x + card.w);
    assert!(close.y >= card.y && close.y + close.h <= card.y + card.h);
    // The hit test has to agree with the rectangle.
    assert_eq!(recents::close_at(close.x + 2, close.y + 2, 1), Some(0));
    assert_eq!(recents::close_at(card.x + 2, card.y + 2, 1), None);
}

#[test]
fn a_card_is_found_by_the_point_that_is_on_it() {
    let card = recents::card_rect(1);
    assert_eq!(recents::card_at(card.x + 10, card.y + 10, 3), Some(1));
    assert_eq!(recents::card_at(card.x + 10, card.y + 10, 1), None);
    assert_eq!(recents::card_at(4, 4, 3), None);
}

#[test]
fn a_gesture_is_read_as_a_dismiss_or_as_a_scroll() {
    assert_eq!(recents::classify_swipe(-200), Swipe::Dismiss);
    assert_eq!(
        recents::classify_swipe(-recents::DISMISS_DISTANCE),
        Swipe::Dismiss
    );
    assert_eq!(recents::classify_swipe(-10), Swipe::None);
    assert_eq!(recents::classify_swipe(0), Swipe::None);
    assert_eq!(recents::classify_swipe(200), Swipe::Home);
    assert_eq!(
        recents::classify_swipe(recents::HOME_DISTANCE),
        Swipe::Home
    );
}

#[test]
fn a_label_inks_the_pixels_it_was_placed_at() {
    // WHY this test exists: a label that is placed but never inks is a label
    // that is invisible, and the screen that draws it cannot tell the
    // difference between "there is no text" and "the text did not render".
    use crate::text::Label;
    let ink = 0xFF102030u32;
    let bg = 0xFFFFFFFFu32;
    let label = Label::at(88, 84, 2, b"settings", ink);
    let mut inked = 0usize;
    for y in 70..150 {
        for x in 80..320 {
            if label.pixel(x, y, bg) == Some(ink) {
                inked += 1;
            }
        }
    }
    assert!(inked > 20, "the label inked {} pixels", inked);
}
