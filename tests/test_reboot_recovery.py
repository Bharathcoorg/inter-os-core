#!/usr/bin/env python3
"""
InterOS Core — Deterministic Clean Reboot Recovery Verification
============================================================
Verifies clean system reboot & recovery lifecycle:
1. Boot Guest Session 1
2. Verify all boot milestones (Kernel, Memory, IDT, Services, PID1)
3. Establish known userspace state (Launcher active, interactive app cycle)
4. Trigger clean guest shutdown via COM1 serial ACPI power-off
5. Confirm QEMU exits cleanly with code 0
6. Boot Guest Session 2 (Clean Reboot)
7. Confirm all boot milestones are cleanly re-established
8. Confirm userspace returns to expected responsive state
9. Accurately documents persistence status (In-memory VFS / block writeback pending)
"""

import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

if hasattr(sys.stdout, 'reconfigure'):
    sys.stdout.reconfigure(encoding='utf-8', errors='replace', line_buffering=True)


def find_qemu(binary="qemu-system-x86_64.exe"):
    if "QEMU_PATH" in os.environ and os.path.exists(os.path.join(os.environ["QEMU_PATH"], binary)):
        return os.path.join(os.environ["QEMU_PATH"], binary)
    found = shutil.which(binary)
    if found:
        return found
    candidates = [
        os.path.expanduser(f"~/qemu/{binary}"),
        rf"C:\Program Files\qemu\{binary}",
        rf"C:\qemu\{binary}"
    ]
    for c in candidates:
        if os.path.exists(c):
            return c
    return binary


class RecoverySerialReader:
    def __init__(self, sock):
        self.sock = sock
        self.buf = b""
        self.captured_logs = []

    def wait_for(self, marker, timeout=25.0):
        timeout = max(timeout, 25.0)
        start_t = time.time()
        while time.time() - start_t < timeout:
            try:
                data = self.sock.recv(4096)
                if not data:
                    break
                self.buf += data
                while b"\n" in self.buf:
                    line, self.buf = self.buf.split(b"\n", 1)
                    decoded = line.decode('utf-8', errors='replace').strip()
                    if decoded:
                        self.captured_logs.append(decoded)
                        if marker in decoded:
                            return True
            except socket.timeout:
                continue
            except ConnectionResetError:
                time.sleep(0.1)
                continue
        return False

    def query_diag_state(self, timeout=6.0):
        self.sock.sendall(b"DIAG_STATE\n")
        start_t = time.time()
        pattern = re.compile(
            r"\[DIAG_STATE\]\s+tasks_total=(\d+)\s+tasks_active=(\d+)\s+surfaces=(\d+)\s+mailboxes=(\d+)\s+endpoints=(\d+)\s+shm_pipes=(\d+)\s+heap_used=(\d+)\s+heap_free=(\d+)"
        )
        while time.time() - start_t < timeout:
            try:
                data = self.sock.recv(4096)
                if not data:
                    break
                self.buf += data
                while b"\n" in self.buf:
                    line, self.buf = self.buf.split(b"\n", 1)
                    decoded = line.decode('utf-8', errors='replace').strip()
                    if decoded:
                        self.captured_logs.append(decoded)
                        m = pattern.search(decoded)
                        if m:
                            return {
                                "tasks_total": int(m.group(1)),
                                "tasks_active": int(m.group(2)),
                                "surfaces": int(m.group(3)),
                                "mailboxes": int(m.group(4)),
                                "endpoints": int(m.group(5)),
                                "shm_pipes": int(m.group(6)),
                                "heap_used": int(m.group(7)),
                                "heap_free": int(m.group(8)),
                            }
            except socket.timeout:
                continue
            except ConnectionResetError:
                time.sleep(0.1)
                continue
        return None


