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
