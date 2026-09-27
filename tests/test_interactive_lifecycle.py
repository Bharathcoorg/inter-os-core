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

def test_app_lifecycle(reader, monitor_sock, phase_num, app_name, app_id, tap_coords, present_marker, dump_prefix, in_app_interactions=None, exit_gesture="back"):
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
    ppm_app = os.path.abspath(rf"target\{dump_prefix}.ppm").replace("\\", "/")
    _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_app}")
    print(f"[LIFECYCLE TEST] Screendump recorded: {ppm_app}")
    time.sleep(0.5)

    # Perform in-app functional interactions if specified
    if in_app_interactions:
        for action_name, action_coords, action_marker, dump_suffix in in_app_interactions:
            print(f"[LIFECYCLE TEST] In-App Interaction: {action_name} at {action_coords}...")
            reader.sock.sendall(f"TAP {action_coords[0]} {action_coords[1]}\n".encode('utf-8'))
            if action_marker:
                action_ok = reader.wait_for(
                    action_marker,
                    timeout=10.0,
                    print_keywords=[app_name.upper(), "APP", "INPUT", action_marker]
                )
                assert action_ok, f"{app_name} failed to process interaction '{action_name}'"
                print(f"[LIFECYCLE TEST] [OK] Interaction '{action_name}' verified.")
            time.sleep(0.8)
            if dump_suffix:
                ppm_inter = os.path.abspath(rf"target\{dump_prefix}_{dump_suffix}.ppm").replace("\\", "/")
                _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_inter}")
                print(f"[LIFECYCLE TEST] In-app interaction screendump recorded: {ppm_inter}")
                time.sleep(0.4)

    # Dispatch touch tap on Back button, Home Gesture Bar, or Swipe Up
    if exit_gesture == "home_bar":
        print(f"[LIFECYCLE TEST] Dispatching touch tap on Home Gesture Bar (x=360, y=1420)...")
        reader.sock.sendall(b"TAP 360 1420\n")
    elif exit_gesture == "swipe_up":
        print(f"[LIFECYCLE TEST] Dispatching swipe-up gesture from bottom bar (360, 1420) -> (360, 1100)...")
        reader.sock.sendall(b"SWIPE 360 1420 360 1100 5\n")
    else:
        print(f"[LIFECYCLE TEST] Dispatching touch tap on Back button (x=62, y=48)...")
        reader.sock.sendall(b"TAP 62 48\n")

    # Wait for child exit and Launcher reaping
    reaped = reader.wait_for(
        "Foreground application exited and was reaped",
        timeout=15.0,
        print_keywords=[app_name.upper(), "APP", "LAUNCHER", "INPUT", "WAITPID"]
    )
    assert reaped, f"{app_name} exit / Launcher waitpid reaping was not observed"
    print(f"[LIFECYCLE TEST] [OK] {exit_gesture.capitalize()} navigation successfully triggered child exit, surface teardown, and parent waitpid reaping.")

    time.sleep(1.2)
    ppm_reaped = os.path.abspath(rf"target\test_launcher_after_{dump_prefix}.ppm").replace("\\", "/")
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
        ppm_launcher = os.path.abspath(r"target\test_launcher.ppm").replace("\\", "/")
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
            dump_prefix="test_calculator",
            in_app_interactions=[
                ("Tap '7'", (100, 680), "Keypad digit pressed.", None),
                ("Tap '+'", (598, 1020), "Keypad operator selected.", None),
                ("Tap '5'", (266, 850), "Keypad digit pressed.", None),
                ("Tap '=' (evaluate 7 + 5)", (598, 1190), "Evaluation completed.", "evaluated"),
            ]
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
            dump_prefix="test_settings",
            in_app_interactions=[
                ("Tap Power & Battery Tab", (228, 140), "tab switched.", "tab_power"),
                ("Tap Thermals & Sensors Tab", (364, 140), "tab switched.", "tab_thermals"),
            ],
            exit_gesture="home_bar"
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
            dump_prefix="test_terminal",
            in_app_interactions=[
                ("Tap terminal console area", (360, 500), "Normalized touch event received.", "touch"),
            ],
            exit_gesture="swipe_up"
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
            dump_prefix="test_vault",
            in_app_interactions=[
                ("Tap Sector Card 0 (Root Keystore)", (200, 350), "Sector card touched. Round-trip re-verified.", "sector_verified"),
            ],
            exit_gesture="back"
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
            dump_prefix="test_camera",
            in_app_interactions=[
                ("Tap Mode Carousel (Document Vision)", (165, 1175), None, "mode_doc"),
            ],
            exit_gesture="swipe_up"
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
            dump_prefix="test_dialer",
            in_app_interactions=[
                ("Tap '9'", (582, 772), "Keypad digit entered.", "digit_entered"),
            ],
            exit_gesture="back"
        )

        # App 7: Browser (Row 3, Card 0: x=180, y=860)
        test_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            phase_num=8,
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
        test_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            phase_num=9,
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
        test_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            phase_num=10,
            app_name="Calendar",
            app_id="org.interos.calendar",
            tap_coords=(180, 1030),
            present_marker="Calendar canvas presented to scanout buffer.",
            dump_prefix="test_calendar",
            exit_gesture="swipe_up"
        )

        # App 10: Music (Row 4, Card 1: x=485, y=1030)
        test_app_lifecycle(
            reader=reader,
            monitor_sock=monitor_sock,
            phase_num=11,
            app_name="Music",
            app_id="org.interos.music",
            tap_coords=(485, 1030),
            present_marker="Music canvas presented to scanout buffer.",
            dump_prefix="test_music",
            in_app_interactions=[
                ("Tap Play/Pause toggle", (360, 880), None, "playback_toggled"),
            ],
            exit_gesture="home_bar"
        )

        # Phase 12: Overview / Recent Apps Switcher Test
        print("\n[LIFECYCLE TEST] === Phase 12: Testing Overview / Recent Apps Deck ===")
        print("[LIFECYCLE TEST] Dispatching swipe-up gesture from bottom bar on homescreen to open overview...")
        reader.sock.sendall(b"SWIPE 360 1420 360 1100 5\n")
        overview_ready = reader.wait_for(
            "render_overview complete: Recent apps overview deck displayed.",
            timeout=10.0,
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
        reader.sock.sendall(b"TAP 360 340\n")
        switched = reader.wait_for(
            "Switched to recent app from overview card 0",
            timeout=10.0,
            print_keywords=["LAUNCHER", "Switched", "APP"]
        )
        assert switched, "Failed to switch to recent app from overview"
        print("[LIFECYCLE TEST] [OK] Switched to recent app from overview card 0.")
        
        recent_presented = reader.wait_for(
            "Music canvas presented to scanout buffer.",
            timeout=10.0,
            print_keywords=["MUSIC", "APP", "canvas"]
        )
        assert recent_presented, "Recent app failed to present canvas within timeout"
        time.sleep(1.0)

        # Exit child app via swipe up to return to launcher
        print("[LIFECYCLE TEST] Dispatching swipe-up from recent app to return to launcher...")
        reader.sock.sendall(b"SWIPE 360 1420 360 1100 5\n")
        reaped = reader.wait_for(
            "Foreground application exited and was reaped",
            timeout=15.0,
            print_keywords=["LAUNCHER", "WAITPID"]
        )
        assert reaped, "Failed to reap recent app upon swipe up"
        print("[LIFECYCLE TEST] [OK] Child application reaped, launcher returned to home grid.")

        # Phase 13: Overview Card Dismissal Test (Tap dismiss badge & swipe-to-dismiss)
        print("\n[LIFECYCLE TEST] === Phase 13: Testing Overview Card Dismissal ===")
        time.sleep(1.0)
        print("[LIFECYCLE TEST] Dispatching swipe-up from homescreen to open overview...")
        reader.sock.sendall(b"SWIPE 360 1420 360 1100 5\n")
        overview2_ready = reader.wait_for(
            "render_overview complete: Recent apps overview deck displayed.",
            timeout=10.0,
            print_keywords=["LAUNCHER", "overview", "deck"]
        )
        assert overview2_ready, "Launcher failed to re-open overview deck"
        print("[LIFECYCLE TEST] [OK] Overview deck re-opened.")
        time.sleep(0.5)

        # Tap dismiss badge [X] on Card 0 (x=610, y=230)
        print("[LIFECYCLE TEST] Tapping dismiss badge on Card 0 (x=610, y=230)...")
        reader.sock.sendall(b"TAP 610 230\n")
        card0_dismissed = reader.wait_for(
            "Card 0 dismissed via dismiss button tap.",
            timeout=10.0,
            print_keywords=["LAUNCHER", "Card", "dismissed"]
        )
        assert card0_dismissed, "Failed to dismiss Card 0 via dismiss button tap"
        print("[LIFECYCLE TEST] [OK] Card 0 dismissed via dismiss badge tap.")
        time.sleep(0.5)

        # Dispatch horizontal swipe on Card 1 (y=660: x1=200 -> x2=550)
        print("[LIFECYCLE TEST] Dispatching horizontal swipe gesture on Card 1 (200, 660) -> (550, 660)...")
        reader.sock.sendall(b"SWIPE 200 660 550 660 5\n")
        card1_dismissed = reader.wait_for(
            "Card 1 dismissed via swipe.",
            timeout=10.0,
            print_keywords=["LAUNCHER", "Card", "dismissed"]
        )
        assert card1_dismissed, "Failed to dismiss Card 1 via horizontal swipe"
        print("[LIFECYCLE TEST] [OK] Card 1 dismissed via horizontal swipe gesture.")
        time.sleep(1.0)

        # Return home via close button tap
        print("[LIFECYCLE TEST] Returning home from overview deck via close button tap...")
        reader.sock.sendall(b"TAP 360 1240\n")
        returned_home = reader.wait_for(
            "Returned to home grid from recents.",
            timeout=10.0,
            print_keywords=["LAUNCHER", "Returned", "home"]
        )
        assert returned_home, "Failed to return home from overview deck"
        print("[LIFECYCLE TEST] [OK] Successfully returned to home grid after card dismissals.")

        ppm_final = os.path.abspath(r"target\test_launcher_final.ppm").replace("\\", "/")
        _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_final}")
        print(f"[LIFECYCLE TEST] Final screendump recorded: {ppm_final}")

        print("\n============================================================")
        print("  INTEROS CORE 10-APP & OVERVIEW LIFECYCLE: ALL PASSED (100%)")
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
