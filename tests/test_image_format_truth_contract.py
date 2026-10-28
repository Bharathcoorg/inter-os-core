"""Truth contract enforcing that docs/RAPP_FORMAT.md accurately reflects real tests."""

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def test_rapp_format_table_honesty():
    doc = read("docs/RAPP_FORMAT.md")
    browser_images = read("apps/browser/src/images.rs")

    # 1. Unimplemented audio/video formats must be marked Refused, never Supported
    for line in doc.splitlines():
        if "`audio/ogg`" in line or "`video/mp4`" in line:
            assert "**Refused**" in line, f"Unimplemented format must be Refused: {line}"
            assert "**Supported**" not in line, f"Unimplemented format must not be Supported: {line}"

    # 2. WebP entry in RAPP_FORMAT.md must name vp8_fixtures.rs
    assert "vp8_fixtures.rs" in doc, "RAPP_FORMAT.md must cite vp8_fixtures.rs for lossy WebP evidence"

    # 3. Browser images.rs must not claim lossy WebP is an unimplemented variant
    assert "lossy WebP" not in browser_images, "apps/browser/src/images.rs must not list lossy WebP as unimplemented variant"

    # 4. Each supported format must have an end-to-end decoder and header decoder in image.rs
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

    # 5. Fixture test suites must exist for WebP and SVG
    assert (ROOT / "sdk/rust/interos-std/tests/vp8_fixtures.rs").exists()
    assert (ROOT / "sdk/rust/interos-std/tests/svg_fixtures.rs").exists()



if __name__ == "__main__":
    test_rapp_format_table_honesty()
    print("Image format truth contract: PASS")
