#!/usr/bin/env python3
"""Generates apps/launcher/src/font.rs -- the launcher's 16x24 condensed bitmap face.

WHY this file exists: 95 glyphs is ~2,300 literal bytes if hand-written, and nobody
can review or maintain a hex table. Instead each glyph is described here as a few
strokes in a 16x24 grid -- stems, bars, arcs and diagonals with a stroke width --
and rasterised to bits by the code below. The design decisions (stem width,
x-height, proportions, which glyphs overshoot) are all readable in this file; the
Rust output is a build product.

HOW TO REGENERATE:
    python tools/gen_font.py            # writes apps/launcher/src/font.rs
    python tools/gen_font.py --report   # also prints the ink/advance table

The output is committed so the firmware builds without running Python. Re-run this
script whenever a glyph description below changes; the generated file carries a
header saying so, and must never be hand-edited.

DESIGN (see the constants below; all values are rows in the 16x24 cell):
    * cell          16 x 24, baseline on row 19, 4 rows of descender below it
    * cap height    17 rows (3..=19), ascender 16 (4..=19), x-height 12 (8..=19)
    * stem          2 px on stems and on horizontal bars alike
    * overshoot     1 px above cap / x-height and below the baseline for every
                    round-sided letter (C G O Q S c e o s ...) -- this single
                    detail is most of what stops a bitmap face looking amateur
    * advance       proportional, one column of sidebearing each side by
                    default; digits are tabular (identical advance, ink centred)
    * bold          the same shapes dilated 1 px horizontally and advanced 1 px,
                    which thickens every stem from 2 to 3 without touching the
                    crossbars' height
"""

import math
import os
import sys

# ---------------------------------------------------------------------------
# The design grid
# ---------------------------------------------------------------------------
CELL_W, CELL_H = 16, 24
CAP_TOP = 3        # first row of a flat-topped capital
BASE = 19          # last row above the baseline
ASC_TOP = 4        # b d f h k l t
XH_TOP = 8         # a c e m n o ...
DESC_BOT = 23      # g j p q y
STEM = 2           # stroke width

FIRST_CHAR = 32
LAST_CHAR = 126
NGLYPH = LAST_CHAR - FIRST_CHAR + 1
NL = "\n"

# Splice markers: everything between them is rewritten on every run, so the
# hand-written API and tests in font.rs live outside them.
START_MARK = '/// Glyph bitmaps, regular weight'
END_MARK = 'pub const FALLBACK_ADVANCE'


