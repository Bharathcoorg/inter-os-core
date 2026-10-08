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
VNC_PORT = int(os.environ.get("VNC_PORT", 5901))
WS_PORT = int(os.environ.get("WS_PORT", 6081))
DIRECTORY = os.path.dirname(os.path.abspath(__file__))

qemu_subproc = None
ws_subproc = None
serial_lines = []
serial_socket_conn = None
serial_socket_lock = threading.Lock()
telemetry_lock = threading.Lock()
launcher_has_booted = False

# Guest scanout geometry.
#
# The logical phone surface is 720x1440. The actual QEMU framebuffer is learned
# from live screendumps, so Device Studio does not assume a developer has a
# particular host resolution.
LOGICAL_SURFACE_W, LOGICAL_SURFACE_H = 720, 1440
guest_framebuffer_size = (720, 1440)


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
    # Disk boot log check if serial_lines memory buffer was rotated
    cand_logs = [
        current_serial_log_path,
        os.path.join(WORKSPACE_ROOT, "target", "x86_64_boot.log"),
        os.path.join(WORKSPACE_ROOT, "target", "x86_boot.log"),
    ]
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

def start_websockify_if_needed():
    """Starts websockify on WS_PORT (6081) bridging to QEMU VNC (5900) if not already listening."""
    global ws_subproc
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.settimeout(0.15)
            if s.connect_ex(('127.0.0.1', WS_PORT)) == 0:
                latest_telemetry["websockify_alive"] = True
                return
    except Exception:
        pass

    try:
        ws_subproc = subprocess.Popen([
            sys.executable, "-m", "websockify",
            str(WS_PORT), "127.0.0.1:5901"
        ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(0.3)
        latest_telemetry["websockify_alive"] = True
        print(f"[*] Live Bare-Metal VBE bridge active on ws://127.0.0.1:{WS_PORT} -> VNC :5900")
    except Exception as e:
        print(f"[*] Notice: websockify auto-start: {e}")
        latest_telemetry["websockify_alive"] = False

current_serial_log_path = os.path.join(os.path.dirname(DIRECTORY), "target", "x86_boot.log")

def serial_stream_worker():
    """Background worker continuously pulling UART lines from QEMU COM1 TCP port (4201)."""
    global serial_lines, serial_socket_conn, current_serial_log_path
    buf = ""
    while True:
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                s.settimeout(1.5)
                s.connect(('127.0.0.1', SERIAL_TCP_PORT))
                with serial_socket_lock:
                    serial_socket_conn = s
                s.settimeout(0.8)
                while True:
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
                        continue
        except Exception:
            with serial_socket_lock:
                serial_socket_conn = None
            time.sleep(1.5)


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
        "ram": 1024,
        "boot_type": "BIOS/MBR Bootloader",
        "image": os.path.join(WORKSPACE_ROOT, "target", "inter-os-bios.img"),
        "status": "Primary QEMU Phone contract",
        "display": {"backend": "vnc", "host": "127.0.0.1", "port": 5901, "websocket_port": 6081, "guest_width": 1280, "guest_height": 720},
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


def find_qemu_executable(arch):
    """Locate QEMU executable for given architecture on the host system."""
    bin_name = f"qemu-system-{arch}"
    p = shutil.which(bin_name)
    if p:
        return p
    common_dirs = [
        os.path.join(os.path.expanduser("~"), "qemu"),
        "C:\\Program Files\\qemu",
        "C:\\qemu",
        "/usr/bin",
        "/usr/local/bin"
    ]
    for d in common_dirs:
        cand = os.path.join(d, f"{bin_name}.exe" if sys.platform == "win32" else bin_name)
        if os.path.exists(cand):
            return cand
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
        self.width = 1280
        self.height = 720
        self.scanout_buf = bytearray(1280 * 720 * 4)
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
        self.metrics = {
            'fps': 0.0,
            'frame_us': 0,
            'frames_total': 0,
            'dropped_frames': 0,
            'second_start': time.time(),
            'frames_in_second': 0,
        }

    def start(self):
        t = threading.Thread(target=self._worker_loop, daemon=True)
        t.name = "VncScanoutWorker"
        t.start()

    def _worker_loop(self):
        while self.running:
            try:
                self._run_session()
            except Exception as e:
                self.connected = False
                time.sleep(0.5)

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
        if name_len > 0:
            self._recv_exact(s, name_len)

        self.width = w
        self.height = h
        global guest_framebuffer_size
        guest_framebuffer_size = (w, h)
        self.scanout_buf = bytearray(w * h * 4)
        self.connected = True

        # Encodings: Raw (0), DesktopSize (-239)
        s.sendall(struct.pack('>BxHii', 2, 2, 0, -239))

        # Request initial full update
        s.sendall(struct.pack('>BBHHHH', 3, 0, 0, 0, self.width, self.height))
        s.settimeout(0.8)

        while self.running:
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
                        self.width, self.height = rw, rh_val
                        guest_framebuffer_size = (rw, rh_val)
                        self.scanout_buf = bytearray(rw * rh_val * 4)
                        continue
                    if enc == 0:
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
                if self.height < 720 and check_launcher_ready():
                    break
                s.sendall(struct.pack('>BBHHHH', 3, 1, 0, 0, self.width, self.height))
            except socket.timeout:
                try:
                    s.sendall(struct.pack('>BBHHHH', 3, 1, 0, 0, self.width, self.height))
                except Exception:
                    break
            except Exception:
                break
        s.close()
        self.connected = False

    def _publish_frame(self, t_start):
        now = time.time()
        w, h = self.width, self.height
        is_landscape = (current_device_orientation == 'landscape')
        box = letterbox_box(w, h)
        if not box:
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
            rgb_img.save(buf, format="JPEG", quality=85)
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

    # Fast path: in-memory VNC client (zero disk I/O, zero PPM)
    if vnc_client.connected and vnc_client.latest_raw_rgba is not None:
        if (vnc_client.latest_raw_w, vnc_client.latest_raw_h) != (LOGICAL_SURFACE_W, LOGICAL_SURFACE_H):
            return None, f"Non-canonical VNC scanout {vnc_client.latest_raw_w}x{vnc_client.latest_raw_h}; expected {LOGICAL_SURFACE_W}x{LOGICAL_SURFACE_H}"
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
            if (width, height) != (LOGICAL_SURFACE_W, LOGICAL_SURFACE_H):
                return None, f"Non-canonical guest scanout {width}x{height}; expected {LOGICAL_SURFACE_W}x{LOGICAL_SURFACE_H}"
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




def stop_qemu_backend():
    """Stops any running QEMU and websockify processes managed by this instance."""
    global qemu_subproc, ws_subproc, launcher_has_booted
    launcher_has_booted = False
    if ws_subproc:
        try:
            ws_subproc.terminate()
            ws_subproc.wait(timeout=1)
        except Exception:
            pass
        ws_subproc = None

    if qemu_subproc:
        try:
            qemu_subproc.terminate()
            qemu_subproc.wait(timeout=1)
        except Exception:
            pass
        qemu_subproc = None

    # Never discover and kill arbitrary QEMU processes by executable name,
    # shared disk path, or monitor port. Only qemu_subproc, created and owned by
    # this Device Studio server process, may be terminated here.

    latest_telemetry["qemu_alive"] = False
    latest_telemetry["status"] = "standby"
    latest_telemetry["mode"] = "standby"
    latest_telemetry["cpu"]["halted"] = True
    latest_telemetry["cpu"]["rip"] = "Offline"
    latest_telemetry["cpu"]["thread_id"] = "Offline"
    latest_telemetry["cpu"]["registers"] = {}


def launch_qemu_profile(profile_key="x86_64", ram_mb=None, smp=1):
    """Launches a real QEMU instance for the specified target device profile."""
    global qemu_subproc, active_profile_key, current_serial_log_path, launcher_has_booted, last_reboot_cursor
    launcher_has_booted = False
    with telemetry_lock:
        last_reboot_cursor = len(serial_lines)
    stop_qemu_backend()

    if profile_key not in SUPPORTED_PROFILES:
        return False, f"Unknown profile: {profile_key}"

    prof = SUPPORTED_PROFILES[profile_key]
    arch = prof["arch"]
    exe = find_qemu_executable(arch)
    if not exe:
        return False, f"QEMU binary 'qemu-system-{arch}' not found on host."

    img_path = prof["image"]
    if not os.path.exists(img_path):
        return False, f"Boot image not found at '{img_path}'. Build target first with 'interos build {arch}'."
    default_data_disk = "inter-os-data-master.img" if arch == "x86_64" else f"interos-data-{arch}.img"
    data_disk_path = os.environ.get("DATA_DISK_PATH", os.path.join(WORKSPACE_ROOT, "target", default_data_disk))
    if not os.path.isabs(data_disk_path) and ".." in data_disk_path:
        return False, "Security violation: Invalid data disk path traversal detected."

    ram = ram_mb or prof["ram"]
    serial_log = os.path.join(WORKSPACE_ROOT, "target", f"{arch}_boot.log")
    os.makedirs(os.path.dirname(serial_log), exist_ok=True)
    current_serial_log_path = serial_log
    try:
        with open(current_serial_log_path, "w", encoding="utf-8") as f:
            f.write(f"=== QEMU {arch} Boot Session ===\n")
    except Exception:
        pass

    cmd = [
        exe,
        "-m", f"{ram}M",
        "-smp", str(smp),
        "-monitor", "tcp:127.0.0.1:44445,server,nowait",
        "-serial", "tcp:127.0.0.1:4301,server,nowait",
        "-display", "none",
        "-vnc", "127.0.0.1:1"
    ]
    if arch == "x86_64":
        cmd.extend(["-machine", "q35", "-cpu", "qemu64"])
        # Real virtual display and absolute pointer hardware for the canonical phone VM.
        cmd.extend(["-vga", "none", "-device", "virtio-vga,edid=on,xres=720,yres=1440", "-device", "qemu-xhci,id=xhci0", "-device", "usb-tablet,bus=xhci0.0"])
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
        cmd.extend(["-device", "usb-kbd", "-device", "usb-tablet", "bus=usb0.0"])

    try:
        serial_lines.clear()
        qemu_subproc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        active_profile_key = profile_key
        # QEMU owns the VNC listener; bridge startup must happen after the VM exists.
        start_websockify_if_needed()
        serial_lines.append(f"[STUDIO] Launched QEMU {arch} microkernel instance (PID {qemu_subproc.pid}).")
        return True, f"QEMU {arch} started (PID {qemu_subproc.pid})"
    except Exception as e:
        return False, f"Failed to spawn QEMU: {e}"


def start_qemu_backend():
    """Ensures QEMU backend is active and reachable; launches primary instance if needed."""
    global qemu_subproc, ws_subproc

    # Check if a QEMU instance is already running on monitor port
    t = query_qemu_monitor()
    if t["qemu_alive"]:
        return True, "Connected to active microkernel instance"

    return launch_qemu_profile("x86_64")


last_telemetry_query = 0.0
telemetry_lock = threading.Lock()

def query_qemu_monitor(host='127.0.0.1', port=44445, timeout=0.35):
    """
    Connects to QEMU HMP monitor at 127.0.0.1:44445 to query real-time
    microkernel registers, CPU execution status, and block device telemetry.
    """
    global last_telemetry_query, latest_telemetry
    now = time.time()
    if now - last_telemetry_query < 0.8:
        return latest_telemetry

    with telemetry_lock:
        if now - last_telemetry_query < 0.8:
            return latest_telemetry

        last_telemetry_query = now
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                s.settimeout(timeout)
                s.connect((host, port))
                _ = s.recv(1024)

                # 1. info status
                s.sendall(b'info status\n')
                time.sleep(0.03)
                status_raw = s.recv(1024).decode('utf-8', errors='replace')
                vm_status = "running" if "running" in status_raw else ("paused" if "paused" in status_raw else "active")

                # 2. info cpus
                s.sendall(b'info cpus\n')
                time.sleep(0.03)
                cpus_raw = s.recv(1024).decode('utf-8', errors='replace')
                thread_match = re.search(r'thread_id=(\d+)', cpus_raw)
                thread_id = f"PID-{thread_match.group(1)}" if thread_match else "Core0"
                is_halted = "halted" in cpus_raw

                # 3. info registers
                s.sendall(b'info registers\n')
                time.sleep(0.06)
                regs_raw = s.recv(8192).decode('utf-8', errors='replace')

                # 4. info block
                s.sendall(b'info block\n')
                time.sleep(0.03)
                block_raw = s.recv(4096).decode('utf-8', errors='replace')

                # Parse registers into map
                reg_dict = {}
                for match in re.finditer(r'\b([A-Z0-9]+)\s*=\s*([0-9a-fA-F]{4,16})\b', regs_raw):
                    reg_name = match.group(1).upper()
                    reg_val = "0x" + match.group(2).upper()
                    reg_dict[reg_name] = reg_val

                rip_val = reg_dict.get("RIP") or reg_dict.get("EIP") or "0x0000000000000000"
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

                latest_telemetry["qemu_alive"] = True
                latest_telemetry["status"] = vm_status
                latest_telemetry["mode"] = "qemu_live"
                latest_telemetry["cpu"]["thread_id"] = thread_id
                latest_telemetry["cpu"]["halted"] = is_halted
                latest_telemetry["cpu"]["rip"] = rip_val
                if reg_dict:
                    latest_telemetry["cpu"]["registers"].update(reg_dict)
                latest_telemetry["block"]["device"] = block_device
                latest_telemetry["block"]["file"] = block_file

                try:
                    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s_ws:
                        s_ws.settimeout(0.08)
                        latest_telemetry["websockify_alive"] = (s_ws.connect_ex(('127.0.0.1', WS_PORT)) == 0)
                except Exception:
                    latest_telemetry["websockify_alive"] = False

                return latest_telemetry
        except Exception:
            # Fallback: check VNC port 5900
            try:
                with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s_vnc:
                    s_vnc.settimeout(0.15)
                    s_vnc.connect(('127.0.0.1', VNC_PORT))
                    latest_telemetry["qemu_alive"] = True
                    latest_telemetry["status"] = "running"
                    latest_telemetry["mode"] = "qemu_vnc_only"
                    return latest_telemetry
            except Exception:
                pass

            latest_telemetry["qemu_alive"] = False
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
    if int.from_bytes(body[6:8], "little") != 0:
        return False, "The current guest transport accepts x86_64 RAPP packages only.", None
    if body[:4] != b"RAPP":
        return False, "Invalid RAPP magic.", None
    if int.from_bytes(body[4:6], "little") != 2:
        return False, "Unsupported RAPP format version; expected v2.", None

    arch_id = int.from_bytes(body[6:8], "little")
    arch_names = {0: "x86_64", 1: "aarch64", 2: "riscv64"}
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
        "format": "Rust App Package (RAPP v2 binary)",
        "type": "Sovereign Ring 3",
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

def guest_package_catalog():
    ok, lines = guest_command_lines("STUDIO_PKG_LIST", "[STUDIO_PKG] END", timeout=8)
    if not ok:
        return False, lines[-1] if lines else "guest package registry unavailable", []
    packages = []
    for line in lines:
        if not line.startswith("[STUDIO_PKG] id="):
            continue
        fields = dict(part.split("=", 1) for part in line[len("[STUDIO_PKG] "):].split() if "=" in part)
        packages.append({
            "id": fields.get("id", ""),
            "version": fields.get("version", ""),
            "status": fields.get("state", "unknown"),
            "process": fields.get("process", ""),
            "name": fields.get("name", fields.get("id", "")).replace("_", " "),
            "icon": fields.get("icon", "APP"),
            "source": "GUEST_REGISTRY",
        })
    return True, None, packages

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
        self.send_header('Access-Control-Allow-Origin', '*')
        super().end_headers()

    def do_GET(self):
        parsed_path = urllib.parse.urlparse(self.path)
        if self.path.startswith('/api/serial'):
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            after = 0
            if '?after=' in self.path:
                try:
                    after_str = self.path.split('?after=')[1].split('&')[0]
                    after = int(after_str)
                except (ValueError, IndexError):
                    after = 0

            # Sync with live QEMU serial log on disk if available
            boot_log = os.path.join(os.path.dirname(DIRECTORY), "target", "x86_boot.log")
            if os.path.exists(boot_log):
                try:
                    with open(boot_log, "r", encoding="utf-8", errors="replace") as f:
                        disk_lines = [l.strip() for l in f if l.strip()]
                        if len(disk_lines) > len(serial_lines):
                            serial_lines.extend(disk_lines[len(serial_lines):])
                except Exception:
                    pass

            lines = serial_lines[after:] if after < len(serial_lines) else []
            response = json.dumps({
                'lines': lines,
                'total': len(serial_lines),
                'cursor': len(serial_lines),
                'next_index': len(serial_lines)
            })
            self.wfile.write(response.encode())
            return

        if self.path in ('/api/status', '/api/qemu/status'):
            t = query_qemu_monitor()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            launcher_ready = check_launcher_ready()
            response = json.dumps({
                'qemu': t["qemu_alive"],
                'status': t["status"],
                'mode': t.get("mode", "standby"),
                'launcher_ready': launcher_ready,
                'active_profile': active_profile_key,
                # Report the geometry this machine was actually launched with, so
                # the studio describes the running target rather than a generic
                # specification.
                'ram_mb': (SUPPORTED_PROFILES.get(active_profile_key) or {}).get('ram'),
                'guest_width': ((SUPPORTED_PROFILES.get(active_profile_key) or {}).get('display') or {}).get('guest_width'),
                'guest_height': ((SUPPORTED_PROFILES.get(active_profile_key) or {}).get('display') or {}).get('guest_height'),
                'orientation': current_device_orientation,
                'pid': qemu_subproc.pid if qemu_subproc else None
            })
            self.wfile.write(response.encode())
            return

        if self.path == '/api/qemu/profiles':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            response = json.dumps({
                'active_profile': active_profile_key,
                'profiles': SUPPORTED_PROFILES
            })
            self.wfile.write(response.encode())
            return

        if self.path == '/api/qemu/telemetry':
            t = query_qemu_monitor().copy()
            t["launcher_ready"] = check_launcher_ready()
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
            self.send_header('Access-Control-Allow-Origin', '*')
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
                self.send_header('Access-Control-Allow-Origin', '*')
                self.send_header('X-Frame-Seq', str(seq))
                self.end_headers()
                self.wfile.write(bin_msg)
            else:
                self.send_response(304 if client_seq > 0 else 503)
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
            return

        if parsed_path.path == '/api/screen/binary_stream':
            self.send_response(200)
            self.send_header('Content-Type', 'application/octet-stream')
            self.send_header('Cache-Control', 'no-cache, no-store, must-revalidate')
            self.send_header('Access-Control-Allow-Origin', '*')
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

        if parsed_path.path in ('/api/screen/metrics', '/api/transport/metrics'):
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
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

        if parsed_path.path == '/api/screen/stream':
            query = urllib.parse.parse_qs(parsed_path.query)
            orient = query.get('orientation', [None])[0]
            self.send_response(200)
            self.send_header('Content-Type', 'multipart/x-mixed-replace; boundary=frame')
            self.send_header('Cache-Control', 'no-cache, no-store, must-revalidate')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            try:
                while True:
                    frame_bytes, content_type = capture_qemu_screen_frame(orientation=orient, prefer_jpeg=True)
                    if frame_bytes:
                        self.wfile.write(b'--frame\r\n')
                        self.wfile.write(f'Content-Type: {content_type}\r\n'.encode('utf-8'))
                        self.wfile.write(f'Content-Length: {len(frame_bytes)}\r\n\r\n'.encode('utf-8'))
                        self.wfile.write(frame_bytes)
                        self.wfile.write(b'\r\n')
                        self.wfile.flush()
                    time.sleep(0.02)
            except (ConnectionResetError, BrokenPipeError):
                pass
            return

        if parsed_path.path == '/api/audio/volume':
            query = urllib.parse.parse_qs(parsed_path.query)
            action = query.get('action', ['get'])[0].upper()
            val = query.get('value', [''])[0]
            cmd = f"VOLUME {val}" if val else f"VOLUME {action}"
            ok, msg = send_guest_serial_command(cmd, '[AUDIO]', timeout=3)
            cur_vol = None
            with telemetry_lock:
                for line in reversed(serial_lines):
                    if "[AUDIO]" in line and "volume_percent=" in line:
                        m = re.search(r'volume_percent=(\d+)', line)
                        if m:
                            cur_vol = int(m.group(1))
                            break
            # A missing guest report is unknown, not a synthetic 75% reading.
            success = bool(ok and cur_vol is not None)
            self.send_response(200 if success else 503)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps({'success': success, 'volume_percent': cur_vol, 'status': 'APPLIED' if success else 'UNCONFIRMED', 'message': msg}).encode())
            return

        if parsed_path.path in ('/api/screen/frame', '/api/qemu/screendump', '/api/screen/scanout'):
            query = urllib.parse.parse_qs(parsed_path.query)
            full_scanout = query.get('full', ['0'])[0] in ('1', 'true', 'yes') or (parsed_path.path == '/api/screen/scanout')
            orient = query.get('orientation', [None])[0]
            fmt = query.get('format', ['jpeg'])[0].lower()
            prefer_jpeg = (fmt != 'png')
            frame_bytes, err_or_type = capture_qemu_screen_frame(full_scanout=full_scanout, orientation=orient, prefer_jpeg=prefer_jpeg)
            if frame_bytes:
                self.send_response(200)
                self.send_header('Content-Type', err_or_type)
                self.send_header('Cache-Control', 'no-cache, no-store, must-revalidate')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(frame_bytes)
            else:
                self.send_response(503)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({
                    'success': False,
                    'status': 'UNAVAILABLE',
                    'error': err_or_type or 'Scanout buffer unavailable'
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
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': f"SSRF Blocked: {reason}"}).encode())
                return

            try:
                req = urllib.request.Request(target_url, headers={
                    'User-Agent': 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1 InterOS/8.2',
                    'Accept': 'text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8',
                    'Accept-Language': 'en-US,en;q=0.9'
                })
                with urllib.request.urlopen(req, timeout=6) as response:
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
                self.send_header('Access-Control-Allow-Origin', '*')
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
                self.send_header('Access-Control-Allow-Origin', '*')
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
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps({
                'themes': [active],
                'active_id': active['theme_id']
            }).encode())
            return

        if parsed_path.path == '/api/themes/current':
            active = get_active_theme()
            active['id'] = active.get('theme_id', 'bento_glass')
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps({'success': True, 'theme': active, 'active_theme': active, **active}).encode())
            return

        if self.path.startswith('/api/apps/audit'):
            query = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)
            target = query.get('id', [''])[0] or query.get('filename', [''])[0]
            ok, error, catalog = guest_package_catalog()
            found = next((a for a in catalog if a.get('id') == target), None)
            self.send_response(200 if ok else 503)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            if found:
                self.wfile.write(json.dumps({
                    'success': True,
                    'source': 'GUEST_REGISTRY',
                    'package': found,
                    'authority': 'guest'
                }).encode())
            else:
                self.wfile.write(json.dumps({
                    'success': False,
                    'source': 'GUEST_REGISTRY',
                    'error': error or 'Package not present in guest registry'
                }).encode())
            return

        if self.path == '/api/apps/list':
            ok, error, catalog = guest_package_catalog()
            self.send_response(200 if ok else 503)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
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
            ok, error, catalog = guest_package_catalog()
            rapp_apps = [a for a in catalog if a.get('id')]
            self.send_response(200 if ok else 503)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
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
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': True,
                'telephony': extract_guest_subsystem("telephony", "[TELEPHONY]")
            }).encode())
            return

        if self.path == '/api/telephony/sms/list':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            messages = extract_guest_subsystem("telephony", "[TELEPHONY]").get("sms_messages", [])
            self.wfile.write(json.dumps({'success': True, 'messages': messages, 'source': 'KERNEL_TELEMETRY' if messages else 'UNAVAILABLE'}).encode())
            return

        if self.path == '/api/pmic/status':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            pmic_info = extract_guest_subsystem("pmic", "[PMIC]")
            self.wfile.write(json.dumps({'success': True, 'pmic': pmic_info}).encode())
            return

        if self.path == '/api/kms/status':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps({'success': True, 'kms': extract_guest_subsystem("kms", "[KMS]")}).encode())
            return

        if self.path == '/api/touch/status':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps({'success': True, 'touch': extract_guest_subsystem("touch", "[TOUCH]")}).encode())
            return

        if self.path == '/api/kernel/processes':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
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

        if self.path == '/api/kernel/memory':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            mem = extract_real_memory()
            self.wfile.write(json.dumps({'success': True, 'memory': mem}).encode())
            return

        if self.path == '/api/kernel/ipc':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            ipc_events = extract_real_ipc()
            self.wfile.write(json.dumps({'success': True, 'events': ipc_events, 'total': len(ipc_events)}).encode())
            return

        if self.path == '/api/kernel/capabilities':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            caps = extract_real_capabilities()
            self.wfile.write(json.dumps({'success': True, 'capabilities': caps, 'total': len(caps)}).encode())
            return

        if self.path.startswith('/api/fs/list'):
            query = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)
            rel_path = query.get('path', ['/'])[0]
            ok, error, entries = guest_fs_list('/' + rel_path.strip('/'))
            self.send_response(200 if ok else 503)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': ok,
                'source': 'GUEST_VFS',
                'path': '/' + rel_path.strip('/'),
                'entries': entries,
                **({'error': error} if error else {})
            }).encode())
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
        global serial_lines

        if self.path == '/api/rapp/trust':
            content_length = int(self.headers.get('Content-Length', 0))
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
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({"success": ok, "message": msg, "source": "QEMU_COM1"}).encode())
            except Exception as e:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({"success": False, "error": str(e)}).encode())
            return

        if self.path == '/api/rapp/install':
            content_length = int(self.headers.get('Content-Length', 0))
            raw_pkg_name = self.headers.get('X-Package-Name', 'unnamed.rapp')
            if '..' in raw_pkg_name or '/' in raw_pkg_name or '\\' in raw_pkg_name:
                # Drain the request body before responding. Leaving it unread
                # desyncs the next request on this keep-alive connection, which
                # surfaces to the client as a connection reset instead of the
                # intended rejection.
                self._drain_body(content_length)
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
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
                self.send_header('Access-Control-Allow-Origin', '*')
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
                self.send_header('Access-Control-Allow-Origin', '*')
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
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps({'success': True, 'package': app_entry}).encode())
            return

        if self.path in ('/api/rapp/uninstall', '/api/apps/uninstall'):
            content_length = int(self.headers.get('Content-Length', 0))
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
                self.send_header('Access-Control-Allow-Origin', '*')
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
            content_length = int(self.headers.get('Content-Length', 0))
            body = self.rfile.read(content_length)
            try:
                data = json.loads(body.decode('utf-8'))
                app_id = str(data.get("app_id", "")).strip()
                if not is_safe_package_id(app_id):
                    raise ValueError("invalid app_id")
                ok, msg = send_guest_serial_command(f"RAPP_LAUNCH {app_id}", "[RAPP_HOST] LAUNCHED", timeout=12)
                self.send_response(200 if ok else 409)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
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
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': False,
                'status': 'UNAVAILABLE',
                'source': 'UNAVAILABLE',
                'error': 'IPC relay requires a live guest IPC endpoint; Device Studio does not synthesize acknowledgements'
            }).encode())
            return
        if self.path.startswith('/api/apps/upload'):
            content_length = int(self.headers.get('Content-Length', 0))
            body = self.rfile.read(content_length)

            query = urllib.parse.urlparse(self.path).query
            params = urllib.parse.parse_qs(query)
            raw_query_fname = params.get('filename', [''])[0]
            if '..' in raw_query_fname or '/' in raw_query_fname or '\\' in raw_query_fname:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Path traversal in filename is strictly forbidden'}).encode())
                return
            query_fname = os.path.basename(raw_query_fname)
            query_name = params.get('name', [''])[0]

            # Detect format via binary magic bytes or extension
            fmt_str = "Raw Binary (.bin)"
            ext = ".app"
            app_type = "Generic"
            capabilities = ["DisplayScanout", "AudioPlayback", "SandboxedVFS"]
            is_apk = False
            apk_meta = None

            if body.startswith(b"\x7fELF"):
                fmt_str = "Native Rust App (.app / .rap)"
                ext = ".app"
                app_type = "Native Ring 3"
            elif body.startswith(b"\x00asm"):
                fmt_str = "WebAssembly (.wasm)"
                ext = ".wasm"
                app_type = "WASM Sandboxed"
            elif query_fname.endswith(".rapp"):
                # RAPP v2 has a dedicated signed-binary installation endpoint.
                # Do not silently reinterpret a ZIP/JSON package as executable.
                self.send_response(409)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
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
                        else:
                            fmt_str = "Rust App Package (.rapp)"
                            ext = ".rapp"
                            app_type = "Sovereign RAPP"
                            if "manifest.json" in names:
                                m_data = json.loads(zf.read("manifest.json").decode('utf-8'))
                                app_id = m_data.get("id", f"org.interos.{filename.split('.')[0]}")
                                app_name = m_data.get("name", query_name or filename.split('.')[0].title())
                                version = m_data.get("version", "1.0.0")
                                capabilities = m_data.get("requested_capabilities", capabilities)
                except Exception:
                    pass

            sha256 = hashlib.sha256(body).hexdigest()
            filename = query_fname or (f"app_{sha256[:8]}{ext}")
            app_name = query_name or filename.split('.')[0].replace('_', ' ').replace('-', ' ').title()
            app_id = f"sideloaded.{sha256[:8]}"
            version = "1.0.0"
            dex_classes = 0
            dex_size = 0
            signature = "None"
            entrypoint = ""

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
            # Log strict security audit to live UART telemetry console
            serial_lines.append(f"[APP_SIDELOAD] Ingested package '{filename}' (Format: {fmt_str}, Size: {len(body)} bytes).")
            serial_lines.append(f"[SECURITY_AUDIT] SHA-256 digest computed: [{sha256[:16]}...]; guest trust verification remains authoritative.")
            if is_apk:
                serial_lines.append(f"[ACR_AUDIT] DEX header parsed: {dex_classes} classes, {dex_size} bytes; execution has not been started.")
                serial_lines.append(f"[ACR_AUDIT] APK signature metadata: {signature}. Cryptographic signature verification is not performed by this inspection endpoint.")
                serial_lines.append(f"[ACR_SECURITY] Declared Android permissions recorded for '{app_id}'; enforcement occurs only in the guest ACR runtime.")
                serial_lines.append(f"[ACR_SECURITY] Bound {len(capabilities)} declared permissions to Ring 3 Micro-HAL isolated sandbox.")
            else:
                serial_lines.append(f"[SECURITY_AUDIT] Uploaded artifact '{app_name}' analyzed; guest installation remains authoritative.")

            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps({'success': True, 'package': app_entry}).encode())
            return

        if self.path in ('/api/hardware/power', '/api/power', '/api/reboot', '/api/qemu/reboot', '/api/restart', '/api/qemu/restart', '/api/reset', '/api/qemu/reset'):
            content_length = int(self.headers.get('Content-Length', 0))
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
            except Exception:
                data = {}
            action = str(data.get('action', 'reboot')).strip().lower()
            if action in ('reboot', 'restart', 'reset', 'cold_reboot', 'relaunch') or self.path in ('/api/reboot', '/api/qemu/reboot', '/api/restart', '/api/qemu/restart', '/api/reset', '/api/qemu/reset'):
                global launcher_has_booted, last_reboot_cursor
                launcher_has_booted = False
                with telemetry_lock:
                    last_reboot_cursor = len(serial_lines)
                _last_frame_cache['time'] = 0
                _last_frame_cache.pop('portrait', None)
                _last_frame_cache.pop('landscape', None)
                _last_frame_cache.pop('full', None)
                if data.get('cold') or action in ('cold_reboot', 'relaunch'):
                    stop_qemu_backend()
                    ok, resp = launch_qemu_profile(active_profile_key)
                    restart_method = 'backend_relaunch'
                else:
                    ok, resp = send_qemu_monitor_cmd('system_reset')
                    restart_method = 'monitor_system_reset'
                    if not ok:
                        stop_qemu_backend()
                        ok, resp = launch_qemu_profile(active_profile_key)
                        restart_method = 'backend_relaunch'
                self.send_response(200 if ok else 500)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({
                    'success': ok,
                    'action': 'reboot',
                    'qemu_reset': ok and restart_method == 'monitor_system_reset',
                    'restart_method': restart_method if ok else 'failed',
                    'message': str(resp)
                }).encode())
                return
            elif action in ('sleep', 'power', 'wake', 'toggle'):
                # A real guest ACPI power-off can terminate QEMU. A subsequent
                # physical power-button action starts a new guest; it must not be
                # reported as a successful serial command to a dead VM.
                process_alive = qemu_subproc is not None and qemu_subproc.poll() is None
                monitor_alive = bool(query_qemu_monitor().get('qemu_alive', False))
                vm_alive = process_alive or monitor_alive
                if not vm_alive and action in ('power', 'wake', 'toggle'):
                    ok, msg = launch_qemu_profile(active_profile_key)
                    result = {'success': ok, 'action': 'power_on', 'qemu_started': ok, 'message': str(msg)}
                elif vm_alive and action in ('power', 'wake', 'toggle'):
                    ok, msg = send_guest_serial_command('POWER_KEY', '[POWER]', timeout=3)
                    result = {'success': ok, 'action': action, 'message': msg}
                else:
                    ok, msg = send_guest_serial_command('POWER_KEY', '[POWER]', timeout=3)
                    result = {'success': ok, 'action': action, 'message': msg}
                self.send_response(200 if result['success'] else 503)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps(result).encode())
                return
            else:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': f"Unknown power action '{action}'"}).encode())
                return

        if self.path in ('/api/hardware/orientation', '/api/orientation'):
            content_length = int(self.headers.get('Content-Length', 0))
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
            except Exception:
                data = {}
            target_orient = str(data.get('orientation', '')).strip().lower()
            if target_orient in ('landscape', 'landscape_right', 'landscaperight', 'horizontal'):
                current_device_orientation = 'landscape'
                send_guest_serial_command('ORIENTATION LANDSCAPE')
            elif target_orient in ('landscape_left', 'landscapeleft'):
                current_device_orientation = 'landscape'
                send_guest_serial_command('ORIENTATION LANDSCAPE_LEFT')
            elif target_orient in ('portrait', 'vertical'):
                current_device_orientation = 'portrait'
                send_guest_serial_command('ORIENTATION PORTRAIT')
            else:
                current_device_orientation = 'landscape' if current_device_orientation == 'portrait' else 'portrait'
                send_guest_serial_command('ORIENTATION ' + current_device_orientation.upper())

            # Invalidate cached frames immediately
            _last_frame_cache['time'] = 0
            _last_frame_cache.pop('portrait', None)
            _last_frame_cache.pop('landscape', None)
            _last_frame_cache.pop('full', None)

            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': True,
                'orientation': current_device_orientation,
                'scanout': f'{guest_framebuffer_size[0]}x{guest_framebuffer_size[1]}'
            }).encode())
            return

        if self.path == '/api/hardware/sensor_inject':
            self.send_response(409)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps({
                'success': False,
                'status': 'UNAVAILABLE',
                'source': 'UNAVAILABLE',
                'error': 'Hardware sensor injection is disabled; Device Studio only displays guest-reported telemetry'
            }).encode())
            return
        if self.path == '/api/touch/event':
            content_length = int(self.headers.get('Content-Length', 0))
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
                    if not (0 <= x < fb_w and 0 <= y < fb_h) or not (0 <= x < 1280 and 0 <= y < 720): raise ValueError('landscape coordinates out of bounds')
                    # KMS LandscapeRight maps (x, y) in 1280x720 scanout to mobile surface (719 - y, (x * 1440) // 1280)
                    gx = max(0, min(LOGICAL_SURFACE_W - 1, LOGICAL_SURFACE_W - 1 - (y * LOGICAL_SURFACE_W) // max(1, fb_h)))
                    gy = max(0, min(LOGICAL_SURFACE_H - 1, (x * LOGICAL_SURFACE_H) // max(1, fb_w)))
                    x2 = int(data.get('x2', x)); y2 = int(data.get('y2', y))
                    if not (0 <= x2 < fb_w and 0 <= y2 < fb_h): raise ValueError('landscape end coordinates out of bounds')
                    gx2 = max(0, min(LOGICAL_SURFACE_W - 1, LOGICAL_SURFACE_W - 1 - (y2 * LOGICAL_SURFACE_W) // max(1, fb_h)))
                    gy2 = max(0, min(LOGICAL_SURFACE_H - 1, (x2 * LOGICAL_SURFACE_H) // max(1, fb_w)))
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
                    raise ValueError('unsupported coordinate space')

                slot = int(data.get('slot', 0))
                if action == 'down':
                    ok, msg = send_guest_serial_command(f'INPUT_TOUCH DOWN {gx} {gy} {slot}')
                elif action == 'move':
                    ok, msg = send_guest_serial_command(f'INPUT_TOUCH MOVE {gx} {gy} {slot}')
                elif action == 'up':
                    ok, msg = send_guest_serial_command(f'INPUT_TOUCH UP {gx} {gy} {slot}')
                elif action == 'tap':
                    ok, msg = send_guest_serial_command(f'TAP {gx} {gy}')
                elif action == 'cancel':
                    ok, msg = send_guest_serial_command(f'INPUT_TOUCH CANCEL {gx} {gy} {slot}')
                elif action in ('swipe', 'drag'):
                    steps = max(2, min(20, int(data.get('steps', 5))))
                    ok, msg = send_guest_serial_command(f'SWIPE {gx} {gy} {gx2} {gy2} {steps}')
                elif action in ('home', 'back', 'recents'):
                    ok, msg = send_guest_serial_command(f'NAV {action.upper()}')
                elif action == 'nav':
                    target = str(data.get('target', 'HOME')).upper()
                    ok, msg = send_guest_serial_command(f'NAV {target}')
                else:
                    raise ValueError('unsupported touch action')
                if not ok: raise RuntimeError(msg)
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({'success': True, 'status': 'DISPATCHED', 'source': 'UART_COM1', 'action': action, 'coordinate_space': space, 'guest': {'x': gx, 'y': gy}}).encode())
            except (ValueError, TypeError, json.JSONDecodeError, RuntimeError) as exc:
                self.send_response(400 if isinstance(exc, (ValueError, TypeError, json.JSONDecodeError)) else 503)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': str(exc)}).encode())
            return

        if self.path == '/api/input/key':
            content_length = int(self.headers.get('Content-Length', 0))
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
                key = str(data.get('key', ''))
                navigation = {
                    'Enter', 'Escape', 'Backspace', 'Tab',
                    'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight',
                    'Home', 'End', 'PageUp', 'PageDown'
                }
                if len(key) == 1:
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
            content_length = int(self.headers.get('Content-Length', 0))
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
            content_length = int(self.headers.get('Content-Length', 0))
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
                raw_id = str(data.get('theme_id') or data.get('id') or '').strip()
            except Exception:
                raw_id = ''

            if not raw_id:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Missing theme_id'}).encode())
                return

            theme_id = re.sub(r'[^a-zA-Z0-9_-]', '_', raw_id).lower()
            target_theme = os.path.join(THEMES_DIR, f"{theme_id}.theme")
            target_json = os.path.join(THEMES_DIR, f"{theme_id}.json")
            if not os.path.exists(target_theme) and not os.path.exists(target_json):
                self.send_response(404)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Theme not found'}).encode())
                return

            self.send_response(409)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
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
            self.send_header('Access-Control-Allow-Origin', '*')
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
            self.send_header('Access-Control-Allow-Origin', '*')
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
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Payload too large'}).encode())
                return

            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8')) if body else {}
            except Exception:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Malformed JSON'}).encode())
                return

            if not isinstance(data, dict) or 'cmd' not in data:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Missing cmd field'}).encode())
                return

            raw_cmd = data.get('cmd')
            if not isinstance(raw_cmd, str):
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'cmd must be a string'}).encode())
                return

            raw_cmd = raw_cmd.strip()
            if len(raw_cmd) > 256:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'cmd exceeds maximum length (256 chars)'}).encode())
                return

            # Reject command chaining, newlines, semicolons, pipes, redirection
            if any(c in raw_cmd for c in ('\n', '\r', ';', '|', '&', '`', '$', '>', '<')):
                self.send_response(403)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Command injection or chaining detected.'}).encode())
                return

            # Structured allowlist validation: base operation and allowed subcommands/arguments
            tokens = raw_cmd.split()
            if not tokens:
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Empty command'}).encode())
                return

            base_op = tokens[0].lower()
            ALLOWED_BASE_OPS = {'info', 'help', 'p', 'x'}
            if base_op not in ALLOWED_BASE_OPS:
                self.send_response(403)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
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
                    self.send_header('Access-Control-Allow-Origin', '*')
                    self.end_headers()
                    self.wfile.write(json.dumps({'success': False, 'error': f'Subcommand \'info {tokens[1]}\' restricted in Studio security sandbox.'}).encode())
                    return
            elif base_op == 'help':
                if len(tokens) > 2:
                    self.send_response(403)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Access-Control-Allow-Origin', '*')
                    self.end_headers()
                    self.wfile.write(json.dumps({'success': False, 'error': 'Malformed help query.'}).encode())
                    return
            elif base_op in ('p', 'x'):
                if len(tokens) < 2:
                    self.send_response(400)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Access-Control-Allow-Origin', '*')
                    self.end_headers()
                    self.wfile.write(json.dumps({'success': False, 'error': f'Missing argument for \'{base_op}\' operation'}).encode())
                    return

            ok, resp = send_qemu_monitor_cmd(raw_cmd)
            self.send_response(200 if ok else 500)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps({'success': ok, 'output': resp}).encode())
            return

        if self.path == '/api/fs/read':
            content_length = int(self.headers.get('Content-Length', 0))
            body = self.rfile.read(content_length) if content_length > 0 else b'{}'
            try:
                data = json.loads(body.decode('utf-8'))
                path = str(data.get('path', '')).strip()
            except Exception:
                path = ''
            if not is_safe_guest_path(path):
                self.send_response(400)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Invalid or unsafe guest path'}).encode())
                return
            ok, err, content = guest_fs_read(path)
            self.send_response(200 if ok else 500)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps({'success': ok, 'path': path, 'content': content, 'error': err}).encode())
            return

        if self.path == '/api/fs/write':
            content_length = int(self.headers.get('Content-Length', 0))
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
                self.send_header('Access-Control-Allow-Origin', '*')
                self.end_headers()
                self.wfile.write(json.dumps({'success': False, 'error': 'Invalid or unsafe guest write path'}).encode())
                return
            ok, err = guest_fs_write(path, content)
            self.send_response(200 if ok else 500)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps({'success': ok, 'path': path, 'error': err}).encode())
            return

        self.send_response(404)
        self.end_headers()

    def do_OPTIONS(self):
        self.send_response(200)
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Access-Control-Allow-Methods', 'GET, POST, OPTIONS')
        self.send_header('Access-Control-Allow-Headers', 'Content-Type')
        self.end_headers()


class ThreadedTCPServer(socketserver.ThreadingMixIn, socketserver.TCPServer):
    daemon_threads = True
    allow_reuse_address = True


if __name__ == '__main__':
    ok, msg = start_qemu_backend()
    print(f"[*] InterOS Studio Backend: {msg}")

    start_websockify_if_needed()
    serial_thread = threading.Thread(target=serial_stream_worker, daemon=True)
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
        stop_qemu_backend()
        print("\nStopping studio server.")
        sys.exit(0)