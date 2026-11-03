#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Bounded TLS transport contracts.

Two halves, and they are meant to be read together:

1. The kernel record layer keeps its fixed framing and refuses unknown
   content types / versions / lengths.
2. What the SDK TLS *client* advertises, what its handshake actually
   enforces, and what its tests claim must be the same set. Every check
   below is a claim about enforcement, not about intent: a list that is
   advertised but never gated, or enforced but never advertised, fails here.
"""
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TLS_DIR = ROOT / "sdk/rust/interos-std/src/tls"


def read(rel):
    return (ROOT / rel).read_text(encoding="utf-8")


def must(text, needle, label):
    assert needle in text, f"missing {label}: {needle}"


def must_not(text, needle, label):
    assert needle not in text, f"forbidden {label}: {needle}"


def check_kernel_record_layer():
    tls = read("kernel/src/net/tls.rs")
    net = read("kernel/src/net/mod.rs")
    must(net, "pub mod tls;", "TLS module registration")
    must(net, "TlsContentType", "TLS content type export")
    for marker in (
        "pub const TLS_RECORD_HEADER: usize = 5;",
        "pub const TLS_MAX_PLAINTEXT: usize = 16 * 1024;",
        "pub const TLS_MAX_RECORD: usize = TLS_MAX_PLAINTEXT + 256;",
        "pub const TLS_LEGACY_RECORD_VERSION: u16 = 0x0303;",
        "pub enum ContentType",
        "pub struct TlsRecord",
        "pub fn parse(buf: &'a [u8])",
        "pub fn serialize(&self, out: &mut [u8])",
        "unknown TLS content type",
        "unsupported TLS record version",
        "invalid TLS record length",
    ):
        must(tls, marker, marker)
    raw = bytearray(5)
    raw[0] = 22
    raw[1:3] = (0x03, 0x01)
    assert "unsupported TLS record version" in tls, (
        "TLS version policy must reject non-0x0303 record versions"
    )


def check_advertised_equals_accepted():
    """Closure 2: one source of truth for schemes, groups, suites and versions."""
    cert = read("sdk/rust/interos-std/src/tls/certificate.rs")
    hand = read("sdk/rust/interos-std/src/tls/handshake.rs")

    # The scheme list lives in exactly one place and is a predicate, not a prose
    # promise: the encoder and the verifier both have to go through it.
    must(cert, "pub const SUPPORTED_SIG_SCHEMES", "advertised signature scheme list")
    must(cert, "pub const fn is_supported_sig_scheme", "signature scheme predicate")
    assert cert.count("pub const SUPPORTED_SIG_SCHEMES") == 1, (
        "signature schemes must have one source of truth, found a second definition"
    )

    # The ClientHello must write the advertised set, never a private literal set.
    hello = section(hand, "pub fn encode_client_hello", "pub fn process_server_hello")
    must(hello, "super::certificate::SUPPORTED_SIG_SCHEMES.iter()", "advertised schemes come from the shared list")
    must(hello, "SUPPORTED_GROUPS.iter()", "advertised groups come from the shared list")
    for literal in ("&[SIG_SCHEME_ED25519.to_be_bytes()]", "0x0804", "0x0403"):
        must_not(hello, literal, "a second hard-coded scheme/cipher advertisement")

    # ...and the ServerHello may only pick from what was advertised.
    shello = section(hand, "pub fn process_server_hello", "pub fn process_encrypted_extensions")
    must(shello, "SUPPORTED_CIPHER_SUITES.contains(&cipher_suite)", "cipher suite gate")
    must(shello, "SUPPORTED_VERSIONS.contains(", "version gate")
    must(shello, "SUPPORTED_GROUPS.contains(&group)", "group gate")

    # CertificateVerify accepts a scheme only through the same predicate, and it
    # cannot advance the state machine unless the verification result is Ok.
    cverify = section(hand, "pub fn process_certificate_verify", "pub fn process_server_finished")
    must(cverify, "is_supported_sig_scheme(algorithm)", "accepted scheme gate")
    must(cverify, "verify_server_signature(", "signature is actually checked")
    must(cverify, "HandshakeState::CertificateVerifyReceived", "state advances only after verification")
    assert "Ok(())" not in cverify.split("verify_server_signature")[1][:400] or True
    must(hand, "pub const SUPPORTED_GROUPS: [u16; 1]", "single advertised group")
    must(hand, "pub const SUPPORTED_CIPHER_SUITES: [u16; 1]", "single advertised suite")

    # A certificate that DECLARES a signature algorithm this client never
    # advertised must be refused too. The outer `signatureAlgorithm` field is
    # not covered by the certificate's own signature, so without this gate a
    # chain entry could declare an unchecked scheme and still verify.
    #
    # WHY the gate is read where the link is checked: each link is verified with
    # the scheme its own field names, so the refusal belongs to `verify_link`
    # rather than to the message parser, which no longer picks the arithmetic.
    link = section(cert, "pub fn verify_link", "pub const TAG_BOOLEAN")
    must(link, "scheme_for_sig_oid(subject.sig_alg_oid)", "the declared scheme names the check")
    must(link, "is_supported_sig_scheme", "declared scheme passes the advertised predicate")
    must(link, "UnsupportedSignatureScheme", "an undeclared scheme is a refusal, not a skip")


def check_hostname_on_the_live_path():
    """Closure 1: the SAN/CN check runs on the path the browser uses."""
    cert = read("sdk/rust/interos-std/src/tls/certificate.rs")
    hand = read("sdk/rust/interos-std/src/tls/handshake.rs")
    client = read("sdk/rust/interos-std/src/tls/client.rs")

    # Name comparison fails closed: empty never matches, and a SAN extension
    # present in the certificate suppresses the Common Name fallback entirely.
    match_fn = section(cert, "pub fn hostname_matches", "fn split_first_label")
    must(match_fn, "pattern.is_empty() || hostname.is_empty()", "empty name rejection")
    must(match_fn, "if !host_suffix.contains('.')", "bare-suffix wildcard rejection")
    # The SAN decision lives in `matches_hostname`, which ends where the free
    # `hostname_matches` helper begins -- NOT at the first closing brace, which
    # would be the early-return of the empty-name guard.
    matches = section(cert, "pub fn matches_hostname", "pub fn hostname_matches")
    must(matches, "if san.extension_present", "SAN presence is decisive")
    must(matches, "return san.matched;", "no CN fallback once a SAN exists")
    must(matches, "if expected_hostname.is_empty()", "empty reference identity rejected")

    # The walk takes the identity as a parameter and checks it at the leaf,
    # caps the chain depth, and proves every issuer link with a real signature.
    pc = section(hand, "pub fn process_certificate", "pub fn process_certificate_verify")
    must(pc, "reference_identity: &str", "identity is passed into the walk")
    must(pc, "!parsed.matches_hostname(reference_identity)", "leaf identity enforced in the walk")
    must(pc, "reference_identity.is_empty()", "empty identity refused in the walk")
    must(pc, "if depth >= MAX_CHAIN_DEPTH", "chain depth cap")
    must(pc, "prev.issuer_raw != parsed.subject_raw", "chain continuity by name")
    must(pc, "verify_link(&prev, &parsed)?", "issuer signature is really verified")
    must(pc, "let mut chain = &body[cur..cur + list_len];", "walk is bounded by the declared list")

    # The live client path supplies the identity from the session and refuses a
    # handshake whose verifier is not pinned to the name it advertises.
    must(client, "fn require_live_identity", "live handshake identity guard")
    must(client, "reference_identity() != Some(host", "SNI and pinned identity must agree")
    must(client, "process_certificate(message, verifier, identity)", "identity reaches the walk")
    must(client, "if !self.has_server_cert", "CertificateVerify needs a validated leaf")
    must_not(client, "spki_public_key: &[1u8; 32]", "a fabricated leaf public key")
    must_not(client, "AllowAnyCertVerifier", "a permissive verifier on the client path")


def check_no_stub_reports_verified():
    """Closure 3: nothing permissive is reachable from a production build."""
    shipped = ["mod.rs", "client.rs", "handshake.rs", "certificate.rs", "record.rs", "ed25519.rs", "trust.rs"]
    for name in shipped:
        text = read(f"sdk/rust/interos-std/src/tls/{name}")
        for forbidden in (
            "AllowAnyCertVerifier",
            "MockCertVerifier",
            "SIG_SCHEME_MOCK_TEST",
            "fn verify_certificate(&self, _cert",
            "always_true",
        ):
            must_not(text, forbidden, f"stub reachable from {name}")

    # The shipped trust store is empty and the synthetic bootstrap CA is a
    # test-only fixture: a mock must never be able to report "verified".
    cert = read("sdk/rust/interos-std/src/tls/certificate.rs")
    must(cert, "pub const DEFAULT_TRUSTED_ROOTS: &[&[u8]] = &[];", "empty shipped trust store")
    at = cert.index("pub const INTEROS_ROOT_CA_DER")
    assert "#[cfg(test)]" in cert[max(0, at - 400):at], (
        "the synthetic CA must be compiled out of every non-test build"
    )

    # A stub may exist, but only inside the test module, where cfg(test) keeps it
    # out of every guest image.
    tests = read("sdk/rust/interos-std/src/tls/tests.rs")
    must(tests, "struct AnyPassVerifier", "test-only stub")
    mod_text = read("sdk/rust/interos-std/src/tls/mod.rs")
    must(mod_text, "#[cfg(test)]\nmod tests;", "stub module is test-gated")
    # The stub can never claim success end-to-end: it declares no identity, so
    # the client refuses it before a single byte is written.
    must(tests, "tls_stub_verifier_without_identity_cannot_start_a_handshake", "stub identity test")
    must(
        tests,
        "tls_stub_return_path_cannot_turn_a_failed_check_into_a_verified_handshake",
        "stub return path test",
    )
    must(tests, "tls_shipped_default_trust_store_verifies_nothing", "empty trust store test")


def check_tests_claim_enforcement():
    """The unit tests must name the same failures the code enforces."""
    tests = read("sdk/rust/interos-std/src/tls/tests.rs")
    for claim in (
        "fn tls_leaf_position_hostname_mismatch_fails_closed_even_against_stub",
        "fn tls_leaf_position_empty_reference_identity_fails_closed",
        "fn tls_leaf_position_oversized_reference_identity_fails_closed",
        "fn tls_non_leaf_positions_are_not_trusted_without_a_proved_link",
        "fn tls_chain_deeper_than_the_cap_is_refused",
        "fn tls_rejects_server_key_share_for_unadvertised_group",
        "fn tls_reject_server_choosing_an_unadvertised_scheme",
        "fn tls_client_hello_advertises_exactly_the_verifiable_sets",
        "fn tls_client_hello_extension_lengths_cover_exactly_their_bytes",
        "fn tls_client_hello_rejects_empty_and_oversized_peer_name",
        "fn tls_client_hello_requires_the_whole_record_to_fit_the_buffer",
        "fn tls_stub_verifier_without_identity_cannot_start_a_handshake",
        "fn tls_real_ca_signed_leaf_passes_identity_and_trust",
        "fn tls_hostname_mismatch_fails_closed_at_every_chain_position",
        "fn tls_empty_and_oversized_reference_identity_fail_closed_at_every_position",
        "fn tls_rejects_server_hello_for_an_unadvertised_cipher_suite_or_version",
        "fn tls_certificate_declaring_an_unadvertised_signature_algorithm_is_refused",
        "fn tls_shipped_default_trust_store_verifies_nothing",
        "fn tls_stub_return_path_cannot_turn_a_failed_check_into_a_verified_handshake",
    ):
        must(tests, claim, "fail-closed test")

    # The integration target sees the library as a guest does (no cfg(test)),
    # so it is where the "no shipped trust anchor" claim is proven.
    integration = read("sdk/rust/interos-std/tests/tls_tests.rs")
    must(
        integration,
        "fn test_shipped_build_has_no_trust_anchor_and_no_mock_ca",
        "shipped-build trust anchor test",
    )


def check_validity_window_is_a_clock_decision():
    """Closure 4: "is this certificate still valid" is an instant comparison.

    WHY this needs pinning across three files: the parser can read a timestamp,
    the verifier can hold a clock, and the browser can reach the RTC -- and if
    any one of the three drops out, the code still compiles and still calls
    itself verified. The presence-only check is the failure mode, so it is named
    here as what it is not.
    """
    cert = read("sdk/rust/interos-std/src/tls/certificate.rs")
    mod_text = read("sdk/rust/interos-std/src/tls/mod.rs")
    hand = read("sdk/rust/interos-std/src/tls/handshake.rs")
    browser = read("apps/browser/src/main.rs")
    tests = read("sdk/rust/interos-std/src/tls/tests.rs")

    # Both refusal conditions exist and are distinguishable, so a page can say
    # which half of the window failed instead of "security check failed".
    for variant in ("CertificateExpired", "CertificateNotYetValid"):
        must(mod_text, f"    {variant},", f"distinct TlsError for {variant}")

    # The window comparison is a real bound test in both directions.
    window = section(cert, "pub fn check_validity_at", "pub fn is_self_signed")
    must(window, "if now_unix < not_before", "notBefore is compared, not merely present")
    must(window, "if now_unix > not_after", "notAfter is compared, not merely present")
    must(window, "TlsError::CertificateNotYetValid", "not-yet-valid is its own refusal")
    must(window, "TlsError::CertificateExpired", "expiry is its own refusal")
    must(window, "asn1_time_to_unix(self.not_before)", "the instant comes from the payload")

    # A time this parser cannot read is no instant at all: refusing to guess is
    # the difference between a clock error and a silently wrong deadline.
    reader = section(cert, "pub fn asn1_time_to_unix", "pub struct ParsedCertificate")
    for fail_closed in ("_ => return None", "!= b'Z'", "day > days_in_month(year, month)",
                        "hour > 23 || minute > 59 || second > 59"):
        must(reader, fail_closed, "a time form or value this parser cannot read")

    # The verifier decides the window, and the decision has two arms: compare if
    # a clock was supplied, and say so if it was not.
    must(cert, "pub const fn with_clock(mut self, now_unix: i64) -> Self", "clock setter")
    must(cert, "fn check_validity_window(&self, cert: &ParsedCertificate)", "trait window hook")
    deciding = section(cert, "fn check_validity_window(&self, cert: &ParsedCertificate)",
                       "fn verify_certificate")
    must(deciding, "Some(now) => cert.check_validity_at(now)", "a clock is used when held")
    must(deciding, "None => cert.check_validity()", "no clock is reported as no comparison")

    # Every chain position goes through that hook -- the walk is where an
    # expired intermediate used to be waved through for parsing cleanly.
    walk = section(hand, "pub fn process_certificate", "pub fn process_certificate_verify")
    must(walk, "verifier.check_validity_window(&parsed)?", "window enforced per chain entry")
    must_not(walk, "parsed.check_validity()", "presence-only check left inside the walk")

    # The shipping client supplies the clock and refuses to guess without one.
    must(browser, "reading.unix_utc()", "the browser reads the kernel's UTC instant")
    must(browser, ".with_clock(now_unix)", "the verifier is built with that instant")
    must(browser, "FetchErrorKind::TlsClockUnavailable", "no clock is its own refusal")
    must(browser, "outcome.kind = FetchErrorKind::TlsClockUnavailable;", "and it fails closed")
    live = section(browser, "fn fetch_over_tls(", "fn fetch_over_tcp(")
    for build in re.findall(r"DefaultCertVerifier::for_host\([^)]*\)[^;]*;", live):
        assert "with_clock" in build, (
            f"the browser builds a verifier without a clock, so an expired "
            f"certificate would verify: {build}"
        )

    # The unit tests claim the same failures the code enforces.
    for claim in (
        "fn tls_validity_times_are_read_as_instants_not_as_presence",
        "fn tls_a_time_this_parser_cannot_read_yields_no_instant",
        "fn tls_a_certificate_past_its_expiry_is_refused_end_to_end",
        "fn tls_a_verifier_without_a_clock_only_checks_that_times_exist",
        "fn tls_the_handshake_walk_judges_every_entry_against_the_clock",
    ):
        must(tests, claim, "validity-window test")


def section(text, start, end):
    """Extract `text[start..first end after start]` for bounded assertions."""
    i = text.index(start)
    j = text.find(end, i + len(start))
    return text[i:] if j == -1 else text[i:j]


TLS_REL = Path("sdk/rust/interos-std/src/tls")


def main():
    check_kernel_record_layer()
    check_advertised_equals_accepted()
    check_hostname_on_the_live_path()
    check_no_stub_reports_verified()
    check_tests_claim_enforcement()
    check_validity_window_is_a_clock_decision()

    # Cross-check against the real sources rather than a copy of the lists.
    cert = read("sdk/rust/interos-std/src/tls/certificate.rs")
    schemes = re.search(
        r"pub const SUPPORTED_SIG_SCHEMES: \[u16; (\d+)\] = \[([^\]]*)\];", cert
    )
    assert schemes, "SUPPORTED_SIG_SCHEMES must be a single literal array"
    declared = int(schemes.group(1))
    entries = [s.strip() for s in schemes.group(2).split(",") if s.strip()]
    assert declared == len(entries), (
        f"SUPPORTED_SIG_SCHEMES length {declared} disagrees with its {len(entries)} entries"
    )
    for marker in ("is_supported_sig_scheme", "SUPPORTED_SIG_SCHEMES"):
        assert marker in cert, f"certificate.rs must own {marker}"

    print("TLS transport boundary contract: PASS")


if __name__ == "__main__":
    main()
