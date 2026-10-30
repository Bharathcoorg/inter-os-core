//! TLS 1.3 Client and Certificate Trust Unit Tests

use super::certificate::{
    asn1_time_to_unix, hostname_matches, is_supported_sig_scheme, transcript_hash_len,
    CertificateVerifier, DefaultCertVerifier, ParsedCertificate, SubjectAltNames,
    INTEROS_ROOT_CA_DER, SIG_SCHEME_ECDSA_SECP256R1_SHA256, SIG_SCHEME_ECDSA_SECP384R1_SHA384,
    SIG_SCHEME_ED25519, SIG_SCHEME_RSA_PKCS1_SHA256, SIG_SCHEME_RSA_PSS_RSAE_SHA256,
    SUPPORTED_SIG_SCHEMES, TAG_INTEGER, TAG_SEQUENCE,
};
use super::client::{TlsClientSession, TlsConfig};
use super::handshake::{
    HandshakeClient, HandshakeState, MAX_CHAIN_DEPTH, HANDSHAKE_CERTIFICATE,
    HANDSHAKE_CERTIFICATE_VERIFY, HANDSHAKE_ENCRYPTED_EXTENSIONS, HANDSHAKE_FINISHED,
    SUPPORTED_GROUPS, TLS_CIPHER_CHACHA20_POLY1305_SHA256, TLS_VERSION_1_3,
};
use super::ed25519::Sha512;
use super::TlsError;

/// Test-only permissive verifier. It lives here, inside `#[cfg(test)]`, and is
/// therefore NOT linked into any guest/userspace build: no production handshake
/// path can reach it. It also declares no reference identity, which the client
/// refuses on a live handshake, so it can never report "verified" end-to-end.
struct AnyPassVerifier;

impl CertificateVerifier for AnyPassVerifier {
    fn verify_certificate(&self, _cert: &ParsedCertificate) -> Result<(), TlsError> {
        Ok(())
    }
    /// Structural only: these fixtures carry no clock, and neither stub can
    /// report a verified handshake anyway (AnyPassVerifier declares no
    /// identity, and the mock store holds a certificate it never signs).
    fn check_validity_window(&self, cert: &ParsedCertificate) -> Result<(), TlsError> {
        cert.check_validity()
    }
    fn verify_trust_anchor(&self, _cert: &ParsedCertificate) -> Result<(), TlsError> {
        Ok(())
    }
    fn verify_reaches_trust_store(&self, _cert: &ParsedCertificate) -> Result<(), TlsError> {
        Ok(())
    }
    fn reference_identity(&self) -> Option<&str> {
        None
    }
    fn verify_server_signature(
        &self,
        _public_key: &[u8],
        _algorithm: u16,
        _context_and_hash: &[u8],
        _signature: &[u8],
    ) -> Result<(), TlsError> {
        Ok(())
    }
}

/// A name longer than any ClientHello ServerName / DER dNSName field can carry.
const OVER_LONG_HOST: &[u8; 300] = &[b'a'; 300];

/// The longest name the ClientHello may still advertise (255-byte bound), used
/// to prove the record no longer overruns a caller's buffer.
const LONG_SNI_NAME: &str = "a.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.test";
static LONG_HOST_VERIFIER: DefaultCertVerifier<'static> =
    DefaultCertVerifier::for_host(LONG_SNI_NAME);

/// Builds a TLS 1.3 Certificate handshake message from raw DER chain entries.
/// Each entry is framed as: cert length (3) || der || entry extensions (2 bytes, zero).
fn build_certificate_message(entries: &[&[u8]], out: &mut [u8]) -> usize {
    let mut list_len = 0usize;
    for entry in entries {
        list_len += 3 + entry.len() + 2;
    }
    let body_len = 1 + 3 + list_len; // request context length + list length + list
    let mut pos = 0;
    out[pos] = HANDSHAKE_CERTIFICATE;
    pos += 1;
    out[1] = (body_len >> 16) as u8;
    out[2] = (body_len >> 8) as u8;
    out[3] = (body_len & 0xff) as u8;
    pos += 3;
    out[pos] = 0; // certificate_request_context: empty
    pos += 1;
    out[pos] = (list_len >> 16) as u8;
    out[pos + 1] = (list_len >> 8) as u8;
    out[pos + 2] = (list_len & 0xff) as u8;
    pos += 3;
    for entry in entries {
        let n = entry.len();
        out[pos] = (n >> 16) as u8;
        out[pos + 1] = (n >> 8) as u8;
        out[pos + 2] = (n & 0xff) as u8;
        pos += 3;
        out[pos..pos + n].copy_from_slice(entry);
        pos += n;
        out[pos] = 0;
        out[pos + 1] = 0;
        pos += 2;
    }
    pos
}

/// Assembles a ServerHello handshake message body for parser tests.
fn build_server_hello(group: u16) -> ([u8; 180], usize) {
    build_server_hello_with(group, TLS_CIPHER_CHACHA20_POLY1305_SHA256, TLS_VERSION_1_3)
}

/// Assembles a ServerHello with an explicit cipher suite and negotiated
/// version, so the "accepted set == advertised set" gate can be exercised
/// with values this client never advertised.
fn build_server_hello_with(group: u16, cipher_suite: u16, version: u16) -> ([u8; 180], usize) {
    let mut body = [0u8; 176];
    let mut pos = 0;
    body[pos..pos + 2].copy_from_slice(&0x0303u16.to_be_bytes());
    pos += 2;
    body[pos..pos + 32].fill(0x11);
    pos += 32;
    body[pos] = 0; // session id length
    pos += 1;
    body[pos..pos + 2].copy_from_slice(&cipher_suite.to_be_bytes());
    pos += 2;
    body[pos] = 0; // compression
    pos += 1;

    let ext_start = pos;
    pos += 2; // extensions length
    // supported_versions
    body[pos..pos + 2].copy_from_slice(&0x002bu16.to_be_bytes());
    pos += 2;
    body[pos..pos + 2].copy_from_slice(&2u16.to_be_bytes());
    pos += 2;
    body[pos..pos + 2].copy_from_slice(&version.to_be_bytes());
    pos += 2;
    // key_share
    body[pos..pos + 2].copy_from_slice(&0x0033u16.to_be_bytes());
    pos += 2;
    body[pos..pos + 2].copy_from_slice(&36u16.to_be_bytes());
    pos += 2;
    body[pos..pos + 2].copy_from_slice(&group.to_be_bytes());
    pos += 2;
    body[pos..pos + 2].copy_from_slice(&32u16.to_be_bytes());
    pos += 2;
    body[pos..pos + 32].fill(0x22);
    pos += 32;
    let ext_len = pos - ext_start - 2;
    body[ext_start..ext_start + 2].copy_from_slice(&(ext_len as u16).to_be_bytes());

    let total = pos;
    let mut msg = [0u8; 180];
    msg[0] = 2;
    msg[1] = (total >> 16) as u8;
    msg[2] = (total >> 8) as u8;
    msg[3] = (total & 0xff) as u8;
    msg[4..4 + total].copy_from_slice(&body[..total]);
    (msg, 4 + total)
}

// 219-byte valid DER X.509 Certificate with CN="example.com"
const EXAMPLE_COM_CERT_DER: &[u8] = &[
    0x30, 0x81, 0xd8, 0x30, 0x81, 0x8b, 0xa0, 0x03, 0x02, 0x01, 0x01, 0x02, 0x01, 0x01, 0x30, 0x05,
    0x06, 0x03, 0x2b, 0x65, 0x70, 0x30, 0x16, 0x31, 0x14, 0x30, 0x12, 0x06, 0x03, 0x55, 0x04, 0x03,
    0x0c, 0x0b, 0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c, 0x65, 0x2e, 0x63, 0x6f, 0x6d, 0x30, 0x1e, 0x17,
    0x0d, 0x32, 0x36, 0x30, 0x31, 0x30, 0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x17, 0x0d,
    0x32, 0x36, 0x31, 0x32, 0x33, 0x31, 0x32, 0x33, 0x35, 0x39, 0x35, 0x39, 0x5a, 0x30, 0x16, 0x31,
    0x14, 0x30, 0x12, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x0b, 0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c,
    0x65, 0x2e, 0x63, 0x6f, 0x6d, 0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21,
    0x00, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09,
    0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09,
    0x09, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x41, 0x00, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
];

// 221-byte valid DER X.509 Certificate with CN="Untrusted CA"
const UNTRUSTED_CA_CERT_DER: &[u8] = &[
    0x30, 0x81, 0xda, 0x30, 0x81, 0x8d, 0xa0, 0x03, 0x02, 0x01, 0x01, 0x02, 0x01, 0x01, 0x30, 0x05,
    0x06, 0x03, 0x2b, 0x65, 0x70, 0x30, 0x17, 0x31, 0x15, 0x30, 0x13, 0x06, 0x03, 0x55, 0x04, 0x03,
    0x0c, 0x0c, 0x55, 0x6e, 0x74, 0x72, 0x75, 0x73, 0x74, 0x65, 0x64, 0x20, 0x43, 0x41, 0x30, 0x1e,
    0x17, 0x0d, 0x32, 0x36, 0x30, 0x31, 0x30, 0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x17,
    0x0d, 0x32, 0x36, 0x31, 0x32, 0x33, 0x31, 0x32, 0x33, 0x35, 0x39, 0x35, 0x39, 0x5a, 0x30, 0x17,
    0x31, 0x15, 0x30, 0x13, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x0c, 0x55, 0x6e, 0x74, 0x72, 0x75,
    0x73, 0x74, 0x65, 0x64, 0x20, 0x43, 0x41, 0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70,
    0x03, 0x21, 0x00, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09,
    0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09,
    0x09, 0x09, 0x09, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x41, 0x00, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
];

// 261-byte valid DER X.509 Certificate with SAN = ["*.interos.org", "interos.org"]
const SAN_INTEROS_CERT_DER: &[u8] = &[
    0x30, 0x82, 0x01, 0x03, 0x30, 0x81, 0xb6, 0xa0, 0x03, 0x02, 0x01, 0x02, 0x02, 0x01, 0x01, 0x30,
    0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x30, 0x16, 0x31, 0x14, 0x30, 0x12, 0x06, 0x03, 0x55, 0x04,
    0x03, 0x0c, 0x0b, 0x69, 0x6e, 0x74, 0x65, 0x72, 0x6f, 0x73, 0x2e, 0x6f, 0x72, 0x67, 0x30, 0x1e,
    0x17, 0x0d, 0x32, 0x36, 0x30, 0x31, 0x30, 0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x17,
    0x0d, 0x32, 0x36, 0x31, 0x32, 0x33, 0x31, 0x32, 0x33, 0x35, 0x39, 0x35, 0x39, 0x5a, 0x30, 0x16,
    0x31, 0x14, 0x30, 0x12, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x0b, 0x69, 0x6e, 0x74, 0x65, 0x72,
    0x6f, 0x73, 0x2e, 0x6f, 0x72, 0x67, 0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03,
    0x21, 0x00, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09,
    0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09,
    0x09, 0x09, 0xa3, 0x29, 0x30, 0x27, 0x30, 0x25, 0x06, 0x03, 0x55, 0x1d, 0x11, 0x04, 0x1e, 0x30,
    0x1c, 0x82, 0x0d, 0x2a, 0x2e, 0x69, 0x6e, 0x74, 0x65, 0x72, 0x6f, 0x73, 0x2e, 0x6f, 0x72, 0x67,
    0x82, 0x0b, 0x69, 0x6e, 0x74, 0x65, 0x72, 0x6f, 0x73, 0x2e, 0x6f, 0x72, 0x67, 0x30, 0x05, 0x06,
    0x03, 0x2b, 0x65, 0x70, 0x03, 0x41, 0x00, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
];

