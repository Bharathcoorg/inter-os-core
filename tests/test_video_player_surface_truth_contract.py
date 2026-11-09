# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Surface contract for the InterOS Video Player: what the screen claims, and
whether the pixels and the press both say it.

WHY a second file beside `test_video_player_truth_contract.py`: that contract
holds the platform truth strings, the capability claims and the control geometry
the QEMU suites tap. The six defects this one holds shut are about the rest of
the surface -- a chip that named no function, a mute control that painted a
broken glyph, two time read-outs that painted hyphen clusters, a lower half that
painted nothing at all, and a still image the transport described as a paused
clip.

The video player's bin target carries `test = false`, so neither
`get_video_pixel` nor `handle_touch` can be called from a host test and these
gates read the source. Where the claim is arithmetic -- the shape of a glyph, the
rung a word stands at, whether a duration is a clock at all -- the arithmetic
moved to `apps/video_player/src/lib.rs`, which does have host tests, and these
gates bind the painter to it.
"""

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def video_lib() -> str:
    return read("apps/video_player/src/lib.rs")


def video_main() -> str:
    return read("apps/video_player/src/main.rs")


def _const_value(source: str, name: str) -> int:
    """The integer a `usize` constant evaluates to, read from its own definition.

    WHY evaluate rather than match a literal: `CHIP_PLATE_ROOM` is authored as
    `154 - 2 * 4` and `GRID_4` sits in the SDK's spacing module, so a gate that
    demanded a bare number would be a gate about how the constant was written.
    Measuring the value is what keeps the room from being authored twice.
    """
    match = re.search(
        rf"const\s+{re.escape(name)}\s*:\s*usize\s*=\s*([^;]+);", source
    )
    assert match, f"{name} is no longer a plain usize constant"
    expr = match.group(1).strip()
    assert re.fullmatch(r"[\d\s+\-*/()]+", expr), f"{name} = {expr!r} is not plain arithmetic"
    return int(eval(expr, {"__builtins__": {}}, {}))


def _rect_member(source: str, name: str, member: str) -> int:
    """One coordinate of a `Rect::new(x, y, w, h)` constant, read from `source`."""
    match = re.search(rf"const\s+{re.escape(name)}\s*:\s*Rect\s*=\s*Rect::new\(([^)]*)\)", source)
    assert match, f"{name} is no longer a plain Rect::new constant"
    args = [a.strip() for a in match.group(1).split(",")]
    expr = args[{"x": 0, "y": 1, "w": 2, "h": 3}[member]]
    assert re.fullmatch(r"[\d\s+\-*/()]+", expr), f"{name}.{member} = {expr!r} is not plain arithmetic"
    return int(eval(expr, {"__builtins__": {}}, {}))


class _Unresolved(Exception):
    """An expression that reaches outside the constants this gate can read."""


_TOKEN = re.compile(r"\b([A-Z][A-Z0-9_]*)\.(x|y|w|h)\b|\b([A-Z][A-Z0-9_]*)\b")


def _const_expr(source: str, name: str) -> "str | None":
    """The right-hand side a `usize` or `u32` constant is authored as, or `None`."""
    match = re.search(rf"const\s+{re.escape(name)}\s*:\s*(?:usize|u32)\s*=\s*([^;]+);", source)
    return match.group(1).strip() if match else None


def _eval(source: str, expr: str, seen: "frozenset[str]" = frozenset()) -> "int | None":
    """The number a Rust arithmetic expression carries over the constants here.

    WHY resolve through the names instead of quoting the digits: the row the
    transport words stand on is authored once as arithmetic over the constants the
    painter already uses -- `NEXT_CX - PLAY_CX`, `BAND3_Y1 - TRANSPORT_LABEL_H` --
    and a gate that demanded the spelled-out number would test how the layout was
    written rather than where it lands. What a resolved value cannot survive is
    two spellings of the same distance disagreeing, which is what a quoted literal
    never notices.

    A constant whose value only the type ladder can answer -- `TRANSPORT_LABEL_H`
    is `ascent_at` at the largest level -- makes the whole expression
    unresolvable, and the caller states that one as its derivation instead.
    """

    def resolve(match: "re.Match[str]") -> str:
        rect, member, name = match.groups()
        if rect:
            value = _eval(source, _rect_exprs(source)[rect][member], seen | {rect})
        else:
            rhs = _const_expr(source, name)
            if rhs is None or name in seen:
                raise _Unresolved(name)
            value = _eval(source, rhs, seen | {name})
        if value is None:
            raise _Unresolved(name or rect)
        return str(value)

    try:
        reduced = _TOKEN.sub(resolve, expr)
    except (_Unresolved, AssertionError, KeyError):
        return None
    if not re.fullmatch(r"[\d\s+\-*/()]+", reduced):
        return None
    return int(eval(reduced, {"__builtins__": {}}, {}))


def _usize(source: str, name: str) -> "int | None":
    """[`_eval`] pointed at one `usize` constant's own definition."""
    rhs = _const_expr(source, name)
    return None if rhs is None else _eval(source, rhs, frozenset({name}))


