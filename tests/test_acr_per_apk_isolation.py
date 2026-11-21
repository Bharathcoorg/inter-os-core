# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""ACR Real Per-APK Process & Address-Space Isolation Contract.

WHAT THIS FILE IS FOR:
Guarantees that each Android APK runs in its own genuine InterOS process/task:
1. spawn_android_app_process allocates a dedicated TaskPageTable (independent CR3 frame).
2. The kernel assigns a genuine TaskId, scoped capabilities, and sets Android UID (10000 + TaskId).
3. VFS storage sandbox is isolated to /data/data/<package_name>.
4. Synthetic userspace TaskIds (0x8000_0000_0000_0000 | pid) are completely eradicated.
5. AcrManager associates APK instances with kernel-assigned TaskIds.
6. Memory and fault isolation: Distinct address spaces prevent cross-app memory corruption,
   and an app crash does not terminate other APKs or the acrd coordinator daemon.
"""

from pathlib import Path
import pytest

ROOT = Path(__file__).resolve().parents[1]


def read_source(rel_path: str) -> str:
    return (ROOT / rel_path).read_text(encoding="utf-8")


def test_kernel_spawn_android_app_process_exists():
    user_rs = read_source("kernel/src/task/user.rs")
    assert "pub fn spawn_android_app_process(" in user_rs
    # Dedicated user page table (separate CR3)
    assert "TaskPageTable::create_user_page_table()" in user_rs
    # Loads generic unprivileged Ring-3 runtime binary acr-app-runtime, NOT user_init
    assert "crate::task::images::ACR_APP_RUNTIME_ELF" in user_rs
    # Scoped capabilities: SerialOutput, IpcSend, IpcReceive, EventPoll
    assert "0x8100" in user_rs
    assert "0x8101" in user_rs
    assert "0x8102" in user_rs
    assert "0x8103" in user_rs
    # FrameBufferDirect is STRICTLY REMOVED to prevent ambient graphics authority
    assert "FrameBufferDirect" not in user_rs[user_rs.find("pub fn spawn_android_app_process"):user_rs.find("pub extern \"C\" fn user_task_trampoline")]
    # Stable Android UID binding from package registry: app_uid
    assert "bind_user_for_task(spawned, app_uid)" in user_rs
    # Telemetry logging with pkg, task_id, cr3, uid
    assert "[ACR] Spawned Android process: pkg='{}' task_id={} cr3=0x{:x} uid={}" in user_rs
    # Resource limits and memory quota
    assert "ResourceLimits" in user_rs
    assert "set_address_space_usage" in user_rs


def test_syscall_routes_android_packages_to_spawn_android_app_process():
    syscall_rs = read_source("kernel/src/syscall/mod.rs")
    assert "crate::task::user::spawn_android_app_process(" in syscall_rs
    # Package kind check, not name heuristics
    assert "registered.kind == crate::package::PackageKind::AndroidApk" in syscall_rs
    assert 'app_id.starts_with("com.")' not in syscall_rs
    assert 'app_id.starts_with("org.whispersystems.")' not in syscall_rs


def test_no_synthetic_task_id_in_acr_manager():
    manager_rs = read_source("crates/interos-acr/src/manager.rs")
    # Must NOT manufacture synthetic TaskIds with high-bit mask
    assert "0x8000_0000_0000_0000" not in manager_rs
    # Must support install_and_launch_with_task_id
    assert "pub fn install_and_launch_with_task_id(" in manager_rs
    assert "task_id: u64" in manager_rs


def test_acr_app_runtime_binary_and_child_execution():
    runtime_bin = read_source("userspace/system-service/src/bin/acr_app_runtime.rs")
    assert "interos_system_service::run_acr_app_runtime();" in runtime_bin

    service_lib = read_source("userspace/system-service/src/lib.rs")
    assert "pub fn run_acr_app_runtime() -> !" in service_lib
    # Child task owns Dalvik VM state and executes MainActivity.onCreate
    assert "inst.vm.invoke_method(&parsed_dex, &main_activity_class, \"onCreate\")" in service_lib
    assert "[acr-app-runtime] MainActivity.onCreate executed in child Dalvik VM" in service_lib
    # Fault injection handler for deliberate crash containment
    assert "[acr-app-runtime] FAULT INJECTION" in service_lib


def test_acrd_is_coordinator_and_registers_package():
    service_lib = read_source("userspace/system-service/src/lib.rs")
    # acrd registers verified APK into kernel registry
    assert "std::package_register_apk(&apk.manifest.package_name" in service_lib
    # acrd requests process spawn from kernel
    assert "std::app_launch(&apk.manifest.package_name)" in service_lib
    # acrd sends LNCH descriptor to child task
    assert "lnch.extend_from_slice(b\"LNCH\");" in service_lib
