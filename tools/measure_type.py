"""Measure the height of the text on the device's screens.

The type scale is a set of cap heights, so the only honest way to ask
"does this screen use the platform's type scale?" is to measure the ink on
the screen and compare. The ladder's rungs are the cap heights
`cap_for_scale` documents: 16, 21, 27, 35, 44 and 56 logical pixels.

WHY the rungs are restated here at all: this tool runs on the host against a
capture and cannot call the Rust ladder, so the numbers are a copy -- and a
copy is exactly what goes stale. `tests/test_type_scale_contract.py` now
compares every value below against `font::cap_for_scale`'s own table, so a
rung that moves here without moving there fails a contract instead of
silently reporting an on-ladder screen as off it.

WHAT this reads, exactly: each screen is projected onto rows, split into inked
bands, and a band's *body height* is the band minus the tails that drop below
the run's typical ink. A run with a capital and no descender is then measured
at its cap height; a run with a descender ("Settings", "gallery") would be
measured taller than its cap if the tail were left in, which is why it is
trimmed; and an all-lowercase run is measured at its x-height, about half a
cap. So the readings are a legibility floor -- a screen whose tallest runs all
sit under the ladder's caption rung has small text, and a screen that reaches
the rungs does not -- rather than a per-glyph font metric.

Usage:  python tools/measure_type.py [capture.png ...]
"""

import sys
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    print("Pillow is required to measure a capture")
    raise SystemExit(2)

# The guest's panel, in logical pixels.
GUEST_WIDTH = 720

# The ladder's rungs and the cap height each one draws at. These are
# `cap_for_scale` rungs 1..6; the hero rung (7 = 76) is a lock-screen clock,
# taller than the 60-row ceiling below that separates a text band from a card,
# so it is measured as a tall band rather than matched here.
LADDER = {
    "CAPTION": 16,
    "BODY": 21,
    "SECTION": 27,
    "TITLE": 35,
    "HEADING": 44,
    "DISPLAY": 56,
}
LADDER_HEIGHTS = tuple(LADDER.values())

# A row belongs to the run's body when it inks at least this fraction of what
# the band's *median* row inks. WHY the median and not the densest row: one
# hairline or one filled toggle inside a band out-inks every text row, and a
# threshold taken from it discards the whole run -- measured, that read a whole
# body line as 4px. The median is the typical glyph row, and a descender's tail
# or an apostrophe's tip inks a small fraction of it, so the two separate.
BODY_SHARE = 2


def _body_rows(rows: list[int]) -> int:
    """How many of a band's rows carry the run's body rather than its tails.

    Trimming is from the ends inward only: a light row in the middle of a band
    sits between two glyphs' stems and is not evidence of a tail.
    """
    pivot = sorted(rows)[len(rows) // 2]
    top, bottom = 0, len(rows)
    while bottom - top > 1 and rows[bottom - 1] * BODY_SHARE < pivot:
        bottom -= 1
    while bottom - top > 1 and rows[top] * BODY_SHARE < pivot:
        top += 1
    return bottom - top


def ink_bands(path: Path) -> tuple[int, list[tuple[int, int, int, int]]]:
    """Returns (width, bands), where a band is (top, bottom, ink, body rows).

    A row counts as inked when it holds some dark pixels but is not a solid
    block: a filled key is a hundred pixels of ink in a row and its band is
    the key's height, not a run's.

    `body rows` is the height a reader sees: the run minus whatever tail drops
    below the baseline and minus the gap above the caps. It equals the cap
    height for a run that carries a capital and no descender, and the x-height
    for a run that is all lowercase -- roughly half a cap. That is why the
    numbers below are read as a legibility floor, not as rung identifiers.
    """
    image = Image.open(path).convert("L")
    width, height = image.size
    pixels = image.load()
    solid = width // 8
    dark_rows = []
    for y in range(height):
        count = sum(1 for x in range(0, width, 2) if pixels[x, y] < 170)
        dark_rows.append(count if 0 < count < solid else 0)
    bands = []
    start = None
    for y, count in enumerate(dark_rows + [0]):
        if count > 0 and start is None:
            start = y
        elif count == 0 and start is not None:
            rows = dark_rows[start:y]
            bands.append((start, y - 1, sum(rows), _body_rows(rows)))
            start = None
    return width, bands


def main() -> int:
    paths = [Path(a) for a in sys.argv[1:]]
    if not paths:
        folder = Path.home() / "AppData" / "Local" / "Temp" / "kilo" / "screens"
        paths = sorted(folder.glob("*.png"))
    for path in paths:
        if not path.is_file():
            continue
        width, bands = ink_bands(path)
        # WHY the scale comes from the file and not a constant: a capture is
        # whatever the host presented it at -- the guest's own 720-wide panel,
        # a scaled VNC frame, a window of another size -- and a fixed divisor
        # reports a 15-pixel cap as a 10-pixel one on a shrunk frame. The
        # panel's logical width is known, so its scale is the frame's.
        scale = width / GUEST_WIDTH
        # A band is a text run when it is thin; a card, a key or an icon
        # makes a tall band, and the ladder's rungs are all under sixty
        # logical pixels.
        sizes = []
        for top, bottom, ink, body in bands:
            height = (bottom - top + 1) / scale
            body_height = body / scale
            if 4 <= height <= 60 and ink > 40:
                sizes.append(round(body_height))
        if not sizes:
            print(f"{path.name:20} no text runs found")
            continue
        unique = sorted(set(sizes))
        matched = [s for s in unique if any(abs(s - rung) <= 2 for rung in LADDER_HEIGHTS)]
        unmatched = [s for s in unique if s not in matched]
        print(
            f"{path.name:20} runs={len(sizes):3}  tallest={max(sizes)}  "
            f"heights={unique}  on-ladder={matched}  off-ladder={unmatched}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
