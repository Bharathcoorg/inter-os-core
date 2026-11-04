# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Truth contract for the launcher's global search surface.

Two claims the search surface made that it had not earned. An empty field read
"No results", which reports a search that ran and found nothing when nothing
was asked of it -- the same surface's own per-source verdicts already refuse
that wording, so the headline contradicted the rows under it. And a file row
whose path did not fit its buffer cut the path at the edge, so
`/storage/downloads/preview_clip.ppm` was painted as `preview_clip.p`: a name
the storage does not hold, presented as one it does.

Every assertion below pins a specific defect observed on the device.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCREEN = ROOT / "apps/launcher/src/screens/search.rs"
INDEX = ROOT / "apps/launcher/src/main.rs"
ENTRIES = ROOT / "apps/launcher/src/search.rs"


def without_comments(source: str) -> str:
    """Drop Rust comments so a defect named in prose cannot pass as a fix.

    String literals stay: this surface's claims *are* literals, and the whole
    contract is about which literal is chosen.
    """
    return re.sub(r"//[^\n]*|/\*.*?\*/", "", source, flags=re.DOTALL)


def code(path: Path) -> str:
    return without_comments(path.read_text(encoding="utf-8"))


def headline_branch(source: str) -> str:
    """The `let headline = ...` binding and the arms it chooses between."""
    match = re.search(r"let headline = .*?\n\s*\};", source, re.DOTALL)
    assert match, "the search surface no longer builds a headline from the result count"
    return match.group(0)


def subtitle_builder(source: str) -> str:
    """The one body that turns a stored path into a file row's subtitle."""
    match = re.search(r"pub fn file_row_subtitle\(.*?\n\}", source, re.DOTALL)
    assert match, (
        "the file row no longer builds its subtitle in one place; two builders "
        "is how a cut path got fixed for `/storage` and left broken one level in"
    )
    return match.group(0)


def test_an_empty_query_is_not_reported_as_a_search_that_found_nothing():
    branch = headline_branch(code(SCREEN))
    assert "state.search_query_len == 0" in branch, (
        "the headline chooses between found and not-found without asking "
        "whether a query was typed at all"
    )
    assert re.search(
        r"search_query_len == 0\s*\{\s*b\"Type to search\"", branch
    ), "an empty query must be told it is waiting for one, the way the per-source verdicts already say"


def test_no_results_is_still_said_when_a_query_genuinely_found_nothing():
    branch = headline_branch(code(SCREEN))
    assert 'b"No results"' in branch, (
        "a query that found nothing has to be told so; the fix is about the "
        "empty query, not about hiding a real zero"
    )


def test_a_cut_file_path_is_marked_as_cut():
    body = subtitle_builder(code(ENTRIES))
    assert "path.len() > room" in body, (
        "a path that overflows its buffer must be cut visibly; a row reading "
        "preview_clip.p names a file the storage does not hold"
    )
    assert 'b"..."' in body, "the cut has to be shown as an ellipsis"


def test_a_path_that_fits_is_painted_whole():
    body = subtitle_builder(code(ENTRIES))
    assert re.search(r"\}\s*else\s*\{\s*buf\[at\.\.at \+ path\.len\(\)\]\.copy_from_slice\(path\)", body), (
        "the non-cutting branch has to copy the whole path; ellipsising every "
        "row would claim a cut where there is none"
    )


def test_both_storage_listings_share_the_one_subtitle_rule():
    callers = code(INDEX)
    assert callers.count("file_row_subtitle(") == 2, (
        "`/storage` and the folders one level into it push the same kind of "
        "row and have to cut it the same way"
    )
    assert 'b"Files: "' not in code(ENTRIES).replace(
        subtitle_builder(code(ENTRIES)), ""
    ), "a second copy of the subtitle prefix is a second rule that can drift"


def test_the_headline_still_counts_the_results_it_painted():
    assert "let count = state.search_results.count;" in code(SCREEN), (
        "the headline is the surface's only statement of how many results the "
        "query found; it has to read the same list the rows paint"
    )


# -----------------------------------------------------------------------------
# Placeholder ink over the space the answer left empty
# -----------------------------------------------------------------------------


def model() -> str:
    """The list model, comment-stripped: `launcher::search`."""
    return code(ENTRIES)


def screen() -> str:
    """The search surface, comment-stripped: `screens/search.rs`."""
    return code(SCREEN)


def shell() -> str:
    """The shell that answers the tap, comment-stripped: `main.rs`."""
    return code(INDEX)


