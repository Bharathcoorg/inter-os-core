"""
InterOS Core — x86_64 QEMU Boot & Milestone Verification

Verifies that the x86_64 microkernel and userspace stack boots in QEMU,
reaches every mandatory boot milestone, spawns PID 1, initializes the
compositor/launcher, and produces an authentic framebuffer screendump.
"""

import os
import sys
import time
import socket
import subprocess
import re

from test_interactive_lifecycle import find_qemu, SerialReader, send_monitor_cmd

WORKSPACE_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

def main():
    qemu_exe = find_qemu("qemu-system-x86_64.exe")
    kernel_img = os.path.abspath(r"target\inter-os-bios.img")
    ppm_path = os.path.abspath(r"target\mobile_screen2.ppm")
    log_path = os.path.abspath(r"target\x86_boot.log")

    if not os.path.exists(kernel_img):
        print(f"[FAIL] Bootable image missing: {kernel_img}")
        sys.exit(1)

    for p in [ppm_path, log_path]:
        if os.path.exists(p):
            try: os.remove(p)
            except Exception: pass

    if sys.platform == "win32":
        subprocess.run(["taskkill", "/F", "/IM", "qemu-system-x86_64.exe"], capture_output=True)
    time.sleep(1.0)

    ser_port = 5555
    mon_port = 5556

    args = [
        qemu_exe,
        "-drive", f"format=raw,file={kernel_img}",
        "-snapshot",
        "-m", "1024M",
        "-display", "none",
        "-chardev", f"socket,id=ser0,host=127.0.0.1,port={ser_port},server=on,wait=off",
        "-serial", "chardev:ser0",
        "-chardev", f"socket,id=mon0,host=127.0.0.1,port={mon_port},server=on,wait=off",
        "-mon", "chardev=mon0,mode=readline",
        "-no-reboot"
    ]

    print("============================================================")
    print("  InterOS Core — x86_64 QEMU Canonical Boot Verification")
    print("============================================================")
    print(f"QEMU Executable: {qemu_exe}")
    print(f"Disk Image:      {kernel_img}")
    print("Launching guest virtual machine...")
    sys._run_start_time = time.time()

    proc = subprocess.Popen(args)
    time.sleep(0.5)

    serial_sock = None
    monitor_sock = None

    try:
        serial_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        serial_sock.settimeout(5.0)
        serial_sock.connect(('127.0.0.1', ser_port))
        serial_sock.settimeout(0.5)
        print("[BOOT TEST] Connected to QEMU serial socket.")
        reader = SerialReader(serial_sock)

        try:
            monitor_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            monitor_sock.settimeout(5.0)
            monitor_sock.connect(('127.0.0.1', mon_port))
            monitor_sock.settimeout(0.5)
            print("[BOOT TEST] Connected to QEMU monitor socket.")
        except Exception as e:
            print(f"[BOOT TEST] Warning: Monitor socket connection failed: {e}")
            monitor_sock = None

        print("[BOOT TEST] Awaiting Launcher homescreen presentation...")
        launcher_ready = reader.wait_for(
            "Step 5: Initial presentation complete",
            timeout=90.0,
            print_keywords=["BOOT", "GDT", "Page", "Heap", "VFS", "Userspace", "LAUNCHER", "PID1", "Step 5"]
        )

        with open(log_path, "w", encoding="utf-8") as f:
            for l in reader.captured_logs:
                f.write(l + "\n")

        if monitor_sock:
            ppm_arg = ppm_path.replace("\\", "/")
            _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_arg}")
            time.sleep(0.5)

        full_log = "\n".join(reader.captured_logs)
        milestones = [
            ("GDT & TSS Initialization", r"GDT & TSS loaded|Creating GDT"),
            ("Page Tables & Memory Setup", r"Page tables and physical frame allocator initialized|Kernel PML4 entry"),
            ("Physical Frame Allocator", r"Page tables and physical frame allocator initialized"),
            ("Microkernel Memory/Hardware", r"Serial COM1 online|Hardware Drivers, Memory Management"),
            ("Persistent VFS Storage", r"Persistent storage encryption root initialized|VFS"),
            ("PID1 Userspace Service Manager", r"PID1 userspace service manager spawned|userspace init started in Ring 3"),
            ("Launcher / UI Canvas Online", r"render_full complete|home screen canvas initialized|Step 5: Initial presentation complete"),
        ]

        all_passed = True
        print("\n--- [Verifying Invariants & Milestones] ---")
        for name, pattern in milestones:
            if re.search(pattern, full_log):
                print(f"  [OK] {name}")
            else:
                print(f"  [FAIL] Missing required milestone: {name}")
                all_passed = False

        run_start_time = getattr(sys, "_run_start_time", 0)
        if os.path.exists(ppm_path):
            mtime = os.path.getmtime(ppm_path)
            if run_start_time > 0 and mtime < run_start_time - 1.0:
                print(f"  [FAIL] Stale framebuffer artifact detected: {ppm_path} (mtime={mtime} < run_start={run_start_time})")
                all_passed = False
            else:
                sz = os.path.getsize(ppm_path)
                print(f"  [OK] Framebuffer PPM verified for current run: {ppm_path} ({sz} bytes, mtime={mtime})")
                if sz < 1000000:
                    print(f"  [FAIL] Framebuffer PPM is abnormally small ({sz} bytes)")
                    all_passed = False
        else:
            print(f"  [FAIL] Framebuffer PPM not found at {ppm_path}")
            all_passed = False

        if not launcher_ready or not all_passed:
            print("\n============================================================")
            print("  [FAIL] x86_64 QEMU boot verification failed invariants.")
            print("============================================================")
            sys.exit(1)

        print("\n============================================================")
        print("  [SUCCESS] x86_64 QEMU boot verification PASSED (100%)")
        print("============================================================")
        sys.exit(0)

    finally:
        if serial_sock:
            try: serial_sock.close()
            except Exception: pass
        if monitor_sock:
            try:
                monitor_sock.sendall(b"quit\n")
                monitor_sock.close()
            except Exception: pass
        if proc.poll() is None:
            try: proc.kill()
            except Exception: pass

if __name__ == "__main__":
    main()