class Bmp:
    """A 16x24 1-bit canvas. Coordinates are pixels; (0,0) is the top-left."""

    def __init__(self):
        self.rows = [[0] * CELL_W for _ in range(CELL_H)]

    def px(self, x, y):
        if 0 <= x < CELL_W and 0 <= y < CELL_H:
            self.rows[y][x] = 1

    def fill(self, x0, y0, x1, y1):
        """Filled rectangle, inclusive on all four edges."""
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                self.px(x, y)

    def stem(self, x, y0, y1, t=STEM):
        """Vertical stroke: `t` columns wide starting at column `x`."""
        self.fill(x, y0, x + t - 1, y1)

    def bar(self, x0, x1, y, t=STEM):
        """Horizontal stroke: `t` rows tall starting at row `y`."""
        self.fill(x0, y, x1, y + t - 1)

    def seg(self, x0, y0, x1, y1, t=STEM):
        """Diagonal stroke, stamped as t x t blocks along the run."""
        self.brush([(x0, y0), (x1, y1)], t)

    def brush(self, pts, t=STEM):
        """Stamp t x t blocks along a polyline. Points are block top-lefts."""
        for i in range(len(pts) - 1):
            ax, ay = pts[i]
            bx, by = pts[i + 1]
            dist = math.hypot(bx - ax, by - ay)
            steps = max(1, int(dist / 0.2))
            for k in range(steps + 1):
                u = k / steps
                x = int(round(ax + (bx - ax) * u))
                y = int(round(ay + (by - ay) * u))
                for dy in range(t):
                    for dx in range(t):
                        self.px(x + dx, y + dy)

    def arc(self, cx, cy, rx, ry, a0, a1, t=STEM):
        """Elliptical arc, angles in degrees, y growing downwards.

        0 deg is the right extreme, 90 the bottom, 180 the left, 270 the top.
        A0 -> A1 sweeps in increasing angle.
        """
        sweep = a1 - a0
        steps = max(16, int(abs(sweep) / 1.0))
        pts = []
        for k in range(steps + 1):
            a = math.radians(a0 + sweep * k / steps)
            pts.append((cx + rx * math.cos(a), cy + ry * math.sin(a)))
        self.brush(pts, t)

    def ring(self, cx, cy, rx, ry, t=STEM):
        self.arc(cx, cy, rx, ry, 0, 360, t)

    def art(self, lines):
        """Stamp an ASCII-art block; '#' is ink. Used only where strokes cannot
        describe the shape honestly (& and the two braces)."""
        y0 = CAP_TOP if all(c in " ." for c in lines[0]) else ASC_TOP
        for j, line in enumerate(lines):
            for i, ch in enumerate(line):
                if ch == "#":
                    self.px(i, y0 + j)

    # -- queries ------------------------------------------------------------
    def ink(self):
        """(left, right) ink columns and (top, bottom) ink rows, or None."""
        cols = [x for y in range(CELL_H) for x in range(CELL_W) if self.rows[y][x]]
        rows = [y for y in range(CELL_H) for x in range(CELL_W) if self.rows[y][x]]
        if not cols:
            return None
        return (min(cols), max(cols), min(rows), max(rows))

    def dilate(self, n=1):
        """Horizontal dilation: every ink pixel also lights `n` columns right.

        WHY right and not both sides: a real bold face keeps its left sidebearing
        and gains the extra ink on the stem's far side, so 'o' grows from a 2 px
        stem pair to a 3 px stem pair without shifting inside its own advance.
        """
        out = [row[:] for row in self.rows]
        for y in range(CELL_H):
            row = self.rows[y]
            for x in range(CELL_W):
                if row[x]:
                    for d in range(1, n + 1):
                        if x + d < CELL_W:
                            out[y][x + d] = 1
        return Bmp.from_rows(out)

    @staticmethod
    def from_rows(rows):
        b = Bmp()
        b.rows = rows
        return b


def blank():
    return Bmp()


# ---------------------------------------------------------------------------
# Glyph descriptions. Each entry draws into a canvas whose x = 0 is the ink's
# leftmost column; the leading sidebearing is applied when the face is packed.
#
# Vertical anchors, from the design:
#   caps            rows 3..=19          (17 rows)
#   round caps      rows 2..=20          (1 px overshoot each end)
#   x-height        rows 8..=19          (12 rows)
#   round lowercase rows 7..=20          (1 px overshoot each end)
#   ascenders       row 4                (16 rows)
#   descenders      row 23
#
# A stroked ellipse of vertical radius r centred at cy inks rows cy-r .. cy+r+1,
# so cy and r are chosen together: a lining bowl is cy=10.5, r=7.5 (rows 3..19),
# a round bowl is cy=10.5, r=8.5 (rows 2..20). Diagonals end at y=18 so their
# last stamped row lands on the baseline instead of one below it.
# ---------------------------------------------------------------------------
GLYPHS = {}


def glyph(ch, fn, lsb=1, rsb=1):
    GLYPHS[ch] = (fn, lsb, rsb)


def tabular(ch, fn, advance):
    """A glyph with a fixed advance, ink centred inside it. WHY: figures that
    shift as their value changes make a clock tick sideways."""
    GLYPHS[ch] = (fn, None, advance)


# -- digits: lining, cap-height, tabular -------------------------------------
def d0(g):
    g.ring(4, 10.5, 3, 7.5)


def d1(g):
    g.stem(2, 3, 19)
    g.seg(0, 6, 2, 3)


def d2(g):
    g.arc(4, 6.5, 3, 3.5, 185, 375)
    g.seg(6, 9, 2, 17)
    g.bar(0, 7, 18)


def d3(g):
    g.arc(4, 7, 3, 4, 190, 450)
    g.arc(4, 15.5, 3.5, 4.5, 270, 540)


def d4(g):
    g.seg(6, 3, 1, 12)
    g.bar(0, 7, 12)
    g.stem(5, 3, 19)


def d5(g):
    g.bar(1, 7, 3)
    g.stem(1, 3, 10)
    g.bar(1, 5, 10)
    g.arc(4, 14.5, 3, 4.5, 270, 450)


def d6(g):
    g.stem(0, 5, 19)
    g.ring(4, 13.5, 3, 6)
    g.arc(3, 7, 3, 3, 180, 270)


