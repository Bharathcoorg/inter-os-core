# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Truth contract for the InterOS gallery application.

A gallery tile that looks like a preview of a photograph or a frame of a
video is the most direct way to show a user something that is not there.
This build decodes real thumbnails for images and has no video decoder at
all, so a video tile has to say that rather than paint a picture of one.
Every assertion below pins a specific defect found by reading the app.
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


def gallery_main() -> str:
    return read("apps/gallery/src/main.rs")


def gallery_source() -> str:
    """Everything the gallery app is made of."""
    return "\n".join(
        path.read_text(encoding="utf-8")
        for path in sorted((ROOT / "apps/gallery/src").rglob("*.rs"))
    )


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

    WHY the pin names the SDK box now: the gallery used to answer a hand-written
    `y >= 1400 && x >= 240 && x <= 480`, a strip of its own invention. The press
    and the swipe now read one declaration -- `design::home_band::HIT` -- so the
    contract is that the gesture is gated by that box at all, not the old rows.
    """
    source = gallery_main()
    assert "let is_home_gesture = HOME_HIT.contains(input.x as usize, input.y as usize);" in source
    assert "const HOME_HIT: Rect = std::design::home_band::HIT;" in source
    assert "if is_home_gesture" in source
    assert "input.y >= 1400 && input.x >= 240 && input.x <= 480" not in source


def test_the_empty_state_does_not_name_a_shortcut_that_does_not_exist():
    """The screenshots empty state said "or by pressing 'S'".

    The gallery has no keyboard handler at all.
    """
    source = gallery_main()
    assert "pressing 'S'" not in source
    assert 'b"Capture a screen with the + Shot button."' in source


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
    source = gallery_source()
    assert slot_arguments(source) == []
    # The app resolves no slot: not through the SDK's slot helpers, and not
    # by looking a capability type up for itself.
    assert "std::cap::" not in source
    assert "std::cap_lookup(" not in source
    # The gallery still opens a surface, still presents it and still draws
    # its thumbnails into it.
    assert "std::surface_create(" in source
    assert "std::surface_flip(" in source
    assert "std::surface_write(" in source
