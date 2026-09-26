#!/usr/bin/env python3
"""Static contracts for display driver/service boundary."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def must(text: str, needle: str, label: str) -> None:
    assert needle in text, f"missing {label}: {needle}"

def main() -> None:
    drivers = (ROOT / "kernel/src/drivers/display.rs").read_text()
    driver_mod = (ROOT / "kernel/src/drivers/mod.rs").read_text()
    kms = (ROOT / "kernel/src/drivers/display_kms.rs").read_text()
    syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text()
    sdk = (ROOT / "sdk/rust/interos-std/src/lib.rs").read_text()
    service = (ROOT / "userspace/system-service/src/lib.rs").read_text()
    displayd = (ROOT / "userspace/system-service/src/bin/displayd.rs").read_text()
    user = (ROOT / "kernel/src/task/user.rs").read_text()

    must(driver_mod, "pub mod display;", "display driver module")
    must(driver_mod, "pub use display::{DisplayDriver, KmsDisplayDriver};", "display boundary export")
    for marker in (
        "pub trait DisplayDriver",
        "fn create_surface",
        "fn write_surface_pixels",
        "fn destroy_surface",
        "fn composite_and_flip",
        "pub struct KmsDisplayDriver",
        "impl DisplayDriver for KmsDisplayDriver",
    ):
        must(drivers, marker, f"display boundary {marker}")
    must(kms, "pub fn write_surface_pixels", "KMS pixel write backend")
    fb=(ROOT/"kernel/src/framebuffer.rs").read_text()
    must(fb,"pub fn present_argb8888","hardware framebuffer scanout")
    must(fb,"aspect ratio","hardware scanout scaling")
    must(kms, "pub fn destroy_surface_owned", "KMS owned surface destruction")

    must(syscall, "fn require_display_service", "display capability gate")
    must(syscall, "verify_object(cap_slot, CapabilityType::FrameBufferDirect, Rights::WRITE, 1)",
         "object-scoped display hardware authority")
    for marker in (
        "pub fn sys_surface_create(cap_slot: usize",
        "pub fn sys_surface_write(cap_slot: usize",
        "pub fn sys_surface_destroy(cap_slot: usize",
        "pub fn sys_surface_flip(cap_slot: usize",
        "73 => {",
        "74 => {",
    ):
        must(syscall, marker, f"display syscall {marker}")

    for marker in (
        "pub fn surface_create",
        "pub fn surface_write",
        "pub fn surface_destroy",
        "pub fn surface_flip",
    ):
        must(sdk, marker, f"display SDK ABI {marker}")

    must(user, "1 => Some((CapabilityType::FrameBufferDirect, Rights::WRITE, service_id as u64))",
         "displayd-only framebuffer capability")
    assert "cap_type: CapabilityType::FrameBufferDirect" not in user.split("fn spawn_userspace_service",1)[0],         "legacy app code must not regain direct framebuffer authority"
    must(service, "pub fn run_display() -> !", "dedicated display service loop")
    must(service, "std::surface_create(4, 320, 480, 0)", "display service creates surface through ABI")
    must(service, "std::surface_write(4, surface", "display service writes through ABI")
    must(service, "std::surface_flip(4)", "display service flips through ABI")
    must(drivers, "crate::framebuffer::present_argb8888", "KMS-to-hardware scanout handoff")
    must(displayd, "interos_system_service::run_display()", "displayd uses dedicated implementation")

    print("display driver boundary contract: PASS")

if __name__ == "__main__":
    main()