// 246-byte valid DER X.509 Certificate with RFC 8032 test public key
const RFC8032_CERT_DER: &[u8] = &[
    0x30, 0x81, 0xf4, 0x30, 0x81, 0xa7, 0xa0, 0x03, 0x02, 0x01, 0x02, 0x02, 0x01, 0x01, 0x30, 0x05,
    0x06, 0x03, 0x2b, 0x65, 0x70, 0x30, 0x16, 0x31, 0x14, 0x30, 0x12, 0x06, 0x03, 0x55, 0x04, 0x03,
    0x0c, 0x0b, 0x72, 0x66, 0x63, 0x2e, 0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c, 0x65, 0x30, 0x1e, 0x17,
    0x0d, 0x32, 0x36, 0x30, 0x31, 0x30, 0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x17, 0x0d,
    0x32, 0x36, 0x31, 0x32, 0x33, 0x31, 0x32, 0x33, 0x35, 0x39, 0x35, 0x39, 0x5a, 0x30, 0x16, 0x31,
    0x14, 0x30, 0x12, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x0b, 0x72, 0x66, 0x63, 0x2e, 0x65, 0x78,
    0x61, 0x6d, 0x70, 0x6c, 0x65, 0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21,
    0x00, 0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07,
    0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07, 0x51,
    0x1a, 0xa3, 0x1a, 0x30, 0x18, 0x30, 0x16, 0x06, 0x03, 0x55, 0x1d, 0x11, 0x04, 0x0f, 0x30, 0x0d,
    0x82, 0x0b, 0x72, 0x66, 0x63, 0x2e, 0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c, 0x65, 0x30, 0x05, 0x06,
    0x03, 0x2b, 0x65, 0x70, 0x03, 0x41, 0x00, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
];

// RFC 8032 Section 7.1 Test 1 signature over empty message b""
const RFC8032_TEST1_SIG: [u8; 64] = [
    0xe5, 0x56, 0x43, 0x00, 0xc3, 0x60, 0xac, 0x72, 0x90, 0x86, 0xe2, 0xcc, 0x80, 0x6e, 0x82, 0x8a,
    0x84, 0x87, 0x7f, 0x1e, 0xb8, 0xe5, 0xd9, 0x74, 0xd8, 0x73, 0xe0, 0x65, 0x22, 0x49, 0x01, 0x55,
    0x5f, 0xb8, 0x82, 0x15, 0x90, 0xa3, 0x3b, 0xac, 0xc6, 0x1e, 0x39, 0x70, 0x1c, 0xf9, 0xb4, 0x6b,
    0xd2, 0x5b, 0xf5, 0xf0, 0x59, 0x5b, 0xbe, 0x24, 0x65, 0x51, 0x41, 0x43, 0x8e, 0x7a, 0x10, 0x0b,
];

/* CA_ED25519_DER: 249 bytes. Self-signed Ed25519 CA, CN "InterOS Test CA".
   Unlike the placeholder-key fixtures above, this carries a REAL RFC 8032
   signature, so a chain built from it exercises the actual verify path. */
const CA_ED25519_DER: &[u8] = &[
    0x30, 0x81, 0xf6, 0x30, 0x81, 0xa9, 0xa0, 0x03, 0x02, 0x01, 0x02, 0x02,
    0x02, 0x03, 0xe7, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x30, 0x1a,
    0x31, 0x18, 0x30, 0x16, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x0f, 0x49,
    0x6e, 0x74, 0x65, 0x72, 0x4f, 0x53, 0x20, 0x54, 0x65, 0x73, 0x74, 0x20,
    0x43, 0x41, 0x30, 0x1e, 0x17, 0x0d, 0x32, 0x36, 0x30, 0x31, 0x30, 0x31,
    0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x17, 0x0d, 0x33, 0x35, 0x31,
    0x32, 0x33, 0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x30, 0x1a,
    0x31, 0x18, 0x30, 0x16, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x0f, 0x49,
    0x6e, 0x74, 0x65, 0x72, 0x4f, 0x53, 0x20, 0x54, 0x65, 0x73, 0x74, 0x20,
    0x43, 0x41, 0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03,
    0x21, 0x00, 0x03, 0x5f, 0x01, 0x27, 0x9c, 0x47, 0x62, 0x05, 0x7a, 0x89,
    0xd5, 0x9b, 0x1f, 0xfa, 0xf1, 0x69, 0x1b, 0xab, 0x34, 0xf2, 0xd5, 0xf0,
    0xa7, 0x26, 0x55, 0x8f, 0xab, 0x57, 0x2e, 0x6b, 0xfd, 0xec, 0xa3, 0x13,
    0x30, 0x11, 0x30, 0x0f, 0x06, 0x03, 0x55, 0x1d, 0x13, 0x01, 0x01, 0xff,
    0x04, 0x05, 0x30, 0x03, 0x01, 0x01, 0xff, 0x30, 0x05, 0x06, 0x03, 0x2b,
    0x65, 0x70, 0x03, 0x41, 0x00, 0x3d, 0x8d, 0xb1, 0x9e, 0xb2, 0xb8, 0x26,
    0xfc, 0x04, 0x41, 0x21, 0x8f, 0x1c, 0x7c, 0x40, 0x6e, 0xf7, 0x18, 0xf8,
    0xff, 0xfe, 0x28, 0xaf, 0x73, 0xfc, 0x32, 0xbc, 0xfa, 0x9c, 0x97, 0xbf,
    0xba, 0xb9, 0xfd, 0xc3, 0xdf, 0x4b, 0x78, 0x70, 0x68, 0x30, 0x1a, 0x3a,
    0xc2, 0x98, 0x3e, 0xed, 0xec, 0x12, 0x8a, 0x61, 0xc4, 0xba, 0x3f, 0x5d,
    0x49, 0x87, 0xdb, 0xea, 0xd8, 0xfc, 0x27, 0x92, 0x05,
];

/* LEAF_GOOD_DER: 277 bytes. Signed by CA_ED25519_DER, SAN dNSName "docs.interos.dev". */
const LEAF_GOOD_DER: &[u8] = &[
    0x30, 0x82, 0x01, 0x11, 0x30, 0x81, 0xc4, 0xa0, 0x03, 0x02, 0x01, 0x02,
    0x02, 0x02, 0x30, 0x39, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x30,
    0x1a, 0x31, 0x18, 0x30, 0x16, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x0f,
    0x49, 0x6e, 0x74, 0x65, 0x72, 0x4f, 0x53, 0x20, 0x54, 0x65, 0x73, 0x74,
    0x20, 0x43, 0x41, 0x30, 0x1e, 0x17, 0x0d, 0x32, 0x36, 0x30, 0x31, 0x30,
    0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x17, 0x0d, 0x33, 0x35,
    0x31, 0x32, 0x33, 0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x30,
    0x1b, 0x31, 0x19, 0x30, 0x17, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x10,
    0x64, 0x6f, 0x63, 0x73, 0x2e, 0x69, 0x6e, 0x74, 0x65, 0x72, 0x6f, 0x73,
    0x2e, 0x64, 0x65, 0x76, 0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65,
    0x70, 0x03, 0x21, 0x00, 0xc6, 0xc9, 0x6c, 0xd2, 0x71, 0xe2, 0xb5, 0xdd,
    0xfe, 0x2d, 0x6a, 0x1e, 0xc8, 0xfd, 0x68, 0xbb, 0x76, 0x47, 0xab, 0x11,
    0xf7, 0x9a, 0x7e, 0x1b, 0x94, 0x78, 0x15, 0x71, 0xec, 0xd0, 0x57, 0x67,
    0xa3, 0x2d, 0x30, 0x2b, 0x30, 0x1b, 0x06, 0x03, 0x55, 0x1d, 0x11, 0x04,
    0x14, 0x30, 0x12, 0x82, 0x10, 0x64, 0x6f, 0x63, 0x73, 0x2e, 0x69, 0x6e,
    0x74, 0x65, 0x72, 0x6f, 0x73, 0x2e, 0x64, 0x65, 0x76, 0x30, 0x0c, 0x06,
    0x03, 0x55, 0x1d, 0x13, 0x01, 0x01, 0xff, 0x04, 0x02, 0x30, 0x00, 0x30,
    0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x41, 0x00, 0x5d, 0xfa, 0x68,
    0xe1, 0x95, 0x70, 0x6d, 0x5d, 0xee, 0x8c, 0x53, 0x48, 0x73, 0xc8, 0xa5,
    0x3d, 0xee, 0xbf, 0xc9, 0x5a, 0x36, 0x1f, 0x32, 0xe3, 0x60, 0x3e, 0xd6,
    0x39, 0xd5, 0x3c, 0x07, 0x3c, 0x2b, 0xfa, 0x7d, 0x82, 0xd5, 0xe9, 0x3c,
    0x97, 0x18, 0xda, 0x9c, 0x45, 0x6b, 0x2b, 0xdb, 0x5e, 0xde, 0xe4, 0xf0,
    0x3d, 0xcc, 0xce, 0x24, 0x4e, 0x23, 0x34, 0x71, 0x19, 0x48, 0x2b, 0x17,
    0x0b,
];

/* LEAF_EVIL_DER: 269 bytes. Same CA and serial, SAN dNSName "evil.example". */
const LEAF_EVIL_DER: &[u8] = &[
    0x30, 0x82, 0x01, 0x09, 0x30, 0x81, 0xbc, 0xa0, 0x03, 0x02, 0x01, 0x02,
    0x02, 0x02, 0x30, 0x39, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x30,
    0x1a, 0x31, 0x18, 0x30, 0x16, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x0f,
    0x49, 0x6e, 0x74, 0x65, 0x72, 0x4f, 0x53, 0x20, 0x54, 0x65, 0x73, 0x74,
    0x20, 0x43, 0x41, 0x30, 0x1e, 0x17, 0x0d, 0x32, 0x36, 0x30, 0x31, 0x30,
    0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x17, 0x0d, 0x33, 0x35,
    0x31, 0x32, 0x33, 0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x30,
    0x17, 0x31, 0x15, 0x30, 0x13, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x0c,
    0x65, 0x76, 0x69, 0x6c, 0x2e, 0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c, 0x65,
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    0x09, 0xb7, 0x22, 0x35, 0x68, 0x15, 0x48, 0xda, 0xae, 0x79, 0x48, 0x7b,
    0x4a, 0xea, 0x7b, 0x1d, 0x18, 0xdb, 0x4f, 0x76, 0x81, 0x6d, 0x83, 0xd4,
    0x71, 0xb8, 0x62, 0xb6, 0xa5, 0xe9, 0xce, 0xf0, 0xa3, 0x29, 0x30, 0x27,
    0x30, 0x17, 0x06, 0x03, 0x55, 0x1d, 0x11, 0x04, 0x10, 0x30, 0x0e, 0x82,
    0x0c, 0x65, 0x76, 0x69, 0x6c, 0x2e, 0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c,
    0x65, 0x30, 0x0c, 0x06, 0x03, 0x55, 0x1d, 0x13, 0x01, 0x01, 0xff, 0x04,
    0x02, 0x30, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x41,
    0x00, 0xd5, 0x35, 0xb7, 0x24, 0x93, 0xff, 0xe5, 0x4c, 0x6c, 0x08, 0x25,
    0x49, 0x27, 0x4c, 0x3a, 0xc8, 0x65, 0x11, 0x7e, 0x4c, 0x4d, 0x89, 0xf9,
    0x81, 0x47, 0x1f, 0x73, 0x7a, 0xda, 0x12, 0x73, 0x37, 0x23, 0xfe, 0xe1,
    0x02, 0x63, 0xcd, 0x50, 0xcd, 0x0e, 0xb4, 0x32, 0x13, 0xa8, 0x88, 0xb0,
    0x3a, 0xfc, 0x6d, 0x79, 0xad, 0x3f, 0x41, 0xf7, 0xe1, 0xc5, 0x8a, 0x49,
    0x29, 0xc3, 0x0d, 0x8a, 0x01,
];

