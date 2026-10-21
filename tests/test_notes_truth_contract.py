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
    """
    source = notes_main()
    assert 'json_field(payload, b"\\"hour\\":").unwrap_or(0)' not in source
    assert 'json_field(payload, b"\\"minute\\":").unwrap_or(0)' not in source
    assert 'let hour = json_field(payload, b"\\"hour\\":")?;' in source
    assert 'let minute = json_field(payload, b"\\"minute\\":")?;' in source


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
    """The panel is 1080x2160 and the app paints at 720x1440.

    A bitmap run is magnified 3:2 on the way to the screen and reads as
    soft. The outline face rasterises the real outline and reads as sharp.
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
