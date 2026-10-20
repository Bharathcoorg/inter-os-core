"""Truth contract for the InterOS file manager application.

A file browser that says "Directory Empty" for a directory the device
refused to read tells the user their files are gone. A file browser that
offers a "Cloud" scope it cannot reach is worse: it advertises a service
the platform does not have. Every assertion below pins a specific defect
found by reading the app.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def fm_main() -> str:
    return read("apps/file_manager/src/main.rs")


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
    """A slot number is a position in a table, not a capability.

    The surface was destroyed through the literal slot 6, which happens to
    hold the framebuffer in this app's table today.
    """
    source = fm_main()
    assert "std::surface_destroy(6," not in source
    assert "std::cap::framebuffer_slot()" in source


def test_the_text_is_drawn_with_the_outline_typeface():
    """The panel is 1080x2160 and the app paints at 720x1440.

    A bitmap run is magnified 3:2 on the way to the screen and reads as
    soft.
    """
    source = fm_main()
    assert "outline_text::place_run" in source
    assert "outline_text::drawn_advance" in source
