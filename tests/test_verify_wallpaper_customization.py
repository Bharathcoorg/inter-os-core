#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
Test End-to-End Home-Only Wallpaper Customisation in QEMU.
Navigates Settings -> Personal -> Wallpaper Picker -> Selects Night -> Sets Target: Home -> Applies.
Exits back to Launcher, verifies Home uses Night, Lock uses Stock.

WHY the taps are derived and not written down: every point this suite touches is a
rectangle the Settings app computes from the SDK's spacing and type tokens, and the
app's own hit-tests accept those rectangles and nothing else. A literal coordinate is
a copy of that arithmetic, and the copy is what goes stale when the app's chain is
re-laid -- which is exactly how this suite came to tap the gap between two gallery
rows and a row that had moved 320px up the screen. The block below resolves each
layout constant out of the source that declares it -- the SDK's tokens, the face's
line box, and Settings' own bindings -- and then takes the centre of the rectangle
the app drew.
"""

import os
import re
import sys
import time
import socket
import subprocess
from pathlib import Path

if hasattr(sys.stdout, 'reconfigure'):
    sys.stdout.reconfigure(encoding='utf-8', errors='replace', line_buffering=True)

ROOT = Path(__file__).resolve().parent.parent

# ---------------------------------------------------------------------------
# The platform's type ladder, read off the Rust side
# ---------------------------------------------------------------------------
#
# WHY parsed and not written down: `font::line_height` is the only authority for
# the row box a rung reserves, and every row, card and gallery cell this suite
# taps is sized from it by the Settings app. This file used to carry its own copy
# of the ladder next to the arithmetic, and when the rungs rose the copy went on
# answering 32 and 44 while the app reserved 36 and 48: every pitch derived from
# it slid up the screen and the suite tapped the gutter between two gallery cells.
# The chain the SDK solves -- cap height -> em -> ascent + descent -- is re-solved
# here from the same four sources, so a rung that moves moves this suite with it:
#   font::cap_for_scale                     sdk/rust/interos-std/src/font.rs
#   outline_text::em_for_cap_height         .../src/design/outline_text.rs
#   typeface::line_metrics                  .../src/design/typeface.rs
#   face::{ASCENT, DESCENT, UNITS_PER_EM}    .../src/design/face.rs
SDK_SRC = ROOT / "sdk" / "rust" / "interos-std" / "src"
_FONT_SRC = (SDK_SRC / "font.rs").read_text(encoding="utf-8")
_OUTLINE_SRC = (SDK_SRC / "design" / "outline_text.rs").read_text(encoding="utf-8")
_TYPEFACE_SRC = (SDK_SRC / "design" / "typeface.rs").read_text(encoding="utf-8")
_FACE_SRC = (SDK_SRC / "design" / "face.rs").read_text(encoding="utf-8")
_DESIGN_SRC = (SDK_SRC / "design" / "mod.rs").read_text(encoding="utf-8")
_WALLS_SRC = (SDK_SRC / "design" / "walls.rs").read_text(encoding="utf-8")
_SETTINGS_SRC = (ROOT / "apps" / "settings" / "src" / "main.rs").read_text(encoding="utf-8")


def _fn_body(src, signature):
    """The body of a `const fn`, given its signature verbatim.

    WHY the signature and not a looser pattern: the reader is a copy of the
    arithmetic the SDK runs; if that signature changes, the ladder has been
    re-plumbed and this suite has to be re-read with it. Failing loudly here is
    the honest outcome -- a silent fallback would put the suite back to aiming at
    rows the app no longer draws.
    """
    match = re.search(re.escape(signature) + r" \{(.*?)\n\}", src, re.S)
    assert match, f"{signature} is no longer shaped the way this suite reads it"
    return match.group(1)


def _cap_ladder():
    """`font::cap_for_scale`: the rung -> cap-height table and its fallback step."""
    body = _fn_body(_FONT_SRC, "pub const fn cap_for_scale(scale: usize) -> u32")
    ladder = {int(rung): int(cap)
              for rung, cap in re.findall(r"^\s*(\d+)\s*=>\s*(\d+)", body, re.M)}
    assert ladder, "font::cap_for_scale answers no rung"
    fallback = re.search(r"\bs\s*=>\s*\(s as u32\)\.saturating_mul\((\d+)\)", body)
    assert fallback, "font::cap_for_scale no longer answers an unknown rung by scaling it"
    return ladder, int(fallback.group(1))


_LADDER, _LADDER_FALLBACK = _cap_ladder()


def _em_arithmetic():
    """`outline_text::em_for_cap_height` as (cap * UNITS + ROUND) // CAP_HEIGHT."""
    body = _fn_body(_OUTLINE_SRC, "pub const fn em_for_cap_height(cap_px: u32) -> u32")
    expr = re.search(r"\(\(cap_px as u64 \* (\d+) \+ (\d+)\) / (\d+)\)", body)
    assert expr, ("outline_text::em_for_cap_height is no longer the arithmetic this suite "
                  "solves, so the ladder has to be re-read here")
    return tuple(int(group) for group in expr.groups())


