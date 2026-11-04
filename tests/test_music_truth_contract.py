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


# -----------------------------------------------------------------------------
# The person's text size on the queue's rows
# -----------------------------------------------------------------------------

# The queue's geometry, authored at one hundred per cent, each paired with the
# anchor its derivation is proved against in the crate. The left-hand names are
# refused in the binary -- a painter that read them would size a row at a rung it
# never asked for -- and the right-hand names stay declared, because a refusal
# that only passes because the number was deleted forbids nothing.
#
# `LIST_TITLE_Y` and `LIST_SUB_Y` are the empty-state card's two lines, refused
# here for the same reason as the row's five numbers.
AUTHORED_QUEUE_BOXES = (
    ("TRACKLIST_ROW_H", "AUTHORED_ROW_H"),
    ("ROW_INK_H", "AUTHORED_ROW_INK_H"),
    ("ROW_CTRL_CY", "AUTHORED_ROW_CTRL_CY"),
    ("LIST_TITLE_Y", "AUTHORED_LIST_EMPTY_TITLE_Y"),
    ("LIST_SUB_Y", "AUTHORED_LIST_EMPTY_SUB_Y"),
)

# The faces the queue's rows are derived with. None of them may appear in a
# region this slice did not adopt: ink that grew while its box stayed authored is
# ink clipped by the box.
RUNG_FACES = (
    "music::draw_level()",
    "music::row_h_at(",
    "music::row_ink_h_at(",
    "music::row_ctrl_cy_at(",
    "music::row_title_dy_at(",
    "music::row_sub_dy_at(",
    "music::row_divider_dy_at(",
    "music::list_empty_title_y_at(",
    "music::list_empty_sub_y_at(",
    "row_line_text(",
    "list_centred_text(",
)


def flat(source: str) -> str:
    """Return `source` with every run of whitespace collapsed to one space.

    WHY: a Rust statement the project's own formatter wrapped over three lines is
    the same statement written on one, and a gate that only accepts one shape is a
    gate that fails on formatting.
    """
    return " ".join(source.split())


def music_lib() -> str:
    return read("apps/music/src/lib.rs")


def lib_prose() -> str:
    """The crate's comments as one flat run of prose.

    WHY: the boundary this file checks is a *sentence*, and a sentence the source
    wrapped over three comment lines is the same sentence. A gate that only matched
    one line length would be a gate that failed on wording, not on the promise.
    """
    comments = "\n".join(re.findall(r"//[^\n]*", music_lib()))
    comments = comments.replace("//", " ")
    return " ".join(comments.split())


def painter_head(code: str) -> str:
    """The player's pixels above the queue: art, now-playing, search, transport.

    Starts at the renderer and stops at the one rung read the queue's rows take,
    so the assertions below are about the regions this slice left at the ladder.
    """
    start = code.index("fn get_music_pixel(")
    return code[start : code.index("let lvl = music::draw_level();", start)]


def queue_band(code: str) -> str:
    """The list's rows and its empty state, as the painter lays them out."""
    start = code.index("let lvl = music::draw_level();")
    return code[start : code.index("fn handle_tap(", start)]


def tap_head(code: str) -> str:
    """The presses outside the queue: transport, art, search, editor controls."""
    start = code.index("fn handle_tap(")
    return code[start : code.index("let lvl = music::draw_level();", start)]


def row_tap(code: str) -> str:
    """The press bands of the rows themselves, as `handle_tap` answers them."""
    start = code.index("let lvl = music::draw_level();", code.index("fn handle_tap("))
    return code[start : code.index("Tap::None", start)]


