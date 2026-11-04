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


def gallery_lib() -> str:
    return read("apps/gallery/src/lib.rs")


def album_card_paint(source: str) -> str:
    """The album grid's own painter, comments and literals out.

    `albums_grid_pixel` in full: the code that cuts each card's plate, places its
    name and its count line and inks both. It is the slice the rung was adopted
    for, so the derived faces are required inside it and the authored boxes it
    replaced are refused inside it.
    """
    code = code_only(source)
    start = code.index("fn albums_grid_pixel(")
    return code[start : code.index("fn screenshots_grid_pixel(", start)]


def album_card_press_band(source: str) -> str:
    """The Albums arm of `handle_tap`, up to the redraw it answers with.

    The arithmetic that decides which card a press landed on -- the same stride
    and the same plate height the painter cut. A card that grew while this band
    stayed authored would be pressable under its own count line and dead above
    it, so this slice is pinned beside the paint.
    """
    code = code_only(source)
    body = code[code.index("fn handle_tap(") : code.index("fn panic(", code.index("fn handle_tap("))]
    start = body.index("GalleryTab::Albums => {")
    return body[start : body.index("return Tap::Redraw(Dirty::All);", start)]


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


# -----------------------------------------------------------------------------
# The person's text size
#
# Settings' "Text size" is a rung on a ladder this binary used to ignore: every
# box was laid out at one hundred per cent, so a reader who raised the setting
# got the same card height while the ink inside it grew. The honest slice that
# was adopted is the album cards -- the four plates on the Albums tab -- because
# their painter, their press band and their scroll extent are all cut from the
# same two numbers. Everything else on this screen (the header, the filter chips,
# the tab row, the media cells and their captions, the timeline rows, the empty
# states, the inspection bar and the viewer's caption plate) is still authored,
# and its ink stays authored with it: growing ink whose box stayed put is the
# clipping defect every gate below refuses.
# -----------------------------------------------------------------------------

def test_the_app_publishes_the_persons_text_size() -> None:
    """A ladder the binary never reads is a ladder nobody climbs.

    The album cards are now derived from the rung the renderer holds, so an app
    that never published Settings' "Text size" would lay them out at the platform
    rung forever and the person's choice would change nothing here. The helper
    existing and compiling was never the failure -- calling it was, twice: once
    before the first frame and once at the top of every wake, because the size
    changes in Settings while this screen is alive.

    WHY the count is pinned: the read is a syscall. One call per wake settles the
    rung for the whole frame; a call inside a paint loop would cost one kernel
    crossing per run of text per frame.
    """
    code = code_only(gallery_main())
    body = code[code.index("fn _start"):]
    boot = body[: body.index("loop {")]
    loop = body[body.index("loop {"):]
    # Before the first frame is laid out, so the opening screen is not drawn at a
    # rung the person has already left and resized a frame later.
    assert "if publish_text_scale() {" in boot, "the rung is not read before the first frame"
    assert "render_gallery_ui(surface, &app);" in boot, "the boot frame is not drawn"
    assert boot.index("if publish_text_scale() {") < boot.index(
        "render_gallery_ui(surface, &app);"
    ), "the first frame is presented before the rung is published"
    # Re-read every wake, and a rung that moved re-solves the scroll extent and
    # repaints the whole surface: the cards, their plates and the grid's content
    # length all move with it, so a band repaint would leave one behind.
    assert "if publish_text_scale() {" in loop, "the loop stopped re-reading the setting"
    assert "app.relayout_for_rung();" in loop, "a moved rung no longer re-solves the extent"
    assert loop.index("if publish_text_scale() {") < loop.index(
        "render_gallery_ui(surface, &app);"
    ), "the repaint stopped redrawing the whole screen at the new rung"
    assert code.count("publish_text_scale()") == 3, (
        "the rung is no longer read exactly once at boot and once per wake -- the "
        "three sites are the definition, the boot read and the wake read"
    )


def test_the_rung_is_read_on_the_stack_and_an_unread_setting_resets_nothing() -> None:
    """Two ways this could quietly lie.

    First, zero heap: the app is `#![no_std]` with no allocator and the read runs
    on the wake edge of the event loop, so the settings buffer is a fixed stack
    array, not a `String`. Second, an unread setting must not fall back to one
    hundred per cent: if the store is busy, the rung the process already holds is
    the honest answer, or the screen would resize under a reader who never
    touched it. And the publish answers whether the rung actually moved, so the
    loop repaints only when the pixels really changed.
    """
    source = gallery_main()
    assert (
        "use interos_std::design::accessibility::{DynamicType, UserTextScale};" in source
    ), "the publisher no longer uses the platform's own shared parser"
    fn = source[source.index("fn publish_text_scale()"):]
    assert 'std::settings_get("display.text_scale", &mut buf)' in fn, (
        "the app stopped asking the platform for the person's text size"
    )
    assert "let mut buf = [0u8; 32];" in fn, "the settings buffer left the stack"
    assert "let next = want.unwrap_or(before);" in fn, (
        "an unread setting fell back to the platform rung instead of the held one"
    )
    assert "DynamicType::new(next).publish() != before" in fn, (
        "the publish stopped answering whether the rung actually moved"
    )


