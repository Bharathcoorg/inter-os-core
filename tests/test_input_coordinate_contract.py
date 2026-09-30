from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def test_kernel_declares_coordinate_spaces():
    source = (ROOT / "kernel/src/input/coordinate.rs").read_text()
    for name in (
        "DevicePhysical",
        "StudioDisplay",
        "GuestFramebuffer",
        "LogicalPortrait",
        "LogicalLandscape",
    ):
        assert name in source
    assert "maps_corners_without_heuristics" in source


def test_studio_declares_every_touch_coordinate_space():
    source = (ROOT / "emulator/server.py").read_text()
    assert "unknown coordinate space" in source
    assert "logical_portrait" in source
    assert "guest_framebuffer" in source
    assert "is_portrait = data.get('coords')" not in source


def test_browser_studio_uses_explicit_spaces():
    source = (ROOT / "emulator/app.js").read_text()
    assert "coords: 'portrait'" in source
    assert "coords: 'guest_framebuffer'" in source


def test_launcher_no_longer_uses_fixed_clock_literal():
    source = (ROOT / "apps/launcher/src/main.rs").read_text()
    assert "CLOCK_TIME" not in source
    assert "std::rtc_get_time" in source
    assert 'b"Meeting with team (11:00)"' not in source
    assert 'b"2 unread messages"' not in source
    assert 'b"Mostly Sunny"' not in source

def test_studio_exposes_only_stock_theme():
    source = (ROOT / "emulator/server.py").read_text()
    assert "'themes': [active]" in source
    assert "Theme switching is disabled" in source
    assert "Custom themes are disabled" in source


def test_launcher_clock_reads_named_rtc_fields():
    source = (ROOT / "apps/launcher/src/main.rs").read_text()
    assert 'field(data, b"\\\"hour\\\"")' in source
    assert 'field(data, b"\\\"minute\\\"")' in source
    assert 'for &b in data' not in source[source.index("fn refresh_clock"):source.index("fn refresh_notifications")]


def test_serial_touch_protocol_matches_studio_transport():
    source = (ROOT / "kernel/src/serial.rs").read_text()
    assert 'INPUT_TOUCH' in source
    assert 'let Some(slot) = parts.next().and_then(|v| v.parse::<u8>().ok())' in source
    assert 'post_touch_event(slot, px, py' in source
