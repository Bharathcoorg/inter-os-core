# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Pins the heap-reading rules in `tools/bench_heap.py`.

WHY these matter: the whole point of the tool is to report a number that came
from the allocator rather than from a configuration constant. A parser that
silently accepted a malformed or mismatched line would turn that guarantee back
into the assumption it replaced, so the parser and the `used + free == HEAP_SIZE`
invariant are pinned here. No guest is needed: these are pure functions.
"""

import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))

import bench_heap as bh  # noqa: E402


HEAP_SIZE = 64 * 1024 * 1024
GOOD_LINE = ("[DIAG_STATE] tasks_total=14 tasks_active=9 surfaces=3 mailboxes=2 "
             "endpoints=4 shm_pipes=0 heap_used=4194304 heap_free=%d"
             % (HEAP_SIZE - 4194304))


def test_parses_a_real_diag_state_line():
    reading = bh.parse_diag_state(GOOD_LINE)
    assert reading["heap_used"] == 4194304
    assert reading["heap_free"] == HEAP_SIZE - 4194304
    assert reading["tasks_total"] == 14
    assert reading["surfaces"] == 3


def test_ignores_unrelated_serial_output():
    assert bh.parse_diag_state("[KMS PERF] flip=4 compose_cycles=1") is None
    assert bh.parse_diag_state("") is None


def test_rejects_a_truncated_diag_state_line_rather_than_defaulting():
    """A line missing the heap fields must not produce a reading with zeros."""
    assert bh.parse_diag_state("[DIAG_STATE] tasks_total=14 tasks_active=9") is None


def test_rejects_non_numeric_heap_values():
    bad = GOOD_LINE.replace("heap_used=4194304", "heap_used=unknown")
    assert bh.parse_diag_state(bad) is None


def test_invariant_accepts_a_self_consistent_reading():
    assert bh.check_invariant(bh.parse_diag_state(GOOD_LINE)) is None


def test_invariant_rejects_a_reading_that_does_not_sum_to_the_heap_size():
    reading = bh.parse_diag_state(GOOD_LINE)
    reading["heap_free"] += 4096
    problem = bh.check_invariant(reading)
    assert problem is not None
    assert "does not equal" in problem


def test_invariant_rejects_a_missing_reading():
    assert bh.check_invariant(None) == "reading is None"


def test_summary_refuses_to_average_inconsistent_readings():
    good = bh.parse_diag_state(GOOD_LINE)
    bad = dict(good)
    bad["heap_used"] = 1
    bad["heap_free"] = bh.HEAP_SIZE - 1 + 5
    readings = [
        {"checkpoint": "services_up", "reading": good},
        {"checkpoint": "after_app_launch", "reading": bad},
    ]
    summary = bh.summarize_readings(readings)
    assert summary["consistent_readings"] == 1
    assert summary["peak_used_bytes"] == good["heap_used"]
    assert summary["inconsistent"], "the bad reading must be reported, not dropped"


def test_summary_of_all_failures_reports_no_peak_rather_than_zero():
    summary = bh.summarize_readings([{"checkpoint": "x", "reading": None}])
    assert summary["consistent_readings"] == 0
    assert summary["peak_used_bytes"] is None
    assert summary["peak_utilisation_pct"] is None


def test_summary_reports_peak_and_final_separately():
    early = bh.parse_diag_state(GOOD_LINE)
    late = dict(early)
    late["heap_used"] = 1024
    late["heap_free"] = bh.HEAP_SIZE - 1024
    summary = bh.summarize_readings([
        {"checkpoint": "services_up", "reading": early},
        {"checkpoint": "after_app_launch", "reading": late},
    ])
    assert summary["peak_used_bytes"] == early["heap_used"]
    assert summary["final_used_bytes"] == 1024
    assert summary["final_free_bytes"] == bh.HEAP_SIZE - 1024


def test_peak_utilisation_is_a_percentage_of_the_configured_heap():
    reading = {"heap_used": bh.HEAP_SIZE // 2, "heap_free": bh.HEAP_SIZE // 2}
    summary = bh.summarize_readings([{"checkpoint": "x", "reading": reading}])
    assert summary["peak_utilisation_pct"] == 50.0


def test_heap_size_matches_the_kernel_constant():
    """If the kernel resizes its heap, this tool must not keep reporting 64 MiB."""
    source = (ROOT / "kernel" / "src" / "allocator.rs").read_text(encoding="utf-8")
    assert "pub const HEAP_SIZE: usize = 64 * 1024 * 1024;" in source
    assert bh.HEAP_SIZE == 64 * 1024 * 1024


def test_checkpoints_key_off_markers_the_guest_actually_prints():
    """A checkpoint must be a string the guest emits, not a delay or an assumption."""
    guest_roots = [ROOT / "kernel" / "src", ROOT / "apps"]
    corpus = []
    for base in guest_roots:
        for path in base.rglob("*.rs"):
            corpus.append(path.read_text(encoding="utf-8", errors="replace"))
    haystack = "\n".join(corpus)
    for name, marker, description in bh.CHECKPOINTS:
        assert marker in haystack, f"{name}: marker {marker!r} is not printed by any guest source"
        assert "sleep" not in description.lower()


@pytest.mark.parametrize("name", ["services_up", "after_app_launch"])
def test_checkpoint_names_used_in_the_output_exist(name):
    assert any(name == c[0] for c in bh.CHECKPOINTS)


def test_tool_states_that_it_is_not_a_high_water_mark():
    """A used/free pair must never be presented as a peak the guest can vouch for."""
    source = (ROOT / "tools" / "bench_heap.py").read_text(encoding="utf-8")
    assert "high-water" in source.lower()