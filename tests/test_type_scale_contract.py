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

These contracts pin five things:

1. the SDK's rungs are the document's cap heights, and each rung is the
   height named above;
2. no application can reach a size the ladder does not have, and none
   draws below the caption rung -- a seven-pixel label reads as a smudge on
   the panel, and that is what a screen that has left the scale looks like;
3. every application can reach every rung, so a screen that wants a page
   title has one to reach for;
4. the host-side copy of the ladder the measurement tool carries is the
   ladder, rung for rung;
5. which applications carry the person's text size into what they draw, and
   which of them state plainly that they do not -- a ladder nobody climbs is
   not a ladder, and a screen that ignores the setting is the difference
   between a device that adjusts and one that only claims to.

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


# The applications that read the size once, before their first frame, and never
# again while the screen is alive. A name here is a smaller gap than the column
# above -- the rung arrives, it just arrives once -- and the gate below insists it
# is *exactly* once, so the screen that starts re-reading has to be moved out of
# this list rather than left in it lying.
#
# Empty is the claim that every screen which reads the size reads it again per
# wake, because the size is changed in Settings while the screen is alive.
PUBLISHES_AT_BUILD_ONLY: set[str] = set()


# The applications that carry the person's text size, each with the helper that
# publishes it. A name here is a claim with three parts, every one of them
# checked below: the app asks the platform for the rung, it names a publisher,
# and it calls that publisher at least twice -- once before its first frame is
# laid out, so the opening screen is not drawn at a rung the person has already
# left, and once per wake, because the size is changed in Settings while the
# screen is alive. An app named in `PUBLISHES_AT_BUILD_ONLY` is checked against
# the weaker claim it can actually meet.
#
# What the list cannot see is how far a rung travels once it arrives. Five of
# these screens are only part-way, and each says so in its own file: Notes reads
# the rung and lays its empty-state card from it, while its chips, PIN filter,
# note rows and editor still stand on authored pixels; Gallery lays its four
# album cards from the rung while its header, filter chips, media cells, timeline
# rows, empty states and viewer plate keep the pixels they were drawn with;
# Calendar lays its agenda cards, while its month grid, header, tabs, weekday
# row, section band, month list, week and day views, details modal and create
# sheet keep theirs; Music lays the queue's track rows -- plate, run origins,
# reorder centres, stride, divider, scroll extent and the press that answers them
# -- and the list's empty state from it, while the artwork card and its caption,
# the now-playing stack, the search row, the sort and import controls, the
# availability caption, the timeline, the volume row, the transport discs, the
# queue header, the publish/ringtone/add controls and the add editor keep the
# pixels they were drawn with; and the shell lays the app drawer's catalogue
# names, its empty state and the power menu's whole stack -- header, both action
# plates, the Cancel pill and the sheet that contains them -- from the rung, while
# the drawer's chips, search field, icon badges and status clock and the home
# grid, shade, lock, recents, search surface and widget cards keep theirs -- each
# of those sits in a pill or band authored to the run inside it, so its painter,
# its plate and its tap test have to move in one stroke. Moving the rest means
# moving each painter and its tap handlers in the same change.
# `tests/test_notes_truth_contract.py`,
# `tests/test_gallery_truth_contract.py`,
# `tests/test_calendar_truth_contract.py`,
# `tests/test_music_truth_contract.py`,
# `tests/test_launcher_power_menu_contract.py` and
# `tests/test_launcher_dirty_band_contract.py` are the files that say how far each
# is carried.
# `camera` carries the rung through its viewfinder chrome: the status card, the
# settings sheet's plates and pagination, the record chip's readout, the capsule
# labels and the shutter's refusal caption are each derived from the rung the
# renderer was handed, and a painted box, its label's plate and its tap band come
# from the same one. The refusal caption is the one place the panel cannot hold the
# rung it is asked for: its room is the strip between the dock's top edge and the
# shutter's ring, 43 rows, and the body rung stands 59 at Extra Large, so the
# caption steps down there rather than inking the ring.
RUNG_AWARE_APPS = {
    "agent_manager": "publish_text_scale",
    "browser": "publish_text_scale",
    "calculator": "publish_text_scale",
    "camera": "publish_text_scale",
    "calendar": "publish_text_scale",
    "dialer": "publish_text_scale",
    "file_manager": "publish_text_scale",
    "gallery": "publish_text_scale",
    "launcher": "publish_text_scale",
    "messages": "publish_user_text_scale",
    "music": "publish_text_scale",
    "notes": "publish_text_scale",
    "settings": "publish_text_scale",
    "vault": "publish_text_scale",
    "video_player": "publish_text_scale",
}

