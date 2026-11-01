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

    # The rows each band reserves: `font::line_height` at the rung that band
    # draws at -- ascent plus descent, which is the box `LabelRun` clips its ink
    # to. The old pair here was the bitmap cell's (24 for a caption, 48 for a
    # body), which under-reserved the HEADING title by 32 rows and so passed the
    # very overlap this test was written to catch.
    #
    # WHY the numbers are written down rather than computed: the authority for
    # them is the Rust side, where `TITLE_ADVANCE` and `SUBTITLE_ADVANCE` are
    # each pinned by a `const _: () = assert!(...)` to the SDK's own
    # `line_height` at the rung the paint site uses. Changing a rung therefore
    # fails the build, not just this file.
    heading_h, body_h, caption_h = 80, 32, 22

    assert title_y + heading_h <= subtitle_y, "title runs into the subtitle"
    assert subtitle_y + body_h <= source_note_y, "subtitle runs into the source note"
    assert source_note_y + caption_h <= pills_y, "source note runs into the pill row"
    assert filter_note_y >= pills_y + pill_h, "filter note starts inside the pill row"
    assert filter_note_y + caption_h <= section_head_y, "filter note runs into the first section head"

    # The section heading and the first card of its own group.
    #
    # WHY this is a separate pair of assertions: the header runs above all end
    # at the pill row's note, and the feed below begins at a card. Between them
    # sits a heading with a line box of its own, which nothing here used to
    # account for. The advance from the heading to its cards was `space::MD`
    # measured from the heading's box *top*, so the card began six rows before
    # the box ended -- and with no clipper on this surface the card won the
    # pixels and printed the heading's baseline out. "Last hour" reached the
    # device missing its bottom four rows.
    label_box_h = _const_usize(source, "SECTION_LABEL_BOX_H")
    head_advance = _const_usize(source, "SECTION_HEAD_ADVANCE")
    row_h = _const_usize(source, "ROW_H")
    feed_bottom = _const_usize(source, "FEED_BOTTOM")

    assert label_box_h == caption_h, (
        "the section heading's box is no longer the caption line box this test assumes"
    )
    assert head_advance > label_box_h, (
        "the section advance is measured from the heading's box top, so the card "
        "starts inside the heading it names"
    )
    assert section_head_y + head_advance + row_h <= feed_bottom, (
        "the first section's card does not fit between its own heading and the action bar"
    )


def test_the_shade_asks_to_clear_its_notifications_once():
    """One destructive control, in one place, on the notifications shade.

    WHY this needs a gate: the shade carried two pills that both read "Clear
    all" and both emptied the event store -- the header's own, and a wider one
    fixed at row 702 in the middle of the feed. Nothing on the surface says
    which of two identical answers the machine meant, and the second one sat
    below a feed that fits two cards, so it read as an item in the list rather
    than as a control over it.

    WHY a source gate: the shell is `#![no_main]` and its bin target carries
    `test = false`, so a `#[cfg(test)]` module under `screens/` never runs on
    the host. The label the painter draws is the detectable fact -- that string
    appears once per control that carries it.
    """
    code = _code_only(read("apps/launcher/src/screens/notifications.rs"))
    assert code.count('b"Clear all"') == 1, (
        "the shade draws the same destructive question more than once"
    )
    # A second rectangle laid out for it would be a second control even if its
    # label were reworded, so the old binding's name is what stays banned. The
    # feed's own edge is pinned against the home indicator by a `const _: () =
    # assert!(...)` in the source, which is where the screen height lives.
    assert "CLEAR_BTN" not in code.replace("HEADER_CLEAR_BTN", ""), (
        "a second clear control is still laid out on this surface"
    )


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


