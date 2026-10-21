"""Contract tests for the InterOS UI design goals.

These tests pin the rules that the native UI is measured against, so that
the goals cannot drift quietly while the apps are being worked on.

WHY this exists: the goals are written down in
`Ui Samples/INTEROS_UI_DESIGN_SYSTEM.md` and compiled into
`interos_std::design`. Nothing checked that the apps still honoured them,
and a rule that is only written down is a rule that is only remembered.
"""

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
APPS = ROOT / "apps"
SDK = ROOT / "sdk" / "rust" / "interos-std" / "src" / "design" / "mod.rs"

# Touch targets below the design system's minimum, named per app.
#
# WHY this list exists rather than a blanket failure: the list makes the
# remaining work visible and it can only shrink. An app that has been
# fixed and left in the list fails the suite, so a name cannot outlive the
# fault it was added for.
UNDERSIZED_TARGETS = {
    "messages": "RECIPIENT_FIELD",
}

# A hit rectangle written as a named constant.
RECT = re.compile(
    r"const\s+([A-Z_0-9]+)\s*:\s*\(usize,\s*usize,\s*usize,\s*usize\)\s*=\s*"
    r"\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\)"
)


def _sources(app_dir: Path):
    return [
        source
        for source in sorted(app_dir.rglob("*.rs"))
        if "target" not in source.parts
    ]


def _undersized(app_dir: Path) -> list[str]:
    """Every named hit rectangle that is shorter than the minimum target."""
    found: list[str] = []
    for source in _sources(app_dir):
        text = source.read_text(encoding="utf-8")
        for match in RECT.finditer(text):
            name, _x, _y, width, height = match.groups()
            width, height = int(width), int(height)
            # A rectangle that is not a target: a rule, a divider, a
            # background band.
            if width < 20 or height < 10:
                continue
            if height < 44:
                found.append(f"{name} ({width}x{height})")
    return found


def test_the_design_goals_hold_the_values_the_apps_are_measured_against():
    """The compiled tokens are the goals, so they are pinned here too.

    WHY: the token module is what an app actually imports. If a palette
    value or the minimum touch target moves, every app that was built
    against the old value is silently off-spec, and nothing else would
    say so.
    """
    sdk = SDK.read_text(encoding="utf-8")
    for declaration in (
        "pub const MIN_TARGET: usize = 44;",
        "pub const PREFERRED_TARGET: usize = 48;",
        "pub const PRIMARY_ACTION: usize = 56;",
        "pub const LOGICAL_WIDTH: usize = 720;",
        "pub const LOGICAL_HEIGHT: usize = 1440;",
        "pub const SCANOUT_WIDTH: usize = 1080;",
        "pub const SCANOUT_HEIGHT: usize = 2160;",
        "pub const CONTENT_PADDING: usize = 24;",
        "pub const STATUS_BAR_HEIGHT: usize = 48;",
        "pub const NAV_BAR_HEIGHT: usize = 64;",
        "pub const GRID_8: usize = 8;",
        "pub const GRID_24: usize = 24;",
        "pub const BACKGROUND: u32 = 0xFFF7F6F2;",
        "pub const SURFACE: u32 = 0xFFFFFFFF;",
        "pub const BORDER: u32 = 0xFFE1E0DB;",
        "pub const TEXT: u32 = 0xFF111214;",
    ):
        assert declaration in sdk, f"the design tokens no longer hold {declaration}"


