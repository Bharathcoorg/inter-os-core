#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Generate the InterOS wallpaper catalogue from the reference artwork.

The guest has no image decoder and no framebuffer budget for photographic
pixels, so a wallpaper is stored the way the lock screen already renders one:
three ramp stops plus two silhouette skylines traced at reference resolution.
The skylines are measured per column out of the supplied artwork, so the
mountain silhouette survives the encoding instead of being invented twice.

Reference geometry is the lock screen's own: `apps/launcher/src/screens/lock.rs`
declares the panel at `REF_W` x `REF_H` reference pixels, and each skyline is the
first reference row at which that column's luminance falls below a threshold and
stays below it.

    python tools/gen_walls.py [--check]

Without `--check` the generated module is written to
`sdk/rust/interos-std/src/design/walls.rs`. With `--check` it is only compared,
so a stale module fails a build instead of silently rendering the wrong art.
"""
from __future__ import annotations

import argparse
import pathlib
import sys

import numpy as np
from PIL import Image

REF_W = 173
REF_H = 427

ROOT = pathlib.Path(__file__).resolve().parents[1]
WALLPAPER_DIR = ROOT / "Ui Samples" / "reference" / "wallpapers"
OUT_PATH = ROOT / "sdk" / "rust" / "interos-std" / "src" / "design" / "walls.rs"

#: The stock wallpaper is the one the shell shipped with, so its stops come from
#: the theme rather than from the artwork and its skylines keep their hand-tuned
#: values. Everything else is measured.
STOCK = {
    "key": "stock",
    "label": "InterOS Stock",
    "source": None,
    "top": 0xFF9CB8D6,
    "mid": 0xFF6E7F8C,
    "bottom": 0xFF241F1C,
    # The stock ramp's sky is a mid blue, so its own type stays dark ink.
    "ink": 0xFF111214,
    "horizon_y": 156,
    "mist_y": 244,
    "rock_y": 330,
    "far_threshold": None,
    "far_hold": None,
    "near_threshold": None,
    "near_hold": None,
}

#: One entry per artwork file that is distinct enough to be its own wallpaper.
#: `01`, `02` and `04` are the same daytime landscape at the same exposure --
#: measured flat, every band is within 2/255 -- so they contribute ONE daylight
#: scene rather than three that look identical. `03` is a separate night scene,
#: so it gets its own relative thresholds: a night sky is never brighter than 60,
#: so an absolute threshold would place its ridge at row zero.
SCENES = [
    STOCK,
    {
        "key": "daylight",
        "label": "InterOS Daylight",
        "source": "home/02-home-1440x3200.jpg",
        "far_threshold": 120,
        "far_hold": 4,
        "near_threshold": 60,
        "near_hold": 3,
    },
    {
        "key": "night",
        "label": "InterOS Night",
        "source": "dark/03-dark-1440x3200.jpg",
        # Measured from the artwork by `brightest_row`; the night scene's haze
        # band is the brightest thing in it and the ridge is searched from below
        # it. See `brightest_row`.
        "far_threshold": 56,
        "far_hold": 3,
        "near_threshold": 30,
        "near_hold": 4,
    },
]

# `far` of the stock wallpaper, one entry per reference column. Traced from the
# reference lock-screen panel at a threshold of 180 and a four-row hold.
STOCK_FAR = [
    161, 161, 161, 160, 159, 159, 160, 159, 159, 158, 158, 157,
    156, 155, 154, 153, 152, 152, 153, 152, 152, 151, 150, 149,
    148, 147, 146, 146, 146, 145, 145, 144, 143, 143, 144, 145,
    146, 146, 148, 148, 148, 149, 151, 152, 153, 154, 155, 156,
    156, 156, 156, 157, 157, 158, 158, 157, 157, 157, 156, 157,
    157, 158, 159, 160, 160, 161, 162, 162, 163, 163, 164, 165,
    166, 167, 167, 166, 166, 166, 165, 165, 165, 164, 164, 163,
    163, 162, 162, 161, 161, 162, 163, 163, 164, 164, 164, 164,
    165, 165, 166, 166, 167, 166, 166, 166, 166, 165, 165, 164,
    163, 163, 163, 162, 161, 161, 160, 159, 158, 157, 156, 155,
    154, 153, 152, 152, 152, 152, 151, 152, 152, 152, 151, 151,
    150, 150, 149, 148, 147, 146, 145, 145, 146, 146, 147, 147,
    148, 149, 150, 151, 151, 151, 151, 152, 153, 154, 155, 155,
    155, 155, 154, 154, 154, 154, 154, 155, 155, 156, 156, 157,
    158, 159, 159, 159, 159,
]

# `near` of the stock wallpaper, one entry per reference column. Traced from the
# reference lock-screen panel at a threshold of 55 and a three-row hold. Columns
# 25..36 keep their traced values because the figure standing on the ridge, not
# the rock behind it, dominates that band.
STOCK_NEAR = [
    236, 240, 241, 240, 241, 241, 236, 236, 242, 238, 243, 243,
    243, 240, 244, 241, 244, 242, 245, 245, 245, 245, 246, 246,
    246, 220, 219, 218, 214, 213, 218, 218, 219, 222, 227, 249,
    249, 249, 250, 248, 249, 249, 250, 250, 250, 250, 250, 249,
    250, 250, 250, 250, 251, 251, 252, 252, 253, 254, 254, 255,
    257, 258, 259, 262, 270, 267, 268, 265, 266, 267, 267, 267,
    268, 268, 269, 269, 272, 270, 273, 277, 274, 271, 279, 275,
    275, 273, 276, 272, 272, 272, 272, 272, 271, 271, 272, 272,
    273, 273, 274, 274, 274, 276, 277, 277, 277, 280, 281, 281,
    282, 282, 282, 282, 282, 282, 282, 282, 283, 283, 284, 285,
    285, 284, 284, 285, 286, 290, 292, 288, 288, 288, 288, 287,
    287, 288, 289, 288, 289, 290, 290, 291, 292, 293, 293, 294,
    295, 296, 298, 301, 301, 301, 302, 303, 305, 307, 308, 309,
    309, 310, 310, 311, 311, 311, 311, 310, 310, 309, 310, 310,
    311, 312, 312, 313, 313,
]


def luminance(img: Image.Image) -> np.ndarray:
    a = np.asarray(img.convert("RGB")).astype("float32")
    return 0.299 * a[:, :, 0] + 0.587 * a[:, :, 1] + 0.114 * a[:, :, 2]


def sample(img: Image.Image, x0: float, x1: float, y0: float, y1: float):
    a = np.asarray(img.convert("RGB")).astype("float32")
    h, w, _ = a.shape
    reg = a[int(y0 * h): max(int(y0 * h) + 1, int(y1 * h)),
            int(x0 * w): max(int(x0 * w) + 1, int(x1 * w))]
    m = reg.reshape(-1, 3).mean(axis=0)
    return tuple(int(round(v)) for v in m)


def luma(rgb) -> float:
    r, g, b = rgb
    return 0.299 * r + 0.587 * g + 0.114 * b


def mix(a, b, t: float):
    return tuple(int(round(a[i] * (1 - t) + b[i] * t)) for i in range(3))


def ref_band_rgb(img: Image.Image, y0_ref: int, y1_ref: int):
    """Mean RGB of an artefact band given in *reference* rows."""
    a = np.asarray(img.convert("RGB")).astype("float32")
    h, w, _ = a.shape
    r0 = max(0, (y0_ref * h) // REF_H)
    r1 = min(h, max(r0 + 1, ((y1_ref * h) // REF_H) + 1))
    m = a[r0:r1, :].reshape(-1, 3).mean(axis=0)
    return tuple(int(round(v)) for v in m)


def skyline(lum: np.ndarray, threshold: float, hold: int, search_from: int = 0):
    h, w = lum.shape
    out = []
    for fx in range(REF_W):
        col = lum[:, min(w - 1, (fx * w) // REF_W)]
        found = 0
        for ry in range(max(search_from, 0), REF_H):
            if col[min(h - 1, (ry * h) // REF_H)] >= threshold:
                continue
            if all(
                col[min(h - 1, ((ry + k) * h) // REF_H)] < threshold
                for k in range(1, hold + 1)
            ):
                found = ry
                break
        out.append(found)
    return out


def brightest_row(lum: np.ndarray) -> int:
    """Reference row of the brightest band in the scene.

    WHY the search for a ridge starts here: in a daylight scene the sky is the
    brightest thing, so this is row zero and the ridge is found normally. In a
    night scene the sky is darker than the haze above the ridge, so an absolute
    threshold would fire on the sky itself; starting the search at the brightest
    row measures "where does the scene stop being the lit band" instead.
    """
    h, _ = lum.shape
    rows = [
        (ry, float(np.median(lum[min(h - 1, (ry * h) // REF_H), :])))
        for ry in range(REF_H)
    ]
    return max(rows, key=lambda r: r[1])[0]


def hexs(rgb) -> str:
    r, g, b = rgb
    return "0xFF%02X%02X%02X" % (max(0, min(255, r)), max(0, min(255, g)), max(0, min(255, b)))


def fmt_array(name: str, values, per_line: int = 12) -> str:
    body = []
    for i in range(0, len(values), per_line):
        body.append("    " + ", ".join(str(v) for v in values[i:i + per_line]) + ",")
    return f"pub const {name}: [u16; SCENE_REF_W] = [\n" + "\n".join(body) + "\n];\n"


def build_scene(spec: dict) -> dict:
    if spec.get("source") is None:
        scene = dict(spec)
        scene["far"] = STOCK_FAR
        scene["near"] = STOCK_NEAR
        # The stock stops are the theme's own, expressed as `0xAARRGGBB`.
        for stop in ("top", "mid", "bottom", "ink"):
            scene[stop] = "0x%08X" % int(spec[stop])
        return scene

    img = Image.open(WALLPAPER_DIR / spec["source"])
    lum = luminance(img)
    far = skyline(lum, spec["far_threshold"], spec["far_hold"],
                  brightest_row(lum))
    near = skyline(lum, spec["near_threshold"], spec["near_hold"],
                   brightest_row(lum))

    # The renderer clamps `near` at `far + 3`, so a column that never reaches the
    # near threshold collapses to a pure silhouette edge instead of inverting.
    near = [max(n, f + 3) for n, f in zip(near, far)]
    # Never trace below the last row: a skyline that runs off the bottom of the
    # panel is a measurement failure, not a mountain.
    bad = [i for i, v in enumerate(far) if v <= 0 or v >= REF_H - 2]
    if bad:
        sys.exit(
            f"{spec['source']}: {len(bad)} of {REF_W} columns did not resolve a "
            f"far skyline at the chosen thresholds (first at column {bad[0]})"
        )

    horizon_y = round(sum(far) / len(far))
    mist_y = round(sum(near) / len(near))
    # The rock line is where the foreground has finished darkening. Measured as
    # the first reference row below the mist line at which the column median has
    # reached the darkest stop the bottom band holds.
    foot = float(np.median(lum[int(0.80 * lum.shape[0]): int(0.95 * lum.shape[0]), :]))
    rock_y = REF_H - 4
    for ry in range(mist_y, REF_H - 2):
        band = lum[:, max(0, min(lum.shape[1] - 1, ((ry * lum.shape[1]) // REF_H)))]
        if float(np.median(band)) <= foot * 1.8:
            rock_y = ry
            break
    # Keep the stock relationship (a rock line ~86 rows below the mist line) so a
    # scene whose measured line is degenerate still has a readable foreground,
    # and never place the rock line at or above a traced near skyline: the
    # foreground band spans `near .. rock_y`, so a rock line under a peak there
    # would give the band a negative extent and the layer order would invert.
    rock_y = min(REF_H - 6, max(mist_y + 30, rock_y, mist_y + 34, max(near) + 6))

    # WHY each stop is sampled from the band it tints rather than from a fixed
    # fraction of the frame: the sky is 55% of the reference frame and the ridge
    # body only 20%, so fixed fractions bled one band's tone into the next. The
    # traced skylines say exactly where each band starts, and that is what is
    # sampled. The sky stop comes from its upper third because the lower two
    # thirds of the reference sky band are the warm haze in front of the ridge,
    # which the model reaches through its own horizon derivation.
    top = ref_band_rgb(img, 0, max(1, horizon_y // 3))
    mid = ref_band_rgb(img, horizon_y + 3, max(horizon_y + 4, mist_y - 3))
    bottom = ref_band_rgb(img, mist_y + 2, REF_H)
    # A near-white sky stop flattens the sky band, and a mid stop that is lighter
    # than the sky stop inverts the scene. Pull each toward its neighbour until
    # the ramp is monotonic, which is what keeps the ridge reading as ridge.
    if luma(mid) > luma(top):
        mid = mix(mid, top, 0.5)
    if luma(bottom) > luma(mid):
        bottom = mix(bottom, mid, 0.5)
    if luma(top) - luma(mid) < 24:
        mid = mix(mid, bottom, 0.25)

    return {
        "key": spec["key"],
        "label": spec["label"],
        "source": spec["source"],
        "top": hexs(top),
        "mid": hexs(mid),
        "bottom": hexs(bottom),
        # Ink for the sky region is chosen from the sky stop's own luminance: a
        # daylight scene has a light sky and needs dark type, a night scene the
        # reverse. Deriving it keeps a scene that is later retuned from ending up
        # with type that cannot be read against its own sky.
        "ink": hexs((255, 255, 255) if luma(top) < 128 else (17, 18, 20)),
        "horizon_y": horizon_y,
        "mist_y": mist_y,
        "rock_y": rock_y,
        "far": far,
        "near": near,
    }


def render(scenes) -> str:
    arrays = []
    entries = []
    for s in scenes:
        fname = f"SCENE_{s['key'].upper()}_FAR"
        nname = f"SCENE_{s['key'].upper()}_NEAR"
        arrays.append(fmt_array(fname, s["far"]))
        arrays.append(fmt_array(nname, s["near"]))
        label = s["label"].encode()
        entries.append(
            "    Scene {\n"
            f"        key: SCENE_KEY_{s['key'].upper()},\n"
            f"        name: b\"{s['label']}\",\n"
            f"        top: {s['top']},\n"
            f"        mid: {s['mid']},\n"
            f"        bottom: {s['bottom']},\n"
            f"        ink: {s['ink']},\n"
            f"        horizon_y: {s['horizon_y']},\n"
            f"        mist_y: {s['mist_y']},\n"
            f"        rock_y: {s['rock_y']},\n"
            f"        far: &{fname},\n"
            f"        near: &{nname},\n"
            "    },"
        )
    keys = "\n".join(
        f"pub const SCENE_KEY_{s['key'].upper()}: &[u8] = b\"{s['key']}\";"
        for s in scenes
    )
    tables = "\n\n".join(arrays)

    return f"""//! The InterOS wallpaper catalogue.