def test_the_app_publishes_the_persons_text_size() -> None:
    """A ladder the binary never reads is a ladder nobody climbs.

    The queue's rows are derived from the rung the renderer holds, so an app that
    never published Settings' "Text size" would lay them out at the platform rung
    forever and the person's choice would change nothing here. The helper existing
    was never the failure -- calling it was. Both call sites are pinned because no
    host test can tell a rung that was read from one left at its default.
    """
    code = code_only(music_main())
    body = code[code.index("fn _start"):]
    boot = body[: body.index("loop {")]
    # Before the first frame is laid out -- and before the scroll extent is solved,
    # because the extent is the row stride times the library's length.
    assert "publish_text_scale();" in boot, "the rung is read after the first frame"
    assert boot.index("publish_text_scale();") < boot.index("music.sync_list_extent();"), (
        "the scroll extent is solved at a rung the app has not published yet"
    )
    assert boot.index("publish_text_scale();") < boot.index("render_music_ui(surface, &music);"), (
        "the first frame is presented before the rung is published"
    )
    # Re-read at every wake, because the size changes in Settings while this screen
    # is alive, and a rung that moved repaints the whole surface: the plate, its two
    # run origins, the reorder centre, the stride, the divider, the extent and the
    # bands a press answers all move with it, so a band repaint would leave part of
    # the chain at the rung the person left.
    loop = body[body.index("loop {"):]
    assert "if publish_text_scale() && surface > 0 {" in loop, (
        "the loop stopped re-reading the setting"
    )
    assert flat(
        "if publish_text_scale() && surface > 0 {\n"
        "            music.sync_list_extent();\n"
        "            render_music_ui(surface, &music);\n"
        "            let _ = std::surface_flip();"
    ) in flat(loop), "a moved rung no longer re-solves the extent and redraws the screen"
    assert loop.index("if publish_text_scale()") < loop.index(
        "match std::event_poll(interest_mask, 150)"
    ), "the rung is only re-read after the event it should have resized for"


def test_the_rung_is_read_on_the_stack_and_an_unread_setting_resets_nothing() -> None:
    """Two ways this could quietly lie.

    First, zero heap: the app is `#![no_std]` with no allocator and the read runs on
    the edge of the event loop, so the settings buffer is a fixed stack array, not a
    `String`. Second, an unread setting must not fall back to one hundred per cent:
    when the store is busy, the rung the process already holds is the honest answer,
    otherwise the screen resizes under a reader who never touched the setting. And
    the publish answers whether the rung actually moved, so a frame that changed no
    pixel does not repaint the surface.
    """
    source = music_main()
    assert "use interos_std::design::accessibility::{DynamicType, UserTextScale};" in source
    fn = source[source.index("fn publish_text_scale()"):]
    assert 'std::settings_get("display.text_scale", &mut buf)' in fn, (
        "the app stopped asking the platform for the person's text size"
    )
    assert "let mut buf = [0u8; 32];" in fn, "the settings buffer left the stack"
    assert "want = UserTextScale::from_settings_value(&buf[..n]);" in fn, (
        "the app stopped using the shared parser, so a spelling the store accepts "
        "would read as unset"
    )
    assert "let next = want.unwrap_or(before);" in fn, (
        "an unread setting fell back to the platform rung instead of the held one"
    )
    assert "DynamicType::new(next).publish() != before" in fn, (
        "the publish stopped answering whether the rung actually moved"
    )


def test_no_pixel_costs_a_settings_call_and_only_the_clamp_sizes_a_row() -> None:
    """What the paint path may ask the platform for, and what it may not.

    The surface is answered one pixel at a time from `get_music_pixel`, so the claim
    worth pinning is not that the rung arrives rarely -- it arrives once for every
    pixel of the queue's band, from a relaxed load of the published rung plus a
    downward walk over this crate's own const arithmetic -- but that no pixel costs a
    syscall. The setting is read in exactly one place, the wake path, and published.

    The second claim is that a row may only ever be sized by the clamped answer.
    `user_text_scale` is the rung before this app has refused what the band cannot
    carry, so a binary that read it directly would paint a row that walks through the
    home box. The ink faces take the rung as an argument rather than asking again.
    """
    code = code_only(music_main())
    main = music_main()
    assert code.count("let lvl = music::draw_level();") == 2, (
        "the rung stopped being read where a band is laid out; either a band lost "
        "its read or the read moved somewhere that cannot size one"
    )
    assert main.count('std::settings_get("display.text_scale"') == 1, (
        "the platform is being asked for the text size in more than one place"
    )
    assert "user_text_scale()" not in code, (
        "the binary reads the published rung directly, bypassing the clamp that "
        "refuses a rung the queue's band cannot carry"
    )
    # The extent the person scrolls against is the stride the screen draws.
    assert "let content_h = self.order_len * music::row_h_at(music::draw_level());" in main, (
        "the scroll extent stopped being solved at the rung's row stride"
    )
    for name in ("fn rung_run_pixel(", "fn rung_fit_run", "fn row_line_text(",
                 "fn list_centred_text("):
        start = code.index(name)
        face = code[start : code.index("\nfn ", start + 1)]
        assert "lvl: u8," in face, f"{name} stopped receiving the rung as an argument"
        assert "draw_level()" not in face, f"{name} started asking the ladder itself"
        assert "settings_get" not in face, f"{name} put a syscall in the paint path"


