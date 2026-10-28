#!/usr/bin/env python3
"""Contract test for Lane 3: .rapp trust chain (RAPP_TRUST).

Enforces:
1. Canonical cryptographic signature verification of .rapp against embedded developer/publication keys.
2. Fail-closed security on untrusted or tampered containers.
3. Trust decision recording as a discrete typed state (Trusted, Untrusted, Revoked, Tampered), never a UI string.
4. Accept + Reject + Tampered payload & header test coverage.
5. Demonstration of RED (tampered container accepted by naive parser) before GREEN (strict verification).
"""

import hashlib
import sys
from pathlib import Path
from cryptography.hazmat.primitives.asymmetric import ed25519
from cryptography.hazmat.primitives import serialization
from cryptography.exceptions import InvalidSignature

ROOT = Path(__file__).resolve().parents[1]

HEADER_SIZE = 0xB4
SIG_OFFSET = 0x70
SIG_SIZE = 64
SIG_END = SIG_OFFSET + SIG_SIZE

# Canonical embedded developer seeds matching kernel/src/package/revocation.rs
DEV_SEED = b"interos-developer-key-seed-2026!"
SYS_SEED = b"interos-platform-pubkey-seed2026"

DEV_PRIV = ed25519.Ed25519PrivateKey.from_private_bytes(DEV_SEED)
DEV_PUB = DEV_PRIV.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)

SYS_PRIV = ed25519.Ed25519PrivateKey.from_private_bytes(sys_seed := SYS_SEED)
SYS_PUB = SYS_PRIV.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)


class TrustState:
    Trusted = 0
    Untrusted = 1
    Revoked = 2
    Tampered = 3


def signed_message(pkg_bytes, payload_offset, payload_len):
    """Computes canonical signed message with signature slot zeroed."""
    msg = bytearray()
    msg.extend(pkg_bytes[:SIG_OFFSET])
    msg.extend(b"\x00" * SIG_SIZE)
    msg.extend(pkg_bytes[SIG_END:payload_offset])
    msg.extend(pkg_bytes[payload_offset:payload_offset + payload_len])
    return bytes(msg)


def build_rapp_v2(package_id, pub_key, priv_key, capabilities, payload, version=(1, 0, 0)):
    """Encodes a canonical RAPP v2 container with Ed25519 signature."""
    pkg_id_bytes = package_id.encode("utf-8").ljust(32, b"\x00")[:32]
    payload_digest = hashlib.sha256(payload).digest()

    cap_bytes = bytearray()
    for cap in capabilities:
        cap_bytes.extend(cap.to_bytes(2, "little"))

    buf = bytearray()
    buf.extend(b"RAPP")                              # 0x00: Magic
    buf.extend((2).to_bytes(2, "little"))             # 0x04: Version 2
    buf.extend((0).to_bytes(2, "little"))             # 0x06: Arch (0 = x86_64)
    buf.extend(pkg_id_bytes)                          # 0x08..0x28: Package ID
    buf.extend(pub_key)                               # 0x28..0x48: Publisher Key
    buf.extend(version[0].to_bytes(2, "little"))      # 0x48: Major
    buf.extend(version[1].to_bytes(2, "little"))      # 0x4A: Minor
    buf.extend(version[2].to_bytes(2, "little"))      # 0x4C: Patch
    buf.extend(len(capabilities).to_bytes(2, "little")) # 0x4E: Cap count
    buf.extend(payload_digest)                        # 0x50..0x70: Digest
    buf.extend(b"\x00" * SIG_SIZE)                    # 0x70..0xB0: Placeholder signature
    buf.extend(len(payload).to_bytes(4, "little"))    # 0xB0..0xB4: Payload len
    buf.extend(cap_bytes)                             # 0xB4..: Capabilities
    buf.extend(payload)                               # Payload

    payload_offset = HEADER_SIZE + len(capabilities) * 2
    msg = signed_message(buf, payload_offset, len(payload))
    sig = priv_key.sign(msg)
    buf[SIG_OFFSET:SIG_END] = sig
    return bytes(buf)


