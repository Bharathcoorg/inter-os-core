#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Static contract: an idle guest keeps the serial console quiet.

Two defects put lines on the wire that nobody asked for:

* the kernel's timer tick armed a full process-table dump on a fixed two-second
  period, so a device sitting doing nothing printed one `[PROC]` line per task
  plus a header every two seconds;
* the catalogue reply that backs ``/api/apps/list`` hardcoded ``state=active``
  for every kernel-embedded package, so an installed app looked like a running
  one and a reviewer could not tell them apart.

The fixes are cadence and provenance rules, not behaviour a running guest can
assert from the host, so they are pinned here the same way the neighbouring
contracts pin source invariants. The live behaviour is covered by the kernel
unit tests ``proc_list_answers_with_the_process_table_on_demand``,
``a_never_launched_package_is_not_reported_active`` and
``the_process_table_is_printed_only_when_it_changes``.
"""
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
INTERRUPTS = ROOT / "kernel" / "src" / "interrupts.rs"
SERIAL = ROOT / "kernel" / "src" / "serial.rs"
SCHEDULER = ROOT / "kernel" / "src" / "task" / "scheduler.rs"


def text(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


@pytest.fixture(scope="module")
def interrupts() -> str:
    return text(INTERRUPTS)


@pytest.fixture(scope="module")
def serial() -> str:
    return text(SERIAL)


def test_timer_tick_never_arms_a_process_table_dump(interrupts: str) -> None:
    """No code path may print the process table because the clock rolled over.

    The dump is a header plus one `[PROC]` line per task. Printing it on a period
    is what made an idle device print ~17.6 lines per second, and it held the
    timer and an arriving touch off for the length of the whole dump.

    WHY the checks are on the declaration and the arming rather than on the
    flag's name: the replacement is documented in prose that names the flag it
    removed, and a contract that fails because a comment is honest is a contract
    nobody keeps.
    """
    assert "static PROCESS_TABLE_DUMP_DUE" not in interrupts, (
        "the periodic dump flag is still declared"
    )
    assert "PROCESS_TABLE_DUMP_DUE.store" not in interrupts, (
        "the timer tick still arms a periodic process-table dump; the table is "
        "printed on request (PROC_LIST) or when the task table changes shape"
    )
    assert "period_crossed(previous, ticks, 2000)" not in interrupts, (
        "a two-second period still drives console output"
    )


def test_process_table_dump_is_gated_on_change(interrupts: str) -> None:
    """The change detector is what replaced the timer, so it must be present."""
    assert "fn process_table_signature" in interrupts, (
        "nothing derives a signature of the task table, so a dump cannot be "
        "gated on an actual change"
    )
    assert "fn process_table_changed" in interrupts, (
        "the deferred diagnostics path does not gate the process table on change"
    )
    assert "fn dump_process_table_if_changed" in interrupts, (
        "nothing prints the table on change"
    )
    assert "PROCESS_TABLE_SHOWN_SIGNATURE" in interrupts, (
        "the last table the console was shown is not remembered, so every "
        "sample would print another one"
    )


def test_process_table_stays_reachable_on_demand(serial: str, interrupts: str) -> None:
    """Silence must not cost Studio or the runtime suites their data.

    `emulator/server.py` reads `[PROC]` lines straight off the console buffer, so
    the kernel has to answer an explicit request in exactly the shape it parses.
    """
    assert 'eq_ignore_ascii_case("PROC_LIST")' in serial, (
        "PROC_LIST is not a recognised serial verb"
    )
    assert "fn process_table_dump_now" in interrupts, (
        "PROC_LIST does not reuse the kernel's existing process-table printer"
    )


def test_catalogue_state_is_derived_not_hardcoded(serial: str) -> None:
    """`state=` must come from the task table for both catalogue branches.

    The literal format string that hardcoded `active` is the blanket value that
    made every installed app read as running in `/api/apps/list`; the resolved
    value is what belongs on the console now.
    """
    assert 'version=1.0.0 state=active process=' not in serial, (
        "the package catalogue still emits a hardcoded active state for "
        "kernel-embedded packages; the value has to be resolved per package "
        "from the live task table"
    )
    assert "fn package_run_state" in serial, (
        "no single place resolves a package's live process state"
    )
    assert "package_run_state(&sched" in serial, (
        "the catalogue reply does not consult the task table it just locked"
    )


def test_studio_pkg_line_shape_is_unchanged(serial: str) -> None:
    """Field names and order are what `guest_package_catalog` parses.

    Changing the *values* is the fix; changing the *shape* would break the
    untouchable Studio. The `[PROC]` line Studio's process panel parses is
    pinned alongside it.
    """
    assert '"[STUDIO_PKG] id={} name={} version=' in serial
    assert '"[STUDIO_PKG] END"' in serial, "the catalogue reply lost its end marker"
    assert '"[PROC] pid={} name={} state=' in text(SCHEDULER), (
        "the process-table line format drifted from what Studio parses"
    )
