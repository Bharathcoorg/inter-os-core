"""Proves the framebuffer check actually looks at pixels.

WHY this exists. `tests/verify_x86_boot.py` claimed to verify an "authentic
framebuffer screendump" while checking only that the file was fresh and larger
than a megabyte. A black screen, a single-colour fill and a buffer of
uninitialised memory all satisfy that, so the display path could regress with
every test in the repository still green. These tests pin the replacement so it
cannot quietly go back to checking the file size.

No emulator is needed: the verifier is a pure function over a PPM file, and each
case writes the frame it needs.
"""

import os
import random
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))

from verify_x86_boot import (  # noqa: E402
    EXPECTED_SCANOUT,
    MAX_DISTINCT_COLOURS,
    MIN_DISTINCT_COLOURS,
    MIN_NON_DOMINANT_FRACTION,
    inspect_framebuffer,
    read_ppm,
)

W, H = EXPECTED_SCANOUT
BG = (5, 8, 18)


def write_ppm(directory, name, pixels, width=W, height=H, maxval=255, magic=b"P6"):
    path = os.path.join(directory, name)
    with open(path, "wb") as fh:
        fh.write(magic + b"\n%d %d\n%d\n" % (width, height, maxval))
        fh.write(pixels)
    return path


def rendered_frame(tmpdir):
    """A frame shaped like the real one: dominant background, a few UI accents.

    Built at the same proportions the measured launcher frame had, so the test
    exercises the same region of the checker rather than an easy case. Rows are
    assembled by slice-multiplying whole runs rather than per-pixel in Python:
    a per-pixel loop over 2.3 M pixels added over a minute to the contract
    suite, which every run pays.
    """
    accents = [(100, 116, 139), (2, 132, 199), (56, 189, 248), (30, 41, 59), (20, 27, 43)]
    bg_row = bytes(BG) * W                                  # one row, 3*W bytes
    accent_rows = [bytes(c) * W for c in accents]
    pixels = bytearray()
    for y in range(H):
        if (y * 7) % 211 < 9:                               # ~4% of rows carry an accent
            pixels += accent_rows[y % len(accent_rows)]     # a row, NOT row*W
        else:
            pixels += bg_row
    return write_ppm(tmpdir, "rendered.ppm", bytes(pixels))


# --------------------------------------------------------------------------
# A real frame is accepted.
# --------------------------------------------------------------------------

def test_a_rendered_launcher_frame_is_accepted(tmp_path):
    ok, stats, problems = inspect_framebuffer(rendered_frame(str(tmp_path)))
    assert ok, problems
    assert stats["width"] == W and stats["height"] == H
    assert stats["black_fraction"] == 0.0


def test_the_real_screendump_from_a_boot_is_accepted():
    """Uses the artefact the last boot verification produced, when present."""
    real = ROOT / "target" / "mobile_screen2.ppm"
    if not real.exists():
        import pytest
        pytest.skip("no screendump on disk; run tests/verify_x86_boot.py first")
    ok, stats, problems = inspect_framebuffer(str(real))
    assert ok, problems
    assert stats["distinct_colours"] >= MIN_DISTINCT_COLOURS


# --------------------------------------------------------------------------
# The failure modes the old size-and-mtime check could not see.
# --------------------------------------------------------------------------

def test_a_blank_single_colour_fill_is_rejected(tmp_path):
    pixels = bytes(BG) * (W * H)
    ok, _stats, problems = inspect_framebuffer(write_ppm(str(tmp_path), "blank.ppm", pixels))
    assert not ok, "a one-colour screen must not pass as a rendered frame"
    assert any("distinct colours" in p for p in problems)


def test_an_all_black_screen_is_rejected(tmp_path):
    pixels = bytes((0, 0, 0)) * (W * H)
    ok, _stats, problems = inspect_framebuffer(write_ppm(str(tmp_path), "black.ppm", pixels))
    assert not ok
    assert problems, "a black screen must produce at least one stated reason"


