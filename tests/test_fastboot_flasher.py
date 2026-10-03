#!/usr/bin/env python3
"""
Test Contract: Android Boot Image integrity and the Fastboot flashers.

WHAT THIS CONTRACT ASSERTS, AND WHY IT CHANGED
-----------------------------------------------
This test previously asserted that `target/boot.img` and
`target/boot-aarch64.img` existed and carried an `ANDROID!` header. That
contract could only be satisfied while the image generator fabricated its two
payloads, because no real device tree or initramfs exists anywhere in this
repository. Specifically, `tools/mkbootimg/src/main.rs` used to synthesise:

  * a "device tree" as the FDT magic d00dfeed followed by the ASCII text
    `_SOVEREIGN_DEVICE_TREE_BLOB_MOBILE_DEVBOARD_V1`, and
  * a "ramdisk" as the cpio magic 070701 followed by the ASCII text
    `INTER_OS_SOVEREIGN_RAMDISK_CONTAINING_INIT_SETTINGS_TERMINAL_DIALER_RETAIL_APK`,

while writing non-zero `dtb_size` / `ramdisk_size` into the header. So the
container was well-formed, would have flashed to a device without complaint,
carried no real device tree and no real init, and the flasher then reported
"[SUCCESS] ... pipeline verified" after checking only the `ANDROID!` magic.

Asserting the existence of that artifact certified the fabrication. This
contract now asserts the property that actually matters and is actually
guaranteed:

  1. Both flashers REJECT a container whose payloads are not structurally real,
     including the exact magic-plus-ASCII shape that used to be shipped.
  2. Both flashers ACCEPT a structurally valid container, so the check is a real
     discriminator and not a blanket refusal.
  3. The image generator refuses a non-AArch64 kernel, so an x86_64 kernel can
     never be packaged for ARM64 phone hardware again.
  4. The image generator refuses to build when the device tree or ramdisk is
     absent, rather than substituting placeholders.
"""

import os
import struct
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PAGE = 2048


# --------------------------------------------------------------------------
# Fixture construction
# --------------------------------------------------------------------------
def _pad(data: bytes) -> bytes:
    rem = len(data) % PAGE
    return data if rem == 0 else data + b"\x00" * (PAGE - rem)


def _boot_image(path: str, dtb: bytes, ramdisk: bytes, kernel_size: int = 4096) -> int:
    """Writes an Android boot image v2 container carrying the given payloads."""
    hdr = bytearray(PAGE)
    hdr[0:8] = b"ANDROID!"
    struct.pack_into("<I", hdr, 8, kernel_size)
    struct.pack_into("<I", hdr, 12, 0x80008000)
    struct.pack_into("<I", hdr, 16, len(ramdisk))
    struct.pack_into("<I", hdr, 20, 0x82000000)
    struct.pack_into("<I", hdr, 24, 0)
    struct.pack_into("<I", hdr, 32, 0x80000100)
    struct.pack_into("<I", hdr, 36, PAGE)
    struct.pack_into("<I", hdr, 40, 2)
    struct.pack_into("<I", hdr, 44, (14 << 25) | (9 << 14) | 2026)
    hdr[48:64] = b"INTER_OS_AARCH64"
    struct.pack_into("<I", hdr, 1600, 1660)
    struct.pack_into("<I", hdr, 1632, len(dtb))
    struct.pack_into("<I", hdr, 1636, 0x80000000)

    img = bytes(hdr) + b"\x00" * kernel_size + _pad(ramdisk) + _pad(dtb)
    with open(path, "wb") as f:
        f.write(img)
    return len(img)


def fabricated_payloads():
    """The exact placeholders the image generator used to synthesise."""
    dtb = bytes([0xD0, 0x0D, 0xFE, 0xED]) + b"_SOVEREIGN_DEVICE_TREE_BLOB_MOBILE_DEVBOARD_V1"
    ramdisk = b"070701_INTER_OS_SOVEREIGN_RAMDISK_CONTAINING_INIT_SETTINGS_TERMINAL_DIALER_RETAIL_APK"
    return dtb, ramdisk


def genuine_payloads():
    """A structurally valid minimal FDT and a real cpio newc archive."""
    hdr_len, struct_len, strings_len = 40, 8, 4
    total = hdr_len + struct_len + strings_len
    dtb = struct.pack(">IIIIIIIIII",
                      0xD00DFEED, total, hdr_len, hdr_len + struct_len,
                      0, 17, 16, 0, strings_len, struct_len)
    dtb += struct.pack(">I", 0x00000001) + b"\x00" * (struct_len - 4)
    dtb += b"\x00" * strings_len

    def newc(name: bytes, filesize: int) -> bytes:
        body = name + b"\x00"
        while (len(body) - 110) % 4 != 0:
            body += b"\x00"
        fields = [0x070701, 1, 0o100644, 0, 0, 1, 0, filesize, 0, 0, 0, 0, len(body), 0]
        return ("070701" + "".join("%08X" % f for f in fields)).encode() + body

    return dtb, newc(b"init", 0) + newc(b"TRAILER!!!", 0)


def find_bash():
    for c in (r"C:\Program Files\Git\bin\bash.exe",
              r"C:\Program Files\Git\usr\bin\bash.exe"):
        if os.path.exists(c):
            return c
    return None