#[test]
fn tls_reject_garbage_signature() {
    let verifier = DefaultCertVerifier::new();
    let cert = ParsedCertificate::parse(INTEROS_ROOT_CA_DER).unwrap();
    let garbage_sig = [0x55u8; 64]; // Non-zero 64-byte garbage signature
    let res = verifier.verify_server_signature(
        cert.spki_public_key,
        SIG_SCHEME_ED25519,
        b"TLS 1.3, server CertificateVerify context and transcript hash",
        &garbage_sig,
    );
    assert_eq!(res, Err(TlsError::InvalidSignature));
}

#[test]
fn tls_reject_mismatched_hostname() {
    let verifier = DefaultCertVerifier::new().with_hostname("interos.org");
    let cert = ParsedCertificate::parse(EXAMPLE_COM_CERT_DER).unwrap();
    let res = verifier.verify_certificate(&cert);
    assert_eq!(res, Err(TlsError::CertificateVerificationFailed));
}

#[test]
fn tls_reject_untrusted_root() {
    let verifier = DefaultCertVerifier::new();
    let untrusted_root = ParsedCertificate::parse(UNTRUSTED_CA_CERT_DER).unwrap();
    let res = verifier.verify_certificate(&untrusted_root);
    assert_eq!(res, Err(TlsError::CertificateVerificationFailed));
}

#[test]
fn tls_accept_valid_rfc8032_signature() {
    let verifier = DefaultCertVerifier::new();
    let cert = ParsedCertificate::parse(RFC8032_CERT_DER).unwrap();
    let res = verifier.verify_server_signature(
        cert.spki_public_key,
        SIG_SCHEME_ED25519,
        b"",
        &RFC8032_TEST1_SIG,
    );
    assert!(res.is_ok(), "Authentic RFC 8032 signature must be accepted");
}

#[test]
fn tls_reject_tampered_signature() {
    let verifier = DefaultCertVerifier::new();
    let cert = ParsedCertificate::parse(RFC8032_CERT_DER).unwrap();
    let mut tampered = RFC8032_TEST1_SIG;
    tampered[0] ^= 0x01; // Tamper 1 bit
    let res = verifier.verify_server_signature(
        cert.spki_public_key,
        SIG_SCHEME_ED25519,
        b"",
        &tampered,
    );
    assert_eq!(res, Err(TlsError::InvalidSignature));
}

#[test]
fn tls_san_wildcard_matching() {
    let cert = ParsedCertificate::parse(SAN_INTEROS_CERT_DER).unwrap();

    // Exact match
    assert!(cert.matches_hostname("interos.org"));
    assert!(cert.matches_hostname("INTEROS.ORG"));

    // Single-level wildcard match
    assert!(cert.matches_hostname("docs.interos.org"));
    assert!(cert.matches_hostname("api.interos.org"));

    // Disallowed multi-level wildcard
    assert!(!cert.matches_hostname("deep.sub.interos.org"));

    // Disallowed mismatch
    assert!(!cert.matches_hostname("evil.com"));
    assert!(!cert.matches_hostname("interos.org.evil.com"));
}

#[test]
fn tls_accept_trusted_root() {
    // The synthetic CA is a fixture, not a shipped anchor: it is trusted only
    // because this test pins it explicitly.
    static ROOTS: [&[u8]; 1] = [INTEROS_ROOT_CA_DER];
    let verifier = DefaultCertVerifier::new().with_hostname("InterOS").with_roots(&ROOTS);
    let root = ParsedCertificate::parse(INTEROS_ROOT_CA_DER).unwrap();
    assert!(verifier.verify_certificate(&root).is_ok());
}

#[test]
fn tls_reject_unknown_signature_scheme() {
    let verifier = DefaultCertVerifier::new();
    let cert = ParsedCertificate::parse(INTEROS_ROOT_CA_DER).unwrap();
    let sig = [0x55u8; 64];

    // ECDSA secp256r1 not supported by Ed25519 verifier
    let res = verifier.verify_server_signature(
        cert.spki_public_key,
        SIG_SCHEME_ECDSA_SECP256R1_SHA256,
        b"message",
        &sig,
    );
    assert_eq!(res, Err(TlsError::UnsupportedSignatureScheme));

    // Unknown code point 0xffff
    let res2 = verifier.verify_server_signature(cert.spki_public_key, 0xffff, b"message", &sig);
    assert_eq!(res2, Err(TlsError::UnsupportedSignatureScheme));
}

#[test]
fn tls_sha512_standard_test_vector() {
    // SHA-512(b"abc") from FIPS 180-4
    let expected: [u8; 64] = [
        0xdd, 0xaf, 0x35, 0xa1, 0x93, 0x61, 0x7a, 0xba, 0xcc, 0x41, 0x73, 0x49, 0xae, 0x20, 0x41, 0x31,
        0x12, 0xe6, 0xfa, 0x4e, 0x89, 0xa9, 0x7e, 0xa2, 0x0a, 0x9e, 0xee, 0xe6, 0x4b, 0x55, 0xd3, 0x9a,
        0x21, 0x92, 0x99, 0x2a, 0x27, 0x4f, 0xc1, 0xa8, 0x36, 0xba, 0x3c, 0x23, 0xa3, 0xfe, 0xeb, 0xbd,
        0x45, 0x4d, 0x44, 0x23, 0x64, 0x3c, 0xe8, 0x0e, 0x2a, 0x9a, 0xc9, 0x4f, 0xa5, 0x4c, 0xa4, 0x9f,
    ];
    let actual = Sha512::digest(b"abc");
    assert_eq!(actual, expected);
}

#[test]
fn tls_reject_wronghost_on_default_path() {
    // A certificate for "InterOS" must NOT be accepted when expected hostname is absent/mismatched.
    // The default path must enforce hostname verification and fail rather than silently skip.
    let verifier = DefaultCertVerifier::new();
    let root = ParsedCertificate::parse(INTEROS_ROOT_CA_DER).unwrap();
    let res = verifier.verify_certificate(&root);
    assert_eq!(res, Err(TlsError::CertificateVerificationFailed));
}

#[test]
fn tls_reject_server_choosing_a_scheme_the_leaf_key_cannot_sign() {
    // 0x0403 is advertised, but the leaf here carries a 32-byte Ed25519 key, and
    // an ECDSA P-256 signature is made by a P-256 point. Pairing a scheme with a
    // key of another type is refused before any arithmetic: the alternative is
    // believing a signature that was made for some other key entirely.
    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::CertificateReceived;
    let cert = ParsedCertificate::parse(RFC8032_CERT_DER).unwrap();
    let verifier = DefaultCertVerifier::for_host("example.com");
    let cv_payload = [
        HANDSHAKE_CERTIFICATE_VERIFY,
        0, 0, 6,
        0x04, 0x03, // SIG_SCHEME_ECDSA_SECP256R1_SHA256
        0, 2,       // sig len = 2
        0xaa, 0xbb, // dummy signature
    ];
    let res = hs.process_certificate_verify(&cv_payload, cert.spki_public_key, &verifier);
    assert_eq!(res, Err(TlsError::UnsupportedSignatureScheme));
    assert_eq!(hs.state, HandshakeState::CertificateReceived);
}

#[test]
fn tls_reject_server_choosing_an_unadvertised_scheme() {
    // 0x0504 (ecdsa_secp521r1_sha512) is a code point this client never offers in
    // its ClientHello and has no arithmetic behind it, so a server that picks it
    // is refused by the gate itself -- ahead of the verifier, which here would
    // have answered Ok to anything.
    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::CertificateReceived;
    let cert = ParsedCertificate::parse(RFC8032_CERT_DER).unwrap();
    let verifier = AnyPassVerifier;
    let cv_payload = [
        HANDSHAKE_CERTIFICATE_VERIFY,
        0, 0, 6,
        0x05, 0x04, // ecdsa_secp521r1_sha512: never advertised
        0, 2,
        0xaa, 0xbb,
    ];
    assert_eq!(
        hs.process_certificate_verify(&cv_payload, cert.spki_public_key, &verifier),
        Err(TlsError::UnsupportedSignatureScheme)
    );
    assert_eq!(hs.state, HandshakeState::CertificateReceived);
    assert!(!is_supported_sig_scheme(0x0504));
    assert!(transcript_hash_len(0x0504).is_none(), "no hash for a scheme that is not offered");
    for scheme in SUPPORTED_SIG_SCHEMES.iter() {
        assert!(is_supported_sig_scheme(*scheme), "{scheme:#06x} is advertised");
        assert!(
            transcript_hash_len(*scheme).is_some(),
            "{scheme:#06x} is advertised without a transcript hash"
        );
    }
}

#[test]
fn tls_session_built_from_default_reports_unavailable() {
    // A session built from DefaultConfig carries static dummy keys and no peer name;
    // it must refuse to proceed with the handshake and report Unavailable.
    let mut session = TlsClientSession::new(TlsConfig::default());
    let mut out = [0u8; 512];
    let res = session.start_handshake(&mut out);
    assert_eq!(res, Err(TlsError::Unavailable));
}

#[test]
fn tls_client_hello_advertises_the_supported_scheme_list() {
    // The ClientHello's `signature_algorithms` is the verifier's accepted list
    // written out byte for byte, in order: a server may pick nothing else, and
    // nothing offered here goes unimplemented.
    let verifier = DefaultCertVerifier::for_host("example.com");
    let mut session = TlsClientSession::new(TlsConfig {
        server_name: Some("example.com"),
        random: [0x55; 32],
        private_key: [0x77; 32],
        verifier: &verifier,
    });
    let mut out = [0u8; 512];
    let len = session.start_handshake(&mut out).unwrap();
    assert!(len > 0);

    // Scan for signature_algorithms extension (0x000d) in the ClientHello record.
    // A candidate only counts when its own lengths match the list this client
    // holds, so a `00 0d` that appears inside the random or a session id is not
    // mistaken for the extension.
    let record = &out[..len];
    let expected_list_len = 2 * SUPPORTED_SIG_SCHEMES.len();
    let mut found = false;
    for i in 0..record.len() - 4 {
        if record[i] != 0x00 || record[i + 1] != 0x0d {
            continue;
        }
        let ext_len = u16::from_be_bytes([record[i + 2], record[i + 3]]) as usize;
        if ext_len != expected_list_len + 2 || record.len() < i + 4 + ext_len {
            continue;
        }
        let list_len = u16::from_be_bytes([record[i + 4], record[i + 5]]) as usize;
        assert_eq!(list_len, expected_list_len, "algorithms list length");
        for (position, scheme) in SUPPORTED_SIG_SCHEMES.iter().enumerate() {
            let at = i + 6 + position * 2;
            assert_eq!(
                u16::from_be_bytes([record[at], record[at + 1]]),
                *scheme,
                "advertised scheme {position}"
            );
        }
        found = true;
        break;
    }
    assert!(found, "signature_algorithms extension must be present in ClientHello");
}

#[test]
fn tls_handshake_rejects_wronghost_explicit() {
    let verifier = DefaultCertVerifier::for_host("wronghost.test");
    let root = ParsedCertificate::parse(INTEROS_ROOT_CA_DER).unwrap();
    // INTEROS_ROOT_CA_DER has CN="InterOS", not "wronghost.test"
    let res = verifier.verify_certificate(&root);
    assert_eq!(res, Err(TlsError::CertificateVerificationFailed));
}

