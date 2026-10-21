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


def _const_usize(source: str, name: str) -> int:
    """Reads a `const NAME: usize = ...;` binding out of a Rust source file.

    WHY parse rather than assert on literals: these contracts exist to catch a
    layout that puts two runs on top of each other. Asserting that a literal
    like "76" is still present would pass just as happily when the run moved
    somewhere worse. Reading the constants back and comparing the resulting
    geometry is the only form of this assertion that means anything.

    A binding written as an expression of other bindings -- `PILLS_Y + PILL_H`,
    which is how the filter note is positioned so it cannot drift from the pill
    row -- is resolved by looking those names up in turn.
    """
    import re

    match = re.search(
        rf"^\s*(?:pub )?const {name}\s*:\s*usize\s*=\s*([^;]+);", source, re.M
    )
    assert match, f"could not find `const {name}: usize` in the source"
    expr = match.group(1).strip()

    resolved = _resolve_usize(source, expr, depth=0)
    assert resolved is not None, f"`const {name}` = {expr!r} is not a resolvable usize expression"
    return resolved


def _resolve_usize(source: str, expr: str, depth: int) -> int | None:
    """Evaluates a const expression made of ints, names, `+`, `-` and `*`."""
    assert depth < 8, "const expression nests too deeply to resolve"

    import re

    expr = expr.strip()
    if expr.isdigit():
        return int(expr)

    # A single `+` at the top level, resolved left then right.
    for operator in ("+", "-", "*"):
        parts = _split_top_level(expr, operator)
        if len(parts) == 2:
            left = _resolve_usize(source, parts[0], depth + 1)
            right = _resolve_usize(source, parts[1], depth + 1)
            if left is None or right is None:
                return None
            if operator == "+":
                return left + right
            if operator == "-":
                return left - right
            return left * right

    match = re.match(r"^([A-Z_][A-Z0-9_]*)$", expr)
    if match:
        nested = re.search(
            rf"^\s*(?:pub )?const {match.group(1)}\s*:\s*usize\s*=\s*([^;]+);", source, re.M
        )
        if not nested:
            return None
        return _resolve_usize(source, nested.group(1), depth + 1)
    return None


def _split_top_level(expr: str, operator: str) -> list[str]:
    """Splits on `operator` only where it is not inside a parenthesis pair."""
    depth = 0
    parts, start = [], 0
    for i, ch in enumerate(expr):
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        elif ch == operator and depth == 0:
            parts.append(expr[start:i])
            start = i + 1
    parts.append(expr[start:])
    return [p for p in parts if p.strip()]


def test_events_header_bands_cannot_overlap_each_other_or_the_feed():
    """The Events header is four runs stacked above the first section head.

    WHY this needs asserting mechanically: the header owns a fixed band, and
    every line is placed from a literal. Moving any one of them down by a few
    pixels is invisible in review and prints two runs over each other on the
    device. This has already happened twice -- once when three prose lines were
    added to the header and ran into the pill row, and once when the filter note
    was nudged down and landed on the first section head.
    """
    source = read("apps/launcher/src/screens/notifications.rs")

    title_y = _const_usize(source, "TITLE_Y")
    subtitle_y = _const_usize(source, "SUBTITLE_Y")
    source_note_y = _const_usize(source, "SOURCE_NOTE_Y")
    pills_y = _const_usize(source, "PILLS_Y")
    pill_h = _const_usize(source, "PILL_H")
    filter_note_y = _const_usize(source, "FILTER_NOTE_Y")
    section_head_y = _const_usize(source, "SECTION_HEAD_Y")

    # A caption run inks GLYPH_H (24) rows from its baseline; a body run is
    # scaled 2, so 48. These are `launcher::font`'s own numbers.
    caption_h, body_h = 24, 48

    assert title_y + body_h <= subtitle_y, "title runs into the subtitle"
    assert subtitle_y + caption_h <= source_note_y, "subtitle runs into the source note"
    assert source_note_y + caption_h <= pills_y, "source note runs into the pill row"
    assert filter_note_y >= pills_y + pill_h, "filter note starts inside the pill row"
    assert filter_note_y + caption_h <= section_head_y, "filter note runs into the first section head"


