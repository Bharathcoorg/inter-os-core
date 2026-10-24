import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def test_phone_profile_is_native_portrait():
    profile = json.loads((ROOT / "platform/x86_64/qemu-phone.json").read_text(encoding="utf-8"))
    assert profile["display"]["width"] == 720
    assert profile["display"]["height"] == 1440
    assert profile["display"]["backend"] == "std-vga-vnc"
    assert profile["display"]["authoring_space"]["width"] == 720
    assert profile["display"]["authoring_space"]["height"] == 1440
    assert profile["display"]["density_dpi"] == 320
    assert profile["display"]["scale_factor_permille"] == 1000
    assert profile["display"]["refresh_rate_hz"] == 60
    # The status bar is 48 logical px and the scanout is 1:1 with the authoring
    # space, so the safe inset is 48 px, not the 48 dp x 1.5 = 72 px the retired
    # 1080x2160 scanout needed.
    assert profile["display"]["safe_insets_px"]["top"] == 48
    assert profile["display"]["safe_insets_px"]["bottom"] == 48
    assert profile["input"]["coordinate_space"] == "portrait-logical-pixels"


def test_phone_profile_transport_ports_match_studio_defaults():
    profile = json.loads((ROOT / "platform/x86_64/qemu-phone.json").read_text(encoding="utf-8"))
    assert profile["endpoints"]["serial"].endswith(":4301")
    assert profile["endpoints"]["vnc"].endswith(":5901")
    assert profile["endpoints"]["websocket"].endswith(":6081")

def test_device_studio_x86_profile_matches_canonical_scanout():
    profile = json.loads((ROOT / "platform/x86_64/qemu-phone.json").read_text(encoding="utf-8"))
    width = profile["display"]["width"]
    height = profile["display"]["height"]
    server = (ROOT / "emulator/server.py").read_text(encoding="utf-8")
    # Strip comments so a doc comment quoting the geometry cannot satisfy this
    # check on behalf of the launch path it is supposed to verify.
    code = "\n".join(
        line for line in server.splitlines() if not line.lstrip().startswith("#")
    )
    assert f'"guest_width": {width}, "guest_height": {height}' in code
    assert f"SCANOUT_W, SCANOUT_H = {width}, {height}" in code
    assert "edid_w, edid_h = device_guest_geometry()" in code
    assert 'f"VGA,edid=on,xres={edid_w},yres={edid_h},vgamem_mb=32"' in code
    assert 'self.width = LOGICAL_SURFACE_W' in server
    assert 'self.height = LOGICAL_SURFACE_H' in server
    assert 'if self.height < 720 and check_launcher_ready()' not in server
