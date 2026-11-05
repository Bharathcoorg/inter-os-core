# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Geometry contract for the Dialer's call glyph.

The call key answers a phone call, so its icon has to read as a telephone
handset at the size it is actually painted -- 160 by 76 rows inside the pill.
What it used to paint was two 12-row discs sat exactly on the ends of an
11-row-wide straight diagonal band: the union was one continuous diagonal blob
whose heads were barely wider than the bar carrying them, with no page between
them, which is a stick or a wrench and not a handset.

Every screen in this repository answers "what colour is pixel (x, y)" and nothing
clips, so a glyph is only correct if every inked offset also lies inside the
plate that paints it. These gates re-derive the glyph's own arithmetic from the
source and measure it, rather than trusting a comment about what it looks like.
"""

import math
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(rel: str) -> str:
    return (ROOT / rel).read_text(encoding="utf-8")


def dialer_main() -> str:
    return read("apps/dialer/src/main.rs")


def launcher_icons() -> str:
    return read("apps/launcher/src/icons.rs")


# ---------------------------------------------------------------------------
# The pictogram frame, read out of the source that draws it.
#
# WHY the same expressions are parsed from both files: the home-screen tile and
# the call key are one object. A gate that only looked at the dialer could pass
# a handset the tile does not show, and a gate that only looked at the launcher
# could pass a call key that had drifted away from it.
# ---------------------------------------------------------------------------
def parse_arm(text: str) -> dict:
    """The seven facts of the launcher's Phone silhouette, from one file's text."""
    ear = re.search(
        r"\(fx - (\d+)\) \* \(fx - \d+\) \+ \(fy \+ (\d+)\) \* \(fy \+ \d+\) <= (\d+)",
        text,
    )
    mouth = re.search(
        r"\(fx \+ (\d+)\) \* \(fx \+ \d+\) \+ \(fy - (\d+)\) \* \(fy - \d+\) <= (\d+)",
        text,
    )
    yoke_c = re.search(r"let \w+ = \(fx - (\d+)\) \* \(fx - \d+\) \+ \(fy - (\d+)\) \* \(fy - \d+\);", text)
    band = re.search(r"\w+ >= (\d+) && \w+ <= (\d+)", text)
    slant = re.search(r"fx <= (\d+) && fy <= (\d+) && fx \+ fy <= (\d+)", text)
    assert ear and mouth and yoke_c and band and slant, (
        "the Phone silhouette's five expressions were not all found; the glyph's "
        "arithmetic has been rewritten in a shape this gate cannot read"
    )
    assert ear.group(3) == mouth.group(3), "the two bulbs no longer share one radius"
    return {
        "ear_x": int(ear.group(1)),
        "ear_y": int(ear.group(2)),
        "bulb": int(ear.group(3)),
        "mouth_x": int(mouth.group(1)),
        "mouth_y": int(mouth.group(2)),
        "yoke_x": int(yoke_c.group(1)),
        "yoke_y": int(yoke_c.group(2)),
        "yoke_min": int(band.group(1)),
        "yoke_max": int(band.group(2)),
        "slant": int(slant.group(3)),
        "yoke_fx_limit": int(slant.group(1)),
        "yoke_fy_limit": int(slant.group(2)),
    }


def launcher_phone_arm() -> dict:
    icons = launcher_icons()
    start = icons.index("IconKind::Phone => {")
    end = icons.index("IconKind::", start + len("IconKind::Phone => {"))
    return parse_arm(icons[start:end])


def dialer_handset_branch() -> str:
    """The `if !in_call { .. }` body of the call pill's painter, and nothing else."""
    source = dialer_main()
    start = source.index("// Vector Phone Handset")
    body = source[start:]
    open_at = body.index("if !in_call {")
    close_at = body.index("} else {", open_at)
    return body[open_at:close_at]


def dialer_handset() -> dict:
    branch = dialer_handset_branch()
    arm = parse_arm(branch)
    scale = re.search(r"let fx = (\d+) \* c_dx - (\d+);", branch)
    fy = re.search(r"let fy = (\d+) \* c_dy - (\d+);", branch)
    assert scale and fy, (
        "the call glyph no longer states its fixed-point frame as "
        "`let fx = <scale> * c_dx - <centre>;`, so its size cannot be measured"
    )
    assert scale.group(1) == fy.group(1), "the glyph's two axes are no longer one scale"
    assert scale.group(2) == fy.group(2), "the glyph is centred on x and not on y"
    arm["scale"] = int(scale.group(1))
    arm["centre"] = int(scale.group(2))
    return arm


def plate_of(source: str):
    """The pill's own stadium, from the constants that painter fills it with."""
    r = int(re.search(r"const CALL_PILL_R: i32 = (\d+);", source).group(1))
    spine = int(re.search(r"const CALL_PILL_HALF_SPINE: i32 = (\d+);", source).group(1))
    return r, spine


