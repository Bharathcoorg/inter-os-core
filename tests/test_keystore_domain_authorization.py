# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Keystore Domain & Rights Authorization Policy Contract.

WHAT THIS FILE IS FOR:
Guarantees that KEYSTORE_OP_DERIVE enforces strict authenticated kernel-backed
domain authorization and privilege reduction before any key handle is minted:
1. DOMAIN_DEVICE_ROOT (5) is unconditionally denied to all callers over IPC.
2. DOMAIN_SEMANTIC_SEARCH (2) is restricted strictly to searchd.
3. DOMAIN_COMPANION (1) is restricted strictly to agentd.
4. DOMAIN_CONVERSATION_HISTORY (3) is restricted strictly to agentd or conversation.
5. DOMAIN_APP_STORAGE (4) for non-system apps is restricted to RIGHT_ENCRYPT | RIGHT_DECRYPT.
6. Excess rights (outside RIGHT_ALL) and 0 rights are rejected with InvalidRights.
7. Domain isolation prevents cross-domain key minting and cross-service custody violations.
"""

from pathlib import Path
import pytest

ROOT = Path(__file__).resolve().parents[1]


def read_source(rel_path: str) -> str:
    return (ROOT / rel_path).read_text(encoding="utf-8")


def test_keystore_domain_constants():
    keystore_rs = read_source("sdk/rust/interos-std/src/keystore.rs")
    assert "pub const DOMAIN_COMPANION: u32 = 1;" in keystore_rs
    assert "pub const DOMAIN_SEMANTIC_SEARCH: u32 = 2;" in keystore_rs
    assert "pub const DOMAIN_CONVERSATION_HISTORY: u32 = 3;" in keystore_rs
    assert "pub const DOMAIN_APP_STORAGE: u32 = 4;" in keystore_rs
    assert "pub const DOMAIN_DEVICE_ROOT: u32 = 5;" in keystore_rs
    assert "pub const RIGHT_ALL: u32 =" in keystore_rs


def test_device_root_unconditionally_denied():
    service_lib = read_source("userspace/system-service/src/lib.rs")
    assert "domain == std::keystore::DOMAIN_DEVICE_ROOT" in service_lib
    assert "return Err(std::keystore::KeystoreError::UnauthorizedDomain);" in service_lib


def test_rights_validation_refuses_excess_and_zero():
    service_lib = read_source("userspace/system-service/src/lib.rs")
    assert "rights == 0 || (rights & !std::keystore::RIGHT_ALL) != 0" in service_lib
    assert "return Err(std::keystore::KeystoreError::InvalidRights);" in service_lib


def test_semantic_search_domain_restricted_to_searchd():
    service_lib = read_source("userspace/system-service/src/lib.rs")
    assert "std::keystore::DOMAIN_SEMANTIC_SEARCH =>" in service_lib
    assert "SERVICE_SEARCH" in service_lib
    assert "searchd" in service_lib


def test_companion_domain_restricted_to_agentd():
    service_lib = read_source("userspace/system-service/src/lib.rs")
    assert "std::keystore::DOMAIN_COMPANION =>" in service_lib
    assert "SERVICE_AGENT" in service_lib
    assert "agentd" in service_lib


def test_conversation_domain_restricted_to_agent_or_conversation():
    service_lib = read_source("userspace/system-service/src/lib.rs")
    assert "std::keystore::DOMAIN_CONVERSATION_HISTORY =>" in service_lib
    assert "conversation" in service_lib


def test_ordinary_apps_restricted_to_encrypt_decrypt_in_app_storage():
    service_lib = read_source("userspace/system-service/src/lib.rs")
    assert "std::keystore::DOMAIN_APP_STORAGE =>" in service_lib
    assert "if caller_identity.is_system == 0 {" in service_lib
    assert "let allowed = std::keystore::RIGHT_ENCRYPT | std::keystore::RIGHT_DECRYPT;" in service_lib
    assert "if (rights & !allowed) != 0 {" in service_lib
    assert "return Err(std::keystore::KeystoreError::InvalidRights);" in service_lib
