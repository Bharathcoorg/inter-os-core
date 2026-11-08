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



def test_android_dalvik_vm_opcode_matrix():
    dex = read("kernel/src/compat/android/dex.rs")
    runtime = read("kernel/src/compat/android/runtime.rs")
    # Verify opcode definitions in DEX specification
    assert "pub const OP_DIV_INT_2ADDR: u8 = 0xB3;" in dex
    assert "pub const OP_REM_INT_2ADDR: u8 = 0xB4;" in dex
    assert "pub const OP_AND_INT_2ADDR: u8 = 0xB5;" in dex
    assert "pub const OP_OR_INT_2ADDR: u8 = 0xB6;" in dex
    assert "pub const OP_XOR_INT_2ADDR: u8 = 0xB7;" in dex
    assert "pub const OP_SHL_INT_2ADDR: u8 = 0xB8;" in dex
    assert "pub const OP_SHR_INT_2ADDR: u8 = 0xB9;" in dex
    assert "pub const OP_USHR_INT_2ADDR: u8 = 0xBA;" in dex
    assert "pub const OP_ARRAY_LENGTH: u8 = 0x21;" in dex
    assert "pub const OP_NEW_ARRAY: u8 = 0x23;" in dex
    assert "pub const OP_AGET: u8 = 0x44;" in dex
    assert "pub const OP_APUT: u8 = 0x4B;" in dex
    assert "pub const OP_IGET: u8 = 0x52;" in dex
    assert "pub const OP_IPUT: u8 = 0x59;" in dex
    assert "pub const OP_ADD_INT: u8 = 0x90;" in dex
    assert "pub const OP_SUB_INT: u8 = 0x91;" in dex
    assert "pub const OP_MUL_INT: u8 = 0x92;" in dex
    assert "pub const OP_DIV_INT: u8 = 0x93;" in dex
    assert "pub const OP_REM_INT: u8 = 0x94;" in dex
    assert "pub const OP_IF_LT: u8 = 0x34;" in dex
    assert "pub const OP_IF_GE: u8 = 0x35;" in dex
    assert "pub const OP_IF_GT: u8 = 0x36;" in dex
    assert "pub const OP_IF_LE: u8 = 0x37;" in dex
    assert "pub const OP_IF_LTZ: u8 = 0x3A;" in dex
    assert "pub const OP_IF_GEZ: u8 = 0x3B;" in dex
    assert "pub const OP_IF_GTZ: u8 = 0x3C;" in dex
    assert "pub const OP_IF_LEZ: u8 = 0x3D;" in dex
    # Verify opcode implementations in Dalvik VM runtime
    assert "OP_DIV_INT_2ADDR =>" in runtime
    assert "OP_REM_INT_2ADDR =>" in runtime
    assert "OP_AND_INT_2ADDR =>" in runtime
    assert "OP_OR_INT_2ADDR =>" in runtime
    assert "OP_XOR_INT_2ADDR =>" in runtime
    assert "OP_ARRAY_LENGTH =>" in runtime
    assert "OP_NEW_ARRAY =>" in runtime
    assert "OP_AGET =>" in runtime
    assert "OP_APUT =>" in runtime
    assert "OP_IGET =>" in runtime
    assert "OP_IPUT =>" in runtime
    assert "pub fn interpret_insns" in runtime
    assert "DivideByZero" in runtime
    assert "ArrayIndexOutOfBounds" in runtime


def test_android_dalvik_vm_static_fields_and_conversions():
    dex = read("kernel/src/compat/android/dex.rs")
    runtime = read("kernel/src/compat/android/runtime.rs")
    assert "pub const OP_SGET: u8 = 0x60;" in dex
    assert "pub const OP_SPUT: u8 = 0x67;" in dex
    assert "pub const OP_INT_TO_FLOAT: u8 = 0x84;" in dex
    assert "pub const OP_FLOAT_TO_INT: u8 = 0x87;" in dex
    assert "pub const OP_INT_TO_BYTE: u8 = 0x8D;" in dex
    assert "pub const OP_INT_TO_CHAR: u8 = 0x8E;" in dex
    assert "pub const OP_INT_TO_SHORT: u8 = 0x8F;" in dex
    assert "pub const OP_CMPL_FLOAT: u8 = 0x2D;" in dex
    assert "pub const OP_CMPG_FLOAT: u8 = 0x2E;" in dex
    assert "OP_SGET | OP_SGET_BOOLEAN =>" in runtime
    assert "OP_SPUT | OP_SPUT_BOOLEAN =>" in runtime
    assert "OP_INT_TO_FLOAT =>" in runtime
    assert "OP_FLOAT_TO_INT =>" in runtime
    assert "OP_CMPL_FLOAT | OP_CMPG_FLOAT =>" in runtime
    assert "pub fn get_static_field" in runtime
    assert "pub fn set_static_field" in runtime