def test_every_app_paints_the_canonical_logical_surface():
    """An app paints 720x1440 and the panel is 1080x2160.

    WHY: the apps draw in logical pixels and the compositor magnifies.
    An app that drew in panel pixels would be scaled by three in width and
    two in height, and everything on it would be stretched.
    """
    offenders = []
    for app_dir in sorted(APPS.iterdir()):
        if not app_dir.is_dir():
            continue
        sources = _sources(app_dir)
        if not sources:
            continue
        # The compositor draws the panel; the launcher owns a boot surface.
        if app_dir.name in ("compositor", "launcher"):
            continue
        text = "\n".join(source.read_text(encoding="utf-8") for source in sources)
        if "const W: usize = 720;" not in text:
            offenders.append(f"{app_dir.name}: no logical width")
        elif "const H: usize = 1440;" not in text:
            offenders.append(f"{app_dir.name}: no logical height")
    assert not offenders, "an app does not paint the canonical logical surface:\n  " + "\n  ".join(
        offenders
    )


def test_no_touch_target_is_smaller_than_the_design_system_allows():
    """Every named hit rectangle is at least 44 logical pixels tall.

    WHY 44: the design system sets 44 as the minimum touch target and 48
    as the preferred one, and calls accessibility a first-class
    requirement rather than an afterthought.

    WHY a named list rather than a blanket failure: the list makes the
    remaining work visible, and it can only shrink.
    """
    offenders = []
    for app_dir in sorted(APPS.iterdir()):
        if not app_dir.is_dir():
            continue
        found = _undersized(app_dir)
        if found and app_dir.name not in UNDERSIZED_TARGETS:
            offenders.append(f"{app_dir.name}: " + ", ".join(sorted(set(found))))
    assert not offenders, (
        "a touch target is smaller than the design system's minimum:\n  "
        + "\n  ".join(offenders)
    )


def test_the_exception_list_does_not_outlive_the_fault_it_was_added_for():
    """A named app must still have the fault it was named for.

    WHY: an exception that no longer describes anything is a hole in the
    contract that nobody remembers is there.
    """
    for name in sorted(UNDERSIZED_TARGETS):
        app_dir = APPS / name
        assert app_dir.is_dir(), f"the exception list names {name}, which is not an app"
        found = _undersized(app_dir)
        assert found, (
            f"{name} is named in the exception list but every one of its "
            "targets now meets the minimum; remove it from the list"
        )
        assert any(
            UNDERSIZED_TARGETS[name] in entry for entry in found
        ), (
            f"{name} is named in the exception list for "
            f"{UNDERSIZED_TARGETS[name]}, which no longer appears among "
            f"its undersized targets: {found}"
        )


def test_text_is_drawn_at_a_step_of_the_design_system_type_scale():
    """Every numeric text scale is one of the type scale's steps.

    WHY: the design system sets a type scale -- caption, body, title,
    display -- and the SDK measures every run against it. An app that
    draws at a size the scale does not contain has invented a size, and
    the scale stops being a scale: it becomes a set of arbitrary numbers
    that happen to look similar.

    The steps are 1, 2, 3 and 6, from `design::typography`.
    """
    steps = {1, 2, 3, 6}
    offenders = []
    for app_dir in sorted(APPS.iterdir()):
        if not app_dir.is_dir():
            continue
        found = set()
        for source in _sources(app_dir):
            text = source.read_text(encoding="utf-8")
            # test_text(x, y, sx, sy, scale, text, fg, bg, bold)
            for match in re.finditer(
                r"\btest_text\(\s*[^,()]+\s*,\s*[^,()]+\s*,\s*[^,()]+\s*,\s*"
                r"[^,()]+\s*,\s*(\d+)\s*,",
                text,
            ):
                found.add(int(match.group(1)))
            # outline_text::place_run(x0, y0, scale, text, ...)
            # outline_text::run_box(x0, y0, scale, text)
            for match in re.finditer(
                r"\b(?:place_run|run_box)\(\s*[^,()]+\s*,\s*[^,()]+\s*,\s*(\d+)\s*,",
                text,
            ):
                found.add(int(match.group(1)))
        outside = sorted(found - steps)
        if outside:
            offenders.append(f"{app_dir.name}: {outside}")
    assert not offenders, (
        "an app draws text at a size the type scale does not contain:\n  "
        + "\n  ".join(offenders)
    )
