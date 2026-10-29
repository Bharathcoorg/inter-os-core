//! Signature checks over certificates nobody wrote for this test.
//!
//! WHY live chains rather than generated ones: these are the links a browser
//! accepts today — ISRG Root X1 down to a leaf issued this month — so a pass
//! here is evidence about the arithmetic, not about two implementations that
//! agree with each other. The RSA-PSS pair is the one locally made fixture,
//! because a PSS signature appears in a TLS `CertificateVerify` rather than in a
//! certificate, and the salt length it was made with is not carried on the wire.

use interos_std::crypto::{p256, rsa};
use interos_std::tls::certificate::{
    asn1_time_to_unix, decode_ecdsa_signature, parse_rsa_public_key, DefaultCertVerifier,
    ParsedCertificate, EC_PUBLIC_KEY_OID, PRIME256V1_OID, RSA_ENCRYPTION_OID, SHA256_WITH_RSA_OID,
};
use interos_std::tls::handshake::{HandshakeClient, HandshakeState};
use interos_std::tls::TlsError;

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/x509")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn certificate(name: &str) -> Vec<u8> {
    let der = fixture(name);
    ParsedCertificate::parse(&der).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    der
}

/// Verifies `cert` the way a chain walk does: the issuer's public key over the
/// certificate's own `tbsCertificate` bytes.
fn verified_by(issuer_der: &[u8], cert_der: &[u8]) -> bool {
    let issuer = ParsedCertificate::parse(issuer_der).unwrap();
    let cert = ParsedCertificate::parse(cert_der).unwrap();
    assert_eq!(cert.sig_alg_oid, SHA256_WITH_RSA_OID, "fixture drifted");
    assert_eq!(issuer.spki_algorithm_oid, RSA_ENCRYPTION_OID, "fixture drifted");
    let key = parse_rsa_public_key(issuer.spki_public_key).unwrap();
    rsa::verify_pkcs1_sha256(key.modulus, key.exponent, cert.signature_value, cert.tbs_raw)
}

#[test]
fn reads_the_rsa_key_a_certificate_carries() {
    let der = certificate("isrg_root_x1.der");
    let root = ParsedCertificate::parse(&der).unwrap();
    let key = parse_rsa_public_key(root.spki_public_key).unwrap();
    assert_eq!(key.modulus.len(), 512, "ISRG Root X1 is a 4096-bit key");
    assert_eq!(key.exponent, &[0x01, 0x00, 0x01], "65537, as every CA uses");
    // DER pads a high-bit-set INTEGER with a zero byte; the modulus starts 0x00
    // only if that pad survived, which would make the arithmetic one byte off.
    assert_ne!(key.modulus[0], 0);

    let short = certificate("leaf_under_yr2.der");
    let leaf = ParsedCertificate::parse(&short).unwrap();
    let leaf_key = parse_rsa_public_key(leaf.spki_public_key).unwrap();
    assert_eq!(leaf_key.modulus.len(), 256, "the leaf is a 2048-bit key");
}

#[test]
fn verifies_every_link_of_the_public_isrg_chain() {
    let root = fixture("isrg_root_x1.der");
    let cross = fixture("root_yr_under_isrg.der");
    let intermediate = fixture("yr2_under_root_yr.der");
    let leaf = fixture("leaf_under_yr2.der");

    // A root is signed by itself, so this is the same call as the others and
    // covers the self-signed case the trust store relies on.
    assert!(verified_by(&root, &root), "ISRG Root X1's own signature");
    assert!(verified_by(&root, &cross), "YR Root CA signed by ISRG Root X1 (4096-bit)");
    assert!(verified_by(&cross, &intermediate), "YR2 signed by YR Root CA");
    assert!(verified_by(&intermediate, &leaf), "the leaf signed by YR2 (2048-bit)");
}

#[test]
fn refuses_a_certificate_whose_contents_changed() {
    let issuer_der = fixture("yr2_under_root_yr.der");
    let issuer = ParsedCertificate::parse(&issuer_der).unwrap();
    let key = parse_rsa_public_key(issuer.spki_public_key).unwrap();
    let leaf_der = fixture("leaf_under_yr2.der");
    let leaf = ParsedCertificate::parse(&leaf_der).unwrap();

    let mut tampered = leaf.tbs_raw.to_vec();
    // Inside the signed bytes and away from anything the parser treats as
    // structural, so only the signature can be what notices.
    let middle = tampered.len() / 2;
    tampered[middle] ^= 0x01;
    assert!(
        !rsa::verify_pkcs1_sha256(key.modulus, key.exponent, leaf.signature_value, &tampered),
        "an edited certificate still verified"
    );

    // The same edit seen through a re-parse: wherever the DER still loads, the
    // signature over what it now says has to fail.
    let offset = leaf.tbs_raw.as_ptr() as usize - leaf_der.as_ptr() as usize;
    let mut broken_der = leaf_der.clone();
    broken_der[offset + middle] ^= 0x01;
    if let Ok(reparsed) = ParsedCertificate::parse(&broken_der) {
        assert_ne!(reparsed.tbs_raw.as_ptr(), leaf.tbs_raw.as_ptr());
        assert!(
            !rsa::verify_pkcs1_sha256(
                key.modulus,
                key.exponent,
                reparsed.signature_value,
                reparsed.tbs_raw
            ),
            "a re-parsed edited certificate still verified"
        );
    }
}

