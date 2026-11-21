# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Package Registry Kind & Zero-Heuristics Android Launch Contract.

WHAT THIS FILE IS FOR:
Guarantees that package execution authority is governed solely by the persistent
PackageRegistry with typed PackageKind:
1. PackageKind distinguishes NativeRapp and AndroidApk.
2. PackageEntry carries kind, persistent stable app_uid (10000..19999 for Android), and code_digest.
3. Syscall 168 (PackageRegisterApk) provides atomic registration of verified Android APKs.
4. launch_registered_package verifies active registration in PACKAGE_REGISTRY.
5. All ad-hoc package name prefix heuristics (com.*, org.whispersystems.*, *.android.*) are eradicated.
"""

from pathlib import Path
import pytest

ROOT = Path(__file__).resolve().parents[1]


def read_source(rel_path: str) -> str:
    return (ROOT / rel_path).read_text(encoding="utf-8")


def test_package_kind_and_entry_fields():
    registry_rs = read_source("kernel/src/package/registry.rs")
    assert "pub enum PackageKind" in registry_rs
    assert "NativeRapp = 0" in registry_rs
    assert "AndroidApk = 1" in registry_rs

    assert "pub kind: PackageKind," in registry_rs
    assert "pub app_uid: u32," in registry_rs
    assert "pub code_digest: [u8; 32]," in registry_rs


def test_register_verified_apk_allocates_stable_uid():
    registry_rs = read_source("kernel/src/package/registry.rs")
    assert "pub fn register_verified_apk(" in registry_rs
    assert "10000 + (self.entries.len() as u32) + 1" in registry_rs
    assert "PackageKind::AndroidApk" in registry_rs


def test_syscall_package_register_apk_registered():
    syscall_rs = read_source("kernel/src/syscall/mod.rs")
    assert "PackageRegisterApk = 168," in syscall_rs
    assert "pub fn sys_package_register_apk(" in syscall_rs

    std_lib_rs = read_source("sdk/rust/interos-std/src/lib.rs")
    assert "PackageRegisterApk = 168," in std_lib_rs
    assert "pub fn package_register_apk(" in std_lib_rs


def test_launch_registered_package_rejects_heuristics():
    syscall_rs = read_source("kernel/src/syscall/mod.rs")
    # Verify launch checks PackageKind::AndroidApk
    assert "if registered.kind == crate::package::PackageKind::AndroidApk {" in syscall_rs
    # Ensure ad-hoc name-prefix heuristics are absent
    launch_fn = syscall_rs[syscall_rs.find("fn launch_registered_package"):syscall_rs.find("fn sys_package_register_apk")]
    assert 'app_id.starts_with("com.")' not in launch_fn
    assert 'app_id.starts_with("org.whispersystems.")' not in launch_fn
    assert 'app_id.contains(".android.")' not in launch_fn
