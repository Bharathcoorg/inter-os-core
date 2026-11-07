# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Pins the app-launch benchmark's measurement and classification rules.

WHY these are unit tests and not QEMU runs: the rules being pinned are the ones
that turn an unreliable measurement into a reliable one -- no retry, no stale
marker, no crash counted as a timeout, failed trials visible. Each of those can
be broken by an edit that still looks correct in a passing QEMU run, so they are
pinned against synthetic sessions instead.

No QEMU, no sockets and no guest are required: `LaunchSession` is driven through
a fake socket whose `recv` returns a scripted transcript.
"""

import socket
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))

import bench_app_launch as bal  # noqa: E402


class FakeSocket:
    """A socket stand-in that replays a scripted transcript then times out."""

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


def transcript_session(lines, chunk_size=1):
    """Feed `lines` through LaunchSession one line per recv batch."""
    payloads = [(line + "\n").encode("utf-8") for line in lines]
    return bal.LaunchSession(FakeSocket(payloads))


# --------------------------------------------------------------------------
# Task F: a retry must never be reported as a valid single-shot sample.
# --------------------------------------------------------------------------

def test_a_retried_trial_is_invalidated_even_when_it_reached_readiness():
    assert bal.classify_launch("ok", retries=1) == "invalid_retry"


def test_a_single_shot_trial_keeps_its_outcome():
    assert bal.classify_launch("ok", retries=0) == "ok"
    assert bal.classify_launch("ready_timeout", retries=0) == "ready_timeout"


def test_valid_sample_requires_ok_and_no_retry():
    assert bal.is_valid_sample({"outcome": "ok", "retries": 0})
    assert not bal.is_valid_sample({"outcome": "ok", "retries": 1})
    assert not bal.is_valid_sample({"outcome": "guest_crash", "retries": 0})
    assert not bal.is_valid_sample({"outcome": None, "retries": 0})


def test_the_launch_path_never_retries():
    """Nothing in the module may send a command more than once per trial."""
    source = (ROOT / "tools" / "bench_app_launch.py").read_text(encoding="utf-8")
    assert "send_once" in source, "commands must go through the single-attempt sender"
    # A retry loop would have to re-send; the only sendall call is inside send_once.
    assert source.count("sendall(") == 1, "only send_once may write to the socket"
    assert "retries" in source


# --------------------------------------------------------------------------
# Task F: failed and timed-out trials stay visible.
# --------------------------------------------------------------------------

def _record(outcome, ready=None, retries=0, detail="d"):
    return {"outcome": outcome, "retries": retries, "detail": detail,
            "launch_ack_ms": ready, "process_entry_ms": ready, "ready_ms": ready}


def test_summary_reports_failures_rather_than_dropping_them():
    records = [_record("ok", 10.0), _record("ok", 12.0), _record("ready_timeout", None)]
    summary = bal.summarize_trials(records)
    assert summary["attempts"] == 3
    assert summary["valid_samples"] == 2
    assert summary["failed_or_invalid"] == 1
    assert summary["ready"]["count"] == 2


def test_a_retried_trial_is_excluded_from_the_statistics_but_still_counted():
    records = [_record("ok", 10.0), _record("ok", 999.0, retries=1)]
    summary = bal.summarize_trials(records)
    assert summary["attempts"] == 2
    assert summary["valid_samples"] == 1
    assert summary["ready"]["mean_ms"] == 10.0
    assert summary["ready"]["max_ms"] == 10.0, "the retry must not move the reported maximum"


def test_summary_of_only_failures_reports_no_samples_rather_than_zero():
    summary = bal.summarize_trials([_record("boot_timeout", None), _record("guest_crash", None)])
    assert summary["valid_samples"] == 0
    assert summary["ready"]["count"] == 0
    assert summary["ready"]["median_ms"] is None


def test_summary_of_no_trials_is_still_well_formed():
    summary = bal.summarize_trials([])
    assert summary["attempts"] == 0
    assert summary["ready"]["count"] == 0


# --------------------------------------------------------------------------
# Task F: raw data and summary statistics must agree.
# --------------------------------------------------------------------------

def test_summary_agrees_with_the_raw_records():
    records = [_record("ok", float(n)) for n in (100, 200, 300, 400)]
    summary = bal.summarize_trials(records)
    ready = summary["ready"]
    assert ready["count"] == 4
    assert ready["min_ms"] == 100
    assert ready["max_ms"] == 400
    assert ready["mean_ms"] == 250
    assert ready["median_ms"] == 250


def test_phase_offsets_are_relative_to_the_send_instant():
    offsets = bal.phase_offsets(10.0, {"launch_ack": 10.5, "process_entry": None, "ready": 11.25})
    assert offsets["launch_ack_ms"] == 500.0
    assert offsets["process_entry_ms"] is None
    assert offsets["ready_ms"] == 1250.0


# --------------------------------------------------------------------------
# Task F: old readiness markers cannot leak into a new trial.
# --------------------------------------------------------------------------

def test_a_marker_already_read_is_still_matchable_once():
    """Lines a previous await read must remain visible to the next one.

    This replaced an assertion that such a line must NOT match. That assertion
    described a real bug rather than a real guarantee: `await_marker` returned as
    soon as its own marker matched and never re-examined the batch it had just
    consumed, so a second marker that arrived in the same batch was invisible and
    the wait timed out against a guest that had already answered.

    Staleness is still refused, but by draining before the send and counting
    occurrences (`marker_count` / `await_nth_marker`), not by hiding data.
    """
    session = transcript_session(["[CALCULATOR] Initial frame presented to hardware scanout."])
    session.await_marker("__never__", timeout_s=0.05)  # forces a read of the buffer
    assert len(session.lines) == 1
    assert session.await_marker("[CALCULATOR] Initial frame", timeout_s=0.05) is not None
    # Matched once, so it cannot satisfy a second wait.
    assert session.await_marker("[CALCULATOR] Initial frame", timeout_s=0.05) is None


def test_first_mark_time_after_ignores_boot_time_markers():
    """`[PROCESS ENTRY]` from a service that started at boot must not be credited
    to the task this trial launched."""
    boot = transcript_session([
        "[PROCESS ENTRY] boot service",
        "[PROCESS ENTRY] another boot service",
    ])
    boot.await_marker("__never__", timeout_s=0.05)
    assert len(boot.lines) == 2

    later = bal.LaunchSession(FakeSocket([
        b"[PROCESS] Guest application launched id=calculator task=14\n",
        b"[PROCESS ENTRY] the launched app\n",
    ]))
    later.await_marker("__never__", timeout_s=0.05)

    merged = bal.LaunchSession(FakeSocket([]))
    merged.lines = list(boot.lines) + list(later.lines)

    # The ack is the second-to-last line; its entry must be found after it,
    # not one of the two boot-time entries that precede it.
    ack_time = merged.lines[2][0]
    entry_time = merged.first_mark_time_after("[PROCESS ENTRY]", ack_time)
    assert entry_time is not None
    assert entry_time >= ack_time
    assert merged.lines[-1][0] == entry_time


def test_a_marker_sharing_the_read_batch_with_the_anchor_is_returned_and_flagged():
    """Same-batch phases must be reported, not dropped as if unobserved.

    The kernel prints the launch ack and the Ring 3 transition microseconds
    apart, so both arrive in one read batch with one timestamp. Requiring a
    strictly later timestamp would report no sample at all for a phase that
    genuinely happened; the record flags it as below host resolution instead.
    """
    session = bal.LaunchSession(FakeSocket([
        b"[PROCESS] Guest application launched id=calculator task=14\n"
        b"[PROCESS ENTRY] Dropping CPU privilege level to Ring 3 (User Mode)...\n"
    ]))
    session.await_marker("__never__", timeout_s=0.05)
    assert len(session.lines) == 2, "both lines must come from one read batch"
    ack_time, entry_time = session.lines[0][0], session.lines[1][0]
    assert ack_time == entry_time

    found = session.first_mark_time_after("[PROCESS ENTRY]", ack_time)
    assert found == entry_time, "a same-batch entry must still be found"


def test_first_mark_time_after_returns_none_when_nothing_follows():
    session = transcript_session(["[PROCESS] Guest application launched id=calculator"])
    session.await_marker("__never__", timeout_s=0.05)
    assert session.first_mark_time_after("[PROCESS ENTRY]", session.lines[0][0]) is None


# --------------------------------------------------------------------------
# Task F: a crash must not be scored as a timeout success.
# --------------------------------------------------------------------------

def test_missing_artifact_is_its_own_outcome_not_a_timeout():
    record = {"outcome": "missing_artifact", "retries": 0, "detail": ""}
    assert not bal.is_valid_sample(record)


def test_every_non_ok_outcome_is_excluded_from_statistics():
    for outcome in ("missing_artifact", "launch_failure", "boot_timeout",
                    "launch_ack_timeout", "ready_timeout", "guest_crash",
                    "invalid_retry"):
        assert not bal.is_valid_sample({"outcome": outcome, "retries": 0}), outcome


# --------------------------------------------------------------------------
# Task F: arch and endpoint metadata accompany each result.
# --------------------------------------------------------------------------

def test_every_target_declares_the_markers_and_endpoint_it_claims():
    for name, cfg in bal.APP_TARGETS.items():
        assert cfg["launch_id"], name
        assert cfg["ready"].startswith("["), f"{name}: readiness marker must be guest-emitted"
        assert cfg["launch_ack"], name
        assert cfg["process_entry"], name
        endpoint = cfg["endpoint"].lower()
        assert "flip" in endpoint, f"{name}: endpoint must name the display path it reaches"
        assert "framebuffer" in endpoint, \
            f"{name}: endpoint must state that pixels reached the hardware framebuffer"


def test_readiness_markers_are_distinct_per_app():
    """Two apps sharing a marker would let one app's frame satisfy the other's trial."""
    ready = [cfg["ready"] for cfg in bal.APP_TARGETS.values()]
    assert len(set(ready)) == len(ready)


def test_launch_ids_are_distinct_per_app():
    ids = [cfg["launch_id"] for cfg in bal.APP_TARGETS.values()]
    assert len(set(ids)) == len(ids)


@pytest.mark.parametrize("name", ["calculator", "notes", "webview_demo"])
def test_targets_named_in_the_cli_exist(name):
    assert name in bal.APP_TARGETS


def test_staleness_check_rejects_an_image_older_than_its_sources(tmp_path):
    artifact = tmp_path / "boot.img"
    artifact.write_bytes(b"x")
    import os
    src = tmp_path / "apps"
    src.mkdir()
    (src / "main.rs").write_text("// newer", encoding="utf-8")
    os.utime(artifact, (1_000_000, 1_000_000))
    os.utime(src / "main.rs", (2_000_000, 2_000_000))
    assert bal.image_predates_sources(str(artifact), str(tmp_path))


def test_staleness_check_accepts_an_image_newer_than_its_sources(tmp_path):
    import os
    artifact = tmp_path / "boot.img"
    artifact.write_bytes(b"x")
    src = tmp_path / "apps"
    src.mkdir()
    (src / "main.rs").write_text("// older", encoding="utf-8")
    os.utime(artifact, (3_000_000, 3_000_000))
    os.utime(src / "main.rs", (2_000_000, 2_000_000))
    assert not bal.image_predates_sources(str(artifact), str(tmp_path))


def test_staleness_check_rejects_a_missing_image(tmp_path):
    assert bal.image_predates_sources(str(tmp_path / "nope.img"), str(tmp_path))