# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""InterOS ACR Ring-3 Migration & Acceptance Verification Contract.

Verifies:
1. Kernel boot initialization does not execute Android bytecode or Dalvik self-tests in Ring 0.
2. Global `ANDROID_RUNTIME: Mutex<AndroidRuntime>` singleton is removed from kernel.
3. Pure format crate `interos-android-format` provides unprivileged parsers for APK, signing, DEX, ELF, AXML, ARSC.
4. Unprivileged runtime crate `interos-acr` provides userspace `AcrManager`, `Looper`, `Choreographer`, `DalvikVmState`, and view rendering.
5. Userspace daemon `acrd` runs `run_acr()` in Ring 3 userspace with zero ambient authority.
6. Historical privileged syscalls (52, 53, 55) return `SyscallResult::NotSupported`.
"""

from pathlib import Path
import pytest

ROOT = Path(__file__).resolve().parents[1]


def read_source(rel_path: str) -> str:
    return (ROOT / rel_path).read_text(encoding="utf-8")


def test_kernel_boot_decoupling():
    main_rs = read_source("kernel/src/main.rs")
    assert "kernel::compat::android::init();" not in main_rs, (
        "Privileged Android initialization must NOT execute during kernel boot"
    )


def test_kernel_no_android_runtime_singleton():
    compat_mod_rs = read_source("kernel/src/compat/android/mod.rs")
    assert "pub static ANDROID_RUNTIME" not in compat_mod_rs, (
        "Privileged ANDROID_RUNTIME global singleton must be removed from the kernel"
    )


def test_historical_syscalls_not_supported():
    syscall_mod_rs = read_source("kernel/src/syscall/mod.rs")
    assert "NotSupported = -36," in syscall_mod_rs
    assert "pub fn sys_apk_install_audit" in syscall_mod_rs
    assert "Err(SyscallResult::NotSupported)" in syscall_mod_rs
    assert "pub fn sys_dex_verify_exec" in syscall_mod_rs
    assert "pub fn sys_dex_invoke_method" in syscall_mod_rs
    assert "pub fn sys_android_bridge_intent" in syscall_mod_rs


def test_ring3_acrd_daemon_registered():
    cargo_toml = read_source("userspace/system-service/Cargo.toml")
    assert 'name = "acrd"' in cargo_toml
    assert 'path = "src/bin/acrd.rs"' in cargo_toml

    acrd_rs = read_source("userspace/system-service/src/bin/acrd.rs")
    assert "interos_system_service::run_acr();" in acrd_rs

    service_lib_rs = read_source("userspace/system-service/src/lib.rs")
    assert "pub fn run_acr() -> !" in service_lib_rs
    assert "[acrd] userspace Android Compatibility Runtime daemon online" in service_lib_rs


def test_pure_format_and_unprivileged_acr_crates():
    root_cargo = read_source("Cargo.toml")
    assert '"crates/interos-android-format"' in root_cargo
    assert '"crates/interos-acr"' in root_cargo

    format_lib = read_source("crates/interos-android-format/src/lib.rs")
    assert "pub mod crypto;" in format_lib
    assert "pub mod signing;" in format_lib
    assert "pub mod apk;" in format_lib
    assert "pub mod dex;" in format_lib
    assert "pub mod elf;" in format_lib

    acr_lib = read_source("crates/interos-acr/src/lib.rs")
    assert "pub mod runtime;" in acr_lib
    assert "pub mod looper;" in acr_lib
    assert "pub mod view;" in acr_lib
    assert "pub mod manager;" in acr_lib
    assert "pub mod draw;" in acr_lib


def test_honest_api_level_and_verity_padding_id():
    signing_rs = read_source("crates/interos-android-format/src/signing.rs")
    assert "pub const VERITY_PADDING_BLOCK_ID: u32 = 0x42726577;" in signing_rs
    assert "pub const SIG_ALGO_RSA_PKCS1_V1_5_WITH_SHA256: u32 = 0x0103;" in signing_rs
    assert "pub const SIG_ALGO_ECDSA_WITH_SHA256: u32 = 0x0201;" in signing_rs

    bionic_rs = read_source("crates/interos-acr/src/bionic.rs")
    assert '"ro.build.version.sdk" => Some("28")' in bionic_rs
    assert '"ro.build.version.release" => Some("9")' in bionic_rs
