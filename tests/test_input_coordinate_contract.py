# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def test_kernel_declares_coordinate_spaces():
    source = (ROOT / "kernel/src/input/coordinate.rs").read_text(encoding="utf-8")
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
    source = (ROOT / "emulator/server.py").read_text(encoding="utf-8")
    assert "unknown coordinate space" in source
    assert "logical_portrait" in source
    assert "guest_framebuffer" in source
    assert "is_portrait = data.get('coords')" not in source


def test_studio_post_handler_declares_orientation_global():
    source = (ROOT / "emulator/server.py").read_text(encoding="utf-8")
    post = source[source.index("def do_POST"):]
    # The rotation endpoint inside do_POST assigns current_device_orientation.
    # Without a global declaration the name became function-local, so the
    # landscape/guest_framebuffer touch branch (which reads it) raised
    # UnboundLocalError on every landscape contact.
    assert "global serial_lines, current_device_orientation" in post
    assert post.index("global serial_lines, current_device_orientation") < post.index(
        "current_device_orientation = 'landscape'"
    )


def test_browser_studio_uses_explicit_spaces():
    source = (ROOT / "emulator/app.js").read_text(encoding="utf-8")
    assert "coords: 'portrait'" in source
    assert "coords: 'guest_framebuffer'" in source


def test_launcher_no_longer_uses_fixed_clock_literal():
    source = (ROOT / "apps/launcher/src/main.rs").read_text(encoding="utf-8")
    assert "CLOCK_TIME" not in source
    assert "std::rtc::Reading::now()" in source
    assert 'b"Meeting with team (11:00)"' not in source
    assert 'b"2 unread messages"' not in source
    assert 'b"Mostly Sunny"' not in source

def test_studio_exposes_only_stock_theme():
    source = (ROOT / "emulator/server.py").read_text(encoding="utf-8")
    assert "'themes': [active]" in source
    assert "Theme switching is disabled" in source
    assert "Custom themes are disabled" in source


def test_launcher_clock_reads_named_rtc_fields():
    """The shell's clock is named fields out of the kernel's RTC payload.

    WHY this now reads two files: the launcher, Notes, Calendar and the Agent
    tool each carried their own scan of the same payload, with three different
    sets of bounds, so one hardware reading printed three different times. The
    scan moved into `interos_std::rtc`, which is where the named-field rule and
    the no-default rule are pinned today. The launcher half of the assertion is
    that it asks that reader and nothing else.
    """
    reader = (ROOT / "sdk/rust/interos-std/src/lib.rs").read_text(encoding="utf-8")
    for name in ("year", "month", "day", "hour", "minute"):
        assert f'field(data, b"\\"{name}\\"")?,' in reader, (
            f"the shared clock reader must demand \"{name}\" and refuse the "
            "whole reading without it"
        )
    assert "rtc_get_time(&mut buf)" in reader
    # The defect this replaces: a reader that defaulted an absent field to zero
    # printed 00:00 for a payload that never reported a time at all.
    assert "unwrap_or(0)" not in reader[reader.index("fn parse(data"):reader.index("fn field(data")]

    source = (ROOT / "apps/launcher/src/main.rs").read_text(encoding="utf-8")
    assert "std::rtc::Reading::now()" in source
    assert 'for &b in data' not in source[source.index("fn refresh_clock"):source.index("fn refresh_notifications")]


def test_serial_touch_protocol_matches_studio_transport():
    source = (ROOT / "kernel/src/serial.rs").read_text(encoding="utf-8")
    assert 'INPUT_TOUCH' in source
    assert 'let Some(slot) = parts.next().and_then(|v| v.parse::<u8>().ok())' in source
    assert 'post_touch_event(slot, px, py' in source


def test_launcher_contains_no_removed_demo_workload_labels():
    source = (ROOT / "apps/launcher/src/main.rs").read_text(encoding="utf-8")
    for label in ("Meeting with team (11:00)", "2 unread messages", "Mostly Sunny", "Finish design work"):
        assert label not in source


