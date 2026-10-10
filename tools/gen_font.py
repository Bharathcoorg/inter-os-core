#!/usr/bin/env python3
"""Generates the InterOS 16x24 bitmap face by rasterising Roboto Condensed.

WHY this file exists: the typeface is the single biggest legibility lever in
this project, and the user asked for "commonly used smartphone fonts". The
previous face described every glyph as parametric strokes -- maintainable, but
its letterforms were not a face anyone recognises, and at 1x (the size the
whole UI actually uses) it read poorly. This generator rasterises **Roboto
Condensed** (the Android family's condensed cut, Apache License 2.0, committed
under tools/fonts/ with its licence) into the SAME 16x24 grid the firmware
already uses: baseline below row 19, cap band rows 3..=19, tabular figures at
advance 11. Every Rust constant, API and test contract is kept; only the
letterforms change.

HOW TO REGENERATE:
    python tools/gen_font.py            # writes both font.rs data blocks
    python tools/gen_font.py --report   # also prints the ink/advance table

Outputs (both spliced between marker comments; the Rust constants, API and
tests live outside the markers and are never touched):
  * apps/launcher/src/font.rs            bitmaps + advances only: the shell's
                                         copy exists for layout math (measure,
                                         ellipsize, ink bounds), because the
                                         shell renders through the SDK face it
                                         does not need the coverage tables
  * sdk/rust/interos-std/src/font.rs     bitmaps + advances + the 2-bit
                                         coverage tables the anti-aliased
                                         `test_text_aa` blitter blends against

Requires Pillow (dev-only; the generated Rust is committed, so the firmware
builds without Python). Both the generator and its output are committed, so
the data is reproducible and auditable.

RASTERISATION (all the design decisions, in one place):
 * em        24 px, rendered at 4x (96 px) and area-downsampled to the cell;
             supersampling stands in for hinting at this size. Probe-verified:
             cap 'H' lands rows 3..=19, round lowercase rows 7..=19, the
             ascenders of b d f h k l reach row 2.
 * baseline  placed on the line below row 19 (cell row 20's top edge).
 * coverage  kept at full precision, then quantised to 2 bits per pixel. The
             SDK face also stores the levels (FONT_COV / FONT_COV_BOLD, one u32
             per row: 16 pixels x 2 bits) so the text blitter can blend glyph
             ink against any background instead of thresholding to hard edges.
             The binary view is `level >= 2`, i.e. exactly the old 0.5
             threshold, so every ink contract stays bit-identical. Round sides
             carry a sub-pixel baseline overshoot (~0.35 px in Roboto) that
             survives downsampling only as ~0.3 coverage in row 20; the
             OVERSHOOT restore lifts such cells to full ink, which is what
             gives 'o c g e s' their documented 1px overshoot.
 * digits    tabular: every figure's ink span is resampled to exactly 8
             columns and centred in advance 11, so a clock never wobbles.
 * wide ink  any glyph whose advance would exceed the 16px cell is clamped to
             16 and its ink resampled to at most 15 columns (W, m).
 * descenders rows below 23 are area-binned into rows 20..=23, so g j p q y
             keep their full tail shape instead of being sliced flat.
 * ascenders ink above row 2 is clipped (Rust bound: ink may start at row 2).
 * bold      the regular bitmap dilated one pixel right (stems 2 -> 3 px),
             advance +1 -- a generated weight, exactly as before.
 * space     blank, advance 6 == Rust FALLBACK_ADVANCE.

The _verify() pass below mirrors every metric assertion the Rust tests in
apps/launcher/src/font.rs make, so a bad regeneration fails here, loudly,
instead of in cargo test.
"""

import os
import sys

from PIL import Image, ImageDraw, ImageFont

# ---------------------------------------------------------------------------
# The design grid (unchanged; these are the firmware's contract)
# ---------------------------------------------------------------------------
CELL_W, CELL_H = 16, 24
CAP_TOP = 3        # first row of a flat-topped capital
BASE = 19          # last row above the baseline
DESC_BOT = 23      # lowest permitted ink row
FIRST_CHAR = 32
LAST_CHAR = 126
NGLYPH = LAST_CHAR - FIRST_CHAR + 1
NL = "\n"

