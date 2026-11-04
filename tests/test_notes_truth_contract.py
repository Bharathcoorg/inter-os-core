# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Truth contract for the InterOS notes application.

The notes app holds the user's own documents, so a claim it cannot support
is worse here than in most other apps: a note that never reached the device
and still reads "Saved" is a lost document with a reassuring label on it.

Every assertion below pins a specific defect that was found by reading the
app rather than by running it, and each one is a rule that has to keep
holding as the app changes.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# The SDK functions that used to take a capability slot as their first
# argument. The kernel now authorises every one of them by the
# capability's *type*, so the app names the authority it wants and the
# kernel finds it.
SLOT_ARG_SDK_CALLS = (
    "app_storage_open",
    "display_set_mode",
    "display_set_orientation",
    "fs_list",
    "fs_open",
    "fs_read",
    "fs_stat",
    "fs_unlink",
    "fs_write",
    "get_ticks",
    "ipc_recv",
    "ipc_send",
    "log",
    "package_install",
    "package_list",
    "package_registry",
    "package_rollback",
    "package_uninstall",
    "service_package_launch",
    "service_package_verify",
    "settings_get",
    "settings_set",
    "surface_create",
    "surface_destroy",
    "surface_flip",
    "surface_write",
    "verify_package",
    "window_focus",
    "window_set_geometry",
    "window_set_input",
)

# A capability slot named in a comment or in a user-visible label is not
# a call, so the call patterns are matched against the code with both
# removed.
_COMMENT_OR_LITERAL = re.compile(
    r"//[^\n]*"
    r"|/\*.*?\*/"
    r'|"(?:[^"\\]|\\.)*"'
    r'|(?:b|br|r#*)"(?:[^"\\]|\\.)*"#*',
    re.DOTALL,
)


def code_only(source: str) -> str:
    """Return `source` with comments and string literals removed."""
    return _COMMENT_OR_LITERAL.sub(" ", source)


def slot_arguments(source: str) -> list[str]:
    """Return the SDK calls in `source` that still take a capability slot.

    A slot argument is either a bare number or the name of a resolved
    slot, and a call that names one is a call that pinned a position in
    the task's capability table.
    """
    code = code_only(source)
    return [
        name
        for name in SLOT_ARG_SDK_CALLS
        if re.search(
            r"\b" + name + r"\s*\(\s*(?:\d|[A-Za-z_][A-Za-z0-9_]*_slot\b|\bslot\b)",
            code,
        )
    ]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def notes_main() -> str:
    return read("apps/notes/src/main.rs")


def notes_lib() -> str:
    return read("apps/notes/src/lib.rs")


def notes_source() -> str:
    """Everything the notes app is made of."""
    return "\n".join(
        path.read_text(encoding="utf-8")
        for path in sorted((ROOT / "apps/notes/src").rglob("*.rs"))
    )


def test_the_note_store_is_opened_within_the_app_sandbox():
    """`app_storage_open` rejects an absolute path.

    The store used to be the absolute "/data/notes.bin", which the kernel
    refuses with `InvalidArguments`, so the open always failed and every note
    the user wrote was lost when the app exited. The path has to stay
    relative to the app's own sandbox root.
    """
    source = notes_main()
    assert 'const STORAGE_PATH: &str = "data/notes.bin";' in source
    assert '"/data/notes.bin"' not in source
    assert "const STORAGE_PATH: &str = \"/" not in source