def test_the_album_card_paint_and_its_press_band_move_to_the_rung_together() -> None:
    """The plate, the press and the scroll extent, or the slice is a lie.

    The authored album card was a 180-row plate on a 200-row stride with its name
    at 112 and its count at 146. The painter now answers
    `gallery::album_card_h_at(lvl)` and `gallery::album_stride_at(lvl)`, places
    its runs with the `*_inset_at(lvl)` derivations, the press band divides by the
    same two faces, and the grid's content height is the same stride times its
    rows -- three readers of one number, which is the whole contract: a rung that
    grew the card and left the press band authored would be tappable under the
    count line and dead above it.

    Two refusals keep the fix honest: the authored boxes may not come back into
    these slices -- but they stay declared in the crate, because a gate satisfied
    by deleting the box it meant to forbid forbids nothing -- and the remembered
    pixels may not be re-typed beside the derivations that replaced them.
    """
    main = gallery_main()
    region = album_card_paint(main)
    band = album_card_press_band(main)
    scroll = code_only(main)
    scroll = scroll[scroll.index("fn grid_content_height("):]
    scroll = scroll[: scroll.index("fn scroll_by(")]

    for rung_face in (
        "gallery::draw_level()",
        "gallery::album_card_h_at(lvl)",
        "gallery::album_stride_at(lvl)",
        "gallery::album_title_inset_at(lvl)",
        "gallery::album_count_inset_at(lvl)",
        "gallery::ALBUM_BANNER_H",
        "gallery::fitted_run(",
        "album_text(x, y, x0 + ALBUM_TEXT_X, title_y,",
        "album_text(x, y, x0 + ALBUM_TEXT_X, count_y,",
    ):
        assert rung_face in region, f"the album card stopped using its rung face: {rung_face}"
    assert "if lvl == gallery::PLATFORM_LEVEL {" in region, (
        "the album card stopped delegating to the ladder at the authored rung"
    )

    # The press band answers the same two derivations, with no authored number.
    assert "gallery::album_stride_at(lvl)" in band, "the press band stopped reading the rung"
    assert "gallery::album_card_h_at(lvl)" in band, "the press band kept the authored plate"
    assert "let row = rel_y / stride;" in band, (
        "the press band stopped dividing by the rung's stride"
    )
    assert "rel_y % stride < card_h" in band, "the press band kept the authored card height"

    # The scroll extent travels over the cards the rung actually drew.
    assert "gallery::album_stride_at(gallery::draw_level())" in scroll, (
        "the grid's content height stopped following the rung's stride"
    )

    # Nothing in the three slices may name the authored box or its remembered rows.
    for authored_name in (
        r"\bAUTHORED_ALBUM_CARD_H\b",
        r"\bAUTHORED_ALBUM_STRIDE\b",
        r"\bALBUM_TITLE_INSET\b",
        r"\bALBUM_COUNT_INSET\b",
    ):
        for label, sliced in (("paint", region), ("press band", band), ("scroll extent", scroll)):
            assert re.search(authored_name, sliced) is None, (
                f"{authored_name} -- the box authored at the platform rung -- is back "
                f"in the album {label}"
            )
    for remembered in (r"\b180\b", r"\b200\b", r"\b112\b", r"\b146\b"):
        for label, sliced in (("paint", region), ("press band", band), ("scroll extent", scroll)):
            assert re.search(remembered, sliced) is None, (
                f"the album {label} went back to the authored pixel {remembered} "
                "instead of the rung-aware derivation"
            )

    # The authored boxes still exist in the crate, so the refusal above is a
    # choice the code could undo rather than an absence that proves nothing.
    lib = gallery_lib()
    for anchor in (
        "pub const AUTHORED_ALBUM_CARD_H: usize = 180;",
        "pub const AUTHORED_ALBUM_STRIDE: usize = 200;",
        "pub const ALBUM_TITLE_INSET: usize",
        "pub const ALBUM_COUNT_INSET: usize",
    ):
        assert anchor in lib, f"the crate stopped declaring its anchor: {anchor}"


