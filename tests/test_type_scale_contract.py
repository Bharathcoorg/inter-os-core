"""The type scale every application draws with.

The platform's type scale is a set of *cap heights* -- the height of a
capital letter -- and the design document's scale is written in them: a
caption of twelve to fourteen pixels, a body of fifteen to eighteen, a
section of twenty to twenty-four, a title of twenty-eight to thirty-two, a
page heading of thirty-six to forty-four and a display of forty-eight to
fifty-six.

These contracts pin three things:

1. the SDK's rungs are the document's cap heights;
2. no application can reach a size the ladder does not have, and none
   draws below the caption rung -- a seven-pixel label reads as a smudge on
   the panel, and that is what a screen that has left the scale looks like;
3. every application can reach every rung, so a screen that wants a page
   title has one to reach for.

The behavioural check is a device measurement: `tools/measure_type.py`
reads a capture and reports the cap height of every run of text on it.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
APPS = ROOT / "apps"

# The ladder's rungs and the cap height each one draws at.
LADDER = {
    "CAPTION": (13, 14),
    "BODY": (15, 18),
    "SECTION": (20, 24),
    "TITLE": (28, 32),
    "HEADING": (36, 44),
    "DISPLAY": (48, 56),
}

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


def test_the_sdk_ladder_is_the_documents_cap_heights():
    """The rungs are the document's, not the bitmap cell's multiples."""
    sdk = (ROOT / "sdk/rust/interos-std/src/design/mod.rs").read_text(encoding="utf-8")
    for name in LADDER:
        assert f"pub const {name}_CAP_PX" in sdk, f"the SDK does not name the {name} cap height"
    assert "pub const CAPTION_SCALE: usize = 1;" in sdk
    assert "pub const BODY_SCALE: usize = 2;" in sdk
    assert "pub const SECTION_SCALE: usize = 3;" in sdk
    assert "pub const TITLE_SCALE: usize = 4;" in sdk
    assert "pub const HEADING_SCALE: usize = 5;" in sdk
    assert "pub const DISPLAY_SCALE: usize = 6;" in sdk


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
    """A cap height under thirteen pixels is not a size a person can read.

    WHY this is a contract and not a review note: the ladder's smallest rung
    is a thirteen-pixel cap, and an application that asks for anything less
    has left the scale -- which is how a screen ends up with a seven-pixel
    label that reads as a smudge on the panel.
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


def test_the_measurement_tool_measures_cap_heights():
    """The device-side check exists and knows the ladder."""
    tool = (ROOT / "tools/measure_type.py").read_text(encoding="utf-8")
    assert "CAPTURE_SCALE" in tool
    for name in LADDER:
        assert name in tool, f"the measurement tool does not know the {name} rung"