def test_the_queue_rows_are_sized_inked_and_tapped_from_the_rung() -> None:
    """The slice only counts if the plate, the ink and the press band move together.

    The painter answers `music::row_ink_h_at(lvl)` for the plate it clips,
    `music::row_h_at(lvl)` for the stride, `music::row_title_dy_at(lvl)` and
    `music::row_sub_dy_at(lvl)` for the two run origins, `music::row_ctrl_cy_at(lvl)`
    for the centre the three reorder plates stand on and
    `music::row_divider_dy_at(lvl)` for the hairline under the row; the empty-state
    card answers its two lines from the same family. `handle_tap` divides by the very
    same stride and refuses outside the very same plate -- a row that grew took its
    press band with it, and a tap that landed on the row the last frame drew is a tap
    on the row that opens.

    At the authored level every branch delegates to the painters this app already
    used, in the same order, so a reader who left the setting alone gets byte-for-byte
    the pixels the screen drew before this change existed.
    """
    code = code_only(music_main())
    paint = queue_band(code)
    tap = row_tap(code)

    for rung_face in (
        "let lvl = music::draw_level();",
        "let row_h = music::row_h_at(lvl);",
        "let ink_h = music::row_ink_h_at(lvl);",
        "let ctrl_cy = music::row_ctrl_cy_at(lvl);",
        "music::row_title_dy_at(lvl)",
        "music::row_sub_dy_at(lvl)",
        "music::row_divider_dy_at(lvl)",
        "music::list_empty_title_y_at(lvl)",
        "music::list_empty_sub_y_at(lvl)",
        "music::ROW_TITLE_RUNG",
        "music::ROW_SUB_RUNG",
        "music::LIST_EMPTY_RUNG",
    ):
        assert rung_face in paint, f"the queue stopped using its rung face: {rung_face}"

    # The plate the row is clipped to and the pitch the chain walks at are the rung's.
    assert "let row_y = LIST_TOP + i * row_h - scroll;" in paint
    assert "if row_y + row_h <= LIST_TOP {" in paint
    assert "let row_bottom = (row_y + ink_h).min(LIST_HIT_Y1);" in paint, (
        "the row's plate stopped being the rung's plate, so the clip and the press "
        "band disagree with the ink"
    )
    assert paint.count("row_line_text(") == 2, (
        "one of the row's two runs left the rung-aware ink face"
    )
    assert paint.count("list_centred_text(") == 2, (
        "one of the empty state's two lines left the rung-aware ink face"
    )

    # The press answers the geometry the screen painted, at the same rung.
    assert "let row_h = music::row_h_at(lvl);" in tap, "the press stopped using the rung's stride"
    assert "let idx = ry / row_h;" in tap and "let coff = ry % row_h;" in tap, (
        "the tap handler is still dividing the list at the authored pitch"
    )
    assert "coff < music::row_ink_h_at(lvl)" in tap, (
        "the press band stopped being the plate the painter clipped the row to"
    )
    assert "let control_y = coff as isize - music::row_ctrl_cy_at(lvl) as isize;" in tap, (
        "the reorder controls' hit bands stopped following the centre the ink drew"
    )

    # The authored ink path may not come back inside a rung-derived plate.
    assert "font::ellipsize(t.title()" not in paint, (
        "the row's title is trimmed at the ladder again while its plate is derived "
        "from the rung"
    )
    assert "text(x, y, MARGIN + ROW.pad, row_y + 50" not in paint, (
        "the row's duration is inked at an authored row again"
    )

    # The byte-for-byte passthrough at the authored level.
    assert flat(
        "if lvl == music::PLATFORM_LEVEL {\n"
        "        let n = font::ellipsize(label, scale, lane, &mut buf);\n"
        "        return text(x, y, x0, y0, scale, &buf[..n], color, bg);"
    ) in flat(code), "the row's ink stopped delegating to the authored painters"
    assert flat(
        "if lvl == music::PLATFORM_LEVEL {\n"
        "        return if bold {\n"
        "            text_centered_bold(x, y, y0, scale, label, color, bg)\n"
        "        } else {\n"
        "            text_centered(x, y, y0, scale, label, color, bg)"
    ) in flat(code), "the empty state stopped delegating whole to the authored centred faces"


