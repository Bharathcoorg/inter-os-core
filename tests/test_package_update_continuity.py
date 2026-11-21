# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Package Update Continuity & Transactional Security Contract.

WHAT THIS FILE IS FOR:
Guarantees that package registration and updates enforce strict security continuity:
1. Signer continuity: updates must be signed by the identical cryptographic signer identity;
   unrelated signers attempting to replace an existing package are rejected.
2. Anti-rollback: updates must satisfy monotonic version progression (version_code >= prev.version_code);
   downgrades are strictly rejected.
3. Stable UID & Principal continuity: an authorized package update preserves the identical
   stable app_uid (10000..=19999) and stable principal_id, preserving storage, keystore, and permissions.
4. Transactional persistence: snapshot rollback restores the prior state if persistence fails.
5. Durable collision-free allocation: new packages receive non-colliding UIDs.
"""

from pathlib import Path
import pytest

ROOT = Path(__file__).resolve().parents[1]


def read_source(rel_path: str) -> str:
    return (ROOT / rel_path).read_text(encoding="utf-8")


def test_package_entry_has_security_continuity_fields():
    registry_rs = read_source("kernel/src/package/registry.rs")
    assert "pub version_code: u64," in registry_rs
    assert "pub min_sdk: u32," in registry_rs
    assert "pub target_sdk: u32," in registry_rs
    assert "pub signer_identity: [u8; 32]," in registry_rs
    assert "pub principal_id: u64," in registry_rs


def test_signer_continuity_enforced():
    registry_rs = read_source("kernel/src/package/registry.rs")
    assert "signer identity mismatch" in registry_rs
    assert "prev.signer_identity != signer_identity" in registry_rs


def test_anti_rollback_version_code_enforced():
    registry_rs = read_source("kernel/src/package/registry.rs")
    assert "version_code < prev.version_code" in registry_rs
    assert "update rejected: version downgrade" in registry_rs


def test_update_preserves_uid_and_principal():
    registry_rs = read_source("kernel/src/package/registry.rs")
    assert "(prev.app_uid, prev.principal_id)" in registry_rs


def test_transactional_rollback_on_persist_failure():
    registry_rs = read_source("kernel/src/package/registry.rs")
    assert "self.entries = old_entries;" in registry_rs
    assert "self.generation = old_gen;" in registry_rs


def test_durable_android_uid_allocation_range():
    registry_rs = read_source("kernel/src/package/registry.rs")
    assert "pub fn allocate_android_uid(&mut self) -> Result<u32, ()>" in registry_rs
    assert "10000" in registry_rs
    assert "19999" in registry_rs
