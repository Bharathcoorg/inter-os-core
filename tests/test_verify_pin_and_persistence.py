#!/usr/bin/env python3
"""
Test that a device with no credential boots to the plain lock screen and unlocks
on a swipe, and that the account layer never claims a credential it does not hold.

WHY the shape of this test changed: the shell no longer forces first-boot PIN
setup -- a lock is set from Settings > Security instead -- so there is no
first-boot setup pad to wait for and no credential on a fresh disk. What this
suite now pins is the contract that replaced it:

  1. A fresh disk reaches the lock screen, and the old "credential setup
     presented" line never appears (so forced setup cannot creep back).
  2. The swipe gesture unlocks a device that has no credential.
  3. After a reboot the same holds, and no credential is "restored" from a disk
     that never had one.

Credential set/change/remove, and their persistence across a reboot, are driven
from the Settings > Security panel and are covered by the account layer's unit
tests in `kernel/src/user/mod.rs` (`user::tests`); driving that panel over
serial taps proved unreliable because a tap sent during a repaint is dropped.
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
                 f"(Get-NetTCPConnection -LocalPort {port} -ErrorAction SilentlyContinue).OwningProcess"],
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

    def send(self, line):
        time.sleep(0.3)
        cmd_bytes = (line if line.endswith("\n") else line + "\n").encode("utf-8")
        self.sock.sendall(cmd_bytes)

    def wait_for(self, marker, timeout=30.0, print_filter=None):
        start = time.time()
        for l in self.captured_logs:
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

def main():
    print("=" * 60)
    print("  Testing First-Boot 6-Digit PIN Setup & Persistence")
    print("=" * 60)

    boot_img = ROOT / "target" / "inter-os-bios.img"
    data_img = ROOT / "target" / "test_pin_data.img"
    if data_img.exists():
        data_img.unlink()

    # Create 64 MB blank disk
    with open(data_img, "wb") as f:
        f.seek(64 * 1024 * 1024 - 1)
        f.write(b"\0")
    print(f"[TEST] Created fresh data disk: {data_img}")

    ser_port = 5560
    mon_port = 5561

    # --- SESSION 1: First Boot, Set PIN 123456 with Confirmation ---
    print("\n--- Session 1: First boot on fresh disk ---")
    proc, ser_sock, mon_sock = launch_qemu(boot_img, data_img, ser_port, mon_port)
    reader = SerialReader(ser_sock)

    print("[TEST] Awaiting the lock screen (no forced credential setup)...")
    ready = reader.wait_for("lock screen is now guest-owned", timeout=90.0,
                            print_filter=["BOOT", "VFS", "LAUNCHER", "USER", "Step 5"])
    assert ready, "Lock screen not reached on a fresh disk!"
    print("[TEST] Lock screen presented without a PIN prompt!")

    # WHY this asserts the ABSENCE of a line: this shell no longer forces
    # first-boot credential setup, so an owner with no lock must reach the plain
    # lock screen. If a future change puts the setup pad back on the boot path,
    # this fails loudly instead of the device quietly demanding a PIN again.
    time.sleep(2.0)
    assert "First boot: credential setup presented" not in "".join(reader.captured_logs), \
        "First-boot credential setup is being forced again!"

    time.sleep(1.0)
    print("[TEST] Step 1: Swiping up from the bottom edge to unlock...")
    reader.send("SWIPE 360 1425 360 1100")

    unlocked = reader.wait_for("Device unlocked via swipe/slide gesture", timeout=45.0,
                               print_filter=["LAUNCHER", "USER", "INPUT"])
    assert unlocked, "A credential-less device did not unlock on the swipe gesture!"
    print("[TEST] Device unlocked on the swipe gesture, as it must be with no PIN!")

    ppm_out = ROOT / "target" / "first_boot_unlocked.ppm"
    screendump(mon_sock, ppm_out)
    if ppm_out.exists() and ppm_out.stat().st_size > 1000000:
        print(f"[TEST] Framebuffer dumped successfully: {ppm_out} ({ppm_out.stat().st_size} bytes)")

    # Shutdown Session 1 cleanly
    print("[TEST] Closing Session 1...")
    try:
        mon_sock.sendall(b"quit\n")
    except Exception:
        pass
    ser_sock.close()
    mon_sock.close()
    proc.wait(timeout=5)

    time.sleep(1.5)

    # --- SESSION 2: Reboot on the same disk, which still holds no credential ---
    print("\n--- Session 2: Rebooting with same disk ---")
    proc, ser_sock, mon_sock = launch_qemu(boot_img, data_img, ser_port, mon_port)
    reader = SerialReader(ser_sock)

    lock_presented = reader.wait_for("lock screen is now guest-owned", timeout=90.0,
                                     print_filter=["LAUNCHER", "Step 5", "USER"])
    assert lock_presented, "Lock screen was not presented on reboot!"
    print("[TEST] Lock screen presented on reboot!")

    # WHY no credential restore is expected: nothing was ever set, so the disk
    # holds none. A restore line here would mean the account layer invented one.
    time.sleep(2.0)
    assert "Restored persisted credential for UID=1000" not in "".join(reader.captured_logs), \
        "A credential was restored from a disk that never had one!"

    time.sleep(1.0)
    print("[TEST] Unlocking on the swipe gesture after reboot...")
    reader.send("SWIPE 360 1425 360 1100")
    unlocked = reader.wait_for("Device unlocked via swipe/slide gesture", timeout=45.0,
                               print_filter=["LAUNCHER", "USER", "INPUT"])
    assert unlocked, "Rebooted device demanded a credential it does not have!"
    print("[TEST] Rebooted device unlocked on the swipe gesture!")

    ppm_reboot = ROOT / "target" / "reboot_unlocked.ppm"
    screendump(mon_sock, ppm_reboot)
    if ppm_reboot.exists() and ppm_reboot.stat().st_size > 1000000:
        print(f"[TEST] Framebuffer dumped successfully: {ppm_reboot} ({ppm_reboot.stat().st_size} bytes)")

    try:
        mon_sock.sendall(b"quit\n")
    except Exception:
        pass
    ser_sock.close()
    mon_sock.close()
    proc.wait(timeout=5)

    print("\n============================================================")
    print("  ALL BOOT, UNLOCK & CREDENTIAL-STATE TESTS PASSED!")
    print("============================================================")

if __name__ == "__main__":
    main()