def test_the_store_uses_the_named_filesystem_capabilities():
    """A capability slot is a position in a table, not an identity.

    WHY this contract changed: a slot is a position in the task's
    capability table, and the kernel now authorises every platform call by
    the capability's *type*, so the SDK's functions no longer take a slot
    argument at all. The app names the authority it wants -- a filesystem
    read, a filesystem write -- and the kernel finds it. Pinning a slot
    number pinned an accident: the launcher proved it by passing slot 0
    while its `FileSystemRead` capability sat at slot 3, so the
    developer-terminal flag it wanted was never read and the refusal
    reached the user with nothing on screen to say why. So the old rule
    ("the two filesystem capabilities have to be named") becomes "the app
    names no slot at all".
    """
    source = notes_source()
    assert slot_arguments(source) == []
    # The app resolves no slot: not through the SDK's slot helpers, and not
    # by looking a capability type up for itself.
    assert "std::cap::" not in source
    assert "std::cap_lookup(" not in source
    # The store is still the user's own: it is opened inside the app's own
    # sandbox, by a relative path, and it is still read and written.
    assert 'const STORAGE_PATH: &str = "data/notes.bin";' in source
    assert '"/data/notes.bin"' not in source
    assert "std::app_storage_open(" in source
    assert "std::fs_read(" in source
    assert "std::fs_write(" in source


def test_the_display_capability_is_looked_up_by_type():
    """The framebuffer capability is found by the kernel, not by the app.

    WHY this contract changed: a slot is a position in the task's
    capability table, and the position depends on the order the kernel
    inserted the capabilities in, so the app cannot name one. The kernel
    now authorises the surface by the capability's *type* and the SDK
    takes no slot argument, so there is nothing left for the app to look
    up. Pinning a slot number pinned an accident: the launcher proved it
    by passing slot 0 while its `FileSystemRead` capability sat at slot 3,
    so the developer-terminal flag it wanted was never read. The old
    contract pinned the framebuffer by type because the app had to hand a
    slot to `surface_create`; it no longer hands over anything.
    """
    source = notes_source()
    assert slot_arguments(source) == []
    assert "std::cap::" not in source
    assert "std::cap_lookup(" not in source
    # The app still asks for a surface, still presents it and still draws
    # into it, and it does not fall back to a literal slot when the
    # platform refuses.
    assert "std::surface_create(" in source
    assert "std::surface_flip(" in source
    assert "std::surface_write(" in source
    assert "Err(_) => 6," not in source


def test_the_note_count_badge_is_not_a_single_digit():
    """The badge used to be built from a single digit.

    The store holds up to sixteen notes, so a device with twelve notes
    reported nine. A count is a measurement and it has to carry every digit.
    """
    source = notes_main()
    lib = notes_lib()
    assert "b'0' + (count.min(9) as u8)" not in source
    assert "count.min(9)" not in source
    assert "pub fn format_count_badge" in lib
    assert "format_count_badge(count, &mut badge)" in source


def test_a_clock_payload_without_a_time_is_not_stamped_as_midnight():
    """The hour and the minute used to default to zero.

    A payload that carried a date but no time was therefore stamped as
    midnight -- a time the clock never reported. The note has to carry no
    timestamp instead.

    WHY the assertion looks in the SDK now: the app stopped scanning the
    kernel's payload itself, because the launcher, Calendar and the Agent tool
    each scanned it differently and disagreed about one reading. `Notes` asks
    `interos_std::rtc` for a reading, and the rule that a missing field is a
    refused reading rather than a zero lives with the reader.
    """
    source = notes_main()
    assert 'json_field(payload, b"\\"hour\\":").unwrap_or(0)' not in source
    assert 'json_field(payload, b"\\"minute\\":").unwrap_or(0)' not in source
    assert "std::rtc::Reading::now()?" in source
    assert "fn current_civil_time() -> Option<CivilTime>" in source

    reader = (ROOT / "sdk/rust/interos-std/src/lib.rs").read_text(encoding="utf-8")
    for name in ("hour", "minute"):
        assert f'field(data, b"\\"{name}\\"")?,' in reader, (
            f"the shared clock reader must demand \"{name}\" rather than default it"
        )
    assert "unwrap_or(0)" not in reader[reader.index("fn parse(data"):reader.index("fn field(data")]