#[test]
fn tls_config_new_rejects_dummy_or_empty_values() {
    let verifier = DefaultCertVerifier::for_host("example.com");
    // Empty server name
    assert_eq!(
        TlsConfig::new("", [0x11; 32], [0x22; 32], &verifier).err(),
        Some(TlsError::Unavailable)
    );
    // All-zero random
    assert_eq!(
        TlsConfig::new("example.com", [0u8; 32], [0x22; 32], &verifier).err(),
        Some(TlsError::Unavailable)
    );
    // Static dummy random 0x42
    assert_eq!(
        TlsConfig::new("example.com", [0x42; 32], [0x22; 32], &verifier).err(),
        Some(TlsError::Unavailable)
    );
    // All-zero private key
    assert_eq!(
        TlsConfig::new("example.com", [0x11; 32], [0u8; 32], &verifier).err(),
        Some(TlsError::Unavailable)
    );
    // Static dummy private key 0x24
    assert_eq!(
        TlsConfig::new("example.com", [0x11; 32], [0x24; 32], &verifier).err(),
        Some(TlsError::Unavailable)
    );
    // Valid configuration
    assert!(TlsConfig::new("example.com", [0x11; 32], [0x22; 32], &verifier).is_ok());
}

// ---------------------------------------------------------------------------
// Advertised == accepted: one source of truth for schemes, groups, suites
// ---------------------------------------------------------------------------

#[test]
fn tls_advertised_schemes_are_all_verifiable() {
    // Every advertised scheme must be a scheme the verifier actually attempts:
    // given a key of the shape that scheme signs with, a garbage signature is
    // reported as a failed check, never as a scheme it declined to run.
    assert_eq!(
        SUPPORTED_SIG_SCHEMES,
        [
            SIG_SCHEME_ECDSA_SECP256R1_SHA256,
            SIG_SCHEME_ECDSA_SECP384R1_SHA384,
            SIG_SCHEME_ED25519,
            SIG_SCHEME_RSA_PSS_RSAE_SHA256,
            SIG_SCHEME_RSA_PKCS1_SHA256,
        ]
    );
    let verifier = DefaultCertVerifier::new();
    // One key per advertised shape. Their contents do not matter: what is being
    // pinned here is that the verifier reaches the arithmetic instead of
    // shrugging at the scheme.
    let ed25519_key = [0x09u8; 32];
    let mut p256_point = [0x2au8; 65];
    p256_point[0] = 0x04;
    let mut p384_point = [0x2au8; 97];
    p384_point[0] = 0x04;
    let rsa_key = rsa_public_key_der();
    let cases: [(u16, &[u8]); 5] = [
        (SIG_SCHEME_ED25519, &ed25519_key),
        (SIG_SCHEME_ECDSA_SECP256R1_SHA256, &p256_point),
        (SIG_SCHEME_ECDSA_SECP384R1_SHA384, &p384_point),
        (SIG_SCHEME_RSA_PSS_RSAE_SHA256, &rsa_key),
        (SIG_SCHEME_RSA_PKCS1_SHA256, &rsa_key),
    ];
    for (scheme, key) in cases {
        let res = verifier.verify_server_signature(key, scheme, b"msg", &[0x55u8; 64]);
        assert_ne!(
            res,
            Err(TlsError::UnsupportedSignatureScheme),
            "scheme {scheme:#06x} must be checked, not skipped"
        );
    }
}

/// A 2048-bit `RSAPublicKey` in the DER the certificate carries it in: only the
/// shape matters, so the modulus is a pattern rather than a real key.
fn rsa_public_key_der() -> std::vec::Vec<u8> {
    let mut der = std::vec::Vec::new();
    der.push(TAG_SEQUENCE);
    der.extend_from_slice(&[0x82, 0x01, 0x0a]); // 266 bytes of content
    der.push(TAG_INTEGER);
    der.extend_from_slice(&[0x82, 0x01, 0x01]); // 257 bytes: 0x00 pad + 256
    der.push(0x00);
    der.extend_from_slice(&[0xAAu8; 256]);
    der.push(TAG_INTEGER);
    der.push(0x03);
    der.extend_from_slice(&[0x01, 0x00, 0x01]); // 65537
    der
}

#[test]
fn tls_client_hello_advertises_exactly_the_verifiable_sets() {
    let verifier = DefaultCertVerifier::for_host("interos.org");
    let mut session = TlsClientSession::new(TlsConfig {
        server_name: Some("interos.org"),
        random: [0x55; 32],
        private_key: [0x77; 32],
        verifier: &verifier,
    });
    let mut out = [0u8; 1024];
    let len = session.start_handshake(&mut out).unwrap();
    let record = &out[..len];

    // signature_algorithms: exactly SUPPORTED_SIG_SCHEMES
    let mut checked_scheme = false;
    let mut i = 0;
    while i + 8 <= record.len() {
        if record[i] == 0x00 && record[i + 1] == 0x0d {
            let ext_len = u16::from_be_bytes([record[i + 2], record[i + 3]]) as usize;
            let list_len = u16::from_be_bytes([record[i + 4], record[i + 5]]) as usize;
            assert_eq!(ext_len, 2 + list_len);
            assert_eq!(list_len, SUPPORTED_SIG_SCHEMES.len() * 2);
            for (n, scheme) in SUPPORTED_SIG_SCHEMES.iter().enumerate() {
                assert_eq!(
                    u16::from_be_bytes([record[i + 6 + n * 2], record[i + 7 + n * 2]]),
                    *scheme
                );
            }
            checked_scheme = true;
            break;
        }
        i += 1;
    }
    assert!(checked_scheme, "signature_algorithms extension must be present");

    // supported_groups: exactly SUPPORTED_GROUPS
    let mut checked_group = false;
    i = 0;
    while i + 8 <= record.len() {
        if record[i] == 0x00 && record[i + 1] == 0x0a {
            let list_len = u16::from_be_bytes([record[i + 4], record[i + 5]]) as usize;
            assert_eq!(list_len, SUPPORTED_GROUPS.len() * 2);
            for (n, group) in SUPPORTED_GROUPS.iter().enumerate() {
                assert_eq!(u16::from_be_bytes([record[i + 6 + n * 2], record[i + 7 + n * 2]]), *group);
            }
            checked_group = true;
            break;
        }
        i += 1;
    }
    assert!(checked_group, "supported_groups extension must be present");

    // Cipher suites: exactly SUPPORTED_CIPHER_SUITES (offset 2+32+1+32 = 67 into body)
    let cs_offset = 5 + 4 + 2 + 32 + 1 + 32;
    let cs_len = u16::from_be_bytes([record[cs_offset], record[cs_offset + 1]]) as usize;
    assert_eq!(cs_len, super::handshake::SUPPORTED_CIPHER_SUITES.len() * 2);
    assert_eq!(
        u16::from_be_bytes([record[cs_offset + 2], record[cs_offset + 3]]),
        super::handshake::TLS_CIPHER_CHACHA20_POLY1305_SHA256
    );
}

/// The framing of every extension in the hello, checked by walking the block the
/// way a peer does.
///
/// WHY a walk rather than one more set comparison: `KeyShareEntry
/// client_shares<1..2^16-1>` (RFC 8446 Section 4.2.8) is a vector, so its own
/// two-byte prefix counts towards the extension's content but NOT towards the
/// vector's length. When the encoder added that prefix to both fields, a peer
/// read a 34-byte key, landed two bytes inside the next extension's header and
/// answered illegal_parameter/decode_error without ever naming a set - so every
/// assertion about what the hello advertises stayed green over it.
///
/// The expected lengths are derived from the RFC and from x25519's 32-byte share,
/// not from the encoder's arithmetic, which is the thing under test.
#[test]
fn tls_client_hello_extension_lengths_cover_exactly_their_bytes() {
    let verifier = DefaultCertVerifier::for_host("interos.org");
    let mut session = TlsClientSession::new(TlsConfig {
        server_name: Some("interos.org"),
        random: [0x55; 32],
        private_key: [0x77; 32],
        verifier: &verifier,
    });
    let mut out = [0u8; 1024];
    let len = session.start_handshake(&mut out).unwrap();
    let record = &out[..len];

    assert_eq!(record[0], 0x16, "the record must carry the handshake content type");
    assert_eq!(record[5], 0x01, "the handshake message must be a client_hello");

    // Read the hello's fixed part field by field, so where the extension block
    // starts is derived from the bytes rather than remembered from an offset.
    let mut i = 5 + 4 + 2 + 32; // handshake header + legacy_version + random
    i += 1 + record[i] as usize; // legacy_session_id
    i += 2 + u16::from_be_bytes([record[i], record[i + 1]]) as usize; // cipher_suites
    i += 1 + record[i] as usize; // legacy_compression_methods
    let ext_len = u16::from_be_bytes([record[i], record[i + 1]]) as usize;
    let ext_end = i + 2 + ext_len;
    assert_eq!(ext_end, record.len(), "the extensions length must reach the end of the record");

    let mut key_share_at: Option<usize> = None;
    let mut key_share_declared = 0usize;
    let mut pos = i + 2;
    while pos + 4 <= ext_end {
        let kind = u16::from_be_bytes([record[pos], record[pos + 1]]);
        let declared = u16::from_be_bytes([record[pos + 2], record[pos + 3]]) as usize;
        if kind == 0x0033 {
            key_share_at = Some(pos + 4);
            key_share_declared = declared;
        }
        pos += 4 + declared;
    }
    assert_eq!(
        pos, ext_end,
        "an extension declares more or less than the bytes it holds, so a peer that \
         steps by the declared length lands inside the next extension's header"
    );

    let ks = key_share_at.expect("the hello must advertise a key_share");
    assert!(ks + 38 <= record.len(), "the key_share content the hello declares is not in the record");
    let vector_len = u16::from_be_bytes([record[ks], record[ks + 1]]) as usize;
    let key_len = u16::from_be_bytes([record[ks + 4], record[ks + 5]]) as usize;
    assert_eq!(key_len, 32, "the advertised group is x25519, whose share is 32 bytes");
    assert_eq!(vector_len, 36, "client_shares counts its entries (2+2+32), not its own two prefix bytes");
    assert_eq!(key_share_declared, 38, "the key_share extension covers the vector prefix once");
}

#[test]
fn tls_rejects_server_key_share_for_unadvertised_group() {
    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::ClientHelloSent;
    let (msg, len) = build_server_hello(0x001e); // x448: never advertised
    assert_eq!(
        hs.process_server_hello(&msg[..len]),
        Err(TlsError::BadHandshakeMessage)
    );
}

#[test]
fn tls_accepts_server_key_share_for_advertised_group() {
    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::ClientHelloSent;
    let (msg, len) = build_server_hello(SUPPORTED_GROUPS[0]);
    assert!(hs.process_server_hello(&msg[..len]).is_ok());
}

#[test]
fn tls_server_hello_short_body_is_rejected_without_overflow() {
    // A ServerHello whose body stops exactly at the session-id length field must
    // be rejected: reading that byte requires one more byte than declared.
    let mut msg = [0u8; 64];
    msg[0] = 2; // HANDSHAKE_SERVER_HELLO
    msg[1] = 0;
    msg[2] = 0;
    msg[3] = 34;
    for i in 4..4 + 34 {
        msg[i] = 0x03;
    }
    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::ClientHelloSent;
    assert_eq!(
        hs.process_server_hello(&msg[..4 + 34]),
        Err(TlsError::BadHandshakeMessage)
    );
}

// ---------------------------------------------------------------------------
// Reference identity (hostname) enforcement on the live handshake path
// ---------------------------------------------------------------------------