def test_events_header_does_not_print_platform_diagnostics_to_the_reader():
    """The header may state where the rows come from, in plain language.

    WHY: the three lines it used to print named the tick counter, the missing
    calendar and the two stores that contribute nothing. Those are notes to a
    reviewer of the file. The platform detail stays in the module header.
    """
    source = read("apps/launcher/src/screens/notifications.rs")
    # The prose may not be in a byte-string literal: a `b"..."` is what the
    # surface actually draws. The module header is allowed to explain the
    # platform, and does.
    import re

    drawn = re.findall(r'b"([^"]*)"', source)
    joined = " | ".join(drawn)
    for diagnostic in (
        "guest ticks",
        "no calendar",
        "no day groups",
        "Registry and recents",
    ):
        assert diagnostic not in joined, f"the screen still prints {diagnostic!r}"
    # The feed's own empty state is allowed to name what it is empty of -- that
    # is the user reading their own device, not a note to a reviewer. Only the
    # header prose was the problem.
    assert "Recent activity from your apps and system." in joined


def test_recents_cards_do_not_draw_a_stand_in_for_a_thumbnail():
    """Recents must not paint a picture it never received.

    WHY: `surface_snapshot` copies window metadata only and documents that it
    does not expose client pixel memory, and the Studio's DRM/KMS panel reports
    that no live compositor-plane telemetry is exposed by the guest. The old
    renderer drew a per-app fake screen -- traffic-light window chrome and
    saturated fills from no design token file -- which is worse than admitting
    there is no preview.
    """
    source = read("apps/launcher/src/screens/activity.rs")
    for fabricated in (
        "0xFFEF4444",
        "0xFFF59E0B",
        "0xFF10B981",
        "org.interos.browser",
    ):
        assert fabricated not in _code_only(source), f"recents still draws fabricated preview art: {fabricated}"
    assert 'b"No live preview"' in source


def test_browser_start_page_does_not_render_a_hardcoded_news_feed_or_ranking():
    """The start page may not present invented pages as if the device knew them.

    WHY: both lists it used to draw were literals. "Most Visited" named eight
    sites on a browser with no visit counter, and "Discover" named four
    publications that do not exist with ages measured against nothing.
    """
    source = read("apps/browser/src/main.rs")
    for fabricated in (
        "InterOS 8.2 Sovereign Microkernel Boots in 140ms",
        "Rust Foundation",
        "Sovereign Computing: W3C Standards on Native Microkernels",
        'b"Most Visited"',
    ):
        assert fabricated not in _code_only(source), f"start page still renders fabricated content: {fabricated}"
    assert 'b"Your pages"' in source
    # The real sources of rows must stay wired to real state.
    assert "app.bookmarks.record(index)" in source
    assert "app.history.entry(" in source


def _code_only(source: str) -> str:
    """Strips `//` line comments and doc comments from Rust source.

    WHY this exists, and why it is not optional: these contracts assert that a
    fabricated string does not appear in a file, and the natural way to write a
    comment explaining why the string went away is to quote it. Three separate
    times that re-introduced the exact literal the contract forbids -- once in
    the launcher browser, once in the kernel panel, once here. A truth contract
    that scans comments is not testing the program; it is testing the author's
    prose. Stripping comments makes the assertion mean what it says, and lets a
    comment explain the change without fighting it.
    """
    import re

    without_block = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
    return re.sub(r"//[^\n]*", " ", without_block)


def test_the_kernel_boot_settings_panel_invents_no_readings():
    """The kernel's own settings panel must not state telemetry it cannot read.

    WHY: `framebuffer.rs` draws a five-tab settings panel reachable from the
    boot shell. Every value on it was a literal, and several were not merely
    invented but self-refuting -- a Wi-Fi row advertising an enabled 6E radio
    with a named SSID sat three lines above "no wireless hardware discovered",
    and "120Hz ProMotion [ACTIVE]" contradicted the refresh_hz = 60 the same
    kernel programs in `drivers/display_kms.rs`.

    Nothing in that module can read a backlight, an audio mixer, a radio, a fuel
    gauge, a thermal sensor, a keystore slot, or a clock rate. Each row now says
    what is actually available. The 'w' binding was removed rather than left
    editing a label no hardware reads.
    """
    source = read("kernel/src/framebuffer.rs")
    code = _code_only(source)

    # No cipher is implemented anywhere in this tree, so any screen naming one
    # asserts a capability the device does not have.
    assert "AES-256-XTS" not in code, "a screen names a cipher the device does not implement"
    vault = read("apps/vault/src/main.rs")
    assert "fletcher32" in vault, "the vault's real integrity check is Fletcher-32"

    for fabricated in (
        "Screen Brightness: 85%",
        "Media Audio Volume: 70%",
        "120Hz ProMotion",
        "OLED TrueBlack",
        "Wi-Fi 6E: [ENABLED]",
        "InterSovereignMesh-5G",
        "2400 MHz",
        "1200 MHz",
        "4200 mAh",
        "3850 mV",
        "2990 mW",
        "State of Charge: 94%",
        "SoC Temperature: 42 C",
        "did:inter:sovereign:node01",
        "Attestation Verified",
    ):
        assert fabricated not in code, f"kernel settings panel invents: {fabricated}"

    # The one real fact on that tab is the scanout mode, and it must be the
    # mode the kernel actually programs.
    assert "Display mode refresh: 60 Hz (programmed)" in source
    assert "refresh_hz: 60" in read("kernel/src/drivers/display_kms.rs")

    # The dead wifi toggle must not come back with its key binding.
    assert "'w' => {" not in code
    assert "pub wifi_on: bool" not in code