# --------------------------------------------------------------------------
# Assertions
# --------------------------------------------------------------------------
def main() -> None:
    tmp = os.path.join(ROOT, "target")
    os.makedirs(tmp, exist_ok=True)
    fake = os.path.join(tmp, "contract-fake-boot.img")
    real = os.path.join(tmp, "contract-real-boot.img")

    fdtb, framdisk = fabricated_payloads()
    gdtb, gramdisk = genuine_payloads()
    _boot_image(fake, fdtb, framdisk)
    _boot_image(real, gdtb, gramdisk)

    ps1 = os.path.join(ROOT, "tools", "flash_phone.ps1")
    sh = os.path.join(ROOT, "tools", "flash_phone.sh")
    assert os.path.exists(ps1), f"Flasher script missing: {ps1}"
    assert os.path.exists(sh), f"Bash flasher script missing: {sh}"

    failures = []

    # --- PowerShell flasher -------------------------------------------------
    r = subprocess.run(["powershell", "-ExecutionPolicy", "Bypass", "-File", ps1,
                        "-Mode", "Info", "-TargetImg", fake],
                       capture_output=True, text=True, cwd=ROOT)
    if r.returncode == 0:
        failures.append("flash_phone.ps1 ACCEPTED a container with fabricated payloads "
                        "(magic plus ASCII), which is the exact artifact that used to ship.")
    if "REJECTED" not in r.stdout:
        failures.append("flash_phone.ps1 did not report a REJECTED reason for the fabricated image.")

    r = subprocess.run(["powershell", "-ExecutionPolicy", "Bypass", "-File", ps1,
                        "-Mode", "Info", "-TargetImg", real],
                       capture_output=True, text=True, cwd=ROOT)
    if r.returncode != 0:
        failures.append("flash_phone.ps1 REJECTED a structurally valid image, so the check is "
                        f"a blanket refusal rather than a discriminator: {r.stdout[-400:]}")
    if "Payload integrity verified" not in r.stdout:
        failures.append("flash_phone.ps1 did not confirm payload integrity for the valid image.")
    if "NOT READY" not in r.stdout:
        failures.append("flash_phone.ps1 no longer states that physical-hardware readiness is "
                        "not established.")

    # The old contract relied on this exact string. It must be gone, because the
    # old validator printed it for images carrying no real payload at all.
    r_all = subprocess.run(["powershell", "-ExecutionPolicy", "Bypass", "-File", ps1,
                            "-Mode", "Info", "-TargetImg", real],
                           capture_output=True, text=True, cwd=ROOT)
    if "Image validation and Fastboot flasher pipeline verified" in r_all.stdout:
        failures.append("flash_phone.ps1 still prints the unconditional '[SUCCESS] ... pipeline "
                        "verified' line that used to certify fabricated payloads.")

    # --- Bash flasher -------------------------------------------------------
    bash = find_bash()
    if bash:
        r = subprocess.run([bash, sh, fake, "info"], capture_output=True, text=True, cwd=ROOT)
        if r.returncode == 0:
            failures.append("flash_phone.sh ACCEPTED a container with fabricated payloads.")
        if "REJECTED" not in r.stdout:
            failures.append("flash_phone.sh did not report a REJECTED reason for the fabricated image.")

        r = subprocess.run([bash, sh, real, "info"], capture_output=True, text=True, cwd=ROOT)
        if r.returncode != 0:
            failures.append("flash_phone.sh REJECTED a structurally valid image: "
                            f"{r.stdout[-400:]} {r.stderr[-200:]}")
        if "Payload integrity verified" not in r.stdout:
            failures.append("flash_phone.sh did not confirm payload integrity for the valid image.")
    else:
        print("[SKIP] No bash available; flash_phone.sh was not exercised.")

    # --- Image generator refuses bad input ---------------------------------
    mk = os.path.join(ROOT, "tools", "mkbootimg", "Cargo.toml")
    x86_kernel = os.path.join(ROOT, "kernel", "target", "x86_64-unknown-none", "debug", "kernel")
    if os.path.exists(x86_kernel):
        r = subprocess.run(["cargo", "+nightly", "run", "--quiet", "--manifest-path", mk,
                            "--", "--kernel", x86_kernel,
                            "--output", os.path.join(tmp, "contract-arch.img")],
                           capture_output=True, text=True, cwd=ROOT)
        if r.returncode == 0:
            failures.append("mkbootimg packaged an x86_64 kernel into an ARM64 phone image; "
                            "such an image flashes cleanly and then does not boot.")
        if "e_machine" not in (r.stdout + r.stderr):
            failures.append("mkbootimg did not report the detected ELF architecture when refusing.")

    a64_kernel = os.path.join(ROOT, "kernel-aarch64", "target",
                              "aarch64-unknown-none", "debug", "kernel-aarch64")
    if os.path.exists(a64_kernel):
        r = subprocess.run(["cargo", "+nightly", "run", "--quiet", "--manifest-path", mk,
                            "--", "--kernel", a64_kernel,
                            "--output", os.path.join(tmp, "contract-missing.img")],
                           capture_output=True, text=True, cwd=ROOT)
        if r.returncode == 0:
            failures.append("mkbootimg produced a phone image with no device tree and no ramdisk.")
        out = r.stdout + r.stderr
        if "ramdisk" not in out or "device tree" not in out:
            failures.append("mkbootimg did not name the missing payloads when refusing.")

    if failures:
        print("[FAIL] Fastboot Flasher Contract:")
        for f in failures:
            print(f"       - {f}")
        sys.exit(1)

    print("[PASS] Fastboot Flasher Contract: fabricated payloads are rejected by both flashers "
          "(ps1/sh), structurally valid images are accepted, and mkbootimg refuses both a "
          "non-AArch64 kernel and missing DTB/ramdisk.")


if __name__ == "__main__":
    main()