#[test]
fn tls_leaf_position_hostname_mismatch_fails_closed_even_against_stub() {
    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::EncryptedExtensionsReceived;
    let mut msg = [0u8; 1024];
    let len = build_certificate_message(&[LEAF_GOOD_DER], &mut msg);
    // The stub answers Ok to every policy question; the transport must still
    // reject a certificate that does not carry the advertised identity.
    let res = hs.process_certificate(&msg[..len], &AnyPassVerifier, "evil.example").err();
    assert_eq!(res, Some(TlsError::CertificateVerificationFailed));
    assert_eq!(hs.state, HandshakeState::EncryptedExtensionsReceived);

    // ...and at the exact-match position too: a near-miss prefix never passes.
    let mut hs2 = HandshakeClient::new([0x11u8; 32]);
    hs2.state = HandshakeState::EncryptedExtensionsReceived;
    assert_eq!(
        hs2.process_certificate(&msg[..len], &AnyPassVerifier, "docs.interos.dev.evil.test").err(),
        Some(TlsError::CertificateVerificationFailed)
    );
}

#[test]
fn tls_leaf_position_empty_reference_identity_fails_closed() {
    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::EncryptedExtensionsReceived;
    let mut msg = [0u8; 1024];
    let len = build_certificate_message(&[LEAF_GOOD_DER], &mut msg);
    assert_eq!(
        hs.process_certificate(&msg[..len], &AnyPassVerifier, "").err(),
        Some(TlsError::CertificateVerificationFailed)
    );
    assert_eq!(hs.state, HandshakeState::EncryptedExtensionsReceived);
}

#[test]
fn tls_leaf_position_oversized_reference_identity_fails_closed() {
    let cert = ParsedCertificate::parse(SAN_INTEROS_CERT_DER).unwrap();
    let long = core::str::from_utf8(OVER_LONG_HOST).unwrap();
    assert!(!cert.matches_hostname(long));
    // And the SAN scan refuses to treat it as an identity at all.
    let san = SubjectAltNames::scan(cert.tbs_raw, long);
    assert!(!san.matched);
}

#[test]
fn tls_non_leaf_positions_are_not_trusted_without_a_proved_link() {
    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::EncryptedExtensionsReceived;
    let mut msg = [0u8; 1024];

    // Positive control: this pair DOES chain (real CA signature over the leaf),
    // so the walk must accept it. Anything else below removes one proof only.
    let len = build_certificate_message(&[LEAF_GOOD_DER, CA_ED25519_DER], &mut msg);
    let leaf = hs
        .process_certificate(&msg[..len], &AnyPassVerifier, "docs.interos.dev")
        .expect("leaf + real issuing CA must walk");
    assert_eq!(leaf.extract_common_name(), Some("docs.interos.dev"));

    // Identity removed: the same chain is refused at the leaf position.
    let mut hs2 = HandshakeClient::new([0x11u8; 32]);
    hs2.state = HandshakeState::EncryptedExtensionsReceived;
    assert_eq!(
        hs2.process_certificate(&msg[..len], &AnyPassVerifier, "evil.example").err(),
        Some(TlsError::CertificateVerificationFailed)
    );

    // Second entry does not continue the chain (its subject is not the leaf's issuer).
    let mut hs3 = HandshakeClient::new([0x11u8; 32]);
    hs3.state = HandshakeState::EncryptedExtensionsReceived;
    let len3 = build_certificate_message(&[LEAF_GOOD_DER, SAN_INTEROS_CERT_DER], &mut msg);
    assert_eq!(
        hs3.process_certificate(&msg[..len3], &AnyPassVerifier, "docs.interos.dev").err(),
        Some(TlsError::CertificateVerificationFailed)
    );

    // Chain by name, but the issuer signature over the leaf is corrupted: the
    // link proof is actually computed and must fail closed (never discarded).
    let mut tampered = [0u8; 512];
    tampered[..LEAF_GOOD_DER.len()].copy_from_slice(LEAF_GOOD_DER);
    let last = LEAF_GOOD_DER.len() - 1;
    tampered[last] ^= 0x01;
    let mut hs4 = HandshakeClient::new([0x11u8; 32]);
    hs4.state = HandshakeState::EncryptedExtensionsReceived;
    let len4 = build_certificate_message(&[&tampered[..LEAF_GOOD_DER.len()], CA_ED25519_DER], &mut msg);
    assert_eq!(
        hs4.process_certificate(&msg[..len4], &AnyPassVerifier, "docs.interos.dev").err(),
        // The scheme's own answer is what surfaces: an Ed25519 link that does not
        // check is reported as the invalid signature it is, not as a generic
        // verification failure that would hide which link broke.
        Some(TlsError::InvalidSignature)
    );
    assert_eq!(hs4.state, HandshakeState::EncryptedExtensionsReceived);
}

#[test]
fn tls_chain_deeper_than_the_cap_is_refused() {
    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::EncryptedExtensionsReceived;
    let mut msg = [0u8; 4096];
    // leaf + CA x4: every link is provable (the CA self-signs), but the list is
    // longer than MAX_CHAIN_DEPTH, so the walk refuses instead of truncating.
    let entries = [
        LEAF_GOOD_DER,
        CA_ED25519_DER,
        CA_ED25519_DER,
        CA_ED25519_DER,
        CA_ED25519_DER,
    ];
    assert_eq!(entries.len(), MAX_CHAIN_DEPTH + 1);
    let len = build_certificate_message(&entries, &mut msg);
    assert_eq!(
        hs.process_certificate(&msg[..len], &AnyPassVerifier, "docs.interos.dev").err(),
        Some(TlsError::CertificateVerificationFailed)
    );
    assert_eq!(hs.state, HandshakeState::EncryptedExtensionsReceived);

    // Exactly at the cap the walk is allowed to finish.
    let at_cap = [LEAF_GOOD_DER, CA_ED25519_DER, CA_ED25519_DER, CA_ED25519_DER];
    let mut hs2 = HandshakeClient::new([0x11u8; 32]);
    hs2.state = HandshakeState::EncryptedExtensionsReceived;
    let len2 = build_certificate_message(&at_cap, &mut msg);
    assert!(hs2.process_certificate(&msg[..len2], &AnyPassVerifier, "docs.interos.dev").is_ok());
}

#[test]
fn tls_certificate_list_declared_longer_than_payload_is_rejected() {
    let mut msg = [0u8; 1024];
    let len = build_certificate_message(&[LEAF_GOOD_DER], &mut msg);
    // Inflate the certificate_list length beyond the record.
    let list_len_field = 4 + 1;
    msg[list_len_field] = 0xff;
    msg[list_len_field + 1] = 0xff;
    msg[list_len_field + 2] = 0xff;
    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::EncryptedExtensionsReceived;
    assert_eq!(
        hs.process_certificate(&msg[..len], &AnyPassVerifier, "docs.interos.dev").err(),
        Some(TlsError::BadHandshakeMessage)
    );
}

#[test]
fn tls_message_order_is_enforced() {
    let cert = ParsedCertificate::parse(RFC8032_CERT_DER).unwrap();
    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::ClientHelloSent;
    // CertificateVerify before a Certificate has been accepted.
    let cv = [HANDSHAKE_CERTIFICATE_VERIFY, 0, 0, 6, 0x08, 0x07, 0, 2, 0xaa, 0xbb];
    assert_eq!(
        hs.process_certificate_verify(&cv, cert.spki_public_key, &AnyPassVerifier),
        Err(TlsError::UnexpectedMessage)
    );
    // Finished before CertificateVerify.
    let fin = [20u8, 0, 0, 32];
    assert_eq!(hs.process_server_finished(&fin), Err(TlsError::UnexpectedMessage));
}

#[test]
fn tls_real_ca_signed_leaf_passes_identity_and_trust() {
    // Nothing permissive here: DefaultCertVerifier plus a real CA fixture.
    static ROOTS: [&[u8]; 1] = [CA_ED25519_DER];
    let verifier = DefaultCertVerifier::for_host("docs.interos.dev").with_roots(&ROOTS);
    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::EncryptedExtensionsReceived;
    let mut msg = [0u8; 1024];
    let len = build_certificate_message(&[LEAF_GOOD_DER], &mut msg);
    let leaf = hs
        .process_certificate(&msg[..len], &verifier, "docs.interos.dev")
        .expect("CA-signed leaf with a matching SAN must pass real verification");
    assert!(leaf.matches_hostname("docs.interos.dev"));
    assert!(!leaf.matches_hostname("evil.example"));
    assert_eq!(hs.state, HandshakeState::CertificateReceived);

    // The same leaf against the wrong identity must fail closed.
    let mut hs2 = HandshakeClient::new([0x11u8; 32]);
    hs2.state = HandshakeState::EncryptedExtensionsReceived;
    assert_eq!(
        hs2.process_certificate(&msg[..len], &verifier, "evil.example").err(),
        Some(TlsError::CertificateVerificationFailed)
    );
}

#[test]
fn tls_leaf_signed_by_untrusted_root_fails_closed() {
    // LEAF_GOOD really is signed by CA_ED25519_DER; pinning a different anchor
    // must refuse it even though the name matches.
    static ROOTS: [&[u8]; 1] = [SAN_INTEROS_CERT_DER];
    let verifier = DefaultCertVerifier::for_host("docs.interos.dev").with_roots(&ROOTS);
    let cert = ParsedCertificate::parse(LEAF_GOOD_DER).unwrap();
    assert_eq!(
        verifier.verify_certificate(&cert),
        Err(TlsError::CertificateVerificationFailed)
    );
}

#[test]
fn tls_pinned_self_issued_leaf_passes_only_when_der_pinned() {
    static ROOTS: [&[u8]; 1] = [SAN_INTEROS_CERT_DER];
    let verifier = DefaultCertVerifier::for_host("interos.org").with_roots(&ROOTS);
    let cert = ParsedCertificate::parse(SAN_INTEROS_CERT_DER).unwrap();
    assert!(cert.is_self_signed());
    assert_eq!(verifier.verify_certificate(&cert), Ok(()));

    // Same name, but the anchor is not pinned in the store: refused.
    let unpinned = DefaultCertVerifier::for_host("interos.org");
    assert_eq!(
        unpinned.verify_certificate(&cert),
        Err(TlsError::CertificateVerificationFailed)
    );
}

// ---------------------------------------------------------------------------
// Validity window
// ---------------------------------------------------------------------------

#[test]
fn tls_validity_times_are_read_as_instants_not_as_presence() {
    // CA_ED25519_DER runs 2026-01-01T00:00:00Z to 2035-12-31T00:00:00Z.
    let cert = ParsedCertificate::parse(CA_ED25519_DER).unwrap();
    assert_eq!(asn1_time_to_unix(cert.not_before), Some(1_767_225_600));
    assert_eq!(asn1_time_to_unix(cert.not_after), Some(2_082_672_000));

    // RFC 5280 makes both endpoints part of the valid period, so the instant a
    // certificate becomes valid and the instant it stops being valid are both
    // usable; one second past either is not.
    assert_eq!(cert.check_validity_at(1_767_225_600), Ok(()));
    assert_eq!(cert.check_validity_at(2_082_672_000), Ok(()));
    assert_eq!(
        cert.check_validity_at(1_767_225_599),
        Err(TlsError::CertificateNotYetValid)
    );
    assert_eq!(
        cert.check_validity_at(2_082_672_001),
        Err(TlsError::CertificateExpired)
    );
}

