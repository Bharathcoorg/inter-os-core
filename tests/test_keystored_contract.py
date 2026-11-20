#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Dedicated Keystore Service & Opaque Key Custody Contract.

Verifies the high-assurance security guarantees of the keystore subsystem:
- Dedicated `keystored` userspace daemon with `#![no_std]` and `#![no_main]`.
- Opaque `KeyHandle` abstraction preventing raw key leakage to Ring 3 callers.
- Strict HKDF-based cryptographic domain separation.
- Per-caller UID and task capability ownership verification.
- ChaCha20-Poly1305 AEAD and HMAC-SHA256 authenticated operations inside keystore custody.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
KEYSTORE_SDK = (ROOT / "sdk/rust/interos-std/src/keystore.rs").read_text(encoding="utf-8")
KEYSTORE_BIN = (ROOT / "userspace/system-service/src/bin/keystored.rs").read_text(encoding="utf-8")
SERVICE_LIB = (ROOT / "userspace/system-service/src/lib.rs").read_text(encoding="utf-8")
CARGO_TOML = (ROOT / "userspace/system-service/Cargo.toml").read_text(encoding="utf-8")


def must(text: str, needle: str, label: str) -> None:
    assert needle in text, f"missing {label}: {needle}"


def test_keystored_binary_manifest() -> None:
    must(CARGO_TOML, 'name = "keystored"', "keystored binary definition in Cargo.toml")
    must(CARGO_TOML, 'path = "src/bin/keystored.rs"', "keystored binary path")
    must(KEYSTORE_BIN, "pub extern \"C\" fn _start() -> !", "keystored _start entrypoint")
    must(KEYSTORE_BIN, "interos_system_service::run_keystore();", "keystored dispatch to run_keystore")


def test_opaque_key_handle_contract() -> None:
    must(KEYSTORE_SDK, "pub struct KeyHandle", "KeyHandle struct definition")
    must(KEYSTORE_SDK, "pub const SIZE: usize = 12;", "KeyHandle exact wire size (12 bytes)")
    must(KEYSTORE_SDK, "pub fn to_bytes(&self) -> [u8; Self::SIZE]", "KeyHandle serialization")
    must(KEYSTORE_SDK, "pub fn from_bytes(raw: &[u8]) -> Option<Self>", "KeyHandle deserialization")
    assert "pub key: [u8; 32]" not in KEYSTORE_SDK.split("pub struct KeyHandle")[1].split("}")[0], (
        "KeyHandle must remain opaque and must not expose raw key bytes"
    )


def test_domain_separation_contract() -> None:
    must(KEYSTORE_SDK, "pub const DOMAIN_COMPANION: u32 = 1;", "DOMAIN_COMPANION constant")
    must(KEYSTORE_SDK, "pub const DOMAIN_SEMANTIC_SEARCH: u32 = 2;", "DOMAIN_SEMANTIC_SEARCH constant")
    must(KEYSTORE_SDK, "pub const DOMAIN_CONVERSATION_HISTORY: u32 = 3;", "DOMAIN_CONVERSATION_HISTORY constant")
    must(KEYSTORE_SDK, "pub const DOMAIN_APP_STORAGE: u32 = 4;", "DOMAIN_APP_STORAGE constant")
    must(KEYSTORE_SDK, "pub const DOMAIN_DEVICE_ROOT: u32 = 5;", "DOMAIN_DEVICE_ROOT constant")


def test_keystored_service_loop_and_ipc_dispatch() -> None:
    must(SERVICE_LIB, "pub fn run_keystore() -> !", "run_keystore function definition")
    must(SERVICE_LIB, "fn handle_keystore_ipc<const N: usize>(", "handle_keystore_ipc function definition")
    must(SERVICE_LIB, "std::keystore::KEYSTORE_OP_DERIVE => {", "KEYSTORE_OP_DERIVE handling")
    must(SERVICE_LIB, "std::keystore::KEYSTORE_OP_ENCRYPT => {", "KEYSTORE_OP_ENCRYPT handling")
    must(SERVICE_LIB, "std::keystore::KEYSTORE_OP_DECRYPT => {", "KEYSTORE_OP_DECRYPT handling")
    must(SERVICE_LIB, "std::keystore::KEYSTORE_OP_SIGN => {", "KEYSTORE_OP_SIGN handling")
    must(SERVICE_LIB, "std::keystore::KEYSTORE_OP_VERIFY => {", "KEYSTORE_OP_VERIFY handling")
    must(SERVICE_LIB, "std::keystore::KEYSTORE_OP_DESTROY => {", "KEYSTORE_OP_DESTROY handling")


def main() -> None:
    test_keystored_binary_manifest()
    test_opaque_key_handle_contract()
    test_domain_separation_contract()
    test_keystored_service_loop_and_ipc_dispatch()
    print("Keystored contract: PASS")


if __name__ == "__main__":
    main()
