#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
InterOS Core — Native 1080x2400 Scanout & Dynamic Framebuffer Verification
========================================================================
Validates the dynamic framebuffer mapping and physical scanout modeset:
1. Boots QEMU with 32 MiB VRAM (`vgamem_mb=32`).
2. Verifies initial 720x1440 portrait scanout with authentic screendump.
3. Dynamically reprograms scanout to native 1080x2400 via COM1 serial command.
4. Verifies physical DISPI reprogramming, DRM_KMS scanout resizing, and page-table extension.
5. Takes QEMU monitor screendump and proves native 1080x2400 geometry (7,776,000 pixel bytes).
6. Tests oversized mode rejection safety gate (no out-of-bounds mapping).
7. Executes clean ACPI shutdown and generates validation report.
"""

import collections
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


def kill_proc_on_port(p):
    try:
        import psutil
        for conn in psutil.net_connections():
            if conn.laddr and conn.laddr.port == p and conn.pid:
                try:
                    proc = psutil.Process(conn.pid)
                    if "qemu" in proc.name().lower():
                        proc.kill()
                except Exception:
                    pass
    except Exception:
        pass


def find_qemu(binary="qemu-system-x86_64"):
    """Locate the QEMU binary on any host OS.

    POSIX uses `qemu-system-x86_64`, Windows `qemu-system-x86_64.exe`; both
    are honoured via QEMU_PATH, PATH or common install locations.
    """
    if sys.platform == "win32":
        binary = "qemu-system-x86_64.exe"
    if "QEMU_PATH" in os.environ and os.path.exists(os.path.join(os.environ["QEMU_PATH"], binary)):
        return os.path.join(os.environ["QEMU_PATH"], binary)
    found = shutil.which(binary)
    if found:
        return found
    candidates = [
        os.path.expanduser(f"~/qemu/{binary}"),
        os.path.expanduser(f"~/.local/bin/{binary}"),
        os.path.expanduser(f"/usr/local/bin/{binary}"),
        rf"C:\Program Files\qemu\{binary}",
        rf"C:\qemu\{binary}"
    ]
    for c in candidates:
        if os.path.exists(c):
            return c
    return binary


class SerialMonitorClient:
    def __init__(self, ser_sock, mon_sock):
        self.ser_sock = ser_sock
        self.mon_sock = mon_sock
        self.buf = b""
        self.captured_logs = []

    def wait_for(self, marker, timeout=25.0):
        start_t = time.time()
        while time.time() - start_t < timeout:
            try:
                data = self.ser_sock.recv(4096)
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

    def send_cmd(self, cmd_str):
        if not cmd_str.endswith("\n"):
            cmd_str += "\n"
        self.ser_sock.sendall(cmd_str.encode('utf-8'))

    def screendump(self, dest_path, min_size=1000, timeout=12.0):
        # Format path using forward slashes for QEMU monitor
        norm_path = str(dest_path).replace("\\", "/")
        if os.path.exists(dest_path):
            try:
                os.remove(dest_path)
            except OSError:
                pass
        self.mon_sock.sendall(f"screendump {norm_path}\n".encode('utf-8'))
        start_t = time.time()
        last_sz = 0
        while time.time() - start_t < timeout:
            if os.path.exists(dest_path):
                sz = os.path.getsize(dest_path)
                if sz >= min_size and sz == last_sz:
                    time.sleep(0.1)
                    return sz
                last_sz = sz
            time.sleep(0.3)
        return last_sz


def read_ppm(path):
    """Parse binary P6 PPM. Returns (width, height, pixel_bytes)."""
    with open(path, "rb") as fh:
        raw = fh.read()

    fields = []
    i = 0
    while len(fields) < 4:
        while i < len(raw) and raw[i:i + 1].isspace():
            i += 1
        if raw[i:i + 1] == b"#":
            while i < len(raw) and raw[i:i + 1] != b"\n":
                i += 1
            continue
        if i >= len(raw):
            raise ValueError("PPM header ended early")
        j = i
        while j < len(raw) and not raw[j:j + 1].isspace():
            j += 1
        fields.append(raw[i:j])
        i = j
    i += 1  # single whitespace byte after maxval

    magic, width, height, maxval = fields[0], int(fields[1]), int(fields[2]), int(fields[3])
    if magic != b"P6":
        raise ValueError(f"expected binary P6 PPM, got {magic}")
    if maxval != 255:
        raise ValueError(f"expected maxval 255, got {maxval}")
    return width, height, raw[i:]


def analyze_ppm(ppm_path):
    width, height, pixels = read_ppm(ppm_path)
    expected_bytes = width * height * 3
    if len(pixels) < expected_bytes:
        raise ValueError(f"truncated: got {len(pixels)} bytes, expected {expected_bytes}")

    step = 7 * 3
    sample = pixels[:expected_bytes:step]
    red, green, blue = sample[0::3], sample[1::3], sample[2::3]
    counts = collections.Counter(zip(red, green, blue))
    dominant_color, dominant_count = counts.most_common(1)[0]
    distinct_colors = len(counts)
    total_sampled = len(sample) // 3
    dominant_fraction = dominant_count / float(total_sampled)

    return {
        "width": width,
        "height": height,
        "total_bytes": len(pixels),
        "expected_bytes": expected_bytes,
        "distinct_colors": distinct_colors,
        "dominant_color": dominant_color,
        "dominant_fraction": round(dominant_fraction, 4),
        "non_dominant_fraction": round(1.0 - dominant_fraction, 4)
    }


def main():
    qemu_exe = find_qemu()
    kernel_img = ROOT / "target" / "inter-os-bios.img"
    assert kernel_img.exists(), f"Kernel image not found: {kernel_img}"

    ser_port = 5590
    mon_port = 5591
    kill_proc_on_port(ser_port)
    kill_proc_on_port(mon_port)
    time.sleep(0.5)

    proc = None
    ser_sock = None
    mon_sock = None

    try:
        # Launch QEMU with 32 MiB VRAM for 1080x2400x32bpp support
        qemu_cmd = [
            qemu_exe,
            "-machine", "q35",
            "-cpu", "qemu64,+rdrand",
            "-drive", f"format=raw,file={kernel_img}",
            "-snapshot",
            "-m", "1024M",
            "-vga", "std",
            "-global", "VGA.vgamem_mb=32",
            "-display", "none",
            "-chardev", f"socket,id=ser0,host=127.0.0.1,port={ser_port},server=on,wait=off",
            "-serial", "chardev:ser0",
            "-chardev", f"socket,id=mon0,host=127.0.0.1,port={mon_port},server=on,wait=off",
            "-mon", "chardev=mon0,mode=readline",
            "-no-reboot"
        ]

        print("[TEST] Launching QEMU with 32 MiB VRAM...")
        proc = subprocess.Popen(qemu_cmd)
        time.sleep(0.5)

        ser_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        ser_sock.settimeout(6.0)
        ser_sock.connect(('127.0.0.1', ser_port))
        ser_sock.settimeout(0.5)

        mon_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        mon_sock.settimeout(6.0)
        mon_sock.connect(('127.0.0.1', mon_port))
        mon_sock.settimeout(0.5)

        client = SerialMonitorClient(ser_sock, mon_sock)

        print("[TEST] Awaiting initial presentation & lock screen...")
        ready = client.wait_for("Step 5: Initial presentation complete", timeout=120.0)
        assert ready, "Failed to reach initial presentation complete within timeout"
        print("  [OK] Initial presentation complete.")

        # Unlock screen
        time.sleep(1.0)
        client.send_cmd("SWIPE 360 1350 360 300 5")
        time.sleep(1.0)

        # ---------------------------------------------------------------------
        # Phase 1: Verify Initial 720x1440 Portrait Baseline Scanout
        # ---------------------------------------------------------------------
        print("\n[PHASE 1] Capturing baseline 720x1440 screendump...")
        ppm_720 = ROOT / "target" / "scanout_baseline_720x1440.ppm"
        sz_720 = client.screendump(ppm_720, min_size=3110400)
        assert sz_720 > 1000, f"Failed to capture 720x1440 screendump (size={sz_720})"
        stats_720 = analyze_ppm(ppm_720)
        print(f"  [OK] Baseline screendump: {stats_720['width']}x{stats_720['height']}, "
              f"bytes={stats_720['total_bytes']}, distinct_colors={stats_720['distinct_colors']}")
        assert stats_720['width'] == 720, f"Expected 720 width, got {stats_720['width']}"
        assert stats_720['height'] == 1440, f"Expected 1440 height, got {stats_720['height']}"
        assert stats_720['distinct_colors'] >= 8, f"Too few colors: {stats_720['distinct_colors']}"

        # ---------------------------------------------------------------------
        # Phase 2: Dynamic Reprogramming to Native 1080x2400
        # ---------------------------------------------------------------------
        print("\n[PHASE 2] Reprogramming scanout to Native 1080x2400 via COM1 serial...")
        client.send_cmd("SET_MODE 1080 2400")
        mode_set_ok = client.wait_for("[DISPLAY_HAL] SET_MODE 1080x2400 kms=true physical=true", timeout=15.0)
        assert mode_set_ok, "SET_MODE 1080 2400 failed or rejected by kernel"
        print("  [OK] Kernel confirmed SET_MODE 1080 2400 (kms=true, physical=true).")

        # Let compositor flip a frame into the new 1080x2400 canvas
        time.sleep(1.5)

        # ---------------------------------------------------------------------
        # Phase 3: Screendump & Pixel Analysis of Native 1080x2400
        # ---------------------------------------------------------------------
        print("\n[PHASE 3] Capturing Native 1080x2400 screendump from QEMU monitor...")
        ppm_1080 = ROOT / "target" / "scanout_native_1080x2400.ppm"
        sz_1080 = client.screendump(ppm_1080, min_size=7776000)
        assert sz_1080 > 1000, f"Failed to capture 1080x2400 screendump (size={sz_1080})"
        stats_1080 = analyze_ppm(ppm_1080)
        print(f"  [OK] Native 1080p screendump: {stats_1080['width']}x{stats_1080['height']}, "
              f"bytes={stats_1080['total_bytes']}, distinct_colors={stats_1080['distinct_colors']}")

        assert stats_1080['width'] == 1080, f"Expected width 1080, got {stats_1080['width']}"
        assert stats_1080['height'] == 2400, f"Expected height 2400, got {stats_1080['height']}"
        assert stats_1080['total_bytes'] == 1080 * 2400 * 3, f"Unexpected byte count: {stats_1080['total_bytes']}"
        assert stats_1080['distinct_colors'] >= 8, f"Insufficient distinct colors: {stats_1080['distinct_colors']}"
        print("  [OK] Native 1080x2400 pixel geometry, RGB buffer integrity and color diversity verified!")

        # ---------------------------------------------------------------------
        # Phase 4: Framebuffer Extension Gating & Safety Bounds
        # ---------------------------------------------------------------------
        print("\n[PHASE 4] Testing safety gate against oversized display requests...")
        # Requesting 5000x5000 would exceed MAX_SCANOUT_PIXELS (3840x2160) and MAX_VRAM_BYTES (32 MiB)
        client.send_cmd("SET_MODE 5000 5000")
        rejected = client.wait_for("[DISPLAY_HAL] SET_MODE 5000x5000 kms=false physical=false", timeout=10.0)
        assert rejected, "Oversized display mode was not rejected by safety gate"
        print("  [OK] Oversized mode safely rejected (no out-of-bounds mapping).")

        # ---------------------------------------------------------------------
        # Phase 5: Clean ACPI Shutdown
        # ---------------------------------------------------------------------
        print("\n[PHASE 5] Executing clean ACPI shutdown...")
        client.send_cmd("SHUTDOWN")
        shutdown_ack = client.wait_for("[SYSTEM] Clean shutdown requested via COM1 serial.", timeout=10.0)
        assert shutdown_ack, "Clean shutdown acknowledgment missing"

        ser_sock.close()
        ser_sock = None
        mon_sock.close()
        mon_sock = None

        exit_code = proc.wait(timeout=15.0)
        print(f"  [OK] QEMU exited cleanly with code {exit_code}")
        assert exit_code == 0, f"QEMU exited with code {exit_code}"

        # ---------------------------------------------------------------------
        # Save JSON Report
        # ---------------------------------------------------------------------
        report_path = ROOT / "target" / "native_1080p_scanout_report.json"
        with open(report_path, "w", encoding="utf-8") as f:
            json.dump({
                "target": "x86_64",
                "test": "Native 1080x2400 Scanout & Dynamic Framebuffer Verification",
                "status": "PASS",
                "vgamem_mb": 32,
                "baseline_720x1440": stats_720,
                "native_1080x2400": stats_1080,
                "safety_gate_verified": True,
                "clean_shutdown": True
            }, f, indent=2)
        print(f"\n[REPORT] Saved report to {report_path}")
        print("\n" + "=" * 72)
        print("  NATIVE 1080x2400 SCANOUT & FRAMEBUFFER VERIFICATION: PASSED!")
        print("=" * 72)
        return 0

    except Exception as e:
        print(f"\n[TEST FAILED] {e}")
        import traceback
        traceback.print_exc()
        # Dump the captured serial log tail so boot/timeout failures are diagnosable.
        try:
            if client and client.captured_logs:
                tail = client.captured_logs[-60:]
                print("\n[SERIAL TAIL] Last 60 lines captured on COM1:")
                for line in tail:
                    print(f"  | {line}")
            else:
                print("\n[SERIAL TAIL] (no serial output captured before failure)")
        except Exception:
            pass
        return 1

    finally:
        for s in [ser_sock, mon_sock]:
            if s:
                try: s.close()
                except Exception: pass
        if proc and proc.poll() is None:
            proc.terminate()
            try: proc.wait(timeout=1.0)
            except Exception: proc.kill()


if __name__ == "__main__":
    sys.exit(main())