//!
//! WHY data and not images: the guest has no image decoder, and a photographic
//! 720x1440 wallpaper is 4 MB of asset the shell would have to carry inside its
//! own binary. What the reference artwork actually contributes is *where the
//! mountains are* and *what tone each layer is*, so that is what is stored: two
//! skylines traced per column at reference resolution, plus the three ramp
//! stops that tint them. Every colour relationship -- the lit ridge, the mist
//! band, the dark foreground -- is resolved from those stops at draw time, so
//! the catalogue can be retuned without re-tracing anything.
//!
//! GENERATED by `tools/gen_walls.py` from `Ui Samples/reference/wallpapers`.
//! Edit the generator, not this file.

/// Reference panel width, in reference pixels. This is the lock screen's own
/// measurement grid (`apps/launcher/src/screens/lock.rs`), so a trace taken at
/// this width lands on the same columns that surface renders.
pub const SCENE_REF_W: usize = {REF_W};

/// Reference panel height, in reference pixels.
pub const SCENE_REF_H: usize = {REF_H};

{keys}

{tables}

/// One wallpaper: its palette, its band boundaries and its two skylines.
///
/// `key` is what the settings store persists and `name` is what a human sees;
/// they are separate so the storage format can stay stable while the label is
/// reworded.
/// WHY PartialEq/Eq: a picker needs to ask whether the scene in front of it
/// is the one that is already applied, and a scene's identity is its key. Deriving
/// both from the fields -- which are all Copy scalars plus byte-string
/// references -- makes that comparison structural rather than by pointer.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Scene {{
    pub key: &'static [u8],
    pub name: &'static [u8],
    /// Sky stop, the top of the ramp.
    pub top: u32,
    /// Ridge-body stop, between the sky and the foreground.
    pub mid: u32,
    /// Foreground stop, the darkest tone the scene holds.
    pub bottom: u32,
    /// Ink for type drawn over the sky region.
    ///
    /// WHY it belongs to the scene and not to a theme: the sky region's
    /// brightness is the scene's own, so a night wallpaper needs light type and a
    /// daylight one dark type, and a theme carries no answer to that question.
    pub ink: u32,
    /// Reference row at which the sky ends and the far ridge begins.
    pub horizon_y: u16,
    /// Reference row at which the far ridge ends and the foreground begins.
    pub mist_y: u16,
    /// Reference row at which the foreground reaches its darkest stop.
    pub rock_y: u16,
    /// Far skyline, one reference row per reference column.
    pub far: &'static [u16; SCENE_REF_W],
    /// Near skyline, one reference row per reference column.
    pub near: &'static [u16; SCENE_REF_W],
}}