def in_plate(dx: int, dy: int, radius: int, spine: int) -> bool:
    dxc = max(abs(dx) - spine, 0)
    return dxc * dxc + dy * dy <= radius * radius


def inked(dx: int, dy: int, g: dict) -> bool:
    fx = g["scale"] * dx - g["centre"]
    fy = g["scale"] * dy - g["centre"]
    ear = (fx - g["ear_x"]) ** 2 + (fy + g["ear_y"]) ** 2 <= g["bulb"]
    mouth = (fx + g["mouth_x"]) ** 2 + (fy - g["mouth_y"]) ** 2 <= g["bulb"]
    d2 = (fx - g["yoke_x"]) ** 2 + (fy - g["yoke_y"]) ** 2
    handle = (
        g["yoke_min"] <= d2 <= g["yoke_max"]
        and fx <= g["yoke_fx_limit"]
        and fy <= g["yoke_fy_limit"]
        and fx + fy <= g["slant"]
    )
    return ear or mouth or handle


def glyph_pixels(g: dict, radius: int, spine: int):
    """Every inked offset the painter can reach, and the plate it must live in."""
    reach = radius + spine + 4
    return [
        (dx, dy)
        for dy in range(-reach, reach + 1)
        for dx in range(-reach, reach + 1)
        if inked(dx, dy, g)
    ]


def bulb_centres(g: dict):
    """Where the two ends of a handset sit, in the button's own pixel offsets.

    `inked` works in the glyph's fixed-point frame; this maps the frame's two
    bulb centres back to device pixels the same way the painter maps offsets
    into it, so a measurement made here is a measurement of what is painted.
    """
    s, c = g["scale"], g["centre"]
    return (
        ((g["ear_x"] + c) / s, (-g["ear_y"] + c) / s),
        ((-g["mouth_x"] + c) / s, (g["mouth_y"] + c) / s),
    )


# ---------------------------------------------------------------------------
# The gates.
# ---------------------------------------------------------------------------
def test_the_call_glyph_is_the_launchers_own_phone_pictogram():
    """The key inside the app is the tile outside it, constant for constant.

    The home screen's dialer tile is drawn by `eval_icon_subpixel`'s
    `IconKind::Phone` arm. The call key now draws that same silhouette in that
    same fixed-point frame, so the two cannot disagree about what a handset is.
    """
    launcher = launcher_phone_arm()
    dialer = dialer_handset()
    shared = set(launcher)
    branch = dialer_handset_branch()
    assert "c_dx" in branch and "c_dy" in branch, (
        "the call glyph no longer answers offsets inside the button's own box"
    )
    assert not re.search(r"\bx\b|\by\b", branch), (
        "the handset is painted from the painter's absolute screen coordinates, not "
        "from the button's own centre, so it would sit off the pill that answers it"
    )
    assert {k: dialer[k] for k in shared} == launcher, (
        "the call key and the launcher's Phone tile no longer hold the same "
        "silhouette: the tile says one thing about a handset and the key another"
    )


def test_the_call_glyph_lives_inside_the_plate_that_paints_it():
    """No clip exists here, so a pixel outside the pill is a pixel erased.

    The plate is drawn by the same `if c_d2 <= CALL_PILL_R * CALL_PILL_R` the
    glyph is tested inside, so an offset the pill does not fill would be painted
    over by the plate's own background colour and silently lost.
    """
    source = dialer_main()
    radius, spine = plate_of(source)
    g = dialer_handset()
    escaped = [p for p in glyph_pixels(g, radius, spine) if not in_plate(*p, radius, spine)]
    assert not escaped, f"the handset inks {len(escaped)} offsets outside its pill: {escaped[:6]}"


def test_the_call_glyph_is_big_enough_to_read():
    """A glyph the size of the old one cannot carry a handset's parts.

    The stick it replaces spanned 37 rows of the pill's 76. The silhouette's own
    box at this scale has to fill most of the plate, or the yoke and its two
    pieces merge into one lump again.
    """
    source = dialer_main()
    radius, spine = plate_of(source)
    g = dialer_handset()
    px = glyph_pixels(g, radius, spine)
    xs = [p[0] for p in px]
    ys = [p[1] for p in px]
    width, height = max(xs) - min(xs) + 1, max(ys) - min(ys) + 1
    assert width >= 44 and height >= 44, (
        f"the handset stands {width} by {height}; the parts have merged back into a bar"
    )
    assert height <= 2 * radius, "the handset is taller than the pill that fills it"


