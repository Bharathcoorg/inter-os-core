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


def test_launcher_contains_no_removed_demo_workload_labels():
    source = (ROOT / "apps/launcher/src/main.rs").read_text()
    for label in ("Meeting with team (11:00)", "2 unread messages", "Mostly Sunny", "Finish design work"):
        assert label not in source


def test_touch_path_uses_canonical_router():
    source = (ROOT / "kernel/src/input/mod.rs").read_text()
    assert "pub static INPUT_ROUTER: Mutex<InputRouter>" in source
    assert "dispatch_pointer_at(pointer, 720, 24, ticks)" in source
    assert "TOUCH_ENGINE.lock()" not in source
    assert "pump_touch_events()" not in source


def test_canonical_gesture_kinds_have_wire_codes():
    source = (ROOT / "kernel/src/drivers/input.rs").read_text()
    assert "Back = 6" in source
    assert "Home = 7" in source
    assert "Recents = 8" in source
    assert "Cancel = 9" in source
    assert "Fling = 10" in source


def test_launcher_has_no_runtime_wallpaper_switcher():
    source = (ROOT / "apps/launcher/src/main.rs").read_text()
    assert "wallpaper_id" not in source
    assert "get_wallpaper_pixel" not in source
    assert "Wallpapers & Procedural Themes" not in source
    assert "Tap any theme to apply across system" not in source
    assert "InterOS Stock" in source


def test_launcher_uses_live_system_state_not_fake_home_state():
    source = (ROOT / "apps/launcher/src/main.rs").read_text()
    for field in ("task_done", "wallpaper_id", "wifi_on", "bt_on", "dark_mode", "media_playing", "notif_tab"):
        assert field not in source
    assert "notification_count" in source
    assert "refresh_notifications" in source
    assert "std::rtc_get_time" in source
    assert "System status" in source
    assert "InterOS Stock" in source


def test_input_event_poll_does_not_reactivate_legacy_touch_gesture_engine():
    source = (ROOT / "kernel/src/syscall/mod.rs").read_text()
    start = source.index("pub fn sys_input_event_poll")
    end = source.index("pub fn sys_touch_poll", start)
    assert "pump_touch_events()" not in source[start:end]


def test_studio_keyboard_contract_supports_text_and_navigation_keys():
    server = (ROOT / "emulator/server.py").read_text()
    app = (ROOT / "emulator/app.js").read_text()
    assert "INPUT_KEYCODE" in server
    assert "'ArrowUp'" in server and "'Escape'" in server
    assert "JSON.stringify({ key: e.key })" in app
    assert "ArrowDown" in app and "PageDown" in app


def test_kernel_keyboard_contract_encodes_canonical_keys():
    input_source = (ROOT / "kernel/src/input/mod.rs").read_text()
    driver = (ROOT / "kernel/src/drivers/input.rs").read_text()
    serial = (ROOT / "kernel/src/serial.rs").read_text()
    assert "pub fn post_key_event" in input_source
    assert "InputEventKind::Key" in driver
    assert "INPUT_KEYCODE" in serial


def test_studio_keyboard_uses_server_key_contract():
    source = (ROOT / "emulator/app.js").read_text()
    assert "body: JSON.stringify({ key: e.key })" in source
    assert "body: JSON.stringify({ code:" not in source


def test_system_gesture_directions_are_explicit():
    source = (ROOT / "kernel/src/input/router.rs").read_text()
    assert "1 if dx >= 40 => GestureKind::Back" in source
    assert "2 if dx <= -40 => GestureKind::Recents" in source
    assert "3 if dy <= -40 => GestureKind::Home" in source


def test_launcher_uses_real_registry_for_drawer_and_recent_surfaces():
    source = (ROOT / "apps/launcher/src/main.rs").read_text()
    assert "fn package_id_at" in source
    assert "fn recent_label" in source
    assert "let app_idx = state.app_drawer_page * 12 + r_idx * 3 + c_idx;" in source
    assert "resolve_app_in_drawer" not in source
    assert "4 apps . 2 tasks" not in source
    assert "Mail" not in source
    assert "Maps" not in source
    assert "Contacts" not in source


def test_launcher_drawer_pagination_is_registry_backed():
    source = (ROOT / "apps/launcher/src/main.rs").read_text()
    assert "let pages = (state.app_count + 11) / 12" in source
    assert "state.app_drawer_page += 1" in source
    assert "let app_idx = state.app_drawer_page * 12" in source


def test_first_party_media_and_browser_surfaces_do_not_bundle_demo_content():
    browser = (ROOT / "apps/browser/src/main.rs").read_text()
    gallery = (ROOT / "apps/gallery/src/main.rs").read_text()
    messages = (ROOT / "apps/messages/src/main.rs").read_text()
    music = (ROOT / "apps/music/src/main.rs").read_text()
    assert "pub static BOOKMARKS: [Bookmark; 0] = [];" in browser
    assert 'current_url: "about:blank"' in browser
    assert "GET /catalog.json" not in browser
    assert "pub static PHOTOS: [PhotoItem; 0] = [];" in gallery
    assert "Mountain Ridge" not in gallery
    assert "pub static THREADS: [ConversationThread; 0] = [];" in messages
    assert "Security Kernel" not in messages
    assert "pub static PLAYLIST: [AudioTrack; 0] = [];" in music
    assert "Sovereign Symphony" not in music
    assert "INTER_DMA_AUDIO_PCM_BLOCK" not in music


def test_studio_touch_endpoint_routes_live_uart_input():
    source = (ROOT / "emulator/server.py").read_text()
    assert "if self.path == '/api/touch/event':" in source
    assert "INPUT_TOUCH CANCEL" in source
    assert "SWIPE {gx} {gy} {gx2} {gy2} {steps}" in source
    assert "UART_COM1" in source


def test_studio_bootloader_branding_is_not_synthetic():
    source = (ROOT / "emulator/server.py").read_text()
    start = source.index("if self.path == '/api/themes/bootloader':")
    end = source.index("self.send_response(404)", start)
    block = source[start:end]
    assert "'status': 'DISABLED'" in block
    assert "set_active_theme(active)" not in block
