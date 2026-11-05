# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def test_canonical_qemu_uses_portrait_scanout_and_guest_supported_pointer_hardware():
    launcher = read("qemu/launch_studio.ps1")
    backend = read("emulator/server.py")
    assert "virtio-vga,edid=on,xres=720,yres=1440" in launcher
    assert "VGA,edid=on,xres={edid_w},yres={edid_h},vgamem_mb=32" in backend
    for source in (launcher, backend):
        # WHY no `ps2-mouse` device flag: no such QEMU device exists (QEMU
        # 11.1.0 `-device help` lists only ISA `i8042`, which q35 already
        # includes), and passing it aborts the VM instantly ("expects a
        # pluggable device type", observed live with an empty boot log). The
        # guest's PS/2 driver talks to the built-in i8042 ports, and Studio
        # pointer input travels as absolute COM1 touch injection, so neither
        # canonical profile needs a -device pointer flag at all.
        assert "ps2-mouse" not in source


def test_device_studio_displays_guest_scanout_without_a_browser_readiness_gate():
    source = read("emulator/app.js")
    start = source.index("function updateDeviceDisplay")
    end = source.index("\n  // Exact sub-rectangle", start)
    body = source[start:end]
    assert "startScanoutStream()" in body
    assert "if (isAlive)" in body
    assert "launcherReady" not in body
    assert "hasLauncherPresented" not in body


def test_device_studio_stream_starts_stopped_and_reconnects_after_any_close():
    source = read("emulator/app.js")
    # The stream used to initialise in the active state, which made the first
    # startScanoutStream() return early: a page loaded while the guest was
    # already running never entered the fetch loop or the polling chain.
    assert "let isStreamActive = false;" in source
    # A pending reconnect timer must not resurrect the stream after an
    # explicit stopScanoutStream().
    assert "if (!isStreamActive) return;" in source
    # Every termination except an explicit stop (aborted signal) must schedule
    # the reconnect and a polling fallback; a clean server-side close fell
    # through with no retry and froze the monitor until a reload.
    reconnect = "setTimeout(startPersistentBinaryStream, 2000);"
    assert source.count(reconnect) == 1
    guard = "if (isStreamActive && !signal.aborted) {"
    assert source.index(guard) < source.index(reconnect)


def test_pointer_events_are_serialized_and_include_down_move_up_and_cancel():
    source = read("emulator/app.js")
    assert "let guestTouchQueue = Promise.resolve()" in source
    for action in ("down", "move", "up", "cancel"):
        assert "action: '" + action + "'" in source
    assert "touchSurface.addEventListener('pointercancel'" in source


def test_guest_owns_boot_display_power_volume_and_restart_state():
    launcher = read("apps/launcher/src/main.rs")
    kms = read("kernel/src/drivers/display_kms.rs")
    syscall = read("kernel/src/syscall/mod.rs")
    assert "ViewMode::BootScreen" in launcher
    assert "Boot presentation complete" in launcher
    assert "pub fn set_powered(&mut self, powered: bool)" in kms
    assert "pub fn sys_device_power(action: u32)" in syscall
    assert "SERVICE_POWER_CONTROL_OBJECT" in syscall
    assert "crate::hal::platform::POWER.reset()" in syscall
    assert "crate::hal::platform::POWER.shutdown()" in syscall


def test_android_runtime_is_not_misrepresented_as_full_art_compatibility():
    runtime = read("kernel/src/compat/android/runtime.rs")
    apk = read("kernel/src/compat/android/apk.rs")
    assert "TOTAL_INSTRUCTION_LIMIT" in runtime
    assert "UnsupportedFrameworkApi" in runtime
    # APKs remain rejected until signer and content digest are actually verified.
    start = apk.index("pub fn verify_apk_signature")
    end = apk.index("\n}\n", start)
    assert "Err(ApkError::InvalidSignature)" in apk[start:end]



def test_quick_controls_never_present_unsupported_features_as_working():
    controls = read("apps/launcher/src/screens/quick_controls.rs")
    launcher = read("apps/launcher/src/main.rs")
    sdk = read("sdk/rust/interos-std/src/lib.rs")
    # Every row without a backend names the reason in the kernel's own terms
    # rather than showing a state, a level or a percentage.
    for reason in (
        'b"Unavailable"',
        'b"Not applicable"',
        'b"No backlight"',
        'b"No flash"',
        'b"Not wired"',
    ):
        assert reason in controls
    # Volume is the exception, and it is one because the device is real: the
    # kernel programs the AC'97 master volume register and answers a query from
    # that register, so the row that shows the number is drawn and answered as
    # the control it is. The panel used to print "No mixer" over it, which
    # under-claimed hardware the guest has.
    assert "state.volume_state_valid" in controls
    assert "QuickControlAction::VolumeUp" in controls
    assert "QuickControlAction::VolumeDown" in controls
    assert "std::audio_set_volume(requested)" in launcher
    assert 'b"No mixer"' not in controls
    # Only the rows with a real backend may act; everything else is a no-op, so
    # a press cannot toggle fabricated state.
    assert "QuickControlAction::NoOp" in controls
    assert "matches!(self, PanelRow::Lock | PanelRow::Volume)" in controls
    assert "pub fn audio_set_volume(volume_percent: u32)" in sdk


