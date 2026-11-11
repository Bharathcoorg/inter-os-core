# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
InterOS Core — RISC-V 64 QEMU Boot & Contract Verification

Verifies that the RISC-V 64 microkernel boots in QEMU virt machine,
validates all Sv39 MMU and U-mode isolation invariants, spawns PID 1 (init),
and exercises the complete system service lifecycle.
"""

import os
import sys
import time
import subprocess
import shutil
from pathlib import Path

WORKSPACE_ROOT = Path(__file__).resolve().parent.parent

REQUIRED_SIGNALS = [
    "OpenSBI",
    "Inter OS",
    "RISC-V 64 Sovereign Microkernel",
    "MMU: Sv39 3-level page tables active",
    "Traps: stvec vector table online",
    "Supervisor isolation: sstatus.SUM cleared after trap init",
    "RISC-V Security Invariant: Kernel address pointer rejected",
    "RISC-V Userspace Syscall: SYS_GET_TASK_ID returned valid Task ID",
    "RISC-V Zero Ambient Authority: Syscalls 8, 9, 10, 11, 62, 71 fail closed",
    "Dropping CPU execution privilege from S-mode to U-mode via sret",
    "Hello from InterOS RISC-V Userspace!",
    "Task exited with code 0",
    "Spawning Userspace PID 1 (init) in U-Mode",
    "[PID1] Inter OS userspace init started in Ring 3.",
    "[PID1] Service stop lifecycle contract exercised.",
    "[PID1] Service restart lifecycle contract exercised.",
    "[PID1] All system services running after bootstrap.",
    "[PID1] Service registry observed; userspace lifecycle loop active.",
]


def find_qemu_riscv64():
    candidates = [
        os.environ.get("QEMU_RISCV64"),
        shutil.which("qemu-system-riscv64"),
        shutil.which("qemu-system-riscv64.exe"),
        r"C:\Program Files\qemu\qemu-system-riscv64.exe",
        os.path.expanduser(r"~\qemu\qemu-system-riscv64.exe"),
        "/usr/bin/qemu-system-riscv64",
        "/usr/local/bin/qemu-system-riscv64",
    ]
    for c in candidates:
        if c and Path(c).is_file():
            return str(Path(c).resolve())
    return None


def main():
    print("[*] InterOS Core RISC-V 64 Verification...")
    qemu_bin = find_qemu_riscv64()
    if not qemu_bin:
        print("[ERROR] qemu-system-riscv64 executable not found.", file=sys.stderr)
        return 1

    kernel_path = WORKSPACE_ROOT / "target" / "riscv64gc-unknown-none-elf" / "debug" / "kernel-riscv64"
    if not kernel_path.exists():
        kernel_path = WORKSPACE_ROOT / "kernel-riscv64" / "target" / "riscv64gc-unknown-none-elf" / "debug" / "kernel-riscv64"

    if not kernel_path.exists():
        print("[*] Kernel binary not found, building via interos.py build --target riscv64...")
        res = subprocess.run([sys.executable, str(WORKSPACE_ROOT / "tools" / "interos.py"), "build", "--target", "riscv64"], cwd=WORKSPACE_ROOT)
        if res.returncode != 0 or not kernel_path.exists():
            print("[ERROR] Failed to build RISC-V 64 kernel.", file=sys.stderr)
            return 1

    cmd = [
        qemu_bin,
        "-M", "virt",
        "-cpu", "rv64",
        "-m", "1024M",
        "-bios", "default",
        "-kernel", str(kernel_path),
        "-display", "none",
        "-serial", "stdio"
    ]

    print(f"[*] Launching QEMU: {' '.join(cmd)}")
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors="replace", bufsize=1)

    captured_lines = []
    start_time = time.time()
    timeout_sec = 25

    try:
        while time.time() - start_time < timeout_sec:
            line = proc.stdout.readline()
            if not line and proc.poll() is not None:
                break
            if line:
                captured_lines.append(line.rstrip())
                combined = "\n".join(captured_lines)
                if all(sig in combined for sig in REQUIRED_SIGNALS):
                    print("[*] All required boot and lifecycle signals observed!")
                    break
    finally:
        try:
            proc.terminate()
            proc.wait(timeout=2)
        except Exception:
            try:
                proc.kill()
            except Exception:
                pass

    full_output = "\n".join(captured_lines)
    missing = [sig for sig in REQUIRED_SIGNALS if sig not in full_output]

    if missing:
        print("\n=== RISC-V 64 Verification FAILED ===")
        print(f"Missing {len(missing)} required signals:")
        for m in missing:
            print(f"  [-] {m}")
        print("\nCaptured output (last 30 lines):")
        for l in captured_lines[-30:]:
            print(f"  {l}")
        return 1

    print("\n[SUCCESS] RISC-V 64 QEMU Boot Verification PASSED (100% contract compliance).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