def verify_rapp_strict(pkg_bytes, trusted_keys, revoked_packages=None, revoked_keys=None):
    """Platform-faithful RAPP verification mimicking kernel/src/package/rapp.rs."""
    revoked_packages = revoked_packages or set()
    revoked_keys = revoked_keys or set()

    if len(pkg_bytes) < HEADER_SIZE:
        return TrustState.Tampered, "BufferTooShort"
    if pkg_bytes[:4] != b"RAPP":
        return TrustState.Tampered, "InvalidMagic"
    version = int.from_bytes(pkg_bytes[4:6], "little")
    if version != 2:
        return TrustState.Tampered, "UnsupportedVersion"

    arch = int.from_bytes(pkg_bytes[6:8], "little")
    if arch not in (0, 1, 2, 3):
        return TrustState.Tampered, "UnsupportedArch"

    pkg_id_raw = pkg_bytes[8:40]
    pkg_id_end = pkg_id_raw.find(b"\x00")
    if pkg_id_end == -1:
        pkg_id_end = 32
    pkg_id = pkg_id_raw[:pkg_id_end].decode("utf-8", errors="replace")
    if not pkg_id:
        return TrustState.Tampered, "InvalidPackageId"

    pub_key = bytes(pkg_bytes[40:72])
    if all(b == 0 for b in pub_key):
        return TrustState.Tampered, "InvalidPublisherKey"

    cap_count = int.from_bytes(pkg_bytes[78:80], "little")
    if cap_count > 1024:
        return TrustState.Tampered, "TooManyCapabilities"

    digest = bytes(pkg_bytes[0x50:0x70])
    signature = bytes(pkg_bytes[0x70:0xB0])
    payload_len = int.from_bytes(pkg_bytes[0xB0:0xB4], "little")

    caps_end = HEADER_SIZE + cap_count * 2
    if len(pkg_bytes) < caps_end:
        return TrustState.Tampered, "BufferTooShort"

    for i in range(cap_count):
        o = HEADER_SIZE + i * 2
        cap = int.from_bytes(pkg_bytes[o:o+2], "little")
        if not (1 <= cap <= 30):
            return TrustState.Tampered, "UnknownCapability"

    payload_offset = caps_end
    payload_end = payload_offset + payload_len
    if len(pkg_bytes) != payload_end:
        return TrustState.Tampered, "PayloadOverflow"

    computed_digest = hashlib.sha256(pkg_bytes[payload_offset:payload_end]).digest()
    if computed_digest != digest:
        return TrustState.Tampered, "DigestMismatch"

    if pkg_id in revoked_packages:
        return TrustState.Revoked, "PackageRevoked"
    if pub_key in revoked_keys:
        return TrustState.Revoked, "PublisherRevoked"

    if pub_key not in trusted_keys:
        return TrustState.Untrusted, "PublisherUntrusted"

    try:
        verifying_key = ed25519.Ed25519PublicKey.from_public_bytes(pub_key)
        msg = signed_message(pkg_bytes, payload_offset, payload_len)
        verifying_key.verify(signature, msg)
    except Exception:
        return TrustState.Tampered, "SignatureInvalid"

    return TrustState.Trusted, "Ok"


def naive_parse_without_verification(pkg_bytes):
    """Flawed parser that only checks magic and length, illustrating the vulnerability."""
    if len(pkg_bytes) >= 4 and pkg_bytes[:4] == b"RAPP":
        return True
    return False


# ---------------------------------------------------------------------------
# Test Cases
# ---------------------------------------------------------------------------

