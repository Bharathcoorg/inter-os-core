# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""The type scale every application draws with.

The platform's type scale is a set of *cap heights* -- the height of a
capital letter -- and the design document's scale is written in them: a
caption of fifteen to sixteen pixels, a body of nineteen to twenty-one, a
section of twenty-five to twenty-seven, a title of thirty-three to
thirty-five, a page heading of thirty-six to forty-four and a display of
forty-eight to fifty-six.

Those are the bands. What each rung draws *inside* its band is the other half
of the ladder, and the rungs sit at the top of the band they are written in:
16, 21, 27, 35, 44, 56 and a hero 76. `font::cap_for_scale` owns those values,
this file names them once as `LADDER_CAPS`, and every check below compares an
SDK or host-side copy against the ladder rather than trusting a remembered
number. WHY the numbers are written down here at all instead of only read off
the SDK: a contract that reads a table back at itself certifies any drift, and
the rungs, the bands in `design/cap_ladder.rs`, this document and the
host-side measurement copy have to move together when a rung moves.

WHY the four lower bands are above the numbers the document first carried:
a type scale is only comparable to another phone's when it is stated as a
share of the panel it is read on, and this panel is 720 wide. The document's
numbers put this device's body at 2.50% of the panel and its caption at
1.81%, while iOS ships 3.00% and 2.30% and Android 2.77% and 2.07% for the
same roles. The bands below are the ones the ladder now draws inside: 2.08%
and 2.78% for caption and body, which is where a phone's text sits.

These contracts pin four things:

1. the SDK's rungs are the document's cap heights, and each rung is the
   height named above;
2. no application can reach a size the ladder does not have, and none
   draws below the caption rung -- a seven-pixel label reads as a smudge on
   the panel, and that is what a screen that has left the scale looks like;
3. every application can reach every rung, so a screen that wants a page
   title has one to reach for;
4. the host-side copy of the ladder the measurement tool carries is the
   ladder, rung for rung.

The behavioural check is a device measurement: `tools/measure_type.py`
reads a capture and reports the cap height of every run of text on it.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
APPS = ROOT / "apps"

# The ladder's bands -- the range each role may be drawn inside -- as the SDK
# writes them in `design/cap_ladder.rs`. `test_the_ladder_bands_are_written_once`
# compares this copy against that table.
LADDER = {
    "CAPTION": (15, 16),
    "BODY": (19, 21),
    "SECTION": (25, 27),
    "TITLE": (33, 35),
    "HEADING": (36, 44),
    "DISPLAY": (48, 56),
}

# The rung number each named role sits on, and the cap height it draws at. The
# rungs 1..6 are the roles the SDK names; rung 7 is the hero a lock-screen clock
# is the whole point of.
ROLE_RUNG = {
    "CAPTION": 1,
    "BODY": 2,
    "SECTION": 3,
    "TITLE": 4,
    "HEADING": 5,
    "DISPLAY": 6,
}

LADDER_CAPS = {1: 16, 2: 21, 3: 27, 4: 35, 5: 44, 6: 56, 7: 76}

# The helpers that take a cap height, and the position of the argument.
CAP_CALLS = {"place_run_cap": 2}

# The helpers that take a scale rung, and the position of the argument.
RUNG_CALLS = {
    "test_text": 4,
    "test_text_bold": 4,
    "place_run": 2,
    "draw_label": 3,
    "text_centered": 3,
    "text_centered_bold": 3,
}


def _app_dirs() -> list[Path]:
    return sorted(p for p in APPS.iterdir() if p.is_dir() and (p / "src").is_dir())