def test_an_unreadable_store_is_not_reported_as_an_empty_one():
    """The old code drew "No Notes Saved Yet" for every failure.

    A refused read, a corrupt store and a genuinely empty store are three
    different facts, and only the third of them means the user has no notes.
    """
    source = notes_main()
    assert "pub enum StorageState" in notes_lib()
    assert "StorageState::Refused" in source
    assert "StorageState::Corrupt" in source
    assert "StorageState::Recovered" in source
    assert 'b"Storage Unavailable"' in source
    assert 'b"Store Failed Validation"' in source
    assert "storage_online" not in source


def test_a_failed_save_is_reported_rather_than_discarded():
    """Every save used to throw its result away.

    The editor header derived "Saved" from the note's dirty flag, which the
    store cleared only after a successful write -- and every write silently
    failed, so a note that had never reached the device still read "Saved".
    """
    source = notes_main()
    assert "save_notes_to_storage(&mut state.store);" not in source
    assert "state.save_to_storage();" in source
    assert "pub fn save_to_storage" in source
    assert 'b"Not saved"' in source


def test_a_store_that_fails_validation_is_kept_rather_than_destroyed():
    """A store that does not validate is a partial write or a foreign file.

    In both cases the user may still want what is in it, so the bytes are
    copied aside before the live store is reinitialised, and the app says the
    store failed validation.
    """
    source = notes_main()
    assert 'const QUARANTINE_PATH: &str = "data/notes.corrupt";' in source
    assert 'const STAGING_PATH: &str = "data/notes.staged";' in source
    assert "quarantine_failed_store" in source


def test_the_note_text_is_drawn_with_the_outline_typeface():
    """The panel and the app's surface are both 720x1440.

    The compositor copies the surface 1:1. The 16x24 bitmap face would be
    magnified to the screen's type sizes and read soft; the outline face
    rasterises the real outline and reads as sharp.
    """
    source = notes_main()
    assert "use interos_std::design::outline_text;" in source
    assert "outline_text::place_run" in source
    assert "outline_text::run_box" in source


def test_the_app_never_seeds_a_note():
    """A notes app whose contents are the user's must not invent documents.

    The app used to seed four invented notes -- one of which asserted
    hardware specifications the app had never measured -- and then
    persisted them, so the device carried documents the user never wrote.
    The startup path must not create a note at all.
    """
    source = notes_main()
    start = source.index("pub extern \"C\" fn _start()")
    startup = source[start:]
    assert "create_note(" not in startup
    assert "create_stamped_note(" not in startup


# -----------------------------------------------------------------------------
# The person's text size
#
# Settings' "Text size" is a rung on a ladder the binary used to ignore: the app
# laid every box out at one hundred per cent, so a reader who raised the setting
# got a smaller reading surface while the ink itself grew. These gates hold the
# honest slice that fixed it -- the empty-state card the list shows when the
# store has nothing to draw -- and refuse, by name, the shortcuts that would make
# the slice look finished without being finished.
# -----------------------------------------------------------------------------

def empty_state_paint(source: str) -> str:
    """The list's empty-state branch of the painter, comments and literals out.

    `if visible_count == 0` through the note-card region's opening `let card_h =
    CARD_H;`: the code that inks the card, its headline and its subtitle when the
    list has nothing to show, and nothing else. The rung-aware faces are required
    inside this slice, the authored run origins refused inside it.
    """
    code = code_only(source)
    start = code.index("if visible_count == 0 {")
    end = code.index("let card_h = CARD_H;")
    return code[start:end]


