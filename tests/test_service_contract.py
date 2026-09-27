from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SERVICES = (ROOT / "kernel/src/services.rs").read_text()
SYSCALL = (ROOT / "kernel/src/syscall/mod.rs").read_text()
STD = (ROOT / "sdk/rust/interos-std/src/lib.rs").read_text()
INIT = (ROOT / "userspace/init/src/main.rs").read_text()

def test_service_contract_is_versioned_and_dependency_aware():
    assert "SERVICE_ABI_VERSION" in SERVICES
    assert "ServiceDescriptor" in SERVICES
    assert "dependencies" in SERVICES
    assert "SERVICE_START_TIMEOUT_TICKS" in SERVICES

def test_recovery_is_single_service_operation():
    assert "recover_failed(id: ServiceId, parent_id: TaskId)" in SERVICES
    assert "sys_service_recover_failed(service_id: u8)" in SYSCALL
    assert "ServiceRecoverFailed,service_id as u64" in STD
    assert "for service_id in 1u8..=10u8" in INIT
