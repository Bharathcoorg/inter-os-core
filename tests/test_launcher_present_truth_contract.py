#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Static contract: the launcher shell presents only what it just drew.

Three measured device facts, all from the same shell:

* after ``POST /api/apps/launch org.interos.music`` the app really painted
  (``[MUSIC APP] FRAMEBREAK ... px=1036800``, ``Music canvas presented to
  scanout buffer``) and the panel still showed the app drawer, while the shell
  logged ``[LAUNCHER] FRAMEBREAK draw=0 write=0 beat=0 px=1036800`` -- a present
  on a beat that drew nothing and wrote nothing;
* a tap on a fully painted drawer tile reported ``view=HomeScreen`` and launched
  nothing, because a launch handler had rewritten ``current_view`` without
  repainting;
* ``render_app_drawer complete: 15-app grid presented`` over ~330 rows of
  unpainted surface between the last tile row and the navigation band.

The kernel's ``surface_flip`` composites every surface and pushes the changed
scanout rows, so a flip is a claim about the whole panel and not about the
caller. That makes all three the same invariant: the surface you painted and the
view you hit-test are decided together, and a beat that put no bytes into the
surface does not present.

WHY A STATIC CONTRACT: ``apps/launcher`` is ``#![no_main]`` with
``test = false`` on its binary target, so cargo cannot attach a harness to the
shell's own event loop. The *semantics* of the mechanism live in the library and
are tested there (``apps/launcher/src/tests.rs``:
``a_beat_that_wrote_nothing_earns_no_present`` and friends). What only a source
contract can hold is the wiring -- that the binary actually routes its presents
and its view changes through that mechanism instead of restating it by hand at
each of its ~130 call sites.
"""
from pathlib import Path
import re

import pytest

ROOT = Path(__file__).resolve().parents[1]
MAIN = ROOT / "apps" / "launcher" / "src" / "main.rs"
LIB = ROOT / "apps" / "launcher" / "src" / "lib.rs"
PRESENT = ROOT / "apps" / "launcher" / "src" / "present.rs"
DRAWER = ROOT / "apps" / "launcher" / "src" / "screens" / "app_drawer.rs"
NAV = ROOT / "apps" / "launcher" / "src" / "nav.rs"
SPACING = ROOT / "sdk" / "rust" / "interos-std" / "src" / "design" / "mod.rs"


@pytest.fixture(scope="module")
def main() -> str:
    return MAIN.read_text(encoding="utf-8", errors="replace")


def lines(text: str) -> list[str]:
    return text.splitlines()


def is_code(line: str) -> bool:
    """Whether a source line is Rust rather than a comment or blank."""
    stripped = line.strip()
    return bool(stripped) and not stripped.startswith("//") and not stripped.startswith("///")


def func_body(text: str, signature: str) -> list[str]:
    """One top-level function, from its `fn` line to the `}` column-0 line that closes it.

    WHY a body and not a file-wide substring search: claims like "the band painter
    consults the record" are about *which function asks*, and a file-wide search is
    satisfied by whichever function happens to contain the words. Scanning to the
    first unindented `}` is enough here because every function this contract reads
    is declared at module level.
    """
    body = lines(text)
    start = next((n for n, line in enumerate(body) if line.startswith(signature)), None)
    assert start is not None, f"{signature} is not in the source this contract reads"
    for n in range(start + 1, len(body)):
        if body[n] == "}":
            return body[start : n + 1]
    raise AssertionError(f"{signature} never closes at column 0")


# ---------------------------------------------------------------------------
# The mechanism exists, is in the library, and is the one the binary uses
# ---------------------------------------------------------------------------


def test_the_present_mechanism_is_in_the_testable_library() -> None:
    """WHY: a rule that only the `#![no_main]` binary can see is a rule no test can break."""
    assert PRESENT.is_file(), "apps/launcher/src/present.rs is the seam the invariant lives in"
    assert "pub mod present;" in LIB.read_text(encoding="utf-8", errors="replace")
    source = PRESENT.read_text(encoding="utf-8", errors="replace")
    assert "pub struct PresentGate" in source
    assert "pub struct PaintedView" in source


def test_the_shell_routes_every_present_through_the_gate(main: str) -> None:
    """WHY: one gate with one hole is not a gate."""
    body = lines(main)
    direct = [
        (n, line)
        for n, line in enumerate(body, 1)
        if is_code(line) and "std::surface_flip()" in line
    ]
    assert len(direct) == 1, (
        "the only call to std::surface_flip() must be inside surface_flip_display, "
        f"which asks the gate first; found {direct}"
    )
    assert "fn surface_flip_display()" in "\n".join(body)


def test_the_shell_records_every_surface_write_it_makes(main: str) -> None:
    """WHY: the gate counts writes, so a write that slips past the counter is a lie."""
    body = lines(main)
    direct = [
        (n, line)
        for n, line in enumerate(body, 1)
        if is_code(line) and "std::surface_write(" in line
    ]
    assert len(direct) == 1, (
        "the only call to std::surface_write() must be inside surface_write_display, "
        f"which notes the pixels for the gate; found {direct}"
    )


# ---------------------------------------------------------------------------
# A: no present on a beat that drew nothing
# ---------------------------------------------------------------------------


def test_no_double_present_after_a_full_repaint(main: str) -> None:
    """`render_view` presents; a second flip in the same beat is the zero-draw present.

    WHY this is the headline defect's mechanism: `render_view` ends with
    `surface_flip_display()`, and the shell's call sites flipped again on the next
    line. The second flip is a present whose beat had drawn nothing and written
    nothing -- which is exactly the `[LAUNCHER] FRAMEBREAK draw=0 write=0` the
    device logged while a launched app was trying to hold the panel.
    """
    body = lines(main)
    offenders = []
    for n, line in enumerate(body):
        if not is_code(line) or "render_view(surface" not in line:
            continue
        for ahead in body[n + 1 : n + 4]:
            if is_code(ahead) and "surface_flip_display()" in ahead:
                offenders.append(n + 1)
                break
    assert not offenders, (
        f"render_view already presents; a surface_flip_display() on the next line is a "
        f"zero-draw present. Remove it at lines: {offenders}"
    )


def test_no_double_present_after_the_dirty_all_path(main: str) -> None:
    """The `Dirty::All` arm of `present_dirty` must not flip a second time."""
    body = lines(main)
    start = next(
        n for n, line in enumerate(body) if line.startswith("fn present_dirty(")
    )
    arm = "\n".join(body[start : start + 60])
    assert "render_view(surface, state)" in arm or "render_view(surface, &state)" in arm
    after = arm.split("render_view(surface,")[1].split("return;")[0]
    assert "surface_flip_display()" not in after, (
        "Dirty::All reaches render_view, which presents; the extra flip is the "
        "zero-draw present this contract exists to forbid"
    )


def test_the_frame_breakdown_reports_the_pixels_it_drew(main: str) -> None:
    """WHY: `px=1036800` was a constant, so it could not tell a 60-row band from a full repaint.

    A measurement that prints the same number for every frame cannot be the
    evidence for "the shell repainted the whole surface", and it was being read
    that way. The line has to carry what the beat actually wrote.
    """
    body = lines(main)
    start = next(
        n for n, line in enumerate(body) if line.startswith("fn log_frame_breakdown(")
    )
    arm = "\n".join(body[start : start + 30])
    assert "W * H" not in arm, (
        "the FRAMEBREAK px field must be the pixels this beat wrote, not the panel's "
        "resolution; a constant makes every frame look like a full-surface repaint"
    )


# ---------------------------------------------------------------------------
# B: the view a tap answers from is the view the surface holds
# ---------------------------------------------------------------------------


PAINT = ("render_view(surface", "render_band(surface", "present_dirty(surface")


def test_every_view_change_is_painted(main: str) -> None:
    """No handler may rename the view and leave the old surface standing.

    WHY: the seven offenders each launched an application and set
    `state.current_view = ViewMode::HomeScreen` with no repaint behind it, so the
    hit test moved to the home grid while the surface still held the drawer or the
    widgets dashboard. A tap on a painted tile then resolved as HomeScreen and
    launched nothing.
    """
    body = lines(main)
    offenders = []
    for n, line in enumerate(body):
        if not is_code(line) or "state.current_view = " not in line:
            continue
        window = body[n + 1 : n + 22]
        if not any(any(p in w for p in PAINT) for w in window if is_code(w)):
            offenders.append(n + 1)
    assert not offenders, (
        f"current_view written with no paint within 21 lines at: {offenders}. "
        "Ask for the view through the painted-view record instead, or paint it."
    )


def test_the_band_path_cannot_mix_two_views(main: str) -> None:
    """A band repaint of one view over the pixels of another is the mixed frame.

    WHY: the periodic status-band repaint draws rows 0..48 of whatever
    `current_view` names into a surface whose other rows belong to a different
    view. The painted-view record makes that combination impossible rather than
    merely unlikely.

    WHY this reads function bodies instead of a spelling: the claim is that *the
    band painter* is the thing that asks the record, before it writes, and that a
    banded present cannot reach the surface any other way. Asserting that some
    substring appears somewhere in `main.rs` is satisfied by a record consulted in
    a function no present ever calls, and it stayed red while the wiring was
    already correct -- a gate that matches a spelling proves nothing about the call
    graph it is meant to be describing.
    """
    band = func_body(main, "fn render_band(")

    guard = [n for n, line in enumerate(band) if is_code(line) and "band_is_safe(" in line]
    assert len(guard) == 1, (
        f"render_band must consult the painted-view record exactly once as it decides "
        f"which rows to draw; found {len(guard)} consults"
    )
    g = guard[0]
    assert re.search(r"\bif\b", band[g]), (
        "the record must *decide* the band, not merely be read: `band_is_safe` has to be "
        f"the condition of an if, and line {g} is: {band[g].strip()!r}"
    )
    writes = [
        n
        for n, line in enumerate(band)
        if is_code(line) and "surface_write_display(" in line
    ]
    assert writes, "render_band is the function that writes the band to the surface"
    assert g < writes[0], (
        "asking the record after the first write is asking it too late: the mixed rows are "
        "already in the surface"
    )

    block = band[g : g + 20]
    close = next((n for n in range(1, len(block)) if block[n].strip() in ("};", "} ;")), None)
    assert close is not None, "the band decision must be one expression that rebinds the rows"
    else_at = next(
        (n for n in range(1, close) if block[n].strip().startswith("} else {")), None
    )
    assert else_at is not None, (
        "the record's answer needs a mismatch arm; a guard with no else is a comment "
        "about the mixed frame, not a prevention of it"
    )
    keep = " ".join(line.strip() for line in block[1:else_at] if is_code(line))
    escalate = " ".join(line.strip() for line in block[else_at + 1 : close] if is_code(line))
    assert keep == "(min_y, max_y)", (
        f"when the surface already holds this view the band keeps the rows it was asked "
        f"for; its safe arm reads {keep!r}"
    )
    assert re.search(r"\bpaint_full\s*\(", escalate), (
        "a mismatch paints the whole surface, and painting it is what moves the record; "
        f"the mismatch arm reads {escalate!r}"
    )
    assert re.search(r"\(\s*0\s*,\s*H\s*\)", escalate), (
        "the escalation has to cover every row -- a widened band is still a band over "
        f"another view's pixels; the mismatch arm reads {escalate!r}"
    )

    present = func_body(main, "fn present_dirty(")
    routed = [n for n, line in enumerate(present) if is_code(line) and "render_band(" in line]
    assert len(routed) == 1, (
        "every banded present goes through render_band, the one painter that asks the "
        f"record; present_dirty calls it {len(routed)} times"
    )
    assert re.search(
        r"render_band\(\s*surface\s*,\s*state\s*,\s*y0\s*,\s*y1\s*\)", present[routed[0]]
    ), "the band present_dirty draws is the band the dirty region named"
    named = next(
        n
        for n, line in enumerate(present)
        if is_code(line) and "dirty.rows()" in line
    )
    tail = present[named:]
    assert not any(is_code(line) and "surface_write_display(" in line for line in tail), (
        "nothing below the row naming may write the surface directly: a second writer is "
        "a band that never asked the record"
    )


# ---------------------------------------------------------------------------
# D: the drawer paints the rows it was given
# ---------------------------------------------------------------------------


CONST_RE = re.compile(r"^(?:pub )?const ([A-Z][A-Z0-9_]*): usize = (.+?);$", re.M)
NUMBER_RE = re.compile(r"^\d+$")


def drawer_numbers(source: str) -> dict[str, str]:
    """Every one-line `const NAME: usize = <expression>;` in the drawer, as written.

    WHY the expression and not a value: the claim this gate makes is about where the
    number *comes from*. A resolved table would let a hand-typed `1133` pass as
    happily as a derivation, and a hand-typed row count is exactly what left 310 rows
    unpainted above the bar.
    """
    return {m.group(1): m.group(2).strip() for m in CONST_RE.finditer(source)}


def externals() -> dict[str, int]:
    """The values the drawer borrows, read from the files that own them.

    WHY read rather than hardcoded: `launcher::nav::BAND_TOP` is the committed edge
    `tests/test_serial_tcp.py` taps, and `space::XL` is the theme's. Repeating either
    as a literal here would let this gate keep passing after the thing it measures
    moved.
    """
    out: dict[str, int] = {}
    nav = NAV.read_text(encoding="utf-8", errors="replace")
    out["launcher::nav::BAND_TOP"] = int(nav.split("pub const BAND_TOP: usize = ")[1].split(";")[0])
    spacing = SPACING.read_text(encoding="utf-8", errors="replace")
    for m in re.finditer(r"pub const ([A-Z_0-9]+): usize = (\d+);", spacing):
        out.setdefault(f"space::{m.group(1)}", int(m.group(2)))
    return out


def drawer_value(name: str, consts: dict[str, str], extra: dict[str, int]) -> int:
    """Evaluate one drawer constant's arithmetic, resolving its whole dependency chain.

    Raises on any name it cannot account for, so a grid height built from a number
    this gate has never heard of fails loudly instead of evaluating to something
    plausible.
    """

    def resolve(key: str, stack: tuple[str, ...]) -> int:
        if NUMBER_RE.match(key):
            return int(key)
        if key in extra:
            return extra[key]
        expr = consts.get(key)
        assert expr is not None, f"{key} is not a `const _: usize` this gate can read"
        assert key not in stack, f"the drawer's geometry recurses through {key}"
        value = eval(
            re.sub(
                r"[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z0-9_]+)*",
                lambda m: str(resolve(m.group(0), stack + (key,))),
                expr,
            ),
            {"__builtins__": {}},
            {},
        )
        assert isinstance(value, int), f"{key} = {expr!r} is not whole rows"
        return value

    return resolve(name, ())


def drawer_chain(name: str, consts: dict[str, str]) -> set[str]:
    """Every name one drawer constant's arithmetic reads, transitively.

    WHY the chain and not the value: "GRID_H is 1133" is a number; "GRID_H is the
    distance left by the navigation band" is a claim. Only the second one keeps being
    true after someone edits the bar.
    """
    out: set[str] = set()
    pending = [name]
    while pending:
        expr = consts.get(pending.pop())
        if expr is None:
            continue
        for token in re.findall(r"[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z0-9_]+)*", expr):
            if token in consts and token not in out:
                pending.append(token)
            out.add(token)
    return out


def test_the_drawer_grid_reaches_the_navigation_band() -> None:
    """Tiles must not stop 310 rows above the bar with nothing to explain the gap.

    WHY the numbers are read back rather than asserted as literals: the grid's band,
    its dot row and the navigation bar are one stack of chrome built from `frame()`,
    and the device frames showed the last tile row ending near y=1070 against a bar at
    y=1380. A literal here would have pinned the broken layout as tightly as the fixed
    one.

    WHY the derivation is checked as well as the result: the floor below is 1300, and
    `GRID_H = 1133` would clear it as a typed number too. The claim is not "the grid is
    tall" but "the grid stops where the bar starts", so the gate follows the arithmetic
    to `launcher::nav::BAND_TOP` and refuses a chain that never gets there.
    """
    source = DRAWER.read_text(encoding="utf-8", errors="replace")
    consts, extra = drawer_numbers(source), externals()
    band_top = extra["launcher::nav::BAND_TOP"]

    grid_y = drawer_value("GRID_Y", consts, extra)
    grid_h = drawer_value("GRID_H", consts, extra)
    dots_y = drawer_value("DOTS_Y", consts, extra)
    dots_h = drawer_value("DOTS_H", consts, extra)
    bottom = grid_y + grid_h

    assert band_top == 1380, (
        f"the navigation band moved to {band_top}; this contract's silence about the bar "
        "is a promise that the drawer was fixed without touching what the runtime suites tap"
    )
    assert bottom >= 1300, (
        f"the drawer grid ends at row {bottom}, leaving {band_top - bottom} unpainted rows "
        f"above the navigation band at {band_top}"
    )
    assert dots_y >= bottom, (
        f"the dot row starts at {dots_y}, above the grid's own foot at {bottom}: two "
        "controls painted in the same rows is the mixed frame this file exists to forbid"
    )
    assert dots_y + dots_h <= band_top, (
        f"the dot band runs to {dots_y + dots_h}, under the navigation band at {band_top}"
    )
    assert "launcher::nav::BAND_TOP" in drawer_chain("GRID_H", consts), (
        "GRID_H must be derived from the navigation band's top, not typed to clear the "
        f"floor above: its arithmetic reads {consts.get('GRID_H')!r} and depends on "
        f"{sorted(drawer_chain('GRID_H', consts))}"
    )


def test_the_drawer_escape_ends_at_the_band_the_drawer_paints(main: str) -> None:
    """The tap that leaves the drawer must end where the drawer's own bar begins.

    WHY both numbers are read from their owning files: the defect was one authored
    constant, `y >= 1375` in the shell, sitting five rows above `launcher::nav::BAND_TOP`
    -- the edge the painter actually uses. Those five rows are dead space that drops
    the person out of the drawer over pixels belonging to no control. Pinning `1375`
    or `1380` as a literal here would re-author the constant a third time; the gate
    instead resolves the escape's comparison against the band the drawer paints, so
    the two cannot disagree and still pass.
    """
    band_top = externals()["launcher::nav::BAND_TOP"]
    body = lines(main)
    escapes = []
    for n, line in enumerate(body):
        if not is_code(line):
            continue
        m = re.search(r"\belse if y >= (.+?) \{", line)
        if not m:
            continue
        # The arm's body, comments skipped: what the comparison decides with.
        window = " ".join(
            l for l in body[n + 1 : n + 12] if is_code(l) and not l.strip().startswith("//")
        )
        if "ViewMode::HomeScreen" in window and "render_view(" in window:
            escapes.append((n + 1, m.group(1).strip()))
    assert escapes, (
        "the drawer's tap-ends-the-view rule is gone from the shell; if the escape "
        "moved, re-aim this gate at the new comparison -- do not delete it"
    )
    for n, expr in escapes:
        if expr == "nav::BAND_TOP":
            value = band_top
        elif NUMBER_RE.match(expr):
            value = int(expr)
        else:
            raise AssertionError(
                f"main.rs:{n} compares y against {expr!r}, which this gate cannot "
                "resolve to a row; derive it from the band the drawer paints"
            )
        assert value == band_top, (
            f"main.rs:{n} ends the drawer at row {value}, but the navigation band the "
            f"drawer paints starts at {band_top}: rows "
            f"{min(value, band_top)}..{max(value, band_top)} are dead space that "
            "silently drops the person out of the drawer"
        )


def test_the_painted_drawer_search_field_answers_taps(main: str) -> None:
    """A search field the drawer paints must be the field the shell's tap resolves.

    WHY the helper body and the call site are both read: the screen inked a search
    pill from `frame().search` while the drawer's tap arm answered chips, dots and
    tiles and nothing else -- a visible control that does nothing on a touch-only
    device. The fix is only real if the hit test asks the same rect the painter used
    (not a re-typed band) and the shell's drawer arm routes that answer into the
    same search opening the home screen's bar uses.
    """
    drawer = DRAWER.read_text(encoding="utf-8", errors="replace")
    painted = [
        line
        for line in lines(drawer)
        if is_code(line) and re.search(r"if f\.search\.contains\(x, y\)", line)
    ]
    assert painted, (
        "the drawer stopped painting its search field from frame().search; if the "
        "field was removed rather than wired, re-aim this gate -- do not delete it"
    )
    helper = func_body(drawer, "pub fn app_drawer_search_at(")
    assert re.search(r"\bframe\(\)\s*\.\s*search\b|\bf\.search\b", " ".join(helper)), (
        "app_drawer_search_at must resolve the tap against the drawer's own frame "
        f"rect, not a re-typed band; its body reads: {' '.join(helper).strip()!r}"
    )
    body = lines(main)
    calls = [
        n
        for n, line in enumerate(body)
        if is_code(line) and "app_drawer_search_at(" in line and "fn app_drawer_search_at" not in line
    ]
    assert calls, (
        "no tap handler in the shell asks the drawer's painted search field; the "
        "control is inert again"
    )
    window = " ".join(line for line in body[calls[0] : calls[0] + 14] if is_code(line))
    for needed in ("refresh_search_index", "ViewMode::Search", "render_view("):
        assert needed in window, (
            f"the drawer's search-field tap must open the search through the same "
            f"path the home bar uses; {needed!r} is missing near main.rs:{calls[0] + 1}"
        )


def test_no_blocking_ai_probe_on_the_keystroke_path(main: str) -> None:
    """Ranking a keystroke may only use an AI answer that has already arrived.

    WHY the real bodies are read: `rank_search_results` ran on every keypress and
    each run issued `probe_system_ai_status` twice -- once directly and once inside
    `search_ranker` -- and the probe ends in a blocking `receive_any` IPC round
    trip. Typing therefore waited on a service that may never answer. The gate
    demands the keystroke function ask a cache, demands the cache be bounded by
    staleness and default to Unavailable (no answer is never reported as semantic
    AI), and demands the ranker factory take the status instead of probing again.
    """
    rank = " ".join(func_body(main, "fn rank_search_results("))
    assert "probe_system_ai_status(" not in rank, (
        "rank_search_results probes the AI service again: every keystroke pays an "
        "IPC round trip that may never be answered"
    )
    assert "cached_ai_status()" in rank, (
        "rank_search_results must ask the cached AI answer, not the service"
    )
    ranker = " ".join(func_body(main, "fn search_ranker("))
    assert "probe_system_ai_status(" not in ranker, (
        "search_ranker probes again behind the caller's back, doubling the blocking "
        "round trip per keystroke"
    )
    cached = " ".join(func_body(main, "fn cached_ai_status("))
    assert "AI_PROBE_STALE_TICKS" in cached, (
        "the cache must carry an explicit staleness bound, not live forever"
    )
    assert "current_ticks" in cached, "staleness has to be measured against the clock"
    assert "Unavailable(" in cached, (
        "with no fresh answered probe the cache must report Unavailable; claiming "
        "Available without an answer is the fabricated-capability the platform "
        "contract forbids"
    )
    m = re.search(r"const AI_PROBE_STALE_TICKS: u64 = ([\d_]+);", main)
    assert m, "the staleness bound is not a whole-tick constant this gate can read"
    assert 0 < int(m.group(1).replace("_", "")) < 600_000, (
        f"AI_PROBE_STALE_TICKS = {m.group(1)} is not a bound, it is amnesia or a lie"
    )


def test_the_drawer_pages_from_the_capacity_it_paints(main: str) -> None:
    """The swipe-to-next-page rule must divide by the tiles a page actually holds.

    WHY: it read `(state.app_count + 11) / 12` while the painter laid out
    `drawer_columns * DRAWER_ROWS`, so a swipe could turn to a page the dot row said
    did not exist -- and a page the grid never painted could not be swiped back from.

    WHY both halves: the absence of one spelling is not the invariant. A re-typed
    divisor with the same defect (`+ 17) / 18` hard-wired, say) would pass a
    not-in test, so every page binding in the shell has to be answered by the screen
    that draws the tiles.
    """
    assert "(state.app_count + 11) / 12" not in main, (
        "page count must come from app_drawer_per_page, the same capacity the grid paints"
    )
    body = lines(main)
    bindings = [
        (n + 1, line.strip())
        for n, line in enumerate(body)
        if is_code(line) and re.search(r"\blet pages\b", line)
    ]
    assert bindings, "the shell pages the drawer somewhere; if that changed, re-aim this gate"
    unbound = [
        (n, text)
        for n, text in bindings
        if "app_drawer_page_count(" not in text and "app_drawer_per_page(" not in text
    ]
    assert not unbound, (
        f"pages computed without the drawer's own capacity at: {unbound}"
    )
