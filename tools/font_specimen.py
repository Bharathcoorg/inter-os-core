#!/usr/bin/env python3
"""Draws a specimen sheet for the generated face so it can actually be looked at.

WHY this exists: a generated face is wrong the first time. Numbers in a table
cannot be reviewed; a picture of `hamburgefonstiv` can. Three panels at 1x, 2x
and 3x, plus a line of real text, because a face that looks fine at 1x can fall
apart the moment it is scaled.

    python tools/font_specimen.py [out.png]

Imports the generator rather than re-implementing it, so the specimen can never
drift from the data that ships.
"""

import os
import struct
import sys
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen_font as G  # noqa: E402

INK = (17, 24, 39)
PAPER = (255, 255, 255)
RULE = (226, 232, 240)
BOLD = (2, 132, 199)


class Canvas:
    def __init__(self, w, h, bg=PAPER):
        self.w, self.h = w, h
        self.px = [[bg for _ in range(w)] for _ in range(h)]

    def dot(self, x, y, c):
        if 0 <= x < self.w and 0 <= y < self.h:
            self.px[y][x] = c

    def rect(self, x, y, w, h, c):
        for yy in range(y, y + h):
            for xx in range(x, x + w):
                self.dot(xx, yy, c)

    def png(self, path):
        raw = bytearray()
        for row in self.px:
            raw.append(0)
            for r, g, b in row:
                raw += bytes((r, g, b))
        comp = zlib.compress(bytes(raw), 9)

        def chunk(tag, data):
            c = struct.pack(">I", len(data)) + tag + data
            return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

        with open(path, "wb") as f:
            f.write(b"\x89PNG\r\n\x1a\n")
            f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", self.w, self.h, 8, 2, 0, 0, 0)))
            f.write(chunk(b"IDAT", comp))
            f.write(chunk(b"IEND", b""))


def draw_glyph(cv, bmp, x0, y0, scale, colour, bold=False):
    src = bmp.dilate() if bold else bmp
    for y in range(G.CELL_H):
        for x in range(G.CELL_W):
            if src.rows[y][x]:
                cv.rect(x0 + x * scale, y0 + y * scale, scale, scale, colour)


def advance(bmp_key, face, bold=False):
    b = face[bmp_key][1] if bold else face[bmp_key][0]
    ink = b.ink()
    return face[bmp_key][2]


def text_width(s, face, bold=False):
    return sum(face[ch][2] for ch in s)


def draw_text(cv, s, x, y, scale, face, colour, bold=False):
    pen = x
    for ch in s:
        b = face[ch][1] if bold else face[ch][0]
        draw_glyph(cv, b, pen, y, scale, colour, bold=False)
        if bold:
            draw_glyph(cv, face[ch][0], pen, y, scale, colour)
        pen += face[ch][2] * scale
    return pen


def panel(cv, face, scale, top, label):
    per_row = 16 if scale == 1 else (16 if scale <= 2 else 10)
    cell_w = (G.CELL_W + 6) * scale
    cell_h = (G.CELL_H + 6) * scale
    pad = 16
    rows = (95 + per_row - 1) // per_row
    y = top
    for r in range(rows):
        for c in range(per_row):
            i = r * per_row + c
            if i >= 95:
                break
            ch = chr(G.FIRST_CHAR + i)
            x = pad + c * cell_w
            draw_glyph(cv, face[ch][0], x, y, scale, INK)
            # faint advance box, so the metrics are visible and not just felt
            adv = face[ch][2] * scale
            cv.rect(x, y + G.CELL_H * scale, adv, 1, RULE)
        y += cell_h
    return y


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "font_specimen.png"
    face = G.render()
    width = 16 * (G.CELL_W + 6) * 3 + 80
    cv = Canvas(width, 2260)

    # 1x: the whole range, the only panel where every pixel is judged.
    panel(cv, face, 1, 24, "1x")
    # Sample copy at 1x so the rhythm of lowercase is visible, not just isolated
    # shapes.
    y = 24 + 6 * (G.CELL_H + 6) + 24
    draw_text(cv, "InterOS Settings Camera 12:34 (42%)", 16, y, 1, face, INK)

    # 2x
    y = panel(cv, face, 2, y + 60, "2x") + 30
    draw_text(cv, "Quick Settings  09:41", 16, y, 2, face, INK)
    draw_text(cv, "Bold 0123456789", 16, y + G.CELL_H * 2 + 16, 2, face, BOLD, bold=True)

    # 3x
    y = panel(cv, face, 3, y + G.CELL_H * 2 * 2 + 70, "3x") + 30
    draw_text(cv, "AaBbGgSs 24", 16, y, 3, face, INK)
    draw_text(cv, "Hamburgefonstiv", 16, y + G.CELL_H * 3 + 20, 3, face, INK)

    cv.png(out)
    print("wrote %s (%dx%d)" % (out, cv.w, cv.h))


if __name__ == "__main__":
    main()