# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
InterOS Core — AArch64 Multicore SMP & GICv3 QEMU Verification

Verifies that the AArch64 microkernel boots in QEMU virt machine with
both `-M virt,gic-version=3 -smp 2` and `-M virt,gic-version=3 -smp 4`:
- Full generic GICv3 distributor & redistributor discovery
- Per-PE redistributor wake-up and full MPIDR registration
- Secondary CPU boot via ARM PSCI CPU_ON with dedicated per-CPU stacks
- Targeted Inter-Processor Interrupt (IPI) delivery via GICv3 ICC_SGI1R_EL1
- Independent per-PE local timer PPI delivery on every core
- Strict per-CPU structured evidence parsing and invariant checking
"""

import os
import re
import sys
import time
import subprocess
import shutil
import threading
import queue
from pathlib import Path

WORKSPACE_ROOT = Path(__file__).resolve().parent.parent

CPU_RECORD_RE = re.compile(
    r"CPU logical=(\d+)\s+mpidr=(0x[0-9A-Fa-f]+)\s+redistributor=(0x[0-9A-Fa-f]+)\s+online=(true|false)\s+timer_ticks=(\d+)\s+sgi_received=(\d+)"
)

BASE_REQUIRED_SIGNALS = [
    "Inter OS",
    "AArch64 Sovereign Smartphone Microkernel",
    "GICv3 from device tree: distributor @",
    "redistributors @",
    "full generic GICv3 initialized, IRQ 27 enabled.",
    "[TEST] GICv3 Robustness: Bounds enforcement, truncated window rejection, and SPI routing validation VERIFIED.",
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


def verify_smp_topology(qemu_bin: str, image_path: Path, smp_count: int) -> bool:
    print(f"\n[*] Running AArch64 GICv3 SMP Verification with -smp {smp_count}...")
    required_signals = list(BASE_REQUIRED_SIGNALS) + [
        f"[TEST] AArch64 Multicore SMP: {smp_count}-core boot, per-PE redistributors, and GICv3 SGI IPI VERIFIED."
    ]
    for cid in range(1, smp_count):
        required_signals.append(f"PSCI CPU_ON dispatched for secondary PE {cid}")
        required_signals.append(f"Secondary CPU {cid} online: VBAR, GICv3 Redistributor, and CPU Interface initialized.")
        required_signals.append(f"CPU 0 sent GICv3 SGI 1 to CPU {cid}")
        required_signals.append(f"GICv3 SGI 1 received specifically by target PE {cid}")

    cmd = [
        qemu_bin,
        "-M", "virt,gic-version=3",
        "-cpu", "cortex-a53",
        "-smp", str(smp_count),
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
                if all(sig in combined for sig in required_signals):
                    print(f"[*] All required -smp {smp_count} signals observed!")
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
    missing = [sig for sig in required_signals if sig not in full_output]
    if missing:
        print(f"\n=== AArch64 SMP (-smp {smp_count}) Verification FAILED ===")
        print(f"Missing {len(missing)} required signals:")
        for m in missing:
            print(f"  [-] {m}")
        print("\nCaptured output (last 40 lines):")
        for l in captured_lines[-40:]:
            print(f"  {l}")
        return False

    # Check for explicit error indicators
    for l in captured_lines:
        if "[ERROR]" in l or "[FATAL]" in l or "leaked to unintended PE" in l:
            print(f"\n[FAILED] Error detected in guest log: {l}")
            return False

    # Parse and validate per-CPU runtime records
    cpu_records = {}
    for l in captured_lines:
        m = CPU_RECORD_RE.search(l)
        if m:
            lid = int(m.group(1))
            mpidr = int(m.group(2), 16)
            rdbase = int(m.group(3), 16)
            online = m.group(4)
            timer_ticks = int(m.group(5))
            sgi_received = int(m.group(6))
            cpu_records[lid] = {
                "mpidr": mpidr,
                "rdbase": rdbase,
                "online": online,
                "timer_ticks": timer_ticks,
                "sgi_received": sgi_received,
                "line": l.strip(),
            }

    if set(cpu_records.keys()) != set(range(smp_count)):
        print(f"\n[FAILED] Expected CPU records for 0..{smp_count - 1}, got {sorted(cpu_records.keys())}")
        return False

    seen_mpidrs = set()
    seen_rdbases = set()
    for lid in range(smp_count):
        rec = cpu_records[lid]
        if rec["online"] != "true":
            print(f"[FAILED] CPU {lid} is not online: {rec['line']}")
            return False
        if rec["rdbase"] == 0:
            print(f"[FAILED] CPU {lid} has null redistributor address: {rec['line']}")
            return False
        if rec["mpidr"] in seen_mpidrs:
            print(f"[FAILED] Duplicate MPIDR detected on CPU {lid}: {hex(rec['mpidr'])}")
            return False
        if rec["rdbase"] in seen_rdbases:
            print(f"[FAILED] Duplicate redistributor detected on CPU {lid}: {hex(rec['rdbase'])}")
            return False
        seen_mpidrs.add(rec["mpidr"])
        seen_rdbases.add(rec["rdbase"])

        if rec["timer_ticks"] < 1:
            print(f"[FAILED] CPU {lid} did not observe local timer interrupts: {rec['line']}")
            return False
        if lid == 0 and rec["sgi_received"] != 0:
            print(f"[FAILED] CPU 0 received unintended SGI(s): {rec['line']}")
            return False
        if lid > 0 and rec["sgi_received"] < 1:
            print(f"[FAILED] Secondary CPU {lid} did not receive targeted SGI: {rec['line']}")
            return False

    print(f"\n--- Diagnostic SMP Evidence (-smp {smp_count}) ---")
    for lid in range(smp_count):
        print(f"  {cpu_records[lid]['line']}")

    print(f"[OK] AArch64 SMP (-smp {smp_count}) Verified: {smp_count} distinct PEs, distinct redistributors, isolated SGI IPIs, and independent timers.")
    return True


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

    if not verify_smp_topology(qemu_bin, image_path, 2):
        return 1
    if not verify_smp_topology(qemu_bin, image_path, 4):
        return 1

    print("\n[SUCCESS] AArch64 Multicore SMP & GICv3 Verification PASSED (100% contract compliance across 2-core and 4-core topologies).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