def test_touch_path_uses_canonical_router():
    source = (ROOT / "kernel/src/input/mod.rs").read_text(encoding="utf-8")
    assert "pub static INPUT_ROUTER: Mutex<InputRouter>" in source
    assert "dispatch_pointer_at(pointer, 720, 24, ticks)" in source
    assert "TOUCH_ENGINE.lock()" not in source
    assert "pump_touch_events()" not in source


def test_canonical_gesture_kinds_have_wire_codes():
    source = (ROOT / "kernel/src/drivers/input.rs").read_text(encoding="utf-8")
    assert "Back = 6" in source
    assert "Home = 7" in source
    assert "Recents = 8" in source
    assert "Cancel = 9" in source
    assert "Fling = 10" in source


def test_launcher_has_no_runtime_wallpaper_switcher():
    source = (ROOT / "apps/launcher/src/main.rs").read_text(encoding="utf-8")
    assert "wallpaper_id" not in source
    assert "get_wallpaper_pixel" not in source
    assert "Wallpapers & Procedural Themes" not in source
    assert "Tap any theme to apply across system" not in source
    assert "InterOS Stock" in source


def test_launcher_uses_live_system_state_not_fake_home_state():
    source = (ROOT / "apps/launcher/src/main.rs").read_text(encoding="utf-8")
    for field in ("task_done", "wallpaper_id", "wifi_on", "bt_on", "dark_mode", "media_playing", "notif_tab"):
        assert field not in source
    assert "notification_count" in source
    assert "refresh_notifications" in source
    assert "std::rtc::Reading::now()" in source
    # WHY this no longer pins the words "System status": they were a comment in this
    # file, and the card they named is now headed "Today" where it is painted. A
    # contract that reads prose fails when somebody rewords the prose, and it never
    # proved anything about the state. What makes the surface live is the reader that
    # fills its rows and the heading the person sees over them, so those are pinned.
    assert "fn refresh_system_readings" in source
    home = (ROOT / "apps/launcher/src/screens/home.rs").read_text(encoding="utf-8")
    assert 'b"Today"' in home, "the card the live readings fill no longer names itself"
    assert "InterOS Stock" in source


def test_input_event_poll_does_not_reactivate_legacy_touch_gesture_engine():
    source = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
    start = source.index("pub fn sys_input_event_poll")
    end = source.index("pub fn sys_touch_poll", start)
    assert "pump_touch_events()" not in source[start:end]


def test_studio_keyboard_contract_supports_text_and_navigation_keys():
    server = (ROOT / "emulator/server.py").read_text(encoding="utf-8")
    app = (ROOT / "emulator/app.js").read_text(encoding="utf-8")
    assert "INPUT_KEYCODE" in server
    assert "'ArrowUp'" in server and "'Escape'" in server
    assert "JSON.stringify({ key: e.key })" in app
    assert "ArrowDown" in app and "PageDown" in app


def test_kernel_keyboard_contract_encodes_canonical_keys():
    input_source = (ROOT / "kernel/src/input/mod.rs").read_text(encoding="utf-8")
    driver = (ROOT / "kernel/src/drivers/input.rs").read_text(encoding="utf-8")
    serial = (ROOT / "kernel/src/serial.rs").read_text(encoding="utf-8")
    assert "pub fn post_key_event" in input_source
    assert "InputEventKind::Key" in driver
    assert "INPUT_KEYCODE" in serial
    # The console used to answer the name from a second, hand-written number table
    # and had Enter and Backspace the wrong way round in it. That table sat below
    # the handler the dispatcher asks first, so nothing ever reached it while the
    # correct table above answered the verb. It is gone, and one table remains.
    assert serial.count('eq_ignore_ascii_case("INPUT_KEYCODE")') == 1
    assert '"Enter" => 0x' not in serial and '"Backspace" => 0x' not in serial
    assert '"ENTER" => crate::input::KeyCode::Enter' in serial
    assert '"BACKSPACE" => crate::input::KeyCode::Backspace' in serial
    # And the numbers those names finally arrive at the application with are the
    # numbers the SDK hands out, or a swap elsewhere would undo the table's fix.
    assert "KeyCode::Enter => 0x10000" in driver
    assert "KeyCode::Backspace => 0x10002" in driver
    sdk = (ROOT / "sdk/rust/interos-std/src/lib.rs").read_text(encoding="utf-8")
    assert "pub const ENTER: u32 = 0x10000" in sdk
    assert "pub const BACKSPACE: u32 = 0x10002" in sdk


