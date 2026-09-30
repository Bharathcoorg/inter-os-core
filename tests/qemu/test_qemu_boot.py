"""Deterministic QEMU boot smoke tests for both kernel architectures.

These require QEMU and are therefore not part of `run_all_contracts.py`, which
only executes source-level contracts. Run them directly:

    python tests/qemu/test_qemu_boot.py

Both machines are booted with the console captured to a file and are then held
for a fixed window. A boot passes only when the kernel reaches its ready
banner without a kernel panic, so a hang or a crash is reported as a failure
rather than silently passing.
"""

import os
import subprocess
import sys
import time
import tempfile

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))

AARCH64_TARGET = "aarch64-unknown-none"
RISCV_TARGET = "riscv64gc-unknown-none-elf"

AARCH64_KERNEL = os.path.join(
    ROOT, "kernel-aarch64", "target", AARCH64_TARGET, "debug", "kernel-aarch64"
)
RISCV_KERNEL = os.path.join(
    ROOT, "kernel-riscv64", "target", RISCV_TARGET, "debug", "kernel-riscv64"
)

# QEMU is available on PATH in CI images only if installed, so probe per binary.
AARCH64_QEMU = "qemu-system-aarch64"
RISCV_QEMU = "qemu-system-riscv64"

BOOT_WINDOW_SECONDS = 20


def _have(binary):
    try:
        subprocess.run(
            [binary, "--version"],
            capture_output=True,
            timeout=30,
            check=True,
        )
        return True
    except (OSError, subprocess.SubprocessError):
        return False


def _boot(qemu, args, log_path, seconds=BOOT_WINDOW_SECONDS):
    """Boots the machine, capturing the console, then terminates it."""
    cmd = [
        qemu,
        "-machine", "virt",
        "-m", "512",
        "-display", "none",
        "-monitor", "none",
        "-serial", "file:" + log_path,
        "-no-reboot",
    ] + args
    proc = subprocess.Popen(
        cmd, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE
    )
    try:
        deadline = time.time() + seconds
        while time.time() < deadline:
            if proc.poll() is not None:
                break
            time.sleep(0.5)
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=10)
    with open(log_path, "r", encoding="utf-8", errors="replace") as handle:
        return handle.read()


def check_aarch64():
    """AArch64 must reach its ready banner via the conformant arm64 Image path."""
    if not os.path.exists(AARCH64_KERNEL):
        return False, "kernel not built: " + AARCH64_KERNEL
    image = os.path.join(ROOT, "target", "kernel-aarch64.img")
    if not os.path.exists(image):
        return False, "flat image not built: " + image

    with tempfile.TemporaryDirectory() as tmp:
        log = os.path.join(tmp, "aarch64.log")
        out = _boot(
            AARCH64_QEMU,
            ["-cpu", "cortex-a57", "-kernel", os.path.relpath(image, ROOT)],
            log,
        )

    if "KERNEL PANIC" in out or "PANIC" in out:
        return False, "kernel panicked"
    if "[READY]" not in out:
        return False, "never reached the ready banner"
    # The device tree must be discovered, not assumed.
    if "Valid DTB discovered" not in out:
        return False, "no device tree discovered"
    if "Physical RAM discovered" not in out:
        return False, "physical RAM not discovered from the device tree"
    # A platform that exposes GICv2m must not be programmed as GICv2.
    if "arm,gic-v2m" not in out:
        return False, "interrupt controller was not identified from the device tree"
    if "written through as GICv2" not in out:
        return False, "interrupt routing was not declined for a non-GICv2 model"
    return True, "reached ready banner with device-tree discovery"


def check_riscv64():
    """RISC-V must drop to U-mode and return through the syscall path."""
    if not os.path.exists(RISCV_KERNEL):
        return False, "kernel not built: " + RISCV_KERNEL

    with tempfile.TemporaryDirectory() as tmp:
        log = os.path.join(tmp, "riscv64.log")
        out = _boot(
            RISCV_QEMU,
            [
                "-cpu", "rv64",
                "-smp", "1",
                "-bios", "default",
                "-kernel", os.path.relpath(RISCV_KERNEL, ROOT),
            ],
            log,
        )

    if "KERNEL PANIC" in out:
        return False, "kernel panicked"
    if "[USER:LOG]" not in out:
        return False, "userspace never executed"
    if "[SYSCALL:EXIT]" not in out:
        return False, "userspace exit was never observed"
    if "Userspace exit recorded by scheduler: true" not in out:
        return False, "scheduler did not record the task exit"
    if "sstatus.SUM cleared" not in out:
        return False, "supervisor user-memory access was not disabled"
    if "rejected (-EFAULT)" not in out:
        return False, "kernel pointer was not rejected"
    return True, "U-mode syscall round trip with supervisor isolation"


def main():
    results = []
    for name, binary, fn in (
        ("aarch64", AARCH64_QEMU, check_aarch64),
        ("riscv64", RISCV_QEMU, check_riscv64),
    ):
        if not _have(binary):
            print(f"SKIP: {name} ({binary} not available)")
            results.append(None)
            continue
        ok, detail = fn()
        print(f"{'PASS' if ok else 'FAIL'}: {name} -> {detail}")
        results.append(ok)

    if any(r is False for r in results):
        sys.exit(1)


if __name__ == "__main__":
    main()