#[test]
fn refuses_a_signature_that_is_not_the_issuer_s() {
    let root = fixture("isrg_root_x1.der");
    let intermediate = fixture("yr2_under_root_yr.der");
    let leaf = fixture("leaf_under_yr2.der");

    // The leaf was signed by YR2, not by the root: checking against the wrong
    // key must fail even though both are valid RSA keys and the padding around a
    // recovered block is the same shape either way.
    assert!(!verified_by(&root, &leaf));
    assert!(!verified_by(&leaf, &leaf));
    assert!(!verified_by(&leaf, &intermediate));
}

#[test]
fn refuses_a_signature_with_one_bit_changed() {
    let intermediate = fixture("yr2_under_root_yr.der");
    let issuer = ParsedCertificate::parse(&intermediate).unwrap();
    let key = parse_rsa_public_key(issuer.spki_public_key).unwrap();
    let leaf = fixture("leaf_under_yr2.der");
    let cert = ParsedCertificate::parse(&leaf).unwrap();

    for byte in 0..cert.signature_value.len() {
        let mut broken = cert.signature_value.to_vec();
        broken[byte] ^= 0x01;
        assert!(
            !rsa::verify_pkcs1_sha256(key.modulus, key.exponent, &broken, cert.tbs_raw),
            "signature byte {byte} flipped and still accepted"
        );
    }
}

#[test]
fn verifies_rsa_pss_for_both_salt_lengths() {
    const MESSAGE: &[u8] = b"interos rsa pss vector: the transcript a server signs";
    for name in ["pss_sha256_salt32.sig", "pss_sha256_salt_auto.sig"] {
        let vector = fixture(name);
        let mut parts = Fields::new(&vector);
        let modulus = parts.next().unwrap();
        let exponent = parts.next().unwrap();
        let signature = parts.next().unwrap();
        assert!(parts.next().is_none(), "{name}: unexpected extra field");
        assert!(
            rsa::verify_pss_sha256(modulus, exponent, signature, MESSAGE),
            "{name} did not verify"
        );
        assert!(
            !rsa::verify_pss_sha256(modulus, exponent, signature, b"other message"),
            "{name}: verified a message that was not signed"
        );
    }
}

/// Reads `len(2 bytes) || bytes` frames out of a fixture written by
/// `target/verify/gen_x509_fixtures.py`.
struct Fields<'a> {
    rest: &'a [u8],
}

impl<'a> Fields<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }
}

impl<'a> Iterator for Fields<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.len() < 2 {
            return None;
        }
        let (length, rest) = self.rest.split_at(2);
        let length = u16::from_be_bytes([length[0], length[1]]) as usize;
        let (value, rest) = rest.split_at(length);
        self.rest = rest;
        Some(value)
    }
}

#[test]
fn verifies_an_ecdsa_p256_certificate_under_its_issuer() {
    let issuer_der = fixture("p256_issuer.der");
    let leaf_der = fixture("p256_leaf.der");
    let issuer = ParsedCertificate::parse(&issuer_der).unwrap();
    let leaf = ParsedCertificate::parse(&leaf_der).unwrap();

    assert_eq!(issuer.spki_algorithm_oid, EC_PUBLIC_KEY_OID);
    assert_eq!(issuer.spki_params, PRIME256V1_OID, "the curve is SPKI parameters");
    assert_eq!(leaf.spki_algorithm_oid, EC_PUBLIC_KEY_OID);
    assert_eq!(leaf.spki_params, PRIME256V1_OID);
    assert_eq!(leaf.spki_public_key.len(), 65, "an uncompressed point");
    assert_eq!(leaf.spki_public_key[0], 0x04);

    let pair = decode_ecdsa_signature(leaf.signature_value).unwrap();
    assert!(p256::verify(
        issuer.spki_public_key,
        leaf.tbs_raw,
        &decode_ecdsa_signature(leaf.signature_value).unwrap()
    ));
    // The leaf's own key did not sign the leaf: the check is key-specific, not
    // just a shape test on the two scalars.
    assert!(!p256::verify(leaf.spki_public_key, leaf.tbs_raw, &pair));
}