def test_studio_keyboard_uses_server_key_contract():
    source = (ROOT / "emulator/app.js").read_text(encoding="utf-8")
    assert "body: JSON.stringify({ key: e.key })" in source
    assert "body: JSON.stringify({ code:" not in source


def test_system_gesture_directions_are_explicit():
    source = (ROOT / "kernel/src/input/router.rs").read_text(encoding="utf-8")
    # The bounds were consolidated into one named constant when the edge
    # classifier was extracted (same value, same three directions), so assert
    # both the constant and each explicit arm against it.
    assert "const EDGE_SWIPE_MIN: i32 = 40;" in source
    assert "1 if dx >= EDGE_SWIPE_MIN => GestureKind::Back" in source
    assert "2 if dx <= -EDGE_SWIPE_MIN => GestureKind::Recents" in source
    assert "3 if dy <= -EDGE_SWIPE_MIN => GestureKind::Home" in source


def test_launcher_uses_real_registry_for_drawer_and_recent_surfaces():
    source = (ROOT / "apps/launcher/src/main.rs").read_text(encoding="utf-8")
    drawer = (ROOT / "apps/launcher/src/screens/app_drawer.rs").read_text(encoding="utf-8")
    assert "fn package_id_at" in source
    assert "fn recent_label" in source
    # The shell no longer re-derives the drawer grid: a tap is answered by the
    # same geometry the screen paints with, and that hit test resolves the slot
    # through the installed-package catalogue.
    assert "app_drawer_hit(state, x, y).and_then(|idx| package_id_at(state, idx))" in source
    assert "let (start, end) = layout::page_slice(count, capacity, app_drawer_page(state));" in drawer
    assert "Some(filtered[start + slot])" in drawer
    assert "resolve_app_in_drawer" not in source
    assert "4 apps . 2 tasks" not in source
    assert "Mail" not in source
    assert "Maps" not in source
    assert "Contacts" not in source


def test_launcher_drawer_pagination_is_registry_backed():
    source = (ROOT / "apps/launcher/src/main.rs").read_text(encoding="utf-8")
    drawer = (ROOT / "apps/launcher/src/screens/app_drawer.rs").read_text(encoding="utf-8")
    assert "let pages = (state.app_count + 11) / 12" in source
    assert "state.app_drawer_page += 1" in source
    # Page extent and the index range shown on it are counted from the registry,
    # not from a fixed twelve-item assumption inside the hit test.
    assert "layout::pages(count, app_drawer_per_page(state))" in drawer
    assert "state.app_drawer_page.min(pages - 1)" in drawer


def test_app_drawer_open_paths_clear_the_filter_that_moves_every_slot():
    source = (ROOT / "apps/launcher/src/main.rs").read_text(encoding="utf-8")
    openings = []
    cursor = 0
    while True:
        at = source.find("App Drawer opened", cursor)
        if at < 0:
            break
        openings.append(source[max(0, at - 900):at])
        cursor = at + 1
    # WHY this pins the reset: a tap in a drawer cell is answered through
    # filtered_indices(state, state.app_drawer_tab, ...), so which package a
    # fixed coordinate launches depends on the filter that was active the last
    # time the drawer was open. The lifecycle suites browse a category chip and
    # then, in the next pass, tap (359,256) expecting the Calculator they
    # measured there -- and the guest answered with the Calendar, because the
    # drawer silently reopened on the Work tab. Opening the drawer from Home is
    # a fresh appointment with the whole catalogue, so every open path has to
    # clear the category, the page and the search in the same breath.
    assert len(openings) >= 2, "the launcher no longer logs both drawer-open paths"
    for block in openings:
        for reset in (
            "state.app_drawer_tab = 0;",
            "state.app_drawer_page = 0;",
            "state.search_query_len = 0;",
        ):
            assert reset in block, f"an App Drawer open path is missing {reset}"


