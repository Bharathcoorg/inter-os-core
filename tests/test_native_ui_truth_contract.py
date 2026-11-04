# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

import re
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]


def _rust_fn(source: str, name: str) -> str:
    """The text of one Rust function, found by its `fn NAME(` and brace matching.

    WHY slice rather than `split` on `"\n}\n"`: a body's own closing braces sit at
    other indents, and the first line that happens to be `}` at column zero ends the
    extraction wherever it likes. These contracts assert that a piece of code does
    NOT appear inside a particular function, so where the slice stops is load
    bearing -- an extraction that stops early proves nothing about the rest of the
    body and an extraction that runs long fails on prose in the next function.
    """
    start = source.index(f"fn {name}(")
    open_brace = source.index("{", start)
    depth = 0
    for index in range(open_brace, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[start:index + 1]
    raise AssertionError(f"`fn {name}` never closes")


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def test_messages_empty_state_has_non_overlapping_text_and_no_dead_action():
    source = read("apps/messages/src/main.rs")
    layout = read("apps/messages/src/lib.rs")
    assert 'b"NO TRANSPORT"' in source
    # WHY the sentence is read from the layout module and the choice from the
    # painter: the empty state's lines now sit beside the boxes that hold them, so
    # grepping the painted bytes out of `main.rs` passed on a screen that no longer
    # spells them anywhere. Both halves are still pinned -- that the sentence
    # exists, and that the branch a device with no transport takes is the one that
    # paints it.
    assert 'pub const EMPTY_TRANSPORT_SUBLINE' in layout
    assert 'b"Messaging unavailable: no transport"' in layout
    assert "_ => (layout::EMPTY_TRANSPORT_SUBLINE," in source
    assert 'b"P2P E2EE"' not in source
    assert 'b"Encrypted P2P Wire Client Active"' not in source
    assert 'b"Hardware Keystore Slot 3 Ready"' not in source
    # The scale-2 title occupies a 48px glyph line. Status lines must reserve
    # that space, and an unavailable backend must not expose a dead action.
    assert "layout::EMPTY_THREADS_TITLE," in source
    assert "pub const EMPTY_THREADS_TITLE: &[u8] = b\"No Conversations Yet\";" in layout
    assert '740' in source
    assert '810' in source
    assert 'b"+ Start New Chat"' not in source


def test_file_manager_does_not_render_fabricated_clock_battery_or_storage_usage():
    source = read("apps/file_manager/src/main.rs")
    assert 'b"09:20"' not in source
    assert 'b"100%"' not in source
    assert 'b"42 GB / 128 GB"' not in source
    assert 'b"Capacity detected; usage unavailable"' in source
    assert 'b"Storage capacity unavailable"' in source


def test_device_studio_does_not_turn_presence_or_io_counters_into_fake_percentages():
    source = read("emulator/app.js")
    assert "fillRamBar.style.width = '0%'; // No guest RAM-usage metric is currently exposed." in source
    assert "fillBlockBar.style.width = '0%'; // IO counters do not measure utilization." in source
    assert "ops > 0 ? '100%' : '50%'" not in source


def test_device_studio_does_not_advertise_unobserved_display_modes_or_planes():
    source = read("emulator/index.html")
    assert "LTPO 120Hz VRR" not in source
    assert "0, 0, 720, 1280" not in source
    assert "Mobile Shell Activity Space" not in source
    assert "No live compositor-plane telemetry is currently exposed by the guest." in source


def test_browser_has_guest_pointer_and_keyboard_bridges():
    source = read("emulator/app.js")
    assert "touchSurface.addEventListener('pointerdown'" in source
    assert "touchSurface.addEventListener('pointermove'" in source
    assert "touchSurface.addEventListener('pointerup'" in source
    assert "touchSurface.addEventListener('pointercancel'" in source
    assert "window.addEventListener('keydown', async (e) =>" in source
    assert "fetch('/api/input/key'" in source
    assert "fetch('/api/touch/event'" in source


def _const_usize(source: str, name: str) -> int:
    """Reads a `const NAME: usize = ...;` binding out of a Rust source file.

    WHY parse rather than assert on literals: these contracts exist to catch a
    layout that puts two runs on top of each other. Asserting that a literal
    like "76" is still present would pass just as happily when the run moved
    somewhere worse. Reading the constants back and comparing the resulting
    geometry is the only form of this assertion that means anything.

    A binding written as an expression of other bindings -- `PILLS_Y + PILL_H`,
    which is how the filter note is positioned so it cannot drift from the pill
    row -- is resolved by looking those names up in turn.
    """
    import re

    match = re.search(
        rf"^\s*(?:pub )?const {name}\s*:\s*usize\s*=\s*([^;]+);", source, re.M
    )
    assert match, f"could not find `const {name}: usize` in the source"
    expr = match.group(1).strip()

    resolved = _resolve_usize(source, expr, depth=0)
    assert resolved is not None, f"`const {name}` = {expr!r} is not a resolvable usize expression"
    return resolved


def _resolve_usize(source: str, expr: str, depth: int) -> int | None:
    """Evaluates a const expression made of ints, names, `+`, `-` and `*`."""
    assert depth < 8, "const expression nests too deeply to resolve"

    import re

    expr = expr.strip()
    if expr.isdigit():
        return int(expr)

    # A single `+` at the top level, resolved left then right.
    for operator in ("+", "-", "*"):
        parts = _split_top_level(expr, operator)
        if len(parts) == 2:
            left = _resolve_usize(source, parts[0], depth + 1)
            right = _resolve_usize(source, parts[1], depth + 1)
            if left is None or right is None:
                return None
            if operator == "+":
                return left + right
            if operator == "-":
                return left - right
            return left * right

    match = re.match(r"^([A-Z_][A-Z0-9_]*)$", expr)
    if match:
        nested = re.search(
            rf"^\s*(?:pub )?const {match.group(1)}\s*:\s*usize\s*=\s*([^;]+);", source, re.M
        )
        if not nested:
            return None
        return _resolve_usize(source, nested.group(1), depth + 1)
    return None


def _split_top_level(expr: str, operator: str) -> list[str]:
    """Splits on `operator` only where it is not inside a parenthesis pair."""
    depth = 0
    parts, start = [], 0
    for i, ch in enumerate(expr):
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        elif ch == operator and depth == 0:
            parts.append(expr[start:i])
            start = i + 1
    parts.append(expr[start:])
    return [p for p in parts if p.strip()]


# ---------------------------------------------------------------------------
# The platform's type ladder, read off the Rust side
# ---------------------------------------------------------------------------
#
# WHY parsed and not written down: the row box a rung reserves is what every
# overlap assertion below is measured in, and `font::line_height` is its only
# authority. This file used to carry its own numbers for those boxes, and when
# the rungs rose the copy went on reserving 32 rows for a body and 22 for a
# caption while the shell paid 36 and 27 -- so the checks passed against a
# snapshot of the ladder and said nothing about the layout on the device. The
# chain the SDK solves -- rung -> cap height -> em -> ascent + descent -- is
# re-solved here from the same four sources it runs on:
#   font::cap_for_scale                   sdk/rust/interos-std/src/font.rs
#   outline_text::em_for_cap_height       .../src/design/outline_text.rs
#   typeface::line_metrics                .../src/design/typeface.rs
#   face::{ASCENT, DESCENT, UNITS_PER_EM}  .../src/design/face.rs
# .../src/design/mod.rs::typography is where each named role's rung is declared.


def _fn_body(src: str, signature: str) -> str:
    """The body of a `const fn`, given its signature verbatim.

    WHY the signature rather than a looser pattern: the reader here is a copy of
    the SDK's arithmetic. If that arithmetic is re-plumbed the signature stops
    matching and this fails loudly; a silent fallback would put the assertions
    back to measuring a ladder that no longer exists.
    """
    match = re.search(re.escape(signature) + r" \{(.*?)\n\}", src, re.S)
    assert match, f"{signature} is no longer shaped the way this contract reads it"
    return match.group(1)


def _cap_ladder() -> tuple[dict[int, int], int]:
    """`font::cap_for_scale`: the rung -> cap-height table and its fallback step."""
    body = _fn_body(read("sdk/rust/interos-std/src/font.rs"),
                    "pub const fn cap_for_scale(scale: usize) -> u32")
    ladder = {int(rung): int(cap)
              for rung, cap in re.findall(r"^\s*(\d+)\s*=>\s*(\d+)", body, re.M)}
    assert ladder, "font::cap_for_scale answers no rung"
    fallback = re.search(r"\bs\s*=>\s*\(s as u32\)\.saturating_mul\((\d+)\)", body)
    assert fallback, "font::cap_for_scale no longer answers an unknown rung by scaling it"
    return ladder, int(fallback.group(1))


def _em_arithmetic() -> tuple[int, int, int]:
    """`outline_text::em_for_cap_height` as (cap * UNITS + ROUND) // CAP_HEIGHT."""
    body = _fn_body(read("sdk/rust/interos-std/src/design/outline_text.rs"),
                    "pub const fn em_for_cap_height(cap_px: u32) -> u32")
    expr = re.search(r"\(\(cap_px as u64 \* (\d+) \+ (\d+)\) / (\d+)\)", body)
    assert expr, ("outline_text::em_for_cap_height is no longer the arithmetic this "
                  "contract solves, so the ladder has to be re-read here")
    return tuple(int(group) for group in expr.groups())


def _face_metrics() -> tuple[int, int, int]:
    """`face`'s metrics, with `typeface::line_metrics`' rule pinned beside them."""
    body = _fn_body(read("sdk/rust/interos-std/src/design/typeface.rs"),
                    "pub const fn line_metrics(px_per_em: u32) -> (u32, u32)")
    assert "let upem = face::UNITS_PER_EM as i32;" in body
    assert "(face::ASCENT as i32 * px_per_em as i32) / upem" in body
    assert "(-(face::DESCENT as i32) * px_per_em as i32) / upem" in body
    face = read("sdk/rust/interos-std/src/design/face.rs")
    upem = int(re.search(r"pub const UNITS_PER_EM: u16 = (\d+);", face).group(1))
    ascent = int(re.search(r"pub const ASCENT: i16 = (-?\d+);", face).group(1))
    descent = int(re.search(r"pub const DESCENT: i16 = (-?\d+);", face).group(1))
    return upem, ascent, descent


_LADDER, _LADDER_FALLBACK = _cap_ladder()
_EM_UNITS_PER_EM, _EM_ROUNDING, _EM_CAP_UNITS = _em_arithmetic()
_FACE_UPM, _FACE_ASCENT, _FACE_DESCENT = _face_metrics()


def _line_height(rung: int) -> int:
    """`font::line_height(rung)`: the outline face's box at that rung, in rows."""
    if rung == 0:
        return 0
    cap = _LADDER.get(rung, rung * _LADDER_FALLBACK)
    em = (cap * _EM_UNITS_PER_EM + _EM_ROUNDING) // _EM_CAP_UNITS
    return (_FACE_ASCENT * em) // _FACE_UPM + ((-_FACE_DESCENT) * em) // _FACE_UPM


def _rung(role: str) -> int:
    """The rung the SDK's `typography` module gives a named role."""
    return _const_usize(read("sdk/rust/interos-std/src/design/mod.rs"), f"{role}_SCALE")


def test_events_header_bands_cannot_overlap_each_other_or_the_feed():
    """The Events header is four runs stacked above the first section head.

    WHY this needs asserting mechanically: the header owns a fixed band, and
    every line is placed from a literal. Moving any one of them down by a few
    pixels is invisible in review and prints two runs over each other on the
    device. This has already happened twice -- once when three prose lines were
    added to the header and ran into the pill row, and once when the filter note
    was nudged down and landed on the first section head.
    """
    source = read("apps/launcher/src/screens/notifications.rs")

    title_y = _const_usize(source, "TITLE_Y")
    subtitle_y = _const_usize(source, "SUBTITLE_Y")
    source_note_y = _const_usize(source, "SOURCE_NOTE_Y")
    pills_y = _const_usize(source, "PILLS_Y")
    pill_h = _const_usize(source, "PILL_H")
    filter_note_y = _const_usize(source, "FILTER_NOTE_Y")
    section_head_y = _const_usize(source, "SECTION_HEAD_Y")

    # The rows each band reserves: `font::line_height` at the rung that band
    # draws at -- ascent plus descent, which is the box `LabelRun` clips its ink
    # to. The old pair here was the bitmap cell's (24 for a caption, 48 for a
    # body), which under-reserved the HEADING title by 32 rows and so passed the
    # very overlap this test was written to catch.
    #
    # WHY read from the SDK rather than written down: the authority for these
    # boxes is `font::line_height` at the rung the paint site below uses, and the
    # shell pins each of its own literals to that answer with a
    # `const _: () = assert!(...)` -- `TITLE_ADVANCE` to the HEADING box, the
    # section note to `SOURCE_NOTE_Y + line_height(CAPTION)`, `SECTION_LABEL_BOX_H`
    # to the caption box. A copy of the numbers here would lag the ladder by
    # exactly the margin this test exists to measure, so each rung is read off the
    # source instead: heading 80 rows today, body 36, caption 27.
    heading_h = _line_height(_rung("HEADING"))
    body_h = _line_height(_rung("BODY"))
    caption_h = _line_height(_rung("CAPTION"))

    # The rungs above are asserted, not assumed: each band's box is only the
    # right amount to reserve if the surface really paints that band at it.
    assert "LabelRun::new(HEADER_X, TITLE_Y, type_scale::HEADING," in source, (
        "the header title no longer draws at the HEADING rung this test reserves"
    )
    assert "        SUBTITLE_Y,\n        type_scale::BODY," in source, (
        "the header subtitle no longer draws at the BODY rung this test reserves"
    )
    assert "        SOURCE_NOTE_Y,\n        type_scale::CAPTION," in source, (
        "the source note no longer draws at the CAPTION rung this test reserves"
    )
    assert "        FILTER_NOTE_Y,\n        type_scale::CAPTION," in source, (
        "the filter note no longer draws at the CAPTION rung this test reserves"
    )
    assert "launcher::font::line_height(type_scale::BODY) + space::GRID_4" in source, (
        "the step from the subtitle to the source note no longer pays for the "
        "whole body box, so the two runs share rows"
    )

    assert title_y + heading_h <= subtitle_y, "title runs into the subtitle"
    assert subtitle_y + body_h <= source_note_y, "subtitle runs into the source note"
    assert source_note_y + caption_h <= pills_y, "source note runs into the pill row"
    assert filter_note_y >= pills_y + pill_h, "filter note starts inside the pill row"
    assert filter_note_y + caption_h <= section_head_y, "filter note runs into the first section head"

    # The section heading and the first card of its own group.
    #
    # WHY this is a separate pair of assertions: the header runs above all end
    # at the pill row's note, and the feed below begins at a card. Between them
    # sits a heading with a line box of its own, which nothing here used to
    # account for. The advance from the heading to its cards was `space::MD`
    # measured from the heading's box *top*, so the card began six rows before
    # the box ended -- and with no clipper on this surface the card won the
    # pixels and printed the heading's baseline out. "Last hour" reached the
    # device missing its bottom four rows.
    label_box_h = _const_usize(source, "SECTION_LABEL_BOX_H")
    head_advance = _const_usize(source, "SECTION_HEAD_ADVANCE")
    row_h = _const_usize(source, "ROW_H")
    feed_bottom = _const_usize(source, "FEED_BOTTOM")

    assert label_box_h == caption_h, (
        "the section heading's box is no longer the caption line box this test assumes"
    )
    assert head_advance > label_box_h, (
        "the section advance is measured from the heading's box top, so the card "
        "starts inside the heading it names"
    )
    assert section_head_y + head_advance + row_h <= feed_bottom, (
        "the first section's card does not fit between its own heading and the action bar"
    )


def test_the_shade_asks_to_clear_its_notifications_once():
    """One destructive control, in one place, on the notifications shade.

    WHY this needs a gate: the shade carried two pills that both read "Clear
    all" and both emptied the event store -- the header's own, and a wider one
    fixed at row 702 in the middle of the feed. Nothing on the surface says
    which of two identical answers the machine meant, and the second one sat
    below a feed that fits two cards, so it read as an item in the list rather
    than as a control over it.

    WHY a source gate: the shell is `#![no_main]` and its bin target carries
    `test = false`, so a `#[cfg(test)]` module under `screens/` never runs on
    the host. The label the painter draws is the detectable fact -- that string
    appears once per control that carries it.
    """
    code = _code_only(read("apps/launcher/src/screens/notifications.rs"))
    assert code.count('b"Clear all"') == 1, (
        "the shade draws the same destructive question more than once"
    )
    # A second rectangle laid out for it would be a second control even if its
    # label were reworded, so the old binding's name is what stays banned. The
    # feed's own edge is pinned against the home indicator by a `const _: () =
    # assert!(...)` in the source, which is where the screen height lives.
    assert "CLEAR_BTN" not in code.replace("HEADER_CLEAR_BTN", ""), (
        "a second clear control is still laid out on this surface"
    )


def test_events_header_does_not_print_platform_diagnostics_to_the_reader():
    """The header may state where the rows come from, in plain language.

    WHY: the three lines it used to print named the tick counter, the missing
    calendar and the two stores that contribute nothing. Those are notes to a
    reviewer of the file. The platform detail stays in the module header.
    """
    source = read("apps/launcher/src/screens/notifications.rs")
    # The prose may not be in a byte-string literal: a `b"..."` is what the
    # surface actually draws. The module header is allowed to explain the
    # platform, and does.
    import re

    drawn = re.findall(r'b"([^"]*)"', source)
    joined = " | ".join(drawn)
    for diagnostic in (
        "guest ticks",
        "no calendar",
        "no day groups",
        "Registry and recents",
    ):
        assert diagnostic not in joined, f"the screen still prints {diagnostic!r}"
    # The feed's own empty state is allowed to name what it is empty of -- that
    # is the user reading their own device, not a note to a reviewer. Only the
    # header prose was the problem.
    assert "Recent activity from your apps and system." in joined


def _store_bound(kernel_src: str) -> int:
    """How many notification records the store will hold at once.

    WHY read from the kernel rather than written down here: the figures every
    notification surface states are bounded by that ceiling, and a gate that typed
    "64" would still pass the day the store grew and the shell's runs started
    clipping their own headline number.
    """
    match = re.search(r"max_active:\s*(\d+)", kernel_src)
    assert match, "the notification store no longer bounds what it holds"
    return int(match.group(1))


def _count_figure_defects(shell: str, home: str, widgets: str, shade: str) -> list:
    """Where the shell's notification arithmetic says one number and means another.

    WHY this is a rule with a gate behind it: the shell cached eight records and
    carried one figure for both the store's total and the rows it could draw, so the
    home subtitle clamped that figure to eight while the Today card and the dashboard
    card printed it raw -- three surfaces of one device, and none of them wrong in
    the same way. The feed then measured what it had hidden against the cache, so a
    device holding thirty notices and showing eight said nothing about the other
    twenty-two. A person reads a count on the home screen as a fact about their
    device; it has to be the store's own answer, and the feed has to say which of
    those answers it actually drew.
    """
    defects = []
    refresh = _code_only(_rust_fn(shell, "refresh_notifications"))
    if "state.notification_records_held = held;" not in refresh:
        defects.append("the poll keeps one figure for both the store's total and the rows it can draw")
    if "state.notification_count = std::notification_query(uid, locked).unwrap_or(0).max(held);" not in refresh:
        defects.append("the count every surface states is not the store's own answer")
    for name, source in (("the home screen", home), ("the dashboard", widgets),
                         ("the shade", shade), ("the shell", shell)):
        if "notification_count.min(" in _code_only(source):
            defects.append(f"{name} clamps the store's count to the shell's own cache before stating it")
    if "let count = state.notification_records_held;" not in _code_only(shade):
        defects.append("the feed decides how many rows to draw from a figure that can exceed its records")
    if "state.notification_count.saturating_sub(row_count)" not in _code_only(shade):
        defects.append("the shade measures its 'more records not shown' figure against the cache, "
                       "so a store larger than the cache reports nothing hidden")
    if "if state.notification_count == 0 {" not in _code_only(shade):
        defects.append("the shade calls itself empty because its cache is empty, not because the store is")
    for name, source, needle in (
        ("home subtitle", home, "push_decimal(&mut subtitle_buf, 0, state.notification_count)"),
        ("Today card", home, "push_decimal(out, 0, state.notification_count)"),
        ("dashboard card", widgets, "push_dec(&mut notif_buf, 0, state.notification_count as u64)"),
    ):
        if needle not in _code_only(source):
            defects.append(f"the {name} no longer states the store's own count, so it is free to "
                           "disagree with the shade that lists the same records")
    return defects


def test_the_notification_count_is_the_same_figure_on_every_surface():
    """Home, dashboard and shade all state the store's total, and the feed draws its cache.

    WHY a source gate: the shell is `#![no_main]` and its bin target carries
    `test = false`, so no `#[cfg(test)]` module over these painters ever runs on the
    host. Which field a run is formatted from is the detectable fact.
    """
    defects = _count_figure_defects(
        read("apps/launcher/src/main.rs"),
        read("apps/launcher/src/screens/home.rs"),
        read("apps/launcher/src/screens/live_widgets.rs"),
        read("apps/launcher/src/screens/notifications.rs"),
    )
    assert not defects, defects


def test_the_widest_count_the_store_can_state_fits_the_runs_that_state_it():
    """The digit budget of every count run is measured against the store's own ceiling.

    WHY now, and not when it breaks: the count used to be clamped to eight, which is
    one glyph, and the home subtitle formatted it with `b'0' + cnt as u8` -- a
    single byte that would have written a colon for ten. The figure is now the
    store's total, so the widest total has to fit the buffer it is written into and
    the card lines that carry it.
    """
    home = read("apps/launcher/src/screens/home.rs")
    widgets = read("apps/launcher/src/screens/live_widgets.rs")
    widest = _store_bound(read("kernel/src/notification/mod.rs"))

    subtitle_words = re.findall(r'b"(\d* ?notifications? waiting)"', home)
    assert subtitle_words, "the home subtitle no longer states a notification count in words"
    subtitle_room = re.search(r"let mut subtitle_buf = \[0u8; (\d+)\];", home)
    assert subtitle_room, "the home subtitle no longer formats into a fixed stack buffer"
    longest_subtitle = max(len(f"{widest}{run}") for run in subtitle_words)
    assert longest_subtitle <= int(subtitle_room.group(1)), (
        f"the widest notification count the store can state, plus {subtitle_words!r}, is "
        f"{longest_subtitle} bytes and the subtitle buffer holds {subtitle_room.group(1)}"
    )

    card_words = re.findall(r'b"( ?unread notifications)"', home)
    assert card_words, "the Today card no longer states a notification count in words"
    card_room = _const_usize(home, "CARD_DETAIL_CAP")
    longest_card = max(len(f"{widest}{run}") for run in card_words)
    assert longest_card <= card_room, (
        f"the widest count the store can state is {longest_card} bytes on the Today card, "
        f"whose detail line holds {card_room}"
    )
    reading_room = re.search(r"let mut notif_buf = \[0u8; (\d+)\];", widgets)
    assert reading_room, "the dashboard's alert-channel card no longer formats into a fixed buffer"
    assert len(f"{widest} active") <= int(reading_room.group(1)), (
        "the dashboard card clips its own count"
    )


def test_the_count_detector_catches_the_one_figure_it_exists_for():
    """Every arm that puts the two figures back into one is refused, and prose is not.

    WHY the gate tests itself: a pin nobody has seen fail is a pin that may be
    asserting nothing. Each arm below is the code this shell actually used to carry,
    put back by hand one line at a time.
    """
    shell = read("apps/launcher/src/main.rs")
    home = read("apps/launcher/src/screens/home.rs")
    widgets = read("apps/launcher/src/screens/live_widgets.rs")
    shade = read("apps/launcher/src/screens/notifications.rs")
    live = (shell, home, widgets, shade)
    assert not _count_figure_defects(*live), "the detector fires on the shell as it stands"
    # A comment inside the very body the detector scans, quoting the figure it
    # forbids in code, must not read as the fault -- and must not read as the fix.
    prose = shell.replace(
        "// WHY a refused record read leaves this at zero and the old rows undrawn: the",
        "// notification_count.min(state.notifications.len()) -- WHY a refused read leaves this zero, the",
    )
    assert prose != shell, "the prose control no longer matches the shell it edits"
    assert not _count_figure_defects(prose, home, widgets, shade), (
        "the detector reads a comment as the arithmetic it explains"
    )

    # Each arm is one line of the arithmetic put back the way this shell used to
    # write it. The needle is asserted before the replace so an arm that has drifted
    # away from the source it claims to edit fails here instead of passing silently.
    for label, name, needle, replacement in (
        ("the poll folding the cache length back over its own figure", "shell",
         "state.notification_records_held = held;", "state.notification_count = held;"),
        ("the count read off the records instead of asked of the store", "shell",
         "state.notification_count = std::notification_query(uid, locked).unwrap_or(0).max(held);",
         "state.notification_count = held;"),
        ("the home subtitle clamped to the cache again", "home",
         "push_decimal(&mut subtitle_buf, 0, state.notification_count);",
         "push_decimal(&mut subtitle_buf, 0, state.notification_count.min(8));"),
        ("the feed drawn from a figure that outruns its records", "shade",
         "let count = state.notification_records_held;",
         "let count = state.notification_count.min(state.notifications.len());"),
        ("the shade hiding its overflow in cache terms", "shade",
         "state.notification_count.saturating_sub(row_count)",
         "cached.saturating_sub(row_count)"),
        ("the shade calling an unread cache empty", "shade",
         "if state.notification_count == 0 {", "if cached == 0 {"),
        ("the dashboard card inventing its own figure", "widgets",
         "push_dec(&mut notif_buf, 0, state.notification_count as u64);",
         "push_dec(&mut notif_buf, 0, state.app_count as u64);"),
    ):
        current = {"shell": shell, "home": home, "widgets": widgets, "shade": shade}[name]
        assert current.count(needle) == 1, f"the arm for {label!r} does not match the source it edits"
        mutated = {**{"shell": shell, "home": home, "widgets": widgets, "shade": shade},
                   name: current.replace(needle, replacement)}
        assert _count_figure_defects(
            mutated["shell"], mutated["home"], mutated["widgets"], mutated["shade"]
        ), f"the detector does not catch {label}"


def test_recents_cards_show_the_real_pixels_or_say_why_there_are_none():
    """Recents must not paint a picture it never received.

    WHY this changed: the old renderer drew a per-app fake screen -- traffic
    lights, a keypad, chat bubbles, saturated fills from no design token file
    -- and put "No live preview" in the corner of the drawing, so the surface
    showed a convincing picture of an app that had never been seen. That was
    the fiction the contract was written against.

    The rework replaced the fiction with the platform's own pixels. The kernel
    holds every client's surface, and a client that holds the
    `SurfaceCapture` capability may read a running app's surface; the shell
    holds that capability and the task switcher shows what it gets. When the
    platform will not answer -- the app has not drawn a frame, the app has no
    surface, the capture was refused -- the card names the reason instead of
    drawing anything.

    So the contract is now: the stand-in renderer is gone, the pixels come
    from a capture, and the card has honest wording for the case where there
    is nothing to show.
    """
    source = read("apps/launcher/src/screens/activity.rs")
    for fabricated in (
        "0xFFEF4444",
        "0xFFF59E0B",
        "0xFF10B981",
        "org.interos.browser",
    ):
        assert fabricated not in _code_only(source), f"recents still draws fabricated preview art: {fabricated}"
    # The per-app stand-in renderer is gone: there is no archetype any more.
    assert "render_mini_preview" not in _code_only(source)
    # The picture comes from the platform, through the capture capability.
    assert "std::surface_capture" in _code_only(source)
    # And the card says so when there is no picture to show.
    assert 'b"No preview"' in source
    assert "the app has not drawn a frame yet" in source
    assert "the shell may not read this app's screen" in source


def test_the_search_offers_only_destinations_that_exist():
    """The search must not offer a destination that is not there.

    WHY: the launcher's search field said it searched "apps, settings & files"
    and it did not. Tapping it opened a launcher-drawn copy of the Settings
    app's own page under a second name -- "System Space" -- so the result set
    held an entry that re-opened the Settings app while claiming to be
    something else. A result that opens something other than what its label
    says is a result that lies.

    So the contract is now: there is no "System Space" result, the apps come
    from the package registry, the settings surfaces come from the platform's
    own catalogue, the files come from the filesystem listing, and each
    category says whether it matched, found nothing, or could not be read.
    """
    search = read("apps/launcher/src/screens/search.rs")
    shell = read("apps/launcher/src/main.rs")
    for source in (search, shell):
        assert "System Space" not in _code_only(source), "the search still offers the System Space duplicate"
    # The three sources are the platform's own.
    assert "std::package_registry" in _code_only(shell)
    assert "std::fs_list" in _code_only(shell)
    assert "interos_std::settings::Section::ALL" in _code_only(shell)
    # And the model-backed ranking seam is marked as such.
    assert "pub trait Ranker" in read("apps/launcher/src/search.rs")
    assert "fn search_ranker()" in _code_only(shell)


def _fn_body(source: str, signature: str) -> str:
    """The braced body that follows `signature`, as far as its own braces close.

    WHY this exists: a gate written against a whole file can be satisfied from
    a comment or from a different function, and the launcher screens carry both
    in quantity. Cutting the one function out keeps the gate about the code.
    """
    at = source.find(signature)
    assert at >= 0, f"the source no longer has {signature}"
    open_at = source.find("{", at)
    assert open_at >= 0, f"{signature} has no body"
    depth = 0
    for index in range(open_at, len(source)):
        char = source[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source[open_at + 1 : index]
    raise AssertionError(f"{signature} has an unbalanced body")


def test_the_search_fields_magnifier_draws_a_lens_and_not_a_hook():
    """The query field's pictogram has to read as the thing it names.

    WHY: the lens was a radius band with a quadrant test bolted onto it --
    `d2 >= 64 && d2 <= 144 && dx <= 0 && dy <= 0` -- so three quarters of the
    circle were never painted, and the handle was a
    `|dx - 8| + |dy - 8| <= 10` band, which fills a lozenge rather than drawing
    a stem. On the device the field showed a stray arc sitting over a blob,
    while the home screen's field one tap upstream -- the same control, opening
    the same query -- drew a closed ring over a diagonal stem. A pictogram that
    changes shape between the two screens that carry it reads as a rendering
    fault, not as a style.

    WHY a source gate: the shell is `#![no_main]` and its bin target carries
    `test = false`, so nothing inside `screens/` can be reached from a host
    test. The detector is the quadrant itself -- a ring band that names `dx` or
    `dy` at all is an arc, and a stem that does not bound `dx - dy` is a blob.
    """
    search = _code_only(read("apps/launcher/src/screens/search.rs"))
    body = _fn_body(search, "fn magnifier")
    ring = [line for line in body.splitlines() if line.strip().startswith("if") and "d2" in line]
    assert ring, "the lens is no longer a radius band, so this gate has lost its subject"
    for line in ring:
        assert "dx" not in line and "dy" not in line, (
            "the lens band is quadrant-limited, so the ring cannot close: "
            f"{line.strip()}"
        )
    assert "(dx - dy).abs()" in body, (
        "the handle is no longer a diagonal stem, so it is filling a shape again"
    )


def test_browser_start_page_does_not_render_a_hardcoded_news_feed_or_ranking():
    """The start page may not present invented pages as if the device knew them.

    WHY: both lists it used to draw were literals. "Most Visited" named eight
    sites on a browser with no visit counter, and "Discover" named four
    publications that do not exist with ages measured against nothing.
    """
    source = read("apps/browser/src/main.rs")
    for fabricated in (
        "InterOS 8.2 Sovereign Microkernel Boots in 140ms",
        "Rust Foundation",
        "Sovereign Computing: W3C Standards on Native Microkernels",
        'b"Most Visited"',
    ):
        assert fabricated not in _code_only(source), f"start page still renders fabricated content: {fabricated}"
    assert 'b"Your pages"' in source
    # The real sources of rows must stay wired to real state.
    assert "app.bookmarks.record(index)" in source
    # WHY the visit log is read through `visit` rather than the older `entry`:
    # a row now shows what the page called itself beside where it was, and the
    # accessor that hands out only an address cannot feed that row.
    assert "app.history.visit(" in source


def test_the_browsers_focus_release_leaves_the_painted_bar_answerable():
    """A tap that dismisses the keyboard may not also dismiss the bar under it.

    WHY: the keyboard docks at the navigation bar's own top edge, so with the
    omnibox focused the bar's five controls are painted and nothing covers
    them. The press path still ended its focus block with "any tap outside the
    pill clears the focus", and that branch returned before the bar's handlers
    were reached -- so Back, Forward, Home, Bookmarks and Tabs were visible,
    legible and dead while a person was typing an address. The bar is drawn
    first, so the bar answers first; the release only consumes rows above it.

    WHY a source gate: the browser's bin target carries `test = false`, so
    `handle_tap` cannot be reached from a host test. The detector is the shape
    of the release branch itself -- a `!PILL.contains` early return that never
    names the bar's top edge is the bug.
    """
    source = _code_only(read("apps/browser/src/main.rs"))
    body = _fn_body(source, "fn handle_tap")
    at = body.find("if !PILL.contains(x, y)")
    assert at >= 0, "the omnibox's release branch is gone, so this gate has lost its subject"
    release = body[at : body.find("}", body.find("return Tap::Redraw", at))]
    assert "NAV_BAR_Y0" in release, (
        "the release branch swallows every row below the pill, including the "
        "navigation bar it paints beside"
    )


def test_the_gallery_names_the_screen_it_is_showing():
    """The gallery's header, chips and empty states all describe the tab in front.

    WHY: the app has four destinations behind one tab row, and three of them
    were described by the first. The header said "Photos" on every tab, with the
    same wallpaper-and-photo count line under it, so the Videos, Albums and
    Screenshots screens each opened under a title and a subtitle naming a
    different screen. The four filter chips, which select a set the timeline
    holds, were painted and answered taps on the three tabs whose grids ignore
    the filter entirely, so choosing one moved the pill and stored a preference
    while the screen behind it did not change; the row still stands on all four
    destinations -- a chip jumps to the timeline with that set chosen -- but only
    the timeline may show one of them filled. And every empty state was pinned
    120 rows under the tab row, which left nine hundred rows of blank panel
    beneath a two-line answer -- the same "is that a fault or an empty list"
    reading the ledger note already avoids by centring itself in the band.

    WHY a source gate: the gallery's bin target carries `test = false`, so
    neither the painter nor the press path can be called from a host test.
    """
    source = _code_only(read("apps/gallery/src/main.rs"))

    header = _fn_body(source, "fn gallery_pixel")
    at = header.find("TITLE_Y,")
    assert at >= 0, "the header's title run is gone, so this gate has lost its subject"
    assert "TAB_LABELS[app.tab as usize]" in header[at : at + 120], (
        "the header names a screen other than the tab in front of it"
    )

    taps = _fn_body(source, "fn handle_tap")
    chip_at = taps.find("if let Some(index) = chip_at(x, y, app)")
    assert chip_at >= 0, "the chip row's press branch is gone, so this gate lost its subject"
    chip_branch = taps[chip_at : taps.find("if y >= TAB_Y", chip_at)]
    assert "app.select_tab(GalleryTab::Photos)" in chip_branch, (
        "a chip can be chosen on a tab whose grid the filter does not reach"
    )

    chips = _fn_body(source, "fn chip_row_pixel")
    fill_at = chips.find("let (fill, ink, border) = if")
    assert fill_at >= 0, "the chip row's selected style is gone, so this gate lost its subject"
    assert "app.tab == GalleryTab::Photos" in chips[fill_at : fill_at + 200], (
        "a filled chip claims the screen in front of the user is filtered by it, "
        "on the three destinations whose grids ignore the filter"
    )

    subtitle = _fn_body(source, "fn refresh_subtitle")
    for arm in (
        "GalleryTab::Photos",
        "GalleryTab::Videos",
        "GalleryTab::Albums",
        "GalleryTab::Screenshots",
    ):
        assert arm in subtitle, (
            "the subtitle quotes the timeline's wallpaper and photo counts on a "
            "destination that holds neither"
        )

    assert "app.tab = " not in taps, (
        "a press branch changes the destination without re-stating the header, "
        "so the counts above the new grid still describe the old one"
    )

    assert "GRID_TOP + 120" not in source, (
        "an empty state is pinned under the tab row again instead of filling the band"
    )
    assert source.count("empty_block_y0(") >= 4, (
        "the empty states no longer share the one placement the band gives them"
    )


def _code_only(source: str) -> str:
    """Strips `//` line comments and doc comments from Rust source.

    WHY this exists, and why it is not optional: these contracts assert that a
    fabricated string does not appear in a file, and the natural way to write a
    comment explaining why the string went away is to quote it. Three separate
    times that re-introduced the exact literal the contract forbids -- once in
    the launcher browser, once in the kernel panel, once here. A truth contract
    that scans comments is not testing the program; it is testing the author's
    prose. Stripping comments makes the assertion mean what it says, and lets a
    comment explain the change without fighting it.
    """
    import re

    without_block = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
    return re.sub(r"//[^\n]*", " ", without_block)


def test_the_kernel_boot_settings_panel_invents_no_readings():
    """The kernel's own settings panel must not state telemetry it cannot read.

    WHY: `framebuffer.rs` draws a five-tab settings panel reachable from the
    boot shell. Every value on it was a literal, and several were not merely
    invented but self-refuting -- a Wi-Fi row advertising an enabled 6E radio
    with a named SSID sat three lines above "no wireless hardware discovered",
    and "120Hz ProMotion [ACTIVE]" contradicted the refresh_hz = 60 the same
    kernel programs in `drivers/display_kms.rs`.

    Nothing in that module can read a backlight, an audio mixer, a radio, a fuel
    gauge, a thermal sensor, a keystore slot, or a clock rate. Each row now says
    what is actually available. The 'w' binding was removed rather than left
    editing a label no hardware reads.
    """
    source = read("kernel/src/framebuffer.rs")
    code = _code_only(source)

    # No cipher is implemented anywhere in this tree, so any screen naming one
    # asserts a capability the device does not have.
    assert "AES-256-XTS" not in code, "a screen names a cipher the device does not implement"
    vault = read("apps/vault/src/main.rs")
    assert "fletcher32" in vault, "the vault's real integrity check is Fletcher-32"

    for fabricated in (
        "Screen Brightness: 85%",
        "Media Audio Volume: 70%",
        "120Hz ProMotion",
        "OLED TrueBlack",
        "Wi-Fi 6E: [ENABLED]",
        "InterSovereignMesh-5G",
        "2400 MHz",
        "1200 MHz",
        "4200 mAh",
        "3850 mV",
        "2990 mW",
        "State of Charge: 94%",
        "SoC Temperature: 42 C",
        "did:inter:sovereign:node01",
        "Attestation Verified",
    ):
        assert fabricated not in code, f"kernel settings panel invents: {fabricated}"

    # The one real fact on that tab is the scanout mode, and it must be the
    # mode the kernel actually programs.
    assert "Display mode refresh: 60 Hz (programmed)" in source
    assert "refresh_hz: 60" in read("kernel/src/drivers/display_kms.rs")

    # The dead wifi toggle must not come back with its key binding.
    assert "'w' => {" not in code
    assert "pub wifi_on: bool" not in code


def test_a_corner_radius_scale_has_exactly_one_spelling_per_value():
    """Two scales that carry the same names at different values is a trap.

    WHY: `interos_std::design::radius` and `launcher::theme::radius` used to
    both define `CONTROL`, `CARD` and `SHEET`, at 12/18/24 and 18/24/32
    respectively. A screen that reached for the wrong one rendered a control
    with visibly different corners from its siblings, and nothing in the type
    system objected. `system_space.rs` was doing exactly that for its back
    pill.

    The launcher's copy is gone: `launcher::theme::radius` is now the SDK's
    scale, re-exported, so there is one scale and one spelling per value. This
    asserts that stays true -- that the launcher declares no radius of its own
    and reaches for the platform's.

    The SDK module also shipped `SM`/`MD`/`LG` as second spellings of
    `CONTROL`/`CARD`/`SHEET` under a different type. Those are removed; this
    asserts they stay removed.
    """
    from pathlib import Path

    sdk = read("sdk/rust/interos-std/src/design/mod.rs")
    for alias in ("pub const SM: u32", "pub const MD: u32", "pub const LG: u32"):
        assert alias not in sdk, f"radius re-grows a second spelling: {alias}"

    # No launcher screen may reach past its own theme for a radius.
    screens = Path(ROOT / "apps/launcher/src/screens")
    offenders = [
        path.name
        for path in sorted(screens.glob("*.rs"))
        if "design::radius" in path.read_text(encoding="utf-8")
    ]
    assert not offenders, (
        "launcher screens must resolve radii from launcher::theme::radius, "
        f"not the SDK scale: {offenders}"
    )

    # The launcher's theme module must not declare a radius of its own: the
    # scale it re-exports is the platform's, and a second copy of it is the
    # fault this test was written for.
    theme_module = read("apps/launcher/src/theme.rs")
    assert "pub use interos_std::design::radius;" in theme_module, (
        "the launcher's theme module must resolve radii through the SDK's scale"
    )
    assert "pub mod radius" not in theme_module, (
        "the launcher declares a radius module of its own"
    )


def test_settings_never_advertises_unobserved_hardware_or_governor_state():
    source = read("apps/settings/src/main.rs")
    for fabricated in (
        'b"SIMULATED"',
        'b"Active Profile: Balanced (1200 MHz)"',
        'b"Dynamic Voltage and Frequency Scaling Active"',
        'b"OPTIMAL"',
        'b"Hardware Status: Simulated Power Bus"',
        'b"Hardware Status: Simulated Sensor Bus"',
        'b"Hardware Status: Unavailable (No Radio)"',
        'b"Hardware Status: Unavailable (No SIM)"',
    ):
        assert fabricated not in source
    assert 'b"Battery telemetry unavailable"' in source
    assert 'b"DVFS state unavailable"' in source
    assert 'b"Temperature telemetry unavailable"' in source


def test_calendar_reports_the_clock_once_per_civil_date_not_once_per_tick():
    """`sync_rtc` runs from the timer branch, so its console lines need a gate.

    WHY this needs pinning: one stress pass carried 383 copies of the same
    "RTC payload" line from two Calendar tasks, because the function printed
    the payload and a success line every time the clock answered -- roughly
    twice a second, forever. The serial console is the suites' only instrument,
    and burying it pushed real input answers seconds late. A civil date is
    news once, and a clock failure is reported once until the state changes.
    """
    source = read("apps/calendar/src/main.rs")
    body = source.split("pub fn sync_rtc(&mut self) -> bool {")[1].split("\n    pub fn ")[0]
    assert "if rtc_report_is_new(self.rtc_reported_date, today)" in body
    assert body.count("self.rtc_unknown_reported") >= 2
    assert "fn report_unknown_state(&mut self" in source
    # The payload may only reach the console inside that gate.
    payload_log = 'std::log("[CALENDAR APP] RTC payload as reported by the kernel:")'
    assert payload_log in body
    assert body.index("if rtc_report_is_new(self.rtc_reported_date, today)") < body.index(payload_log)


def test_calendar_browser_and_gallery_do_not_draw_sample_status_values():
    calendar = read("apps/calendar/src/main.rs")
    browser = read("apps/browser/src/main.rs")
    gallery = read("apps/gallery/src/main.rs")
    assert 'b"Hardware RTC Civil Clock Active"' not in calendar
    assert 'b"09:20"' not in browser
    assert 'b"09:20"' not in gallery
    assert 'Fill level ~70%' not in browser


def test_calendar_event_cards_do_not_name_a_location_that_was_never_stored():
    """An event card may not print a venue the platform never recorded.

    WHY this needs pinning: every card used to print the invented string
    "Vault Sector 203 Encrypted" as its subtitle, in every view, so a made-up
    place was presented as real event data. The stored record carries a civil
    date, a wall-clock time and a title, and no location field at all, so the
    card has to report the absence of one instead.
    """
    main = read("apps/calendar/src/main.rs")
    lib = read("apps/calendar/src/lib.rs")

    # The fabricated venue is gone from the renderer, and the app source
    # holds no literal of it at all.
    assert 'b"Vault Sector 203 Encrypted"' not in main
    assert "Vault Sector 203 Encrypted" not in lib

    # The card draws its subtitle from the record, so the renderer holds no
    # literal of its own there.
    assert "event_location(item)" in main
    assert 'b"No location"' in lib


def test_calendar_day_view_arrows_point_the_way_they_move_the_day():
    """The calendar's chevrons point the way the row they sit on actually goes.

    WHY this needs pinning: the left chevron was drawn with its apex on the
    right and the right chevron with its apex on the left, so the arrow on
    the left pointed at the next day and the arrow on the right pointed back
    a day. The glyph direction and the day the tap handler selects now come
    from the same named values, and the row indicators in the month, week,
    day and agenda views were drawn the same mirrored way.
    """
    main = read("apps/calendar/src/main.rs")
    lib = read("apps/calendar/src/lib.rs")

    # The header arrows are drawn through the shared chevron helper with an
    # explicit direction.
    assert (
        "chevron_pixel(x, y, DAY_PREV_ARROW_APEX_X, DAY_PREV_ARROW_APEX_Y, ChevronDir::Left)"
        in main
    )
    assert (
        "chevron_pixel(x, y, DAY_NEXT_ARROW_APEX_X, DAY_NEXT_ARROW_APEX_Y, ChevronDir::Right)"
        in main
    )

    # The tap handler steps the day with the same directions.
    assert "step_selected_day(d, DayArrow::Previous, max_d)" in main
    assert "step_selected_day(d, DayArrow::Next, max_d)" in main

    # The month, week, day and agenda rows carry no navigation arrow of their
    # own - their chevron points forward into the row it opens. Every one of
    # them goes through the shared helper with an explicit direction, because
    # the hand-rolled arm tests they used to carry measured dx away from a
    # left-hand apex and drew a '<' on a row that opens forward.
    assert "dx >= 0 && dx <= 8" not in main
    assert main.count("chevron_pixel(x, y, 658,") == 4
    for view in (
        "fn week_view_pixel",
        "fn day_view_pixel",
        "fn agenda_view_pixel",
        "fn event_list_pixel",
    ):
        body = main.split(view)[1].split("\nfn ")[0]
        assert "ChevronDir::Right" in body, f"{view} lost its row chevron"

    # The shared helper keeps the direction in the signature.
    assert "pub enum ChevronDir" in lib
    assert "pub fn chevron_pixel" in lib
    assert "pub fn step_selected_day" in lib


def test_touchpad_uses_valid_guest_contact_actions_and_reports_real_request_result():
    source = read("emulator/app.js")
    assert "action: 'down', coords: point.coords" in source
    assert "action: 'move', coords: point.coords" in source
    assert "sendGuestTouchEvent({ action, coords: point.coords" in source
    assert "type: gType" not in source
    assert "return request;" in source
    assert "touchSurface.focus({ preventScroll: true })" in source
    assert "document.activeElement !== touchSurface" in source
    assert "const target = [liveScanoutCanvas, liveScanoutImg, screenLiveScanout].find(visible);" in source
    assert "state.isLandscape" in source
    assert "? { width: 1440, height: 720, coords: 'guest_framebuffer' }" in source
    assert ": { width: 720, height: 1440, coords: 'portrait' }" in source
    assert "function guestPointerGeometry()" in source
    assert "coords: 'guest_framebuffer'" in source


def test_the_wallpaper_refactor_cannot_change_a_colour():
    """Hoisting the scene's derived tones out of the per-pixel path must be a
    pure speed change.

    WHY this needs pinning: the wallpaper is full-bleed, so a one-unit drift in
    any stop is visible across the whole screen. `SceneRef::new` and
    `scene_pixel` have to agree exactly, and the near-skyline clamp has to
    resolve identically -- it is per column, so a refactor that hoisted it
    wrongly would pass every other test and still render a wrong ridge.
    """
    source = read("sdk/rust/interos-std/src/design/walls.rs")
    code = _code_only(source)

    assert "pub struct SceneRef" in code
    assert "pub fn scene_pixel_ref" in code

    # The clamp is the subtle part: `near` must still be floored to `far + 3`,
    # per column. Resolving it once is the whole point; resolving it wrongly is
    # invisible until a ridge inverts.
    assert "near[i] = if n < far + 3 { far + 3 } else { n };" in code

    # The resolved path must read the resolved tones rather than re-derive them,
    # which is the entire saving.
    # Sliced by brace matching, not by the first line that happens to be `}`: the
    # assertion below is a "does not appear", and a slice that stopped early would
    # pass on a re-derivation sitting in the part of the body it never read.
    ref_body = _rust_fn(code, "scene_pixel_ref")
    for rederived in ("scene.horizon()", "scene.shadow()", "scene.mist()"):
        assert rederived not in ref_body, (
            f"scene_pixel_ref re-derives {rederived} on the per-pixel path again"
        )

    # The home surface must actually be on the resolved path -- and never back on
    # the per-pixel one.
    #
    # WHY the call takes the plan and not the scene reference: the plan is
    # what carries the wallpaper the user chose, and a document wallpaper has
    # to be paintable by the same four painters that paint the scene. The
    # resolved path itself is unchanged: the plan's `scene_ref` is still what
    # `scene_pixel_ref` reads, and the resolved tones are still not
    # re-derived per pixel.
    #
    # WHAT CHANGED SINCE ad9fe66 and 6e662ac: this pinned the literal call
    # `backdrop_ref(p, x, y)`, which is how the first hoist wrote it. The second
    # hoist moved the pixel behind a `Backdrop` that resolves itself once, so the
    # call reads `backdrop_ref(self.p, self.x, self.y)` and the reference it takes
    # is the plan's field rather than a parameter. The routing is the same; the
    # pin had simply stopped describing the code. So it is re-pointed at the
    # routing itself, which is the thing that was ever meant to be true.
    home = _code_only(read("apps/launcher/src/screens/home.rs"))
    assert "SceneRef::new(state.home_wallpaper)" in home
    # The surface's one backdrop entry point, fed by the plan's already-resolved
    # reference -- not by a `SceneRef::new` built per pixel, which would put the
    # hoisted work straight back on the pixel path.
    assert "fn backdrop_ref(p: &Paint, x: usize, y: usize) -> u32" in home
    assert "backdrop_from(&p.scene_ref, p.wallpaper_document, x, y)" in home
    # ...and the resolved path is what answers the pixel.
    assert "scene_pixel_ref(scene_ref, x, y, SCREEN_WIDTH, SCREEN_HEIGHT)" in home
    # The home surface must not have crept back onto the per-pixel entry.
    # `scene_pixel_ref(` does not contain `scene_pixel(`, so this tells them apart.
    assert "walls::scene_pixel(" not in home

    # WHAT THE PINS ABOVE CANNOT SAY: they say the fast path is in place. They do
    # not say it kept the colours, and no string search can. The evidence for that
    # is a host test in the SDK -- a preserved copy of the shipped per-pixel
    # expression, compared against the row-table path pixel for pixel over every
    # scene at the full 720x1440 domain. Pinned here so the proof cannot be
    # quietly narrowed, and executed by the companion test below so that a green
    # run means the pixels were actually compared, not merely that a comparison
    # exists somewhere.
    assert "fn legacy_scene_pixel(" in code

    # The reference must stay independent of the code under proof. If it read a row
    # table it would be a second copy of the thing being tested and would agree
    # with it for the wrong reason.
    legacy = _rust_fn(code, "legacy_scene_pixel")
    # The table path reaches the precomputed rows through three accessors, so staying
    # independent means avoiding the accessors as well as the fields: `sky_at` on the
    # preserved side would be the same row table wearing a different name, and the pin
    # would pass on the very defect it exists to catch.
    for table in ("sky_at(", "mtn_at(", "rock_at(", "sky_ramp", "mtn_ramp", "rock_ramp"):
        assert table not in legacy, (
            f"the preserved reference reads {table}; it is no longer independent of the table path"
        )
    # ...and the fast path really does answer from those rows, or the pins above would be
    # guarding names nothing reads. Two levels: the resolved path calls the accessors, and
    # each accessor indexes the row table the refactor precomputed. A `sky_at` that stopped
    # reading `sky_ramp` would leave the reference and the fast path identical by
    # construction, and the independence claim would mean nothing.
    for accessor, table in (("sky_at(", "sky_ramp"), ("mtn_at(", "mtn_ramp"), ("rock_at(", "rock_ramp")):
        assert accessor in ref_body, (
            f"scene_pixel_ref no longer calls {accessor}, so it is not the table path any more"
        )
        assert table in _rust_fn(code, accessor.rstrip("(")), (
            f"fn {accessor.rstrip('(')} no longer reads {table}: the fast path is not reading a table"
        )
    # The reference must still do the arithmetic the tables replaced, per pixel, for all
    # three vertical stops -- not merely mention them.
    assert "rel as u32 * 256 / horizon_y as u32" in legacy
    assert "let vert_mtn =" in legacy
    assert "let vert_rock =" in legacy
    assert "band(vert_sky, across)" in legacy
    # `near` is the one resolved value both paths share, exactly as it did before the row
    # tables existed: the skyline is per column and was already hoisted, so the preserved
    # copy reads `s.near` rather than re-deriving the clamp.
    assert "s.near[col]" in legacy

    proof = _rust_fn(
        code, "the_table_path_reproduces_the_shipped_expression_pixel_for_pixel"
    )
    # Compared with whitespace removed: the proof's `assert_eq!` is wrapped across
    # lines by the formatter, and a needle that only matches one layout would fail
    # on a reflow that changed nothing about what is proved.
    tight = "".join(proof.split())
    assert "forsceneinSCENES.iter()" in tight, "the proof no longer walks every scene"
    assert "foryin0..1440usize" in tight, "the proof no longer walks every row"
    assert "forxin0..720usize" in tight, "the proof no longer walks every column"
    assert "legacy_scene_pixel(&s,x,y,720,1440)" in tight
    assert "scene_pixel_ref(&s,x,y,720,1440)" in tight
    assert "assert_eq!(tables,shipped," in tight, "the proof no longer asserts equality"


def test_the_wallpaper_equivalence_proof_actually_runs():
    """Run the SDK's pixel-for-pixel wallpaper proof instead of only citing it.

    WHY execute rather than re-point the string search: the invariant is that a
    refactor paid for per-pixel speed changed no colour. That is 6 scenes x
    1,036,800 pixels of comparison against a preserved copy of the shipped
    expression, and the comparison is what carries the claim. The host suite takes
    under a second once the crate is built, so a contract can afford to report its
    result instead of its existence.

    WHERE IT CANNOT RUN: with no cargo on PATH the contract says so out loud
    rather than letting the suite report a colour proof that was never performed.
    """
    import shutil
    import subprocess

    cargo = shutil.which("cargo")
    if cargo is None:
        pytest.skip(
            "cargo is not on PATH: the wallpaper equivalence proof was not executed"
        )

    manifest = ROOT / "sdk" / "rust" / "interos-std" / "Cargo.toml"
    result = subprocess.run(
        [cargo, "test", "--manifest-path", str(manifest), "--lib", "design::walls"],
        capture_output=True,
        text=True,
        cwd=ROOT,
        timeout=900,
    )
    output = result.stdout + result.stderr
    assert result.returncode == 0, (
        "the SDK's wallpaper host tests are not passing:\n" + output[-4000:]
    )
    for proof in (
        "the_table_path_reproduces_the_shipped_expression_pixel_for_pixel",
        "the_convenience_entry_agrees_with_the_reference_path",
        "the_table_path_holds_at_other_surface_sizes",
    ):
        assert f"design::walls::tests::{proof} ... ok" in output, (
            f"{proof} did not run and pass, so the equivalence it is relied on for "
            "was not demonstrated"
        )
def test_calculator_and_vault_do_not_paint_fabricated_status_or_success():
    calculator = read("apps/calculator/src/main.rs")
    vault = read("apps/vault/src/main.rs")
    assert 'b"09:20"' not in calculator
    assert 'b"100%"' not in calculator
    assert 'b"3,072 / 3,072 BYTES ALLOCATED - 100% HEALTH"' not in vault
    assert 'b"Hardware Master Key & Multi-Sector Ledger Online"' not in vault
    assert 'b"AES-256-XTS & ChaCha20-Poly1305 Verified"' not in vault
    assert "pub fn generate_iv(&self) -> Option<[u8; 16]>" in vault
    assert "None\n    }" in vault
    assert "[0xA5; 16]" not in vault
    assert "let Some(iv) = self.generate_iv() else" in vault


def test_settings_security_privacy_and_rtc_are_not_assumed_successful():
    source = read("apps/settings/src/main.rs")
    for fabricated in (
        'b"Cryptographic Root Key Sealed in Hardware"',
        'b"Sectors 200-205 Cryptographically Sealed"',
        'b"Anti-Tracking Query & Header Sanitizer Active"',
        'b"Civil Wall-Clock Synchronization Active"',
        'b"RTC SYNC"',
    ):
        assert fabricated not in source
    # WHY the probe outcome and the absent query, and not "status unavailable":
    # the keystore row reports the result of the derivation probe this app is
    # allowed to attempt, and the vault and filter rows name the missing query
    # rather than implying a status the platform never answered.
    assert 'b"Derivation refused by the kernel"' in source
    assert 'b"No vault encryption status query is exposed"' in source
    assert 'b"No privacy filter status query is exposed"' in source


def test_native_keyboard_events_are_decoded_and_used_by_browser_and_calculator():
    sdk = read("sdk/rust/interos-std/src/lib.rs")
    browser = read("apps/browser/src/main.rs")
    calculator = read("apps/calculator/src/main.rs")
    assert "pub const INPUT_EVENT_KIND_KEY: u8 = 11;" in sdk
    assert "pub fn key_event(&self) -> Option<InputKeyEvent>" in sdk
    assert "if let Some(key) = input.key_event()" in browser
    assert "pub address_focused: bool" in browser
    assert "fn handle_browser_key(app: &mut BrowserApp, key: std::InputKeyEvent)" in browser
    # WHY two strings: the responsive rewrite (f7f2b64) split the old combined
    # "Navigation unavailable: no network transport" literal into a headline
    # plus a wrapped body; both halves are asserted so the honest outcome
    # (parsed, attempted, nothing sent, nothing loaded) stays pinned.
    assert '(b"Navigation unavailable", BODY_NO_TRANSPORT)' in browser
    assert "no network transport is bound to this app" in browser
    assert "if let Some(key) = input.key_event()" in calculator
    assert "fn handle_keyboard_key(calc: &mut CalculatorState, code: u32) -> bool" in calculator


def test_layout_fixes_reserve_full_glyph_line_height():
    settings = read("apps/settings/src/main.rs")
    calendar = read("apps/calendar/src/main.rs")
    file_manager = read("apps/file_manager/src/main.rs")
    messages = read("apps/messages/src/main.rs")
    messages_layout = read("apps/messages/src/lib.rs")
    # WHY these three settings assertions changed: the Settings app was rebuilt
    # as the design system's "System Space" (light category grid + stack
    # detail screens), so the old dark tab-layout coordinates ("60, 270, 1"
    # etc.) no longer exist. The invariant they pinned is unchanged and still
    # asserted here on the new geometry: every detail row reserves the title
    # rung's whole line box, the caption starts a caption line box and a gap
    # below it, and a row carrying a second caption line reserves that line's
    # box too -- i.e. no text band is allowed to start inside the line box above
    # it. WHY the offsets are pinned as derivations and not as the row numbers
    # this comment used to quote: the boxes come off the type ladder, so the
    # remembered 16 / 72 / 98 went stale the moment the rungs moved, while the
    # layout that followed the ladder stayed correct. The sum is the contract.
    #
    # WHY these three now name `boxes.*_dy()` instead of `ROW_*_DY` constants --
    # and why that is the same contract, not a weakened one: the Font Scaling
    # setting is read after the binary is built, so the offset a band starts at
    # cannot be a constant at the call site any more; a row asks its surface's
    # rung through `RowBoxes`. The constants kept their names and their sums are
    # still pinned below. What is pinned here instead is the whole chain, so a
    # painted band still reaches a line box in one link and cannot be paired with
    # another rung: `cy + boxes.cap_dy()` -> `fn cap_dy() { cap_dy_at(self.level()) }`
    # -> `cap_dy_at(level) = ROW_TITLE_DY + title_at(level) + ROW_LINE_GAP` ->
    # asserted equal to `ROW_CAP_DY` at the platform's level. The rule the comment
    # above states is now also measured at EVERY level the setting can hold
    # (`bands_clear_at_every_level`, gated by a `const assert!`), which is exactly
    # the case the old fixed pin could not see: a caption that clears its title at
    # 100% and rides into it at 150% used to pass here.
    assert "cy + boxes.title_dy()," in settings
    assert "cy + boxes.cap_dy()," in settings
    assert "cy + boxes.cap2_dy()," in settings
    assert "const LINE_H: usize = std::design::font::line_height(TYPE_H3);" in settings
    assert "const CAPTION_LINE_H: usize = std::design::font::line_height(TYPE_CAPTION);" in settings
    flat_settings = " ".join(settings.split())
    assert "const ROW_CAP_DY: usize = ROW_TITLE_DY + LINE_H + ROW_LINE_GAP;" in flat_settings, (
        "the settings caption no longer starts a whole title line box plus a gap "
        "below the title it sits under"
    )
    assert "const ROW_CAP2_DY: usize = ROW_CAP_DY + CAPTION_LINE_H + ROW_LINE_GAP;" in flat_settings, (
        "the settings row no longer reserves a whole caption line box for its "
        "second caption line"
    )
    # The accessors the painted bands call are the ladder derivations, one link
    # each, and the derivations are the sums the constants state.
    assert "fn title_dy(&self) -> usize { ROW_TITLE_DY }" in flat_settings, (
        "a detail row's title band no longer starts at the offset this file pins; "
        "the band's top is the row's pad -- whitespace, not type -- so it is the "
        "one band a text-size rung must not move"
    )
    assert "fn cap_dy(&self) -> usize { cap_dy_at(self.level()) }" in flat_settings, (
        "the caption band no longer starts at the offset derived from the same "
        "rung its ink is drawn at"
    )
    assert "fn cap2_dy(&self) -> usize { cap2_dy_at(self.level()) }" in flat_settings, (
        "the second caption band no longer starts at the offset derived from the "
        "same rung its ink is drawn at"
    )
    assert (
        "const fn cap_dy_at(level: u8) -> usize { "
        "ROW_TITLE_DY + title_at(level) + ROW_LINE_GAP }"
    ) in flat_settings, (
        "the caption's derivation is no longer a whole title line box plus a gap "
        "below the offset the title starts at"
    )
    assert (
        "const fn cap2_dy_at(level: u8) -> usize { "
        "cap_dy_at(level) + caption_at(level) + ROW_LINE_GAP }"
    ) in flat_settings, (
        "the row no longer reserves a whole caption line box for its second "
        "caption line, at whatever rung the row is drawn at"
    )
    # At the platform's rung the derivation is the constant, so a page that never
    # asked for a text size is not resized by the code that reads one.
    assert "const _: () = assert!(cap_dy_at(PLATFORM_LEVEL) == ROW_CAP_DY);" in settings
    assert "const _: () = assert!(cap2_dy_at(PLATFORM_LEVEL) == ROW_CAP2_DY);" in settings
    # A level enters the row's geometry in exactly one place, and the band rule is
    # measured over every level the setting holds rather than compiled for one.
    assert (
        "fn level(&self) -> u8 { if self.u { std::design::font::user_text_scale() } "
        "else { PLATFORM_LEVEL } }"
    ) in flat_settings, (
        "a row can now take its ink from one rung and its box from another"
    )
    assert "const _: () = assert!(bands_clear_at_every_level());" in settings
    assert "while level <= EXTRA_LARGE_LEVEL {" in settings
    assert "if ROW_TITLE_DY + title_at(level) > cap_dy_at(level) {" in settings
    assert "if cap_dy_at(level) + caption_at(level) > cap2_dy_at(level) {" in settings
    assert "if cap2_dy_at(level) + caption_at(level) > tall_alt_at(level) {" in settings
    assert "if cap_dy_at(level) + caption_at(level) > row_h_at(level) {" in settings
    assert "centered_x(detail_str, 1, 24, 672)" in calendar
    # WHY this no longer greps the row's remembered y: the month view's "nothing
    # listed" card takes its top from the list band, the list band takes its top
    # from the section band, and the detail line stands MONTH_EMPTY_DETAIL_DY
    # rows inside that card. 760 was that sum for the rungs the ladder no longer
    # draws; the invariant -- the detail run reserves the caption rung's whole
    # line box, the press answers exactly the band it is painted in, and that
    # band ends above the "+ New Event" box it would otherwise fall under -- is
    # carried by the derivation and the app's own const gate.
    assert "const MONTH_EMPTY_CARD_Y0: usize = MONTH_LIST_Y0;" in calendar
    assert "let detail_y = MONTH_EMPTY_CARD_Y0 + MONTH_EMPTY_DETAIL_DY;" in calendar
    assert "let detail_h = std::design::font::line_height(1);" in calendar
    assert "if y >= detail_y && y < detail_y + detail_h {" in calendar
    flat_calendar = " ".join(calendar.split())
    assert (
        "const _: () = assert!( MONTH_EMPTY_CARD_Y0 + MONTH_EMPTY_DETAIL_DY "
        "+ std::design::font::line_height(1) <= ACTION_BTN_Y0 );"
    ) in flat_calendar, "the month view's detail line box is no longer held above the action band"
    assert "760," not in _code_only(calendar), (
        "the month view's detail row is authored from a remembered y again"
    )
    # WHY this assertion names the constants now: the calendar's four views did
    # not share a number for "+ New Event". Month painted and tapped the band
    # these constants state; Week, Day and Agenda painted and tapped 816..872, so
    # the same control sat sixteen rows lower in three of the four screens and the
    # cards stacked above it -- capped by this band's top edge -- were measured
    # against a box the button no longer occupied. The invariant is unchanged and
    # is now carried by the one box every view reads: the button is hit where it
    # is painted, at rows 800..856, on every view.
    assert "const ACTION_BTN_Y0: usize = 800;" in calendar
    assert "const ACTION_BTN_Y1: usize = ACTION_BTN_Y0 + ACTION_BTN_H;" in calendar
    assert calendar.count("y >= ACTION_BTN_Y0 && y < ACTION_BTN_Y1") == 8
    assert "y >= 816 && y < 872" not in calendar
    # WHY these no longer pin the header's coordinates: "Files" was painted at a
    # hand-authored (48, 64) while its back control stood on the row underneath,
    # at the app's own inset and size -- a chip in the wrong corner of the one
    # screen the rest of the device answers at the platform's rect. The title's
    # box is now derived from that rect, so the invariant carried here is the
    # derivation itself: paint and target both come from `back_button`, and the
    # title starts to the right of it.
    assert "back_button::pixel_for_theme(x, y, &THEME)" in file_manager
    assert "back_button::contains(x, y)" in file_manager
    assert "const TITLE_X: usize =\n    std::design::back_button::X + std::design::back_button::SIZE + spacing::GRID_16;" in file_manager
    assert "test_text(x, y, 48, 64, 3, b\"Files\"" not in file_manager
    # WHY this names the constant and the branch rather than the bytes at the
    # painter: the empty state's sentences moved beside the boxes that hold them,
    # and the line the transport-refused branch paints has to clear the title's
    # whole glyph line box -- which is what 740 and 810 are for.
    assert 'pub const EMPTY_TRANSPORT_SUBLINE' in messages_layout
    assert 'b"Messaging unavailable: no transport"' in messages_layout
    assert "_ => (layout::EMPTY_TRANSPORT_SUBLINE," in messages
    assert "layout::EMPTY_THREADS_TITLE," in messages


def test_device_studio_does_not_infer_guest_cpu_state_from_qemu_liveness():
    source = read("emulator/app.js")
    assert "QEMU / BARE-METAL" not in source
    assert "CPU state unavailable" in source
    assert "CPU telemetry reports active" in source
    assert "data.cpu?.thread_id || 'Active'" not in source
    assert "data.cpu?.rip || 'Offline'" not in source


def test_unavailable_settings_services_use_neutral_not_success_styling():
    source = read("apps/settings/src/main.rs")
    # WHY the platform's role and not a name of the app's own: the settings
    # app's colours come from the platform's theme now, so "the neutral ink"
    # is `on_surface_variant` and "the success ink" is `success`. The claim
    # under test is unchanged: an unavailable service is reported in the
    # neutral ink and never in the success ink.
    assert 'b"DVFS state unavailable", THEME.on_surface_variant' in source
    # The rows are multi-line calls, so the assertions match the argument
    # sequence with whitespace flattened: caption, ink, badge, badge ink.
    compact = " ".join(source.split())
    assert (
        'b"Derivation refused by the kernel", b"REFUSED", THEME.on_surface_variant,'
        in compact
    )
    assert (
        'b"No vault encryption status query is exposed", THEME.on_surface_variant, '
        'b"NOT REPORTED", THEME.on_surface_variant'
        in compact
    )
    assert (
        'b"No privacy filter status query is exposed", THEME.on_surface_variant, '
        'b"NOT REPORTED", THEME.on_surface_variant'
        in compact
    )
    assert 'b"Derivation refused by the kernel", THEME.success' not in compact
    assert 'b"No vault encryption status query is exposed", THEME.success' not in compact
    assert 'b"No privacy filter status query is exposed", THEME.success' not in compact


def test_the_back_affordance_paints_the_box_a_person_can_tap():
    """One Back control per screen, and its tap area is its painted plate.

    WHY a source gate: the chips are answered by absolute coordinates from the
    QEMU suites (`test_interactive_lifecycle.py` taps the chip's centre at
    (48, 80)), so a coordinate change is a runtime regression, not a refactor.
    What is pinned here is the derivation -- the app must not own a second copy
    of a rect the SDK already owns, and must not answer a column it never inks.

    The platform chip is `back_button`: X 24, Y viewport::HEADER_TOP (56),
    SIZE 48, painted by `pixel_for_theme` and answered by `contains`, both over
    the identical 24..72 by 56..104 square.
    """
    flat = {
        app: " ".join(read(f"apps/{app}/src/main.rs").split())
        for app in (
            "browser", "vault", "camera", "video_player", "calendar",
            "agent_manager", "messages", "music",
        )
    }

    # The four apps that adopt the chip: paint and target from the same SDK call.
    for app in ("vault", "camera", "calendar", "agent_manager"):
        assert "back_button::pixel_for_theme(x, y, &THEME)" in flat[app] or \
               "back_button::pixel_for_theme(x, y, &CHASSIS_THEME)" in flat[app], (
                f"{app} no longer paints the platform chip")
        assert "back_button::contains(" in flat[app], (
            f"{app} no longer tests the tap through the SDK's own box")
    # The boxes these four re-derived by hand are gone: each name now reads off
    # `back_button`, so the header layout and the painted chip cannot drift.
    assert "const BACK_X: usize = std::design::back_button::X;" in flat["vault"]
    assert "const BACK_Y: usize = std::design::back_button::Y;" in flat["vault"]
    assert "const BACK_S: usize = std::design::back_button::SIZE;" in flat["vault"]
    assert "const BACK_RECT: Rect = Rect::new( std::design::back_button::X, " \
           "std::design::back_button::Y, std::design::back_button::SIZE, " \
           "std::design::back_button::SIZE, );" in flat["camera"]
    # No second copy of the same square in a press path, and no band wider than
    # the plate: 24..100 (calendar) and 24..88 (agent manager) both answered
    # columns of bare surface between the chip and the run beside it.
    assert "in_round(x, y, BACK_X, BACK_Y, BACK_S, BACK_S, 14)" not in flat["vault"]
    assert "BACK_RECT.contains(x, y)" not in flat["camera"]
    assert "x >= 24 && x < 100 && y >= HEADER_TOP" not in flat["calendar"]
    assert "ux >= 24 && ux < 88 && uy >= HEADER_TOP" not in flat["agent_manager"]

    # The two apps that closed task #209's back-chip reconciliation. Both painted
    # the platform chip through `pixel_for_theme` -- plate 24..72 -- while the
    # press unioned that box with a hand-written wider one: Messages answered
    # 24..100 (a 28-column strip of bare surface, ending just short of the
    # accent dot, which is a status mark and not a control), and Music carried a
    # second copy of the rect, `const BACK: Rect = Rect::new(24, HEADER_TOP, 72,
    # 48)`, whose columns 72..96 were blank header between the plate and the
    # centred title. Tapping that dead space popped a view (Messages) or quit the
    # app (Music). After the fix each press names the SDK's own square, and the
    # second copy is gone from the source entirely.
    for app in ("messages", "music"):
        assert "back_button::pixel_for_theme(x, y, &THEME)" in flat[app], (
            f"{app} no longer paints the platform chip")
        assert "back_button::contains(x, y)" in flat[app], (
            f"{app} no longer tests the tap through the SDK's own box")
    assert "x >= 24 && x < 100" not in flat["messages"]
    assert "const BACK: Rect = Rect::new(24," not in flat["music"]
    assert "BACK.contains(x, y)" not in flat["music"]

    # The two overrides with a real layout reason keep their own rect, but the
    # rect is built from the SDK's control size, and one rect feeds both the
    # painter and the press.
    browser = read("apps/browser/src/main.rs")
    assert "const NAV_BTN_H: usize = back_button::SIZE;" in browser
    assert "const BACK_BTN: Rect = Rect::new(back_button::X, NAV_ROW_Y, 110, NAV_BTN_H);" in browser
    assert "const _: () = assert!(BACK_BTN.y >= NAV_BAR_Y0 && BACK_BTN.y + BACK_BTN.h <= NAV_BAR_Y1);" in browser
    assert "Rect::new(24, 1356, 110, 48)" not in browser
    # Painted == hit is structural: both branches name the same rect.
    assert "chrome_control_px(x, y, BACK_BTN, BTN_R)" in browser
    assert "if BACK_BTN.contains(x, y)" in browser
    assert "back_button::pixel_for_theme(x, y, &THEME)" in browser
    assert "if back_button::contains(x, y)" in browser

    video = read("apps/video_player/src/main.rs")
    assert "const LAND_BACK_X: usize = LAND_BAR.x + LAND_BAR_AIR;" in video
    assert "Rect::new( 36, LAND_BAR.y" not in " ".join(video.split())
    # The plate moved from a const to `land_back_btn()` because the landscape
    # scrim is now the two runs' line boxes at the published rung, and a chip
    # centred in the platform's rows drifts off the middle of a bar that grew
    # under it. What is NOT relaxed: both branches name the one accessor --
    # painted == hit stays structural -- and the platform-rung const the gates
    # read is still the same 48x48 stood on the bar's own air.
    assert "let back_hit = land_back_btn();" in video
    assert "if back_hit.contains(x, y)" in video
    assert "if land_back_btn().contains(x, y)" in video
    assert (
        "const _: () = assert!(LAND_BACK_X >= back_button::X && "
        "LAND_BACK_BTN.y >= back_button::Y);"
    ) in " ".join(video.split())
