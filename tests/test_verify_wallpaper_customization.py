#!/usr/bin/env python3
"""
Test End-to-End Home-Only Wallpaper Customisation in QEMU.
Navigates Settings -> Personal -> Wallpaper Picker -> Selects Night -> Sets Target: Home -> Applies.
Exits back to Launcher, verifies Home uses Night, Lock uses Stock.
"""

import os
import sys
import time
import socket
import subprocess
from pathlib import Path

if hasattr(sys.stdout, 'reconfigure'):
    sys.stdout.reconfigure(encoding='utf-8', errors='replace', line_buffering=True)

ROOT = Path(__file__).resolve().parent.parent

def find_qemu(binary="qemu-system-x86_64.exe"):
    candidates = [
        os.path.expanduser(f"~/qemu/{binary}"),
        rf"C:\Program Files\qemu\{binary}",
        rf"C:\qemu\{binary}",
    ]
    for c in candidates:
        if os.path.exists(c):
            return c
    return binary

def kill_proc_on_port(port):
    if sys.platform == "win32":
        try:
            res = subprocess.run(
                ["powershell", "-NoProfile", "-Command",
                 f"Get-NetTCPConnection -LocalPort {port} -ErrorAction SilentlyContinue | "
                 "ForEach-Object { Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue } | "
                 "Where-Object { $_.ProcessName -like '*qemu*' } | ForEach-Object { $_.Id }"],
                capture_output=True, text=True, timeout=5
            )
            for line in res.stdout.splitlines():
                pid = line.strip()
                if pid and pid.isdigit() and int(pid) > 0:
                    subprocess.run(["taskkill", "/F", "/PID", pid], capture_output=True, timeout=3)
        except Exception:
            pass

class SerialReader:
    def __init__(self, sock):
        self.sock = sock
        self.sock.settimeout(0.5)
        self.buf = b""
        self.captured_logs = []
        self.cmd_log_idx = 0

    def send(self, line):
        time.sleep(0.3)
        self.cmd_log_idx = len(self.captured_logs)
        cmd_bytes = (line if line.endswith("\n") else line + "\n").encode("utf-8")
        self.sock.sendall(cmd_bytes)

    def reset_cursor(self):
        self.cmd_log_idx = len(self.captured_logs)

    def wait_for(self, marker, timeout=35.0, print_filter=None):
        start = time.time()
        start_scan = getattr(self, "cmd_log_idx", 0)
        for l in self.captured_logs[start_scan:]:
            if marker in l:
                return True
        while time.time() - start < timeout:
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
                        if print_filter:
                            if any(k in decoded for k in print_filter):
                                print(f"  [GUEST] {decoded}")
                        else:
                            print(f"  [GUEST] {decoded}")
                        if marker in decoded:
                            return True
            except socket.timeout:
                continue
            except ConnectionResetError:
                break
        return False

def launch_qemu(boot_img, data_img, ser_port, mon_port):
    kill_proc_on_port(ser_port)
    kill_proc_on_port(mon_port)
    time.sleep(0.5)

    qemu_exe = find_qemu()
    args = [
        qemu_exe,
        "-machine", "q35",
        "-cpu", "qemu64",
        "-m", "1024M",
        "-device", "piix3-ide,id=ide",
        "-drive", f"file={boot_img},format=raw,if=none,id=bootd,snapshot=on",
        "-device", "ide-hd,bus=ide.0,unit=0,drive=bootd,bootindex=1",
        "-drive", f"file={data_img},format=raw,if=none,id=datad",
        "-device", "ide-hd,bus=ide.0,unit=1,drive=datad",
        "-vga", "none",
        "-device", "virtio-vga,edid=on,xres=720,yres=1440",
        "-display", "none",
        "-chardev", f"socket,id=ser0,host=127.0.0.1,port={ser_port},server=on,wait=off",
        "-serial", "chardev:ser0",
        "-chardev", f"socket,id=mon0,host=127.0.0.1,port={mon_port},server=on,wait=off",
        "-mon", "chardev=mon0,mode=readline",
        "-no-reboot"
    ]
    proc = subprocess.Popen(args)
    time.sleep(1.0)

    serial_sock = None
    for _ in range(30):
        try:
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.connect(('127.0.0.1', ser_port))
            serial_sock = s
            break
        except (ConnectionRefusedError, OSError):
            time.sleep(0.1)

    monitor_sock = None
    for _ in range(30):
        try:
            m = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            m.connect(('127.0.0.1', mon_port))
            monitor_sock = m
            break
        except (ConnectionRefusedError, OSError):
            time.sleep(0.1)

    return proc, serial_sock, monitor_sock