def test_launcher_home_taps_are_answered_only_by_painted_tiles():
    source = (ROOT / "apps/launcher/src/main.rs").read_text(encoding="utf-8")
    routing = source[source.index("fn resolve_app_at_touch"):source.index("fn resolve_app_at_touch") + 6_000]
    # WHY this pins absence rather than presence: a fixed 40x40 coordinate
    # matrix, left over from a harness that addressed tiles by number, ran ahead
    # of the painted geometry. The deck moved and the boxes did not, so a tap on
    # Phone opened the Browser and a tap on the Today card opened Settings. The
    # boxes are gone; nothing that addresses a slot by number may return.
    for stale in (
        "(410..=450).contains(&x) && (240..=280).contains(&y)",
        "(390..=430).contains(&x) && (1160..=1200).contains(&y)",
        "(530..=570).contains(&x) && (1160..=1200).contains(&y)",
        "(840..=880).contains(&y)",
        "let r_idx = if (180..=398).contains(&y)",
        "automated test matrix (targeted",
    ):
        assert stale not in routing
    # The two readings that remain are the painted deck and the painted dock.
    assert "screens::home::home_hit(state, x, y)" in routing
    assert "(795..1010).contains(&y)" in routing


def test_first_party_media_and_browser_surfaces_do_not_bundle_demo_content():
    browser = (ROOT / "apps/browser/src/main.rs").read_text(encoding="utf-8")
    gallery = (ROOT / "apps/gallery/src/main.rs").read_text(encoding="utf-8")
    messages = (ROOT / "apps/messages/src/main.rs").read_text(encoding="utf-8")
    music = (ROOT / "apps/music/src/main.rs").read_text(encoding="utf-8")
    assert "pub static BOOKMARKS: [Bookmark; 0] = [];" in browser
    # WHY this form: the responsive rewrite (f7f2b64) initializes the address
    # buffer with `let start = b"about:blank"` copied into `current_url`
    # instead of the old struct literal, so the old
    # `current_url: "about:blank"` text is gone while the behavior is kept
    # (verified: the address bar reads about:blank on-device).
    assert 'let start = b"about:blank";' in browser
    assert "GET /catalog.json" not in browser
    assert "pub static PHOTOS: [PhotoItem; 0] = [];" in gallery
    assert "Mountain Ridge" not in gallery
    assert "pub static THREADS: [ConversationThread; 0] = [];" in messages
    assert "Security Kernel" not in messages
    assert "pub static PLAYLIST: [AudioTrack; 0] = [];" in music
    assert "Sovereign Symphony" not in music
    assert "INTER_DMA_AUDIO_PCM_BLOCK" not in music


def test_studio_touch_endpoint_routes_live_uart_input():
    source = (ROOT / "emulator/server.py").read_text(encoding="utf-8")
    assert "if self.path == '/api/touch/event':" in source
    assert "INPUT_TOUCH CANCEL" in source
    assert "SWIPE {gx} {gy} {gx2} {gy2} {steps}" in source
    assert "UART_COM1" in source


def test_studio_bootloader_branding_is_not_synthetic():
    source = (ROOT / "emulator/server.py").read_text(encoding="utf-8")
    start = source.index("if self.path == '/api/themes/bootloader':")
    end = source.index("self.send_response(404)", start)
    block = source[start:end]
    assert "'status': 'DISABLED'" in block
    assert "set_active_theme(active)" not in block


def test_first_party_apps_do_not_ship_synthetic_user_content():
    browser = (ROOT / "apps/browser/src/main.rs").read_text(encoding="utf-8")
    messages = (ROOT / "apps/messages/src/main.rs").read_text(encoding="utf-8")
    music = (ROOT / "apps/music/src/main.rs").read_text(encoding="utf-8")
    assert 'interlayer://sovereign.mesh/catalog' not in browser
    assert '127.0.0.1:4200/telemetry' not in browser
    assert 'Meeting at 11:00 AM' not in messages
    assert 'Security Kernel' not in messages
    assert 'Sovereign Symphony in C' not in music
    assert 'INTER_DMA_AUDIO_PCM_BLOCK' not in music
