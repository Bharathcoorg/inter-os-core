"""Truth contract for the InterOS music application.

A player that says a track was found in local storage when it never
searched a filesystem, and that drops the sentence explaining why the
scrubber does not move exactly when the user selects a track, is the same
defect twice. Every assertion below pins a specific defect found by
reading the app.
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


def music_main() -> str:
    return read("apps/music/src/main.rs")


def music_source() -> str:
    """Everything the music app is made of."""
    return "\n".join(
        path.read_text(encoding="utf-8")
        for path in sorted((ROOT / "apps/music/src").rglob("*.rs"))
    )


def test_the_empty_state_does_not_claim_a_filesystem_search():
    """The empty library said "No audio files were found in local storage".

    The app never searched a filesystem: the library is a record in the
    platform's settings store. A refused read, a store that failed
    validation and an empty store are three different facts.
    """
    source = music_main()
    assert "store::StoreState::ReadRefused => (" in source
    assert "store::StoreState::Invalid => (" in source
    assert 'b"Track store unreadable"' in source
    assert 'b"The device refused to read the track store"' in source


def test_the_scrub_note_is_not_silently_dropped():
    """The note was 783 pixels wide and the content band is 672.

    The centred helper refuses a run that does not fit, so the sentence
    explaining why the scrubber stays at zero was dropped exactly when a
    track was selected.
    """
    source = music_main()
    assert "b\"No audio backend reports a playback position;\"" in source
    assert "SCRUB_NOTE_Y + font::line_height(1)" in source
    # The old single-line form is gone.
    assert (
        'b"No audio backend reports a playback position; the scrubber stays at 0:00"'
        not in source
    )


def test_the_track_list_can_be_scrolled():
    """The library holds up to eight tracks and the visible band held three.

    The lower five could not be reached at all, and the screen did not say
    the list was truncated.
    """
    source = music_main()
    assert "pub list_scroll: std::design::scroll::ScrollState," in source
    assert "pub fn sync_list_extent(&mut self)" in source
    assert "music.list_scroll.apply_drag(input.dy);" in source
    assert "thumb_rect(LIST_TOP, LIST_VIEWPORT_H, 24)" in source
    # The tap handler has to use the same offset the rows were drawn at.
    assert "y.saturating_sub(LIST_TOP) + app.list_scroll.offset" in source


def test_the_text_is_drawn_with_the_outline_typeface():
    """The panel is 1080x2160 and the app paints at 720x1440.

    A bitmap run is magnified 3:2 on the way to the screen and reads as
    soft.
    """
    source = music_main()
    assert "use std::design::outline_text;" in source
    assert "outline_text::place_run" in source
    assert "outline_text::run_box" in source


def test_a_capability_slot_is_resolved_by_its_type():
    """A capability slot is a position in a table, not an identity.

    WHY this contract changed: a slot is a position in the task's
    capability table, and the position depends on the order the kernel
    inserted the capabilities in, so the app cannot name one. The kernel
    now authorises every platform call by the capability's *type* and the
    SDK's functions no longer take a slot argument, so there is nothing
    left for the app to resolve. Pinning a slot number pinned an
    accident: the launcher proved it by passing slot 0 while its
    `FileSystemRead` capability sat at slot 3, so the developer-terminal
    flag it wanted was never read and the refusal reached the user with
    nothing on screen to say why.
    """
    source = music_source()
    assert slot_arguments(source) == []
    # The app resolves no slot: not through the SDK's slot helpers, and not
    # by looking a capability type up for itself.
    assert "std::cap::" not in source
    assert "std::cap_lookup(" not in source
    # The player still opens a surface, presents it and tears it down.
    assert "std::surface_create(" in source
    assert "std::surface_flip(" in source
    assert "std::surface_destroy(" in source
    # The keystore argument is a keystore slot, not a capability slot, and
    # naming it keeps the difference visible at the call site.
    assert "const MEDIA_KEY_SLOT: u32 = 1;" in source
    assert "std::keystore_derive(MEDIA_KEY_SLOT," in source
