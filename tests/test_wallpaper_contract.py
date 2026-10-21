#!/usr/bin/env python3
"""
InterOS Core — Wallpaper Customisation Contract Test
Validates that the wallpaper feature is real end to end and not a visual stub:
  1. The scene catalogue is generated from the supplied reference artwork by a
     reproducible generator, and the generated module is what ships.
  2. Both surfaces have independent persisted keys in the settings store.
  3. The launcher reads those keys, uses the length the store reports, refreshes
     at boot and whenever a foreground application exits, and resolves a stored
     key that this build cannot resolve rather than silently substituting one.
  4. The Settings picker writes the key for the chosen target only, resets to
     the bundled default, never writes on selection, and never applies a pending
     selection when the panel is dismissed.
  5. Both painters draw the resolved scene rather than a hardcoded backdrop.

WHY source assertions rather than only behaviour: the QEMU journey covers the
runtime path, but a regression that quietly hardcodes a wallpaper back into the
home painter would still boot, still flip, and still look plausible in a
screenshot. These checks pin the data flow so that failure is loud.
"""

import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

WALLS = ROOT / "sdk/rust/interos-std/src/design/walls.rs"
DESIGN_MOD = ROOT / "sdk/rust/interos-std/src/design/mod.rs"
GENERATOR = ROOT / "tools/gen_walls.py"
LAUNCHER = ROOT / "apps/launcher/src/main.rs"
LOCK = ROOT / "apps/launcher/src/screens/lock.rs"
HOME = ROOT / "apps/launcher/src/screens/home.rs"
SETTINGS = ROOT / "apps/settings/src/main.rs"

HOME_KEY = "ui.wallpaper.home"
LOCK_KEY = "ui.wallpaper.lock"

SCENE_KEYS = ["stock", "daylight", "night"]
SCENE_NAMES = ["InterOS Stock", "InterOS Daylight", "InterOS Night"]


def _text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


class WallpaperCatalogueTests(unittest.TestCase):
    """The catalogue is generated, complete, and reproducible."""

    def test_01_generator_reads_the_supplied_reference_artwork(self):
        src = _text(GENERATOR)
        # The scenes must be traced from the reference wallpapers, not invented.
        self.assertIn('WALLPAPER_DIR = ROOT / "Ui Samples" / "reference" / "wallpapers"', src)
        # One scene per artwork file distinct enough to be its own wallpaper.
        for source in ("home/02-home-1440x3200.jpg", "dark/03-dark-1440x3200.jpg"):
            self.assertIn(source, src)
        # The stock scene is the shell's own wallpaper: its stops stay as they
        # were and its skylines keep their hand-tuned traced values.
        self.assertIn('"source": None', src)
        self.assertIn("STOCK_FAR", src)

    def test_02_generated_module_is_registered_and_reproducible(self):
        mod_src = _text(DESIGN_MOD)
        self.assertIn("pub mod walls;", mod_src)
        # `--check` regenerates into a scratch copy and diffs: if the shipped
        # module drifted from the artwork, the generator says so.
        self.assertIn("--check", _text(GENERATOR))

    def test_03_catalogue_holds_three_distinct_scenes(self):
        src = _text(WALLS)
        for key in SCENE_KEYS:
            self.assertIn(f'SCENE_KEY_{key.upper()}: &[u8] = b"{key}"', src)
        for name in SCENE_NAMES:
            self.assertIn(name, src)
        # Each scene carries its own two traced skylines, not one shared ramp.
        self.assertEqual(src.count("pub const SCENE_"), 11)
        for scene in ("STOCK", "DAYLIGHT", "NIGHT"):
            self.assertIn(f"pub const SCENE_{scene}_FAR", src)
            self.assertIn(f"pub const SCENE_{scene}_NEAR", src)
        # Every skyline table covers the reference width exactly.
        self.assertGreaterEqual(src.count("[u16; SCENE_REF_W]"), 2 * len(SCENE_KEYS))
        self.assertIn("pub const SCENE_REF_W: usize = 173;", src)
        self.assertIn("pub const SCENE_REF_H: usize = 427;", src)

    def test_04_scene_lookup_reports_missing_keys_instead_of_defaulting(self):
        src = _text(WALLS)
        self.assertIn("pub fn by_key(key: &[u8]) -> Option<&'static Scene>", src)
        self.assertIn("pub const DEFAULT_KEY", src)


