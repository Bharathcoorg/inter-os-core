import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def test_phone_profile_is_native_portrait():
    profile = json.loads((ROOT / "platform/x86_64/qemu-phone.json").read_text(encoding="utf-8"))
    assert profile["display"]["width"] == 720
    assert profile["display"]["height"] == 1440
    assert profile["display"]["backend"] == "virtio-vga-vnc"
    assert profile["input"]["coordinate_space"] == "portrait-logical-pixels"


def test_phone_profile_transport_ports_match_studio_defaults():
    profile = json.loads((ROOT / "platform/x86_64/qemu-phone.json").read_text(encoding="utf-8"))
    assert profile["endpoints"]["serial"].endswith(":4301")
    assert profile["endpoints"]["vnc"].endswith(":5901")
    assert profile["endpoints"]["websocket"].endswith(":6081")

def test_device_studio_x86_profile_matches_canonical_scanout():
    server = (ROOT / "emulator/server.py").read_text(encoding="utf-8")
    x86_start = server.index('if arch == "x86_64":')
    x86_end = server.index('elif arch == "aarch64":', x86_start)
    x86_profile = server[x86_start:x86_end]
    assert '"guest_width": 720, "guest_height": 1440' in x86_profile
    assert 'self.width = LOGICAL_SURFACE_W' in server
    assert 'self.height = LOGICAL_SURFACE_H' in server
    assert 'if self.height < 720 and check_launcher_ready()' not in server
