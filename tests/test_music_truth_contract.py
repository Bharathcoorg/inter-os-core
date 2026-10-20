"""Truth contract for the InterOS music application.

A player that says a track was found in local storage when it never
searched a filesystem, and that drops the sentence explaining why the
scrubber does not move exactly when the user selects a track, is the same
defect twice. Every assertion below pins a specific defect found by
reading the app.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def music_main() -> str:
    return read("apps/music/src/main.rs")


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
    """The surface was created, flipped and destroyed through the literal
    slot 6."""
    source = music_main()
    assert "surface_create(6," not in source
    assert "surface_flip(6)" not in source
    assert "surface_destroy(6," not in source
    assert "std::cap::framebuffer_slot()" in source
    # The keystore argument is a keystore slot, not a capability slot, and
    # naming it keeps the difference visible at the call site.
    assert "const MEDIA_KEY_SLOT: u32 = 1;" in source
    assert "std::keystore_derive(MEDIA_KEY_SLOT," in source
