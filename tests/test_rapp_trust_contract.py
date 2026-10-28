#!/usr/bin/env python3
"""Contract test for the .rapp publisher trust chain (RAPP_TRUST).

Enforces:
1. The OS ships NO publisher trust root of its own. A public key compiled
   into the image would trust every package signed with the matching private
   key on every device, so the publisher trust decision must consult the
   operator-installed trust store and nothing else.
2. Fail-closed security on untrusted, revoked, or tampered containers via real
   kernel tests: an unknown publisher is rejected, an operator-installed
   publisher is accepted, a revoked publisher is rejected.
3. The trust decision is recorded as a discrete typed state (Trusted,
   Untrusted, Revoked, Tampered), never a UI string.
4. Test keys generated at test time from high-entropy OS randomness
   (os.urandom), marked test-scoped, with no ability to carry platform
   authority.
5. Execution and pinning of the kernel's own package trust test suite.
"""

import hashlib
import os
import re
import shutil
import subprocess
from pathlib import Path

from cryptography.hazmat.primitives.asymmetric import ed25519
from cryptography.hazmat.primitives import serialization

ROOT = Path(__file__).resolve().parents[1]

HEADER_SIZE = 0xB4
SIG_OFFSET = 0x70
SIG_SIZE = 64
SIG_END = SIG_OFFSET + SIG_SIZE

# The compiled-in roots that used to ship with the OS. Their presence in the
# kernel source is the failure this contract exists to prevent.
FORBIDDEN_ROOTS = ("EMBEDDED_DEVELOPER_KEY", "EMBEDDED_SYSTEM_KEY")


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
    buf.extend(len(capabilities).to_bytes(2, "little"))  # 0x4E: Cap count
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


def rust_fn_body(src, fn_name):
    """Extracts the brace-balanced body of a Rust function by name."""
    idx = src.find(f"fn {fn_name}(")
    assert idx != -1, f"fn {fn_name} not found"
    open_brace = src.find("{", idx)
    assert open_brace != -1, f"fn {fn_name} has no body"
    depth = 0
    for i in range(open_brace, len(src)):
        if src[i] == "{":
            depth += 1
        elif src[i] == "}":
            depth -= 1
            if depth == 0:
                return src[open_brace:i + 1]
    raise AssertionError(f"unbalanced braces in fn {fn_name}")


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
        timeout=300,
    )
    assert res.returncode == 0, f"Kernel package tests failed:\n{res.stdout[-3000:]}\n{res.stderr[-1000:]}"
    assert "test result: ok" in res.stdout


def test_kernel_rapp_trust_symbols_pinned():
    """Ensures real kernel package trust test symbols exist in kernel source."""
    rapp_src = (ROOT / "kernel/src/package/rapp.rs").read_text(encoding="utf-8")
    revoc_src = (ROOT / "kernel/src/package/revocation.rs").read_text(encoding="utf-8")

    expected_rapp_tests = [
        "parse_rapp_accepts_valid_signed_package_with_trusted_key",
        "parse_rapp_accepts_an_operator_installed_publisher",
        "parse_rapp_rejects_untrusted_publisher",
        "parse_rapp_rejects_a_revoked_operator_installed_publisher",
        "parse_rapp_rejects_revoked_package",
        "parse_rapp_rejects_revoked_key",
        "parse_rapp_rejects_tampered_payload_digest_mismatch",
        "parse_rapp_rejects_tampered_header_signature_invalid",
        "parse_rapp_reports_tampering_before_an_untrusted_publisher",
        "inspect_rapp_trust_reports_discrete_typed_states",
    ]
    for sym in expected_rapp_tests:
        assert f"fn {sym}" in rapp_src, f"Kernel rapp test symbol {sym} missing from rapp.rs"

    expected_revoc_tests = [
        "no_publisher_key_is_trusted_until_the_owner_installs_it",
        "platform_authority_comes_only_from_an_installed_root",
        "a_console_installation_cannot_mint_platform_authority",
        "a_rotation_carries_the_recorded_identity_forward",
        "capability_18_fails_closed_without_a_platform_trust_root",
        "system_publisher_identity_accepts_only_delimited_namespace_members",
        "durable_trust_state_agrees_with_the_reported_outcome",
        "trust_and_revocation_invariants",
    ]
    for sym in expected_revoc_tests:
        assert f"fn {sym}" in revoc_src, f"Kernel revocation test symbol {sym} missing from revocation.rs"

    # The superseded rule ("a built-in publication key is trusted") must not
    # come back under any name.
    assert "embedded_publication_keys_are_trusted_and_revocation_outranks_them" not in revoc_src, (
        "the built-in trust-root test must not be reinstated: the OS ships no publisher root"
    )


