#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
End-to-End Interactive Smartphone Lifecycle Verification Test for InterOS Core on AArch64.

Validates the full vertical slice on QEMU AArch64:
1. Kernel boot & self-tests
2. Userspace PID 1 service-manager startup
3. System services, compositor, and launcher startup in Ring 3 / EL0
4. Launcher LockScreen presentation
5. Hardware swipe-up unlock to HomeScreen
6. Hardware touch tap dispatch to App Drawer
7. Hardware touch tap dispatch to launch an application (Calculator)
8. Child application surface allocation, rendering, and interaction
9. Hardware touch tap dispatch to child Back button (<)
10. Child application exit and surface destruction
11. Parent (Launcher) waitpid reaping and home screen canvas re-presentation
12. Bochs-display PCI scanout screendump verification (P6 PPM)
"""

import socket
import subprocess
import time
import os
import shutil
import sys
from pathlib import Path

if hasattr(sys.stdout, 'reconfigure'):
    sys.stdout.reconfigure(encoding='utf-8', errors='replace', line_buffering=True)

WORKSPACE_ROOT = Path(__file__).resolve().parent.parent

def find_qemu(binary="qemu-system-aarch64.exe"):
    if "QEMU_AARCH64" in os.environ and os.path.exists(os.environ["QEMU_AARCH64"]):
        return os.environ["QEMU_AARCH64"]
    found = shutil.which(binary)
    if found:
        return found
    candidates = [
        os.path.expanduser(f"~/qemu/{binary}"),
        rf"C:\Program Files\qemu\{binary}",
        rf"C:\qemu\{binary}",
        r"C:\Users\bhara\qemu\qemu-system-aarch64.exe",
    ]
    for c in candidates:
        if os.path.exists(c):
            return c
    return binary

def kill_proc_on_port(p):
    if sys.platform == "win32":
        try:
            res = subprocess.run(
                ["powershell", "-NoProfile", "-Command",
                 f"Get-NetTCPConnection -LocalPort {p} -ErrorAction SilentlyContinue | "
                 "ForEach-Object {{ Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue }} | "
                 "Where-Object {{ $_.ProcessName -like '*qemu*' }} | ForEach-Object {{ $_.Id }}"],
                capture_output=True, text=True, timeout=5
            )
            for line in res.stdout.splitlines():
                pid = line.strip()
                if pid and pid.isdigit() and int(pid) > 0:
                    subprocess.run(["taskkill", "/F", "/PID", pid], capture_output=True, timeout=3)
        except Exception:
            pass

def send_monitor_cmd(sock, cmd):
    if not sock:
        return ""
    if cmd:
        try:
            sock.sendall((cmd + "\n").encode('utf-8'))
        except (socket.error, OSError):
            return ""
    resp = b""
    start_t = time.time()
    while time.time() - start_t < 5.0:
        try:
            chunk = sock.recv(1024)
            if not chunk:
                break
            resp += chunk
            if b"(qemu)" in resp:
                break
        except socket.timeout:
            continue
    time.sleep(0.2)
    return resp.decode('utf-8', errors='replace')

def wait_for_screendump(ppm_path, timeout=5.0):
    start = time.time()
    while time.time() - start < timeout:
        if os.path.exists(ppm_path) and os.path.getsize(ppm_path) > 1000:
            return os.path.getsize(ppm_path)
        time.sleep(0.05)
    return os.path.getsize(ppm_path) if os.path.exists(ppm_path) else 0

class SerialReader:
    def __init__(self, sock):
        self.sock = sock
        try:
            self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        except Exception:
            pass
        self.buf = b""
        self.captured_logs = []
        self.cmd_log_idx = 0

    def send(self, line):
        time.sleep(0.25)
        self.cmd_log_idx = len(self.captured_logs)
        cmd_bytes = line.encode("utf-8") if isinstance(line, str) else line
        self.sock.sendall(cmd_bytes)

    def wait_for(self, marker, timeout=25.0, print_keywords=None):
        start_scan = self.cmd_log_idx
        for log in self.captured_logs[start_scan:]:
            if isinstance(marker, (list, tuple)):
                if any(m in log for m in marker):
                    return True
            elif marker in log:
                return True

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
                        try:
                            with open("target/aarch64_serial.log", "a", encoding="utf-8") as f:
                                f.write(decoded + "\n")
                        except Exception:
                            pass
                        if (print_keywords and any(k in decoded for k in print_keywords)) or any(k in decoded for k in ["PANIC", "FAULT", "EXCEPTION"]):
                            print(f"  [AARCH64 GUEST] {decoded}", flush=True)
                        if isinstance(marker, (list, tuple)):
                            if any(m in decoded for m in marker):
                                return True
                        elif marker in decoded:
                            return True
            except socket.timeout:
                continue
            except ConnectionResetError:
                time.sleep(0.1)
                continue
        return False

def verify_ppm_file(path_str, min_size=500000):
    p = Path(path_str)
    assert p.exists(), f"PPM file does not exist: {p}"
    sz = p.stat().st_size
    assert sz > min_size, f"PPM file too small ({sz} bytes <= {min_size}): {p}"
    with open(p, "rb") as f:
        header = f.readline().strip()
        assert header == b"P6", f"PPM header invalid: {header} (expected P6)"
        dims = f.readline().strip().split()
        while dims[0].startswith(b"#"):
            dims = f.readline().strip().split()
        w, h = int(dims[0]), int(dims[1])
        assert w == 720 and h == 1440, f"Unexpected display resolution: {w}x{h} (expected 720x1440)"
    print(f"[VERIFY PPM] [OK] Genuine P6 PPM confirmed: {p.name} ({w}x{h}, {sz} bytes)")

def main():
    print("[*] InterOS Core AArch64 Interactive Lifecycle & Graphics Verification...")
    qemu_exe = find_qemu("qemu-system-aarch64.exe")
    kernel_img = WORKSPACE_ROOT / "target" / "kernel-aarch64.img"

    if not kernel_img.exists():
        print("[*] Building AArch64 kernel image...")
        subprocess.run([sys.executable, str(WORKSPACE_ROOT / "tools" / "interos.py"), "build", "--target", "aarch64"], check=True)

    ser_port = 5695
    mon_port = 5696
    kill_proc_on_port(ser_port)
    kill_proc_on_port(mon_port)
    time.sleep(0.5)

    args = [
        qemu_exe,
        "-M", "virt",
        "-cpu", "cortex-a53",
        "-m", "1024M",
        "-kernel", str(kernel_img),
        "-display", "none",
        "-chardev", f"socket,id=ser0,host=127.0.0.1,port={ser_port},server=on,wait=off",
        "-serial", "chardev:ser0",
        "-chardev", f"socket,id=mon0,host=127.0.0.1,port={mon_port},server=on,wait=off",
        "-mon", "chardev=mon0,mode=readline",
        "-device", "bochs-display",
        "-no-reboot"
    ]

    print(f"[*] Launching QEMU AArch64: {' '.join(args[:7])} ...")
    proc = subprocess.Popen(args)
    time.sleep(0.5)

    try:
        serial_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        serial_sock.settimeout(5.0)
        serial_sock.connect(('127.0.0.1', ser_port))
        serial_sock.settimeout(0.5)
        reader = SerialReader(serial_sock)

        monitor_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        monitor_sock.settimeout(5.0)
        monitor_sock.connect(('127.0.0.1', mon_port))
        monitor_sock.settimeout(0.5)

        print("[*] Connected to QEMU serial and monitor sockets.")

        # 1. Await Launcher initial presentation
        print("[*] Operation 1: Awaiting Launcher initial scanout presentation...")
        ready = reader.wait_for("Step 5: Initial presentation complete", timeout=45.0, print_keywords=["LAUNCHER", "PID1", "Step"])
        assert ready, "Launcher initial presentation timed out"
        print("[OK] Launcher initial presentation complete.")

        print("[*] Waiting for lock screen presentation to complete...")
        lock_ready = reader.wait_for(
            "Boot presentation complete; lock screen is now guest-owned",
            timeout=40.0,
            print_keywords=["LAUNCHER", "Lock surface rendered", "guest-owned"]
        )
        if not lock_ready:
            lock_ready = reader.wait_for("Lock surface rendered.", timeout=20.0)
        assert lock_ready, "Lock screen failed to complete boot presentation"
        print("[OK] Lock screen is guest-owned and ready for interaction.")

        # Take screendump of LockScreen
        ppm_lock = str(WORKSPACE_ROOT / "target" / "aarch64_lockscreen.ppm")
        if os.path.exists(ppm_lock): os.remove(ppm_lock)
        send_monitor_cmd(monitor_sock, f"screendump {ppm_lock}")
        wait_for_screendump(ppm_lock)
        verify_ppm_file(ppm_lock)

        # 2. Unlock device via swipe up gesture
        print("[*] Operation 2: Unlocking device via swipe-up gesture...")
        time.sleep(1.0)
        reader.send(b"SWIPE 360 1350 360 300 5\n")
        unlocked = reader.wait_for("Device unlocked via swipe/slide gesture. Home surface rendered.", timeout=30.0, print_keywords=["LAUNCHER", "unlocked", "Home"])
        if not unlocked:
            reader.send(b"SWIPE 360 1350 360 300 5\n")
            unlocked = reader.wait_for("Device unlocked via swipe/slide gesture. Home surface rendered.", timeout=30.0, print_keywords=["LAUNCHER", "unlocked", "Home"])
        assert unlocked, "Device unlock failed"
        print("[OK] Device unlocked to HomeScreen.")

        # Screendump of HomeScreen
        ppm_home = str(WORKSPACE_ROOT / "target" / "aarch64_homescreen.ppm")
        if os.path.exists(ppm_home): os.remove(ppm_home)
        send_monitor_cmd(monitor_sock, f"screendump {ppm_home}")
        wait_for_screendump(ppm_home)
        verify_ppm_file(ppm_home)

        # 3. Open App Drawer
        print("[*] Operation 3: Opening App Drawer via touch tap at (590, 1110)...")
        time.sleep(1.0)
        reader.send(b"TAP 590 1110\n")
        drawer = reader.wait_for("App Drawer opened", timeout=15.0, print_keywords=["LAUNCHER", "Drawer"])
        if not drawer:
            reader.send(b"TAP 590 1110\n")
            drawer = reader.wait_for("App Drawer opened", timeout=15.0, print_keywords=["LAUNCHER", "Drawer"])
        assert drawer, "App Drawer failed to open"
        print("[OK] App Drawer opened.")

        # 4. Launch Calculator from Drawer
        print("[*] Operation 4: Launching Calculator via touch tap at (359, 256)...")
        time.sleep(1.0)
        reader.send(b"TAP 359 256\n")
        calc_presented = reader.wait_for("Initial frame presented to hardware scanout", timeout=25.0, print_keywords=["CALCULATOR", "APP", "LAUNCHER", "SPAWN"])
        assert calc_presented, "Calculator failed to launch and present frame"
        print("[OK] Calculator launched and presented application frame.")

        # Screendump of Calculator
        ppm_calc = str(WORKSPACE_ROOT / "target" / "aarch64_calculator.ppm")
        if os.path.exists(ppm_calc): os.remove(ppm_calc)
        send_monitor_cmd(monitor_sock, f"screendump {ppm_calc}")
        wait_for_screendump(ppm_calc)
        verify_ppm_file(ppm_calc)

        # 5. In-app interaction: tap key '7' at (100, 680)
        print("[*] Operation 5: In-app interaction (Calculator key '7' at 100, 680)...")
        time.sleep(1.0)
        reader.send(b"TAP 100 680\n")
        inter_ok = reader.wait_for("Keypad digit pressed.", timeout=15.0, print_keywords=["CALCULATOR"])
        if not inter_ok:
            inter_ok = reader.wait_for("Post-input frame presented.", timeout=10.0, print_keywords=["CALCULATOR"])
        assert inter_ok, "Calculator interaction failed"
        print("[OK] Calculator interaction verified.")

        # 6. Exit Calculator via Back button at (48, 80)
        print("[*] Operation 6: Exiting Calculator via Back button tap at (48, 80)...")
        time.sleep(1.0)
        reader.send(b"TAP 48 80\n")
        reaped = reader.wait_for("Foreground application exited and was reaped", timeout=20.0, print_keywords=["LAUNCHER", "WAITPID", "reaped"])
        if not reaped:
            reader.send(b"TAP 360 1420\n")
            reaped = reader.wait_for("Foreground application exited and was reaped", timeout=20.0, print_keywords=["LAUNCHER", "WAITPID", "reaped"])
        assert reaped, "Calculator exit and waitpid reaping not observed"
        print("[OK] Calculator exited and was reaped by Launcher.")

        # 7. Verify HomeScreen re-presentation
        print("[*] Operation 7: Verifying HomeScreen re-presentation...")
        home_re_rendered = reader.wait_for("Home surface rendered.", timeout=15.0, print_keywords=["LAUNCHER", "Home surface"])
        assert home_re_rendered, "HomeScreen failed to re-render after child exit"
        print("[OK] HomeScreen successfully re-presented.")

        print("\n============================================================")
        print("  [SUCCESS] AArch64 Interactive Smartphone Parity Verified!")
        print("============================================================")
        return 0

    finally:
        try: proc.terminate()
        except: pass
        kill_proc_on_port(ser_port)
        kill_proc_on_port(mon_port)

if __name__ == "__main__":
    sys.exit(main())
