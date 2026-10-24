"""Truth contract for the InterOS recorder package.

The recorder is a RAPP: its canonical source lives in the application
ecosystem repository, not in the core one. The contract is still checked
from the core suite, because the core suite is the gate for the platform;
when the ecosystem repository is not alongside the core one, the checks
that need it are skipped rather than failed.

Every assertion below pins a specific defect found by reading the app.
"""

from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
RAPP = ROOT.parent / "interos-apps" / "recorder"

pytestmark = pytest.mark.skipif(
    not (RAPP / "src" / "main.rs").is_file(),
    reason="the recorder package lives in the application ecosystem repository",
)


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def recorder_main() -> str:
    return read(RAPP / "src" / "main.rs")


def recorder_lib() -> str:
    return read(RAPP / "src" / "lib.rs")


def test_the_store_is_reached_through_capabilities_resolved_by_type():
    """The store's read and write used the literal slots 4 and 5.

    This package's manifest asks for logging, framebuffer, event poll,
    filesystem read, filesystem write, timer and audio control, in that
    order, so the read capability lands in slot 3 and the write capability
    in slot 4. The old constants addressed the write capability for the
    read and the timer capability for the write, so the store never loaded
    and never persisted.
    """
    lib = recorder_lib()
    assert "pub const STORE_CAP_READ: usize = 4;" not in lib
    assert "pub const STORE_CAP_WRITE: usize = 5;" not in lib
    assert "pub fn store_read_slot() -> Option<usize>" in lib
    assert "pub fn store_write_slot() -> Option<usize>" in lib
    assert "interos_std::cap::fs_read_slot()" in lib
    assert "interos_std::cap::fs_write_slot()" in lib


def test_the_recording_timer_reads_a_real_clock():
    """The tick read used the literal slot 3, which holds the filesystem
    read capability.

    The kernel verifies the capability's type at the slot it is given, so
    the read was refused, the counter never moved and the timer never
    advanced.
    """
    source = recorder_main()
    assert "std::get_ticks(3)" not in source
    assert "std::cap::timer_slot()" in source


def test_the_surface_is_not_created_through_a_fallback_slot():
    """The framebuffer lookup fell back to the literal slot 6.

    In this package's capability table slot 6 is the audio control
    capability. A surface created through the wrong authority is not the
    one the compositor will present.
    """
    source = recorder_main()
    assert ".unwrap_or(6)" not in source
    assert "std::cap::framebuffer_slot()" in source


def test_the_recording_count_is_written_out_in_full():
    """The count readout was a single digit built by adding to a byte.

    The capacity beside it was typed in rather than taken from the store's
    own limit.
    """
    source = recorder_main()
    assert "b'0' + state.store.len() as u8" not in source
    assert "fmt_u32(MAX_RECORDINGS as u32," in source
    assert "fmt_u32(state.store.len() as u32," in source


def test_the_text_is_drawn_with_the_outline_typeface():
    """The panel and the app's surface are both 720x1440.

    The compositor copies the surface 1:1. The 16x24 bitmap face would be
    magnified to the screen's type sizes and read soft, so text uses the
    outline face rasterised at the screen's own resolution.
    """
    source = recorder_main()
    assert "use interos_std::design::outline_text;" in source
    assert "outline_text::place_run" in source
    assert "outline_text::run_box" in source


def test_the_microphone_is_only_bound_when_a_capture_device_opens():
    """The kernel's hardware-status query answers "available" for every
    category it does not enumerate.

    A platform with no capture HAL still answers "available" for the word
    "microphone". The decision has to rest on a capture device actually
    opening.
    """
    source = recorder_main()
    assert 'std::settings_hardware_status("microphone")' in source
    assert "std::devfs_read(path, &mut probe).is_ok()" in source
    assert "HARDWARE CAPTURE BOUND: a capture device was found" in source
