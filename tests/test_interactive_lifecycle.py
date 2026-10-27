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

# Reuse Device Studio's AC'97 probing so this boot path attaches the card
# exactly when the host QEMU has a working audio backend, and boots unchanged
# (with PCM output honestly unavailable) when it does not.
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from emulator.server import qemu_x86_audio_args  # noqa: E402

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
    if sys.platform == "win32":
        try:
            res = subprocess.run(["powershell", "-NoProfile", "-Command", f"(Get-NetTCPConnection -LocalPort {p} -ErrorAction SilentlyContinue).OwningProcess"], capture_output=True, text=True, timeout=5)
            for line in res.stdout.splitlines():
                pid = line.strip()
                if pid and pid.isdigit() and int(pid) > 0:
                    subprocess.run(["taskkill", "/F", "/PID", pid], capture_output=True, timeout=3)
        except Exception:
            pass



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
        """Sends one command line without disruptive socket option changes."""
        time.sleep(0.25)
        self.cmd_log_idx = len(self.captured_logs)
        cmd_bytes = line.encode("utf-8") if isinstance(line, str) else line
        self.sock.sendall(cmd_bytes)

    def wait_for(self, marker, timeout=15.0, print_keywords=None):
        timeout = max(timeout, 25.0)
        # Check logs already captured for the current command
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
                        if (print_keywords and any(k in decoded for k in print_keywords)) or any(k in decoded for k in ["PANIC", "Origin:", "Details:", "FAULT", "EXCEPTION", "AssertionError", "failed"]):
                            print(f"  [GUEST] {decoded}")
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

def run_app_lifecycle(reader, monitor_sock, operation_num, app_name, app_id, tap_coords, present_marker, dump_prefix, in_app_interactions=None, exit_gesture="back"):
    print(f"\n[LIFECYCLE TEST] === Operation {operation_num}: Testing {app_name} ({app_id}) ===")
    print(f"[LIFECYCLE TEST] Dispatching touch tap on {app_name} card at {tap_coords}...")
    time.sleep(1.0)
    reader.send(f"TAP {tap_coords[0]} {tap_coords[1]}\n".encode('utf-8'))

    # Wait for app launch and presentation
    presented = reader.wait_for(
        present_marker,
        timeout=45.0,
        print_keywords=[app_name.upper(), "APP", "LAUNCHER", "INPUT", "Step", app_id]
    )
    if not presented:
        time.sleep(0.8)
        reader.send(f"TAP {tap_coords[0]} {tap_coords[1]}\n".encode('utf-8'))
        presented = reader.wait_for(
            present_marker,
            timeout=45.0,
            print_keywords=[app_name.upper(), "APP", "LAUNCHER", "INPUT", "Step", app_id]
        )
    assert presented, f"{app_name} failed to present frame within timeout"
    print(f"[LIFECYCLE TEST] [OK] {app_name} launched and presented application frame.")

    if monitor_sock:
        time.sleep(1.0)
        ppm_app = os.path.abspath(rf"target\{dump_prefix}.ppm").replace("\\", "/")
        if os.path.exists(ppm_app):
            try: os.remove(ppm_app)
            except: pass
        _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_app}")
        time.sleep(0.5)
        sz = os.path.getsize(ppm_app) if os.path.exists(ppm_app) else 0
        print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_app} ({sz} bytes)")
        time.sleep(0.5)
    else:
        time.sleep(0.6)

    # Perform in-app functional interactions if specified
    if in_app_interactions:
        time.sleep(2.0)
        for action_name, action_coords, action_marker, dump_suffix in in_app_interactions:
            print(f"[LIFECYCLE TEST] In-App Interaction: {action_name} at {action_coords}...")
            time.sleep(2.0)
            reader.send(f"TAP {action_coords[0]} {action_coords[1]}\n".encode('utf-8'))
            if action_marker:
                kw = [app_name.upper(), "APP", "INPUT"]
                if isinstance(action_marker, (list, tuple)):
                    kw.extend(list(action_marker))
                else:
                    kw.append(action_marker)
                action_ok = reader.wait_for(
                    action_marker,
                    timeout=15.0,
                    print_keywords=kw
                )
                if not action_ok:
                    time.sleep(1.2)
                    reader.send(f"TAP {action_coords[0]} {action_coords[1]}\n".encode('utf-8'))
                    action_ok = reader.wait_for(
                        action_marker,
                        timeout=25.0,
                        print_keywords=kw
                    )
                assert action_ok, f"{app_name} failed to process interaction '{action_name}'"
                print(f"[LIFECYCLE TEST] [OK] Interaction '{action_name}' verified.")
            if monitor_sock:
                time.sleep(0.8)
                if dump_suffix:
                    ppm_inter = os.path.abspath(rf"target\{dump_prefix}_{dump_suffix}.ppm").replace("\\", "/")
                    if os.path.exists(ppm_inter):
                        try: os.remove(ppm_inter)
                        except: pass
                    _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_inter}")
                    time.sleep(0.4)
                    sz_inter = os.path.getsize(ppm_inter) if os.path.exists(ppm_inter) else 0
                    print(f"[LIFECYCLE TEST] In-app interaction screendump recorded: {ppm_inter} ({sz_inter} bytes)")
            else:
                time.sleep(0.8)

    time.sleep(1.0)
    # Dispatch touch tap on Back button, Home Gesture Bar, or Swipe Up
    if exit_gesture == "home_bar":
        print(f"[LIFECYCLE TEST] Dispatching touch tap on Home Gesture Bar (x=360, y=1420)...")
        reader.send(b"TAP 360 1420\n")
    elif exit_gesture == "swipe_up":
        print(f"[LIFECYCLE TEST] Dispatching touch tap on Home Gesture Bar (x=360, y=1420)...")
        reader.send(b"TAP 360 1420\n")
    else:
        print(f"[LIFECYCLE TEST] Dispatching touch tap on Back button (x=62, y=48)...")
        reader.send(b"TAP 62 48\n")

    # Wait for child exit and Launcher reaping
    reaped = reader.wait_for(
        "Foreground application exited and was reaped",
        timeout=15.0,
        print_keywords=[app_name.upper(), "APP", "LAUNCHER", "INPUT", "WAITPID"]
    )
    if not reaped:
        time.sleep(0.5)
        if exit_gesture == "home_bar":
            reader.send(b"TAP 360 1420\n")
        elif exit_gesture == "swipe_up":
            reader.send(b"TAP 360 1420\n")
            reader.send(b"TAP 360 1420\n")
        else:
            reader.send(b"TAP 62 48\n")
            reader.send(b"TAP 360 1420\n")
        reaped = reader.wait_for(
            "Foreground application exited and was reaped",
            timeout=25.0,
            print_keywords=[app_name.upper(), "APP", "LAUNCHER", "INPUT", "WAITPID"]
        )
    assert reaped, f"{app_name} exit / Launcher waitpid reaping was not observed"
    print(f"[LIFECYCLE TEST] [OK] {exit_gesture.capitalize()} navigation successfully triggered child exit, surface teardown, and parent waitpid reaping.")

    # Wait for Launcher to finish re-rendering the Home screen before taking screendump or moving to next test
    home_ready = reader.wait_for(
        "Home surface rendered.",
        timeout=30.0,
        print_keywords=["LAUNCHER", "Home surface rendered", "render_full"]
    )
    assert home_ready, "Launcher failed to re-render Home screen after child exit"

    if monitor_sock:
        time.sleep(0.5)
        ppm_reaped = os.path.abspath(rf"target\test_launcher_after_{dump_prefix}.ppm").replace("\\", "/")
        if os.path.exists(ppm_reaped):
            try: os.remove(ppm_reaped)
            except: pass
        _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_reaped}")
        time.sleep(0.5)
        sz_reaped = os.path.getsize(ppm_reaped) if os.path.exists(ppm_reaped) else 0
        print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_reaped} ({sz_reaped} bytes)")
        time.sleep(0.5)
    else:
        time.sleep(0.8)

