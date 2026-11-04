# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Truth contract for the InterOS calendar application.

Calendar draws a person's appointments inside boxes this app authored by hand, so
the one defect class that matters here is the screen disagreeing with itself: a
card inked from a size the painter derived and tapped from a size the painter
remembered opens the event two rows above the one the finger landed on.

Every assertion below pins a rule the text-size change established, and each is a
rule that has to keep holding as the app changes:

- the person's "Text size" is read from the platform and published, once per
  region and never per pixel, on the stack, and never reset by a store that did
  not answer;
- the Agenda cards' plate, run origins, stride and count -- and the tap bands that
  answer them -- come from the same rung-aware faces;
- the derived geometry answers the authored pixels exactly at the level the screen
  was drawn at, so a reader who never touched the setting sees no pixel move;
- the boxes that were *not* adopted stay at the ladder, because growing ink whose
  box stayed authored clips the run it enlarged.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# Comments and string literals are not code: a rung named in a WHY comment or a
# label drawn on the card must not satisfy -- or defeat -- an assertion about the
# painter.
_COMMENT_OR_LITERAL = re.compile(
    r"//[^\n]*"
    r"|/\*.*?\*/"
    r'|"(?:[^"\\]|\\.)*"'
    r'|(?:b|br|r#*)"(?:[^"\\]|\\.)*"#*',
    re.DOTALL,
)

# The Agenda boxes this app authored as literals at the platform rung, each paired
# with the anchor its derivation is proved against in the crate. The left-hand names
# are refused in the binary because the painter and the tap handler now derive them
# from the person's level; the right-hand names stay declared, because a refusal that
# only passes because the number was deleted forbids nothing.
AUTHORED_AGENDA_BOXES = (
    ("AGENDA_CARD_H", "AUTHORED_AGENDA_CARD_H"),
    ("AGENDA_LINE_H", "AUTHORED_AGENDA_LINE_H"),
    ("AGENDA_LINE_PAD", "AGENDA_LINE_PAD"),
    ("AGENDA_LINE_STACK_Y", "AUTHORED_AGENDA_STACK_Y"),
    ("AGENDA_TITLE_Y", "AUTHORED_AGENDA_TITLE_Y"),
    ("AGENDA_CARD_GAP", "AGENDA_CARD_GAP"),
    ("AGENDA_CARD_PITCH", "AUTHORED_AGENDA_PITCH"),
    ("AGENDA_CARD_MAX", "AUTHORED_AGENDA_CARD_MAX"),
)


def code_only(source: str) -> str:
    """Return `source` with comments and string literals removed."""
    return _COMMENT_OR_LITERAL.sub(" ", source)


def flat(source: str) -> str:
    """Return `source` with every run of whitespace collapsed to one space.

    WHY: a Rust line the project's own formatter wrapped over three lines is the
    same statement as one written on a single line, and a gate that only accepts
    one of the two shapes is a gate that fails on formatting.
    """
    return " ".join(source.split())


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def calendar_main() -> str:
    return read("apps/calendar/src/main.rs")


def calendar_lib() -> str:
    return read("apps/calendar/src/lib.rs")


def fn_body(code: str, signature: str, terminator: str) -> str:
    """Return `code` from `signature` up to the first `terminator` after it."""
    start = code.index(signature)
    return code[start : code.index(terminator, start)]


def agenda_paint(code: str) -> str:
    """The Agenda view's chronological cards, as the painter lays them out.

    Starts at the one rung read inside `agenda_view_pixel` and stops at the next
    renderer, so the assertions below are about the cards and nothing else.
    """
    start = code.index("let lvl = calendar::draw_level();")
    return code[start : code.index("fn empty_state_pixel(", start)]


def agenda_tap(code: str) -> str:
    """The press bands of the same cards, as `handle_tap` answers them."""
    start = code.index("ViewMode::Agenda => {")
    return code[start : code.index("Tap::None", start)]


