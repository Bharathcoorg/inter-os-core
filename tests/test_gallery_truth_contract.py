"""Truth contract for the InterOS gallery application.

A gallery tile that looks like a preview of a photograph or a frame of a
video is the most direct way to show a user something that is not there.
This build decodes real thumbnails for images and has no video decoder at
all, so a video tile has to say that rather than paint a picture of one.
Every assertion below pins a specific defect found by reading the app.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def gallery_main() -> str:
    return read("apps/gallery/src/main.rs")


def test_a_video_tile_is_not_painted_as_a_frame():
    """The video card's thumbnail was a blend between two fixed colours.

    A two-tone ramp in the shape of a frame reads as a preview of the
    video. This build has no video decoder, so there is no frame to show.
    """
    source = gallery_main()
    assert "blend_argb(item.primary_color, item.secondary_color, t)" not in source
    assert 'b"No preview"' in source


def test_the_video_surface_does_not_claim_a_direct_scanout():
    """The player painted an animated scanline ramp labelled "DRM Direct
    Scanout".

    That read as a playing video, and it was nothing of the sort: the
    gallery decodes no video at all.
    """
    source = gallery_main()
    assert "DRM Direct Scanout" not in source
    assert "DRM/KMS direct scanout animated frame pattern" not in source
    assert 'b"No video decoder on this device"' in source


def test_the_media_resolution_is_not_the_display_resolution():
    """The info sheet fell back to "720 x 1440 (Native Display)".

    That is the size of the screen the app draws on, not a measurement of
    the media, and it was shown under a "Resolution:" label.
    """
    source = gallery_main()
    assert "720 x 1440 (Native Display)" not in source
    assert 'b"not measured"' in source


def test_a_built_in_wallpaper_is_not_given_a_path():
    """The info sheet fell back to "/system/wallpaper".

    No such path exists on this platform, and a built-in wallpaper is not
    a file.
    """
    source = gallery_main()
    assert "/system/wallpaper" not in source
    assert 'b"built-in wallpaper"' in source


def test_the_wallpapers_are_not_counted_as_photographs():
    """The summary said "N wallpapers, M photos on device" and counted the
    wallpapers in both numbers.

    The honest empty state -- no photographs on the device -- was also
    unreachable, because the grid always held the built-in wallpapers.
    """
    source = gallery_main()
    assert "pub fn device_photos_count(&self) -> usize" in source
    assert "app.device_photos_count() == 0" in source
    assert "let photos_count = app.device_photos_count();" in source


def test_the_home_gesture_only_leaves_the_app_from_the_home_strip():
    """A pan or a fling from anywhere exited the gallery.

    Scrolling the grid threw the user out of the app.
    """
    source = gallery_main()
    assert "let is_home_gesture = input.y >= 1400 && input.x >= 240 && input.x <= 480;" in source
    assert "if is_home_gesture" in source


def test_the_empty_state_does_not_name_a_shortcut_that_does_not_exist():
    """The screenshots empty state said "or by pressing 'S'".

    The gallery has no keyboard handler at all.
    """
    source = gallery_main()
    assert "pressing 'S'" not in source
    assert 'b"Capture a screen with the + Shot button."' in source


def test_a_capability_slot_is_resolved_by_its_type():
    """The surface was destroyed through the literal slot 6."""
    source = gallery_main()
    assert "std::surface_destroy(6," not in source
    assert "std::cap::framebuffer_slot()" in source
