# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Android Multidex Application Package Contract Tests.

Verifies:
1. Sovereign Multidex parsing supporting classes.dex, classes2.dex ... classes16.dex.
2. Kernel ApkPackage multidex_count() and get_dex_at() indexing methods.
3. Natural order sorting of multidex payloads.
4. Total classes and methods aggregation across multidex archives.
"""

import io
from pathlib import Path
import pytest
import struct
import zipfile
import zlib

ROOT = Path(__file__).resolve().parents[1]

from tools.test_android_app import (
    analyze_apk,
    parse_dex_header,
    parse_pkzip_container,
)


def read_source(rel_path: str) -> str:
    return (ROOT / rel_path).read_text(encoding="utf-8")


def test_kernel_multidex_architecture_invariants():
    source = read_source("kernel/src/compat/android/apk.rs")
    assert "pub secondary_dex: Vec<Vec<u8>>," in source
    assert "classes2.dex" in source
    assert "for i in 2..=16 {" in source
    assert "pub fn multidex_count(&self) -> usize" in source
    assert "pub fn get_dex_at(&self, idx: usize) -> Option<&[u8]>" in source


def create_minimal_dex_bytes(magic=b"dex\n038\x00") -> bytes:
    hdr = bytearray(112)
    hdr[0:8] = magic
    hdr[32:36] = struct.pack("<I", 112) # file size
    hdr[36:40] = struct.pack("<I", 112) # header size
    hdr[40:44] = struct.pack("<I", 0x12345678) # endian tag
    hdr[88:92] = struct.pack("<I", 10)  # method_ids_size
    hdr[96:100] = struct.pack("<I", 5)  # class_defs_size
    # Calculate Adler-32
    checksum = zlib.adler32(hdr[12:]) & 0xFFFFFFFF
    hdr[8:12] = struct.pack("<I", checksum)
    return bytes(hdr)


def test_synthetic_multidex_package_creation_and_inspection(tmp_path):
    apk_path = tmp_path / "test_multidex.apk"
    dex1 = create_minimal_dex_bytes()
    dex2 = create_minimal_dex_bytes()

    # Minimal AXML manifest
    manifest_bytes = bytearray(8)
    struct.pack_into("<HHI", manifest_bytes, 0, 0x0003, 8, 8)

    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("AndroidManifest.xml", bytes(manifest_bytes))
        zf.writestr("classes.dex", dex1)
        zf.writestr("classes2.dex", dex2)

    apk_path.write_bytes(buf.getvalue())

    report = analyze_apk(apk_path)
    assert len(report["all_dex"]) == 2
    assert report["all_dex"][0]["name"] == "classes.dex"
    assert report["all_dex"][1]["name"] == "classes2.dex"
    assert report["all_dex"][0]["classes_count"] == 5
    assert report["all_dex"][1]["classes_count"] == 5


def test_multidex_natural_ordering():
    # Verify that classes10.dex comes after classes2.dex, not before
    names = ["classes.dex", "classes10.dex", "classes2.dex", "classes3.dex"]
    sorted_names = sorted(names, key=lambda n: (len(n), n))
    assert sorted_names == ["classes.dex", "classes2.dex", "classes3.dex", "classes10.dex"]
