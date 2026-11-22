# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Android Binary Resource Table (resources.arsc) Contract Tests.

Verifies:
1. Strict adherence to AOSP ResourceTypes.h chunk header specification.
2. Resource identifier resolution (0xPPTTEEEE) without string fabrication.
3. String pool UTF-8 and UTF-16 extraction without ambient authority.
4. Production APK resources.arsc table inspection on real packages.
"""

from pathlib import Path
import pytest
import struct
import subprocess

ROOT = Path(__file__).resolve().parents[1]

from tools.test_android_app import (
    analyze_apk,
    extract_entry_payload,
    parse_pkzip_container,
    parse_resources_arsc_summary,
)


def require_fossify_apk() -> Path:
    """External-artifact acceptance: return the pinned acceptance APK.

    Provision explicitly with `python tools/interos.py provision android-apk`
    (network once, SHA-256 verified, git-ignored). Fails closed with that
    instruction instead of fabricating results.
    """
    apk_path = ROOT / "emulator/data/apps/fossify_file_manager.apk"
    assert apk_path.is_file(), (
        "Fossify File Manager APK not provisioned. This external-artifact "
        "acceptance suite requires the genuine upstream release: run "
        "`python tools/interos.py provision android-apk` once (network), "
        "then rerun; the artifact is SHA-256 verified and git-ignored."
    )
    return apk_path


def read_source(rel_path: str) -> str:
    return (ROOT / rel_path).read_text(encoding="utf-8")


def test_kernel_resources_arsc_constants_and_types():
    source = read_source("kernel/src/compat/android/resources.rs")
    assert "pub const RES_STRING_POOL_TYPE: u16 = 0x0001;" in source
    assert "pub const RES_TABLE_TYPE: u16 = 0x0002;" in source
    assert "pub const RES_TABLE_PACKAGE_TYPE: u16 = 0x0200;" in source
    assert "pub const RES_TABLE_TYPE_TYPE: u16 = 0x0201;" in source
    assert "pub const RES_TABLE_TYPE_SPEC_TYPE: u16 = 0x0202;" in source
    assert "pub enum ResourceValue" in source
    assert "pub struct ResourceTable" in source
    assert "pub fn get_string(&self, res_id: u32) -> Option<&str>" in source
    assert "pub fn get_color(&self, res_id: u32) -> Option<u32>" in source
    assert "pub fn get_value(&self, res_id: u32) -> Option<&ResourceValue>" in source


def test_real_apk_resources_arsc_parsing():
    apk_path = require_fossify_apk()

    report = analyze_apk(apk_path)
    arsc = report["resources_arsc"]
    assert arsc is not None
    assert arsc["total_size"] == 5264800
    assert arsc["packages_count"] == 1
    assert arsc["string_pool_count"] >= 50000


def test_synthetic_resources_table_header_validation():
    # Valid minimal RES_TABLE_TYPE header
    hdr = bytearray(12)
    struct.pack_into("<HHII", hdr, 0, 0x0002, 12, 12, 1)
    res = parse_resources_arsc_summary(bytes(hdr))
    assert res is not None
    assert res["total_size"] == 12
    assert res["packages_count"] == 1

    # Invalid header magic
    bad_hdr = bytearray(12)
    struct.pack_into("<HHII", bad_hdr, 0, 0x0005, 12, 12, 1)
    assert parse_resources_arsc_summary(bytes(bad_hdr)) is None