impl Scene {{
    /// The value the settings store persists by default.
    pub const DEFAULT_KEY: &'static [u8] = SCENE_KEY_STOCK;

    /// Looks a scene up by its persisted key.
    ///
    /// WHY `Option` and not a default fallback: a stored key the catalogue
    /// cannot resolve means the store and the binary disagree, and silently
    /// substituting a wallpaper would hide that instead of reporting it.
    pub fn by_key(key: &[u8]) -> Option<&'static Scene> {{
        SCENES.iter().find(|scene| scene.key == key)
    }}

    /// The scene a stored key resolves to, or the stock wallpaper.
    pub fn resolved(key: Option<&[u8]>) -> &'static Scene {{
        key.and_then(Scene::by_key).unwrap_or(&SCENES[0])
    }}

    /// Sky tone just above the far ridge, lifted from the sky stop toward the
    /// light. Derived rather than measured so a retuned ramp cannot go stale.
    pub fn horizon(&self) -> u32 {{
        blend(self.top, 0xFFFFFFFF, 150)
    }}

    /// Ridge body just below the far skyline: the ridge stop pulled toward the
    /// foreground stop, because rock in shadow is darker than the sky on it.
    pub fn shadow(&self) -> u32 {{
        blend(self.mid, self.bottom, 80)
    }}

    /// The mist band behind the foreground: the ridge stop lifted toward the
    /// light, because mist scatters it.
    pub fn mist(&self) -> u32 {{
        blend(self.mid, 0xFFFFFFFF, 150)
    }}
}}

