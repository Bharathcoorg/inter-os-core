"""
InterOS Core — x86_64 QEMU Boot & Contract Verification

Verifies that the x86_64 microkernel and userspace stack boots in QEMU,
reaches every mandatory boot invariant, spawns PID 1, initializes the
compositor/launcher, and produces an authentic framebuffer screendump.
"""

import os
import sys
import time
import socket
import subprocess
import re
import collections

from test_interactive_lifecycle import find_qemu, SerialReader, send_monitor_cmd

# The canonical portrait scanout the KMS declares (kernel/src/drivers/display_kms.rs).
# A screendump that is not this geometry did not come from this display path.
EXPECTED_SCANOUT = (720, 1440)

# Sample every Nth pixel. A full 1,04 M-pixel pass in Python costs seconds per
# boot for statistics that a 7x sample resolves just as well.
PPM_SAMPLE_STRIDE = 7

# A real launcher frame measured 98.30% one background colour and 48 distinct
# colours. These thresholds sit well clear of that on both sides so they
# describe a rendered screen rather than today's exact artwork: a blank or
# single-colour frame scores 0.0% non-dominant and 1 colour, while uninitialised
# memory scores a colour count in the tens of thousands.
MIN_NON_DOMINANT_FRACTION = 0.005   # 0.5%; measured 1.70%
MIN_DISTINCT_COLOURS = 8            # measured 48
MAX_DISTINCT_COLOURS = 20000        # noise / uninitialised memory


def read_ppm(path):
    """Parse a binary P6 PPM. Returns (width, height, pixel_bytes) or raises."""
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
        raise ValueError("expected a binary P6 PPM, got %r" % magic)
    if maxval != 255:
        raise ValueError("expected maxval 255, got %d" % maxval)
    return width, height, raw[i:]


def inspect_framebuffer(ppm_path):
    """Look at the screendump's pixels. Returns (ok, stats, problems).

    WHY this exists: the previous check only proved the file was fresh and
    larger than a megabyte. A black screen, a one-colour fill or a buffer of
    uninitialised memory all satisfy that, so the display path could regress
    with every test still green. The numbers this reports come from a real
    launcher frame measured on this host.
    """
    problems = []
    try:
        width, height, pixels = read_ppm(ppm_path)
    except (OSError, ValueError) as exc:
        return False, {}, ["could not parse the screendump: %s" % exc]

    if (width, height) != EXPECTED_SCANOUT:
        problems.append("geometry is %dx%d, expected the %dx%d scanout"
                        % (width, height, EXPECTED_SCANOUT[0], EXPECTED_SCANOUT[1]))

    expected_bytes = width * height * 3
    if len(pixels) < expected_bytes:
        problems.append("truncated: %d pixel bytes for %dx%d, expected %d"
                        % (len(pixels), width, height, expected_bytes))
        return False, {"width": width, "height": height}, problems

    step = PPM_SAMPLE_STRIDE * 3
    sample = pixels[:expected_bytes:step]
    red, green, blue = sample[0::3], sample[1::3], sample[2::3]
    counts = collections.Counter(zip(red, green, blue))
    sampled = len(red)
    if sampled == 0:
        return False, {"width": width, "height": height}, ["no pixels could be sampled"]

    distinct = len(counts)
    dominant_rgb, dominant_n = counts.most_common(1)[0]
    non_dominant = sampled - dominant_n
    non_dominant_fraction = non_dominant / sampled
    black = sum(n for c, n in counts.items() if c == (0, 0, 0))

    stats = {
        "width": width,
        "height": height,
        "sampled": sampled,
        "distinct_colours": distinct,
        "dominant_rgb": "#%02X%02X%02X" % dominant_rgb,
        "dominant_fraction": round(dominant_n / sampled, 4),
        "non_dominant_fraction": round(non_dominant_fraction, 4),
        "black_fraction": round(black / sampled, 4),
    }

    if distinct < MIN_DISTINCT_COLOURS:
        problems.append("only %d distinct colours; a rendered launcher frame has at least %d "
                        "(a blank or single-colour screen would pass every other check)"
                        % (distinct, MIN_DISTINCT_COLOURS))
    if distinct > MAX_DISTINCT_COLOURS:
        problems.append("%d distinct colours looks like uninitialised memory, not a rendered frame"
                        % distinct)
    if non_dominant_fraction < MIN_NON_DOMINANT_FRACTION:
        problems.append("only %.2f%% of sampled pixels differ from the dominant colour; "
                        "a filled or blank screen would look like this"
                        % (100.0 * non_dominant_fraction))
    if black == sampled:
        problems.append("every sampled pixel is pure black")

    return (not problems), stats, problems