def test_quick_controls_squares_are_painted_and_hit_from_one_geometry():
    controls = read("apps/launcher/src/screens/quick_controls.rs")
    # One definition, two callers: a painter and a hit box that each hold their
    # own arithmetic is how this surface has come to answer a control several
    # rows away from the one under the finger.
    assert "const fn volume_steps() -> [Rect; 2]" in controls
    assert "const VOLUME_STEPS: [Rect; 2] = volume_steps();" in controls
    assert "Some(VOLUME_STEPS)" in controls
    assert "VOLUME_STEPS.iter().position(|s| s.contains(x, y))" in controls
    # The squares are cleared at the platform's touch floor, not at the icon box.
    assert "const STEP_W: usize = MIN_TOUCH_TARGET;" in controls
    # A step repaints the row that moved, and its cached frame, rather than
    # leaving the shade drawing the number from before the step.
    launcher = read("apps/launcher/src/main.rs")
    assert "Dirty::ShadeLevel" in launcher
    assert "screens::quick_controls::invalidate_frame_cache()" in launcher


def test_shell_volume_state_starts_unread_not_assumed():
    launcher = read("apps/launcher/src/main.rs")
    start = launcher.index("pub const fn new() -> Self {")
    end = launcher.index("\n    }\n", start)
    body = launcher[start:end]
    # WHY: the shell used to seed 70% and mark it valid before any query, so a
    # refused audio service still printed a confident level on the shade, the
    # volume HUD and the audio widget.
    assert "volume_state_valid: false" in body
    assert "volume_state_valid: true" not in body


def test_widgets_dashboard_prints_queried_readings_not_invented_ones():
    widgets = read("apps/launcher/src/screens/live_widgets.rs")
    # WHY the absence list is the assertion: every one of these was a painted
    # label while the card presented it as a reading from the device. The RAM
    # figure was a literal against a guest booted with 1 GiB, "60 FPS" named a
    # refresh the kernel only ever configures and never measures, "Dynamic DVFS
    # Active" described a governor nothing feeds a load sample to, and the sound
    # line named a controller this kernel has no driver for (the PCM path is
    # AC'97). Matched as label literals so a comment may still name them.
    for claim in (
        'b"13.2 GB"',
        'b"6.00 GB"',
        'b"60 FPS Scanout"',
        'b"Dynamic DVFS Active"',
        'b"VirtIO Sound Device',
        'b"10.0.2.15 Online"',
        'b"Clock Synced"',
        'b"Audio Service Online"',
    ):
        assert claim not in widgets
    # The figures that remain are read: capacity and RAM come from the kernel,
    # and the audio line changes with whether the mixer answered at all.
    assert "push_capacity(&mut cap_buf, state.storage_total_bytes)" in widgets
    assert "push_capacity(&mut ram_buf, state.memory_total_bytes)" in widgets
    assert 'b"No counters reported"' in widgets
    assert 'b"Volume: not reported"' in widgets
    assert "b\"AC'97 codec, 48 kHz PCM\"" in widgets


def test_shell_reads_ram_and_capacity_and_repaints_the_cards_they_move():
    launcher = read("apps/launcher/src/main.rs")
    assert "fn refresh_system_readings(state: &mut LauncherState) -> bool" in launcher
    assert "std::memory_telemetry()" in launcher
    assert "std::storage_query()" in launcher
    # A refused query must not leave the last number standing, and the cards that
    # did move get repainted by a band the painter itself names.
    assert "!state.memory_state_valid" in launcher
    assert "Dirty::WidgetsReadings => Some(screens::live_widgets::readings_band())" in launcher
    assert "screens::live_widgets::invalidate_widgets_cache()" in launcher
    widgets = read("apps/launcher/src/screens/live_widgets.rs")
    # One geometry owner for the band and the cards inside it: a band retyped
    # away from the rows it covers is how a reading ends up stale on screen. Both
    # now take the rung the painter lays its cards at, so raising "Text size"
    # moves the repainted rows with the rows that actually moved.
    assert "pub fn readings_band() -> (usize, usize)" in widgets
    assert "readings_band_at(held_level_at(launcher::scale::draw_level()))" in widgets
    assert "let first = storage_row_at(level);" in widgets
    assert "let last = cal_row_at(level);" in widgets
    assert "let cal_rect = cal_row_at(level);" in widgets


def test_installed_ram_total_counts_ram_and_not_the_mmio_hole():
    memory = read("kernel/src/memory.rs")
    start = memory.index("let mut detected_ram: u64 = 0;")
    end = memory.index("if detected_ram >=", start)
    body = memory[start:end]
    # WHY: the map names the 256 MiB PCI aperture and the low BIOS windows as
    # regions too, and a sum over every kind booted a 4 GiB guest reporting
    # 4351 MB, which the memory card then printed as its total.
    assert "MemoryRegionKind::Usable | MemoryRegionKind::Bootloader" in body
    assert "detected_ram = detected_ram.saturating_add(r.end.saturating_sub(r.start));" in body
    assert "for r in memory_regions.iter() {\n        detected_ram" not in memory

