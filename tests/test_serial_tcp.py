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
data_img = os.path.abspath(r"target\\test_serial_data.img")
if not os.path.exists(data_img):
    # The gallery writes its capture under /storage, which only exists once a
    # data disk is attached, so the walkthrough boots with one. It is snapshotted
    # like the boot disk, so no run leaves bytes behind.
    with open(data_img, "wb") as fh:
        fh.write(b"\0" * (64 * 1024 * 1024))


k1 = os.path.abspath(r"kernel\target\x86_64-unknown-none\debug\kernel")
k2 = os.path.abspath(r"target\x86_64-unknown-none\debug\kernel")
kernel_elf = k1 if os.path.exists(k1) else k2
if os.path.exists(kernel_elf):
    if not os.path.exists(kernel_img) or os.path.getmtime(kernel_elf) > os.path.getmtime(kernel_img):
        print("[TEST SETUP] Updating bootable disk image with latest kernel ELF...")
        subprocess.run(["cargo", "+nightly", "run", "--manifest-path", "boot/Cargo.toml", "--", kernel_elf, kernel_img], check=True)

def kill_proc_on_port(p):
    try:
        import psutil
        for conn in psutil.net_connections():
            if conn.laddr and conn.laddr.port == p and conn.pid:
                try:
                    proc = psutil.Process(conn.pid)
                    # A foreign owner means another device is using this port;
                    # leave it running and let the bind below fail loudly.
                    if "qemu" in proc.name().lower():
                        proc.kill()
                except Exception:
                    pass
    except Exception:
        pass

# WHY not 5555/5556: those are the Android emulator's console band, and a guest
# serial socket there can bind over - or talk to - somebody else's device.
port = 5593
kill_proc_on_port(port)
time.sleep(0.5)
args = [
    qemu_exe,
    "-machine", "q35",
    "-cpu", "qemu64",
    "-device", "piix3-ide,id=ide",
    "-drive", f"file={kernel_img},format=raw,if=none,id=bootd,snapshot=on",
    "-device", "ide-hd,bus=ide.0,unit=0,drive=bootd,bootindex=1",
    "-drive", f"file={data_img},format=raw,if=none,id=datad,snapshot=on",
    "-device", "ide-hd,bus=ide.0,unit=1,drive=datad",
    "-m", "1024M",
    "-display", "none",
    "-chardev", f"socket,id=ser0,host=127.0.0.1,port={port},server=on,wait=off",
    "-serial", "chardev:ser0",
    "-no-reboot",
]

proc = subprocess.Popen(args)
walkthrough_failure = None
time.sleep(0.5)

