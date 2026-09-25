#!/usr/bin/env python3
"""Static contract tests for the PID1-owned InterOS service lifecycle.

These checks complement QEMU/build validation by preventing accidental drift
between the kernel lifecycle ABI, userspace SDK, and service-manager ownership
model.
"""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
KERNEL_SYSCALL = ROOT / "kernel/src/syscall/mod.rs"
SDK = ROOT / "sdk/rust/interos-std/src/lib.rs"
SERVICES = ROOT / "kernel/src/services.rs"
PID1 = ROOT / "userspace/init/src/main.rs"
SCHED = ROOT / "kernel/src/task/scheduler.rs"


def must(text: str, needle: str, label: str) -> None:
    assert needle in text, f"missing {label}: {needle}"


def main() -> None:
    syscall = KERNEL_SYSCALL.read_text()
    sdk = SDK.read_text()
    services = SERVICES.read_text()
    pid1 = PID1.read_text()
    sched = SCHED.read_text()

    # Kernel and SDK lifecycle ABI must stay numerically aligned.
    for name, number in (
        ("ServiceList", 81),
        ("SettingsGet", 82),
        ("SettingsSet", 83),
        ("StorageQuery", 84),
        ("FsList", 85),
        ("ServiceStart", 86),
        ("ServiceStop", 87),
        ("ServiceRecoverFailed", 88),
    ):
        must(syscall, f"{name} = {number}", f"kernel {name}")
        must(sdk, f"{name} = {number}", f"SDK {name}")

    # PID1 is the only startup owner; kernel init must only register definitions.
    must(services, "PID 1 controls startup.", "kernel startup ownership")
    must(services, "spawn_with_parent(name, entry, caps(id), priority, parent_id)",
         "explicit service parent ownership")
    must(pid1, "std::service_start(service_id)", "PID1 service startup ABI")
    must(pid1, "std::service_recover_failed()", "PID1 recovery ABI")

    # Service-manager authority is object-scoped, not an ambient SystemControl
    # right. PID1 must hold the dedicated manager object capability and the
    # syscall gate must verify that exact object before lifecycle operations.
    must(syscall, "task.cap_table.authorize_object(CapabilityType::SystemControl, Rights::INVOKE, SERVICE_MANAGER_OBJECT)",
         "object-scoped service-manager authorization")
    must(pid1, "cap_type: CapabilityType::SystemControl,", "PID1 manager capability type")
    must(pid1, "object_id: crate::syscall::SERVICE_MANAGER_OBJECT,", "PID1 manager object binding")
    must(pid1, "expect(\"PID1 SystemControl capability\")", "PID1 manager capability installation")

    # Service isolation and lifecycle state transitions are explicit.
    must(services, "CapabilityType::IpcReceive", "service IPC receive capability")
    must(services, "IpcSend is intentionally omitted", "no broad service IPC send")
    must(services, "state: ServiceState::Starting", "starting state")
    must(services, "state: ServiceState::Failed", "failed state")
    must(services, "SERVICE_RETRY_AFTER[idx].store",
         "restart backoff reservation")
    must(services, "scheduler.reap_child(parent_id, task)",
         "recovery child reaping")
    must(services, 'record.state == ServiceState::Failed && record.task != TaskId(0)',
         "failed worker cannot be started twice")
    must(services, "task.parent_id != parent_id",
         "service stop parent ownership")
    must(services, 'return Err("service is not owned by caller")',
         "cross-parent service stop rejection")
    must(services, "Do not grant generic SystemControl to individual services.",
         "no ambient SystemControl in workers")
    must(services, "ServiceId::Power | ServiceId::Modem | ServiceId::Audio | ServiceId::Camera =>",
         "privileged service capability restriction")
    must(services, "static SERVICE_LIFECYCLE_LOCK: spin::Mutex<()>",
         "serialized lifecycle transition lock")
    must(services, "let _lifecycle = SERVICE_LIFECYCLE_LOCK.lock();",
         "lifecycle lock acquisition")
    must(services, "fn start_locked(id: ServiceId, parent_id: TaskId)",
         "non-recursive locked start helper")
    must(services, "start_locked(id, parent_id).is_ok()",
         "recovery uses locked start helper")

    # Service capability matrix: each worker gets only the narrow authority
    # required for its current kernel-hosted boundary. Privileged hardware
    # services must not inherit unrelated process, storage, network, or control
    # powers while their dedicated ABIs are still being designed.
    for forbidden in (
        "CapabilityType::ProcessKill",
        "CapabilityType::ProcessSpawn",
        "CapabilityType::StorageRawBlock",
        "CapabilityType::SystemControl",
        "CapabilityType::NetworkSend",
        "CapabilityType::NetworkReceive",
        "CapabilityType::HardwarePortIo",
    ):
        must(services, f"// forbidden:{forbidden}", f"capability isolation marker {forbidden}")

    for marker in (
        "// service-cap:Display=FrameBufferDirect:WRITE",
        "// service-cap:Input=EventPoll:READ",
        "// service-cap:Storage=FileSystemRead:READ|WRITE",
        "// service-cap:Network=NetworkSocket:READ|WRITE",
        "// service-cap:Power=EventPoll:READ",
        "// service-cap:Modem=EventPoll:READ",
        "// service-cap:Audio=EventPoll:READ",
        "// service-cap:Camera=EventPoll:READ",
        "// service-cap:Init=ProcessSpawn:GRANT",
    ):
        must(services, marker, f"service capability matrix {marker}")

    # M11 executable contract: PID1 must exercise the complete deterministic
    # lifecycle smoke sequence used by the QEMU vertical slice.
    must(pid1, "const SERVICES: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];",
         "complete system-service bootstrap set")
    must(pid1, "match std::service_stop(1)", "PID1 service stop exercise")
    must(pid1, "std::service_start(1)", "PID1 service restart exercise")
    must(pid1, "[PID1] Service stop lifecycle contract exercised.",
         "PID1 successful stop evidence")
    must(pid1, "[PID1] Service restart lifecycle contract exercised.",
         "PID1 successful restart evidence")
    must(pid1, "std::service_list(&mut service_buf)",
         "PID1 service registry observation")
    must(pid1, "std::service_recover_failed()",
         "PID1 explicit recovery request")

    # Supervisor must inspect scheduler state before mutating the registry.
    must(services, "let sched = SCHEDULER.lock();", "supervisor scheduler lock")
    must(services, "let mut registry = SERVICE_RECORDS.lock();",
         "supervisor registry update")
    must(sched, "tcb.parent_id = parent_id;", "scheduler parent assignment")

    # Lifecycle termination must not recurse into the service registry while
    # scheduler lock is held; this is a lock-order invariant.
    must(sched, "Do not update the service registry while holding the scheduler lock.",
         "scheduler/service lock-order guard")

    print("service lifecycle contract: PASS")


if __name__ == "__main__":
    main()