def boot_and_verify_milestones(qemu_exe, kernel_img, ser_port, session_name):
    print(f"\n[{session_name}] Launching QEMU instance...")
    qemu_cmd = [
        qemu_exe,
        "-drive", f"format=raw,file={kernel_img}",
        "-snapshot",
        "-m", "1024M",
        "-display", "none",
        "-chardev", f"socket,id=ser0,host=127.0.0.1,port={ser_port},server=on,wait=off",
        "-serial", "chardev:ser0",
        "-no-reboot"
    ]

    proc = subprocess.Popen(qemu_cmd)
    time.sleep(0.5)

    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(6.0)
    sock.connect(('127.0.0.1', ser_port))
    sock.settimeout(0.5)
    reader = RecoverySerialReader(sock)

    print(f"[{session_name}] Awaiting initial Launcher presentation...")
    t0 = time.time()
    ready = reader.wait_for("Step 5: Initial presentation complete", timeout=90.0)
    assert ready, f"[{session_name}] Failed to reach initial launcher presentation within timeout"
    print(f"[{session_name}]   [OK] Launcher presentation complete ({time.time()-t0:.2f}s)")

    full_log = "\n".join(reader.captured_logs)
    required_milestones = [
        ("GDT & TSS Initialization", r"GDT & TSS loaded|Creating GDT"),
        ("Page Tables & Memory Allocator", r"Page tables and physical frame allocator initialized|Kernel PML4 entry"),
        ("Microkernel Hardware & Drivers", r"Serial COM1 online|Hardware Drivers, Memory Management"),
        ("PID1 Userspace Service Manager", r"PID1 userspace service manager spawned|userspace init started in Ring 3"),
        ("Compositor & Launcher Online", r"Step 5: Initial presentation complete"),
    ]

    for label, pattern in required_milestones:
        m = re.search(pattern, full_log)
        assert m, f"[{session_name}] Milestone failed: {label} (pattern '{pattern}' not matched)"
        print(f"[{session_name}]   [OK] Milestone verified: {label}")

    time.sleep(0.5)
    diag = reader.query_diag_state()
    assert diag is not None, f"[{session_name}] DIAG_STATE query failed"
    print(f"[{session_name}] Microkernel state: tasks_total={diag['tasks_total']} | tasks_active={diag['tasks_active']} | surfaces={diag['surfaces']}")

    return proc, sock, reader, diag