/// Every wallpaper the device offers, in the order a picker lists them.
pub const SCENES: [Scene; {len(scenes)}] = [
{chr(10).join(entries)}
];

/// Blends two `0xAARRGGBB` values. `t` is 0..=256.
pub fn blend(a: u32, b: u32, t: u32) -> u32 {{
    let t = t.min(256);
    let inv = 256 - t;
    let mut out = 0u32;
    for shift in [24u32, 16, 8, 0] {{
        let ca = (a >> shift) & 0xFF;
        let cb = (b >> shift) & 0xFF;
        out |= ((ca * inv + cb * t) >> 8) << shift;
    }}
    out
}}

/// The tone of a three-stop vertical ramp at `t` in 0..=256.
///
/// This is the cheap path for a surface that wants the wallpaper's colour
/// without its silhouettes -- a thumbnail, a sheet background, a preview strip.
/// The lock screen draws the full composition; everything else reads this.
pub fn ramp3(scene: &Scene, t: u32) -> u32 {{
    let t = t.min(256);
    if t < 128 {{
        blend(scene.top, scene.mid, t * 2)
    }} else {{
        blend(scene.mid, scene.bottom, (t - 128) * 2)
    }}
}}

/// How far the sun lifts the right of the frame, as a fraction of the 0..=256
/// ramp scale out of a full `across` of 256 at the right edge.
pub const ACROSS_SPAN: u32 = 256;
const ACROSS_PERCENT: u32 = 26;
const ACROSS_REF_W: usize = SCENE_REF_W - 1;

