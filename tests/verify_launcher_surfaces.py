"""Boots the QEMU guest, opens the launcher's search and the task switcher, and
writes a screendump of each.

WHY this exists: the contract suite checks the source; this checks the pixels.
The search surface and the task switcher are new surfaces, and a surface that
compiles is not a surface that draws.
"""
import os
import socket
import subprocess
import sys
import time
import zlib

sys.path.insert(0, os.path.join(os.path.dirname(__file__)))
from test_interactive_lifecycle import SerialReader, send_monitor_cmd, find_qemu, kill_proc_on_port


def ppm_to_png(src, dst, max_width=540):
    with open(src, "rb") as handle:
        data = handle.read()
    fields = []
    index = 0
    while len(fields) < 4:
        while data[index:index + 1].isspace():
            index += 1
        if data[index:index + 1] == b"#":
            while data[index:index + 1] not in (b"\n", b""):
                index += 1
            continue
        start = index
        while not data[index:index + 1].isspace():
            index += 1
        fields.append(data[start:index])
    index += 1
    width, height, maxval = int(fields[1]), int(fields[2]), int(fields[3])
    pixels = data[index:index + width * height * 3]
    step = 1
    if width > max_width:
        step = width // max_width
    out_w, out_h = width // step, height // step
    raw = bytearray()
    for y in range(0, height, step):
        raw.append(0)
        row = y * width * 3
        for x in range(0, width, step):
            at = row + x * 3
            raw.extend(pixels[at:at + 3])
    def chunk(tag, payload):
        return (len(payload).to_bytes(4, "big") + tag + payload
                + (zlib.crc32(tag + payload) & 0xFFFFFFFF).to_bytes(4, "big"))
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", out_w.to_bytes(4, "big") + out_h.to_bytes(4, "big") + bytes([8, 2, 0, 0, 0]))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 6))
    png += chunk(b"IEND", b"")
    with open(dst, "wb") as handle:
        handle.write(png)
    print(f"[VERIFY] {src} ({width}x{height}) -> {dst} ({out_w}x{out_h})")


