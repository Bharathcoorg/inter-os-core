#!/usr/bin/env python3
"""InterOS service lifecycle and boot evidence gate.

Versioned trusted-service ABI, PID1-owned service lifecycle/recovery, and canonical x86_64 QEMU boot/lifecycle validation.

This script performs source/contract checks only unless --runtime is supplied.
It never labels source checks as runtime verification.
"""
from pathlib import Path
import argparse
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]

def must(text, needle, label):
    if needle not in text:
        raise AssertionError(f"missing {label}: {needle}")

def static_gate():
    abi = (ROOT / "kernel/src/service_abi.rs").read_text(encoding="utf-8")
    sdk_abi = (ROOT / "sdk/rust/interos-std/src/service.rs").read_text(encoding="utf-8")
    services = (ROOT / "kernel/src/services.rs").read_text(encoding="utf-8")
    syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
    pid1 = (ROOT / "userspace/init/src/main.rs").read_text(encoding="utf-8")
    cli = (ROOT / "tools/interos.py").read_text(encoding="utf-8")
    qemu = (ROOT / "tests/verify_x86_boot.py").read_text(encoding="utf-8")
    lifecycle = (ROOT / "tests/test_interactive_lifecycle.py").read_text(encoding="utf-8")

    for needle, label in (
        ('SERVICE_ABI_MAGIC: [u8; 4] = *b"ISVC"', "kernel service ABI magic"),
        ("SERVICE_ABI_VERSION: u16 = 1", "kernel service ABI version"),
        ("SERVICE_ABI_MAX_PAYLOAD: u16 = 4096", "kernel ABI payload bound"),
        ("ServiceOp", "kernel service operations"),
        ("pub fn decode", "kernel service decoder"),
    ):
        must(abi, needle, label)

    for needle, label in (
        ('MAGIC: [u8; 4] = *b"ISVC"', "SDK service ABI magic"),
        ("VERSION: u16 = 1", "SDK service ABI version"),
        ("MAX_PAYLOAD: u16 = 4096", "SDK ABI payload bound"),
        ("pub fn encode", "SDK service encoder"),
        ("pub fn decode", "SDK service decoder"),
    ):
        must(sdk_abi, needle, label)

    for needle, label in (
        ("SERVICE_LIFECYCLE_LOCK", "serialized lifecycle transitions"),
        ("spawn_userspace_service", "Ring-3 service spawning"),
        ("heartbeat_for_task", "task-bound heartbeat"),
        ("recover_failed(id: ServiceId, parent_id: TaskId)", "explicit PID1 recovery"),
        ("service has running dependents", "dependency-aware shutdown"),
        ("failed service task is not terminal", "terminal recovery gate"),
        ("failed service is owned by another parent", "parent ownership recovery gate"),
        ("SERVICE_RETRY_AFTER[idx].store", "bounded restart backoff"),
        ("spawn_verified_rapp_process", "signed service package execution"),
    ):
        must(services, needle, label)

    for needle, label in (
        ("ServiceStart = 86", "service start syscall"),
        ("ServiceStop = 87", "service stop syscall"),
        ("ServiceRecoverFailed = 88", "service recovery syscall"),
        ("ServicePackageVerify = 89", "service package verification syscall"),
        ("ServiceHeartbeat = 90", "service heartbeat syscall"),
        ("ServicePackageLaunch = 91", "service package launch syscall"),
        ("SERVICE_MANAGER_OBJECT", "object-scoped manager authority"),
    ):
        must(syscall, needle, label)

    for needle, label in (
        ("const SERVICES: [u8; 11]", "complete PID1 service set"),
        ("registry_has_all_running_services", "complete service bootstrap validation"),
        ("std::service_recover_failed(service_id)", "PID1 recovery exercise"),
    ):
        must(pid1, needle, label)

    for needle, label in (
        ('target_lower in ("demo", "qemu", "phone")', "canonical QEMU build target"),
        ('"cargo", "build", "--locked"', "reproducible locked build"),
    ):
        must(cli, needle, label)

    for needle, label in (
        ("x86_64 QEMU Canonical Boot Verification", "canonical QEMU verifier"),
        ("Initial presentation complete", "launcher readiness gate"),
        ("Framebuffer PPM verified for current run", "fresh framebuffer evidence"),
    ):
        must(qemu, needle, label)

    must(lifecycle, "QEMU", "interactive QEMU lifecycle verifier")

    print("Service Manager Static Gate: PASS")
    print("Service Supervision Static Gate: PASS")
    print("Lifecycle Architecture Static Gate: PASS")
    return 0

def runtime_gate():
    commands = [
        [sys.executable, str(ROOT / "tests/verify_x86_boot.py")],
        [sys.executable, str(ROOT / "tests/test_interactive_lifecycle.py")],
    ]
    for command in commands:
        print("RUN:", " ".join(command))
        result = subprocess.run(command, cwd=str(ROOT))
        if result.returncode != 0:
            return result.returncode
    print("Interactive Lifecycle Runtime Gate: PASS")
    return 0

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--runtime", action="store_true",
                        help="run the canonical x86_64 QEMU boot and lifecycle gates")
    args = parser.parse_args()
    static_gate()
    if args.runtime:
        return runtime_gate()
    print("Runtime QEMU gate not run; source checks are not runtime evidence.")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