def _rect_exprs(source: str) -> "dict[str, dict[str, str]]":
    """Every `const NAME: Rect = Rect::new(a, b, c, d)` in `source`, by member."""
    found = {}
    for name, raw in re.findall(
        r"const\s+([A-Z_][A-Z0-9_]*)\s*:\s*Rect\s*=\s*Rect::new\(\s*([^)]*)\)", source
    ):
        args = [a.strip() for a in raw.split(",")]
        if len(args) == 4:
            found[name] = dict(zip(("x", "y", "w", "h"), args))
    return found


def _plate(source: str, name: str) -> "dict[str, str]":
    """The four expressions a label plate is built from, through its factory.

    WHY the factory and not the digits: the six transport plates are one function
    called with each control's own centre, step and row, so measuring a plate
    means measuring that call -- and a plate hand-authored back into literals is
    one of the things this gate has to refuse.
    """
    build = re.search(
        rf"const\s+{re.escape(name)}\s*:\s*Rect\s*=\s*(\w+)\(([^)]*)\)\s*;", source
    )
    assert build, f"{name} is no longer a plate built by a factory call"
    factory, raw_args = build.group(1), build.group(2)
    body = re.search(
        rf"const fn {re.escape(factory)}\(([^)]*)\)\s*->\s*Rect\s*\{{\s*Rect::new\(\s*(.*?)\)\s*;?\s*\}}",
        source,
    )
    assert body, f"{factory} is no longer a `Rect::new` factory over named parameters"
    params = [p.split(":")[0].strip() for p in body.group(1).split(",")]
    args = [a.strip() for a in raw_args.split(",")]
    assert len(params) == len(args), f"{name} passes {args} to a factory of {params}"
    local = dict(zip(params, args))
    out = {}
    for member, raw in zip(("x", "y", "w", "h"), body.group(2).split(",")):
        text = raw.strip()
        for param, arg in local.items():
            text = re.sub(rf"\b{re.escape(param)}\b", f"({arg})", text)
        out[member] = text
    return out


def _code_only(source: str) -> str:
    """Strips `//` line comments and block comments from Rust source.

    WHY: these gates assert that a retired mark is gone from the program, and the
    natural way to write why a mark went away is to quote it in a comment.
    Scanning comments would make the contract test the author's prose.
    """
    without_block = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
    return re.sub(r"//[^\n]*", " ", without_block)