def main():
    qemu_exe = find_qemu("qemu-system-x86_64.exe")
    kernel_img = os.path.abspath(r"target\inter-os-bios.img")
    assert os.path.exists(kernel_img), "no boot image"
    data_disk = os.path.abspath(r"target\inter-os-data-master.img")
    if not os.path.exists(data_disk):
        with open(data_disk, "wb") as handle:
            handle.truncate(512 * 1024 * 1024)

    ser_port, mon_port = 5571, 5572
    kill_proc_on_port(ser_port)
    kill_proc_on_port(mon_port)
    time.sleep(0.5)
    args = [
        qemu_exe, "-machine", "q35", "-cpu", "qemu64",
        "-device", "piix3-ide,id=ide",
        "-drive", f"file={kernel_img},format=raw,if=none,id=bootd,snapshot=on",
        "-device", "ide-hd,bus=ide.0,unit=0,drive=bootd,bootindex=1",
        # WHY the snapshot: this suite boots the shared data disk, and an
        # application that writes during the run used to leave its marks on the
        # master image every device starts from. The overlay keeps the run
        # hermetic and the master image as it was built.
        "-drive", f"file={data_disk},format=raw,if=none,id=datad,snapshot=on",
        "-device", "ide-hd,bus=ide.0,unit=1,drive=datad",
        "-m", "1024M", "-display", "none",
        "-chardev", f"socket,id=ser0,host=127.0.0.1,port={ser_port},server=on,wait=off",
        "-serial", "chardev:ser0",
        "-chardev", f"socket,id=mon0,host=127.0.0.1,port={mon_port},server=on,wait=off",
        "-mon", "chardev=mon0,mode=readline", "-no-reboot",
    ]
    print("[VERIFY] Launching QEMU...")
    print(f"[VERIFY] qemu: {qemu_exe}")
    proc = subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    serial_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    serial_sock.settimeout(5.0)
    connected = False
    for _ in range(40):
        try:
            serial_sock.connect(("127.0.0.1", ser_port))
            connected = True
            break
        except OSError:
            if proc.poll() is not None:
                out = proc.stdout.read().decode("utf-8", errors="replace") if proc.stdout else ""
                raise AssertionError(f"QEMU exited with {proc.returncode}: {out[:800]}")
            time.sleep(0.5)
    assert connected, "the guest's serial socket never opened"
    serial_sock.settimeout(0.5)
    reader = SerialReader(serial_sock)
    monitor_sock = None
    try:
        monitor_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        monitor_sock.settimeout(5.0)
        monitor_sock.connect(("127.0.0.1", mon_port))
        monitor_sock.settimeout(0.5)
    except OSError:
        monitor_sock = None

    try:
        assert reader.wait_for("Lock surface rendered.", timeout=240.0,
                                print_keywords=["LAUNCHER", "BOOT"]), "no lock screen"
        print("[VERIFY] Lock screen presented.")
        time.sleep(2.0)
        reader.send(b"SWIPE 360 1200 360 300 5\n")
        assert reader.wait_for("Home surface rendered.", timeout=180.0), "no home screen"
        print("[VERIFY] Unlocked.")

        # A running app first, so the task switcher has a card to show.
        reader.send(b"TAP 127 900\n")
        reader.wait_for("Launched application", timeout=180.0)
        time.sleep(30.0)
        # The app's own surface owns the bottom of the screen, so the way back
        # is the app's own exit gesture.
        reader.send(b"SWIPE 360 1420 360 1100 5\n")
        reader.wait_for("Home surface rendered.", timeout=180.0)
        time.sleep(3.0)

        # The task switcher.
        reader.send(b"TAP 660 1410\n")
        switched = reader.wait_for("Recent apps surface rendered.", timeout=180.0,
                                   print_keywords=["LAUNCHER", "render_overview"])
        print(f"[VERIFY] task switcher rendered: {switched}")
        time.sleep(30.0)
        ppm2 = os.path.abspath(r"target\verify_recents.ppm").replace("\\", "/")
        send_monitor_cmd(monitor_sock, f"screendump {ppm2}")
        time.sleep(3.0)
        if os.path.exists(ppm2):
            ppm_to_png(ppm2, os.path.abspath(r"target\verify_recents.png"))

        # Back to the home surface, then into the search.
        reader.send(b"TAP 360 1410\n")
        reader.wait_for("Home surface rendered.", timeout=180.0)
        time.sleep(3.0)
        reader.send(b"TAP 360 700\n")
        opened = reader.wait_for("Search surface rendered.", timeout=180.0,
                                 print_keywords=["LAUNCHER", "render_search"])
        print(f"[VERIFY] search surface rendered: {opened}")
        time.sleep(6.0)
        ppm = os.path.abspath(r"target\verify_search.ppm").replace("\\", "/")
        send_monitor_cmd(monitor_sock, f"screendump {ppm}")
        time.sleep(3.0)
        if os.path.exists(ppm):
            ppm_to_png(ppm, os.path.abspath(r"target\verify_search.png"))

        # Type a query with the keyboard and capture the ranked results.
        for key in ("s", "e", "t", "t", "i", "n", "g", "s"):
            send_monitor_cmd(monitor_sock, f"sendkey {key}")
            time.sleep(1.5)
        time.sleep(30.0)
        ppm_query = os.path.abspath(r"target\verify_search_query.ppm").replace("\\", "/")
        send_monitor_cmd(monitor_sock, f"screendump {ppm_query}")
        time.sleep(3.0)
        if os.path.exists(ppm_query):
            ppm_to_png(ppm_query, os.path.abspath(r"target\verify_search_query.png"))
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
        if monitor_sock:
            monitor_sock.close()
        serial_sock.close()


if __name__ == "__main__":
    main()
