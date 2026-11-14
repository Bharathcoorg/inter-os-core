# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
InterOS Core — AArch64 Multicore SMP & GICv3 QEMU Verification

Verifies that the AArch64 microkernel boots in QEMU virt machine with
`-M virt,gic-version=3 -smp 2`:
- Full generic GICv3 distributor & redistributor discovery
- Per-PE redistributor wake-up and registration
- Secondary CPU boot via ARM PSCI CPU_ON
- Inter-Processor Interrupt (IPI) delivery via GICv3 ICC_SGI1R_EL1
- SGI interrupt acknowledgment, count increment, and EOI
- Dual-core architectural timer PPI readiness
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
    "GICv3 from device tree: distributor @",
    "redistributors @",
    "full generic GICv3 initialized, IRQ 27 enabled.",
    "[TEST] GICv3 Robustness: Bounds enforcement, truncated window rejection, and SPI routing validation VERIFIED.",
    "PSCI CPU_ON dispatched for secondary PE 1",
    "Secondary CPU 1 online: VBAR, GICv3 Redistributor, and CPU Interface initialized.",
    "CPU 0 sent GICv3 SGI 1 to CPU 1",
    "Multicore GICv3 SGI 1 delivered, acknowledged, and EOI verified on secondary CPU.",
    "GICv3 SGI 1 received specifically by target PE 1 (PE 1 count: 1, PE 0 count: 0).",
    "Independent per-PE timer delivery: PE 0 timer ticks =",
    "[TEST] AArch64 Multicore SMP: Dual-core boot, per-PE redistributors, and GICv3 SGI IPI VERIFIED.",
    "Spawning Userspace PID 1 (init) at EL0",
    "[PID1] Inter OS userspace init started in Ring 3.",
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
    print("[*] InterOS Core AArch64 Multicore SMP & GICv3 Verification...")
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
        "-M", "virt,gic-version=3",
        "-cpu", "cortex-a53",
        "-smp", "2",
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
                    print("[*] All required multicore SMP & GICv3 signals observed!")
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
        print("\n=== AArch64 Multicore SMP Verification FAILED ===")
        print(f"Missing {len(missing)} required signals:")
        for m in missing:
            print(f"  [-] {m}")
        print("\nCaptured output (last 40 lines):")
        for l in captured_lines[-40:]:
            print(f"  {l}")
        return 1

    print("\n--- Diagnostic SMP Evidence ---")
    for l in captured_lines:
        if any(k in l for k in ["SMP", "SGI", "PE", "PSCI", "timer delivery", "GICv3"]):
            print(f"  {l}")

    print("\n[SUCCESS] AArch64 Multicore SMP & GICv3 Verification PASSED (100% contract compliance).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
