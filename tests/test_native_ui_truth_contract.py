from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def test_messages_empty_state_has_non_overlapping_text_and_no_dead_action():
    source = read("apps/messages/src/main.rs")
    assert 'b"NO TRANSPORT"' in source
    assert 'b"Messaging unavailable: no transport"' in source
    assert 'b"P2P E2EE"' not in source
    assert 'b"Encrypted P2P Wire Client Active"' not in source
    assert 'b"Hardware Keystore Slot 3 Ready"' not in source
    # The scale-2 title occupies a 48px glyph line. Status lines must reserve
    # that space, and an unavailable backend must not expose a dead action.
    assert 'b"No Conversations Yet"' in source
    assert '740' in source
    assert '810' in source
    assert 'b"Messaging unavailable: no transport"' in source
    assert 'b"+ Start New Chat"' not in source


def test_file_manager_does_not_render_fabricated_clock_battery_or_storage_usage():
    source = read("apps/file_manager/src/main.rs")
    assert 'b"09:20"' not in source
    assert 'b"100%"' not in source
    assert 'b"42 GB / 128 GB"' not in source
    assert 'b"Capacity detected; usage unavailable"' in source
    assert 'b"Storage capacity unavailable"' in source


def test_device_studio_does_not_turn_presence_or_io_counters_into_fake_percentages():
    source = read("emulator/app.js")
    assert "fillRamBar.style.width = '0%'; // No guest RAM-usage metric is currently exposed." in source
    assert "fillBlockBar.style.width = '0%'; // IO counters do not measure utilization." in source
    assert "ops > 0 ? '100%' : '50%'" not in source


def test_device_studio_does_not_advertise_unobserved_display_modes_or_planes():
    source = read("emulator/index.html")
    assert "LTPO 120Hz VRR" not in source
    assert "0, 0, 720, 1280" not in source
    assert "Mobile Shell Activity Space" not in source
    assert "No live compositor-plane telemetry is currently exposed by the guest." in source


def test_browser_has_guest_pointer_and_keyboard_bridges():
    source = read("emulator/app.js")
    assert "touchSurface.addEventListener('pointerdown'" in source
    assert "touchSurface.addEventListener('pointermove'" in source
    assert "touchSurface.addEventListener('pointerup'" in source
    assert "touchSurface.addEventListener('pointercancel'" in source
    assert "window.addEventListener('keydown', async (e) =>" in source
    assert "fetch('/api/input/key'" in source
    assert "fetch('/api/touch/event'" in source


def test_settings_never_advertises_unobserved_hardware_or_governor_state():
    source = read("apps/settings/src/main.rs")
    for fabricated in (
        'b"SIMULATED"',
        'b"Active Profile: Balanced (1200 MHz)"',
        'b"Dynamic Voltage and Frequency Scaling Active"',
        'b"OPTIMAL"',
        'b"Hardware Status: Simulated Power Bus"',
        'b"Hardware Status: Simulated Sensor Bus"',
        'b"Hardware Status: Unavailable (No Radio)"',
        'b"Hardware Status: Unavailable (No SIM)"',
    ):
        assert fabricated not in source
    assert 'b"Battery telemetry unavailable"' in source
    assert 'b"DVFS state unavailable"' in source
    assert 'b"Temperature telemetry unavailable"' in source


def test_calendar_browser_and_gallery_do_not_draw_sample_status_values():
    calendar = read("apps/calendar/src/main.rs")
    browser = read("apps/browser/src/main.rs")
    gallery = read("apps/gallery/src/main.rs")
    assert 'b"Hardware RTC Civil Clock Active"' not in calendar
    assert 'b"09:20"' not in browser
    assert 'b"09:20"' not in gallery
    assert 'Fill level ~70%' not in browser


def test_touchpad_uses_valid_guest_contact_actions_and_reports_real_request_result():
    source = read("emulator/app.js")
    assert "action: 'down', coords: point.coords" in source
    assert "action: 'move', coords: point.coords" in source
    assert "sendGuestTouchEvent({ action, coords: point.coords" in source
    assert "type: gType" not in source
    assert "return request;" in source
    assert "touchSurface.focus({ preventScroll: true })" in source
    assert "document.activeElement !== touchSurface" in source
    assert "const target = [liveScanoutCanvas, liveScanoutImg, screenLiveScanout].find(visible);" in source
    assert "state.isLandscape ? (1280 / 720) : (720 / 1440)" in source
    assert "function guestPointerGeometry()" in source
    assert "coords: 'guest_framebuffer'" in source


