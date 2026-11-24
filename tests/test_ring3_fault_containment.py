#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
InterOS Core — Ring-3 Unprivileged Fault & Crash Containment Test
================================================================
Proves controlled userspace application failure does not terminate the OS
or leak system resources.

Verification Points:
1. Authentic Ring-3 unprivileged CPU faults (#UD via ud2, #DE via divide-by-zero)
   executed in Ring-3 user space.
2. Hardware CPU exception vector (#UD / #DE) caught by kernel IDT handler with RPL=3.
3. Faulted task is terminated cleanly via recover_and_terminate_current().
4. Kernel continues running (zero kernel panics, zero OS termination).
5. All surfaces owned by the faulted task are destroyed via DRM_KMS.
6. Shell / Launcher surface is restored to foreground visibility.
7. Launcher remains fully responsive and interactive to touch/nav/app launch commands.
8. Resources are reclaimed with zero leaks (verified via DIAG_STATE telemetry).
"""

import os
import re
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace", line_buffering=True)

try:
    from emulator.server import qemu_x86_audio_args
except ImportError:
    def qemu_x86_audio_args(_):
        return []

from tests.verify_acr_x86 import ensure_test_apk_generator, generate_apk, stage_apk_to_qemu


def find_qemu() -> str | None:
    names = (
        ["qemu-system-x86_64.exe", "qemu-system-x86_64"]
        if sys.platform == "win32"
        else ["qemu-system-x86_64"]
    )
    qemu_env = os.environ.get("QEMU_PATH")
    candidates: list[Path] = []
    if qemu_env:
        env_path = Path(qemu_env)
        if env_path.is_file():
            candidates.append(env_path)
        else:
            for name in names:
                candidates.append(env_path / name)

    for name in names:
        which = shutil.which(name)
        if which:
            candidates.append(Path(which))

    for name in names:
        candidates.append(Path.home() / "qemu" / name)
    if sys.platform == "win32":
        candidates.append(Path("C:/Program Files/qemu/qemu-system-x86_64.exe"))
        candidates.append(Path("C:/qemu/qemu-system-x86_64.exe"))
    else:
        candidates.append(Path("/usr/bin/qemu-system-x86_64"))
        candidates.append(Path("/usr/local/bin/qemu-system-x86_64"))
        candidates.append(Path("/opt/homebrew/bin/qemu-system-x86_64"))

    for cand in candidates:
        if cand.exists():
            try:
                res = subprocess.run(
                    [str(cand), "--version"],
                    capture_output=True,
                    text=True,
                    timeout=5,
                )
                if res.returncode == 0 and "QEMU" in res.stdout:
                    return str(cand)
            except (OSError, subprocess.SubprocessError):
                continue
    return None


def get_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


class SerialReader:
    def __init__(self, sock: socket.socket):
        self.sock = sock
        try:
            self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        except OSError:
            pass
        self.sock.settimeout(0.5)
        self.buf = b""
        self.captured_logs: list[str] = []
        self.cmd_log_idx = 0

    def send(self, line: str) -> None:
        time.sleep(0.3)
        self.cmd_log_idx = len(self.captured_logs)
        cmd_bytes = (line if line.endswith("\n") else line + "\n").encode("utf-8")
        self.sock.sendall(cmd_bytes)

    def wait_for(self, marker, timeout: float = 30.0, print_keywords=None) -> bool:
        start_scan = self.cmd_log_idx
        for log in self.captured_logs[start_scan:]:
            if isinstance(marker, (list, tuple)):
                if any(m in log for m in marker):
                    return True
            elif marker in log:
                return True

        start_t = time.time()
        while time.time() - start_t < timeout:
            try:
                data = self.sock.recv(4096)
                if not data:
                    break
                self.buf += data
                while b"\n" in self.buf:
                    raw_line, self.buf = self.buf.split(b"\n", 1)
                    decoded = raw_line.decode("utf-8", errors="replace").strip()
                    if decoded:
                        self.captured_logs.append(decoded)
                        if print_keywords and any(k in decoded for k in print_keywords):
                            print(f"  [GUEST] {decoded}")
                        if isinstance(marker, (list, tuple)):
                            if any(m in decoded for m in marker):
                                return True
                        elif marker in decoded:
                            return True
            except socket.timeout:
                continue
            except ConnectionResetError:
                time.sleep(0.1)
                continue
        return False


def parse_diag_state(reader: SerialReader) -> dict:
    reader.send("DIAG_STATE")
    assert reader.wait_for("[DIAG_STATE]", timeout=10.0), "Timeout waiting for DIAG_STATE"
    diag_line = None
    for line in reversed(reader.captured_logs[reader.cmd_log_idx:]):
        if "[DIAG_STATE]" in line:
            diag_line = line
            break
    assert diag_line, "Failed to capture fresh DIAG_STATE line"
    fields = {}
    for part in diag_line.split():
        if "=" in part:
            k, v = part.split("=", 1)
            try:
                fields[k] = int(v)
            except ValueError:
                fields[k] = v
    return fields


def query_surface_owners(reader: SerialReader) -> set[int]:
    reader.send("SURFACES")
    reader.send("DIAG_STATE")
    assert reader.wait_for("[DIAG_STATE]", timeout=10.0), "Timeout waiting for SURFACES sentinel"
    owners: set[int] = set()
    for line in reader.captured_logs[reader.cmd_log_idx - 2:]:
        if line.startswith("[SURFACES]"):
            m = re.search(r"owner=(\d+)", line)
            if m:
                owners.add(int(m.group(1)))
    return owners


def assert_no_kernel_crash(reader: SerialReader, proc: subprocess.Popen) -> None:
    assert proc.poll() is None, "QEMU process terminated unexpectedly (Kernel Panic / Triple Fault)"
    fatal_markers = (
        "INTEROS CORE KERNEL PANIC",
        "FatalKernelFault",
        "Double Fault (#DF)",
        "EXCEPTION: DOUBLE FAULT",
        "[KERNEL PAGE FAULT]",
        "[KERNEL GPF]",
    )
    for line in reader.captured_logs:
        for fatal in fatal_markers:
            assert fatal not in line, f"Fatal kernel fault detected in serial log: {line}"


def run_fault_containment_test() -> int:
    print("=================================================================")
    print("  InterOS Core — Ring-3 Unprivileged Fault & Crash Containment   ")
    print("=================================================================")

    qemu_exe = find_qemu()
    if not qemu_exe:
        print("SKIP: qemu-system-x86_64 binary not available or failed --version check.")
        return 2

    from tools.image_provenance import verify_boot_image_provenance

    kernel_img = ROOT / "target" / "verify" / "e2e_acceptance" / "inter-os-bios-tls-dev.img"
    assert kernel_img.exists(), (
        f"Missing acceptance dev kernel image at {kernel_img}. "
        "Build via 'python tools/interos.py build acceptance-dev'."
    )
    verify_boot_image_provenance(kernel_img, expected_debug_control_plane=True, allow_source_diff=True)

    artifact_dir = ROOT / "target" / "verify" / "fault_containment"
    artifact_dir.mkdir(parents=True, exist_ok=True)

    data_disk = artifact_dir / "inter-os-data-fault-test.img"
    with data_disk.open("wb") as f:
        f.truncate(512 * 1024 * 1024)

    ser_port = get_free_port()
    mon_port = get_free_port()

    args = [
        qemu_exe,
        "-machine", "q35",
        "-cpu", "qemu64,+rdrand",
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
        "-no-reboot",
    ]
    args.extend(qemu_x86_audio_args(qemu_exe))
    proc = subprocess.Popen(args)

    serial_sock = None
    try:
        for _ in range(50):
            try:
                s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                s.connect(("127.0.0.1", ser_port))
                serial_sock = s
                break
            except OSError:
                time.sleep(0.2)
        assert serial_sock is not None, "Failed to connect to QEMU serial socket"
        reader = SerialReader(serial_sock)

        # Step 1: Boot & Homescreen
        print("[STEP 1] Booting device & presenting Homescreen...")
        assert reader.wait_for(
            "Step 5: Initial presentation complete",
            timeout=75.0,
            print_keywords=["LAUNCHER", "PID1", "Step 5", "acrd"],
        ), "Launcher failed presentation"
        lock_ok = reader.wait_for(
            "Boot presentation complete; lock screen is now guest-owned",
            timeout=20.0,
        )
        if not lock_ok:
            reader.wait_for("Lock surface rendered.", timeout=15.0)
        assert reader.wait_for(
            "userspace Android Compatibility Runtime daemon online",
            timeout=30.0,
            print_keywords=["acrd"],
        ), "acrd daemon failed to come online"

        time.sleep(1.0)
        reader.send("SWIPE 360 1350 360 300 5")
        unlocked = reader.wait_for("Device unlocked", timeout=20.0, print_keywords=["LAUNCHER"])
        if not unlocked:
            reader.send("SWIPE 360 1350 360 300 5")
            unlocked = reader.wait_for("Device unlocked", timeout=20.0, print_keywords=["LAUNCHER"])
        assert unlocked, "Failed to unlock device"
        assert reader.wait_for("Home surface rendered", timeout=15.0), "Homescreen failed to render"
        print("  [OK] Device online and unlocked to Homescreen.")

        # Step 2: Baseline Telemetry
        print("[STEP 2] Recording baseline resource telemetry...")
        time.sleep(0.5)
        baseline = parse_diag_state(reader)
        print(
            f"  [BASELINE] tasks_active={baseline.get('tasks_active')} "
            f"surfaces={baseline.get('surfaces')} "
            f"mailboxes={baseline.get('mailboxes')} "
            f"endpoints={baseline.get('endpoints')}"
        )

        # Step 3: Stage test APK #1 (for #UD)
        print("[STEP 3] Generating and staging test APK (com.example.hello)...")
        gen_bin = ensure_test_apk_generator()
        apk_hello_path = artifact_dir / "app_hello.apk"
        apk_hello_bytes = generate_apk(gen_bin, "com.example.hello", apk_hello_path)

        kw = ["APK_HOST", "acr-app-runtime", "acrd", "FAULT"]
        stage_apk_to_qemu(reader, "com.example.hello", apk_hello_bytes)
        assert reader.wait_for(
            "[ACR] Spawned Android process: pkg='com.example.hello'",
            timeout=30.0,
            print_keywords=kw,
        ), "Timeout waiting for com.example.hello spawn"
        assert reader.wait_for(
            "[acr-app-runtime] rendered View hierarchy and presented pixels to compositor",
            timeout=25.0,
            print_keywords=kw,
        ), "Timeout waiting for child View presentation"
        time.sleep(0.8)

        mid_state = parse_diag_state(reader)
        print(
            f"  [RUNNING APP 1] tasks_active={mid_state.get('tasks_active')} "
            f"surfaces={mid_state.get('surfaces')}"
        )
        assert mid_state.get("tasks_active", 0) > baseline.get("tasks_active", 0), (
            "Active task count did not increase on child launch"
        )

        # Step 4: Inject Ring-3 #UD (Invalid Opcode)
        print("[STEP 4] Injecting authentic Ring-3 #UD (Invalid Opcode via ud2)...")
        fault_idx = len(reader.captured_logs)
        reader.send("APK_FAULT com.example.hello ud")
        assert reader.wait_for(
            "[APK_HOST] FAULT_INJECTED pkg=com.example.hello mode=ud",
            timeout=10.0,
            print_keywords=kw,
        ), "Timeout waiting for FAULT_INJECTED mode=ud"
        assert reader.wait_for(
            "[acr-app-runtime] FAULT INJECTION: triggering deliberate Ring-3 #UD (Invalid Opcode) via ud2",
            timeout=10.0,
            print_keywords=kw,
        ), "Timeout waiting for child runtime #UD trigger log"
        assert reader.wait_for(
            "[FAULT] Ring-3 Invalid Opcode (#UD)",
            timeout=10.0,
            print_keywords=kw,
        ), "Timeout waiting for hardware [FAULT] Ring-3 Invalid Opcode (#UD)"
        assert reader.wait_for(
            "Terminating user task.",
            timeout=10.0,
            print_keywords=kw,
        ), "Timeout waiting for 'Terminating user task.'"

        child_tid = None
        for line in reader.captured_logs[fault_idx:]:
            m = re.search(r"\[APK_HOST\] FAULT_INJECTED pkg=com\.example\.hello mode=ud child=(\d+)", line)
            if m:
                child_tid = int(m.group(1))
                break
        assert child_tid is not None, "Could not parse child task ID from FAULT_INJECTED log"
        print(f"  [OK] Ring-3 #UD caught by kernel IDT handler; child task {child_tid} terminated.")

        # Step 5: Verify surface cleanup, zero resource leak, and OS continuity after #UD
        time.sleep(1.0)
        assert_no_kernel_crash(reader, proc)
        owners_after_ud = query_surface_owners(reader)
        assert child_tid not in owners_after_ud, (
            f"Dead child task {child_tid} still owns KMS surface(s): {owners_after_ud}"
        )
        reclaimed_ud = parse_diag_state(reader)
        print(
            f"  [AFTER #UD] tasks_active={reclaimed_ud.get('tasks_active')} "
            f"surfaces={reclaimed_ud.get('surfaces')} "
            f"mailboxes={reclaimed_ud.get('mailboxes')} "
            f"endpoints={reclaimed_ud.get('endpoints')}"
        )
        assert reclaimed_ud.get("tasks_active") == baseline.get("tasks_active"), (
            f"Active tasks leaked after #UD: {reclaimed_ud.get('tasks_active')} != {baseline.get('tasks_active')}"
        )
        assert reclaimed_ud.get("surfaces") == baseline.get("surfaces"), (
            f"Surfaces leaked after #UD: {reclaimed_ud.get('surfaces')} != {baseline.get('surfaces')}"
        )
        assert reclaimed_ud.get("mailboxes") == baseline.get("mailboxes"), (
            f"Mailboxes leaked after #UD: {reclaimed_ud.get('mailboxes')} != {baseline.get('mailboxes')}"
        )
        assert reclaimed_ud.get("endpoints") == baseline.get("endpoints"), (
            f"Endpoints leaked after #UD: {reclaimed_ud.get('endpoints')} != {baseline.get('endpoints')}"
        )

        # Step 6: Stage test APK #2 and inject Ring-3 #DE (Divide Error)
        print("[STEP 6] Staging second APK (com.example.counter) & injecting Ring-3 #DE...")
        apk_counter_path = artifact_dir / "app_counter.apk"
        apk_counter_bytes = generate_apk(gen_bin, "com.example.counter", apk_counter_path)
        stage_apk_to_qemu(reader, "com.example.counter", apk_counter_bytes)
        assert reader.wait_for(
            "[ACR] Spawned Android process: pkg='com.example.counter'",
            timeout=30.0,
            print_keywords=kw,
        ), "Timeout waiting for com.example.counter spawn"
        assert reader.wait_for(
            "[acr-app-runtime] rendered View hierarchy and presented pixels to compositor",
            timeout=25.0,
            print_keywords=kw,
        ), "Timeout waiting for counter View presentation"
        time.sleep(0.8)

        de_idx = len(reader.captured_logs)
        reader.send("APK_FAULT com.example.counter de")
        assert reader.wait_for(
            "[APK_HOST] FAULT_INJECTED pkg=com.example.counter mode=de",
            timeout=10.0,
            print_keywords=kw,
        ), "Timeout waiting for FAULT_INJECTED mode=de"
        assert reader.wait_for(
            "[acr-app-runtime] FAULT INJECTION: triggering deliberate Ring-3 #DE (Divide Error)",
            timeout=10.0,
            print_keywords=kw,
        ), "Timeout waiting for child runtime #DE trigger log"
        assert reader.wait_for(
            "[FAULT] Ring-3 Divide Error (#DE)",
            timeout=10.0,
            print_keywords=kw,
        ), "Timeout waiting for hardware [FAULT] Ring-3 Divide Error (#DE)"
        assert reader.wait_for(
            "Terminating user task.",
            timeout=10.0,
            print_keywords=kw,
        ), "Timeout waiting for 'Terminating user task.' after #DE"

        child_tid2 = None
        for line in reader.captured_logs[de_idx:]:
            m = re.search(r"\[APK_HOST\] FAULT_INJECTED pkg=com\.example\.counter mode=de child=(\d+)", line)
            if m:
                child_tid2 = int(m.group(1))
                break
        assert child_tid2 is not None, "Could not parse child task ID for com.example.counter"
        print(f"  [OK] Ring-3 #DE caught by kernel IDT handler; child task {child_tid2} terminated.")

        # Step 7: Verify Launcher Foreground Recovery, Interactivity, and Final Reclamation
        print("[STEP 7] Verifying Launcher foreground recovery, app launch interactivity, and final reclamation...")
        time.sleep(1.0)
        assert_no_kernel_crash(reader, proc)
        owners_after_de = query_surface_owners(reader)
        assert child_tid2 not in owners_after_de, (
            f"Dead child task {child_tid2} still owns KMS surface(s): {owners_after_de}"
        )

        reclaimed_final = parse_diag_state(reader)
        print(
            f"  [AFTER #DE] tasks_active={reclaimed_final.get('tasks_active')} "
            f"surfaces={reclaimed_final.get('surfaces')} "
            f"mailboxes={reclaimed_final.get('mailboxes')} "
            f"endpoints={reclaimed_final.get('endpoints')}"
        )
        assert reclaimed_final.get("tasks_active") == baseline.get("tasks_active"), (
            f"Active tasks leaked after #DE: {reclaimed_final.get('tasks_active')} != {baseline.get('tasks_active')}"
        )
        assert reclaimed_final.get("surfaces") == baseline.get("surfaces"), (
            f"Surfaces leaked after #DE: {reclaimed_final.get('surfaces')} != {baseline.get('surfaces')}"
        )
        assert reclaimed_final.get("mailboxes") == baseline.get("mailboxes"), (
            f"Mailboxes leaked after #DE: {reclaimed_final.get('mailboxes')} != {baseline.get('mailboxes')}"
        )
        assert reclaimed_final.get("endpoints") == baseline.get("endpoints"), (
            f"Endpoints leaked after #DE: {reclaimed_final.get('endpoints')} != {baseline.get('endpoints')}"
        )

        reader.send("NAV HOME")
        assert reader.wait_for("Home surface rendered", timeout=15.0, print_keywords=["LAUNCHER"]), (
            "Launcher failed to re-present Home surface after child crashes"
        )
        reader.send("RAPP_LAUNCH notes")
        assert reader.wait_for(
            "[NOTES] Initial frame presented via hardware scanout.",
            timeout=25.0,
            print_keywords=["NOTES", "LAUNCHER"],
        ), "Launcher/OS failed to launch native Notes app after Ring-3 crashes"
        reader.send("NAV HOME")
        assert reader.wait_for("Home surface rendered", timeout=15.0, print_keywords=["LAUNCHER"]), (
            "Launcher failed to return Home after post-crash app launch"
        )
        assert_no_kernel_crash(reader, proc)

        serial_log_out = artifact_dir / "fault_containment_serial.log"
        serial_log_out.write_text("\n".join(reader.captured_logs) + "\n", encoding="utf-8")
        print(f"  [OK] Launcher interactive & native app launch verified. Log: {serial_log_out}")

        print("\n=================================================================")
        print("  Ring-3 Fault Containment & Resource Reclamation: ALL PASS       ")
        print("=================================================================")
        return 0

    finally:
        if serial_sock is not None:
            try:
                serial_sock.close()
            except OSError:
                pass
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()


if __name__ == "__main__":
    sys.exit(run_fault_containment_test())
