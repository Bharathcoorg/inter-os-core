# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Gates the shell's banded repaint: a touch re-inks the rows it moved, not the panel.

WHY these are source gates and not a paint diff: the shell is a `#![no_main]`
binary whose screens live under `apps/launcher/src/screens/` and read their state
from `LauncherState`, which is declared in that binary's crate root. The host test
harness the crate does have (`apps/launcher/src/lib.rs`) therefore cannot compile a
screen, so there is no host process that can paint one band and the whole surface
and compare them. What these gates assert instead are the two properties that make
band-versus-surface pixel-identical, and the geometry they rest on is tested for
real in the lib (`apps/launcher/src/tests.rs` proves the nav marks are painted
inside the band that answers them and nowhere outside it):

  1. Every band `Dirty::rows()` names is read back from the screen module that
     draws those rows, so the band and its painter cannot be two numbers held
     together by hand.
  2. A screen that caches its frame between pixels -- rebuilding only when it is
     handed `(0, 0)`, which a band never is -- must have that cache cleared by the
     band that reads it, or the band re-inks the previous frame and the control
     answers with nothing. This is not hypothetical: the notifications pill row and
     card dismissal had exactly that defect, and
     `test_a_cached_frame_is_cleared_by_the_band_that_reads_it` is what stops it
     coming back on any other screen.