def test_the_authored_queue_boxes_are_refused_in_the_binary() -> None:
    """A derived box and its authored literal cannot both describe one row.

    These are the numbers the queue was drawn with at one hundred per cent. They are
    refused in the binary -- a painter or a press that read them would size a row at a
    rung it never asked for -- and each still exists in the crate as the anchor its
    derivation is proved against, which is what makes the refusal mean something
    rather than merely record an absence.
    """
    main = code_only(music_main())
    lib = music_lib()

    for authored, anchor in AUTHORED_QUEUE_BOXES:
        assert re.search(r"\b" + authored + r"\b", main) is None, (
            f"{authored} -- the box authored at the platform rung -- is back in the "
            "binary, while the queue derives its geometry from the person's rung"
        )
        assert anchor in lib, (
            f"{anchor} was deleted instead of refused: an anchor the derivation of "
            f"{authored} cannot be proved against forbids nothing"
        )

    # The band the rows are clipped against is one number stated in two files, and
    # the binary pins both equalities rather than re-typing either row.
    assert "const _: () = assert!(music::LIST_BAND_TOP == LIST_TOP);" in main
    assert "const _: () = assert!(music::LIST_BAND_BOTTOM == LIST_HIT_Y1);" in main, (
        "the foot the fit gate refuses against is no longer the foot the painter "
        "clips the rows to"
    )


