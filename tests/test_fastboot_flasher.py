#!/usr/bin/env python3
"""
Test Contract: Fastboot Flasher & Android Boot Image v2 Specifications
Validates target/boot.img, target/boot-aarch64.img, and tools/flash_phone.ps1
in dry-run/info mode.
"""

import os
import struct
import subprocess
import sys

def verify_boot_image_header(path: str):
    assert os.path.exists(path), f"Boot image does not exist: {path}"
    file_size = os.path.getsize(path)
    assert file_size > 1024 * 1024, f"Boot image is suspiciously small: {file_size} bytes"

    with open(path, "rb") as f:
        header = f.read(64)

    magic = header[:8]
    assert magic == b"ANDROID!", f"Invalid boot image magic: {magic} in {path}"

    kernel_size, kernel_addr, ramdisk_size, ramdisk_addr = struct.unpack("<IIII", header[8:24])
    assert kernel_size > 0, f"Kernel size in {path} must be > 0"
    
    page_size, header_version = struct.unpack("<II", header[36:44])
    assert page_size in (2048, 4096), f"Unexpected page size: {page_size}"
    assert header_version == 2, f"Expected Android Boot Image v2, got version {header_version}"

    return {
        "path": path,
        "file_size": file_size,
        "kernel_size": kernel_size,
        "ramdisk_size": ramdisk_size,
        "page_size": page_size,
        "header_version": header_version,
    }

def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    flasher_script = os.path.join(root, "tools", "flash_phone.ps1")
    assert os.path.exists(flasher_script), f"Flasher script missing: {flasher_script}"

    # 1. Header integrity check for AArch64 phone image
    aarch64_img = os.path.join(root, "target", "boot-aarch64.img")
    info_aarch64 = verify_boot_image_header(aarch64_img)

    # 2. Header integrity check for x86_64 phone image
    x86_img = os.path.join(root, "target", "boot.img")
    info_x86 = verify_boot_image_header(x86_img)

    # 3. Dry-run PowerShell flasher with -Mode Info (default: AArch64)
    res_aarch64 = subprocess.run(
        ["powershell", "-ExecutionPolicy", "Bypass", "-File", flasher_script, "-Mode", "Info"],
        capture_output=True,
        text=True,
        cwd=root,
    )
    assert res_aarch64.returncode == 0, f"flash_phone.ps1 failed: {res_aarch64.stderr}"
    assert "Android Boot Image v2 format verified" in res_aarch64.stdout
    assert "target\\boot-aarch64.img" in res_aarch64.stdout or "target/boot-aarch64.img" in res_aarch64.stdout

    # 4. Dry-run PowerShell flasher with -Mode Info for x86_64 image
    res_x86 = subprocess.run(
        ["powershell", "-ExecutionPolicy", "Bypass", "-File", flasher_script, "-Mode", "Info", "-TargetImg", x86_img],
        capture_output=True,
        text=True,
        cwd=root,
    )
    assert res_x86.returncode == 0, f"flash_phone.ps1 failed for x86_64: {res_x86.stderr}"
    assert "Android Boot Image v2 format verified" in res_x86.stdout
    assert "target\\boot.img" in res_x86.stdout or "target/boot.img" in res_x86.stdout

    print(
        f"[PASS] Fastboot Flasher Contract: AOSP v2 headers verified (aarch64={info_aarch64['file_size']}B, x86_64={info_x86['file_size']}B) and flash_phone.ps1 validated."
    )

if __name__ == "__main__":
    main()
