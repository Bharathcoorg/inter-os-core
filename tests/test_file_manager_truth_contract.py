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

    # The label of a raised control is cut and centred once, when the frame builds
    # the plate it stands on, so the site is `plate_label` rather than the painter.
    # The claim is the same: the lane is derived from the control's own width and
    # the measurement is taken at the rung the same plate inks at.
    for painter_site in (
        "ellipsize_run(name, ENTRY_SUB_SCALE, ENTRY_NAME_LANE, &mut name_buf)",
        "ellipsize_run(name, GRID_NAME_SCALE, GRID_NAME_LANE, &mut name_buf)",
        "fitted_run(label, LABEL_RUNG, control_label_lane(rect.w), &mut buf)",
    ):
        assert painter_site in main, f"the painter stopped using the library's rung and lane: {painter_site}"

    # The details card's two slots moved out of the binary and into the model, so
    # the rungs they measure at are pinned there -- see
    # `test_the_details_fit_cuts_when_its_lane_refuses` for why the model, and not
    # the painter, owns this answer.
    for slot_site in (
        "fit_details_slot(\n        &mut view.title,\n        &mut view.title_len,\n"
        "        DETAILS_TITLE_SCALE,\n        DETAILS_TITLE_LANE,",
        "fit_details_slot(\n            &mut view.lines[index],\n            &mut view.line_lens[index],\n"
            "            DETAILS_LINE_SCALE,\n            DETAILS_LINE_LANE,",
    ):
        assert slot_site in lib, (
            f"the details card stopped measuring at the rung its own card draws at: {slot_site}"
        )


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
    assert "buf[..fit].copy_from_slice(&label[..fit]);" in fitted, (
        "the cut is read straight back out of a buffer the refusal never wrote, so "
        "a refused trim would ink the plate's stale bytes instead of the label's front"
    )
    assert "let fit_len = longest_fit(text, em, max_w - dots_w, out_buf.len() - 3);" in trim, (
        "the trim and the hard cut no longer spend the lane's width in one walk"
    )
    assert lib.count("cut without an ellipsis") + lib.count("silent clip") >= 2
    # The centred labels of this app go through the helper rather than straight
    # to the rasteriser.
    assert main.count("fitted_run(") >= 8


# The details card's own fit, sliced out of the library so a pin cannot be
# satisfied by some other function's identical line.
def _slot_body(lib: str) -> str:
    start = lib.index("fn fit_details_slot<const N: usize>(")
    rest = lib[start:]
    return rest[: rest.index("\n}\n") + 3]


def _details_fit_faults(lib: str, main: str) -> list[str]:
    """Every way the details card can go back to keeping a run its lane refused.

    Tagged so the live test reports what it found and the self-test below can name
    the fault each arm is meant to cause.
    """
    faults: list[str] = []
    if "fn fit_details_slot<const N: usize>(" not in lib:
        return ["slot.missing: the model no longer owns one fit for a details slot"]
    body = _slot_body(lib)
    code = code_only(body)

    # The slot's answer comes from `fitted_run`, the one helper that cuts a
    # refusal. A direct `ellipsize_run` call here is the defect: its `0` has no
    # cut behind it.
    if "fitted_run(&slot[..*len], scale, lane, plate)" not in body:
        faults.append("slot.trim: the details slot no longer asks fitted_run its answer")
    if "ellipsize_run(&slot[..*len]" in code:
        faults.append("slot.raw-trim: the details slot calls the trim directly, so a "
                      "refusal has no cut behind it")

    # A refusal answered by keeping what the slot already held.
    if "if n == 0 {\n            return;" in code or "return;" in code.split("fitted_run")[0]:
        faults.append("slot.whole: an early return leaves the slot the label its lane refused")

    # The answer is copied out before the slot is overwritten, and both halves of
    # the slot are written back.
    if "next[..take].copy_from_slice(&run[..take]);" not in body:
        faults.append("slot.copy: the answer is not copied into the slot's own plate")
    if "*slot = next;" not in body or "*len = kept;" not in body:
        faults.append("slot.write-back: the fitted answer never reaches the slot the painter inks")

    # The card fits its title and every line it holds, through the same slot fit.
    fit = lib[lib.index("pub fn fit_details(view: &mut DetailsView)"):]
    fit = fit[: fit.index("\n}\n\n")] if "\n}\n\n" in fit else fit
    for rung_lane in (
        "DETAILS_TITLE_SCALE,\n        DETAILS_TITLE_LANE,",
        "DETAILS_LINE_SCALE,\n            DETAILS_LINE_LANE,",
    ):
        if rung_lane not in fit:
            faults.append(f"fit.rung: the card stopped naming its own rung/lane pair: {rung_lane}")
    if fit.count("let mut plate = [0u8; MAX_DETAILS_LINE];") != 2:
        faults.append("fit.plate: a slot is fitted through a plate that is not the card's own width")

    # The binary's job is to ask for the fit -- and the pre-fix shape, a trim
    # adopted only `if len > 0` into a slot it rewrote by hand, must not come back.
    if main.count("fit_details(&mut view);") < 2:
        faults.append("painter.call: the painter no longer asks the model to fit the card")
    main_code = code_only(main)
    for pre_fix in ("if line_len > 0 {", "view.title = [0u8; 64];"):
        if pre_fix in main_code:
            faults.append(f"painter.whole: the binary fits the card again, adopting only "
                          f"a non-zero trim: {pre_fix}")
    return faults


