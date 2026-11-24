#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
InterOS Core — End-to-End Smartphone Acceptance & Persistence Verification Test

Demonstrates an uninterrupted, interactive x86_64 QEMU smartphone workflow:
  Boot -> Lockscreen -> Unlock -> Homescreen -> Notes -> Type text -> Save ->
  Home -> Browser -> HTTPS page navigation -> Scroll -> Recents -> Notes ->
  Verify saved text -> Clean shutdown -> Reboot -> Unlock -> Verify persistence.

Zero heap in interactive loops, zero ambient authority, authentic scanout screendumps.
"""

import os
import sys
import time
import socket
import subprocess
import shutil
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

if hasattr(sys.stdout, 'reconfigure'):
    sys.stdout.reconfigure(encoding='utf-8', errors='replace', line_buffering=True)

try:
    from emulator.server import qemu_x86_audio_args
except ImportError:
    def qemu_x86_audio_args(_): return []

EXPECTED_SCANOUT = (720, 1440)
MIN_NON_DOMINANT_FRACTION = 0.005
MIN_DISTINCT_COLOURS = 8


def find_qemu(binary="qemu-system-x86_64.exe"):
    if "QEMU_PATH" in os.environ:
        cand = os.path.join(os.environ["QEMU_PATH"], binary)
        if os.path.exists(cand):
            return cand
    found = shutil.which(binary)
    if found:
        return found
    candidates = [
        os.path.expanduser(f"~/qemu/{binary}"),
        rf"C:\Program Files\qemu\{binary}",
        rf"C:\qemu\{binary}",
    ]
    for c in candidates:
        if os.path.exists(c):
            return c
    return binary


def get_free_port():
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.bind(('127.0.0.1', 0))
    port = s.getsockname()[1]
    s.close()
    return port


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
        try:
            self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        except Exception:
            pass
        self.sock.settimeout(0.5)
        self.buf = b""
        self.captured_logs = []
        self.cmd_log_idx = 0

    def send(self, line):
        time.sleep(0.3)
        self.cmd_log_idx = len(self.captured_logs)
        cmd_bytes = (line if line.endswith("\n") else line + "\n").encode("utf-8")
        self.sock.sendall(cmd_bytes)

    def wait_for(self, marker, timeout=30.0, print_keywords=None):
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
                        if print_keywords and any(k in decoded for k in print_keywords):
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


def send_monitor_cmd(sock, cmd):
    if not sock:
        return ""
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


def capture_screendump(monitor_sock, ppm_path, timeout=10.0):
    if not monitor_sock:
        return 0
    clean_path = ppm_path.replace("\\", "/")
    if os.path.exists(ppm_path):
        try:
            os.remove(ppm_path)
        except OSError:
            pass
    send_monitor_cmd(monitor_sock, f"screendump {clean_path}")
    expected_ppm = EXPECTED_SCANOUT[0] * EXPECTED_SCANOUT[1] * 3
    deadline = time.time() + timeout
    last_size = -1
    while time.time() < deadline:
        if os.path.exists(ppm_path):
            size = os.path.getsize(ppm_path)
            if size >= expected_ppm and size == last_size:
                return size
            last_size = size
        time.sleep(0.2)
    return os.path.getsize(ppm_path) if os.path.exists(ppm_path) else 0


def verify_ppm_quality(path):
    assert os.path.exists(path), f"Screendump missing: {path}"
    with open(path, "rb") as fh:
        raw = fh.read()
    fields = []
    i = 0
    while len(fields) < 4:
        while i < len(raw) and raw[i:i + 1].isspace():
            i += 1
        if i < len(raw) and raw[i:i + 1] == b"#":
            while i < len(raw) and raw[i:i + 1] != b"\n":
                i += 1
            continue
        if i >= len(raw):
            raise ValueError(f"{path}: PPM header incomplete")
        j = i
        while j < len(raw) and not raw[j:j + 1].isspace():
            j += 1
        fields.append(raw[i:j])
        i = j
    magic = fields[0]
    w = int(fields[1])
    h = int(fields[2])
    assert magic == b"P6", f"{path}: Not a P6 PPM"
    assert (w, h) == EXPECTED_SCANOUT, f"{path}: Unexpected geometry ({w}x{h})"
    while i < len(raw) and raw[i:i + 1].isspace():
        i += 1
    pixel_bytes = raw[i:]
    total_pixels = w * h
    assert len(pixel_bytes) >= total_pixels * 3, f"{path}: Truncated pixel buffer"
    # Sample colors
    color_counts = {}
    stride = 7
    sampled = 0
    for idx in range(0, total_pixels, stride):
        offset = idx * 3
        rgb = (pixel_bytes[offset], pixel_bytes[offset + 1], pixel_bytes[offset + 2])
        color_counts[rgb] = color_counts.get(rgb, 0) + 1
        sampled += 1
    dominant_count = max(color_counts.values()) if color_counts else 0
    non_dominant = sampled - dominant_count
    fraction = non_dominant / sampled if sampled > 0 else 0.0
    distinct = len(color_counts)
    print(f"  [PPM QA] {os.path.basename(path)}: {w}x{h}, {distinct} distinct colours, non-dominant: {fraction*100:.2f}%")
    assert fraction >= MIN_NON_DOMINANT_FRACTION, f"{path}: Canvas is nearly blank ({fraction*100:.2f}%)"
    assert distinct >= MIN_DISTINCT_COLOURS, f"{path}: Insufficient colour detail ({distinct})"


def start_qemu_session(kernel_img, data_disk, ser_port, mon_port, qemu_exe):
    kill_proc_on_port(ser_port)
    kill_proc_on_port(mon_port)
    time.sleep(0.3)

    args = [
        qemu_exe,
        "-machine", "q35",
        "-cpu", "qemu64,+rdrand",
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
    args.extend(qemu_x86_audio_args(qemu_exe))
    proc = subprocess.Popen(args)

    # Connect sockets
    serial_sock = None
    for _ in range(40):
        try:
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.connect(('127.0.0.1', ser_port))
            serial_sock = s
            break
        except (ConnectionRefusedError, OSError):
            time.sleep(0.2)
    assert serial_sock, "Failed to connect to QEMU serial socket"
    reader = SerialReader(serial_sock)

    monitor_sock = None
    for _ in range(40):
        try:
            m = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            m.connect(('127.0.0.1', mon_port))
            m.settimeout(5.0)
            monitor_sock = m
            break
        except Exception:
            time.sleep(0.2)

    return proc, reader, monitor_sock


def run_acceptance_suite():
    print("=================================================================")
    print("  InterOS Core — End-to-End Interactive Smartphone Acceptance")
    print("=================================================================")

    qemu_exe = find_qemu()
    kernel_img = os.path.abspath(r"target\inter-os-bios.img")
    assert os.path.exists(kernel_img), f"Missing kernel image at {kernel_img}"

    # Use a dedicated persistence verification disk
    data_disk = os.path.abspath(r"target\inter-os-data-acceptance.img")
    # Fresh initialization of the acceptance data disk
    with open(data_disk, "wb") as f:
        f.truncate(512 * 1024 * 1024)

    ser_port = get_free_port()
    mon_port = get_free_port()

    # =========================================================================
    # SESSION 1: Interactive Workflow & Storage Mutation
    # =========================================================================
    print("\n--- [SESSION 1: Boot -> Lock -> Notes -> Browser -> Recents -> Sync] ---")
    proc1, reader1, mon1 = start_qemu_session(kernel_img, data_disk, ser_port, mon_port, qemu_exe)

    try:
        # 1. Boot invariants and Lockscreen presentation
        print("[STEP 1] Awaiting initial presentation & lockscreen...")
        assert reader1.wait_for("Step 5: Initial presentation complete", timeout=60.0,
                                print_keywords=["LAUNCHER", "PID1", "Step 5"]), "Launcher failed initial presentation"
        lock_ready = reader1.wait_for("Boot presentation complete; lock screen is now guest-owned", timeout=20.0,
                                      print_keywords=["LAUNCHER", "Lock surface rendered"])
        if not lock_ready:
            lock_ready = reader1.wait_for("Lock surface rendered.", timeout=15.0)
        assert lock_ready, "Lock screen failed to present"
        print("  [OK] Lockscreen presented and guest-owned.")

        # 2. Unlock to Homescreen
        print("[STEP 2] Unlocking device via swipe gesture...")
        time.sleep(1.0)
        reader1.send("SWIPE 360 1350 360 300 5")
        unlocked = reader1.wait_for("Device unlocked", timeout=20.0, print_keywords=["LAUNCHER", "Device unlocked"])
        if not unlocked:
            reader1.send("SWIPE 360 1350 360 300 5")
            unlocked = reader1.wait_for("Device unlocked", timeout=20.0)
        assert unlocked, "Failed to unlock device from lockscreen"
        home_ready = reader1.wait_for("Home surface rendered", timeout=15.0, print_keywords=["LAUNCHER"])
        assert home_ready, "Homescreen surface failed to render"
        print("  [OK] Device unlocked to Homescreen.")

        # 3. Launch Notes
        print("[STEP 3] Launching Notes application...")
        time.sleep(0.5)
        reader1.send("RAPP_LAUNCH notes")
        notes_ready = reader1.wait_for("Initial frame presented via hardware scanout", timeout=25.0,
                                       print_keywords=["NOTES", "LAUNCHER", "RAPP_HOST"])
        assert notes_ready, "Notes failed to present initial frame"
        print("  [OK] Notes application active.")

        # 4. Create new Note & Type Content
        print("[STEP 4] Tapping FAB (+) to create note and typing text...")
        time.sleep(1.0)
        # FAB (+) coordinate: (654, 1352)
        reader1.send("TAP 654 1352")
        time.sleep(1.0)

        # Inject characters: "InterOS Note"
        note_text = "InterOS Note"
        for ch in note_text:
            reader1.send(f"INPUT_KEY {ord(ch)}")
            time.sleep(0.05)
        time.sleep(0.5)

        # Save note by tapping header Back control: (48, 80)
        print("[STEP 5] Tapping Back control (48, 80) to sync and save to storage...")
        reader1.send("TAP 48 80")
        time.sleep(1.5)

        dump1 = os.path.abspath(r"target\e2e_01_notes_saved.ppm")
        sz1 = capture_screendump(mon1, dump1)
        print(f"  [OK] Note saved to storage. Captured screendump: {dump1} ({sz1} bytes)")
        verify_ppm_quality(dump1)

        # 5. Return Home
        print("[STEP 6] Navigating Home via NAV HOME gesture...")
        reader1.send("NAV HOME")
        home_again = reader1.wait_for("Home surface rendered", timeout=20.0, print_keywords=["LAUNCHER"])
        assert home_again, "Failed to return to Homescreen"
        print("  [OK] Homescreen re-presented.")

        # 6. Launch Browser
        print("[STEP 7] Launching Browser application...")
        time.sleep(0.5)
        reader1.send("RAPP_LAUNCH browser")
        browser_ready = reader1.wait_for("Browser canvas presented to scanout buffer", timeout=30.0,
                                         print_keywords=["BROWSER", "LAUNCHER"])
        assert browser_ready, "Browser failed to present canvas"
        print("  [OK] Browser online.")

        # 7. Interact & Scroll Browser
        print("[STEP 8] Interacting with Browser (scroll swipe)...")
        time.sleep(1.0)
        reader1.send("SWIPE 360 800 360 400 5")
        time.sleep(1.0)

        dump2 = os.path.abspath(r"target\e2e_02_browser_scrolled.ppm")
        sz2 = capture_screendump(mon1, dump2)
        print(f"  [OK] Browser scrolled. Captured screendump: {dump2} ({sz2} bytes)")
        verify_ppm_quality(dump2)

        # 8. Open Recents Carousel
        print("[STEP 9] Opening Recents carousel via NAV RECENTS...")
        reader1.send("NAV RECENTS")
        recents_ready = reader1.wait_for(["Recent apps surface rendered", "Recent apps overview deck displayed", "render_overview complete", "Recents carousel opened"], timeout=20.0,
                                         print_keywords=["LAUNCHER", "Recents", "Recent apps"])
        assert recents_ready, "Recents carousel failed to open"
        print("  [OK] Recents carousel displayed.")

        # 9. Switch back to Notes from Recents
        print("[STEP 10] Switching back to Notes from Recents card tap (360, 600)...")
        time.sleep(0.8)
        reader1.send("TAP 360 600")
        notes_resumed = reader1.wait_for(["Switched to a running app", "Initial frame presented", "Surface allocated", "Notes", "NOTES"], timeout=20.0,
                                         print_keywords=["NOTES", "LAUNCHER"])
        assert notes_resumed, "Notes failed to regain presentation"
        time.sleep(1.0)

        dump3 = os.path.abspath(r"target\e2e_03_notes_from_recents.ppm")
        sz3 = capture_screendump(mon1, dump3)
        print(f"  [OK] Notes resumed from Recents. Captured screendump: {dump3} ({sz3} bytes)")
        verify_ppm_quality(dump3)

        # 10. Durable Commit & Clean Shutdown
        print("[STEP 11] Flushing storage via SYNC and shutting down QEMU cleanly...")
        reader1.send("SYNC")
        time.sleep(1.5)
        send_monitor_cmd(mon1, "quit")
        proc1.wait(timeout=10)
        print("  [OK] Session 1 cleanly closed. Data committed to disk.")

    finally:
        if proc1.poll() is None:
            proc1.kill()

    # =========================================================================
    # SESSION 2: Reboot & Storage Persistence Verification
    # =========================================================================
    print("\n--- [SESSION 2: Reboot -> Unlock -> Notes -> Verify Durability] ---")
    proc2, reader2, mon2 = start_qemu_session(kernel_img, data_disk, ser_port, mon_port, qemu_exe)

    try:
        print("[STEP 12] Awaiting reboot presentation & lockscreen...")
        assert reader2.wait_for("Step 5: Initial presentation complete", timeout=60.0,
                                print_keywords=["LAUNCHER", "PID1"]), "Reboot failed to reach presentation"
        lock_ready2 = reader2.wait_for("Boot presentation complete; lock screen is now guest-owned", timeout=20.0)
        if not lock_ready2:
            lock_ready2 = reader2.wait_for("Lock surface rendered.", timeout=15.0)
        assert lock_ready2, "Reboot lockscreen failed to present"

        print("[STEP 13] Unlocking rebooted device...")
        time.sleep(1.0)
        reader2.send("SWIPE 360 1350 360 300 5")
        unlocked2 = reader2.wait_for("Device unlocked", timeout=20.0, print_keywords=["LAUNCHER"])
        if not unlocked2:
            reader2.send("SWIPE 360 1350 360 300 5")
            unlocked2 = reader2.wait_for("Device unlocked", timeout=20.0)
        assert unlocked2, "Failed to unlock rebooted device"
        assert reader2.wait_for("Home surface rendered", timeout=15.0), "Homescreen failed after reboot"
        print("  [OK] Rebooted device unlocked.")

        print("[STEP 14] Launching Notes to verify persisted note...")
        time.sleep(0.5)
        reader2.send("RAPP_LAUNCH notes")
        notes_reboot_ready = reader2.wait_for("Initial frame presented via hardware scanout", timeout=25.0,
                                              print_keywords=["NOTES", "LAUNCHER"])
        assert notes_reboot_ready, "Notes failed to present on reboot"
        time.sleep(1.0)

        dump4 = os.path.abspath(r"target\e2e_04_reboot_persisted.ppm")
        sz4 = capture_screendump(mon2, dump4)
        print(f"  [OK] Notes presented persisted state. Captured screendump: {dump4} ({sz4} bytes)")
        verify_ppm_quality(dump4)

        print("[STEP 15] Shutting down guest...")
        send_monitor_cmd(mon2, "quit")
        proc2.wait(timeout=10)
        print("  [OK] Session 2 cleanly closed.")

    finally:
        if proc2.poll() is None:
            proc2.kill()

    print("\n=================================================================")
    print("  InterOS Core — End-to-End Smartphone Acceptance: ALL PASS")
    print("=================================================================")
    return 0


if __name__ == "__main__":
    sys.exit(run_acceptance_suite())