def d7(g):
    g.bar(0, 7, 3)
    g.seg(6, 4, 0, 18)


def d8(g):
    g.ring(4, 6.5, 2.5, 3.5)
    g.ring(4, 15, 3, 4)


def d9(g):
    g.ring(4, 7.5, 3, 4.5)
    g.stem(7, 10, 18)
    g.arc(4, 18, 3, 1.5, 0, 90)


for _c, _f in zip("0123456789", [d0, d1, d2, d3, d4, d5, d6, d7, d8, d9]):
    tabular(_c, _f, 11)


# -- capitals ---------------------------------------------------------------
def cA(g):
    g.fill(4, 3, 6, 5)
    g.seg(4, 3, 0, 18)
    g.seg(6, 3, 10, 18)
    g.bar(2, 8, 11)


def cB(g):
    g.stem(0, 3, 19)
    g.bar(0, 6, 3)
    g.bar(0, 6, 10)
    g.bar(0, 6, 18)
    g.arc(5.5, 6.5, 2, 3.5, 270, 450)
    g.arc(5.5, 14, 2, 4, 270, 450)


def cC(g):
    g.arc(5, 10.5, 4, 8.5, 45, 315)


def cD(g):
    g.stem(0, 3, 19)
    g.bar(0, 5, 3)
    g.bar(0, 5, 18)
    g.arc(5, 10.5, 3.5, 8.5, 270, 450)


def cE(g):
    g.stem(0, 3, 19)
    g.bar(0, 8, 3)
    g.bar(0, 7, 10)
    g.bar(0, 8, 18)


def cF(g):
    g.stem(0, 3, 19)
    g.bar(0, 8, 3)
    g.bar(0, 7, 10)


def cG(g):
    g.arc(5, 10.5, 4, 8.5, 30, 315)
    g.bar(5, 9, 12)
    g.stem(8, 12, 18)


def cH(g):
    g.stem(0, 3, 19)
    g.stem(8, 3, 19)
    g.bar(0, 9, 10)


def cI(g):
    g.stem(0, 3, 19)


def cJ(g):
    g.stem(6, 3, 19)
    g.arc(3.5, 16.5, 2.5, 2, 0, 180)


def cK(g):
    g.stem(0, 3, 19)
    g.seg(9, 3, 1, 11)
    g.seg(3, 9, 9, 18)


def cL(g):
    g.stem(0, 3, 19)
    g.bar(0, 7, 18)


def cM(g):
    g.stem(0, 3, 19)
    g.stem(11, 3, 19)
    g.seg(1, 3, 5, 13)
    g.seg(12, 3, 8, 13)


def cN(g):
    g.stem(0, 3, 19)
    g.stem(9, 3, 19)
    g.seg(1, 3, 9, 18)


def cO(g):
    g.ring(5.5, 10.5, 4, 8.5)


def cP(g):
    g.stem(0, 3, 19)
    g.bar(0, 6, 3)
    g.bar(0, 6, 10)
    g.arc(5.5, 6.5, 2.5, 3.5, 270, 450)


def cQ(g):
    g.ring(5.5, 10.5, 4, 8.5)
    g.seg(6.5, 14.5, 10, 19.5)


def cR(g):
    g.stem(0, 3, 19)
    g.bar(0, 6, 3)
    g.bar(0, 6, 10)
    g.arc(5.5, 6.5, 2.5, 3.5, 270, 450)
    g.seg(4, 10, 9, 18)


def cS(g):
    g.arc(4.5, 6.5, 3, 4.5, 135, 330)
    g.arc(4.5, 14.5, 3, 4.5, 315, 495)


def cT(g):
    g.bar(0, 9, 3)
    g.stem(4, 4, 19)


def cU(g):
    g.stem(0, 3, 13)
    g.stem(8, 3, 13)
    g.arc(4.5, 13, 3.5, 5.5, 0, 180)


def cV(g):
    g.seg(0, 3, 5, 18)
    g.seg(10, 3, 5, 18)


def cW(g):
    g.seg(0, 3, 3, 18)
    g.seg(3, 18, 6, 3)
    g.seg(6, 3, 9, 18)
    g.seg(9, 18, 12, 3)


def cX(g):
    g.seg(0, 3, 9, 18)
    g.seg(9, 3, 0, 18)