# The applications that lay words on the panel and still lay them out at the rung
# the machine was built at. All 15 text-rendering applications now read and publish
# the user's preferred text scale rung.
RUNG_BLIND_APPS: set[str] = set()

# The stems of the calls that put a run of text on the panel -- `test_text`, its
# anti-aliased arm, the outline face's `place_run`, the kit's centred labels and
# the app-side wrappers named after them. A stem rather than a full name because
# an app that grew a `test_text_aa_user` is still drawing text.
TEXT_DRAW_STEMS = ("test_text", "place_run", "draw_text", "run_text", "text_centered", "draw_label")

SCALE_KEY = '"display.text_scale"'


def _without_comments(text: str) -> str:
    """`text` with its comments out and its string literals left in.

    WHY the strings survive: the setting's own key is a string, and a check that
    strips literals cannot see the one call this contract is about. WHY the
    comments do not: an app that only *explains* the text size in a doc comment
    has not read it, and whether it read it is exactly the claim on the table.
    """
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.S)
    return re.sub(r"^[ \t]*//.*$", " ", text, flags=re.M)


def _app_code(app: Path) -> str:
    return "\n".join(
        _without_comments(source.read_text(encoding="utf-8"))
        for source in sorted((app / "src").rglob("*.rs"))
    )


def _draws_text(code: str) -> bool:
    """Whether the app puts words on the panel at all.

    WHY a definition does not count: a screen's `#[test] fn test_text_editor_...`
    is a host test, not a run of ink, and a suite that read it as text-on-screen
    would call an app that draws nothing a drawer.
    """
    pattern = r"\b\w*(?:" + "|".join(TEXT_DRAW_STEMS) + r")\w*\s*\("
    for match in re.finditer(pattern, code):
        if code[: match.start()].rstrip().endswith("fn"):
            continue
        return True
    return False


def _asks_for_the_rung(code: str) -> bool:
    """Whether the app reads the size the person chose.

    Either half counts: the setting's key, which a publisher reads, and the font
    module's `user_text_scale`, which is the rung the renderer already holds.
    """
    return SCALE_KEY in code or "user_text_scale()" in code


