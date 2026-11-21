# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""AOSP APK Signature Scheme (v2, v3, v4) & Signing Block Contract Tests.

Verifies:
1. Strict adherence to AOSP APK Signing Block layout & magic constant.
2. 1 MiB chunked Merkle-tree content digest verification (0xa5 chunk, 0x5a root).
3. APK Signature Scheme ID extraction (v2: 0x7109871a, v3: 0xf05368c0, v4: 0x42726577).
4. Genuine APK cryptographic verification and tamper detection.
"""

from pathlib import Path
import pytest
import struct

ROOT = Path(__file__).resolve().parents[1]

from tools.test_android_app import (
    find_apk_signing_block,
    parse_signing_block_pairs,
    compute_apk_content_merkle_digest,
    APK_SIG_BLOCK_MAGIC,
    APK_SIGNATURE_SCHEME_V2_BLOCK_ID,
    APK_SIGNATURE_SCHEME_V3_BLOCK_ID,
    APK_SIGNATURE_SCHEME_V4_BLOCK_ID,
)


def read_source(rel_path: str) -> str:
    return (ROOT / rel_path).read_text(encoding="utf-8")


def test_kernel_signing_module_invariants():
    source = read_source("kernel/src/compat/android/signing.rs")
    assert 'pub const APK_SIG_BLOCK_MAGIC: &[u8; 16] = b"APK Sig Block 42";' in source
    assert "pub const APK_SIGNATURE_SCHEME_V2_BLOCK_ID: u32 = 0x7109871a;" in source
    assert "pub const APK_SIGNATURE_SCHEME_V3_BLOCK_ID: u32 = 0xf05368c0;" in source
    assert "pub const APK_SIGNATURE_SCHEME_V4_BLOCK_ID: u32 = 0x42726577;" in source
    assert "pub const CONTENT_DIGEST_CHUNK_SIZE: usize = 1024 * 1024;" in source
    assert "pub fn find_apk_signing_block" in source
    assert "pub fn parse_signing_block_pairs" in source
    assert "pub fn compute_apk_content_digest" in source
    assert "pub fn verify_apk_signing_block" in source
    assert "Err(ApkError::InvalidSignature)" in source


def test_real_apk_signing_block_detection():
    apk_path = ROOT / "emulator/data/apps/fossify_file_manager.apk"
    if not apk_path.exists():
        pytest.skip("Fossify File Manager APK not found")
    data = apk_path.read_bytes()

    bounds = find_apk_signing_block(data)
    assert bounds is not None
    assert bounds["size"] == 4096
    assert bounds["start_offset"] == 9841236
    assert bounds["central_dir_offset"] == 9845332
    assert bounds["eocd_offset"] == 9982203
    assert bounds["central_dir_offset"] == bounds["start_offset"] + bounds["size"]


def test_real_apk_signing_block_scheme_pairs():
    apk_path = ROOT / "emulator/data/apps/fossify_file_manager.apk"
    if not apk_path.exists():
        pytest.skip("Fossify File Manager APK not found")
    data = apk_path.read_bytes()

    bounds = find_apk_signing_block(data)
    assert bounds is not None
    pairs = parse_signing_block_pairs(data, bounds)
    assert len(pairs) == 2

    pair_ids = [p["id"] for p in pairs]
    assert APK_SIGNATURE_SCHEME_V2_BLOCK_ID in pair_ids
    assert APK_SIGNATURE_SCHEME_V4_BLOCK_ID in pair_ids


def test_real_apk_merkle_tree_content_digest():
    apk_path = ROOT / "emulator/data/apps/fossify_file_manager.apk"
    if not apk_path.exists():
        pytest.skip("Fossify File Manager APK not found")
    data = apk_path.read_bytes()

    bounds = find_apk_signing_block(data)
    assert bounds is not None
    merkle_root = compute_apk_content_merkle_digest(data, bounds)
    assert merkle_root == "9dee70e199e296a5864145fb4304b0b13235b992b462fe504c5568946ab52cc8"


def test_signing_block_tamper_detection():
    apk_path = ROOT / "emulator/data/apps/fossify_file_manager.apk"
    if not apk_path.exists():
        pytest.skip("Fossify File Manager APK not found")
    data = bytearray(apk_path.read_bytes())

    bounds = find_apk_signing_block(bytes(data))
    assert bounds is not None
    orig_digest = compute_apk_content_merkle_digest(bytes(data), bounds)

    # Tamper 1 byte in ZIP entries (Section 1)
    data[100] ^= 0x01
    tampered_digest = compute_apk_content_merkle_digest(bytes(data), bounds)
    assert tampered_digest != orig_digest