#[test]
fn tls_a_time_this_parser_cannot_read_yields_no_instant() {
    // RFC 5280 wants "Z" and nothing else; a numeric offset is both illegal and
    // silently wrong by hours, so it must produce no instant rather than a
    // plausible one.
    assert_eq!(asn1_time_to_unix(b"260101000000"), None);
    assert_eq!(asn1_time_to_unix(b"2601010000+0530"), None);
    assert_eq!(asn1_time_to_unix(b"20260101000000"), None);
    // Fields that are digits but not dates, and a non-leap 29 February.
    assert_eq!(asn1_time_to_unix(b"260132000000Z"), None);
    assert_eq!(asn1_time_to_unix(b"261301000000Z"), None);
    assert_eq!(asn1_time_to_unix(b"260101240000Z"), None);
    assert_eq!(asn1_time_to_unix(b"260229000000Z"), None);
    assert_eq!(asn1_time_to_unix(b"notatime!!!!!"), None);
    // UTCTime's century window: 50 onward is 19YY, below it is 20YY.
    assert_eq!(asn1_time_to_unix(b"500101000000Z"), Some(-631_152_000));
    assert_eq!(asn1_time_to_unix(b"490101000000Z"), Some(2_493_072_000));
    // GeneralizedTime is the other form, and must land on the same instant.
    assert_eq!(asn1_time_to_unix(b"20351231000000Z"), Some(2_082_672_000));
    assert_eq!(asn1_time_to_unix(b"240229000000Z"), Some(1_709_164_800));
}

#[test]
fn tls_a_verifier_without_a_clock_only_checks_that_times_exist() {
    // What the browser contract is built on: this degradation is real, so the
    // shipping path must supply a clock or refuse the connection outright.
    static ROOTS: [&[u8]; 1] = [CA_ED25519_DER];
    let leaf = ParsedCertificate::parse(LEAF_GOOD_DER).unwrap();
    let no_clock = DefaultCertVerifier::for_host("docs.interos.dev").with_roots(&ROOTS);
    assert_eq!(no_clock.now_unix, None);
    assert_eq!(no_clock.verify_certificate(&leaf), Ok(()));
}

#[test]
fn tls_a_certificate_past_its_expiry_is_refused_end_to_end() {
    // The whole point of the window: a leaf that really is signed by a pinned
    // anchor must still be refused once the anchor's own validity has run out.
    static ROOTS: [&[u8]; 1] = [CA_ED25519_DER];
    let leaf = ParsedCertificate::parse(LEAF_GOOD_DER).unwrap();
    let inside = DefaultCertVerifier::for_host("docs.interos.dev")
        .with_roots(&ROOTS)
        .with_clock(1_767_225_600);
    assert_eq!(inside.verify_certificate(&leaf), Ok(()));

    let expired = DefaultCertVerifier::for_host("docs.interos.dev")
        .with_roots(&ROOTS)
        .with_clock(2_082_672_001);
    assert_eq!(
        expired.verify_certificate(&leaf),
        Err(TlsError::CertificateExpired)
    );
}

#[test]
fn tls_the_handshake_walk_judges_every_entry_against_the_clock() {
    // The verifier is only worth having if the live walk consults it: this is
    // the shape a real Certificate message arrives in, and the refusal has to
    // come from inside the walk rather than from a test calling the verifier.
    static ROOTS: [&[u8]; 1] = [CA_ED25519_DER];
    let mut msg = [0u8; 1024];
    let len = build_certificate_message(&[LEAF_GOOD_DER, CA_ED25519_DER], &mut msg);
    let chain = &msg[..len];

    let stale = DefaultCertVerifier::for_host("docs.interos.dev")
        .with_roots(&ROOTS)
        .with_clock(2_082_672_001);
    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::EncryptedExtensionsReceived;
    assert_eq!(
        hs.process_certificate(chain, &stale, "docs.interos.dev").err(),
        Some(TlsError::CertificateExpired)
    );
    assert_ne!(hs.state, HandshakeState::CertificateReceived);

    let live = DefaultCertVerifier::for_host("docs.interos.dev")
        .with_roots(&ROOTS)
        .with_clock(1_767_225_600);
    let mut hs2 = HandshakeClient::new([0x11u8; 32]);
    hs2.state = HandshakeState::EncryptedExtensionsReceived;
    assert!(hs2.process_certificate(chain, &live, "docs.interos.dev").is_ok());
}

// ---------------------------------------------------------------------------
// Name comparison hardening
// ---------------------------------------------------------------------------

#[test]
fn tls_hostname_match_rejects_empty_names_and_bare_suffix_wildcards() {
    assert!(!hostname_matches("", ""));
    assert!(!hostname_matches("", "interos.org"));
    assert!(!hostname_matches("interos.org", ""));
    assert!(!hostname_matches("*", "interos.org"));
    assert!(!hostname_matches("*.", "a.interos.org"));
    assert!(!hostname_matches("*.com", "evil.com"));
    assert!(!hostname_matches("*.org", "a.interos.org"));
    assert!(!hostname_matches("interos.org", "interos.org.evil.test"));
    assert!(!hostname_matches("interos.org", "xinteros.org"));
    assert!(!hostname_matches("sub.interos.org", "interos.org"));
    // A wildcard never covers more than exactly one label.
    assert!(!hostname_matches("*.interos.org", "a.b.interos.org"));
    assert!(!hostname_matches("*.interos.org", "*.interos.org"));
    assert!(!hostname_matches("*.interos.org", "interos.org"));
    // Legitimate forms still match.
    assert!(hostname_matches("interos.org", "INTEROS.ORG"));
    assert!(hostname_matches("*.interos.org", "docs.interos.org"));
    assert!(hostname_matches("*.interos.org", "DOCS.INTEROS.ORG"));
    // KNOWN LIMITATION, pinned by test: this zero-alloc build carries no public
    // suffix list, so a wildcard whose suffix is itself multi-label is honoured
    // verbatim. A CA-issued "*.co.uk" pattern would therefore match "evil.co.uk".
    // Only the platform's own anchors are in the trust store today, so this is
    // not reachable from a real WebPKI name; it is recorded rather than hidden.
    assert!(hostname_matches("*.co.uk", "evil.co.uk"));
}

#[test]
fn tls_san_present_disables_common_name_fallback() {
    // Same certificate as SAN_INTEROS_CERT_DER but with both dNSNames replaced;
    // the CN still reads "interos.org", which must NOT authorize the identity.
    let mut buf = [0u8; 512];
    let der = SAN_INTEROS_CERT_DER;
    buf[..der.len()].copy_from_slice(der);
    let bytes = &mut buf[..der.len()];

    let wildcard_at = bytes
        .windows(13)
        .position(|w| w == b"*.interos.org")
        .expect("wildcard SAN present in fixture");
    bytes[wildcard_at..wildcard_at + 13].copy_from_slice(b"*.bad.test.cc");

    // The bare dNSName is the one encoded as 0x82 0x0b "interos.org".
    let mut bare_at: Option<usize> = None;
    for i in 0..bytes.len().saturating_sub(13) {
        if bytes[i] == 0x82
            && bytes[i + 1] == 0x0b
            && &bytes[i + 2..i + 13] == b"interos.org"
        {
            bare_at = Some(i + 2);
            break;
        }
    }
    let bare_at = bare_at.expect("dns SAN present in fixture");
    bytes[bare_at..bare_at + 11].copy_from_slice(b"bad.test.cc");

    let cert = ParsedCertificate::parse(&buf[..der.len()]).unwrap();
    assert_eq!(cert.extract_common_name(), Some("interos.org"));
    let san = SubjectAltNames::scan(cert.tbs_raw, "interos.org");
    assert!(san.extension_present && san.dns_name_count == 2 && !san.matched);
    assert!(!cert.matches_hostname("interos.org"));
    assert!(cert.matches_hostname("www.bad.test.cc"));
}

// ---------------------------------------------------------------------------
// Stub / mock containment
// ---------------------------------------------------------------------------

#[test]
fn tls_stub_verifier_without_identity_cannot_start_a_handshake() {
    static STUB: AnyPassVerifier = AnyPassVerifier;
    // A verifier that declares no reference identity is refused at configuration.
    assert_eq!(
        TlsConfig::new("interos.org", [0x11; 32], [0x22; 32], &STUB).err(),
        Some(TlsError::Unavailable)
    );
    // ...and refused again on the live path even when the struct is built by hand.
    let mut session = TlsClientSession::new(TlsConfig {
        server_name: Some("interos.org"),
        random: [0x11; 32],
        private_key: [0x22; 32],
        verifier: &STUB,
    });
    let mut out = [0u8; 1024];
    assert_eq!(session.start_handshake(&mut out), Err(TlsError::Unavailable));
    assert_eq!(session.handle_server_hello(&[0u8; 8]), Err(TlsError::Unavailable));
    assert_eq!(
        session.handle_handshake_message(&[0u8; 8]),
        Err(TlsError::Unavailable)
    );
    assert!(!session.is_connected());
}

#[test]
fn tls_advertised_sni_must_equal_the_pinned_identity() {
    // Verifier pins a different name than the one advertised in SNI.
    let mismatched = DefaultCertVerifier::for_host("other.test");
    assert_eq!(
        TlsConfig::new("interos.org", [0x11; 32], [0x22; 32], &mismatched).err(),
        Some(TlsError::Unavailable)
    );
    // A verifier with no identity at all cannot be used either.
    let unbound = DefaultCertVerifier::new();
    assert_eq!(
        TlsConfig::new("interos.org", [0x11; 32], [0x22; 32], &unbound).err(),
        Some(TlsError::Unavailable)
    );
    // Same failure through the public struct fields, on the handshake entry point.
    static UNBOUND: DefaultCertVerifier<'static> = DefaultCertVerifier::new();
    let mut session = TlsClientSession::new(TlsConfig {
        server_name: Some("interos.org"),
        random: [0x11; 32],
        private_key: [0x22; 32],
        verifier: &UNBOUND,
    });
    let mut out = [0u8; 1024];
    assert_eq!(session.start_handshake(&mut out), Err(TlsError::Unavailable));
}

#[test]
fn tls_client_hello_rejects_empty_and_oversized_peer_name() {
    let long = core::str::from_utf8(OVER_LONG_HOST).unwrap();
    let verifier = DefaultCertVerifier::for_host(long);
    let mut session = TlsClientSession::new(TlsConfig {
        server_name: Some(long),
        random: [0x55; 32],
        private_key: [0x77; 32],
        verifier: &verifier,
    });
    let mut out = [0u8; 1024];
    assert_eq!(session.start_handshake(&mut out), Err(TlsError::Unavailable));

    // A verifier pinned to an empty name is rejected as having no identity.
    static EMPTY_VERIFIER: DefaultCertVerifier<'static> = DefaultCertVerifier {
        expected_hostname: Some(""),
        trusted_roots: super::certificate::DEFAULT_TRUSTED_ROOTS,
        now_unix: None,
    };
    let mut empty_session = TlsClientSession::new(TlsConfig {
        server_name: Some("interos.org"),
        random: [0x55; 32],
        private_key: [0x77; 32],
        verifier: &EMPTY_VERIFIER,
    });
    assert_eq!(
        empty_session.start_handshake(&mut out),
        Err(TlsError::Unavailable)
    );
}

