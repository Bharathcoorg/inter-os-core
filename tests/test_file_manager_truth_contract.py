"""Truth contract for the InterOS file manager application.

A file browser that says "Directory Empty" for a directory the device
refused to read tells the user their files are gone. A file browser that
offers a "Cloud" scope it cannot reach is worse: it advertises a service
the platform does not have. Every assertion below pins a specific defect
found by reading the app.
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


def fm_main() -> str:
    return read("apps/file_manager/src/main.rs")


def fm_source() -> str:
    """Everything the file manager app is made of."""
    return "\n".join(
        path.read_text(encoding="utf-8")
        for path in sorted((ROOT / "apps/file_manager/src").rglob("*.rs"))
    )


def test_the_app_opens_on_a_directory_it_is_allowed_to_read():
    """The kernel's app path policy refuses `/data` before the path is
    examined.

    The app's default path was `/data`, so every listing it ever asked for
    was refused, and the refusal was displayed as an empty directory.
    `/storage` is one of the paths the policy allows.
    """
    source = fm_main()
    assert 'let init = b"/storage";' in source
    assert 'let init = b"/data";' not in source


def test_the_category_rows_are_the_directories_that_exist():
    """The VFS mounts `/storage` with `photos`, `pictures` and `videos`.

    The old rows named scopes the platform does not have -- "Recent",
    "Images", "Videos", "Documents", "Downloads", "Internal Storage" and
    "Cloud (Optional)" -- and every one of them opened `/data`, which the
    kernel refuses. Each row led to a refusal dressed up as a category.
    """
    source = fm_main()
    assert 'pub const CATEGORY_PATHS: [&str; 4] = ["/storage", "/storage/photos", "/storage/pictures", "/storage/videos"];' in source
    assert 'pub const CATEGORY_LABELS: [&[u8]; 4] = [b"Storage", b"Photos", b"Pictures", b"Videos"];' in source
    assert "Cloud (Optional)" not in source
    for invented in (b"Recent", b"Documents", b"Downloads"):
        assert f'b"{invented.decode()}"' not in source


def test_an_unreadable_directory_is_not_reported_as_an_empty_one():
    """The empty card said "Directory Empty" for every failure.

    A refused listing, a listing that has not happened and a genuinely
    empty directory are three different facts.
    """
    source = fm_main()
    assert "pub enum ListingState" in source
    assert "ListingState::Refused" in source
    assert "ListingState::NotListed" in source
    assert 'b"Directory Unreadable"' in source
    assert 'b"Not Listed Yet"' in source
    assert 'b"This directory holds no entries"' in source
    assert "Directory empty or unread" not in source


def test_a_refused_launch_is_not_logged_as_a_launch():
    """The old code logged "Launching associated media viewer" before the
    launch and threw the answer away.

    A refused launch was therefore logged as a launch.
    """
    source = fm_main()
    assert "Launching associated media viewer" not in source
    assert "Launched the associated media viewer." in source
    assert "The associated media viewer could not be launched." in source
    assert "match std::app_launch(app_id)" in source


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
    nothing on screen to say why. The framebuffer slot that happened
    to be the one number that was right in every app was exactly the
    accident this removes.
    """
    source = fm_source()
    assert slot_arguments(source) == []
    # The app resolves no slot: not through the SDK's slot helpers, and not
    # by looking a capability type up for itself.
    assert "std::cap::" not in source
    assert "std::cap_lookup(" not in source
    # The browser still asks the platform for a directory listing and still
    # opens, presents and releases a surface to draw the result.
    assert "std::fs_list(" in source
    assert "std::surface_create(" in source
    assert "std::surface_flip(" in source
    assert "std::surface_destroy(" in source


def test_the_text_is_drawn_with_the_outline_typeface():
    """The panel is 1080x2160 and the app paints at 720x1440.

    A bitmap run is magnified 3:2 on the way to the screen and reads as
    soft.
    """
    source = fm_main()
    assert "outline_text::place_run" in source
    assert "outline_text::drawn_advance" in source