def _placeholder_line_faults(model_code: str, screen_code: str, shell_code: str) -> list[str]:
    """Every half of the placeholder rule, as faults rather than as asserts.

    WHAT the surface had wrong, in three parts. It walked the ranking in its painter
    and stopped at the first hit whose entry the index could not name, while the tap
    path hit-tested `state.search_results.count` slots: the panel inked two rows,
    answered taps on six, and the slots under the ink were dead space. That dead
    space carried a fixed line -- "Tap a result to open it. Backspace edits the
    query." -- whether or not a needle had been typed and whether or not the search
    had brought anything back, so the region the answer left empty was filled with a
    promise about results the launcher did not have. And the field's own hint had a
    second copy of its rule written at the paint site, where a later edit could ink
    it behind a typed query or in a row slot.

    WHY the rules now sit in `launcher::search`: the screen is part of a
    `#![no_main]` binary, so nothing under `screens/` is reachable from
    `cargo test -p launcher --lib`. A rule only the painter can see is a rule only a
    screenshot can check, and this defect is exactly the one a screenshot showed and
    no test could name.

    WHY the code is comment-stripped: a doc comment that quotes the defect is a note
    about it, and must neither trip a pin nor satisfy one.
    """
    faults: list[str] = []

    def block(source: str, start: str, end: str, tag: str) -> str:
        """The slice between two markers, or nothing plus a named fault.

        WHY a fault instead of an exception: a marker that moved is the same news as
        a needle that went -- the gate can no longer see the shape it watches -- and
        a crash hides which one it was.
        """
        at = source.find(start)
        if at < 0:
            faults.append(f"{tag}: `{start}` is gone, so the block this pin reads is not there")
            return ""
        after = source.find(end, at + len(start))
        if after < 0:
            faults.append(f"{tag}: `{end}` is gone, so the block this pin reads has no end")
            return ""
        return source[at:after]

    def tail(source: str, start: str, tag: str) -> str:
        """The body from `start` to the end of the file, or nothing plus a fault.

        WHY the last item in a module needs no end marker: there is nothing after
        it to stop at, and inventing one is a marker that can only ever be missing.
        """
        at = source.find(start)
        if at < 0:
            faults.append(f"{tag}: `{start}` is gone, so the block this pin reads is not there")
            return ""
        return source[at:]

    def needle(text_block: str, text: str, tag: str, why: str) -> None:
        if text not in text_block:
            faults.append(f"{tag}: {why}")

    # --- the list model: which hits are rows, and what a row answers with -------
    build = block(
        model_code,
        "pub fn build(results: &Results, index: &Index) -> ListRows {",
        "pub fn len(&self)",
        "model.build",
    )
    needle(
        build,
        "if index.get(hit.index as usize).is_some() {",
        "model.rows",
        "a row is inked for a hit the index cannot name, so the list holds ink with no "
        "answer behind it in the very slot a tap is answered from",
    )
    row_at = block(
        model_code,
        "pub fn row_at(&self, x: usize, y: usize)",
        "pub fn row_rect(",
        "model.row_at",
    )
    needle(
        row_at,
        "return self.get(slot).map(|row| row.rank);",
        "model.rank",
        "a tap opens the hit numbered by the slot it landed on rather than the one that "
        "row was painted for, so the row says one thing and the tap does another",
    )
    if "while slot < self.len" not in row_at:
        faults.append(
            "model.walk: the hit-test walks the ranking's own count again, which is how "
            "the panel inked two rows and answered taps on six"
        )

    # --- the field's hint: a hint, in the field, and only while it is empty ------
    hint = block(model_code, "pub const fn field_hint(", "pub const fn foot_line_applies(", "model.hint")
    needle(
        hint,
        "needle_len == 0",
        "model.hint",
        "the field's hint is offered whatever the query is, so it stands on top of what "
        "the person typed and reads as a suggestion they did not ask for",
    )
    if model_code.count("Search apps, settings and files") != 1:
        faults.append(
            "model.hint-copy: the hint's copy is written more than once, and a second "
            "copy is a second rule about where a hint may be inked"
        )

    # --- the line under the verdict card: a promise about rows that may not exist -
    foot = tail(model_code, "pub const fn foot_line_applies(", "model.foot")
    needle(
        foot,
        "needle_len > 0 && rows > 0",
        "model.foot",
        "the line under the card promises a result to tap and a query to edit whatever "
        "the answer holds, so it fills the empty state with an example of a result",
    )

    # --- the painter: one list, no second copy of any of it ----------------------
    paint = block(screen_code, "fn build(state: &LauncherState) -> Plan {", "fn verdict_plan(", "screen.build")
    needle(
        paint,
        "let rows = ListRows::build(&state.search_results, &state.search_index);",
        "screen.list",
        "the painter no longer reads the list, so it is back to deciding on its own "
        "which hits have rows and which are ink over nothing",
    )
    needle(
        paint,
        "while row < rows.len() {",
        "screen.walk",
        "the painter walks the ranking's slots instead of the rows the list holds",
    )
    needle(
        paint,
        "launcher::search::foot_line_applies(state.search_query_len, rows.len())",
        "screen.hint",
        "the line under the card is inked unconditionally again, standing over the "
        "region the answer left empty",
    )
    needle(
        paint,
        "launcher::search::field_hint(state.search_query_len)",
        "screen.field",
        "the surface decides the hint's rule at the paint site again, where it can ink "
        "it behind a typed needle",
    )
    if "Search apps, settings and files" in paint:
        faults.append(
            "screen.hint-copy: the hint's copy is written out in the surface as well as "
            "in the model, which is how it ended up where a row would be painted"
        )
    fingerprint = block(screen_code, "let should_rebuild = unsafe {", "if should_rebuild {", "screen.cache")
    needle(
        fingerprint,
        "p.cached_query != state.search_query",
        "screen.cache",
        "the cached plan is keyed on the needle's length alone, so `browel` keeps being "
        "inked as `browser`'s answer -- rows from an answer nobody is looking at",
    )
    tap = block(screen_code, "pub fn result_at(", "pub fn query_band(", "screen.tap")
    needle(
        tap,
        "ListRows::build(&state.search_results, &state.search_index).row_at(x, y)",
        "screen.tap",
        "the surface answers taps from the ranking's count instead of the rows it holds",
    )
    if "0..count.min(" in screen_code:
        faults.append(
            "screen.count: the surface hit-tests a count of its own again, so a tap can "
            "land on a slot it never inked"
        )

    # --- the shell: the tap is answered from the same list ------------------------
    needle(
        shell_code,
        "screens::search::result_at(x, y, &state)",
        "shell.tap",
        "the shell no longer asks the surface which rows it holds before opening one",
    )
    if "result_at(x, y, state.search_results.count)" in shell_code:
        faults.append(
            "shell.count: the shell hit-tests the ranking's count, so a tap below the "
            "inked rows opens a result the person never saw"
        )
    return faults


