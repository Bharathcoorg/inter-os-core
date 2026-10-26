#!/usr/bin/env python3
"""
Generate the scalable outline table the guest rasterises, from Roboto Condensed.

WHY outlines and not the font file: the guest must not carry a TrueType parser.
A `no_std` bare-metal shell gains nothing from `cmap`/`loca`/`glyf`/`hmtx`
lookup logic and pays for it in image size and in per-glyph work, while the only
thing the renderer actually needs is a point list. So the outline is extracted
here, at build time, into a flat table the guest walks with a cursor.

WHY this and not simply a bigger bitmap face: the bitmap face is a 16x24 cell
magnified by an integer scale, which is what makes large text soft (a magnified
anti-aliased edge is a visible ramp rather than an edge). Rasterising outlines
at the requested pixel size makes the edge land where the outline says it does,
at any size -- which is what a higher-resolution scanout will need.

Output is byte-for-byte reproducible: the same input file and the same glyph set
produce the same table, and `--check` regenerates into a scratch copy and diffs.

The letterforms come from Roboto Condensed (Apache License 2.0,
`tools/fonts/Apache-2.0.txt`), the same face `tools/gen_font.py` rasterises.
"""

import argparse
import os
import struct
import sys

from fontTools.ttLib import TTFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TTF = os.path.join(ROOT, "tools", "fonts", "RobotoCondensed-Regular.ttf")
OUT_PATH = os.path.join(
    ROOT, "sdk", "rust", "interos-std", "src", "design", "face.rs"
)

FIRST_CHAR = 32
LAST_CHAR = 126

NL = "\n"


def fmt_u8_array(name: str, values) -> str:
    body = []
    for i in range(0, len(values), 16):
        body.append("    " + ", ".join(str(v) for v in values[i:i + 16]) + ",")
    return (
        f"pub const {name}: [u8; {len(values)}] = [\n" + NL.join(body) + NL + "];\n"
    )


def fmt_u16_array(name: str, values) -> str:
    body = []
    for i in range(0, len(values), 12):
        body.append("    " + ", ".join(str(v) for v in values[i:i + 12]) + ",")
    return (
        f"pub const {name}: [u16; {len(values)}] = [\n" + NL.join(body) + NL + "];\n"
    )


def build():
    if not os.path.exists(TTF):
        sys.exit(f"missing source font: {TTF}")

    font = TTFont(TTF, lazy=False)
    upem = font["head"].unitsPerEm
    hhea = font["hhea"]
    hmtx = font["hmtx"]
    cmap = font.getBestCmap()
    glyf = font["glyf"]

    advances = []
    blob = bytearray()
    # One offset per character slot, LAST_CHAR + 2 entries so the rasterizer can
    # read `offsets[c + 1]` for the end of glyph `c` without a bounds check of
    # its own on a 95-entry table.
    offsets = []
    ascent = hhea.ascent
    descent = hhea.descent

    for cp in range(FIRST_CHAR, LAST_CHAR + 1):
        offsets.append(len(blob))
        name = cmap.get(cp)
        if name is None:
            advances.append(0)
            continue
        advance = hmtx[name][0]
        advances.append(advance)

        glyph = glyf[name]
        if glyph.numberOfContours == 0:
            # Space and friends: an advance with no outline. Recording it as a
            # zero-contour glyph keeps every character's slot the same shape,
            # so the rasterizer never has to special-case an empty entry.
            blob += struct.pack(">B", 0)
            continue

        coords, end_pts, flags = glyph.getCoordinates(glyf)
        n_contours = len(end_pts)

        start = 0
        contours = []
        for ci, end in enumerate(end_pts):
            pts = []
            for pi in range(start, end + 1):
                x, y = coords[pi]
                on_curve = 1 if (flags[pi] & 0x01) else 0
                # Points are stored in font units. TrueType y grows up; the
                # rasterizer flips to screen space, so the sign is preserved
                # here rather than baked in, which keeps this table a faithful
                # copy of the outline instead of a transformed one.
                pts.append((on_curve, int(x), int(y)))
            contours.append(pts)
            start = end + 1

        if n_contours > 255 or any(len(c) > 255 for c in contours):
            sys.exit(f"U+{cp:04X}: outline too large for the compact encoding")

        blob += struct.pack(">B", n_contours)
        for pts in contours:
            blob += struct.pack(">B", len(pts))
            for on_curve, x, y in pts:
                blob += struct.pack(">Bhh", on_curve, x, y)

    offsets.append(len(blob))

    # Keep the ascent/descent the guest positions type against consistent with
    # the outline's own units, and record the bbox the face actually occupies so
    # a caller can lay out a line without guessing.
    return {
        "upem": upem,
        "ascent": ascent,
        "descent": descent,
        "advances": advances,
        "offsets": offsets,
        "blob": bytes(blob),
        "first": FIRST_CHAR,
        "last": LAST_CHAR,
    }