def test_recents_cards_show_the_real_pixels_or_say_why_there_are_none():
    """Recents must not paint a picture it never received.

    WHY this changed: the old renderer drew a per-app fake screen -- traffic
    lights, a keypad, chat bubbles, saturated fills from no design token file
    -- and put "No live preview" in the corner of the drawing, so the surface
    showed a convincing picture of an app that had never been seen. That was
    the fiction the contract was written against.

    The rework replaced the fiction with the platform's own pixels. The kernel
    holds every client's surface, and a client that holds the
    `SurfaceCapture` capability may read a running app's surface; the shell
    holds that capability and the task switcher shows what it gets. When the
    platform will not answer -- the app has not drawn a frame, the app has no
    surface, the capture was refused -- the card names the reason instead of
    drawing anything.

    So the contract is now: the stand-in renderer is gone, the pixels come
    from a capture, and the card has honest wording for the case where there
    is nothing to show.
    """
    source = read("apps/launcher/src/screens/activity.rs")
    for fabricated in (
        "0xFFEF4444",
        "0xFFF59E0B",
        "0xFF10B981",
        "org.interos.browser",
    ):
        assert fabricated not in _code_only(source), f"recents still draws fabricated preview art: {fabricated}"
    # The per-app stand-in renderer is gone: there is no archetype any more.
    assert "render_mini_preview" not in _code_only(source)
    # The picture comes from the platform, through the capture capability.
    assert "std::surface_capture" in _code_only(source)
    # And the card says so when there is no picture to show.
    assert 'b"No preview"' in source
    assert "the app has not drawn a frame yet" in source
    assert "the shell may not read this app's screen" in source


def test_the_search_offers_only_destinations_that_exist():
    """The search must not offer a destination that is not there.

    WHY: the launcher's search field said it searched "apps, settings & files"
    and it did not. Tapping it opened a launcher-drawn copy of the Settings
    app's own page under a second name -- "System Space" -- so the result set
    held an entry that re-opened the Settings app while claiming to be
    something else. A result that opens something other than what its label
    says is a result that lies.

    So the contract is now: there is no "System Space" result, the apps come
    from the package registry, the settings surfaces come from the platform's
    own catalogue, the files come from the filesystem listing, and each
    category says whether it matched, found nothing, or could not be read.
    """
    search = read("apps/launcher/src/screens/search.rs")
    shell = read("apps/launcher/src/main.rs")
    for source in (search, shell):
        assert "System Space" not in _code_only(source), "the search still offers the System Space duplicate"
    # The three sources are the platform's own.
    assert "std::package_registry" in _code_only(shell)
    assert "std::fs_list" in _code_only(shell)
    assert "interos_std::settings::Section::ALL" in _code_only(shell)
    # And the model-backed ranking seam is marked as such.
    assert "pub trait Ranker" in read("apps/launcher/src/search.rs")
    assert "fn search_ranker()" in _code_only(shell)


