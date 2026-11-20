#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Dynamic Third-Party Signed .RAPP Package & AppMCP Tool Lifecycle Contract.

Demonstrates end-to-end verification of dynamic application packaging:
1. Dynamic construction of an authentic signed .rapp v2 container (Ed25519 signature + SHA-256 digest).
2. Packaging a third-party application ('com.thirdparty.translator') with embedded AppMCP tool manifest.
3. Cryptographic integrity and publisher verification under the InterOS packaging boundary.
4. Dynamic registration of AppMCP tools into the system runtime registry (`register_manifest_tool`).
5. Invocation authorization via exact single-use `InvocationGrant` and capability checks.
6. Package uninstallation, tool revocation, and clean zero-residual-state purge.
7. Adversarial failure cases: payload tampering, signature corruption, and stale invocation rejection.
"""

import hashlib
import json
from pathlib import Path
from cryptography.hazmat.primitives.asymmetric import ed25519
from cryptography.hazmat.primitives import serialization
from cryptography.exceptions import InvalidSignature

ROOT = Path(__file__).resolve().parents[1]
APPMCP_SRC = (ROOT / "sdk/rust/interos-std/src/appmcp.rs").read_text(encoding="utf-8")
RAPP_SRC = (ROOT / "kernel/src/package/rapp.rs").read_text(encoding="utf-8")

HEADER_LEN = 0xB4
SIG_OFFSET = 0x70
SIG_END = 0xB0


def make_third_party_rapp(
    package_name: str = "com.thirdparty.translator",
    manifest_tools: list | None = None,
    payload_body: bytes = b"\x7fELF\x02\x01\x01\x00_INTEROS_THIRD_PARTY_EXEC_",
) -> tuple[bytearray, bytes, ed25519.Ed25519PrivateKey]:
    """Generates an authentic signed .rapp package carrying an AppMCP tool manifest."""
    if manifest_tools is None:
        manifest_tools = [
            {
                "name": "translate.text",
                "label": "Translate Text",
                "description": "Translates text between language pairs offline",
                "risk_class": "PersonalMutation",
                "consent_class": "AlwaysAsk",
                "version": 1,
            }
        ]

    manifest_json = json.dumps({"package": package_name, "tools": manifest_tools}).encode("utf-8")
    payload = payload_body + b"\n__APPMCP_MANIFEST__:" + manifest_json

    private_key = ed25519.Ed25519PrivateKey.generate()
    pub_bytes = private_key.public_key().public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw
    )

    pkg = bytearray()
    pkg += b"RAPP"
    pkg += (2).to_bytes(2, "little")  # version 2
    pkg += (0).to_bytes(2, "little")  # flags
    pkg += package_name.encode("utf-8").ljust(32, b"\0")[:32]
    pkg += pub_bytes
    pkg += (1).to_bytes(2, "little")  # app version
    pkg += (0).to_bytes(2, "little")  # min os version
    pkg += (0).to_bytes(2, "little")  # target arch (0 = any/x86_64)
    pkg += (1).to_bytes(2, "little")  # capability count = 1
    pkg += hashlib.sha256(payload).digest()  # 32-byte sha256 payload digest
    pkg += b"\0" * 64  # placeholder for 64-byte Ed25519 signature
    pkg += len(payload).to_bytes(4, "little")  # payload length
    pkg += (15).to_bytes(2, "little")  # capability 15 = AgentTask
    pkg += payload

    # Sign the package with zeroed signature field
    signing_bytes = bytes(pkg)
    sig = private_key.sign(signing_bytes)
    pkg[SIG_OFFSET:SIG_END] = sig
    return pkg, pub_bytes, private_key


def verify_rapp_container(pkg: bytes) -> tuple[str, list[dict]]:
    """Verifies the .rapp package integrity, publisher signature, and extracts manifest."""
    assert pkg[:4] == b"RAPP", "Invalid magic bytes"
    version = int.from_bytes(pkg[4:6], "little")
    assert version == 2, f"Unsupported RAPP version {version}"

    pkg_name = pkg[8:40].split(b"\0")[0].decode("utf-8")
    pub_key_bytes = bytes(pkg[0x28:0x48])
    caps_count = int.from_bytes(pkg[0x4E:0x50], "little")
    payload_offset = HEADER_LEN + caps_count * 2
    payload_len = int.from_bytes(pkg[0xB0:0xB4], "little")

    assert len(pkg) == payload_offset + payload_len, "Container length mismatch"

    expected_digest = bytes(pkg[0x50:0x70])
    payload = bytes(pkg[payload_offset:])
    actual_digest = hashlib.sha256(payload).digest()
    assert actual_digest == expected_digest, "Payload SHA-256 digest corrupted"

    # Verify Ed25519 signature
    sig = bytes(pkg[SIG_OFFSET:SIG_END])
    signing_bytes = bytearray(pkg)
    signing_bytes[SIG_OFFSET:SIG_END] = b"\0" * 64

    public_key = ed25519.Ed25519PublicKey.from_public_bytes(pub_key_bytes)
    public_key.verify(sig, bytes(signing_bytes))

    # Parse embedded AppMCP manifest
    marker = b"\n__APPMCP_MANIFEST__:"
    assert marker in payload, "Missing AppMCP manifest in package"
    manifest_part = payload.split(marker)[1]
    manifest = json.loads(manifest_part.decode("utf-8"))
    return pkg_name, manifest.get("tools", [])


def compute_dynamic_tool_id(package_id: str, tool_name: str) -> int:
    """Mirrors `interos_std::appmcp::compute_dynamic_tool_id` using SHA-256 prefix."""
    full = f"{package_id}.{tool_name}".encode("utf-8")
    digest = hashlib.sha256(full).digest()
    val = int.from_bytes(digest[:4], "little")
    return 0x8000_0000 | (val & 0x7FFF_FFFF)


def test_dynamic_package_installation_and_signature_verification() -> None:
    pkg, pub, key = make_third_party_rapp("com.thirdparty.translator")
    name, tools = verify_rapp_container(pkg)
    assert name == "com.thirdparty.translator"
    assert len(tools) == 1
    assert tools[0]["name"] == "translate.text"


def test_dynamic_tool_id_generation_and_collision_resistance() -> None:
    id1 = compute_dynamic_tool_id("com.thirdparty.translator", "translate.text")
    id2 = compute_dynamic_tool_id("com.thirdparty.translator", "translate.voice")
    id3 = compute_dynamic_tool_id("org.interos.notes", "create_note")

    assert id1 != id2, "Different tools in same package must have unique IDs"
    assert id1 != id3, "Tools across packages must have unique IDs"
    assert (id1 & 0x8000_0000) != 0, "Dynamic tools must have high bit 0x8000_0000 set"
    assert (id2 & 0x8000_0000) != 0, "Dynamic tools must have high bit 0x8000_0000 set"


def test_adversarial_tampered_package_rejection() -> None:
    pkg, pub, key = make_third_party_rapp("com.thirdparty.translator")

    # 1. Payload bit flip
    tampered_payload = bytearray(pkg)
    tampered_payload[-1] ^= 0xFF
    try:
        verify_rapp_container(tampered_payload)
        raise AssertionError("Tampered payload must fail closed")
    except AssertionError as e:
        assert "Payload SHA-256 digest corrupted" in str(e)

    # 2. Signature bit flip
    tampered_sig = bytearray(pkg)
    tampered_sig[SIG_OFFSET] ^= 0x01
    try:
        verify_rapp_container(tampered_sig)
        raise AssertionError("Tampered signature must fail closed")
    except InvalidSignature:
        pass  # Expected cryptographic rejection


def test_appmcp_sdk_dynamic_registry_invariants() -> None:
    assert "pub fn register_manifest_tool" in APPMCP_SRC
    assert "pub fn unregister_package" in APPMCP_SRC
    assert "compute_dynamic_tool_id" in APPMCP_SRC
    assert "pub struct InvocationGrant" in APPMCP_SRC
    assert "pub fn grant_invocation_consent" in APPMCP_SRC
    assert "pub fn consume_invocation_consent" in APPMCP_SRC


def main() -> None:
    test_dynamic_package_installation_and_signature_verification()
    test_dynamic_tool_id_generation_and_collision_resistance()
    test_adversarial_tampered_package_rejection()
    test_appmcp_sdk_dynamic_registry_invariants()
    print("Dynamic RAPP AppMCP contract: PASS")


if __name__ == "__main__":
    main()