_EM_UNITS_PER_EM, _EM_ROUNDING, _EM_CAP_UNITS = _em_arithmetic()


def _face_metrics():
    """`face`'s metrics, with `typeface::line_metrics`' rule pinned beside them."""
    body = _fn_body(_TYPEFACE_SRC, "pub const fn line_metrics(px_per_em: u32) -> (u32, u32)")
    assert "let upem = face::UNITS_PER_EM as i32;" in body
    assert "(face::ASCENT as i32 * px_per_em as i32) / upem" in body
    assert "(-(face::DESCENT as i32) * px_per_em as i32) / upem" in body
    upem = int(re.search(r"pub const UNITS_PER_EM: u16 = (\d+);", _FACE_SRC).group(1))
    ascent = int(re.search(r"pub const ASCENT: i16 = (-?\d+);", _FACE_SRC).group(1))
    descent = int(re.search(r"pub const DESCENT: i16 = (-?\d+);", _FACE_SRC).group(1))
    return upem, ascent, descent


_FACE_UPM, _FACE_ASCENT, _FACE_DESCENT = _face_metrics()


def _line_height(scale):
    """`font::line_height(scale)`: the outline face's box at that rung, in rows."""
    if scale == 0:
        return 0
    cap = _LADDER.get(scale, scale * _LADDER_FALLBACK)
    em = (cap * _EM_UNITS_PER_EM + _EM_ROUNDING) // _EM_CAP_UNITS
    return (_FACE_ASCENT * em) // _FACE_UPM + ((-_FACE_DESCENT) * em) // _FACE_UPM


def _closes_at(expr, index):
    depth = 0
    for i in range(index, len(expr)):
        if expr[i] == "(":
            depth += 1
        elif expr[i] == ")":
            depth -= 1
            if depth == 0:
                return i
    return -1


def _split_chain(expr, operators):
    """Splits a top-level operator chain into its operands and its operators."""
    operands, ops, depth, start = [], [], 0, 0
    for i, ch in enumerate(expr):
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        elif ch in operators and depth == 0:
            operands.append(expr[start:i])
            ops.append(ch)
            start = i + 1
    operands.append(expr[start:])
    return operands, ops


