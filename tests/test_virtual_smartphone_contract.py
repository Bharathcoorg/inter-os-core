from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def test_canonical_qemu_exposes_native_portrait_display_and_absolute_pointer():
    launch = read("qemu/launch_studio.ps1")
    server = read("emulator/server.py")
    for source in (launch, server):
        assert "virtio-vga,xres=720,yres=1440" in source
        assert "qemu-xhci" in source
        assert "usb-tablet" in source


def test_guest_power_is_authoritative():
    kms = read("kernel/src/drivers/display_kms.rs")
    syscall = read("kernel/src/syscall/mod.rs")
    launcher = read("apps/launcher/src/main.rs")
    assert "pub powered: bool" in kms
    assert "toggle_powered" in kms
    assert "DevicePower = 127" in syscall
    assert "sys_device_power" in syscall
    assert "std::device_power(0)" in launcher
    assert "std::device_power(1)" in launcher
    assert "std::device_power(5)" in launcher
    serial = read("kernel/src/serial.rs")
    assert 'verb.eq_ignore_ascii_case("POWER_KEY")' in serial
    assert "toggle_powered()" in serial


def test_boot_and_wake_are_guest_owned():
    launcher = read("apps/launcher/src/main.rs")
    boot = read("apps/launcher/src/screens/boot.rs")
    assert "ViewMode::BootScreen" in launcher
    assert "boot_started_ticks" in launcher
    assert "Boot presentation complete" in launcher
    assert "Display wake observed; presenting real lock screen." in launcher
    assert "get_boot_screen_color" in boot


def test_studio_clicks_have_real_pointer_down_and_up():
    app = read("emulator/app.js")
    down = app.index("action: 'down', coords: 'portrait'")
    pointer_down = app.index("touchSurface.addEventListener('pointerdown'")
    assert down > pointer_down
    assert "action: 'up', coords: 'portrait'" in app
    assert "action: 'tap', coords: 'portrait'" not in app[pointer_down:app.index("touchSurface.addEventListener('wheel'")]


def test_legacy_kernel_phone_shell_is_not_canonical_boot_path():
    main = read("kernel/src/main.rs")
    assert "draw_mobile_shell()" not in main
    assert "redraw_phone()" not in main


def test_reboot_keeps_guest_scanout_visible_in_studio():
    app = read("emulator/app.js")
    assert "Rebooting InterOS..." in app
    assert "fetch('/api/qemu/reboot'" in app
    assert "if (isBooting) {" not in app[app.index("function updateDeviceDisplay"):app.index("// Exact sub-rectangle")]


def test_volume_is_guest_rendered_as_a_system_overlay():
    launcher = read("apps/launcher/src/main.rs")
    assert "render_volume_hud" in launcher
    assert "std::window_set_geometry(4, id, 100, 220, 100, true)" in launcher


def test_studio_does_not_gate_real_scanout_on_launcher_ready_state():
    app = read("emulator/app.js")
    assert "hasLauncherPresented" not in app
    assert "expose its real scanout" in app

def test_android_boundary_does_not_fabricate_framework_or_signature_success():
    vfs = read("kernel/src/compat/android/vfs.rs")
    apk = read("kernel/src/compat/android/apk.rs")
    runtime = read("kernel/src/compat/android/mod.rs")
    assert "ANDROID_APP_PROCESS_VIRTUAL_STUB" not in vfs
    assert "ANDROID_FRAMEWORK_JAR_ISOLATED_CONTAINER_RUNTIME" not in vfs
    verifier = apk[apk.index("pub fn verify_apk_signature"):apk.index("impl ApkPackage")]
    assert "Err(ApkError::InvalidSignature)" in verifier
    assert "return Ok(())" not in verifier
    assert "full Android framework/runtime compatibility is not yet implemented" in runtime

def test_volume_state_is_never_fabricated_when_guest_does_not_confirm():
    server = read("emulator/server.py")
    launcher = read("apps/launcher/src/main.rs")
    assert "cur_vol = 75" not in server
    assert "'status': 'APPLIED' if success else 'UNCONFIRMED'" in server
    assert "volume_state_valid" in launcher
    assert "unwrap_or(75)" not in launcher


def test_device_studio_power_on_and_reboot_are_not_reported_as_simulated():
    server = read("emulator/server.py")
    app = read("emulator/app.js")
    assert "monitor_alive = bool(query_qemu_monitor().get('qemu_alive', False))" in server
    assert "restart_method = 'monitor_system_reset'" in server
    assert "restart_method = 'backend_relaunch'" in server
    assert "simulated" not in app
    assert "[BOOT_TIMEOUT]" in app


def test_device_studio_never_kills_unowned_qemu_processes():
    server = read("emulator/server.py")
    stop = server[server.index("def stop_qemu_backend"):server.index("def launch_qemu_profile")]
    assert "psutil.process_iter" not in stop
    assert "p.kill()" not in stop
    assert "Only qemu_subproc, created and owned by" in stop


def test_boot_renderer_is_constant_time_and_all_view_matches_cover_boot():
    boot = read("apps/launcher/src/screens/boot.rs")
    launcher = read("apps/launcher/src/main.rs")
    assert "while distance.saturating_add(1)" not in boot
    assert "distance_sq" in boot
    assert "ViewMode::BootScreen => {" in launcher
    assert "Boot presentation complete" in launcher


def test_android_apk_without_dex_is_rejected_before_container_approval():
    apk = read("kernel/src/compat/android/apk.rs")
    audit = apk[apk.index("pub fn audit_and_create_container"):]
    assert "if !self.has_classes_dex" in audit
    assert "status: AcrAuditStatus::InvalidApkFormat" in audit
