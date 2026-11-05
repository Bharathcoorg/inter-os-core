# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""WHAT THIS FILE IS FOR: it freezes which tracked files may hold non-source bytes.

WHY: a tree that is about to be read in public has to answer one question about
every file in it -- is this a source of truth, or is it something a build would
produce? A committed `apps/calculator/bin/calculator.elf` answers that question
wrong twice over. Anyone can rebuild it, so the copy in the index is not the
truth; and a reader who cannot tell the two apart trusts whichever they open
first, then repeats a figure that came out of a build from three commits ago.
The same argument in reverse covers the fixtures this file pins: an `.mp3` a
decoder test reads is not reproducible from anything in the tree, so it *is* a
source, and it belongs in the index. The distinction is the whole rule, and it
is worth a gate rather than a habit.

WHY a frozen set instead of a pattern: a pattern says "no `.elf`", and the next
build output arrives as `.img`, then as a `.bin` with a different name, and the
gate has to be edited to notice. A frozen set says "these, and only these" -- so
anything new trips it, whatever it is called, and the trip is the moment a human
gets asked whether a blob of bytes really belongs in the repository.

WHY the gate also reads the first bytes and not only the name: a linker's
output renamed to `.dat` still begins with the four bytes `\x7fELF`, and a file
whose name tells no story is exactly the kind of file that gets committed by
accident. The name rule and the shape rule are checked separately, because they
fail for different reasons and the reader needs the right one.

