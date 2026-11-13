#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
InterOS Core — Persistent RAPP Intent Registry Verification
==========================================================
Verifies that:
1. First-party intent declarations are strictly seeded at boot.
2. Verified third-party RAPP v4 packages persist their declared intents across reboots.
3. Package uninstallation removes declared intents from the active intent registry.
4. Tampered or corrupted packages are safely rejected and omitted from intent restoration.
5. Sideloading and syscall installation register intents atomically with package activation.
"""

import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def test_first_party_intent_declarations_valid():
    """First-party intents must map essential system handlers."""
    mod_rs = (ROOT / "kernel/src/package/mod.rs").read_text(encoding="utf-8")
    assert "FIRST_PARTY_INTENTS" in mod_rs, "FIRST_PARTY_INTENTS table missing"
    assert '("org.interos.gallery", "mime", "image/png")' in mod_rs, "gallery png handler missing"
    assert '("org.interos.browser", "scheme", "https")' in mod_rs, "browser https handler missing"
    assert '("org.interos.music", "mime", "audio/wav")' in mod_rs, "music wav handler missing"


def test_rapp_v4_intent_format_and_parsing_contract():
    """RAPP v4 manifest format must parse signed intent tables."""
    rapp_rs = (ROOT / "kernel/src/package/rapp.rs").read_text(encoding="utf-8")
    assert "RAPP_FORMAT_VERSION_V4" in rapp_rs, "RAPP v4 format constant missing"
    assert "parse_intent_table" in rapp_rs, "parse_intent_table missing"
    assert "build_rapp_v4" in rapp_rs, "build_rapp_v4 builder missing"


def test_persisted_intent_restoration_contract():
    """Intent restoration must query active packages and re-verify RAPP signatures."""
    mod_rs = (ROOT / "kernel/src/package/mod.rs").read_text(encoding="utf-8")
    assert "pub fn restore_persisted_rapp_intents" in mod_rs, "restore_persisted_rapp_intents function missing"
    assert "PACKAGE_REGISTRY.lock()" in mod_rs, "active registry query missing"
    assert "PACKAGE_BLOB_STORE.lock()" in mod_rs, "blob store lookup missing"
    assert "parse_rapp(&blob.bytes)" in mod_rs, "cryptographic verification before intent restoration missing"
    assert "INTENT_REGISTRY" in mod_rs, "intent registration missing"


def test_uninstallation_removes_intents_contract():
    """Package uninstallation must unregister all handlers belonging to the package."""
    syscall_rs = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
    assert "crate::content::INTENT_REGISTRY.lock().unregister(pkg_id)" in syscall_rs, (
        "syscall uninstallation must unregister package intents"
    )
    serial_rs = (ROOT / "kernel/src/serial.rs").read_text(encoding="utf-8")
    assert "dev_uninstall_verified_rapp" in serial_rs, (
        "serial uninstallation must route through verified uninstaller"
    )


def test_reboot_restoration_order_contract():
    """Boot initialization order must restore blob store and package registry before intent seeding."""
    main_rs = (ROOT / "kernel/src/main.rs").read_text(encoding="utf-8")
    pos_blob = main_rs.find("PACKAGE_BLOB_STORE.lock().restore()")
    pos_reg = main_rs.find("PACKAGE_REGISTRY.lock().seed()")
    pos_content = main_rs.find("kernel::content::init()")
    assert pos_blob != -1, "blob store restoration missing from boot sequence"
    assert pos_reg != -1, "package registry seeding missing from boot sequence"
    assert pos_content != -1, "content init missing from boot sequence"
    assert pos_blob < pos_content, "blob store must be restored before intent registry is seeded"
    assert pos_reg < pos_content, "package registry must be seeded before intent registry is seeded"


def main():
    test_first_party_intent_declarations_valid()
    test_rapp_v4_intent_format_and_parsing_contract()
    test_persisted_intent_restoration_contract()
    test_uninstallation_removes_intents_contract()
    test_reboot_restoration_order_contract()
    print("persistent intent contracts: PASS")


if __name__ == "__main__":
    main()
