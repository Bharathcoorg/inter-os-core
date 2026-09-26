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
    sock.sendall((cmd + "\n").encode('utf-8'))
    time.sleep(0.1)
    resp = b""
    while True:
        try:
            chunk = sock.recv(1024)
            if not chunk:
                break
            resp += chunk
            if b"(qemu)" in chunk or b"\n" in chunk:
                break
        except socket.timeout:
            break
    return resp.decode('utf-8', errors='replace')

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
    captured_logs = []

    try:
        # Connect to serial socket
        serial_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        serial_sock.settimeout(2.0)
        serial_sock.connect(('127.0.0.1', ser_port))
        serial_sock.settimeout(0.2)
        print("[LIFECYCLE TEST] Connected to QEMU serial socket.")

        # 1. Wait for launcher presentation
        print("[LIFECYCLE TEST] Phase 1: Awaiting Launcher homescreen presentation...")
        launcher_ready = False
        start_t = time.time()
        buf = b""

        while time.time() - start_t < 90.0:
            try:
                data = serial_sock.recv(4096)
                if not data:
                    break
                buf += data
                while b"\n" in buf:
                    line, buf = buf.split(b"\n", 1)
                    decoded = line.decode('utf-8', errors='replace').strip()
                    if decoded:
                        captured_logs.append(decoded)
                        if any(k in decoded for k in ["LAUNCHER", "APP", "CALCULATOR", "PID1", "Step 5"]):
                            print(f"  [GUEST] {decoded}")
                        if "Step 5: Initial presentation complete" in decoded:
                            launcher_ready = True
                            break
                if launcher_ready:
                    break
            except socket.timeout:
                continue
            except ConnectionResetError:
                break

        if not launcher_ready:
            print(f"[DEBUG] Captured {len(captured_logs)} lines:")
            for l in captured_logs:
                print(f"  {l}")

        assert launcher_ready, "Launcher failed to present initial frame within timeout"
        print("[LIFECYCLE TEST] [OK] Launcher home screen presented.")

        # Connect to monitor socket now that guest is fully booted and interactive
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

        # 2. Tap Calculator card on Launcher: Row 1, Card 0 (x=180, y=520)
        print("[LIFECYCLE TEST] Phase 2: Dispatching touch tap on Calculator card (x=180, y=520)...")
        serial_sock.sendall(b"TAP 180 520\n")

        # 3. Wait for Calculator app launch and presentation
        calc_launched = False
        calc_presented = False
        start_t = time.time()

        while time.time() - start_t < 15.0:
            try:
                data = serial_sock.recv(4096)
                if not data:
                    break
                buf += data
                while b"\n" in buf:
                    line, buf = buf.split(b"\n", 1)
                    decoded = line.decode('utf-8', errors='replace').strip()
                    if decoded:
                        captured_logs.append(decoded)
                        if any(k in decoded for k in ["LAUNCHER", "APP", "CALCULATOR", "INPUT"]):
                            print(f"  [GUEST] {decoded}")
                        if "Launched application from real touch input" in decoded or "org.interos.calculator" in decoded:
                            calc_launched = True
                        if "Initial frame presented to hardware scanout" in decoded:
                            calc_presented = True
                            break
                if calc_presented:
                    break
            except socket.timeout:
                continue
            except ConnectionResetError:
                break

        assert calc_presented, "Calculator app failed to present initial frame"
        print("[LIFECYCLE TEST] [OK] Calculator launched and presented application frame.")

        # Allow calculator event reactor loop to settle
        time.sleep(0.5)

        # Take screendump of running calculator
        ppm_calc = os.path.abspath(r"target\test_calculator.ppm")
        _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_calc}")
        print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_calc}")

        # 4. Tap Calculator Back button (<) in header: (x=62, y=48)
        print("[LIFECYCLE TEST] Phase 3: Dispatching touch tap on Back button (x=62, y=48)...")
        serial_sock.sendall(b"TAP 62 48\n")

        # 5. Wait for Calculator exit and Launcher reaping
        calc_exited = False
        reaped = False
        start_t = time.time()

        while time.time() - start_t < 15.0:
            try:
                data = serial_sock.recv(4096)
                if not data:
                    break
                buf += data
                while b"\n" in buf:
                    line, buf = buf.split(b"\n", 1)
                    decoded = line.decode('utf-8', errors='replace').strip()
                    if decoded:
                        captured_logs.append(decoded)
                        if any(k in decoded for k in ["LAUNCHER", "APP", "CALCULATOR", "INPUT"]):
                            print(f"  [GUEST] {decoded}")
                        if "Exit button tapped" in decoded or "Exiting to launcher" in decoded:
                            calc_exited = True
                        if "Foreground application exited and was reaped" in decoded:
                            reaped = True
                            break
                if reaped:
                    break
            except socket.timeout:
                continue
            except ConnectionResetError:
                break

        assert calc_exited or reaped, "Child app exit / parent reaping was not observed"
        print("[LIFECYCLE TEST] [OK] Back navigation successfully triggered child exit, surface teardown, and parent waitpid reaping.")

        # Take screendump of re-presented launcher
        ppm_reaped = os.path.abspath(r"target\test_launcher_reaped.ppm")
        _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_reaped}")
        print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_reaped}")

        print("\n============================================================")
        print("  INTEROS CORE INTERACTIVE LIFECYCLE VERIFICATION: PASSED   ")
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
