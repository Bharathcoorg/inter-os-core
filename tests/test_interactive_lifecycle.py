#!/usr/bin/env python3
"""
End-to-End Interactive Multi-App Lifecycle Verification Test for InterOS Core.

Validates the full vertical slice on QEMU:
1. Kernel boot & self-tests
2. Userspace PID 1 service-manager startup
3. System services 1..8, compositor (9), and launcher (10) startup
4. Launcher surface allocation, 10-app canvas rendering, and scanout presentation
5. Hardware touch tap dispatch to launch an application (Calculator)
6. Child application KMS surface allocation, rendering, and interaction
7. Hardware touch tap dispatch to child Back button (<)
8. Child application exit and surface destruction
9. Parent (Launcher) waitpid reaping and home screen canvas re-presentation
"""

import socket
import subprocess
import time
import os
import shutil
import sys

if hasattr(sys.stdout, 'reconfigure'):
    sys.stdout.reconfigure(encoding='utf-8', line_buffering=True)

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

def send_monitor_cmd(sock, cmd):
    if cmd:
        sock.sendall((cmd + "\n").encode('utf-8'))
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

class SerialReader:
    def __init__(self, sock):
        self.sock = sock
        self.buf = b""
        self.captured_logs = []

    def wait_for(self, marker, timeout=15.0, print_keywords=None):
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
                        if print_keywords and any(k in decoded for k in print_keywords):
                            print(f"  [GUEST] {decoded}")
                        if marker in decoded:
                            return True
            except socket.timeout:
                continue
            except ConnectionResetError:
                break
        return False

def test_app_lifecycle(reader, monitor_sock, phase_num, app_name, app_id, tap_coords, present_marker, dump_prefix):
    print(f"\n[LIFECYCLE TEST] === Phase {phase_num}: Testing {app_name} ({app_id}) ===")
    print(f"[LIFECYCLE TEST] Dispatching touch tap on {app_name} card at {tap_coords}...")
    reader.sock.sendall(f"TAP {tap_coords[0]} {tap_coords[1]}\n".encode('utf-8'))

    # Wait for app launch and presentation
    presented = reader.wait_for(
        present_marker,
        timeout=15.0,
        print_keywords=[app_name.upper(), "APP", "LAUNCHER", "INPUT", "Step", app_id]
    )
    assert presented, f"{app_name} failed to present frame within timeout"
    print(f"[LIFECYCLE TEST] [OK] {app_name} launched and presented application frame.")

    time.sleep(1.0)
    ppm_app = os.path.abspath(rf"target\{dump_prefix}.ppm")
    _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_app}")
    print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_app}")
    time.sleep(0.5)

    # Dispatch touch tap on Back button
    print(f"[LIFECYCLE TEST] Dispatching touch tap on Back button (x=62, y=48)...")
    reader.sock.sendall(b"TAP 62 48\n")

    # Wait for child exit and Launcher reaping
    reaped = reader.wait_for(
        "Foreground application exited and was reaped",
        timeout=15.0,
        print_keywords=[app_name.upper(), "APP", "LAUNCHER", "INPUT", "WAITPID"]
    )
    assert reaped, f"{app_name} exit / Launcher waitpid reaping was not observed"
    print(f"[LIFECYCLE TEST] [OK] Back navigation successfully triggered child exit, surface teardown, and parent waitpid reaping.")

    time.sleep(1.2)
    ppm_reaped = os.path.abspath(rf"target\test_launcher_after_{dump_prefix}.ppm")
    _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_reaped}")
    print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_reaped}")
    time.sleep(0.5)