# Splice markers: everything between them is rewritten on every run, so the
# hand-written API and tests in font.rs live outside them.
START_MARK = '/// Glyph bitmaps, regular weight'
END_MARK = 'pub const FALLBACK_ADVANCE'
# The SDK block anchors on the first line the generated data itself emits, so
# the splice is idempotent. Why not a hand-written marker line: the first run
# of this generator (cc1e4c4) spliced over exactly such a marker, consumed it
# and never re-emitted it, which left the SDK file un-spliceable (its start
# anchor was gone, only the end marker remained). Anchoring on the first
# generated line cannot happen twice, because that line is part of every
# regeneration.
SDK_START_MARK = START_MARK
SDK_END_MARK = '// <<< generated block ends above this line'

# ---------------------------------------------------------------------------
# Rasterisation parameters (probe-verified against Roboto Condensed)
# ---------------------------------------------------------------------------
SS = 4                    # supersample factor for the FreeType render
EM = 24                   # em size in cell pixels: cap height lands on 17 rows
THRESH = 0.5              # coverage -> ink
OVERSHOOT = 0.20          # coverage that restores the sub-pixel baseline dip
MIN_TOP = 2               # ink may not start above this row (Rust test bound)
DIGIT_ADV = 11            # tabular figure advance (Rust test contract)
DIGIT_INK = 8             # tabular figure ink width (Rust test contract)
MAX_ADV = CELL_W          # no advance may exceed the cell (Rust test bound)
SPACE_ADV = 6             # == Rust FALLBACK_ADVANCE
CANVAS_W, CANVAS_H = 48, 36   # 1x canvas in cell pixels (ink + bearings + tail)
BASE_LINE = 20            # baseline is the line below row 19

_TTF = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                    'fonts', 'RobotoCondensed-Regular.ttf')
_font_cache = []


def _face_font():
    """The FreeType face at supersampled size, loaded once."""
    if not _font_cache:
        if not os.path.exists(_TTF):
            raise SystemExit(
                'missing %s -- Roboto Condensed (Apache-2.0) must sit in '
                'tools/fonts/ for the face to regenerate' % _TTF)
        _font_cache.append(ImageFont.truetype(_TTF, EM * SS))
    return _font_cache[0]


class Bmp:
    """A 16x24 coverage canvas. Coordinates are pixels; (0,0) is the top-left.

    Each cell holds coverage in 0..1 rather than a hard bit. `ink()` applies the
    binary threshold (`>= THRESH`) for the metric contracts, and the emitter
    quantises the same values to 2-bit levels for the anti-aliased blitter, so
    both views come from one source and cannot drift.
    """

    def __init__(self):
        self.rows = [[0.0] * CELL_W for _ in range(CELL_H)]

    def px(self, x, y):
        if 0 <= x < CELL_W and 0 <= y < CELL_H:
            self.rows[y][x] = 1.0

    def ink(self):
        """(left, right) ink columns and (top, bottom) ink rows, or None.

        Binary view: a cell counts as ink when its coverage reaches THRESH, so
        every metric contract (cap band, overshoot, descenders, digit width)
        measures exactly the ink the 1-bit table asserts.
        """
        cols = [x for y in range(CELL_H) for x in range(CELL_W)
                if self.rows[y][x] >= THRESH]
        rows = [y for y in range(CELL_H) for x in range(CELL_W)
                if self.rows[y][x] >= THRESH]
        if not cols:
            return None
        return (min(cols), max(cols), min(rows), max(rows))

    def dilate(self, n=1):
        """Horizontal dilation: every ink pixel also lights `n` columns right.

        WHY right and not both sides: a real bold face keeps its left
        sidebearing and gains the extra ink on the stem's far side, so 'o'
        grows from a 2 px stem pair to a 3 px stem pair without shifting
        inside its own advance.
        """
        out = [row[:] for row in self.rows]
        for y in range(CELL_H):
            row = self.rows[y]
            for x in range(CELL_W):
                if row[x] >= THRESH:
                    for d in range(1, n + 1):
                        if x + d < CELL_W:
                            out[y][x + d] = 1.0
        return Bmp.from_rows(out)

    @staticmethod
    def from_rows(rows):
        b = Bmp()
        b.rows = rows
        return b


def blank():
    return Bmp()


# ---------------------------------------------------------------------------
# Coverage pipeline
# ---------------------------------------------------------------------------
def _coverage(ch):
    """1x coverage grid (rows of floats 0..1) for `ch`, baseline-aligned."""
    font = _face_font()
    img = Image.new("L", (CANVAS_W * SS, CANVAS_H * SS), 0)
    ImageDraw.Draw(img).text((0, BASE_LINE * SS), ch, font=font, fill=255,
                             anchor="ls")
    px = img.load()
    grid = []
    for y in range(CANVAS_H):
        row = []
        for x in range(CANVAS_W):
            acc = 0
            for dy in range(SS):
                for dx in range(SS):
                    acc += px[x * SS + dx, y * SS + dy]
            row.append(acc / (SS * SS * 255.0))
        grid.append(row)
    return grid


