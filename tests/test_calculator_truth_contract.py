# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""The calculator's paint cost, measured off the code that pays it.

A live-device review found the calculator's taps laggy. The screen's answer to a
digit is dominated by how many pixels the app re-inks and by what it rebuilds
while inking them -- not by the flip. These gates therefore measure the two
quantities that decide a tap's cost, rather than reading a comment that says the
work was done:

  1. what the per-pixel callback builds while it runs. The pixel path is asked a
     little over a million questions a frame, so a layout object constructed
     inside it is constructed a million times;
  2. how many pixels a press re-inks. This is read out of the production
     classifier (`calculator::chrome::press_dirty`) by a host test that prints
     its own regions, and the printed numbers are then re-derived here from the
     geometry constants the classifier reads -- `apps/calculator/src/chrome.rs`
     and `apps/calculator/src/lib.rs` -- so a region that was widened, narrowed
     or simply typed in cannot pass.

A gate that only quoted the size of the panel would pass whether the app walked
the panel or the rows it moved, which is exactly the mistake these replace.
Nothing here runs QEMU and nothing here measures a latency.
"""

from pathlib import Path
import re
import subprocess

import pytest

ROOT = Path(__file__).resolve().parents[1]
MAIN = ROOT / "apps" / "calculator" / "src" / "main.rs"
CHROME = ROOT / "apps" / "calculator" / "src" / "chrome.rs"
LIB = ROOT / "apps" / "calculator" / "src" / "lib.rs"
VIEWPORT = ROOT / "sdk" / "rust" / "interos-std" / "src" / "design" / "mod.rs"

# The rows the platform's status band owns -- the band the pixel path answers for
# every one of its pixels, and the first region a banded repaint must not walk.
STATUS_ROWS = 48


def code_only(path: Path) -> str:
    """`path` with its comments out, so a gate can only be fed by executable text."""
    source = path.read_text(encoding="utf-8", errors="replace")
    without_block = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
    return re.sub(r"//[^\n]*", " ", without_block)


def body_of(code: str, signature: str) -> str:
    """The text of one item, from its signature to the next item at the same indent."""
    start = code.index(signature)
    head = code[start:].split("\n", 1)[0]
    indent = len(head) - len(head.lstrip())
    lines = code[start:].split("\n")
    for offset, line in enumerate(lines[1:], start=1):
        if not line.strip():
            continue
        if (len(line) - len(line.lstrip())) <= indent and offset > 1:
            return "\n".join(lines[:offset])
    return "\n".join(lines)


def const_int(code: str, name: str) -> int:
    """The integer a `pub const <name>: usize = <int>;` answers."""
    match = re.search(rf"pub const {name}: usize = (\d+);", code)
    assert match, f"{name} is no longer a plain integer constant -- re-audit, do not guess"
    return int(match.group(1))


def grid(code: str, name: str) -> list:
    """The members of a `pub const <name>: [usize; N] = [..];`."""
    match = re.search(rf"pub const {name}: \[usize; \d+\] = \[([0-9,\s]+)\];", code)
    assert match, f"{name} is no longer a plain array -- re-audit, do not guess"
    return [int(v) for v in match.group(1).replace(" ", "").strip(",").split(",")]


def panel() -> tuple:
    viewport = code_only(VIEWPORT)
    width = const_int(viewport, "LOGICAL_WIDTH")
    height = const_int(viewport, "LOGICAL_HEIGHT")
    return width, height


# ---------------------------------------------------------------------------
# The measurement: what the production classifier says a press re-inks.
# ---------------------------------------------------------------------------


def measure_dirty_regions() -> dict:
    """Run the host test that paints its regions out, and parse them.

    WHY a subprocess and not a copy of the arithmetic: the numbers this gate
    checks are the ones `calculator::chrome::press_dirty` answers at run time,
    from the same constants the painter walks. A second implementation here
    would certify itself.
    """
    res = subprocess.run(
        ["cargo", "test", "-p", "calculator", "--lib", "test_measured_dirty_regions",
         "--", "--nocapture"],
        capture_output=True, text=True, timeout=600, cwd=str(ROOT),
    )
    assert res.returncode == 0, (
        f"the calculator's own dirty-region measurement did not run:\n{res.stdout}\n{res.stderr}"
    )
    regions = {}
    for line in res.stdout.splitlines():
        match = re.match(r"^CALC-DIRTY name=(\w+) rects=(\d+) pixels=(\d+)(.*)$", line.strip())
        if not match:
            continue
        rects = [tuple(int(v) for v in part.split(","))
                 for part in re.findall(r"rect\d+=([0-9,]+)", match.group(4))]
        regions[match.group(1)] = {
            "rects": int(match.group(2)),
            "pixels": int(match.group(3)),
            "regions": rects,
        }
    assert regions, "the measurement printed no CALC-DIRTY line; the gate has nothing to check"
    return regions


@pytest.fixture(scope="module")
def measured():
    return measure_dirty_regions()


def test_the_measurement_reports_its_own_areas():
    """A region list that does not sum to its own pixel count is not a measurement."""
    regions = measure_dirty_regions()
    for name, entry in regions.items():
        assert len(entry["regions"]) == entry["rects"], name
        assert sum(w * h for _, _, w, h in entry["regions"]) == entry["pixels"], name


def test_a_digit_press_re_inks_the_display_rows_and_nothing_above_them(measured):
    """The rows a digit moves are the card's, and they are the chrome's numbers.

    The digit key's own cap does not change -- nothing on the keypad reads the
    expression -- so the keypad rows are not walked, and neither is the status
    band above them.
    """
    width, _ = panel()
    chrome = code_only(CHROME)
    y0 = const_int(chrome, "DISPLAY_Y0")
    y1 = const_int(chrome, "KEYPAD_TOP")
    entry = measured["digit"]
    assert entry["regions"] == [(0, y0, width, y1 - y0)], (
        f"a digit press re-inks {entry['regions']}, not the display rows "
        f"[{y0}, {y1}) the chrome names"
    )
    assert all(y >= y0 and y + h <= y1 for _, y, _, h in entry["regions"]), (
        f"a digit press re-inked rows outside [{y0}, {y1}): {entry['regions']}"
    )
    assert not any(y < STATUS_ROWS for _, y, _, _ in entry["regions"]), (
        "a digit press walked the status band's rows"
    )


def test_an_operator_press_re_inks_the_display_rows_and_the_two_caps(measured):
    """Arming an operator moves its cap's shading and the previous one's, no more.

    The armed key is drawn from `active_operation`, so a press that arms one key
    and disarms another has two caps to re-ink -- and the display badge is in the
    card. That is three regions; it is not the panel.
    """
    width, _ = panel()
    chrome = code_only(CHROME)
    lib = code_only(LIB)
    y0 = const_int(chrome, "DISPLAY_Y0")
    y1 = const_int(chrome, "KEYPAD_TOP")
    rows = y1 - y0
    btn_w = const_int(chrome, "BTN_W")
    btn_h = const_int(lib, "KEYPAD_ROW_H")
    cols = grid(chrome, "COL_X")
    caps = grid(chrome, "ROW_Y")

    entry = measured["operator"]
    assert entry["rects"] == 3, (
        f"an operator press carries {entry['rects']} regions; the display rows and "
        "the two keycaps are three"
    )
    card, first, second = entry["regions"]
    assert card == (0, y0, width, rows), card
    for cap in (first, second):
        assert cap[2] == btn_w and cap[3] == btn_h, (
            f"{cap} is not a keycap of {btn_w}x{btn_h}; the caps are what the "
            "keypad's ink answers, so the region that re-inks them has to be one"
        )
        assert cap[0] in cols and cap[1] in caps, f"{cap} is not on the keypad grid"
        assert cap[1] >= y1, f"{cap} lies outside the keypad rows it belongs to"
    assert first != second, "the two caps that changed shading are the same cell"

    # The claim this whole gate file exists for: an operator press is a small
    # part of the panel, and a change back to the whole-panel walk trips it.
    width, height = panel()
    assert entry["pixels"] == rows * width + 2 * btn_w * btn_h, (
        f"an operator press re-inks {entry['pixels']} pixels, which is not the "
        f"card's rows plus the two caps the chrome's own numbers make"
    )
    assert entry["pixels"] * 3 <= width * height, (
        f"an operator press re-inks {entry['pixels']} of {width * height} pixels"
    )


def test_the_panel_walk_is_still_the_panel(measured):
    """The reference the other numbers are compared against, measured not remembered."""
    width, height = panel()
    assert measured["panel"]["regions"] == [(0, 0, width, height)]
    assert measured["panel"]["pixels"] == width * height


def test_an_idle_or_unrelated_press_walks_nothing(measured):
    """A tap that answers no key re-inks no pixel.

    This is the shape that makes the two numbers above mean something: the model
    can express zero, so a region list is a decision and not a default.
    """
    assert measured["none"]["pixels"] == 0
    assert measured["none"]["rects"] == 0


# ---------------------------------------------------------------------------
# 2. What the per-pixel callback builds while it runs.
# ---------------------------------------------------------------------------


def test_the_pixel_path_builds_no_band_geometry():
    """`StatusBand::new(W)` lays out the instruments; it is not a per-pixel job.

    The band's box and its strip are a function of the panel's width alone. Built
    inside the pixel callback, they are built once per pixel of the band's rows
    and once per pixel of every repaint that walks them.
    """
    main = code_only(MAIN)
    pixel_body = body_of(main, "fn get_calculator_pixel(")
    for built in ("StatusBand::new(", "Instruments::unmeasured()"):
        assert built not in pixel_body, (
            f"the pixel callback still constructs {built.strip('(')} for every "
            "pixel it is handed"
        )
    assert re.search(r"STATUS_BAND\s*\.\s*pixel\(", pixel_body), (
        "the band's pixels must be answered by the geometry this file resolved "
        "once, not by a copy of it"
    )


def test_the_band_geometry_is_resolved_once_at_file_scope():
    """The cache is a `static`, so it costs no heap and no per-frame work."""
    main = code_only(MAIN)
    assert re.search(
        r"^static STATUS_BAND: components::StatusBand = components::StatusBand::new\(W\);",
        main,
        flags=re.M,
    ), "the band's geometry is not resolved once at file scope into a static"
    assert main.count("StatusBand::new(W)") == 1, (
        "the band is laid out more than once in this file; the pixel path is "
        "supposed to read one answer"
    )


# ---------------------------------------------------------------------------
# 3. The settings query: one per wake, never one per repaint.
# ---------------------------------------------------------------------------


def test_the_text_rung_is_not_read_inside_a_repaint():
    """`settings_get` is a blocking IPC, and a repaint used to pay it every time.

    The rung belongs to the wake that brought the repaint about; reading it in
    `repaint` costs an IPC per presented frame and forces the whole panel to be
    re-inked whenever the value differs from what the last frame used.
    """
    main = code_only(MAIN)
    repaint_body = body_of(main, "fn repaint(")
    assert "publish_text_scale()" not in repaint_body, (
        "repaint still asks the settings store on its own account"
    )
    for signature in ("fn render_dirty(", "fn render_calculator_ui(", "fn render_calculator_band("):
        if f"fn {signature[3:]}" not in main:
            continue
        assert "publish_text_scale()" not in body_of(main, signature), (
            f"{signature[3:-1]} reads the settings store inside the paint walk"
        )


def publish_calls(code: str) -> int:
    """How many times `code` asks the settings store for the rung.

    WHY the definition is not a call: `fn publish_text_scale()` carries the same
    name the call sites do, and counting it would make every number here one too
    large -- and a gate calibrated on the wrong count is a gate that passes the
    defect it was written to stop.
    """
    return len(re.findall(r"(?<!fn )publish_text_scale\(\)", code))


def test_the_text_rung_is_read_once_per_wake_and_adoption_is_a_full_repaint():
    """The behaviour delivered earlier, kept: a real size change resizes the screen.

    WHY this is not the same as the check above: reading the rung per wake is only
    an improvement if a moved rung still re-inks the panel and re-wraps the tape;
    a rung read nobody acts on is a setting that does nothing.
    """
    main = code_only(MAIN)
    start = main.index('pub extern "C" fn _start')
    after_start = main[start:]
    reactor = after_start[after_start.index("    loop {"):]
    assert publish_calls(reactor) == 1, (
        "the event loop must read the rung once per wake; a second read inside it "
        "is a second settings query for one wake"
    )
    assert publish_calls(main) == 2, (
        "the rung is read at launch and once per wake -- a third call site is a "
        "call inside the paint path"
    )
    wake = body_of(reactor, "if let Ok(events) = std::event_poll(")
    assert "publish_text_scale()" in wake, "the per-wake read is not in the wake path"
    assert re.search(r"events\s*!=\s*0\s*&&\s*publish_text_scale\(\)", wake), (
        "the rung is read on every poll, timed out or not: an idle app would ask "
        "the settings store a hundred times a second"
    )
    assert re.search(r"if rung_moved \{[\s\S]{0,400}?calc\.sync_tape\(\)", wake), (
        "a moved rung has to re-wrap the tape before the frame is planned at it"
    )
    assert re.search(r"Dirty::all\(\)", wake), (
        "a moved rung must re-ink the whole panel: every band is laid out at the rung"
    )


# ---------------------------------------------------------------------------
# 4. The repaint walks the regions it is handed.
# ---------------------------------------------------------------------------


def test_the_repaint_walks_regions_and_not_two_hardcoded_walks():
    """One walker, one plan, and the regions the press chose.

    WHY this is pinned: with `render_calculator_ui` and `render_calculator_band`
    each owning their own loop, a caller can hand a small region to a walker that
    ignores it, and the classifier's arithmetic above stops meaning anything.
    """
    main = code_only(MAIN)
    repaint_body = body_of(main, "fn repaint(")
    assert "render_dirty(" in repaint_body, (
        "repaint does not hand its regions to the region walker"
    )
    walk_body = body_of(main, "fn render_dirty(")
    assert "surface_write(" in walk_body and ".regions(" in walk_body, (
        "the region walker does not iterate the regions it was handed"
    )
    assert "while offset < W * H" not in walk_body, (
        "the region walker still owns a whole-panel loop of its own"
    )


def test_no_keypad_press_falls_back_to_the_whole_panel():
    """Every keypad answer names its regions; the drawer and the rung may not.

    WHY keyed on the call site and not a hand-written list: the press is what the
    repaint exists for, so a new key path that forgot the regions fails here
    instead of in front of somebody tapping.
    """
    main = code_only(MAIN)
    body = body_of(main, "fn handle_touch_point(")
    press = body[body.index("calc.press_key(action);"):]
    press = press[: press.index("return Tap::None")]
    assert "chrome::press_dirty(" in press, (
        "the keypad's touch answer no longer comes from the region classifier"
    )
    assert "Dirty::all()" not in press, (
        "a keypad press is again asking for the whole panel"
    )


def test_the_host_tests_that_measure_the_regions_are_run_by_the_suite():
    """The measurement above is a test the crate runs, not a snippet this file owns.

    WHY the classifier's own definition is checked here too: the measurement is
    only a measurement if it asks the function the binary asks. A second copy of
    the arithmetic, sitting in the test file and agreeing with itself, would pass
    every number above while the app walked the panel.
    """
    lib = code_only(LIB)
    assert "fn test_measured_dirty_regions" in lib, (
        "the calculator crate no longer carries the measurement this gate reads"
    )
    assert lib.count("press_dirty(") >= 4, (
        "the measurement no longer asks the production classifier about presses"
    )
    chrome = code_only(CHROME)
    assert "pub fn press_dirty(" in chrome, (
        "the classifier the measurement calls is not defined in the module that "
        "owns the rects it names"
    )


if __name__ == "__main__":
    measured = measure_dirty_regions()
    test_the_measurement_reports_its_own_areas()
    test_a_digit_press_re_inks_the_display_rows_and_nothing_above_them(measured)
    test_an_operator_press_re_inks_the_display_rows_and_the_two_caps(measured)
    test_the_panel_walk_is_still_the_panel(measured)
    test_an_idle_or_unrelated_press_walks_nothing(measured)
    test_the_pixel_path_builds_no_band_geometry()
    test_the_band_geometry_is_resolved_once_at_file_scope()
    test_the_text_rung_is_not_read_inside_a_repaint()
    test_the_text_rung_is_read_once_per_wake_and_adoption_is_a_full_repaint()
    test_the_repaint_walks_regions_and_not_two_hardcoded_walks()
    test_no_keypad_press_falls_back_to_the_whole_panel()
    test_the_host_tests_that_measure_the_regions_are_run_by_the_suite()
    print("Calculator paint-cost truth contract: PASS")