def test_the_details_fit_cuts_when_its_lane_refuses():
    """A details slot is only ever handed an answer its own lane can draw.

    `ellipsize_run` answers `0` on a refusal, and the card's fit used to read that
    as "adopt nothing" -- so the slot kept the label the card had just said it
    could not hold, and the painter inked it past both insets, over whatever the
    page drew under the card. The rule is that the slot's content comes from
    [`fitted_run`], which is the label whole when the lane carries it, the label
    with a visible mark when it does not, and the leading glyphs the lane holds
    when even the mark will not fit.

    WHY it lives in the model rather than the binary: the app is built with
    `test = false`, so the only place a refusal can be reached by a test is the
    library -- and a refusal is reached by the plate, not the lane, because every
    real lane of this card is wider than the three-byte mark.
    """
    faults = _details_fit_faults(fm_lib(), fm_main())
    assert not faults, faults


def test_the_details_fit_contract_can_fail():
    """Each arm re-creates one lost half of the rule, and names its own fault.

    A pin nobody has broken on purpose is a pin that has not been shown to watch
    anything. Every arm is a mutation of the current source, each guarded by a
    count so an arm cannot silently stop applying.
    """
    lib = fm_lib()
    baseline = _details_fit_faults(lib, fm_main())
    assert not baseline, baseline

    def arm(needle: str, replacement: str, tag: str, expected: str, *,
            main: bool = False, times: int = 1) -> None:
        """Mutate one line of the model (or the binary) and require its fault."""
        source = fm_main() if main else lib
        assert source.count(needle) == times, f"{tag}: the arm's needle is no longer the source's"
        mutated = source.replace(needle, replacement)
        if not main:
            # An arm that stops applying would leave the fault list empty and read
            # as a blind spot rather than a mutation.
            assert mutated != source, f"{tag}: the arm changed nothing"
        faults = _details_fit_faults(mutated if not main else lib, mutated if main else fm_main())
        assert any(fault.startswith(f"{expected}:") for fault in faults), (
            f"{tag} left the contract blind: {faults}"
        )

    # The defect itself: the refusal adopted nothing, so the slot kept the label
    # its lane had just refused.
    arm(
        "        let run = fitted_run(&slot[..*len], scale, lane, plate);\n",
        "        let n = ellipsize_run(&slot[..*len], scale, lane, plate);\n"
        "        if n == 0 {\n            return;\n        }\n"
        "        let run = &plate[..n];\n",
        "whole-label restore",
        "slot.trim",
    )
    # The slot asks the trim directly instead of the helper that cuts.
    arm(
        "let run = fitted_run(&slot[..*len], scale, lane, plate);",
        "let run = &plate[..ellipsize_run(&slot[..*len], scale, lane, plate)];",
        "raw trim",
        "slot.raw-trim",
    )
    # The answer is read out of a plate the refusal never wrote.
    arm(
        "next[..take].copy_from_slice(&run[..take]);",
        "next[..take].fill(0);",
        "no copy",
        "slot.copy",
    )
    # The fitted answer is computed and thrown away.
    arm(
        "    *slot = next;\n    *len = kept;",
        "    let _ = (next, kept);",
        "no write-back",
        "slot.write-back",
    )
    # The card's lines are fitted against a lane the card does not own.
    arm(
        "            DETAILS_LINE_SCALE,\n            DETAILS_LINE_LANE,",
        "            DETAILS_LINE_SCALE,\n            ENTRY_NAME_LANE,",
        "wrong lane",
        "fit.rung",
    )
    # The painter stops asking the model to fit the card at all.
    arm(
        "fit_details(&mut view);",
        "let _ = &mut view;",
        "painter stops fitting",
        "painter.call",
        main=True,
        times=2,
    )