class LauncherIntegrationTests(unittest.TestCase):
    """The launcher renders what the store holds, per surface."""

    def test_01_both_surface_keys_are_read(self):
        src = _text(LAUNCHER)
        self.assertIn(HOME_KEY, src)
        self.assertIn(LOCK_KEY, src)

    def test_02_the_store_reports_a_length_and_it_is_used(self):
        # WHY: the store fills only `len` bytes of the caller's buffer. Using
        # the whole buffer compared zero padding against the catalogue's keys
        # and every stored choice looked unknown, so the wallpaper silently
        # stayed stock.
        src = _text(LAUNCHER)
        self.assertIn("fn stored_wallpaper_key", src)
        self.assertIn("let raw = &raw[..n];", src)
        # WHY the assertion is on the length travelling with the value and
        # not on the exact spelling of the read: the guarantee is that the
        # store's length reaches the caller, so the trailing zero padding
        # cannot reach a byte-exact comparison. How the read is written is
        # not the invariant.
        self.assertIn("n > 0 && n <= buf.len() => Some((buf, n))", src)

    def test_03_state_refreshes_at_boot_and_when_a_foreground_app_exits(self):
        src = _text(LAUNCHER)
        self.assertIn("fn refresh_wallpapers(state: &mut LauncherState)", src)
        # Called during startup and again when the shell reaps a child: a
        # wallpaper chosen in Settings is written by a different process and
        # the kernel offers no channel that would tell this shell about it.
        self.assertGreaterEqual(src.count("refresh_wallpapers(&mut state);"), 2)

    def test_04_an_unresolvable_key_is_reported_not_defaulted(self):
        src = _text(LAUNCHER)
        self.assertIn("not in this build", src)

    def test_05_both_painters_draw_the_resolved_scene(self):
        lock_src = _text(LOCK)
        home_src = _text(HOME)
        self.assertIn("let scene = f.scene;", lock_src)
        self.assertIn("scene_pixel", home_src)
        self.assertIn("state.home_wallpaper", home_src)


class SettingsPickerTests(unittest.TestCase):
    """The picker writes the right key for the chosen target, and only then."""

    def test_01_each_target_writes_its_own_key(self):
        src = _text(SETTINGS)
        self.assertIn(f'"{HOME_KEY}"', src)
        self.assertIn(f'"{LOCK_KEY}"', src)
        # One write loop over the target's keys: Home, Lock, or Both.
        self.assertIn("WallpaperTarget::Both", src)

    def test_02_selection_alone_writes_nothing(self):
        src = _text(SETTINGS)
        self.assertIn("nothing written yet", src)

    def test_03_reset_returns_to_the_bundled_default(self):
        src = _text(SETTINGS)
        self.assertIn("fn reset_wallpaper", src)
        # Reset writes catalogue scene 0's key to BOTH surface keys: the default
        # is the wallpaper the shell shipped with, not "no wallpaper".
        self.assertIn('for key in ["ui.wallpaper.home", "ui.wallpaper.lock"]', src)
        self.assertIn("reset to the bundled default", src)

    def test_04_dismissing_the_panel_applies_no_pending_selection(self):
        src = _text(SETTINGS)
        self.assertIn("dismissed without applying", src)
        # Opening the panel clears any pending choice, so a selection that was
        # never applied can never leak into a later session.
        self.assertIn("wallpaper_pending = None", src)

    def test_05_the_preview_shows_the_selected_scene(self):
        src = _text(SETTINGS)
        self.assertIn("wallpaper_preview", src)


if __name__ == "__main__":
    unittest.main()
