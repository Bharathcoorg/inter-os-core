import socket
import subprocess
import time
import os

qemu_exe = r"C:\Users\bhara\qemu\qemu-system-x86_64.exe"
kernel_img = os.path.abspath(r"target\inter-os-bios.img")

subprocess.run(["taskkill", "/F", "/IM", "qemu-system-x86_64.exe"], capture_output=True)
time.sleep(0.5)

port = 5555
args = [
    qemu_exe,
    "-drive", f"format=raw,file={kernel_img}",
    "-m", "512M",
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
        while time.time() - start_t < 30.0:
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