def cY(g):
    g.seg(0, 3, 4, 10)
    g.seg(9, 3, 4, 10)
    g.fill(3, 9, 5, 11)
    g.stem(4, 10, 19)


def cZ(g):
    g.bar(0, 8, 3)
    g.seg(8, 4, 0, 18)
    g.bar(0, 8, 18)


glyph("A", cA, 1, 1)
glyph("B", cB, 1, 1)
glyph("C", cC, 1, 1)
glyph("D", cD, 1, 1)
glyph("E", cE, 1, 1)
glyph("F", cF, 1, 2)
glyph("G", cG, 1, 1)
glyph("H", cH, 1, 1)
glyph("I", cI, 2, 2)
glyph("J", cJ, 1, 1)
glyph("K", cK, 1, 1)
glyph("L", cL, 1, 1)
glyph("M", cM, 1, 0)
glyph("N", cN, 1, 1)
glyph("O", cO, 1, 0)
glyph("P", cP, 1, 1)
glyph("Q", cQ, 1, 0)
glyph("R", cR, 1, 1)
glyph("S", cS, 1, 1)
glyph("T", cT, 1, 1)
glyph("U", cU, 1, 1)
glyph("V", cV, 1, 1)
glyph("W", cW, 1, 1)
glyph("X", cX, 1, 1)
glyph("Y", cY, 1, 1)
glyph("Z", cZ, 1, 1)


# -- lowercase --------------------------------------------------------------
def ca(g):
    g.arc(3.5, 15, 2.5, 4, 340, 700)
    g.arc(4.5, 11, 2.5, 4, 340, 200)
    g.stem(6, 8, 19)


def cb(g):
    g.stem(0, 4, 19)
    g.arc(3.5, 13, 2.5, 6, 300, 660)


def cc(g):
    g.arc(3.5, 13, 2.5, 6, 45, 315)


def cd(g):
    g.stem(5, 4, 19)
    g.arc(3.5, 13, 2.5, 6, 240, -120)


def ce(g):
    g.bar(0, 6, 13)
    g.arc(3.5, 13, 2.5, 6, 0, 340)


def cf(g):
    g.stem(2, 5, 19)
    g.arc(4, 9, 3, 3.5, 270, 180)
    g.bar(0, 4, 9)


def cg(g):
    g.arc(3.5, 13, 2.5, 6, 340, 700)
    g.stem(6, 9, 21)
    g.arc(3.5, 20.5, 2.5, 1.5, 0, 180)


def ch(g):
    g.stem(0, 4, 19)
    g.arc(3.5, 12.5, 2.5, 4.5, 180, 360)
    g.stem(6, 12, 19)


def ci(g):
    g.fill(0, 5, 1, 6)
    g.stem(0, 8, 19)


def cj(g):
    g.fill(2, 5, 3, 6)
    g.stem(2, 8, 21)
    g.arc(1.5, 21, 1.5, 2, 0, 180)


def ck(g):
    g.stem(0, 4, 19)
    g.seg(7, 8, 1, 14)
    g.seg(3, 12, 7, 18)


def cl(g):
    g.stem(0, 4, 19)


def cm(g):
    g.stem(0, 8, 19)
    g.stem(5, 8, 19)
    g.stem(10, 8, 19)
    g.arc(2.5, 12.5, 2.5, 4.5, 180, 360)
    g.arc(7.5, 12.5, 2.5, 4.5, 180, 360)


def cn(g):
    g.stem(0, 8, 19)
    g.arc(3.5, 12.5, 2.5, 4.5, 180, 360)
    g.stem(6, 12, 19)


def co(g):
    g.ring(3.5, 13, 2.5, 6)


def cp(g):
    g.stem(0, 8, 23)
    g.ring(3.5, 13, 2.5, 6)


def cq(g):
    g.stem(5, 8, 23)
    g.ring(3.5, 13, 2.5, 6)


def cr(g):
    g.stem(0, 8, 19)
    g.arc(3, 12.5, 2.5, 4.5, 180, 330)


def cs(g):
    g.arc(3.5, 10, 2, 3, 135, 330)
    g.arc(3.5, 16, 2, 3, 315, 495)


def ct(g):
    g.stem(1, 5, 17)
    g.arc(3.5, 17, 2, 2, 180, 360)
    g.bar(0, 4, 8)


def cu(g):
    g.stem(0, 8, 15)
    g.arc(3.5, 14, 2.5, 4, 0, 180)
    g.stem(5, 8, 15)


