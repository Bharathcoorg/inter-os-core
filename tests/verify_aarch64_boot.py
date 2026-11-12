# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
InterOS Core — AArch64 QEMU Boot & Contract Verification

Verifies that the AArch64 microkernel boots in QEMU virt machine,
validates all hardware/MMU/EL0 isolation invariants, spawns PID 1 (init),
and exercises the complete system service lifecycle.
"""

import os
import sys
import time
import subprocess
import shutil
import threading
import queue
from pathlib import Path

WORKSPACE_ROOT = Path(__file__).resolve().parent.parent

REQUIRED_SIGNALS = [
    "Inter OS",
    "AArch64 Sovereign Smartphone Microkernel",
    "SimpleFB rasteriser: 720x1440 RGB565 surface rendered",
    "SDHCI / eMMC 5.1",
    "Goodix GT911",
    "AArch64 Context Switch",
    "AArch64 Multi-Tasking Scheduler",
    "AArch64 Capability-Guarded IPC",
    "Dropping CPU execution privilege from EL1 to EL0 via hardware eret",
    "AArch64 EL0 Privilege Isolation: Attempted 'mrs CurrentEL' from EL0 trapped (EC=0x00) VERIFIED.",
    "AArch64 EL0 MMU Memory Isolation: Unauthorized read of kernel DRAM @ 0x40000000 trapped (EC=0x24) VERIFIED.",
    "Task exited with code: 42",
    "[TEST] AArch64 Transactional Switch Rollback VERIFIED.",
    "[TEST] AArch64 5-Stage Reap Evacuation Protocol VERIFIED.",
    "[TEST] MMU Ownership Reclamation Errors & Quarantine Protection VERIFIED.",
    "Spawning Userspace PID 1 (init) at EL0",
    "[PID1] Inter OS userspace init started in Ring 3.",
    "[PID1] Service stop lifecycle contract exercised.",
    "[PID1] Service restart lifecycle contract exercised.",
    "[PID1] All system services running after bootstrap.",
    "[PID1] Service registry observed; userspace lifecycle loop active.",
]


def find_qemu_aarch64():
    candidates = [
        os.environ.get("QEMU_AARCH64"),
        shutil.which("qemu-system-aarch64"),
        shutil.which("qemu-system-aarch64.exe"),
        r"C:\Program Files\qemu\qemu-system-aarch64.exe",
        os.path.expanduser(r"~\qemu\qemu-system-aarch64.exe"),
        "/usr/bin/qemu-system-aarch64",
        "/usr/local/bin/qemu-system-aarch64",
    ]
    for c in candidates:
        if c and Path(c).is_file():
            return str(Path(c).resolve())
    return None


def main():
    print("[*] InterOS Core AArch64 Verification...")
    qemu_bin = find_qemu_aarch64()
    if not qemu_bin:
        print("[ERROR] qemu-system-aarch64 executable not found.", file=sys.stderr)
        return 1

    image_path = WORKSPACE_ROOT / "target" / "kernel-aarch64.img"
    if not image_path.exists():
        print("[*] Image not found, building via interos.py build --target aarch64...")
        res = subprocess.run([sys.executable, str(WORKSPACE_ROOT / "tools" / "interos.py"), "build", "--target", "aarch64"], cwd=WORKSPACE_ROOT)
        if res.returncode != 0 or not image_path.exists():
            print("[ERROR] Failed to build AArch64 image.", file=sys.stderr)
            return 1

    cmd = [
        qemu_bin,
        "-M", "virt",
        "-cpu", "cortex-a53",
        "-m", "1024M",
        "-kernel", str(image_path),
        "-display", "none",
        "-serial", "stdio"
    ]

    print(f"[*] Launching QEMU: {' '.join(cmd)}")
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors="replace", bufsize=1)

    reader_queue = queue.Queue()
    def enqueue_output(out, q):
        for line in iter(out.readline, ''):
            q.put(line)
        out.close()

    t = threading.Thread(target=enqueue_output, args=(proc.stdout, reader_queue))
    t.daemon = True
    t.start()

    captured_lines = []
    start_time = time.time()
    timeout_sec = 25

    try:
        while time.time() - start_time < timeout_sec:
            try:
                line = reader_queue.get(timeout=0.2)
                captured_lines.append(line.rstrip())
                combined = "\n".join(captured_lines)
                if all(sig in combined for sig in REQUIRED_SIGNALS):
                    print("[*] All required boot and lifecycle signals observed!")
                    break
            except queue.Empty:
                if proc.poll() is not None:
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
        print("\n=== AArch64 Verification FAILED ===")
        print(f"Missing {len(missing)} required signals:")
        for m in missing:
            print(f"  [-] {m}")
        print("\nCaptured output (last 30 lines):")
        for l in captured_lines[-30:]:
            print(f"  {l}")
        return 1

    print("\n[SUCCESS] AArch64 QEMU Boot Verification PASSED (100% contract compliance).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