def test_the_row_geometry_anchors_to_the_authored_pixels() -> None:
    """The derivations are faithful only if the authored rung answers the authored number.

    A queue row was drawn as an 84-row plate holding its title at 14 and its duration
    at 50, its three reorder plates on a 24-row centre, its hairline at 88 and the next
    row 96 rows after it, and its empty state stood at 1170 and 1210. Every one of
    those is the right-hand side of an anchor over the derivation, so a change that
    moved a pixel for a reader who never opened Settings fails the build. The air
    between the runs and the plate's foot is solved from the authored pair rather than
    invented, which is what makes the floor, the stride and the hairline three answers
    to one plate.
    """
    lib = music_lib()

    # The restated metrics answer the ladder's own faces at the platform level.
    for ladder_face in (
        "const _: () = assert!(rows_at(ROW_TITLE_RUNG, PLATFORM_LEVEL)\n"
        "    == std::design::outline_text::row_band(0, ROW_TITLE_RUNG).1);",
        "const _: () = assert!(rows_at(ROW_SUB_RUNG, PLATFORM_LEVEL)\n"
        "    == std::design::outline_text::row_band(0, ROW_SUB_RUNG).1);",
        "const _: () = assert!(rows_at(LIST_EMPTY_RUNG, PLATFORM_LEVEL)\n"
        "    == std::design::outline_text::row_band(0, LIST_EMPTY_RUNG).1);",
        "const _: () = assert!(line_at(ROW_TITLE_RUNG, PLATFORM_LEVEL)\n"
        "    == std::design::font::line_height(ROW_TITLE_RUNG));",
        "const _: () = assert!(cap_at(ROW_TITLE_RUNG, PLATFORM_LEVEL)\n"
        "    == std::design::font::cap_for_scale(ROW_TITLE_RUNG));",
        "const _: () = assert!(ascent_at(ROW_TITLE_RUNG, PLATFORM_LEVEL)\n"
        "    == std::design::font::ascent_for_scale(ROW_TITLE_RUNG));",
        "const _: () = assert!(em_at(ROW_SUB_RUNG, PLATFORM_LEVEL)\n"
        "    == std::design::font::em_for_scale(ROW_SUB_RUNG));",
    ):
        assert flat(ladder_face) in flat(lib), (
            f"a rung-derived metric stopped being proved against the platform's own face: "
            f"{ladder_face[:60]}"
        )

    # The pixels this screen has always drawn.
    for anchor in (
        "const _: () = assert!(row_title_dy_at(PLATFORM_LEVEL) == AUTHORED_ROW_TITLE_DY);",
        "const _: () = assert!(row_sub_dy_at(PLATFORM_LEVEL) == AUTHORED_ROW_SUB_DY);",
        "const _: () = assert!(row_ctrl_cy_at(PLATFORM_LEVEL) == AUTHORED_ROW_CTRL_CY);",
        "const _: () = assert!(row_ink_h_at(PLATFORM_LEVEL) == AUTHORED_ROW_INK_H);",
        "const _: () = assert!(row_h_at(PLATFORM_LEVEL) == AUTHORED_ROW_H);",
        "const _: () = assert!(row_divider_dy_at(PLATFORM_LEVEL) == AUTHORED_ROW_DIVIDER_DY);",
        "const _: () = assert!(list_empty_title_y_at(PLATFORM_LEVEL) == AUTHORED_LIST_EMPTY_TITLE_Y);",
        "const _: () = assert!(list_empty_sub_y_at(PLATFORM_LEVEL) == AUTHORED_LIST_EMPTY_SUB_Y);",
        "const _: () = assert!(AUTHORED_ROW_BOTTOM_AIR == 4);",
        "const _: () = assert!(AUTHORED_ROW_GAP == 12);",
        "const _: () = assert!(AUTHORED_ROW_DIVIDER_AIR == 4);",
    ):
        assert flat(anchor) in flat(lib), f"an authored pixel stopped being anchored: {anchor}"

    # The authored literals themselves, so a relaxed anchor cannot be satisfied by
    # moving the number it is proved against.
    for literal in (
        "pub const AUTHORED_ROW_TITLE_DY: usize = 14;",
        "pub const AUTHORED_ROW_SUB_DY: usize = 50;",
        "pub const AUTHORED_ROW_CTRL_CY: usize = 24;",
        "pub const AUTHORED_ROW_INK_H: usize = 84;",
        "pub const AUTHORED_ROW_H: usize = 96;",
        "pub const AUTHORED_ROW_DIVIDER_DY: usize = 88;",
        "pub const AUTHORED_LIST_EMPTY_TITLE_Y: usize = 1170;",
        "pub const AUTHORED_LIST_EMPTY_SUB_Y: usize = 1210;",
    ):
        assert literal in lib, f"an authored pixel left the crate: {literal}"

    # Every raised face is the authored stride plus the rows the rung's box gained --
    # a face that invented its own arithmetic would not be a derivation of the anchor.
    for derivation in (
        "grown(AUTHORED_ROW_SUB_DY, ROW_TITLE_RUNG, level)",
        "grown(AUTHORED_ROW_CTRL_CY, ROW_TITLE_RUNG, level)",
        "grown(AUTHORED_LIST_EMPTY_SUB_Y, LIST_EMPTY_RUNG, level)",
        "row_ink_h_at(level) + AUTHORED_ROW_GAP",
        "row_ink_h_at(level) + AUTHORED_ROW_DIVIDER_AIR",
    ):
        assert derivation in lib, f"a row face stopped being derived from its anchor: {derivation}"