def cv(g):
    g.seg(0, 8, 3, 18)
    g.seg(7, 8, 3, 18)


def cw(g):
    g.seg(0, 8, 2.5, 18)
    g.seg(2.5, 18, 5, 8)
    g.seg(5, 8, 7.5, 18)
    g.seg(7.5, 18, 10, 8)


def cx(g):
    g.seg(0, 8, 7, 18)
    g.seg(7, 8, 0, 18)


def cy(g):
    g.seg(0, 8, 3.5, 14)
    g.seg(7, 8, 1, 22)


def cz(g):
    g.bar(0, 6, 8)
    g.seg(6, 9, 0, 18)
    g.bar(0, 6, 18)


glyph("a", ca, 1, 1)
glyph("b", cb, 1, 1)
glyph("c", cc, 1, 1)
glyph("d", cd, 1, 1)
glyph("e", ce, 1, 1)
glyph("f", cf, 1, 2)
glyph("g", cg, 1, 1)
glyph("h", ch, 1, 1)
glyph("i", ci, 2, 2)
glyph("j", cj, 1, 1)
glyph("k", ck, 1, 1)
glyph("l", cl, 2, 2)
glyph("m", cm, 1, 0)
glyph("n", cn, 1, 1)
glyph("o", co, 1, 1)
glyph("p", cp, 1, 1)
glyph("q", cq, 1, 1)
glyph("r", cr, 1, 2)
glyph("s", cs, 1, 1)
glyph("t", ct, 1, 2)
glyph("u", cu, 1, 1)
glyph("v", cv, 1, 1)
glyph("w", cw, 1, 1)
glyph("x", cx, 1, 1)
glyph("y", cy, 1, 1)
glyph("z", cz, 1, 1)


# -- punctuation and symbols ------------------------------------------------
def p_space(g):
    pass


def p_excl(g):
    g.stem(0, 3, 14)
    g.fill(0, 18, 1, 19)


def p_quot(g):
    g.stem(0, 3, 8)
    g.stem(3, 3, 8)


def p_hash(g):
    g.stem(2, 3, 19)
    g.stem(6, 3, 19)
    g.bar(0, 8, 8)
    g.bar(0, 8, 14)


def p_dollar(g):
    g.arc(4.5, 9, 3, 4, 135, 330)
    g.arc(4.5, 15, 3, 4, 315, 495)
    g.stem(3, 2, 22)


def p_pct(g):
    g.ring(2, 5.5, 1.5, 2)
    g.ring(7, 16.5, 1.5, 2)
    g.seg(9, 3, 0, 19)


def p_amp(g):
    # '&' is two stacked bowls and a tail. WHY this and not a proper ampersand
    # skeleton: at 17 rows the diagonal joins that make a real '&' need a third
    # stroke crossing, and two bowls plus a tail read correctly at this size.
    g.arc(2.5, 7.5, 1.5, 3, 340, 200)
    g.seg(3.5, 10, 0.5, 14)
    g.arc(3, 15.5, 2.5, 3.5, 25, 195)
    g.seg(5, 12.5, 9, 19)


def p_apos(g):
    g.stem(0, 3, 8)


def p_lpar(g):
    g.arc(3.5, 11, 2.5, 8, 60, 300)


def p_rpar(g):
    g.arc(1.5, 11, 2.5, 8, 240, 480)


def p_star(g):
    g.seg(0, 7, 6, 13)
    g.seg(0, 13, 6, 7)
    g.stem(2, 5, 15)


def p_plus(g):
    g.stem(3, 7, 16)
    g.bar(0, 6, 11)


def p_comma(g):
    g.fill(1, 17, 2, 18)
    g.seg(2, 19, 0, 23)


def p_dash(g):
    g.bar(0, 4, 11)


def p_dot(g):
    g.fill(0, 18, 1, 19)


def p_slash(g):
    g.seg(0, 20, 6, 3)


def p_colon(g):
    g.fill(0, 10, 1, 11)
    g.fill(0, 18, 1, 19)


def p_semi(g):
    g.fill(1, 10, 2, 11)
    g.fill(1, 17, 2, 18)
    g.seg(2, 19, 0, 23)


def p_lt(g):
    g.seg(6, 7, 0, 11)
    g.seg(0, 11, 6, 15)


def p_eq(g):
    g.bar(0, 6, 8)
    g.bar(0, 6, 13)


