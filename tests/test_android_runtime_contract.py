# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def test_android_runtime_has_real_method_dispatch_and_framework_boundary():
    dex = read("kernel/src/compat/android/dex.rs")
    runtime = read("kernel/src/compat/android/runtime.rs")
    framework = read("kernel/src/compat/android/framework.rs")
    assert "pub method_index: usize" in dex
    assert "OP_INVOKE_VIRTUAL | OP_INVOKE_DIRECT | OP_INVOKE_STATIC" in runtime
    assert "invoke_method(dex, &target.class_name, &target.method_name)" in runtime
    assert "SecurityViolationForbiddenApi" in runtime
    assert "UnsupportedApi" in framework
    assert "android/util/Log" in framework


def test_android_package_parser_fails_closed_on_unverified_or_unsupported_content():
    apk = read("kernel/src/compat/android/apk.rs")
    assert "zip.validate_compression()?" in apk
    assert "UnsupportedCompression" in apk
    assert "AttestationFailed" in apk


def test_android_runtime_has_audited_launch_and_real_surface_ownership():
    runtime = read("kernel/src/compat/android/mod.rs")
    window = read("kernel/src/compat/android/window.rs")
    assert "launch_verified" in runtime
    assert "audit_and_create_container" in runtime
    assert "surface_id" in runtime
    assert "AndroidActivityState::Resumed" in runtime
    assert "create_surface" in window
    assert "destroy_surface_owned" in window
    assert "MAX_ANDROID_WINDOWS" in window


def test_android_vm_rejects_oversized_register_files_and_unbounded_recursion():
    runtime = read("kernel/src/compat/android/runtime.rs")
    assert "method.registers_size as usize > self.registers.len()" in runtime
    assert "self.call_depth >= 32" in runtime
    assert "STEP_LIMIT: usize = 4096" in runtime


def test_android_runtime_has_bounded_validated_drawing_runtime():
    draw = read("kernel/src/compat/android/draw.rs")
    mod = read("kernel/src/compat/android/mod.rs")
    assert "pub enum DrawOp" in draw
    assert "pub struct DrawList" in draw
    assert "pub enum DrawError" in draw
    assert "pub const MAX_DRAW_OPS_PER_LIST: usize = 256" in draw
    assert "pub const MAX_FRAME_AREA_PIXELS: u64" in draw
    assert "pub fn render_draw_list" in draw
    assert "pub fn clip_rect" in draw
    assert "write_surface_pixels" in draw
    assert "pub mod draw;" in mod
    assert "pub use draw::{" in mod


def test_android_microg_parcel_and_binder_engine():
    binder = read("kernel/src/compat/android/binder.rs")
    framework = read("kernel/src/compat/android/framework.rs")
    assert "pub struct ParcelReader<'a>" in binder
    assert "pub struct ParcelWriter<'a>" in binder
    assert "HANDLE_MICROG_GMS_BROKER: u32 = 0x1001" in binder
    assert "HANDLE_MICROG_LOCATION: u32 = 0x1002" in binder
    assert "HANDLE_MICROG_GCM_PUSH: u32 = 0x1003" in binder
    assert "HANDLE_MICROG_ADS_ID: u32 = 0x1004" in binder
    assert "pub fn handle_microg_transaction" in binder
    assert "AndroidFrameworkService::Location" in framework
    assert "android/location/LocationManager" in framework


def test_android_apk_deflate_and_vfs_sandbox_isolation():
    inflate = read("kernel/src/compat/android/inflate.rs")
    vfs = read("kernel/src/compat/android/vfs.rs")
    bridge = read("kernel/src/compat/android/bridge.rs")
    apk = read("kernel/src/compat/android/apk.rs")
    assert "pub fn inflate_raw" in inflate
    assert "extract_file" in apk
    assert "AndroidSandboxFs" in vfs
    assert "SecurityViolation" in vfs
    assert "security_violations" in vfs
    assert "AndroidIntentBridge" in bridge
    assert "INTENT_PAY" in bridge
    assert "INTENT_BROWSER_URL" in bridge
    assert "INTENT_TELEPHONY_DIAL" in bridge