def test_the_ladder_sweeps_every_rung_the_ladder_can_be_handed() -> None:
    """A derivation proved only at the rung it was written from is unproven elsewhere.

    The gates sweep the clamp, not the raw rung, because the clamp is what the painter
    is handed, and they run to `MAX_LEVEL` because a sweep over a level no renderer can
    receive would be no sweep at all. The clamp must never raise a rung the band cannot
    carry, must never touch the authored one -- otherwise the app resizes the screen for
    a reader who asked for nothing -- and every rung it hands back must still hold its
    two runs, its three reorder plates and its empty state inside the band it is drawn in.
    """
    lib = music_lib()

    assert "pub const PLATFORM_LEVEL: u8 = std::design::font::TEXT_SCALE_NORMAL;" in lib
    assert "pub const MAX_LEVEL: u8 = 3;" in lib, (
        "the sweep's ceiling moved away from the ladder's largest selectable level"
    )

    assert "const _: () = assert!(draw_level_holds_at_every_level());" in lib
    assert "const _: () = assert!(queue_rows_hold_at_every_level());" in lib
    assert "while want <= MAX_LEVEL {" in lib, "the clamp is no longer swept to the top rung"
    assert "while level <= MAX_LEVEL as usize {" in lib, (
        "the queue's rows are no longer swept to the top rung"
    )

    gate = lib[lib.index("const fn draw_level_holds_at_every_level()"):]
    assert "if draw_level_at(PLATFORM_LEVEL) != PLATFORM_LEVEL {" in gate, (
        "the clamp could move the authored rung, so a reader who never opened "
        "Settings would get a resized screen"
    )
    assert "if chosen > want {" in gate, "the clamp could raise a rung the band cannot carry"
    assert "if !list_holds_at(chosen) {" in gate, (
        "the sweep stopped checking the fit at the rung the clamp actually hands back"
    )

    # The clamp walks down, because the plate grows with the rung and never shrinks.
    walk = lib[lib.index("pub const fn draw_level_at("):]
    assert "while level > 0 && !list_holds_at(level) {" in walk
    assert "level -= 1;" in walk[:300], "the clamp no longer walks down to a rung that fits"


def test_the_row_never_shrinks_below_the_authored_plate_and_stays_inside_its_band() -> None:
    """Adoption may only ever grow a box, never shrink one it was drawn with.

    The plate is also the row's press band and the clip the painter cuts it against, so
    a rung below the authored one may draw smaller ink but must not shorten the reach of
    the row it selects -- the platform's own 44-pixel floor holds at every rung. And the
    plate grows without limit if nothing refuses it, so the fit gate and the sweep both
    refuse a row whose plate would walk past the home box the list is clipped to, and an
    empty state whose second line would land below it.
    """
    lib = music_lib()

    ink = lib[lib.index("pub const fn row_ink_h_at(") : lib.index("pub const fn row_h_at(")]
    assert "later(\n        AUTHORED_ROW_INK_H," in ink[:400], (
        "row_ink_h_at stopped flooring at the authored plate, so a lower rung could "
        "draw the row shorter than the screen was built with"
    )
    # The plate is the two runs and their air, not one of them: a plate that stopped
    # reserving the duration run's own rows would let a raised rung ink past the bottom
    # of the row it was cut for, and the press band is that same bottom.
    assert "row_sub_dy_at(level) + row_sub_rows_at(level) + AUTHORED_ROW_BOTTOM_AIR" in ink, (
        "the plate stopped reserving the duration run's rows, so the rung's ink could "
        "outrun the plate and the press band built on it"
    )

    fits = lib[lib.index("const fn row_band_fits("):]
    assert "row_ctrl_cy_at(level) >= std::design::touch::MIN_TARGET / 2" in fits[:900], (
        "the fit gate stopped refusing reorder plates that ride above the band's ceiling"
    )
    assert "row_ctrl_cy_at(level) + std::design::touch::MIN_TARGET / 2 <= ink_h" in fits[:900], (
        "a control's plate could hang out of the row it belongs to"
    )
    assert "ink_h >= std::design::touch::MIN_TARGET" in fits[:900], (
        "the fit gate stopped refusing a plate below the platform's touch floor"
    )
    assert "ink_h <= LIST_BAND_BOTTOM - LIST_BAND_TOP" in fits[:900], (
        "the fit gate stopped refusing a row that does not fit the band it is drawn in"
    )
    # The two runs of a row may not be refused for overlapping *bands*: the duration
    # line begins under the title's baseline, and the authored row already answered
    # that, so the gate is walked on baselines rather than box edges.
    assert "row_title_dy_at(level) + ascent_at(ROW_TITLE_RUNG, level) <= row_sub_dy_at(level)" in fits[:900], (
        "the row's fit gate stopped being the baseline-preserving one the authored row meets"
    )

    sweep = lib[lib.index("const fn queue_rows_hold_at_every_level()"):]
    assert "if row_ink_h_at(lvl) < std::design::touch::MIN_TARGET {" in sweep, (
        "the sweep stopped checking the touch floor at every rung"
    )
    assert "if row_ink_h_at(lvl) > LIST_BAND_BOTTOM - LIST_BAND_TOP {" in sweep, (
        "the sweep stopped checking the band at every rung"
    )
    assert "if row_divider_dy_at(lvl) >= row_h_at(lvl) {" in sweep, (
        "a hairline could be drawn on the next row's plate instead of in the gap"
    )
    assert "if list_empty_sub_y_at(lvl) + rows_at(LIST_EMPTY_RUNG, lvl) > LIST_BAND_BOTTOM {" in sweep, (
        "the empty state's second line is no longer refused below the band's foot"
    )

    # The floor is the platform's, not a number this app remembers.
    assert "pub const MIN_TARGET: usize = 44;" in read("sdk/rust/interos-std/src/design/mod.rs")