def _block(source: str, marker: str) -> str:
    """The braced block that opens after `marker`, matched to its own close.

    WHY a band and not the file: `get_video_pixel` answers one surface row at a
    time out of six `if` blocks, and a gate about the volume row's ink is a gate
    about that block. Asked of the whole file, "no red cross in the speaker
    plate" would fail on the error text the canvas paints two bands above it.
    """
    at = source.find(marker)
    assert at >= 0, f"the painter no longer opens with {marker!r}"
    open_at = source.find("{", at)
    assert open_at >= 0, f"{marker} has no body"
    depth = 0
    for index in range(open_at, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[open_at + 1 : index]
    raise AssertionError(f"{marker} has an unbalanced block")


def test_the_loop_chip_names_the_mode_it_presses():
    """The control row's third plate names its feature and its state together.

    `1.0x  FIT  OFF  PIP` gave three plates a value and one plate a bare state
    word, so the person looking at the row could not tell that the third one
    repeats the playlist. The chip paints `LOOP OFF` / `LOOP ONE` / `LOOP ALL`;
    the state names the platform contract quotes stay `OFF` / `ONE` / `ALL`.
    """
    lib = video_lib()
    main = _code_only(video_main())
    painter = _block(main, "fn get_video_pixel")

    assert "pub const fn chip_label(self) -> &'static str" in lib
    for spelling in ('"LOOP OFF"', '"LOOP ONE"', '"LOOP ALL"'):
        assert spelling in lib, f"the loop chip lost {spelling}"
    # The state names the truth contract pins are still there, unchanged.
    for spelling in ('"OFF"', '"ONE"', '"ALL"'):
        assert spelling in lib

    assert "app.loop_mode.chip_label()" in painter, (
        "the loop plate paints a state word with no feature on it again"
    )
    assert "app.loop_mode.label().as_bytes()" not in painter, (
        "the loop plate is back to painting the bare state name"
    )


def test_a_plate_label_stays_inside_the_plate_at_every_rung():
    """A control's word is trimmed by stepping its rung, never by walking off it.

    WHY the plates cannot grow: they are touch targets whose height the contract
    names as a literal, so the only move left when the person raises Text size is
    the rung. The fleet's own answer is the camera's refusal caption, which steps
    its rung rather than inking the ring beside it.
    """
    lib = video_lib()
    main = _code_only(video_main())
    flat = " ".join(main.split())

    assert "pub fn label_rung(" in lib and "pub fn label_w(" in lib
    assert "const CHIP_LABEL_AIR: usize = spacing::GRID_4;" in flat

    # The plate room the library's own fit test measures and the painter hands
    # `label_rung` are one number, not two that can drift.
    #
    # WHY this is measured and not quoted: the room is a plain arithmetic
    # constant in `lib.rs`, the air is the SDK's grid step, and the plate is a
    # `Rect::new` in `main.rs`. Production imports `CHIP_PLATE_ROOM` into the
    # bin's namespace, so the tie reads as the bare name there -- a gate that
    # demanded one spelling would test the import, not the number. What cannot
    # survive any spelling is the arithmetic disagreeing.
    room = _const_value(lib, "CHIP_PLATE_ROOM")
    air = _const_value(_code_only(read("sdk/rust/interos-std/src/design/mod.rs")), "GRID_4")
    plate_w = _rect_member(main, "SPEED_BTN", "w")
    assert re.search(
        r"const _:\s*\(\)\s*=\s*assert!\(\s*SPEED_BTN\.w\s*-\s*2\s*\*\s*CHIP_LABEL_AIR\s*==\s*"
        r"(?:video_player::)?CHIP_PLATE_ROOM\s*\)",
        main,
    ), "the painter no longer ties the plate room to the library's room at compile time"
    assert plate_w - 2 * air == room, (
        f"the chip's room is authored twice again: {plate_w} - 2 * {air} != {room}"
    )
    # One painter for all six plates, so no control keeps a private fit rule.
    assert main.count("paint_plate_label(") >= 6, (
        "the secondary row is back to six hand-written label placements"
    )
    assert "fn plate_label_rung(" in flat
    # The centring sum now follows the rung the label is drawn at.
    assert "const fn plate_label_dy_at(level: u8, plate_h: usize, rung: usize)" in flat
    assert "const fn plate_label_ink_top_at(level: u8, plate_h: usize, rung: usize)" in flat
    assert "const _: () = assert!(secondary_labels_hold_in_plates_at_every_level());" in flat


def test_the_mute_control_inks_a_speaker():
    """The volume plate draws a horn, not a box with a red cross in it.

    The glyph the screen carried was a plate, four hand-written offsets for a
    cone, and a two-pixel red X on a band of the same colour -- which is what a
    missing glyph looks like. The SDK's `Icon` enum has a mic, a bell and a play
    triangle and no speaker, so the app draws one on the platform's twenty-four
    unit grid, and the geometry lives in `lib.rs` where a host test can ink it.

    WHY the plate blocks are the scope of the orphan hunt: the retired mark is
    retired from the speaker, and the cone it was poked from collides with
    nothing else but the speaker. Asked of the whole file, `dx >= -10 && dx <= -4`
    is Band 3's pause glyph -- two honest bars, drawn where the press answers
    them -- and `0xFFE05A68` is the ink every decoder refusal is painted in. A
    gate that banned either file-wide would be a gate about unrelated geometry.
    The offsets that name only the speaker stay file-wide, so a relocation of the
    same broken glyph onto another plate is still caught.
    """
    lib = video_lib()
    main = _code_only(video_main())
    band4 = _block(main, "if y >= BAND4_Y0 && y < BAND4_Y1 {")
    plates = {
        "portrait": _block(
            main,
            "draw_rounded_rect(x, y, VOL_MUTE_BTN, 12, COLOR_SURFACE, Some(COLOR_BORDER), 1)",
        ),
        "landscape": _block(
            main,
            "draw_rounded_rect(x, y, LAND_MUTE_BTN, 12, COLOR_SURFACE, Some(COLOR_BORDER), 1)",
        ),
    }

    assert "pub fn speaker_hit(" in lib
    assert "pub fn speaker_horn_hit(" in lib
    assert "pub enum SpeakerState" in lib
    assert "pub const ICON_GRID: i32 = 24;" in lib

    assert "speaker_px(" in band4, "the volume plate no longer draws the app's speaker"

    for name, plate in plates.items():
        assert "speaker_px(" in plate, (
            f"the {name} mute plate is back to drawing the speaker its own way"
        )
        assert "SpeakerState" in plate or "speaker_state" in plate, (
            f"the {name} mute plate no longer reads its state from the level"
        )
        assert "0xFFE05A68" not in plate, (
            f"the {name} mute plate is painting an inline cross colour again"
        )
        # The retired cone, in the only places it was ever poked.
        for orphan in ("dx >= -10 && dx <= -4", "spk_dx", "d_arc"):
            assert orphan not in plate, (
                f"the hand-poked speaker is back in the {name} plate: {orphan}"
            )

    # Offsets that belong to the speaker and to nothing else stay gone from the
    # whole file, wherever the glyph is moved to.
    for orphan in (
        "spk_dx >= -8 && spk_dx <= -3",
        "((dx - 10) - dy).abs() <= 1",
        "d_arc >= 64 && d_arc <= 100",
    ):
        assert orphan not in main, f"the hand-poked speaker is back: {orphan}"
    # Portrait and landscape are the same glyph from the same accessor.
    assert main.count("speaker_px(") >= 2, (
        "one orientation is still drawing the speaker its own way"
    )
    assert "SpeakerState::from_volume(" in main
    assert main.count("SpeakerState::from_volume(") >= 2, (
        "one orientation guesses at the speaker's state instead of reading it"
    )


def test_the_scrubber_says_no_running_time():
    """A media with no clock reads as no clock, in words, not as `00:00 --:--`.

    Two claims used to sit on one band: `00:00` beside `--:--`, and a track with
    its thumb at the far left. A person reads that as a clip paused at the start.
    It is not -- `nature_clip.ppm` is one still frame, and a header nobody read
    is the other case -- so the band answers the sentence `NO RUNNING TIME` and
    paints no track and no thumb, and the press on the track is answered only
    while a duration is actually held.

    WHY the same rule is checked in both orientations: the landscape HUD is the
    same transport on a scrim, and a fix that only moved the portrait band would
    leave the rotated screen claiming a paused clip.
    """
    lib = video_lib()
    main = _code_only(video_main())
    painter = _block(main, "fn get_video_pixel")

    # The marker is the library's words, and the app's private copy is gone.
    assert "pub const NO_RUNNING_TIME: &[u8] = b\"NO RUNNING TIME\";" in lib
    assert "pub const NO_TIME_SHORT: &[u8] = b\"NO TIME\";" in lib
    assert "pub fn duration_label(" in lib
    assert "fn duration_label(known: bool, seconds: u32, negative: bool, out: &mut [u8; 12])" not in main, (
        "the app's own marker is back, and it is the one that painted `--:--`"
    )
    assert 'b"--:--"' not in main, "the dash cluster is still in the source"
    assert "duration_label," in main.replace(" ", ""), (
        "main.rs no longer imports the library's running-time words"
    )

    assert painter.count("NO_RUNNING_TIME") >= 2, (
        "only one orientation reads the running time as absent"
    )
    assert "b\"--\"" not in main

    # The transport's two number columns stand only on a media that has a clock.
    assert "if app.duration_sec > 0 {" in painter
    # ...and so does the press that seeks on them.
    touch = _block(main, "fn handle_touch")
    assert re.search(r"if app\.duration_sec > 0 && SEEK_HIT\.contains\(x, y\)", touch), (
        "a seek is answered again on a timeline the app says it does not have"
    )


def test_the_volume_readout_is_a_word_in_a_column_that_holds_it():
    """The level read-out answers three states in words, and the slider gives up
    the columns those words need.

    `--` at the body rung is the same broken-glyph look as the scrubber's marker,
    and the words that replace it are wider than the eighty-six columns the
    read-out had: `UNKNOWN` at the platform's level is a hundred and twenty-seven
    columns. So the column is derived from the ladder -- three body line boxes at
    the largest rung -- and the bar's width and its press band are read back off
    it, which is the same rule the portrait scrubber already states about the
    remaining-time number: a band that runs under the first strokes of a read-out
    is a band that changes the value it is beside.

    WHAT THIS GATE FOUND ALREADY DONE: the column, its start and the bar's width
    are derived in production exactly as the paragraphs above describe, and
    neither the `610` nor the `490` literal survives. Those assertions stay
    because they are the reason the read-out is legible, not because the numbers
    were missing. What was missing is the words' half of the agreement: the fit
    was measured only inside the library's `mod tests`, against a copy of the
    column the screen never reads, so nothing in the binary the phone runs said a
    word still stood in its column. The compile-time helper, the library's own
    column, and the tie between the two spellings are what the last two
    assertions now hold shut.
    """
    lib = video_lib()
    main = _code_only(video_main())
    flat = " ".join(main.split())
    band4 = _block(main, "if y >= BAND4_Y0 && y < BAND4_Y1 {")

    assert "pub fn volume_readout(" in lib
    assert "pub const UNKNOWN_LEVEL: &[u8] = b\"UNKNOWN\";" in lib
    assert "pub const MUTED: &[u8] = b\"MUTED\";" in lib
    assert "volume_readout(" in band4

    assert "const VOL_LABEL_COL: usize = 3 * line_at(2, EXTRA_LARGE_LEVEL);" in flat
    assert "const VOL_LABEL_X: usize = W - 24 - VOL_LABEL_COL;" in flat
    assert "const VOL_LABEL_X: usize = 610;" not in flat, (
        "the read-out is back in eighty-six columns that cannot hold its words"
    )
    assert "const VOL_BAR_W: usize = VOL_LABEL_X - VOL_BAR_AIR - VOL_BAR_X;" in flat
    assert "const VOL_BAR_W: usize = 490;" not in flat
    assert "const _: () = assert!(volume_readout_words_hold_at_every_level());" in flat
    # The helper is the library's, and the column it measures words against is
    # the library's own number -- which `main.rs` ties to the column it paints
    # with a const, so the fit cannot be proven against a width the screen
    # never used.
    assert "pub const fn volume_readout_words_hold_at_every_level() -> bool" in lib
    assert "pub const VOLUME_READOUT_COL: usize =" in lib
    assert "const _: () = assert!(VOL_LABEL_COL == VOLUME_READOUT_COL);" in flat
    # The thumb is inked only where a level was reported, so a level nobody read
    # is not drawn as zero.
    assert "if app.volume_known {" in band4, (
        "an unreported level is painted as a thumb at the end of the track again"
    )


def test_the_surface_below_the_controls_carries_the_library():
    """The rows under the control plate are the device's own tracks, not black.

    WHY the playlist and not a filler panel: the media library is the platform's
    inventory -- `/storage/videos` and `/storage/downloads`, read at launch by
    `rescan_storage` -- and it is already the thing the drawer lists. Three
    hundred and sixty rows of `COLOR_BG` under a button that opens a full-screen
    list of the same files was the least informative use of them.

    WHY the row's rect is one accessor: the contract this repo already states is
    that a control is only drawn where its own press answers it. A strip that
    painted rows from one sum and hit-tested them from another would be the last
    partial row the drawer used to show -- a visible track nobody could tap.
    """
    main = _code_only(video_main())
    flat = " ".join(main.split())
    painter = _block(main, "fn get_video_pixel")
    touch = _block(main, "fn handle_touch")

    assert "fn track_row_rect(i: usize) -> Rect" in flat
    assert "track_row_rect(" in painter, "the strip no longer paints the rows"
    assert "track_row_rect(" in touch, "the strip paints tracks its press does not answer"
    assert "const TRACK_STRIP_Y0: usize = INFO_BTN.y + INFO_BTN.h + TRACK_STRIP_AIR;" in flat
    assert "const _: () = assert!(track_strip_holds_at_every_level());" in flat
    # The strip ends where the system's home band begins, and nowhere past it.
    assert "track_strip_foot_at(level) <= HOME_HIT.y" in flat

    # It names where its rows came from, and what it did not show.
    assert 'b"FROM /storage/videos & /storage/downloads"' in painter
    assert "MORE IN LIBRARY" in painter
    assert "NO VIDEO FILES" in painter

    # One code path opens a track, for the strip, the drawer and a dispatch.
    assert "fn open_track(&mut self, index: usize) -> bool" in flat
    assert main.count("open_track(row_idx)") >= 2, (
        "a row list opened its tracks one way and the other list another way"
    )

    # The properties sheet is an overlay: while it is up, the press that lands
    # inside it does not reach the tracks painted under it.
    assert re.search(r"if app\.info_open && SHEET_RECT\.contains\(x, y\)", touch), (
        "a press on the sheet falls through to the track list behind it"
    )


def test_a_still_image_is_not_reported_as_a_clip():
    """A Portable Pixmap says it is a still, in the header and on the transport.

    WHY: the format is one frame. Nothing in this player can advance it, so a
    subtitle reading `PPM P6 - 1.0x` promised a rate for something with no clock
    to run at. The header now says `PPM P6 - STILL IMAGE`, and the library row's
    duration field says `NO TIME` -- the media library is the platform's
    inventory, not this app's own claim about what it can play.

    WHY the verdict is counted rather than matched once: the badge is painted
    twice, once on the portrait band and once on the landscape HUD's scrim, and
    one copy satisfies a plain `in painter`. A still that reads as a clip only
    after a rotation is still the defect this gate holds shut, so both copies
    have to consult the format and neither may state a rate unconditionally.
    """
    lib = video_lib()
    main = _code_only(video_main())
    painter = _block(main, "fn get_video_pixel")

    assert "pub const fn is_still_image(self) -> bool" in lib
    assert "app.current_codec().is_still_image()" in painter
    assert painter.count("app.current_codec().is_still_image()") >= 2, (
        "one orientation claims a running rate for a media with no clock to run at"
    )
    assert 'b" - STILL IMAGE"' in painter
    # The rate claim is not made on a format that has no time to run.
    assert "STILL IMAGE" in painter
    assert "let speed_label = app.speed.label().as_bytes();" in painter, (
        "the speed run has been dropped rather than moved behind the still check"
    )
    # Joining the codec to the rate is the branch's else arm, never its trunk:
    # a badge that assembles `codec - speed` before asking what the format is
    # would pass a substring test while still promising a rate. So each badge is
    # read from its own buffer to its own draw call, and the rate's copy has to
    # sit after the `else` of the format's verdict inside that window.
    for badge, drawn, rate in (
        ("badge_buf", "badge_buf[..bl]", "s_label"),
        ("sub_buf", "sub_buf[..sl]", "speed_label"),
    ):
        start = painter.index(f"let mut {badge} = [0u8; 32];")
        window = painter[start : painter.index(drawn, start)]
        assert "if app.current_codec().is_still_image() {" in window, (
            f"the {badge} badge paints its rate without the format's verdict"
        )
        verdict, _, tail = window.partition("} else {")
        assert tail, f"the {badge} badge has no else arm to hold its rate"
        assert f"{rate}.len()" in tail and f"{rate}.len()" not in verdict, (
            f"the {badge} badge states its rate on the trunk, not behind the verdict"
        )


def test_the_library_row_still_quotes_the_measurements_it_has():
    """The drawer and the strip carry the file's own header values.

    WHY this is restated here: the track list moved from a full-screen drawer
    into the surface under the controls, and the metadata run -- size, format,
    frame size, running time -- must not become a title-only list on the way. The
    running time comes from `duration_label`, which is the same words the
    transport uses.
    """
    main = _code_only(video_main())
    painter = _block(main, "fn get_video_pixel")

    assert "format_file_size(item.file_size" in painter
    assert "format_resolution(item.width, item.height" in painter
    assert "duration_label(item.duration_sec > 0" in painter
    assert 'b" - "' in painter
    # The pinned row/press boundary of the full-screen drawer is untouched.
    assert "if row_idx < app.playlist.count && y < LIB_VIEW_Y1 {" in painter


def test_the_transport_band_words_the_controls_it_inks():
    """Every disc the transport inks is a disc the transport names, in writing.

    Band 3 has always drawn three round marks and nothing else. On a phone nobody
    has used before, three dots in a row are a puzzle rather than a control row,
    and this app already refuses that look everywhere else: the loop plate answers
    `LOOP OFF` and not a bare mark, the mute plate carries the horn and its
    read-out, the drawer's buttons spell `LIBRARY` and `MEDIA INFO`. The fix is
    the same one the rest of the screen uses -- the word, at the rung the plate
    ladder reaches, centred by `paint_plate_label` -- not a new type of its own.

    WHAT THIS GATE REFUSES:
    - a disc inked with a glyph and still left without a word (the count of
      `draw_circle` calls in the band has to equal the count of word calls, and
      each word has to be the production word of that control),
    - a word placed by a remembered offset rather than derived from the control's
      own centre and step,
    - a word whose room is measured against a width the plate does not leave it,
    - a play control that paints one word for both of its faces, and
    - one orientation treated and the other forgotten.
    """
    lib = video_lib()
    main = _code_only(video_main())
    flat = " ".join(main.split())
    portrait = _block(main, "if y >= BAND3_Y0 && y < BAND3_Y1 {")
    landscape = _block(main, "if bot_panel.contains(x, y) {")

    # The words are the library's, so the host test can measure them against the
    # room each orientation leaves, and the screen paints the same names.
    for spelling in (
        'pub const PREV_WORD: &[u8] = b"PREV";',
        'pub const NEXT_WORD: &[u8] = b"NEXT";',
        'pub const PLAY_WORD: &[u8] = b"PLAY";',
        'pub const PAUSE_WORD: &[u8] = b"PAUSE";',
        "pub const fn transport_action_word(playing: bool) -> &'static [u8]",
    ):
        assert spelling in lib, f"the transport lost its library word: {spelling}"

    # The room each row measures its words against is the centre distance the
    # discs stand on less the air every plate label keeps -- one number, spelled
    # in two files, tied at compile time the way the chip row ties its own.
    for step, room in (("TRANSPORT_STEP", "TRANSPORT_WORD_ROOM"),
                       ("LAND_TRANSPORT_STEP", "LAND_TRANSPORT_WORD_ROOM")):
        assert re.search(
            rf"const _:\s*\(\)\s*=\s*assert!\(\s*{step}\s*-\s*2\s*\*\s*CHIP_LABEL_AIR\s*==\s*"
            rf"(?:video_player::)?{room}\s*\)",
            main,
        ), f"{step} no longer ties its word room to the library's {room} at compile time"
    assert _eval(main, "NEXT_CX - PLAY_CX") == _eval(main, "PLAY_CX - PREV_CX"), (
        "the portrait transport centres are no longer evenly spaced, so the step "
        "the word rooms are derived from measures half of something else"
    )
    assert _eval(main, "LAND_STEP_CX_RIGHT - LAND_PLAY_CX") == _eval(
        main, "LAND_PLAY_CX - LAND_STEP_CX_LEFT"
    ), "the landscape transport centres are no longer evenly spaced"

    # The plate under a control is that control's centre widened to its
    # neighbours, on the row the band keeps under the tallest disc in it.
    for plate, cx, step, row in (
        ("PREV_LABEL_PLATE", "PREV_CX", "TRANSPORT_STEP", "TRANSPORT_LABEL_Y0"),
        ("PLAY_LABEL_PLATE", "PLAY_CX", "TRANSPORT_STEP", "TRANSPORT_LABEL_Y0"),
        ("NEXT_LABEL_PLATE", "NEXT_CX", "TRANSPORT_STEP", "TRANSPORT_LABEL_Y0"),
        ("LAND_PREV_LABEL_PLATE", "LAND_STEP_CX_LEFT", "LAND_TRANSPORT_STEP",
         "LAND_TRANSPORT_LABEL_Y0"),
        ("LAND_PLAY_LABEL_PLATE", "LAND_PLAY_CX", "LAND_TRANSPORT_STEP",
         "LAND_TRANSPORT_LABEL_Y0"),
        ("LAND_NEXT_LABEL_PLATE", "LAND_STEP_CX_RIGHT", "LAND_TRANSPORT_STEP",
         "LAND_TRANSPORT_LABEL_Y0"),
    ):
        box = _plate(main, plate)
        assert box["y"] == f"({row})", f"{plate} stands on a row of its own, not the band's"
        assert box["h"] == "(TRANSPORT_LABEL_H)", f"{plate} invented its own height"
        assert box["w"] == f"({step})", f"{plate} is not as wide as the step it belongs to"
        want = f"({cx}) - (({step}) / 2)"
        assert box["x"] == want, f"{plate} starts at {box['x']}, not centred on its control"

    # Every plate sits under every disc it names and inside the rows its band
    # repaints -- otherwise the word is ink the transport cannot keep.
    assert _usize(main, "TRANSPORT_LABEL_Y0") == _eval(
        main, "BAND3_Y1 - TRANSPORT_LABEL_H"
    ) or _const_expr(main, "TRANSPORT_LABEL_Y0") == "BAND3_Y1 - TRANSPORT_LABEL_H", (
        "the portrait word row is no longer the rows band 3 keeps at its foot"
    )
    assert _const_expr(main, "TRANSPORT_CY") == "TRANSPORT_LABEL_Y0 - PLAY_R", (
        "the portrait discs are no longer the words' ceiling; their row is a "
        "remembered number again, which is how a word ends up inked over a disc"
    )
    assert _const_expr(main, "LAND_TRANSPORT_LABEL_Y0") == (
        "LAND_TRANSPORT_CY + LAND_PLAY_R"
    ), "the landscape word row is not under the discs it names"
    # The landscape scrim had to give the row somewhere to stand, and it gave it
    # by moving its own volume band to the foot of the panel -- the alternative
    # was words inked over a press.
    assert _rect_exprs(main)["LAND_VOL_HIT"]["y"] == (
        "LAND_PANEL.y + LAND_PANEL.h - LAND_VOL_BAND_H"
    ), "the landscape volume row is not the scrim's foot, so the words have no row"
    assert "const _: () = assert!(transport_labels_hold_in_their_plates_at_every_level());" in flat, (
        "no compile-time gate holds the transport words inside their row at every "
        "level the text-size setting can hold"
    )
    assert "const _: () = assert!(transport_label_row_clears_the_discs_at_every_size());" in flat, (
        "nothing proves the transport words clear the discs above them and the "
        "band's own foot, in either orientation"
    )

    # The words themselves: one per disc, the plate of the control it names, and
    # the count of discs is the count of words. The defect this gate was written
    # for is a glyph that stands with no word under it, so the two are bound
    # together here rather than checked one at a time.
    for name, band, prefix, cy in (
        ("portrait", portrait, "", "TRANSPORT_CY"),
        ("landscape", landscape, "LAND_", "LAND_TRANSPORT_CY"),
    ):
        discs = band.count("draw_circle(")
        words = band.count("paint_plate_label(")
        assert discs >= 3, f"the {name} transport no longer inks its three discs"
        assert words >= discs, (
            f"the {name} transport inks {discs} discs and words {words}: a control "
            f"the person has to guess at"
        )
        for control, word in (
            (f"{prefix}PREV_LABEL_PLATE", "PREV_WORD"),
            (f"{prefix}NEXT_LABEL_PLATE", "NEXT_WORD"),
        ):
            assert f"paint_plate_label(x, y, {control}, {word}" in band, (
                f"the {name} {control} is not worded by the library's {word}"
            )
        play_plate = f"{prefix}PLAY_LABEL_PLATE"
        assert f"paint_plate_label(x, y, {play_plate}, transport_action_word(playing)" in band, (
            f"the {name} play control paints a word that does not follow its own glyph"
        )
        assert "let playing = app.state == PlaybackState::Playing;" in band, (
            f"the {name} transport's glyph and its word no longer read one predicate"
        )
        assert band.count("if playing") >= 1, (
            f"the {name} play disc still decides its mark somewhere the word cannot follow"
        )

    # The press answers the plate's own control: each word's disc is the rect the
    # touch path tests, and the words are not ink over a dead zone.
    touch = _block(main, "fn handle_touch")
    for hit in ("PREV_HIT.contains(x, y)", "PLAY_HIT.contains(x, y)", "NEXT_HIT.contains(x, y)"):
        assert hit in touch, f"the portrait word for {hit.split('_')[0]} lost its press"
    for hit in (
        "LAND_PREV_HIT.contains(x, y)",
        "LAND_PLAY_HIT.contains(x, y)",
        "LAND_NEXT_HIT.contains(x, y)",
    ):
        assert hit in touch, f"the landscape word for {hit.split('_')[0]} lost its press"


def test_previous_asks_the_list_before_it_restarts():
    """The `PREV` control steps to the previous track, in both orientations.

    The landscape copy of the transport used to answer its own version of the
    press: whenever the clip was an AVI it re-opened *this* file, so the mark on
    the disc -- and now the word under it -- said previous while the clock said
    restarted. The list steps for every codec (`Playlist::prev` moves the index,
    `load_current_track` re-dispatches from the entry's own header), so the fix is
    the press, not the wording: a screen does not get to soften a label because
    one of its two copies of a handler was shorter than the other.

    The one restart that stays is the restart the same mark means everywhere: a
    clip more than `PREV_RESTART_SECONDS` into its running time goes back to its
    first second before the list steps, and for a single-track list the previous
    position of the list is that first second.
    """
    main = _code_only(video_main())
    flat = " ".join(main.split())
    touch = _block(main, "fn handle_touch")
    press = _block(main, "fn press_previous(app: &mut VideoApp) -> Dirty")

    # One path, answered by both orientations' presses.
    assert touch.count("press_previous(app)") == 2, (
        "the two orientations answer previous with two handlers again, which is "
        "how one of them drifted into restarting the clip it was already on"
    )
    assert touch.count("if PREV_HIT.contains(x, y) {") == 1
    assert touch.count("if LAND_PREV_HIT.contains(x, y) {") == 1
    for marker in ("if PREV_HIT.contains(x, y)", "if LAND_PREV_HIT.contains(x, y)"):
        branch = _block(touch, marker)
        assert "press_previous(app)" in branch, f"{marker} answers a different action"
        assert "load_current_track" not in branch, (
            f"{marker} carries its own copy of the restart again"
        )

    # That one path asks the list, and its restart window is a named number.
    assert "app.playlist.prev()" in press, "the previous control stopped asking the list"
    assert "app.load_current_track()" in press, (
        "the previous control steps the index but never opens the track it moved to"
    )
    assert _const_expr(main, "PREV_RESTART_SECONDS") is not None, (
        "the window in which previous means this clip's first second is a "
        "remembered literal again"
    )
    assert "app.elapsed_sec > PREV_RESTART_SECONDS" in press or (
        "app.elapsed_sec <= PREV_RESTART_SECONDS" in press
    ), "the press no longer reads the window it names"
    assert "Dirty::Scrubber" in press and "Dirty::All" in press, (
        "one of the two answers -- the clip's first second, or the track before "
        "this one -- is gone from the shared press"
    )


if __name__ == "__main__":
    test_the_loop_chip_names_the_mode_it_presses()
    test_a_plate_label_stays_inside_the_plate_at_every_rung()
    test_the_transport_band_words_the_controls_it_inks()
    test_previous_asks_the_list_before_it_restarts()
    test_the_mute_control_inks_a_speaker()
    test_the_scrubber_says_no_running_time()
    test_the_volume_readout_is_a_word_in_a_column_that_holds_it()
    test_the_surface_below_the_controls_carries_the_library()
    test_a_still_image_is_not_reported_as_a_clip()
    test_the_library_row_still_quotes_the_measurements_it_has()
    print("All video player surface contract tests PASSED!")