def test_uninitialised_memory_is_rejected(tmp_path):
    rng = random.Random(7)
    pixels = rng.randbytes(W * H * 3)
    ok, _stats, problems = inspect_framebuffer(write_ppm(str(tmp_path), "noise.ppm", pixels))
    assert not ok, "noise must not be mistaken for a rendered frame"
    assert any("uninitialised memory" in p for p in problems)


def test_a_frame_with_nearly_one_colour_is_rejected(tmp_path):
    """A fill with a single stray pixel is as blank as a pure fill."""
    pixels = bytearray(bytes(BG) * (W * H))
    pixels[0:3] = bytes((255, 255, 255))
    ok, stats, _problems = inspect_framebuffer(write_ppm(str(tmp_path), "speck.ppm", bytes(pixels)))
    assert not ok
    assert stats["non_dominant_fraction"] < MIN_NON_DOMINANT_FRACTION


def test_the_wrong_geometry_is_rejected(tmp_path):
    pixels = bytes(BG) * (100 * 100)
    ok, _stats, problems = inspect_framebuffer(
        write_ppm(str(tmp_path), "small.ppm", pixels, 100, 100))
    assert not ok
    assert any("geometry" in p for p in problems)


def test_a_truncated_file_is_rejected_not_read_as_shorter(tmp_path):
    pixels = bytes(BG) * (W * H // 2)
    ok, _stats, problems = inspect_framebuffer(write_ppm(str(tmp_path), "trunc.ppm", pixels))
    assert not ok
    assert any("truncated" in p for p in problems)


def test_a_missing_file_is_reported_rather_than_raising(tmp_path):
    ok, _stats, problems = inspect_framebuffer(str(tmp_path / "absent.ppm"))
    assert not ok
    assert any("could not parse" in p for p in problems)


def test_a_non_ppm_file_is_rejected(tmp_path):
    path = tmp_path / "not.ppm"
    path.write_bytes(b"this is not a PPM at all, not even close")
    ok, _stats, problems = inspect_framebuffer(str(path))
    assert not ok
    assert any("could not parse" in p for p in problems)


# --------------------------------------------------------------------------
# Parser behaviour.
# --------------------------------------------------------------------------

def test_ppm_header_with_a_comment_is_parsed(tmp_path):
    pixels = bytes(BG) * (W * H)
    path = tmp_path / "comment.ppm"
    path.write_bytes(b"P6\n# written by the verifier\n%d %d\n255\n" % (W, H) + pixels)
    width, height, body = read_ppm(str(path))
    assert (width, height) == (W, H)
    assert body[:3] == bytes(BG)


def test_a_wrong_maxval_is_rejected(tmp_path):
    path = tmp_path / "maxval.ppm"
    path.write_bytes(b"P6\n%d %d\n65535\n" % (W, H) + bytes(64))
    ok, _stats, problems = inspect_framebuffer(str(path))
    assert not ok
    assert any("maxval" in p for p in problems)


def test_thresholds_leave_margin_on_both_sides_of_a_real_frame():
    """The thresholds must describe a rendered screen, not today's artwork."""
    real = ROOT / "target" / "mobile_screen2.ppm"
    if not real.exists():
        import pytest
        pytest.skip("no screendump on disk")
    _ok, stats, _p = inspect_framebuffer(str(real))
    assert stats["non_dominant_fraction"] >= MIN_NON_DOMINANT_FRACTION * 2, \
        "too little headroom above the blank threshold to survive artwork changes"
    assert MIN_DISTINCT_COLOURS <= stats["distinct_colours"] < MAX_DISTINCT_COLOURS


def test_the_boot_script_no_longer_claims_success_from_size_alone():
    """The old check printed OK on file size; that path must not come back."""
    source = (ROOT / "tests" / "verify_x86_boot.py").read_text(encoding="utf-8")
    assert "inspect_framebuffer" in source
    assert "Framebuffer PPM verified for current run" not in source, \
        "the size-only success message must be gone"