def test_the_album_derivations_answer_the_authored_pixels_at_every_level() -> None:
    """The derivations are only faithful if every rung answers the authored number.

    Each album face is a `const fn` of a level so the compile-time gates can
    sweep all four selectable rungs, and the point of the `_at(PLATFORM_LEVEL)
    == authored` anchors is that adopting the setting moves no pixel for a reader
    who left it alone. The sweep beside them is what makes the slice provable
    rather than merely plausible: the card holds both of its runs, the stride is
    never shallower than the plate it separates, the plate never falls below the
    height it was drawn with, the two rows still fit inside the grid band, and the
    short edge the press answers stays above the platform's touch floor -- 44 rows
    at every rung, because a bigger letter is not a bigger target.

    If those anchors were relaxed to chase green, the app would resize the screen
    under someone who asked for nothing: the defect class this change must not
    create while fixing the other one.
    """
    lib = gallery_lib()
    main = gallery_main()
    for pin in (
        "const _: () = assert!(album_title_inset_at(PLATFORM_LEVEL) == 112);",
        "const _: () = assert!(album_count_inset_at(PLATFORM_LEVEL) == 146);",
        "const _: () = assert!(album_card_h_at(PLATFORM_LEVEL) == AUTHORED_ALBUM_CARD_H);",
        "const _: () = assert!(album_stride_at(PLATFORM_LEVEL) == AUTHORED_ALBUM_STRIDE);",
        "const _: () = assert!(album_grid_holds_at_every_level());",
        "while level <= MAX_LEVEL as usize {",
        "if !album_card_holds_its_runs(lvl) {",
        "if album_card_h_at(lvl) < AUTHORED_ALBUM_CARD_H {",
        "if album_card_h_at(lvl) < interos_std::design::touch::MIN_TARGET {",
    ):
        assert pin in lib, f"the library lost its rung anchor or its sweep: {pin}"

    # The restatement of the platform's own band is pinned to the platform's own
    # answer at the level it was authored at -- otherwise `rows_at` is a number
    # invented beside the ladder rather than the ladder read at a rung.
    assert "const _: () = assert!(rows_at(ALBUM_TITLE_RUNG, PLATFORM_LEVEL)" in lib, (
        "rows_at is no longer pinned to outline_text::row_band at the authored level"
    )
    assert "row_band(0, ALBUM_TITLE_RUNG).1" in lib
    assert "row_band(0, ALBUM_COUNT_RUNG).1" in lib

    # The binary owns the band the cards sit in, so it proves the stride fits at
    # every rung the ladder can hand it -- including the floor the press answers.
    for pin in (
        "const _: () = assert!(album_grid_fits_the_band());",
        "if ALBUM_ROWS * gallery::album_stride_at(lvl) > GRID_BOTTOM - GRID_TOP {",
        "if gallery::album_card_h_at(lvl) < touch::MIN_TARGET {",
        "while level <= gallery::MAX_LEVEL as usize {",
    ):
        assert pin in main, f"the binary lost its band-fit sweep: {pin}"