def test_the_apps_that_carry_the_persons_text_size_are_named():
    """A screen either climbs the ladder or says plainly that it does not.

    Every box on the rung-aware screens is derived from the rung the renderer
    holds, so a screen that never asks for it lays out at the platform rung
    whatever the person chooses in Settings -- and Settings' "Text size" then
    changes nothing the person can see on that screen. No host test can watch a
    person choose a size, so this is pinned as a named set instead: membership is
    read out of the sources, both lists are compared against what the code
    actually does, and an app can join either column only by the code changing.
    """
    aware: set[str] = set()
    blind: set[str] = set()
    assert PUBLISHES_AT_BUILD_ONLY <= set(RUNG_AWARE_APPS), (
        "a screen written down as reading the size once has to be a screen that "
        "reads it at all -- the gap list names members of the rung-aware list"
    )
    for app in _app_dirs():
        code = _app_code(app)
        if not _draws_text(code):
            continue
        (aware if _asks_for_the_rung(code) else blind).add(app.name)
    assert aware == set(RUNG_AWARE_APPS), (
        f"the apps that read the person's text size are {sorted(aware)}, which is "
        f"not the named set {sorted(RUNG_AWARE_APPS)} -- an app that stopped "
        "reading it shows nothing until Settings is opened again, and an app that "
        "started has to be named here so the gap list stays the truth"
    )
    assert blind == RUNG_BLIND_APPS, (
    f"the apps that draw words at the rung the machine was built at are "
    f"{sorted(blind)}, not the {sorted(RUNG_BLIND_APPS)} this file states as "
    "unfinished -- move a name out of the gap list when its screen reads the size"
    )
    for name, helper in RUNG_AWARE_APPS.items():
        code = _app_code(APPS / name)
        assert f"fn {helper}" in code, (
            f"{name} reads the text size without a publisher, so nothing hands "
            "the rung to the code that lays the screen out"
        )
        calls = len(re.findall(r"(?<!fn )" + re.escape(helper) + r"\(\s*\)", code))
        if name in PUBLISHES_AT_BUILD_ONLY:
            assert calls == 1, (
                f"{name} is written down as a screen that publishes the rung once, "
                f"before its first frame, and it now publishes it {calls} times -- "
                "move it out of `PUBLISHES_AT_BUILD_ONLY` if it has started "
                "re-reading the size per wake"
            )
            continue
        assert calls >= 2, (
            f"{name} publishes the rung {calls} time(s); a screen has to read it "
            "before its first frame and again per wake, because the size is "
            "changed in Settings while the screen is alive"
        )


CACHED_SURFACE_INVALIDATORS = {
    "home.rs": "screens::home::invalidate_paint_cache",
    "app_drawer.rs": "screens::app_drawer::invalidate_app_drawer_frame",
    "live_widgets.rs": "screens::live_widgets::invalidate_widgets_cache",
    "search.rs": "screens::search::invalidate_search_frame",
    "activity.rs": "screens::activity::invalidate_activity_frame",
    "recents.rs": "screens::recents::invalidate_recents_frame",
    "notifications.rs": "screens::notifications::invalidate_events_frame",
    "quick_controls.rs": "screens::quick_controls::invalidate_frame_cache",
    "lock.rs": "screens::lock::invalidate_lock_frame",
}


def _rung_invalidation_defects(main_code: str) -> list[str]:
    defects = []
    if "fn invalidate_rung_caches()" not in main_code:
        defects.append("main.rs does not define invalidate_rung_caches")
        return defects

    start = main_code.index("fn invalidate_rung_caches()")
    brace = main_code.index("{", start)
    end = main_code.index("}", brace)
    body = main_code[brace:end]

    for screen_file, invoker in CACHED_SURFACE_INVALIDATORS.items():
        if invoker not in body:
            defects.append(f"invalidate_rung_caches does not call {invoker} for {screen_file}")

    if "let rung_moved = publish_text_scale();" not in main_code or "invalidate_rung_caches();" not in main_code:
        defects.append("main.rs does not call invalidate_rung_caches when publish_text_scale moves")

    return defects


def test_launcher_screens_with_paint_caches_are_invalidated_on_rung_change():
    """All 9 launcher screens with cached frame plans are invalidated when the text scale moves."""
    main_code = (APPS / "launcher" / "src" / "main.rs").read_text(encoding="utf-8")
    defects = _rung_invalidation_defects(main_code)
    assert defects == [], "; ".join(defects)


def test_the_launcher_rung_invalidation_gate_can_fail():
    """Each detector above trips when an invalidator is dropped or rung change does not call it."""
    main_code = (APPS / "launcher" / "src" / "main.rs").read_text(encoding="utf-8")
    assert _rung_invalidation_defects(main_code) == []

    # 1. Dropping one screen invalidator
    dropped = main_code.replace("screens::lock::invalidate_lock_frame();", "")
    assert any("screens::lock::invalidate_lock_frame" in d for d in _rung_invalidation_defects(dropped))

    # 2. Dropping the call when rung moves
    uncall = main_code.replace("invalidate_rung_caches();", "")
    assert any("does not call invalidate_rung_caches" in d for d in _rung_invalidation_defects(uncall))

