"""Truth contract for the InterOS notes application.

The notes app holds the user's own documents, so a claim it cannot support
is worse here than in most other apps: a note that never reached the device
and still reads "Saved" is a lost document with a reassuring label on it.

Every assertion below pins a specific defect that was found by reading the
app rather than by running it, and each one is a rule that has to keep
holding as the app changes.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def notes_main() -> str:
    return read("apps/notes/src/main.rs")


def notes_lib() -> str:
    return read("apps/notes/src/lib.rs")


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
    """The store used to ask for capability `2`, which is the framebuffer.

    The VFS verifies the capability *type* at the slot it is given, so a
    framebuffer capability presented for filesystem authority is refused and
    the store never opens. The two filesystem capabilities have to be named.
    """
    source = notes_main()
    assert "std::cap_lookup(std::cap_type::FILE_SYSTEM_READ, 0)" in source
    assert "std::cap_lookup(std::cap_type::FILE_SYSTEM_WRITE, 0)" in source
    assert "std::cap_lookup(2, 0)" not in source
    assert "std::cap_lookup(0, 0)" not in source


def test_the_display_capability_is_looked_up_by_type():
    """`cap_lookup(0, ...)` asks for a capability type that does not exist.

    The lookup therefore always failed and the app fell back to the literal
    slot 6, which is the IPC receive capability in the notes table. The app
    only ever drew because the kernel searched for a framebuffer capability
    on its behalf.
    """
    source = notes_main()
    assert "std::cap_lookup(std::cap_type::FRAMEBUFFER_DIRECT, 0)" in source
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
