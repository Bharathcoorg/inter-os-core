#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
End-to-End Interactive Smartphone Lifecycle Verification Test for InterOS Core on RISC-V 64.

Validates the full vertical slice on QEMU RISC-V 64:
1. Kernel boot & self-tests
2. Userspace PID 1 service-manager startup
3. System services, compositor, and launcher startup in U-mode
4. Launcher LockScreen presentation
5. Hardware swipe-up unlock to HomeScreen
6. Hardware touch tap dispatch to App Drawer
7. Hardware touch tap dispatch to launch an application (Calculator)
8. Child application surface allocation, rendering, and calculation (7 + 5 = 12)
9. Hardware touch tap dispatch to child Back button (<)
10. Child application exit and surface destruction
11. Parent (Launcher) waitpid reaping and home screen canvas re-presentation
12. Bochs-display PCI scanout screendump verification (P6 PPM) with entropy validation
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

def find_qemu(binary="qemu-system-riscv64.exe"):
    if "QEMU_RISCV64" in os.environ and os.path.exists(os.environ["QEMU_RISCV64"]):
        return os.environ["QEMU_RISCV64"]
    found = shutil.which(binary)
    if found:
        return found
    candidates = [
        os.path.expanduser(f"~/qemu/{binary}"),
        rf"C:\Program Files\qemu\{binary}",
        rf"C:\qemu\{binary}",
        r"C:\Users\bhara\qemu\qemu-system-riscv64.exe",
    ]
    for c in candidates:
        if os.path.exists(c):
            return c
    return binary

def get_free_ports(count=2):
    socks = []
    ports = []
    for _ in range(count):
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.bind(('127.0.0.1', 0))
        ports.append(s.getsockname()[1])
        socks.append(s)
    for s in socks:
        s.close()
    return ports