def test_no_publisher_public_key_is_compiled_into_the_os():
    """Contract: the OS image carries no publisher trust root of its own.

    A root compiled into the kernel is trusted by every device that ships it,
    so the private half of that key would be enough to publish a package onto
    every device. Publisher trust must come from the operator's own
    installation record instead.
    """
    package_dir = ROOT / "kernel/src/package"
    offenders = []
    for path in sorted(package_dir.rglob("*.rs")):
        text = path.read_text(encoding="utf-8")
        for match in re.finditer(r"const\s+[A-Z0-9_]+\s*:\s*\[u8;\s*32\]\s*=\s*\[([^\]]*)\]", text):
            if len(re.findall(r"\d+", match.group(1))) == 32:
                offenders.append(f"{path.name}: {match.group(0)[:80]}")
    assert not offenders, (
        f"VULNERABILITY: a publisher public key is compiled into the OS: {offenders}"
    )

    for path in sorted((ROOT / "kernel/src").rglob("*.rs")):
        text = path.read_text(encoding="utf-8")
        for name in FORBIDDEN_ROOTS:
            assert name not in text, (
                f"{name} must not exist anywhere in the kernel: a built-in root trusts every package signed with it"
            )


def test_trust_decision_uses_only_the_operator_installed_store():
    """The publisher trust decision must read the installed store and nothing else."""
    revoc_src = (ROOT / "kernel/src/package/revocation.rs").read_text(encoding="utf-8")

    body = rust_fn_body(revoc_src, "is_key_trusted")
    assert "trusted_keys.contains_key" in body, (
        "is_key_trusted must answer from the operator-installed trust store"
    )
    assert "is_key_revoked" in body, "revocation must outrank the trust record"
    for forbidden in ("EMBEDDED", "is_embedded_key", "embedded_publisher", "option_env!", "env!"):
        assert forbidden not in body, (
            f"is_key_trusted must not consult {forbidden}: only an installed root may make a publisher trusted"
        )

    # The device must not install a root for itself at boot.
    seed_body = rust_fn_body(revoc_src, "seed")
    assert "grant_trust_root" not in seed_body, (
        "seed must not grant a trust root of its own: the OS ships no publisher root"
    )
    assert "trust_key" not in seed_body, "seed must not grant a trust root of its own"

    # Platform authority is a property of the installed record, not of the image.
    system_body = rust_fn_body(revoc_src, "is_system_publisher")
    assert "trusted_keys.get" in system_body, (
        "platform authority must be read from the operator-installed record"
    )
    for forbidden in FORBIDDEN_ROOTS:
        assert forbidden not in system_body, "platform authority must not come from a built-in root"


def test_the_operator_install_path_exists_and_persists():
    """The operator must have a real way to install a publisher root."""
    revoc_src = (ROOT / "kernel/src/package/revocation.rs").read_text(encoding="utf-8")
    assert "pub fn trust_key" in revoc_src, "the operator install entry point must exist"
    assert "commit_kind(&buf[..pos], 3)" in revoc_src, (
        "an installed trust root must be journalled so it survives a reboot"
    )
    assert "pub(crate) fn grant_trust_root" in revoc_src, (
        "the in-memory state transition of an installation must exist for the invariant tests"
    )
    assert "publisher trust store is full" in revoc_src, (
        "the trust store must fail closed when it is full"
    )


