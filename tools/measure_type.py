"""Measure the cap heights of the text on the device's screens.

The type scale is a set of cap heights, so the only honest way to ask
"does this screen use the platform's type scale?" is to measure the ink on
the screen and compare. A run's cap height is the height of the band of
rows it inks, and the ladder's rungs are 13, 16, 22, 30, 40 and 52
logical pixels.

Usage:  python tools/measure_type.py [capture.png ...]
"""

import sys
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    print("Pillow is required to measure a capture")
    raise SystemExit(2)

# The capture is the panel's own resolution: 1088 wide for a 720-wide guest
# screen, so a measured pixel count is 1.511 logical pixels.
CAPTURE_SCALE = 1088 / 720

# The ladder's rungs and the cap height each one draws at.
LADDER = {
    "CAPTION": 13,
    "BODY": 16,
    "SECTION": 22,
    "TITLE": 30,
    "HEADING": 40,
    "DISPLAY": 52,
}
LADDER_HEIGHTS = tuple(LADDER.values())


def ink_bands(path: Path) -> list[tuple[int, int, int]]:
    """Returns (top, bottom, ink pixels) for every horizontal band of ink.

    A row counts as inked when it holds some dark pixels but is not a solid
    block: a filled key is a hundred pixels of ink in a row and its band is
    the key's height, not a run's.
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
    for y, count in enumerate(dark_rows):
        if count > 0 and start is None:
            start = y
        elif count == 0 and start is not None:
            bands.append((start, y - 1, sum(dark_rows[start:y])))
            start = None
    if start is not None:
        bands.append((start, len(dark_rows) - 1, sum(dark_rows[start:])))
    return bands


def main() -> int:
    paths = [Path(a) for a in sys.argv[1:]]
    if not paths:
        folder = Path.home() / "AppData" / "Local" / "Temp" / "kilo" / "screens"
        paths = sorted(folder.glob("*.png"))
    for path in paths:
        if not path.is_file():
            continue
        bands = ink_bands(path)
        # A band is a text run when it is thin; a card, a key or an icon
        # makes a tall band, and the ladder's rungs are all under sixty
        # logical pixels.
        sizes = []
        for top, bottom, ink in bands:
            height = (bottom - top + 1) / CAPTURE_SCALE
            if 4 <= height <= 60 and ink > 40:
                sizes.append(round(height))
        if not sizes:
            print(f"{path.name:20} no text runs found")
            continue
        unique = sorted(set(sizes))
        matched = [s for s in unique if any(abs(s - rung) <= 2 for rung in LADDER_HEIGHTS)]
        unmatched = [s for s in unique if s not in matched]
        print(
            f"{path.name:20} runs={len(sizes):3}  heights={unique}  "
            f"on-ladder={matched}  off-ladder={unmatched}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