#[test]
fn refuses_tampered_ecdsa_signatures_and_points() {
    let issuer_der = fixture("p256_issuer.der");
    let leaf_der = fixture("p256_leaf.der");
    let issuer = ParsedCertificate::parse(&issuer_der).unwrap();
    let leaf = ParsedCertificate::parse(&leaf_der).unwrap();
    let pair = decode_ecdsa_signature(leaf.signature_value).unwrap();
    let key = issuer.spki_public_key;

    for byte in 0..64 {
        let mut broken = pair;
        broken[byte] ^= 0x01;
        assert!(!p256::verify(key, leaf.tbs_raw, &broken), "scalar byte {byte}");
    }

    let mut tampered = leaf.tbs_raw.to_vec();
    let last = tampered.len() - 1;
    tampered[last] ^= 0x01;
    assert!(!p256::verify(key, &tampered, &pair));

    // A public key that is not a curve point, and a point of the wrong shape,
    // are both refused before any arithmetic runs.
    let mut not_a_point = [0u8; 65];
    not_a_point[0] = 0x04;
    assert!(!p256::verify(&not_a_point, leaf.tbs_raw, &pair));
    assert!(!p256::verify(&key[..64], leaf.tbs_raw, &pair));
    assert!(!p256::verify(key, leaf.tbs_raw, &pair[..63].to_vec()));
}

#[test]
fn decodes_an_ecdsa_signature_and_refuses_garbage() {
    let leaf_der = fixture("p256_leaf.der");
    let leaf = ParsedCertificate::parse(&leaf_der).unwrap();
    let pair = decode_ecdsa_signature(leaf.signature_value).unwrap();
    // The two halves are the r and s the DER carried, big-endian and unpadded.
    let r = &pair[..32];
    let s = &pair[32..];
    assert_ne!(r, &[0u8; 32][..]);
    assert_ne!(s, &[0u8; 32][..]);

    assert!(decode_ecdsa_signature(&[]).is_err());
    assert!(decode_ecdsa_signature(&[0x30, 0x00]).is_err());
    // An `r` wider than a scalar does not fit the frame the verifier takes.
    let mut oversized = vec![0x30, 0x45, 0x02, 0x21];
    oversized.extend_from_slice(&[0xff; 33]);
    oversized.extend_from_slice(&[0x02, 0x20]);
    oversized.extend_from_slice(&[0x01; 32]);
    assert!(decode_ecdsa_signature(&oversized).is_err());
}

/// The 24-bit lengths the TLS 1.3 `Certificate` message uses for the handshake
/// body, the `certificate_list` and each entry.
fn push24(out: &mut Vec<u8>, value: usize) {
    out.extend_from_slice(&[(value >> 16) as u8, (value >> 8) as u8, value as u8]);
}

/// Frames `chain` exactly as a server sends it: leaf first, each issuer above the
/// entry before it, no per-entry extensions.
fn certificate_message(chain: &[&[u8]]) -> Vec<u8> {
    let mut list = Vec::new();
    for der in chain {
        push24(&mut list, der.len());
        list.extend_from_slice(der);
        list.extend_from_slice(&[0x00, 0x00]);
    }
    let mut body = Vec::new();
    body.push(0x00); // certificate_request_context, empty for a server
    push24(&mut body, list.len());
    body.extend_from_slice(&list);

    let mut message = vec![0x0b]; // HandshakeType.certificate
    push24(&mut message, body.len());
    message.extend_from_slice(&body);
    message
}

/// One certificate with a bit flipped inside its own signature, which leaves
/// every length, tag and name in the DER exactly as it was: only the arithmetic
/// can notice.
fn tampered_signature(der: &[u8]) -> Vec<u8> {
    let cert = ParsedCertificate::parse(der).unwrap();
    let last = cert.signature_value.len() - 1;
    let offset = cert.signature_value.as_ptr() as usize - der.as_ptr() as usize + last;
    let mut broken = der.to_vec();
    broken[offset] ^= 0x01;
    broken
}

fn walk(
    message: &[u8],
    roots: &[&[u8]],
    now: i64,
) -> Result<(), TlsError> {
    let mut handshake = HandshakeClient::new([0u8; 32]);
    handshake.state = HandshakeState::EncryptedExtensionsReceived;
    let verifier = DefaultCertVerifier::for_host("developer.mozilla.org")
        .with_roots(roots)
        .with_clock(now);
    handshake
        .process_certificate(message, &verifier, "developer.mozilla.org")
        .map(|_| ())
}