def kill_proc_on_port(p):
    try:
        import psutil
        for conn in psutil.net_connections():
            if conn.laddr and conn.laddr.port == p and conn.pid:
                try:
                    psutil.Process(conn.pid).kill()
                except Exception:
                    pass
    except Exception:
        pass
    if sys.platform == "win32":
        try:
            res = subprocess.run(["powershell", "-NoProfile", "-Command", f"(Get-NetTCPConnection -LocalPort {p} -ErrorAction SilentlyContinue).OwningProcess"], capture_output=True, text=True, timeout=5)
            for line in res.stdout.splitlines():
                pid = line.strip()
                if pid and pid.isdigit() and int(pid) > 0:
                    subprocess.run(["taskkill", "/F", "/PID", pid], capture_output=True, timeout=3)
        except Exception:
            pass



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

    ser_port = 5555
    mon_port = 5556
    kill_proc_on_port(ser_port)
    kill_proc_on_port(mon_port)
    time.sleep(0.5)

    data_disk = os.environ.get("DATA_DISK_PATH", os.path.abspath(r"target\inter-os-data-master.img"))
    if not os.path.exists(data_disk):
        with open(data_disk, "wb") as f:
            f.truncate(512 * 1024 * 1024)

    args = [
        qemu_exe,
        "-machine", "q35",
        "-cpu", "qemu64",
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

    print("============================================================")
    print("  InterOS Core — x86_64 QEMU Canonical Boot Verification")
    print("============================================================")
    print(f"QEMU Executable: {qemu_exe}")
    print(f"Disk Image:      {kernel_img}")
    print("Launching guest virtual machine...")
    sys._run_start_time = time.time()

    proc = subprocess.Popen(args)
    time.sleep(0.1)

    serial_sock = None
    monitor_sock = None

    try:
        for _ in range(40):
            try:
                serial_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                serial_sock.connect(('127.0.0.1', ser_port))
                serial_sock.settimeout(0.5)
                break
            except (ConnectionRefusedError, OSError):
                time.sleep(0.05)

        if not serial_sock:
            raise RuntimeError("Failed to connect to QEMU serial socket after retries")

        print("[BOOT TEST] Connected to QEMU serial socket.")
        reader = SerialReader(serial_sock)

        for _ in range(20):
            try:
                monitor_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                monitor_sock.connect(('127.0.0.1', mon_port))
                monitor_sock.settimeout(5.0)
                print("[BOOT TEST] Connected to QEMU monitor socket.")
                break
            except Exception as e:
                time.sleep(0.05)

        print("[BOOT TEST] Awaiting Launcher homescreen presentation...")
        launcher_ready = reader.wait_for(
            "Step 5: Initial presentation complete",
            timeout=180.0,
            print_keywords=["BOOT", "GDT", "Page", "Heap", "VFS", "Userspace", "LAUNCHER", "PID1", "Step 5"]
        )

        # Wait for boot animation to finish and unlock to HomeScreen
        lock_ready = reader.wait_for("Boot presentation complete; lock screen is now guest-owned", timeout=20.0)
        if lock_ready:
            time.sleep(0.5)
            reader.send(b"SWIPE 360 1350 360 300 5\n")
            _ = reader.wait_for("Home surface rendered", timeout=15.0)
            time.sleep(0.5)

        with open(log_path, "w", encoding="utf-8") as f:
            for l in reader.captured_logs:
                f.write(l + "\n")

        if monitor_sock:
            ppm_arg = ppm_path.replace("\\", "/")
            _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_arg}")
            # WHY this waits for the write to finish instead of sleeping a
            # fixed interval: a 720x1440 P6 dump is ~3.1 MB, and QEMU writes it
            # from the monitor command's completion path. A 0.5 s sleep read a
            # partially written file and reported it as an "abnormally small
            # framebuffer", which failed the boot check even though the guest
            # had presented its homescreen. Poll until the size reaches the
            # expected payload and stops changing.
            expected_ppm = EXPECTED_SCANOUT[0] * EXPECTED_SCANOUT[1] * 3
            deadline = time.time() + 20.0
            last_size = -1
            while time.time() < deadline:
                if os.path.exists(ppm_path):
                    size = os.path.getsize(ppm_path)
                    if size >= expected_ppm and size == last_size:
                        break
                    last_size = size
                time.sleep(0.2)

        full_log = "\n".join(reader.captured_logs)
        boot_invariants = [
            ("GDT & TSS Initialization", r"GDT & TSS loaded|Creating GDT"),
            ("Page Tables & Memory Setup", r"Page tables and physical frame allocator initialized|Kernel PML4 entry"),
            ("Physical Frame Allocator", r"Page tables and physical frame allocator initialized"),
            ("Microkernel Memory/Hardware", r"Serial COM1 online|Hardware Drivers, Memory Management"),
            ("Persistent VFS Storage", r"Persistent storage encryption root initialized|VFS"),
            ("PID1 Userspace Service Manager", r"PID1 userspace service manager spawned|userspace init started in Ring 3"),
            ("Launcher / UI Canvas Online", r"render_full complete|home screen canvas initialized|Step 5: Initial presentation complete"),
        ]

        all_passed = True
        print("\n--- [Verifying Kernel & Userspace Boot Invariants] ---")
        for name, pattern in boot_invariants:
            if re.search(pattern, full_log):
                print(f"  [OK] {name}")
            else:
                print(f"  [FAIL] Missing required boot invariant: {name}")
                all_passed = False

        run_start_time = getattr(sys, "_run_start_time", 0)
        if os.path.exists(ppm_path):
            mtime = os.path.getmtime(ppm_path)
            if run_start_time > 0 and mtime < run_start_time - 1.0:
                print(f"  [FAIL] Stale framebuffer artifact detected: {ppm_path} (mtime={mtime} < run_start={run_start_time})")
                all_passed = False
            else:
                sz = os.path.getsize(ppm_path)
                if sz < 1000000:
                    print(f"  [FAIL] Framebuffer PPM is abnormally small ({sz} bytes)")
                    all_passed = False
                else:
                    # WHY pixels are inspected and not just the file size: a black
                    # screen, a single-colour fill and a buffer of uninitialised
                    # memory all satisfy a freshness-and-size check. The display
                    # path could regress with every other test still green, which
                    # is exactly the hole this closes.
                    fb_ok, fb_stats, fb_problems = inspect_framebuffer(ppm_path)
                    if fb_ok:
                        print("  [OK] Framebuffer pixels verified: %dx%d, %d distinct colours, "
                              "dominant %s at %.1f%%, %.2f%% non-dominant (sampled %d px)"
                              % (fb_stats["width"], fb_stats["height"],
                                 fb_stats["distinct_colours"], fb_stats["dominant_rgb"],
                                 100.0 * fb_stats["dominant_fraction"],
                                 100.0 * fb_stats["non_dominant_fraction"],
                                 fb_stats["sampled"]))
                    else:
                        print(f"  [FAIL] Framebuffer is not a rendered frame ({sz} bytes):")
                        for problem in fb_problems:
                            print(f"         - {problem}")
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
