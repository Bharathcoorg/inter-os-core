# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Truth contract for the format table in docs/RAPP_FORMAT.md.

WHAT THIS SUITE ACTUALLY ENFORCES, because the sentence above is the kind of claim
this file exists to keep honest:

- the rows that name a refused audio or video type are present and still say
  **Refused** (and the gate fails if the rows are deleted, rather than agreeing
  that a doc naming nothing is an honest doc);
- every decoder entry point the supported image rows depend on exists in
  `sdk/rust/interos-std/src/image.rs`;
- the `image/webp` row states the execution the lossy parity was measured under,
  in the four specifics that a run can check, plus a fixture count that has to
  equal what the fixture directory holds;
- the fixture suites it cites exist and, when this suite runs them with cargo,
  pass.

It does NOT prove pixel parity on hardware, does not re-derive the libwebp
reference dumps (no libwebp is installed on this host), and does not certify any
row it does not name. Runs of the VP8 suite under the default parallel harness are
expected to fail nondeterministically -- that is a tracked defect, kept out of the
gate so the gate cannot be satisfied by nobody running anything.
"""

from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]

VP8_FIXTURE_DIR = ROOT / "sdk" / "rust" / "interos-std" / "tests" / "fixtures" / "vp8"

# The four things the `image/webp` row has to say about the lossy path, each one a
# fact this host measured rather than a tone: parity ran against the committed
# `.rgba` dumps, it ran with `--test-threads=1`, the parallel harness is
# nondeterministic, and libwebp is not installed here so nothing was re-decoded.
WEBP_ROW_MUST_SAY = (
    ".rgba",
    "--test-threads=1",
    "nondeterministic",
    "not a live re-decode",
)

# What the row must not say: the wording that used to sit here, which described a
# measured failure as an absence of verification.
WEBP_ROW_MUST_NOT_SAY = ("unverified in parallel",)

REFUSED_TYPES = ("`audio/ogg`", "`video/mp4`")


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def doc_row(doc: str, type_token: str) -> str | None:
    """The single table row naming `type_token`, or None when the doc dropped it."""
    hits = [line for line in doc.splitlines() if type_token in line]
    return hits[0] if len(hits) == 1 else None


def _format_doc_defects(doc: str) -> list:
    """Every way the format table over-claims, named rather than raised."""
    defects = []

    for token in REFUSED_TYPES:
        rows = [line for line in doc.splitlines() if token in line]
        if not rows:
            defects.append(
                f"{token} is no longer named anywhere in the table, so the rule that "
                f"it must be refused has nothing left to check"
            )
        for line in rows:
            if "**Refused**" not in line:
                defects.append(f"{token} row does not say Refused: {line}")
            if "**Supported**" in line:
                defects.append(f"{token} row claims Supported for a format with no decoder: {line}")

    row = doc_row(doc, "`image/webp`")
    if row is None:
        defects.append("the `image/webp` row is gone or duplicated, so its claim is unchecked")
    else:
        for phrase in WEBP_ROW_MUST_SAY:
            if phrase not in row:
                defects.append(
                    f"the `image/webp` row does not say `{phrase}`, which is what the "
                    f"lossy parity run actually established"
                )
        for phrase in WEBP_ROW_MUST_NOT_SAY:
            if phrase in row:
                defects.append(
                    f"the `image/webp` row still says `{phrase}`: the parallel run was "
                    f"executed and failed nondeterministically, which is worse than unverified"
                )
        stated = re.findall(r"(\d+) fixtures", row)
        held = len(sorted(VP8_FIXTURE_DIR.glob("*.webp"))) if VP8_FIXTURE_DIR.is_dir() else 0
        if not stated:
            defects.append("the `image/webp` row cites no fixture count to check itself against")
        elif len(stated) != 1:
            defects.append(f"the `image/webp` row states {len(stated)} fixture counts, not one")
        elif int(stated[0]) != held:
            defects.append(
                f"the `image/webp` row says {stated[0]} fixtures, the fixture directory "
                f"holds {held}"
            )

    return defects


def test_rapp_format_table_honesty():
    doc = read("docs/RAPP_FORMAT.md")

    assert _format_doc_defects(doc) == []

    # Each supported format must have an end-to-end decoder and header decoder in image.rs
    image_src = read("sdk/rust/interos-std/src/image.rs")
    for decoder in [
        "decode_png_header",
        "decode_jpeg_header",
        "decode_gif_header",
        "decode_webp_header",
        "decode_bmp_header",
        "decode_qoi_header",
        "decode_svg_header",
        "decode_image",
    ]:
        assert f"pub fn {decoder}" in image_src, f"Missing public decoder function {decoder} in image.rs"

    # Fixture test suites must exist for WebP and SVG
    assert (ROOT / "sdk/rust/interos-std/tests/vp8_fixtures.rs").exists()
    assert (ROOT / "sdk/rust/interos-std/tests/svg_fixtures.rs").exists()


def test_the_format_doc_contract_can_fail():
    """Each rule above fires when its own claim is weakened.

    WHY THIS CASE EXISTS: a gate over a doc is worth nothing until it has been
    shown to move when the doc moves. Each fixture below is one edit to one row,
    and each names the defect it must produce -- including the two edits that used
    to pass silently: deleting the refusal rows, and describing a measured failure
    as a missing verification.
    """
    doc = read("docs/RAPP_FORMAT.md")
    original = _format_doc_defects(doc)
    assert original == [], f"cannot prove the rules against a doc they already fail: {original}"

    fixtures = [
        (
            "both refusal rows deleted",
            lambda d: "\n".join(
                line
                for line in d.splitlines()
                if "`audio/ogg`" not in line and "`video/mp4`" not in line
            ),
            "no longer named anywhere in the table",
        ),
        (
            "the Ogg row re-labelled Supported",
            lambda d: d.replace(
                "| `audio/ogg`, `audio/aac`, `audio/mp4` | **Refused** |",
                "| `audio/ogg`, `audio/aac`, `audio/mp4` | **Supported** |",
            ),
            "claims Supported for a format with no decoder",
        ),
        (
            "the WebP row back to describing a failure as an absence",
            lambda d: d.replace(
                "the parallel harness fails nondeterministically on `grad_q100`",
                "pixel parity is unverified in parallel execution",
            ),
            "which is worse than unverified",
        ),
        (
            "the WebP row losing the serialized qualifier",
            lambda d: d.replace(" (`--test-threads=1`)", ""),
            "does not say `--test-threads=1`",
        ),
        (
            "the WebP row claiming a live re-decode it cannot do",
            lambda d: d.replace(
                ", so the comparison is against the committed bytes, not a live re-decode.",
                ", so the comparison is re-derived from libwebp at test time.",
            ),
            "not a live re-decode",
        ),
        (
            "a fixture count that is not the directory's",
            lambda d: d.replace("5 fixtures in tree", "9 fixtures in tree"),
            "the fixture directory holds",
        ),
        (
            "the WebP row deleted whole",
            lambda d: "\n".join(line for line in d.splitlines() if "`image/webp`" not in line),
            "the `image/webp` row is gone or duplicated",
        ),
    ]

    for name, mutate, expected in fixtures:
        mutated = mutate(doc)
        assert mutated != doc, f"fixture {name!r} no longer matches the doc it mutates"
        defects = _format_doc_defects(mutated)
        assert defects, f"fixture {name!r} produced no defect"
        assert any(expected in d for d in defects), (
            f"fixture {name!r} fired, but not the rule it was written for. "
            f"Looked for {expected!r} in:\n" + "\n".join(defects)
        )


def test_image_fixture_suites_execute_and_pass():
    """Real evidence execution: runs the cargo fixture suites cited in RAPP_FORMAT.md."""
    # 1. Bounded SVG subset renderer tests must pass
    r_svg = subprocess.run(
        ["cargo", "test", "--test", "svg_fixtures", "test_", "--manifest-path", "sdk/rust/interos-std/Cargo.toml"],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert r_svg.returncode == 0, f"svg_fixtures failed:\n{r_svg.stdout}\n{r_svg.stderr}"

    # 2. Lossless WebP fixtures must pass
    r_webp = subprocess.run(
        ["cargo", "test", "--test", "webp_fixtures", "--manifest-path", "sdk/rust/interos-std/Cargo.toml"],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert r_webp.returncode == 0, f"webp_fixtures failed:\n{r_webp.stdout}\n{r_webp.stderr}"

    # 3. GIF fixtures must pass
    r_gif = subprocess.run(
        ["cargo", "test", "--test", "gif_fixtures", "--manifest-path", "sdk/rust/interos-std/Cargo.toml"],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert r_gif.returncode == 0, f"gif_fixtures failed:\n{r_gif.stdout}\n{r_gif.stderr}"

    # 4. Dispatch decoders in image.rs must pass
    r_img = subprocess.run(
        ["cargo", "test", "--manifest-path", "sdk/rust/interos-std/Cargo.toml", "decode_image"],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert r_img.returncode == 0, f"decode_image tests failed:\n{r_img.stdout}\n{r_img.stderr}"


def test_lossy_webp_vp8_fixture_truth():
    """The lossy WebP suite must pass when run serialized, and this case is the gate.

    WHY SERIALIZED: `VP8_SCRATCH` is one static buffer shared by every decode, so
    two fixtures decoding at once overwrite each other. That is not a property the
    platform has and is not a property this case hides -- the parallel failure was
    measured (the `grad_q100 fixture differs in 36,684 of 49,152 bytes on a
    default-harness run) and it is a tracked defect, so this case pins the harness
    it is known to pass under rather than asserting parity is unconditional.

    WHAT PASSING HERE BUYS: RFC 6386 intra-keyframe decoding agrees byte-for-byte
    with the committed `.rgba` reference dumps in `tests/fixtures/vp8/`. It does
    NOT re-run libwebp: no `cwebp`/`dwebp` is installed on this host, so the
    reference is the committed bytes and their provenance is stated in
    `vp8_fixtures.rs` itself.
    """
    r_vp8 = subprocess.run(
        ["cargo", "test", "--test", "vp8_fixtures", "--manifest-path", "sdk/rust/interos-std/Cargo.toml", "--", "--test-threads=1"],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert r_vp8.returncode == 0, (
        "lossy WebP pixel parity no longer holds under serialized execution "
        f"(vp8_fixtures.rs exit code {r_vp8.returncode}). Either the decoder or the "
        "committed .rgba reference moved; docs/RAPP_FORMAT.md's `image/webp` row "
        "claims parity here and has to be corrected in the same change:\n"
        f"{r_vp8.stdout}\n{r_vp8.stderr}"
    )


if __name__ == "__main__":
    test_rapp_format_table_honesty()
    test_the_format_doc_contract_can_fail()
    test_image_fixture_suites_execute_and_pass()
    test_lossy_webp_vp8_fixture_truth()
    print("Image format truth contract: PASS")