def render(data) -> str:
    blob = data["blob"]
    out = []
    out.append(
        "//! Scalable outline table for the guest type rasteriser.\n"
        "//!\n"
        "//! GENERATED FILE - do not edit by hand. Produced by\n"
        "//! `tools/gen_typeface.py` from `tools/fonts/RobotoCondensed-Regular.ttf`\n"
        "//! (Apache License 2.0, see `tools/fonts/Apache-2.0.txt`). Regenerate with\n"
        "//! `python tools/gen_typeface.py`.\n"
        "//!\n"
        "//! WHY this is an outline table rather than the font file: the guest is\n"
        "//! `no_std` bare metal, so it pays for every byte of this table and for\n"
        "//! every instruction it runs per glyph. Resolving `cmap`/`loca`/`glyf`/\n"
        "//! `hmtx` at runtime buys nothing here -- the renderer only ever wanted a\n"
        "//! point list -- and the bitmap face this replaces is what makes large\n"
        "//! text soft, because it magnifies an anti-aliased edge instead of\n"
        "//! placing one.\n"
        "//!\n"
        "//! Layout, walked by `design::typeface`:\n"
        "//! - `OUTLINE_OFFSET[c]` is where character `c`'s outline begins and\n"
        "//!   `OUTLINE_OFFSET[c + 1]` where it ends.\n"
        "//! - An outline is `contour_count: u8`, then per contour\n"
        "//!   `point_count: u8` followed by `point_count` points of\n"
        "//!   `on_curve: u8, x: i16, y: i16`, big-endian.\n"
        "//! - A glyph with no outline is a single zero contour byte, so an empty\n"
        "//!   slot and a space are the same shape to the reader.\n"
        "//!\n"
        "// >>> generated block begins below this line\n"
    )
    out.append(f"pub const UNITS_PER_EM: u16 = {data['upem']};")
    out.append(f"pub const ASCENT: i16 = {data['ascent']};")
    out.append(f"pub const DESCENT: i16 = {data['descent']};")
    out.append(f"pub const FIRST_CHAR: u8 = {data['first']};")
    out.append(f"pub const LAST_CHAR: u8 = {data['last']};")
    out.append(NL)
    out.append("/// Horizontal advance per character slot, in font units.")
    out.append(fmt_u16_array("ADVANCE", data["advances"]))
    out.append(NL)
    out.append("/// Byte offset of each character's outline, with a trailing end offset.")
    out.append(fmt_u16_array("OUTLINE_OFFSET", data["offsets"]))
    out.append(NL)
    out.append("/// Every glyph outline, concatenated in character order.")
    out.append(fmt_u8_array("OUTLINE_BYTES", list(blob)))
    out.append("// <<< generated block ends above this line\n")
    return NL.join(out)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--check",
        action="store_true",
        help="regenerate into a scratch copy and diff against the shipped file",
    )
    args = parser.parse_args()

    data = build()
    text = render(data)

    if args.check:
        if not os.path.exists(OUT_PATH):
            sys.exit(f"{OUT_PATH} does not exist")
        with open(OUT_PATH, encoding="utf-8") as handle:
            if handle.read() != text:
                sys.exit(
                    f"{OUT_PATH} differs from the generated table; run "
                    f"`python tools/gen_typeface.py`"
                )
        print(f"{OUT_PATH} is current")
        return

    os.makedirs(os.path.dirname(OUT_PATH), exist_ok=True)
    with open(OUT_PATH, "w", encoding="utf-8", newline=NL) as handle:
        handle.write(text)
    print(f"wrote {OUT_PATH}")
    print(f"  glyphs: {LAST_CHAR - FIRST_CHAR + 1}")
    print(f"  outline bytes: {len(data['blob'])}")
    print(f"  units per em: {data['upem']}")


if __name__ == "__main__":
    main()