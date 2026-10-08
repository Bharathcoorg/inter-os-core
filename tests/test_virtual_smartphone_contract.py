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
    assert "std::device_power(2)" in launcher


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
    assert "Keep the real QEMU scanout visible during reset" in app
    assert "if (isBooting) {" not in app[app.index("function updateDeviceDisplay"):app.index("// Exact sub-rectangle")]