/// The two ramp fractions for one pixel, folded into a single interpolation.
///
/// WHY they are folded instead of blended one after the other: the vertical ramp
/// and the horizontal sun lift both move a pixel toward the bright end of the
/// same band, so together they are one interpolation at
/// `1 - (1 - v)(1 - s)` rather than two blends in sequence. This is the only
/// full-bleed layer on the wallpaper, so it runs once per pixel, and folding
/// takes it from six channel blends per pixel to two.
fn band(vertical: u32, across: u32) -> u32 {{
    vertical
        .saturating_add(across)
        .saturating_sub((vertical.saturating_mul(across)) >> 8)
}}

/// The wallpaper at pixel `(x, y)` of a `w` x `h` surface.
///
/// This is the one implementation of the scene, shared by every surface that
/// draws a full-bleed wallpaper. A surface that wants something on top of it --
/// a figure, a veil, a card -- composes over the result.
pub fn scene_pixel(scene: &Scene, x: usize, y: usize, w: usize, h: usize) -> u32 {{
    if w == 0 || h == 0 {{
        return scene.bottom;
    }}
    let fx = (x * SCENE_REF_W) / w;
    let rel = ((y * SCENE_REF_H) / h) as u16;
    let far = scene.far[fx.min(SCENE_REF_W - 1)];
    // WHY the clamp: the two skylines are traced at two independent thresholds,
    // so a dark notch picked up by one can land above the layer it should sit
    // under. A minimum separation stops the layers inverting.
    let mut near = scene.near[fx.min(SCENE_REF_W - 1)];
    if near < far + 3 {{
        near = far + 3;
    }}
    let horizon_y = scene.horizon_y;
    let mist_y = scene.mist_y;
    let rock_y = scene.rock_y;

    // WHY a fraction of the ramp rather than a second measured column per band:
    // the artwork's left/right split was one photograph's exposure, and the stop
    // owns the exposure now. The sun is behind the far ridge on the right, so
    // every band is lifted toward its own bright end as the frame goes right.
    let across = ((fx * ACROSS_SPAN as usize) / ACROSS_REF_W.min(w.max(1) - 1)).min(256) as u32
        * ACROSS_PERCENT
        / 256;

    let horizon = scene.horizon();
    let shadow = scene.shadow();
    let mist = scene.mist();
    if rel < far {{
        let vertical = rel as u32 * 256 / horizon_y as u32;
        blend(scene.top, horizon, band(vertical, across))
    }} else if rel < near {{
        // WHY saturating: the traced far ridge is not level, so a pixel can be
        // below the ridge and above the nominal horizon height at the same
        // time, and a plain subtraction there underflows.
        let vertical =
            rel.saturating_sub(horizon_y) as u32 * 256 / (mist_y - horizon_y) as u32;
        blend(shadow, mist, band(vertical, across))
    }} else if (rel as usize) < rock_y as usize {{
        let vertical =
            rel.saturating_sub(mist_y) as u32 * 256 / (rock_y - mist_y) as u32;
        blend(mist, scene.bottom, band(vertical, across))
    }} else {{
        // The near rock itself: the ramp's darkest stop, held, with only the sun
        // lift on top. WHY it is the dark stop and not a blend toward it: below
        // the rock line the artwork is not a gradient at all, it is the shadowed
        // face of the foreground ridge.
        blend(scene.bottom, mist, across)
    }}
}}