def test_the_app_publishes_the_persons_text_size() -> None:
    """A ladder the binary never reads is a ladder nobody climbs.

    The empty state is now derived from the rung the renderer holds, so an app
    that never published Settings' "Text size" would lay that card out at the
    platform rung forever and the person's choice would change nothing here. The
    helper existing and compiling was never the failure -- calling it was. Two
    call sites are pinned because no host test can tell a rung that was read from
    one left at its default.
    """
    code = code_only(notes_main())
    body = code[code.index("fn _start"):]
    boot = body[: body.index("loop {")]
    # Before the first frame is laid out, so the opening screen is not drawn at a
    # rung the person has already left and resized a frame later.
    assert "publish_text_scale();" in boot, "the rung is read after the first frame"
    assert "render_notes_ui(surface, &state);" in boot, "the boot frame is not drawn"
    assert boot.index("publish_text_scale();") < boot.index(
        "render_notes_ui(surface, &state);"
    ), "the first frame is presented before the rung is published"
    # Re-read every poll, because the size changes in Settings while this screen is
    # alive; a rung that moved repaints the whole surface -- the card, the headline
    # and the subtitle all move with it, so a band repaint would leave one behind.
    loop = body[body.index("loop {"):]
    assert "if publish_text_scale() {" in loop, "the loop stopped re-reading the setting"
    assert "if publish_text_scale() {\n            needs_redraw = true;" in loop, (
        "a moved rung no longer asks for a repaint"
    )
    assert "if needs_redraw {\n            render_notes_ui(surface, &state);" in loop, (
        "the repaint stopped redrawing the whole screen at the new rung"
    )


def test_the_rung_is_read_on_the_stack_and_an_unread_setting_resets_nothing() -> None:
    """Two ways this could quietly lie.

    First, zero heap: the app is `#![no_std]` with no allocator, and the read runs
    on the repaint edge of the event loop, so the settings buffer is a fixed stack
    array, not a `String`. Second, an unread setting must not fall back to one
    hundred per cent: if the store is busy the rung the process already holds is
    the honest answer, otherwise the screen would resize under a reader who never
    touched it. And the publish returns whether the rung actually moved so the loop
    repaints only when the pixels really changed.
    """
    source = notes_main()
    assert "use interos_std::design::accessibility::{DynamicType, UserTextScale};" in source
    fn = source[source.index("fn publish_text_scale()"):]
    assert 'std::settings_get("display.text_scale", &mut buf)' in fn, (
        "the app stopped asking the platform for the person's text size"
    )
    assert "let mut buf = [0u8; 32];" in fn, "the settings buffer left the stack"
    assert "let next = want.unwrap_or(before);" in fn, (
        "an unread setting fell back to the platform rung instead of the held one"
    )
    assert "DynamicType::new(next).publish() != before" in fn, (
        "the publish stopped answering whether the rung actually moved"
    )


def test_the_empty_state_paint_is_derived_from_the_rung() -> None:
    """The card must be sized and inked at the rung, or the slice is a lie.

    The authored empty-state height was the box the screen drew at one hundred per
    cent; the empty state now answers `empty_card_h_at(lvl)`, and its headline and
    subtitle answer the `*_y_at(lvl)` derivations, so the rungs above the authored
    one move the plate as well as the ink. Two refusals keep the fix honest:

    - the authored `EMPTY_CARD_H` may not come back into the binary at all -- but
      it stays declared in the crate, because a gate satisfied by deleting the box
      it was meant to forbid forbids nothing; and
    - the run origins may not be re-typed as the remembered `500` / `560` the
      screen was authored with, because a rung-aware plate with a hand-typed run
      inside it is the same clip the fix removed.

    At the authored level every branch delegates to the ladder's own calls, so the
    pixels a reader who left the setting alone gets are byte for byte unchanged.
    """
    main = notes_main()
    lib = notes_lib()
    region = empty_state_paint(main)

    # The rung the card reads, and the three faces that size and place it.
    assert "notes::draw_level()" in region, "the empty state stopped reading the rung"
    for rung_face in (
        "empty_card_h_at(",
        "empty_title_y_at(",
        "empty_subtitle_y_at(",
        "empty_text(x, y, title_x, title_y,",
        "empty_text(x, y, sub_x, sub_y,",
    ):
        assert rung_face in region, f"the empty state stopped using its rung face: {rung_face}"

    # The authored height is gone from the binary -- refused with word boundaries
    # so `EMPTY_CARD_X`/`EMPTY_CARD_W`, which do not move, are not implicated.
    assert re.search(r"\bEMPTY_CARD_H\b", code_only(main)) is None, (
        "EMPTY_CARD_H -- the box authored at the platform rung -- is back in the "
        "binary, while the empty state inks empty_card_h_at(lvl) at the person's rung"
    )
    # The remembered run origins are gone from the paint, replaced by the faces.
    for remembered in ("500", "560"):
        assert remembered not in region, (
            f"the empty state went back to the authored origin {remembered} instead "
            "of the rung-aware derivation"
        )

    # The authored boxes still exist in the crate: refusing them in the binary has
    # to be a choice the code could undo, not an absence that proves nothing.
    for authored in (
        "pub const EMPTY_CARD_H:",
        "pub const AUTHORED_EMPTY_CARD_H:",
        "pub const EMPTY_CARD_Y:",
        "pub const EMPTY_SUBTITLE_LANE:",
    ):
        assert authored in lib, f"the crate stopped declaring its anchor: {authored}"

    # The byte-for-byte passthrough at the authored level: the platform branch
    # still centres and trims with the ladder's own calls.
    assert "lvl == notes::PLATFORM_LEVEL" in region, (
        "the empty state stopped delegating to the ladder at the authored rung"
    )
    assert "std::design::font::center_x(" in region, "the platform-centre fallback is gone"
    assert "std::design::font::ellipsize(" in region, "the platform-trim fallback is gone"


