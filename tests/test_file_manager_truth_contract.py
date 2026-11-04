# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
    lib = fm_lib()
    assert "outline_text::place_run" in source
    # The advance a run is laid out with now lives in the library's rung-aware
    # `drawn_w`, because the width has to be measured at the rung the painter
    # inks and the painter's rung is one number the binary no longer hard-codes.
    # Both halves are pinned: a `drawn_w` the painter stopped calling, or a
    # `drawn_w` that stopped answering from the outline face, is the soft
    # magnified glyph this gate was written for.
    assert "drawn_w(" in source, "the painter stopped asking the library for its advance"
    assert "outline_text::drawn_advance" in lib, (
        "the platform rung's advance stopped coming from the outline face, so the "
        "painter measures a run it does not ink"
    )
    # Above the platform rung `drawn_w` charges the same face at the raised em,
    # through `run_w_at` -- the ladder's own run measure. A `drawn_w` that grew a
    # third model here would agree with the painter at rung 1 and disagree above it.
    assert "let w = run_w_at(text, scale, lvl);" in lib, (
        "the raised rung's advance stopped coming from the outline face's own "
        "metrics, so `drawn_w` is measuring a run it does not ink"
    )
    # The trim walks the same face, per codepoint where the run decodes and per
    # byte where it does not -- the split `typeface::text_advance_px` makes once
    # for the whole slice. Both arms are pinned because a trim that charges one
    # model and cuts with the other leaves a run past the lane it was cut to.
    assert "advance_codepoint(ch as u32, em)" in lib, (
        "the trim stopped charging the decodable run codepoint by codepoint"
    )
    assert "typeface::advance_px(text[next], em)" in lib, (
        "the trim stopped charging the undecodable run byte by byte"
    )


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

    The painter's half moved from `font::ellipsize` to the library's
    `ellipsize_run` when this app started answering the person's text size, and
    that is the same claim rather than a weaker one: at the authored rung
    `ellipsize_run` *is* `font::ellipsize` -- the gate below pins that branch --
    and above it the helper charges the raised em's own advances rather than the
    ladder's, and refuses rather than hand back a run it could not cut to the
    lane. What the rung path is *not* is host-tested: `draw_level()` reads the
    published rung, so a host suite only ever exercises the platform branch, and
    the pins here are the code's shape plus the authored-rung equalities the
    library asserts.
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

    # The rung-aware trim, and the branch that makes it the authored helper at
    # the rung the host can actually run.
    assert "return font::ellipsize(text, scale, max_w, out_buf);" in lib, (
        "`ellipsize_run` no longer falls back to the authored ellipsize at the "
        "platform rung, so the fit gate above measures a different call than the "
        "painter makes"
    )

    for painter_site in (
        "ellipsize_run(name, ENTRY_SUB_SCALE, ENTRY_NAME_LANE, &mut name_buf)",
        "ellipsize_run(name, GRID_NAME_SCALE, GRID_NAME_LANE, &mut name_buf)",
        "ellipsize_run(\n        &view.title[..view.title_len],\n        DETAILS_TITLE_SCALE,\n        DETAILS_TITLE_LANE,",
        "ellipsize_run(\n            &view.lines[index][..view.line_lens[index]],\n            DETAILS_LINE_SCALE,\n            DETAILS_LINE_LANE,",
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

    The trim this app makes is `ellipsize_run`, which is `font::ellipsize` at the
    authored rung and its equal at a raised one, so the promise has to be pinned
    on both branches: a run whose ellipsis will not fit is answered zero rather
    than inked past the lane. What `fitted_run` answered that zero with used to be
    `label` whole -- a refusal that was correct and a fallback that was not, since
    the run handed back is the run that does not fit -- so it now answers with the
    leading glyphs the lane holds, walked in the same em the painter inks at, and
    the width that walk spends is the width the ellipsis was going to be charged
    against. The library's own test sweeps every lane in this file against the
    mark its own label inks at every rung, which is what keeps this fallback a
    bound rather than a route any of this app's words takes.
    """
    lib = fm_lib()
    main = fm_main()
    assert "pub fn fitted_run" in lib
    assert "let len = ellipsize_run(label, scale, lane, buf);" in lib
    trim = lib[lib.index("pub fn ellipsize_run("):]
    assert "return font::ellipsize(text, scale, max_w, out_buf);" in trim
    assert "if dots_w > max_w || out_buf.len() < 3 {\n        return 0;\n    }" in trim, (
        "the raised rung's trim lost the refusal that makes a cut run say so"
    )
    fitted = lib[lib.index("pub fn fitted_run"):]
    assert "let fit = longest_fit(label, em_at(scale, draw_level()), lane, buf.len());" in fitted, (
        "`fitted_run` stopped cutting a refused trim to the lane, so the whole "
        "label is handed back to a container that cannot hold it"
    )
    assert "let fit_len = longest_fit(text, em, max_w - dots_w, out_buf.len() - 3);" in trim, (
        "the trim and the hard cut no longer spend the lane's width in one walk"
    )
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

    The width the tap band is built from has to be the rung-aware one: `crumb_step`
    advances the strip by `run_w` of the run it just trimmed, so a hit test that
    measured the same run with `font::measure` would answer a narrower crumb than
    the one painted as soon as the person raised the text size.
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
    assert "run_w(&buf[..step.len], BREADCRUMB_SCALE)" in hit_test, (
        "the crumb's tap band stopped measuring the inked run at the rung the"
        " strip inks it, so the band and the paint are two widths again"
    )
    assert "(36..684)" not in code_only(main), "the crumb row went back to remembered columns"


def test_the_app_asks_the_platform_for_the_persons_text_size():
    """A ladder the binary never reads is a ladder nobody climbs.

    Every box on this screen is derived from the rung the renderer holds, so an
    app that never publishes the setting lays out at the platform rung forever
    and Settings' "Text size" changes nothing the person can see here. The
    helper existed, compiled, and was never called -- which is why this is pinned
    as a call site and not as a green suite: no host test can tell a rung that
    was read from one that was left at its default.

    Two call sites, and the repaint they imply: once before the first frame is
    laid out, so the opening screen is not drawn at a rung the person has already
    left, and once per wake, because the size is changed in Settings while this
    screen is alive. A moved rung repaints the whole surface -- the header, the
    card, the toolbar and the status strip all move with it, so a band repaint
    would leave a region at the size the person just left behind.
    """
    code = code_only(fm_main())
    body = code[code.index("fn _start"):]
    boot = body[: body.index("loop {")]
    assert "publish_text_scale();" in boot, (
        "the first frame is laid out before the rung is read"
    )
    assert "let rung_moved = publish_text_scale();" in body, (
        "the loop stopped re-reading the setting, so a size change while this "
        "app is open shows nothing until the next cold open"
    )
    site = body.index("if rung_moved && !repainted_wholly && surface > 0 {")
    repaint = body[site : site + 200]
    assert "render_file_manager_ui(surface, &app);" in repaint, (
        "a moved rung no longer repaints the whole screen"
    )
    assert "render_file_manager_band" not in repaint, (
        "a moved rung repaints only a band, so the regions it also moves stay "
        "at the rung the person left"
    )
    # The extent moves with the rung, and before anything paints: the card's
    # height and its rows are derived from the rung, so a scroll state solved at
    # the old one bounds a screen that no longer exists.
    before_poll = body[body.index("let rung_moved = publish_text_scale();"):
                       body.index("std::event_poll(")]
    assert "app.relayout_for_rung();" in before_poll, (
        "the rung moved but the listing kept the scroll extent the previous rung "
        "solved, so the thumb sizes a card that is not there"
    )
    relayout = fm_main()
    start = relayout.index("pub fn relayout_for_rung(")
    relayout_body = relayout[start : relayout.index("\n    }", start)]
    assert "self.scroll = self.scroll_for(self.scroll.offset);" in relayout_body, (
        "the re-layout dropped the reader's place in the listing instead of "
        "re-solving the extent the offset sits in"
    )


# The authored box, and the ladder call that answers a press with its rung-aware
# twin. Each name on the left is geometry the screen had at the platform rung;
# every one of them still anchors the derivation, so the right column is the same
# rectangle re-measured at the rung the person chose.
AUTHORED_BOXES_A_PRESS_MAY_NOT_ANSWER_WITH: list[tuple[str, str]] = [
    ("SEARCH_FIELD", "search_field()"),
    ("SEARCH_CLEAR", "search_clear()"),
    ("SCOPE_THIS_PILL", "scope_this_pill()"),
    ("SCOPE_ALL_PILL", "scope_all_pill()"),
    ("SORT_PILL", "toolbar_pill(0)"),
    ("LIST_PILL", "toolbar_pill(1)"),
    ("GRID_PILL", "toolbar_pill(2)"),
    ("BREADCRUMB_HIT_Y0", "crumb_hit()"),
    ("BREADCRUMB_HIT_Y1", "crumb_hit()"),
    # The toolbar's own row is answered by the pills drawn in it, so the rung-aware
    # name beside the authored top is `crumb_hit()`: the strip that stops at the
    # toolbar is the only press that reads this edge.
    ("TOOLBAR_Y", "crumb_hit()"),
    ("CATEGORY_Y0", "category_y0()"),
    ("CATEGORY_Y1", "category_y1()"),
    ("HOME_RECENTS_Y0", "home_recents_y0()"),
    ("HOME_RECENT_ROW_H", "home_recent_row_h()"),
    ("HOME_BROWSE_BTN", "home_browse_btn()"),
    ("DETAILS_OPEN", "details_open()"),
    ("DETAILS_CLOSE", "details_close()"),
    ("DETAILS_CARD", "details_card()"),
]


def press_region(code: str) -> str:
    """Everything in the binary that turns a finger into a decision.

    `crumb_at` through `long_press_hit`: the tap, long-press and gesture handlers,
    and nothing else. The rung-aware column of the table above is compared against
    this region, because a box the press never answers is not this gate's business.
    """
    return code[code.index("fn crumb_at(") : code.index("fn widen(")]


def test_a_press_is_measured_against_the_box_that_was_painted():
    """A hit test left at the authored rung answers a rectangle nobody inked.

    Every box on this screen is derived from the rung the renderer holds, so the
    press that answers it has to be measured from the same derivation. It was not:
    the details card offered "Open" at `details_open()` and tested the press
    against `DETAILS_OPEN`, so above the authored rung a finger on the word the
    person just read fell outside the authored rect, hit the `!card.contains`
    escape instead, and closed the card. The same split across the search field,
    its clear button, the scope and toolbar pills, the crumb band, the recents
    rows, the browse button and the category band.

    No host test can see this: the crate's suite exercises the derivations in
    `lib.rs` and the binary's handlers only compile-check, so a handler left at
    the authored constant is green at every level. What this gate is, is a textual
    bound on the *name*, and it is deliberately one-sided: the authored constant is
    refused anywhere in the binary -- press site or paint site, because a
    `details_color` that inked the authored rect while the press answered the
    derived one is the same split wearing the other half -- while the rung-aware
    twin is required in the press region. It does not prove the two agree: a
    re-typed number reads as clean here, and the equality of a painted box and the
    box that answers it is pinned instead by the crate's `*_at(PLATFORM_LEVEL) ==
    AUTHORED` anchors and the sweeps beside them.

    The refusals are only as strong as the boxes they could still be written
    against, which is why every authored constant on the left has to stay declared
    in the crate: a gate satisfied by deleting the box it was meant to forbid
    forbids nothing.
    """
    main = fm_main()
    lib = fm_lib()
    binary = code_only(main)
    press = press_region(binary)
    for authored, live in AUTHORED_BOXES_A_PRESS_MAY_NOT_ANSWER_WITH:
        assert re.search(r"\b" + authored + r"\b", binary) is None, (
            f"{authored} -- the box authored at the platform rung -- is back in the "
            f"binary, while the screen inks {live} at the person's rung"
        )
        assert live in press, (
            f"{live} no longer answers a press, so {authored} has no rung-aware "
            "counterpart for this gate to compare against"
        )
        # The authored constant is the ladder's anchor, not a leftover: deleting
        # it would turn the refusals above into vacuous passes. The declaration is
        # matched to its colon, because `pub const SEARCH_CLEAR_CLEARANCE` is not
        # the box `SEARCH_CLEAR` was.
        assert re.search(r"\bpub const " + authored + r"\s*:", lib), (
            f"{authored} stopped existing in the crate, so refusing to use it "
            "proves nothing about the code that could"
        )
    # A count, so the region cannot be shrunk to nothing to pass: 27 rectangles
    # are answered by these handlers and at least a dozen of them are the boxes
    # the ladder moves.
    assert press.count(".contains(") >= 20, (
        "the press region stopped measuring rectangles at all, which would make "
        "every refusal above vacuously true"
    )
    # The name this gate cannot cover: refusing the authored *name* does not
    # refuse the authored *number*. A press re-measured at the platform rung
    # without naming it -- `Rect::new(380, 884, 280, 48)` or
    # `details_close_at(PLATFORM_LEVEL)` -- reads the same here as the parity fix.
    # The rung the renderer holds is the only rung a press may be measured at, so
    # asking the ladder for the platform's is caught by name.
    assert "PLATFORM_LEVEL" not in press, (
        "a press is measured at the rung the machine was built at rather than the "
        "rung the person chose, which is the split this gate exists to refuse"
    )
    # The sheet's name field is the one box the binary composes itself instead of
    # asking the crate for. Paint and press spell it identically today; a third
    # spelling is how the two become two screens again.
    field = "Rect::new(SHEET.x + 20, sheet_name_y(), SHEET.w - 40, sheet_name_h())"
    assert code_only(fm_main()).count(field) == 2, (
        "the sheet's name field is no longer one expression inked and answered "
        "twice from the same line"
    )


def test_a_progress_tick_repaints_the_rows_the_painter_inks():
    """The band a progress tick repaints is a geometry decision, and it was authored.

    `perform_action` writes the status line and repaints only that line, because
    a copy is long and the whole screen would flicker per file. The band it asked
    for was `STATUS_Y` plus one `font::line_height(1)` -- the strip the status row
    occupied at the platform rung. Above it the row grows past that strip, so each
    tick repainted the top of the line and left the tail of the previous file's
    name on the surface underneath.

    Two sites spell this band, and both are pinned: the painter's own region test
    and the tick's repaint call. A fix that moved one and not the other would
    repaint a strip the surface no longer paints, which is the same leftover
    pixels wearing a different hat.
    """
    main = code_only(fm_main())
    action = main[main.index("fn perform_action(") : main.index("fn exit_to_launcher(")]
    site = action.index("let status_top = status_y();")
    band = action[site : site + 200]
    assert (
        "render_file_manager_band(surface, app, status_top, status_top + status_rows())"
        in band
    ), "the progress tick repaints a band the painter did not derive at the person's rung"
    # The painter's half: the region that inks the status run, measured with the
    # same two calls in the same order.
    assert "if (status_top..status_top + status_rows()).contains(&y)" in main, (
        "the surface stopped painting the status rows the ladder derives, so the "
        "tick's band no longer matches what is on it"
    )
    assert main.count("let status_top = status_y();") == 2, (
        "the status band is no longer two sites -- the paint and the repaint -- "
        "written from one derivation"
    )
    assert "STATUS_Y" not in main, (
        "the repaint went back to the authored status strip"
    )
    assert "font::line_height(1)" not in main, (
        "the repaint counted one authored line where the ladder rows the line out"
    )
    # The refusals above are only worth having if the rung-aware pair is real.
    lib = fm_lib()
    assert "pub fn status_y() -> usize {\n    status_y_at(draw_level())\n}" in lib, (
        "`status_y` stopped answering the rung the renderer holds"
    )
    assert "pub fn status_rows() -> usize {\n    rows_at(STATUS_SCALE, draw_level())\n}" in lib, (
        "`status_rows` stopped counting the rows the ladder rows the status line out"
    )

