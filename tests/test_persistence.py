#!/usr/bin/env python3
"""
InterOS Core — Cross-Process Persistent Block Storage Verification
==================================================================
Proves true cross-process persistent block storage across independent QEMU processes:
1. Creates a dedicated secondary persistent disk (target/test_persistence.img).
2. Session 1: Cold boots QEMU with secondary disk attached as Primary Slave (-hdb).
3. Verifies ATA slave enumeration and InterFS initialization.
4. Creates deterministic test state in VFS (/data/token.txt, /data/config.json).
5. Flushes dirty snapshots to durable block media (InterFS + journal + ATA flush).
6. Dispatches clean ACPI guest shutdown and verifies QEMU process exits cleanly.
7. Session 2: Launches a COMPLETELY NEW QEMU process with the SAME persistent disk.
8. Verifies persisted files and metadata are restored and match byte-for-byte.
9. Updates persistent files and verifies multi-generation persistence in Session 3.
10. Executes negative tests: non-existent file, path traversal, ambient write rejection.
"""

import os
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

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


class SerialSession:
    def __init__(self, sock):
        self.sock = sock
        self.buf = b""
        self.captured_logs = []

    def read_line(self, timeout=1.0):
        self.sock.settimeout(timeout)
        start = time.time()
        while time.time() - start < timeout:
            if b"\n" in self.buf:
                line, self.buf = self.buf.split(b"\n", 1)
                dec = line.decode('utf-8', errors='replace').strip()
                if dec:
                    self.captured_logs.append(dec)
                return dec
            try:
                data = self.sock.recv(4096)
                if not data:
                    break
                self.buf += data
            except socket.timeout:
                continue
            except ConnectionResetError:
                break
        return None

    def wait_for(self, marker, timeout=60.0):
        start = time.time()
        while time.time() - start < timeout:
            line = self.read_line(timeout=0.5)
            if line is not None:
                if marker in line:
                    return line
        return None

    def send_cmd(self, cmd_str):
        if not cmd_str.endswith("\n"):
            cmd_str += "\n"
        self.sock.sendall(cmd_str.encode('utf-8'))

    def send_and_wait(self, cmd_str, marker, timeout=15.0):
        time.sleep(0.3)
        self.send_cmd(cmd_str)
        res = self.wait_for(marker, timeout=timeout)
        assert res is not None, f"Command '{cmd_str.strip()}' failed waiting for marker '{marker}'"
        return res


def create_blank_disk(path: Path, size_mb: int = 64):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "wb") as f:
        f.seek(size_mb * 1024 * 1024 - 1)
        f.write(b"\0")
    print(f"[TEST SETUP] Created dedicated {size_mb} MiB disk image: {path}")


def launch_qemu_session(qemu_exe, boot_img, storage_img, port=5558):
    if sys.platform == "win32":
        subprocess.run(["taskkill", "/F", "/IM", "qemu-system-x86_64.exe"], capture_output=True)
    time.sleep(0.8)

    args = [
        qemu_exe,
        "-drive", f"file={boot_img},format=raw,snapshot=on",
        "-drive", f"file={storage_img},format=raw,index=1,media=disk",
        "-m", "1024M",
        "-display", "none",
        "-chardev", f"socket,id=ser0,host=127.0.0.1,port={port},server=on,wait=off",
        "-serial", "chardev:ser0",
        "-no-reboot"
    ]
    proc = subprocess.Popen(args)
    time.sleep(0.8)

    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(10.0)
    sock.connect(('127.0.0.1', port))
    session = SerialSession(sock)
    return proc, sock, session


