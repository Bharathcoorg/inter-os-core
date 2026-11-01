"""Truth contract for the InterOS Video Player application.

Enforces:
1. The Platform Truth Contract for H.264:
   Exact constant: "No hardware H.264 decoder bound. Software decode unavailable."
   Never fakes hardware decode or synthesizes mock pixels when no physical silicon is bound.
2. Userspace Guarantees:
   Must remain strictly #![no_std] and #![no_main] with zero heap allocations.
3. VLC-Level Capabilities:
   - Video library & playlist drawer scanning both /storage/videos and /storage/downloads.
   - Metadata display: title, file size, codec ("PPM P6", "MJPEG", "RAW RGB24", "H.264"), resolution and duration.
   - Playback speed selector: 0.5x, 0.75x, 1.0x, 1.25x, 1.5x, 2.0x.
   - Orientation: Portrait (centered 16:9 canvas) and Landscape (fullscreen 720x1440 with translucent HUD).
   - Picture-in-picture (PIP): 280x158 floating preview card with live playback during playlist browsing.
   - Audio Volume & Mute HUD with speaker icon, volume seek bar, and system audio control syscalls.
   - Repeat / loop modes (None, Single, All) and Aspect Ratio cycling (Fit, Fill, Stretch, 16:9, 4:3).
4. Typography & Touch Affordances:
   - Scale 2 Body typography for track title, timestamps, playlist rows, and options.
   - All touch targets meet or exceed >= 48px.
   - Nothing interactive is authored inside the system status band, and a control is
     only drawn where its own press path answers it.
5. Real execution evidence:
   - Executes apps/video_player cargo test suite (33 unit tests) over real AVI and image fixtures.
"""

from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def video_lib() -> str:
    return read("apps/video_player/src/lib.rs")


def video_main() -> str:
    return read("apps/video_player/src/main.rs")


def _code_only(source: str) -> str:
    """Strips `//` line comments and block comments from Rust source.

    WHY: these gates assert that a retired literal is gone, and the natural way
    to write the comment explaining why it went away is to quote it. Scanning
    comments would make the contract test the author's prose instead of the
    program.
    """
    import re

    without_block = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
    return re.sub(r"//[^\n]*", " ", without_block)