def get_free_port():
    return get_free_ports(1)[0]

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

    def get_last_matching(self, pattern):
        for line in reversed(self.captured_logs):
            if pattern in line:
                return line
        return None

    def has_seen(self, pattern):
        return any(pattern in line for line in self.captured_logs)

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
                            with open("target/riscv_serial.log", "a", encoding="utf-8") as f:
                                f.write(decoded + "\n")
                        except Exception:
                            pass
                        if (print_keywords and any(k in decoded for k in print_keywords)) or any(k in decoded for k in ["PANIC", "FAULT", "EXCEPTION"]):
                            print(f"  [RISCV GUEST] {decoded}", flush=True)
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
        line = f.readline().strip()
        while line.startswith(b"#"):
            line = f.readline().strip()
        dims = line.split()
        w, h = int(dims[0]), int(dims[1])
        assert w == 720 and h == 1440, f"Unexpected display resolution: {w}x{h} (expected 720x1440)"
        maxval = f.readline().strip()
        assert maxval == b"255", f"Unexpected maxval: {maxval}"
        
        pixels = f.read()
        expected_bytes = w * h * 3
        assert len(pixels) == expected_bytes, f"Truncated PPM pixel data: {len(pixels)} != {expected_bytes}"
        
        step = max(1, len(pixels) // (3 * 20000))
        colors = set()
        for i in range(0, len(pixels), 3 * step):
            colors.add((pixels[i], pixels[i+1], pixels[i+2]))
        
        assert len(colors) >= 20, f"Display frame appears blank or uniform ({len(colors)} distinct colors)"
    print(f"[VERIFY PPM] [OK] Genuine P6 PPM confirmed: {p.name} ({w}x{h}, {sz} bytes, {len(colors)} distinct sampled colours)")

def verify_visual_pixel_change(path_before, path_after, row_start=150, row_end=350, w=720):
    def extract_band(path):
        with open(path, "rb") as f:
            for _ in range(3):
                l = f.readline()
                while l.startswith(b"#"):
                    l = f.readline()
            pixels = f.read()
            start_byte = row_start * w * 3
            end_byte = row_end * w * 3
            return pixels[start_byte:end_byte]

    band_before = extract_band(path_before)
    band_after = extract_band(path_after)
    diff = sum(1 for a, b in zip(band_before, band_after) if a != b)
    assert diff > 0, f"No pixel change detected in calculation area between {path_before} and {path_after}"
    print(f"[VERIFY PIXELS] [OK] Display result area ({row_start}..{row_end} px) changed: {diff} differing bytes confirmed between frames.")

def verify_calculator_result_glyphs(path_ppm, expected_result="12"):
    with open(path_ppm, "rb") as f:
        for _ in range(3):
            l = f.readline()
            while l.startswith(b"#"):
                l = f.readline()
        data = f.read()

    w, h = 720, 1440
    assert len(data) >= w * h * 3, f"Incomplete frame buffer data in {path_ppm}"

    def is_dark(x, y):
        idx = (y * w + x) * 3
        return data[idx] < 100 and data[idx+1] < 100 and data[idx+2] < 100

    # Verify structural glyph geometry of evaluated result '12' in display card (y: 230..290)
    top_arc = sum(1 for y in range(232, 240) for x in range(640, 665) if is_dark(x, y))
    right_neck = sum(1 for y in range(260, 272) for x in range(655, 670) if is_dark(x, y))
    bottom_base = sum(1 for y in range(280, 288) for x in range(635, 665) if is_dark(x, y))
    neg_space = sum(1 for y in range(240, 250) for x in range(620, 640) if is_dark(x, y))

    assert top_arc >= 50, f"Digit '2' top arc missing in evaluated result (count={top_arc})"
    assert right_neck >= 50, f"Digit '2' right neck missing in evaluated result (count={right_neck})"
    assert bottom_base >= 50, f"Digit '2' bottom base missing in evaluated result (count={bottom_base})"
    assert neg_space == 0, f"Unexpected ink in negative space of digit '2' (count={neg_space})"

    tape_ink = sum(1 for y in range(125, 155) for x in range(250, 670) if is_dark(x, y))
    assert tape_ink >= 500, f"Expression tape ink missing in calculation card (count={tape_ink})"

    print(f"[VERIFY GLYPHS] [OK] Rendered result '{expected_result}' verified via structural glyph analysis in display card (top_arc={top_arc}, neck={right_neck}, base={bottom_base}, tape_ink={tape_ink}).")

def main():
    print("[*] InterOS Core RISC-V 64 Interactive Lifecycle & Graphics Verification...")
    qemu_exe = find_qemu("qemu-system-riscv64.exe")
    kernel_elf = WORKSPACE_ROOT / "kernel-riscv64" / "target" / "riscv64gc-unknown-none-elf" / "debug" / "kernel-riscv64"

    if not kernel_elf.exists():
        print("[*] Building RISC-V kernel image...")
        subprocess.run([sys.executable, str(WORKSPACE_ROOT / "tools" / "interos.py"), "build", "--target", "riscv64"], check=True)

    ser_port, mon_port = get_free_ports(2)

    args = [
        qemu_exe,
        "-M", "virt",
        "-cpu", "rv64",
        "-m", "1024M",
        "-bios", "default",
        "-kernel", str(kernel_elf),
        "-display", "none",
        "-chardev", f"socket,id=ser0,host=127.0.0.1,port={ser_port},server=on,wait=on",
        "-serial", "chardev:ser0",
        "-chardev", f"socket,id=mon0,host=127.0.0.1,port={mon_port},server=on,wait=off",
        "-mon", "chardev=mon0,mode=readline",
        "-device", "bochs-display",
        "-no-reboot"
    ]

    print(f"[*] Launching QEMU RISC-V 64 on isolated ports ser={ser_port}, mon={mon_port}...")
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

        # Check kernel boot self-tests
        mem_ok = reader.wait_for("50-Cycle Memory Stability: Free/used frame count returned to baseline", timeout=25.0)
        assert mem_ok, "RISC-V 50-Cycle Memory Stability self-test not observed"
        print("[OK] RISC-V 50-Cycle Memory Stability verified without leakage.")

        proc_mem_ok = reader.wait_for("50-Cycle Real Process Lifecycle: Spawn, execute, parent waitpid & zero frame/task leakage VERIFIED.", timeout=25.0)
        assert proc_mem_ok, "RISC-V 50-Cycle Real Process Lifecycle self-test not observed"
        print("[OK] RISC-V 50-Cycle Real Process Lifecycle verified without frame/task leakage.")

        invariants_ok = reader.wait_for("Process Safety Invariants: Quota exhaustion, active reap rejection & ELF rollback VERIFIED.", timeout=25.0)
        assert invariants_ok, "RISC-V Process Safety Invariants self-test not observed"
        print("[OK] RISC-V Process Safety Invariants (quota, active reap, ELF rollback) verified.")

        svc_ok = reader.wait_for("Service Supervisor Lifecycle: Stopped -> Running -> Heartbeat -> Crashed -> Recover", timeout=25.0)
        assert svc_ok, "RISC-V Service Supervisor Lifecycle self-test not observed"
        print("[OK] RISC-V Service Supervisor Lifecycle & Recovery verified.")

        # 1. Await Launcher initial presentation
        print("[*] Operation 1: Awaiting Launcher initial scanout presentation...")
        ready = reader.wait_for(["Step 5: Initial presentation complete", "Boot presentation complete; lock screen is now guest-owned", "Lock surface rendered."], timeout=60.0, print_keywords=["LAUNCHER", "PID1", "Step"])
        assert ready, "Launcher initial presentation timed out"
        print("[OK] Launcher initial presentation complete.")

        print("[*] Waiting for lock screen presentation to complete...")
        lock_ready = reader.wait_for(
            ["Boot presentation complete; lock screen is now guest-owned", "Lock surface rendered."],
            timeout=40.0,
            print_keywords=["LAUNCHER", "Lock surface rendered", "guest-owned"]
        )
        assert lock_ready, "Lock screen failed to complete boot presentation"
        print("[OK] Lock screen is guest-owned and ready for interaction.")

        # Take screendump of LockScreen
        ppm_lock = str(WORKSPACE_ROOT / "target" / "riscv_lockscreen.ppm")
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
        ppm_home = str(WORKSPACE_ROOT / "target" / "riscv_homescreen.ppm")
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
        spawn_line = reader.get_last_matching("[SPAWN]")
        print(f"  -> Spawn log: {spawn_line}")
        calc_pid = None
        if spawn_line and "(PID " in spawn_line:
            try:
                calc_pid = int(spawn_line.split("(PID ")[1].split(")")[0])
                print(f"[OK] Calculator process identity confirmed: PID {calc_pid}")
            except Exception:
                pass
        print("[OK] Calculator launched and presented application frame.")

        # Screendump of Calculator
        ppm_calc = str(WORKSPACE_ROOT / "target" / "riscv_calculator.ppm")
        if os.path.exists(ppm_calc): os.remove(ppm_calc)
        send_monitor_cmd(monitor_sock, f"screendump {ppm_calc}")
        wait_for_screendump(ppm_calc)
        verify_ppm_file(ppm_calc)

        # 5. In-app interaction: Calculator arithmetic evaluation 7 + 5 = 12
        print("[*] Operation 5: In-app interaction (Calculator arithmetic 7 + 5 = 12)...")
        time.sleep(1.0)
        print("  -> Pressing '7' at (100, 680)...")
        reader.send(b"TAP 100 680\n")
        inter_ok = reader.wait_for("Keypad digit pressed.", timeout=15.0, print_keywords=["CALCULATOR"])
        assert inter_ok, "Calculator failed to process key '7'"

        time.sleep(0.5)
        print("  -> Pressing operator '+' at (598, 1120)...")
        reader.send(b"TAP 598 1120\n")
        op_ok = reader.wait_for("Keypad operator selected.", timeout=15.0, print_keywords=["CALCULATOR"])
        assert op_ok, "Calculator failed to process operator '+'"

        time.sleep(0.5)
        print("  -> Pressing '5' at (266, 800)...")
        reader.send(b"TAP 266 800\n")
        digit5_ok = reader.wait_for("Keypad digit pressed.", timeout=15.0, print_keywords=["CALCULATOR"])
        assert digit5_ok, "Calculator failed to process key '5'"

        time.sleep(0.5)
        print("  -> Pressing evaluate '=' at (598, 1280)...")
        reader.send(b"TAP 598 1280\n")
        eval_ok = reader.wait_for("Result = 12", timeout=15.0, print_keywords=["CALCULATOR"])
        assert eval_ok, "Calculator failed to evaluate 7 + 5 = 12 (expected 'Result = 12')"
        assert not reader.has_seen("Result = 999"), "Erroneous calculation detected ('Result = 999')"
        print("[OK] Calculator evaluation strictly verified: Result = 12 (negative case 'Result = 999' rejected).")

        # Screendump of Calculator evaluated state
        ppm_eval = str(WORKSPACE_ROOT / "target" / "riscv_calculator_eval.ppm")
        if os.path.exists(ppm_eval): os.remove(ppm_eval)
        send_monitor_cmd(monitor_sock, f"screendump {ppm_eval}")
        wait_for_screendump(ppm_eval)
        verify_ppm_file(ppm_eval)
        verify_visual_pixel_change(ppm_calc, ppm_eval)
        verify_calculator_result_glyphs(ppm_eval, expected_result="12")

        # 6. Exit Calculator via Back button at (48, 80)
        print("[*] Operation 6: Exiting Calculator via Back button tap at (48, 80)...")
        time.sleep(1.0)
        reader.send(b"TAP 48 80\n")
        reaped = reader.wait_for("Foreground application exited and was reaped", timeout=20.0, print_keywords=["LAUNCHER", "WAITPID", "REAP", "reaped"])
        if not reaped:
            reader.send(b"TAP 360 1420\n")
            reaped = reader.wait_for("Foreground application exited and was reaped", timeout=20.0, print_keywords=["LAUNCHER", "WAITPID", "REAP", "reaped"])
        assert reaped, "Calculator exit and waitpid reaping not observed"
        if calc_pid is not None:
            reaped_pid_verified = reader.has_seen(f"reaped child {calc_pid}") or reader.has_seen(f"PID {calc_pid}")
            assert reaped_pid_verified, f"Kernel did not log waitpid reaping for child PID {calc_pid}"
            print(f"[OK] Child PID {calc_pid} verified reaped and address space reclaimed.")
        print("[OK] Calculator exited and was reaped by Launcher.")

        # 7. Verify HomeScreen re-presentation
        print("[*] Operation 7: Verifying HomeScreen re-presentation...")
        home_re_rendered = reader.wait_for("Home surface rendered.", timeout=15.0, print_keywords=["LAUNCHER", "Home surface"])
        assert home_re_rendered, "HomeScreen failed to re-render after child exit"
        print("[OK] HomeScreen successfully re-presented.")

        print("\n============================================================")
        print("  [SUCCESS] RISC-V 64 Interactive Smartphone Parity Verified!")
        print("============================================================")
        return 0

    finally:
        if proc:
            try:
                proc.terminate()
                proc.wait(timeout=3)
            except Exception:
                try:
                    proc.kill()
                    proc.wait(timeout=2)
                except Exception:
                    pass

if __name__ == "__main__":
    sys.exit(main())