def _resample_row(row, left, right, n_out):
    """Area-average the coverage span [left..right] (inclusive) into n_out bins."""
    width = right - left + 1
    out = []
    for i in range(n_out):
        a = left + i * width / n_out
        b = left + (i + 1) * width / n_out
        acc = 0.0
        n = 0.0
        col = int(a)
        while col < b and col < len(row):
            lo = max(a, col)
            hi = min(b, col + 1)
            if hi > lo:
                acc += row[col] * (hi - lo)
                n += hi - lo
            col += 1
        out.append(acc / n if n > 0 else 0.0)
    return out


def _bin_rows(grid, first, last):
    """Area-bin grid rows [first..last] (inclusive) into exactly 4 rows.

    Used for descenders: Roboto's tails reach ~5-6 rows below the baseline at
    this em, and the cell reserves 4. Binning preserves the whole tail shape
    (the hook of a 'g') where clipping would slice it flat.
    """
    n = last - first + 1
    width = len(grid[0])
    out = []
    for i in range(4):
        a = first + i * n / 4.0
        b = first + (i + 1) * n / 4.0
        acc = [0.0] * width
        total = 0.0
        rowf = a
        while rowf < b:
            r = int(rowf)
            lo = max(a, r)
            hi = min(b, r + 1)
            if hi > lo and 0 <= r < len(grid):
                for x in range(width):
                    acc[x] += grid[r][x] * (hi - lo)
                total += hi - lo
            rowf += 1
        if total > 0:
            acc = [v / total for v in acc]
        out.append(acc)
    return out


def _threshold_row(row, thresh=THRESH):
    return [1 if v >= thresh else 0 for v in row]


def _ink_bbox(rows):
    top = bot = left = right = None
    for y, row in enumerate(rows):
        for x, v in enumerate(row):
            if v:
                if top is None:
                    top = y
                bot = y
                if left is None or x < left:
                    left = x
                if right is None or x > right:
                    right = x
    return (left, right, top, bot)


def _build_glyph(ch):
    """Rasterise `ch` -> (Bmp, advance), honouring every documented contract."""
    if ch == " ":
        return blank(), SPACE_ADV

    cov = _coverage(ch)

    # -- vertical fit, in coverage -----------------------------------------
    # clip above MIN_TOP
    for y in range(min(MIN_TOP, len(cov))):
        cov[y] = [0.0] * len(cov[y])

    def _bottom(grid):
        b = _ink_bbox([_threshold_row(r) for r in grid])
        return None if b[3] is None else b[3]

    bottom = _bottom(cov)
    if bottom is not None and bottom > DESC_BOT:
        # descender: bin rows [BASE+1 .. bottom] into the 4 reserved rows
        head = cov[: BASE + 1]
        tail = _bin_rows(cov, BASE + 1, bottom)
        cov = head + tail

    # -- horizontal fit, in coverage ---------------------------------------
    thresh_rows = [_threshold_row(r) for r in cov]
    box = _ink_bbox(thresh_rows)
    if box[0] is None:
        raise SystemExit("glyph %r rasterised blank" % ch)
    left, right = box[0], box[1]

    if ch in "0123456789":
        n_out, lsb, adv = DIGIT_INK, 1, DIGIT_ADV
    else:
        adv = int(round(_face_font().getlength(ch) / SS))
        if adv > MAX_ADV:
            adv = MAX_ADV
        if adv < 2:
            adv = 2
        ink_w = right - left + 1
        n_out = min(ink_w, MAX_ADV - 1)
        if adv < n_out + 1:
            n_out = max(1, adv - 1)
        lsb = min(left, max(0, adv - n_out - 1))

    cell = [[0.0] * CELL_W for _ in range(CELL_H)]
    for y in range(CELL_H):
        span = _resample_row(cov[y], left, right, n_out)
        for i, v in enumerate(span):
            cell[y][lsb + i] = v

    # -- overshoot restoration ---------------------------------------------
    # Round sides dip ~0.35 px below the baseline; a plain threshold rounds
    # that away. Where the coverage grid says a round side sits below the
    # baseline, lift that cell to full ink: the binary view keeps the 1px
    # overshoot 'o c g e s' are documented to have, and the coverage view
    # paints it at full weight rather than as a hard new block.
    if BASE + 1 < CELL_H and any(v >= THRESH for v in cell[BASE]):
        for x in range(n_out):
            if cov[BASE + 1][left + x] >= OVERSHOOT:
                cell[BASE + 1][lsb + x] = max(cell[BASE + 1][lsb + x], THRESH)

    return Bmp.from_rows(cell), adv