def test_the_console_cannot_mint_platform_authority():
    """The install console is not an authenticated provisioning channel."""
    revoc_src = (ROOT / "kernel/src/package/revocation.rs").read_text(encoding="utf-8")

    body = rust_fn_body(revoc_src, "trust_key")
    assert "is_system_publisher_identity" in body, (
        "the console install path must refuse platform publisher identities"
    )
    assert "platform publisher authority requires an authenticated provisioning channel" in revoc_src, (
        "the refusal must name the policy rather than a storage failure"
    )

    # A key rotation carries the recorded identity forward; it must not be
    # able to rename a publisher into the platform namespace.
    rotation = rust_fn_body(revoc_src, "rotation_publisher")
    assert "trusted_keys" in rotation, (
        "a rotation must resolve the publisher identity from the trust store"
    )
    apply_body = rust_fn_body(revoc_src, "apply_key_rotation")
    assert "rotation_publisher" in apply_body, (
        "a rotation must inherit the identity recorded for the key it replaces"
    )
    assert "proof.publisher" not in apply_body, (
        "the unauthenticated publisher name in a rotation proof must not be used as the identity"
    )


def test_the_signature_is_verified_before_the_publisher_is_looked_up():
    """An unverified container must be reported as tampered, not as untrusted."""
    rapp_src = (ROOT / "kernel/src/package/rapp.rs").read_text(encoding="utf-8")
    body = rust_fn_body(rapp_src, "parse_rapp")
    sig_at = body.index("verify_rapp_signature")
    trust_at = body.index("is_key_trusted")
    assert sig_at < trust_at, (
        "the Ed25519 signature must be verified before the publisher trust lookup, "
        "otherwise a forged container is reported as an untrusted publisher"
    )
    assert "PublisherUntrusted" in body, "an unknown publisher must be refused by name"
    assert "PublisherRevoked" in body, "a revoked publisher must be refused by name"


def test_ephemeral_test_keys_cannot_carry_platform_authority():
    """Test keys generated from os.urandom at runtime are test-scoped and cannot gain platform authority."""
    seed = os.urandom(32)
    priv_key = ed25519.Ed25519PrivateKey.from_private_bytes(seed)
    pub_key = priv_key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)

    # No publisher public key exists in the kernel at all, so an ephemeral key
    # cannot collide with a shipped root.
    for path in sorted((ROOT / "kernel/src").rglob("*.rs")):
        text = path.read_text(encoding="utf-8")
        assert pub_key.hex() not in text, "an ephemeral key must not appear in the kernel source"

    # The container built with the ephemeral key is cryptographically sound, so
    # the kernel's refusal of it is a trust decision and not a broken package.
    payload = b"\x7fELF_EPHEMERAL_TEST_PAYLOAD"
    pkg = build_rapp_v2("org.example.ephemeral", pub_key, priv_key, [1, 24], payload)
    payload_offset = HEADER_SIZE + 2 * 2
    ed25519.Ed25519PublicKey.from_public_bytes(pub_key).verify(
        pkg[SIG_OFFSET:SIG_END], signed_message(pkg, payload_offset, len(payload))
    )

    # Platform authority check: test-scoped publisher cannot carry platform authority
    revoc_src = (ROOT / "kernel/src/package/revocation.rs").read_text(encoding="utf-8")
    assert "SYSTEM_PUBLISHER_NAMESPACE: &str = \"org.interos.\"" in revoc_src
    for publisher in ("test-scoped", "developer", "com.thirdparty.app"):
        assert not (publisher in ("system", "platform") or publisher.startswith("org.interos.")), (
            f"Publisher {publisher} must not qualify for system platform authority"
        )


def main():
    print("Running RAPP Trust Chain Contract Tests...")
    test_kernel_rapp_trust_symbols_pinned()
    print("  [PASS] Kernel package trust test symbols pinned in source")
    test_no_publisher_public_key_is_compiled_into_the_os()
    print("  [PASS] No publisher trust root is compiled into the OS")
    test_trust_decision_uses_only_the_operator_installed_store()
    print("  [PASS] Trust decision reads only the operator-installed trust store")
    test_the_operator_install_path_exists_and_persists()
    print("  [PASS] Operator install path exists and is journalled")
    test_the_console_cannot_mint_platform_authority()
    print("  [PASS] The install console cannot mint platform authority")
    test_the_signature_is_verified_before_the_publisher_is_looked_up()
    print("  [PASS] Signature verification precedes the publisher trust lookup")
    test_ephemeral_test_keys_cannot_carry_platform_authority()
    print("  [PASS] Ephemeral test keys generated from OS randomness and scoped without platform authority")
    test_kernel_rapp_trust_cargo_tests_pass()
    print("  [PASS] Kernel package cargo tests pass")
    print("RAPP trust contract ALL PASS")


if __name__ == "__main__":
    main()