def test_calculator_and_vault_do_not_paint_fabricated_status_or_success():
    calculator = read("apps/calculator/src/main.rs")
    vault = read("apps/vault/src/main.rs")
    assert 'b"09:20"' not in calculator
    assert 'b"100%"' not in calculator
    assert 'b"3,072 / 3,072 BYTES ALLOCATED - 100% HEALTH"' not in vault
    assert 'b"Hardware Master Key & Multi-Sector Ledger Online"' not in vault
    assert 'b"AES-256-XTS & ChaCha20-Poly1305 Verified"' not in vault
    assert "pub fn generate_iv(&self) -> Option<[u8; 16]>" in vault
    assert "None\n    }" in vault
    assert "[0xA5; 16]" not in vault
    assert "let Some(iv) = self.generate_iv() else" in vault


def test_settings_security_privacy_and_rtc_are_not_assumed_successful():
    source = read("apps/settings/src/main.rs")
    for fabricated in (
        'b"Cryptographic Root Key Sealed in Hardware"',
        'b"Sectors 200-205 Cryptographically Sealed"',
        'b"Anti-Tracking Query & Header Sanitizer Active"',
        'b"Civil Wall-Clock Synchronization Active"',
        'b"RTC SYNC"',
    ):
        assert fabricated not in source
    assert 'b"Keystore status unavailable"' in source
    assert 'b"Vault encryption status unavailable"' in source
    assert 'b"Privacy filter status unavailable"' in source


def test_native_keyboard_events_are_decoded_and_used_by_browser_and_calculator():
    sdk = read("sdk/rust/interos-std/src/lib.rs")
    browser = read("apps/browser/src/main.rs")
    calculator = read("apps/calculator/src/main.rs")
    assert "pub const INPUT_EVENT_KIND_KEY: u8 = 11;" in sdk
    assert "pub fn key_event(&self) -> Option<InputKeyEvent>" in sdk
    assert "if let Some(key) = input.key_event()" in browser
    assert "pub address_focused: bool" in browser
    assert "fn handle_browser_key(app: &mut BrowserApp, key: std::InputKeyEvent)" in browser
    # WHY two strings: the responsive rewrite (f7f2b64) split the old combined
    # "Navigation unavailable: no network transport" literal into a headline
    # plus a wrapped body; both halves are asserted so the honest outcome
    # (parsed, attempted, nothing sent, nothing loaded) stays pinned.
    assert '(b"Navigation unavailable", BODY_NO_TRANSPORT)' in browser
    assert "no network transport is bound to this app" in browser
    assert "if let Some(key) = input.key_event()" in calculator
    assert "fn handle_keyboard_key(calc: &mut CalculatorState, code: u32) -> bool" in calculator


def test_layout_fixes_reserve_full_glyph_line_height():
    settings = read("apps/settings/src/main.rs")
    calendar = read("apps/calendar/src/main.rs")
    file_manager = read("apps/file_manager/src/main.rs")
    messages = read("apps/messages/src/main.rs")
    # WHY these three settings assertions changed: the Settings app was rebuilt
    # as the design system's "System Space" (light category grid + stack
    # detail screens), so the old dark tab-layout coordinates ("60, 270, 1"
    # etc.) no longer exist. The invariant they pinned is unchanged and still
    # asserted here on the new geometry: every detail row reserves the bold
    # title's full 48px glyph line (band starts at cy+16), the caption sits a
    # full 24px line below it (cy+72), and rows with a second caption line
    # reserve that line too (cy+98) -- i.e. no text band is allowed to start
    # inside the line box above it.
    assert "cy + ROW_TITLE_DY," in settings
    assert "cy + ROW_CAP_DY," in settings
    assert "cy + ROW_CAP2_DY," in settings
    assert "centered_x(detail_str, 1, 24, 672)" in calendar
    assert "760," in calendar
    assert "y >= 800 && y < 856" in calendar
    assert 'test_text(x, y, 48, 64, 3, b"Files"' in file_manager
    assert 'b"Messaging unavailable: no transport"' in messages


def test_device_studio_does_not_infer_guest_cpu_state_from_qemu_liveness():
    source = read("emulator/app.js")
    assert "QEMU / BARE-METAL" not in source
    assert "CPU state unavailable" in source
    assert "CPU telemetry reports active" in source
    assert "data.cpu?.thread_id || 'Active'" not in source
    assert "data.cpu?.rip || 'Offline'" not in source


def test_unavailable_settings_services_use_neutral_not_success_styling():
    source = read("apps/settings/src/main.rs")
    assert 'b"DVFS state unavailable", TEXT_MUTED' in source
    assert 'b"Keystore status unavailable", TEXT_MUTED' in source
    assert 'b"Vault encryption status unavailable", TEXT_MUTED' in source
    assert 'b"Privacy filter status unavailable", TEXT_MUTED' in source
    assert 'b"Keystore status unavailable", SUCCESS_EMERALD' not in source
    assert 'b"Vault encryption status unavailable", SUCCESS_EMERALD' not in source
    assert 'b"Privacy filter status unavailable", SUCCESS_EMERALD' not in source
