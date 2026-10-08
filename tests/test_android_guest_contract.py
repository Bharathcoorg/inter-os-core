from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def test_android_guest_uses_real_framework_vm_boundary():
    script = read("qemu/launch_android_compat.ps1")
    assert "INTEROS_ANDROID_KERNEL" in script
    assert "INTEROS_ANDROID_RAMDISK" in script
    assert "INTEROS_ANDROID_SYSTEM" in script
    assert "INTEROS_ANDROID_DATA" in script
    assert '"-device", "virtio-vga,xres=720,yres=1440"' in script
    assert '"-device", "qemu-xhci,id=android-xhci"' in script
    assert '"-device", "usb-tablet,bus=android-xhci.0"' in script
    assert '"-device", "virtio-net-pci,netdev=android-net"' in script


def test_android_apk_attestation_is_fail_closed():
    apk = read("kernel/src/compat/android/apk.rs")
    assert "if !self.is_signed" in apk
    assert "AcrAuditStatus::AttestationFailed" in apk


def test_android_binder_authorization_is_target_bound():
    binder = read("kernel/src/compat/android/binder.rs")
    assert "binder_service_object" in binder
    assert "authorize_object(CapabilityType::IpcSend" in binder
    assert "transaction.data.len() > 64 * 1024" in binder