# The offsets a painter adds to a box it did not measure, and the face that owns
# each one now. Every pair is the authored rung's number on the left -- the number
# the app used to write inside the branch that inked the mark.
_GLYPH_OFFSETS = (
    ("home_recents_y0() + 8,", "home_recents_y0() + home_recents_note_dy(),"),
    ("field.y + 26,", "field.y + search_glyph_dy(),"),
    ("            16,\n            mime,", "            entry_glyph_dy(),\n            mime,"),
    ("cell_h, 83, 30, mime, ink)", "cell_h, GRID_GLYPH_X, grid_glyph_dy(), mime, ink)"),
    ("for dy in 0..26usize {", "for dy in 0..GLYPH_ROWS {"),
    ("for dx in 0..34usize {", "for dx in 0..GLYPH_COLS {"),
)


def _paint_pixel_faults(lib: str, main: str) -> list[str]:
    """Every offset that stopped moving with the plate it stands on."""
    faults: list[str] = []
    code = code_only(main)
    for remembered, derived in _GLYPH_OFFSETS:
        if remembered in code:
            faults.append(
                f"paint.pixel: {remembered!r} is added inside the branch again, so the "
                "mark stays put while the plate it stands on grows with the rung"
            )
        if derived not in code:
            faults.append(f"paint.face: {derived!r} is no longer what the painter adds")
    if "const _: () = assert!(authored_paint_pixels_hold_at_every_level());" not in lib:
        faults.append("paint.sweep: the library no longer proves each offset holds its "
                      "plate at every rung")
    return faults


def test_the_marks_that_stand_on_a_grown_plate_move_with_it():
    """A plate the ladder grew and an offset the ladder did not is a drifted layout.

    The recents notice, the field's magnifier, the row's and the cell's glyph and
    the glyph box itself were each placed by a number written beside the call that
    inked them -- `+ 8`, `+ 26`, `16`, `83, 30`, `0..26`, `0..34`. The plates they
    stand on are `plate_at`'s and `entry_row_h_at`'s, so above the authored rung the
    box grew and the mark did not: the glyph glued to the top of a card the person
    had just made taller, its caption -- which does move -- closing on it. Each one
    now answers a `*_at(level)` face, and the library sweeps all four levels to
    prove the mark still sits inside its plate and runs forwards with the rung.
    """
    faults = _paint_pixel_faults(fm_lib(), fm_main())
    assert not faults, faults


def test_the_paint_pixel_contract_can_fail():
    """Each remembered literal, put back, is refused by the gate that replaced it."""
    lib, main = fm_lib(), fm_main()
    assert not _paint_pixel_faults(lib, main)
    for remembered, derived in _GLYPH_OFFSETS:
        assert main.count(derived) == 1, f"{derived!r} is no longer the painter's own line"
        faults = _paint_pixel_faults(lib, main.replace(derived, remembered))
        assert any(f.startswith("paint.pixel:") for f in faults), (
            f"restoring {remembered!r} left the gate blind: {faults}"
        )
    # The sweep gate itself: without it the offsets are unproven at three rungs.
    faults = _paint_pixel_faults(
        lib.replace("const _: () = assert!(authored_paint_pixels_hold_at_every_level());", ""),
        main,
    )
    assert any(f.startswith("paint.sweep:") for f in faults), faults


# The two rows a search result stacks inside one list row, and the plate that has
# to carry both. The painter used to add each offset beside the call that inked
# it, while the plate was the *name's* box -- so the lower run grew by a caption
# box the plate never measured, and `draw_buffer_text` clipped the found-in path's
# own bottom away at every rung, including the one the layout was drawn at.
_ROW_SEARCH_FACES = (
    ("grown(6, ENTRY_SUB_SCALE, lvl),", "entry_search_name_dy(),"),
    ("grown(28, ENTRY_SUB_SCALE, lvl),", "entry_search_path_dy(),"),
)
_ROW_GATE_CHECK = (
    "        if entry_search_ink_bottom_at(level) > entry_row_h_at(level) {\n"
    "            return false;\n"
    "        }\n"
)


def _fn_body(lib: str, signature: str) -> str:
    """One function's own text, so a pin cannot be met by an identical line elsewhere."""
    if signature not in lib:
        return ""
    start = lib.index(signature)
    rest = lib[start:]
    return rest[: rest.index("\n}\n") + 3]