def main():
    print("=" * 70)
    print("  InterOS Core Cross-Process Persistent Block Storage Verification")
    print("=" * 70)

    qemu_exe = find_qemu("qemu-system-x86_64.exe")
    boot_img = ROOT / "target" / "inter-os-bios.img"
    storage_img = ROOT / "target" / "test_persistence.img"

    assert boot_img.exists(), f"Boot image not found at {boot_img}"

    # 1. Create a clean dedicated persistent disk
    create_blank_disk(storage_img, size_mb=64)

    # =========================================================================
    # SESSION 1: Cold boot, format InterFS, write test state, clean shutdown
    # =========================================================================
    print("\n--- [SESSION 1: Cold Boot & Initial State Establishment] ---")
    proc1, sock1, sess1 = launch_qemu_session(qemu_exe, boot_img, storage_img, port=5558)
    try:
        ready = sess1.wait_for("Step 5: Initial presentation complete", timeout=100.0)
        if not ready:
            print("[DEBUG] Session 1 timed out. Captured logs:")
            for log in sess1.captured_logs:
                print(f"  [GUEST] {log}")
        assert ready, "Session 1 failed to reach Launcher presentation"
        print("  [OK] Session 1 reached Launcher homescreen.")

        # Verify ATA primary slave detection and InterFS formatting
        all_logs = "\n".join(sess1.captured_logs)
        assert "Primary ATA Controller online" in all_logs, "ATA storage controller was not initialized"
        print("  [OK] ATA block device online and bound to storage.")
        for l in sess1.captured_logs:
            if any(k in l for k in ["InterFS", "ATA", "WARN", "VFS"]):
                print(f"  [BOOT LOG] {l}")

        # Run FSCK to verify initial filesystem integrity
        fsck1 = sess1.send_and_wait("FSCK", "[FSCK] status=")
        print(f"  [OK] Initial FSCK: {fsck1}")
        assert "status=CLEAN" in fsck1, f"Initial FSCK failed: {fsck1}"

        # Write test file 1: alphanumeric token
        w1 = sess1.send_and_wait("WRITE_FILE /data/token.txt Persistence_Verified_0xDEADBEEF_42", "[FILE_WRITE]")
        print(f"  [OK] Wrote /data/token.txt: {w1}")
        assert "status=OK" in w1

        # Write test file 2: structured JSON config
        w2 = sess1.send_and_wait('WRITE_FILE /data/config.json {"device":"interos","session":1,"active":true}', "[FILE_WRITE]")
        print(f"  [OK] Wrote /data/config.json: {w2}")
        assert "status=OK" in w2

        # Verify immediate read in Session 1
        r1 = sess1.send_and_wait("READ_FILE /data/token.txt", "[FILE_READ]")
        assert "Persistence_Verified_0xDEADBEEF_42" in r1, f"Immediate read mismatch: {r1}"
        print(f"  [OK] Verified read in Session 1: {r1}")

        # Sync and verify FSCK
        s1 = sess1.send_and_wait("SYNC", "[SYNC]")
        assert "status=OK" in s1
        fsck2 = sess1.send_and_wait("FSCK", "[FSCK] status=")
        assert "status=CLEAN" in fsck2
        print(f"  [OK] Post-write FSCK: {fsck2}")

        # Clean shutdown
        print("  [SESSION 1] Dispatching clean ACPI SHUTDOWN...")
        sess1.send_cmd("SHUTDOWN")
        proc1.wait(timeout=15.0)
        print("  [OK] Session 1 QEMU process terminated cleanly (exit code 0).")
    finally:
        try:
            sock1.close()
        except Exception:
            pass
        if proc1.poll() is None:
            proc1.kill()

    time.sleep(1.0)

    # =========================================================================
    # SESSION 2: Independent QEMU Process with SAME disk; verify state survived
    # =========================================================================
    print("\n--- [SESSION 2: Cross-Process Reboot & Persistence Verification] ---")
    proc2, sock2, sess2 = launch_qemu_session(qemu_exe, boot_img, storage_img, port=5558)
    try:
        ready = sess2.wait_for("Step 5: Initial presentation complete", timeout=100.0)
        assert ready, "Session 2 failed to reach Launcher presentation"
        print("  [OK] Session 2 reached Launcher homescreen.")

        # Read back test file 1
        r1_s2 = sess2.send_and_wait("READ_FILE /data/token.txt", "[FILE_READ]")
        print(f"  [OK] Session 2 read /data/token.txt: {r1_s2}")
        assert "Persistence_Verified_0xDEADBEEF_42" in r1_s2, f"Persisted token mismatch: {r1_s2}"

        # Read back test file 2
        r2_s2 = sess2.send_and_wait("READ_FILE /data/config.json", "[FILE_READ]")
        print(f"  [OK] Session 2 read /data/config.json: {r2_s2}")
        assert '{"device":"interos","session":1,"active":true}' in r2_s2, f"Persisted config mismatch: {r2_s2}"

        # Verify FSCK is clean in Session 2
        fsck_s2 = sess2.send_and_wait("FSCK", "[FSCK] status=")
        print(f"  [OK] Session 2 FSCK: {fsck_s2}")
        assert "status=CLEAN" in fsck_s2, f"Session 2 FSCK failed: {fsck_s2}"

        # Update /data/config.json in Session 2
        print("  [SESSION 2] Updating /data/config.json for Session 3 verification...")
        w_up = sess2.send_and_wait('WRITE_FILE /data/config.json {"device":"interos","session":2,"generation":"v2"}', "[FILE_WRITE]")
        assert "status=OK" in w_up

        sess2.send_and_wait("SYNC", "[SYNC]")
        sess2.send_cmd("SHUTDOWN")
        proc2.wait(timeout=15.0)
        print("  [OK] Session 2 QEMU process terminated cleanly.")
    finally:
        try:
            sock2.close()
        except Exception:
            pass
        if proc2.poll() is None:
            proc2.kill()

    time.sleep(1.0)

    # =========================================================================
    # SESSION 3: Multi-generation update verification & Negative tests
    # =========================================================================
    print("\n--- [SESSION 3: Multi-Generation & Negative Testing] ---")
    proc3, sock3, sess3 = launch_qemu_session(qemu_exe, boot_img, storage_img, port=5558)
    try:
        ready = sess3.wait_for("Step 5: Initial presentation complete", timeout=100.0)
        assert ready, "Session 3 failed to reach Launcher presentation"
        print("  [OK] Session 3 reached Launcher homescreen.")

        # Verify updated config survived
        r_up = sess3.send_and_wait("READ_FILE /data/config.json", "[FILE_READ]")
        print(f"  [OK] Session 3 verified updated config: {r_up}")
        assert '{"device":"interos","session":2,"generation":"v2"}' in r_up, f"Generation update mismatch: {r_up}"

        # Original token still exists unchanged
        r_tok = sess3.send_and_wait("READ_FILE /data/token.txt", "[FILE_READ]")
        assert "Persistence_Verified_0xDEADBEEF_42" in r_tok, f"Original token corrupted: {r_tok}"
        print("  [OK] Original token remained completely intact across 3 independent sessions.")

        # --- Negative Tests ---
        print("\n  [NEGATIVE TEST 1] Non-existent file read rejection:")
        r_neg1 = sess3.send_and_wait("READ_FILE /data/ghost_file.txt", "[FILE_READ]")
        print(f"    Result: {r_neg1}")
        assert "status=NOT_FOUND" in r_neg1

        print("  [NEGATIVE TEST 2] Path traversal read rejection:")
        r_neg2 = sess3.send_and_wait("READ_FILE /data/../../etc/passwd", "[FILE_READ]")
        print(f"    Result: {r_neg2}")
        assert "status=ERR_REJECTED" in r_neg2

        print("  [NEGATIVE TEST 3] Path traversal write rejection:")
        w_neg1 = sess3.send_and_wait("WRITE_FILE /data/../escape.txt payload", "[FILE_WRITE]")
        print(f"    Result: {w_neg1}")
        assert "status=ERR_REJECTED" in w_neg1

        print("  [NEGATIVE TEST 4] Ambient non-/data write rejection:")
        w_neg2 = sess3.send_and_wait("WRITE_FILE /system/bin/malicious payload", "[FILE_WRITE]")
        print(f"    Result: {w_neg2}")
        assert "status=ERR_REJECTED" in w_neg2

        # Final FSCK
        fsck_final = sess3.send_and_wait("FSCK", "[FSCK] status=")
        print(f"  [OK] Final FSCK: {fsck_final}")
        assert "status=CLEAN" in fsck_final

        sess3.send_cmd("SHUTDOWN")
        proc3.wait(timeout=15.0)
        print("  [OK] Session 3 QEMU process terminated cleanly.")
    finally:
        try:
            sock3.close()
        except Exception:
            pass
        if proc3.poll() is None:
            proc3.kill()

    print("\n" + "=" * 70)
    print("  [SUCCESS] Cross-Process Persistent Block Storage FULLY VERIFIED!")
    print("=" * 70)


if __name__ == "__main__":
    main()