def test_the_empty_state_anchors_to_the_authored_pixels_at_every_level() -> None:
    """The derivations are only faithful if every rung answers the authored number.

    Each empty-state face is a `const fn` of a level so the compile-time gates can
    sweep all four selectable rungs, and the whole point of the `_at(PLATFORM_LEVEL)
    == authored` anchors is that adopting the setting moves no pixel for a reader
    who left it alone. If those anchors were relaxed to chase green, the app would
    resize the screen under someone who asked for nothing -- the exact defect class
    this change must not create while fixing the other one.
    """
    lib = notes_lib()
    # The authored pixels the empty state has always drawn.
    assert "const _: () = assert!(empty_title_y_at(PLATFORM_LEVEL) == 500);" in lib
    assert "const _: () = assert!(empty_subtitle_y_at(PLATFORM_LEVEL) == 560);" in lib
    assert "const _: () = assert!(empty_card_h_at(PLATFORM_LEVEL) == AUTHORED_EMPTY_CARD_H);" in lib

    # The sweeps are run against the clamp, not the raw rung, and reach the top of
    # the ladder -- a sweep over a level no renderer can be handed would be no sweep.
    assert "const _: () = assert!(draw_level_holds_at_every_level());" in lib
    assert "const _: () = assert!(empty_card_holds_its_notice());" in lib
    assert "while want <= MAX_LEVEL {" in lib
    assert "while level <= MAX_LEVEL as usize {" in lib

    # The clamp never raises a rung the panel cannot carry, and never touches the
    # authored one; the fit is checked at every level the clamp actually hands back.
    gate = lib[lib.index("const fn draw_level_holds_at_every_level()"):]
    assert "if draw_level_at(PLATFORM_LEVEL) != PLATFORM_LEVEL {" in gate
    assert "if chosen > want {" in gate
    assert "if !empty_state_fits(chosen) {" in gate