{TESTS_BLOCK}
"""


TESTS_BLOCK = """
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_has_a_full_skyline() {
        for scene in SCENES.iter() {
            assert_eq!(scene.far.len(), SCENE_REF_W);
            assert_eq!(scene.near.len(), SCENE_REF_W);
            for column in 0..SCENE_REF_W {
                let far = scene.far[column];
                let near = scene.near[column];
                assert!(far > 0 && far < SCENE_REF_H as u16, "{:?}: bad far", scene.key);
                assert!(far <= scene.horizon_y + 40);
                // A near skyline below the far skyline is what keeps the two
                // silhouette layers from inverting, and one above the rock line
                // would give the foreground band a negative span.
                assert!(near >= far, "{:?}: near under far at column {}", scene.key, column);
                assert!(near < scene.rock_y, "{:?}: near above rock at column {}", scene.key, column);
            }
            assert!(scene.horizon_y < scene.mist_y, "{:?}: horizon under mist", scene.key);
            assert!(scene.mist_y < scene.rock_y, "{:?}: mist under rock", scene.key);
            assert!(scene.rock_y < SCENE_REF_H as u16, "{:?}: rock off the panel", scene.key);
        }
    }

    #[test]
    fn the_stock_scene_keeps_the_shipped_palette() {
        let stock = &SCENES[0];
        assert_eq!(stock.key, b"stock");
        assert_eq!(stock.top, 0xFF9CB8D6);
        assert_eq!(stock.mid, 0xFF6E7F8C);
        assert_eq!(stock.bottom, 0xFF241F1C);
        assert_eq!(stock.ink, 0xFF111214);
        assert_eq!(stock.horizon_y, 156);
        assert_eq!(stock.mist_y, 244);
        assert_eq!(stock.rock_y, 330);
    }

    #[test]
    fn ink_matches_the_sky_it_is_drawn_on() {
        for scene in SCENES.iter() {
            let sky_luma = 0.299 * ((scene.top >> 16) & 0xFF) as f32
                + 0.587 * ((scene.top >> 8) & 0xFF) as f32
                + 0.114 * (scene.top & 0xFF) as f32;
            assert_eq!(scene.ink >> 24, 0xFF, "{:?}: ink alpha", scene.key);
            if sky_luma < 128.0 {
                // A dark sky takes light ink.
                assert_eq!(
                    (scene.ink >> 16) & 0xFF,
                    0xFF,
                    "{:?}: dark sky, light ink",
                    scene.key
                );
            } else {
                assert_eq!(
                    (scene.ink >> 16) & 0xFF,
                    0x11,
                    "{:?}: light sky, dark ink",
                    scene.key
                );
            }
        }
    }

    #[test]
    fn a_stored_key_resolves_or_reports_itself_missing() {
        assert!(Scene::by_key(b"stock").is_some());
        assert!(Scene::by_key(b"night").is_some());
        assert!(Scene::by_key(b"polaroid").is_none());
        assert_eq!(Scene::resolved(None).key, b"stock");
        assert_eq!(Scene::resolved(Some(b"night")).key, b"night");
        // An unknown key falls back to stock rather than to the first scene by
        // accident, so the two behaviours are distinguishable.
        assert_eq!(Scene::resolved(Some(b"nope")).key, b"stock");
    }

    #[test]
    fn the_ramp_spans_both_ends_of_the_scene() {
        let stock = &SCENES[0];
        assert_eq!(ramp3(stock, 0), stock.top);
        assert_eq!(ramp3(stock, 256), stock.bottom);
        assert_eq!(ramp3(stock, 128), stock.mid);
    }

    #[test]
    fn the_composition_darkens_from_sky_to_foreground() {
        let luma = |c: u32| {
            0.299 * ((c >> 16) & 0xFF) as f32
                + 0.587 * ((c >> 8) & 0xFF) as f32
                + 0.114 * (c & 0xFF) as f32
        };
        for scene in SCENES.iter() {
            let row = |ref_row: u16| {
                ((ref_row as usize) * 1440 / SCENE_REF_H).min(1439)
            };
            // Sample the sky half way to the horizon and the floor just below
            // the rock line: those are the two bands every scene shares, so the
            // comparison holds whether the scene's own sky is brighter or darker
            // than its horizon derivation.
            let sky = scene_pixel(scene, 360, row(scene.horizon_y) / 2, 720, 1440);
            let floor = scene_pixel(scene, 360, row(scene.rock_y.saturating_add(24)), 720, 1440);
            assert!(
                luma(sky) > luma(floor),
                "{:?}: sky is not brighter than the floor",
                scene.key
            );
            // A scene whose sky stop already equals its horizon would draw a flat
            // sky, which the artwork does not have.
            assert_ne!(scene.horizon(), scene.top, "{:?}: flat sky", scene.key);
        }
    }

    #[test]
    fn every_sampled_pixel_stays_in_the_scene_palette() {
        for scene in SCENES.iter() {
            // A lattice across the surface: no coordinate may panic, and none
            // may fall outside the scene's own tonal range.
            for y in (0..1440usize).step_by(97) {
                for x in (0..720usize).step_by(53) {
                    let c = scene_pixel(scene, x, y, 720, 1440);
                    assert_eq!(c >> 24, 0xFF, "{:?}: pixel alpha", scene.key);
                }
            }
        }
    }
}
"""


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true",
                    help="fail if the generated module is stale")
    a = ap.parse_args()

    scenes = [build_scene(spec) for spec in SCENES]
    text = render(scenes)
    if a.check:
        if not OUT_PATH.exists():
            print(f"missing {OUT_PATH}", file=sys.stderr)
            return 1
        if OUT_PATH.read_text(encoding="utf-8") != text:
            print(f"stale {OUT_PATH}: re-run tools/gen_walls.py", file=sys.stderr)
            return 1
        print(f"{OUT_PATH.relative_to(ROOT)} is current")
        return 0
    OUT_PATH.write_text(text, encoding="utf-8")
    print(f"wrote {OUT_PATH.relative_to(ROOT)} ({len(scenes)} scenes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