WHY every case fails closed: `tests/test_licence_contracts.py` already refuses
to call a scan green when it looked at nothing, and the same trap sits here --
an unreadable index, a missing `git`, or an empty listing would all leave a
pattern-free gate reporting that no artefacts were found. A gate that passes
because it saw nothing is not a gate.
"""

import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# Byte-for-byte what a linked object or a packaged app begins with. Names are a
# convention a careless commit can break; a magic number is not a convention.
ELF_MAGIC = b"\x7fELF"

# Extensions that belong to a toolchain's output and to nothing else. Held to the
# classes this repository actually builds: the linker's object and archive forms,
# the guest's packaged app, and the disk image the emulator boots. A fixture whose
# bytes happen to begin with an ELF header is caught by the magic check instead.
BUILT_SUFFIXES = (".elf", ".o", ".a", ".so", ".dll", ".exe", ".rapp", ".img", ".iso")

# WHY the fix is carried into the message: the reader who trips this gate is
# usually mid-work and does not have the build routes in memory, and "do not
# commit binaries" tells them nothing about how the file got here. `apps/*/bin/`
# is ignored already, so a file sitting in the index under that path was pushed
# in by hand, which is the fact the message has to surface.
WHAT_TO_DO_INSTEAD = (
    "Do not add it to the index. Build it where it is read from: "
    "`python tools/interos.py build <app>` writes `apps/<app>/bin/<app>.elf`, and "
    "`.gitignore` keeps that directory -- and every `*.img`, `*.rapp` and `*.bin` -- "
    "out of the tracked set. If the file is a fixture a test genuinely reads as "
    "committed bytes, add its exact path to TRACKED_BINARY_PAYLOADS in this file and "
    "state, in the same change, which test reads it and why rebuilding it is not "
    "possible here."
)

# The tracked files whose bytes are not source, pinned as of this writing. Every
# one is read back as committed bytes by something in the tree: the `sdk/` and
# `apps/video_player/` fixtures are the inputs their decoder tests hold against
# known-good output, the `emulator/` icons are referenced by `manifest.json` and
# `index.html`, and the font is the face `tools/gen_font.py` rasterises into the
# SDK. A new entry is a decision; that is the point of pinning them.
TRACKED_BINARY_PAYLOADS = frozenset(
    {
        "apps/video_player/tests/fixtures/mjpeg_10fps.avi",
        "apps/video_player/tests/fixtures/mjpeg_5fps_audio.avi",
        "emulator/apple-touch-icon.png",
        "emulator/favicon-16.png",
        "emulator/favicon-32.png",
        "emulator/favicon-64.png",
        "emulator/favicon.ico",
        "emulator/icon-192.png",
        "emulator/icon-512.png",
        "sdk/rust/interos-std/tests/fixtures/flac/mono_44100_s16.flac",
        "sdk/rust/interos-std/tests/fixtures/flac/mono_fullscale_44100_s16.flac",
        "sdk/rust/interos-std/tests/fixtures/flac/noise_44100_s16.flac",
        "sdk/rust/interos-std/tests/fixtures/flac/silence_44100_s16.flac",
        "sdk/rust/interos-std/tests/fixtures/flac/stereo_44100_s16.flac",
        "sdk/rust/interos-std/tests/fixtures/flac/stereo_44100_s24.flac",
        "sdk/rust/interos-std/tests/fixtures/flac/stereo_48000_s16.flac",
        "sdk/rust/interos-std/tests/fixtures/flac/stereo_ms_44100_s16.flac",
        "sdk/rust/interos-std/tests/fixtures/flac/stereo_rs_44100_s16.flac",
        "sdk/rust/interos-std/tests/fixtures/gif/basic.gif",
        "sdk/rust/interos-std/tests/fixtures/gif/basic.rgba",
        "sdk/rust/interos-std/tests/fixtures/gif/transparent.gif",
        "sdk/rust/interos-std/tests/fixtures/gif/transparent.rgba",
        "sdk/rust/interos-std/tests/fixtures/mp3/id3_44100_stereo_128k.mp3",
        "sdk/rust/interos-std/tests/fixtures/mp3/reservoir_44100_stereo_32k.mp3",
        "sdk/rust/interos-std/tests/fixtures/mp3/tone_44100_mono_64k.mp3",
        "sdk/rust/interos-std/tests/fixtures/mp3/tone_44100_stereo_128k.mp3",
        "sdk/rust/interos-std/tests/fixtures/mp3/tone_48000_stereo_128k.mp3",
        "sdk/rust/interos-std/tests/fixtures/svg/bounds_1280.rgba",
        "sdk/rust/interos-std/tests/fixtures/svg/nested_transforms.rgba",
        "sdk/rust/interos-std/tests/fixtures/svg/opacity.rgba",
        "sdk/rust/interos-std/tests/fixtures/svg/paths.rgba",
        "sdk/rust/interos-std/tests/fixtures/svg/primitives.rgba",
        "sdk/rust/interos-std/tests/fixtures/svg/stroke_joins.rgba",
        "sdk/rust/interos-std/tests/fixtures/vp8/grad_q100.rgba",
        "sdk/rust/interos-std/tests/fixtures/vp8/grad_q100.webp",
        "sdk/rust/interos-std/tests/fixtures/vp8/grey_q75.rgba",
        "sdk/rust/interos-std/tests/fixtures/vp8/grey_q75.webp",
        "sdk/rust/interos-std/tests/fixtures/vp8/odd_q40.rgba",
        "sdk/rust/interos-std/tests/fixtures/vp8/odd_q40.webp",
        "sdk/rust/interos-std/tests/fixtures/vp8/palette_q75.rgba",
        "sdk/rust/interos-std/tests/fixtures/vp8/palette_q75.webp",
        "sdk/rust/interos-std/tests/fixtures/vp8/photo_q75.rgba",
        "sdk/rust/interos-std/tests/fixtures/vp8/photo_q75.webp",
        "sdk/rust/interos-std/tests/fixtures/wav/tone_44100_mono.wav",
        "sdk/rust/interos-std/tests/fixtures/webp/grad.rgba",
        "sdk/rust/interos-std/tests/fixtures/webp/grad.webp",
        "sdk/rust/interos-std/tests/fixtures/webp/noise.rgba",
        "sdk/rust/interos-std/tests/fixtures/webp/noise.webp",
        "sdk/rust/interos-std/tests/fixtures/webp/odd.rgba",
        "sdk/rust/interos-std/tests/fixtures/webp/odd.webp",
        "sdk/rust/interos-std/tests/fixtures/webp/one.rgba",
        "sdk/rust/interos-std/tests/fixtures/webp/one.webp",
        "sdk/rust/interos-std/tests/fixtures/webp/pal.rgba",
        "sdk/rust/interos-std/tests/fixtures/webp/pal.webp",
        "sdk/rust/interos-std/tests/fixtures/webp/two.rgba",
        "sdk/rust/interos-std/tests/fixtures/webp/two.webp",
        "sdk/rust/interos-std/tests/fixtures/x509/cloudflare_ecc_ca3.der",
        "sdk/rust/interos-std/tests/fixtures/x509/isrg_root_x1.der",
        "sdk/rust/interos-std/tests/fixtures/x509/leaf_under_cloudflare_ecc.der",
        "sdk/rust/interos-std/tests/fixtures/x509/leaf_under_yr2.der",
        "sdk/rust/interos-std/tests/fixtures/x509/p256_issuer.der",
        "sdk/rust/interos-std/tests/fixtures/x509/p256_leaf.der",
        "sdk/rust/interos-std/tests/fixtures/x509/p384_issuer.der",
        "sdk/rust/interos-std/tests/fixtures/x509/p384_leaf.der",
        "sdk/rust/interos-std/tests/fixtures/x509/pss_sha256_salt32.sig",
        "sdk/rust/interos-std/tests/fixtures/x509/pss_sha256_salt_auto.sig",
        "sdk/rust/interos-std/tests/fixtures/x509/root_yr_under_isrg.der",
        "sdk/rust/interos-std/tests/fixtures/x509/sslcom_root_2022.der",
        "sdk/rust/interos-std/tests/fixtures/x509/sslcom_root_2022_under_comodo.der",
        "sdk/rust/interos-std/tests/fixtures/x509/sslcom_root_2022_wrong_key.der",
        "sdk/rust/interos-std/tests/fixtures/x509/sslcom_transit_ecc_r2.der",
        "sdk/rust/interos-std/tests/fixtures/x509/yr2_under_root_yr.der",
        "tools/fonts/RobotoCondensed-Regular.ttf",
    }
)

# WHY a timeout: reading the index and the heads of a few hundred files costs
# well under a second, so a run that hangs is a broken gate rather than a slow
# one, and the sweep has to say so and move on.
GIT_TIMEOUT_SECONDS = 120
HEAD_BYTES = 8000


def tracked_paths():
    """Every path in the index, forward-slashed, or a raised error."""
    try:
        result = subprocess.run(
            ["git", "ls-files", "-z"],
            cwd=str(ROOT),
            capture_output=True,
            timeout=GIT_TIMEOUT_SECONDS,
        )
    except FileNotFoundError:
        raise RuntimeError(
            "`git` is not on PATH, so the tracked set cannot be read. This gate "
            "reports that rather than passing an unreadable tree."
        )
    except subprocess.TimeoutExpired:
        raise RuntimeError(f"`git ls-files` did not return within {GIT_TIMEOUT_SECONDS}s.")
    if result.returncode != 0:
        raise RuntimeError(
            "`git ls-files` exited "
            f"{result.returncode}: {result.stderr.decode('utf-8', 'replace').strip()}"
        )
    listed = [raw.decode("utf-8", "replace") for raw in result.stdout.split(b"\0") if raw]
    return sorted(p.replace("\\", "/") for p in listed)


def head_bytes(path, base=ROOT):
    """The first bytes of a tracked file, or empty when it cannot be read.

    WHY empty rather than an exception: a path the index lists but the working
    tree does not hold is another agent's mid-edit state, and this file must not
    be the thing that reports it. The cases that care about content skip such a
    path, while the name-based rule still judges it -- a forced-in artefact is a
    violation before any byte of it is read.
    """
    try:
        with open(base / path, "rb") as handle:
            return handle.read(HEAD_BYTES)
    except OSError:
        return b""


def is_non_source_bytes(path, reader=head_bytes):
    """Whether a file's own bytes mark it as something other than source text."""
    head = reader(path)
    if not head:
        return False
    return b"\0" in head or head.startswith(ELF_MAGIC)


