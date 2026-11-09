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


def block_from(source: str, start: int) -> str:
    """The Rust block whose header begins at `start`, measured by brace depth.

    WHY measure the extent instead of slicing a fixed number of bytes: an earlier
    version read 400 bytes past the flag clear. The hide then grew a comment
    explaining why the beat is allowed to present without writing, the present
    call slid past the slice, and the gate failed on source that was correct --
    the same class of defect it exists to catch, only in the gate.
    """
    opening = source.index("{", start)
    depth = 0
    for offset in range(opening, len(source)):
        if source[offset] == "{":
            depth += 1
        elif source[offset] == "}":
            depth -= 1
            if depth == 0:
                return source[start : offset + 1]
    raise AssertionError("unbalanced braces in the shell's event loop")


def shell_functions(source: str) -> dict:
    """Every top-level `fn` in the shell, mapped to its body.

    WHY top-level only: the present helpers are declared at column zero and their
    bodies never put a brace there, so the closing `^}` is the whole delimiter.
    """
    return {
        name: body
        for name, body in re.findall(
            r"^(?:pub )?fn ([A-Za-z_][A-Za-z0-9_]*)\([^)]*\)[^{]*\{\n(.*?)^\}", source, re.S | re.M
        )
    }


def reaches_the_scanout(calls, source, functions=None) -> bool:
    """True when one of `calls` reaches `std::surface_flip()` through the shell's helpers.

    WHY resolve the call instead of naming one function: the expiry used to call
    `surface_flip_display()` and now calls `surface_recompose_display()`, which
    opens the present gate for a beat that changes the panel by hiding a window
    rather than by writing pixels -- an exception the shell states by name
    instead of leaving a hole in the rule. A gate pinned to either name would
    fail on a rename that keeps the present, and pass on a helper that quietly
    stopped presenting. What has to hold is that the beat reaches the syscall.
    """
    functions = shell_functions(source) if functions is None else functions
    seen = set()
    pending = list(calls)
    while pending:
        name = pending.pop()
        if name in seen:
            continue
        seen.add(name)
        body = functions.get(name)
        if body is None:
            continue
        if "std::surface_flip()" in body:
            return True
        pending.extend(re.findall(r"\b([a-z_][a-z0-9_]*)\s*\(\s*\)", body))
    return False


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
    window = block_from(loop, guard)
    assert "current_ticks()" in loop[guard:expiry], (
        "the expiry must compare against the same clock the arms wrote"
    )
    assert re.search(
        r"window_set_geometry\(\s*id,\s*100,\s*220,\s*100,\s*false\s*\)", window
    ), "the expiry must lower the HUD window it armed, with the same geometry"
    calls = set(re.findall(r"\b(surface_[a-z0-9_]+)\s*\(", window))
    assert calls, "the expiry window no longer calls a present helper at all"
    assert reaches_the_scanout(calls, shell_source()), (
        f"hiding a window with no present leaves the frame on screen: neither of "
        f"{sorted(calls)} reaches std::surface_flip()"
    )
