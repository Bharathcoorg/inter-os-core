# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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


def test_the_scrubber_is_omitted_rather_than_drawn_inert():
    """The scrubber was a permanently inert bar with a note under it.

    No audio backend reports a playback position and this app has no
    transport that can seek, so the control was drawn and could do nothing
    at all. The design document says to omit the control when seeking is
    unsupported, and a note under an inert bar is a workaround rather than
    an answer.
    """
    source = music_main()
    assert "SCRUB_NOTE_Y" not in source
    assert "TRACK_Y" not in source
    assert "TIMES_Y" not in source
    assert 'b"No audio backend reports a playback position;"' not in source
    assert "the scrubber stays at 0:00" not in source


def test_the_transport_streams_the_device_s_pcm_output():
    """The player used to claim background playback while streaming nothing.

    The transport now opens the AC'97 stream, hands it converted samples, and
    reads the position back from the device's own frame counter. The app owns
    that stream, so leaving the screen stops it instead of claiming audio
    continues in the background.
    """
    playback = read("apps/music/src/playback.rs")
    assert "std::audio_stream_open(" in playback
    assert "std::audio_stream_write(" in playback
    assert "std::audio_stream_status(" in playback
    assert "std::audio_stream_close(" in playback
    assert "position_millis(s.frames_consumed)" in playback
    # The file is resolved from the shared storage, never invented.
    assert "library::resolve_source(" in playback
    # The refusal paths say what happened instead of pretending to play.
    assert "PlaybackError::UnsupportedFormat" in playback
    source = music_source()
    assert "Background playback active" not in source
    assert "total_dma_buffers_streamed" not in source
    assert "std::audio_stream_pause" not in source
    manifest = read("apps/music/manifest.json")
    assert '"CAP_AUDIO_STREAM"' in manifest


def test_the_library_is_published_into_the_device_s_document_index():
    """The library was a record in the app's own settings store.

    The file manager could not list it, the launcher's search could not
    find it and the platform's document index did not know it existed.
    """
    library = read("apps/music/src/library.rs")
    assert 'pub const STORAGE_ROOT: &str = "/storage";' in library
    assert "std::fs_open(" in library
    assert "std::content::content_index_refresh()" in library
    # The document is named after the track, so the launcher's search finds
    # it by the word a person would type.
    assert 'pub const DOC_PREFIX: &[u8] = b"music-";' in library


def test_the_library_can_be_searched_sorted_and_imported():
    """The design document asks for a search, an order and an import.

    The app held neither: the list was flat, there was nothing to type
    into, and the app read only its own store while holding the authority
    to read the shared storage.
    """
    source = music_main()
    library = read("apps/music/src/library.rs")
    assert "const SEARCH_FIELD: Rect" in source
    assert "pub search: [u8; SEARCH_CAP]" in source
    assert "const SORT_BTN: Rect" in source
    assert "pub sort: SortMode" in source
    assert "const IMPORT_BTN: Rect" in source
    assert "std::fs_list(" in library
    assert "pub fn import_from_storage(" in library


def test_the_queue_is_the_library_and_playlists_are_reported_unavailable():
    """The design document asks for a queue and for playlists.

    The library store holds one flat ordered list, so the queue is that
    list and the list can be reordered. There is no second list to hold a
    playlist, so the screen says so instead of offering a control that
    does nothing.
    """
    source = music_main()
    lib = read("apps/music/src/lib.rs")
    assert "pub fn move_to(&mut self, idx: usize, to: usize) -> bool" in lib
    assert "pub fn sort_by_title(&mut self) -> bool" in lib
    assert "Playlists unavailable" in source
    assert "const PUBLISH_BTN: Rect" in source


def test_the_artwork_uses_the_platform_s_card_component():
    """The artwork was a hand-rolled rounded rectangle with its own fill.

    A card that invents its own fill, its own hairline and its own radius
    is a card a third-party app cannot match, and the platform already says
    what a card looks like.
    """
    source = music_main()
    assert "components::card_style(&THEME, components::Elevation::Resting)" in source
    assert "ART_CARD.radius" in source
    assert "ART_CARD.fill" in source
    # The track rows are the platform's list row.
    assert "components::list_row_style(&THEME, false)" in source
    assert "components::list_row_style(&THEME, true)" in source
    assert "ROW.radius" in source
    assert "ROW.pad" in source


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
    """The panel and the app's surface are both 720x1440.

    The compositor copies the surface 1:1. The 16x24 bitmap face would be
    magnified to the screen's type sizes and read soft, so text uses the
    outline face rasterised at the screen's own resolution.
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