def _fn_body(source: str, signature: str) -> str:
    """The braced body that follows `signature`, as far as its own braces close.

    WHY this exists: a gate written against a whole file can be satisfied from
    a comment or from a different function, and the launcher screens carry both
    in quantity. Cutting the one function out keeps the gate about the code.
    """
    at = source.find(signature)
    assert at >= 0, f"the source no longer has {signature}"
    open_at = source.find("{", at)
    assert open_at >= 0, f"{signature} has no body"
    depth = 0
    for index in range(open_at, len(source)):
        char = source[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source[open_at + 1 : index]
    raise AssertionError(f"{signature} has an unbalanced body")


def test_the_search_fields_magnifier_draws_a_lens_and_not_a_hook():
    """The query field's pictogram has to read as the thing it names.

    WHY: the lens was a radius band with a quadrant test bolted onto it --
    `d2 >= 64 && d2 <= 144 && dx <= 0 && dy <= 0` -- so three quarters of the
    circle were never painted, and the handle was a
    `|dx - 8| + |dy - 8| <= 10` band, which fills a lozenge rather than drawing
    a stem. On the device the field showed a stray arc sitting over a blob,
    while the home screen's field one tap upstream -- the same control, opening
    the same query -- drew a closed ring over a diagonal stem. A pictogram that
    changes shape between the two screens that carry it reads as a rendering
    fault, not as a style.

    WHY a source gate: the shell is `#![no_main]` and its bin target carries
    `test = false`, so nothing inside `screens/` can be reached from a host
    test. The detector is the quadrant itself -- a ring band that names `dx` or
    `dy` at all is an arc, and a stem that does not bound `dx - dy` is a blob.
    """
    search = _code_only(read("apps/launcher/src/screens/search.rs"))
    body = _fn_body(search, "fn magnifier")
    ring = [line for line in body.splitlines() if line.strip().startswith("if") and "d2" in line]
    assert ring, "the lens is no longer a radius band, so this gate has lost its subject"
    for line in ring:
        assert "dx" not in line and "dy" not in line, (
            "the lens band is quadrant-limited, so the ring cannot close: "
            f"{line.strip()}"
        )
    assert "(dx - dy).abs()" in body, (
        "the handle is no longer a diagonal stem, so it is filling a shape again"
    )


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
    # WHY the visit log is read through `visit` rather than the older `entry`:
    # a row now shows what the page called itself beside where it was, and the
    # accessor that hands out only an address cannot feed that row.
    assert "app.history.visit(" in source


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


def test_calendar_reports_the_clock_once_per_civil_date_not_once_per_tick():
    """`sync_rtc` runs from the timer branch, so its console lines need a gate.

    WHY this needs pinning: one stress pass carried 383 copies of the same
    "RTC payload" line from two Calendar tasks, because the function printed
    the payload and a success line every time the clock answered -- roughly
    twice a second, forever. The serial console is the suites' only instrument,
    and burying it pushed real input answers seconds late. A civil date is
    news once, and a clock failure is reported once until the state changes.
    """
    source = read("apps/calendar/src/main.rs")
    body = source.split("pub fn sync_rtc(&mut self) -> bool {")[1].split("\n    pub fn ")[0]
    assert "if rtc_report_is_new(self.rtc_reported_date, today)" in body
    assert body.count("self.rtc_unknown_reported") >= 2
    assert "fn report_unknown_state(&mut self" in source
    # The payload may only reach the console inside that gate.
    payload_log = 'std::log("[CALENDAR APP] RTC payload as reported by the kernel:")'
    assert payload_log in body
    assert body.index("if rtc_report_is_new(self.rtc_reported_date, today)") < body.index(payload_log)


def test_calendar_browser_and_gallery_do_not_draw_sample_status_values():
    calendar = read("apps/calendar/src/main.rs")
    browser = read("apps/browser/src/main.rs")
    gallery = read("apps/gallery/src/main.rs")
    assert 'b"Hardware RTC Civil Clock Active"' not in calendar
    assert 'b"09:20"' not in browser
    assert 'b"09:20"' not in gallery
    assert 'Fill level ~70%' not in browser


def test_calendar_event_cards_do_not_name_a_location_that_was_never_stored():
    """An event card may not print a venue the platform never recorded.

    WHY this needs pinning: every card used to print the invented string
    "Vault Sector 203 Encrypted" as its subtitle, in every view, so a made-up
    place was presented as real event data. The stored record carries a civil
    date, a wall-clock time and a title, and no location field at all, so the
    card has to report the absence of one instead.
    """
    main = read("apps/calendar/src/main.rs")
    lib = read("apps/calendar/src/lib.rs")

    # The fabricated venue is gone from the renderer, and the app source
    # holds no literal of it at all.
    assert 'b"Vault Sector 203 Encrypted"' not in main
    assert "Vault Sector 203 Encrypted" not in lib

    # The card draws its subtitle from the record, so the renderer holds no
    # literal of its own there.
    assert "event_location(item)" in main
    assert 'b"No location"' in lib


def test_calendar_day_view_arrows_point_the_way_they_move_the_day():
    """The calendar's chevrons point the way the row they sit on actually goes.

    WHY this needs pinning: the left chevron was drawn with its apex on the
    right and the right chevron with its apex on the left, so the arrow on
    the left pointed at the next day and the arrow on the right pointed back
    a day. The glyph direction and the day the tap handler selects now come
    from the same named values, and the row indicators in the month, week,
    day and agenda views were drawn the same mirrored way.
    """
    main = read("apps/calendar/src/main.rs")
    lib = read("apps/calendar/src/lib.rs")

    # The header arrows are drawn through the shared chevron helper with an
    # explicit direction.
    assert (
        "chevron_pixel(x, y, DAY_PREV_ARROW_APEX_X, DAY_PREV_ARROW_APEX_Y, ChevronDir::Left)"
        in main
    )
    assert (
        "chevron_pixel(x, y, DAY_NEXT_ARROW_APEX_X, DAY_NEXT_ARROW_APEX_Y, ChevronDir::Right)"
        in main
    )

    # The tap handler steps the day with the same directions.
    assert "step_selected_day(d, DayArrow::Previous, max_d)" in main
    assert "step_selected_day(d, DayArrow::Next, max_d)" in main

    # The month, week, day and agenda rows carry no navigation arrow of their
    # own - their chevron points forward into the row it opens. Every one of
    # them goes through the shared helper with an explicit direction, because
    # the hand-rolled arm tests they used to carry measured dx away from a
    # left-hand apex and drew a '<' on a row that opens forward.
    assert "dx >= 0 && dx <= 8" not in main
    assert main.count("chevron_pixel(x, y, 658,") == 4
    for view in (
        "fn week_view_pixel",
        "fn day_view_pixel",
        "fn agenda_view_pixel",
        "fn event_list_pixel",
    ):
        body = main.split(view)[1].split("\nfn ")[0]
        assert "ChevronDir::Right" in body, f"{view} lost its row chevron"

    # The shared helper keeps the direction in the signature.
    assert "pub enum ChevronDir" in lib
    assert "pub fn chevron_pixel" in lib
    assert "pub fn step_selected_day" in lib


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
    assert "state.isLandscape" in source
    assert "? { width: 1440, height: 720, coords: 'guest_framebuffer' }" in source
    assert ": { width: 720, height: 1440, coords: 'portrait' }" in source
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
    #
    # WHY the call takes the plan and not the scene reference: the plan is
    # what carries the wallpaper the user chose, and a document wallpaper has
    # to be paintable by the same four painters that paint the scene. The
    # resolved path itself is unchanged: the plan's `scene_ref` is still what
    # `scene_pixel_ref` reads, and the resolved tones are still not
    # re-derived per pixel.
    home = _code_only(read("apps/launcher/src/screens/home.rs"))
    assert "backdrop_ref(p, x, y)" in home
    assert "scene_pixel_ref(&p.scene_ref, x, y," in home
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
    # WHY the probe outcome and the absent query, and not "status unavailable":
    # the keystore row reports the result of the derivation probe this app is
    # allowed to attempt, and the vault and filter rows name the missing query
    # rather than implying a status the platform never answered.
    assert 'b"Derivation refused by the kernel"' in source
    assert 'b"No vault encryption status query is exposed"' in source
    assert 'b"No privacy filter status query is exposed"' in source


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
    # WHY this assertion names the constants now: the calendar's four views did
    # not share a number for "+ New Event". Month painted and tapped the band
    # these constants state; Week, Day and Agenda painted and tapped 816..872, so
    # the same control sat sixteen rows lower in three of the four screens and the
    # cards stacked above it -- capped by this band's top edge -- were measured
    # against a box the button no longer occupied. The invariant is unchanged and
    # is now carried by the one box every view reads: the button is hit where it
    # is painted, at rows 800..856, on every view.
    assert "const ACTION_BTN_Y0: usize = 800;" in calendar
    assert "const ACTION_BTN_Y1: usize = ACTION_BTN_Y0 + ACTION_BTN_H;" in calendar
    assert calendar.count("y >= ACTION_BTN_Y0 && y < ACTION_BTN_Y1") == 8
    assert "y >= 816 && y < 872" not in calendar
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
    # WHY the platform's role and not a name of the app's own: the settings
    # app's colours come from the platform's theme now, so "the neutral ink"
    # is `on_surface_variant` and "the success ink" is `success`. The claim
    # under test is unchanged: an unavailable service is reported in the
    # neutral ink and never in the success ink.
    assert 'b"DVFS state unavailable", THEME.on_surface_variant' in source
    # The rows are multi-line calls, so the assertions match the argument
    # sequence with whitespace flattened: caption, ink, badge, badge ink.
    compact = " ".join(source.split())
    assert (
        'b"Derivation refused by the kernel", b"REFUSED", THEME.on_surface_variant,'
        in compact
    )
    assert (
        'b"No vault encryption status query is exposed", THEME.on_surface_variant, '
        'b"NOT REPORTED", THEME.on_surface_variant'
        in compact
    )
    assert (
        'b"No privacy filter status query is exposed", THEME.on_surface_variant, '
        'b"NOT REPORTED", THEME.on_surface_variant'
        in compact
    )
    assert 'b"Derivation refused by the kernel", THEME.success' not in compact
    assert 'b"No vault encryption status query is exposed", THEME.success' not in compact
    assert 'b"No privacy filter status query is exposed", THEME.success' not in compact