def render():
    """Returns {char: (regular_bitmap, bold_bitmap, advance)}."""
    out = {}
    for k in range(FIRST_CHAR, LAST_CHAR + 1):
        ch = chr(k)
        bmp, adv = _build_glyph(ch)
        out[ch] = (bmp, bmp.dilate(), adv)
    return out


# ---------------------------------------------------------------------------
# The contracts the Rust tests make, mirrored here so a bad run fails loud
# ---------------------------------------------------------------------------
def _verify(face):
    def adv(c):
        return face[c][2]

    def ink(c):
        return face[c][0].ink()

    problems = []
    for k in range(FIRST_CHAR, LAST_CHAR + 1):
        c = chr(k)
        if c == " ":
            if ink(c) is not None:
                problems.append("space must be blank")
            continue
        b = ink(c)
        if b is None:
            problems.append("glyph %r is blank" % c)
            continue
        left, right, top, bot = b
        if top < MIN_TOP:
            problems.append("%r starts at row %d, above the ascender" % (c, top))
        if bot > DESC_BOT:
            problems.append("%r reaches row %d, past the descender" % (c, bot))
        if adv(c) > MAX_ADV:
            problems.append("%r advance %d exceeds the cell" % (c, adv(c)))
        if right >= adv(c):
            problems.append("%r ink right %d >= advance %d" % (c, right, adv(c)))
    for d in "0123456789":
        if adv(d) != DIGIT_ADV:
            problems.append("digit %r advance %d != %d (tabular)"
                            % (d, adv(d), DIGIT_ADV))
        b = ink(d)
        if b is not None and b[1] - b[0] + 1 != DIGIT_INK:
            problems.append("digit %r ink width %d != %d (condensed)"
                            % (d, b[1] - b[0] + 1, DIGIT_INK))
    h = ink("H")
    if h is not None and not (CAP_TOP - 1 <= h[2] <= CAP_TOP + 1
                              and h[3] in (BASE, BASE + 1)):
        problems.append("'H' spans rows %d..%d, outside the cap band"
                        % (h[2], h[3]))
    for c in "ocges":
        b = ink(c)
        if b is None:
            continue
        if b[2] > 7:
            problems.append("%r reaches row %d, below the x-height band" % (c, b[2]))
        if b[3] < BASE + 1:
            problems.append("%r stops at row %d, no baseline overshoot" % (c, b[3]))
    for c in "gjpqy":
        b = ink(c)
        if b is not None and b[3] < BASE + 1:
            problems.append("%r has no descender below the baseline" % c)
    if problems:
        raise SystemExit("face violates the Rust test contracts:\n  "
                         + "\n  ".join(problems))


# ---------------------------------------------------------------------------
# Emission (identical shape to the previous generator's output)
# ---------------------------------------------------------------------------
def pack_rows(bmp, coverage=False):
    """One word per row: MSB (or top bit pair) is the leftmost pixel.

    `coverage=False` keeps the historical 1-bit u16 packing. `coverage=True`
    packs the 2-bit coverage level of each of the 16 pixels into a u32, two
    bits per pixel, so a 24-row glyph is 24 words with no index arithmetic on
    the reading side beyond the shift. Levels are 0..=3, quantised with
    round-half-up so a cell at exactly the old 0.5 threshold lands on level 2
    and stays ink in the binary view.
    """
    words = []
    for y in range(CELL_H):
        w = 0
        for x in range(CELL_W):
            v = bmp.rows[y][x]
            if coverage:
                level = min(3, int(v * 3 + 0.5))
                if level:
                    w |= level << (2 * (CELL_W - 1 - x))
            elif v >= THRESH:
                w |= 1 << (CELL_W - 1 - x)
        words.append(w)
    return words