def test_measured_runs_are_runtime_faces_and_ladder_geometry_is_const() -> None:
    """The E0015 trap this slice had to avoid, pinned as a shape.

    A box whose width comes from a measured run can only be asked at run time: the
    SDK's glyph advance tables are ordinary functions, so a `const fn` that reached for
    a measured lane would not compile -- and a compile-time anchor over such a face
    fails at const-eval instead. Every measured face is therefore a runtime `fn`, and
    the refusals below catch a regression that re-declared one `const`. The ladder's
    own vertical metrics *are* compile-time, so the geometry faces that use only those
    stay `const fn` and the per-rung sweeps can prove them.

    The measured faces fall back to the ladder at the authored rung, byte for byte, and
    a run whose trim cannot fit its mark is refused with 0 and answered with a hard cut
    whose bytes are copied into the caller's buffer -- never the untouched label, and
    never a slice read back out of a buffer the refusal never wrote.
    """
    lib = music_lib()

    for runtime in ("run_w_at", "ellipsize_run", "longest_fit"):
        assert f"pub fn {runtime}" in lib, f"`{runtime}` must be a runtime face"
        assert f"pub const fn {runtime}" not in lib, (
            f"`{runtime}` measures a run and cannot be `const` -- the advance tables "
            "are not compile-time, so this would not compile"
        )
    for geo in (
        "later",
        "cap_at",
        "line_at",
        "ascent_at",
        "em_at",
        "rows_at",
        "grown",
        "row_title_rows_at",
        "row_sub_rows_at",
        "row_title_dy_at",
        "row_sub_dy_at",
        "row_ctrl_cy_at",
        "row_ink_h_at",
        "row_h_at",
        "row_divider_dy_at",
        "list_empty_title_y_at",
        "list_empty_sub_y_at",
        "draw_level_at",
    ):
        assert f"pub const fn {geo}" in lib, f"`{geo}` is ladder geometry and must be const"

    # The ladder's own answers, at the authored rung.
    assert "return std::design::font::measure(text, scale);" in lib, (
        "run_w_at stopped delegating to the ladder's own measure at the authored rung"
    )
    assert "return std::design::font::ellipsize(text, scale, max_w, out_buf);" in lib, (
        "ellipsize_run stopped delegating to the ladder's own ellipsize at the "
        "authored rung, so the default screen would trim a run differently than it "
        "always did"
    )
    trim = lib[lib.index("pub fn ellipsize_run("):]
    assert "if dots_w > max_w || out_buf.len() < 3 {" in trim
    assert "return 0;" in trim[:400], (
        "a run the trim cannot cut inside its lane is being inked anyway, past the "
        "edge of the row it was trimmed for"
    )
    assert "let fit_len = longest_fit(text, em, max_w - dots_w, out_buf.len() - 3);" in trim, (
        "the trim's own walk stopped charging the mark's three bytes, so a cut run "
        "could be written past the end of the buffer the painter was handed"
    )
    # The rung arrives as an argument, so the trim never asks the platform for it.
    assert "pub fn ellipsize_run(\n    text: &[u8],\n    scale: usize,\n    max_w: usize,\n    lvl: u8," in lib, (
        "ellipsize_run started reading the rung itself instead of receiving it, so "
        "the paint path would ask the platform per run"
    )

    # The refusal is answered with the run's own bytes, copied into the caller's buffer.
    main = music_main()
    fit = main[main.index("fn rung_fit_run"):]
    fit = fit[: fit.index("\nfn ", 1)]
    assert "let n = music::ellipsize_run(label, scale, lane, lvl, buf);" in fit
    assert "let fit = music::longest_fit(label, music::em_at(scale, lvl), lane, buf.len());" in fit, (
        "a refused trim is handing the painter the whole run again, so a title wider "
        "than its lane prints past the row that framed it"
    )
    assert "buf[..fit].copy_from_slice(&label[..fit]);" in fit, (
        "the cut is read straight back out of a buffer the refusal never wrote, so a "
        "refused row would ink the stack's stale bytes instead of the run's front"
    )
    assert "&buf[..fit]" in fit, (
        "the hard cut is not what the caller is handed back"
    )

    # The ink faces ask the rung for their cap, their band and their placement, and
    # grow the bitmap band with the level so a scaled run keeps its descenders.
    ink = main[main.index("fn rung_run_pixel("):]
    ink = ink[: ink.index("\nfn ", 1)]
    assert "let cap = music::cap_at(scale, lvl);" in ink
    assert "outline_text::place_run_cap(x0, y0, cap, fitted, None, bold)" in ink
    assert "y0 + music::rows_at(scale, lvl)" in ink, (
        "the bitmap fallback stopped growing its band with the rung, so an enlarged "
        "run would lose its descenders at the authored cell's edge"
    )
    assert "std::design::monochrome::text_px(" in ink
    assert "let mut buf = [0u8; 48];" in main[main.index("fn row_line_text("):], (
        "the row's trim buffer left the stack"
    )


