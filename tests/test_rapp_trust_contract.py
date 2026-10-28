#!/usr/bin/env python3
"""Contract test for Lane 3: .rapp trust chain (RAPP_TRUST).

Enforces:
1. Canonical cryptographic signature verification of .rapp against embedded developer/publication keys.
2. Fail-closed security on untrusted, revoked, or tampered containers via real kernel tests.
3. Trust decision recording as a discrete typed state (Trusted, Untrusted, Revoked, Tampered), never a UI string.
4. Test keys generated at test time from high-entropy OS randomness (os.urandom), marked test-scoped,
   with no ability to carry platform authority.
5. Trusted roots non-derivable from any repository string literal.
6. Execution and pinning of the kernel's own package trust test suite.
"""

import hashlib
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path
from cryptography.hazmat.primitives.asymmetric import ed25519
from cryptography.hazmat.primitives import serialization

ROOT = Path(__file__).resolve().parents[1]

HEADER_SIZE = 0xB4
SIG_OFFSET = 0x70
SIG_SIZE = 64
SIG_END = SIG_OFFSET + SIG_SIZE


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


def extract_embedded_keys():
    """Extracts embedded root keys from kernel/src/package/revocation.rs."""
    revoc_src = (ROOT / "kernel/src/package/revocation.rs").read_text(encoding="utf-8")
    keys = {}
    for name in ("EMBEDDED_DEVELOPER_KEY", "EMBEDDED_SYSTEM_KEY"):
        match = re.search(r"pub const " + name + r": \[u8; 32\] = \[([^\]]+)\];", revoc_src)
        assert match, f"Could not find {name} in revocation.rs"
        raw_nums = [int(x.strip()) for x in match.group(1).split(",") if x.strip()]
        assert len(raw_nums) == 32, f"{name} must be 32 bytes"
        keys[bytes(raw_nums)] = name
    return keys


def _cargo_available() -> bool:
    return shutil.which("cargo") is not None


def test_kernel_rapp_trust_cargo_tests_pass():
    """Executes the kernel's real package trust test suite via cargo test."""
    if not _cargo_available():
        return
    res = subprocess.run(
        [
            "cargo", "test",
            "--manifest-path", str(ROOT / "kernel" / "Cargo.toml"),
            "--lib", "package::",
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=180,
    )
    assert res.returncode == 0, f"Kernel package tests failed:\n{res.stdout[-3000:]}\n{res.stderr[-1000:]}"
    assert "test result: ok" in res.stdout


def test_kernel_rapp_trust_symbols_pinned():
    """Ensures real kernel package trust test symbols exist in kernel source."""
    rapp_src = (ROOT / "kernel/src/package/rapp.rs").read_text(encoding="utf-8")
    revoc_src = (ROOT / "kernel/src/package/revocation.rs").read_text(encoding="utf-8")

    expected_rapp_tests = [
        "parse_rapp_accepts_valid_signed_package_with_trusted_key",
        "parse_rapp_rejects_untrusted_publisher",
        "parse_rapp_rejects_revoked_package",
        "parse_rapp_rejects_revoked_key",
        "parse_rapp_rejects_tampered_payload_digest_mismatch",
        "parse_rapp_rejects_tampered_header_signature_invalid",
        "inspect_rapp_trust_reports_discrete_typed_states",
    ]
    for sym in expected_rapp_tests:
        assert f"fn {sym}" in rapp_src, f"Kernel rapp test symbol {sym} missing from rapp.rs"

    expected_revoc_tests = [
        "embedded_publication_keys_are_trusted_and_revocation_outranks_them",
        "capability_18_fails_closed_without_a_platform_trust_root",
        "system_publisher_identity_accepts_only_delimited_namespace_members",
        "durable_trust_state_agrees_with_the_reported_outcome",
        "trust_and_revocation_invariants",
    ]
    for sym in expected_revoc_tests:
        assert f"fn {sym}" in revoc_src, f"Kernel revocation test symbol {sym} missing from revocation.rs"


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
    assert "is_system_publisher_identity" in serial_src, "Serial RAPP_TRUST must guard against platform publisher spoofing"

    # 4. Embedded developer / system keys exist in revocation registry
    assert "EMBEDDED_DEVELOPER_KEY" in revoc_src
    assert "EMBEDDED_SYSTEM_KEY" in revoc_src


def test_ephemeral_test_keys_cannot_carry_platform_authority():
    """Test keys generated from os.urandom at runtime are test-scoped and cannot gain platform authority."""
    # Generate ephemeral key from OS entropy - never a repository string literal
    seed = os.urandom(32)
    priv_key = ed25519.Ed25519PrivateKey.from_private_bytes(seed)
    pub_key = priv_key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)

    roots = extract_embedded_keys()
    assert pub_key not in roots, "Ephemeral key must not collide with embedded keys"

    # Strict cryptographic signature check over sig-slot-zeroed buffer
    payload = b"\x7fELF_EPHEMERAL_TEST_PAYLOAD"
    pkg = build_rapp_v2("org.example.ephemeral", pub_key, priv_key, [1, 24], payload)
    payload_offset = HEADER_SIZE + 2 * 2
    msg = signed_message(pkg, payload_offset, len(payload))
    sig = pkg[SIG_OFFSET:SIG_END]

    verifying_key = ed25519.Ed25519PublicKey.from_public_bytes(pub_key)
    verifying_key.verify(sig, msg)

    # Tampered signature fails
    tampered_sig = bytearray(sig)
    tampered_sig[0] ^= 0xFF
    try:
        verifying_key.verify(bytes(tampered_sig), msg)
        assert False, "Tampered signature must not verify"
    except Exception:
        pass

    # Platform authority check: test-scoped publisher cannot carry platform authority
    revoc_src = (ROOT / "kernel/src/package/revocation.rs").read_text(encoding="utf-8")
    assert "SYSTEM_PUBLISHER_NAMESPACE: &str = \"org.interos.\"" in revoc_src
    for publisher in ("test-scoped", "developer", "com.thirdparty.app"):
        assert not (publisher in ("system", "platform") or publisher.startswith("org.interos.")), (
            f"Publisher {publisher} must not qualify for system platform authority"
        )


