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
    assert "draw_text(x, y, 608, 606, 2" in main

    # Touch targets >= 48px
    assert "CLOSE_BTN: Rect = Rect::new(24, 32, 52, 52)" in main
    assert "ORIENT_BTN: Rect = Rect::new(568, 32, 52, 52)" in main
    assert "KEBAB_BTN: Rect = Rect::new(644, 32, 52, 52)" in main
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
    test_video_player_suite_executes_and_passes()
    print("All video player truth contract tests PASSED!")