def test_the_rung_stops_at_the_slice_this_app_declared() -> None:
    """The scope of this change is stated, and the boxes outside it are proved untouched.

    Only the queue's rows and the list's empty state were adopted, because only there
    does a plate have vertical slack the person's rung can spend. The artwork card and
    its caption, the now-playing stack, the search row, the sort and import controls,
    the availability caption, the timeline, the volume row, the transport discs, the
    queue header, the publish/ringtone/add controls and the add editor keep the pixels
    they were drawn with, and their press bands keep them too -- a region whose ink grew
    while its box stayed authored is a region that clips what it enlarged.
    """
    code = code_only(music_main())

    for region, body in (
        ("the renderer above the queue", painter_head(code)),
        ("the presses outside the queue", tap_head(code)),
    ):
        for face in RUNG_FACES:
            assert face not in body, (
                f"{region} now derives geometry from the person's rung, but its boxes "
                "were authored at the ladder: adopt the region end to end or leave its "
                "ink at the ladder too"
            )

    # The row's lane is still authored arithmetic: the three reorder plates are the
    # platform's floor-sized squares and a person's text size does not widen the
    # columns they stand in.
    assert "const ROW_TITLE_LANE: usize = CONTENT_W - 220;" in code, (
        "the row's text lane started moving with the rung while the plates it clears "
        "are authored"
    )

    # And the crate states the boundary rather than leaving it to be inferred.
    prose = lib_prose()
    assert "SCOPE, stated rather than hidden" in prose, (
        "the library no longer states which regions of the player the rung reaches"
    )
    assert "keep the pixels they were drawn with" in prose, (
        "the library stopped naming the regions left at the authored ladder"
    )
    assert "the publish/ringtone/add controls and the add editor" in prose, (
        "the stated boundary stopped naming the controls it leaves at the ladder"
    )
    assert "pub const ROW_TITLE_RUNG: usize = crate::theme::type_scale::BODY;" in music_lib()
    assert "pub const ROW_SUB_RUNG: usize = crate::theme::type_scale::CAPTION;" in music_lib()