lines = []
try:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.settimeout(2.0)
        s.connect(('127.0.0.1', port))
        s.settimeout(0.5)
        start_t = time.time()
        buf = b""
        while time.time() - start_t < 400.0:
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
                        if "Boot presentation complete; lock screen is now guest-owned" in decoded:
                            print(f"[SUCCESS] Launcher lock screen ready at {time.time() - start_t:.1f}s")
                            non_local_buf = bytearray(buf)
                            def send_and_wait_for(cmd, target_str, timeout=40.0):
                                time.sleep(0.8)
                                s.sendall(cmd)
                                start_t = time.time()
                                deadline = start_t + timeout
                                retried = False
                                found = False
                                while time.time() < deadline:
                                    try:
                                        if not retried and (time.time() - start_t > 12.0):
                                            print(f"[RETRY] Re-sending {cmd.strip()} waiting for '{target_str}'...")
                                            s.sendall(cmd)
                                            retried = True
                                        d = s.recv(4096)
                                        if d:
                                            non_local_buf.extend(d)
                                            while b"\n" in non_local_buf:
                                                idx = non_local_buf.index(b"\n")
                                                nl = bytes(non_local_buf[:idx])
                                                del non_local_buf[:idx+1]
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
                                assert found, f"Timed out waiting for '{target_str}' after sending {cmd}"
                                return True

                            print("[TEST STEP 1] Tapping battery icon at (670, 25) to open Power Menu...")
                            send_and_wait_for(b"TAP 670 25\n", "Power menu opened via battery status bar tap")
                            print("[TEST STEP 2] Tapping Lock Device card at (450, 230)...")
                            send_and_wait_for(b"TAP 450 230\n", "Device locked via power menu")
                            print("[TEST STEP 3] Tapping Flashlight shortcut at (72, 1330)...")
                            send_and_wait_for(b"TAP 72 1330\n", "Flashlight unavailable")
                            print("[TEST STEP 4] Swiping up to unlock (360 1370 -> 360 1100)...")
                            send_and_wait_for(b"SWIPE 360 1370 360 1100 5\n", "Device unlocked")
                            print("[TEST STEP 5] Tapping top status bar left at (100, 25) to open Notifications...")
                            send_and_wait_for(b"TAP 100 25\n", "Notifications shade opened")
                            print("[TEST STEP 6] Tapping the header Clear all pill at (620, 80)...")
                            send_and_wait_for(b"TAP 620 80\n", "Notifications cleared")
                            print("[TEST STEP 7] Swiping up from middle (360 800 -> 360 400) to open App Drawer...")
                            send_and_wait_for(b"SWIPE 360 800 360 400 5\n", "App Drawer opened")
                            print("[TEST STEP 8] Tapping Tools tab at (540, 137)...")
                            send_and_wait_for(b"TAP 540 137\n", "App drawer category tab switched")
                            print("[TEST STEP 9] Tapping bottom home pill at (360, 1420) to return Home...")
                            send_and_wait_for(b"TAP 360 1420\n", "Returned to Home from App Drawer")
                            print("[TEST STEP 10] Tapping universal search bar at (360, 700) to open Global search...")
                            send_and_wait_for(b"TAP 360 700\n", "Global search opened via universal search bar tap")
                            print("[TEST STEP 11] Tapping bottom home pill at (360, 1420) to return Home from search...")
                            send_and_wait_for(b"TAP 360 1420\n", "Returned to home grid from search via home tap")
                            print("[TEST STEP 12] Tapping Today card checkbox at (78, 318) to toggle task state...")
                            send_and_wait_for(b"TAP 78 318\n", "Contextual task 0 toggled")
                            print("[TEST STEP 13] Swiping left (500 800 -> 300 800) to open Live Widgets...")
                            send_and_wait_for(b"SWIPE 500 800 300 800 5\n", "Live Widgets opened")
                            print("[TEST STEP 14] Tapping bottom home pill at (360, 1420) to return Home...")
                            send_and_wait_for(b"TAP 360 1420\n", "Returned to Home from Live Widgets")
                            print("[TEST STEP 15] Swiping up from middle to open App Drawer...")
                            send_and_wait_for(b"SWIPE 360 800 360 400 5\n", "App Drawer opened")
                            print("[TEST STEP 16] Tapping the home pill to leave the App Drawer...")
                            send_and_wait_for(b"TAP 360 1420\n", "Returned to Home from App Drawer")
                            print("[TEST STEP 17] Tapping the Files dock tile at (130, 1110) to launch...")
                            send_and_wait_for(b"TAP 130 1110\n", "Files UI presented to scanout buffer")
                            print("[TEST STEP 18] Tapping the first Files category row at (260, 720)...")
                            send_and_wait_for(b"TAP 260 720\n", "Category filter shortcut switched")
                            print("[TEST STEP 19] Tapping the home pill to exit Files back to Launcher...")
                            send_and_wait_for(b"TAP 360 1420\n", "Foreground application exited and was reaped")
                            print("[TEST STEP 20] Launching Gallery through the guest bridge...")
                            send_and_wait_for(b"APP_LAUNCH gallery\n", "Guest application launched id=gallery")
                            print("[TEST STEP 21] Tapping the Gallery capture button at (612, 58)...")
                            send_and_wait_for(b"TAP 612 58\n", "Screenshot saved to /storage/photos")
                            print("[TEST STEP 22] Tapping the Wallpapers filter chip at (200, 175) in Gallery...")
                            send_and_wait_for(b"TAP 200 175\n", "Gallery filter chip switched")
                            print("[TEST STEP 23] Tapping the home pill to exit Gallery...")
                            send_and_wait_for(b"TAP 360 1420\n", "[GALLERY] Home gesture pressed")
                            print("[TEST STEP 24] Launching Calculator through the guest bridge...")
                            send_and_wait_for(b"APP_LAUNCH calculator\n", "Guest application launched id=calculator")
                            print("[TEST STEP 25] Tapping digit '7' on Calculator keypad at (100, 680)...")
                            send_and_wait_for(b"TAP 100 680\n", "Keypad digit pressed")
                            print("[TEST STEP 26] Tapping the home pill to exit Calculator...")
                            send_and_wait_for(b"TAP 360 1420\n", "[CALCULATOR] Home gesture tapped")
                            print("[TEST STEP 27] Launching Dialer through the guest bridge...")
                            send_and_wait_for(b"APP_LAUNCH dialer\n", "Guest application launched id=dialer")
                            print("[TEST STEP 28] Tapping digit '1' on Dialer keypad at (140, 470)...")
                            send_and_wait_for(b"TAP 140 470\n", "Keypad digit entered")
                            print("[TEST STEP 29] Tapping Call button at (360, 1060)...")
                            send_and_wait_for(b"TAP 360 1060\n", "Call request rejected by telephony service")
                            print("[TEST STEP 30] Tapping the home pill to exit Dialer...")
                            send_and_wait_for(b"TAP 360 1420\n", "[DIALER APP] Home gesture pressed")
                            print("[TEST COMPLETE] All interactive gesture steps completed.")
                            lines.append("[TEST COMPLETE]")
                            break
                if any("[TEST COMPLETE]" in l for l in lines) or any("All interactive gesture steps completed" in l for l in lines):
                    break
            except socket.timeout:
                continue
except AssertionError as failure:
    # WHY the AssertionError is kept rather than printed and dropped: the walkthrough
    # steps assert on the guest log, and this handler reported their failure as a socket
    # error and exited 0, so the suite passed while every step after the first missed
    # tap had never run.
    walkthrough_failure = str(failure)

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
    if any(k in l for k in ["[TEST]", "SDHCI", "Goodix", "SimpleFB", "PANIC", "assertion", "[TEST STEP", "[TEST COMPLETE"]):
        print(l)

# WHY the walkthrough decides the exit code: reaching the end of the serial loop is not
# proof the taps landed, and a suite that prints its report and exits 0 while half its
# steps timed out is worse than one that never ran.
if walkthrough_failure:
    raise SystemExit(f"Interactive walkthrough failed: {walkthrough_failure}")
if not any("[TEST COMPLETE]" in l for l in lines):
    raise SystemExit("Interactive walkthrough never reached its final step")