def _font_ladder() -> dict[int, int]:
    """`font::cap_for_scale`: the rung -> cap-height table the platform draws with.

    WHY parsed and not written down elsewhere: `cap_for_scale` is the one
    authority for what a rung is, and every other copy of it -- the SDK's named
    cap heights, this file's `LADDER_CAPS`, the measurement tool's table -- is a
    copy that has to be checked against it.
    """
    font = (ROOT / "sdk/rust/interos-std/src/font.rs").read_text(encoding="utf-8")
    match = re.search(
        r"pub const fn cap_for_scale\(scale: usize\) -> u32 \{(.*?)\n\}", font, re.S
    )
    assert match, "font::cap_for_scale is no longer shaped the way this suite reads it"
    ladder = {
        int(rung): int(cap)
        for rung, cap in re.findall(r"^\s*(\d+)\s*=>\s*(\d+)", match.group(1), re.M)
    }
    assert ladder, "font::cap_for_scale answers no rung"
    return ladder


def _split_arguments(text: str) -> list[str]:
    """Splits a call's argument list on its top-level commas.

    WHY this is not `text.split(",")`: an argument may itself be a call --
    `test_text(x, y, centered_x(label, width), 0, 2, ...)` -- and a naive
    split would count the inner call's arguments as the outer call's and
    read a size out of the wrong position.
    """
    arguments: list[str] = []
    depth = 0
    current: list[str] = []
    for character in text:
        if character in "([{":
            depth += 1
        elif character in ")]}":
            depth -= 1
        if character == "," and depth == 0:
            arguments.append("".join(current).strip())
            current = []
            continue
        current.append(character)
    if current:
        arguments.append("".join(current).strip())
    return arguments


def test_a_rung_names_one_size_and_both_faces_honour_it():
    """A label is drawn with one face or the other, so both must size it alike.

    WHY this matters: the ladder was once written with the design
    document's own numbers, which are a third smaller than the bitmap
    cell's multiples. Every label drawn with the outline face then came
    out at half the size of the same label drawn with the bitmap face, and
    the platform became a patchwork of two sizes. That is what "the text
    is very small" was.
    """
    sdk = (ROOT / "sdk/rust/interos-std/src/design/mod.rs").read_text(encoding="utf-8")
    for name in LADDER:
        assert f"pub const {name}_CAP_PX" in sdk, f"the SDK does not name the {name} cap height"
    assert "pub const CAPTION_SCALE: usize = 1;" in sdk
    assert "pub const BODY_SCALE: usize = 2;" in sdk
    assert "pub const SECTION_SCALE: usize = 3;" in sdk
    assert "pub const TITLE_SCALE: usize = 4;" in sdk
    assert "pub const HEADING_SCALE: usize = 5;" in sdk
    assert "pub const DISPLAY_SCALE: usize = 6;" in sdk
    # The font module carries the rule the two faces share.
    font = (ROOT / "sdk/rust/interos-std/src/font.rs").read_text(encoding="utf-8")
    assert "pub const fn cap_for_scale" in font
    assert "pub const fn em_for_cap_scale" in font, (
        "a cap height the ladder produced has to resolve back to the size the "
        "ladder named, so the outline face and the bitmap face draw the same "
        "label at the same size"
    )
    # WHY the SDK derives its rungs instead of naming them: a second copy of
    # the ladder is a second ladder, and the two drift apart the moment one
    # of them is touched. This holds for every role, not only the caption.
    for name, rung in ROLE_RUNG.items():
        assert f"pub const {name}_SCALE: usize = {rung};" in sdk, (
            f"the SDK no longer puts {name} on rung {rung}"
        )
        assert f"pub const {name}_CAP_PX: u32 = font::cap_for_scale(" in sdk, (
            f"the SDK's {name} cap height is not read off the font module's ladder"
        )
        assert f"pub const {name}_LINE_PX: usize = font::line_height(" in sdk, (
            f"the SDK's {name} line box is not read off the font module's ladder"
        )
    assert "font::cap_for_scale(CAPTION_SCALE)" in sdk, (
        "the SDK's caption rung must come from the font module's ladder"
    )
    assert "font::line_height(CAPTION_SCALE)" in sdk, (
        "the SDK's caption line box must come from the font module's ladder"
    )
    # The kit the applications draw with resolves a cap the same way.
    kit = (ROOT / "sdk/rust/interos-std/src/design/monochrome.rs").read_text(encoding="utf-8")
    assert "em_for_cap_scale" in kit, (
        "the monochrome kit must resolve a cap height through the ladder"
    )