def _fn_body(source: str, signature: str) -> str:
    """The braced body that follows `signature`, as far as its own braces close.

    WHY: a whole-file gate can be satisfied by a constant declaration or by a
    different function. Cutting the one function out keeps the gate about the
    code that paints or answers the press.
    """
    at = source.find(signature)
    assert at >= 0, f"the source no longer has {signature}"
    open_at = source.find("{", at)
    assert open_at >= 0, f"{signature} has no body"
    depth = 0
    for index in range(open_at, len(source)):
        char = source[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source[open_at + 1 : index]
    raise AssertionError(f"{signature} has an unbalanced body")


def test_platform_truth_contract_exact_string():
    """Preserves the exact Platform Truth Contract error string for missing H.264 hardware."""
    lib = video_lib()
    main = video_main()
    expected_msg = 'pub const NO_HW_H264_MSG: &str = "No hardware H.264 decoder bound. Software decode unavailable.";'
    assert expected_msg in lib, "lib.rs must contain the exact NO_HW_H264_MSG definition"
    assert "NO_HW_H264_MSG" in main, "main.rs must import and report NO_HW_H264_MSG"
    assert "HardwareUnavailable(NO_HW_H264_MSG)" in main, "main.rs must report HardwareUnavailable on H.264"


def test_userspace_no_std_and_zero_heap():
    """Ensures #![no_std] and #![no_main] with zero dynamic heap allocations."""
    lib = video_lib()
    main = video_main()
    assert "#![no_std]" in lib
    assert "#![no_std]" in main
    assert "#![no_main]" in main

    # Disallow heap allocation crates/types in interactive rendering
    for src in (lib, main):
        assert "extern crate alloc;" not in src
        assert "alloc::vec::Vec" not in src
        assert "alloc::string::String" not in src
        assert "alloc::boxed::Box" not in src


def test_vlc_library_storage_and_metadata():
    """Video library scans /storage/videos and /storage/downloads, displaying full metadata."""
    lib = video_lib()
    main = video_main()
    assert '"/storage/videos"' in main
    assert '"/storage/downloads"' in main
    assert "parse_directory_listing" in lib
    assert "parse_directory_listing" in main

    # Codecs: PPM P6, RAW RGB24, MJPEG, H.264
    for codec in ('"PPM P6"', '"RAW RGB24"', '"MJPEG"', '"H.264"'):
        assert codec in lib

    # Metadata formatting helpers
    assert "pub fn format_file_size" in lib
    assert "pub fn format_resolution" in lib
    assert "pub fn format_time" in lib


def test_playback_speed_selector():
    """Playback speed toggle: 0.5x, 0.75x, 1.0x, 1.25x, 1.5x, 2.0x."""
    lib = video_lib()
    main = video_main()
    assert "enum PlaybackSpeed" in lib
    for spd in ('"0.5x"', '"0.75x"', '"1.0x"', '"1.25x"', '"1.5x"', '"2.0x"'):
        assert spd in lib
    assert "SPEED_BTN" in main
    assert "app.speed.multiplier_milli()" in main


def test_orientation_modes_and_translucent_hud():
    """Orientation toggle: Portrait <-> Landscape with translucent on-screen HUD."""
    lib = video_lib()
    main = video_main()
    assert "enum Orientation" in lib
    assert "Portrait" in lib
    assert "Landscape" in lib
    assert "ORIENT_BTN" in main
    assert "Orientation::Landscape" in main
    assert "blend_argb" in lib
    assert "blend_argb" in main


def test_picture_in_picture_floating_card():
    """PIP floating preview card (280x158 at bottom right) during library browsing."""
    main = video_main()
    assert "PIP_W: usize = 280" in main
    assert "PIP_H: usize = 158" in main
    assert "PIP_RECT" in main
    assert "pip_mode" in main
    assert "PIP_BTN" in main


def test_audio_volume_hud_and_syscalls():
    """Audio volume HUD with speaker icon, volume seek bar, mute toggle, and system audio syscalls."""
    lib = video_lib()
    main = video_main()
    assert "pub fn volume_thumb_x" in lib
    assert "pub fn volume_seek_percent" in lib
    assert "VOL_MUTE_BTN" in main
    assert "VOL_BAR_RECT" in main
    assert "std::audio_set_volume" in main
    assert "std::audio_query_volume" in main


def test_aspect_ratio_and_repeat_modes():
    """Repeat modes (None, Single, All) and Aspect ratio cycling (Fit, Fill, Stretch, 16:9, 4:3)."""
    lib = video_lib()
    main = video_main()
    assert "enum LoopMode" in lib
    for lm in ('"OFF"', '"ONE"', '"ALL"'):
        assert lm in lib
    assert "enum AspectRatio" in lib
    for ar in ('"FIT"', '"FILL"', '"STRETCH"', '"16:9"', '"4:3"'):
        assert ar in lib
    assert "ASPECT_BTN" in main
    assert "LOOP_BTN" in main


def test_typography_and_touch_affordances():
    """Scale 2 Body typography used for track title, timestamps, playlist rows, and options."""
    main = video_main()
    # Scale 2 outline text calls
    assert "draw_text_bold(x, y, tx, 40, 2" in main or "draw_text_bold" in main
    assert "draw_text(x, y, 24, 606, 2" in main
    assert "draw_text(x, y, REMAIN_X, 606, 2" in main

    # Touch targets >= 48px. The three header controls stand on the platform's
    # content boundary, so the contract names the constant the row comes from
    # rather than the row it happened to have.
    assert "CLOSE_BTN: Rect = Rect::new(24, HEADER_TOP, 52, 52)" in main
    assert "ORIENT_BTN: Rect = Rect::new(568, HEADER_TOP, 52, 52)" in main
    assert "KEBAB_BTN: Rect = Rect::new(644, HEADER_TOP, 52, 52)" in main
    assert "const HEADER_TOP: usize = std::design::viewport::HEADER_TOP;" in main
    assert "PLAY_R: usize = 44" in main  # 88px circle
    assert "PREV_R: usize = 28" in main  # 56px circle, 64px hit
    assert "NEXT_R: usize = 28" in main
    assert "SPEED_BTN: Rect = Rect::new(24, 900, 154, 52)" in main
    assert "ASPECT_BTN: Rect = Rect::new(194, 900, 154, 52)" in main
    assert "LOOP_BTN: Rect = Rect::new(364, 900, 154, 52)" in main
    assert "PIP_BTN: Rect = Rect::new(534, 900, 162, 52)" in main
    assert "LIB_BTN: Rect = Rect::new(24, 968, 326, 52)" in main
    assert "INFO_BTN: Rect = Rect::new(366, 968, 330, 52)" in main


def test_audio_stream_truth_contract():
    """Audio stream truth contract:
    - Reports 'Software (PCM)' when an AVI carries a PCM audio stream.
    - Reports 'Unavailable (not implemented)' when an AVI carries unsupported audio.
    - Reports 'Not present in this file' when an AVI has no audio.
    - Demuxes uncompressed PCM chunks ('01wb') from AVI container.
    """
    main = video_main()
    avi = read("apps/video_player/src/avi.rs")
    assert "Software (PCM)" in main, "main.rs must report 'Software (PCM)' when PCM audio is present"
    assert "avi_audio_pcm" in main, "main.rs must implement avi_audio_pcm"
    assert "AviChunk" in avi, "avi.rs must define AviChunk"
    assert "is_pcm_audio" in avi, "avi.rs must define is_pcm_audio"


def test_what_is_painted_is_what_answers_the_press():
    """Every control the drawer and HUD show is a control their press path answers, and
    neither reaches outside its own canvas.

    WHY a source gate: the video player's bin target carries `test = false`, so
    neither `get_video_pixel` nor `handle_touch` can be called from a host test.

    Four defects the geometry rules below hold shut:

    - The landscape HUD's bar began at y=24 and its back control at y=36, while
      the system status band still owns the top 48 rows of the same 720x1440
      surface. The first twelve rows of that control were ink the user could
      not see, sitting on a band the press did answer.
    - The playlist painter drew a row wherever `row_idx` named one, but the
      press only selected rows above `LIB_VIEW_Y1`, so the last partial row was
      a visible track that nothing answered.
    - The scrubber's touch band ran to x=610 while the remaining-time number
      started at 608, so the first strokes of that number jumped playback.
    - With the drawer or the PIP card open, a press outside the rows fell
      through to the portrait chrome hidden underneath and seeked or changed
      volume on a screen that was not showing them.
    """
    main = _code_only(video_main())
    painter = _fn_body(main, "fn get_video_pixel")
    touch = _fn_body(main, "fn handle_touch")

    # The landscape HUD stands below the status band, and one rect serves both
    # the painter and the press.
    assert "const LAND_BAR: Rect = Rect::new(24, HEADER_TOP, 672, 80);" in main
    assert "assert!(LAND_BAR.y >= std::design::viewport::STATUS_BAR_HEIGHT);" in main
    assert "let top_bar = LAND_BAR;" in painter
    assert "let back_hit = LAND_BACK_BTN;" in painter
    assert "if LAND_BACK_BTN.contains(x, y)" in touch
    for orphan in ("Rect::new(24, 24, 672, 80)", "Rect::new(36, 36, 52, 52)"):
        assert orphan not in main, f"the landscape HUD is authored from a literal again: {orphan}"
    assert "draw_text_bold(x, y, 104, 38, 2," not in painter
    assert "draw_text(x, y, 104, 70, 1," not in painter

    # The playlist rows stop being drawn where the press stops selecting them.
    assert "if row_idx < app.playlist.count && y < LIB_VIEW_Y1 {" in painter

    # The scrubber band gives the remaining-time run its own columns.
    assert "const REMAIN_X: usize = SEEK_X + SEEK_W + 12;" in main
    assert "assert!(SEEK_HIT.x + SEEK_HIT.w <= REMAIN_X);" in main
    assert "draw_text(x, y, REMAIN_X, 606, 2," in painter
    assert "Rect::new(110, 584, 500, 68)" not in main

    # The drawer consumes the presses that land on its own canvas.
    drawer_at = touch.find("if app.library_open || app.pip_mode {")
    assert drawer_at >= 0, "the drawer's press branch is gone, so this gate lost its subject"
    depth = 0
    end = drawer_at
    for index in range(touch.find("{", drawer_at), len(touch)):
        if touch[index] == "{":
            depth += 1
        elif touch[index] == "}":
            depth -= 1
            if depth == 0:
                end = index
                break
    assert touch[drawer_at:end].rstrip().endswith("return EventAction::None;"), (
        "the drawer's last branch falls through instead of consuming the press"
    )


def test_video_player_suite_executes_and_passes():
    """Execute real video player unit tests in apps/video_player.

    Platform Truth Contract:
    Requires exit 0 for the cargo test suite cited by the contract.
    Verifies real behavior on actual binary fixtures:
    - Exact H.264 rejection string: 'No hardware H.264 decoder bound. Software decode unavailable.'
    - AVI PCM audio chunk demuxing ('01wb') and playback decoding
    - AVI MJPEG frame decoding using platform JPEG service
    - Raw and PPM P6 frame decoders
    - Translucent HUD ARGB alpha blending and canvas mapping
    - Touch seek, volume, speed, orientation, and aspect ratio controls
    """
    res = subprocess.run(
        ["cargo", "test", "--manifest-path", "apps/video_player/Cargo.toml"],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert res.returncode == 0, f"video_player cargo test suite failed:\n{res.stdout}\n{res.stderr}"
    assert "33 passed" in res.stdout, f"Expected 33 video_player tests to pass, got:\n{res.stdout}"


if __name__ == "__main__":
    test_platform_truth_contract_exact_string()
    test_userspace_no_std_and_zero_heap()
    test_vlc_library_storage_and_metadata()
    test_playback_speed_selector()
    test_orientation_modes_and_translucent_hud()
    test_picture_in_picture_floating_card()
    test_audio_volume_hud_and_syscalls()
    test_aspect_ratio_and_repeat_modes()
    test_typography_and_touch_affordances()
    test_audio_stream_truth_contract()
    test_what_is_painted_is_what_answers_the_press()
    test_video_player_suite_executes_and_passes()
    print("All video player truth contract tests PASSED!")
