# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Gates the power menu's boxes: the rectangle a press is measured against IS the box painted.

WHY these are source gates: the modal lives in `apps/launcher/src/screens/power.rs`,
which belongs to the shell's `#![no_main]` binary, so no host process can call
`power_menu_hit` and compare it against `get_power_menu_color`. The compile-time gates
beside the derivation do prove the arithmetic (they sweep the stack's fit across every
rung and pin the authored box at the platform's), but they are inside the binary, and
what they cannot see is the shape of the code: whether the painter and the press test
still ask the SAME expression, or have gone back to holding two numbers together by
hand.

That is not a hypothetical here -- it is the defect this file exists to refuse. The
pill was painted from `Rect::new(200, 524, 320, 54)` while its press answered
`CANCEL_HIT_TOP_Y..590` across `180..540`, two boxes solved independently. Rows 524..527
of the visible pill fell inside the row above, whose two bands were the Restart range
(`80..340`) and the Emergency range (`360..640`), so a press on the top edge of Cancel
could reboot the phone; and rows 578..589 plus twenty columns on either side answered
Cancel with no ink at all. Every gate below is one of those two failures made
unrepresentable.

Nothing here runs QEMU, and nothing here paints a pixel.
"""

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
SCREENS = ROOT / "apps" / "launcher" / "src" / "screens"
POWER = SCREENS / "power.rs"
SCALE = ROOT / "apps" / "launcher" / "src" / "scale.rs"


def power_source() -> str:
    return POWER.read_text(encoding="utf-8", errors="replace")


def scale_source() -> str:
    return SCALE.read_text(encoding="utf-8", errors="replace")


def body_of(source: str, signature: str) -> str:
    """A function's text, from its signature to the dedent that closes it.

    WHY brace counting is avoided: these bodies carry braces inside strings and
    comments, and reading one arm or one guard only needs the run of lines between the
    signature and the line that dedents back to it.

    WHY the opening brace is searched for: the sheet's placement helpers are written with
    one argument per line, so the signature is not one line, and a body measured from the
    signature's own line would stop at the `) -> Option<u32> {` that ends it.
    """
    start = source.index(signature)
    head = source[start:].split("\n", 1)[0]
    indent = len(head) - len(head.lstrip())
    lines = source[start:].split("\n")
    opened = 0
    for offset, line in enumerate(lines):
        if line.rstrip().endswith("{"):
            opened = offset
            break
    for offset, line in enumerate(lines[opened + 1 :], start=opened + 1):
        if not line.strip():
            continue
        if (len(line) - len(line.lstrip())) <= indent and offset >= opened:
            return "\n".join(lines[:offset])
    return "\n".join(lines)


def code_only(text: str) -> str:
    """`text` with its line comments removed.

    WHY this exists: the history of the hazard is written into this module's comments on
    purpose, so a gate that scanned raw text would read `590` inside the sentence that
    explains why `590` is forbidden. The gates below measure code, never prose.
    """
    return "\n".join(line.split("//", 1)[0] for line in text.split("\n"))


HIT = code_only(body_of(power_source(), "pub fn power_menu_hit("))
PAINT = code_only(body_of(power_source(), "pub fn get_power_menu_color("))

def test_each_control_has_one_box_and_both_callers_ask_it():
    """The painter and the press test must call the same box expressions."""
    for box in ("cancel_pill(", "action_plate("):
        assert box in HIT, f"the press test no longer asks {box}; it is answering a band it does not paint"
        assert box in PAINT, f"the painter no longer draws {box}; the box has become press-only"

    # Each box is built in exactly one place: a `const fn` of the rung, whose runtime
    # caller is a thin wrapper. A second definition would be a second answer.
    for definition in ("const fn cancel_rect_at(", "const fn action_plate_at("):
        assert power_source().count(definition) == 1, f"{definition} must be written once"


def test_the_press_test_holds_no_coordinate():
    """`power_menu_hit` may not contain a row or column of its own.

    WHY the bound is "no literal above two": the only arithmetic the test legitimately
    does is `index / 2` for the row and the rung's zero-based numbering. Any three-digit
    number in here is a band edge or a lane edge that somebody typed, which is precisely
    how `590`, `180..540`, `80..340` and `360..640` came to disagree with the sheet.
    """
    literals = [int(n) for n in re.findall(r"\b\d+\b", HIT)]
    assert literals, "the press test is empty -- its boxes are no longer asked anywhere"
    offenders = sorted({n for n in literals if n > 2})
    assert not offenders, (
        f"power_menu_hit answers literal coordinates {offenders}; every region it tests "
        "has to come from cancel_pill()/action_plate(), the boxes the painter draws"
    )
    assert ".." not in HIT, "a range literal is back in the press test; ask a Rect instead"


def test_the_pill_is_tested_before_the_row_bands():
    """Cancel must be the first thing a press is measured against.

    WHY the ordering is a gate and not a comment: the pill's top row is the row the plate
    above it closes on, so the two boxes share an edge exactly once -- and if the plates
    are tested first, an off-by-one in either direction hands that edge to Restart or
    Emergency, which is the hazard this modal shipped with.
    """
    assert HIT.index("cancel_pill(") < HIT.index("action_plate("), (
        "the action plates are tested before the Cancel pill, so the row they end on "
        "outranks the pill painted on it"
    )


def test_the_pill_top_is_the_plate_bottom_and_says_so_at_compile_time():
    """The pill's row must be derived from the plate above it, and pinned to the authored box."""
    definition = code_only(body_of(power_source(), "const fn cancel_rect_at("))
    assert "block_bottom_at(1, level)" in definition, (
        "the Cancel pill no longer starts on the row the second plate closes on; "
        "its top has become a number of its own again"
    )
    source = power_source()
    assert re.search(
        r"assert!\(\s*cancel_rect_at\(scale::PLATFORM_LEVEL\)\.y\s*==\s*block_bottom_at\(1,\s*scale::PLATFORM_LEVEL\)\s*\)",
        source,
    ), "the compile-time gate tying the pill's row to the plate above it is gone"
    assert re.search(
        r"assert!\(cancel_rect_at\(scale::PLATFORM_LEVEL\)\.y == CANCEL_RECT\.y\)", source
    ), "the pill's derivation is no longer pinned to the row it was authored to paint"


def test_the_two_boxes_never_reappear_as_independent_bands():
    """No constant or literal may carry a Cancel band beside the pill's own rectangle."""
    source = code_only(power_source())
    for forbidden in ("CANCEL_HIT_TOP_Y", "CANCEL_HIT_GAP", "ROW0_HIT_TOP_Y", "ROW_BOUNDARY_Y"):
        assert forbidden not in source, (
            f"{forbidden} is back: a band edge named apart from the box it covers is how "
            "the pill and the press stopped being the same rectangle"
        )
    assert "590" not in source, "the literal 590 is back -- the pill's bottom is derived"
    assert "Rect::new(200," not in source.replace(
        "Rect::new(200, 524, 320, 54)", "", 1
    ), "a second pill rectangle is authored; the box must be built by cancel_rect_at"


def test_both_callers_walk_the_same_action_table():
    """One description of the grid, walked in the same order by the ink and the press."""
    assert power_source().count("ACTIONS.iter().enumerate()") >= 2, (
        "the painter and the press test no longer walk one table, so their rows can disagree"
    )
    assert "index / 2" in HIT and "index / 2" in PAINT, (
        "the row an action belongs to is no longer the table index in both callers"
    )


def test_the_whole_modal_is_solved_from_one_rung_resolved_once_per_call():
    """A frame may not ask the published rung twice, and may not place ink level-blind."""
    for body, name in ((HIT, "power_menu_hit"), (PAINT, "get_power_menu_color")):
        assert body.count("scale::draw_level()") == 1, (
            f"{name} resolves the rung {body.count('scale::draw_level()')} times; one box "
            "per frame means one answer per frame"
        )
    # The sheet's ink is placed by two helpers, and the painter hands each of them the
    # rung it resolved. WHY the helpers are the measured text: centring and trimming are
    # what a rung moves, and a gate that only read the painter would miss them.
    placers = code_only(body_of(power_source(), "fn line_pixel(")) + code_only(
        body_of(power_source(), "fn start_pixel(")
    )
    assert "scale::ellipsize_at(" in placers, "the sheet no longer trims at the rung it inks at"
    assert "scale::center_x_painted_at(" in placers, (
        "the sheet no longer centres on the width the rung reserves, so a raised name "
        "can be inked past the lane its press answers"
    )
    assert "test_text_aa_level(" in placers, "the sheet rasterises without naming a rung"
    assert "level," in PAINT, (
        "the painter no longer passes the frame's rung down to the ink it places"
    )
    assert "cancel_pill(level)" in HIT and "action_plate(action.cx, index / 2, level)" in HIT, (
        "the press test no longer asks both boxes at the rung the frame resolved"
    )
    assert "cancel_pill(level)" in PAINT and "action_plate(action.cx, row, level)" in PAINT, (
        "the painter no longer draws both boxes at the rung the frame resolved"
    )
    # Placement inside the modal never reads the ladder directly. The gates beside the
    # derivations are allowed to: they are what proves the derivation answers the ladder
    # at the platform's rung.
    placed = HIT + PAINT + placers
    for forbidden in ("text::measure(", "text::center_x", "font::line_height(", "outline_text::em_for_scale("):
        assert forbidden not in placed, (
            f"{forbidden} places or sizes the sheet's ink: a layout read at the ladder and "
            "drawn at a rung is the drift the derived boxes exist to refuse"
        )


def test_the_grown_stack_is_swept_against_its_container():
    """The fit check must run over every selectable rung, not the authored one alone."""
    source = power_source()
    sweep = code_only(body_of(source, "const fn stack_holds_at_every_level()"))
    assert "scale::MAX_LEVEL" in sweep, "the stack's fit is no longer swept to the top rung"
    for measure in (
        "cancel_rect_at(l)",
        "sheet_rect_at(l)",
        "row0_cy_at(l)",
        "block_bottom_at(1, l)",
        "SCREEN_HEIGHT",
    ):
        assert measure in sweep, f"the swept fit gate no longer measures {measure}"
    assert "assert!(\n    stack_holds_at_every_level()," in source or "assert!(stack_holds_at_every_level())" in source.replace(" ", ""), (
        "the swept fit gate is defined but never refused"
    )


def test_a_container_the_shell_does_not_size_clamps_the_rung_it_carries():
    """The status strip is the clock's container, so the clock asks the band, not the person.

    WHY this is a gate: the sheet draws the platform's status strip, whose height is an SDK
    viewport constant the launcher may not resize. A screen that carried the published rung
    there would push the clock's box past the band it is clipped to -- the same failure in
    the opposite direction -- so the clamp lives beside the clock's own geometry in
    `status.rs`, and the band's answer is asserted rather than assumed.
    """
    assert "status::clock_rung_at(level)" in PAINT, (
        "the sheet's clock no longer asks the band what it carries"
    )
    status_src = code_only(
        body_of(
            (SCREENS / "status.rs").read_text(encoding="utf-8", errors="replace"),
            "pub fn clock_rung_at(",
        )
    )
    assert "launcher::scale::band_level_at(" in status_src, (
        "the clock's rung no longer comes from the band it is drawn in"
    )
    scale_src = scale_source()
    for face in (
        "pub const fn band_holds(",
        "pub const fn band_level_at(",
        "pub fn band_level(",
    ):
        assert face in scale_src, f"{face} is gone from scale.rs"
    assert "level > PLATFORM_LEVEL" in code_only(
        body_of(scale_src, "pub const fn band_level_at(")
    ), "the band clamp no longer floors at the authored rung, so a Normal reader could be redrawn smaller"
    assert "band_level_is_honest(" in scale_src, "the band clamp's own identity gates are gone"
