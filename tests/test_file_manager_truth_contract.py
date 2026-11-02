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


def test_a_refused_dispatch_is_not_logged_as_a_dispatch():
    """The old code logged "Launching associated media viewer" before the
    launch and threw the answer away.

    A refused launch was therefore logged as a launch.

    WHY this contract changed: the app no longer decides for itself which
    program opens a file. The platform's intent registry holds what the
    installed packages declared, and the app asks the platform to dispatch
    the document to the handler that declared its media type. The rule the
    contract protects is unchanged -- a refused dispatch is not reported as a
    dispatch -- but the mechanism is now the platform's registry rather than
    the app's private table.
    """
    source = fm_main()
    assert "Launching associated media viewer" not in source
    assert "The document was dispatched to its handler." in source
    assert "The document's handler could not be launched." in source
    assert "match std::content::intent_dispatch(" in source
    # A media type with no handler is reported as such, and names nothing.
    assert "No app handles this file." in source
    # The private table is no longer what decides.
    assert "get_app_for_mime(" not in source


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
    """The panel and the app's surface are both 720x1440.

    The compositor copies the surface 1:1. The 16x24 bitmap face would be
    magnified to the screen's type sizes and read soft, so text uses the
    outline face rasterised at the screen's own resolution.
    """
    source = fm_main()
    assert "outline_text::place_run" in source
    assert "outline_text::drawn_advance" in source


def fm_lib() -> str:
    return read("apps/file_manager/src/lib.rs")


def test_every_text_lane_is_derived_from_the_container_that_holds_it():
    """A lane authored inside the branch that paints it is a number no test can
    check.

    The app's binary is built without a test harness, so its fit gate can only be
    a `#[test]` in the library, and a `#[test]` can only ask a container the
    library exports. The lanes used to be literals in the painter -- the details
    card trimmed to `600`, a list row to `ROW_BUFFER_W - 64 - 20`, a grid cell to
    `CELL_BUFFER_W - 24` -- and the one host gate that claimed to bound the app's
    text measured `510`, `150` and `260`, widths no row, cell, card or chip on
    this screen owns. Raise a rung or narrow a container and the suite stayed
    green while the words on the device lost their tails.

    Every lane below is derived from the box it is drawn in, and the remembered
    numbers are refused so they cannot come back as literals.
    """
    lib = fm_lib()
    for derived in (
        "pub const ENTRY_NAME_LANE: usize = ENTRIES_CARD.w - ENTRY_TEXT_X - ENTRY_RIGHT_AIR;",
        "pub const GRID_NAME_LANE: usize = GRID_CELL_W - 2 * GRID_TEXT_INSET;",
        "pub const DETAILS_TITLE_LANE: usize = DETAILS_CARD.w - 2 * DETAILS_INSET;",
        "pub const SCOPE_CAPTION_MAX_W: usize = SCOPE_THIS_PILL.x - SCOPE_TEXT_X"
        " - SCOPE_PILL_CLEARANCE;",
        "pub const CATEGORY_LABEL_LANE: usize = CATEGORY_ROW_X1 - CATEGORY_LABEL_X"
        " - CATEGORY_RIGHT_AIR;",
        "pub const SEARCH_TEXT_LANE: usize = SEARCH_CLEAR.x - SEARCH_TEXT_X"
        " - SEARCH_CLEAR_CLEARANCE;",
        "pub const EMPTY_CARD_LANE: usize = control_label_lane(ENTRIES_CARD.w);",
    ):
        assert derived in lib, f"the lane stopped describing its container: {derived}"

    # The remembered literals are gone from both halves of the app. Checked
    # against the code with the comments out: this file's own prose names the
    # numbers it refuses, and a gate that trips on the explanation is a gate that
    # gets deleted rather than obeyed.
    main_code = code_only(fm_main())
    for remembered in (
        ", 2, 600,",
        ", 1, 600,",
        "CELL_BUFFER_W - 24",
        "ROW_BUFFER_W - 64 - 20",
        "SCOPE_CAPTION_MAX_W: usize = 380",
    ):
        assert remembered not in main_code, f"a lane came back as a literal: {remembered}"
    lib_code = code_only(lib)
    for remembered in (", 2, 510,", ", 1, 150)", ", 1, 260)"):
        assert remembered not in lib_code, f"the fit gate went back to a remembered width: {remembered}"


