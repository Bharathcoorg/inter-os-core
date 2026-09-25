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