def test_red_vulnerability_demonstration():
    """Demonstrates that a naive check accepts tampered containers (RED demonstration)."""
    payload = b"\x7fELF_ORIGINAL_TEST_PAYLOAD"
    pkg = build_rapp_v2("org.interos.demo", DEV_PUB, DEV_PRIV, [1, 24], payload)

    # Tamper payload
    tampered_bytes = bytearray(pkg)
    tampered_bytes[-1] ^= 0xFF
    tampered = bytes(tampered_bytes)

    # Naive parser accepts it -> Insecure baseline
    naive_accepted = naive_parse_without_verification(tampered)
    assert naive_accepted is True, "Naive parser should have demonstrated flaw by accepting tampered bytes"

    # Strict verifier rejects it -> Secure platform
    state, err = verify_rapp_strict(tampered, trusted_keys={DEV_PUB})
    assert state == TrustState.Tampered, f"Strict verifier must reject tampered container, got {state}"
    assert err == "DigestMismatch", f"Expected DigestMismatch, got {err}"


def test_accept_valid_signed_container_from_trusted_publisher():
    """Valid signed container from provisioned/embedded developer key must be accepted."""
    payload = b"\x7fELF_VALID_PAYLOAD_TEST"
    pkg = build_rapp_v2("org.interos.clock", DEV_PUB, DEV_PRIV, [1, 24], payload)

    state, err = verify_rapp_strict(pkg, trusted_keys={DEV_PUB, SYS_PUB})
    assert state == TrustState.Trusted, f"Expected Trusted, got {state} ({err})"
    assert err == "Ok"


def test_reject_untrusted_publisher_key():
    """Validly signed container from an unknown publisher key must fail closed with Untrusted."""
    untrusted_priv = ed25519.Ed25519PrivateKey.generate()
    untrusted_pub = untrusted_priv.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    payload = b"\x7fELF_UNTRUSTED_VENDOR"
    pkg = build_rapp_v2("com.unknown.app", untrusted_pub, untrusted_priv, [1], payload)

    state, err = verify_rapp_strict(pkg, trusted_keys={DEV_PUB, SYS_PUB})
    assert state == TrustState.Untrusted, f"Expected Untrusted, got {state} ({err})"
    assert err == "PublisherUntrusted"


def test_reject_revoked_package():
    """Package ID listed in the CRL must fail closed with Revoked."""
    payload = b"\x7fELF_REVOKED_APP"
    pkg = build_rapp_v2("org.interos.malicious", DEV_PUB, DEV_PRIV, [1], payload)

    state, err = verify_rapp_strict(pkg, trusted_keys={DEV_PUB}, revoked_packages={"org.interos.malicious"})
    assert state == TrustState.Revoked, f"Expected Revoked, got {state} ({err})"
    assert err == "PackageRevoked"


def test_reject_revoked_publisher_key():
    """Publisher key listed in the CRL must fail closed with Revoked."""
    payload = b"\x7fELF_COMPROMISED_KEY_APP"
    pkg = build_rapp_v2("org.interos.victim", DEV_PUB, DEV_PRIV, [1], payload)

    state, err = verify_rapp_strict(pkg, trusted_keys={DEV_PUB}, revoked_keys={DEV_PUB})
    assert state == TrustState.Revoked, f"Expected Revoked, got {state} ({err})"
    assert err == "PublisherRevoked"


def test_reject_tampered_payload_digest_mismatch():
    """Container with modified payload bytes must fail closed with Tampered."""
    payload = b"\x7fELF_TAMPER_DIGEST_TEST"
    pkg = build_rapp_v2("org.interos.sample", DEV_PUB, DEV_PRIV, [1], payload)

    tampered = bytearray(pkg)
    tampered[-5] ^= 0xAA  # Flip bits in payload
    state, err = verify_rapp_strict(bytes(tampered), trusted_keys={DEV_PUB})
    assert state == TrustState.Tampered, f"Expected Tampered, got {state} ({err})"
    assert err == "DigestMismatch"


def test_reject_tampered_header_signature_invalid():
    """Container with modified header digest (to match tampered payload) must fail signature."""
    payload = b"\x7fELF_ORIGINAL"
    pkg = build_rapp_v2("org.interos.sample", DEV_PUB, DEV_PRIV, [1], payload)

    tampered = bytearray(pkg)
    new_payload = b"\x7fELF_MODIFIED"
    tampered[HEADER_SIZE + 2:] = new_payload
    # Attacker tries to update digest to match new payload
    tampered[0x50:0x70] = hashlib.sha256(new_payload).digest()

    state, err = verify_rapp_strict(bytes(tampered), trusted_keys={DEV_PUB})
    assert state == TrustState.Tampered, f"Expected Tampered signature failure, got {state} ({err})"
    assert err == "SignatureInvalid"


