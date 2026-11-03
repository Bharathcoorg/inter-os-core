# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Real QEMU interaction test for the x86_64 InterOS guest.

This drives the actual guest through a smartphone interaction sequence using the
guest's own COM1 input protocol and the QEMU monitor's screendump, then asserts
on the rendered framebuffer.

Validation is semantic and region-based rather than pixel-exact: it measures
luminance and contrast in regions of the screen, so a legitimate redraw or a
theme tweak does not break the test while a genuinely blank or unreadable screen
does.

Usage:
    python tests/qemu/interaction_x86.py
    python tests/qemu/interaction_x86.py --out target/interaction --keep
"""

import argparse
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import tempfile
import threading
import time

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
IMAGE = os.path.join(ROOT, "target", "inter-os-bios.img")
DATA_DISK = os.path.join(ROOT, "target", "inter-os-data.img")
DATA_DISK_BYTES = 512 * 1024 * 1024

MONITOR_PORT = 44551
SERIAL_PORT = 43551

# The x86_64 guest engages a native 720x1440 portrait DISPI scanout once paging
# is live (kernel::framebuffer::engage_native_mode). The screendump is that
# scanout, so the phone surface is the whole frame. The legacy 1280x720
# letterboxed landscape mode is no longer produced by the current kernel.
FRAMEBUFFER_W, FRAMEBUFFER_H = 720, 1440
PHONE_X0 = 0
PHONE_W = FRAMEBUFFER_W

BOOT_TIMEOUT = 240.0
SETTLE = 2.5


class QemuMonitor:
    def __init__(self, port=MONITOR_PORT):
        self.sock = socket.create_connection(("127.0.0.1", port), timeout=10)
        self.sock.settimeout(4.0)
        self._drain()

    def _drain(self):
        try:
            while self.sock.recv(65536):
                pass
        except socket.timeout:
            pass
        except OSError:
            pass

    def cmd(self, text, settle=0.6):
        self._drain()
        self.sock.sendall(text.encode() + b"\n")
        time.sleep(settle)
        return self._drain()

    def screendump(self, ppm_path):
        self.cmd("screendump " + ppm_path.replace("\\", "/"), settle=1.2)
        for _ in range(40):
            if os.path.exists(ppm_path):
                return True
            time.sleep(0.25)
        return False


class GuestSerial:
    """Single long-lived COM1 connection drained by a background thread.

    The guest's UART accepts exactly one client and emits boot banners before any
    test step runs, so the reader must be attached immediately and must never
    block the harness on a read.
    """

    def __init__(self, port=SERIAL_PORT, wait=60.0):
        deadline = time.time() + wait
        self.sock = None
        while time.time() < deadline:
            try:
                self.sock = socket.create_connection(("127.0.0.1", port), timeout=5)
                break
            except OSError:
                time.sleep(0.4)
        if self.sock is None:
            raise RuntimeError("guest serial port never accepted a connection")

        self.log = []
        self._lock = threading.Lock()
        self._running = True
        self._thread = threading.Thread(target=self._reader, daemon=True)
        self._thread.start()

    def _reader(self):
        while self._running:
            try:
                chunk = self.sock.recv(65536)
            except socket.timeout:
                continue
            except OSError:
                break
            if not chunk:
                break
            with self._lock:
                self.log.append(chunk.decode(errors="replace"))

    def text(self):
        with self._lock:
            return "".join(self.log)

    def wait_for(self, pattern, timeout=60.0):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if re.search(pattern, self.text()):
                return True
            time.sleep(0.3)
        return False

    def send(self, command, settle=0.4):
        self.sock.sendall(command.encode() + b"\n")
        time.sleep(settle)

    def swipe_up(self, steps=10):
        """Swipe from the home indicator towards the top of the phone surface."""
        x, y0, y1 = 360, 1300, 640
        self.send(f"INPUT_TOUCH DOWN {x} {y0} 0", settle=0.15)
        for i in range(1, steps):
            y = y0 + (y1 - y0) * i // steps
            self.send(f"INPUT_TOUCH MOVE {x} {y} 0", settle=0.05)
        self.send(f"INPUT_TOUCH UP {x} {y1} 0", settle=SETTLE)

    def tap(self, x, y):
        self.send(f"INPUT_TOUCH DOWN {x} {y} 0", settle=0.15)
        self.send(f"INPUT_TOUCH UP {x} {y} 0", settle=SETTLE)

    def close(self):
        self._running = False
        try:
            self.sock.close()
        except OSError:
            pass


def start_qemu(extra_args=()):
    if not os.path.exists(IMAGE):
        raise SystemExit(f"[SKIP] boot image missing: {IMAGE}")
    exe = None
    for cand in ("qemu-system-x86_64", "qemu-system-x86_64.exe"):
        try:
            subprocess.run([cand, "--version"], capture_output=True, check=True, timeout=30)
            exe = cand
            break
        except (OSError, subprocess.SubprocessError):
            continue
    if not exe:
        raise SystemExit("[SKIP] qemu-system-x86_64 not available on PATH")

    # A dedicated data disk is mandatory. The kernel uses the Primary Master as
    # the boot image and the Primary Slave as persistent storage; with only one
    # disk attached it falls back to formatting the boot image itself, which
    # destroys the bootloader stages the kernel is embedded in, so every boot
    # after the first loads a corrupted kernel.
    ensure_data_disk()

    cmd = [
        exe,
        "-drive", f"format=raw,file={IMAGE},index=0",
        "-drive", f"format=raw,file={DATA_DISK},index=1",
        "-m", "1024M",
        "-display", "none",
        "-monitor", f"tcp:127.0.0.1:{MONITOR_PORT},server,nowait",
        "-serial", f"tcp:127.0.0.1:{SERIAL_PORT},server,nowait",
        "-no-reboot",
    ] + list(extra_args)
    return subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def ppm_size(path):
    with open(path, "rb") as handle:
        header = handle.read(64)
    match = re.match(rb"P\d\s+(\d+)\s+(\d+)", header)
    if not match:
        return None
    return int(match.group(1)), int(match.group(2))



def ensure_data_disk():
    """Creates the dedicated data disk if it does not exist yet.

    The guest formats whatever disk it is given as its own filesystem. Attached
    to the boot image that destroys the bootloader stages the kernel is embedded
    in, so the next boot runs a corrupted kernel.
    """
    if os.path.exists(DATA_DISK) and os.path.getsize(DATA_DISK) >= DATA_DISK_BYTES:
        return
    os.makedirs(os.path.dirname(DATA_DISK), exist_ok=True)
    subprocess.run(["qemu-img", "create", "-f", "raw", DATA_DISK, str(DATA_DISK_BYTES)],
                   check=True, capture_output=True)

def wait_for_graphical_framebuffer(monitor, workdir):
    """Waits until the guest leaves text mode and paints a real framebuffer."""
    deadline = time.time() + BOOT_TIMEOUT
    probe = os.path.join(workdir, "probe.ppm")
    while time.time() < deadline:
        if os.path.exists(probe):
            os.remove(probe)
        if monitor.screendump(probe):
            size = ppm_size(probe)
            if size and size == (FRAMEBUFFER_W, FRAMEBUFFER_H):
                return True
        time.sleep(3.0)
    return False


def capture(monitor, workdir, name, outdir):
    ppm = os.path.join(workdir, name + ".ppm")
    if os.path.exists(ppm):
        os.remove(ppm)
    if not monitor.screendump(ppm):
        raise RuntimeError(f"screendump failed for {name}")
    full = os.path.join(outdir, name + "_full.png")
    phone = os.path.join(outdir, name + "_phone.png")
    try:
        from PIL import Image
    except ImportError:
        raise SystemExit("[SKIP] Pillow is required to decode screendumps")
    with Image.open(ppm) as img:
        img.convert("RGB").save(full)
        img.convert("RGB").crop((PHONE_X0, 0, PHONE_X0 + PHONE_W, FRAMEBUFFER_H)).save(phone)
    return full, phone


def luminance_stats(phone_png):
    """Mean luminance and near-white coverage of the phone surface."""
    from PIL import Image

    with Image.open(phone_png) as img:
        pixels = list(img.convert("L").getdata())
    if not pixels:
        return 0.0, 0.0
    mean = sum(pixels) / len(pixels)
    bright = sum(1 for p in pixels if p >= 200)
    return mean, bright / len(pixels)


class Results:
    def __init__(self):
        self.passed = 0
        self.failed = 0

    def check(self, ok, label, detail=""):
        if ok:
            self.passed += 1
            print(f"PASS: {label}{(' -> ' + detail) if detail else ''}")
        else:
            self.failed += 1
            print(f"FAIL: {label}{(' -> ' + detail) if detail else ''}")
        return ok


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default=os.path.join(ROOT, "target", "interaction"))
    parser.add_argument("--keep", action="store_true", help="leave QEMU running")
    args = parser.parse_args()

    os.makedirs(args.out, exist_ok=True)
    proc = start_qemu()
    results = Results()

    try:
        monitor = None
        for _ in range(40):
            try:
                monitor = QemuMonitor()
                break
            except OSError:
                time.sleep(0.5)
        if monitor is None:
            raise RuntimeError("QEMU monitor did not accept a connection")

        # Attach serial before waiting on the framebuffer so the boot banners are
        # captured; they explain why a surface is missing when one is.
        serial = GuestSerial()

        with tempfile.TemporaryDirectory() as workdir:
            if not results.check(
                wait_for_graphical_framebuffer(monitor, workdir),
                "guest reaches a graphical framebuffer",
                f"{FRAMEBUFFER_W}x{FRAMEBUFFER_H}",
            ):
                Path(args.out, "serial_tail.txt").write_text(
                    serial.text(), encoding="utf-8", errors="replace"
                )
                raise SystemExit(1)

            results.check(
                serial.wait_for(r"name=launcher", timeout=120.0),
                "userspace launcher task is running",
                "boot banner contains a launcher process",
            )

            # 1. Lock screen must be present and legible.
            _, lock_phone = capture(monitor, workdir, "01_lock", args.out)
            lock_mean, lock_bright = luminance_stats(lock_phone)
            results.check(
                lock_mean < 70,
                "lock screen uses a dark backdrop",
                f"mean luminance {lock_mean:.1f}",
            )
            results.check(
                lock_bright > 0.001,
                "lock screen renders legible light content",
                f"near-white coverage {lock_bright * 100:.2f}%",
            )

            # 2. Unlock gesture must move to the home surface.
            before_mean = lock_mean
            serial.swipe_up()
            serial.wait_for(r"Device unlocked via (tap|swipe)", timeout=30.0)
            time.sleep(2.0)
            _, home_phone = capture(monitor, workdir, "02_home", args.out)
            home_mean, home_bright = luminance_stats(home_phone)
            results.check(
                abs(home_mean - before_mean) > 4 or abs(home_bright - lock_bright) > 0.001,
                "unlock gesture changes the rendered surface",
                f"luminance {before_mean:.1f} -> {home_mean:.1f}",
            )

            # 3. Tapping a launcher icon must actually start an application.
            # Assert on the launch record rather than a pixel delta: an app that
            # renders in a similar palette would defeat a luminance comparison
            # without indicating any real failure.
            before_log = len(serial.text())
            serial.tap(130, 840)   # Phone (dialer) dock tile, row 0 col 0
            _, app_phone = capture(monitor, workdir, "03_app", args.out)
            app_mean, app_bright = luminance_stats(app_phone)
            new_log = serial.text()[before_log:]
            results.check(
                "Launched application from real touch input" in new_log
                or "Launched registered application" in new_log,
                "tapping a launcher icon starts an application",
                "launcher recorded the launch",
            )
            results.check(
                abs(app_mean - home_mean) > 1 or abs(app_bright - home_bright) > 0.0002,
                "the launched application renders a different surface",
                f"luminance {home_mean:.1f} -> {app_mean:.1f}",
            )

            Path(args.out, "serial_tail.txt").write_text(
                serial.text(), encoding="utf-8", errors="replace"
            )
            tail = serial.text()
            results.check(
                "KERNEL PANIC" not in tail,
                "guest reported no kernel panic during interaction",
            )
            serial.close()
    except SystemExit:
        raise
    except Exception as exc:  # noqa: BLE001 - report any harness failure as a failure
        results.failed += 1
        print(f"FAIL: interaction harness -> {exc}")
    finally:
        if not args.keep and proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()

    print(f"\n{results.passed} passed, {results.failed} failed")
    return 1 if results.failed else 0


if __name__ == "__main__":
    sys.exit(main())