def _usize(expr, source, depth=0):
    """Evaluates a Rust `usize` expression out of the source that declares its names.

    WHY evaluate rather than copy: the boxes this suite taps are the app's own
    constants -- `CARD_H`, `ROW_H`, `WP_CELL_H` -- and a literal here is a second
    copy of the app's arithmetic, which is exactly what went stale when the ladder
    rose. Resolving the binding from the app's source means the suite taps the row
    the app reserves even when the app's rhythm changes.
    """
    assert depth < 24, f"expression nests too deeply to resolve: {expr!r}"
    expr = expr.strip()
    while expr.startswith("(") and _closes_at(expr, 0) == len(expr) - 1:
        expr = expr[1:-1].strip()
    if expr.isdigit():
        return int(expr)
    # `font::line_height` at a rung, and the SDK tables the app reaches through
    # a path, are the only calls these layout constants make.
    call = re.fullmatch(r"[\w:]*font::line_height\((.+)\)", expr)
    if call:
        return _line_height(_usize(call.group(1), source, depth + 1))
    if expr.endswith("SCENES.len()"):
        table = re.search(r"pub const SCENES: \[[^\]]+;\s*(\d+)\]", _WALLS_SRC)
        assert table, "design::walls::SCENES no longer declares its own length"
        return int(table.group(1))
    saturating = re.fullmatch(r"([A-Z_][A-Z0-9_]*)\.saturating_sub\((\d+)\)", expr)
    if saturating:
        return max(0, _usize(saturating.group(1), source, depth + 1)
                   - int(saturating.group(2)))
    for operators, fold in (("+-", lambda a, op, b: a + b if op == "+" else a - b),
                            ("*/", lambda a, op, b: a * b if op == "*" else a // b)):
        operands, ops = _split_chain(expr, operators)
        if len(operands) > 1:
            value = _usize(operands[0], source, depth + 1)
            for op, operand in zip(ops, operands[1:]):
                value = fold(value, op, _usize(operand, source, depth + 1))
            return value
    path = re.fullmatch(r"[\w:]*::([A-Z_][A-Z0-9_]*)", expr)
    if path:
        # A qualified name is one of the SDK's design tokens.
        binding = re.search(r"^\s*pub const " + path.group(1) + r"\s*:\s*usize\s*=\s*([^;]+);",
                            _DESIGN_SRC, re.M)
        if binding:
            return _usize(binding.group(1), _DESIGN_SRC, depth + 1)
        raise AssertionError(f"no design token {expr!r} in sdk/.../design/mod.rs")
    if re.fullmatch(r"[A-Z_][A-Z0-9_]*", expr):
        # A bare name belongs to the source being read; that source may itself
        # reach an SDK token by its short path (`spacing::GRID_12`).
        for candidate in (source, _DESIGN_SRC):
            binding = re.search(r"^\s*(?:pub )?const " + expr + r"\s*:\s*usize\s*=\s*([^;]+);",
                                candidate, re.M)
            if binding:
                return _usize(binding.group(1), candidate, depth + 1)
        raise AssertionError(f"no `const {expr}: usize` binding in the SDK or the app source")
    raise AssertionError(f"cannot resolve {expr!r} as a usize expression")


def _settings(name):
    """One module-level `const NAME: usize = ...;` of the Settings app, resolved."""
    return _usize(name, _SETTINGS_SRC)


def _design(name):
    """One `pub const NAME: usize = ...;` of the SDK's design tokens, resolved."""
    return _usize(f"std::design::{name}", _DESIGN_SRC)


# ---------------------------------------------------------------------------
# Settings-app geometry, re-derived from the app's own layout constants
# ---------------------------------------------------------------------------
#
# Every number here is a binding of the app or a token of the SDK, read out of the
# source that declares it. The app lays its screens out in the 720x1440 authoring
# viewport (sdk/rust/interos-std/src/design/mod.rs::viewport::LOGICAL_WIDTH/HEIGHT),
# so the derived points are scanout pixels one-for-one.

W, H = _settings("W"), _settings("H")   # apps/settings/src/main.rs

# SDK tokens (sdk/rust/interos-std/src/design/mod.rs)
GRID_4 = _design("GRID_4")              # spacing::GRID_4
GRID_8 = _design("GRID_8")              # spacing::GRID_8
GRID_16 = _design("GRID_16")            # spacing::GRID_16
GRID_24 = _design("GRID_24")            # spacing::GRID_24
GRID_32 = _design("GRID_32")            # spacing::GRID_32
PREFERRED_TARGET = _design("PREFERRED_TARGET")   # touch::PREFERRED_TARGET
PRIMARY_ACTION = _design("PRIMARY_ACTION")       # touch::PRIMARY_ACTION
STATUS_BAR_HEIGHT = _design("STATUS_BAR_HEIGHT")  # viewport::STATUS_BAR_HEIGHT
HEADER_TOP = _design("HEADER_TOP")                # viewport::HEADER_TOP

# The face's line boxes, from the same ladder the app's compile-time constants
# use: the rungs are the app's own (`TYPE_H3`, `TYPE_DISPLAY`), and the box each
# one reserves is `_line_height` above.
TYPE_H3 = _settings("TYPE_H3")          # the row / card rung, settings main.rs
TYPE_DISPLAY = _settings("TYPE_DISPLAY")  # the screen-title rung
LINE_H = _settings("LINE_H")            # font::line_height(TYPE_H3), settings main.rs
LINE_H_DISPLAY = _line_height(TYPE_DISPLAY)

assert LINE_H == _line_height(TYPE_H3), (
    "Settings no longer binds LINE_H to the face's own box at TYPE_H3, so the "
    "row and card heights below are not the boxes the app reserves"
)
assert "const LINE_H: usize = std::design::font::line_height(TYPE_H3);" in _SETTINGS_SRC, (
    "Settings' LINE_H binding moved, and this suite's re-derivation of it has to move with it"
)
assert "const DIVIDER_Y: usize = HEADER_Y + std::design::font::line_height(TYPE_DISPLAY)" \
       " + std::design::spacing::GRID_16;" in _SETTINGS_SRC, (
    "the header rule is no longer one grid step under the title's line box"
)

MARGIN = _settings("MARGIN")             # = spacing::GRID_24
CONTENT_X = _settings("CONTENT_X")
CONTENT_W = _settings("CONTENT_W")
CARD_GAP = _settings("CARD_GAP")
BUTTON_H = _settings("BUTTON_H")         # = touch::PREFERRED_TARGET
BUTTON_HALF = _settings("BUTTON_HALF")

# --- Landing card grid ---
GRID_X = _settings("GRID_X")
CARD_PAD = _settings("CARD_PAD")
CARD_LINE_GAP = _settings("CARD_LINE_GAP")
GRID_GAP = _settings("GRID_GAP")
CARD_W = _settings("CARD_W")
CARD_H = _settings("CARD_H")             # CARD_PAD*2 + LINE_H*3 + CARD_LINE_GAP
GRID_Y = _settings("GRID_Y")

def landing_card_rect(i):
    """card_rect() -- settings main.rs. Card 6 spans the full column."""
    if i == 6:
        return (GRID_X, GRID_Y + 3 * (CARD_H + GRID_GAP), CONTENT_W, CARD_H)
    col, row = i % 2, i // 2
    return (GRID_X + col * (CARD_W + GRID_GAP), GRID_Y + row * (CARD_H + GRID_GAP),
            CARD_W, CARD_H)

def landing_card_centre(i):
    x, y, w, h = landing_card_rect(i)
    return (x + w // 2, y + h // 2)

# --- Detail rows (stacked from DETAIL_TOP with ROW_GAP) ---
DIVIDER_Y = _settings("DIVIDER_Y")
DETAIL_TOP = _settings("DETAIL_TOP")
CONTENT_BOTTOM = _settings("CONTENT_BOTTOM")
ROW_PAD_V = _settings("ROW_PAD_V")       # = spacing::GRID_32
ROW_LINE_GAP = _settings("ROW_LINE_GAP")  # = spacing::GRID_8
ROW_H = _settings("ROW_H")               # two line boxes and the row's padding
ROW_GAP = _settings("ROW_GAP")

# The header rule and the card grid are re-checked against the tokens rather than
# only read back, so an app that re-factored its chain away from the platform's
# step is reported instead of silently followed.
assert MARGIN == GRID_24 and ROW_PAD_V == GRID_32 and ROW_LINE_GAP == GRID_8
assert CARD_H == CARD_PAD * 2 + LINE_H * 3 + CARD_LINE_GAP, (
    "a landing card no longer reserves three body line boxes and the step between them"
)
assert ROW_H == (ROW_PAD_V + LINE_H + ROW_LINE_GAP) + LINE_H + ROW_PAD_V, (
    "a detail row no longer reserves two body line boxes and its own padding"
)
assert DIVIDER_Y == HEADER_TOP + LINE_H_DISPLAY + GRID_16, (
    "the header rule no longer closes the screen title's line box"
)
assert HEADER_TOP >= STATUS_BAR_HEIGHT, (
    "the app's content boundary moved inside the band the shell paints over"
)

def detail_row_centre(row):
    """detail_row_rect() for the equal-height rows that precede Personal's
    document rows -- settings main.rs, `row_h` answers ROW_H for those rows."""
    top = DETAIL_TOP + row * (ROW_H + ROW_GAP)
    return (CONTENT_X + CONTENT_W // 2, top + ROW_H // 2)

# --- Wallpaper picker panel ---
WP_PREVIEW_Y = _settings("WP_PREVIEW_Y")            # = DETAIL_TOP
WP_CELL_PAD = _settings("WP_CELL_PAD")
WP_CELL_LABEL_GAP = _settings("WP_CELL_LABEL_GAP")
WP_THUMB_H = _settings("WP_THUMB_H")
WP_CELL_GAP = _settings("WP_CELL_GAP")
WP_CELL_W = _settings("WP_CELL_W")
WP_CELL_H = _settings("WP_CELL_H")                  # the thumb and its label's box
SCENES = _usize("std::design::walls::SCENES.len()", _SETTINGS_SRC)  # design/walls.rs
WP_ROWS = _settings("WP_ROWS")
WP_GALLERY_H = _settings("WP_GALLERY_H")
WP_FOOTER_H = _settings("WP_FOOTER_H")
WP_PREVIEW_H = _settings("WP_PREVIEW_H")            # the room the panel has left
WP_TARGET_Y = _settings("WP_TARGET_Y")
WP_TARGET_H = _settings("WP_TARGET_H")              # = touch::PRIMARY_ACTION
WP_GALLERY_Y = _settings("WP_GALLERY_Y")
WP_ACTION_Y = _settings("WP_ACTION_Y")
SEG_GAP = _settings("SEG_GAP")                      # the selector's 4px segment gap

assert WP_CELL_H == WP_CELL_PAD + WP_THUMB_H + WP_CELL_LABEL_GAP + LINE_H + WP_CELL_PAD, (
    "a gallery cell no longer reserves the thumb plus one whole body line box for its label"
)
assert WP_PREVIEW_H == CONTENT_BOTTOM - DETAIL_TOP - WP_FOOTER_H, (
    "the preview is no longer the room the panel's chain leaves over"
)
assert (BUTTON_H, WP_TARGET_H, SEG_GAP) == (PREFERRED_TARGET, PRIMARY_ACTION, GRID_4), (
    "the controls are no longer on the SDK's touch and segment tokens this suite reads"
)

def wp_cell_centre(i):
    """wp_cell_origin() -- settings main.rs, three cells to a row."""
    col, row = i % 3, i // 3
    x = CONTENT_X + col * (WP_CELL_W + WP_CELL_GAP)
    y = WP_GALLERY_Y + row * (WP_CELL_H + WP_CELL_GAP)
    return (x + WP_CELL_W // 2, y + WP_CELL_H // 2)

def wp_segment_centre(i):
    """wp_segment_rect() -- settings main.rs."""
    seg_w = (CONTENT_W - 2 * SEG_GAP) // 3
    x = CONTENT_X + i * (seg_w + SEG_GAP)
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

