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
SERVICE_ABI = ROOT / "kernel/src/service_abi.rs"


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
    must(services, "spawn_userspace_service(id as u8, parent_id)",
         "PID1-owned Ring-3 service process spawn")
    must((ROOT / "kernel/src/task/user.rs").read_text(), "pub fn spawn_userspace_service",
         "userspace service process launcher")
    must(pid1, "std::service_start(service_id)", "PID1 service startup ABI")
    must(pid1, "std::service_recover_failed()", "PID1 recovery ABI")

    # Service-manager authority is object-scoped, not an ambient SystemControl
    # right. PID1 must hold the dedicated manager object capability and the
    # syscall gate must verify that exact object before lifecycle operations.
    must(syscall, "task.cap_table.authorize_object(CapabilityType::SystemControl, Rights::INVOKE, SERVICE_MANAGER_OBJECT)",
         "object-scoped service-manager authorization")
    user_task = (ROOT / "kernel/src/task/user.rs").read_text()
    must(user_task, "cap_type: CapabilityType::SystemControl,", "PID1 manager capability type")
    must(user_task, "object_id: crate::syscall::SERVICE_MANAGER_OBJECT,", "PID1 manager object binding")
    must(user_task, 'expect("PID1 SystemControl capability")', "PID1 manager capability installation")

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
        must((ROOT / "kernel/src/task/user.rs").read_text(), f"// forbidden:{forbidden}", f"capability isolation marker {forbidden}")

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
        must((ROOT / "kernel/src/task/user.rs").read_text(), marker, f"service capability matrix {marker}")

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

    # PID1 must prove that the complete service set is running after bootstrap,
    # rather than only proving that one start syscall returned successfully.
    must(pid1, "registry_has_all_running_services(&service_buf[..len])",
         "PID1 full service bootstrap validation")
    must(pid1, "[PID1] All system services running after bootstrap.",
         "PID1 complete service bootstrap evidence")
    must(pid1, 'state != b"running" || task_bytes == b"0"',
         "PID1 rejects non-running/zero-task registry entries")
    must(pid1, "seen == 0xff", "PID1 requires all eight services")


    # M13/M14 process and capability architecture must be userspace-owned and isolated.\n    must(services, "heartbeat_for_task", "service heartbeat ownership check")\n    must((ROOT / "kernel/src/task/user.rs").read_text(), "CapabilityType::SystemControl", "PID1-only manager authority remains distinct")\n    must((ROOT / ".github/workflows/ci.yml").read_text(), "userspace/system-service/Cargo.toml", "Ring-3 service build gate")\n\n    # M15 trusted userspace service ABI is explicit, versioned, and bounded.
    service_abi = SERVICE_ABI.read_text()
    must(service_abi, 'SERVICE_ABI_MAGIC: [u8; 4] = *b"ISVC"', "service ABI magic")
    must(service_abi, "SERVICE_ABI_VERSION: u16 = 1", "service ABI version")
    must(service_abi, "pub enum ServiceOp", "service ABI operation enum")
    must(service_abi, "Register = 1", "service register operation")
    must(service_abi, "Heartbeat = 2", "service heartbeat operation")
    must(service_abi, "pub fn decode", "service ABI decoder")
    must(service_abi, "pub payload_len: u16", "service ABI payload width")
    must(service_abi, "service_capability_allowed", "service capability policy")
    must(sdk, "pub fn service_package_verify", "signed service package SDK ABI")

    # M16 signed service packages must use the RAPP verifier and the narrow
    # service namespace/capability policy before M17 is allowed to execute them.
    must(syscall, "ServicePackageVerify = 89", "service package verification syscall number")
    must(syscall, "sys_service_package_verify", "service package verification kernel ABI")
    must(syscall, "sys_service_package_launch", "verified service package launch handoff")
    must(syscall, "spawn_verified_rapp_process", "verified RAPP Ring-3 execution handoff")
    must(syscall, "launch_verified_package", "verified package launch enters service lifecycle registry")
    must(services, "pub fn launch_verified_package", "authoritative verified service lifecycle launcher")
    must(services, "record.state == ServiceState::Running && record.task != TaskId(0)", "verified launch duplicate-instance guard")
    must(services, 'return Err("running service is not owned by caller")', "verified launch parent ownership guard")
    must(services, "state: ServiceState::Starting", "verified launch starting transition")
    must(services, "state: ServiceState::Failed", "verified launch failure transition")
    must(services, "SERVICE_HEARTBEATS[id as usize].store", "verified launch heartbeat initialization")
    must(sdk, "ServicePackageLaunch = 91", "userspace verified service package launch ABI")
    must(sdk, "pub fn service_package_launch", "userspace verified service package launcher")
    must(syscall, 'path.starts_with("/packages/")', "service package path boundary")
    must(syscall, 'path.ends_with(".rapp")', "service package format boundary")
    must(syscall, "sys_verify_package(cap_slot, path)", "service package signed RAPP verification")
    must(syscall, "service_id_from_package_id", "trusted service package namespace")
    must(syscall, "service_capability_allowed", "service package capability policy")
    must(service_abi, '"org.interos.service.displayd"', "trusted display service namespace")
    must(service_abi, '"org.interos.service.camerad"', "trusted camera service namespace")
    must(service_abi, "1 | 2 | 3 | 6 | 17", "display service capability allowlist")
    must(service_abi, "1 | 3 | 6 | 9 | 10 | 17", "storage service capability allowlist")
    must(service_abi, "1 | 3 | 6 | 12 | 17", "network service capability allowlist")
    must(sdk, "pub mod service;", "userspace trusted service ABI module")
    must(sdk, "ServicePackageVerify = 89", "userspace signed service package ABI")
    must(sdk, "ServiceHeartbeat = 90", "userspace service heartbeat ABI")
    must(sdk, "pub fn service_heartbeat", "userspace heartbeat SDK")
    must((ROOT / "userspace/system-service/src/lib.rs").read_text(), "ci-fault-injection", "service crash fault-injection test hook")
    must((ROOT / ".github/workflows/ci.yml").read_text(), "--features ci-fault-injection", "QEMU crash-recovery validation mode")

    # Supervisor must inspect scheduler state before mutating the registry.
    must(services, "let sched = SCHEDULER.lock();", "supervisor scheduler lock")
    must(services, "let mut registry = SERVICE_RECORDS.lock();",
         "supervisor registry update")
    must(sched, "tcb.parent_id = parent_id;", "scheduler parent assignment")

    # M13 scheduler ownership/reaping invariants: children are explicitly
    # attached to PID1, dead parents' live children are reparented, and reaping
    # requires both the expected parent and a terminal child state.
    must(sched, "if task.id != task_id && task.parent_id == task_id",
         "child reparenting scan")
    must(sched, "task.parent_id = TaskId(0);",
         "orphan reparenting to primordial init")
    must(sched, "&& task.parent_id == parent_id",
         "parent-scoped reaping")
    must(sched, "matches!(task.state, TaskState::Zombie | TaskState::Terminated)",
         "terminal-state reaping gate")
    must(sched, 'return Err("task cannot be terminated by lifecycle manager")',
         "protected current/kernel task termination")

    # Lifecycle termination must not recurse into the service registry while
    # scheduler lock is held; this is a lock-order invariant.
    must(sched, "Do not update the service registry while holding the scheduler lock.",
         "scheduler/service lock-order guard")

    print("service lifecycle contract: PASS")


if __name__ == "__main__":
    main()