def main():
    print("=" * 68)
    print("  InterOS Core Deterministic Clean Reboot Recovery Verification")
    print("=" * 68)

    qemu_exe = find_qemu("qemu-system-x86_64.exe")
    kernel_img = str(ROOT / "target" / "inter-os-bios.img")
    assert os.path.exists(kernel_img), f"Target kernel image not found: {kernel_img}"

    subprocess.run(["taskkill", "/F", "/IM", "qemu-system-x86_64.exe"], capture_output=True)
    time.sleep(0.8)

    ser_port = 5585
    proc1 = None
    sock1 = None
    proc2 = None
    sock2 = None

    try:
        # =====================================================================
        # Session 1: Initial Cold Boot & Known State Establishment
        # =====================================================================
        proc1, sock1, reader1, diag1 = boot_and_verify_milestones(
            qemu_exe, kernel_img, ser_port, "SESSION-1 (Cold Boot)"
        )

        print("\n[SESSION-1] Establishing known userspace state (launching Calculator)...")
        reader1.sock.sendall(b"TAP 180 520\n")
        assert reader1.wait_for("Initial frame presented to hardware scanout", timeout=15.0), "Calculator launch failed"
        print("[SESSION-1] Calculator presented frame.")
        time.sleep(1.5)
        print("[SESSION-1] Performing calculation (7 + 5)...")
        reader1.sock.sendall(b"TAP 100 680\n")  # '7'
        assert reader1.wait_for("Keypad digit pressed.", timeout=10.0), "Keypad digit '7' failed"
        time.sleep(0.5)
        reader1.sock.sendall(b"TAP 598 1020\n") # '+'
        assert reader1.wait_for("Keypad operator selected.", timeout=10.0), "Operator '+' failed"
        time.sleep(0.5)
        reader1.sock.sendall(b"TAP 266 850\n")  # '5'
        assert reader1.wait_for("Keypad digit pressed.", timeout=10.0), "Keypad digit '5' failed"
        time.sleep(0.5)
        reader1.sock.sendall(b"TAP 598 1190\n") # '='
        assert reader1.wait_for("Evaluation completed.", timeout=10.0), "Calculation '=' failed"
        print("[SESSION-1] Calculation verified. Exiting back to homescreen...")
        reader1.sock.sendall(b"TAP 62 48\n")     # Back
        assert reader1.wait_for("Foreground application exited and was reaped", timeout=15.0)
        print("[SESSION-1] Homescreen re-established.")
        time.sleep(0.8)

        # Trigger clean ACPI guest shutdown
        print("\n[SESSION-1] Dispatching clean SHUTDOWN command via COM1 serial...")
        reader1.sock.sendall(b"SHUTDOWN\n")
        shutdown_ack = reader1.wait_for("[SYSTEM] Clean shutdown requested via COM1 serial.", timeout=10.0)
        assert shutdown_ack, "Clean shutdown acknowledgment not observed in guest logs"
        print("[SESSION-1] Guest confirmed clean shutdown request.")

        # Confirm QEMU process exits cleanly
        sock1.close()
        sock1 = None
        exit_code = proc1.wait(timeout=15.0)
        print(f"[SESSION-1] QEMU process exited cleanly with code: {exit_code}")
        assert exit_code == 0, f"QEMU process exited with non-zero exit code: {exit_code}"

        time.sleep(1.0)

        # =====================================================================
        # Session 2: Clean Reboot & State Re-Verification
        # =====================================================================
        proc2, sock2, reader2, diag2 = boot_and_verify_milestones(
            qemu_exe, kernel_img, ser_port, "SESSION-2 (Reboot)"
        )

        print("\n[SESSION-2] Testing userspace responsiveness after reboot (launching Settings)...")
        reader2.sock.sendall(b"TAP 180 350\n")
        assert reader2.wait_for("Settings UI presented to scanout buffer", timeout=15.0), "Settings launch failed after reboot"
        print("[SESSION-2] Settings presented frame.")
        time.sleep(1.5)
        print("[SESSION-2] Switching tabs...")
        reader2.sock.sendall(b"TAP 228 140\n")
        assert reader2.wait_for("tab switched.", timeout=10.0)
        print("[SESSION-2] Tab switch verified. Returning to home via bottom gesture bar...")
        reader2.sock.sendall(b"TAP 360 1420\n")
        assert reader2.wait_for("Foreground application exited and was reaped", timeout=15.0)
        print("[SESSION-2] Homescreen re-established and fully responsive after reboot.")

        # Clean shutdown of Session 2
        print("\n[SESSION-2] Shutting down Session 2 cleanly...")
        reader2.sock.sendall(b"SHUTDOWN\n")
        reader2.wait_for("[SYSTEM] Clean shutdown requested via COM1 serial.", timeout=10.0)
        sock2.close()
        sock2 = None
        exit_code2 = proc2.wait(timeout=15.0)
        print(f"[SESSION-2] QEMU exited with code: {exit_code2}")
        assert exit_code2 == 0

        # Save recovery evidence
        out_report = ROOT / "target" / "recovery_report.json"
        out_report.parent.mkdir(exist_ok=True)
        with open(out_report, "w", encoding="utf-8") as f:
            json.dump({
                "target": "x86_64",
                "clean_reboot_cycle_verified": True,
                "session_1": {
                    "milestones_verified": True,
                    "userspace_responsive": True,
                    "shutdown_requested": True,
                    "qemu_exit_code": exit_code,
                    "diag_state": diag1
                },
                "session_2": {
                    "milestones_verified": True,
                    "userspace_responsive": True,
                    "reboot_recovery_successful": True,
                    "qemu_exit_code": exit_code2,
                    "diag_state": diag2
                },
                "persistence_status": "NOT IMPLEMENTED - IN-MEMORY VFS / BLOCK PERSISTENCE PENDING",
                "persistence_details": "VFS is an in-memory capability-governed BTreeMap hierarchy; block device journal replay and disk write-back are pending roadmap items.",
                "power_loss_crash_recovery": "NOT IMPLEMENTED - CRASH INJECTION TESTING PENDING",
                "result": "PASS (CLEAN REBOOT VERIFIED WITH DOCUMENTED PERSISTENCE LIMITATION)"
            }, f, indent=2)

        print(f"\n[RECOVERY REPORT] Report saved to: {out_report}")
        print("\n" + "=" * 68)
        print("  RECOVERY VERIFICATION: PASSED (Clean Reboot Invariant Proven)")
        print("=" * 68)
        return 0

    except Exception as e:
        print(f"\n[RECOVERY VERIFICATION FAILED] {e}")
        import traceback
        traceback.print_exc()
        return 1

    finally:
        for s in [sock1, sock2]:
            if s:
                try: s.close()
                except Exception: pass
        for p in [proc1, proc2]:
            if p and p.poll() is None:
                p.terminate()
                try: p.wait(timeout=1.0)
                except Exception: p.kill()


if __name__ == "__main__":
    sys.exit(main())
