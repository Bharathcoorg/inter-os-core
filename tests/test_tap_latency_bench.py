# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Pins the correlation rules that make the tap-to-post-flip number trustworthy.

WHY these are unit tests and not QEMU runs: every rule below is about refusing to
score a trial against the wrong event. A passing benchmark run does not
demonstrate that a stale marker was rejected, only that the right marker happened
to arrive. These pin the refusals directly, against a scripted transcript and a
fake socket, with no emulator.

The properties that matter most:
  - a marker printed before the trial cannot complete it
  - a marker from an unrelated redraw path is not counted as the response
  - a missing marker times out and fails rather than defaulting to zero
  - the tap is sent exactly once
  - raw trial counts reconcile with the summary
"""

import re
import socket
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))

import bench_app_launch as bal  # noqa: E402
import bench_tap_latency as btl  # noqa: E402


class FakeSocket:
    """Replays a scripted transcript, then times out like an idle guest."""

    def __init__(self, chunks):
        self.chunks = list(chunks)
        self.sent = []

    def sendall(self, payload):
        self.sent.append(payload)

    def recv(self, _n):
        if self.chunks:
            return self.chunks.pop(0)
        raise socket.timeout()

    def setsockopt(self, *_a):
        pass


def session_with(lines):
    return bal.LaunchSession(FakeSocket([(l + "\n").encode("utf-8") for l in lines]))


# --------------------------------------------------------------------------
# The marker must exist, sit after the flip, and only fire on an input redraw.
# --------------------------------------------------------------------------

def _calculator_main():
    return (ROOT / "apps" / "calculator" / "src" / "main.rs").read_text(encoding="utf-8")


def _calculator_lib():
    return (ROOT / "apps" / "calculator" / "src" / "lib.rs").read_text(encoding="utf-8")


def _rust_code(source: str) -> str:
    """Strips `//` line and `/* */` block comments from Rust source.

    WHY: a presence assertion that scans prose passes on a comment that quotes
    the very expression the code no longer contains. The repository already does
    this in `tests/test_native_ui_truth_contract.py` for the same reason.
    """
    without_block = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
    return re.sub(r"//[^\n]*", " ", without_block)


_USIZE_CONST = re.compile(
    r"^[ \t]*(?:pub[ \t]+)?const[ \t]+(\w+)[ \t]*:[ \t]*usize[ \t]*=[ \t]*([^;]+);",
    re.M,
)
_USIZE_TABLE = re.compile(
    r"^[ \t]*(?:pub[ \t]+)?const[ \t]+(\w+)[ \t]*:[ \t]*\[usize;[ \t]*\d+[ \t]*\]"
    r"[ \t]*=[ \t]*\[([^\]]*)\];",
    re.M,
)
_TOKEN = re.compile(r"\d+|[A-Za-z_]\w*(?:::[A-Za-z_]\w*)*|[-+*/%()\[\]]")
_ADDITIVE = ("+", "-")
_MULTIPLICATIVE = ("*", "/", "%")


def _app_layout(*sources: str):
    """Every `usize` const and `[usize; N]` table the given Rust sources declare.

    WHY parse rather than pin the literals: the keypad's geometry is deliberately
    spread and derived now -- `BTN_H` in the renderer names
    `calculator::KEYPAD_ROW_H` in the library, because the row is the plate a key
    label is drawn on, the plate a thumb lands on and the height the label clamp is
    checked against, and three places that have to agree are better as one. A
    literal pin fails on a layout that never moved, and passes on a layout that did
    move as long as the literal still survives somewhere in the file. Reading the
    bindings back and recomputing the tap is the only form of this check that means
    "the bench presses where the app paints".

    A binding written as an expression of others -- `BTN_W * 2 + (COL_X[1] -
    COL_X[0] - BTN_W)`, the width of a two-column key -- is resolved by looking
    those names up in turn, in whichever file declares them. An expression this
    reader cannot resolve raises rather than guessing.
    """
    consts, tables = {}, {}
    for source in sources:
        for name, expr in _USIZE_CONST.findall(source):
            consts.setdefault(name, expr)
        for name, items in _USIZE_TABLE.findall(source):
            tables.setdefault(name, [i.strip() for i in items.split(",") if i.strip()])
    return consts, tables


def _eval_usize(consts, tables, expr: str) -> int:
    tokens = _TOKEN.findall(expr)
    value, pos = _parse_sum(consts, tables, tokens, 0)
    assert tokens and pos == len(tokens), (
        f"`{expr}` is not a plain usize expression (stopped at {tokens[pos:pos + 2]!r})"
    )
    return value


def _read_usize(consts, tables, name: str) -> int:
    assert name in consts, f"no `const {name}: usize` in the calculator's own sources"
    return _eval_usize(consts, tables, consts[name])


def _read_table(consts, tables, name: str) -> list[int]:
    assert name in tables, f"no `const {name}: [usize; N]` in the calculator's own sources"
    return [_eval_usize(consts, tables, item) for item in tables[name]]


def _parse_sum(consts, tables, tokens, pos):
    value, pos = _parse_product(consts, tables, tokens, pos)
    while pos < len(tokens) and tokens[pos] in _ADDITIVE:
        operator = tokens[pos]
        right, pos = _parse_product(consts, tables, tokens, pos + 1)
        value = value + right if operator == "+" else value - right
    return value, pos


def _parse_product(consts, tables, tokens, pos):
    value, pos = _parse_atom(consts, tables, tokens, pos)
    while pos < len(tokens) and tokens[pos] in _MULTIPLICATIVE:
        operator = tokens[pos]
        right, pos = _parse_atom(consts, tables, tokens, pos + 1)
        if operator == "*":
            value *= right
        elif operator == "/":
            value //= right
        else:
            value %= right
    return value, pos


def _parse_atom(consts, tables, tokens, pos):
    assert pos < len(tokens), "a usize expression ended where a value was expected"
    token, pos = tokens[pos], pos + 1
    if token == "(":
        value, pos = _parse_sum(consts, tables, tokens, pos)
        assert pos < len(tokens) and tokens[pos] == ")", "an expression has no closing `)`"
        return value, pos + 1
    if token == "-":
        value, pos = _parse_atom(consts, tables, tokens, pos)
        return -value, pos
    if token.isdigit():
        return int(token), pos
    name = token.rsplit("::", 1)[-1]
    if name in tables:
        assert pos < len(tokens) and tokens[pos] == "[", f"`{token}` is a table; index it"
        index, pos = _parse_sum(consts, tables, tokens, pos + 1)
        assert pos < len(tokens) and tokens[pos] == "]", "a table index has no closing `]`"
        pos += 1
        assert 0 <= index < len(tables[name]), f"`{token}[{index}]` is out of range"
        return _eval_usize(consts, tables, tables[name][index]), pos
    assert name in consts, f"`{token}` is declared in no calculator source"
    return _read_usize(consts, tables, name), pos


def test_calculator_emits_the_post_flip_marker():
    assert btl.POST_FLIP_MARKER in _calculator_main()


def test_the_marker_is_emitted_after_the_surface_flip_it_reports():
    """A marker placed before the flip would prove nothing about presentation."""
    source = _calculator_main()
    idx = source.index(btl.POST_FLIP_MARKER)
    # The flip it must follow is the post-input one, which is the second
    # surface_flip in the file (the first is the app's initial frame).
    second_flip = source.index("surface_flip", source.index("surface_flip") + 1)
    assert idx > second_flip, "the marker must be logged after the post-input surface_flip"


def test_the_marker_cannot_fire_without_an_input_triggered_redraw():
    """It must sit inside the `dirty != Dirty::None` branch, not beside it."""
    source = _calculator_main()
    branch = source.index("if dirty != Dirty::None {")
    marker = source.index(btl.POST_FLIP_MARKER)
    assert marker > branch, "the marker must be inside the input-triggered redraw branch"
    # The branch must still close after the marker, so the log is inside it.
    close = source.index("\n            }", marker)
    assert close > marker


def test_no_marker_was_added_to_other_applications():
    for app in ("messages", "music", "gallery", "browser", "dialer"):
        path = ROOT / "apps" / app / "src"
        for rs in path.rglob("*.rs"):
            assert btl.POST_FLIP_MARKER not in rs.read_text(encoding="utf-8"), app


def test_only_one_marker_emission_site_exists():
    """Two sites would make occurrence counting ambiguous."""
    assert _calculator_main().count(btl.POST_FLIP_MARKER) == 1


def test_the_metric_is_not_named_as_photon_latency():
    """A misleading name would outlive any caveat printed next to it."""
    source = (ROOT / "tools" / "bench_tap_latency.py").read_text(encoding="utf-8")
    assert btl.METRIC_NAME == "tap_to_post_surface_flip_ms"
    lowered = source.lower()
    for banned in ("input_to_photon", "input-to-photon latency", "photon latency is"):
        assert banned not in lowered.replace("not input-to-photon latency", "")
    assert "not input-to-photon latency" in lowered


# --------------------------------------------------------------------------
# A stale marker cannot complete a new trial.
# --------------------------------------------------------------------------

def test_marker_count_sees_earlier_occurrences():
    s = session_with([btl.POST_FLIP_MARKER, btl.POST_FLIP_MARKER, "other"])
    s.await_marker("__never__", timeout_s=0.05)
    assert s.marker_count(btl.POST_FLIP_MARKER) == 2


def test_a_marker_from_before_the_trial_cannot_satisfy_the_next_occurrence():
    """The core anti-stale rule: occurrence N+1 cannot be satisfied by occurrence N."""
    s = session_with([btl.POST_FLIP_MARKER])
    s.await_marker("__never__", timeout_s=0.05)
    before = s.marker_count(btl.POST_FLIP_MARKER)
    assert before == 1
    # Waiting for the next one must time out: nothing new was printed.
    assert s.await_nth_marker(btl.POST_FLIP_MARKER, before + 1, timeout_s=0.05) is None


def test_the_next_occurrence_completes_the_trial():
    s = bal.LaunchSession(FakeSocket([(btl.POST_FLIP_MARKER + "\n").encode("utf-8")]))
    before = 0
    at = s.await_nth_marker(btl.POST_FLIP_MARKER, before + 1, timeout_s=0.5)
    assert at is not None


def test_await_nth_returns_none_when_n_is_never_reached():
    s = session_with([btl.POST_FLIP_MARKER, btl.POST_FLIP_MARKER])
    s.await_marker("__never__", timeout_s=0.05)
    assert s.await_nth_marker(btl.POST_FLIP_MARKER, 3, timeout_s=0.05) is None


# --------------------------------------------------------------------------
# Unrelated markers and unrelated redraws are not the response.
# --------------------------------------------------------------------------

def test_an_unrelated_redraw_marker_is_not_counted_as_the_post_flip_marker():
    s = session_with([
        "[KMS PERF] flip=912 compose_cycles=1 present_cycles=2",
        "[CALCULATOR] Keypad digit pressed.",
        "[LAUNCHER] Rendering launcher UI canvas",
    ])
    s.await_marker("__never__", timeout_s=0.05)
    assert s.marker_count(btl.POST_FLIP_MARKER) == 0
    assert s.await_nth_marker(btl.POST_FLIP_MARKER, 1, timeout_s=0.05) is None


def test_the_input_accepted_marker_requires_the_exact_coordinates():
    """A rejected or different tap must not satisfy the acceptance check."""
    assert btl.INPUT_ACCEPTED_MARKER % (btl.KEYPAD_X, btl.KEYPAD_Y) in \
        "[COM1 INPUT] Hardware touch tap received at (%d, %d)." % (btl.KEYPAD_X, btl.KEYPAD_Y)
    wrong = btl.INPUT_ACCEPTED_MARKER % (btl.KEYPAD_X, btl.KEYPAD_Y)
    assert wrong not in "[COM1 INPUT] REJECT tap out of bounds (100, 680)"


def test_keypad_coordinates_come_from_the_apps_own_geometry():
    """The bench must press the key the app paints, read out of the app's own layout.

    WHY the literals this replaced are gone: `BTN_H` is no longer `140` written
    out in the renderer. It is `calculator::KEYPAD_ROW_H`, which the text-size
    work moved into the calculator's library so the renderer, the hit test and
    the label clamp all name one number (apps/calculator/src/lib.rs declares it
    as `pub const KEYPAD_ROW_H: usize = 140`). The keypad's geometry therefore
    did not change -- this test now proves that, instead of re-asserting it
    against a literal that had simply moved.

    WHY six rows and four columns: the keypad carries twenty-two keys and a
    four-column grid holds sixteen in four rows. The clear-all, clear-entry and
    backspace keys are three keys a person has to be able to tell apart, and a
    five-row grid had no room for them. A bench that kept the old y would tap
    the row below the one it meant and measure the wrong key.
    """
    main = _rust_code(_calculator_main())
    lib = _rust_code(_calculator_lib())
    consts, tables = _app_layout(main, lib)

    col_x = _read_table(consts, tables, "COL_X")
    row_y = _read_table(consts, tables, "ROW_Y")
    btn_w = _read_usize(consts, tables, "BTN_W")
    btn_h = _read_usize(consts, tables, "BTN_H")

    # The grid the renderer indexes is the grid the library declares. `KEY_CELLS`
    # is their product, so a grid that gained a row in one file only cannot pass.
    assert len(col_x) == _read_usize(consts, tables, "KEY_COLS")
    assert len(row_y) == _read_usize(consts, tables, "KEY_ROWS")
    assert _read_usize(consts, tables, "KEY_CELLS") == len(col_x) * len(row_y)

    # Digit '7' is keypad row 1, column 0, from the app's own key map.
    assert "(1, 0) => Some('7')" in lib

    # WHY all of this is pinned, and not just the arithmetic below: the tap is
    # only "where the app paints" while one accessor forms every key's rectangle
    # and both the painter and the hit test ask it. `key_rect`'s plain-cell arm is
    # `(row, col) -> Rect::new(COL_X[c], ROW_Y[r], BTN_W, BTN_H)`; if that arm took
    # another shape, the centre computed below would pin nothing.
    assert "fn key_rect(row: usize, col: usize) -> Option<Rect>" in main
    assert "Some(Rect::new(COL_X[c], ROW_Y[r], BTN_W, BTN_H))" in main
    assert "let rect = key_rect(row, col)?;" in main            # key_edges -- the ink
    assert "let Some(rect) = key_rect(row, col) else { return false; };" in main  # key_hit -- the press

    row, col = 1, 0
    x0, y0 = col_x[col], row_y[row]

    # Keys may not overlap: an overlapping grid would put one point inside two
    # keys, and a centre "inside its own button" would stop meaning anything.
    for left, right in zip(col_x, col_x[1:]):
        assert left + btn_w <= right, "keypad columns overlap"
    for above, below in zip(row_y, row_y[1:]):
        assert above + btn_h <= below, "keypad rows overlap"

    # The bench taps the centre of the key the app paints at (1, 0).
    assert btl.KEYPAD_X == x0 + btn_w // 2
    assert btl.KEYPAD_Y == y0 + btn_h // 2

    # ...inside its own key, and outside every other key's plate.
    assert x0 <= btl.KEYPAD_X < x0 + btn_w
    assert y0 <= btl.KEYPAD_Y < y0 + btn_h
    for index, top in enumerate(row_y):
        if index != row:
            assert not top <= btl.KEYPAD_Y < top + btn_h, (
                f"the (1, 0) centre falls inside row {index}"
            )
    for index, left in enumerate(col_x):
        if index != col:
            assert not left <= btl.KEYPAD_X < left + btn_w, (
                f"the (1, 0) centre falls inside column {index}"
            )


# --------------------------------------------------------------------------
# The tap is sent once; failures are never retried.
# --------------------------------------------------------------------------

def test_two_markers_in_one_read_batch_are_both_awaitable():
    """Regression: sequential awaits must see lines a previous await already read.

    The kernel accepts a tap, routes it and the app answers inside one serial
    batch. An await that returned as soon as its own marker matched left the
    second marker already in `lines`, and the next await only ever looked at
    bytes arriving afterwards -- so it timed out and reported a guest that had
    actually responded. That produced ten spurious failures out of twelve.
    """
    batch = (
        "[COM1 INPUT] Hardware touch tap received at (100, 680).\n"
        "[INPUT GESTURE] emitted kind=Tap pos=(100, 680)\n"
        "[CALCULATOR] Keypad digit pressed.\n"
        "[CALCULATOR] Post-input frame presented.\n"
    )
    s = bal.LaunchSession(FakeSocket([batch.encode("utf-8")]))
    accepted = s.await_marker("[COM1 INPUT] Hardware touch tap received", timeout_s=0.05)
    state = s.await_marker("[CALCULATOR] Keypad digit pressed.", timeout_s=0.05)
    post = s.await_marker("[CALCULATOR] Post-input frame presented.", timeout_s=0.05)
    assert accepted is not None
    assert state is not None, "a marker read in the same batch must still be awaitable"
    assert post is not None
    # All three arrived together, so the host cannot separate them.
    assert accepted == state == post


def test_each_await_consumes_exactly_one_match():
    """A repeated marker must not be satisfied twice by the same line."""
    s = bal.LaunchSession(FakeSocket([
        (btl.POST_FLIP_MARKER + "\n" + btl.POST_FLIP_MARKER + "\n").encode("utf-8")
    ]))
    assert s.await_marker(btl.POST_FLIP_MARKER, timeout_s=0.05) is not None
    assert s.await_marker(btl.POST_FLIP_MARKER, timeout_s=0.05) is not None
    assert s.await_marker(btl.POST_FLIP_MARKER, timeout_s=0.05) is None


def test_a_later_await_cannot_match_a_line_the_cursor_already_passed():
    s = bal.LaunchSession(FakeSocket([(btl.STATE_CHANGED_MARKER + "\n").encode("utf-8")]))
    assert s.await_marker(btl.STATE_CHANGED_MARKER, timeout_s=0.05) is not None
    assert s.await_marker(btl.STATE_CHANGED_MARKER, timeout_s=0.05) is None


def test_send_once_writes_exactly_one_line_and_never_repeats():
    sock = FakeSocket([])
    s = bal.LaunchSession(sock)
    s.send_once("TAP 100 680")
    assert sock.sent == [b"TAP 100 680\n"]


def test_the_tap_benchmark_module_never_re_sends_a_tap():
    source = (ROOT / "tools" / "bench_tap_latency.py").read_text(encoding="utf-8")
    tap_sends = [ln for ln in source.splitlines()
                 if "send_once(" in ln and '"TAP ' in ln]
    assert len(tap_sends) == 1, "the tap must be sent from exactly one place, exactly once"
    assert 'retries": 0' in source


def test_every_trial_record_declares_zero_retries():
    for outcome in ("ok", "input_rejected", "no_state_change", "post_flip_timeout"):
        assert btl.classify(outcome, 0) == outcome
    assert btl.classify("ok", 1) == "invalid_retry"


# --------------------------------------------------------------------------
# Classification: a crash or a missing marker is never a success.
# --------------------------------------------------------------------------

@pytest.mark.parametrize("outcome", [
    "missing_artifact", "launch_failure", "boot_timeout", "app_not_ready",
    "input_rejected", "no_state_change", "post_flip_timeout", "invalid_retry",
])
def test_no_failure_outcome_counts_as_a_valid_sample(outcome):
    assert not btl.is_valid_sample({"outcome": outcome, "retries": 0})


def test_a_valid_sample_needs_ok_and_no_retry():
    assert btl.is_valid_sample({"outcome": "ok", "retries": 0})
    assert not btl.is_valid_sample({"outcome": "ok", "retries": 1})


# --------------------------------------------------------------------------
# Raw counts reconcile with the summary.
# --------------------------------------------------------------------------

def _rec(outcome, value=None, retries=0, detail="d"):
    return {"outcome": outcome, "retries": retries, "detail": detail,
            "input_accepted_ms": 1.0, "state_changed_ms": 2.0,
            "tap_to_post_surface_flip_ms": value}


def test_summary_reconciles_with_raw_records():
    records = [_rec("ok", v) for v in (100.0, 200.0, 300.0, 400.0)]
    s = btl.summarize_trials(records)
    assert s["attempts"] == 4
    assert s["valid_samples"] == 4
    assert s["failed_or_invalid"] == 0
    m = s[btl.METRIC_NAME]
    assert (m["count"], m["min_ms"], m["max_ms"], m["mean_ms"]) == (4, 100.0, 400.0, 250.0)


def test_failures_stay_counted_and_excluded():
    records = [_rec("ok", 100.0), _rec("post_flip_timeout", None), _rec("ok", 999.0, retries=1)]
    s = btl.summarize_trials(records)
    assert s["attempts"] == 3
    assert s["valid_samples"] == 1
    assert s["failed_or_invalid"] == 2
    assert s[btl.METRIC_NAME]["max_ms"] == 100.0, "invalid samples must not move the maximum"


def test_summary_of_all_failures_has_no_samples_rather_than_zero():
    s = btl.summarize_trials([_rec("boot_timeout", None)])
    assert s["valid_samples"] == 0
    assert s[btl.METRIC_NAME]["count"] == 0
    assert s[btl.METRIC_NAME]["median_ms"] is None


# --------------------------------------------------------------------------
# Outlier attribution: guest work vs host scheduling.
# --------------------------------------------------------------------------

def test_attribution_is_unknown_when_the_guest_reported_nothing():
    assert "unknown" in btl.attribute_outlier({}, 1000)


def test_attribution_is_unknown_without_a_baseline():
    assert "unknown" in btl.attribute_outlier({"guest_frame_cycles": 10}, 0)


def test_a_costlier_frame_is_attributed_to_guest_work():
    verdict = btl.attribute_outlier({"guest_frame_cycles": 900}, 500)
    assert verdict.startswith("guest work")
    assert "900" in verdict and "500" in verdict, "the comparison must stay auditable"


def test_a_cheap_frame_on_a_slow_trial_is_attributed_to_host_scheduling():
    verdict = btl.attribute_outlier({"guest_frame_cycles": 400}, 500)
    assert verdict.startswith("host scheduling")


def test_guest_frame_cost_is_summarised_over_valid_trials_only():
    records = [
        dict(_rec("ok", 200.0), guest_frame_cycles=400),
        dict(_rec("ok", 700.0), guest_frame_cycles=450),
        dict(_rec("post_flip_timeout", None), guest_frame_cycles=99999),
    ]
    s = btl.summarize_trials(records)
    assert s["guest_frame_cycles"]["count"] == 2
    assert s["guest_frame_cycles"]["max_ms"] == 450, \
        "a failed trial's reading must not enter the baseline"


def test_trials_without_a_cost_do_not_enter_the_cost_baseline():
    records = [dict(_rec("ok", 200.0), guest_frame_cycles=None),
               dict(_rec("ok", 300.0), guest_frame_cycles=250)]
    s = btl.summarize_trials(records)
    assert s["guest_frame_cycles"]["count"] == 1
    assert s["guest_frame_cycles"]["min_ms"] == 250


def test_guest_frame_cost_is_read_from_the_diag_state_channel():
    """The attribution must come from the guest, not from a host-side guess."""
    session = bal.LaunchSession(FakeSocket([
        b"[DIAG_STATE] tasks_total=14 heap_used=100 heap_free=200 last_flip=91 "
        b"last_flip_compose_cycles=300000000 last_flip_present_cycles=40000000\n"
    ]))
    cost = btl.read_guest_frame_cost(session)
    assert cost == {"flip": 91, "compose_cycles": 300000000, "present_cycles": 40000000}
    assert session.sock.sent == [b"DIAG_STATE\n"], "exactly one diagnostic request per trial"


def test_cost_reading_is_none_when_the_guest_never_answers():
    session = bal.LaunchSession(FakeSocket([]))
    assert btl.read_guest_frame_cost(session, timeout_s=0.05) is None


def test_a_diag_state_without_flip_fields_yields_no_cost():
    """An older kernel must read as unknown, not as a free frame."""
    session = bal.LaunchSession(FakeSocket([
        b"[DIAG_STATE] tasks_total=14 heap_used=100 heap_free=200\n"
    ]))
    assert btl.read_guest_frame_cost(session, timeout_s=0.05) is None


def test_the_flip_number_is_reported_so_repeated_reads_are_detectable():
    session = bal.LaunchSession(FakeSocket([
        b"[DIAG_STATE] heap_used=100 heap_free=200 last_flip=91 "
        b"last_flip_compose_cycles=5 last_flip_present_cycles=5\n"
    ]))
    assert btl.read_guest_frame_cost(session)["flip"] == 91


def test_failure_reasons_are_surfaced():
    s = btl.summarize_trials([_rec("no_state_change", None, detail="tap missed the keypad")])
    assert s["failure_reasons"] == ["tap missed the keypad"]


def test_delta_is_none_when_either_endpoint_is_missing():
    assert btl._delta(1.0, None) is None
    assert btl._delta(None, 1.0) is None
    assert btl._delta(1.0, 1.25) == 250.0


# --------------------------------------------------------------------------
# Artifact handling: the benchmark refuses a stale or missing image.
# --------------------------------------------------------------------------

def test_missing_artifact_is_refused(tmp_path):
    assert btl.image_predates_sources(str(tmp_path / "absent.img"), str(tmp_path))


def test_a_fresh_image_against_old_sources_is_accepted(tmp_path):
    import os
    artifact = tmp_path / "boot.img"
    artifact.write_bytes(b"x")
    src = tmp_path / "apps"
    src.mkdir()
    (src / "main.rs").write_text("// old", encoding="utf-8")
    os.utime(artifact, (3_000_000, 3_000_000))
    os.utime(src / "main.rs", (2_000_000, 2_000_000))
    assert not btl.image_predates_sources(str(artifact), str(tmp_path))


def test_the_calculator_source_change_invalidates_the_image(tmp_path):
    """A newer calculator source must make the image refuse to be measured."""
    import os
    artifact = tmp_path / "boot.img"
    artifact.write_bytes(b"x")
    src = tmp_path / "apps" / "calculator"
    src.mkdir(parents=True)
    (src / "main.rs").write_text("// new", encoding="utf-8")
    os.utime(artifact, (2_000_000, 2_000_000))
    os.utime(src / "main.rs", (3_000_000, 3_000_000))
    assert btl.image_predates_sources(str(artifact), str(tmp_path))


def test_preflight_rejects_a_missing_artifact(tmp_path):
    problem = btl.preflight(str(tmp_path / "absent.img"), str(tmp_path))
    assert problem is not None
    assert "not found" in problem


def test_preflight_rejects_an_image_that_predates_the_calculator_change(tmp_path):
    """The guard that `dev.ps1 build` does not provide."""
    import os
    artifact = tmp_path / "boot.img"
    artifact.write_bytes(b"x")
    src = tmp_path / "apps" / "calculator"
    src.mkdir(parents=True)
    (src / "main.rs").write_text("// new", encoding="utf-8")
    os.utime(artifact, (2_000_000, 2_000_000))
    os.utime(src / "main.rs", (3_000_000, 3_000_000))
    problem = btl.preflight(str(artifact), str(tmp_path))
    assert problem is not None
    assert "predates" in problem


def test_preflight_accepts_an_image_that_matches_its_sources(tmp_path):
    import os
    artifact = tmp_path / "boot.img"
    artifact.write_bytes(b"x")
    src = tmp_path / "apps" / "calculator"
    src.mkdir(parents=True)
    (src / "main.rs").write_text("// same", encoding="utf-8")
    os.utime(artifact, (3_000_000, 3_000_000))
    os.utime(src / "main.rs", (2_000_000, 2_000_000))
    assert btl.preflight(str(artifact), str(tmp_path)) is None