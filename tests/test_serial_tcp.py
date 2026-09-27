import socket
import subprocess
import time
import os
import shutil
import sys

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

qemu_exe = find_qemu("qemu-system-x86_64.exe")
kernel_img = os.path.abspath(r"target\inter-os-bios.img")

k1 = os.path.abspath(r"kernel\target\x86_64-unknown-none\debug\kernel")
k2 = os.path.abspath(r"target\x86_64-unknown-none\debug\kernel")
kernel_elf = k1 if os.path.exists(k1) else k2
if os.path.exists(kernel_elf):
    if not os.path.exists(kernel_img) or os.path.getmtime(kernel_elf) > os.path.getmtime(kernel_img):
        print("[TEST SETUP] Updating bootable disk image with latest kernel ELF...")
        subprocess.run(["cargo", "+nightly", "run", "--manifest-path", "boot/Cargo.toml", "--", kernel_elf, kernel_img], check=True)

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
                            def send_and_wait_for(cmd, target_str, timeout=6.0):
                                s.sendall(cmd)
                                deadline = time.time() + timeout
                                non_local_buf = b""
                                found = False
                                while time.time() < deadline:
                                    try:
                                        d = s.recv(4096)
                                        if d:
                                            non_local_buf += d
                                            while b"\n" in non_local_buf:
                                                nl, non_local_buf = non_local_buf.split(b"\n", 1)
                                                dec = nl.decode('utf-8', errors='replace').strip()
                                                if dec:
                                                    lines.append(dec)
                                                    print(f"[SERIAL] {dec}")
                                                    if target_str in dec:
                                                        found = True
                                        if found:
                                            time.sleep(0.3)
                                            return True
                                    except socket.timeout:
                                        pass
                                return found

                            print("[TEST STEP 1] Tapping battery icon at (670, 25) to open Power Menu...")
                            send_and_wait_for(b"TAP 670 25\n", "Power menu opened via battery status bar tap")
                            print("[TEST STEP 2] Tapping Lock Device card at (450, 230)...")
                            send_and_wait_for(b"TAP 450 230\n", "Device locked via power menu")
                            print("[TEST STEP 3] Tapping Flashlight shortcut at (72, 1330)...")
                            send_and_wait_for(b"TAP 72 1330\n", "Flashlight toggled on lock screen")
                            print("[TEST STEP 4] Swiping up to unlock (360 1370 -> 360 1100)...")
                            send_and_wait_for(b"SWIPE 360 1370 360 1100 5\n", "Device unlocked")
                            print("[TEST STEP 5] Tapping top status bar left at (100, 25) to open Notifications...")
                            send_and_wait_for(b"TAP 100 25\n", "Notifications shade opened")
                            print("[TEST STEP 6] Tapping Clear All at (360, 720) to dismiss Notifications...")
                            send_and_wait_for(b"TAP 360 720\n", "Notifications cleared")
                            print("[TEST STEP 7] Swiping up from middle (360 800 -> 360 400) to open App Drawer...")
                            send_and_wait_for(b"SWIPE 360 800 360 400 5\n", "App Drawer opened")
                            print("[TEST STEP 8] Tapping Tools tab at (540, 160)...")
                            send_and_wait_for(b"TAP 540 160\n", "App drawer category tab switched")
                            print("[TEST STEP 9] Tapping bottom home pill at (360, 1420) to return Home...")
                            send_and_wait_for(b"TAP 360 1420\n", "Returned to Home from App Drawer")
                            print("[TEST STEP 10] Tapping mic icon at (640, 1330) to open AI Companion...")
                            send_and_wait_for(b"TAP 640 1330\n", "AI Companion opened via mic tap")
                            print("[TEST STEP 11] Tapping mic icon at (640, 1330) to toggle voice listening...")
                            send_and_wait_for(b"TAP 640 1330\n", "AI voice listening toggled")
                            print("[TEST STEP 12] Tapping bottom home pill at (360, 1420) to return Home...")
                            send_and_wait_for(b"TAP 360 1420\n", "Returned to Home from AI Companion")
                            print("[TEST STEP 13] Swiping left (500 800 -> 300 800) to open Live Widgets...")
                            send_and_wait_for(b"SWIPE 500 800 300 800 5\n", "Live Widgets opened")
                            print("[TEST STEP 14] Tapping bottom home pill at (360, 1420) to return Home...")
                            send_and_wait_for(b"TAP 360 1420\n", "Returned to Home from Live Widgets")
                            print("[TEST STEP 15] Swiping up from middle to open App Drawer...")
                            send_and_wait_for(b"SWIPE 360 800 360 400 5\n", "App Drawer opened")
                            print("[TEST STEP 16] Tapping Files app at (430, 260) in App Drawer to launch...")
                            send_and_wait_for(b"TAP 430 260\n", "Files UI presented to scanout buffer")
                            print("[TEST STEP 17] Tapping Images category shortcut at (260, 280) in Files...")
                            send_and_wait_for(b"TAP 260 280\n", "Category filter shortcut switched")
                            print("[TEST STEP 18] Tapping bottom home pill at (360, 1420) to exit Files back to Launcher...")
                            send_and_wait_for(b"TAP 360 1420\n", "Foreground application exited and was reaped")
                            print("[TEST STEP 19] Tapping Gallery dock item at (410, 1180) to launch Gallery...")
                            send_and_wait_for(b"TAP 410 1180\n", "Gallery UI presented to scanout buffer")
                            print("[TEST STEP 20] Tapping photo tile 0 at (180, 300) in Gallery...")
                            send_and_wait_for(b"TAP 180 300\n", "Photo tapped: inspection modal opened")
                            print("[TEST STEP 21] Tapping Close button at (360, 1170) to close inspection modal...")
                            send_and_wait_for(b"TAP 360 1170\n", "Closed photo inspection modal")
                            print("[TEST STEP 22] Tapping bottom home pill at (360, 1420) to exit Gallery back to Launcher...")
                            send_and_wait_for(b"TAP 360 1420\n", "Foreground application exited and was reaped")
                            print("[TEST STEP 23] Tapping Calculator card on HomeScreen at (180, 520) to launch...")
                            send_and_wait_for(b"TAP 180 520\n", "Initial frame presented to hardware scanout")
                            print("[TEST STEP 24] Tapping digit '7' on Calculator keypad at (100, 680)...")
                            send_and_wait_for(b"TAP 100 680\n", "Keypad digit pressed")
                            print("[TEST STEP 25] Tapping bottom home pill at (360, 1420) to exit Calculator back to Launcher...")
                            send_and_wait_for(b"TAP 360 1420\n", "Foreground application exited and was reaped")
                            print("[TEST COMPLETE] All interactive gesture steps completed.")
                            lines.append("[TEST COMPLETE]")
                            break
                if any("[TEST COMPLETE]" in l for l in lines) or any("All interactive gesture steps completed" in l for l in lines):
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
