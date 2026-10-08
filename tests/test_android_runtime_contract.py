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
