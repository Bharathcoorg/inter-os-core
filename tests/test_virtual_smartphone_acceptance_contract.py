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
