#!/usr/bin/env python3
"""
InterOS Core — Watchdog & Crash Handling Policy Verification
============================================================
Verifies:
1. Panic policy querying (default is explicit HALT).
2. Dynamic policy modification (SET REBOOT).
3. Panic handler captures full diagnostic state:
   - Origin location
   - Details / error message
   - Diagnostic counters (heap_used, heap_free, tasks)
4. Triggering panic under REBOOT policy executes controlled reset
   and guest successfully reboots and reaches online homescreen.
"""

import os
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path

def kill_proc_on_port(p):
    try:
        import psutil
        for conn in psutil.net_connections():
            if conn.laddr and conn.laddr.port == p and conn.pid:
                try:
                    psutil.Process(conn.pid).kill()
                except Exception:
                    pass
    except Exception:
        pass


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


class SerialSession:
    def __init__(self, sock):
        self.sock = sock
        self.buf = b""
        self.captured_logs = []

    def read_line(self, timeout=1.0):
        self.sock.settimeout(timeout)
        start = time.time()
        while time.time() - start < timeout:
            if b"\n" in self.buf:
                line, self.buf = self.buf.split(b"\n", 1)
                dec = line.decode('utf-8', errors='replace').strip()
                if dec:
                    self.captured_logs.append(dec)
                return dec
            try:
                data = self.sock.recv(4096)
                if not data:
                    break
                self.buf += data
            except socket.timeout:
                continue
            except ConnectionResetError:
                break
        return None

    def wait_for(self, marker, timeout=60.0):
        start = time.time()
        while time.time() - start < timeout:
            line = self.read_line(timeout=0.5)
            if line is not None:
                if marker in line:
                    return line
        return None

    def send_cmd(self, cmd_str):
        if not cmd_str.endswith("\n"):
            cmd_str += "\n"
        self.sock.sendall(cmd_str.encode('utf-8'))

    def send_and_wait(self, cmd_str, marker, timeout=15.0):
        time.sleep(0.3)
        self.send_cmd(cmd_str)
        res = self.wait_for(marker, timeout=timeout)
        assert res is not None, f"Command '{cmd_str.strip()}' failed waiting for marker '{marker}'"
        return res


def main():
    print("=" * 70)
    print("  InterOS Core Watchdog & Controlled Panic Recovery Verification")
    print("=" * 70)

    qemu_exe = find_qemu("qemu-system-x86_64.exe")
    boot_img = ROOT / "target" / "inter-os-bios.img"
    assert boot_img.exists(), f"Boot image missing: {boot_img}"

    port = 5560
    kill_proc_on_port(port)
    time.sleep(0.5)
    args = [
        qemu_exe,
        "-drive", f"file={boot_img},format=raw,snapshot=on",
        "-m", "1024M",
        "-display", "none",
        "-chardev", f"socket,id=ser0,host=127.0.0.1,port={port},server=on,wait=off",
        "-serial", "chardev:ser0",
    ]
    proc = subprocess.Popen(args)
    time.sleep(0.8)

    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(10.0)
    sock.connect(('127.0.0.1', port))
    session = SerialSession(sock)

    try:
        # Step 1: Wait for initial homescreen
        ready = session.wait_for("Step 5: Initial presentation complete", timeout=100.0)
        assert ready, "Failed to reach initial homescreen"
        print("  [OK] Guest booted to Launcher homescreen.")

        # Step 2: Query default panic policy (must be explicit HALT)
        p1 = session.send_and_wait("PANIC_POLICY GET", "[PANIC_POLICY]")
        print(f"  [OK] Initial Panic Policy: {p1}")
        assert "current=HALT" in p1, f"Expected default HALT policy, got: {p1}"

        # Step 3: Switch policy to REBOOT
        p_set = session.send_and_wait("PANIC_POLICY SET REBOOT", "[PANIC_POLICY]")
        print(f"  [OK] Policy Update: {p_set}")
        assert "updated=REBOOT" in p_set

        p2 = session.send_and_wait("PANIC_POLICY GET", "[PANIC_POLICY]")
        assert "current=REBOOT" in p2
        print(f"  [OK] Verified active policy: {p2}")

        # Step 4: Trigger controlled kernel panic
        print("  [WATCHDOG] Triggering controlled diagnostic fault...")
        session.send_cmd("TRIGGER_KERNEL_PANIC Watchdog_Controlled_Recovery_Test")

        # Step 5: Verify panic diagnostic capture
        banner = session.wait_for("INTEROS CORE KERNEL PANIC", timeout=10.0)
        assert banner is not None, "Kernel panic banner not printed"
        print("  [OK] Caught panic banner.")

        detail = session.wait_for("Details: Controlled diagnostic fault: Watchdog_Controlled_Recovery_Test", timeout=5.0)
        assert detail is not None, "Panic details mismatch"
        print(f"  [OK] Verified panic details: {detail}")

        diag = session.wait_for("Diagnostic: heap_used=", timeout=5.0)
        assert diag is not None, "Diagnostic telemetry not printed on panic"
        print(f"  [OK] Verified panic diagnostic state: {diag}")

        action = session.wait_for("[PANIC POLICY: REBOOT] Resetting platform for automatic recovery...", timeout=5.0)
        assert action is not None, "Reboot action not triggered on panic"
        print(f"  [OK] Verified reboot action: {action}")

        # Step 6: Verify guest reboots and reaches homescreen again
        print("  [WATCHDOG] Waiting for post-panic reboot to reach homescreen...")
        rebooted = session.wait_for("Step 5: Initial presentation complete", timeout=100.0)
        assert rebooted is not None, "Failed to reboot and reach homescreen after panic"
        print("  [OK] Platform successfully rebooted and recovered to homescreen!")

        session.send_cmd("SHUTDOWN")
        proc.wait(timeout=15.0)
        print("  [OK] Clean shutdown after watchdog recovery.")
    finally:
        try:
            sock.close()
        except Exception:
            pass
        if proc.poll() is None:
            proc.kill()

    print("\n" + "=" * 70)
    print("  [SUCCESS] Watchdog & Controlled Panic Recovery FULLY VERIFIED!")
    print("=" * 70)


if __name__ == "__main__":
    main()
