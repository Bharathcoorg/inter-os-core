# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Truth contract enforcing that docs/RAPP_FORMAT.md accurately reflects real tests."""

from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def test_rapp_format_table_honesty():
    doc = read("docs/RAPP_FORMAT.md")

    # 1. Unimplemented audio/video formats must be marked Refused, never Supported
    for line in doc.splitlines():
        if "`audio/ogg`" in line or "`video/mp4`" in line:
            assert "**Refused**" in line, f"Unimplemented format must be Refused: {line}"
            assert "**Supported**" not in line, f"Unimplemented format must not be Supported: {line}"

    # 2. Each supported format must have an end-to-end decoder and header decoder in image.rs
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

    # 3. Fixture test suites must exist for WebP and SVG
    assert (ROOT / "sdk/rust/interos-std/tests/vp8_fixtures.rs").exists()
    assert (ROOT / "sdk/rust/interos-std/tests/svg_fixtures.rs").exists()


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
    """Lossy WebP fixture truth contract:
    Executes vp8_fixtures in cargo test serialized via `--test-threads=1` to eliminate
    the unsynchronized static decode scratch (`VP8_SCRATCH`) data race.
    Asserts RFC 6386 intra keyframe decoding and libwebp fixture pixel parity succeed
    when tests are executed deterministically.
    """
    r_vp8 = subprocess.run(
        ["cargo", "test", "--test", "vp8_fixtures", "--manifest-path", "sdk/rust/interos-std/Cargo.toml", "--", "--test-threads=1"],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert r_vp8.returncode == 0, (
        f"lossy WebP pixel parity is currently failing in cargo test (vp8_fixtures.rs exit code {r_vp8.returncode}):\n"
        f"{r_vp8.stdout}\n{r_vp8.stderr}"
    )


if __name__ == "__main__":
    test_rapp_format_table_honesty()
    test_image_fixture_suites_execute_and_pass()
    test_lossy_webp_vp8_fixture_truth()
    print("Image format truth contract: PASS")