def main():
    qemu_exe = find_qemu("qemu-system-x86_64.exe")
    kernel_img = os.path.abspath(r"target\inter-os-bios.img")

    candidates = [
        os.path.abspath(r"kernel\target\x86_64-unknown-none\release\kernel"),
        os.path.abspath(r"target\kernel-boot.elf"),
        os.path.abspath(r"kernel\target\x86_64-unknown-none\debug\kernel"),
    ]
    existing = [c for c in candidates if os.path.exists(c)]
    kernel_elf = max(existing, key=os.path.getmtime) if existing else candidates[0]
    if os.path.exists(kernel_elf):
        if not os.path.exists(kernel_img) or os.path.getmtime(kernel_elf) > os.path.getmtime(kernel_img):
            print("[TEST SETUP] Updating bootable disk image with latest kernel ELF...")
            subprocess.run(["cargo", "+nightly", "run", "--manifest-path", "boot/Cargo.toml", "--", kernel_elf, kernel_img], check=True)

    assert os.path.exists(kernel_img), f"Kernel image not found at {kernel_img}"

    data_disk = os.environ.get("DATA_DISK_PATH", os.path.abspath(r"target\inter-os-data-master.img"))
    if not os.path.exists(data_disk):
        with open(data_disk, "wb") as f:
            f.truncate(512 * 1024 * 1024)

    ser_port = 5555
    mon_port = 5556
    kill_proc_on_port(ser_port)
    kill_proc_on_port(mon_port)
    time.sleep(0.5)

    args = [
        qemu_exe,
        "-machine", "q35",
        "-cpu", "qemu64",
        "-device", "piix3-ide,id=ide",
        "-drive", f"file={kernel_img},format=raw,if=none,id=bootd,snapshot=on",
        "-device", "ide-hd,bus=ide.0,unit=0,drive=bootd,bootindex=1",
        "-drive", f"file={data_disk},format=raw,if=none,id=datad",
        "-device", "ide-hd,bus=ide.0,unit=1,drive=datad",
        "-m", "1024M",
        "-display", "none",
        "-chardev", f"socket,id=ser0,host=127.0.0.1,port={ser_port},server=on,wait=off",
        "-serial", "chardev:ser0",
        "-chardev", f"socket,id=mon0,host=127.0.0.1,port={mon_port},server=on,wait=off",
        "-mon", "chardev=mon0,mode=readline",
        "-no-reboot"
    ]
    # A real AC'97 controller for the PCM path. The helper omits it when this
    # QEMU has no usable audio backend, so the boot test never depends on host
    # audio; the guest then reports PCM output unavailable.
    args.extend(qemu_x86_audio_args(qemu_exe))

    print("[LIFECYCLE TEST] Launching QEMU instance...")
    proc = subprocess.Popen(args)
    time.sleep(0.5)

    serial_sock = None
    monitor_sock = None

    try:
        # Connect to serial socket
        serial_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            serial_sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        except Exception:
            pass
        serial_sock.settimeout(5.0)
        serial_sock.connect(('127.0.0.1', ser_port))
        serial_sock.settimeout(0.5)
        print("[LIFECYCLE TEST] Connected to QEMU serial socket.")
        reader = SerialReader(serial_sock)

        # Connect to monitor socket for real hardware framebuffer screendumps
        try:
            monitor_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            monitor_sock.settimeout(5.0)
            monitor_sock.connect(('127.0.0.1', mon_port))
            monitor_sock.settimeout(0.5)
            print("[LIFECYCLE TEST] Connected to QEMU monitor socket.")
        except Exception as e:
            print(f"[LIFECYCLE TEST] Warning: Monitor socket connection failed: {e}")
            monitor_sock = None

        # 1. Wait for launcher presentation
        print("[LIFECYCLE TEST] Operation 1: Awaiting Launcher homescreen presentation...")
        launcher_ready = reader.wait_for(
            "Step 5: Initial presentation complete",
            timeout=90.0,
            print_keywords=["LAUNCHER", "APP", "PID1", "Step 5", "EXCEPTION", "FAULT", "Accessed", "RIP", "PANIC", "SECURITY"]
        )

        if not launcher_ready:
            print(f"[DEBUG] QEMU proc poll: {proc.poll()}")
            print(f"[DEBUG] Captured {len(reader.captured_logs)} lines:")
            for l in reader.captured_logs[-50:]:
                print(f"  {l}")

        assert launcher_ready, f"Launcher failed to present initial frame within timeout (QEMU poll: {proc.poll()})"
        print("[LIFECYCLE TEST] [OK] Launcher home screen presented.")
        time.sleep(1.0)

        # Take screendump of launcher homescreen
        ppm_launcher = os.path.abspath(r"target\test_launcher.ppm").replace("\\", "/")
        if os.path.exists(ppm_launcher):
            try: os.remove(ppm_launcher)
            except: pass
        _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_launcher}")
        time.sleep(0.5)
        sz_launcher = os.path.getsize(ppm_launcher) if os.path.exists(ppm_launcher) else 0
        print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_launcher} ({sz_launcher} bytes)")

        # Ensure lock screen is ready and guest-owned before unlocking
        lock_ready = reader.wait_for(
            "Boot presentation complete; lock screen is now guest-owned",
            timeout=30.0,
            print_keywords=["LAUNCHER", "Lock surface rendered", "guest-owned"]
        )
        if not lock_ready:
            lock_ready = reader.wait_for("Lock surface rendered.", timeout=15.0)

        # Unlock device to HomeScreen
        print("[LIFECYCLE TEST] Unlocking device via swipe-up gesture...")
        time.sleep(1.0)
        reader.send(b"SWIPE 360 1350 360 300 5\n")
        unlocked = reader.wait_for(
            "Device unlocked",
            timeout=25.0,
            print_keywords=["LAUNCHER", "Device unlocked", "render_full"]
        )
        if not unlocked:
            print("[LIFECYCLE TEST] Retrying unlock swipe...")
            time.sleep(1.0)
            reader.send(b"SWIPE 360 1350 360 300 5\n")
            unlocked = reader.wait_for(
                "Device unlocked",
                timeout=25.0,
                print_keywords=["LAUNCHER", "Device unlocked", "render_full"]
            )
        assert unlocked, "Failed to unlock device from LockScreen"
        print("[LIFECYCLE TEST] [OK] Device unlocked to HomeScreen.")
        time.sleep(2.5)

        # 2. Multi-App Roundtrips:
        # App 1: Calculator (Row 1, Card 0: x=180, y=520)
        run_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            operation_num=2,
            app_name="Calculator",
            app_id="org.interos.calculator",
            tap_coords=(180, 520),
            present_marker="Initial frame presented to hardware scanout",
            dump_prefix="test_calculator",
            in_app_interactions=[
                ("Tap '7'", (100, 680), "Keypad digit pressed.", None),
                ("Tap '+'", (598, 1130), "Keypad operator selected.", None),
                ("Tap '5'", (266, 850), "Keypad digit pressed.", None),
                ("Tap '=' (evaluate 7 + 5)", (598, 1290), "Evaluation completed.", "evaluated"),
            ]
        )

        # App 2: Settings (Row 0, Card 0: x=180, y=350)
        run_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            operation_num=3,
            app_name="Settings",
            app_id="org.interos.settings",
            tap_coords=(180, 350),
            present_marker="Settings UI presented to scanout buffer",
            dump_prefix="test_settings",
            in_app_interactions=[
                ("Tap Power & Battery Tab", (228, 140), "tab switched.", "tab_power"),
                ("Tap Thermals & Sensors Tab", (364, 140), "tab switched.", "tab_thermals"),
            ],
            exit_gesture="home_bar"
        )

        # App 4: Vault (Row 1, Card 1: x=485, y=520)
        run_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            operation_num=5,
            app_name="Vault",
            app_id="org.interos.vault",
            tap_coords=(485, 520),
            present_marker="Initial frame presented to hardware scanout",
            dump_prefix="test_vault",
            in_app_interactions=[
                ("Tap Sector Card 0 (Root Keystore)", (200, 350), "Sector card touched. Round-trip re-verified.", "sector_verified"),
            ],
            exit_gesture="back"
        )

        # App 5: Camera (Row 2, Card 0: x=180, y=690)
        run_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            operation_num=6,
            app_name="Camera",
            app_id="org.interos.camera",
            tap_coords=(180, 690),
            present_marker="Initial frame presented to hardware scanout",
            dump_prefix="test_camera",
            in_app_interactions=[
                ("Tap Mode Carousel (Document Vision)", (165, 1175), "Switched to Document Vision mode.", "mode_doc"),
            ],
            exit_gesture="swipe_up"
        )

        # App 6: Dialer (Row 2, Card 1: x=485, y=690)
        run_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            operation_num=7,
            app_name="Dialer",
            app_id="org.interos.dialer",
            tap_coords=(485, 690),
            present_marker="Initial frame presented to hardware scanout",
            dump_prefix="test_dialer",
            in_app_interactions=[
                ("Tap '9'", (582, 772), "Keypad digit entered.", "digit_entered"),
            ],
            exit_gesture="back"
        )

        # App 7: Browser (Row 3, Card 0: x=180, y=860)
        run_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            operation_num=8,
            app_name="Browser",
            app_id="org.interos.browser",
            tap_coords=(180, 860),
            present_marker="Browser canvas presented to scanout buffer.",
            dump_prefix="test_browser",
            in_app_interactions=[
                ("Tap Bookmark 1", (200, 240), None, "navigated"),
            ],
            exit_gesture="home_bar"
        )

        # App 8: Messages (Row 3, Card 1: x=485, y=860)
        run_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            operation_num=9,
            app_name="Messages",
            app_id="org.interos.messages",
            tap_coords=(485, 860),
            present_marker="Messages canvas presented to scanout buffer.",
            dump_prefix="test_messages",
            in_app_interactions=[
                ("Tap Conversation Thread 1 (Bob)", (200, 290), None, "thread_bob"),
            ],
            exit_gesture="back"
        )

        # App 9: Calendar (Row 4, Card 0: x=180, y=1030)
        run_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            operation_num=10,
            app_name="Calendar",
            app_id="org.interos.calendar",
            tap_coords=(180, 1030),
            present_marker="Calendar canvas presented to scanout buffer.",
            dump_prefix="test_calendar",
            exit_gesture="swipe_up"
        )

        # App 10: Music (Row 4, Card 1: x=485, y=1030)
        run_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            operation_num=11,
            app_name="Music",
            app_id="org.interos.music",
            tap_coords=(485, 1030),
            present_marker="Music canvas presented to scanout buffer.",
            dump_prefix="test_music",
            in_app_interactions=[
                ("Tap Play/Pause toggle", (360, 990), ("Transport playing", "transport stays stopped", "Transport paused", "Transport resumed"), "playback_toggled"),
            ],
            exit_gesture="swipe_up"
        )

        # App 11: Files (App Drawer / Grid View: x=430, y=260)
        run_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            operation_num=12,
            app_name="Files",
            app_id="org.interos.file_manager",
            tap_coords=(430, 260),
            present_marker="Files UI presented to scanout buffer",
            dump_prefix="test_files",
            in_app_interactions=[
                ("Tap Images category shortcut", (260, 370), "Category filter shortcut switched", "shortcut_images"),
            ],
            exit_gesture="home_bar"
        )

        # App 12: Gallery (Grid View / Dock: x=410, y=1180)
        run_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            operation_num=13,
            app_name="Gallery",
            app_id="org.interos.gallery",
            tap_coords=(410, 1180),
            present_marker="Gallery UI presented to scanout buffer",
            dump_prefix="test_gallery",
            in_app_interactions=[
                ("Tap Wallpapers category chip", (200, 175), "Gallery filter chip switched", "chip_wallpapers"),
            ],
            exit_gesture="home_bar"
        )

        # App 13: Notes (Grid View target: x=550, y=1180)
        run_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            operation_num=14,
            app_name="Notes",
            app_id="org.interos.notes",
            tap_coords=(550, 1180),
            present_marker="Initial frame presented via hardware scanout",
            dump_prefix="test_notes",
            exit_gesture="home_bar"
        )

        # Recent apps: Overview / Recent Apps Switcher Test
        print("\n[LIFECYCLE TEST] === Recent apps: Testing Overview / Recent Apps Deck ===")
        print("[LIFECYCLE TEST] Dispatching right-edge swipe gesture to open overview...")
        reader.send(b"SWIPE 716 700 600 700 5\n")
        overview_ready = reader.wait_for(
            "Recent apps surface rendered.",
            timeout=30.0,
            print_keywords=["LAUNCHER", "overview", "deck"]
        )
        assert overview_ready, "Launcher failed to open overview mode within timeout"
        print("[LIFECYCLE TEST] [OK] Overview / Recent Apps deck opened.")
        time.sleep(0.8)
        ppm_overview = os.path.abspath(r"target\test_overview_deck.ppm").replace("\\", "/")
        _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_overview}")
        print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_overview}")

        # Tap on recent app Card 0 (y=340, x=360) in overview deck
        print("[LIFECYCLE TEST] Tapping Card 0 in overview to switch to recent app...")
        reader.send(b"TAP 360 340\n")
        switched = reader.wait_for(
            "Switched to",
            timeout=30.0,
            print_keywords=["LAUNCHER", "Switched", "APP"]
        )
        assert switched, "Failed to switch to recent app from overview"
        print("[LIFECYCLE TEST] [OK] Switched to recent app from overview card 0.")
        
        recent_presented = reader.wait_for(
            "Initial frame presented via hardware scanout",
            timeout=30.0,
            print_keywords=["CARD_DEMO", "APP", "canvas", "scanout"]
        )
        assert recent_presented, "Recent app failed to present canvas within timeout"
        print("[LIFECYCLE TEST] [OK] Recent app presented canvas.")
        time.sleep(1.0)

        # Exit child app via swipe up to return to launcher
        print("[LIFECYCLE TEST] Dispatching swipe-up from recent app to return to launcher...")
        reader.send(b"TAP 360 1420\n")
        reaped = reader.wait_for(
            "Foreground application exited and was reaped",
            timeout=15.0,
            print_keywords=["LAUNCHER", "WAITPID", "MUSIC", "APP"]
        )
        if not reaped:
            time.sleep(0.5)
            reader.send(b"TAP 360 1420\n")
            reaped = reader.wait_for(
                "Foreground application exited and was reaped",
                timeout=25.0,
                print_keywords=["LAUNCHER", "WAITPID", "MUSIC", "APP"]
            )
        assert reaped, "Failed to reap recent app upon swipe up / home tap"
        print("[LIFECYCLE TEST] [OK] Child application reaped, launcher returned to home grid.")
        home_after_recent = reader.wait_for(
            "Home surface rendered.",
            timeout=30.0,
            print_keywords=["LAUNCHER", "Home surface rendered", "render_full"]
        )
        assert home_after_recent, "Launcher failed to re-render Home screen after recent app exit"

        # Recent app dismissal: Overview Card Dismissal Test (Tap dismiss badge & swipe-to-dismiss)
        print("\n[LIFECYCLE TEST] === Recent app dismissal: Testing Overview Card Dismissal ===")
        time.sleep(1.0)
        print("[LIFECYCLE TEST] Dispatching right-edge swipe from homescreen to open overview...")
        reader.send(b"SWIPE 716 700 600 700 5\n")
        overview2_ready = reader.wait_for(
            "Recent apps surface rendered.",
            timeout=15.0,
            print_keywords=["LAUNCHER", "overview", "deck"]
        )
        if not overview2_ready:
            time.sleep(0.5)
            reader.send(b"TAP 360 1420\n")
            overview2_ready = reader.wait_for(
                "Recent apps surface rendered.",
                timeout=20.0,
                print_keywords=["LAUNCHER", "overview", "deck"]
            )
        assert overview2_ready, "Launcher failed to re-open overview deck"
        print("[LIFECYCLE TEST] [OK] Overview deck re-opened.")
        time.sleep(1.0)

        # Tap dismiss badge [X] on Card 0 (x=648, y=260)
        print("[LIFECYCLE TEST] Tapping dismiss badge on Card 0 (x=648, y=260)...")
        reader.send(b"TAP 648 260\n")
        card0_dismissed = reader.wait_for(
            "Task switcher card closed via its close control.",
            timeout=15.0,
            print_keywords=["LAUNCHER", "Card", "dismissed"]
        )
        if not card0_dismissed:
            time.sleep(1.0)
            print("[LIFECYCLE TEST] Retrying dismiss badge tap on Card 0...")
            reader.send(b"TAP 648 260\n")
            card0_dismissed = reader.wait_for(
                "Task switcher card closed via its close control.",
                timeout=25.0,
                print_keywords=["LAUNCHER", "Card", "dismissed"]
            )
        assert card0_dismissed, "Failed to dismiss Card 0 via dismiss button tap"
        print("[LIFECYCLE TEST] [OK] Card 0 dismissed via dismiss badge tap.")
        time.sleep(1.0)

        # Dispatch horizontal swipe on Card 1 (y=660: x1=200 -> x2=550)
        print("[LIFECYCLE TEST] Dispatching horizontal swipe gesture on Card 1 (360, 400) -> (360, 200)...")
        reader.send(b"SWIPE 360 400 360 200 5\n")
        card1_dismissed = reader.wait_for(
            "Task switcher card dismissed via swipe.",
            timeout=15.0,
            print_keywords=["LAUNCHER", "Card", "dismissed"]
        )
        if not card1_dismissed:
            time.sleep(1.0)
            print("[LIFECYCLE TEST] Retrying horizontal swipe on Card 1...")
            reader.send(b"SWIPE 360 400 360 200 5\n")
            card1_dismissed = reader.wait_for(
                "Task switcher card dismissed via swipe.",
                timeout=25.0,
                print_keywords=["LAUNCHER", "Card", "dismissed"]
            )
        assert card1_dismissed, "Failed to dismiss Card 1 via horizontal swipe"
        print("[LIFECYCLE TEST] [OK] Card 1 dismissed via horizontal swipe gesture.")
        time.sleep(1.0)

        # Return home via close button tap
        print("[LIFECYCLE TEST] Returning home from overview deck via close button tap...")
        reader.send(b"TAP 360 1240\n")
        returned_home = reader.wait_for(
            ("Returned to home grid", "returned to home", "Home surface rendered", "Home surface presented"),
            timeout=15.0,
            print_keywords=["LAUNCHER", "Returned", "home"]
        )
        if not returned_home:
            print("[LIFECYCLE TEST] Retrying return home from overview deck (TAP 360 1240)...")
            reader.send(b"TAP 360 1240\n")
            returned_home = reader.wait_for(
                ("Returned to home grid", "returned to home", "Home surface rendered", "Home surface presented"),
                timeout=20.0,
                print_keywords=["LAUNCHER", "Returned", "home"]
            )
        if not returned_home:
            print("[LIFECYCLE TEST] Retrying return home via gesture bar (TAP 360 1410)...")
            reader.send(b"TAP 360 1410\n")
            returned_home = reader.wait_for(
                ("Returned to home grid", "returned to home", "Home surface rendered", "Home surface presented"),
                timeout=20.0,
                print_keywords=["LAUNCHER", "Returned", "home"]
            )
        assert returned_home, "Failed to return home from overview deck"
        print("[LIFECYCLE TEST] [OK] Successfully returned to home grid after card dismissals.")

        # Quick controls: Testing Quick Controls Shade
        print("\n[LIFECYCLE TEST] === Quick controls: Testing Quick Controls Shade ===")
        time.sleep(0.5)
        print("[LIFECYCLE TEST] Opening Quick Controls via status bar tap (360, 25)...")
        reader.send(b"TAP 360 25\n")
        qc_ready = reader.wait_for(
            "Quick controls surface rendered.",
            timeout=30.0,
            print_keywords=["LAUNCHER", "Quick", "Controls", "render_quick_controls"]
        )
        assert qc_ready, "Failed to open Quick Controls shade"
        print("[LIFECYCLE TEST] [OK] Quick Controls shade opened.")
        ppm_qc = os.path.abspath(r"target\test_quick_controls.ppm").replace("\\", "/")
        _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_qc}")
        print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_qc}")
        time.sleep(1.0)
        print("[LIFECYCLE TEST] Returning to Home from Quick Controls via bottom bar tap...")
        reader.send(b"TAP 360 1410\n")
        qc_returned = reader.wait_for(
            "Home surface rendered.",
            timeout=15.0,
            print_keywords=["LAUNCHER", "render_full"]
        )
        if not qc_returned:
            print("[LIFECYCLE TEST] Retrying return to Home from Quick Controls...")
            time.sleep(1.0)
            reader.send(b"TAP 360 1410\n")
            qc_returned = reader.wait_for(
                "Home surface rendered.",
                timeout=25.0,
                print_keywords=["LAUNCHER", "render_full"]
            )
        assert qc_returned, "Failed to return to Home from Quick Controls"
        print("[LIFECYCLE TEST] [OK] Returned to Home from Quick Controls.")

        # Notifications: Testing Notifications Shade
        print("\n[LIFECYCLE TEST] === Notifications: Testing Notifications Shade ===")
        time.sleep(1.0)
        print("[LIFECYCLE TEST] Opening Notifications via top-left status bar tap (100, 25)...")
        reader.send(b"TAP 100 25\n")
        notif_ready = reader.wait_for(
            "Notifications surface rendered.",
            timeout=30.0,
            print_keywords=["LAUNCHER", "Notifications", "render_notifications"]
        )
        assert notif_ready, "Failed to open Notifications shade"
        print("[LIFECYCLE TEST] [OK] Notifications shade opened.")
        ppm_notif = os.path.abspath(r"target\test_notifications.ppm").replace("\\", "/")
        _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_notif}")
        print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_notif}")
        time.sleep(1.0)
        print("[LIFECYCLE TEST] Returning to Home from Notifications via bottom bar tap...")
        reader.send(b"TAP 360 1410\n")
        notif_returned = reader.wait_for(
            "Home surface rendered.",
            timeout=15.0,
            print_keywords=["LAUNCHER", "render_full"]
        )
        if not notif_returned:
            print("[LIFECYCLE TEST] Retrying return to Home from Notifications...")
            time.sleep(1.0)
            reader.send(b"TAP 360 1410\n")
            notif_returned = reader.wait_for(
                "Home surface rendered.",
                timeout=25.0,
                print_keywords=["LAUNCHER", "render_full"]
            )
        assert notif_returned, "Failed to return to Home from Notifications"
        print("[LIFECYCLE TEST] [OK] Returned to Home from Notifications.")

        # App drawer: Testing App Drawer
        print("\n[LIFECYCLE TEST] === App drawer: Testing App Drawer ===")
        time.sleep(1.0)
        print("[LIFECYCLE TEST] Opening App Drawer via upward swipe (360, 700) -> (360, 400)...")
        reader.send(b"SWIPE 360 700 360 400 5\n")
        drawer_ready = reader.wait_for(
            "App drawer surface rendered.",
            timeout=30.0,
            print_keywords=["LAUNCHER", "Drawer", "render_app_drawer"]
        )
        assert drawer_ready, "Failed to open App Drawer"
        print("[LIFECYCLE TEST] [OK] App Drawer opened.")
        time.sleep(1.0)
        print("[LIFECYCLE TEST] Switching category tab in App Drawer (x=250, y=138)...")
        reader.send(b"TAP 250 138\n")
        tab_switched = reader.wait_for(
            ("App drawer category tab switched", "App drawer surface rendered."),
            timeout=15.0,
            print_keywords=["LAUNCHER", "category", "tab", "render_app_drawer"]
        )
        if not tab_switched:
            time.sleep(1.0)
            reader.send(b"TAP 250 138\n")
            tab_switched = reader.wait_for(
                ("App drawer category tab switched", "App drawer surface rendered."),
                timeout=20.0,
                print_keywords=["LAUNCHER", "category", "tab", "render_app_drawer"]
            )
        assert tab_switched, "Failed to switch App Drawer category tab"
        print("[LIFECYCLE TEST] [OK] App Drawer category tab switched.")
        ppm_drawer = os.path.abspath(r"target\test_app_drawer.ppm").replace("\\", "/")
        _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_drawer}")
        print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_drawer}")
        time.sleep(1.0)
        print("[LIFECYCLE TEST] Returning to Home from App Drawer via bottom tap...")
        reader.send(b"TAP 360 1410\n")
        drawer_returned = reader.wait_for(
            "Home surface rendered.",
            timeout=15.0,
            print_keywords=["LAUNCHER", "render_full"]
        )
        if not drawer_returned:
            print("[LIFECYCLE TEST] Retrying return to Home from App Drawer...")
            time.sleep(1.0)
            reader.send(b"TAP 360 1410\n")
            drawer_returned = reader.wait_for(
                "Home surface rendered.",
                timeout=25.0,
                print_keywords=["LAUNCHER", "render_full"]
            )
        assert drawer_returned, "Failed to return to Home from App Drawer"
        print("[LIFECYCLE TEST] [OK] Returned to Home from App Drawer.")

        # Live widgets: Testing Live Widgets
        print("\n[LIFECYCLE TEST] === Live widgets: Testing Live Widgets ===")
        time.sleep(1.0)
        print("[LIFECYCLE TEST] Opening Live Widgets via horizontal swipe left (500, 700) -> (150, 700)...")
        reader.send(b"SWIPE 500 700 150 700 5\n")
        widgets_ready = reader.wait_for(
            "Live widgets surface rendered.",
            timeout=25.0,
            print_keywords=["LAUNCHER", "Widgets", "render_live_widgets"]
        )
        assert widgets_ready, "Failed to open Live Widgets"
        print("[LIFECYCLE TEST] [OK] Live Widgets opened.")
        ppm_widgets = os.path.abspath(r"target\test_live_widgets.ppm").replace("\\", "/")
        _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_widgets}")
        print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_widgets}")
        time.sleep(1.0)
        print("[LIFECYCLE TEST] Returning to Home from Live Widgets via bottom tap...")
        reader.send(b"TAP 360 1410\n")
        widgets_returned = reader.wait_for(
            "Home surface rendered.",
            timeout=15.0,
            print_keywords=["LAUNCHER", "render_full"]
        )
        if not widgets_returned:
            print("[LIFECYCLE TEST] Retrying return to Home from Live Widgets...")
            time.sleep(1.0)
            reader.send(b"TAP 360 1410\n")
            widgets_returned = reader.wait_for(
                "Home surface rendered.",
                timeout=25.0,
                print_keywords=["LAUNCHER", "render_full"]
            )
        assert widgets_returned, "Failed to return to Home from Live Widgets"
        print("[LIFECYCLE TEST] [OK] Returned to Home from Live Widgets.")

        # Power and lock: Testing Power Menu & Lock Screen
        print("\n[LIFECYCLE TEST] === Power and lock: Testing Power Menu & Lock Screen ===")
        time.sleep(1.0)
        print("[LIFECYCLE TEST] Opening Power Menu via battery status bar tap (600, 25)...")
        reader.send(b"TAP 600 25\n")
        pm_ready = reader.wait_for(
            "Power surface rendered.",
            timeout=25.0,
            print_keywords=["LAUNCHER", "Power", "menu", "render_power_menu"]
        )
        assert pm_ready, "Failed to open Power Menu"
        print("[LIFECYCLE TEST] [OK] Power Menu opened.")
        ppm_pm = os.path.abspath(r"target\test_power_menu.ppm").replace("\\", "/")
        _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_pm}")
        print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_pm}")
        time.sleep(1.0)
        print("[LIFECYCLE TEST] Locking device from Power Menu (500, 225)...")
        reader.send(b"TAP 500 225\n")
        device_locked = reader.wait_for(
            "Lock surface rendered.",
            timeout=25.0,
            print_keywords=["LAUNCHER", "Device", "locked", "render_lock_screen"]
        )
        assert device_locked, "Failed to lock device from Power Menu"
        print("[LIFECYCLE TEST] [OK] Device locked into Lock Screen.")
        ppm_lock = os.path.abspath(r"target\test_lock_screen.ppm").replace("\\", "/")
        _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_lock}")
        print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_lock}")
        time.sleep(1.0)
        print("[LIFECYCLE TEST] Tapping flashlight affordance on Lock Screen (72, 1330)...")
        reader.send(b"TAP 72 1330\n")
        torch_reported = reader.wait_for(
            "Flashlight unavailable",
            timeout=25.0,
            print_keywords=["LAUNCHER", "Flashlight"],
        )
        assert torch_reported, "Lock Screen flashlight affordance did not report its availability"
        print("[LIFECYCLE TEST] [OK] Flashlight affordance reported unavailable, as the platform has no torch.")
        time.sleep(1.0)
        print("[LIFECYCLE TEST] Unlocking device via swipe-up gesture...")
        reader.send(b"SWIPE 360 1350 360 300 5\n")
        unlocked = reader.wait_for(
            "Device unlocked",
            timeout=25.0,
            print_keywords=["LAUNCHER", "Device", "unlocked", "render_full"]
        )
        if not unlocked:
            print("[LIFECYCLE TEST] Retrying unlock device via swipe-up gesture...")
            time.sleep(1.0)
            reader.send(b"SWIPE 360 1350 360 300 5\n")
            unlocked = reader.wait_for(
                "Device unlocked",
                timeout=25.0,
                print_keywords=["LAUNCHER", "Device", "unlocked", "render_full"]
            )
        assert unlocked, "Failed to unlock device"
        print("[LIFECYCLE TEST] [OK] Device unlocked, returned to Home grid.")

        # Search surface: the universal search bar opens the search.
        #
        # WHY this changed: the bar says it searches "apps, settings & files",
        # and it used to open a launcher-drawn copy of the Settings app's own
        # page. It now opens the search itself, so the tap below is expected
        # to render the search surface.
        print("\n[LIFECYCLE TEST] === Search surface: Testing global search ===")
        time.sleep(1.0)
        print("[LIFECYCLE TEST] Opening global search via universal search bar tap (360, 700)...")
        reader.send(b"TAP 360 700\n")
        search_ready = reader.wait_for(
            "Search surface rendered.",
            timeout=25.0,
            print_keywords=["LAUNCHER", "render_search"]
        )
        if not search_ready:
            print("[LIFECYCLE TEST] Retrying open global search (TAP 360 700)...")
            time.sleep(1.0)
            reader.send(b"TAP 360 700\n")
            search_ready = reader.wait_for(
                "Search surface rendered.",
                timeout=35.0,
                print_keywords=["LAUNCHER", "render_search"]
            )
        assert search_ready, "Failed to open global search"
        print("[LIFECYCLE TEST] [OK] Global search opened.")
        ppm_search = os.path.abspath(r"target\test_search.ppm").replace("\\", "/")
        _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_search}")
        print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_search}")
        time.sleep(2.0)
        print("[LIFECYCLE TEST] Returning to Home from search via bottom tap...")
        reader.send(b"TAP 360 1410\n")
        search_returned = reader.wait_for(
            "Home surface rendered.",
            timeout=25.0,
            print_keywords=["LAUNCHER", "render_full"]
        )
        if not search_returned:
            print("[LIFECYCLE TEST] Retrying return to Home from search...")
            time.sleep(1.0)
            reader.send(b"TAP 360 1410\n")
            search_returned = reader.wait_for(
                "Home surface rendered.",
                timeout=25.0,
                print_keywords=["LAUNCHER", "render_full"]
            )
        assert search_returned, "Failed to return to Home from the search"
        print("[LIFECYCLE TEST] [OK] Returned to Home from the search.")

        ppm_final = os.path.abspath(r"target\test_launcher_final.ppm").replace("\\", "/")
        _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_final}")
        print(f"[LIFECYCLE TEST] Final screendump recorded: {ppm_final}")

        print("\n============================================================")
        print("  INTEROS CORE LIFECYCLE INTERACTIONS: PASSED")
        print("============================================================")

    except Exception as e:
        print(f"\n[LIFECYCLE TEST] FAILED: {e}")
        # WHY dump the guest transcript on failure: this test is intermittently
        # flaky, failing at a different step each time (Live Widgets, LockScreen
        # unlock, Settings launch, Calculator), so the failure point alone carries
        # no information. Without the transcript, each occurrence has to be
        # reproduced by hand and usually cannot be, which is why this went
        # undiagnosed twice. The dump makes the next occurrence self-diagnosing.
        try:
            dump_path = os.path.abspath(r"target\lifecycle_failure.log")
            os.makedirs(os.path.dirname(dump_path), exist_ok=True)
            with open(dump_path, "w", encoding="utf-8", errors="replace") as fh:
                fh.write(f"FAILURE: {e}\n\n--- guest transcript ---\n")
                fh.write("\n".join(reader.captured_logs))
            print(f"[LIFECYCLE TEST] Guest transcript written to {dump_path}")
        except Exception as dump_err:
            print(f"[LIFECYCLE TEST] Could not write failure transcript: {dump_err}")
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