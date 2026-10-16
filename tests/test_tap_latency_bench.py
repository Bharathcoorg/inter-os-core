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
    source = _calculator_main()
    assert "const COL_X: [usize; 4] = [28, 194, 360, 526];" in source
    assert "const ROW_Y: [usize; 5] = [440, 610, 780, 950, 1120];" in source
    assert "const BTN_W: usize = 144;" in source
    assert "const BTN_H: usize = 140;" in source
    # Digit '7' is keypad row 1, column 0.
    assert "(1, 0) => Some('7')" in source
    # Row 1 starts at y=610, column 0 at x=28; both centres must be inside
    # their own button so the tap cannot fall between two keys.
    assert btl.KEYPAD_X == 28 + 144 // 2
    assert btl.KEYPAD_Y == 610 + 140 // 2
    assert 28 <= btl.KEYPAD_X < 28 + 144
    assert 610 <= btl.KEYPAD_Y < 610 + 140


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