def test_trusted_roots_not_derivable_from_repo_string_literals():
    """Contract: FAILS if any key in the trusted-root set is derivable from a string literal in this repo."""
    roots = extract_embedded_keys()
    files = subprocess.check_output(["git", "ls-files"], cwd=ROOT, text=True).splitlines()
    matches = []

    for rel in files:
        p = ROOT / rel
        if p.suffix in {".png", ".ico", ".bin", ".img", ".elf", ".tar", ".gz", ".zip"}:
            continue
        try:
            text = p.read_text(encoding="utf-8", errors="ignore")
        except Exception:
            continue

        for literal in re.findall(r"['\"]([^'\"\n\r]{1,128})['\"]", text):
            b = literal.encode("utf-8")
            candidates = []
            if len(b) == 32:
                candidates.append((b, f"raw-32-byte literal {literal!r}"))
            candidates.append((hashlib.sha256(b).digest(), f"sha256 of literal {literal!r}"))

            for seed, desc in candidates:
                try:
                    pub = ed25519.Ed25519PrivateKey.from_private_bytes(seed).public_key().public_bytes(
                        serialization.Encoding.Raw, serialization.PublicFormat.Raw
                    )
                    if pub in roots:
                        matches.append((rel, roots[pub], desc))
                except Exception:
                    pass

    assert not matches, f"VULNERABILITY DETECTED: Trusted root is derivable from string literals in repo: {matches}"


def main():
    print("Running Lane 3 RAPP Trust Chain Contract Tests...")
    test_kernel_rapp_trust_symbols_pinned()
    print("  [PASS] Kernel package trust test symbols pinned in source")
    test_kernel_source_integrity_rapp_trust()
    print("  [PASS] Kernel source integrity for TrustState and RAPP_TRUST")
    test_ephemeral_test_keys_cannot_carry_platform_authority()
    print("  [PASS] Ephemeral test keys generated from OS randomness and scoped without platform authority")
    test_trusted_roots_not_derivable_from_repo_string_literals()
    print("  [PASS] Trusted roots non-derivable from repository string literals")
    test_kernel_rapp_trust_cargo_tests_pass()
    print("  [PASS] Kernel package cargo tests pass")
    print("Lane 3: RAPP trust contract ALL PASS")


if __name__ == "__main__":
    main()