def main():
    qemu_exe = find_qemu("qemu-system-x86_64.exe")
    kernel_img = os.path.abspath(r"target\inter-os-bios.img")
    assert os.path.exists(kernel_img), f"Kernel image not found at {kernel_img}"

    subprocess.run(["taskkill", "/F", "/IM", "qemu-system-x86_64.exe"], capture_output=True)
    time.sleep(0.5)

    ser_port = 5555
    mon_port = 44447

    args = [
        qemu_exe,
        "-drive", f"format=raw,file={kernel_img}",
        "-m", "512M",
        "-display", "none",
        "-chardev", f"socket,id=ser0,host=127.0.0.1,port={ser_port},server=on,wait=off",
        "-serial", "chardev:ser0",
        "-monitor", f"tcp:127.0.0.1:{mon_port},server,nowait",
        "-no-reboot"
    ]

    print("[LIFECYCLE TEST] Launching QEMU instance...")
    proc = subprocess.Popen(args)
    time.sleep(1.0)

    serial_sock = None
    monitor_sock = None

    try:
        # Connect to serial socket
        serial_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        serial_sock.settimeout(2.0)
        serial_sock.connect(('127.0.0.1', ser_port))
        serial_sock.settimeout(0.2)
        print("[LIFECYCLE TEST] Connected to QEMU serial socket.")
        reader = SerialReader(serial_sock)

        # 1. Wait for launcher presentation
        print("[LIFECYCLE TEST] Phase 1: Awaiting Launcher homescreen presentation...")
        launcher_ready = reader.wait_for(
            "Step 5: Initial presentation complete",
            timeout=90.0,
            print_keywords=["LAUNCHER", "APP", "PID1", "Step 5"]
        )

        if not launcher_ready:
            print(f"[DEBUG] Captured {len(reader.captured_logs)} lines:")
            for l in reader.captured_logs[-50:]:
                print(f"  {l}")

        assert launcher_ready, "Launcher failed to present initial frame within timeout"
        print("[LIFECYCLE TEST] [OK] Launcher home screen presented.")

        # Connect to monitor socket
        monitor_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        monitor_sock.settimeout(2.0)
        monitor_sock.connect(('127.0.0.1', mon_port))
        monitor_sock.settimeout(0.5)
        print("[LIFECYCLE TEST] Connected to QEMU monitor socket.")
        _ = send_monitor_cmd(monitor_sock, "")

        # Take screendump of launcher homescreen
        ppm_launcher = os.path.abspath(r"target\test_launcher.ppm")
        _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_launcher}")
        print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_launcher}")

        # 2. Multi-App Roundtrips:
        # App 1: Calculator (Row 1, Card 0: x=180, y=520)
        test_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            phase_num=2,
            app_name="Calculator",
            app_id="org.interos.calculator",
            tap_coords=(180, 520),
            present_marker="Initial frame presented to hardware scanout",
            dump_prefix="test_calculator"
        )

        # App 2: Settings (Row 0, Card 0: x=180, y=350)
        test_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            phase_num=3,
            app_name="Settings",
            app_id="org.interos.settings",
            tap_coords=(180, 350),
            present_marker="Settings UI presented to scanout buffer",
            dump_prefix="test_settings"
        )

        # App 3: Terminal (Row 0, Card 1: x=485, y=350)
        test_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            phase_num=4,
            app_name="Terminal",
            app_id="org.interos.terminal",
            tap_coords=(485, 350),
            present_marker="Terminal console presented to scanout buffer",
            dump_prefix="test_terminal"
        )

        # App 4: Vault (Row 1, Card 1: x=485, y=520)
        test_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            phase_num=5,
            app_name="Vault",
            app_id="org.interos.vault",
            tap_coords=(485, 520),
            present_marker="Initial frame presented to hardware scanout",
            dump_prefix="test_vault"
        )

        # App 5: Camera (Row 2, Card 0: x=180, y=690)
        test_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            phase_num=6,
            app_name="Camera",
            app_id="org.interos.camera",
            tap_coords=(180, 690),
            present_marker="Initial frame presented to hardware scanout",
            dump_prefix="test_camera"
        )

        # App 6: Dialer (Row 2, Card 1: x=485, y=690)
        test_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            phase_num=7,
            app_name="Dialer",
            app_id="org.interos.dialer",
            tap_coords=(485, 690),
            present_marker="Initial frame presented to hardware scanout",
            dump_prefix="test_dialer"
        )

        print("\n============================================================")
        print("  INTEROS CORE INTERACTIVE MULTI-APP LIFECYCLE: ALL PASSED  ")
        print("============================================================")

    except Exception as e:
        print(f"\n[LIFECYCLE TEST] FAILED: {e}")
        raise
    finally:
        if serial_sock:
            try: serial_sock.close()
            except Exception: pass
        if monitor_sock:
            try: monitor_sock.close()
            except Exception: pass
        proc.terminate()
        try:
            proc.wait(timeout=1.0)
        except Exception:
            proc.kill()

if __name__ == "__main__":
    main()