def test_every_rung_is_reachable_from_the_sdk():
    """A screen that wants a page title has a rung to reach for."""
    sdk = (ROOT / "sdk/rust/interos-std/src/design/mod.rs").read_text(encoding="utf-8")
    for name in LADDER:
        assert f"pub const {name}_CAP_PX" in sdk, f"the SDK does not offer {name}"
    theme = (ROOT / "sdk/rust/interos-std/src/design/theme.rs").read_text(encoding="utf-8")
    for field in ("caption", "body", "section", "title", "heading", "display"):
        assert f"pub {field}: typography::TextStyle" in theme, (
            f"the theme does not carry a {field} rung"
        )
        assert f"{field}: typography::" in theme, f"the theme's {field} rung is not the SDK's"


def test_every_application_can_reach_every_rung():
    """An application that cannot name a rung cannot use one."""
    for app in _app_dirs():
        themes = sorted((app / "src").glob("theme.rs"))
        if not themes:
            continue
        text = themes[0].read_text(encoding="utf-8")
        # Either the theme names the rungs it uses, or it carries the whole
        # scale and the screens reach through it.
        if "TypeScale::STANDARD" in text:
            continue
        for name in ("CAPTION", "BODY", "SECTION", "TITLE", "HEADING", "DISPLAY"):
            assert name in text, (
                f"{app.name}'s theme does not expose the {name} rung, so no screen "
                "in it can draw at that size"
            )


def test_no_application_draws_at_a_size_the_ladder_does_not_have():
    """A cap height under the caption band is not a size a person can read.

    WHY this is a contract and not a review note: the caption band runs fifteen
    to sixteen pixels -- the rung itself draws sixteen -- and an application that
    asks for anything under the band has left the scale, which is how a screen
    ends up with a seven-pixel label that reads as a smudge on the panel.
    """
    problems: list[str] = []
    for app in _app_dirs():
        for source in sorted((app / "src").rglob("*.rs")):
            text = source.read_text(encoding="utf-8")
            tables = (
                (CAP_CALLS, LADDER["CAPTION"][0], LADDER["CAPTION"][1], "cap"),
                (RUNG_CALLS, 1, 6, "rung"),
            )
            for table, low, high, what in tables:
                for call, slot in table.items():
                    pattern = r"\b" + re.escape(call) + r"\s*\(([^;]{0,400}?)\)"
                    for match in re.finditer(pattern, text):
                        # A function *definition* is not a call.
                        if text[: match.start()].rstrip().endswith("fn"):
                            continue
                        arguments = _split_arguments(match.group(1))
                        if len(arguments) <= slot:
                            continue
                        value = arguments[slot].strip()
                        if not value.isdigit():
                            continue
                        number = int(value)
                        if not low <= number <= high:
                            problems.append(
                                f"{source.relative_to(ROOT)}: {call} asks for {what} "
                                f"{number}, which is not on the ladder"
                            )
    assert not problems, (
        "an application draws at a size the ladder does not have:\n  "
        + "\n  ".join(problems[:20])
    )