def test_measured_runs_are_runtime_faces_and_ladder_geometry_is_const() -> None:
    """The E0015 trap this slice had to avoid, pinned as a shape.

    A box whose width comes from a measured run can only be asked at run time:
    the SDK's glyph advance tables are ordinary functions, so a `const fn` that
    reached for a measured lane would not compile -- and a `const _: assert!` on
    one fails at const-eval instead. Every measured face is therefore a runtime
    `fn`, and the refusal below catches a regression that re-declared one `const`.
    The ladder's own vertical metrics *are* compile-time, so the geometry faces
    that use only those stay `const fn` and the per-level sweeps can prove them.

    The measured faces also have to fall back to the ladder at the authored rung,
    byte for byte, and `fitted_run` must hand the caller the whole label when the
    trim refuses rather than paint an empty plate.
    """
    lib = gallery_lib()

    for runtime in ("run_w_at", "ellipsize_run", "fitted_run"):
        assert f"pub fn {runtime}" in lib, f"`{runtime}` must be a runtime face"
        assert f"pub const fn {runtime}" not in lib, (
            f"`{runtime}` measures a run and cannot be `const` -- the advance tables "
            "are not compile-time, so this would not compile"
        )
    for geo in (
        "later",
        "rows_at",
        "grown",
        "plate_at",
        "cap_at",
        "em_at",
        "line_at",
        "ascent_at",
        "album_title_inset_at",
        "album_count_inset_at",
        "album_card_h_at",
        "album_stride_at",
    ):
        assert f"pub const fn {geo}" in lib, (
            f"`{geo}` is ladder geometry and must be const, or the per-level sweeps "
            "cannot prove it"
        )

    assert "return interos_std::design::font::measure(text, scale);" in lib, (
        "run_w_at stopped delegating to the ladder's own measure at the authored rung"
    )
    assert (
        "return interos_std::design::font::ellipsize(text, scale, max_w, out_buf);" in lib
    ), "ellipsize_run stopped delegating to the ladder's own ellipsize at the authored rung"

    # A run the trim cannot cut is refused (0) rather than inked past the lane, and
    # a refused trim answers with a cut to that lane -- never with the whole label,
    # which is the run the plate has already said it cannot hold.
    trim = lib[lib.index("pub fn ellipsize_run("):]
    assert "if dots_w > max_w || out_buf.len() < 3 {" in trim
    assert "return 0;" in trim[: trim.index("fn longest_fit")]
    assert "let fit_len = longest_fit(text, em, max_w - dots_w, out_buf.len() - 3);" in trim, (
        "the trim's own walk stopped charging the mark's three bytes, so a cut run "
        "could be written past the end of the buffer the painter was handed"
    )
    fitted = lib[lib.index("pub fn fitted_run"):]
    assert "let len = ellipsize_run(label, scale, lane, buf);" in fitted
    assert "let fit = longest_fit(label, em_at(scale, draw_level()), lane, buf.len());" in fitted, (
        "a refused trim is handing the caller the whole label again, so a name wider "
        "than its card prints past the plate that framed it instead of into it"
    )
    assert "buf[..fit].copy_from_slice(&label[..fit]);" in fitted, (
        "the cut is read straight back out of a buffer the refusal never wrote, so a "
        "refused trim would ink the plate's stale bytes instead of the label's front"
    )

    # The album card's own ink is a runtime face that asks the rung for its cap,
    # its band and its outline placement -- and delegates to the shared painter at
    # the authored level so no pixel moves there.
    main = gallery_main()
    assert "fn album_text(" in main
    assert "const fn album_text(" not in main
    body = main[main.index("fn album_text("): main.index("fn append(")]
    assert (
        "if lvl == gallery::PLATFORM_LEVEL {\n        return text(x, y, x0, y0, scale, s, color, bg);"
        in body
    ), "album_text stopped delegating to the authored painter at the platform rung"
    assert "gallery::cap_at(scale, lvl)" in body, "the album ink stopped using the rung's cap"
    assert "outline_text::place_run_cap(" in body, "the album ink stopped placing a rung cap"
    assert "gallery::rows_at(scale, lvl)" in body, (
        "the bitmap fallback stopped growing its band with the rung"
    )


def test_the_album_derivations_floor_at_the_authored_size_at_every_level() -> None:
    """Adoption may only ever grow a box, never shrink one it was drawn with.

    The card's height is a number the crate's own sweep gates and the press path
    both read, so a rung-aware face that could return below the authored value
    would let a gate green-light a plate the painter no longer fills. The floors
    are structural: a stride grows by the rows its rung's box gained *above* the
    authored level -- `saturating_sub`, so it is zero there and never negative --
    a plate is `later(authored, ...)`, so the authored box is its minimum at every
    level, and the row stride is that plate plus the authored air between two
    cards. The clamp that hands back a rung is bounded by the sweep gates; this is
    the bound on the geometry those rungs size.
    """
    lib = gallery_lib()
    assert (
        "rows_at(scale, level).saturating_sub(rows_at(scale, PLATFORM_LEVEL))" in lib
    ), "`grown` lost its saturating floor, so a lower rung could shrink an authored stride"

    card = lib[lib.index("pub const fn album_card_h_at("):]
    assert "plate_at(\n        AUTHORED_ALBUM_CARD_H," in card[:250], (
        "album_card_h_at stopped flooring at the authored plate, so a rung could draw "
        "the card shorter than the screen was built for"
    )

    plate = lib[lib.index("pub const fn plate_at("): lib.index("pub fn run_w_at(")]
    assert "later(authored, inset + rows_at(scale, level) + air)" in plate, (
        "plate_at stopped flooring at the height it was drawn with"
    )

    stride = lib[lib.index("pub const fn album_stride_at("):]
    assert "album_card_h_at(level) + ALBUM_ROW_AIR" in stride[:200], (
        "album_stride_at stopped being the rung's plate plus the authored air, so a "
        "row could be laid out shallower than the card it separates"
    )

    count = lib[lib.index("pub const fn album_count_inset_at("):]
    assert "grown(ALBUM_COUNT_STRIDE, ALBUM_TITLE_RUNG, level)" in count[:250], (
        "the count line stopped following the title box it was stacked under"
    )