def test_the_search_surface_inks_only_the_rows_the_answer_returned():
    """No placeholder line, and no row the launcher does not hold.

    The empty state is the surface's own two truthful lines -- the field's hint while
    nobody has typed, and the headline that says `Type to search` or `No results`
    according to whether a needle exists -- plus the per-source verdicts. Everything
    else in the list region is a row the ranking returned and the index can still
    name, or it is not inked.
    """
    faults = _placeholder_line_faults(model(), screen(), shell())
    assert not faults, faults


def test_the_placeholder_line_contract_can_fail():
    """A gate nobody has seen fail is a comment.

    Each arm puts one half of the old surface back into a copy of the honest source
    and runs the detector the gate above runs, so the check is proved to fire on the
    defect it exists for -- and an arm that no longer matches its source is refused,
    because a drifted arm would otherwise go quiet.
    """
    honest_model, honest_screen, honest_shell = model(), screen(), shell()
    clean = _placeholder_line_faults(honest_model, honest_screen, honest_shell)
    assert not clean, f"the detector fires on the surface as it stands: {clean}"

    unconditional_foot_line = (
        "    plan.hint = if launcher::search::foot_line_applies(state.search_query_len, rows.len()) {\n"
        "        launcher::layout::search_hint_label(theme.on_surface_variant)\n"
        "    } else {\n"
        "        Label::EMPTY\n"
        "    };"
    )
    count_bounded_tap = (
        "                                if let Some(rank) =\n"
        "                                    screens::search::result_at(x, y, state.search_results.count)\n"
        "                                {"
    )
    arms = (
        (
            "inks the line under the card whatever the answer holds",
            "screen",
            unconditional_foot_line,
            "    plan.hint = launcher::layout::search_hint_label(theme.on_surface_variant);",
            "screen.hint",
        ),
        (
            "promises a result to tap from an empty list",
            "model",
            "    needle_len > 0 && rows > 0",
            "    let _ = (needle_len, rows);\n    true",
            "model.foot",
        ),
        (
            "keeps a hit the index cannot name as a row",
            "model",
            "if index.get(hit.index as usize).is_some() {",
            "if true {",
            "model.rows",
        ),
        (
            "offers the field's hint behind a typed needle",
            "model",
            "    if needle_len == 0 {",
            "    if true {",
            "model.hint",
        ),
        (
            "answers a tap with the slot's number, not the rank painted there",
            "model",
            "return self.get(slot).map(|row| row.rank);",
            "return Some(slot);",
            "model.rank",
        ),
        (
            "walks the ranking's slots instead of the list's rows",
            "screen",
            "    while row < rows.len() {",
            "    while row < MAX_ROWS {",
            "screen.walk",
        ),
        (
            "reuses the last needle's plan for a needle of the same length",
            "screen",
            "                    || p.cached_query != state.search_query\n",
            "",
            "screen.cache",
        ),
        (
            "hit-tests the ranking's own count in the shell",
            "shell",
            "                                let tapped = screens::search::result_at(x, y, &state);\n"
            "                                if let Some(rank) = tapped {",
            count_bounded_tap,
            "shell.count",
        ),
    )
    sources = {"model": honest_model, "screen": honest_screen, "shell": honest_shell}
    for label, which, marker, replacement, tag in arms:
        source = sources[which]
        assert source.count(marker) == 1, (
            f"the arm that {label} no longer matches its source once, so it proves nothing"
        )
        armed = dict(sources)
        armed[which] = source.replace(marker, replacement)
        faults = _placeholder_line_faults(armed["model"], armed["screen"], armed["shell"])
        assert any(f.startswith(f"{tag}:") for f in faults), (
            f"the detector does not catch a surface that {label}: {faults}"
        )