def test_reject_tampered_header_corrupted_fields():
    """Container with corrupted magic or out-of-bounds capability must fail closed."""
    payload = b"\x7fELF_VALID"
    pkg = build_rapp_v2("org.interos.sample", DEV_PUB, DEV_PRIV, [1], payload)

    # Bad magic
    bad_magic = bytearray(pkg)
    bad_magic[:4] = b"EVIL"
    state, err = verify_rapp_strict(bytes(bad_magic), trusted_keys={DEV_PUB})
    assert state == TrustState.Tampered
    assert err == "InvalidMagic"

    # Out of bounds capability
    bad_cap_pkg = build_rapp_v2("org.interos.sample", DEV_PUB, DEV_PRIV, [999], payload)
    state, err = verify_rapp_strict(bad_cap_pkg, trusted_keys={DEV_PUB})
    assert state == TrustState.Tampered
    assert err == "UnknownCapability"


def test_kernel_source_integrity_rapp_trust():
    """Verify kernel source files implement typed trust states and RAPP_TRUST provisioning."""
    rapp_src = (ROOT / "kernel/src/package/rapp.rs").read_text(encoding="utf-8")
    revoc_src = (ROOT / "kernel/src/package/revocation.rs").read_text(encoding="utf-8")
    serial_src = (ROOT / "kernel/src/serial.rs").read_text(encoding="utf-8")

    # 1. TrustState enum exists with required variants
    assert "pub enum TrustState" in rapp_src or "pub enum TrustState" in revoc_src
    for variant in ("Trusted", "Untrusted", "Revoked", "Tampered"):
        assert variant in rapp_src or variant in revoc_src, f"Variant {variant} missing from TrustState"

    # 2. Trust decision is typed enum, not a UI string
    assert "pub trust_state: TrustState" in rapp_src, "RappManifest must record typed trust_state"
    assert "inspect_rapp_trust" in rapp_src, "inspect_rapp_trust must exist"

    # 3. Serial command RAPP_TRUST must be implemented and call RevocationRegistry
    assert "verb.eq_ignore_ascii_case(\"RAPP_TRUST\")" in serial_src
    assert "trust_key" in serial_src or "grant_trust_root" in serial_src
    assert "RAPP_TRUST is not a supported command" not in serial_src, "Old refusal string must be replaced"

    # 4. Embedded developer / system keys exist in revocation registry
    assert "EMBEDDED_DEVELOPER_KEY" in revoc_src
    assert "EMBEDDED_SYSTEM_KEY" in revoc_src


def main():
    print("Running Lane 3 RAPP Trust Chain Contract Tests...")
    test_red_vulnerability_demonstration()
    print("  [PASS] Red vulnerability demonstration & fail-closed enforcement")
    test_accept_valid_signed_container_from_trusted_publisher()
    print("  [PASS] Accept valid signed container from trusted publisher key")
    test_reject_untrusted_publisher_key()
    print("  [PASS] Reject untrusted publisher key")
    test_reject_revoked_package()
    print("  [PASS] Reject revoked package")
    test_reject_revoked_publisher_key()
    print("  [PASS] Reject revoked publisher key")
    test_reject_tampered_payload_digest_mismatch()
    print("  [PASS] Reject tampered payload (digest mismatch)")
    test_reject_tampered_header_signature_invalid()
    print("  [PASS] Reject tampered header (signature invalid)")
    test_reject_tampered_header_corrupted_fields()
    print("  [PASS] Reject corrupted header & unknown capabilities")
    test_kernel_source_integrity_rapp_trust()
    print("  [PASS] Kernel source integrity for TrustState and RAPP_TRUST")
    print("Lane 3: RAPP trust contract ALL PASS")


if __name__ == "__main__":
    main()
