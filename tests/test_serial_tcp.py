import socket
import subprocess
import time
import os
import shutil
import sys

if hasattr(sys.stdout, 'reconfigure'):
    sys.stdout.reconfigure(encoding='utf-8', errors='replace')

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

qemu_exe = find_qemu("qemu-system-x86_64.exe")
kernel_img = os.path.abspath(r"target\inter-os-bios.img")

subprocess.run(["taskkill", "/F", "/IM", "qemu-system-x86_64.exe"], capture_output=True)
time.sleep(0.5)

port = 5555
args = [
    qemu_exe,
    "-drive", f"format=raw,file={kernel_img}",
    "-m", "1024M",
    "-display", "none",
    "-chardev", f"socket,id=ser0,host=127.0.0.1,port={port},server=on,wait=off",
    "-serial", "chardev:ser0",
    "-no-reboot"
]

proc = subprocess.Popen(args)
time.sleep(0.5)

lines = []
try:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.settimeout(2.0)
        s.connect(('127.0.0.1', port))
        s.settimeout(0.5)
        start_t = time.time()
        buf = b""
        while time.time() - start_t < 120.0:
            try:
                data = s.recv(4096)
                if not data:
                    break
                buf += data
                while b"\n" in buf:
                    line, buf = buf.split(b"\n", 1)
                    decoded = line.decode('utf-8', errors='replace').strip()
                    if decoded:
                        lines.append(decoded)
                        print(f"[SERIAL] {decoded}")
                        if "Step 5: Initial presentation complete" in decoded:
                            print(f"[SUCCESS] Launcher presentation complete at {time.time() - start_t:.1f}s")
                            print("[TEST STEP] Tapping battery icon at (670, 25) to open Power Menu...")
                            s.sendall(b"TAP 670 25\n")
                            time.sleep(1.0)
                            print("[TEST STEP] Tapping Lock Device card at (450, 230)...")
                            s.sendall(b"TAP 450 230\n")
                            time.sleep(1.0)
                            print("[TEST STEP] Tapping Flashlight shortcut at (72, 1330)...")
                            s.sendall(b"TAP 72 1330\n")
                            time.sleep(1.0)
                            print("[TEST STEP] Swiping up to unlock (360 1370 -> 360 1100)...")
                            s.sendall(b"SWIPE 360 1370 360 1100 5\n")
                            time.sleep(1.0)
                            print("[TEST STEP] Tapping top status bar left at (100, 25) to open Notifications...")
                            s.sendall(b"TAP 100 25\n")
                            time.sleep(1.0)
                            print("[TEST STEP] Tapping Clear All at (360, 720) to dismiss Notifications...")
                            s.sendall(b"TAP 360 720\n")
                            time.sleep(1.0)
                            print("[TEST STEP] Swiping up from middle (360 800 -> 360 400) to open App Drawer...")
                            s.sendall(b"SWIPE 360 800 360 400 5\n")
                            time.sleep(1.0)
                            print("[TEST STEP] Tapping Tools tab at (540, 160)...")
                            s.sendall(b"TAP 540 160\n")
                            time.sleep(1.0)
                            print("[TEST STEP] Tapping bottom home pill at (360, 1420) to return Home...")
                            s.sendall(b"TAP 360 1420\n")
                            time.sleep(1.5)
                            break
                if any("Step 5: Initial presentation complete" in l for l in lines):
                    break
            except socket.timeout:
                continue
except Exception as e:
    print(f"Socket error: {e}")
finally:
    proc.terminate()
    try:
        proc.wait(timeout=1.0)
    except Exception:
        proc.kill()

print(f"Total lines captured: {len(lines)}")
for l in lines:
    if any(k in l for k in ["[TEST]", "SDHCI", "Goodix", "SimpleFB", "PANIC", "assertion"]):
        print(l)