#[test]
fn tls_client_hello_requires_the_whole_record_to_fit_the_buffer() {
    let verifier = DefaultCertVerifier::for_host("interos.org");
    let mut session = TlsClientSession::new(TlsConfig {
        server_name: Some("interos.org"),
        random: [0x55; 32],
        private_key: [0x77; 32],
        verifier: &verifier,
    });
    let mut buf = [0u8; 1024];
    let needed = session
        .start_handshake(&mut buf)
        .expect("a full-size buffer must emit a ClientHello");
    assert_eq!(session.handshake.state, HandshakeState::ClientHelloSent);

    // The guard is the length actually produced, not a placeholder minimum:
    // one byte short fails closed, exactly enough succeeds.
    let mut s2 = TlsClientSession::new(TlsConfig {
        server_name: Some("interos.org"),
        random: [0x55; 32],
        private_key: [0x77; 32],
        verifier: &verifier,
    });
    let mut scratch = [0u8; 1024];
    assert_eq!(
        s2.start_handshake(&mut scratch[..needed - 1]),
        Err(TlsError::BufferTooSmall)
    );
    // No partial transcript state may survive a rejected emit.
    assert_eq!(s2.handshake.state, HandshakeState::Initial);
    assert_eq!(s2.start_handshake(&mut scratch[..needed]), Ok(needed));

    // A maximally long advertised name still has to fit the same record.
    let mut s3 = TlsClientSession::new(TlsConfig {
        server_name: Some(LONG_SNI_NAME),
        random: [0x55; 32],
        private_key: [0x77; 32],
        verifier: &LONG_HOST_VERIFIER,
    });
    let mut small = [0u8; 256];
    // The old guard accepted any buffer >= 256 bytes, then wrote the whole
    // record anyway: with an advertisable 192-byte name the record is ~350
    // bytes, so those bytes went past the end of the caller's slice.
    assert!(LONG_SNI_NAME.len() <= super::certificate::MAX_REFERENCE_IDENTITY_LEN);
    assert!(LONG_SNI_NAME.len() > 100);
    assert_eq!(s3.start_handshake(&mut small), Err(TlsError::BufferTooSmall));
    assert_eq!(s3.handshake.state, HandshakeState::Initial);
}

#[test]
fn tls_certificate_verify_without_a_validated_leaf_is_rejected() {
    static ROOTS: [&[u8]; 1] = [SAN_INTEROS_CERT_DER];
    let verifier = DefaultCertVerifier::for_host("interos.org").with_roots(&ROOTS);
    let mut session = TlsClientSession::new(TlsConfig {
        server_name: Some("interos.org"),
        random: [0x55; 32],
        private_key: [0x77; 32],
        verifier: &verifier,
    });
    // Force the post-ServerHello state without ever receiving a Certificate.
    session.handshake.state = HandshakeState::CertificateReceived;
    session.has_server_cert = false;
    let cv = [HANDSHAKE_CERTIFICATE_VERIFY, 0, 0, 6, 0x08, 0x07, 0, 2, 0xaa, 0xbb];
    let mut enc_buf = [0u8; 256];
    // Without a pinned leaf key the session must refuse, never substitute a key.
    let record_len = {
        use super::record::{ContentType, RecordCipher};
        let mut cipher = RecordCipher::new([0x01u8; 32], [0x02u8; 12]);
        cipher
            .encrypt_record(ContentType::Handshake, &cv, &mut enc_buf)
            .unwrap()
    };
    session.recv_cipher = Some({
        use super::record::RecordCipher;
        RecordCipher::new([0x01u8; 32], [0x02u8; 12])
    });
    let mut plain = [0u8; 256];
    let arrived = session
        .open_handshake_record(&enc_buf[..record_len], &mut plain)
        .unwrap();
    assert_eq!(
        session.handle_handshake_message(&plain[..arrived]),
        Err(TlsError::UnexpectedMessage)
    );
}

// ---------------------------------------------------------------------------
// Closure 1: the reference identity is enforced at every chain position
// ---------------------------------------------------------------------------

#[test]
fn tls_hostname_mismatch_fails_closed_at_every_chain_position() {
    let mut msg = [0u8; 1024];

    // Position 0 (end-entity): the leaf must carry the advertised identity.
    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::EncryptedExtensionsReceived;
    let evil_len = build_certificate_message(&[LEAF_EVIL_DER], &mut msg);
    assert_eq!(
        hs.process_certificate(&msg[..evil_len], &AnyPassVerifier, "docs.interos.dev").err(),
        Some(TlsError::CertificateVerificationFailed)
    );
    assert_eq!(hs.state, HandshakeState::EncryptedExtensionsReceived);

    // Position 0 holding the CA instead of the leaf: the CA's own name is not
    // the advertised identity, so the walk refuses at the first position
    // instead of accepting the CA as the end-entity.
    let mut hs2 = HandshakeClient::new([0x11u8; 32]);
    hs2.state = HandshakeState::EncryptedExtensionsReceived;
    let flipped_len = build_certificate_message(&[CA_ED25519_DER, LEAF_GOOD_DER], &mut msg);
    assert_eq!(
        hs2.process_certificate(&msg[..flipped_len], &AnyPassVerifier, "docs.interos.dev").err(),
        Some(TlsError::CertificateVerificationFailed)
    );
    assert_eq!(hs2.state, HandshakeState::EncryptedExtensionsReceived);

    // Position 1 (issuer): an issuer carries no reference identity of its
    // own, so what decides the chain there is the proved link. The same
    // two certificates, with the identity the leaf really carries, are
    // accepted -- the rejections above are the name, not the chain shape.
    let mut hs3 = HandshakeClient::new([0x11u8; 32]);
    hs3.state = HandshakeState::EncryptedExtensionsReceived;
    let good_len = build_certificate_message(&[LEAF_GOOD_DER, CA_ED25519_DER], &mut msg);
    assert!(hs3
        .process_certificate(&msg[..good_len], &AnyPassVerifier, "docs.interos.dev")
        .is_ok());

    // ...and asking for the issuer's name instead of the leaf's still fails
    // closed at position 0.
    let mut hs4 = HandshakeClient::new([0x11u8; 32]);
    hs4.state = HandshakeState::EncryptedExtensionsReceived;
    assert_eq!(
        hs4.process_certificate(&msg[..good_len], &AnyPassVerifier, "InterOS Test CA").err(),
        Some(TlsError::CertificateVerificationFailed)
    );
}

#[test]
fn tls_empty_and_oversized_reference_identity_fail_closed_at_every_position() {
    let long = core::str::from_utf8(OVER_LONG_HOST).unwrap();
    let mut msg = [0u8; 1024];
    let len = build_certificate_message(&[LEAF_GOOD_DER, CA_ED25519_DER], &mut msg);

    for identity in ["", long] {
        let mut hs = HandshakeClient::new([0x11u8; 32]);
        hs.state = HandshakeState::EncryptedExtensionsReceived;
        assert_eq!(
            hs.process_certificate(&msg[..len], &AnyPassVerifier, identity).err(),
            Some(TlsError::CertificateVerificationFailed),
            "identity {identity:?} must fail closed"
        );
        assert_eq!(hs.state, HandshakeState::EncryptedExtensionsReceived);
    }

    // The verifier itself refuses an empty pinned name: it declares no
    // identity, so it can never be used to check a name.
    static EMPTY: DefaultCertVerifier<'static> = DefaultCertVerifier {
        expected_hostname: Some(""),
        trusted_roots: super::certificate::DEFAULT_TRUSTED_ROOTS,
        now_unix: None,
    };
    assert_eq!(EMPTY.reference_identity(), None);
    let cert = ParsedCertificate::parse(LEAF_GOOD_DER).unwrap();
    assert_eq!(
        EMPTY.verify_certificate(&cert),
        Err(TlsError::CertificateVerificationFailed)
    );

    // An oversized pinned name is refused by the verifier as well.
    static LONG: DefaultCertVerifier<'static> = DefaultCertVerifier {
        expected_hostname: Some(LONG_SNI_NAME),
        trusted_roots: super::certificate::DEFAULT_TRUSTED_ROOTS,
        now_unix: None,
    };
    assert_eq!(LONG.reference_identity(), Some(LONG_SNI_NAME));
    assert_eq!(
        LONG.verify_certificate(&cert),
        Err(TlsError::CertificateVerificationFailed)
    );
}

// ---------------------------------------------------------------------------
// Closure 2: advertised set == accepted set, for every value on the wire
// ---------------------------------------------------------------------------

#[test]
fn tls_rejects_server_hello_for_an_unadvertised_cipher_suite_or_version() {
    // 0x1302 (TLS_AES_128_GCM_SHA256) was never advertised.
    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::ClientHelloSent;
    let (msg, len) = build_server_hello_with(SUPPORTED_GROUPS[0], 0x1302, TLS_VERSION_1_3);
    assert_eq!(
        hs.process_server_hello(&msg[..len]),
        Err(TlsError::UnsupportedCipherSuite)
    );

    // 0x0303 (TLS 1.2) was never advertised.
    let mut hs2 = HandshakeClient::new([0x11u8; 32]);
    hs2.state = HandshakeState::ClientHelloSent;
    let (msg2, len2) =
        build_server_hello_with(SUPPORTED_GROUPS[0], TLS_CIPHER_CHACHA20_POLY1305_SHA256, 0x0303);
    assert_eq!(
        hs2.process_server_hello(&msg2[..len2]),
        Err(TlsError::BadHandshakeMessage)
    );

    // Control: the advertised suite and version are accepted.
    let mut hs3 = HandshakeClient::new([0x11u8; 32]);
    hs3.state = HandshakeState::ClientHelloSent;
    let (msg3, len3) =
        build_server_hello_with(SUPPORTED_GROUPS[0], TLS_CIPHER_CHACHA20_POLY1305_SHA256, TLS_VERSION_1_3);
    assert!(hs3.process_server_hello(&msg3[..len3]).is_ok());
}

#[test]
fn tls_certificate_declaring_an_unadvertised_signature_algorithm_is_refused() {
    // The outer `signatureAlgorithm` of a certificate is NOT covered by the
    // certificate's own signature, so it can be rewritten without breaking
    // the Ed25519 signature over the TBS. A chain entry that declares a
    // scheme this client never advertised must still be refused.
    const SIG_ALG_PATTERN: &[u8] = b"\x30\x05\x06\x03\x2b\x65\x70";

    let mut leaf = [0u8; 512];
    leaf[..LEAF_GOOD_DER.len()].copy_from_slice(LEAF_GOOD_DER);
    let leaf_at = leaf[..LEAF_GOOD_DER.len()]
        .windows(SIG_ALG_PATTERN.len())
        .rposition(|w| w == SIG_ALG_PATTERN)
        .expect("the outer signatureAlgorithm is present in the fixture");
    // The OID content sits after SEQUENCE header + length + OID tag + length.
    leaf[leaf_at + 4..leaf_at + 7].copy_from_slice(&[0x2a, 0x86, 0x48]); // OID 1.2.840

    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::EncryptedExtensionsReceived;
    let mut msg = [0u8; 1024];
    let len = build_certificate_message(&[&leaf[..LEAF_GOOD_DER.len()]], &mut msg);
    assert_eq!(
        hs.process_certificate(&msg[..len], &AnyPassVerifier, "docs.interos.dev").err(),
        Some(TlsError::UnsupportedSignatureScheme)
    );
    assert_eq!(hs.state, HandshakeState::EncryptedExtensionsReceived);

    // The same rewrite on an issuer position is refused as well.
    let mut ca = [0u8; 512];
    ca[..CA_ED25519_DER.len()].copy_from_slice(CA_ED25519_DER);
    let ca_at = ca[..CA_ED25519_DER.len()]
        .windows(SIG_ALG_PATTERN.len())
        .rposition(|w| w == SIG_ALG_PATTERN)
        .expect("the outer signatureAlgorithm is present in the fixture");
    ca[ca_at + 4..ca_at + 7].copy_from_slice(&[0x2a, 0x86, 0x48]);
    let mut hs2 = HandshakeClient::new([0x11u8; 32]);
    hs2.state = HandshakeState::EncryptedExtensionsReceived;
    let len2 = build_certificate_message(&[LEAF_GOOD_DER, &ca[..CA_ED25519_DER.len()]], &mut msg);
    assert_eq!(
        hs2.process_certificate(&msg[..len2], &AnyPassVerifier, "docs.interos.dev").err(),
        Some(TlsError::UnsupportedSignatureScheme)
    );
    assert_eq!(hs2.state, HandshakeState::EncryptedExtensionsReceived);
}