def p_gt(g):
    g.seg(0, 7, 6, 11)
    g.seg(6, 11, 0, 15)


def p_quest(g):
    g.arc(3.5, 6.5, 2.5, 3, 185, 375)
    g.seg(5, 9, 3, 13)
    g.fill(3, 18, 4, 19)


def p_at(g):
    g.arc(4.5, 11.5, 4, 6.5, 35, 330)
    g.arc(4, 11, 2, 2.5, 340, 210)
    g.stem(6, 9, 14)
    g.seg(6, 13, 9, 16)


def p_lbra(g):
    g.stem(2, 3, 19)
    g.bar(0, 2, 3)
    g.bar(0, 2, 18)


def p_bslash(g):
    g.seg(0, 3, 6, 20)


def p_rbra(g):
    g.stem(1, 3, 19)
    g.bar(0, 3, 3)
    g.bar(0, 3, 18)


def p_circ(g):
    g.seg(0, 8, 3.5, 3)
    g.seg(3.5, 3, 7, 8)


def p_under(g):
    g.bar(0, 9, 21)


def p_grave(g):
    g.seg(0, 3, 2, 6)


def p_lcurly(g):
    g.seg(4, 3, 2, 5)
    g.seg(2, 5, 2, 9)
    g.seg(2, 9, 0, 11)
    g.seg(0, 11, 2, 13)
    g.seg(2, 13, 2, 17)
    g.seg(2, 17, 4, 19)


def p_bar(g):
    g.stem(0, 2, 21)


def p_rcurly(g):
    g.seg(0, 3, 2, 5)
    g.seg(2, 5, 2, 9)
    g.seg(2, 9, 4, 11)
    g.seg(4, 11, 2, 13)
    g.seg(2, 13, 2, 17)
    g.seg(2, 17, 0, 19)


def p_tilde(g):
    g.arc(2.5, 12, 2.5, 2, 180, 360)
    g.arc(6.5, 12, 2.5, 2, 0, 180)


glyph(" ", p_space, 2, 2)
glyph("!", p_excl, 2, 2)
glyph('"', p_quot, 1, 1)
glyph("#", p_hash, 1, 1)
glyph("$", p_dollar, 1, 1)
glyph("%", p_pct, 1, 1)
glyph("&", p_amp, 1, 1)
glyph("'", p_apos, 2, 2)
glyph("(", p_lpar, 1, 1)
glyph(")", p_rpar, 1, 1)
glyph("*", p_star, 1, 1)
glyph("+", p_plus, 1, 1)
glyph(",", p_comma, 2, 1)
glyph("-", p_dash, 1, 1)
glyph(".", p_dot, 2, 2)
glyph("/", p_slash, 1, 1)
glyph(":", p_colon, 2, 2)
glyph(";", p_semi, 2, 1)
glyph("<", p_lt, 1, 1)
glyph("=", p_eq, 1, 1)
glyph(">", p_gt, 1, 1)
glyph("?", p_quest, 1, 1)
glyph("@", p_at, 1, 1)
glyph("[", p_lbra, 1, 1)
glyph("\\", p_bslash, 1, 1)
glyph("]", p_rbra, 1, 1)
glyph("^", p_circ, 1, 1)
glyph("_", p_under, 1, 1)
glyph("`", p_grave, 1, 2)
glyph("{", p_lcurly, 1, 1)
glyph("|", p_bar, 2, 2)
glyph("}", p_rcurly, 1, 1)
glyph("~", p_tilde, 1, 1)


# ---------------------------------------------------------------------------
# Rasterise, then pack
# ---------------------------------------------------------------------------
def render():
    """Returns {char: (regular_bitmap, bold_bitmap, advance)}."""
    out = {}
    for ch in range(FIRST_CHAR, LAST_CHAR + 1):
        key = chr(ch)
        if key not in GLYPHS:
            raise SystemExit("no glyph described for %r (0x%02X)" % (key, ch))
        fn, lsb, rsb = GLYPHS[key]
        b = blank()
        fn(b)
        ink = b.ink()
        if key == " ":
            out[key] = (b, b.dilate(), 6)
            continue
        if ink is None:
            raise SystemExit("glyph %r rasterised blank" % key)
        left, right, top, bottom = ink
        if key in "0123456789":
            # Tabular figures: identical advance, ink centred inside it.
            width = right - left + 1
            advance = rsb
            off = 1 + (advance - width - 2) // 2
            gap = advance - width - off
            if gap < 1:
                raise SystemExit("digit %r does not fit its tabular advance" % key)
        else:
            off = lsb
            gap = rsb
        if off + (right - left + 1) + gap > CELL_W:
            raise SystemExit("glyph %r is %d columns wide, cell is %d"
                             % (key, off + (right - left + 1) + gap, CELL_W))
        shifted = Bmp()
        for y in range(CELL_H):
            for x in range(left, right + 1):
                if b.rows[y][x]:
                    shifted.px(x - left + off, y)
        adv = off + (right - left + 1) + gap
        out[key] = (shifted, shifted.dilate(), adv)
    return out


