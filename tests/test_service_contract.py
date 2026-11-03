# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SERVICES = (ROOT / "kernel/src/services.rs").read_text(encoding="utf-8")
SYSCALL = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
STD = (ROOT / "sdk/rust/interos-std/src/lib.rs").read_text(encoding="utf-8")
INIT = (ROOT / "userspace/init/src/main.rs").read_text(encoding="utf-8")

def test_service_contract_is_versioned_and_dependency_aware():
    assert "SERVICE_ABI_VERSION" in SERVICES
    assert "ServiceDescriptor" in SERVICES
    assert "dependencies" in SERVICES
    assert "SERVICE_START_TIMEOUT_TICKS" in SERVICES

def test_recovery_is_single_service_operation():
    assert "recover_failed(id: ServiceId, parent_id: TaskId)" in SERVICES
    assert "sys_service_recover_failed(service_id: u8)" in SYSCALL
    # 429c0ab rustfmt-reformatted the SDK syscall wrapper; the argument list now
    # carries a space after the comma.
    assert "ServiceRecoverFailed, service_id as u64" in STD
    # 1ade2d6 replaced PID1's hardcoded eleven-service recovery loop (last seen
    # as `[1u8,2,...11]` after 81550f1; reformatted by 429c0ab; de-duplicated
    # by 751955f) with a registry-driven scan: recovery now iterates exactly
    # the services the kernel registry declares and issues one recover call each.
    assert "fn declared_service_ids" in INIT
    assert "for service_id in declared_service_ids(&recovery_buf[..recovery_len]).iter().copied().flatten()" in INIT
    assert "std::service_recover_failed(service_id)" in INIT
