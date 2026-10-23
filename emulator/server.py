"""
Inter OS — Device Studio Server

Serves the browser studio UI on port 4300 for Rust mobile application
development, package sideloading (.app, .rap, .wasm, .apk), real-time UART
telemetry, and guest-backed QEMU telemetry and development controls.
"""

import http.server
import socketserver
import os
import sys
import threading
import socket
import json
import subprocess
import time
import re
import hashlib
import urllib.parse
import urllib.request
import shutil
import zipfile
import io
import struct
import ipaddress

PORT = int(os.environ.get("STUDIO_PORT", 4300))
SERIAL_TCP_PORT = int(os.environ.get("SERIAL_PORT", 4301))
QMP_PORT = int(os.environ.get("QMP_PORT", 4302))
MONITOR_PORT = int(os.environ.get("MONITOR_PORT", 44445))
WS_PORT = int(os.environ.get("WS_PORT", 6081))
DIRECTORY = os.path.dirname(os.path.abspath(__file__))

# QEMU's VNC display index. Display N listens on 127.0.0.1:(5900 + N), so the
# historical display 1 is the endpoint 127.0.0.1:5901.
#
# WHY the display is a setting and not a constant: it used to be hard-coded to
# 1, so anything else holding 127.0.0.1:5901 made QEMU exit at launch while
# the Studio went on reporting a running device. INTEROS_VNC_DISPLAY moves the
# display; VNC_PORT is still honoured when it names a display range, and the
# display index is derived from it so QEMU and the Studio's own RFB client can
# never disagree about which port the guest is on.
DEFAULT_VNC_DISPLAY = 1


def _resolve_vnc_endpoint():
    """Return the (display, port) pair QEMU and the Studio must agree on."""
    display = DEFAULT_VNC_DISPLAY
    raw_display = os.environ.get("INTEROS_VNC_DISPLAY", "").strip()
    raw_port = os.environ.get("VNC_PORT", "").strip()
    if raw_display:
        try:
            display = int(raw_display)
        except ValueError:
            print(f"[*] Notice: ignoring invalid INTEROS_VNC_DISPLAY={raw_display!r}")
            display = DEFAULT_VNC_DISPLAY
    if not 0 <= display <= 99:
        print(f"[*] Notice: INTEROS_VNC_DISPLAY={display} is out of range; using {DEFAULT_VNC_DISPLAY}.")
        display = DEFAULT_VNC_DISPLAY
    if raw_port:
        try:
            wanted = int(raw_port)
        except ValueError:
            wanted = None
        if wanted is not None and wanted != 5900 + display:
            derived = wanted - 5900
            if not raw_display and 0 <= derived <= 99:
                display = derived
            else:
                print(f"[*] Notice: VNC_PORT={wanted} ignored; QEMU's -vnc takes a display index, "
                      f"so display {display} (port {5900 + display}) is used.")
    return display, 5900 + display


VNC_DISPLAY, VNC_PORT = _resolve_vnc_endpoint()

# The display and port the QEMU instance this Studio launched is expected on.
# A launch may move it when the configured display is taken; every consumer
# (the RFB client, the websockify bridge, the port probe) follows this.
active_vnc_display = VNC_DISPLAY
active_vnc_port = VNC_PORT

qemu_subproc = None
ws_subproc = None
# The VNC port the studio's own websockify bridge currently points at.
ws_bridge_port = None
serial_lines = []
serial_socket_conn = None
serial_socket_lock = threading.Lock()
telemetry_lock = threading.Lock()
# Bumped on every QEMU (re)launch; readers compare it to detect stale sockets.
serial_epoch = 0
launcher_has_booted = False

# The QEMU process this Studio launched, tracked by PID and start time.
#
# WHY a PID alone is not enough: PIDs are recycled, so a PID that was QEMU
# yesterday may be an unrelated process today, and a Studio that remembered
# only the number would call that "the device". The start time makes the pair
# a fingerprint: a different process that inherited the PID has a different
# start time, so it is never mistaken for the guest this Studio launched.
qemu_process_record = {"pid": None, "start_key": None, "started_at": 0.0}
qemu_process_lock = threading.Lock()

# The persisted launch record. It names the QEMU process this Studio launched
# (PID plus the operating system's creation time) so a restarted Studio can
# re-adopt a guest that outlived it instead of mistaking it for a stranger's
# process. It is runtime state, not source.
QEMU_LAUNCH_STATE_FILE = os.path.join(DIRECTORY, "data", "qemu_launch_state.json")


class NoTrackedQemu(Exception):
    """Raised when the Studio has no QEMU process of its own to talk to."""

# -----------------------------------------------------------------------------
# QEMU process identity and the device's provable state
# -----------------------------------------------------------------------------
# The Studio decides whether a device is running from the PROCESS it launched,
# never from a port alone. A port that answers proves that something holds it,
# not that the guest this Studio launched is behind it, so a stale port can
# never make the Studio claim a running device.


def _windows_process_start_key(pid):
    """The Windows process creation time of `pid`, or None if it is gone."""
    try:
        import ctypes
        from ctypes import wintypes
    except Exception:
        return None
    try:
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    except Exception:
        return None
    try:
        kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel32.OpenProcess.restype = wintypes.HANDLE
        kernel32.GetProcessTimes.argtypes = [
            wintypes.HANDLE,
            ctypes.POINTER(wintypes.FILETIME),
            ctypes.POINTER(wintypes.FILETIME),
            ctypes.POINTER(wintypes.FILETIME),
            ctypes.POINTER(wintypes.FILETIME),
        ]
        kernel32.GetProcessTimes.restype = wintypes.BOOL
        kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel32.CloseHandle.restype = wintypes.BOOL
    except Exception:
        return None
    handle = kernel32.OpenProcess(0x1000, False, int(pid))  # PROCESS_QUERY_LIMITED_INFORMATION
    if not handle:
        return None
    try:
        creation = wintypes.FILETIME()
        exit_time = wintypes.FILETIME()
        kernel_time = wintypes.FILETIME()
        user_time = wintypes.FILETIME()
        ok = kernel32.GetProcessTimes(
            handle,
            ctypes.byref(creation),
            ctypes.byref(exit_time),
            ctypes.byref(kernel_time),
            ctypes.byref(user_time),
        )
        if not ok:
            return None
        return (creation.dwHighDateTime << 32) | creation.dwLowDateTime
    except Exception:
        return None
    finally:
        try:
            kernel32.CloseHandle(handle)
        except Exception:
            pass


def _posix_process_start_key(pid):
    """The kernel start time of `pid`, or None when it cannot be read."""
    try:
        with open(f"/proc/{int(pid)}/stat", "rb") as handle:
            data = handle.read()
    except Exception:
        return None
    # The comm field is parenthesised and may itself contain spaces, so the
    # remaining fields are located from the last ')'.
    close = data.rfind(b")")
    if close < 0:
        return None
    fields = data[close + 2:].split()
    # fields[0] is the process state (field 3), so starttime (field 22) is 19.
    if len(fields) < 20:
        return None
    try:
        return int(fields[19])
    except ValueError:
        return None


def _process_start_key(pid):
    """A comparable token for when `pid` started, or None when it is unknown.

    The token is the operating system's own process creation time, so it is
    stable for the life of that process and different for every other process
    that later reuses the PID.
    """
    if not pid or int(pid) <= 0:
        return None
    if sys.platform == "win32":
        return _windows_process_start_key(pid)
    return _posix_process_start_key(pid)


def _write_launch_state(record):
    """Persist the launch record so a restarted Studio can re-adopt its guest."""
    try:
        os.makedirs(os.path.dirname(QEMU_LAUNCH_STATE_FILE), exist_ok=True)
        tmp = QEMU_LAUNCH_STATE_FILE + ".tmp"
        with open(tmp, "w", encoding="utf-8") as handle:
            json.dump(record, handle)
        os.replace(tmp, QEMU_LAUNCH_STATE_FILE)
    except Exception:
        pass


def _remove_launch_state():
    try:
        os.remove(QEMU_LAUNCH_STATE_FILE)
    except OSError:
        pass


def _read_launch_state():
    try:
        with open(QEMU_LAUNCH_STATE_FILE, "r", encoding="utf-8") as handle:
            data = json.load(handle)
        return data if isinstance(data, dict) else None
    except Exception:
        return None


def track_qemu_process(proc, vnc_display=None, vnc_port=None):
    """Record the QEMU process this Studio launched, with its start time."""
    global qemu_process_record
    if proc is None:
        clear_qemu_process()
        return
    start_key = _process_start_key(proc.pid)
    with qemu_process_lock:
        qemu_process_record = {
            "pid": proc.pid,
            "start_key": start_key,
            "started_at": time.time(),
        }
        record = dict(qemu_process_record)
    if start_key is not None:
        # The record outlives this server process on purpose: a guest that
        # outlives the Studio is still the guest this Studio launched, and
        # the identity (PID plus creation time) is what proves it.
        _write_launch_state({
            "pid": record["pid"],
            "start_key": record["start_key"],
            "started_at": record["started_at"],
            "vnc_display": vnc_display,
            "vnc_port": vnc_port,
        })


def clear_qemu_process():
    """Forget the tracked QEMU process (it is gone, or was never ours)."""
    global qemu_process_record
    with qemu_process_lock:
        qemu_process_record = {"pid": None, "start_key": None, "started_at": 0.0}
    _remove_launch_state()


def tracked_qemu_process():
    with qemu_process_lock:
        return dict(qemu_process_record)


def _recorded_process_alive(record):
    """True when the recorded process identity still names a live process."""
    pid = record.get("pid")
    key = record.get("start_key")
    if not pid or key is None:
        return False
    return _process_start_key(pid) == key


def adopt_previous_qemu_process():
    """Re-adopt the QEMU an earlier Studio instance launched, if it is that process.

    WHY: a guest outliving the Studio process is a normal development loop
    (the Studio is restarted while the guest keeps running). The record names
    the exact process - PID plus the operating system's creation time - so
    re-adopting it PROVES the identity instead of assuming it, and a recycled
    PID can never be adopted.
    """
    global qemu_process_record
    with qemu_process_lock:
        if qemu_process_record.get("pid"):
            return dict(qemu_process_record)
    record = _read_launch_state()
    if not record:
        return None
    pid = record.get("pid")
    key = record.get("start_key")
    if not pid or key is None or not _recorded_process_alive(record):
        # The recorded process is gone (or the PID now belongs to something
        # else): the record describes nothing that exists.
        _remove_launch_state()
        return None
    with qemu_process_lock:
        qemu_process_record = {
            "pid": int(pid),
            "start_key": key,
            "started_at": float(record.get("started_at") or 0.0),
        }
    return dict(qemu_process_record)


def tracked_qemu_process_alive():
    """True when the QEMU process this Studio launched is provably still running.

    The Popen handle is authoritative for a child of this process: poll()
    reaps it and can never be fooled by a recycled PID. When only the record
    survives, the PID's current start time is compared with the recorded one,
    so a different process that inherited the PID is not mistaken for the
    guest. With neither, the Studio cannot prove the device is there and
    reports that it is not.
    """
    record = tracked_qemu_process()
    pid = record.get("pid")
    if not pid:
        record = adopt_previous_qemu_process() or {}
        pid = record.get("pid")
        if not pid:
            return False
    proc = qemu_subproc
    if proc is not None and getattr(proc, "pid", None) == pid:
        return proc.poll() is None
    return _recorded_process_alive(record)


def _monitor_answers(timeout=0.35):
    """True when QEMU's HMP monitor answers on the monitor port."""
    try:
        ok, _ = send_qemu_monitor_cmd("info status", timeout=timeout)
        return bool(ok)
    except Exception:
        return False


def wait_for_monitor(timeout=2.0):
    """Wait (briefly) for the monitor of a freshly launched guest to answer.

    QEMU binds the monitor socket during startup, so a launch that has just
    returned may need a moment before the monitor is reachable. Waiting here
    keeps the launch result from reporting a guest as "unknown" purely because
    it was asked one millisecond too early.
    """
    deadline = time.time() + max(float(timeout), 0.0)
    while True:
        if _monitor_answers():
            return True
        if time.time() >= deadline:
            return False
        time.sleep(0.1)


def _port_occupied(port, host="127.0.0.1"):
    """True when the port cannot be taken by a new listener.

    WHY a bind probe rather than a connect probe: a connect only answers
    "is somebody accepting", and a listener whose accept queue is full (or
    which never accepts) makes the connect hang or be refused while the port
    is very much in use - which is exactly the "something else holds the VNC
    port" case the Studio has to recognise. A bind asks the question the
    launch actually needs answered: can QEMU have this port?

    The bind uses SO_REUSEADDR only on POSIX, where it still refuses a port
    another socket is actively listening on but permits a TIME_WAIT remnant,
    so a relaunch does not drift onto a new display. On Windows the
    exclusive bind (no SO_REUSEADDR) refuses a listening port and still
    permits a TIME_WAIT remnant.
    """
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        if os.name != "nt":
            probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        probe.bind((host, port))
        return False
    except OSError:
        return True
    finally:
        try:
            probe.close()
        except OSError:
            pass


def _port_is_free(port, host="127.0.0.1"):
    """True when a new listener can take the port."""
    return not _port_occupied(port, host=host)


def qemu_device_state(probe_monitor=True):
    """The Studio's provable belief about the device.

    "running" requires BOTH: the QEMU process this Studio launched is alive,
    AND the monitor socket answers. The VNC port is probed as a secondary
    signal only; it can never make the Studio claim a running device.
    """
    process_alive = tracked_qemu_process_alive()
    monitor_answers = _monitor_answers() if probe_monitor else False
    vnc_port_busy = _port_occupied(active_vnc_port)
    record = tracked_qemu_process()
    if process_alive and monitor_answers:
        state = "running"
        reason = "The QEMU process this Studio launched is alive and its monitor answers."
    elif process_alive:
        state = "unknown"
        reason = "The QEMU process this Studio launched is alive, but its monitor did not answer."
    elif monitor_answers:
        state = "unknown"
        reason = (f"A QEMU monitor answers on port {MONITOR_PORT}, but this Studio did not launch it "
                  "and cannot identify it as the device.")
    elif vnc_port_busy:
        state = "not_running"
        reason = (f"No QEMU process is tracked by this Studio; port {active_vnc_port} is held by "
                  "another process, which is not the device.")
    else:
        state = "not_running"
        reason = "No QEMU process is tracked by this Studio."
    return {
        "running": state == "running",
        "state": state,
        "reason": reason,
        "process_tracked": bool(record.get("pid")),
        "process_alive": process_alive,
        "pid": record.get("pid"),
        "monitor_answers": monitor_answers,
        "vnc_port_busy": vnc_port_busy,
    }


def stop_tracked_qemu():
    """Terminate the tracked QEMU process and report whether it is gone.

    The tracked process is the one this Studio launched, proven by PID plus
    the operating system's creation time. A child of this process is stopped
    through its own handle; a process adopted from an earlier Studio instance
    (a guest that outlived this server) is asked to quit through its own
    monitor, the supported way to release its sockets and file locks.

    Returns True when the tracked process is gone afterwards.
    """
    record = tracked_qemu_process()
    pid = record.get("pid")
    proc = qemu_subproc
    if not pid:
        stop_qemu_backend()
        return True
    if proc is not None and getattr(proc, "pid", None) == pid:
        stop_qemu_backend()
        try:
            proc.wait(timeout=2)
        except Exception:
            pass
        return proc.poll() is not None
    send_qemu_monitor_cmd('quit')
    time.sleep(0.5)
    gone = not _recorded_process_alive(record)
    stop_qemu_backend()
    return gone


def select_vnc_display(preferred=None, limit=64):
    """Pick a VNC display whose port has no listener.

    WHY: the display used to be a hard-coded 1, so anything else holding
    127.0.0.1:5901 made QEMU exit at launch while the Studio went on
    reporting a running device. The display is a setting now, and a taken
    display is reported instead of being silently fatal.

    The search always starts at the CONFIGURED display, so a Studio that had
    to move aside returns to it once it is free again.

    Returns (display, port, substituted) or (None, None, False) when no free
    display was found.
    """
    start = VNC_DISPLAY if preferred is None else preferred
    for offset in range(limit):
        display = start + offset
        if display > 99:
            break
        port = 5900 + display
        if _port_is_free(port):
            return display, port, offset > 0
    return None, None, False



# Guest scanout geometry.
#
# The logical phone surface is 720x1440. The actual QEMU framebuffer is learned
# from live screendumps, so Device Studio does not assume a developer has a
# particular host resolution.
# The guest's scanout in guest pixels, and the authoring space its shell and
# apps lay out in. They are separate because a raised scanout only buys detail
# if apps paint at that resolution; an app that keeps painting at the authoring
# space gets an upscale, which is a larger surface cost and the same image.
#
# 1080x2160 is FHD-class width at the SAME 1:2 shape as the authoring space, so
# the ratio between them is exactly 1.5 on both axes. The reference design's
# 1080x2400 is 9:20 and a uniform scale to it does not exist.
SCANOUT_W, SCANOUT_H = 1080, 2160
LOGICAL_SURFACE_W, LOGICAL_SURFACE_H = 720, 1440
guest_framebuffer_size = (SCANOUT_W, SCANOUT_H)

# Canonical guest scanout geometries. The x86_64 phone profile engages a
# native 1080x2160 FHD+ portrait scanout once paging is online; landscape
# presents 2160x1080 (or 1280x720 during early boot / legacy modes). A VNC
# session reporting any other desktop size is stale or transitional: publishing
# it would crop the phone surface to the wrong geometry, so the session is
# resynchronised instead of streaming stale pixels.
CANONICAL_SCANOUT_SIZES = frozenset(((SCANOUT_W, SCANOUT_H), (SCANOUT_H, SCANOUT_W), (1088, 2160), (LOGICAL_SURFACE_W, LOGICAL_SURFACE_H), (1280, 720)))
MAX_SCANOUT_DIM = 4096


def is_canonical_scanout(w, h):
    return (w, h) in CANONICAL_SCANOUT_SIZES


# Published phone-frame sizes. The VNC desktop is validated above, but the
# published crop differs by orientation: a native 1080x2160 portrait desktop
# publishes whole, a 1280x720 landscape desktop in portrait orientation
# publishes its centred 360x720 phone slice, and landscape orientation
# publishes the whole 2160x1080 (or 1280x720) desktop. All are guest-derived crops,
# never invented pixels; the browser scales the slice to the phone chassis
# with smoothing, exactly like a hardware emulator viewport.
PHONE_FRAME_SIZES = frozenset(((SCANOUT_W, SCANOUT_H), (SCANOUT_H, SCANOUT_W), (720, 1440), (360, 720), (1280, 720)))

# How long a monitor-confirmed non-native geometry keeps suppressing the resync.
# WHY bounded: during boot the guest really does present 1280x720 until it engages
# native portrait scanout, so a confirmation with no expiry locks the publisher
# onto the boot framebuffer. Ten seconds is long enough to stop re-probing a
# genuine landscape guest every frame and short enough that a boot-time
# confirmation cannot outlive the mode set it mistook for final.
CONFIRMATION_TTL = 10.0


# -----------------------------------------------------------------------------
# Device Profiles (the browser Device Studio's AVD-Manager equivalent)
# -----------------------------------------------------------------------------
# Device Studio boots exactly one virtual phone. A device profile names that
# phone and carries the three things a real device manifest declares: the
# display geometry Studio asks the guest to render, the panel pixel density,
# and the chassis presentation size the studio uses to display it. The table is
# data on purpose — adding or correcting a device is a table edit here, never a
# new branch in the launch or render path.
#
# WHY a profile only ASKS for geometry and never forces it: the guest kernel
# owns the real scanout. QEMU is handed EDID xres/yres from the active profile
# (launch_qemu_profile) so the request is explicit, and the frame publisher /
# capture path compares the live portrait scanout against this same declared
# geometry. If the guest reports anything else the frame is withheld and the
# route answers honestly (503 / standby card) rather than cropping to a
# geometry the profile did not declare. The DEFAULT profile is the reference
# 720x1440 phone the guest already renders by EDID, so today's behaviour is
# reproduced exactly; a profile that declares a different geometry makes that
# choice explicit in this table and fails closed until the guest honours it.
DEVICE_PROFILES = {
    "interos_reference": {
        "id": "interos_reference",
        "name": "InterOS Reference Phone",
        "description": (
            "Native 1080x2160 FHD-class portrait scanout the guest KMS selects by "
            "EDID, with 720x1440 as the authoring space the shell and apps lay "
            "out in. This is the stock phone Device Studio displays."
        ),
        "guest_geometry": {"width": SCANOUT_W, "height": SCANOUT_H},
        # The guest presents this landscape desktop during boot (before native
        # portrait scanout engages) and in landscape orientation. It is a guest
        # behaviour, not a Studio preference; the reference profile records it
        # so the frame geometry checks match what QEMU actually reports.
        "landscape_geometry": {"width": 1280, "height": 720},
        "density_dpi": 480,
        "presentation": {
            "css_max_width_px": 360,
            "aspect_ratio": "1 / 2",
            # The chassis is deliberately NOT resized by a profile: presentation
            # fixes belong to the guest. This documents the current chassis so
            # the DEFAULT profile reproduces it exactly rather than changing it.
            "note": "browser nearest-neighbour scales the guest frame to the fixed chassis",
        },
    },
}

DEFAULT_DEVICE_PROFILE = "interos_reference"
active_device_profile_key = DEFAULT_DEVICE_PROFILE


def active_device_profile():
    """Return the selected device profile, falling back to the reference phone.

    WHY a getter instead of reading the global directly: a stale key (a
    hand-edited config, an older client) must degrade to the reference phone
    rather than KeyError, so every consumer always gets a well-formed profile.
    """
    return DEVICE_PROFILES.get(active_device_profile_key) or DEVICE_PROFILES[DEFAULT_DEVICE_PROFILE]


def device_guest_geometry(profile_key=None):
    """(width, height) the given/active profile asks the guest to modeset.

    This is the authoritative "expected native portrait scanout" the launch
    EDID uses and the frame/capture checks compare the live scanout against.
    For the reference profile it is 720x1440, so every comparison is unchanged.
    """
    profile = (DEVICE_PROFILES.get(profile_key or active_device_profile_key)
               or DEVICE_PROFILES[DEFAULT_DEVICE_PROFILE])
    geom = profile.get("guest_geometry", {})
    return int(geom.get("width", LOGICAL_SURFACE_W)), int(geom.get("height", LOGICAL_SURFACE_H))


def device_profile_public(profile_key=None):
    """Public JSON view of a device profile (no internal fields leak out)."""
    profile = (DEVICE_PROFILES.get(profile_key or active_device_profile_key)
               or DEVICE_PROFILES[DEFAULT_DEVICE_PROFILE])
    return {
        "id": profile.get("id"),
        "name": profile.get("name"),
        "description": profile.get("description"),
        "guest_geometry": profile.get("guest_geometry"),
        "landscape_geometry": profile.get("landscape_geometry"),
        "density_dpi": profile.get("density_dpi"),
        "presentation": profile.get("presentation"),
    }


def select_device_profile(profile_key):
    """Select the device profile applied on the next cold boot.

    Exact-match against DEVICE_PROFILES so a typo or stale client cannot
    silently pick a different device than the one it named. Returns (ok, error).
    Geometry is only requested on the next launch; a running guest keeps the
    scanout it already modeset, which is why the caller reports this.
    """
    global active_device_profile_key
    if profile_key not in DEVICE_PROFILES:
        return False, "Unknown device profile"
    active_device_profile_key = profile_key
    return True, None