def test_the_call_glyph_is_centred_on_the_button_that_answers_it():
    """An off-centre glyph reads as a different control than the one it labels.

    The tap band is the whole pill, centred on (CALL_CX, CALL_CY); the glyph is
    answered from the same offsets, so its own box has to sit on that centre.
    """
    source = dialer_main()
    radius, spine = plate_of(source)
    g = dialer_handset()
    px = glyph_pixels(g, radius, spine)
    xs = [p[0] for p in px]
    ys = [p[1] for p in px]
    cx = (min(xs) + max(xs)) / 2
    cy = (min(ys) + max(ys)) / 2
    assert abs(cx) <= 3 and abs(cy) <= 3, f"the handset's box is centred at ({cx}, {cy})"


def test_the_call_glyph_has_an_open_hook_and_not_one_straight_bar():
    """The one fact that separates a handset from a stick.

    A handset is two pieces held apart by a yoke that bows, so the line joining
    the pieces passes over an open page. The paint this app replaced did exactly
    the opposite: two discs sat on the ends of a straight band, so the line
    between the two heads was inked its whole length -- 1293 of 1293 interior
    samples -- and the ink never left it by more than 6 rows. Both numbers are
    measured here from the glyph's own arithmetic, so a regression to any
    straight-bar construction fails on the shape, not on a comment.
    """
    source = dialer_main()
    radius, spine = plate_of(source)
    g = dialer_handset()
    ear, mouth = bulb_centres(g)
    bulb_px = g["bulb"] ** 0.5 / g["scale"]

    # The open page: the line between the two heads, past the bulbs themselves.
    clear = inked_on_line = 0
    for i in range(2001):
        t = i / 2000
        px, py = ear[0] + (mouth[0] - ear[0]) * t, ear[1] + (mouth[1] - ear[1]) * t
        if min(math.hypot(px - ear[0], py - ear[1]), math.hypot(px - mouth[0], py - mouth[1])) <= bulb_px:
            continue
        if inked(round(px), round(py), g):
            inked_on_line += 1
        else:
            clear += 1
    total = clear + inked_on_line
    assert total > 0 and clear / total >= 0.9, (
        f"the line joining the handset's two heads is inked for {inked_on_line} of "
        f"{total} samples: that is a bar between them, not a hook"
    )

    # The bow: some ink has to stand off that line, or there is no handle to hold
    # the two pieces apart.
    lx, ly = mouth[0] - ear[0], mouth[1] - ear[1]
    span = math.hypot(lx, ly)
    bow = max(abs((dx - ear[0]) * ly - (dy - ear[1]) * lx) / span for dx, dy in glyph_pixels(g, radius, spine))
    assert bow >= 12, (
        f"the ink bows only {bow:.1f} rows off the line between its own two heads "
        f"(the discarded straight bar measured 5.7): it is a stick again"
    )


def test_the_call_glyph_is_monochrome():
    """Its neighbours ink one colour; a two-tone handset is a different icon.

    The branch returns the painter's single `glyph` ink and no theme colour, so
    the call key keeps the same one-colour build as the hangup it alternates with
    and the backspace beside it.
    """
    branch = dialer_handset_branch()
    assert "THEME." not in branch, "the handset branch now chooses its own colour"
    assert branch.count("return glyph;") == 1, (
        "the handset branch returns more than one ink; it is no longer monochrome"
    )


def test_the_old_stick_construction_is_gone():
    """The two discs on a straight band are the defect, pinned so they stay gone."""
    branch = dialer_handset_branch()
    assert "n1 <= 36 || n2 <= 36" not in branch, (
        "the call glyph is back to two r=6 discs, which read as a stick"
    )
    assert "diff.abs() <= 5" not in branch, (
        "the call glyph is back to an 11-row straight diagonal band"
    )


def test_the_ink_answers_the_three_parts_and_nothing_else():
    """The silhouette is only real if the branch that paints it asks the parts.

    WHY this gate exists beside the arithmetic ones: every measurement above reads
    the ear, mouth and yoke constants out of the source, so a painter that kept
    those bindings and returned a different predicate -- a band, a disc, anything
    -- would hold them all green while inking something else. This gate names the
    three parts the ink actually answers, so the arithmetic and the paint cannot
    drift apart.
    """
    branch = dialer_handset_branch()
    painted = re.search(r"if ([a-z_0-9 |]+) \{\s*\n\s*return glyph;", branch)
    assert painted, (
        "the handset no longer returns its ink from one predicate over named parts, "
        "so what is painted is no longer what these gates measure"
    )
    names = {n.strip() for n in painted.group(1).split("|") if n.strip()}
    assert names == {"ear", "mouth", "handle"}, (
        f"the call key inks {sorted(names)}; a handset is its two pieces and the "
        f"yoke that holds them apart"
    )
    for part in names:
        assert re.search(rf"let {part} = ", branch), f"{part} is painted but never derived"
