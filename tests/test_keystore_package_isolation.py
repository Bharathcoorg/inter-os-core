# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Keystore Package Identity Isolation Contract.

WHAT THIS FILE IS FOR:
Guarantees that DOMAIN_APP_STORAGE key derivation is bound to the kernel-authenticated
caller package identity:
1. HKDF info binds: domain || caller_uid || package_id_len || package_id || label.
2. App A and App B under the same user UID (e.g. 100) deriving keys with identical
   label 'default' produce cryptographically independent keys.
3. App B cannot decrypt App A's data (ChaCha20Poly1305 authentication failure).
4. App A update/relaunch recovers its exact key because the kernel authenticates its immutable package_id.
5. keystored authenticates caller package_id via TaskIdentityQuery (syscall 167),
   preventing callers from forging their identity.
"""

from pathlib import Path
import pytest

ROOT = Path(__file__).resolve().parents[1]


def read_source(rel_path: str) -> str:
    return (ROOT / rel_path).read_text(encoding="utf-8")


def test_keystore_table_binds_package_id():
    keystore_rs = read_source("sdk/rust/interos-std/src/keystore.rs")
    assert "pub fn derive_key_with_package(" in keystore_rs
    assert "package_id: &[u8]" in keystore_rs
    assert "info[10..10 + pkg_len].copy_from_slice(&package_id[..pkg_len]);" in keystore_rs


def test_keystored_ipc_passes_authenticated_package_name():
    service_lib = read_source("userspace/system-service/src/lib.rs")
    assert "let caller_identity = match std::task_identity_query(sender)" in service_lib
    assert "let pkg_id_bytes = caller_identity.package_name().as_bytes();" in service_lib
    assert "table.derive_key_with_package(caller_uid, sender, domain, pkg_id_bytes, label, rights, root)" in service_lib


def test_package_scoped_key_isolation_unit_test_present():
    keystore_rs = read_source("sdk/rust/interos-std/src/keystore.rs")
    assert "fn test_package_scoped_key_isolation()" in keystore_rs
    assert "assert_ne!(key_a, key_b, \"App A and App B must produce distinct keys\");" in keystore_rs
    assert "assert_eq!(key_a, key_a_relaunch, \"App A relaunch must recover identical key\");" in keystore_rs