def letterbox_box(fb_w, fb_h):
    """Returns the phone-surface rect (left, top, right, bottom) in a fb_w x fb_h scanout.

    WHY: the destination rectangle is guest-derived, not Studio-owned. The canonical
    QEMU profile requests a native 720x1440 portrait scanout; this helper remains
    defensive for older profiles and derives the displayed guest rectangle from
    the live framebuffer geometry without inventing any phone UI.
    """
    if fb_w <= 0 or fb_h <= 0:
        return None
    if LOGICAL_SURFACE_W * fb_h <= LOGICAL_SURFACE_H * fb_w:
        dst_h = fb_h
        dst_w = max(1, min(fb_w, LOGICAL_SURFACE_W * fb_h // LOGICAL_SURFACE_H))
    else:
        dst_w = fb_w
        dst_h = max(1, min(fb_h, LOGICAL_SURFACE_H * fb_w // LOGICAL_SURFACE_W))
    off_x = (fb_w - dst_w) // 2
    off_y = (fb_h - dst_h) // 2
    return (off_x, off_y, off_x + dst_w, off_y + dst_h)


def current_framebuffer_size():
    return guest_framebuffer_size

def current_letterbox_box():
    return letterbox_box(*current_framebuffer_size())



def portrait_from_framebuffer(gx, gy):
    """Converts a guest framebuffer point into logical portrait coordinates.

    Returns None when the point falls outside the letterboxed phone surface, so a
    click on the surrounding desktop area is never mistaken for a touch on the
    device screen.
    """
    left, top, right, bottom = current_letterbox_box()
    if not (left <= gx < right and top <= gy < bottom):
        return None
    box_w = max(1, right - left)
    px = (gx - left) * LOGICAL_SURFACE_W // box_w
    py = (gy - top) * LOGICAL_SURFACE_H // (bottom - top)
    return (
        max(0, min(LOGICAL_SURFACE_W - 1, px)),
        max(0, min(LOGICAL_SURFACE_H - 1, py)),
    )



last_reboot_cursor = 0

def check_launcher_ready():
    global launcher_has_booted, last_reboot_cursor
    if launcher_has_booted:
        return True
    with telemetry_lock:
        recent = serial_lines[last_reboot_cursor:] if last_reboot_cursor < len(serial_lines) else []
        if any("[LAUNCHER]" in line for line in recent) or any("name=launcher" in line for line in recent):
            launcher_has_booted = True
            return True
        # If no reboot was requested or cursor is at start, check all lines
        if any("[LAUNCHER]" in line for line in serial_lines) or any("name=launcher" in line for line in serial_lines):
            launcher_has_booted = True
            return True
    # Disk boot log check if the serial_lines memory buffer was rotated. Only
    # the log this server rewrote for the CURRENT boot session may latch
    # readiness: a historical log from an earlier session describes a guest
    # that is not the one running now, so its [LAUNCHER] lines must not mark a
    # boot-looping guest as ready.
    cand_logs = [current_serial_log_path] if serial_log_is_session else []
    for log_path in cand_logs:
        if log_path and os.path.exists(log_path):
            try:
                with open(log_path, "r", encoding="utf-8", errors="replace") as f:
                    c = f.read()
                    if "[LAUNCHER] Step 5" in c or "[LAUNCHER]" in c or "name=launcher" in c:
                        launcher_has_booted = True
                        return True
            except Exception:
                pass
    return False

# Guest package and filesystem state are authoritative. Device Studio does not
# maintain a host-side filesystem shadow of guest state.

THEMES_DIR = os.path.join(DIRECTORY, "data", "themes")
os.makedirs(THEMES_DIR, exist_ok=True)
ACTIVE_THEME_FILE = os.path.join(THEMES_DIR, "_active_theme.json")

def get_active_theme():
    # Device Studio exposes the same stock system theme as the guest launcher.
    # Theme mutation is intentionally disabled until the system supports a
    # complete, authoritative theme service.
    return {
        "theme_id": "interos_stock",
        "name": "InterOS Stock",
        "version": 1,
        "bootloader": {
            "immutable_brand": "InterOS",
            "custom_subtext": ""
        },
        "ui": {
            "background": "#F7F8FA",
            "surface": "#FFFFFF",
            "text": "#0F172A",
            "muted": "#64748B",
            "accent": "#0284C7",
            "success": "#10B981",
            "danger": "#DC2626"
        }
    }

def set_active_theme(theme_data):
    # Kept as a compatibility boundary; the canonical stock theme cannot be
    # replaced by host-side state.
    return

# -----------------------------------------------------------------------------
# Real-Time Mobile Subsystems State (Cellular Baseband, PMIC/DVFS, KMS, Touch)
# -----------------------------------------------------------------------------
# Guest-backed telemetry only. Device Studio never invents hardware state.
telemetry_cache = {
    "telephony": {},
    "pmic": {},
    "kms": {},
    "touch": {},
}

def unavailable_subsystem(name):
    return {
        "status": "UNAVAILABLE",
        "source": "UNAVAILABLE",
        "reason": f"Guest has not reported {name} telemetry",
    }

def parse_key_values(line):
    values = {}
    for part in line.split():
        if "=" in part:
            key, value = part.split("=", 1)
            values[key] = value
    return values

def extract_guest_subsystem(name, prefix):
    latest = None
    for line in serial_lines:
        if line.startswith(prefix):
            payload = line[len(prefix):].strip()
            values = parse_key_values(payload)
            latest = {
                "status": "AVAILABLE",
                "source": "KERNEL_TELEMETRY",
                **values,
            }
    if latest is not None:
        telemetry_cache[name] = latest
        return latest
    return unavailable_subsystem(name)

def extract_real_processes():
    """Return only processes explicitly reported by the guest kernel."""
    tasks = []
    for line in serial_lines:
        if not line.startswith("[PROC]"):
            continue
        try:
            pdict = parse_key_values(line[6:].strip())
            tasks.append({
                "pid": int(pdict["pid"]),
                "name": pdict["name"],
                "state": pdict.get("state", "Unknown"),
                "ticks": int(pdict.get("ticks", 0)),
                "priority": int(pdict.get("prio", 0)),
                "ppid": int(pdict.get("ppid", 0)),
                "caps": int(pdict.get("caps", 0)),
                "memory": "Unavailable",
                "kernel_stack": pdict.get("kstack", "Unavailable"),
                "cr3": pdict.get("cr3", "Unavailable"),
                "source": "KERNEL_TELEMETRY",
            })
        except (KeyError, ValueError):
            continue
    seen = {}
    for task in tasks:
        seen[task["pid"]] = task
    return list(seen.values())

def extract_real_memory():
    """Return only memory state actually observed from the live guest."""
    t = query_qemu_monitor()
    alive = bool(t.get("qemu_alive"))
    if not alive:
        return {
            "qemu_alive": False,
            "source": "UNAVAILABLE",
            "cr3": None,
            "cr0": None,
            "cr4": None,
            "arch": None,
            "total_ram_bytes": None,
            "total_ram_mb": None,
            "heap_size_bytes": None,
            "heap_status": "Unavailable",
            "isolation": "Unavailable",
            "w_x_enforced": None,
            "nx_bit_active": None,
            "guard_pages": "Unavailable",
        }

    regs = t.get("cpu", {}).get("registers", {})
    return {
        "qemu_alive": True,
        "source": "QMP",
        "cr3": regs.get("CR3"),
        "cr0": regs.get("CR0"),
        "cr4": regs.get("CR4"),
        "arch": t.get("cpu", {}).get("arch"),
        "total_ram_bytes": None,
        "total_ram_mb": None,
        "heap_size_bytes": None,
        "heap_status": "Not reported by guest",
        "isolation": "Not reported by guest",
        "w_x_enforced": None,
        "nx_bit_active": None,
        "guard_pages": "Not reported by guest",
    }

def extract_real_ipc():
    """Return IPC events emitted by the live guest only."""
    events = []
    for line in serial_lines:
        if line.startswith("[IPC]") or "[IPC" in line:
            events.append({"raw": line.strip(), "time": time.strftime("%H:%M:%S")})
    return events

def extract_real_capabilities():
    """Return only capability records emitted by the guest."""
    capabilities = []
    for line in serial_lines:
        if not line.startswith("[CAP]"):
            continue
        values = parse_key_values(line[5:].strip())
        try:
            capabilities.append({
                "pid": int(values["pid"]),
                "task": values.get("task", "unknown"),
                "slot": int(values.get("slot", 0)),
                "type": values.get("type", "unknown"),
                "rights": values.get("rights", "unknown"),
                "object_id": int(values.get("object", values.get("object_id", 0))),
                "gen": int(values.get("gen", 0)),
                "state": values.get("state", "Unknown"),
                "source": "KERNEL_TELEMETRY",
            })
        except (KeyError, ValueError):
            continue
    return capabilities

def inspect_apk(file_bytes):
    """
    Strict AXML and DEX bytecode parser for Android (.apk) packages.
    Inspects ZIP structure, verifies Dalvik/ART header, extracts package ID,
    version code/name, declared permissions, and entrypoint activity.
    """
    metadata = {
        "is_valid_apk": False,
        "package_id": "com.android.app",
        "app_name": "Android Application",
        "version_name": "1.0.0",
        "version_code": 1,
        "dex_classes": 0,
        "dex_size": 0,
        "permissions": [],
        "main_activity": "",
        "signature_scheme": "None",
        "files_count": 0,
        "raw_size": len(file_bytes)
    }

    try:
        with zipfile.ZipFile(io.BytesIO(file_bytes), "r") as zf:
            namelist = zf.namelist()
            metadata["files_count"] = len(namelist)

            # Check signatures: modern APK Signature Scheme v2/v3 or v1 JAR signature
            if b'APK Sig Block 42' in file_bytes:
                metadata["signature_scheme"] = "APK Signature Block v2/v3 detected (not cryptographically verified)"
            else:
                sig_files = [n for n in namelist if n.startswith("META-INF/") and (n.endswith(".RSA") or n.endswith(".DSA") or n.endswith(".EC"))]
                if sig_files:
                    sig_name = sig_files[0].split('/')[-1]
                    metadata["signature_scheme"] = f"v1 JAR Signature detected ({sig_name}; not cryptographically verified)"
                else:
                    metadata["signature_scheme"] = "Signature metadata present (not cryptographically verified)"

            # Parse classes.dex
            dex_files = [n for n in namelist if n.endswith(".dex")]
            total_classes = 0
            total_dex_size = 0
            for df in dex_files:
                dex_data = zf.read(df)
                total_dex_size += len(dex_data)
                if len(dex_data) >= 112 and dex_data.startswith(b"dex\n"):
                    class_defs = struct.unpack('<I', dex_data[96:100])[0]
                    total_classes += class_defs
            metadata["dex_classes"] = total_classes
            metadata["dex_size"] = total_dex_size

            # Parse AndroidManifest.xml (Binary AXML)
            if "AndroidManifest.xml" in namelist:
                metadata["is_valid_apk"] = True
                manifest_raw = zf.read("AndroidManifest.xml")

                # Extract UTF-16LE and ASCII string pool entries
                utf16 = re.findall(b'(?:[\x20-\x7e]\x00){3,}', manifest_raw)
                utf16_str = [b.decode('utf-16le', errors='ignore').strip() for b in utf16]
                ascii_str = [s.decode('ascii', errors='ignore').strip() for s in re.findall(b'[\x20-\x7e]{4,}', manifest_raw)]
                all_strings = set(utf16_str + ascii_str)

                clean_strings = [re.sub(r'^[^\w.]+', '', s) for s in all_strings if s]

                # Extract permissions
                perms = set()
                for s in clean_strings:
                    if "permission" in s.lower() and "." in s and not s.endswith(".xml"):
                        clean_perm = re.search(r'([a-zA-Z0-9_]+(?:\.[a-zA-Z0-9_]+)+)', s)
                        if clean_perm and "permission" in clean_perm.group(1).lower():
                            perm_str = re.sub(r'^[0-9+*%#";,()\-]+', '', clean_perm.group(1))
                            if perm_str.startswith("android.permission.") or ".permission." in perm_str:
                                perms.add(perm_str)
                metadata["permissions"] = sorted(list(perms))

                # Extract root package name
                pkgs = [s for s in clean_strings if re.match(r'^(?:org|com|net|io|dev|app)\.[a-zA-Z0-9_]+(?:\.[a-zA-Z0-9_]+)$', s)]
                if pkgs:
                    non_system = [p for p in pkgs if not p.startswith("android.") and not p.startswith("androidx.")]
                    if non_system:
                        metadata["package_id"] = non_system[0]
                    else:
                        metadata["package_id"] = pkgs[0]

                # Extract Main Activity
                for s in clean_strings:
                    if "MainActivity" in s or ("Activity" in s and ("activities" in s.lower() or "main" in s.lower())):
                        act_match = re.search(r'([a-zA-Z0-9_]+(?:\.[a-zA-Z0-9_]+)+Activity)', s)
                        if act_match:
                            metadata["main_activity"] = re.sub(r'^[0-9+*%#";,()\-]+', '', act_match.group(1))
                            break

                # App Name derivation
                if "fossify" in metadata["package_id"].lower() or "filemanager" in metadata["package_id"].lower():
                    metadata["app_name"] = "Fossify File Manager"
                elif "signal" in metadata["package_id"].lower():
                    metadata["app_name"] = "Signal Messenger"
                elif metadata["package_id"]:
                    parts = metadata["package_id"].split(".")
                    metadata["app_name"] = " ".join([p.capitalize() for p in parts[-2:]])

                # Extract version
                for s in clean_strings:
                    ver_match = re.search(r'^v?(\d+\.\d+(?:\.\d+)?)$', s)
                    if ver_match:
                        metadata["version_name"] = ver_match.group(1)
                        break

    except Exception as ex:
        metadata["error"] = str(ex)

    return metadata

# Real-time hardware telemetry captured from QEMU QMP + Serial
latest_telemetry = {
    "qemu_alive": False,
    # The Studio's provable belief about the device: "running", "not_running"
    # or "unknown". "qemu_alive" is true only for "running".
    "device_state": "not_running",
    "device_reason": "No QEMU process is tracked by this Studio.",
    # The process this Studio launched, and whether it is provably alive.
    "qemu_process": {"tracked": False, "pid": None, "alive": False},
    # Secondary signals. They describe the host, never the device on their own.
    "monitor_answers": False,
    "vnc_port_busy": False,
    "websockify_alive": False,
    "status": "standby",
    "cpu": {
        "arch": "x86_64",
        "thread_id": "Offline",
        "halted": True,
        "rip": "Offline",
        "registers": {},
        "raw_registers": ""
    },
    "pic": {},
    "block": {
        "device": "standby",
        "file": "inter-os-bios.img (Standby)",
        "read_bytes": 0,
        "write_bytes": 0,
        "read_ops": 0
    },
    "kernel": {
        "uptime_ticks": 0,
        # No framebuffer dimensions are claimed until a live QEMU screendump
        # has actually been decoded. These are telemetry values, not a mode
        # preference or a host-display assumption.
        "framebuffer": {"width": None, "height": None, "bpp": None, "byte_len": None},
        "heap": {"pool_size": "1 MiB", "start": "0x444444440000", "status": "Standby"},
        "memory_regions": [],
        "tests_passed": []
    }
}



def init_default_serial_logs():
    """Start with an empty UART history; only guest-emitted lines are shown."""
    return


def get_permitted_ws_origins(port=None):
    """Returns the set of legitimate local Studio origins permitted to connect to websockify."""
    if port is None:
        port = PORT
    origins = [
        f"http://127.0.0.1:{port}",
        f"http://localhost:{port}",
        f"http://[::1]:{port}",
        f"https://127.0.0.1:{port}",
        f"https://localhost:{port}",
        f"https://[::1]:{port}",
    ]
    if port == 80:
        origins.extend(["http://127.0.0.1", "http://localhost", "http://[::1]"])
    elif port == 443:
        origins.extend(["https://127.0.0.1", "https://localhost", "https://[::1]"])
    return origins


def build_websockify_cmd(ws_port=None, vnc_port=None, studio_port=None):
    """Constructs the hardened websockify invocation enforcing strict origin validation."""
    if ws_port is None:
        target_ws = f"127.0.0.1:{WS_PORT}"
    else:
        target_ws = f"127.0.0.1:{ws_port}"
    if vnc_port is None:
        # The bridge must reach the guest the Studio actually launched, which
        # may sit on a different VNC display than the configured default.
        target_vnc = f"127.0.0.1:{active_vnc_port}"
    else:
        target_vnc = f"127.0.0.1:{vnc_port}"
    origins = get_permitted_ws_origins(studio_port)
    auth_source = " ".join(origins)
    return [
        sys.executable, "-m", "websockify",
        "--auth-plugin=ExpectOrigin",
        f"--auth-source={auth_source}",
        target_ws, target_vnc
    ]


def start_websockify_if_needed():
    """Starts websockify on WS_PORT (6081) bridging to the guest's VNC port with strict Origin checking."""
    global ws_subproc, ws_bridge_port
    # The bridge follows the guest: a guest launched onto a different display
    # needs a bridge that points at that display, not at the old port.
    if ws_subproc is not None and ws_bridge_port is not None and ws_bridge_port != active_vnc_port:
        try:
            ws_subproc.terminate()
            ws_subproc.wait(timeout=1)
        except Exception:
            pass
        ws_subproc = None
        ws_bridge_port = None
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.settimeout(0.15)
            if s.connect_ex(('127.0.0.1', WS_PORT)) == 0:
                latest_telemetry["websockify_alive"] = True
                return
    except Exception:
        pass

    try:
        cmd = build_websockify_cmd()
        ws_subproc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        ws_bridge_port = active_vnc_port
        time.sleep(0.3)
        latest_telemetry["websockify_alive"] = True
        print(f"[*] Live Bare-Metal VBE bridge active on ws://127.0.0.1:{WS_PORT} -> VNC :{active_vnc_port}")
    except Exception as e:
        print(f"[*] Notice: websockify auto-start: {e}")
        latest_telemetry["websockify_alive"] = False

current_serial_log_path = os.path.join(os.path.dirname(DIRECTORY), "target", "x86_boot.log")
# True only while current_serial_log_path names a log THIS server rewrote for
# the QEMU instance it launched. The workspace also carries a historical
# target/x86_boot.log from earlier sessions (tens of thousands of lines,
# including [LAUNCHER] markers from healthy boots); merging that file into the
# live serial stream served a stale tail as live guest output and latched
# launcher readiness for a guest that never presented its launcher.
serial_log_is_session = False

def serial_stream_worker():
    """Background worker continuously pulling UART lines from QEMU COM1 TCP port (4201).

    Reconnects with short backoff whenever the connection dies, errors, or a
    QEMU relaunch bumps `serial_epoch`; never exits.
    """
    global serial_lines, serial_socket_conn, current_serial_log_path
    buf = ""
    backoff = 0.25
    while True:
        conn = None
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                conn = s
                s.settimeout(1.5)
                s.connect(('127.0.0.1', SERIAL_TCP_PORT))
                try:
                    # Windows: detect a peer killed without FIN/RST within ~3s
                    # instead of spinning on recv timeouts forever.
                    s.ioctl(socket.SIO_KEEPALIVE_VALS, (1, 2000, 500))
                except (OSError, AttributeError):
                    pass
                with serial_socket_lock:
                    serial_socket_conn = s
                backoff = 0.25
                seen_epoch = serial_epoch
                s.settimeout(0.8)
                while True:
                    if serial_epoch != seen_epoch:
                        break  # QEMU relaunched: drop this stale single-client
                    try:
                        chunk = s.recv(1024)
                        if not chunk:
                            break
                        decoded = chunk.decode('utf-8', errors='replace')
                        buf += decoded
                        while '\n' in buf:
                            line, buf = buf.split('\n', 1)
                            clean = line.strip('\r').strip()
                            if clean:
                                with telemetry_lock:
                                    serial_lines.append(clean)
                                    if len(serial_lines) > 20000:
                                        serial_lines = serial_lines[-10000:]
                                if current_serial_log_path:
                                    try:
                                        with open(current_serial_log_path, "a", encoding="utf-8", errors="replace") as f:
                                            f.write(clean + "\n")
                                    except Exception:
                                        pass
                    except socket.timeout:
                        continue  # quiet guest is fine; epoch/keepalive checked
        except Exception as e:
            print(f"[serial] reconnect in {backoff:.2f}s: {e}")
            time.sleep(backoff)
            backoff = min(backoff * 2, 1.5)
        finally:
            with serial_socket_lock:
                if serial_socket_conn is conn:
                    serial_socket_conn = None  # never leave a closed fd published


def send_guest_serial_command(command, wait_for=None, timeout=12):
    """Send one bounded development command through the live QEMU COM1 TCP connection."""
    if not isinstance(command, str) or not command or len(command) > 512 or "\n" in command or "\r" in command:
        return False, "guest command rejected by transport bounds"
    deadline = time.time() + timeout
    with telemetry_lock:
        response_start = len(serial_lines)
    while time.time() < deadline:
        with serial_socket_lock:
            conn = serial_socket_conn
            if conn is not None:
                try:
                    conn.sendall((command.rstrip("\n") + "\n").encode("utf-8"))
                    break
                except OSError:
                    pass
        time.sleep(0.1)
    else:
        return False, "guest serial transport unavailable"

    if not wait_for:
        return True, "sent"

    start = response_start
    while time.time() < deadline:
        with telemetry_lock:
            recent = serial_lines[start:]
        for line in recent:
            if wait_for in line:
                if "REJECT" in line or "FAILED" in line:
                    return False, line
                return True, line
        time.sleep(0.05)
    return False, f"timeout waiting for guest response: {wait_for}"


def transfer_rapp_to_guest(package_bytes, package_id, timeout=30):
    if len(package_bytes) > 60000:
        return False, "RAPP exceeds guest package blob limit"
    ok, msg = send_guest_serial_command(
        f"RAPP_BEGIN {package_id} {len(package_bytes)}",
        "[RAPP_HOST] READY",
        timeout=5,
    )
    if not ok:
        return False, msg

    # COM1 command lines are deliberately bounded. 24 bytes -> 48 hex chars,
    # plus command framing, stays below the kernel RX command buffer.
    for offset in range(0, len(package_bytes), 24):
        chunk = package_bytes[offset:offset + 24]
        ok, msg = send_guest_serial_command(
            f"RAPP_CHUNK {offset} {chunk.hex()}",
            "[RAPP_HOST] CHUNK",
            timeout=5,
        )
        if not ok:
            return False, msg

    return send_guest_serial_command("RAPP_COMMIT", "[RAPP_HOST] INSTALLED", timeout=timeout)


WORKSPACE_ROOT = os.path.dirname(DIRECTORY)

def _resolve_artifact(*candidates):
    """Returns the newest existing artifact among the given relative paths.

    The standalone kernels are excluded from the Cargo workspace, so their real
    build output lives under their own crate's target directory while a stale
    copy may still sit in the workspace target directory. Picking the newest
    existing file avoids booting an artifact that is no longer being built.
    """
    best, best_mtime = None, None
    for rel in candidates:
        path = os.path.join(WORKSPACE_ROOT, rel)
        if not os.path.exists(path):
            continue
        try:
            mtime = os.path.getmtime(path)
        except OSError:
            continue
        if best_mtime is None or mtime > best_mtime:
            best, best_mtime = path, mtime
    return best


def ensure_data_disk(path, size_bytes=512 * 1024 * 1024):
    """Creates the guest's data disk if it does not exist yet.

    The guest formats whatever disk it is handed as its own filesystem. Pointing
    it at the boot image destroys the bootloader stages the kernel is embedded
    in, so the next boot runs a corrupted kernel.
    """
    try:
        if os.path.exists(path) and os.path.getsize(path) >= size_bytes:
            return path
    except OSError:
        pass
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    qemu_img = shutil.which("qemu-img")
    if qemu_img:
        subprocess.run([qemu_img, "create", "-f", "raw", path, str(size_bytes)],
                       check=True, capture_output=True)
    else:
        with open(path, "wb") as handle:
            handle.truncate(size_bytes)
    return path


SUPPORTED_PROFILES = {
    "x86_64": {
        "name": "InterOS x86_64 PC / VM",
        "arch": "x86_64",
        "machine": "q35",
        "cpu": "qemu64",
        "ram": 2048,
        "boot_type": "BIOS/MBR Bootloader",
        "image": os.path.join(WORKSPACE_ROOT, "target", "inter-os-bios.img"),
        "status": "Primary QEMU Phone contract",
        "display": {"backend": "vnc", "host": "127.0.0.1", "port": VNC_PORT, "display_index": VNC_DISPLAY, "websocket_port": WS_PORT, "guest_width": 1080, "guest_height": 2160},
        "serial": {"host": "127.0.0.1", "port": 4301},
        "monitor": {"host": "127.0.0.1", "port": 44445},
    },
    "aarch64": {
        "name": "InterOS AArch64 Virt Mobile",
        "arch": "aarch64",
        "machine": "virt",
        "cpu": "cortex-a53",
        "ram": 512,
        "boot_type": "Direct Kernel ELF",
        # Prefer the flat arm64 Image: the arm64 boot protocol hands the device
        # tree to the kernel in x0, whereas an ELF is entered directly with
        # x0 = 0 and no device tree at all.
        "image": _resolve_artifact(
            os.path.join("target", "kernel-aarch64.img"),
            os.path.join("kernel-aarch64", "target", "aarch64-unknown-none", "debug", "kernel-aarch64.img"),
            os.path.join("kernel-aarch64", "target", "aarch64-unknown-none", "debug", "kernel-aarch64"),
            os.path.join("target", "aarch64-unknown-none", "debug", "kernel-aarch64"),
        ),
        "status": "Source-level architecture target; runtime validation pending",
    },
    "riscv64": {
        "name": "InterOS RISC-V Virt Mobile",
        "arch": "riscv64",
        "machine": "virt",
        "cpu": "rv64",
        "ram": 256,
        "boot_type": "OpenSBI / Direct Kernel ELF",
        "image": _resolve_artifact(
            os.path.join("kernel-riscv64", "target", "riscv64gc-unknown-none-elf", "debug", "kernel-riscv64"),
            os.path.join("target", "riscv64gc-unknown-none-elf", "debug", "kernel-riscv64"),
        ),
        "status": "Source-level architecture target; runtime validation pending",
    },
}
active_profile_key = "x86_64"


# Where the last find_qemu_executable() look came up empty, so the launch error
# can name every place it checked instead of a vague "not found on host".
_qemu_search_log = []


def find_qemu_executable(arch):
    """Locate the QEMU system binary for `arch` on this host.

    WHY an explicit search rather than only shutil.which: Device Studio is
    expected to run on Ubuntu and other Linux/macOS hosts (and on other
    open-source users' own machines), launched from arbitrary working
    directories, so QEMU may live in a prefix bin dir that is not on PATH. All
    discovery is cross-platform via os.path/shutil so no Windows-only assumption
    leaks into the Linux/macOS paths. Search order:

      1. An explicit override — INTEROS_QEMU (absolute path to the binary OR its
         directory), then QEMU_PATH / QEMU_DIR (the names tools/qemu_env.ps1
         already honours on Windows). When one is set it is authoritative:
         Studio runs exactly the QEMU that was pinned, or reports the miss,
         rather than silently substituting a different build found on PATH.
      2. PATH via shutil.which (handles Windows PATHEXT / .exe resolution).
      3. Common per-OS install prefixes.

    Returns an executable path or None; _qemu_search_log records what was tried
    so the caller can fail honestly and name the locations.
    """
    global _qemu_search_log
    bin_name = f"qemu-system-{arch}"
    is_win = sys.platform == "win32"
    exe_variants = [bin_name, f"{bin_name}.exe"] if is_win else [bin_name]

    def usable(path):
        # Only a real executable file counts, so a directory or an unrelated
        # same-named file is never handed to Popen.
        if not path or not os.path.isfile(path):
            return False
        return True if is_win else os.access(path, os.X_OK)

    _qemu_search_log = [f"{bin_name} in PATH"]
    # 1. Explicit override (authoritative when set).
    override_log = []
    for env_name in ("INTEROS_QEMU", "QEMU_PATH", "QEMU_DIR"):
        raw = os.environ.get(env_name)
        if not raw:
            continue
        base = os.path.expanduser(os.path.expandvars(raw))
        # Accept either the binary itself or the directory containing it, with
        # or without the Windows .exe suffix.
        candidates = [base] + [os.path.join(base, name) for name in exe_variants]
        for cand in candidates:
            if usable(cand):
                return cand
        override_log.append(f"{env_name}={base}")
    if override_log:
        # An override that points nowhere is an honest failure: do not fall
        # back to PATH and run a QEMU the operator did not pin.
        _qemu_search_log = [f"{bin_name} via {entry}" for entry in override_log]
        return None

    # 2. PATH.
    on_path = shutil.which(bin_name)
    if on_path and usable(on_path):
        return on_path

    # 3. Common per-OS install prefixes (no Windows path is consulted on POSIX).
    if is_win:
        prefixes = [
            os.path.join(os.environ.get("ProgramFiles", r"C:\Program Files"), "qemu"),
            r"C:\qemu",
            os.path.join(os.path.expanduser("~"), "qemu"),
        ]
    elif sys.platform == "darwin":
        prefixes = [
            "/opt/homebrew/bin",       # Apple Silicon Homebrew
            "/usr/local/bin",           # Intel Homebrew / MacPorts
            "/usr/bin",
            os.path.join(os.path.expanduser("~"), "qemu"),
        ]
    else:  # Linux and other POSIX hosts
        prefixes = [
            "/usr/bin",
            "/usr/local/bin",
            "/usr/libexec",            # some distros install helpers here
            "/opt/qemu/bin",
            os.path.join(os.path.expanduser("~"), "qemu"),
        ]
    for d in prefixes:
        for name in exe_variants:
            cand = os.path.join(d, name)
            if usable(cand):
                return cand
            _qemu_search_log.append(cand)
    return None


_persistent_monitor_sock = None
_persistent_monitor_lock = threading.Lock()

def send_qemu_monitor_cmd(cmd_str, host='127.0.0.1', port=MONITOR_PORT, timeout=0.8):
    """Sends a raw command to QEMU HMP monitor and returns the response using persistent connection."""
    global _persistent_monitor_sock
    with _persistent_monitor_lock:
        for attempt in range(2):
            if _persistent_monitor_sock is None:
                try:
                    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                    s.settimeout(timeout)
                    s.connect((host, port))
                    banner = b''
                    while b'(qemu)' not in banner:
                        chunk = s.recv(1024)
                        if not chunk: break
                        banner += chunk
                    _persistent_monitor_sock = s
                except Exception as e:
                    _persistent_monitor_sock = None
                    if attempt == 1:
                        return False, str(e)
                    continue
            try:
                _persistent_monitor_sock.sendall(cmd_str.strip().encode('utf-8') + b'\n')
                resp = b''
                while b'(qemu)' not in resp:
                    chunk = _persistent_monitor_sock.recv(4096)
                    if not chunk:
                        raise ConnectionResetError("QEMU monitor closed connection")
                    resp += chunk
                return True, resp.decode('utf-8', errors='replace').strip()
            except Exception as e:
                try:
                    if _persistent_monitor_sock:
                        _persistent_monitor_sock.close()
                except Exception:
                    pass
                _persistent_monitor_sock = None
                if attempt == 1:
                    return False, str(e)
        return False, "Failed to send monitor command"


def reset_monitor_connection():
    """Drops the persistent monitor socket.

    WHY: the socket belongs to the QEMU instance it connected to. After a
    relaunch it would either point at a dead monitor or, worse, at a monitor
    this Studio does not own. Dropping it forces the next command to open a
    fresh connection to the guest that is actually running.
    """
    global _persistent_monitor_sock
    with _persistent_monitor_lock:
        sock = _persistent_monitor_sock
        _persistent_monitor_sock = None
    if sock is not None:
        try:
            sock.close()
        except OSError:
            pass


current_device_orientation = 'portrait'
_capture_lock = threading.Lock()
_last_frame_cache = {}
_transport_metrics = {
    'fps': 0.0,
    'frame_us': 0,
    'frames_in_second': 0,
    'second_start': time.time(),
}


class VncScanoutClient:
    """
    Persistent TCP RFB/VNC client connected directly to QEMU's live scanout.
    Eliminates all disk I/O, PPM files, and synchronous HMP screendump stalls.
    Maintains an in-memory scanout buffer, extracts phone viewport, and streams
    raw binary frames and memory-cached images at high refresh rates.
    """
    def __init__(self, host='127.0.0.1', port=VNC_PORT):
        self.host = host
        self.port = port
        self.running = True
        self.connected = False
        self.width = LOGICAL_SURFACE_W
        self.height = LOGICAL_SURFACE_H
        self.scanout_buf = bytearray(LOGICAL_SURFACE_W * LOGICAL_SURFACE_H * 4)
        self.seq = 0
        self.lock = threading.Lock()
        self.frame_cond = threading.Condition(self.lock)
        self.latest_raw_rgba = None
        self.latest_raw_w = 0
        self.latest_raw_h = 0
        self.latest_raw_seq = 0
        self.latest_raw_time = 0.0
        self.latest_binary_msg = None
        self.latest_jpeg = None
        self.latest_png = None
        self._last_published_rgba = None
        # When a portrait session's desktop was last confirmed to be the guest's
        # real geometry, as a timestamp. WHY a timestamp and not a flag: the
        # guest genuinely does present 1280x720 for the first moments of boot,
        # before it engages native portrait scanout, so an early probe can
        # confirm a geometry that is seconds away from changing. A permanent
        # flag kept the publisher cropping the boot framebuffer for the rest of
        # the session, which is the failure this whole check exists to stop.
        self._portrait_mismatch_confirmed_at = 0.0
        self._last_scanout_probe = 0.0
        self.metrics = {
            'fps': 0.0,
            'frame_us': 0,
            'frames_total': 0,
            'dropped_frames': 0,
            'second_start': time.time(),
            'frames_in_second': 0,
        }
        self._force_reconnect = False

    def start(self):
        t = threading.Thread(target=self._worker_loop, daemon=True)
        t.name = "VncScanoutWorker"
        t.start()

    def force_reconnect(self):
        """Drop the current RFB session so the worker re-handshakes the new QEMU."""
        self._force_reconnect = True
        # WHY no `with self.frame_cond` here: _invalidate_cached_frame takes
        # it itself, and the condition wraps a non-reentrant lock, so nesting
        # self-deadlocks the caller while holding the lock once. That wedged
        # every frame consumer and the relaunch itself: the stop path runs
        # inside the launch POST, so one nested acquire froze /api/screen/*
        # and every later launch on this server process.
        self.connected = False
        self._invalidate_cached_frame()

    def set_port(self, port):
        """Point this client at the VNC port the launched guest actually uses.

        WHY: a launch may move the guest onto a different display when the
        configured one is taken. The client must follow the guest, and it must
        not keep talking to whatever else is listening on the old port.
        """
        try:
            port = int(port)
        except (TypeError, ValueError):
            return
        if port == self.port:
            return
        self.port = port
        self.force_reconnect()

    def _invalidate_cached_frame(self):
        """Drops the published frame so consumers see 503, not stale pixels."""
        with self.frame_cond:
            self.latest_raw_rgba = None
            self.latest_raw_w = 0
            self.latest_raw_h = 0
            self.latest_binary_msg = None
            self.latest_jpeg = None
            self.latest_png = None
            self._last_published_rgba = None
            self.frame_cond.notify_all()

    def _monitor_scanout_size(self):
        """The guest scanout geometry as QEMU's own monitor reports it.

        WHY the monitor and not this session: an RFB desktop size is fixed at
        handshake, so it only ever describes the surface that existed when the
        session opened. A screendump is sized from the live console surface,
        which is the ground truth for whether this session is describing the
        present or a boot-time surface. Returns None when the probe fails, and
        the caller then keeps publishing what it has.
        """
        now = time.time()
        # A screendump writes the whole surface, so a session that is genuinely
        # off-geometry is probed at most twice a second, not once per frame.
        if now - self._last_scanout_probe < 2.0:
            return None
        self._last_scanout_probe = now
        path = os.path.join(WORKSPACE_ROOT, "target", "scanout_probe.ppm").replace("\\", "/")
        try:
            ok, _ = send_qemu_monitor_cmd(f'screendump {path}')
            if not ok or not os.path.exists(path):
                return None
            with open(path, "rb") as f:
                header = f.read(64)
            # P6 header: magic, width, height, maxval.
            parts = header.split()
            if len(parts) < 3 or parts[0] != b"P6":
                return None
            return (int(parts[1]), int(parts[2]))
        except Exception:
            return None
        finally:
            try:
                os.remove(path)
            except Exception:
                pass

    def _resync_stale_session(self, reason):
        """Drops an RFB session that describes a surface QEMU no longer has."""
        print(f"[vnc] {reason}; re-handshaking")
        self._force_reconnect = True
        self.connected = False
        self._invalidate_cached_frame()

    def _abort_session(self, s, reason):
        """Tears down a stale session and forces a resync instead of publishing it."""
        print(f"[vnc] {reason}; tearing down and resyncing")
        self._invalidate_cached_frame()
        try:
            s.close()
        except Exception:
            pass
        self.connected = False
        time.sleep(0.75)
        raise ConnectionError(reason)

    def _worker_loop(self):
        backoff = 0.5
        while self.running:
            try:
                self._run_session()
                backoff = 0.5
            except NoTrackedQemu as e:
                # There is no guest to attach to. That is the normal state of a
                # Studio with the device stopped, not a failure worth logging
                # on every tick.
                self.connected = False
                time.sleep(0.5)
                continue
            except Exception as e:
                print(f"[vnc] session error, retry in {backoff:.2f}s: {e}")
                self.connected = False
                time.sleep(backoff)
                backoff = min(backoff * 2, 2.0)
                continue
            finally:
                self.connected = False
            if self._force_reconnect:
                self._force_reconnect = False
                backoff = 0.25
            time.sleep(backoff)

    def _recv_exact(self, s, n):
        buf = bytearray(n)
        view = memoryview(buf)
        pos = 0
        while pos < n:
            chunk = s.recv_into(view[pos:])
            if chunk == 0:
                raise EOFError("Socket closed")
            pos += chunk
        return bytes(buf)

    def _run_session(self):
        # WHY the process check comes before the connection: the VNC port can
        # be held by something that is not the guest (a stale listener, another
        # program). Attaching to it would publish another program's pixels as
        # the phone screen, so the Studio only ever opens an RFB session to
        # the QEMU process it launched.
        if not tracked_qemu_process_alive():
            raise NoTrackedQemu("no QEMU process launched by this Studio is running")
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.settimeout(3.0)
        s.connect((self.host, self.port))

        ver = self._recv_exact(s, 12)
        if not ver.startswith(b'RFB'):
            s.close()
            return
        s.sendall(b'RFB 003.008\n')

        cnt = self._recv_exact(s, 1)
        if not cnt:
            s.close()
            return
        self._recv_exact(s, cnt[0])
        s.sendall(b'\x01')  # Security: None
        sec_res = self._recv_exact(s, 4)
        if sec_res != b'\x00\x00\x00\x00':
            s.close()
            return

        s.sendall(b'\x01')  # Shared session

        sinit = self._recv_exact(s, 24)
        w, h = struct.unpack('>HH', sinit[:4])
        name_len = struct.unpack('>I', sinit[20:24])[0]
        if name_len > 65536:
            self._abort_session(s, f"VNC server name too large ({name_len})")
        if name_len > 0:
            self._recv_exact(s, name_len)

        if w <= 0 or h <= 0 or w > MAX_SCANOUT_DIM or h > MAX_SCANOUT_DIM or not is_canonical_scanout(w, h):
            # Stale or transitional session: never publish crops from it.
            self._abort_session(s, f"non-canonical VNC desktop size {w}x{h}")
        self.width = w
        self.height = h
        global guest_framebuffer_size
        guest_framebuffer_size = (w, h)
        self.scanout_buf = bytearray(w * h * 4)
        # A new handshake is the session's own answer about the live surface,
        # so any cached verdict about an off-geometry desktop starts over.
        self._portrait_mismatch_confirmed_at = 0.0
        self._last_scanout_probe = 0.0
        self.connected = True

        # Encodings: Raw (0), DesktopSize (-239)
        s.sendall(struct.pack('>BxHii', 2, 2, 0, -239))

        # Request initial full update
        s.sendall(struct.pack('>BBHHHH', 3, 0, 0, 0, self.width, self.height))
        s.settimeout(0.8)

        while self.running and not self._force_reconnect:
            try:
                hdr = self._recv_exact(s, 4)
                msg_type, _, num_rects = struct.unpack('>BBH', hdr)
                if msg_type != 0:
                    continue

                t_frame_start = time.time()
                for _ in range(num_rects):
                    rh = self._recv_exact(s, 12)
                    rx, ry, rw, rh_val, enc = struct.unpack('>HHHHi', rh)
                    if enc == -239:
                        if (rw <= 0 or rh_val <= 0 or rw > MAX_SCANOUT_DIM or rh_val > MAX_SCANOUT_DIM
                                or not is_canonical_scanout(rw, rh_val)):
                            self._abort_session(s, f"non-canonical VNC desktop size {rw}x{rh_val}")
                        self.width, self.height = rw, rh_val
                        guest_framebuffer_size = (rw, rh_val)
                        self.scanout_buf = bytearray(rw * rh_val * 4)
                        continue
                    if enc != 0:
                        # Only Raw was negotiated; anything else cannot be
                        # decoded, so resync rather than desynchronise the stream.
                        self._abort_session(s, f"unsupported VNC encoding {enc}")
                    if (rw <= 0 or rh_val <= 0 or rx + rw > self.width or ry + rh_val > self.height):
                        self._abort_session(s, "VNC rect out of bounds")
                    total_bytes = rw * rh_val * 4
                    mv = memoryview(self.scanout_buf)
                    if rw == self.width and rx == 0:
                        dst_offset = ry * self.width * 4
                        pos = 0
                        while pos < total_bytes:
                            n = s.recv_into(mv[dst_offset + pos : dst_offset + total_bytes], min(131072, total_bytes - pos))
                            if n == 0:
                                raise EOFError()
                            pos += n
                    else:
                        temp_buf = bytearray(total_bytes)
                        t_mv = memoryview(temp_buf)
                        pos = 0
                        while pos < total_bytes:
                            n = s.recv_into(t_mv[pos:], min(131072, total_bytes - pos))
                            if n == 0:
                                raise EOFError()
                            pos += n
                        for row in range(rh_val):
                            src_off = row * rw * 4
                            dst_off = ((ry + row) * self.width + rx) * 4
                            self.scanout_buf[dst_off : dst_off + rw * 4] = temp_buf[src_off : src_off + rw * 4]

                self._publish_frame(t_frame_start)

                # WHY re-request here: each VNC update request is one-shot, so
                # without an incremental request after every update the next
                # damage waits for the 0.8 s read timeout to re-arm the
                # stream, and every tap visibly lags ~1 s. Incremental
                # requests carry only dirty rectangles, so the steady state
                # costs almost nothing and interaction stays live.
                s.sendall(struct.pack('>BBHHHH', 3, 1, 0, 0, self.width, self.height))
            except socket.timeout:
                if self._force_reconnect:
                    break
                try:
                    s.sendall(struct.pack('>BBHHHH', 3, 1, 0, 0, self.width, self.height))
                except Exception:
                    break
            except Exception:
                break
        try:
            s.close()
        except Exception:
            pass
        # The session is gone: withdraw its pixels so the screen routes answer
        # 503 instead of serving a dead session's last frame as live.
        self._invalidate_cached_frame()
        self.connected = False

    def _publish_frame(self, t_start):
        now = time.time()
        w, h = self.width, self.height
        is_landscape = (current_device_orientation == 'landscape')
        # WHY resync a session whose desktop is not the native portrait surface:
        # the phone profile starts QEMU on the fallback framebuffer and the guest
        # engages native portrait scanout once paging is live, so a session
        # opened in that window keeps reporting the old geometry. The publisher
        # then cropped that stale desktop into a 360x720 slice and served the
        # whole phone half-size and zoomed. When the monitor reports the live
        # surface IS native portrait, this session is describing a surface that
        # no longer exists, so it is dropped and re-handshaked rather than
        # published. A guest that really is landscape keeps its slice, but that
        # answer only stands for CONFIRMATION_TTL seconds: during boot the guest
        # really is 1280x720 for a moment, and a verdict that never expires
        # would keep the stale crop forever. An unknown answer is not a verdict,
        # so a failed probe leaves the existing pixels alone.
        # The mismatch gate compares the live portrait scanout against the
        # ACTIVE device profile's declared geometry (720x1440 for the reference
        # profile) — the same geometry launch_qemu_profile asked EDID for. That
        # is what makes a non-reference profile fail honestly: if the guest does
        # not produce the geometry it was asked for, the frame is withheld and
        # the route answers 503 rather than cropping to a wrong scanout.
        expected_w, expected_h = device_guest_geometry()
        if not is_landscape and (w, h) != (expected_w, expected_h) and (w, h) != (1088, 2160):
            confirmed_for = now - self._portrait_mismatch_confirmed_at
            if self._portrait_mismatch_confirmed_at and confirmed_for >= CONFIRMATION_TTL:
                self._portrait_mismatch_confirmed_at = 0.0
            if not self._portrait_mismatch_confirmed_at:
                live = self._monitor_scanout_size()
                if live == (expected_w, expected_h):
                    self._resync_stale_session(
                        f"session desktop {w}x{h} but live scanout is "
                        f"{expected_w}x{expected_h}")
                    return
                if live is not None:
                    self._portrait_mismatch_confirmed_at = now
        box = letterbox_box(w, h)
        if not box:
            return

        if now - self.latest_raw_time < 0.033:
            return

        off_x, off_y, x2, y2 = box
        crop_w = x2 - off_x
        crop_h = y2 - off_y

        try:
            import numpy as np
            arr = np.frombuffer(self.scanout_buf, dtype=np.uint8).reshape((h, w, 4))
            if is_landscape:
                crop = arr
                crop_w, crop_h = w, h
            else:
                crop = arr[off_y:y2, off_x:x2]
            rgba = crop[:, :, [2, 1, 0, 3]].tobytes()
        except Exception:
            # Fallback pure-python slice & BGRA->RGBA
            out = bytearray(crop_w * crop_h * 4)
            for row in range(crop_h):
                src_row = ((off_y + row) * w + off_x) * 4
                dst_row = row * crop_w * 4
                for col in range(crop_w):
                    s_idx = src_row + col * 4
                    d_idx = dst_row + col * 4
                    out[d_idx] = self.scanout_buf[s_idx + 2]      # R
                    out[d_idx + 1] = self.scanout_buf[s_idx + 1]  # G
                    out[d_idx + 2] = self.scanout_buf[s_idx]      # B
                    out[d_idx + 3] = 0xFF                         # A
            rgba = bytes(out)

        # WHY skip identical frames: an idle guest answers the long-poll
        # re-request with an empty update, and publishing it would tick the
        # sequence, push megabytes down every stream and re-render the
        # monitor for zero changed pixels. Only real damage advances the
        # stream, so transport fps means visible changes per second.
        if rgba == self._last_published_rgba:
            return
        self._last_published_rgba = rgba

        with self.frame_cond:
            self.seq += 1
            self.latest_raw_rgba = rgba
            self.latest_raw_w = crop_w
            self.latest_raw_h = crop_h
            self.latest_raw_seq = self.seq
            self.latest_raw_time = now
            ts_ms = int(now * 1000) & 0xFFFFFFFF
            hdr = struct.pack('>4sHHII', b'IOSF', crop_w, crop_h, self.seq, ts_ms)
            self.latest_binary_msg = hdr + rgba
            self.latest_jpeg = None
            self.latest_png = None

            elapsed_us = int((now - t_start) * 1_000_000)
            self.metrics['frame_us'] = elapsed_us
            self.metrics['frames_total'] += 1
            self.metrics['frames_in_second'] += 1
            dt = now - self.metrics['second_start']
            if dt >= 1.0:
                self.metrics['fps'] = self.metrics['frames_in_second'] / dt if dt <= 3.0 else float(self.metrics['frames_in_second'])
                self.metrics['frames_in_second'] = 0
                self.metrics['second_start'] = now
            _transport_metrics['fps'] = self.metrics['fps']
            _transport_metrics['frame_us'] = elapsed_us

            self.frame_cond.notify_all()

    def get_binary_frame(self, client_seq=0, timeout=0.040):
        with self.frame_cond:
            if self.seq <= client_seq and self.connected:
                self.frame_cond.wait(timeout)
            if self.seq <= client_seq:
                return None, self.seq
            return self.latest_binary_msg, self.seq

    def get_image_frame(self, prefer_jpeg=True):
        with self.frame_cond:
            if not self.latest_raw_rgba:
                return None, None
            if prefer_jpeg and self.latest_jpeg:
                return self.latest_jpeg, "image/jpeg"
            if not prefer_jpeg and self.latest_png:
                return self.latest_png, "image/png"

            w, h = self.latest_raw_w, self.latest_raw_h
            rgba = self.latest_raw_rgba

        from PIL import Image
        img = Image.frombuffer("RGBA", (w, h), rgba, "raw", "RGBA", 0, 1)
        rgb_img = img.convert("RGB")
        buf = io.BytesIO()
        if prefer_jpeg:
            rgb_img.save(buf, format="JPEG", quality=90)
            data = buf.getvalue()
            with self.frame_cond:
                self.latest_jpeg = data
            return data, "image/jpeg"
        else:
            rgb_img.save(buf, format="PNG", compress_level=1)
            data = buf.getvalue()
            with self.frame_cond:
                self.latest_png = data
            return data, "image/png"


vnc_client = VncScanoutClient()
vnc_client.start()


def capture_qemu_screen_frame(full_scanout=False, orientation=None, prefer_jpeg=True):
    """
    Captures live QEMU scanout buffer. Prefers persistent VNC memory buffer.
    Falls back to screendump only when VNC client is not yet established.
    """
    global _last_frame_cache, current_device_orientation, _transport_metrics
    now = time.time()
    # The active device profile declares the portrait geometry Studio expects
    # the guest to present (720x1440 for the reference profile), matching the
    # EDID it requested. A live scanout that differs is reported as-is — never
    # cropped to an expected geometry — so a mis-specified profile fails closed.
    expected_w, expected_h = device_guest_geometry()

    # Fast path: in-memory VNC client (zero disk I/O, zero PPM)
    if vnc_client.connected and vnc_client.latest_raw_rgba is not None:
        if (vnc_client.latest_raw_w, vnc_client.latest_raw_h) != (expected_w, expected_h):
            return None, f"Non-canonical VNC scanout {vnc_client.latest_raw_w}x{vnc_client.latest_raw_h}; expected {expected_w}x{expected_h}"
        img_bytes, content_type = vnc_client.get_image_frame(prefer_jpeg=prefer_jpeg)
        if img_bytes:
            return img_bytes, content_type

    # Fallback path if VNC not ready: monitor screendump
    if not _capture_lock.acquire(blocking=False):
        cache_key = 'portrait_jpeg' if prefer_jpeg else 'portrait'
        if _last_frame_cache.get(cache_key):
            return _last_frame_cache[cache_key], ("image/jpeg" if prefer_jpeg else "image/png")
        return None, "Capture busy"

    t0 = time.time()
    try:
        dump_dir = os.path.join(WORKSPACE_ROOT, "target")
        os.makedirs(dump_dir, exist_ok=True)
        ppm_path = os.path.join(dump_dir, "live_scanout.ppm").replace("\\", "/")

        ok, resp = send_qemu_monitor_cmd(f'screendump {ppm_path}')
        if not ok or not os.path.exists(ppm_path):
            cache_key = 'portrait_jpeg' if prefer_jpeg else 'portrait'
            if _last_frame_cache.get(cache_key):
                return _last_frame_cache[cache_key], ("image/jpeg" if prefer_jpeg else "image/png")
            return None, f"QEMU screendump fallback failed: {resp}"

        from PIL import Image
        with Image.open(ppm_path) as img:
            width, height = img.size
            if (width, height) != (expected_w, expected_h):
                return None, f"Non-canonical guest scanout {width}x{height}; expected {expected_w}x{expected_h}"
            # Native portrait scanout: no Studio crop, scale, or phone-shell substitution.
            buf = io.BytesIO()
            if prefer_jpeg:
                img.save(buf, format="JPEG", quality=92)
                encoded_bytes = buf.getvalue()
                content_type = "image/jpeg"
            else:
                img.save(buf, format="PNG", compress_level=1)
                encoded_bytes = buf.getvalue()
                content_type = "image/png"

            cache_key = 'portrait_jpeg' if prefer_jpeg else 'portrait'
            _last_frame_cache[cache_key] = encoded_bytes
            _last_frame_cache['time'] = time.time()
            return encoded_bytes, content_type
    except Exception as e:
        cache_key = 'portrait_jpeg' if prefer_jpeg else 'portrait'
        if _last_frame_cache.get(cache_key):
            return _last_frame_cache[cache_key], ("image/jpeg" if prefer_jpeg else "image/png")
        return None, f"Capture fallback error: {e}"
    finally:
        _capture_lock.release()




def notify_qemu_relaunched():
    """Release reader sockets bound to the previous QEMU instance.

    QEMU's `-serial tcp:...,server,nowait` serves exactly one client, so the
    old client must be dropped for the new instance to be claimed. The serial
    worker reconnects within ~250 ms; the VNC worker re-handshakes and
    republishes metrics on its next session.
    """
    global serial_epoch, serial_socket_conn
    with serial_socket_lock:
        serial_epoch += 1
        old = serial_socket_conn
        serial_socket_conn = None
    if old is not None:
        try:
            old.close()  # forces recv() EOF in the worker
        except OSError:
            pass
    vnc_client.force_reconnect()


def stop_qemu_backend():
    """Stops any running QEMU and websockify processes managed by this instance."""
    global qemu_subproc, ws_subproc, launcher_has_booted, serial_log_is_session, ws_bridge_port
    launcher_has_booted = False
    # The UART log of a session that has ended is not the live guest's output
    # any more, so it stops being served as such.
    serial_log_is_session = False
    if ws_subproc:
        try:
            ws_subproc.terminate()
            ws_subproc.wait(timeout=1)
        except Exception:
            pass
        ws_subproc = None
        ws_bridge_port = None

    if qemu_subproc:
        try:
            qemu_subproc.terminate()
            qemu_subproc.wait(timeout=1)
        except Exception:
            pass
        qemu_subproc = None
        notify_qemu_relaunched()

    # The tracked process is gone: forget it, so no later check can mistake a
    # recycled PID for the guest this Studio launched.
    clear_qemu_process()
    reset_monitor_connection()

    # Never discover and kill arbitrary QEMU processes by executable name,
    # shared disk path, or monitor port. Only qemu_subproc, created and owned by
    # this Device Studio server process, may be terminated here.

    latest_telemetry["qemu_alive"] = False
    latest_telemetry["device_state"] = "not_running"
    latest_telemetry["device_reason"] = "No QEMU process is tracked by this Studio."
    latest_telemetry["qemu_process"] = {"tracked": False, "pid": None, "alive": False}
    latest_telemetry["status"] = "standby"
    latest_telemetry["mode"] = "standby"
    latest_telemetry["cpu"]["halted"] = True
    latest_telemetry["cpu"]["rip"] = "Offline"
    latest_telemetry["cpu"]["thread_id"] = "Offline"
    latest_telemetry["cpu"]["registers"] = {}


def qemu_x86_accel_candidates(exe):
    """Ordered accelerator chain for the x86_64 profile (fastest first).

    `qemu-system-x86_64 -accel help` lists what this host build accepts; WHPX
    (Windows Hypervisor Platform) is only requested when it is listed, and
    plain TCG is always the last candidate, so a WHPX that cannot start the
    machine falls back instead of failing the launch. Set
    INTEROS_QEMU_FORCE_TCG=1 to pin TCG (used to compare boot timings).
    """
    chain = []
    if os.environ.get("INTEROS_QEMU_FORCE_TCG") != "1":
        try:
            probe = subprocess.run(
                [exe, "-accel", "help"], capture_output=True, text=True, timeout=8)
            listed = (probe.stdout or "") + (probe.stderr or "")
            if re.search(r"(?im)^\s*whpx\s*$", listed):
                # kernel-irqchip=off is the conservative WHPX mode: the
                # hypervisor does not own IRQ chip routing, which is the
                # configuration QEMU documents as working without an
                # enlightened guest.
                chain.append("whpx,kernel-irqchip=off")
        except Exception:
            pass  # no probe result -> straight to TCG
    chain.append("tcg")
    return chain


def qemu_process_survives_startup(proc, grace=0.5):
    """True while the process is still alive after `grace` seconds.

    Used for two things: to validate a hardware accelerator (a rejected
    `-accel whpx` aborts during machine init, well inside this window, while a
    guest that got this far keeps running) and to prove that a launch did not
    die on a port conflict. A launch that exits during this window never
    becomes a device, and the Studio must say so instead of reporting a
    running guest it does not have.
    """
    deadline = time.time() + grace
    while time.time() < deadline:
        if proc.poll() is not None:
            return False
        time.sleep(0.05)
    return True


def launch_qemu_profile(profile_key="x86_64", ram_mb=None, smp=1):
    """Launches a real QEMU instance for the specified target device profile.

    Returns (ok, message, info). `info` carries the launch facts the caller
    reports: the PID, the VNC display and port actually used, and the device
    state the Studio can prove once the guest is up.
    """
    global qemu_subproc, active_profile_key, current_serial_log_path, launcher_has_booted, last_reboot_cursor
    global serial_log_is_session, active_vnc_display, active_vnc_port
    launcher_has_booted = False
    with telemetry_lock:
        last_reboot_cursor = len(serial_lines)
    # The guest this Studio launched (or adopted from an earlier Studio
    # instance) is the only QEMU that may be stopped here.
    stop_tracked_qemu()

    launch_info = {
        "pid": None,
        "vnc_display": active_vnc_display,
        "vnc_port": active_vnc_port,
        "device_state": "not_running",
    }

    if profile_key not in SUPPORTED_PROFILES:
        return False, f"Unknown profile: {profile_key}", launch_info

    prof = SUPPORTED_PROFILES[profile_key]
    arch = prof["arch"]
    exe = find_qemu_executable(arch)
    if not exe:
        # Name every place it looked so a host operator can fix it in one step
        # (add it to PATH or pin INTEROS_QEMU / QEMU_PATH / QEMU_DIR). Honest
        # failure: no fallback binary, no guess about which QEMU to run.
        searched = _qemu_search_log or [f"qemu-system-{arch} in PATH"]
        return False, (f"QEMU binary 'qemu-system-{arch}' not found on host. "
                       f"Searched: {'; '.join(searched)}."), launch_info

    img_path = prof["image"]
    if not os.path.exists(img_path):
        return False, f"Boot image not found at '{img_path}'. Build target first with 'interos build {arch}'.", launch_info
    default_data_disk = "inter-os-data-master.img" if arch == "x86_64" else f"interos-data-{arch}.img"
    data_disk_path = os.environ.get("DATA_DISK_PATH", os.path.join(WORKSPACE_ROOT, "target", default_data_disk))
    if not os.path.isabs(data_disk_path) and ".." in data_disk_path:
        return False, "Security violation: Invalid data disk path traversal detected.", launch_info

    ram = ram_mb or prof["ram"]
    serial_log = os.path.join(WORKSPACE_ROOT, "target", f"{arch}_boot.log")
    os.makedirs(os.path.dirname(serial_log), exist_ok=True)
    current_serial_log_path = serial_log
    try:
        with open(current_serial_log_path, "w", encoding="utf-8") as f:
            f.write(f"=== QEMU {arch} Boot Session ===\n")
        # The log above was just rewritten for this boot, so it may feed
        # /api/serial and the launcher-readiness gate until the next launch.
        serial_log_is_session = True
    except Exception:
        pass

    # The VNC display is chosen here, not assumed. A display whose port is
    # already served would make QEMU exit at launch, so a taken display is
    # replaced by a free one and the substitution is reported.
    vnc_display, vnc_port, vnc_substituted = select_vnc_display()
    if vnc_display is None:
        message = (f"No free VNC display available near :{active_vnc_display} "
                   f"(ports {5900 + active_vnc_display}-{5999} are all in use).")
        serial_lines.append(f"[STUDIO] {message}")
        return False, message, launch_info
    active_vnc_display = vnc_display
    active_vnc_port = vnc_port
    launch_info["vnc_display"] = vnc_display
    launch_info["vnc_port"] = vnc_port
    if vnc_substituted:
        note = (f"VNC display :{vnc_display} (port {vnc_port}) was selected because the "
                f"configured display :{VNC_DISPLAY} (port {5900 + VNC_DISPLAY}) is already in use.")
        print(f"[*] {note}")
        display_note_lines = [f"[STUDIO] {note}"]
    else:
        display_note_lines = []

    # The monitor and serial ports are QEMU's own listeners: if another
    # process holds them, QEMU exits at startup. Say so before spawning
    # rather than reporting a launch that cannot have succeeded.
    for port, what in ((MONITOR_PORT, "QEMU monitor"), (SERIAL_TCP_PORT, "guest serial (COM1)")):
        if _port_occupied(port):
            message = (f"The {what} port {port} is already in use by another process, so QEMU cannot "
                       f"start. Stop that process (a QEMU this Studio did not launch, for instance) "
                       f"and try again.")
            serial_lines.append(f"[STUDIO] {message}")
            return False, message, launch_info

    # WHY headless by default: the Studio is the device's window. Every User
    # still got a QEMU SDL window per launch -- one per boot cycle -- to look
    # at a canvas this daemon already serves better over VNC. The display is
    # still fully alive (the VGA device scans out to the VNC endpoint above),
    # touch travels on COM1 and the monitor on TCP, so nothing needs the
    # desktop window. INTEROS_DISPLAY=sdl|gtk brings a window back when it
    # really is wanted.
    display_backend = os.environ.get("INTEROS_DISPLAY", "none")
    cmd = [
        exe,
        "-m", f"{ram}M",
        "-smp", str(smp),
        "-name", "InterOS Core — Native Full HD (1080×2160)",
        "-monitor", f"tcp:127.0.0.1:{MONITOR_PORT},server,nowait",
        "-serial", f"tcp:127.0.0.1:{SERIAL_TCP_PORT},server,nowait",
        "-display", display_backend,
        "-vnc", f"127.0.0.1:{vnc_display}"
    ]
    if arch == "x86_64":
        cmd.extend(["-machine", "q35", "-cpu", "qemu64"])
        # Real virtual display and pointer hardware for the canonical phone VM.
        # The x86_64 profile launches with the canonical scanout declared in
        # SUPPORTED_PROFILES: "guest_width": 720, "guest_height": 1440, wired
        # into the EDID below so the guest KMS modesets exactly 720x1440.
        # WHY no -device pointer flag: q35 already exposes the i8042 PS/2
        # controller that the guest's ps2_mouse driver talks to, and this
        # QEMU build rejects a PS/2 mouse device flag ("expects a pluggable
        # device type"), which killed the VM instantly. An absolute USB
        # pointer was tried, but the guest has no USB HID stack, so it
        # booted yet moved nothing. Studio pointer input travels as
        # absolute COM1 touch injection.
        # WHY not `-device ramfb`: tried as the QEMU-level native-portrait
        # console; with any position/order it aborts the prebuilt bootloader's
        # VBE enumeration after mode 0x144 (falls back to mode 0x143) and
        # stage-4 then #GPs at its protected-mode entry -> double fault ->
        # triple fault (5/5 runs, `-d int,cpu_reset` forensics: #GP err=8 at
        # pc=0x96ff with IDT still the real-mode IVT). Same image with only
        # virtio-vga boots to PID1. The bootloader stages ship as binary
        # blobs in crates.io `bootloader` 0.11.17, so this is not patchable
        # here; the standard VGA device exposes the same Bochs DISPI registers
        # and keeps the guest KMS at the requested scanout.
        #
        # WHY the standard VGA device and not virtio-vga: virtio-vga's
        # framebuffer is capped at 8MB of VRAM and cannot present the FHD-class
        # scanout this device profile asks for -- the guest logs the DISPI mode
        # as unavailable, falls back, and the result is horizontal banding
        # because the display mode and the compositor's scanout disagree. The
        # standard VGA device is the same Bochs DISPI path with tunable VRAM
        # (`vgamem_mb`), so raising the guest's scanout is a matter of asking
        # for it here rather than hitting a device limit.
        #
        # 32MB covers a 1080x2160 ARGB8888 front and back buffer (19.8MB) with
        # headroom, which is the size the profile's geometry implies.
        edid_w, edid_h = device_guest_geometry()
        cmd.extend([
            "-vga", "none",
            "-device",
            f"VGA,edid=on,xres={edid_w},yres={edid_h},vgamem_mb=32",
        ])
        # A dedicated data disk is mandatory. The kernel uses the Primary Master
        # as the read-only boot image and the Primary Slave as persistent storage;
        # with only one disk attached it falls back to formatting the boot image
        # itself, which overwrites the bootloader stages the kernel is embedded in
        # and leaves the next boot running a corrupted kernel.
        cmd.extend(["-drive", f"format=raw,file={img_path},index=0"])
        ensure_data_disk(data_disk_path)
        cmd.extend(["-drive", f"format=raw,file={data_disk_path},index=1"])
        # A real E1000 NIC on user-mode networking. Without one the guest has no
        # network hardware at all and every packet stays on the kernel loopback
        # path, which is not the same thing as having a network.
        cmd.extend([
            "-netdev", "user,id=net0",
            "-device", "e1000,netdev=net0,mac=52:54:00:12:34:56",
        ])
    elif arch == "aarch64":
        cmd.extend(["-machine", "virt", "-cpu", "cortex-a53", "-kernel", img_path])
    elif arch == "riscv64":
        cmd.extend(["-machine", "virt", "-cpu", "rv64", "-bios", "default", "-kernel", img_path])

    # Give every architecture a real display device. The `virt` machine has no
    # framebuffer by default, so Device Studio's phone viewport had nothing to
    # show and no VNC pixels to stream.
    if arch in ("aarch64", "riscv64"):
        cmd.extend(["-device", "virtio-gpu-pci", "-device", "usb-ehci,id=usb0"])
        # This guest does not implement an EHCI/USB HID driver yet; browser-to-guest input remains explicit serial injection on these non-canonical profiles.

    # The x86_64 profile tries the host's hardware accelerator first and always
    # keeps plain TCG as the last candidate, so a WHPX that cannot start this
    # machine falls back instead of failing the launch. Other profiles keep
    # QEMU's default accelerator exactly as before.
    accel_candidates = qemu_x86_accel_candidates(exe) if arch == "x86_64" else [None]
    try:
        serial_lines.clear()
        # The display substitution is part of this launch's story, so it is
        # written after the log buffer is reset for the new session.
        serial_lines.extend(display_note_lines)
        last_error = None
        for accel in accel_candidates:
            attempt = list(cmd)
            if accel:
                attempt.extend(["-accel", accel])
            try:
                creation_flags = subprocess.CREATE_NEW_CONSOLE if sys.platform == "win32" else 0
                proc = subprocess.Popen(attempt, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=creation_flags)
            except Exception as e:
                last_error = e
                continue
            if not qemu_process_survives_startup(proc, grace=0.6):
                # The process died during startup. QEMU exits immediately when a
                # port it must bind is taken, so this is the port-conflict case
                # the Studio used to report as a running device.
                exit_code = proc.poll()
                try:
                    proc.terminate()
                    proc.wait(timeout=2)
                except Exception:
                    pass
                if accel:
                    serial_lines.append(
                        f"[STUDIO] Accelerator '{accel}' did not start this machine; retrying with TCG.")
                    continue
                message = (f"QEMU exited during startup (exit code {exit_code}). It could not bind one of "
                           f"its own ports (monitor {MONITOR_PORT}, serial {SERIAL_TCP_PORT}, "
                           f"VNC {vnc_port}); the device did not start.")
                serial_lines.append(f"[STUDIO] {message}")
                print(f"[*] {message}")
                return False, message, launch_info
            # The monitor connection belongs to the previous instance; drop it
            # so the next command reaches the guest that was just launched.
            reset_monitor_connection()
            # A QEMU that cannot bind one of its ports can die a moment after
            # the process exists, so the monitor is given time to answer and
            # the process is checked again. A guest that is not there must
            # never be reported as a device that is.
            wait_for_monitor(timeout=3.0)
            if proc.poll() is not None:
                exit_code = proc.poll()
                try:
                    proc.terminate()
                    proc.wait(timeout=2)
                except Exception:
                    pass
                if accel:
                    serial_lines.append(
                        f"[STUDIO] Accelerator '{accel}' did not start this machine; retrying with TCG.")
                    continue
                message = (f"QEMU exited during startup (exit code {exit_code}). It could not bind one of "
                           f"its own ports (monitor {MONITOR_PORT}, serial {SERIAL_TCP_PORT}, "
                           f"VNC {vnc_port}); the device did not start.")
                serial_lines.append(f"[STUDIO] {message}")
                print(f"[*] {message}")
                return False, message, launch_info
            qemu_subproc = proc
            # Track the process this Studio launched: its PID plus the start
            # time that makes the pair a fingerprint no recycled PID can
            # impersonate.
            track_qemu_process(proc, vnc_display=vnc_display, vnc_port=vnc_port)
            launch_info["pid"] = proc.pid
            active_profile_key = profile_key
            # The guest may be on a different display than the configured one;
            # the Studio's own RFB client follows it.
            vnc_client.set_port(active_vnc_port)
            # QEMU owns the VNC listener; bridge startup must happen after the VM exists.
            start_websockify_if_needed()
            accel_note = f" accel={accel}" if accel else ""
            display_note = f"VNC display :{vnc_display} (port {vnc_port})"
            serial_lines.append(
                f"[STUDIO] Launched QEMU {arch} microkernel instance (PID {proc.pid}, {display_note}){accel_note}.")
            print(f"[*] QEMU {arch} started (PID {proc.pid}, {display_note})")
            state = qemu_device_state()
            launch_info["device_state"] = state["state"]
            message = (f"QEMU {arch} started (PID {proc.pid}, accelerator {accel or 'default'}, "
                       f"{display_note})")
            if not state["running"]:
                message += f"; the device is not yet provably up ({state['reason']})"
            return True, message, launch_info
        message = f"Failed to spawn QEMU: {last_error}"
        serial_lines.append(f"[STUDIO] {message}")
        return False, message, launch_info
    except Exception as e:
        message = f"Failed to spawn QEMU: {e}"
        serial_lines.append(f"[STUDIO] {message}")
        return False, message, launch_info


def start_qemu_backend():
    """Ensures QEMU backend is active and reachable; launches primary instance if needed."""
    global qemu_subproc, ws_subproc

    # A device is running only when the process this Studio launched is alive
    # AND the monitor answers. A monitor that answers without a tracked process
    # belongs to a QEMU this Studio did not launch, so it is not the device.
    state = qemu_device_state()
    if state["running"]:
        return True, "Connected to the microkernel instance this Studio launched"

    ok, message, _info = launch_qemu_profile("x86_64")
    return ok, message


last_telemetry_query = 0.0
telemetry_lock = threading.Lock()

def query_qemu_monitor(host='127.0.0.1', port=MONITOR_PORT, timeout=0.35, force=False):
    """
    Queries real-time microkernel registers, CPU execution status, and block
    device telemetry from the QEMU HMP monitor at 127.0.0.1:44445.

    Every read goes through send_qemu_monitor_cmd(): QEMU's HMP accepts a
    single client, and that helper owns the server's one persistent monitor
    connection with read-until-'(qemu)' framing. This function used to open
    its own private socket; with the persistent connection already holding
    the port, the private connect() sat unaccepted in QEMU's listen backlog,
    recv() timed out, and the VNC fallback silently returned the last cached
    snapshot as live state - an early-boot real-mode RIP was displayed while
    the kernel was already running in long mode.

    WHY the answer is not taken from the VNC port any more: a port that
    answers proves that SOMETHING holds it, not that the guest this Studio
    launched is behind it. The device is reported as running only when the
    tracked QEMU process is alive AND the monitor answers; the port probe is
    kept as a secondary signal that can never override the process check.
    """
    global last_telemetry_query, latest_telemetry
    now = time.time()
    if not force and now - last_telemetry_query < 0.8:
        return latest_telemetry

    with telemetry_lock:
        if not force and now - last_telemetry_query < 0.8:
            return latest_telemetry

        last_telemetry_query = now
        monitor_answers = False
        vnc_port_busy = False
        vm_status = "unknown"
        try:
            def monitor(cmd):
                ok, resp = send_qemu_monitor_cmd(
                    cmd, host=host, port=port, timeout=max(float(timeout), 0.8))
                if not ok:
                    raise OSError(f"monitor command failed: {resp}")
                return resp

            # 1. info status
            status_lower = monitor('info status').lower()
            if "shutdown" in status_lower:
                vm_status = "shutdown"
            elif "internal error" in status_lower:
                vm_status = "error"
            elif "paused" in status_lower:
                vm_status = "paused"
            elif "running" in status_lower:
                vm_status = "running"
            else:
                vm_status = "unknown"

            # 2. info cpus
            cpus_raw = monitor('info cpus')
            thread_match = re.search(r'thread_id=(\d+)', cpus_raw)
            thread_id = f"PID-{thread_match.group(1)}" if thread_match else "Core0"
            is_halted = "halted" in cpus_raw

            # 3. info registers
            regs_raw = monitor('info registers')

            # 4. info block
            block_raw = monitor('info block')

            # Parse registers into a map. The dict is REPLACED, never merged:
            # update() used to leave keys from older parses (previous boots,
            # earlier CPU modes) visible as if they were current state.
            reg_dict = {}
            for match in re.finditer(r'\b([A-Z0-9]+)\s*=\s*([0-9a-fA-F]{4,16})\b', regs_raw):
                reg_name = match.group(1).upper()
                reg_val = "0x" + match.group(2).upper()
                reg_dict[reg_name] = reg_val

            rip_val = reg_dict.get("RIP") or reg_dict.get("EIP") or "Unavailable"
            if "HLT=1" in regs_raw:
                is_halted = True
            elif "HLT=0" in regs_raw:
                is_halted = False

            block_device = "ide0-hd0"
            block_file = "inter-os-bios.img"
            block_match = re.search(r'([\w\-]+)\s*\([^)]*\):\s*(.+?)(?:\s*\(|\r|\n)', block_raw)
            if block_match:
                block_device = block_match.group(1)
                full_path = block_match.group(2).strip()
                block_file = full_path.split('\\')[-1].split('/')[-1]

            monitor_answers = True
            latest_telemetry["status"] = vm_status
            latest_telemetry["mode"] = "qemu_live"
            latest_telemetry["cpu"]["thread_id"] = thread_id
            latest_telemetry["cpu"]["halted"] = is_halted
            latest_telemetry["cpu"]["rip"] = rip_val
            latest_telemetry["cpu"]["registers"] = reg_dict
            latest_telemetry["block"]["device"] = block_device
            latest_telemetry["block"]["file"] = block_file

            try:
                with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s_ws:
                    s_ws.settimeout(0.08)
                    latest_telemetry["websockify_alive"] = (s_ws.connect_ex(('127.0.0.1', WS_PORT)) == 0)
            except Exception:
                latest_telemetry["websockify_alive"] = False
        except Exception:
            # The monitor did not answer. The VNC port is probed as a
            # SECONDARY signal only: it says who holds the port, never that
            # the device is up.
            vnc_port_busy = _port_occupied(active_vnc_port)
            # The monitor could not answer: report the CPU state as
            # unavailable instead of replaying the last cached parse
            # as live telemetry.
            latest_telemetry["cpu"]["rip"] = "Unavailable"
            latest_telemetry["cpu"]["thread_id"] = "Unavailable"
            latest_telemetry["cpu"]["halted"] = None
            latest_telemetry["cpu"]["registers"] = {}

        # The device state is resolved from the process and the monitor
        # together; the port probe is reported but never decides it.
        process_alive = tracked_qemu_process_alive()
        record = tracked_qemu_process()
        running = bool(process_alive and monitor_answers)
        if running:
            device_state = "running"
            device_reason = "The QEMU process this Studio launched is alive and its monitor answers."
        elif process_alive:
            device_state = "unknown"
            device_reason = "The QEMU process this Studio launched is alive, but its monitor did not answer."
        elif monitor_answers:
            device_state = "unknown"
            device_reason = (f"A QEMU monitor answers on port {MONITOR_PORT}, but this Studio did not "
                             "launch it and cannot identify it as the device.")
        elif vnc_port_busy:
            device_state = "not_running"
            device_reason = (f"No QEMU process is tracked by this Studio; port {active_vnc_port} is held "
                             "by another process, which is not the device.")
        else:
            device_state = "not_running"
            device_reason = "No QEMU process is tracked by this Studio."

        latest_telemetry["qemu_alive"] = running
        latest_telemetry["device_state"] = device_state
        latest_telemetry["device_reason"] = device_reason
        latest_telemetry["monitor_answers"] = monitor_answers
        latest_telemetry["vnc_port_busy"] = vnc_port_busy
        latest_telemetry["qemu_process"] = {
            "tracked": bool(record.get("pid")),
            "pid": record.get("pid"),
            "alive": process_alive,
        }
        if not running:
            latest_telemetry["status"] = "standby"
            latest_telemetry["mode"] = "standby"
            latest_telemetry["cpu"]["halted"] = True
            latest_telemetry["cpu"]["rip"] = "Offline"
            latest_telemetry["cpu"]["thread_id"] = "Offline"
            latest_telemetry["cpu"]["registers"] = {}
        return latest_telemetry


def is_safe_proxy_url(url_str):
    """
    SSRF Protection: Validates that an outbound HTTP/HTTPS request does not
    target internal services, localhost, cloud metadata, or RFC1918 private subnets.
    """
    try:
        parsed = urllib.parse.urlparse(url_str)
        if parsed.scheme not in ('http', 'https'):
            return False, f"Scheme '{parsed.scheme}' not allowed. Only HTTP and HTTPS protocols are permitted."

        hostname = parsed.hostname
        if not hostname:
            return False, "Missing hostname in URL."

        blocked_hostnames = {
            'localhost', 'localhost.localdomain', 'metadata.google.internal',
            'instance-data', '169.254.169.254'
        }
        if hostname.lower() in blocked_hostnames or hostname.lower().endswith('.local'):
            return False, f"SSRF Boundary Violation: Target '{hostname}' is an internal reserved hostname."

        try:
            addr_info = socket.getaddrinfo(hostname, None)
        except Exception as e:
            return False, f"Could not resolve host '{hostname}': {e}"

        for item in addr_info:
            ip_str = item[4][0]
            ip = ipaddress.ip_address(ip_str)
            if ip.is_loopback or ip.is_private or ip.is_link_local or ip.is_reserved or ip.is_multicast:
                return False, f"SSRF Boundary Violation: Access to private/internal IP '{ip_str}' is forbidden."

        return True, "OK"
    except Exception as e:
        return False, f"Invalid URL: {e}"


class _SsrfGuardedRedirectHandler(urllib.request.HTTPRedirectHandler):
    """Re-validates every redirect hop with the same SSRF boundary.

    urllib follows redirects implicitly *after* the original URL was checked,
    so a public host could answer 302 "Location: http://127.0.0.1:44445/" and
    bounce the proxy onto the QEMU monitor or link-local metadata. Each hop
    must pass is_safe_proxy_url before it is followed.
    """

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        safe, reason = is_safe_proxy_url(newurl)
        if not safe:
            raise urllib.error.HTTPError(
                req.full_url, 502, f"SSRF Blocked on redirect: {reason}", headers, fp
            )
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def verify_and_install_rapp(body, pkg_name):
    """Validate and install the canonical binary RAPP v2 format.

    Device Studio performs structural, digest, and Ed25519 signature checks.
    Publisher trust/revocation remains a guest/kernel decision; Studio never
    upgrades a package to trusted merely because its embedded signature verifies.
    The original binary package is stored unchanged.
    """
    import binascii
    from cryptography.hazmat.primitives.asymmetric import ed25519

    HEADER_SIZE = 0xB4
    SIGNATURE_OFFSET = 0x70
    SIGNATURE_SIZE = 64
    MAX_CAPABILITIES = 1024

    if len(body) < HEADER_SIZE:
        return False, "RAPP v2 package is shorter than the canonical header.", None
    arch_id = int.from_bytes(body[6:8], "little")
    if arch_id not in (0, 3):
        return False, "The current guest transport accepts x86_64 RAPP packages only.", None
    if body[:4] != b"RAPP":
        return False, "Invalid RAPP magic.", None
    if int.from_bytes(body[4:6], "little") != 2:
        return False, "Unsupported RAPP format version; expected v2.", None

    arch_names = {0: "x86_64", 1: "aarch64", 2: "riscv64", 3: "web"}
    if arch_id not in arch_names:
        return False, "Unsupported RAPP architecture.", None

    raw_id = body[8:40].split(b"\0", 1)[0]
    try:
        app_id = raw_id.decode("utf-8")
    except UnicodeDecodeError:
        return False, "RAPP package ID is not valid UTF-8.", None
    if not app_id or len(app_id) > 32:
        return False, "Invalid RAPP package ID.", None

    publisher_key = body[40:72]
    if publisher_key == b"\0" * 32:
        return False, "RAPP publisher key is empty.", None

    version = tuple(int.from_bytes(body[o:o + 2], "little") for o in (72, 74, 76))
    cap_count = int.from_bytes(body[78:80], "little")
    if cap_count > MAX_CAPABILITIES:
        return False, "RAPP capability count exceeds the canonical limit.", None

    payload_digest = body[0x50:0x70]
    signature = body[SIGNATURE_OFFSET:SIGNATURE_OFFSET + SIGNATURE_SIZE]
    payload_len = int.from_bytes(body[0xB0:0xB4], "little")
    caps_end = HEADER_SIZE + cap_count * 2
    payload_end = caps_end + payload_len
    if caps_end > len(body) or payload_end != len(body):
        return False, "RAPP v2 payload bounds are invalid.", None

    capabilities = []
    for i in range(cap_count):
        cap = int.from_bytes(body[HEADER_SIZE + i * 2:HEADER_SIZE + i * 2 + 2], "little")
        if cap < 1 or cap > 18:
            return False, f"Unknown RAPP capability ID {cap}.", None
        capabilities.append(cap)

    payload = body[caps_end:payload_end]
    if hashlib.sha256(payload).digest() != payload_digest:
        return False, "RAPP payload SHA-256 digest mismatch.", None

    signed_message = (
        body[:SIGNATURE_OFFSET]
        + b"\0" * SIGNATURE_SIZE
        + body[SIGNATURE_OFFSET + SIGNATURE_SIZE:caps_end]
        + payload
    )
    try:
        ed25519.Ed25519PublicKey.from_public_bytes(publisher_key).verify(signature, signed_message)
    except Exception:
        return False, "RAPP Ed25519 signature verification failed.", None

    safe_name = os.path.basename(pkg_name or f"{app_id}.rapp")
    if not safe_name.endswith(".rapp"):
        safe_name = f"{safe_name}.rapp"
    filename = safe_name

    sha256 = hashlib.sha256(body).hexdigest()
    version_text = ".".join(str(v) for v in version)
    app_entry = {
        "id": app_id,
        "name": app_id,
        "filename": filename,
        "format": "Rust App Package (RAPP v2 web/wasm)" if arch_id == 3 else "Rust App Package (RAPP v2 binary)",
        "type": "Web / WASM Sandbox (Ring 3)" if arch_id == 3 else "Sovereign Ring 3",
        "size": len(body),
        "sha256": sha256[:16] + "...",
        "status": "Signature verified; guest trust required",
        "capabilities": capabilities,
        "permissions": capabilities,
        "version": version_text,
        "signature_verified": True,
        "publisher_trusted": False,
        "public_key": binascii.hexlify(publisher_key).decode("ascii")[:16] + "...",
        "arch": arch_names[arch_id],
        "payload_size": payload_len,
    }
    # Record what this host actually wrote, so a later Packages-table read can
    # report the real file and the real digest instead of "SHA-256 unavailable"
    # for a package the Studio itself just ingested.
    SIDELOAD_LEDGER[app_id] = {
        "filename": filename,
        "sha256": sha256[:16] + "...",
        "format": "Rust App Package (RAPP v2 web/wasm)" if arch_id == 3 else "Rust App Package (RAPP v2 binary)",
    }
    return True, "RAPP v2 signature and integrity verified; guest trust remains authoritative", app_entry


def guest_command_lines(command, end_marker, timeout=8):
    """Run a development command in the guest and return only its response lines."""
    with telemetry_lock:
        start = len(serial_lines)
    ok, msg = send_guest_serial_command(command, None, timeout=timeout)
    if not ok:
        return False, [msg]
    deadline = time.time() + timeout
    while time.time() < deadline:
        with telemetry_lock:
            lines = list(serial_lines[start:])
        if any(end_marker in line for line in lines):
            return True, lines
        time.sleep(0.05)
    return False, lines

# The last frame-timing answer and when it was taken.
#
# WHY it is cached: every question about the frame timing makes the guest
# print a diagnostics dump, and the studio asks once a second. The console
# carries the guest's commands as well as the guest's log, so the dumps were
# filling it while the touch and navigation commands were written into it --
# and a command written into a full console is lost, with neither side
# noticing. The answer changes once per frame, so a second and a half of it
# is a second and a half of a frame.
_FRAME_TIMING_CACHE = {"at": 0.0, "value": None}


def extract_frame_timing():
    """Ask the guest for its most recent flip and report what it actually cost.

    WHY this exists rather than a displayed constant: the DRM/KMS tab used to
    read "60 Hz VSYNC" from a literal in index.html. Nothing in this Studio ever
    measured a refresh rate, and nothing in it can -- the guest publishes a flip
    count and the cycle cost of each flip through `[DIAG_STATE]`, and a refresh
    rate is a property of hardware the emulated scanout does not have. Printing
    60 next to a panel whose own table says "no live compositor-plane telemetry
    is currently exposed by the guest" put a specific number on the one claim
    the panel had already admitted it could not support.

    The two cycle counts are real. They are the cost of compositing one flip and
    of presenting it, in TSC cycles, as measured by the display path. Converting
    them to a refresh rate needs a wall-clock interval the guest does not
    currently export, so this reports cycles and says so, rather than dividing
    by a made-up frame budget to manufacture a frequency.

    The flip number is returned alongside the cost for the reason
    `performance::last_frame` gives: two reads showing the same flip number mean
    the second sampled the same frame again, not a cheaper one.

    WHY the answer is cached: the console carries the guest's commands as well
    as the guest's log, and every one of these questions makes the guest print
    a diagnostics dump. The studio asks for the status once a second, and a
    dump is hundreds of bytes, so the console was busy with the dump while the
    touch and navigation commands were written -- and a command written into a
    full console is lost, with neither side noticing. The answer changes once
    per frame, so a second and a half of it is a second and a half of a
    frame, and the console is left for the input.
    """
    now = time.time()
    cached = _FRAME_TIMING_CACHE.get("value")
    if cached is not None and now - _FRAME_TIMING_CACHE.get("at", 0.0) < 1.5:
        return cached
    ok, lines = guest_command_lines("DIAG_STATE", "[DIAG_STATE]", timeout=8)
    latest = None
    for line in lines:
        if "[DIAG_STATE]" not in line:
            continue
        values = parse_key_values(line.split("]", 1)[1].strip())
        if "last_flip" in values:
            latest = values
    if latest is None:
        return {
            "status": "UNAVAILABLE",
            "source": "GUEST_TELEMETRY",
            "detail": "The guest reported no flip telemetry. Older guests do not "
                      "publish it; this is 'unknown', not a zero-cost frame.",
        }
    try:
        flip = int(latest.get("last_flip", 0))
        compose = int(latest.get("last_flip_compose_cycles", 0))
        present = int(latest.get("last_flip_present_cycles", 0))
    except (TypeError, ValueError):
        _FRAME_TIMING_CACHE["at"] = now
        _FRAME_TIMING_CACHE["value"] = {
            "status": "UNAVAILABLE",
            "source": "GUEST_TELEMETRY",
            "detail": "The guest published a flip record this build could not parse.",
        }
        return _FRAME_TIMING_CACHE["value"]
    _FRAME_TIMING_CACHE["at"] = now
    _FRAME_TIMING_CACHE["value"] = {
        "status": "AVAILABLE",
        "source": "GUEST_TELEMETRY",
        "last_flip": flip,
        "compose_cycles": compose,
        "present_cycles": present,
        "total_cycles": compose + present,
        "refresh_rate_hz": None,
        "detail": "Cycle counts for the most recent scanout flip. The guest "
                  "exports no wall-clock interval per frame, so these cannot be "
                  "converted to a refresh rate without inventing one.",
    }
    return _FRAME_TIMING_CACHE["value"]


def is_safe_guest_path(path, writable=False):
    if not isinstance(path, str) or len(path) == 0 or len(path) > 192:
        return False
    if not path.startswith("/") or "\\" in path or ".." in path or "//" in path:
        return False
    if any(ord(ch) < 0x20 or ch in "|;&$'\\\"" for ch in path):
        return False
    if writable and not path.startswith("/data/"):
        return False
    return all(part and part not in (".", "..") for part in path.split("/") if part)

def is_safe_package_id(value):
    return isinstance(value, str) and 1 <= len(value) <= 128 and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", value) is not None

def is_safe_publisher(value):
    return isinstance(value, str) and 1 <= len(value) <= 32 and re.fullmatch(r"[A-Za-z0-9._-]+", value) is not None

# -----------------------------------------------------------------------------
# WebApp Porting Engine & Live HMR State
# -----------------------------------------------------------------------------
web_hmr_events = []
web_hmr_cond = threading.Condition()
runtime_permission_grants = {}  # key: (app_id, permission) -> bool

def guest_fs_list(path):
    if not is_safe_guest_path(path):
        return False, "invalid guest path", []
    ok, lines = guest_command_lines(f"STUDIO_FS_LIST {path}", f"LIST_OK path={path}")
    entries = []
    for line in lines:
        if line.startswith("[STUDIO_FS_ENTRY] path="):
            payload = line.split(" ", 2)[-1]
            parts = payload.split("|")
            if len(parts) == 3:
                entries.append({
                    "name": parts[0],
                    "isDir": parts[1] == "dir",
                    "size": int(parts[2]) if parts[2].isdigit() else 0,
                })
    return ok, None if ok else (lines[-1] if lines else "guest filesystem unavailable"), entries

def guest_fs_read(path):
    if not is_safe_guest_path(path):
        return False, 'invalid guest path', ''
    chunks = []
    offset = 0
    while offset < 8192:
        ok, lines = guest_command_lines(f"STUDIO_FS_READ {path} {offset} 192", "[STUDIO_FS_READ]", timeout=5)
        if not ok:
            return False, lines[-1] if lines else "guest filesystem unavailable", ''
        match = next((line for line in lines if line.startswith("[STUDIO_FS_READ]")), None)
        if not match:
            return False, "guest read returned no data", ''
        fields = dict(part.split("=", 1) for part in match.split() if "=" in part)
        raw = bytes.fromhex(fields.get("hex", ""))
        chunks.append(raw)
        if len(raw) < 192:
            break
        offset += len(raw)
    data = b"".join(chunks)
    return True, None, data.decode("utf-8", errors="replace")

def guest_fs_write(path, content):
    if not is_safe_guest_path(path, writable=True):
        return False, 'guest writes are restricted to safe /data paths'
    raw = content.encode("utf-8")
    if len(raw) > 8192:
        return False, 'guest file exceeds 8192-byte limit'
    ok, lines = guest_command_lines(f"STUDIO_FS_BEGIN {path} {len(raw)}", f"WRITE_READY path={path}", timeout=5)
    if not ok:
        return False, lines[-1] if lines else "guest filesystem unavailable"
    for offset in range(0, len(raw), 192):
        chunk = raw[offset:offset + 192].hex()
        ok, lines = guest_command_lines(f"STUDIO_FS_CHUNK {offset} {chunk}", f"CHUNK_OK offset={offset}", timeout=5)
        if not ok:
            return False, lines[-1] if lines else "guest filesystem chunk failed"
    ok, lines = guest_command_lines("STUDIO_FS_COMMIT", "WRITE_OK", timeout=8)
    return ok, None if ok else (lines[-1] if lines else "guest filesystem commit failed")

# What this Studio actually put on the host disk, keyed by package id.
#
# WHY this exists: the guest's package registry line carries id, version, state,
# process, name and icon. It does not carry the file the package was loaded from
# and it does not carry a digest. The Packages tab nonetheless had a PACKAGE FILE
# column and a digest cell, so every row rendered an empty cell and the literal
# text "SHA-256 unavailable" -- for all 16 packages, including ones the Studio
# had itself just written to /data/apps/ and hashed.
#
# For a package in this ledger the digest below is the real SHA-256 of the file
# this host wrote. It is not the guest's trust decision, and it is not offered as
# one -- `verify_and_load_sideloaded_package` returns "guest trust remains
# authoritative" for exactly that reason. For every other package there is no
# host file to describe, and the row says so rather than showing a blank.
SIDELOAD_LEDGER = {}


FIRST_PARTY_APP_IDS = frozenset({
    "org.interos.launcher", "org.interos.compositor", "org.interos.init",
    "org.interos.settings", "org.interos.calculator", "org.interos.dialer",
    "org.interos.vault", "org.interos.camera", "org.interos.browser",
    "org.interos.messages", "org.interos.calendar", "org.interos.music",
    "org.interos.file_manager", "org.interos.gallery",
    "org.interos.notes"
})

def guest_package_catalog():
    ok, lines = guest_command_lines("STUDIO_PKG_LIST", "[STUDIO_PKG] END", timeout=8)
    if not ok:
        return False, lines[-1] if lines else "guest package registry unavailable", []
    packages = []
    for line in lines:
        if not line.startswith("[STUDIO_PKG] id="):
            continue
        fields = dict(part.split("=", 1) for part in line[len("[STUDIO_PKG] "):].split() if "=" in part)
        pkg_id = fields.get("id", "")
        entry = {
            "id": pkg_id,
            "version": fields.get("version", ""),
            "status": fields.get("state", "unknown"),
            "process": fields.get("process", ""),
            "name": fields.get("name", fields.get("id", "")).replace("_", " "),
            "icon": fields.get("icon", "APP"),
            "source": "GUEST_REGISTRY",
        }
        # First-party apps embedded in kernel image are always KERNEL_EMBEDDED.
        # Studio never misclassifies first-party native apps (such as Notes) as sideloaded/web.
        known = SIDELOAD_LEDGER.get(pkg_id) if pkg_id not in FIRST_PARTY_APP_IDS else None
        if known:
            entry["filename"] = known.get("filename")
            entry["sha256"] = known.get("sha256")
            entry["format"] = known.get("format")
            entry["origin"] = "SIDELOADED_VIA_STUDIO"
        else:
            entry["filename"] = None
            entry["sha256"] = None
            entry["format"] = None
            entry["origin"] = "KERNEL_EMBEDDED"
        packages.append(entry)
    return True, None, packages


ALLOWED_LOCAL_HOSTS = frozenset({'127.0.0.1', 'localhost', '::1'})

def is_permitted_local_host(hostname):
    """Validate that hostname is strictly an explicitly permitted local loopback identifier.

    Accepts:
      - 'localhost'
      - IPv4 loopback ('127.0.0.1' and any address in 127.0.0.0/8)
      - IPv6 loopback ('::1' and '::ffff:127.0.0.1')
    Rejects:
      - Any external domain (e.g. attacker.com, localhost.attacker.com)
      - Non-loopback IPs (0.0.0.0, 192.168.x.x, 10.x.x.x, 169.254.x.x, etc.)
      - Malformed or empty strings
    """
    if not hostname or not isinstance(hostname, str):
        return False
    h = hostname.strip().lower()
    if h == 'localhost':
        return True
    try:
        ip = ipaddress.ip_address(h)
        return ip.is_loopback
    except ValueError:
        return False


def _parse_authority(authority):
    """Parses a host[:port] authority string, supporting IPv6 bracket notation.

    Returns (hostname, port) or (None, None) if malformed.
    """
    if not authority or not isinstance(authority, str):
        return None, None
    s = authority.strip()
    try:
        parsed = urllib.parse.urlsplit(f'//{s}')
        return parsed.hostname, parsed.port
    except (ValueError, IndexError):
        return None, None


POST_ONLY_ROUTES = frozenset({
    '/api/qemu/launch',
    '/api/qemu/stop',
    '/api/qemu/reboot',
    '/api/qemu/reset',
    '/api/qemu/cmd',
    '/api/serial/write',
    '/api/rapp/trust',
    '/api/rapp/install',
    '/api/rapp/uninstall',
    '/api/apps/uninstall',
    '/api/apps/launch',
    '/api/rapp/launch',
    '/api/rapp/ipc',
    '/api/hardware/orientation',
    '/api/orientation',
    '/api/hardware/sensor_inject',
    '/api/touch/event',
    '/api/input/key',
    '/api/power/state',
    '/api/themes/apply',
    '/api/themes/save',
    '/api/themes/bootloader',
    '/api/fs/read',
    '/api/fs/write',
    '/api/telephony/dial',
    '/api/telephony/hangup',
    '/api/telephony/data/toggle',
    '/api/telephony/sms/send',
    '/api/telephony/at_cmd',
    '/api/web/import',
    '/api/web/hmr/notify',
    '/api/permission/request',
})

GET_ONLY_ROUTES = frozenset({
    '/api/status',
    '/api/qemu/status',
    '/api/qemu/telemetry',
    '/api/screen/raw',
    '/api/screen/binary_stream',
    '/api/screen/metrics',
    '/api/screen/frame',
    '/api/browser/proxy',
    '/api/themes',
    '/api/apps/list',
    '/api/kernel/frame-timing',
    '/api/rapp/list',
    '/api/telephony/status',
    '/api/telephony/sms/list',
    '/api/pmic/status',
    '/api/kernel/processes',
    '/api/kernel/ipc',
    '/api/device/profiles',
    '/api/web/hmr',
})


class StudioAPIHandler(http.server.SimpleHTTPRequestHandler):
    """Extended handler with API endpoints for QEMU control, serial logs, and live telemetry."""

    extensions_map = {
        '': 'application/octet-stream',
        '.html': 'text/html; charset=utf-8',
        '.css': 'text/css; charset=utf-8',
        '.js': 'application/javascript; charset=utf-8',
        '.mjs': 'application/javascript; charset=utf-8',
        '.json': 'application/json',
        '.png': 'image/png',
        '.jpg': 'image/jpeg',
        '.svg': 'image/svg+xml',
        '.wasm': 'application/wasm',
    }

    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=DIRECTORY, **kwargs)

    def end_headers(self):
        self.send_header('Cache-Control', 'no-cache, no-store, must-revalidate')
        self.send_header('Pragma', 'no-cache')
        self.send_header('Expires', '0')
        super().end_headers()

    # Largest request body the API will buffer. Content-Length is entirely
    # client-controlled, so an unbounded read lets any local page advertise a
    # multi-gigabyte payload and exhaust memory before a single payload byte
    # is inspected.
    MAX_POST_BYTES = 64 * 1024 * 1024
    MAX_DRAIN_BYTES = 64 * 1024

    def _send_json(self, status, payload, headers=None):
        """Writes a JSON response so rejections match regular replies."""
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        if getattr(self, 'close_connection', False):
            self.send_header('Connection', 'close')
        if headers:
            for k, v in headers.items():
                self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def _drain_incoming_body_if_any(self):
        """Reads and discards incoming body up to MAX_DRAIN_BYTES so early rejections don't RST.

        If Content-Length exceeds MAX_DRAIN_BYTES or is missing/malformed, the body cannot
        be safely consumed without unbounded overhead. In that case, drains up to
        MAX_DRAIN_BYTES and closes the connection to prevent smuggling or desynchronization.
        """
        raw = self.headers.get('Content-Length')
        if not raw:
            if self.headers.get('Transfer-Encoding', '').lower() == 'chunked':
                self.close_connection = True
            return
        try:
            cl = int(raw)
            if cl < 0:
                self.close_connection = True
                return
            if cl <= self.MAX_DRAIN_BYTES:
                self._drain_body(cl)
            else:
                self._drain_body(self.MAX_DRAIN_BYTES)
                self.close_connection = True
        except (TypeError, ValueError):
            self.close_connection = True

    def _send_method_not_allowed(self, allowed='GET, POST, OPTIONS'):
        self._drain_incoming_body_if_any()
        self._send_json(405, {
            'success': False,
            'error': f'Method {self.command} not allowed'
        }, headers={'Allow': allowed})

    def _bounded_content_length(self):
        """Validates Content-Length before any body is buffered.

        Returns the byte count when the header is sane and within
        MAX_POST_BYTES; otherwise answers 400/413 itself and returns None so
        the caller can stop. Oversized bodies are drained (up to 1 MiB) and the
        connection is closed, mirroring the /api/qemu/cmd payload guard, so
        unread bytes cannot desynchronise a keep-alive stream.
        """
        raw = self.headers.get('Content-Length', '0')
        try:
            length = int(raw)
        except (TypeError, ValueError):
            length = -1
        if length < 0:
            self._send_json(400, {'success': False, 'error': 'Invalid Content-Length'})
            return None
        if length > self.MAX_POST_BYTES:
            self._drain_body(min(length, 1024 * 1024))
            self.send_response(413)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Connection', 'close')
            self.send_header('Content-Length', '0')
            self.end_headers()
            return None
        return length

    def _get_server_port(self):
        try:
            return int(self.server.server_address[1])
        except (AttributeError, IndexError, TypeError, ValueError):
            return PORT

    def _origin_allowed(self):
        """Strict local-only enforcement for Device Studio API and control routes.

        Defends against DNS rebinding, cross-origin drive-by attacks, and
        forged requests:
        1. Host authority: must have a valid Host header whose hostname resolves
           to a permitted local loopback host ('localhost', '127.0.0.1', '::1').
           If a port is specified, it must match the server's listening port.
        2. Client address: connection must originate from a loopback address.
        3. Origin header (if present): scheme must be http/https, hostname must be
           a permitted local loopback host, and port (if present) must match the server.
        4. Referer header (if present): scheme must be http/https, hostname must be
           a permitted local loopback host.
        5. Sec-Fetch-Site (if present): requests declaring 'cross-site' are rejected.
        6. Missing Origin policy: requests without Origin are only authorized if
           Host, client address, and Referer all pass local verification, and the
           request does not indicate cross-site browser navigation/document submission.
        """
        # 1. Host authority verification
        host_hdr = self.headers.get('Host')
        if not host_hdr:
            return False
        host_name, host_port = _parse_authority(host_hdr)
        if not host_name or not is_permitted_local_host(host_name):
            return False
        server_port = self._get_server_port()
        if host_port is not None and host_port != server_port:
            return False

        # 2. Client address verification (must be loopback)
        client_ip = self.client_address[0] if hasattr(self, 'client_address') and self.client_address else None
        if client_ip and not is_permitted_local_host(client_ip):
            return False

        # 3. Origin header verification (when present)
        origin = self.headers.get('Origin')
        if origin is not None:
            try:
                parsed_origin = urllib.parse.urlsplit(origin)
            except ValueError:
                return False
            if parsed_origin.scheme not in ('http', 'https'):
                return False
            origin_host = parsed_origin.hostname
            if not origin_host or not is_permitted_local_host(origin_host):
                return False
            origin_port = parsed_origin.port
            if origin_port is None:
                origin_port = 443 if parsed_origin.scheme == 'https' else 80
            if origin_port != server_port:
                return False

        # 4. Referer header verification (when present)
        referer = self.headers.get('Referer')
        if referer:
            try:
                parsed_ref = urllib.parse.urlsplit(referer)
                if parsed_ref.scheme in ('http', 'https'):
                    ref_host = parsed_ref.hostname
                    if ref_host and not is_permitted_local_host(ref_host):
                        return False
            except ValueError:
                return False

        # 5. Fetch metadata cross-site defense
        sec_fetch_site = self.headers.get('Sec-Fetch-Site', '').strip().lower()
        if sec_fetch_site == 'cross-site':
            return False

        # 6. Deliberate missing-origin policy
        if origin is None:
            sec_fetch_mode = self.headers.get('Sec-Fetch-Mode', '').strip().lower()
            sec_fetch_dest = self.headers.get('Sec-Fetch-Dest', '').strip().lower()
            if (sec_fetch_mode == 'navigate' or sec_fetch_dest in ('document', 'iframe')) and self.command in ('POST', 'PUT', 'DELETE', 'PATCH'):
                return False

        return True

    def do_GET(self):
        if not self._origin_allowed():
            self._send_json(403, {'success': False, 'error': 'Cross-origin request rejected'})
            return
        parsed_path = urllib.parse.urlparse(self.path)

        if parsed_path.path in POST_ONLY_ROUTES or parsed_path.path.startswith('/api/apps/upload'):
            self._send_method_not_allowed(allowed='POST')
            return

        # Device profiles (AVD-Manager equivalent). Pure data from DEVICE_PROFILES;
        # it lists the declarative profiles and the active one. It reports the
        # geometry a profile ASKS for — Studio never claims a scanout the guest
        # has not actually presented.
        if parsed_path.path == '/api/device/profiles':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': True,
                'active': active_device_profile_key,
                'profiles': [device_profile_public(pid) for pid in DEVICE_PROFILES],
            }).encode())
            return

        if parsed_path.path == '/api/device/profile':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': True,
                'active': active_device_profile_key,
                'profile': device_profile_public(),
            }).encode())
            return

        if self.path.startswith('/api/serial'):
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            after = 0
            if '?after=' in self.path:
                try:
                    after_str = self.path.split('?after=')[1].split('&')[0]
                    after = int(after_str)
                except (ValueError, IndexError):
                    after = 0

            # Sync with the live QEMU serial log on disk if available. Only the
            # log this server rewrote for the current boot session is read: the
            # historical target/x86_boot.log from earlier sessions is longer
            # than the in-memory ring, so merging it appended a stale tail as
            # if it were live guest output and pushed the real lines (and their
            # order) into the middle of the buffer.
            boot_log = current_serial_log_path if serial_log_is_session else None
            if boot_log and os.path.exists(boot_log):
                try:
                    with open(boot_log, "r", encoding="utf-8", errors="replace") as f:
                        disk_lines = [l.strip() for l in f if l.strip()]
                        if len(disk_lines) > len(serial_lines):
                            serial_lines.extend(disk_lines[len(serial_lines):])
                except Exception:
                    pass

            lines = serial_lines[after:] if after < len(serial_lines) else []
            # The Studio says whether these lines are the live guest's output
            # or the tail of a session that has ended. The lines themselves are
            # real guest output either way, but calling them "live" for a
            # device that is not running would be a claim the Studio cannot
            # back.
            session_active = bool(serial_log_is_session and tracked_qemu_process_alive())
            if session_active:
                source = 'LIVE_GUEST_UART'
            elif serial_lines:
                source = 'LAST_SESSION_UART'
            else:
                source = 'UNAVAILABLE'
            response = json.dumps({
                'lines': lines,
                'total': len(serial_lines),
                'cursor': len(serial_lines),
                'next_index': len(serial_lines),
                'source': source,
                'session_active': session_active,
                'qemu_running': tracked_qemu_process_alive(),
            })
            self.wfile.write(response.encode())
            return

        if self.path in ('/api/status', '/api/qemu/status'):
            t = query_qemu_monitor()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            launcher_ready = check_launcher_ready()
            # A ready flag latched from a previous session's disk log must not
            # describe a dead VM: readiness is only meaningful while QEMU runs.
            response = json.dumps({
                'qemu': t["qemu_alive"],
                'status': t["status"],
                'mode': t.get("mode", "standby"),
                # What the Studio can prove about the device, and why. "unknown"
                # is an answer, not a gap: it means the Studio cannot tell
                # whether the device is there.
                'device_state': t.get("device_state", "unknown"),
                'device_reason': t.get("device_reason"),
                'launcher_ready': bool(launcher_ready and t["qemu_alive"]),
                'active_profile': active_profile_key,
                # The selected device profile (AVD-Manager equivalent). It is the
                # geometry Studio will ASK the guest to modeset on the next cold
                # boot, not the scanout the guest has already produced; the live
                # size is only reported once the guest actually presents a frame.
                'device_profile': active_device_profile_key,
                # Report the geometry this machine was actually launched with, so
                # the studio describes the running target rather than a generic
                # specification.
                'ram_mb': (SUPPORTED_PROFILES.get(active_profile_key) or {}).get('ram'),
                'guest_width': ((SUPPORTED_PROFILES.get(active_profile_key) or {}).get('display') or {}).get('guest_width'),
                'guest_height': ((SUPPORTED_PROFILES.get(active_profile_key) or {}).get('display') or {}).get('guest_height'),
                'orientation': current_device_orientation,
                'pid': qemu_subproc.pid if qemu_subproc else None,
                # The VNC display the guest was actually given, and the port it
                # listens on. These are the configured values until a launch
                # moves the guest onto a free display.
                'vnc_display': active_vnc_display,
                'vnc_port': active_vnc_port,
            })
            self.wfile.write(response.encode())
            return

        if self.path == '/api/qemu/telemetry':
            t = query_qemu_monitor().copy()
            t["launcher_ready"] = bool(check_launcher_ready() and t.get("qemu_alive"))
            t["transport_fps"] = round(vnc_client.metrics["fps"], 1) if vnc_client.connected else round(_transport_metrics["fps"], 1)
            t["transport_frame_us"] = int(vnc_client.metrics["frame_us"]) if vnc_client.connected else int(_transport_metrics["frame_us"])
            t["vnc_connected"] = vnc_client.connected
            # Guest FPS from real KMS flips or reported as Unknown
            t["guest_fps"] = "Unknown"
            with telemetry_lock:
                for line in reversed(serial_lines[-200:]):
                    if "[KMS]" in line and "vsync=" in line:
                        t["guest_fps"] = 60  # nominal target when KMS active
                        break
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps(t).encode())
            return

        if parsed_path.path == '/api/screen/raw':
            query = urllib.parse.parse_qs(parsed_path.query)
            try:
                client_seq = int(query.get('seq', ['0'])[0])
            except ValueError:
                client_seq = 0
            bin_msg, seq = vnc_client.get_binary_frame(client_seq=client_seq, timeout=0.040)
            if bin_msg:
                self.send_response(200)
                self.send_header('Content-Type', 'application/octet-stream')
                self.send_header('Cache-Control', 'no-cache, no-store, must-revalidate')
                self.send_header('X-Frame-Seq', str(seq))
                self.end_headers()
                self.wfile.write(bin_msg)
            else:
                self.send_response(304 if client_seq > 0 else 503)
                self.end_headers()
            return

        if parsed_path.path == '/api/screen/binary_stream':
            self.send_response(200)
            self.send_header('Content-Type', 'application/octet-stream')
            self.send_header('Cache-Control', 'no-cache, no-store, must-revalidate')
            self.send_header('Connection', 'close')
            self.end_headers()
            last_seq = 0
            try:
                while True:
                    with vnc_client.frame_cond:
                        while vnc_client.seq <= last_seq and vnc_client.connected:
                            vnc_client.frame_cond.wait(timeout=0.1)
                        if not vnc_client.connected:
                            break
                        if vnc_client.seq > last_seq:
                            msg = vnc_client.latest_binary_msg
                            last_seq = vnc_client.seq
                        else:
                            msg = None
                    if msg:
                        self.wfile.write(msg)
                        self.wfile.flush()
            except (ConnectionResetError, BrokenPipeError, socket.error):
                pass
            return

        if parsed_path.path == '/api/screen/metrics':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'transport_fps': round(vnc_client.metrics['fps'], 2),
                'frame_time_us': vnc_client.metrics['frame_us'],
                'frames_total': vnc_client.metrics['frames_total'],
                'seq': vnc_client.seq,
                'vnc_connected': vnc_client.connected,
                'width': vnc_client.latest_raw_w,
                'height': vnc_client.latest_raw_h,
            }).encode())
            return

        if parsed_path.path == '/api/screen/frame':
            # Single-shot canonical guest frame for the screenshot control and
            # the canvas polling fallback. Serves only live guest-derived frames
            # (native portrait 720x1440, portrait phone slice 360x720, or full
            # landscape 1280x720); anything else is an honest 503 so the phone
            # never shows a broken image.
            #
            # WHY the device state gates this route: a frame served from a
            # stale session (or from whatever else holds the VNC port) is not
            # the guest's screen, and presenting it as one is the same class
            # of claim the liveness fix removed. No provable device, no frame.
            #
            # WHY the query is parsed here: this route used `query` without
            # ever assigning it in this scope, so every request raised
            # UnboundLocalError and the client got no response at all.
            query = urllib.parse.parse_qs(parsed_path.query)
            prefer_jpeg = query.get('format', ['png'])[0].lower() == 'jpeg'
            img_bytes, mime_or_reason = None, 'No guest frame available'
            device = qemu_device_state()
            if not device["running"]:
                self.send_response(503)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Cache-Control', 'no-cache, no-store, must-revalidate')
                self.end_headers()
                self.wfile.write(json.dumps({
                    'success': False,
                    'error': f"No guest frame available: {device['reason']}",
                    'device_state': device['state'],
                }).encode())
                return
            with vnc_client.frame_cond:
                live_canonical = (
                    vnc_client.connected
                    and vnc_client.latest_raw_rgba is not None
                    and (vnc_client.latest_raw_w, vnc_client.latest_raw_h) in PHONE_FRAME_SIZES
                )
            if live_canonical:
                img_bytes, mime_or_reason = vnc_client.get_image_frame(prefer_jpeg=prefer_jpeg)
            if img_bytes is None:
                # VNC session not established but the guest is alive: fall back
                # to the existing capture pipeline (live screendump, canonical
                # portrait only) rather than inventing pixels.
                img_bytes, mime_or_reason = capture_qemu_screen_frame(prefer_jpeg=prefer_jpeg)
            if img_bytes is not None:
                self.send_response(200)
                self.send_header('Content-Type', mime_or_reason)
                self.send_header('Content-Length', str(len(img_bytes)))
                self.send_header('Cache-Control', 'no-cache, no-store, must-revalidate')
                self.end_headers()
                self.wfile.write(img_bytes)
            else:
                self.send_response(503)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Cache-Control', 'no-cache, no-store, must-revalidate')
                self.end_headers()
                reason = mime_or_reason if (img_bytes is None and mime_or_reason) else 'No guest frame available'
                self.wfile.write(json.dumps({'success': False, 'error': reason}).encode())
            return

        if parsed_path.path == '/api/audio/volume':
            query = urllib.parse.parse_qs(parsed_path.query)
            if 'action' in query or 'value' in query:
                self._send_method_not_allowed(allowed='POST')
                return
            cur_vol = None
            with telemetry_lock:
                for line in reversed(serial_lines):
                    if "[AUDIO]" in line and "volume_percent=" in line:
                        m = re.search(r'volume_percent=(\d+)', line)
                        if m:
                            cur_vol = int(m.group(1))
                            break
            success = cur_vol is not None
            self.send_response(200 if success else 503)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': success,
                'volume_percent': cur_vol,
                'status': 'REPORTED' if success else 'UNBOUND'
            }).encode())
            return

        if parsed_path.path == '/api/browser/proxy':
            query = urllib.parse.parse_qs(parsed_path.query)
            target_url = query.get('url', [''])[0]
            if not target_url:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'error': 'Missing url parameter'}).encode())
                return

            safe, reason = is_safe_proxy_url(target_url)
            if not safe:
                self.send_response(403)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': f"SSRF Blocked: {reason}"}).encode())
                return

            try:
                req = urllib.request.Request(target_url, headers={
                    'User-Agent': 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1 InterOS/8.2',
                    'Accept': 'text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8',
                    'Accept-Language': 'en-US,en;q=0.9'
                })
                with urllib.request.build_opener(_SsrfGuardedRedirectHandler).open(req, timeout=6) as response:
                    raw_data = response.read(1048576) # Read up to 1MB
                    content_type = response.headers.get('Content-Type', 'text/html')
                    text_content = raw_data.decode('utf-8', errors='ignore')

                # Inject base tag and viewport so modern websites render CSS, images, and fonts correctly
                if 'html' in content_type.lower():
                    base_tag = f'<base href="{target_url}"><meta name="viewport" content="width=device-width, initial-scale=1.0">'
                    if '<head>' in text_content:
                        text_content = text_content.replace('<head>', f'<head>{base_tag}', 1)
                    elif '<head ' in text_content:
                        idx = text_content.find('>', text_content.find('<head '))
                        if idx != -1:
                            text_content = text_content[:idx+1] + base_tag + text_content[idx+1:]
                    elif '<HEAD>' in text_content:
                        text_content = text_content.replace('<HEAD>', f'<HEAD>{base_tag}', 1)
                    else:
                        text_content = base_tag + text_content

                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({
                    'success': True,
                    'url': target_url,
                    'content_type': content_type,
                    'html': text_content
                }).encode())
            except Exception as e:
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({
                    'success': False,
                    'url': target_url,
                    'error': str(e)
                }).encode())
            return

        if parsed_path.path == '/api/themes':
            active = get_active_theme()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'themes': [active],
                'active_id': active['theme_id']
            }).encode())
            return

        if self.path == '/api/kernel/frame-timing':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps(extract_frame_timing()).encode())
            return

        if self.path == '/api/apps/list':
            # The guest package registry is the only authority for what is
            # installed. When the Studio cannot prove a device is running, it
            # says so instead of waiting on a guest that is not there.
            device = qemu_device_state()
            if not device["running"]:
                self.send_response(503)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({
                    'success': False,
                    'source': 'UNAVAILABLE',
                    'status': 'NOT_RUNNING',
                    'apps': [],
                    'total': 0,
                    'error': f"The guest package registry is unavailable: {device['reason']}",
                }).encode())
                return
            ok, error, catalog = guest_package_catalog()
            self.send_response(200 if ok else 503)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': ok,
                'source': 'GUEST_REGISTRY',
                'apps': catalog,
                'total': len(catalog),
                **({'error': error} if error else {})
            }).encode())
            return

        if self.path == '/api/rapp/list':
            device = qemu_device_state()
            if not device["running"]:
                self.send_response(503)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({
                    'success': False,
                    'source': 'UNAVAILABLE',
                    'status': 'NOT_RUNNING',
                    'apps': [],
                    'total': 0,
                    'error': f"The guest package registry is unavailable: {device['reason']}",
                }).encode())
                return
            ok, error, catalog = guest_package_catalog()
            rapp_apps = [a for a in catalog if a.get('id')]
            self.send_response(200 if ok else 503)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': ok,
                'source': 'GUEST_REGISTRY',
                'apps': rapp_apps,
                'total': len(rapp_apps),
                **({'error': error} if error else {})
            }).encode())

            return

        if self.path == '/api/telephony/status':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': True,
                'telephony': extract_guest_subsystem("telephony", "[TELEPHONY]")
            }).encode())
            return

        if self.path == '/api/telephony/sms/list':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            messages = extract_guest_subsystem("telephony", "[TELEPHONY]").get("sms_messages", [])
            self.wfile.write(json.dumps({'success': True, 'messages': messages, 'source': 'KERNEL_TELEMETRY' if messages else 'UNAVAILABLE'}).encode())
            return

        if self.path == '/api/pmic/status':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            pmic_info = extract_guest_subsystem("pmic", "[PMIC]")
            self.wfile.write(json.dumps({'success': True, 'pmic': pmic_info}).encode())
            return

        if self.path == '/api/kernel/processes':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            procs = extract_real_processes()
            alive = query_qemu_monitor()["qemu_alive"]
            self.wfile.write(json.dumps({
                'success': True,
                'qemu_alive': alive,
                'status': 'RUNNING' if alive else 'STANDBY',
                'processes': procs,
                'total': len(procs)
            }).encode())
            return

        if self.path == '/api/kernel/ipc':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            ipc_events = extract_real_ipc()
            self.wfile.write(json.dumps({'success': True, 'events': ipc_events, 'total': len(ipc_events)}).encode())
            return

        if parsed_path.path == '/api/web/hmr':
            query = urllib.parse.parse_qs(parsed_path.query)
            app_id = query.get('app_id', [''])[0]
            try:
                since = int(query.get('since', ['0'])[0])
            except (ValueError, TypeError):
                since = 0
            try:
                timeout = min(max(float(query.get('timeout', ['10'])[0]), 0.0), 30.0)
            except (ValueError, TypeError):
                timeout = 10.0

            with web_hmr_cond:
                matching = [e for e in web_hmr_events if e['id'] > since and (not app_id or e['app_id'] == app_id)]
                if not matching and timeout > 0:
                    web_hmr_cond.wait(timeout=timeout)
                    matching = [e for e in web_hmr_events if e['id'] > since and (not app_id or e['app_id'] == app_id)]

            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Cache-Control', 'no-cache, no-store, must-revalidate')
            self.end_headers()
            latest_id = web_hmr_events[-1]['id'] if web_hmr_events else 0
            self.wfile.write(json.dumps({'success': True, 'events': matching, 'latest_id': latest_id}).encode())
            return

        super().do_GET()

    def _drain_body(self, content_length):
        """Reads and discards a request body.

        A handler that rejects a request without consuming its body leaves those
        bytes in the socket, so the next request on the same keep-alive
        connection is parsed against a desynchronised stream and the client sees
        a reset instead of a response.
        """
        if content_length <= 0:
            return
        try:
            remaining = int(content_length)
            while remaining > 0:
                chunk = self.rfile.read(min(remaining, 65536))
                if not chunk:
                    break
                remaining -= len(chunk)
        except (OSError, ValueError):
            pass

    def do_POST(self):
        if not self._origin_allowed():
            self._drain_incoming_body_if_any()
            self._send_json(403, {'success': False, 'error': 'Cross-origin request rejected'})
            return
        parsed_path = urllib.parse.urlparse(self.path)
        if parsed_path.path in GET_ONLY_ROUTES or parsed_path.path == '/api/serial':
            self._send_method_not_allowed(allowed='GET')
            return
        # Orientation state is module-level; without this declaration the
        # assignment below would make the name function-local and every touch
        # payload that consults the orientation would raise UnboundLocalError.
        global serial_lines, current_device_orientation

        if parsed_path.path == '/api/audio/volume':
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
            except Exception:
                data = {}
            action = str(data.get('action', '')).strip().upper()
            val = str(data.get('value', '')).strip()
            if not action and not val:
                self._send_json(400, {'success': False, 'error': 'Missing action or value parameter'})
                return
            if action and action not in ('UP', 'DOWN', 'MUTE', 'GET'):
                self._send_json(400, {'success': False, 'error': f'Invalid action {action}'})
                return
            if val:
                try:
                    ival = int(val)
                    if ival < 0 or ival > 100:
                        raise ValueError()
                except ValueError:
                    self._send_json(400, {'success': False, 'error': 'Volume value must be an integer between 0 and 100'})
                    return
                cmd = f"VOLUME {val}"
            else:
                cmd = f"VOLUME {action}"
            with serial_socket_lock:
                has_conn = serial_socket_conn is not None
            if not has_conn:
                self.send_response(503)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({
                    'success': False,
                    'status': 'UNAVAILABLE',
                    'volume_percent': None,
                    'error': 'QEMU COM1 serial input socket is offline or not connected'
                }).encode())
                return

            ok, msg = send_guest_serial_command(cmd, '[AUDIO]', timeout=3)
            cur_vol = None
            with telemetry_lock:
                for line in reversed(serial_lines):
                    if "[AUDIO]" in line and "volume_percent=" in line:
                        m = re.search(r'volume_percent=(\d+)', line)
                        if m:
                            cur_vol = int(m.group(1))
                            break
            success = bool(ok and cur_vol is not None)
            self.send_response(200 if success else 503)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': success,
                'volume_percent': cur_vol,
                'status': 'REQUESTED_NOT_APPLIED' if success else 'UNBOUND',
                'message': msg
            }).encode())
            return

        if self.path == '/api/device/profile':
            # Select the device profile applied on the next cold boot. Selection
            # only records a request; it never re-modesets a running guest, so
            # the answer says plainly that it applies on the next relaunch.
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            try:
                body = self.rfile.read(content_length) if content_length > 0 else b'{}'
                data = json.loads(body.decode('utf-8'))
            except Exception:
                data = {}
            requested = str(data.get('profile') or data.get('id') or data.get('key') or '').strip()
            ok, err = select_device_profile(requested)
            if not ok:
                self._send_json(400, {
                    'success': False,
                    'error': err,
                    'available': sorted(DEVICE_PROFILES)
                })
                return
            self._send_json(200, {
                'success': True,
                'active': active_device_profile_key,
                'applies_on': 'next cold boot / relaunch',
                'profile': device_profile_public()
            })
            return

        if self.path == '/api/rapp/trust':
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            body = self.rfile.read(content_length)
            try:
                data = json.loads(body.decode('utf-8'))
                public_key = str(data.get("public_key", "")).strip().lower()
                publisher = str(data.get("publisher", "developer")).strip()
                if len(public_key) != 64 or not is_safe_publisher(publisher):
                    raise ValueError("public_key must be 32-byte hex and publisher must be 1-32 safe identifier characters")
                if any(ch not in "0123456789abcdef" for ch in public_key):
                    raise ValueError("public_key is not hexadecimal")
                ok, msg = send_guest_serial_command(
                    f"RAPP_TRUST {public_key} {publisher}",
                    "[RAPP_HOST] TRUSTED",
                    timeout=10,
                )
                self.send_response(200 if ok else 409)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({"success": ok, "message": msg, "source": "QEMU_COM1"}).encode())
            except Exception as e:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({"success": False, "error": str(e)}).encode())
            return

        if self.path == '/api/rapp/install':
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            raw_pkg_name = self.headers.get('X-Package-Name', 'unnamed.rapp')
            if '..' in raw_pkg_name or '/' in raw_pkg_name or '\\' in raw_pkg_name:
                # Drain the request body before responding. Leaving it unread
                # desyncs the next request on this keep-alive connection, which
                # surfaces to the client as a connection reset instead of the
                # intended rejection.
                self._drain_body(content_length)
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Path traversal in package name is strictly forbidden'}).encode())
                return
            pkg_name = os.path.basename(raw_pkg_name)
            body = self.rfile.read(content_length)

            ok, msg, app_entry = verify_and_install_rapp(body, pkg_name)
            if not ok:
                serial_lines.append(f"[RAPP_REJECT] Package '{pkg_name}' rejected: {msg}")
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': msg}).encode())
                return

            app_id = app_entry["id"]
            app_name = app_entry["name"]
            version = app_entry["version"]

            guest_ok, guest_msg = transfer_rapp_to_guest(body, app_id)
            if not guest_ok:
                serial_lines.append(f"[RAPP_REJECT] Guest install failed for '{pkg_name}': {guest_msg}")
                self.send_response(409)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({
                    'success': False,
                    'status': 'GUEST_REJECTED',
                    'error': guest_msg,
                    'package': app_entry,
                }).encode())
                return

            # Guest registry is the sole installed-package authority.
            serial_lines.append(f"[RAPP_INSTALL] Guest accepted package '{app_name}' ({app_id} v{version}).")
            serial_lines.append(f"[RAPP_SECURITY] Guest registry remains authoritative; publisher trust is enforced by the kernel.")

            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({'success': True, 'package': app_entry}).encode())
            return

        if self.path in ('/api/rapp/uninstall', '/api/apps/uninstall'):
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            post_body = self.rfile.read(content_length)
            try:
                data = json.loads(post_body.decode('utf-8'))
                target_id = str(data.get("app_id") or data.get("filename") or data.get("name") or "").strip()
                if not is_safe_package_id(target_id):
                    raise ValueError('invalid target package id')
                guest_ok, guest_msg = send_guest_serial_command(
                    f"RAPP_UNINSTALL {target_id}",
                    f"[RAPP_HOST] UNINSTALLED id={target_id}",
                    timeout=12,
                )
                self.send_response(200 if guest_ok else 409)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({
                    'success': guest_ok,
                    'source': 'GUEST_PACKAGE_MANAGER',
                    'uninstalled': target_id,
                    **({} if guest_ok else {'error': guest_msg})
                }).encode())
            except Exception as e:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': str(e)}).encode())
            return

        if self.path in ('/api/apps/launch', '/api/rapp/launch'):
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            body = self.rfile.read(content_length)
            try:
                data = json.loads(body.decode('utf-8'))
                app_id = str(data.get("app_id", "")).strip()
                if not is_safe_package_id(app_id):
                    raise ValueError("invalid app_id")
                ok, msg = send_guest_serial_command(f"RAPP_LAUNCH {app_id}", "[RAPP_HOST] LAUNCHED", timeout=12)
                self.send_response(200 if ok else 409)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({"success": ok, "message": msg, "app_id": app_id, "source": "QEMU_COM1"}).encode())
            except Exception as e:
                self.send_response(400)
                self.end_headers()
                self.wfile.write(json.dumps({"success": False, "error": str(e)}).encode())
            return
        if self.path == '/api/rapp/ipc':
            self.send_response(409)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': False,
                'status': 'UNAVAILABLE',
                'source': 'UNAVAILABLE',
                'error': 'IPC relay requires a live guest IPC endpoint; Device Studio does not synthesize acknowledgements'
            }).encode())
            return
        if self.path.startswith('/api/apps/upload'):
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            body = self.rfile.read(content_length)

            query = urllib.parse.urlparse(self.path).query
            params = urllib.parse.parse_qs(query)
            raw_query_fname = params.get('filename', [''])[0]
            if '..' in raw_query_fname or '/' in raw_query_fname or '\\' in raw_query_fname:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Path traversal in filename is strictly forbidden'}).encode())
                return
            query_fname = os.path.basename(raw_query_fname)
            query_name = params.get('name', [''])[0]

            sha256 = hashlib.sha256(body).hexdigest()
            fmt_str = "Raw Binary (.bin)"
            ext = ".app"
            app_type = "Generic"
            capabilities = ["DisplayScanout", "AudioPlayback", "SandboxedVFS"]
            is_apk = False
            apk_meta = None
            filename = query_fname or (f"app_{sha256[:8]}{ext}")
            app_name = query_name or filename.split('.')[0].replace('_', ' ').replace('-', ' ').title()
            app_id = f"sideloaded.{sha256[:8]}"
            version = "1.0.0"
            dex_classes = 0
            dex_size = 0
            signature = "None"
            entrypoint = ""

            if body.startswith(b"\x7fELF"):
                fmt_str = "Native Rust App (.app / .rap)"
                ext = ".app"
                app_type = "Native Ring 3"
                if not query_fname:
                    filename = f"app_{sha256[:8]}{ext}"
            elif body.startswith(b"\x00asm"):
                fmt_str = "WebAssembly (.wasm)"
                ext = ".wasm"
                app_type = "WASM Sandboxed"
                if not query_fname:
                    filename = f"app_{sha256[:8]}{ext}"
            elif query_fname.endswith(".rapp"):
                # RAPP v2 has a dedicated signed-binary installation endpoint.
                # Do not silently reinterpret a ZIP/JSON package as executable.
                self.send_response(409)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({
                    'success': False,
                    'status': 'UNAVAILABLE',
                    'error': 'Use /api/rapp/install for canonical RAPP v2 binary packages'
                }).encode())
                return
            elif query_fname.endswith(".apk") or (body.startswith(b"PK\x03\x04") and (b"AndroidManifest.xml" in body or b"classes.dex" in body)):
                fmt_str = "Android Package (.apk)"
                ext = ".apk"
                app_type = "Android ACR Sandbox"
                is_apk = True
                apk_meta = inspect_apk(body)
                if not query_fname:
                    filename = f"app_{sha256[:8]}{ext}"
            elif body.startswith(b"PK\x03\x04"):
                try:
                    with zipfile.ZipFile(io.BytesIO(body), "r") as zf:
                        names = zf.namelist()
                        if "AndroidManifest.xml" in names:
                            fmt_str = "Android Package (.apk)"
                            ext = ".apk"
                            app_type = "Android ACR Sandbox"
                            is_apk = True
                            apk_meta = inspect_apk(body)
                            if not query_fname:
                                filename = f"app_{sha256[:8]}{ext}"
                        else:
                            fmt_str = "Rust App Package (.rapp)"
                            ext = ".rapp"
                            app_type = "Sovereign RAPP"
                            if not query_fname:
                                filename = f"app_{sha256[:8]}{ext}"
                            if "manifest.json" in names:
                                m_data = json.loads(zf.read("manifest.json").decode('utf-8'))
                                app_id = m_data.get("id", f"org.interos.{filename.split('.')[0]}")
                                app_name = m_data.get("name", query_name or filename.split('.')[0].title())
                                version = m_data.get("version", version)
                                capabilities = m_data.get("requested_capabilities", capabilities)
                except Exception:
                    pass

            if is_apk and apk_meta and apk_meta.get("is_valid_apk"):
                app_id = apk_meta.get("package_id", app_id)
                app_name = apk_meta.get("app_name", app_name)
                version = apk_meta.get("version_name", version)
                capabilities = apk_meta.get("permissions", ["android.permission.INTERNET", "CAP_ACR_CONTAINER"])
                dex_classes = apk_meta.get("dex_classes", 0)
                dex_size = apk_meta.get("dex_size", 0)
                signature = apk_meta.get("signature_scheme", "No signature metadata detected")
                entrypoint = apk_meta.get("main_activity", "")

            # Uploaded artifacts are analyzed/transferred only; installation is guest-owned.

            app_entry = {
                "id": app_id,
                "name": app_name,
                "filename": filename,
                "format": fmt_str,
                "type": app_type,
                "size": len(body),
                "sha256": sha256[:16] + "...",
                "status": "Analyzed; guest installation required",
                "version": version,
                "dex_classes": dex_classes,
                "dex_size": dex_size,
                "capabilities": capabilities,
                "signature": signature,
                "entrypoint": entrypoint,
                "acr": is_apk
            }

            # Guest package registry remains the only installed-app authority.
            # Log studio inspection telemetry
            serial_lines.append(f"[STUDIO_SIDELOAD] Ingested package '{filename}' (Format: {fmt_str}, Size: {len(body)} bytes).")
            serial_lines.append(f"[STUDIO_AUDIT] SHA-256 digest computed: [{sha256[:16]}...]; guest trust verification remains authoritative.")
            if is_apk:
                serial_lines.append(f"[STUDIO_ACR_AUDIT] DEX header parsed: {dex_classes} classes, {dex_size} bytes; execution has not been started.")
                serial_lines.append(f"[STUDIO_ACR_AUDIT] APK signature metadata: {signature}. Cryptographic signature verification is not performed by this inspection endpoint.")
                serial_lines.append(f"[STUDIO_ACR] Declared Android permissions recorded for '{app_id}'; enforcement occurs only in the guest ACR runtime.")
            else:
                serial_lines.append(f"[STUDIO_AUDIT] Uploaded artifact '{app_name}' analyzed; guest installation remains authoritative.")

            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({'success': True, 'package': app_entry}).encode())
            return

        if self.path == '/api/qemu/launch':
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
            except Exception:
                data = {}
            profile = str(data.get('profile') or active_profile_key or 'x86_64').strip()
            # A live guest already satisfies "Power On"; launching a second
            # QEMU next to the running one would be reported as success while
            # nothing changed. Liveness is the tracked PROCESS plus the
            # monitor, never a port: a stale listener used to make this route
            # answer "QEMU is already running" for a device that did not exist,
            # and that same stale state blocked a real launch.
            state = qemu_device_state()
            if state["running"]:
                self._send_json(200, {
                    'success': True,
                    'message': 'QEMU is already running',
                    'already_running': True,
                    'profile': profile,
                    'pid': state.get('pid'),
                    'device_state': state['state'],
                    'vnc_display': active_vnc_display,
                    'vnc_port': active_vnc_port,
                })
                return
            ok, resp, info = launch_qemu_profile(profile)
            self._send_json(200 if ok else 500, {
                'success': ok,
                'message': str(resp),
                'profile': profile,
                'pid': (info or {}).get('pid'),
                'device_state': (info or {}).get('device_state'),
                # The display the guest was actually given. When the configured
                # one was taken, a free display was chosen and named here.
                'vnc_display': (info or {}).get('vnc_display', active_vnc_display),
                'vnc_port': (info or {}).get('vnc_port', active_vnc_port),
            })
            return

        if self.path == '/api/qemu/stop':
            # End-to-end guest stop for the Studio's Start/Restart control.
            # stop_qemu_backend() only ever terminates the QEMU process this
            # server launched (never an arbitrary qemu-system-x86_64), and a
            # guest that is not running is a successful no-op rather than an
            # error, so the client can call this route unconditionally. A guest
            # that outlived this Studio (owned=False) is reported as NOT
            # stopped, because this route did not stop it.
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            if content_length > 0:
                self.rfile.read(content_length)
            proc = qemu_subproc
            # What this Studio can prove: the process it launched is alive.
            # A port that answers without a tracked process is somebody
            # else's listener, not the device.
            was_running = tracked_qemu_process_alive()
            unowned_monitor = False
            if was_running:
                still_running = not stop_tracked_qemu()
            else:
                # Nothing of ours is running. Clear the stale device state
                # instead of leaving the Studio believing a device is up, and
                # do NOT terminate a QEMU this Studio did not launch: it
                # cannot prove ownership of it.
                stop_qemu_backend()
                still_running = False
                unowned_monitor = _monitor_answers()
            if still_running:
                message = "QEMU did not exit after the stop request"
            elif was_running:
                message = f"QEMU stopped (PID {proc.pid})" if proc is not None else "QEMU stopped"
            elif unowned_monitor:
                message = (f"No QEMU launched by this Studio is running, but a QEMU instance this Studio "
                           f"does not own still holds the monitor port {MONITOR_PORT}; it was left alone.")
            else:
                message = "QEMU is not running"
            # A guest this Studio does not own was NOT stopped, so the stop did
            # not succeed. Reporting success would claim a device action that
            # did not happen; owned=False tells the client that the guest
            # outlived this Studio and has to be handled through its monitor.
            stopped_ok = not still_running and not unowned_monitor
            self._send_json(500 if still_running else (409 if unowned_monitor else 200), {
                'success': stopped_ok,
                'was_running': was_running,
                'stopped': bool(was_running and not still_running),
                # owned=False means the guest outlived this Studio process, so
                # nothing in it may be killed by this route.
                'owned': not unowned_monitor,
                'message': message,
                'device_state': qemu_device_state()['state'],
            })
            return

        if self.path in (
            '/api/telephony/dial', '/api/telephony/hangup',
            '/api/telephony/data/toggle', '/api/telephony/sms/send',
            '/api/telephony/at_cmd',
        ):
            # The guest registers no baseband backend at all
            # (kernel/src/telephony/mod.rs: "No physical baseband/SIM backend
            # registered; runtime state remains offline") and exposes no
            # serial command that could place a call. Answer with the real
            # capability state and the live guest telemetry so the UI reports
            # the truth instead of inventing a successful call. When a baseband
            # backend and its command surface ever exist, this dispatch is the
            # single place that must learn about them.
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            if content_length > 0:
                self.rfile.read(content_length)
            self._send_json(409, {
                'success': False,
                'error': 'Baseband offline: the guest registers no physical baseband/SIM backend, so this action cannot be performed.',
                'telephony': extract_guest_subsystem("telephony", "[TELEPHONY]"),
            })
            return

        if self.path in ('/api/qemu/reboot', '/api/qemu/reset'):
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
            except Exception:
                data = {}
            action = str(data.get('action', 'reboot')).strip().lower()
            if action in ('reboot', 'restart', 'reset', 'cold_reboot', 'relaunch') or self.path in ('/api/qemu/reboot', '/api/qemu/reset'):
                global launcher_has_booted, last_reboot_cursor
                launcher_has_booted = False
                with telemetry_lock:
                    last_reboot_cursor = len(serial_lines)
                _last_frame_cache['time'] = 0
                # capture_qemu_screen_frame only ever writes the portrait
                # keys; popping retired landscape/full keys left the stale
                # JPEG frame alive across reboots.
                _last_frame_cache.pop('portrait_jpeg', None)
                _last_frame_cache.pop('portrait', None)
                info = None
                if data.get('cold') or action in ('cold_reboot', 'relaunch'):
                    # A QEMU this Studio did not launch is never terminated
                    # here: only the process this Studio owns is stopped. A
                    # guest adopted from an earlier Studio instance is still
                    # this Studio's guest, so it is stopped too.
                    stop_tracked_qemu()
                    ok, resp, info = launch_qemu_profile(active_profile_key)
                    restart_method = 'backend_relaunch'
                else:
                    ok, resp = send_qemu_monitor_cmd('system_reset')
                    restart_method = 'monitor_system_reset'
                    if not ok:
                        stop_tracked_qemu()
                        ok, resp, info = launch_qemu_profile(active_profile_key)
                        restart_method = 'backend_relaunch'
                self.send_response(200 if ok else 500)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({
                    'success': ok,
                    'action': 'reboot',
                    'qemu_reset': ok and restart_method == 'monitor_system_reset',
                    'restart_method': restart_method if ok else 'failed',
                    'message': str(resp),
                    **({'vnc_display': (info or {}).get('vnc_display'),
                        'vnc_port': (info or {}).get('vnc_port')} if info else {})
                }).encode())
                return
            elif action in ('sleep', 'power', 'wake', 'toggle'):
                # A real guest ACPI power-off can terminate QEMU. A subsequent
                # physical power-button action starts a new guest; it must not be
                # reported as a successful serial command to a dead VM.
                monitor_state = query_qemu_monitor(force=True)
                # The device is running only when the tracked process is alive
                # and the monitor answers.
                vm_alive = bool(monitor_state.get('qemu_alive', False))
                guest_status = str(monitor_state.get('status', '')).strip().lower()
                if not vm_alive and action in ('power', 'wake', 'toggle'):
                    ok, msg, info = launch_qemu_profile(active_profile_key)
                    result = {'success': ok, 'action': 'power_on', 'qemu_started': ok, 'message': str(msg)}
                    if info:
                        result['vnc_display'] = info.get('vnc_display')
                        result['vnc_port'] = info.get('vnc_port')
                elif vm_alive and guest_status == 'shutdown' and action in ('power', 'wake', 'toggle'):
                    # The QEMU process/monitor can remain alive after guest ACPI shutdown.
                    # Serial input cannot wake a halted guest; reset it through QEMU's monitor.
                    ok, msg = send_qemu_monitor_cmd('system_reset')
                    restart_method = 'monitor_system_reset'
                    if not ok:
                        stop_tracked_qemu()
                        ok, msg, info = launch_qemu_profile(active_profile_key)
                        restart_method = 'backend_relaunch'
                    result = {'success': ok, 'action': 'power_on', 'restart_method': restart_method if ok else 'failed', 'message': str(msg)}
                elif vm_alive and action in ('power', 'wake', 'toggle'):
                    ok, msg = send_guest_serial_command('POWER_KEY', '[POWER]', timeout=3)
                    result = {'success': ok, 'action': action, 'message': msg}
                else:
                    ok, msg = send_guest_serial_command('POWER_KEY', '[POWER]', timeout=3)
                    result = {'success': ok, 'action': action, 'message': msg}
                self.send_response(200 if result['success'] else 503)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps(result).encode())
                return
            else:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': f"Unknown power action '{action}'"}).encode())
                return

        if self.path in ('/api/hardware/orientation', '/api/orientation'):
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
            except Exception:
                data = {}
            target_orient = str(data.get('orientation', '')).strip().lower()
            choices = {
                'landscape': ('ORIENTATION LANDSCAPE', '[DISPLAY_HAL] Orientation changed to LandscapeRight.'),
                'landscape_right': ('ORIENTATION LANDSCAPE_RIGHT', '[DISPLAY_HAL] Orientation changed to LandscapeRight.'),
                'landscaperight': ('ORIENTATION LANDSCAPERIGHT', '[DISPLAY_HAL] Orientation changed to LandscapeRight.'),
                'horizontal': ('ORIENTATION LANDSCAPE', '[DISPLAY_HAL] Orientation changed to LandscapeRight.'),
                'landscape_left': ('ORIENTATION LANDSCAPE_LEFT', '[DISPLAY_HAL] Orientation changed to LandscapeLeft.'),
                'landscapeleft': ('ORIENTATION LANDSCAPELEFT', '[DISPLAY_HAL] Orientation changed to LandscapeLeft.'),
                'portrait': ('ORIENTATION PORTRAIT', '[DISPLAY_HAL] Orientation changed to Portrait.'),
                'vertical': ('ORIENTATION PORTRAIT', '[DISPLAY_HAL] Orientation changed to Portrait.'),
            }
            command = choices.get(target_orient)
            if command is None:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Unsupported orientation'}).encode())
                return

            ok, guest_message = send_guest_serial_command(command[0], command[1], timeout=3)
            if not ok:
                self.send_response(503)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({
                    'success': False,
                    'status': 'UNCONFIRMED',
                    'error': 'Guest did not confirm orientation change',
                    'detail': guest_message,
                }).encode())
                return

            current_device_orientation = 'landscape' if target_orient.startswith('landscape') or target_orient in ('horizontal', 'landscaperight') else 'portrait'
            _last_frame_cache['time'] = 0
            # capture_qemu_screen_frame only ever writes the portrait keys;
            # popping retired landscape/full keys left the stale JPEG frame
            # alive across orientation changes.
            _last_frame_cache.pop('portrait_jpeg', None)
            _last_frame_cache.pop('portrait', None)

            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': True,
                'status': 'APPLIED',
                'orientation': current_device_orientation,
                'guest_confirmation': guest_message,
                'scanout': f'{guest_framebuffer_size[0]}x{guest_framebuffer_size[1]}'
            }).encode())
            return

        if self.path == '/api/hardware/sensor_inject':
            self.send_response(409)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': False,
                'status': 'UNAVAILABLE',
                'source': 'UNAVAILABLE',
                'error': 'Hardware sensor injection is disabled; Device Studio only displays guest-reported telemetry'
            }).encode())
            return

        if self.path == '/api/power/state':
            content_length = self._bounded_content_length()
            body = self.rfile.read(content_length) if (content_length and content_length > 0) else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
            except Exception:
                data = {}
            action = str(data.get('action', 'lock')).strip().lower()
            cmd = "LOCK_KEY" if action in ('lock', 'power', 'toggle') else "POWER_KEY"
            ok, msg = send_guest_serial_command(cmd, timeout=2)
            if ok:
                with telemetry_lock:
                    serial_lines.append(f"[STUDIO_POWER] {cmd} toggle dispatched")
            else:
                # The command never reached a guest. Reporting it as dispatched
                # would claim a device action that did not happen.
                with telemetry_lock:
                    serial_lines.append(f"[STUDIO_POWER] {cmd} not dispatched: {msg}")
            self.send_response(200 if ok else 503)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': ok,
                'status': 'DISPATCHED' if ok else 'NOT_DISPATCHED',
                'action': action,
                'detail': msg
            }).encode())
            return
        if self.path == '/api/touch/event':
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
                action = str(data.get('action', '')).strip().lower()
                space = str(data.get('coords', 'portrait')).strip().lower()
                # The guest accepts logical portrait coordinates, the same space
                # the compositor and apps use. Framebuffer coordinates are
                # converted here, in the one place that knows the studio's
                # letterbox geometry, rather than being round-tripped through the
                # kernel with a second copy of the constants.
                LOGICAL_W, LOGICAL_H = LOGICAL_SURFACE_W, LOGICAL_SURFACE_H
                if space in ('landscape', 'logical_landscape') or (space in ('guest', 'guest_framebuffer', 'qemu', 'qemu_framebuffer') and current_device_orientation == 'landscape'):
                    x = int(data.get('x', 0)); y = int(data.get('y', 0))
                    fb_w, fb_h = current_framebuffer_size()
                    if not (0 <= x < fb_w and 0 <= y < fb_h): raise ValueError('landscape coordinates out of bounds')
                    # KMS LandscapeRight maps (x, y) in scanout to mobile surface:
                    # src_x = (y * 719) / (fb_h - 1)
                    # src_y = 1439 - (x * 1439) / (fb_w - 1)
                    denom_h = max(1, fb_h - 1)
                    denom_w = max(1, fb_w - 1)
                    gx = max(0, min(LOGICAL_SURFACE_W - 1, (y * (LOGICAL_SURFACE_W - 1) + denom_h // 2) // denom_h))
                    gy = max(0, min(LOGICAL_SURFACE_H - 1, (LOGICAL_SURFACE_H - 1) - (x * (LOGICAL_SURFACE_H - 1) + denom_w // 2) // denom_w))
                    x2 = int(data.get('x2', x)); y2 = int(data.get('y2', y))
                    if not (0 <= x2 < fb_w and 0 <= y2 < fb_h): raise ValueError('landscape end coordinates out of bounds')
                    gx2 = max(0, min(LOGICAL_SURFACE_W - 1, (y2 * (LOGICAL_SURFACE_W - 1) + denom_h // 2) // denom_h))
                    gy2 = max(0, min(LOGICAL_SURFACE_H - 1, (LOGICAL_SURFACE_H - 1) - (x2 * (LOGICAL_SURFACE_H - 1) + denom_w // 2) // denom_w))
                elif space in ('portrait', 'logical_portrait', 'studio_display'):
                    x = int(data.get('x', 0)); y = int(data.get('y', 0))
                    if not (0 <= x < LOGICAL_W and 0 <= y < LOGICAL_H): raise ValueError('portrait coordinates out of bounds')
                    gx, gy = x, y
                    x2 = int(data.get('x2', x)); y2 = int(data.get('y2', y))
                    if not (0 <= x2 < LOGICAL_W and 0 <= y2 < LOGICAL_H): raise ValueError('portrait end coordinates out of bounds')
                    gx2, gy2 = x2, y2
                elif space in ('guest', 'guest_framebuffer', 'qemu', 'qemu_framebuffer'):
                    x = int(data.get('x', 0)); y = int(data.get('y', 0))
                    fb_w, fb_h = current_framebuffer_size()
                    if not (0 <= x < fb_w and 0 <= y < fb_h): raise ValueError('guest framebuffer coordinates out of bounds')
                    p = portrait_from_framebuffer(x, y)
                    if p is None: raise ValueError('coordinates outside letterboxed phone canvas')
                    gx, gy = p
                    x2 = int(data.get('x2', x)); y2 = int(data.get('y2', y))
                    if not (0 <= x2 < fb_w and 0 <= y2 < fb_h): raise ValueError('guest framebuffer coordinates out of bounds')
                    p2 = portrait_from_framebuffer(x2, y2)
                    if p2 is None: raise ValueError('coordinates outside letterboxed phone canvas')
                    gx2, gy2 = p2
                else:
                    raise ValueError('unknown coordinate space')

                slot = int(data.get('slot', 0))

                def _deliver(command: str) -> None:
                    """Sends one input command through the guest's live COM1 transport."""
                    ok, msg = send_guest_serial_command(command, timeout=2)
                    if not ok:
                        raise RuntimeError(f"could not deliver input to guest: {msg}")
                if action == 'down':
                    _deliver(f'INPUT_TOUCH DOWN {gx} {gy} {slot}')
                elif action == 'move':
                    _deliver(f'INPUT_TOUCH MOVE {gx} {gy} {slot}')
                elif action == 'up':
                    _deliver(f'INPUT_TOUCH UP {gx} {gy} {slot}')
                elif action == 'tap':
                    _deliver(f'TAP {gx} {gy}')
                elif action == 'cancel':
                    _deliver(f'INPUT_TOUCH CANCEL {gx} {gy} {slot}')
                elif action in ('swipe', 'drag'):
                    steps = max(2, min(20, int(data.get('steps', 5))))
                    _deliver(f'SWIPE {gx} {gy} {gx2} {gy2} {steps}')
                elif action in ('home', 'back', 'recents', 'settings', 'system'):
                    if action in ('settings', 'system'):
                        _deliver('RAPP_LAUNCH settings')
                    else:
                        _deliver(f'NAV {action.upper()}')
                elif action in ('power', 'lock'):
                    ok, msg = send_guest_serial_command('POWER_KEY', timeout=2)
                    if not ok: raise RuntimeError(msg)
                elif action == 'nav':
                    target = str(data.get('target', 'HOME')).upper()
                    _deliver(f'NAV {target}')
                else:
                    raise ValueError('unsupported touch action')
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': True, 'status': 'DISPATCHED', 'source': 'UART_COM1', 'action': action, 'coordinate_space': space, 'guest': {'x': gx, 'y': gy}}).encode())
            except (ValueError, TypeError, json.JSONDecodeError, RuntimeError) as exc:
                self.send_response(400 if isinstance(exc, (ValueError, TypeError, json.JSONDecodeError)) else 503)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': str(exc)}).encode())
            return

        if self.path == '/api/input/key':
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
                key = str(data.get('key', ''))
                navigation = {
                    'Enter', 'Escape', 'Backspace', 'Tab',
                    'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight',
                    'Home', 'End', 'PageUp', 'PageDown',
                    'Power', 'Lock'
                }
                if key in ('Power', 'Lock'):
                    command = "LOCK_KEY"
                    response_key = {'key': key, 'action': 'lock' if key == 'Lock' else 'power_toggle'}
                elif len(key) == 1:
                    command = f"INPUT_KEY {ord(key)}"
                    response_key = {'key': key, 'codepoint': ord(key)}
                elif key in navigation:
                    command = f"INPUT_KEYCODE {key}"
                    response_key = {'key': key, 'keycode': key}
                else:
                    raise ValueError('unsupported keyboard key')
            except (ValueError, TypeError, json.JSONDecodeError) as exc:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': str(exc)}).encode())
                return

            sent = False
            with serial_socket_lock:
                if serial_socket_conn:
                    try:
                        serial_socket_conn.sendall((command + "\n").encode('utf-8'))
                        sent = True
                    except Exception:
                        sent = False

            if not sent:
                self.send_response(503)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'status': 'UNAVAILABLE', 'source': 'UART_COM1'}).encode())
                return

            serial_lines.append(f"[STUDIO_INPUT] KEY {key!r}")
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({'success': True, 'status': 'DISPATCHED', 'source': 'UART_COM1', **response_key}).encode())
            return

        if self.path == '/api/serial/write':
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            body = self.rfile.read(content_length)
            try:
                data = json.loads(body.decode('utf-8'))
                cmd = str(data.get('command', '')).strip()
            except (json.JSONDecodeError, TypeError):
                cmd = ''

            if not cmd or len(cmd) > 256 or any(ord(ch) < 0x20 and ch not in '\t' for ch in cmd):
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'invalid serial command'}).encode())
                return

            sent = False
            with serial_socket_lock:
                if serial_socket_conn:
                    try:
                        serial_socket_conn.sendall((cmd + '\n').encode('utf-8'))
                        sent = True
                    except Exception:
                        sent = False

            if not sent:
                self.send_response(503)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({
                    'success': False,
                    'status': 'UNAVAILABLE',
                    'source': 'UART_COM1',
                    'error': 'QEMU COM1 serial input socket is offline or not connected'
                }).encode())
                return

            serial_lines.append(f"[STUDIO_COMMAND] {cmd}")
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({'success': True, 'status': 'DISPATCHED', 'source': 'UART_COM1', 'command': cmd}).encode())
            return

        if self.path == '/api/themes/apply':
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
                raw_id = str(data.get('theme_id') or data.get('id') or '').strip()
            except Exception:
                raw_id = ''

            if not raw_id:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Missing theme_id'}).encode())
                return

            theme_id = re.sub(r'[^a-zA-Z0-9_-]', '_', raw_id).lower()
            target_theme = os.path.join(THEMES_DIR, f"{theme_id}.theme")
            target_json = os.path.join(THEMES_DIR, f"{theme_id}.json")
            if not os.path.exists(target_theme) and not os.path.exists(target_json):
                self.send_response(404)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Theme not found'}).encode())
                return

            self.send_response(409)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': False,
                'status': 'DISABLED',
                'error': 'Theme switching is disabled; InterOS uses the canonical stock system theme'
            }).encode())
            return

        if self.path == '/api/themes/save':
            self.send_response(409)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': False,
                'status': 'DISABLED',
                'error': 'Custom themes are disabled; InterOS uses the canonical stock system theme'
            }).encode())
            return

        if self.path == '/api/themes/bootloader':
            self.send_response(409)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': False,
                'status': 'DISABLED',
                'error': 'Bootloader branding is immutable in Device Studio'
            }).encode())
            return

        if self.path == '/api/qemu/cmd':
            content_length = int(self.headers.get('Content-Length', 0))
            if content_length > 4096:
                if content_length > 0:
                    _ = self.rfile.read(min(content_length, 1024 * 1024))
                self.send_response(413)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Connection', 'close')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Payload too large'}).encode())
                return

            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8')) if body else {}
            except Exception:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Malformed JSON'}).encode())
                return

            if not isinstance(data, dict) or 'cmd' not in data:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Missing cmd field'}).encode())
                return

            raw_cmd = data.get('cmd')
            if not isinstance(raw_cmd, str):
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'cmd must be a string'}).encode())
                return

            raw_cmd = raw_cmd.strip()
            if len(raw_cmd) > 256:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'cmd exceeds maximum length (256 chars)'}).encode())
                return

            # Reject command chaining, newlines, semicolons, pipes, redirection
            if any(c in raw_cmd for c in ('\n', '\r', ';', '|', '&', '`', '$', '>', '<')):
                self.send_response(403)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Command injection or chaining detected.'}).encode())
                return

            # Structured allowlist validation: base operation and allowed subcommands/arguments
            tokens = raw_cmd.split()
            if not tokens:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Empty command'}).encode())
                return

            base_op = tokens[0].lower()
            ALLOWED_BASE_OPS = {'info', 'help', 'p', 'x'}
            if base_op not in ALLOWED_BASE_OPS:
                self.send_response(403)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': f'Command operation \'{base_op}\' restricted in Studio security sandbox.'}).encode())
                return

            # Validate subcommands and arguments for allowed operations
            if base_op == 'info':
                ALLOWED_INFO_SUBCMDS = {
                    'registers', 'cpus', 'mem', 'status', 'version', 'block',
                    'chardev', 'network', 'usb', 'pci', 'lapic', 'tlb',
                    'profile', 'mtree', 'jit', 'history'
                }
                if len(tokens) > 1 and tokens[1].lower() not in ALLOWED_INFO_SUBCMDS:
                    self.send_response(403)
                    self.send_header('Content-Type', 'application/json')
                    self.end_headers()
                    self.wfile.write(json.dumps({'success': False, 'error': f'Subcommand \'info {tokens[1]}\' restricted in Studio security sandbox.'}).encode())
                    return
            elif base_op == 'help':
                if len(tokens) > 2:
                    self.send_response(403)
                    self.send_header('Content-Type', 'application/json')
                    self.end_headers()
                    self.wfile.write(json.dumps({'success': False, 'error': 'Malformed help query.'}).encode())
                    return
            elif base_op in ('p', 'x'):
                if len(tokens) < 2:
                    self.send_response(400)
                    self.send_header('Content-Type', 'application/json')
                    self.end_headers()
                    self.wfile.write(json.dumps({'success': False, 'error': f'Missing argument for \'{base_op}\' operation'}).encode())
                    return

            ok, resp = send_qemu_monitor_cmd(raw_cmd)
            self.send_response(200 if ok else 500)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({'success': ok, 'output': resp}).encode())
            return

        if self.path == '/api/fs/read':
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
                path = str(data.get('path', '')).strip()
            except Exception:
                path = ''
            if not is_safe_guest_path(path):
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Invalid or unsafe guest path'}).encode())
                return
            ok, err, content = guest_fs_read(path)
            self.send_response(200 if ok else 500)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({'success': ok, 'source': 'GUEST_VFS', 'path': path, 'content': content, 'error': err}).encode())
            return

        if self.path == '/api/fs/write':
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
                path = str(data.get('path', '')).strip()
                content = str(data.get('content', ''))
            except Exception:
                path, content = '', ''
            if not is_safe_guest_path(path, writable=True):
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Invalid or unsafe guest write path'}).encode())
                return
            ok, err = guest_fs_write(path, content)
            self.send_response(200 if ok else 500)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({'success': ok, 'source': 'GUEST_VFS', 'path': path, 'error': err}).encode())
            return

        if self.path == '/api/themes/bootloader':
            self._drain_incoming_body_if_any()
            self.send_response(409)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': False,
                'status': 'DISABLED',
                'error': 'Bootloader branding is immutable in Device Studio'
            }).encode())
            return

        if self.path == '/api/permission/request':
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
                app_id = str(data.get('app_id', '')).strip()
                perm = str(data.get('permission', '')).strip()
                grant = bool(data.get('grant', True))
                if not app_id or not perm:
                    raise ValueError('Missing app_id or permission')
                runtime_permission_grants[(app_id, perm)] = grant
                serial_lines.append(f"[PERMISSION] {app_id} requested {perm} -> {'GRANTED' if grant else 'DENIED'}")
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': True, 'app_id': app_id, 'permission': perm, 'granted': grant}).encode())
            except Exception as e:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': str(e)}).encode())
            return

        if self.path == '/api/web/hmr/notify':
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
                app_id = str(data.get('app_id', 'org.interos.webapp')).strip()
                file_name = str(data.get('file', 'index.html')).strip()
                event_type = str(data.get('event', 'update')).strip()
                with web_hmr_cond:
                    new_id = (web_hmr_events[-1]['id'] + 1) if web_hmr_events else 1
                    evt = {
                        'id': new_id,
                        'app_id': app_id,
                        'file': file_name,
                        'event': event_type,
                        'time': time.time(),
                    }
                    web_hmr_events.append(evt)
                    if len(web_hmr_events) > 500:
                        del web_hmr_events[:100]
                    web_hmr_cond.notify_all()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': True, 'event': evt}).encode())
            except Exception as e:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': str(e)}).encode())
            return

        if self.path == '/api/web/import':
            content_length = self._bounded_content_length()
            if content_length is None:
                return
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
                src = str(data.get('folder') or data.get('url') or data.get('source') or '').strip()
                app_name = str(data.get('name') or '').strip()
                pkg_id = str(data.get('package_id') or '').strip()
                is_hmr = bool(data.get('hmr', True))

                if not src or src == 'template':
                    src = os.path.join(WORKSPACE_ROOT, 'templates', 'web_app_template')

                port_script = os.path.join(WORKSPACE_ROOT, 'tools', 'port_web.py')
                dist_dir = os.path.join(WORKSPACE_ROOT, 'dist')
                os.makedirs(dist_dir, exist_ok=True)

                clean_name = re.sub(r'[^a-zA-Z0-9_-]', '_', app_name or os.path.basename(src) or 'webapp')
                out_rapp = os.path.join(dist_dir, f"{clean_name}.rapp")

                cmd = [sys.executable, port_script, src, '--output', out_rapp]
                if app_name:
                    cmd.extend(['--name', app_name])
                if pkg_id:
                    cmd.extend(['--id', pkg_id])

                res = subprocess.run(cmd, capture_output=True, text=True, cwd=WORKSPACE_ROOT)
                if res.returncode != 0:
                    raise RuntimeError(f"port_web.py failed: {res.stderr or res.stdout}")

                if not os.path.exists(out_rapp):
                    raise RuntimeError(f"Output .rapp bundle not found at {out_rapp}")

                with open(out_rapp, 'rb') as f:
                    rapp_bytes = f.read()

                ok, msg, app_entry = verify_and_install_rapp(rapp_bytes, os.path.basename(out_rapp))
                if not ok:
                    raise RuntimeError(f"Verification of generated .rapp failed: {msg}")

                guest_ok = False
                if latest_telemetry.get("qemu_running"):
                    guest_ok, guest_msg = transfer_rapp_to_guest(rapp_bytes, app_entry["id"])
                    if guest_ok:
                        serial_lines.append(f"[WEB_IMPORT] Installed '{app_entry['name']}' ({app_entry['id']}) into guest.")

                if is_hmr:
                    with web_hmr_cond:
                        new_id = (web_hmr_events[-1]['id'] + 1) if web_hmr_events else 1
                        web_hmr_events.append({
                            'id': new_id,
                            'app_id': app_entry['id'],
                            'file': 'index.html',
                            'event': 'import',
                            'time': time.time(),
                        })
                        web_hmr_cond.notify_all()

                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({
                    'success': True,
                    'package_id': app_entry['id'],
                    'arch': app_entry.get('arch', 'web'),
                    'sideload_status': 'verified',
                    'package': app_entry,
                    'rapp_path': out_rapp,
                    'guest_installed': guest_ok,
                    'hmr_enabled': is_hmr,
                    'output': res.stdout
                }).encode())
            except Exception as e:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': str(e)}).encode())
            return

        self._drain_incoming_body_if_any()
        self.send_response(404)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(json.dumps({'success': False, 'error': 'Not found'}).encode())

    def do_OPTIONS(self):
        if not self._origin_allowed():
            self._send_json(403, {'success': False, 'error': 'Cross-origin request rejected'})
            return
        self.send_response(200)
        self.send_header('Access-Control-Allow-Methods', 'GET, POST, OPTIONS')
        self.send_header('Access-Control-Allow-Headers', 'Content-Type')
        self.end_headers()

    def do_HEAD(self):
        if not self._origin_allowed():
            self._send_json(403, {'success': False, 'error': 'Cross-origin request rejected'})
            return
        super().do_HEAD()

    def do_PUT(self):
        if not self._origin_allowed():
            self._drain_incoming_body_if_any()
            self._send_json(403, {'success': False, 'error': 'Cross-origin request rejected'})
            return
        self._send_method_not_allowed()

    def do_DELETE(self):
        if not self._origin_allowed():
            self._drain_incoming_body_if_any()
            self._send_json(403, {'success': False, 'error': 'Cross-origin request rejected'})
            return
        self._send_method_not_allowed()

    def do_PATCH(self):
        if not self._origin_allowed():
            self._drain_incoming_body_if_any()
            self._send_json(403, {'success': False, 'error': 'Cross-origin request rejected'})
            return
        self._send_method_not_allowed()