def pack_rows(bmp):
    """One u16 per row, most-significant bit = leftmost pixel."""
    words = []
    for y in range(CELL_H):
        w = 0
        for x in range(CELL_W):
            if bmp.rows[y][x]:
                w |= 1 << (CELL_W - 1 - x)
        words.append(w)
    return words


def main():
    face = render()
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    out = os.path.join(root, 'apps', 'launcher', 'src', 'font.rs')
    data = data_block(face)
    if os.path.exists(out):
        with open(out, encoding='utf-8') as f:
            src = f.read()
        start = src.index(START_MARK)
        end = src.index(END_MARK)
        if end <= start:
            raise SystemExit(
                'font.rs is malformed: the %s definition appears before the '
                'generated block, so splicing would duplicate it'
                % END_MARK)
        src = src[:start] + data + src[end:]
    else:
        src = data
    with open(out, 'w', encoding='utf-8', newline=NL) as f:
        f.write(src)
    print('wrote %s: %d glyphs, %d bytes of bitmaps per weight, %d bytes total'
          % (out, NGLYPH, NGLYPH * CELL_H * 2,
             NGLYPH * CELL_H * 2 * 2 + NGLYPH * 2))
    if '--report' in sys.argv:
        for k in range(FIRST_CHAR, LAST_CHAR + 1):
            c = chr(k)
            ink = face[c][0].ink()
            desc = 'blank' if ink is None else 'ink w=%2d rows %2d..%2d' % (
                ink[1] - ink[0] + 1, ink[2], ink[3])
            print('  0x%02X %r adv %2d %s' % (k, c, face[c][2], desc))


def data_block(face):
    lines = []
    lines.append("/// Glyph bitmaps, regular weight: %d * %d words, glyph-major."
                 % (NGLYPH, CELL_H))
    lines.append("pub const FONT: [u16; %d] = [" % (NGLYPH * CELL_H))
    for i, c in enumerate(chr(k) for k in range(FIRST_CHAR, LAST_CHAR + 1)):
        lines.append("    // %d %r" % (i + FIRST_CHAR, c))
        rows = pack_rows(face[c][0])
        for s in range(0, CELL_H, 8):
            lines.append("    " + ", ".join("0x%04X" % w for w in rows[s:s + 8]) + ",")
    lines.append("];")
    lines.append("")
    lines.append("/// Glyph bitmaps, bold weight, same layout as [`FONT`].")
    lines.append("pub const FONT_BOLD: [u16; %d] = [" % (NGLYPH * CELL_H))
    for i, c in enumerate(chr(k) for k in range(FIRST_CHAR, LAST_CHAR + 1)):
        lines.append("    // %d %r" % (i + FIRST_CHAR, c))
        rows = pack_rows(face[c][1])
        for s in range(0, CELL_H, 8):
            lines.append("    " + ", ".join("0x%04X" % w for w in rows[s:s + 8]) + ",")
    lines.append("];")
    lines.append("")
    lines.append("/// Per-glyph advances in pixels, regular weight, indexed by")
    lines.append("/// `glyph_index(c)`.")
    lines.append("pub const ADVANCE: [u8; %d] = [%s];"
                 % (NGLYPH, ", ".join(str(face[c][2]) for c in face)))
    lines.append("")
    lines.append("/// Per-glyph advances for the bold weight: one wider than regular")
    lines.append("/// for every glyph with ink, because the stems are one pixel thicker.")
    lines.append("pub const ADVANCE_BOLD: [u8; %d] = [%s];"
                 % (NGLYPH, ", ".join(
                     str(face[c][2] + (0 if c == " " else 1)) for c in face)))
    lines.append("")
    return "\n".join(lines)


if __name__ == "__main__":
    main()