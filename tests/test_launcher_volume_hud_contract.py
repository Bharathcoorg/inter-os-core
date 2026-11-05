# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Gates the volume HUD's dwell: a wall-clock budget, retired on a wake that is certain to happen.

The HUD is a surface the shell raises and lowers itself, so both halves of its
life are the shell's business. Each half failed here once already:

  1. The dwell was written as the bare literal `180` in two places. The kernel's
     system tick is 1 kHz, so 180 is 180 ms -- under eleven frames at 60 Hz. A
     person who stepped the volume and looked at the bar watched it disappear
     before their eyes reached it.
  2. The retirement ran inside the shell's `EVENT_TIMER_TICK` branch, and an idle
     shell at the home grid never enters it: 22 s of idling on the device produced
     no launcher wake-up carrying the tick bit. The budget expiring was therefore
     never noticed, and the surface stayed up.

These are source gates because the shell is a `#![no_main]` binary with no host
process that can raise a window and wait for it to fall; `apps/launcher/src/tests.rs`
covers the geometry it draws, not the loop that hides it. The device measurement
behind rule 2 is reproduced by
`test_the_expiry_runs_before_the_timer_branch_does`, which pins where in the loop
the check lives rather than trusting a comment about it.

Nothing here runs QEMU.
"""

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
SHELL = ROOT / "apps" / "launcher" / "src" / "main.rs"
KERNEL_INTERRUPTS = ROOT / "kernel" / "src" / "interrupts.rs"

BUDGET = "VOLUME_HUD_TICKS"


def shell_source() -> str:
    return SHELL.read_text(encoding="utf-8", errors="replace")


def loop_source() -> str:
    """The shell's event loop, taken as everything from `_start` onwards.

    WHY from `_start` and not the whole file: the crate declares `LauncherState`
    and a render-complete logger above the loop, both of which name the same views
    and the same fields without being part of the wake path.
    """
    source = shell_source()
    return source[source.index('pub extern "C" fn _start') :]


def tick_branch(source: str) -> str:
    """The body of the loop's `EVENT_TIMER_TICK` branch, up to its own dedent."""
    line = next(
        line for line in source.split("\n")
        if "if (events & std::EVENT_TIMER_TICK) != 0 {" in line
    )
    indent = len(line) - len(line.lstrip())
    body = source[source.index(line) :]
    lines = body.split("\n")
    for offset, candidate in enumerate(lines[1:], start=1):
        if not candidate.strip():
            continue
        if (len(candidate) - len(candidate.lstrip())) <= indent and offset > 1:
            return "\n".join(lines[:offset])
    return body


# ---------------------------------------------------------------------------
# 1. The budget is a named value whose units the kernel agrees with.
# ---------------------------------------------------------------------------


def test_the_kernel_tick_is_one_millisecond():
    """The dwell is written in ticks and read as milliseconds; that holds only at 1 kHz."""
    interrupts = KERNEL_INTERRUPTS.read_text(encoding="utf-8", errors="replace")
    assert re.search(r"pub const SYSTEM_TICK_HZ: u32 = 1_000;", interrupts), (
        "SYSTEM_TICK_HZ moved; `VOLUME_HUD_TICKS` and its doc comment both speak in"
        " milliseconds and have to be re-derived, not left asserting a rate that ended"
    )


def test_the_budget_is_named_and_readable():
    """A bare literal in a surface's own units is how 180 ms got shipped as a 'duration'."""
    declaration = re.search(
        rf"const {BUDGET}: u64 = ([\d_]+);", shell_source()
    )
    assert declaration, f"{BUDGET} is not declared as a named constant"
    dwell_ms = int(declaration.group(1).replace("_", ""))
    assert dwell_ms >= 1_000, (
        f"the HUD holds for {dwell_ms} ms; a bar carrying a number a person has to read"
        " needs at least a second"
    )


def test_no_show_site_arms_a_literal_budget():
    """Both places that raise the HUD must arm the named budget, and only that."""
    arms = re.findall(r"volume_hud_until = [^\n]*", shell_source())
    assert len(arms) >= 2, (
        "the two paths that raise the HUD (Quick Controls and the level-change poll)"
        " both disappeared from the source; re-audit rather than relax this gate"
    )
    for arm in arms:
        if "saturating_add" in arm:
            assert BUDGET in arm, f"a HUD dwell was armed from a literal: {arm.strip()}"


# ---------------------------------------------------------------------------
# 2. The retirement runs somewhere the shell is certain to wake.
# ---------------------------------------------------------------------------


def test_the_expiry_runs_before_the_timer_branch_does():
    """The check must sit in the per-wake section, not inside the tick branch.

    The loop's poll timeout is 25 ms, so a deadline compared on every wake is
    honoured within 25 ms of passing. The same position is where the lock read
    lives, for the same measured reason: the idle shell never sees a tick.
    """
    loop = loop_source()
    expiry = loop.index("state.volume_hud_until = 0;")
    timer = loop.index("if (events & std::EVENT_TIMER_TICK) != 0 {")
    assert expiry < timer, (
        "the HUD expiry moved back inside the timer branch; an idle shell never"
        " enters it, so the surface would stay raised past its own budget"
    )


def test_the_timer_branch_does_not_retire_the_hud():
    """A second retirement inside the branch would go unnoticed once it stopped running."""
    assert "volume_hud_until = 0" not in tick_branch(loop_source()), (
        "the tick branch carries a HUD retirement again"
    )


def test_the_expiry_lowers_the_surface_it_raised():
    """The guard and the hide are one pair: an expiry that only clears a flag leaves pixels up."""
    loop = loop_source()
    expiry = loop.index("state.volume_hud_until = 0;")
    guard = loop[: expiry].rindex("if state.volume_hud_until != 0")
    window = loop[guard : expiry + 400]
    assert "current_ticks()" in loop[guard:expiry], (
        "the expiry must compare against the same clock the arms wrote"
    )
    assert re.search(
        r"window_set_geometry\(\s*id,\s*100,\s*220,\s*100,\s*false\s*\)", window
    ), "the expiry must lower the HUD window it armed, with the same geometry"
    assert "surface_flip_display()" in window, (
        "hiding a window with no present leaves the frame on screen"
    )