/// The point of the whole exercise: a chain nobody here issued, with only the
/// root pinned, accepted by the real walk — not by a fixture-level helper that
/// agrees with itself.
#[test]
fn walks_the_live_isrg_chain_with_only_the_root_pinned() {
    let root = fixture("isrg_root_x1.der");
    let cross = fixture("root_yr_under_isrg.der");
    let intermediate = fixture("yr2_under_root_yr.der");
    let leaf_der = fixture("leaf_under_yr2.der");
    let leaf = ParsedCertificate::parse(&leaf_der).unwrap();
    let not_before = asn1_time_to_unix(leaf.not_before).unwrap();
    let not_after = asn1_time_to_unix(leaf.not_after).unwrap();
    // Mid-way through the leaf's own window, so this says the same thing after
    // the fixture's ninety days have passed as it says today.
    let now = not_before + (not_after - not_before) / 2;
    let roots: [&[u8]; 1] = [&root];

    // As a real server sends it: the leaf and two intermediates, no root.
    let message = certificate_message(&[&leaf_der, &intermediate, &cross]);
    walk(&message, &roots, now).expect("the walk refused a live public chain");

    // And when a server appends the root it was not asked for, the path still
    // ends on a pinned anchor rather than tripping the depth limit.
    let with_root = certificate_message(&[&leaf_der, &intermediate, &cross, &root]);
    walk(&with_root, &roots, now).expect("the walk refused a chain that included its root");
}

#[test]
fn refuses_a_chain_that_stops_one_link_short_of_the_pin() {
    let root = fixture("isrg_root_x1.der");
    let intermediate = fixture("yr2_under_root_yr.der");
    let leaf_der = fixture("leaf_under_yr2.der");
    let leaf = ParsedCertificate::parse(&leaf_der).unwrap();
    let now = asn1_time_to_unix(leaf.not_before).unwrap();
    let roots: [&[u8]; 1] = [&root];

    // The cross-signed root taken out of the path: the intermediate is not the
    // certificate the pin names, so a client that accepted it would trust any
    // intermediate under any root.
    let message = certificate_message(&[&leaf_der, &intermediate]);
    assert_eq!(
        walk(&message, &roots, now),
        Err(TlsError::CertificateVerificationFailed)
    );
}

#[test]
fn refuses_a_live_chain_when_nothing_is_pinned() {
    let cross = fixture("root_yr_under_isrg.der");
    let intermediate = fixture("yr2_under_root_yr.der");
    let leaf_der = fixture("leaf_under_yr2.der");
    let leaf = ParsedCertificate::parse(&leaf_der).unwrap();
    let now = asn1_time_to_unix(leaf.not_before).unwrap();

    let message = certificate_message(&[&leaf_der, &intermediate, &cross]);
    assert_eq!(
        walk(&message, &[], now),
        Err(TlsError::CertificateVerificationFailed),
        "an empty trust store accepted a chain"
    );
}

#[test]
fn refuses_a_link_whose_signature_was_edited() {
    let root = fixture("isrg_root_x1.der");
    let cross = fixture("root_yr_under_isrg.der");
    let intermediate = fixture("yr2_under_root_yr.der");
    let leaf_der = fixture("leaf_under_yr2.der");
    let leaf = ParsedCertificate::parse(&leaf_der).unwrap();
    let now = asn1_time_to_unix(leaf.not_before).unwrap();
    let roots: [&[u8]; 1] = [&root];

    let broken = tampered_signature(&intermediate);
    assert!(ParsedCertificate::parse(&broken).is_ok(), "edit broke the DER");
    let message = certificate_message(&[&leaf_der, &broken, &cross]);
    assert_eq!(
        walk(&message, &roots, now),
        Err(TlsError::CertificateVerificationFailed),
        "the walk trusted a link it never proved"
    );
}

#[test]
fn refuses_the_chain_once_the_leaf_has_expired() {
    let root = fixture("isrg_root_x1.der");
    let cross = fixture("root_yr_under_isrg.der");
    let intermediate = fixture("yr2_under_root_yr.der");
    let leaf_der = fixture("leaf_under_yr2.der");
    let leaf = ParsedCertificate::parse(&leaf_der).unwrap();
    let not_after = asn1_time_to_unix(leaf.not_after).unwrap();
    let roots: [&[u8]; 1] = [&root];

    let message = certificate_message(&[&leaf_der, &intermediate, &cross]);
    // Every link still verifies; the clock is what refuses this, at the first
    // position the walk visits.
    assert_eq!(
        walk(&message, &roots, not_after + 1),
        Err(TlsError::CertificateExpired)
    );
}