def test_the_ladder_bands_are_written_once():
    """The bands in this file are the bands the SDK's own ladder test carries.

    WHY this is checked: the ladder's ranges used to live in three places --
    here, in `design/cap_ladder.rs` and in a `const assert` inside the
    webview -- and re-measuring the rungs broke the two copies while the
    layout that follows the ladder was correct. This file reads the SDK's
    table instead of trusting its own.
    """
    ladder = (ROOT / "sdk/rust/interos-std/src/design/cap_ladder.rs").read_text(encoding="utf-8")
    rows = re.findall(
        r"\(\s*(\d)\s*,\s*typography::([A-Z_]+)_CAP_PX\s*,\s*(\d+)\s*,\s*(\d+)\s*\)",
        ladder,
    )
    assert len(rows) == len(LADDER), (
        f"the SDK's ladder table names {len(rows)} rungs, this contract names {len(LADDER)}"
    )
    named = {name: (int(low), int(high)) for _rung, name, low, high in rows}
    for name, band in LADDER.items():
        assert name in named, f"the SDK's ladder table does not carry the {name} rung"
        assert named[name] == band, (
            f"the {name} band is {band} here and {named[name]} in the SDK; "
            "one of them is a stale copy of the ladder"
        )


def test_the_ladder_draws_at_the_rung_heights_named_here():
    """The rungs are the heights the raised ladder documents, inside their bands.

    WHY this is a contract beside the band test: a band says only what a rung may
    not go under or over, so a ladder that slid to the bottom of every band --
    which is what made this device's text read small -- kept passing a band-only
    check. The rung values below are what every application's derived line box
    is built on, so a rung that moves has to move here, in the SDK's band table
    and in the measurement tool in the same change.
    """
    ladder = _font_ladder()
    for rung, cap in sorted(LADDER_CAPS.items()):
        assert ladder.get(rung) == cap, (
            f"rung {rung} draws a {ladder.get(rung)}px cap; the ladder's readers "
            f"(this file, the SDK's bands, tools/measure_type.py) expect {cap}px"
        )
    for name, (low, high) in LADDER.items():
        rung = ROLE_RUNG[name]
        assert low <= ladder[rung] <= high, (
            f"the {name} rung draws {ladder[rung]}px, outside its {low}..{high} band"
        )
    # The hierarchy has to read as steps, not as one size rounded differently:
    # every neighbouring pair is at least four pixels and 1.13x apart, which is
    # the same floor the SDK's own ladder test carries.
    for rung in sorted(LADDER_CAPS)[:-1]:
        cap, nxt = ladder[rung], ladder[rung + 1]
        assert nxt >= cap + 4 and nxt * 100 >= cap * 113, (
            f"rung {rung} at {cap}px and rung {rung + 1} at {nxt}px read as one size"
        )


def test_the_measurement_tool_measures_cap_heights():
    """The device-side check exists and knows the ladder.

    WHY the comparison is against the rungs and not the bands: a band lets a
    copy of the ladder sit anywhere inside it, so the tool went on naming a
    fifteen-pixel caption while the ladder had moved the rung to sixteen, and
    every check kept passing. Equality is what makes the copy honest.
    """
    tool = (ROOT / "tools/measure_type.py").read_text(encoding="utf-8")
    # WHY the panel width and not a fixed divisor: a capture arrives at
    # whatever resolution the host presented, so the tool measures the frame
    # it is given and divides by the panel's own logical width. A constant
    # scale silently reported a shrunk frame as a screen of undersized text.
    assert "GUEST_WIDTH = 720" in tool, "the tool no longer knows the panel it measures"
    ladder = _font_ladder()
    for name, rung in ROLE_RUNG.items():
        found = re.search(rf'"{name}":\s*(\d+)', tool)
        assert found, f"the measurement tool does not know the {name} rung"
        assert int(found.group(1)) == ladder[rung], (
            f"the tool measures {name} at {found.group(1)}px while the ladder's "
            f"rung {rung} draws {ladder[rung]}px, so it would mark a legal rung "
            "off-ladder on a real capture"
        )
    # The docstring's own statement of the rungs is part of the copy: a reader
    # of the tool has to be told the same heights the tool matches against.
    docstring = tool.split('"""')[1]
    for name, rung in ROLE_RUNG.items():
        assert str(ladder[rung]) in docstring, (
            f"the tool's docstring never names the {ladder[rung]}px height its "
            f"{name} rung draws at"
        )