def _emit_bitmaps(face, cov):
    """Bitmap + advance constants. `cov` adds the 2-bit coverage tables."""
    lines = []

    def table(name, weight, coverage):
        lines.append("/// Glyph bitmaps, %s weight: %d * %d %s, glyph-major."
                     % (weight, NGLYPH, CELL_H,
                        "coverage words (2 bits/pixel)" if coverage else "words"))
        lines.append("pub const %s: [%s; %d] = ["
                     % (name, "u32" if coverage else "u16", NGLYPH * CELL_H))
        for i, c in enumerate(chr(k) for k in
                              range(FIRST_CHAR, LAST_CHAR + 1)):
            lines.append("    // %d %r" % (i + FIRST_CHAR, c))
            rows = pack_rows(face[c][0 if weight == "regular" else 1],
                             coverage=coverage)
            for s in range(0, CELL_H, 8):
                lines.append("    " + ", ".join("0x%08X" if coverage else "0x%04X"
                                                % w for w in rows[s:s + 8]) + ",")
        lines.append("];")
        lines.append("")

    table("FONT", "regular", False)
    table("FONT_BOLD", "bold", False)
    if cov:
        lines.append("/// Per-pixel coverage levels for the regular weight, packed")
        lines.append("/// two bits per pixel: column `x` of row `y` holds")
        lines.append("/// `(word >> (2 * (15 - x))) & 0b11`. Level 0..=3 maps to")
        lines.append("/// alpha 0/85/170/255 for `test_text_aa`, so glyph ink blends")
        lines.append("/// against any background. `level >= 2` is exactly the ink")
        lines.append("/// bit in `FONT`, so the two views can never disagree.")
        lines.append("pub const FONT_COV: [u32; %d] = [" % (NGLYPH * CELL_H))
        for i, c in enumerate(chr(k) for k in
                              range(FIRST_CHAR, LAST_CHAR + 1)):
            lines.append("    // %d %r" % (i + FIRST_CHAR, c))
            rows = pack_rows(face[c][0], coverage=True)
            for s in range(0, CELL_H, 8):
                lines.append("    "
                             + ", ".join("0x%08X" % w for w in rows[s:s + 8]) + ",")
        lines.append("];")
        lines.append("")
        lines.append("/// Coverage levels for the bold weight, same packing as")
        lines.append("/// [`FONT_COV`].")
        lines.append("pub const FONT_COV_BOLD: [u32; %d] = [" % (NGLYPH * CELL_H))
        for i, c in enumerate(chr(k) for k in
                              range(FIRST_CHAR, LAST_CHAR + 1)):
            lines.append("    // %d %r" % (i + FIRST_CHAR, c))
            rows = pack_rows(face[c][1], coverage=True)
            for s in range(0, CELL_H, 8):
                lines.append("    "
                             + ", ".join("0x%08X" % w for w in rows[s:s + 8]) + ",")
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


def data_block(face, coverage_tables=False):
    """The generated block for one font.rs.

    `coverage_tables` is set only for the SDK face: the shell's copy exists for
    layout maths and renders through the SDK face, so carrying a second copy of
    the coverage data there would just be dead weight in the launcher image.
    """
    return _emit_bitmaps(face, coverage_tables)


def _splice(path, start_mark, end_mark, data, label, word_bytes=2):
    with open(path, encoding='utf-8') as f:
        src = f.read()
    start = src.index(start_mark)
    end = src.index(end_mark)
    if end <= start:
        raise SystemExit(
            '%s is malformed: the %s definition appears before the '
            'generated block, so splicing would duplicate it'
            % (label, end_mark))
    src = src[:start] + data + src[end:]
    with open(path, 'w', encoding='utf-8', newline=NL) as f:
        f.write(src)
    print('wrote %s (%d glyphs, %d bytes of bitmaps per weight)'
          % (label, NGLYPH, NGLYPH * CELL_H * word_bytes))


def main():
    face = render()
    _verify(face)
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    # The shell's copy stays bitmap-only: it exists for layout maths and
    # renders through the SDK face, so it does not need the coverage words.
    _splice(os.path.join(root, 'apps', 'launcher', 'src', 'font.rs'),
             START_MARK, END_MARK, data_block(face),
             'apps/launcher/src/font.rs')
    _splice(os.path.join(root, 'sdk', 'rust', 'interos-std', 'src', 'font.rs'),
             SDK_START_MARK, SDK_END_MARK,
             data_block(face, coverage_tables=True),
             'sdk/rust/interos-std/src/font.rs', word_bytes=4)
    if '--report' in sys.argv:
        for k in range(FIRST_CHAR, LAST_CHAR + 1):
            c = chr(k)
            ink = face[c][0].ink()
            desc = 'blank' if ink is None else 'ink w=%2d rows %2d..%2d' % (
                ink[1] - ink[0] + 1, ink[2], ink[3])
            print('  0x%02X %r adv %2d %s' % (k, c, face[c][2], desc))


if __name__ == "__main__":
    main()