def built_output_violations(paths):
    """Paths whose name or header marks them as a toolchain's output.

    Takes any listing rather than only the live index so that the rule itself can
    be shown rejecting something: a gate nobody can demonstrate failing is an
    assertion of good intentions, not a check.
    """
    offenders = []
    for path in paths:
        named = path.lower().endswith(BUILT_SUFFIXES)
        shaped = head_bytes(path).startswith(ELF_MAGIC)
        if named or shaped:
            reason = "carries an ELF header" if shaped and not named else "is a toolchain output by name"
            offenders.append(f"{path} -- {reason}")
    return offenders


def unknown_payload_violations(paths, reader=head_bytes):
    """Tracked files holding non-source bytes that the frozen set does not name."""
    unknown = [p for p in paths if is_non_source_bytes(p, reader) and p not in TRACKED_BINARY_PAYLOADS]
    return sorted(unknown)


class TestTrackedArtefactContracts(unittest.TestCase):
    def test_the_index_can_be_judged_at_all(self):
        """A readable, non-empty index: nothing here passes by seeing no files."""
        try:
            listed = tracked_paths()
        except RuntimeError as error:
            self.fail(f"{error}")
        self.assertGreater(
            len(listed),
            0,
            "the index listed 0 files, so every rule below would report a clean tree it "
            "never looked at. Nothing in this file is green on an empty scan.",
        )
        # WHY the pinned set is also checked against reality from this side: a
        # frozen list whose entries have all vanished is a frozen list nobody
        # maintained, and the first symptom is a gate that cannot fail.
        present = set(listed)
        dangling = sorted(p for p in TRACKED_BINARY_PAYLOADS if p not in present)
        self.assertEqual(
            dangling,
            [],
            "TRACKED_BINARY_PAYLOADS names files the index no longer holds: "
            + ", ".join(dangling)
            + ". Remove them from this file in the same change that removes them "
              "from the repository, so the pin keeps describing the tree it guards.",
        )

    def test_no_tracked_file_is_a_build_output(self):
        """No linked object, package or disk image sits in the index."""
        offenders = built_output_violations(tracked_paths())
        self.assertEqual(
            offenders,
            [],
            "these tracked files are the output of a build anyone can re-run, so the "
            "copy in the index is not a source of truth:\n  "
            + "\n  ".join(offenders)
            + "\n\n"
            + WHAT_TO_DO_INSTEAD,
        )

    def test_the_tracked_non_source_set_has_not_grown(self):
        """Only the pinned fixtures hold non-source bytes."""
        offenders = unknown_payload_violations(tracked_paths())
        self.assertEqual(
            offenders,
            [],
            "these tracked files hold binary bytes and are not in the frozen set this "
            "file pins:\n  "
            + "\n  ".join(offenders)
            + "\n\n"
            + WHAT_TO_DO_INSTEAD,
        )

    def test_the_gate_rejects_an_injected_build_output(self):
        """The rules reject a forced-in `.elf` and one renamed to hide it.

        WHY this case exists: the other three judge the tree as it is, so on a
        clean day they prove only that nothing is wrong. This one judges a tree
        that is deliberately wrong, which is the only way to show the rejection
        and its advice still work when nobody has committed a binary in months.
        """
        fake_listing = sorted(
            list(TRACKED_BINARY_PAYLOADS) + ["apps/calculator/bin/calculator.elf"]
        )
        offenders = built_output_violations(fake_listing)
        self.assertEqual(
            offenders,
            ["apps/calculator/bin/calculator.elf -- is a toolchain output by name"],
            "a committed `.elf` slipped past the name rule, so the gate no longer "
            "stops the pile from growing",
        )
        # WHY the renamed file is checked separately: `.dat` answers to no
        # extension, and the header is the only rule left to catch it.
        written = {"apps/probe/payload.dat": ELF_MAGIC + b"\0" + b"\x00" * 32}
        reader = lambda path: written.get(path, b"")
        self.assertTrue(
            is_non_source_bytes("apps/probe/payload.dat", reader),
            "a linker's output renamed to `.dat` was read as source text",
        )
        self.assertEqual(
            unknown_payload_violations(["apps/probe/payload.dat"], reader),
            ["apps/probe/payload.dat"],
            "the frozen set accepted an unpinned binary, so the freeze is not a gate",
        )

    def test_the_advice_names_a_route_back_to_the_build(self):
        """A failure here tells the reader how the file should have been made.

        WHY it is worth its own case: the message is the only part of this gate a
        reader meets at the moment they need it, and a bare "binary not allowed"
        sends them looking for the build route somewhere else.
        """
        self.assertIn("tools/interos.py build", WHAT_TO_DO_INSTEAD)
        self.assertIn("apps/", WHAT_TO_DO_INSTEAD)
        self.assertTrue(
            TRACKED_BINARY_PAYLOADS,
            "the frozen set is empty, which would let any fixture through while "
            "reading as a rule that has been applied",
        )


if __name__ == "__main__":
    unittest.main()
