#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Static contracts for display driver/service boundary."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def must(text: str, needle: str, label: str) -> None:
    assert needle in text, f"missing {label}: {needle}"

def must_not(text: str, needle: str, label: str) -> None:
    assert needle not in text, f"forbidden {label}: {needle}"


def main() -> None:
    drivers = (ROOT / "kernel/src/drivers/display.rs").read_text(encoding="utf-8")
    driver_mod = (ROOT / "kernel/src/drivers/mod.rs").read_text(encoding="utf-8")
    kms = (ROOT / "kernel/src/drivers/display_kms.rs").read_text(encoding="utf-8")
    syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
    sdk = (ROOT / "sdk/rust/interos-std/src/lib.rs").read_text(encoding="utf-8")
    service = (ROOT / "userspace/system-service/src/lib.rs").read_text(encoding="utf-8")
    displayd = (ROOT / "userspace/system-service/src/bin/displayd.rs").read_text(encoding="utf-8")
    user = (ROOT / "kernel/src/task/user.rs").read_text(encoding="utf-8")

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
    fb=(ROOT/"kernel/src/framebuffer.rs").read_text(encoding="utf-8")
    must(fb,"pub fn present_argb8888","hardware framebuffer scanout")
    # The scanout keeps the source aspect ratio and centres it, rather than
    # stretching a portrait surface across a landscape framebuffer.
    must(fb, "src_w.saturating_mul(info.height) <= src_h.saturating_mul(info.width)",
         "aspect-fit scanout geometry")
    must(fb, "let off_x = (info.width - dst_w) / 2;", "centred scanout letterbox")
    must(kms, "pub fn destroy_surface_owned", "KMS owned surface destruction")
    must(kms, "pub const MAX_SCANOUT_PIXELS: usize = 3840 * 2160;", "bounded scanout budget")
    must(kms, "checked_mul(self.scanout_height as usize)", "checked scanout allocation arithmetic")
    must(kms, "pixels > MAX_SCANOUT_PIXELS", "scanout budget enforcement")
    must(kms, "sx.saturating_add(s_w)", "composition coordinate overflow protection")
    must(kms, "sy.saturating_add(s_h)", "composition coordinate overflow protection")

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

    services = (ROOT / "kernel/src/services.rs").read_text(encoding="utf-8")
    must(services, "cap_type: CapabilityType::FrameBufferDirect, rights: Rights::WRITE, object_id: 1",
         "displayd-only framebuffer capability")
    must(service, "pub fn run_display() -> !", "dedicated display service loop")
    # WHY the spelling changed: a capability *slot* is a position in the
    # task's capability table, not a capability. The kernel now authorises
    # every platform call by the capability's *type*, so the caller no
    # longer names a slot and the SDK's display functions take no slot
    # argument. The intent is unchanged: the display service must create a
    # surface, write pixels into it, and flip it to the scanout.
    # WHY the display service holds no surface: it used to create a
    # 320x480 one, write four pixels into it and flip, and the
    # unfilled part of it went under the foreground application as a
    # black rectangle in the corner of every screen. A service that
    # draws nothing should hold nothing, and the kernel's compositor
    # owns the scanout.
    must_not(service, "std::surface_create(", "display service holds no surface")
    must_not(service, "std::surface_write(", "display service writes nothing")
    must(drivers, "crate::framebuffer::present_argb8888", "KMS-to-hardware scanout handoff")
    must(displayd, "interos_system_service::run_display()", "displayd uses dedicated implementation")

    # Canonical DisplayMetrics Contract
    design_mod = (ROOT / "sdk/rust/interos-std/src/design/mod.rs").read_text(encoding="utf-8")
    kernel_display_hal = (ROOT / "kernel/src/display_hal.rs").read_text(encoding="utf-8")
    aarch64_display_hal = (ROOT / "kernel-aarch64/src/display_hal.rs").read_text(encoding="utf-8")

    for marker in (
        "pub struct DisplayMetrics",
        "pub struct Dp(",
        "pub struct Px(",
        "pub enum Orientation",
        "pub struct SafeAreaInsetsPx",
        "pub const fn dp_to_px",
        "pub const fn px_to_dp",
        "pub const fn hairline_px",
        "pub const fn frame_budget_ns",
        "pub fn with_orientation",
    ):
        must(design_mod, marker, f"SDK DisplayMetrics {marker}")

    for marker in (
        "pub struct DisplayMetrics",
        "pub struct Dp(",
        "pub struct Px(",
        "pub enum Orientation",
        "pub struct SafeAreaInsetsPx",
    ):
        must(kernel_display_hal, marker, f"x86_64 display_hal {marker}")
        must(aarch64_display_hal, marker, f"aarch64 display_hal {marker}")

    print("display driver boundary and DisplayMetrics contract: PASS")


def test_display_driver_boundary_and_display_metrics_contract() -> None:
    """Expose the contract to pytest.

    Every check lives in `main()` so the file stays directly executable
    (`python tests/test_display_contract.py`); this wrapper runs the very
    same checks under pytest without duplicating any of them.
    """
    main()


if __name__ == "__main__":
    main()