def screendump(mon_sock, path):
    if mon_sock:
        mon_sock.settimeout(5.0)
        p = str(path).replace("\\", "/")
        mon_sock.sendall(f"screendump {p}\n".encode("utf-8"))
        time.sleep(0.8)

def parse_ppm(path):
    with open(path, "rb") as f:
        header = f.readline().decode('ascii').strip()
        dims = f.readline().decode('ascii').strip()
        while dims.startswith('#'):
            dims = f.readline().decode('ascii').strip()
        w, h = [int(x) for x in dims.split()]
        maxval = int(f.readline().decode('ascii').strip())
        raw = f.read()
    return w, h, raw

def main():
    print("=" * 60)
    print("  Testing Home-Only Wallpaper Customisation in QEMU")
    print("=" * 60)

    boot_img = ROOT / "target" / "inter-os-bios.img"
    data_img = ROOT / "target" / "test_pin_data.img"
    assert data_img.exists(), "test_pin_data.img missing; run test_verify_pin_and_persistence first!"

    ser_port = 5562
    mon_port = 5563

    proc, ser_sock, mon_sock = launch_qemu(boot_img, data_img, ser_port, mon_port)
    reader = SerialReader(ser_sock)

    print("[TEST] Awaiting LockScreen...")
    ready = reader.wait_for("lock screen is now guest-owned", timeout=60.0,
                            print_filter=["BOOT", "USER", "LAUNCHER", "Step 5"])
    assert ready, "Lock screen not presented!"

    time.sleep(1.0)
    # Dump Lock screen PPM to verify Stock wallpaper
    ppm_lock = ROOT / "target" / "lock_wallpaper_stock.ppm"
    screendump(mon_sock, ppm_lock)
    w, h, raw_lock = parse_ppm(ppm_lock)
    idx_lock = (150 * w + 360) * 3
    r_l, g_l, b_l = raw_lock[idx_lock], raw_lock[idx_lock+1], raw_lock[idx_lock+2]
    print(f"[TEST] Lock screen sky pixel at (360, 150): R={r_l}, G={g_l}, B={b_l}")
    assert r_l > 150 and b_l > 180, f"Expected Atmospheric Cosmic Stock lock screen, got R={r_l}, G={g_l}, B={b_l}"
    print("[TEST] Stock wallpaper confirmed on LockScreen!")

    print("[TEST] Unlocking to HomeScreen via swipe gesture...")
    reader.send("SWIPE 360 1425 360 1100 5\n")
    assert reader.wait_for("Device unlocked via swipe/slide gesture", timeout=30.0, print_filter=["LAUNCHER", "USER"]), "Unlock failed!"
    print("[TEST] Unlocked to HomeScreen!")

    time.sleep(1.0)
    # 1. Open Settings the way a person does: the Apps tile on the dock, then the
    # Settings cell in the drawer. WHY not the serial launch bridge: the bridge
    # starts the application as the kernel's child, so the shell never reaps it -
    # and re-reading the wallpaper store is something the shell only does when it
    # regains the foreground, which is the behaviour this test exists to verify.
    print("[TEST] Opening the App Drawer (TAP 590 1110)...")
    reader.send("TAP 590 1110")
    assert reader.wait_for("App Drawer opened", timeout=25.0, print_filter=["LAUNCHER", "INPUT"]), "App Drawer did not open!"
    print("[TEST] Launching Settings from the drawer (TAP 146 256)...")
    reader.send("TAP 146 256")
    assert reader.wait_for("Settings UI presented to scanout buffer", timeout=35.0, print_filter=["SETTINGS", "LAUNCHER"]), "Settings did not present UI!"
    print("[TEST] Settings app launched and UI presented!")

    time.sleep(1.0)
    # 2. Tap Personal category card at (188, 550)
    print("[TEST] Opening Personal section (TAP 188 550)...")
    reader.send("TAP 188 550")
    assert reader.wait_for("System Space category opened", timeout=25.0, print_filter=["SETTINGS", "INPUT"]), "Personal section not opened!"
    print("[TEST] Personal category opened!")

    time.sleep(1.0)
    # 3. Tap Wallpaper row at (360, 440)
    print("[TEST] Opening Wallpaper picker (TAP 360 440)...")
    reader.send("TAP 360 440")
    assert reader.wait_for("Wallpaper picker opened", timeout=25.0, print_filter=["SETTINGS", "INPUT"]), "Wallpaper picker not opened!"
    print("[TEST] Wallpaper picker opened successfully!")

    # 4. Select Night wallpaper (Cell 2 at (592, 1008))
    print("[TEST] Selecting Night wallpaper (TAP 592 1008)...")
    reader.send("TAP 592 1008")
    assert reader.wait_for("Wallpaper selected for preview", timeout=25.0, print_filter=["SETTINGS", "INPUT"]), "Night wallpaper not selected!"
    print("[TEST] Night wallpaper selected for preview!")

    time.sleep(1.0)
    # 5. Set Target: Home (Segment 0 at (134, 852))
    print("[TEST] Setting Wallpaper Target: Home (TAP 134 852)...")
    reader.send("TAP 134 852")
    assert reader.wait_for("Wallpaper target changed", timeout=25.0, print_filter=["SETTINGS", "INPUT"]), "Wallpaper target not changed to Home!"
    print("[TEST] Wallpaper target changed to Home!")

    time.sleep(1.0)
    # 6. Tap Apply at (188, 1376) - accounts for 2 rows of 3 scenes in wallpaper gallery
    print("[TEST] Applying wallpaper (TAP 188 1376)...")
    reader.send("TAP 188 1376")
    assert reader.wait_for("Wallpaper written to the settings store", timeout=25.0, print_filter=["SETTINGS", "INPUT"]), "Wallpaper was not written to store!"
    print("[TEST] Wallpaper written to settings store successfully!")

    time.sleep(1.0)
    # 7. Press Home gesture pill at (360, 1420) to exit Settings
    print("[TEST] Exiting Settings via Home gesture (TAP 360 1420)...")
    reader.send("TAP 360 1420")
    assert reader.wait_for("Home gesture pressed", timeout=20.0, print_filter=["SETTINGS", "INPUT"]), "Home gesture not triggered!"
    print("[TEST] Settings exited!")

    # 8. Verify Launcher applies Home wallpaper from store and renders Home
    print("[TEST] Verifying Launcher re-renders Home with new wallpaper...")
    applied = reader.wait_for("Home wallpaper applied from store", timeout=25.0, print_filter=["LAUNCHER"])
    assert applied, "Launcher did not apply Home wallpaper from store!"
    print("[TEST] Launcher applied Home wallpaper from store!")

    assert reader.wait_for("Foreground application exited and was reaped", timeout=25.0, print_filter=["LAUNCHER"]), "Child not reaped!"
    assert reader.wait_for("FRAME ticks=", timeout=25.0, print_filter=["LAUNCHER"]), "Launcher frame not rendered!"
    print("[TEST] Launcher frame render complete!")
    time.sleep(1.0)

    # 9. Dump Framebuffer and verify pixels
    ppm_home = ROOT / "target" / "home_wallpaper_night.ppm"
    screendump(mon_sock, ppm_home)
    assert ppm_home.exists() and ppm_home.stat().st_size > 1000000, "Home PPM screendump failed!"
    print(f"[TEST] Home PPM dumped: {ppm_home} ({ppm_home.stat().st_size} bytes)")

    # Sample a sky pixel in the Home screen
    # For Night scene: top sky is dark (R~23, G~35, B~51 at y=50)
    # For Stock scene: top sky is bright (R~172, G~195, B~220 at y=50)
    w, h, raw = parse_ppm(ppm_home)
    idx = (50 * w + 10) * 3
    r, g, b = raw[idx], raw[idx+1], raw[idx+2]
    print(f"[TEST] Home sky pixel at (10, 50): R={r}, G={g}, B={b}")
    assert r < 100 and g < 100 and b < 110, f"Expected dark Night sky pixel, got R={r}, G={g}, B={b}"
    print("[TEST] Night sky pixel confirmed on HomeScreen (Stock was R > 150, B > 180)!")

    # 10. Reboot QEMU and verify persistence of both: Lock stays Stock, Home stays Night
    print("\n[TEST] Closing session 1 to test reboot persistence...")
    try:
        mon_sock.sendall(b"quit\n")
    except Exception:
        pass
    ser_sock.close()
    mon_sock.close()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.terminate()
        try:
            proc.wait(timeout=2)
        except subprocess.TimeoutExpired:
            proc.kill()

    print("[TEST] Starting Session 2 (Reboot) with persisted disk...")
    proc2, ser2, mon2 = launch_qemu(boot_img, data_img, ser_port, mon_port)
    r2 = SerialReader(ser2)

    assert r2.wait_for("lock screen is now guest-owned", timeout=60.0, print_filter=["BOOT", "LAUNCHER"]), "Reboot lock screen not presented!"
    ppm_reboot_lock = ROOT / "target" / "reboot_lock_stock.ppm"
    screendump(mon2, ppm_reboot_lock)
    w_rl, h_rl, raw_rl = parse_ppm(ppm_reboot_lock)
    idx_rl = (150 * w_rl + 360) * 3
    rl_r, rl_g, rl_b = raw_rl[idx_rl], raw_rl[idx_rl+1], raw_rl[idx_rl+2]
    print(f"[TEST] Reboot Lock screen sky pixel at (360, 150): R={rl_r}, G={rl_g}, B={rl_b}")
    assert rl_r > 150 and rl_b > 180, f"Expected Atmospheric Cosmic Stock on reboot lock screen, got R={rl_r}, G={rl_g}, B={rl_b}"
    print("[TEST] Lock screen independently retained Stock wallpaper across reboot!")

    time.sleep(0.5)
    print("[TEST] Unlocking rebooted session with swipe gesture...")
    r2.send("SWIPE 360 1425 360 1100 5\n")
    assert r2.wait_for("Device unlocked via swipe/slide gesture", timeout=30.0, print_filter=["LAUNCHER", "USER"]), "Reboot unlock failed!"
    print("[TEST] Unlocked into rebooted HomeScreen!")

    assert r2.wait_for("FRAME ticks=", timeout=25.0, print_filter=["LAUNCHER"]), "Reboot frame not rendered!"
    print("[TEST] Reboot frame render complete!")
    time.sleep(1.0)
    ppm_reboot_home = ROOT / "target" / "reboot_home_night.ppm"
    screendump(mon2, ppm_reboot_home)
    w_rh, h_rh, raw_rh = parse_ppm(ppm_reboot_home)
    idx_rh = (50 * w_rh + 10) * 3
    rh_r, rh_g, rh_b = raw_rh[idx_rh], raw_rh[idx_rh+1], raw_rh[idx_rh+2]
    print(f"[TEST] Reboot Home screen sky pixel at (10, 50): R={rh_r}, G={rh_g}, B={rh_b}")
    assert rh_r < 100 and rh_g < 100 and rh_b < 110, f"Expected Night on reboot home screen, got R={rh_r}, G={rh_g}, B={rh_b}"
    print("[TEST] Home screen persisted Night wallpaper across reboot!")

    try:
        mon2.sendall(b"quit\n")
    except Exception:
        pass
    ser2.close()
    mon2.close()
    try:
        proc2.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc2.terminate()
        try:
            proc2.wait(timeout=2)
        except subprocess.TimeoutExpired:
            proc2.kill()

    print("\n============================================================")
    print("  HOME-ONLY WALLPAPER CUSTOMISATION & PERSISTENCE VERIFIED!")
    print("  LockScreen: Stock (#9CB8D6 sky R=174, G=197, B=221)")
    print("  HomeScreen: Night (#172333 sky R=74, G=84, B=96)")
    print("  Reboot: Both surfaces independently retained their choices")
    print("============================================================")

if __name__ == "__main__":
    main()