def _row_plate_faults(lib: str, main: str) -> list[str]:
    """Every way a search row's ink can outrun the plate that carries it again."""
    faults: list[str] = []
    plate = _fn_body(lib, "pub const fn entry_row_h_at(level: u8) -> usize {")
    if not plate:
        return ["plate.missing: the library no longer owns the list row's plate"]
    bottom = _fn_body(lib, "pub const fn entry_search_ink_bottom_at(level: u8) -> usize {")
    if not bottom:
        faults.append("plate.missing: the library no longer names the search stack's own foot")

    if "plate_at(ENTRY_ROW_H, ENTRY_NAME_SCALE, level)" not in plate:
        faults.append("plate.name: the row's plate stopped carrying the name's own box")
    if "entry_search_ink_bottom_at(level)" not in plate:
        faults.append("plate.bound: the row's plate stopped measuring the search stack it carries")
    if "later(" not in plate:
        faults.append("plate.later: the row's plate takes one bound, not the taller of two")
    if bottom and "rows_at(ENTRY_SUB_SCALE, level)" not in bottom:
        faults.append("plate.stack: the plate's bound is the run's offset alone, not the box it inks")

    code = code_only(main)
    for remembered, derived in _ROW_SEARCH_FACES:
        if remembered in code:
            faults.append(
                f"plate.literal: {remembered!r} is added inside the branch again, so the "
                "run moves while nothing else in the app knows the row grew"
            )
        if derived not in code:
            faults.append(f"plate.face: {derived!r} is no longer the row the painter stacks on")

    gate = _fn_body(lib, "const fn authored_paint_pixels_hold_at_every_level() -> bool {")
    if _ROW_GATE_CHECK not in gate:
        faults.append("plate.sweep: the library no longer proves the search stack holds its "
                      "plate at every rung")
    return faults


def test_the_search_stack_stands_inside_the_plate_that_carries_it():
    """A list row's plate is the taller of the name's box and the search stack's foot.

    Both are the same row: a browsing row inks one name at the body rung, a search
    row inks two runs at the caption rung, and `entry_row_h_at` answers for both --
    it is the row's rect, the buffer it is blitted through and the divisor the hit
    test taps with. A plate sized from the name alone was two rows short at the
    rung the layout was authored for and up to ten short at the largest, so the
    found-in path lost its own bottom to the clip and the tap band still answered
    the shorter row.
    """
    faults = _row_plate_faults(fm_lib(), fm_main())
    assert not faults, faults


def test_the_row_plate_contract_can_fail():
    """Each way the defect comes back is refused by the pin that replaced it."""
    lib, main = fm_lib(), fm_main()
    assert not _row_plate_faults(lib, main)

    def arm(needle: str, replacement: str, tag: str, *, in_main: bool = False):
        source = main if in_main else lib
        assert source.count(needle) == 1, f"{needle!r} is not the one line the arm replaces"
        mutated = source.replace(needle, replacement)
        assert mutated != source, f"{tag}: the arm changed nothing"
        faults = _row_plate_faults(lib if in_main else mutated, mutated if in_main else main)
        assert any(f.startswith(f"{tag}:") for f in faults), (
            f"{tag} left the contract blind: {faults}"
        )

    for remembered, derived in _ROW_SEARCH_FACES:
        arm(derived, remembered, "plate.literal", in_main=True)
    # The plate goes back to the name's box alone, and the bound stops measuring
    # the box the lower run inks.
    arm("        entry_search_ink_bottom_at(level),\n", "", "plate.bound")
    arm(
        "    entry_search_path_dy_at(level) + rows_at(ENTRY_SUB_SCALE, level)",
        "    entry_search_path_dy_at(level)",
        "plate.stack",
    )
    # The swept gate that proves it at the three rungs nobody draws by default.
    arm(_ROW_GATE_CHECK, "", "plate.sweep")



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
    # The painter's half: the band the frame hands the region test, measured with
    # the same two calls in the same order. Once this was one `if` inside the
    # painter; the pass now resolves it into the frame before the first pixel, so
    # the pair is pinned where the band is derived and where the pixel reads it.
    assert "status_y0: status_y(),\n        status_y1: status_y() + status_rows()," in main, (
        "the surface stopped painting the status rows the ladder derives, so the "
        "tick's band no longer matches what is on it"
    )
    assert "(frame.status_y0..frame.status_y1).contains(&y)" in main, (
        "the status band reached the pixel test by a route the frame did not derive"
    )
    assert main.count("status_y() + status_rows()") == 1, (
        "the band's foot is derived in more than one place again -- one for the "
        "paint, one for the repaint, and they drift"
    )
    assert main.count("status_top + status_rows()") == 1, (
        "the repaint no longer carries the tick's own half of the band"
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

