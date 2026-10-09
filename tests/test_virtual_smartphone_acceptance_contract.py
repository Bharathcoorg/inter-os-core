from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def test_canonical_qemu_uses_portrait_scanout_and_guest_supported_pointer_hardware():
    launcher = read("qemu/launch_studio.ps1")
    backend = read("emulator/server.py")
    for source in (launcher, backend):
        assert "virtio-vga,edid=on,xres=720,yres=1440" in source
        # The guest implements a PS/2 mouse driver
        # (kernel/src/drivers/ps2_mouse.rs) and no USB HID stack, so the
        # canonical profiles attach ps2-mouse. The earlier usb-tablet /
        # qemu-xhci expectations predate that boundary and cannot coexist with
        # the "usb-tablet not in x86_profile" contract in
        # test_virtual_smartphone_contract.py.
        assert "ps2-mouse" in source


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
    assert "let brightness_available = false" in controls
    assert 'b"Unavailable"' in controls
    assert 'b"N/A"' in controls
    assert "pub fn audio_set_volume(volume_percent: u32)" in sdk
    assert "std::audio_set_volume(requested)" in launcher