Nothing here runs QEMU, and nothing here measures a latency.
"""

from pathlib import Path
import re

import pytest

ROOT = Path(__file__).resolve().parents[1]
SHELL = ROOT / "apps" / "launcher" / "src" / "main.rs"
SCREENS = ROOT / "apps" / "launcher" / "src" / "screens"

# The two full-surface walks no touch reaches: the first frame the shell paints,
# and the catalogue refresh whose new count relabels tiles on two surfaces.
NON_TOUCH_WALKS = ("Step 3: Rendering launcher UI canvas", "refresh_package_catalog")


def shell_source() -> str:
    return SHELL.read_text(encoding="utf-8", errors="replace")


def screen_source(module: str) -> str:
    return (SCREENS / f"{module}.rs").read_text(encoding="utf-8", errors="replace")


def body_of(source: str, signature: str) -> str:
    """A function's text, taken as the lines from its signature to its own closing
    brace at the same indent.

    WHY brace counting is avoided: the shell's bodies carry braces inside strings
    and comments, and reading a match arm or a guard only ever needs the run of
    lines between the signature and the dedent that closes it.
    """
    start = source.index(signature)
    head = source[start:].split("\n", 1)[0]
    indent = len(head) - len(head.lstrip())
    lines = source[start:].split("\n")
    for offset, line in enumerate(lines[1:], start=1):
        if not line.strip():
            continue
        if (len(line) - len(line.lstrip())) <= indent and offset > 1:
            return "\n".join(lines[:offset])
    return "\n".join(lines)


def rows_arms() -> dict:
    """`Dirty` variant name -> the band expression inside its `rows()` arm.

    The `Some(...)` wrapper is removed here so that every gate below reads the same
    expression the painter owns, rather than a string with a trailing paren on it.
    """
    body = body_of(
        shell_source(), "    fn rows(self) -> Option<(usize, usize)> {"
    )
    arms = {}
    for line in body.split("\n"):
        stripped = line.strip().rstrip(",")
        if stripped.startswith("Dirty::") and "=> Some(" in stripped:
            variant = stripped.split("=>")[0].strip()[len("Dirty::") :]
            # `Dirty::Band(y0, y1)` is the combinator's arm; the variant is `Band`.
            variant = variant.split("(")[0]
            inner = stripped.split("=>", 1)[1].strip()
            if inner.startswith("Some(") and inner.endswith(")"):
                inner = inner[len("Some(") : -1]
            arms[variant] = inner
    return arms


def present_dirty_body() -> str:
    return body_of(
        shell_source(),
        "fn present_dirty(surface: u32, state: &LauncherState, dirty: Dirty) {",
    )


def view_arm(view: str) -> str:
    """The shell's `ViewMode::<view> => { ... }` input arm, up to its next sibling.

    WHY searched from `_start` and not from the top of the file: the render-complete
    logger above it matches on the same view names, and its arm is not the input
    loop this gate is about.
    """
    source = shell_source()
    start = source.index(f"ViewMode::{view} => {{", source.index('pub extern "C" fn _start'))
    tail = source[start:]
    end = tail.find("\n                        ViewMode::", 1)
    return tail[: end if end != -1 else len(tail)]


# ---------------------------------------------------------------------------
# 1. Every band comes from the screen that draws it.
# ---------------------------------------------------------------------------


def band_module_and_accessor(rhs: str) -> tuple:
    """`screens::home::deck_band()` -> `("home", "deck_band")`."""
    assert rhs.startswith("screens::"), rhs
    path, fn = rhs[len("screens::") :].rsplit("::", 1)
    return path, fn[: -len("()")]


def test_every_band_is_read_from_the_screen_that_paints_it():
    """A band retyped in the shell is a band that can disagree with its painter.

    The disagreement shows up as stale pixels in exactly the rows nobody thought to
    repaint, which is why only `Band`/`All`/`None` -- regions that name no screen --
    are allowed a literal here.
    """
    for variant, rhs in rows_arms().items():
        if variant in ("None", "Band", "All"):
            continue
        if variant == "NavBar":
            assert rhs == "(nav::BAND_TOP, nav::BAND_BOTTOM)", (
                "the nav band must be the bar `nav::target_at` answers, not a copy"
            )
            continue
        assert "band()" in rhs, (
            f"Dirty::{variant} must answer with the owning screen's band accessor"
        )


def test_each_band_accessor_exists_in_the_screen_that_owns_it():
    for variant, rhs in rows_arms().items():
        if not rhs.startswith("screens::"):
            continue
        module, fn = band_module_and_accessor(rhs)
        assert re.search(
            rf"pub (?:const )?fn {fn}\(\) -> \(usize, usize\)", screen_source(module)
        ), (
            f"Dirty::{variant} paints from {module}::{fn}, which does not exist"
        )


def test_the_nav_band_is_the_home_surface_bottom_bar():
    nav = (ROOT / "apps" / "launcher" / "src" / "nav.rs").read_text(
        encoding="utf-8"
    )
    assert "pub const BAND_TOP: usize = 1380;" in nav
    assert "pub const BAND_BOTTOM: usize = SCREEN_HEIGHT;" in nav
    # The bar's ink is painted from that same band, so a nav tap cannot move a row
    # the shell is not re-presenting.
    assert "y >= launcher::nav::BAND_TOP" in screen_source("home")


def test_every_dirty_variant_is_reached_by_the_shell():
    """A variant that nothing names is a band that quietly stopped being painted.

    Each one is either named by the shell's input loop -- presented directly, or
    folded into a `dirty` value the loop then presents -- or cleared in
    `present_dirty` for the band that reads it. A variant that is neither is dead
    code wearing a comment.
    """
    source = shell_source()
    loop = source[source.index('pub extern "C" fn _start') :]
    cleared = present_dirty_body()
    for variant in rows_arms():
        if variant in ("None", "All", "Band"):
            continue
        assert f"Dirty::{variant}" in loop or f"Dirty::{variant}" in cleared, (
            f"Dirty::{variant} names a band the shell neither presents nor clears"
        )


# ---------------------------------------------------------------------------
# 2. A cached frame must be cleared by the band that reads it.
# ---------------------------------------------------------------------------


def cached_screen_modules() -> list:
    """The screen modules a band repaints, deduced from `rows()` itself."""
    modules = {
        band_module_and_accessor(rhs)[0]
        for rhs in rows_arms().values()
        if rhs.startswith("screens::")
    }
    return sorted(modules)


@pytest.mark.parametrize("module", cached_screen_modules())
def test_a_cached_frame_is_cleared_by_the_band_that_reads_it(module: str):
    """The rule, applied to whichever screen a band happens to repaint.

    A cache that also compares the state it read (`cached_query_len`,
    `cached_fingerprint`, ...) rebuilds itself when a band hands it a row it never
    saw, so it needs nothing from the shell. A cache keyed only on `(0, 0)` does:
    a band never passes through that pixel.
    """
    source = screen_source(module)
    holds_a_cache = "static mut" in source and "(x == 0 && y == 0)" in source
    if not holds_a_cache:
        return
    if "cached_" in source:
        return
    assert re.search(r"pub fn invalidate_", source), (
        f"screens::{module} caches its frame and rebuilds it only at (0, 0), so it"
        " must expose an invalidation for the banded path"
    )
    assert f"screens::{module}::invalidate_" in present_dirty_body(), (
        f"{module}'s frame cache must be cleared inside present_dirty, or a banded"
        " repaint draws the frame built for the state that came before"
    )


def test_events_band_clears_the_frame_it_repaints():
    """Regression gate: the pills carry real per-source counts and the feed re-flows.

    Both the filter pill and a card dismissal present `Dirty::EventsFilter`; before
    this clear existed, the band was painted from the frame built for the previous
    feed, so the pill stayed unlit and the dismissed card stayed on screen.
    """
    assert "pub fn invalidate_events_frame" in screen_source("notifications")
    assert "screens::notifications::invalidate_events_frame()" in present_dirty_body()


def test_notification_dismissal_and_open_present_the_feed_band():
    """Neither leaves the view, so neither may walk 1,036,800 pixels."""
    shell = shell_source()
    for marker in (
        "[LAUNCHER] Notification dismissed.",
        "[LAUNCHER] Notification action activated.",
    ):
        site = shell.index(marker)
        arm_start = shell.rindex("if let Some(", 0, site)
        window = shell[arm_start:site]
        assert "present_dirty(surface, &state, Dirty::EventsFilter)" in window, marker
        if "render_view(surface, &state);" in window:
            # Only the branch that leaves for the home grid may walk the surface.
            assert "state.current_view = ViewMode::HomeScreen;" in window, marker


# ---------------------------------------------------------------------------
# 3. Typing on the drawer and the search surface paints rows, not the panel.
# ---------------------------------------------------------------------------


def test_a_query_field_mutation_never_walks_the_surface():
    """Every edit of the query text must be answered by a band.

    WHY keyed on the mutation and not on a hand-listed set of call sites: the edit
    is what the repaint exists for, so a new keystroke path that forgot the band
    fails here instead of in front of somebody typing.
    """
    for view, mutators in (
        (
            "AppDrawer",
            (
                "state.search_query_len -= 1;",
                "state.search_query_len = 0;",
                "state.search_query_len += 1;",
            ),
        ),
        (
            "Search",
            (
                "state.search_query_len -= 1;",
                "state.search_query_len = 0;",
                "state.search_query_len += 1;",
            ),
        ),
    ):
        arm = view_arm(view)
        edits = [
            line for line in arm.split("\n") if any(m in line for m in mutators)
        ]
        assert edits, f"{view} lost the query edits this gate was written against"
        for index, line in enumerate(arm.split("\n")):
            if line not in edits:
                continue
            following = "\n".join(arm.split("\n")[index : index + 5])
            assert "render_view(surface, &state);" not in following, (
                f"{view}: a keystroke edit fell back to a full-surface walk"
            )


@pytest.mark.parametrize(
    "view,variant,count",
    [("AppDrawer", "Dirty::DrawerQuery", 3), ("Search", "Dirty::SearchQuery", 3)],
)
def test_the_keystroke_arms_present_their_band(view, variant, count):
    assert (
        view_arm(view).count(f"present_dirty(surface, &state, {variant})") == count
    ), f"{view}: every keystroke arm must present {variant}"


def test_a_chip_or_page_selection_presents_the_rows_it_moves():
    """A category chip re-filters the page, so the pills and the grid move together."""
    drawer = view_arm("AppDrawer")
    assert "present_dirty(surface, &state, Dirty::DrawerFilter)" in drawer
    assert drawer.count("present_dirty(surface, &state, Dirty::DrawerPages)") == 3


def test_the_banded_views_status_bar_still_gets_its_reading():
    """A band that excludes rows 0..48 freezes the clock unless the timer visits.

    The drawer and the search surface used to be carried along by their own
    full-surface repaints; once those became bands, the once-a-minute status band
    had to cover them too, or the clock would stop while a person typed.
    """
    shell = shell_source()
    site = shell.index("render_band(surface, &state, 0, 48);")
    guard = shell[shell.rindex("if matches!(", 0, site) : site]
    for view in ("HomeScreen", "LiveWidgets", "AppDrawer", "Search"):
        assert f"ViewMode::{view}" in guard, f"{view} lost its status-bar reading"


# ---------------------------------------------------------------------------
# 4. What a full walk is still for.
# ---------------------------------------------------------------------------


def test_every_full_walk_in_the_shell_loop_is_a_view_change():
    """A touch that stays on one screen must name the rows it moved.

    Each walk allowed here is a screen change -- `state.current_view` is assigned in
    the same arm -- or one of the two paths no touch reaches: the first frame and
    the catalogue refresh.
    """
    lines = shell_source().split("\n")
    start = next(i for i, line in enumerate(lines) if 'fn _start' in line)
    offenders = []
    walks = 0
    for number in range(start, len(lines)):
        if "render_view(surface, &state);" not in lines[number]:
            continue
        walks += 1
        before = " ".join(
            line.strip()
            for line in lines[max(start, number - 30) : number]
            if line.strip() and not line.strip().startswith("//")
        )
        after = " ".join(
            line.strip()
            for line in lines[number : min(len(lines), number + 12)]
            if line.strip() and not line.strip().startswith("//")
        )
        if "state.current_view =" in before:
            continue
        if any(marker in before or marker in after for marker in NON_TOUCH_WALKS):
            continue
        offenders.append(number + 1)
    assert not offenders, (
        f"lines {offenders} walk the whole surface without changing view"
    )
    assert walks >= 60, "the walk inventory below shrank; re-audit rather than guess"
