#!/usr/bin/env python3
"""Contracts for the compositing rule that keeps a client's own pixels on screen.

WHY this file exists: a solid black rectangle used to appear in the top-left
corner of every foreground application. The cause was not the application: the
display service creates a 320x480 surface, writes four pixels into it and
flips, so every other pixel of that surface was still the allocator's fill
(opaque black). The compositor blended that half-written surface, which put a
black rectangle underneath the foreground application, and the application's
translucent cards then blended against that black instead of against the
platform background.

The rule this file pins is the one that removes it: the compositor publishes a
surface only when the surface holds a whole frame the owner drew. A surface
that holds no frame is not a picture of anything, and the capture path already
refused to capture one.

These contracts are static on purpose: they are the gate that has to hold when
nobody has a device in front of them. The behavioural proof lives in the
kernel's own unit tests, whose names are pinned at the bottom of this file.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

KMS = ROOT / "kernel/src/drivers/display_kms.rs"
SYSCALL = ROOT / "kernel/src/syscall/mod.rs"
LAUNCHER = ROOT / "apps/launcher/src/main.rs"


def _read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def _composite_body() -> str:
    """Returns the body of `DrmKmsController::composite`."""
    text = _read(KMS)
    start = text.index("pub fn composite(&mut self)")
    # The function ends at the next `    pub fn ` or `    fn ` at the same
    # indentation, which is the controller's method boundary.
    rest = text[start:]
    match = re.search(r"\n    (pub )?fn ", rest)
    assert match, "composite() has no closing method boundary"
    return rest[: match.start()]


def _skip_condition() -> str:
    """Returns the condition that decides which surfaces reach the blitter."""
    body = _composite_body()
    match = re.search(r"if !surface\.visible.*?\{", body, re.DOTALL)
    assert match, "the compositor no longer skips a surface on visibility"
    return match.group(0)


def test_the_compositor_refuses_a_surface_that_holds_no_frame():
    """The invariant: the allocator's fill is not content, so it is not blended."""
    condition = _skip_condition()
    assert "!surface.frame_complete" in condition, (
        "the compositor would blend a surface the owner never drew whole; "
        "that is the black rectangle this contract exists to prevent"
    )
    # The guard has to be the compositor's own, not a property some caller has
    # to remember to set.
    assert "surface.frame_complete" in condition


def test_a_surface_starts_out_holding_no_frame():
    """A new surface holds the allocator's fill, so it holds no frame."""
    text = _read(KMS)
    new = text[text.index("pub fn new(id: u32") :]
    new = new[: new.index("\n    }")]
    assert "frame_complete: false" in new, (
        "a newly created surface must not claim to hold a frame"
    )
    assert "has_frame: false" in new


def test_the_write_paths_record_which_rows_the_owner_filled():
    """The compositor learns a frame is whole from the writes themselves."""
    text = _read(KMS)
    # Every path that puts client pixels into a surface ends at
    # `mark_dirty_rows`, so that is where the coverage frontier lives.
    assert "fn mark_dirty_rows" in text
    mark = text[text.index("pub fn mark_dirty_rows") :]
    mark = mark[: mark.index("\n    }")]
    assert "note_written_rows" in mark, (
        "a pixel write would no longer record that the owner drew those rows"
    )
    note = text[text.index("fn note_written_rows") :]
    note = note[: note.index("\n    }")]
    assert "written_rows" in note
    assert "self.frame_complete = true" in note, (
        "a surface the owner wrote from edge to edge must be recognised as "
        "holding a whole frame"
    )


def test_the_raw_surface_write_syscall_goes_through_the_coverage_hook():
    """The client's own write path must record coverage, or nothing does."""
    syscall = _read(SYSCALL)
    assert "surface.mark_dirty_rows(band.0, band.1)" in syscall, (
        "the surface-write syscall no longer records the rows it filled"
    )


def test_the_capture_path_and_the_compositor_agree_about_a_surface_with_no_frame():
    """One rule for both paths: a surface with no frame is not a picture."""
    syscall = _read(SYSCALL)
    capture = syscall[syscall.index("pub fn sys_surface_capture") :]
    capture = capture[: capture.index("\n}")]
    assert "!surface.has_frame" in capture, (
        "the capture path would hand out the allocator's fill as a preview"
    )
    assert "!surface.frame_complete" in _skip_condition()


def test_the_compositor_does_not_latch_a_half_written_surface():
    """The flip's commit may not make an unfinished frame publishable."""
    text = _read(KMS)
    commit = text[text.index("pub fn commit_surfaces") :]
    commit = commit[: commit.index("\n    }")]
    # The commit still latches the owner's surfaces, but the compositor's own
    # guard is what decides publication, so a latched surface that holds no
    # frame stays off the display.
    assert "s.frame_ready = true" in commit
    assert "!surface.frame_complete" in _skip_condition()


def test_the_launcher_does_not_leave_an_undrawn_surface_over_the_foreground_app():
    """The launcher's own surfaces must not become the backdrop of an app."""
    launcher = _read(LAUNCHER)
    assert "std::surface_create(" in launcher, "the launcher owns a surface"
    # The volume HUD is created at startup and only drawn when the volume
    # changes, so it must not be left visible over a foreground application
    # while it still holds the allocator's fill.
    create = launcher[launcher.index("std::surface_create(") :]
    create = create[: create.index("\n")]
    assert "520, 160" in create or "520,160" in create, (
        "the launcher's volume HUD changed shape; this contract no longer "
        "knows which surface it is talking about"
    )
    assert "std::window_set_geometry(" in launcher, (
        "the launcher must place its HUD explicitly rather than leave it at "
        "the origin on top of the foreground application"
    )


def test_the_kernel_unit_suite_pins_the_behaviour():
    """The behavioural proof is in the kernel's own tests; keep their names."""
    text = _read(KMS)
    for name in (
        "fn a_surface_that_holds_no_frame_is_never_blended()",
        "fn a_surface_the_owner_wrote_whole_is_published()",
        "fn atomic_frame_commit_latch_prevents_uncommitted_surface_composition()",
    ):
        assert name in text, f"the kernel unit suite lost {name}"