// ---------------------------------------------------------------------------
// Closure 3: no mock may be reported as "verified"
// ---------------------------------------------------------------------------

#[test]
fn tls_shipped_default_trust_store_verifies_nothing() {
    // The synthetic bootstrap CA is a fixture, not a shipped anchor: it is
    // compiled out of every non-test build and the shipped trust store is
    // empty, so this blob can never be reported as "verified" in production.
    assert!(super::certificate::DEFAULT_TRUSTED_ROOTS.is_empty());
    let root = ParsedCertificate::parse(INTEROS_ROOT_CA_DER).unwrap();

    let shipped = DefaultCertVerifier::for_host("InterOS");
    assert_eq!(
        shipped.verify_certificate(&root),
        Err(TlsError::CertificateVerificationFailed)
    );
    assert_eq!(
        shipped.verify_trust_anchor(&root),
        Err(TlsError::CertificateVerificationFailed)
    );

    // It is trusted only when a platform pins it explicitly.
    static ROOTS: [&[u8]; 1] = [INTEROS_ROOT_CA_DER];
    let pinned = DefaultCertVerifier::for_host("InterOS").with_roots(&ROOTS);
    assert_eq!(pinned.verify_certificate(&root), Ok(()));
}

#[test]
fn tls_stub_return_path_cannot_turn_a_failed_check_into_a_verified_handshake() {
    // The stub answers Ok to every policy question it is asked, including
    // the trust-anchor position. Every gate below is enforced by the
    // transport itself, so each one still refuses with the stub in place and
    // the stub never gets to say "verified" on its own.
    let mut evil_msg = [0u8; 1024];
    let evil_len = build_certificate_message(&[LEAF_EVIL_DER], &mut evil_msg);
    let mut good_msg = [0u8; 1024];
    let good_len = build_certificate_message(&[LEAF_GOOD_DER], &mut good_msg);

    // 1. Name mismatch at the leaf position.
    let mut hs = HandshakeClient::new([0x11u8; 32]);
    hs.state = HandshakeState::EncryptedExtensionsReceived;
    assert_eq!(
        hs.process_certificate(&evil_msg[..evil_len], &AnyPassVerifier, "docs.interos.dev").err(),
        Some(TlsError::CertificateVerificationFailed)
    );

    // 2. Empty reference identity.
    let mut hs2 = HandshakeClient::new([0x11u8; 32]);
    hs2.state = HandshakeState::EncryptedExtensionsReceived;
    assert_eq!(
        hs2.process_certificate(&good_msg[..good_len], &AnyPassVerifier, "").err(),
        Some(TlsError::CertificateVerificationFailed)
    );

    // 3. A signature scheme that was never advertised. 0x0804 and 0x0503 became
    // schemes this client does offer, so the code point has to be one it still
    // does not: the gate is the advertised list, whatever is on it.
    let mut hs3 = HandshakeClient::new([0x11u8; 32]);
    hs3.state = HandshakeState::CertificateReceived;
    let cert = ParsedCertificate::parse(RFC8032_CERT_DER).unwrap();
    let cv = [HANDSHAKE_CERTIFICATE_VERIFY, 0, 0, 6, 0x05, 0x04, 0, 2, 0xaa, 0xbb];
    assert_eq!(
        hs3.process_certificate_verify(&cv, cert.spki_public_key, &AnyPassVerifier),
        Err(TlsError::UnsupportedSignatureScheme)
    );
    assert_eq!(hs3.state, HandshakeState::CertificateReceived);

    // 3b. An advertised scheme paired with a leaf key that cannot carry it: the
    // transport refuses the combination itself, so a stub cannot make an Ed25519
    // key sign an RSA-PSS transcript.
    let mut hs3b = HandshakeClient::new([0x11u8; 32]);
    hs3b.state = HandshakeState::CertificateReceived;
    let cv_pss = [HANDSHAKE_CERTIFICATE_VERIFY, 0, 0, 6, 0x08, 0x04, 0, 2, 0xaa, 0xbb];
    assert_eq!(
        hs3b.process_certificate_verify(
            &cv_pss,
            cert.spki_public_key,
            &DefaultCertVerifier::for_host("docs.interos.dev")
        ),
        Err(TlsError::UnsupportedSignatureScheme)
    );
    assert_eq!(hs3b.state, HandshakeState::CertificateReceived);

    // 4. A chain whose issuer does not prove the link.
    let mut hs4 = HandshakeClient::new([0x11u8; 32]);
    hs4.state = HandshakeState::EncryptedExtensionsReceived;
    let mut mixed = [0u8; 1024];
    let mixed_len = build_certificate_message(&[LEAF_GOOD_DER, SAN_INTEROS_CERT_DER], &mut mixed);
    assert_eq!(
        hs4.process_certificate(&mixed[..mixed_len], &AnyPassVerifier, "docs.interos.dev").err(),
        Some(TlsError::CertificateVerificationFailed)
    );
}


// ---------------------------------------------------------------------------
// Handshake records belong to the server; handshake messages belong to the
// protocol, and the two boundaries do not agree.
// ---------------------------------------------------------------------------

/// Writes `bytes` into the rejoiner the way a decrypted record does.
fn feed(reader: &mut super::client::HandshakeReader<'_>, bytes: &[u8]) -> Result<(), TlsError> {
    let room = reader.tail();
    assert!(bytes.len() <= room.len(), "the record does not fit the room under test");
    room[..bytes.len()].copy_from_slice(bytes);
    reader.commit(bytes.len())
}

/// A handshake message of `body_len` bytes, carrying `kind` and a body that
/// counts so a test can see whether the bytes handed over are the ones sent.
fn stamped_message(kind: u8, body_len: usize, out: &mut [u8]) -> usize {
    let total = 4 + body_len;
    out[0] = kind;
    out[1] = (body_len >> 16) as u8;
    out[2] = (body_len >> 8) as u8;
    out[3] = body_len as u8;
    for (i, byte) in out[4..total].iter_mut().enumerate() {
        *byte = (i % 251) as u8;
    }
    total
}

#[test]
fn holds_a_message_until_the_record_that_completes_it_arrives() {
    let mut message = [0u8; 64];
    let total = stamped_message(HANDSHAKE_CERTIFICATE, 40, &mut message);
    assert_eq!(total, 44);

    let mut room = [0u8; 128];
    let mut reader = super::client::HandshakeReader::new(&mut room);
    let mut handed = 0usize;

    // A record that carries only the first 20 bytes cannot be a message yet.
    feed(&mut reader, &message[..20]).unwrap();
    reader.drain(|_| {
        handed += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(handed, 0, "a prefix of a message is not a message");

    // The record that completes it hands over the whole thing, first byte to last.
    feed(&mut reader, &message[20..total]).unwrap();
    let mut seen = [0u8; 64];

    let mut seen_len = 0usize;
    reader
        .drain(|complete| {
            handed += 1;
            seen_len = complete.len();
            seen[..complete.len()].copy_from_slice(complete);
            Ok(())
        })
        .unwrap();
    assert_eq!(handed, 1);
    assert_eq!(seen_len, total);
    assert_eq!(&seen[..total], &message[..total]);
}

#[test]
fn hands_over_every_message_one_record_carries() {
    let mut first = [0u8; 64];
    let mut second = [0u8; 64];
    let first_len = stamped_message(HANDSHAKE_ENCRYPTED_EXTENSIONS, 6, &mut first);
    let second_len = stamped_message(HANDSHAKE_CERTIFICATE_VERIFY, 10, &mut second);

    let mut joined = [0u8; 128];
    joined[..first_len].copy_from_slice(&first[..first_len]);
    joined[first_len..first_len + second_len].copy_from_slice(&second[..second_len]);

    let mut room = [0u8; 128];
    let mut reader = super::client::HandshakeReader::new(&mut room);
    feed(&mut reader, &joined[..first_len + second_len]).unwrap();

    let mut kinds = [0u8; 4];
    let mut handed = 0usize;
    reader
        .drain(|message| {
            kinds[handed] = message[0];
            handed += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(handed, 2, "both messages arrived in one record");
    assert_eq!(kinds[0], HANDSHAKE_ENCRYPTED_EXTENSIONS);
    assert_eq!(kinds[1], HANDSHAKE_CERTIFICATE_VERIFY);
}

#[test]
fn keeps_only_what_still_has_a_message_missing() {
    // A complete message followed by the header of one that has not arrived.
    let mut whole = [0u8; 32];
    let whole_len = stamped_message(HANDSHAKE_FINISHED, 8, &mut whole);
    let mut partial = [0u8; 32];
    let partial_len = stamped_message(HANDSHAKE_CERTIFICATE, 20, &mut partial);

    let mut room = [0u8; 128];
    let mut reader = super::client::HandshakeReader::new(&mut room);
    feed(&mut reader, &whole[..whole_len]).unwrap();
    feed(&mut reader, &partial[..5]).unwrap();

    let mut handed = 0usize;
    reader
        .drain(|message| {
            handed += 1;
            assert_eq!(message[0], HANDSHAKE_FINISHED);
            Ok(())
        })
        .unwrap();
    assert_eq!(handed, 1);

    // The bytes still held are the beginning of the certificate, and the rest
    // of it completes the message on the next record.
    feed(&mut reader, &partial[5..partial_len]).unwrap();
    let mut again = 0usize;
    reader
        .drain(|message| {
            again += 1;
            assert_eq!(message[0], HANDSHAKE_CERTIFICATE);
            assert_eq!(message.len(), partial_len);
            Ok(())
        })
        .unwrap();
    assert_eq!(again, 1);
}

#[test]
fn refuses_a_chain_bigger_than_the_room_it_was_given() {
    // A certificate message that names more bytes than this client will ever
    // reassemble is refused by name; it is not read in pieces and not truncated.
    let mut room = [0u8; super::client::MAX_HANDSHAKE_MESSAGE_BYTES];
    let mut reader = super::client::HandshakeReader::new(&mut room);
    let header = [HANDSHAKE_CERTIFICATE, 0xff, 0xff, 0xff];
    feed(&mut reader, &header).unwrap();
    assert_eq!(
        reader.drain(|_| Ok(())),
        Err(TlsError::RecordTooLarge),
        "a message too large to reassemble must not wait for more records"
    );
}

#[test]
fn the_room_holds_a_public_certificate_chain_whole() {
    // Measured on the wire: the chain developer.mozilla.org sends is a 4113-byte
    // certificate message, and example.com's is 3686 bytes. Both arrive as one
    // protected record larger than the frame this client used to read into.
    const MDN_CERTIFICATE_MESSAGE_BYTES: usize = 4113;
    let mut message = [0u8; MDN_CERTIFICATE_MESSAGE_BYTES];
    let total = stamped_message(
        HANDSHAKE_CERTIFICATE,
        MDN_CERTIFICATE_MESSAGE_BYTES - 4,
        &mut message,
    );
    assert_eq!(total, MDN_CERTIFICATE_MESSAGE_BYTES);

    let mut room = [0u8; super::client::MAX_HANDSHAKE_MESSAGE_BYTES];
    let mut reader = super::client::HandshakeReader::new(&mut room);
    // Split where a server would: partway through the DER of the second certificate.
    feed(&mut reader, &message[..3000]).unwrap();
    let mut handed = 0usize;
    reader.drain(|_| { handed += 1; Ok(()) }).unwrap();
    assert_eq!(handed, 0);
    feed(&mut reader, &message[3000..]).unwrap();
    let mut length_seen = 0usize;
    reader
        .drain(|complete| {
            handed += 1;
            length_seen = complete.len();
            Ok(())
        })
        .unwrap();
    assert_eq!(handed, 1);
    assert_eq!(length_seen, MDN_CERTIFICATE_MESSAGE_BYTES);
}
