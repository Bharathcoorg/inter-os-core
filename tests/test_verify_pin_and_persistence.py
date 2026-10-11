#!/usr/bin/env python3
"""
Test First-Boot 6-Digit PIN Setup, Confirmation, Persistence and Reboot Unlock.
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

    print("[TEST] Awaiting first-boot PIN setup surface...")
    ready = reader.wait_for("First boot: credential setup presented", timeout=60.0,
                            print_filter=["BOOT", "VFS", "LAUNCHER", "USER", "Step 5"])
    assert ready, "First boot PIN setup screen not reached!"
    print("[TEST] First boot PIN setup screen confirmed!")

    time.sleep(1.0)
    print("[TEST] Step 1: Entering PIN 123456C for initial setup...")
    reader.send("PIN 123456C")

    print("[TEST] Awaiting confirmation request...")
    confirmed_req = reader.wait_for("confirmation requested", timeout=45.0,
                                    print_filter=["PIN", "LAUNCHER", "USER", "INPUT"])
    assert confirmed_req, "PIN confirmation was not requested!"
    print("[TEST] PIN confirmation request received!")

    time.sleep(1.0)
    print("[TEST] Step 2: Entering matching PIN 123456C to confirm...")
    reader.send("PIN 123456C")

    print("[TEST] Awaiting credential persistence & unlock...")
    persisted = reader.wait_for("Credential persisted to", timeout=45.0,
                                print_filter=["PIN", "LAUNCHER", "USER", "INPUT", "VFS"])
    assert persisted, "Credential was not persisted to disk!"
    print("[TEST] Credential persisted to VFS successfully!")

    unlocked = reader.wait_for("First-boot credential established; device unlocked", timeout=45.0,
                               print_filter=["PIN", "LAUNCHER", "USER"])
    assert unlocked, "Device was not unlocked after PIN confirmation!"
    print("[TEST] Device unlocked to HomeScreen!")

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

    # --- SESSION 2: Reboot with Persisted Credential, Test Lock Screen & Unlock ---
    print("\n--- Session 2: Rebooting with same disk to verify persistence ---")
    proc, ser_sock, mon_sock = launch_qemu(boot_img, data_img, ser_port, mon_port)
    reader = SerialReader(ser_sock)

    print("[TEST] Awaiting credential restore and LockScreen presentation...")
    restored = reader.wait_for("Restored persisted credential for UID=1000", timeout=60.0,
                               print_filter=["BOOT", "VFS", "USER", "LAUNCHER", "RESTORE"])
    assert restored, "Credential was not restored on reboot!"
    print("[TEST] Credential restored from disk successfully!")

    lock_presented = reader.wait_for("lock screen is now guest-owned", timeout=30.0,
                                     print_filter=["LAUNCHER", "Step 5"])
    assert lock_presented, "Lock screen was not presented on reboot!"
    print("[TEST] Lock screen presented on reboot!")

    time.sleep(1.0)
    # Unlock via tap to open PIN entry and PIN 123456C
    print("[TEST] Tapping lock screen to open PIN entry...")
    reader.send("TAP 360 800")
    pin_opened = reader.wait_for("credential entry required", timeout=15.0,
                                 print_filter=["LAUNCHER", "INPUT"])
    assert pin_opened, "PIN entry was not opened from lock screen tap!"
    print("[TEST] PIN entry surface opened!")

    time.sleep(0.5)
    print("[TEST] Entering PIN 123456C to authenticate...")
    reader.send("PIN 123456C")

    auth_ok = reader.wait_for("User authentication succeeded; device unlocked", timeout=45.0,
                              print_filter=["LAUNCHER", "USER", "INPUT"])
    assert auth_ok, "PIN unlock failed on reboot!"
    print("[TEST] User authentication succeeded! Device unlocked!")

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
    print("  ALL FIRST-BOOT, CONFIRM, PERSISTENCE & UNLOCK TESTS PASSED!")
    print("============================================================")

if __name__ == "__main__":
    main()