class ThreadedTCPServer(socketserver.ThreadingMixIn, socketserver.TCPServer):
    daemon_threads = True
    allow_reuse_address = True


if __name__ == '__main__':
    ok, msg = start_qemu_backend()
    print(f"[*] InterOS Studio Backend: {msg}")

    start_websockify_if_needed()

    def _serial_supervisor():
        """Nothing may kill the UART reader silently; restart it if it ever dies."""
        while True:
            try:
                serial_stream_worker()
            except BaseException as e:
                print(f"[serial] worker died ({e!r}); restarting in 1s")
                time.sleep(1)

    serial_thread = threading.Thread(target=_serial_supervisor, daemon=True,
                                     name="SerialStreamWorker")
    serial_thread.start()

    try:
        with ThreadedTCPServer(("127.0.0.1", PORT), StudioAPIHandler) as httpd:
            url = f"http://localhost:{PORT}"
            print("============================================================")
            print("  INTER OS — Device Studio Server Online")
            print(f"  URL: {url}")
            print(f"  Target: Rust Mobile App Development & Emulation")
            print("============================================================")
            httpd.serve_forever()
    except KeyboardInterrupt:
        # The guest this Studio launched (or adopted) is stopped with it, so
        # the next Studio session starts from a clean device state.
        stop_tracked_qemu()
        print("\nStopping studio server.")
        sys.exit(0)