def test_the_fit_gate_measures_at_the_rung_the_painter_inks():
    """Measuring a run at one rung and drawing it at another measures nothing.

    The gate ellipsized a file name at a typed `2` while the row that shows it
    inked its search result at a typed `1`, and it checked the directory labels at
    a typed `1` against a width the destination slot stopped using the day the
    sheet was re-laid. Each measurement now names the rung the matching draw site
    passes, in both files, so the ladder cannot move one and not the other.
    """
    lib = fm_lib()
    main = fm_main()
    for rung_lane in (
        "font::ellipsize(long_name, ENTRY_NAME_SCALE, ENTRY_NAME_LANE, &mut buf)",
        "font::ellipsize(&held_name, GRID_NAME_SCALE, GRID_NAME_LANE, &mut buf)",
        "font::ellipsize(&held_name, DETAILS_TITLE_SCALE, DETAILS_TITLE_LANE, &mut buf)",
        "font::measure(label, LABEL_RUNG) <= control_label_lane(SHEET_DEST_SLOT_W)",
        "font::measure(btn, LABEL_RUNG) <= control_label_lane(SHEET_BUTTON_W)",
    ):
        assert rung_lane in lib, f"the fit gate stopped measuring the painter's rung: {rung_lane}"

    for painter_site in (
        "font::ellipsize(name, ENTRY_SUB_SCALE, ENTRY_NAME_LANE, &mut name_buf)",
        "font::ellipsize(name, GRID_NAME_SCALE, GRID_NAME_LANE, &mut name_buf)",
        "DETAILS_TITLE_SCALE,\n        DETAILS_TITLE_LANE,",
        "fitted_run(label, LABEL_RUNG, control_label_lane(rect.w), &mut label_buf)",
    ):
        assert painter_site in main, f"the painter stopped using the library's rung and lane: {painter_site}"


def test_a_cut_run_says_so_rather_than_disappearing():
    """`font::ellipsize` answers zero when even the ellipsis will not fit.

    A painter that drew nothing in that case would not be trimming a word, it
    would be dropping it -- the same class of loss as the label that ran past its
    control because `centered_x` left-aligns what it cannot centre. Every centred
    label now goes through the model's `fitted_run`, and the gate beside it
    refuses a container narrower than the mark that says a run was cut.
    """
    lib = fm_lib()
    main = fm_main()
    assert "pub fn fitted_run" in lib
    assert "let len = font::ellipsize(label, scale, lane, buf);" in lib
    assert lib.count("cut without an ellipsis") + lib.count("silent clip") >= 2
    # The centred labels of this app go through the helper rather than straight
    # to the rasteriser.
    assert main.count("fitted_run(") >= 8


def test_the_crumb_row_painted_is_the_crumb_row_tapped():
    """The crumb strip laid its runs out at one rung and its tap handler measured
    them at another, with no edge on either side.

    The painter gated its branch on the remembered columns `(36..684)` and walked
    every crumb, so a component longer than the strip painted past the band that
    answers it while the tap handler still measured the raw label -- at a typed
    `1` -- and answered letters that were never inked. Both halves now walk the
    model's `crumb_step`, which trims a run to the room the strip has left and
    stops when there is none.
    """
    lib = fm_lib()
    main = fm_main()
    assert "pub fn crumb_step(" in lib
    assert "pub const CRUMB_ROW_X0: usize = ENTRIES_CARD.x;" in lib
    assert "pub const CRUMB_ROW_X1: usize = ENTRIES_CARD.x + ENTRIES_CARD.w;" in lib
    assert "let Some(step) = crumb_step(" in main
    start = main.index("fn crumb_at(")
    hit_test = main[start : main.index("fn handle_tap(")]
    assert "crumb_step(label, cursor" in hit_test
    assert "font::measure(label, BREADCRUMB_SCALE)" not in hit_test, (
        "the tap handler went back to measuring the raw label instead of the run"
        " the strip actually inked"
    )
    assert "(36..684)" not in code_only(main), "the crumb row went back to remembered columns"

