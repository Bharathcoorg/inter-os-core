# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from tools.test_android_app import (
    analyze_apk,
    audit_apk_security,
    decode_axml_manifest,
    parse_dex_header,
    parse_pkzip_container,
)


def test_android_apk_tool_exists_and_runs():
    tool_path = ROOT / "tools/test_android_app.py"
    assert tool_path.exists(), "tools/test_android_app.py must exist"
    res = subprocess.run([sys.executable, str(tool_path), "--help"], capture_output=True, text=True)
    assert res.returncode == 0
    assert "InterOS Core: Android APK Inspection" in res.stdout or "usage:" in res.stdout


def test_real_apk_container_structure():
    apk_path = ROOT / "emulator/data/apps/fossify_file_manager.apk"
    assert apk_path.exists(), "Fossify File Manager APK must exist in emulator/data/apps"
    raw_data = apk_path.read_bytes()
    container = parse_pkzip_container(raw_data)
    assert container["total_entries"] == 1947
    assert container["cd_offset"] > 0
    names = [e["name"] for e in container["entries"]]
    assert "AndroidManifest.xml" in names
    assert "classes.dex" in names


def test_real_apk_manifest_decoding_without_fabrication():
    apk_path = ROOT / "emulator/data/apps/fossify_file_manager.apk"
    report = analyze_apk(apk_path)
    manifest = report["manifest"]
    assert manifest["package_name"] == "org.fossify.filemanager"
    assert manifest["version_code"] == 13
    assert manifest["target_sdk"] >= 34
    assert "android.permission.WRITE_EXTERNAL_STORAGE" in manifest["permissions"]
    assert "android.permission.REQUEST_INSTALL_PACKAGES" in manifest["permissions"]
    assert len(manifest["activities"]) >= 10


def test_real_apk_dex_header_and_adler32_integrity():
    apk_path = ROOT / "emulator/data/apps/fossify_file_manager.apk"
    report = analyze_apk(apk_path)
    dex = report["dex"]
    assert dex is not None
    assert dex["magic"] in ["dex\n035\0", "dex\n037\0", "dex\n038\0", "dex\n039\0"]
    assert dex["adler32_verified"] is True
    assert dex["classes_count"] > 1000
    assert dex["methods_count"] > 10000


def test_real_apk_zero_trust_security_attestation_fails_closed():
    apk_path = ROOT / "emulator/data/apps/fossify_file_manager.apk"
    report = analyze_apk(apk_path)
    audit = report["audit"]
    assert audit["status"] == "AttestationFailed"
    assert "Unverified APK cryptographic signature" in audit["reason"]
    assert audit["granted_caps_count"] >= 1


def test_synthetic_manifest_ambient_authority_rejection():
    malicious_manifest = {
        "package_name": "com.exploit.ambient",
        "version_code": 1,
        "target_sdk": 34,
        "permissions": ["android.permission.INSTALL_PACKAGES", "android.permission.INTERNET"],
        "activities": [],
    }
    audit = audit_apk_security(malicious_manifest, {"classes_count": 1}, is_signed=True)
    assert audit["status"] == "AmbientAuthorityDenied"
    assert audit["denied_ambient_count"] == 1
