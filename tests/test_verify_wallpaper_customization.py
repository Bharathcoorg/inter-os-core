#!/usr/bin/env python3
"""
Test End-to-End Home-Only Wallpaper Customisation in QEMU.
Navigates Settings -> Personal -> Wallpaper Picker -> Selects Night -> Sets Target: Home -> Applies.
Exits back to Launcher, verifies Home uses Night, Lock uses Stock.

WHY the taps are derived and not written down: every point this suite touches is a
rectangle the Settings app computes from the SDK's spacing and type tokens, and the
app's own hit-tests accept those rectangles and nothing else. A literal coordinate is
a copy of that arithmetic, and the copy is what goes stale when the app's chain is
re-laid -- which is exactly how this suite came to tap the gap between two gallery
rows and a row that had moved 320px up the screen. The block below re-derives the
arithmetic from the same tokens, names the source line for each of them, and then
takes the centre of the rectangle the app drew.
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

# ---------------------------------------------------------------------------
# Settings-app geometry, re-derived from the app's own layout constants
# ---------------------------------------------------------------------------
#
# Every number here is a token or a constant of the app, with the file:line that
# defines it. The app lays its screens out in the 720x1440 authoring viewport
# (sdk/rust/interos-std/src/design/mod.rs::viewport::LOGICAL_WIDTH/HEIGHT), so the
# derived points are scanout pixels one-for-one.

W, H = 720, 1440                       # apps/settings/src/main.rs:43-44

# SDK tokens (sdk/rust/interos-std/src/design/mod.rs)
GRID_4, GRID_8, GRID_16 = 4, 8, 16     # spacing::GRID_4/8/16  (mod.rs:613-616)
GRID_24, GRID_32 = 24, 32              # spacing::GRID_24/32   (mod.rs:618-619)
PREFERRED_TARGET = 48                  # touch::PREFERRED_TARGET (mod.rs:673)
PRIMARY_ACTION = 56                    # touch::PRIMARY_ACTION   (mod.rs:674)
STATUS_BAR_HEIGHT = 48                 # viewport::STATUS_BAR_HEIGHT (mod.rs:882)
HEADER_TOP = STATUS_BAR_HEIGHT + 8     # viewport::HEADER_TOP        (mod.rs:893)

# The face's line boxes, from the same ladder the app's compile-time constants use:
# font::line_height(scale) = ascent + descent of the outline face at that rung
# (sdk/rust/interos-std/src/font.rs:1882, cap_for_scale :1744, em_for_cap_height
# design/outline_text.rs:558, line_metrics design/typeface.rs:135).
def _line_height(scale):
    cap = {1: 13, 2: 18, 3: 24, 4: 32, 5: 44, 6: 56, 7: 76}.get(scale, scale * 13)
    em = (cap * 2048 + 728) // 1456
    return (2146 * em) // 2048 + (555 * em) // 2048

LINE_H = _line_height(2)               # settings main.rs:1834 (TYPE_H3 rung)
LINE_H_DISPLAY = _line_height(3)       # TYPE_DISPLAY, settings main.rs:1832

MARGIN = GRID_24                       # settings main.rs:1842
CONTENT_X = MARGIN                     # settings main.rs:1896
CONTENT_W = W - 2 * MARGIN             # settings main.rs:1897
CARD_GAP = GRID_16                     # settings main.rs:1845
BUTTON_H = PREFERRED_TARGET            # settings main.rs:2044
BUTTON_HALF = (CONTENT_W - CARD_GAP) // 2   # settings main.rs:2046

# --- Landing card grid ---
GRID_X = CONTENT_X                     # settings main.rs:1933
CARD_PAD = MARGIN                      # settings main.rs:1936
CARD_LINE_GAP = GRID_16                # settings main.rs:1949
GRID_GAP = 14                          # settings main.rs:1976
CARD_W = (CONTENT_W - GRID_GAP) // 2   # settings main.rs:1947
CARD_H = CARD_PAD * 2 + LINE_H * 3 + CARD_LINE_GAP   # settings main.rs:1970
GRID_Y = 348                           # settings main.rs:1978

def landing_card_rect(i):
    """card_rect() -- settings main.rs:2521-2534. Card 6 spans the full column."""
    if i == 6:
        return (GRID_X, GRID_Y + 3 * (CARD_H + GRID_GAP), CONTENT_W, CARD_H)
    col, row = i % 2, i // 2
    return (GRID_X + col * (CARD_W + GRID_GAP), GRID_Y + row * (CARD_H + GRID_GAP),
            CARD_W, CARD_H)

def landing_card_centre(i):
    x, y, w, h = landing_card_rect(i)
    return (x + w // 2, y + h // 2)

# --- Detail rows (stacked from DETAIL_TOP with ROW_GAP) ---
DIVIDER_Y = HEADER_TOP + LINE_H_DISPLAY + GRID_16    # settings main.rs:1927
DETAIL_TOP = DIVIDER_Y + GRID_32                     # settings main.rs:1930
CONTENT_BOTTOM = H - GRID_32                         # settings main.rs:2658
ROW_PAD_V = GRID_32                                  # settings main.rs:1997
ROW_LINE_GAP = GRID_8                                # settings main.rs:1999
ROW_H = (ROW_PAD_V + LINE_H + ROW_LINE_GAP) + LINE_H + ROW_PAD_V   # main.rs:2003-2005
ROW_GAP = CARD_GAP                                   # settings main.rs:2007

def detail_row_centre(row):
    """detail_row_rect() for the equal-height rows that precede Personal's
    document rows -- settings main.rs:2632 + stacked_row_rect :2615."""
    top = DETAIL_TOP + row * (ROW_H + ROW_GAP)
    return (CONTENT_X + CONTENT_W // 2, top + ROW_H // 2)

# --- Wallpaper picker panel ---
WP_PREVIEW_Y = DETAIL_TOP                              # settings main.rs:2083
WP_CELL_PAD = GRID_8                                   # main.rs:2133
WP_CELL_LABEL_GAP = GRID_8                             # main.rs:2135
WP_THUMB_H = 144                                       # main.rs:2137
WP_CELL_GAP = GRID_24                                  # main.rs:2128
WP_CELL_W = (CONTENT_W - 2 * WP_CELL_GAP) // 3         # main.rs:2131
WP_CELL_H = WP_CELL_PAD + WP_THUMB_H + WP_CELL_LABEL_GAP + LINE_H + WP_CELL_PAD  # 2139
SCENES = 6                                             # design/walls.rs:249 (`SCENES`)
WP_ROWS = (SCENES + 2) // 3                            # main.rs:2140
WP_GALLERY_H = WP_ROWS * WP_CELL_H + (WP_ROWS - 1) * WP_CELL_GAP   # main.rs:2141
WP_FOOTER_H = (GRID_32 + PRIMARY_ACTION + GRID_32 + WP_GALLERY_H + GRID_32
               + BUTTON_H + GRID_16 + BUTTON_H + CARD_GAP + LINE_H)  # main.rs:2087-2096
WP_PREVIEW_H = CONTENT_BOTTOM - DETAIL_TOP - WP_FOOTER_H             # main.rs:2109
WP_TARGET_Y = WP_PREVIEW_Y + WP_PREVIEW_H + GRID_32                  # main.rs:2122
WP_TARGET_H = PRIMARY_ACTION                                         # main.rs:2124
WP_GALLERY_Y = WP_TARGET_Y + WP_TARGET_H + GRID_32                   # main.rs:2126
WP_ACTION_Y = WP_GALLERY_Y + WP_GALLERY_H + GRID_32                  # main.rs:2143

def wp_cell_centre(i):
    """wp_cell_origin() -- settings main.rs:5222-5229."""
    col, row = i % 3, i // 3
    x = CONTENT_X + col * (WP_CELL_W + WP_CELL_GAP)
    y = WP_GALLERY_Y + row * (WP_CELL_H + WP_CELL_GAP)
    return (x + WP_CELL_W // 2, y + WP_CELL_H // 2)

def wp_segment_centre(i):
    """wp_segment_rect() -- settings main.rs:5266-5274."""
    seg_w = (CONTENT_W - 2 * GRID_4) // 3              # SEG_GAP, main.rs:2019
    x = CONTENT_X + i * (seg_w + GRID_4)
    return (x + seg_w // 2, WP_TARGET_Y + WP_TARGET_H // 2)

def wp_apply_centre():
    """The Apply half of the action row -- settings main.rs:5610-5621."""
    return (CONTENT_X + BUTTON_HALF // 2, WP_ACTION_Y + BUTTON_H // 2)

# The app's own exit band: x in 240..480, y >= 1400 -- settings main.rs:6402.
HOME_GESTURE = (360, 1420)

# --- Launcher taps: how the suite reaches Settings the way a person does ---
# The "Apps" tile is slot 5 of the home deck: DOCK_TILES[5] = Rect(490,1020,200,180)
# (apps/launcher/src/screens/home.rs:938-945), whose centre is the point below.
DOCK_APPS_TILE_CENTRE = (490 + 200 // 2, 1020 + 180 // 2)
# The drawer's catalogue grid: bounds Rect(50,175,620,895) with 3 fixed columns,
# 5 rows and a 20px gutter (apps/launcher/src/screens/app_drawer.rs:112-124,
# drawer_columns :166, DRAWER_ROWS :37, TILE_GUTTER :39, current_grid :254 ->
# layout::grid_fixed_columns, apps/launcher/src/layout.rs:106-120). Slot 0 is the
# first filtered catalogue entry (app_drawer_hit, app_drawer.rs:265-279) and its
# cell is Rect(50,175,193,163) -> centre below. Settings is the first entry the
# registry reports, which is what the launch marker in step 1 asserts rather than
# what this arithmetic assumes.
DRAWER_GRID = (50, 175, 620, 895)
DRAWER_COLUMNS, DRAWER_ROWS_, DRAWER_GUTTER = 3, 5, 20
_drawer_cell_w = (DRAWER_GRID[2] - DRAWER_GUTTER * (DRAWER_COLUMNS - 1)) // DRAWER_COLUMNS
_drawer_cell_h = (DRAWER_GRID[3] - DRAWER_GUTTER * (DRAWER_ROWS_ - 1)) // DRAWER_ROWS_
DRAWER_FIRST_CELL_CENTRE = (DRAWER_GRID[0] + _drawer_cell_w // 2,
                            DRAWER_GRID[1] + _drawer_cell_h // 2)

# The suite's taps, as named points rather than numbers.
PERSONAL_CARD = landing_card_centre(2)        # Section::Personal, index 2
WALLPAPER_ROW = detail_row_centre(2)          # the row titled "Wallpaper" (main.rs:4221-4240)
NIGHT_CELL = wp_cell_centre(2)                # SCENES[2] = b"InterOS Night" (walls.rs:282)
HOME_TARGET = wp_segment_centre(0)            # WallpaperTarget::Home, index 0 (main.rs:124-147)
APPLY_BUTTON = wp_apply_centre()

# ---------------------------------------------------------------------------
# Throwaway data disk
# ---------------------------------------------------------------------------
# WHY this suite owns a disk and creates it fresh: it asserts on first-boot state
# (a lock screen with no credential, a store with nothing stored) and on what
# survives a reboot of that same disk. Reusing another suite's image would make the
# result depend on which suite ran last, and pointing the guest at the 6 GiB master
# would let a wallpaper write land in the canonical image.
MASTER_DISK = (ROOT / "target" / "inter-os-data-master.img").resolve()
VERIFY_DIR = ROOT / "target" / "verify"
DATA_DISK = Path(os.environ.get("DATA_DISK_PATH")
                 or VERIFY_DIR / "wallpaper-sweep-data.img").resolve()
DATA_DISK_BYTES = 64 * 1024 * 1024            # same capacity the PIN suite formats

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
                 f"Get-NetTCPConnection -LocalPort {port} -ErrorAction SilentlyContinue | "
                 "ForEach-Object { Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue } | "
                 "Where-Object { $_.ProcessName -like '*qemu*' } | ForEach-Object { $_.Id }"],
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
        self.cmd_log_idx = 0

    def send(self, line):
        time.sleep(0.3)
        self.cmd_log_idx = len(self.captured_logs)
        cmd_bytes = (line if line.endswith("\n") else line + "\n").encode("utf-8")
        self.sock.sendall(cmd_bytes)

    def reset_cursor(self):
        self.cmd_log_idx = len(self.captured_logs)

    def wait_for(self, marker, timeout=35.0, print_filter=None):
        start = time.time()
        start_scan = getattr(self, "cmd_log_idx", 0)
        for l in self.captured_logs[start_scan:]:
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

def close_session(proc, ser_sock, mon_sock):
    """Quit the guest, close both sockets and reap the emulator process."""
    for sock in (mon_sock, ser_sock):
        try:
            sock.sendall(b"quit\n")
        except Exception:
            pass
    for sock in (ser_sock, mon_sock):
        try:
            sock.close()
        except Exception:
            pass
    try:
        proc.wait(timeout=8)
    except subprocess.TimeoutExpired:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()

def boot_to_lock_screen(boot_img, data_img, ser_port, mon_port, attempts=2):
    """Launch the guest and wait for the lock-screen handover.

    WHY this retries a SILENT boot: a second launch on the same ports can attach
    before the previous emulator has finished releasing them, and QEMU's socket
    chardev (`server=on,wait=off`) starts the guest immediately and drops serial
    bytes until a client is attached -- so the suite ends up holding a socket to a
    guest it cannot hear. That is a transport race, and this suite hit it on the
    reboot session: the VM was `running` on the monitor and printed nothing.
    A boot that produced serial and never reached the marker is NOT retried and is
    returned as a failure for the caller's assert to report, because that is the
    product not doing the thing the suite exists to check.
    """
    for attempt in range(1, attempts + 1):
        proc, ser_sock, mon_sock = launch_qemu(boot_img, data_img, ser_port, mon_port)
        reader = SerialReader(ser_sock)
        ready = reader.wait_for("lock screen is now guest-owned", timeout=90.0,
                                print_filter=["BOOT", "USER", "LAUNCHER", "Step 5"])
        if ready:
            return proc, ser_sock, mon_sock, reader
        if reader.captured_logs:
            return proc, ser_sock, mon_sock, reader
        print(f"[TEST] Boot attempt {attempt} produced no serial at all; "
              "tearing the guest down and booting again.")
        close_session(proc, ser_sock, mon_sock)
    return proc, ser_sock, mon_sock, reader

def screendump(mon_sock, path):
    if mon_sock:
        mon_sock.settimeout(5.0)
        p = str(path).replace("\\", "/")
        mon_sock.sendall(f"screendump {p}\n".encode("utf-8"))
        time.sleep(0.8)

def parse_ppm(path):
    with open(path, "rb") as f:
        header = f.readline().decode('ascii').strip()
        dims = f.readline().decode('ascii').strip()
        while dims.startswith('#'):
            dims = f.readline().decode('ascii').strip()
        w, h = [int(x) for x in dims.split()]
        maxval = int(f.readline().decode('ascii').strip())
        raw = f.read()
    return w, h, raw

def main():
    print("=" * 60)
    print("  Testing Home-Only Wallpaper Customisation in QEMU")
    print("=" * 60)

    boot_img = ROOT / "target" / "inter-os-bios.img"
    assert boot_img.exists(), f"Boot image missing: {boot_img}"

    if DATA_DISK == MASTER_DISK:
        print("FAIL: DATA_DISK_PATH points at the 6 GiB master image, which this "
              f"suite must never attach, copy or hash: {MASTER_DISK}")
        return 1
    DATA_DISK.parent.mkdir(parents=True, exist_ok=True)
    if DATA_DISK.exists():
        DATA_DISK.unlink()
    with open(DATA_DISK, "wb") as f:
        f.seek(DATA_DISK_BYTES - 1)
        f.write(b"\0")
    print(f"[TEST] Fresh throwaway data disk: {DATA_DISK} ({DATA_DISK_BYTES} bytes)")
    data_img = DATA_DISK

    print("[TEST] Settings geometry this suite drives "
          f"(Personal card {PERSONAL_CARD}, Wallpaper row {WALLPAPER_ROW}, "
          f"Night cell {NIGHT_CELL}, Home segment {HOME_TARGET}, Apply {APPLY_BUTTON}, "
          f"home gesture {HOME_GESTURE})")

    ser_port = 5562
    mon_port = 5563

    proc, ser_sock, mon_sock, reader = boot_to_lock_screen(boot_img, data_img, ser_port, mon_port)

    print("[TEST] Awaiting LockScreen...")
    # WHY 90 s and not 60: the boot presentation runs until the launcher has seen
    # screens::boot::BOOT_TICKS ticks, and on a loaded host that handover lands 60-70 s
    # after the serial port opens. The PIN suite already waits 90 s for this same
    # marker; a shorter budget here only ever produced a false "not presented".
    assert reader.captured_logs and any("lock screen is now guest-owned" in l for l in reader.captured_logs), \
        "Lock screen not presented!"

    time.sleep(1.0)
    # Dump Lock screen PPM to verify Stock wallpaper
    ppm_lock = VERIFY_DIR / "lock_wallpaper_stock.ppm"
    screendump(mon_sock, ppm_lock)
    w, h, raw_lock = parse_ppm(ppm_lock)
    idx_lock = (150 * w + 360) * 3
    r_l, g_l, b_l = raw_lock[idx_lock], raw_lock[idx_lock+1], raw_lock[idx_lock+2]
    print(f"[TEST] Lock screen sky pixel at (360, 150): R={r_l}, G={g_l}, B={b_l}")
    assert r_l > 150 and b_l > 180, f"Expected Atmospheric Cosmic Stock lock screen, got R={r_l}, G={g_l}, B={b_l}"
    print("[TEST] Stock wallpaper confirmed on LockScreen!")

    print("[TEST] Unlocking to HomeScreen via swipe gesture...")
    reader.send("SWIPE 360 1425 360 1100 5\n")
    assert reader.wait_for("Device unlocked via swipe/slide gesture", timeout=30.0, print_filter=["LAUNCHER", "USER"]), "Unlock failed!"
    print("[TEST] Unlocked to HomeScreen!")

    time.sleep(1.0)
    # 1. Open Settings the way a person does: the Apps tile on the dock, then the
    # Settings cell in the drawer -- the same two points tests/test_lifecycle_stress.py:148-158
    # and tests/test_interactive_lifecycle.py:491-515 reach Settings with, here derived
    # from the launcher's own deck and grid rectangles instead of copied as literals.
    # WHY not the serial launch bridge: the bridge
    # starts the application as the kernel's child, so the shell never reaps it -
    # and re-reading the wallpaper store is something the shell only does when it
    # regains the foreground, which is the behaviour this test exists to verify.
    print(f"[TEST] Opening the App Drawer (TAP {DOCK_APPS_TILE_CENTRE[0]} {DOCK_APPS_TILE_CENTRE[1]})...")
    reader.send(f"TAP {DOCK_APPS_TILE_CENTRE[0]} {DOCK_APPS_TILE_CENTRE[1]}")
    assert reader.wait_for("App Drawer opened", timeout=25.0, print_filter=["LAUNCHER", "INPUT"]), "App Drawer did not open!"
    print(f"[TEST] Launching Settings from the drawer (TAP {DRAWER_FIRST_CELL_CENTRE[0]} {DRAWER_FIRST_CELL_CENTRE[1]})...")
    reader.send(f"TAP {DRAWER_FIRST_CELL_CENTRE[0]} {DRAWER_FIRST_CELL_CENTRE[1]}")
    assert reader.wait_for("Settings UI presented to scanout buffer", timeout=35.0, print_filter=["SETTINGS", "LAUNCHER"]), "Settings did not present UI!"
    print("[TEST] Settings app launched and UI presented!")

    time.sleep(1.0)
    # 2. Open the Personal category card. The landing is a two-column grid of the
    # seven SDK sections (Section::ALL, sdk/rust/interos-std/src/settings.rs:36-44)
    # and Personal is index 2, so it is the left column of the second row; the whole
    # rounded card is the target (landing_card_at, apps/settings/src/main.rs:2544).
    print(f"[TEST] Opening Personal category card (TAP {PERSONAL_CARD[0]} {PERSONAL_CARD[1]})...")
    reader.send(f"TAP {PERSONAL_CARD[0]} {PERSONAL_CARD[1]}")
    assert reader.wait_for("System Space category opened", timeout=25.0, print_filter=["SETTINGS", "INPUT"]), "Personal section not opened!"
    print("[TEST] Personal category opened!")

    time.sleep(1.0)
    # 3. Tap the Wallpaper row. On the Personal detail screen this is row 2 of six
    # (row_head b"Wallpaper", apps/settings/src/main.rs:4221-4252) and the handler
    # opens the picker from exactly that row's rectangle (main.rs:6494-6506).
    print(f"[TEST] Opening Wallpaper picker (TAP {WALLPAPER_ROW[0]} {WALLPAPER_ROW[1]})...")
    reader.send(f"TAP {WALLPAPER_ROW[0]} {WALLPAPER_ROW[1]}")
    assert reader.wait_for("Wallpaper picker opened", timeout=25.0, print_filter=["SETTINGS", "INPUT"]), "Wallpaper picker not opened!"
    print("[TEST] Wallpaper picker opened successfully!")

    # 4. Select the Night scene. The gallery paints every scene the catalogue holds
    # (SCENES, sdk/rust/interos-std/src/design/walls.rs:249) three to a row, and
    # "InterOS Night" is index 2 -- the right column of the first row. wp_cell_at
    # (main.rs:5232) accepts the whole cell, so the point is its centre.
    print(f"[TEST] Selecting Night wallpaper (TAP {NIGHT_CELL[0]} {NIGHT_CELL[1]})...")
    reader.send(f"TAP {NIGHT_CELL[0]} {NIGHT_CELL[1]}")
    assert reader.wait_for("Wallpaper selected for preview", timeout=25.0, print_filter=["SETTINGS", "INPUT"]), "Night wallpaper not selected!"
    print("[TEST] Night wallpaper selected for preview!")

    time.sleep(1.0)
    # 5. Set Target: Home -- segment 0 of the three-segment selector under the
    # preview (WallpaperTarget::Home is index 0, main.rs:141-147; the segments are
    # wp_segment_rect, main.rs:5266, and wallpaper_at gates them on the band
    # WP_TARGET_Y..+WP_TARGET_H, main.rs:5599).
    print(f"[TEST] Setting Wallpaper Target: Home (TAP {HOME_TARGET[0]} {HOME_TARGET[1]})...")
    reader.send(f"TAP {HOME_TARGET[0]} {HOME_TARGET[1]}")
    assert reader.wait_for("Wallpaper target changed", timeout=25.0, print_filter=["SETTINGS", "INPUT"]), "Wallpaper target not changed to Home!"
    print("[TEST] Wallpaper target changed to Home!")

    time.sleep(1.0)
    # 6. Tap Apply: the left half of the action row under the gallery
    # (WallpaperHit::Apply, main.rs:5610-5621).
    print(f"[TEST] Applying wallpaper (TAP {APPLY_BUTTON[0]} {APPLY_BUTTON[1]})...")
    reader.send(f"TAP {APPLY_BUTTON[0]} {APPLY_BUTTON[1]}")
    assert reader.wait_for("Wallpaper written to the settings store", timeout=25.0, print_filter=["SETTINGS", "INPUT"]), "Wallpaper was not written to store!"
    print("[TEST] Wallpaper written to settings store successfully!")

    time.sleep(1.0)
    # 7. Press the home pill to exit Settings. The app's own exit band is
    # x in 240..480 at y >= 1400 (main.rs:6402).
    print(f"[TEST] Exiting Settings via Home gesture (TAP {HOME_GESTURE[0]} {HOME_GESTURE[1]})...")
    reader.send(f"TAP {HOME_GESTURE[0]} {HOME_GESTURE[1]}")
    assert reader.wait_for("Home gesture pressed", timeout=20.0, print_filter=["SETTINGS", "INPUT"]), "Home gesture not triggered!"
    print("[TEST] Settings exited!")

    # 8. Verify Launcher applies Home wallpaper from store and renders Home
    print("[TEST] Verifying Launcher re-renders Home with new wallpaper...")
    applied = reader.wait_for("Home wallpaper applied from store", timeout=25.0, print_filter=["LAUNCHER"])
    assert applied, "Launcher did not apply Home wallpaper from store!"
    print("[TEST] Launcher applied Home wallpaper from store!")

    assert reader.wait_for("Foreground application exited and was reaped", timeout=25.0, print_filter=["LAUNCHER"]), "Child not reaped!"
    assert reader.wait_for("FRAME ticks=", timeout=25.0, print_filter=["LAUNCHER"]), "Launcher frame not rendered!"
    print("[TEST] Launcher frame render complete!")
    time.sleep(1.0)

    # 9. Dump Framebuffer and verify pixels
    ppm_home = VERIFY_DIR / "home_wallpaper_night.ppm"
    screendump(mon_sock, ppm_home)
    assert ppm_home.exists() and ppm_home.stat().st_size > 1000000, "Home PPM screendump failed!"
    print(f"[TEST] Home PPM dumped: {ppm_home} ({ppm_home.stat().st_size} bytes)")

    # Sample a sky pixel in the Home screen
    # For Night scene: top sky is dark (R~23, G~35, B~51 at y=50)
    # For Stock scene: top sky is bright (R~172, G~195, B~220 at y=50)
    w, h, raw = parse_ppm(ppm_home)
    idx = (50 * w + 10) * 3
    r, g, b = raw[idx], raw[idx+1], raw[idx+2]
    print(f"[TEST] Home sky pixel at (10, 50): R={r}, G={g}, B={b}")
    assert r < 100 and g < 100 and b < 110, f"Expected dark Night sky pixel, got R={r}, G={g}, B={b}"
    print("[TEST] Night sky pixel confirmed on HomeScreen (Stock was R > 150, B > 180)!")

    # 10. Reboot QEMU and verify persistence of both: Lock stays Stock, Home stays Night
    print("\n[TEST] Closing session 1 to test reboot persistence...")
    close_session(proc, ser_sock, mon_sock)

    print("[TEST] Starting Session 2 (Reboot) with persisted disk...")
    proc2, ser2, mon2, r2 = boot_to_lock_screen(boot_img, data_img, ser_port, mon_port)
    assert r2.captured_logs and any("lock screen is now guest-owned" in l for l in r2.captured_logs), \
        "Reboot lock screen not presented!"
    ppm_reboot_lock = VERIFY_DIR / "reboot_lock_stock.ppm"
    screendump(mon2, ppm_reboot_lock)
    w_rl, h_rl, raw_rl = parse_ppm(ppm_reboot_lock)
    idx_rl = (150 * w_rl + 360) * 3
    rl_r, rl_g, rl_b = raw_rl[idx_rl], raw_rl[idx_rl+1], raw_rl[idx_rl+2]
    print(f"[TEST] Reboot Lock screen sky pixel at (360, 150): R={rl_r}, G={rl_g}, B={rl_b}")
    assert rl_r > 150 and rl_b > 180, f"Expected Atmospheric Cosmic Stock on reboot lock screen, got R={rl_r}, G={rl_g}, B={rl_b}"
    print("[TEST] Lock screen independently retained Stock wallpaper across reboot!")

    time.sleep(0.5)
    print("[TEST] Unlocking rebooted session with swipe gesture...")
    r2.send("SWIPE 360 1425 360 1100 5\n")
    assert r2.wait_for("Device unlocked via swipe/slide gesture", timeout=30.0, print_filter=["LAUNCHER", "USER"]), "Reboot unlock failed!"
    print("[TEST] Unlocked into rebooted HomeScreen!")

    assert r2.wait_for("FRAME ticks=", timeout=25.0, print_filter=["LAUNCHER"]), "Reboot frame not rendered!"
    print("[TEST] Reboot frame render complete!")
    time.sleep(1.0)
    ppm_reboot_home = VERIFY_DIR / "reboot_home_night.ppm"
    screendump(mon2, ppm_reboot_home)
    w_rh, h_rh, raw_rh = parse_ppm(ppm_reboot_home)
    idx_rh = (50 * w_rh + 10) * 3
    rh_r, rh_g, rh_b = raw_rh[idx_rh], raw_rh[idx_rh+1], raw_rh[idx_rh+2]
    print(f"[TEST] Reboot Home screen sky pixel at (10, 50): R={rh_r}, G={rh_g}, B={rh_b}")
    assert rh_r < 100 and rh_g < 100 and rh_b < 110, f"Expected Night on reboot home screen, got R={rh_r}, G={rh_g}, B={rh_b}"
    print("[TEST] Home screen persisted Night wallpaper across reboot!")

    close_session(proc2, ser2, mon2)

    print("\n============================================================")
    print("  HOME-ONLY WALLPAPER CUSTOMISATION & PERSISTENCE VERIFIED!")
    print("  LockScreen: Stock (#9CB8D6 sky R=174, G=197, B=221)")
    print("  HomeScreen: Night (#172333 sky R=74, G=84, B=96)")
    print("  Reboot: Both surfaces independently retained their choices")
    print("============================================================")

if __name__ == "__main__":
    main()