def test_measured_runs_are_runtime_faces_and_ladder_geometry_is_const() -> None:
    """The E0015 trap this slice had to avoid, pinned as a shape.

    A box whose width comes from a measured run can only be asked at run time: the
    SDK's glyph advance tables are ordinary functions, so a `const fn` that reached
    for `chip_advances_at` or a measured lane would not compile. Every measured
    face is therefore a runtime `fn` -- and the refusal below catches a regression
    that re-declared one `const`. The ladder's own vertical metrics *are* compile-
    time, so the geometry faces that use only those stay `const fn` and the sweep
    gates can prove them at every level.

    The measured faces also have to fall back to the ladder at the authored rung,
    byte for byte, and `fitted_run` must hand the caller the whole label when the
    trim refuses rather than paint an empty plate.
    """
    lib = notes_lib()

    # Measured-width faces are runtime functions, never `const fn`.
    for runtime in ("run_w_at", "run_w", "ellipsize_run", "fitted_run", "centered_x"):
        assert f"pub fn {runtime}" in lib, f"`{runtime}` must be a runtime face"
        assert f"pub const fn {runtime}" not in lib, (
            f"`{runtime}` measures a run and cannot be `const` -- the advance "
            "tables are not compile-time, so this would not compile"
        )
    # Ladder-only geometry stays const so the per-level sweeps can prove it.
    for geo in ("rows_at", "grown", "plate_at", "cap_at", "em_at", "line_at", "ascent_at"):
        assert f"pub const fn {geo}" in lib, f"`{geo}` is ladder geometry and must be const"

    # The authored-rung passthroughs, byte for byte.
    assert "return interos_std::design::font::measure(text, scale);" in lib, (
        "run_w_at stopped delegating to the ladder's own measure at the authored rung"
    )
    assert "return interos_std::design::font::ellipsize(text, scale, max_w, out_buf);" in lib, (
        "ellipsize_run stopped delegating to the ladder's own ellipsize at the authored rung"
    )
    # A run the trim cannot cut is refused (0) rather than inked past the lane, and
    # `fitted_run` answers such a refusal with the whole label, not an empty plate.
    trim = lib[lib.index("pub fn ellipsize_run("):]
    assert "if dots_w > max_w || out_buf.len() < 3 {" in trim
    assert "return 0;" in trim[: trim.index("pub fn fitted_run")]
    fitted = lib[lib.index("pub fn fitted_run"):]
    assert "let len = ellipsize_run(label, scale, lane, buf);" in fitted
    assert "return label;" in fitted, (
        "fitted_run stopped falling back to the whole label, so a refused trim "
        "would paint an empty plate"
    )

    # The empty state's own ink is a runtime face that asks the rung for its cap,
    # its band and its outline placement -- and delegates to the shared painter at
    # the authored level so no pixel moves there.
    main = notes_main()
    assert "fn empty_text(" in main
    assert "const fn empty_text(" not in main
    body = main[main.index("fn empty_text("):main.index("fn test_text(")]
    assert "if lvl == notes::PLATFORM_LEVEL {\n        return test_text(" in body, (
        "empty_text stopped delegating to the authored painter at the platform rung"
    )
    assert "notes::cap_at(scale, lvl)" in body, "the empty ink stopped using the rung's cap"
    assert "outline_text::place_run_cap(" in body, "the empty ink stopped placing a rung cap"
    assert "notes::rows_at(scale, lvl)" in body, (
        "the bitmap fallback stopped growing its band with the rung"
    )


def test_the_derivation_floors_at_the_authored_size_at_every_level() -> None:
    """Adoption may only ever grow a box, never shrink one it was drawn with.

    Several empty-state heights are numbers the crate's own fit gates read, so a
    rung-aware face that could return below the authored value would let a gate
    green-light a plate the painter no longer fills. The floors are structural: a
    stride grows by the rows its rung's box gained *above* the authored level --
    `saturating_sub`, so it is zero there and never negative -- and the card height
    is `later(AUTHORED_EMPTY_CARD_H, ...)`, so the authored box is the minimum at
    every level. The clamp that hands back a rung is bounded by the sweep gates;
    this is the bound on the geometry those rungs size.
    """
    lib = notes_lib()
    assert "rows_at(scale, level).saturating_sub(rows_at(scale, PLATFORM_LEVEL))" in lib, (
        "`grown` lost its saturating floor, so a lower rung could shrink an "
        "authored stride"
    )
    card = lib[lib.index("pub const fn empty_card_h_at("):]
    assert "later(\n        AUTHORED_EMPTY_CARD_H," in card[:200], (
        "empty_card_h_at stopped flooring at the authored box, so a rung could "
        "draw the notice shorter than the screen was built for"
    )
    plate = lib[lib.index("pub const fn plate_at("):]
    assert "later(authored, rows_at(scale, level) + air)" in plate[:200], (
        "plate_at stopped flooring at the height it was drawn with"
    )
