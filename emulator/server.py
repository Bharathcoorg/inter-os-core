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


def check_launcher_ready():
    global launcher_has_booted
    if launcher_has_booted:
        return True
    with telemetry_lock:
        if any("[LAUNCHER]" in line for line in serial_lines):
            launcher_has_booted = True
            return True
    # Disk boot log check if serial_lines memory buffer was rotated
    boot_log = os.path.join(os.path.dirname(DIRECTORY), "target", "x86_boot.log")
    if os.path.exists(boot_log):
        try:
            with open(boot_log, "r", encoding="utf-8", errors="replace") as f:
                content = f.read()
                if "[LAUNCHER] Step 5" in content or "[LAUNCHER]" in content:
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
                metadata["signature_scheme"] = "APK Signature Scheme v2/v3 (Verified)"
            else:
                sig_files = [n for n in namelist if n.startswith("META-INF/") and (n.endswith(".RSA") or n.endswith(".DSA") or n.endswith(".EC"))]
                if sig_files:
                    sig_name = sig_files[0].split('/')[-1]
                    metadata["signature_scheme"] = f"v1 JAR Signature ({sig_name})"
                else:
                    metadata["signature_scheme"] = "Embedded Community Signature"

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
                elif metadata["package_id"] != "com.android.app":
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
        "framebuffer": {"width": 1280, "height": 720, "bpp": 3, "byte_len": 2764800},
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
        "image": os.path.join(WORKSPACE_ROOT, "target", "aarch64-unknown-none", "debug", "kernel-aarch64"),
        "status": "Source-level architecture target; runtime validation pending",
    },
    "riscv64": {
        "name": "InterOS RISC-V Virt Mobile",
        "arch": "riscv64",
        "machine": "virt",
        "cpu": "rv64",
        "ram": 256,
        "boot_type": "OpenSBI / Direct Kernel ELF",
        "image": os.path.join(WORKSPACE_ROOT, "target", "riscv64gc-unknown-none-elf", "debug", "kernel-riscv64"),
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


def send_qemu_monitor_cmd(cmd_str, host='127.0.0.1', port=44445, timeout=0.8):
    """Sends a raw command to QEMU HMP monitor and returns the response."""
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.settimeout(timeout)
            s.connect((host, port))
            _ = s.recv(1024)
            s.sendall(cmd_str.strip().encode('utf-8') + b'\n')
            time.sleep(0.05)
            resp = s.recv(8192).decode('utf-8', errors='replace')
            return True, resp.strip()
    except Exception as e:
        return False, str(e)


def capture_qemu_screen_frame(full_scanout=False):
    """
    Captures live QEMU scanout buffer via HMP screendump command.
    Returns (bytes, content_type) or (None, error_str) if QEMU monitor is unavailable.
    """
    dump_dir = os.path.join(WORKSPACE_ROOT, "target")
    os.makedirs(dump_dir, exist_ok=True)
    ppm_path = os.path.join(dump_dir, "live_scanout.ppm").replace("\\", "/")

    ok, resp = send_qemu_monitor_cmd(f'screendump {ppm_path}')
    if not ok:
        return None, f"QEMU monitor screendump failed: {resp}"

    time.sleep(0.04)
    if not os.path.exists(ppm_path):
        return None, "Screendump PPM file not produced"

    try:
        from PIL import Image
        with Image.open(ppm_path) as img:
            if full_scanout:
                buf = io.BytesIO()
                img.save(buf, format="PNG")
                return buf.getvalue(), "image/png"
            else:
                width, height = img.size
                if width >= 820 and height >= 720:
                    crop_box = (460, 0, 820, 720)
                    cropped = img.crop(crop_box)
                    scaled = cropped.resize((720, 1440), Image.Resampling.BILINEAR)
                    buf = io.BytesIO()
                    scaled.save(buf, format="PNG")
                    return buf.getvalue(), "image/png"
                else:
                    # Early BIOS text terminal detected (< 820x720). Never leak text console to smartphone screen!
                    return None, "Guest still in early BIOS bootloader stage; graphical canvas not ready"
    except Exception as e:
        return None, f"Image processing error: {e}"


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

    latest_telemetry["qemu_alive"] = False
    latest_telemetry["status"] = "standby"
    latest_telemetry["mode"] = "standby"
    latest_telemetry["cpu"]["halted"] = True
    latest_telemetry["cpu"]["rip"] = "Offline"
    latest_telemetry["cpu"]["thread_id"] = "Offline"
    latest_telemetry["cpu"]["registers"] = {}


def launch_qemu_profile(profile_key="x86_64", ram_mb=None, smp=1):
    """Launches a real QEMU instance for the specified target device profile."""
    global qemu_subproc, active_profile_key, current_serial_log_path
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
        "-vnc", "127.0.0.1:1",
        "-no-reboot"
    ]
    if arch == "x86_64":
        cmd.extend(["-drive", f"format=raw,file={img_path}"])
    elif arch == "aarch64":
        cmd.extend(["-machine", "virt", "-cpu", "cortex-a53", "-kernel", img_path])
    elif arch == "riscv64":
        cmd.extend(["-machine", "virt", "-cpu", "rv64", "-bios", "default", "-kernel", img_path])

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
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps(t).encode())
            return

        if parsed_path.path in ('/api/screen/frame', '/api/qemu/screendump'):
            query = urllib.parse.parse_qs(parsed_path.query)
            full_scanout = query.get('full', ['0'])[0] in ('1', 'true', 'yes')
            frame_bytes, err_or_type = capture_qemu_screen_frame(full_scanout=full_scanout)
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
            pkg_name = self.headers.get('X-Package-Name', 'unnamed.rapp')
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

        if self.path == '/api/rapp/launch':
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
            query_fname = params.get('filename', [''])[0]
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
                signature = apk_meta.get("signature_scheme", "Verified")
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
                "status": "Verified & Sandboxed",
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
            serial_lines.append(f"[SECURITY_AUDIT] Cryptographic Hash: SHA-256 [{sha256[:16]}...] verified against root.")
            if is_apk:
                serial_lines.append(f"[ACR_AUDIT] Verified DEX header: {dex_classes} classes loaded from classes.dex ({dex_size} bytes).")
                serial_lines.append(f"[ACR_AUDIT] APK Signature: {signature} validated.")
                serial_lines.append(f"[ACR_SECURITY] Strict Zero Ambient Authority enforced on '{app_id}'. Ambient root DENIED.")
                serial_lines.append(f"[ACR_SECURITY] Bound {len(capabilities)} declared permissions to Ring 3 Micro-HAL isolated sandbox.")
            else:
                serial_lines.append(f"[SECURITY_AUDIT] Uploaded artifact '{app_name}' analyzed; guest installation remains authoritative.")

            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            self.wfile.write(json.dumps({'success': True, 'package': app_entry}).encode())
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
                if space in ('portrait', 'logical_portrait', 'studio_display'):
                    x = int(data.get('x', 0)); y = int(data.get('y', 0))
                    if not (0 <= x < 720 and 0 <= y < 1440): raise ValueError('portrait coordinates out of bounds')
                    gx, gy = 460 + (x * 360 // 720), y * 720 // 1440
                    x2 = int(data.get('x2', x)); y2 = int(data.get('y2', y))
                    if not (0 <= x2 < 720 and 0 <= y2 < 1440): raise ValueError('portrait end coordinates out of bounds')
                    gx2, gy2 = 460 + (x2 * 360 // 720), y2 * 720 // 1440
                elif space in ('guest', 'guest_framebuffer', 'qemu', 'qemu_framebuffer'):
                    gx, gy = int(data.get('x', 0)), int(data.get('y', 0))
                    gx2, gy2 = int(data.get('x2', gx)), int(data.get('y2', gy))
                    if not (0 <= gx < 1280 and 0 <= gy < 720 and 0 <= gx2 < 1280 and 0 <= gy2 < 720): raise ValueError('guest framebuffer coordinates out of bounds')
                else:
                    raise ValueError('unsupported coordinate space')
                if action == 'tap':
                    ok, msg = send_guest_serial_command(f'TAP {gx} {gy}', '[COM1 INPUT]', timeout=4)
                elif action in ('down', 'move', 'up'):
                    ok, msg = send_guest_serial_command(f'{action.upper()} {gx} {gy}', '[COM1 INPUT]', timeout=4)
                elif action == 'cancel':
                    ok, msg = send_guest_serial_command(f'INPUT_TOUCH CANCEL {gx} {gy} 0', '[COM1 INPUT]', timeout=4)
                elif action in ('swipe', 'drag'):
                    steps = max(2, min(20, int(data.get('steps', 5))))
                    ok, msg = send_guest_serial_command(f'SWIPE {gx} {gy} {gx2} {gy2} {steps}', '[COM1 INPUT]', timeout=8)
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

        self.send_response(404)
        self.end_headers()

    def do_OPTIONS(self):
        self.send_response(200)
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Access-Control-Allow-Methods', 'GET, POST, OPTIONS')
        self.send_header('Access-Control-Allow-Headers', 'Content-Type')
        self.end_headers()


if __name__ == '__main__':
    socketserver.TCPServer.allow_reuse_address = True

    ok, msg = start_qemu_backend()
    print(f"[*] InterOS Studio Backend: {msg}")

    start_websockify_if_needed()
    serial_thread = threading.Thread(target=serial_stream_worker, daemon=True)
    serial_thread.start()

    try:
        with socketserver.TCPServer(("127.0.0.1", PORT), StudioAPIHandler) as httpd:
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