def test_the_app_publishes_the_persons_text_size() -> None:
    """A ladder the binary never reads is a ladder nobody climbs.

    The Agenda cards are now derived from the rung the renderer holds, so an app
    that never published Settings' "Text size" would lay them out at the platform
    rung forever and the person's choice would change nothing here. The helper
    existing and compiling was never the failure -- calling it was. Both call sites
    are pinned because no host test can tell a rung that was read from one left at
    its default.
    """
    code = code_only(calendar_main())
    body = code[code.index("fn _start"):]
    boot = body[: body.index("loop {")]
    # Before the first frame is laid out, so the opening screen is not drawn at a
    # rung the person has already left and resized a frame later.
    assert "publish_text_scale();" in boot, "the rung is read after the first frame"
    assert "render_calendar_ui(surface, &calendar);" in boot, "the boot frame is not drawn"
    assert boot.index("publish_text_scale();") < boot.index(
        "render_calendar_ui(surface, &calendar);"
    ), "the first frame is presented before the rung is published"
    # Re-read at every wake, because the size changes in Settings while this screen
    # is alive, and a rung that moved repaints the whole surface: the plate, its
    # stride, the count the band holds and the rows they answer all move with it, so
    # a band repaint would leave part of the chain at the rung the person left.
    loop = body[body.index("loop {"):]
    assert "if publish_text_scale() && surface > 0 {" in loop, (
        "the loop stopped re-reading the setting"
    )
    assert "if publish_text_scale() && surface > 0 {\n            render_calendar_ui(surface, &calendar);" in loop, (
        "a moved rung no longer redraws the whole screen at the new rung"
    )
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
    the publish returns whether the rung actually moved, so a frame that changed no
    pixel does not repaint the surface.
    """
    source = calendar_main()
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


def test_no_pixel_costs_a_settings_call_and_only_the_clamp_sizes_a_box() -> None:
    """What the paint path may ask the platform for, and what it may not.

    The surface is answered one pixel at a time from `get_calendar_pixel`, so the
    claim worth pinning here is not that the rung arrives rarely -- it arrives once
    for every pixel of the Agenda band, from a relaxed load plus a downward walk
    over this crate's own const arithmetic -- but that no pixel costs a syscall.
    The setting is read exactly once, in the wake path, and published; a pixel that
    asked `settings_get` would put a platform call inside a million-pixel loop.

    The second claim is that a box may only ever be sized by the clamped answer.
    `user_text_scale` is the rung the renderer holds before this app has refused
    what the band cannot carry, so a binary that read it directly would paint a rung
    the Agenda cards overflow. The ink face takes the rung as an argument rather
    than asking for it again, which is what keeps a glyph free of the ladder.
    """
    code = code_only(calendar_main())
    main = calendar_main()
    assert code.count("let lvl = calendar::draw_level();") == 2, (
        "the rung stopped being read where a band is laid out; either a band lost its "
        "read or the read moved somewhere that cannot size one"
    )
    assert main.count('std::settings_get("display.text_scale"') == 1, (
        "the platform is being asked for the text size in more than one place"
    )
    assert "user_text_scale()" not in code, (
        "the binary reads the published rung directly, bypassing the clamp that "
        "refuses a rung the Agenda band cannot carry"
    )
    # The ink face takes the rung as an argument rather than asking for it, so no
    # glyph costs a read of the ladder.
    ink = fn_body(code, "fn agenda_line_text(", "fn get_month_abbr(")
    assert "lvl: u8," in ink, "the agenda ink stopped receiving the rung as an argument"
    assert "draw_level()" not in ink, "a per-pixel face started asking the ladder itself"
    assert "settings_get" not in ink


def test_the_agenda_cards_are_sized_inked_and_tapped_from_the_rung() -> None:
    """The slice only counts if the plate, the ink and the press band move together.

    The painter answers `agenda_card_h_at(lvl)` for the plate, `agenda_line_y_at(n,
    lvl)` for all three run origins, `agenda_card_pitch_at(lvl)` for the stride and
    `agenda_card_max_at(lvl)` for how many cards the band holds, and `handle_tap`
    answers the very same faces -- a card that grew took its press band with it, and
    one that stopped fitting left the band rather than the control under it. The
    chevron that says a row opens sits on the title line's box, so its band grew with
    the line it marks instead of staying over the row.

    At the authored level every branch delegates to the ladder's own calls, so the
    pixels a reader who left the setting alone gets are byte for byte unchanged.
    """
    code = code_only(calendar_main())
    paint = agenda_paint(code)
    tap = agenda_tap(code)

    for rung_face in (
        "calendar::draw_level()",
        "calendar::agenda_card_h_at(lvl)",
        "calendar::agenda_rows_at(lvl)",
        "calendar::agenda_line_y_at(0, lvl)",
        "calendar::agenda_line_y_at(1, lvl)",
        "calendar::agenda_line_y_at(2, lvl)",
        "calendar::agenda_card_pitch_at(lvl)",
    ):
        assert rung_face in paint, f"the Agenda cards stopped using their rung face: {rung_face}"

    # The count the band holds, in the painter and in the press band, so a card that
    # no longer fits is neither inked nor tappable.
    assert "total.min(calendar::agenda_card_max_at(lvl))" in paint, (
        "the painter stopped clipping the chain to what the rung's band holds"
    )
    assert "total.min(calendar::agenda_card_max_at(lvl))" in tap, (
        "the tap handler stopped clipping to the cards the painter actually drew"
    )
    assert "let card_y = calendar::AGENDA_CARDS_Y0 + i * calendar::agenda_card_pitch_at(lvl);" in tap
    assert "if y >= card_y && y < card_y + card_h && x >= 24 && x < W - 24 {" in tap, (
        "the press band stopped being the rung's plate"
    )

    # All three lines ink through the rung-aware face, at the rung they are set in
    # and inside the authored lane, so a wider face cannot walk past the card.
    for origin in ("dt_y", "title_y", "label_y"):
        assert f"agenda_line_text(x, y, lvl, 48, card_y + {origin}, calendar::AGENDA_LINE_RUNG," in paint, (
            f"the Agenda card's {origin} line stopped inking at the rung"
        )
    assert paint.count("agenda_line_text(") == 3
    assert "550," in paint, "the Agenda card's text lane stopped being authored"

    # The chevron's band grew with the line it marks.
    assert "y >= card_y + title_y && y < card_y + title_y + rows" in paint, (
        "the row's chevron stopped following the rung's title line"
    )

    # The authored ink path may not come back inside the rung-derived plate: a
    # ladder-sized run inside a rung-sized card is the clip this change removed.
    assert "test_text_bold(x, y, 48, card_y" not in paint, (
        "the Agenda cards went back to the authored ink face while their plate "
        "is derived from the rung"
    )

    # The byte-for-byte passthrough at the authored level.
    assert "calendar::PLATFORM_LEVEL" in code, "the platform rung stopped being named"
    assert "if lvl == calendar::PLATFORM_LEVEL {\n        return if bold {" in code, (
        "the agenda ink stopped delegating to the authored painters at the "
        "platform rung, so the default screen is no longer byte for byte the one "
        "this app drew before"
    )


def test_the_authored_agenda_boxes_are_refused_in_the_binary() -> None:
    """A derived box and its authored literal cannot both describe one card.

    These are the eight numbers the Agenda cards were drawn with at one hundred per
    cent. They are refused in the binary -- a painter that read them would size a
    card at a rung it never asked for -- and each one still exists in the crate as
    the anchor its derivation is proved against, which is what makes the refusal
    mean something rather than merely record an absence.
    """
    main = code_only(calendar_main())
    lib = calendar_lib()

    for authored, anchor in AUTHORED_AGENDA_BOXES:
        assert re.search(r"\b" + authored + r"\b", main) is None, (
            f"{authored} -- the box authored at the platform rung -- is back in the "
            "binary, while the Agenda cards derive their geometry from the person's rung"
        )
        assert anchor in lib, (
            f"{anchor} was deleted instead of refused: an anchor the derivation of "
            f"{authored} cannot be proved against forbids nothing"
        )

    # The band's floor is one number stated in two files, and the binary pins the
    # equality rather than re-typing the row.
    assert "const _: () = assert!(ACTION_BTN_Y0 == calendar::AGENDA_LIST_Y1);" in main, (
        "the row the Agenda chain is measured against stopped being pinned to the "
        "row the action button starts on"
    )


def test_the_agenda_geometry_anchors_to_the_authored_pixels() -> None:
    """The derivations are only faithful if the authored rung answers the authored number.

    Each Agenda face is a `const fn` of a level so the compile-time gates can sweep
    all four selectable rungs, and the whole point of the `_at(PLATFORM_LEVEL) ==
    authored` anchors is that adopting the setting moves no pixel for a reader who
    left it alone: the card was drawn as a 125-row plate holding its three lines at
    offsets 4, 43 and 82 on a 137-row stride, four cards deep. If those anchors were
    relaxed to chase green, the app would resize the screen under someone who asked
    for nothing -- the exact defect class this change must not create while fixing
    the other one.
    """
    lib = calendar_lib()

    # The derived boxes answer the ladder's own faces at the platform level.
    assert "const _: () = assert!(agenda_rows_at(PLATFORM_LEVEL)" in lib
    assert "== interos_std::design::outline_text::row_band(0, AGENDA_LINE_RUNG).1);" in lib, (
        "the restated row band stopped being proved against the platform's own answer"
    )
    for ladder_face in (
        "const _: () = assert!(line_at(AGENDA_LINE_RUNG, PLATFORM_LEVEL)\n    == interos_std::design::font::line_height(AGENDA_LINE_RUNG));",
        "const _: () = assert!(cap_at(AGENDA_LINE_RUNG, PLATFORM_LEVEL)\n    == interos_std::design::font::cap_for_scale(AGENDA_LINE_RUNG));",
        "const _: () = assert!(ascent_at(AGENDA_LINE_RUNG, PLATFORM_LEVEL)\n    == interos_std::design::font::ascent_for_scale(AGENDA_LINE_RUNG));",
        "const _: () = assert!(em_at(AGENDA_LINE_RUNG, PLATFORM_LEVEL)\n    == interos_std::design::font::em_for_scale(AGENDA_LINE_RUNG));",
    ):
        assert flat(ladder_face) in flat(lib), (
            f"a rung-derived metric stopped being anchored to the ladder's own face: {ladder_face[:60]}"
        )

    # The pixels this screen has always drawn.
    for anchor in (
        "const _: () = assert!(agenda_rows_at(PLATFORM_LEVEL) == AUTHORED_AGENDA_LINE_H);",
        "const _: () = assert!(agenda_card_h_at(PLATFORM_LEVEL) == AUTHORED_AGENDA_CARD_H);",
        "const _: () = assert!(agenda_card_h_at(PLATFORM_LEVEL) == 125);",
        "const _: () = assert!(agenda_stack_y_at(PLATFORM_LEVEL) == 4);",
        "const _: () = assert!(agenda_line_y_at(0, PLATFORM_LEVEL) == 4);",
        "const _: () = assert!(agenda_line_y_at(1, PLATFORM_LEVEL) == 43);",
        "const _: () = assert!(agenda_line_y_at(2, PLATFORM_LEVEL) == 82);",
        "const _: () = assert!(agenda_card_pitch_at(PLATFORM_LEVEL) == 137);",
        "const _: () = assert!(agenda_card_max_at(PLATFORM_LEVEL) == 4);",
    ):
        assert flat(anchor) in flat(lib), f"an authored pixel stopped being anchored: {anchor}"


def test_the_ladder_sweeps_every_rung_the_ladder_can_be_handed() -> None:
    """A derivation proved only at the rung it was written from is unproven elsewhere.

    The gates sweep the clamp, not the raw rung, because the clamp is what the
    painter is handed, and they run to `MAX_LEVEL` because a sweep over a level no
    renderer can receive would be no sweep at all. The clamp itself must never raise
    a rung the band cannot carry, must never touch the authored one -- otherwise the
    app resizes the screen for a reader who asked for nothing -- and every level it
    hands back must still hold three lines inside its plate, one card at least in
    the chain, the whole chain above the action button, and a plate a finger can
    land on.
    """
    lib = calendar_lib()

    # The rung the app was authored at, and the rung the ladder stops at.
    assert "pub const PLATFORM_LEVEL: u8 = interos_std::design::font::TEXT_SCALE_NORMAL;" in lib
    assert "pub const MAX_LEVEL: u8 = 3;" in lib, (
        "the sweep's ceiling moved away from the ladder's largest selectable level"
    )

    assert "const _: () = assert!(draw_level_holds_at_every_level());" in lib
    assert "const _: () = assert!(agenda_chain_holds_at_every_level());" in lib
    assert "while want <= MAX_LEVEL {" in lib, "the clamp is no longer swept to the top rung"
    assert "while level <= MAX_LEVEL as usize {" in lib, (
        "the chain is no longer swept to the top rung"
    )

    gate = lib[lib.index("const fn draw_level_holds_at_every_level()"):]
    assert "if draw_level_at(PLATFORM_LEVEL) != PLATFORM_LEVEL {" in gate, (
        "the clamp could move the authored rung, so a reader who never opened "
        "Settings would get a resized screen"
    )
    assert "if chosen > want {" in gate, "the clamp could raise a rung the band cannot carry"
    assert "if !agenda_band_fits(chosen) {" in gate, (
        "the sweep stopped checking the fit at the rung the clamp actually hands back"
    )

    # The clamp walks down, because the plate grows with the rung and never shrinks.
    walk = lib[lib.index("pub const fn draw_level_at("):]
    assert "while level > 0 && !agenda_band_fits(level) {" in walk
    assert "level -= 1;" in walk[:300], "the clamp no longer walks down to a rung that fits"


def test_the_plate_never_shrinks_below_the_authored_box_and_stays_tappable() -> None:
    """Adoption may only ever grow a box, never shrink one it was drawn with.

    The plate is also the row's press band, so a rung below the authored one may
    draw smaller ink but must not shorten the reach of the row it opens -- the
    platform's own 44-pixel floor holds at every level. The fit gate and the sweep
    both carry it, so a future rung that sized a card below the floor fails the
    compile rather than shipping a row a finger cannot land on.
    """
    lib = calendar_lib()

    card = lib[lib.index("pub const fn agenda_card_h_at("):]
    assert "later(\n        AUTHORED_AGENDA_CARD_H," in card[:400], (
        "agenda_card_h_at stopped flooring at the authored box, so a lower rung "
        "could draw the card shorter than the screen was built with"
    )

    fits = lib[lib.index("const fn agenda_band_fits("):]
    assert "card_h >= crate::theme::MIN_TOUCH_TARGET" in fits[:600], (
        "the fit gate stopped refusing a plate below the platform's touch floor"
    )
    sweep = lib[lib.index("const fn agenda_chain_holds_at_every_level()"):]
    assert "if agenda_card_h_at(lvl) < crate::theme::MIN_TOUCH_TARGET {" in sweep, (
        "the sweep stopped checking the touch floor at every rung"
    )

    # A band that cannot hold one plate answers zero cards rather than ink one
    # through the control under it.
    chain = lib[lib.index("pub const fn agenda_card_max_at("):]
    assert "if card_h > band {" in chain[:400]
    assert "return 0;" in chain[:400], (
        "a rung whose plate does not fit the band would be drawn anyway, over "
        "the action button"
    )
    assert (
        "const _: () = assert!(ACTION_BTN_Y0 == calendar::AGENDA_LIST_Y1);"
        in code_only(calendar_main())
    )

    # The floor itself is the platform's, not a number this app remembers.
    theme = read("apps/calendar/src/theme.rs")
    assert "pub const MIN_TOUCH_TARGET: usize = interos_std::design::touch::MIN_TARGET;" in theme


def test_measured_runs_are_runtime_faces_and_ladder_geometry_is_const() -> None:
    """The E0015 trap this slice had to avoid, pinned as a shape.

    A box whose width comes from a measured run can only be asked at run time: the
    SDK's glyph advance tables are ordinary functions, so a `const fn` that reached
    for a measured lane would not compile -- and a compile-time anchor over such a
    face fails at const-eval instead, which is how a previous attempt at this exact
    change was lost. Every measured face is therefore a runtime `fn`, and the
    refusals below catch a regression that re-declared one `const`. The ladder's own
    vertical metrics *are* compile-time, so the geometry faces that use only those
    stay `const fn` and the per-level sweeps can prove them.

    The measured faces also have to fall back to the ladder at the authored rung,
    byte for byte, and a run the trim cannot cut is refused rather than inked past
    the lane it was cut for.
    """
    lib = calendar_lib()

    for runtime in ("run_w_at", "ellipsize_run"):
        assert f"pub fn {runtime}" in lib, f"`{runtime}` must be a runtime face"
        assert f"pub const fn {runtime}" not in lib, (
            f"`{runtime}` measures a run and cannot be `const` -- the advance tables "
            "are not compile-time, so this would not compile"
        )
    for geo in (
        "rows_at",
        "cap_at",
        "line_at",
        "ascent_at",
        "em_at",
        "later",
        "agenda_rows_at",
        "agenda_card_h_at",
        "agenda_stack_y_at",
        "agenda_line_y_at",
        "agenda_card_pitch_at",
        "agenda_card_max_at",
        "draw_level_at",
    ):
        assert f"pub const fn {geo}" in lib, f"`{geo}` is ladder geometry and must be const"

    # The trim is a runtime face over the rung's own advances, and it delegates to
    # the ladder at the authored level so no lane is cut differently there.
    assert "return interos_std::design::font::measure(text, scale);" in lib, (
        "run_w_at stopped delegating to the ladder's own measure at the authored rung"
    )
    assert "return interos_std::design::font::ellipsize(text, scale, max_w, out_buf);" in lib, (
        "ellipsize_run stopped delegating to the ladder's own ellipsize at the "
        "authored rung, so the default screen would trim a run differently than it "
        "always did"
    )
    trim = lib[lib.index("pub fn ellipsize_run("):]
    assert "if dots_w > max_w || out_buf.len() < 3 {" in trim
    assert "return 0;" in trim[:400], (
        "a run the trim cannot cut inside its lane is being inked anyway, past the "
        "edge of the card it was trimmed for"
    )
    assert "let fit_len = longest_fit(text, em, max_w - dots_w, out_buf.len() - 3);" in trim, (
        "the trim's own walk stopped charging the mark's three bytes, so a cut run "
        "could be written past the end of the buffer the painter was handed"
    )

    # The level arrives as an argument, so the trim never asks the platform for it:
    # the pixel face reads the rung once and hands that answer to every run in the
    # card.
    assert "pub fn ellipsize_run(\n    text: &[u8],\n    scale: usize,\n    max_w: usize,\n    lvl: u8," in lib, (
        "ellipsize_run started reading the rung itself instead of receiving it, so "
        "the paint path would ask the platform per run"
    )

    # The Agenda ink is a runtime face that asks the rung for its cap, its band and
    # its outline placement, and grows its bitmap band with the level so a scaled
    # run keeps its descenders instead of losing them at the unscaled cell's edge.
    main = calendar_main()
    assert "fn agenda_line_text(" in main
    assert "const fn agenda_line_text(" not in main
    ink = main[main.index("fn agenda_line_text(") :]
    assert "calendar::ellipsize_run(text, scale, max_w, lvl, &mut buf)" in ink
    assert "calendar::cap_at(scale, lvl)" in ink, "the agenda ink stopped using the rung's cap"
    assert "outline_text::place_run_cap(" in ink, "the agenda ink stopped placing a rung cap"
    assert "calendar::rows_at(scale, lvl)" in ink, (
        "the bitmap fallback stopped growing its band with the rung, so an enlarged "
        "run would lose its descenders at the authored cell's edge"
    )
    assert "let mut buf = [0u8; 96];" in ink, "the trim buffer left the stack"
    assert "let fit = calendar::longest_fit(text, calendar::em_at(scale, lvl), max_w, buf.len());" in ink, (
        "a refused trim is handing the painter the whole run again, so an agenda line "
        "wider than its card prints past the plate that framed it"
    )
    assert "buf[..fit].copy_from_slice(&text[..fit]);" in ink, (
        "the cut is read straight back out of a buffer the refusal never wrote, so a "
        "refused agenda line would ink the stack's stale bytes instead of its front"
    )


def test_the_rung_stops_at_the_slice_this_app_declared() -> None:
    """The scope of this change is stated, and the boxes outside it are proved untouched.

    Only the Agenda cards' chain was adopted, because only there does a plate have
    vertical slack: the header row has two rows of headroom against its band, the
    month grid is a fixed lattice a taller cell would break, and the section band's
    floor feeds the month list and the authored action button. Everything outside
    the slice therefore still lays out at the ladder, and this gate refuses a rung
    read anywhere but the Agenda cards -- a region whose ink grew while its box
    stayed authored is a region that clips what it enlarged.
    """
    code = code_only(calendar_main())

    rung_faces = (
        "calendar::draw_level()",
        "calendar::agenda_card_h_at(",
        "calendar::agenda_line_y_at(",
        "calendar::agenda_card_pitch_at(",
        "calendar::agenda_card_max_at(",
        "agenda_line_text(",
    )
    authored_regions = (
        "fn month_view_pixel(",
        "fn week_view_pixel(",
        "fn day_view_pixel(",
        "fn tabs_pixel(",
        "fn event_list_pixel(",
        "fn empty_state_pixel(",
        "fn action_button_pixel(",
        "fn details_pixel(",
        "fn flow_pixel(",
    )
    for region in authored_regions:
        start = code.index(region)
        body = code[start : code.index("\nfn ", start + 1)]
        for face in rung_faces:
            assert face not in body, (
                f"{region} now derives geometry from the person's rung, but its "
                "boxes were authored at the ladder: either adopt the region end to "
                "end or leave its ink at the ladder too"
            )

    # The grid's lattice is still authored arithmetic, so the week row cannot be
    # broken by a taller cell.
    month = code[code.index("fn month_view_pixel(") :]
    month = month[: month.index("\nfn ", 1)]
    assert "ROW_PITCH" in month and "COL_W" in month, (
        "the month grid's lattice stopped being the authored fixed lattice this "
        "slice leaves at the ladder"
    )

    # And the crate says so, rather than leaving the boundary to be inferred.
    lib = calendar_lib()
    assert "so the grid stays authored" in flat(lib), (
        "the library no longer states that the month grid is left at the ladder"
    )
    assert "pub const AGENDA_LINE_RUNG: usize = crate::theme::type_scale::BODY;" in lib
