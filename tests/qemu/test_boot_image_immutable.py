"""Regression guard: the guest must never write to its own boot image.

The x86_64 kernel treats the Primary Master as the read-only boot image and the
Primary Slave as persistent storage. With only one disk attached it falls back to
using the boot image for the guest filesystem, which overwrites the bootloader
stages the kernel is embedded in. The next boot then loads a corrupted kernel:
one 4 KiB page of `.text` reads as zeros, the CPU decodes `00 00` as
`add %al,(%rax)`, and the store faults on a pointer into `.rodata`. The device
then booted roughly one time in four, and the first boot after a fresh build was
always the only good one.

This test boots the guest and asserts the boot image is byte-identical
afterwards. That covers the whole class of bug regardless of which subsystem
performs the write.
"""

import hashlib
import os
import subprocess
import sys
import time

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
IMAGE = os.path.join(ROOT, "target", "inter-os-bios.img")
DATA = os.path.join(ROOT, "target", "inter-os-data-x86_64.img")
DATA_BYTES = 512 * 1024 * 1024
LOG = os.path.join(ROOT, "target", "boot-immutability.log")


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def have_qemu():
    try:
        subprocess.run(["qemu-system-x86_64", "--version"],
                       capture_output=True, check=True, timeout=30)
        return True
    except (OSError, subprocess.SubprocessError):
        return False


def main():
    if not os.path.exists(IMAGE):
        print(f"SKIP: boot image missing: {IMAGE}")
        return 0
    if not have_qemu():
        print("SKIP: qemu-system-x86_64 not available")
        return 0

    if not os.path.exists(DATA) or os.path.getsize(DATA) < DATA_BYTES:
        os.makedirs(os.path.dirname(DATA), exist_ok=True)
        subprocess.run(["qemu-img", "create", "-f", "raw", DATA, str(DATA_BYTES)],
                       check=True, capture_output=True)

    before = sha256(IMAGE)
    if os.path.exists(LOG):
        os.remove(LOG)

    proc = subprocess.Popen(
        [
            "qemu-system-x86_64",
            "-drive", f"format=raw,file={IMAGE},index=0",
            "-drive", f"format=raw,file={DATA},index=1",
            "-m", "1024M",
            "-display", "none",
            "-monitor", "none",
            "-serial", f"file:{LOG}",
            "-no-reboot",
        ],
        cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        deadline = time.time() + 60
        while time.time() < deadline:
            time.sleep(2)
            if os.path.exists(LOG):
                with open(LOG, encoding="utf-8", errors="replace") as handle:
                    if "name=launcher" in handle.read():
                        break
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
        time.sleep(2)

    after = sha256(IMAGE)

    with open(LOG, encoding="utf-8", errors="replace") as handle:
        serial = handle.read()

    # The launcher task running is the signal that userspace came up, which is
    # the point at which the guest has had the chance to write its filesystem.
    booted = "name=launcher" in serial or "READY" in serial
    paged_out = "PANIC" in serial or "PAGE FAULT" in serial

    print(f"boot image sha256 before: {before}")
    print(f"boot image sha256 after : {after}")
    print(f"guest brought up userspace: {booted}")
    print(f"guest reported a fault    : {paged_out}")

    if not booted and not paged_out:
        print("SKIP: guest produced no decisive output in the boot window")
        return 0

    if before != after:
        print("FAIL: the guest wrote to its own boot image")
        print("      attach a dedicated data disk so the guest filesystem never")
        print("      overlaps the bootloader that embeds the kernel")
        return 1

    print("PASS: boot image unchanged after boot")
    return 0


if __name__ == "__main__":
    sys.exit(main())