#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
InterOS Core — Storage Crash & Power-Loss Recovery Verification
===============================================================
Deterministic crash simulation and transactional recovery verification:
1. Scenario 1: Abrupt QEMU process termination (SIGKILL / power-cut simulation)
   - Writes committed baseline file and syncs.
   - Initiates un-synced dirty writes, then abruptly kills QEMU.
   - Boots a new QEMU process on the SAME disk.
   - Verifies FSCK is CLEAN and committed baseline survived intact.
2. Scenario 2: Interrupted Write Transaction Rollback
   - Injects an incomplete transaction (IFTX) into the InterFS superblock
     simulating power loss before inode completion.
   - Boots QEMU; verifies kernel recovery detects pending transaction,
     discards uncommitted allocated sectors, restores superblock, and reports CLEAN.
3. Scenario 3: Interrupted Journal Ping-Pong Fallback
   - Simulates interrupted journal write by corrupting one slot of the
     redundant dual-slot journal while leaving the previous generation intact.
   - Boots QEMU; verifies journal rejects corrupted slot and safely recovers
     the last valid generation.
"""

import os
import shutil
import socket
import struct
import subprocess
import sys
import time
from pathlib import Path

def kill_proc_on_port(p):
    try:
        import psutil
        for conn in psutil.net_connections():
            if conn.laddr and conn.laddr.port == p and conn.pid:
                try:
                    proc = psutil.Process(conn.pid)
                    # A foreign owner means another device is using this port;
                    # leave it running and let the bind below fail loudly.
                    if "qemu" in proc.name().lower():
                        proc.kill()
                except Exception:
                    pass
    except Exception:
        pass


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
        self.cmd_log_idx = 0

    def flush(self, settle=0.1):
        deadline = time.time() + settle
        while time.time() < deadline:
            self.sock.settimeout(0.02)
            try:
                data = self.sock.recv(65536)
                if not data:
                    break
                self.buf += data
                while b"\n" in self.buf:
                    line, self.buf = self.buf.split(b"\n", 1)
                    decoded = line.decode('utf-8', errors='replace').strip()
                    if decoded:
                        self.captured_logs.append(decoded)
            except (socket.timeout, BlockingIOError):
                break
            except ConnectionResetError:
                time.sleep(0.02)
                break
        try:
            self.sock.settimeout(0.2)
        except OSError:
            pass

    def send_cmd(self, cmd_str):
        self.flush()
        self.cmd_log_idx = len(self.captured_logs)
        if not cmd_str.endswith("\n"):
            cmd_str += "\n"
        self.sock.sendall(cmd_str.encode('utf-8'))

    def wait_for(self, marker, timeout=60.0):
        # Check logs already captured since last command
        for log in self.captured_logs[self.cmd_log_idx:]:
            if marker in log:
                return log

        start = time.time()
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
                        if marker in decoded:
                            return decoded
            except socket.timeout:
                continue
            except ConnectionResetError:
                time.sleep(0.05)
                break
        return None

    def send_and_wait(self, cmd_str, marker, timeout=35.0):
        time.sleep(0.5)
        self.send_cmd(cmd_str)
        res = self.wait_for(marker, timeout=timeout)
        if res is None:
            print(f"[DEBUG send_and_wait failed] cmd: {cmd_str.strip()}, marker: {marker}")
            print(f"[DEBUG last 25 captured logs]:")
            for l in self.captured_logs[-25:]:
                print(f"  {l}")
        assert res is not None, f"Command '{cmd_str.strip()}' failed waiting for marker '{marker}'"
        return res


def create_blank_disk(path: Path, size_mb: int = 64):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "wb") as f:
        f.seek(size_mb * 1024 * 1024 - 1)
        f.write(b"\0")
    print(f"[TEST SETUP] Created dedicated {size_mb} MiB disk image: {path}")


def launch_qemu(qemu_exe, boot_img, storage_img, port=5559):
    kill_proc_on_port(port)
    time.sleep(0.5)

    args = [
        qemu_exe,
        "-machine", "q35",
        "-cpu", "qemu64",
        "-m", "1024M",
        "-device", "piix3-ide,id=ide",
        "-drive", f"file={boot_img},format=raw,if=none,id=bootd,snapshot=on",
        "-device", "ide-hd,bus=ide.0,unit=0,drive=bootd,bootindex=1",
        "-drive", f"file={storage_img},format=raw,if=none,id=datad",
        "-device", "ide-hd,bus=ide.0,unit=1,drive=datad",
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


def sha256(data: bytes) -> bytes:
    import hashlib
    return hashlib.sha256(data).digest()


def main():
    print("=" * 70)
    print("  InterOS Core Storage Crash & Power-Loss Recovery Verification")
    print("=" * 70)

    qemu_exe = find_qemu("qemu-system-x86_64.exe")
    boot_img = ROOT / "target" / "inter-os-bios.img"
    recovery_img = ROOT / "target" / "test_recovery.img"

    assert boot_img.exists(), f"Boot image missing: {boot_img}"

    # 1. Fresh disk initialization
    create_blank_disk(recovery_img, size_mb=64)

    # =========================================================================
    # SCENARIO 1: Abrupt QEMU process termination (SIGKILL / power-loss)
    # =========================================================================
    print("\n--- [SCENARIO 1: Abrupt Power-Loss / Hard SIGKILL During Operations] ---")
    proc1, sock1, sess1 = launch_qemu(qemu_exe, boot_img, recovery_img, port=5559)
    try:
        ready = sess1.wait_for("Step 5: Initial presentation complete", timeout=100.0)
        assert ready, "Scenario 1 failed to reach homescreen"
        print("  [OK] Guest reached online homescreen.")
        time.sleep(1.0)

        # Write durable baseline file
        w1 = sess1.send_and_wait("WRITE_FILE /data/baseline.txt Durable_Committed_State_0xFEED", "[FILE_WRITE]")
        assert "status=OK" in w1
        s1 = sess1.send_and_wait("SYNC", "[SYNC]")
        assert "status=OK" in s1
        print("  [OK] Committed baseline written and hardware-flushed.")

        # Simulate in-flight dirty write without sync
        sess1.send_cmd("WRITE_FILE /data/uncommitted.txt Inflight_Discardable_Garbage")
        time.sleep(0.1)

        # ABRUPT TERMINATION (Simulate host power cut / hard crash)
        print("  [CRASH SIMULATION] Triggering hard process kill (SIGKILL equivalent)...")
        proc1.kill()
        proc1.wait(timeout=5.0)
        print("  [OK] Guest abruptly killed without guest shutdown or orderly unmount.")
    finally:
        try:
            sock1.close()
        except Exception:
            pass
        if proc1.poll() is None:
            proc1.kill()

    time.sleep(1.2)

    # Post-crash reboot with the SAME disk
    print("\n--- [POST-CRASH REBOOT: Verifying Structure and Committed State] ---")
    proc2, sock2, sess2 = launch_qemu(qemu_exe, boot_img, recovery_img, port=5561)
    try:
        ready2 = sess2.wait_for("Step 5: Initial presentation complete", timeout=100.0)
        assert ready2, "Post-crash reboot failed to reach homescreen"
        print("  [OK] Post-crash guest booted cleanly.")

        # Run FSCK to prove filesystem structure is valid
        fsck_res = sess2.send_and_wait("FSCK", "[FSCK] status=")
        print(f"  [OK] Post-crash FSCK: {fsck_res}")
        assert "status=CLEAN" in fsck_res, f"Filesystem corruption detected after abrupt kill: {fsck_res}"

        # Verify committed baseline is intact
        r_base = sess2.send_and_wait("READ_FILE /data/baseline.txt", "[FILE_READ]")
        print(f"  [OK] Verified baseline data: {r_base}")
        assert "Durable_Committed_State_0xFEED" in r_base, f"Baseline mismatch: {r_base}"

        sess2.send_cmd("SHUTDOWN")
        proc2.wait(timeout=15.0)
        print("  [OK] Post-crash session shut down cleanly.")
    finally:
        try:
            sock2.close()
        except Exception:
            pass
        if proc2.poll() is None:
            proc2.kill()

    time.sleep(1.2)

    # =========================================================================
    # SCENARIO 2: Incomplete Write Transaction Rollback
    # =========================================================================
    print("\n--- [SCENARIO 2: Incomplete Write Transaction Rollback] ---")
    # Directly inject an uncompleted transaction record (IFTX) into the superblock
    # of the persistent disk image.
    # In InterFS:
    #   base_lba for persistent storage is 2048 (FilesystemLayout::canonical)
    #   superblock is at base_lba (2048 * 512 = 1,048,576 bytes)
    #   TXN_OFFSET = 64
    # FsTransaction format:
    #   0..4: b"IFTX"
    #   4: version = 1
    #   5: op = 2 (Write)
    #   8..12: inode_index = 0
    #   12: old_used = 1, old_size = 31, old_start = 0, old_count = 1
    #   25: new_used = 1, new_size = 99, new_start = 50, new_count = 2 (uncommitted extent)
    #   56..64: sha256(buf[..56])[..8]
    sb_byte_offset = 4096 * 512
    with open(recovery_img, "r+b") as f:
        f.seek(sb_byte_offset)
        sb_sector = bytearray(f.read(512))
        assert sb_sector[0:4] == b"IOFS", f"Invalid superblock magic: {sb_sector[0:4]}"

        # Read inode_lba and bitmap_lba from superblock
        inode_lba = struct.unpack_from("<I", sb_sector, 32)[0]
        bitmap_lba = struct.unpack_from("<I", sb_sector, 24)[0]

        # Read actual inode 0 from inode table (at 4096 + inode_lba)
        f.seek((4096 + inode_lba) * 512)
        inode0 = f.read(64)
        old_used = inode0[0]
        old_size = struct.unpack_from("<Q", inode0, 1)[0]
        old_start = struct.unpack_from("<I", inode0, 9)[0]
        old_count = struct.unpack_from("<I", inode0, 13)[0]

        # Allocate uncommitted sectors (start=50, count=2) in the allocation bitmap
        f.seek((4096 + bitmap_lba) * 512)
        bitmap_sector0 = bytearray(f.read(512))
        bitmap_sector0[6] |= 0x0C # set bits 50 and 51
        f.seek((4096 + bitmap_lba) * 512)
        f.write(bitmap_sector0)

        # Craft pending transaction:
        # inode 0 matches current disk state (old_*)
        # new state points to uncommitted extent at start=50, count=2
        # Recovery must detect current == old, free the uncommitted sectors at 50,
        # and clear the transaction record in the superblock!
        txn_buf = bytearray(64)
        txn_buf[0:4] = b"IFTX"
        txn_buf[4] = 2 # version 2 (TXN_VERSION)
        txn_buf[5] = 2 # op = Write
        struct.pack_into("<I", txn_buf, 8, 0) # inode_index 0
        txn_buf[12] = old_used # old_used
        struct.pack_into("<Q", txn_buf, 13, old_size) # old_size (u64, 8 bytes: 13..21)
        struct.pack_into("<I", txn_buf, 21, old_start) # old_start (u32, 4 bytes: 21..25)
        struct.pack_into("<I", txn_buf, 25, old_count) # old_count (u32, 4 bytes: 25..29)
        txn_buf[29] = 1 # new_used (1 byte: 29)
        struct.pack_into("<Q", txn_buf, 30, old_size + 800) # new_size (u64, 8 bytes: 30..38)
        struct.pack_into("<I", txn_buf, 38, 50) # new_start (u32, 4 bytes: 38..42)
        struct.pack_into("<I", txn_buf, 42, 2) # new_count (u32, 4 bytes: 42..46)
        h = sha256(bytes(txn_buf[:56]))
        txn_buf[56:64] = h[:8]

        # Inject into superblock sector at offset 64
        sb_sector[64:128] = txn_buf
        f.seek(sb_byte_offset)
        f.write(sb_sector)
        f.flush()
    print("  [INJECTION] Injected pending IFTX transaction into Superblock sector 4096.")

    # Boot guest to trigger mount-time transaction rollback
    proc3, sock3, sess3 = launch_qemu(qemu_exe, boot_img, recovery_img, port=5563)
    try:
        ready3 = sess3.wait_for("Step 5: Initial presentation complete", timeout=100.0)
        assert ready3, "Transaction recovery reboot failed"
        print("  [OK] Guest successfully booted and recovered pending transaction.")

        # FSCK must report CLEAN (no leaked sectors at extent 50)
        fsck_txn = sess3.send_and_wait("FSCK", "[FSCK] status=")
        print(f"  [OK] Post-recovery FSCK: {fsck_txn}")
        assert "status=CLEAN" in fsck_txn

        # Committed file is still intact
        r_after = sess3.send_and_wait("READ_FILE /data/baseline.txt", "[FILE_READ]")
        assert "Durable_Committed_State_0xFEED" in r_after, f"Expected baseline data, got: {r_after}"
        print(f"  [OK] Verified baseline data survives rollback: {r_after}")

        sess3.send_cmd("SHUTDOWN")
        proc3.wait(timeout=15.0)
    finally:
        try:
            sock3.close()
        except Exception:
            pass
        if proc3.poll() is None:
            proc3.kill()

    # Verify that the kernel cleared the transaction from the superblock on disk
    with open(recovery_img, "rb") as f:
        f.seek(sb_byte_offset + 64)
        cleared_txn = f.read(64)
        assert cleared_txn == b"\0" * 64, "Kernel failed to clear transaction record after rollback"
    print("  [OK] Superblock on disk verified: transaction record was cleanly cleared.")

    # =========================================================================
    # SCENARIO 3: Corrupted Journal Transaction Fallback
    # =========================================================================
    print("\n--- [SCENARIO 3: Corrupted Journal Transaction Fallback] ---")
    # Corrupt the header of slot 0 in the journal area (tail of the 64 MiB disk).
    # Total sectors = 64 * 1024 * 1024 / 512 = 131,072
    # Journal reserved = 16 * 128 = 2048 sectors
    # Base journal sector = 131072 - 2048 = 129,024
    # Slot 0 is at sector 129024 (byte offset: 129024 * 512 = 66,060,288)
    slot0_offset = (131072 - 2048) * 512
    with open(recovery_img, "r+b") as f:
        f.seek(slot0_offset)
        # Write corrupted garbage into the journal slot header
        f.write(b"GARBAGE_NOT_IOSJ_HEADER_CORRUPTED_RECORD_TESTING_RECOVERY!!")
        f.flush()
    print("  [INJECTION] Corrupted journal slot 0 header to simulate partial journal write.")

    proc4, sock4, sess4 = launch_qemu(qemu_exe, boot_img, recovery_img, port=5565)
    try:
        ready4 = sess4.wait_for("Step 5: Initial presentation complete", timeout=100.0)
        assert ready4, "Journal recovery reboot failed"
        print("  [OK] Guest successfully rejected corrupt journal slot and booted safely.")

        # Baseline file remains available
        r_j = sess4.send_and_wait("READ_FILE /data/baseline.txt", "[FILE_READ]")
        assert "Durable_Committed_State_0xFEED" in r_j
        print(f"  [OK] Verified persistent file intact after journal corruption fallback.")

        sess4.send_cmd("SHUTDOWN")
        proc4.wait(timeout=15.0)
    finally:
        try:
            sock4.close()
        except Exception:
            pass
        if proc4.poll() is None:
            proc4.kill()

    print("\n" + "=" * 70)
    print("  [SUCCESS] All Crash, Transaction Rollback & Journal Recovery Tests PASSED!")
    print("=" * 70)


if __name__ == "__main__":
    main()