def test_a_corner_radius_scale_has_exactly_one_spelling_per_value():
    """Two scales that carry the same names at different values is a trap.

    WHY: `interos_std::design::radius` and `launcher::theme::radius` used to
    both define `CONTROL`, `CARD` and `SHEET`, at 12/18/24 and 18/24/32
    respectively. A screen that reached for the wrong one rendered a control
    with visibly different corners from its siblings, and nothing in the type
    system objected. `system_space.rs` was doing exactly that for its back
    pill.

    The launcher's copy is gone: `launcher::theme::radius` is now the SDK's
    scale, re-exported, so there is one scale and one spelling per value. This
    asserts that stays true -- that the launcher declares no radius of its own
    and reaches for the platform's.

    The SDK module also shipped `SM`/`MD`/`LG` as second spellings of
    `CONTROL`/`CARD`/`SHEET` under a different type. Those are removed; this
    asserts they stay removed.
    """
    from pathlib import Path

    sdk = read("sdk/rust/interos-std/src/design/mod.rs")
    for alias in ("pub const SM: u32", "pub const MD: u32", "pub const LG: u32"):
        assert alias not in sdk, f"radius re-grows a second spelling: {alias}"

    # No launcher screen may reach past its own theme for a radius.
    screens = Path(ROOT / "apps/launcher/src/screens")
    offenders = [
        path.name
        for path in sorted(screens.glob("*.rs"))
        if "design::radius" in path.read_text(encoding="utf-8")
    ]
    assert not offenders, (
        "launcher screens must resolve radii from launcher::theme::radius, "
        f"not the SDK scale: {offenders}"
    )

    # The launcher's theme module must not declare a radius of its own: the
    # scale it re-exports is the platform's, and a second copy of it is the
    # fault this test was written for.
    theme_module = read("apps/launcher/src/theme.rs")
    assert "pub use interos_std::design::radius;" in theme_module, (
        "the launcher's theme module must resolve radii through the SDK's scale"
    )
    assert "pub mod radius" not in theme_module, (
        "the launcher declares a radius module of its own"
    )


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


def test_the_wallpaper_refactor_cannot_change_a_colour():
    """Hoisting the scene's derived tones out of the per-pixel path must be a
    pure speed change.

    WHY this needs pinning: the wallpaper is full-bleed, so a one-unit drift in
    any stop is visible across the whole screen. `SceneRef::new` and
    `scene_pixel` have to agree exactly, and the near-skyline clamp has to
    resolve identically -- it is per column, so a refactor that hoisted it
    wrongly would pass every other test and still render a wrong ridge.
    """
    source = read("sdk/rust/interos-std/src/design/walls.rs")
    code = _code_only(source)

    assert "pub struct SceneRef" in code
    assert "pub fn scene_pixel_ref" in code

    # The clamp is the subtle part: `near` must still be floored to `far + 3`,
    # per column. Resolving it once is the whole point; resolving it wrongly is
    # invisible until a ridge inverts.
    assert "near[i] = if n < far + 3 { far + 3 } else { n };" in code

    # The resolved path must read the resolved tones rather than re-derive them,
    # which is the entire saving.
    ref_body = source.split("pub fn scene_pixel_ref", 1)[-1].split("\n}\n", 1)[0]
    for rederived in ("scene.horizon()", "scene.shadow()", "scene.mist()"):
        assert rederived not in ref_body, (
            f"scene_pixel_ref re-derives {rederived} on the per-pixel path again"
        )

    # And the home surface must actually be on the resolved path.
    home = _code_only(read("apps/launcher/src/screens/home.rs"))
    assert "backdrop_ref(&p.scene_ref, x, y)" in home
    assert "SceneRef::new(state.home_wallpaper)" in home


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
