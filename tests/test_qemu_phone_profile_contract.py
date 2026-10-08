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
