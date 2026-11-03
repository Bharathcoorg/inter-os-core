// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! TLS 1.3 Client Handshake State Machine (RFC 8446 Section 4 & 7)
//!
//! Handles:
//! - ClientHello encoding (x25519 key share, TLS_CHACHA20_POLY1305_SHA256, SNI)
//! - ServerHello decoding and verification
//! - Cryptographic key schedule (Early Secret -> Handshake Secret -> Master Secret)
//! - EncryptedExtensions, Certificate, CertificateVerify, and Finished handling
//! - Client Finished generation
//! Pure `#![no_std]`, zero heap allocations.

use crate::crypto::hkdf::{derive_secret, empty_hash, hkdf_expand_label, hkdf_extract};
use crate::crypto::hmac::HmacSha256;
use crate::crypto::sha256::Sha256;
use crate::crypto::sha384::Sha384;
use crate::crypto::x25519::{x25519, x25519_base};
use super::certificate::{
    declares_advertised_scheme, is_supported_sig_scheme, spki_is_verifiable, transcript_hash_len,
    verify_link, CertificateVerifier, ParsedCertificate, MAX_REFERENCE_IDENTITY_LEN,
    SUPPORTED_SIG_SCHEMES,
};
use super::ed25519::Sha512;
use super::record::{ContentType, RecordCipher, RECORD_HEADER_LEN};
use super::TlsError;

pub const HANDSHAKE_CLIENT_HELLO: u8 = 1;
pub const HANDSHAKE_SERVER_HELLO: u8 = 2;
pub const HANDSHAKE_ENCRYPTED_EXTENSIONS: u8 = 8;
pub const HANDSHAKE_CERTIFICATE: u8 = 11;
pub const HANDSHAKE_CERTIFICATE_VERIFY: u8 = 15;
pub const HANDSHAKE_FINISHED: u8 = 20;

pub const EXT_SERVER_NAME: u16 = 0x0000;
pub const EXT_SUPPORTED_GROUPS: u16 = 0x000a;
pub const EXT_SIGNATURE_ALGORITHMS: u16 = 0x000d;
pub const EXT_SUPPORTED_VERSIONS: u16 = 0x002b;
pub const EXT_KEY_SHARE: u16 = 0x0033;

pub const TLS_CIPHER_CHACHA20_POLY1305_SHA256: u16 = 0x1303;
pub const GROUP_X25519: u16 = 0x001d;
pub const TLS_VERSION_1_3: u16 = 0x0304;

/// One source of truth for the groups: this exact list is what the ClientHello
/// writes into `supported_groups` and what a ServerHello `key_share` is checked
/// against. A group here is a group we can do X25519 math for, and vice versa.
pub const SUPPORTED_GROUPS: [u16; 1] = [GROUP_X25519];

/// One source of truth for the cipher suites: advertised and accepted from the
/// same list.
pub const SUPPORTED_CIPHER_SUITES: [u16; 1] = [TLS_CIPHER_CHACHA20_POLY1305_SHA256];

/// One source of truth for the supported versions written into `supported_versions`
/// and required of a ServerHello.
pub const SUPPORTED_VERSIONS: [u16; 1] = [TLS_VERSION_1_3];

/// The ClientHello carries exactly one `key_share` entry, derived from the
/// single X25519 private key of this client. Adding a group to the advertised
/// list requires a matching key share, so it is refused at compile time.
const _: () = assert!(SUPPORTED_GROUPS.len() == 1);

/// Worst-case ClientHello body length: every fixed field, every extension
/// this client writes, and a maximum-length SNI name.
///
/// WHY this exists: the body is written into a fixed 512-byte stack buffer. A
/// future extension that pushed the encoding past that buffer would panic the
/// guest mid-handshake instead of refusing the connection, so the bound is
/// checked at compile time rather than discovered on a device.
const MAX_CLIENT_HELLO_BODY_LEN: usize = 2 /* legacy_version */
    + 32 /* random */
    + 1 /* legacy_session_id length */
    + 32 /* legacy_session_id */
    + 2 /* cipher_suites length */
    + 2 * SUPPORTED_CIPHER_SUITES.len()
    + 1 /* compression methods length */
    + 1 /* compression method */
    + 2 /* extensions length */
    + 4 /* supported_versions extension */
    + 2 * SUPPORTED_VERSIONS.len()
    + 4 /* supported_groups extension */
    + 2 * SUPPORTED_GROUPS.len()
    + 4 /* signature_algorithms extension */
    + 2 * SUPPORTED_SIG_SCHEMES.len()
    + 4 /* key_share extension */
    + 2 /* key_share list length */
    + 2 /* key_share group */
    + 2 /* key_share key length */
    + 32 /* key_share key */
    + 4 /* server_name extension */
    + 2 /* server_name list length */
    + 1 /* host_name type */
    + 2 /* host_name length */
    + MAX_REFERENCE_IDENTITY_LEN;

/// The ClientHello body buffer must be able to hold the worst case.
const _: () = assert!(MAX_CLIENT_HELLO_BODY_LEN <= 512);

/// Deepest certificate chain this client will walk before failing closed.
pub const MAX_CHAIN_DEPTH: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeState {
    Initial,
    ClientHelloSent,
    ServerHelloReceived,
    EncryptedExtensionsReceived,
    CertificateReceived,
    CertificateVerifyReceived,
    ServerFinishedReceived,
    Connected,
    Failed,
}

/// The handshake messages hashed with the algorithms the non-SHA-256 advertised
/// signature schemes name.
///
/// WHY a second copy of every update: RFC 8446 Section 4.4.3 takes the
/// `Transcript-Hash` inside `CertificateVerify` from the signature algorithm, not
/// from the cipher suite, so a P-384 leaf signs a SHA-384 hash of exactly the
/// bytes the record layer has already absorbed into SHA-256. The messages
/// themselves are not kept anywhere, so the only way to hold both at the moment
/// the server picks one is to run both over the stream as it passes.
#[derive(Clone, Copy)]
struct SchemeTranscripts {
    sha384: Sha384,
    sha512: Sha512,
}

pub struct HandshakeClient {
    pub state: HandshakeState,
    pub transcript: Sha256,
    scheme_transcripts: SchemeTranscripts,
    pub client_private_key: [u8; 32],
    pub client_public_key: [u8; 32],
    pub server_public_key: [u8; 32],
    pub client_handshake_traffic_secret: [u8; 32],
    pub server_handshake_traffic_secret: [u8; 32],
    pub client_application_traffic_secret: [u8; 32],
    pub server_application_traffic_secret: [u8; 32],
    pub client_handshake_key: [u8; 32],
    pub client_handshake_iv: [u8; 12],
    pub server_handshake_key: [u8; 32],
    pub server_handshake_iv: [u8; 12],
    pub client_application_key: [u8; 32],
    pub client_application_iv: [u8; 12],
    pub server_application_key: [u8; 32],
    pub server_application_iv: [u8; 12],
}

impl HandshakeClient {
    /// Creates a new handshake state machine with the given client private key.
    pub fn new(client_private_key: [u8; 32]) -> Self {
        let client_public_key = x25519_base(&client_private_key);
        Self {
            state: HandshakeState::Initial,
            transcript: Sha256::new(),
            scheme_transcripts: SchemeTranscripts {
                sha384: Sha384::new(),
                sha512: Sha512::new(),
            },
            client_private_key,
            client_public_key,
            server_public_key: [0u8; 32],
            client_handshake_traffic_secret: [0u8; 32],
            server_handshake_traffic_secret: [0u8; 32],
            client_application_traffic_secret: [0u8; 32],
            server_application_traffic_secret: [0u8; 32],
            client_handshake_key: [0u8; 32],
            client_handshake_iv: [0u8; 12],
            server_handshake_key: [0u8; 32],
            server_handshake_iv: [0u8; 12],
            client_application_key: [0u8; 32],
            client_application_iv: [0u8; 12],
            server_application_key: [0u8; 32],
            server_application_iv: [0u8; 12],
        }
    }

    /// Absorbs one handshake message into every transcript this client keeps.
    ///
    /// Every message that reaches the key schedule arrives here, so a signature
    /// scheme whose hash is not SHA-256 cannot find a transcript that has fallen
    /// behind the one the records were encrypted from.
    fn update_transcripts(&mut self, message: &[u8]) {
        self.transcript.update(message);
        self.scheme_transcripts.sha384.update(message);
        self.scheme_transcripts.sha512.update(message);
    }

    /// Encodes a complete ClientHello handshake message inside a TLS record.
    ///
    /// Writes the 5-byte record header followed by ClientHello.
    /// Returns total record length.
    ///
    /// Every list written here (cipher suites, versions, groups, signature
    /// algorithms) comes from the same constants the response parsers enforce,
    /// so the client can never advertise a scheme it cannot verify nor accept
    /// one it did not advertise. A hostname that is empty or longer than the
    /// wire field allows is rejected instead of being silently dropped from SNI.
    pub fn encode_client_hello(
        &mut self,
        random: &[u8; 32],
        sni_hostname: Option<&str>,
        out: &mut [u8],
    ) -> Result<usize, TlsError> {
        let host_bytes: &[u8] = match sni_hostname {
            Some(host) => {
                if host.is_empty() || host.len() > MAX_REFERENCE_IDENTITY_LEN {
                    return Err(TlsError::BufferTooSmall);
                }
                host.as_bytes()
            }
            None => &[],
        };

        let mut body = [0u8; 512];
        let mut pos = 0;

        // legacy_version = 0x0303
        body[pos..pos + 2].copy_from_slice(&0x0303u16.to_be_bytes());
        pos += 2;

        // random (32 bytes)
        body[pos..pos + 32].copy_from_slice(random);
        pos += 32;

        // legacy_session_id (32 bytes for middlebox compatibility)
        body[pos] = 32;
        pos += 1;
        body[pos..pos + 32].copy_from_slice(random); // echo random as session id
        pos += 32;

        // cipher_suites: the advertised set itself
        let cipher_suite_list_len = SUPPORTED_CIPHER_SUITES.len() * 2;
        body[pos..pos + 2].copy_from_slice(&(cipher_suite_list_len as u16).to_be_bytes());
        pos += 2;
        for suite in SUPPORTED_CIPHER_SUITES.iter() {
            body[pos..pos + 2].copy_from_slice(&suite.to_be_bytes());
            pos += 2;
        }

        // legacy_compression_methods: length 1, 0x00
        body[pos] = 1;
        pos += 1;
        body[pos] = 0;
        pos += 1;

        // Extensions
        let ext_len_offset = pos;
        pos += 2; // leave room for 2-byte extensions length

        // 1. supported_versions
        let versions_list_len = SUPPORTED_VERSIONS.len() * 2;
        let versions_ext_len = 1 + versions_list_len;
        body[pos..pos + 2].copy_from_slice(&EXT_SUPPORTED_VERSIONS.to_be_bytes());
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&(versions_ext_len as u16).to_be_bytes());
        pos += 2;
        body[pos] = versions_list_len as u8;
        pos += 1;
        for version in SUPPORTED_VERSIONS.iter() {
            body[pos..pos + 2].copy_from_slice(&version.to_be_bytes());
            pos += 2;
        }

        // 2. supported_groups
        let groups_list_len = SUPPORTED_GROUPS.len() * 2;
        body[pos..pos + 2].copy_from_slice(&EXT_SUPPORTED_GROUPS.to_be_bytes());
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&(2 + groups_list_len as u16).to_be_bytes());
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&(groups_list_len as u16).to_be_bytes());
        pos += 2;
        for group in SUPPORTED_GROUPS.iter() {
            body[pos..pos + 2].copy_from_slice(&group.to_be_bytes());
            pos += 2;
        }

        // 3. signature_algorithms: only schemes this verifier can actually check
        // (certificate::SUPPORTED_SIG_SCHEMES is the same list the CertificateVerify
        // parser and the verifier consume). RFC 8446 Section 4.2.3.
        let schemes_list_len = super::certificate::SUPPORTED_SIG_SCHEMES.len() * 2;
        body[pos..pos + 2].copy_from_slice(&EXT_SIGNATURE_ALGORITHMS.to_be_bytes());
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&(2 + schemes_list_len as u16).to_be_bytes());
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&(schemes_list_len as u16).to_be_bytes());
        pos += 2;
        for scheme in super::certificate::SUPPORTED_SIG_SCHEMES.iter() {
            body[pos..pos + 2].copy_from_slice(&scheme.to_be_bytes());
            pos += 2;
        }

        // 4. key_share: exactly one entry for the single advertised group
        //
        // RFC 8446 Section 4.2.8 declares this extension's content as
        // `KeyShareEntry client_shares<1..2^16-1>`. The vector's two prefix
        // bytes belong to the extension's content but NOT to the vector's own
        // length, so the entry's size is the vector length and the content is
        // that size plus the prefix. Counting the prefix in both made a peer
        // read a 34-byte key, step over the next extension's header and refuse
        // the hello with illegal_parameter before looking at any set we advertise.
        let ks_entry_len = 2 + 2 + self.client_public_key.len();
        body[pos..pos + 2].copy_from_slice(&EXT_KEY_SHARE.to_be_bytes());
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&(2 + ks_entry_len as u16).to_be_bytes());
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&(ks_entry_len as u16).to_be_bytes());
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&SUPPORTED_GROUPS[0].to_be_bytes());
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&(self.client_public_key.len() as u16).to_be_bytes());
        pos += 2;
        body[pos..pos + self.client_public_key.len()].copy_from_slice(&self.client_public_key);
        pos += self.client_public_key.len();

        // 5. Optional SNI: the reference identity we will verify names against
        if !host_bytes.is_empty() {
            let server_name_list_len = 3 + host_bytes.len(); // type (1) + host length (2) + host
            let sni_ext_len = 2 + server_name_list_len;
            body[pos..pos + 2].copy_from_slice(&EXT_SERVER_NAME.to_be_bytes());
            pos += 2;
            body[pos..pos + 2].copy_from_slice(&(sni_ext_len as u16).to_be_bytes());
            pos += 2;
            body[pos..pos + 2].copy_from_slice(&(server_name_list_len as u16).to_be_bytes());
            pos += 2;
            body[pos] = 0; // host_name type
            pos += 1;
            body[pos..pos + 2].copy_from_slice(&(host_bytes.len() as u16).to_be_bytes());
            pos += 2;
            body[pos..pos + host_bytes.len()].copy_from_slice(host_bytes);
            pos += host_bytes.len();
        }

        // Fill total extensions length
        let total_ext_len = pos - ext_len_offset - 2;
        body[ext_len_offset..ext_len_offset + 2].copy_from_slice(&(total_ext_len as u16).to_be_bytes());

        let body_len = pos;

        // Handshake header: msg_type (1 byte) + length (3 bytes)
        let mut hs_msg = [0u8; 516];
        hs_msg[0] = HANDSHAKE_CLIENT_HELLO;
        hs_msg[1] = (body_len >> 16) as u8;
        hs_msg[2] = (body_len >> 8) as u8;
        hs_msg[3] = (body_len & 0xff) as u8;
        hs_msg[4..4 + body_len].copy_from_slice(&body[..body_len]);

        let total_hs_len = 4 + body_len;

        // The record must fit the caller's buffer in full: check the length this
        // encode actually produced, not a placeholder minimum.
        let record_len = RECORD_HEADER_LEN + total_hs_len;
        if record_len > out.len() || total_hs_len > u16::MAX as usize {
            return Err(TlsError::BufferTooSmall);
        }

        // Add to transcript
        self.update_transcripts(&hs_msg[..total_hs_len]);

        // Wrap in plaintext TLS record
        out[0] = ContentType::Handshake as u8;
        out[1..3].copy_from_slice(&0x0303u16.to_be_bytes());
        out[3..5].copy_from_slice(&(total_hs_len as u16).to_be_bytes());
        out[5..5 + total_hs_len].copy_from_slice(&hs_msg[..total_hs_len]);

        self.state = HandshakeState::ClientHelloSent;
        Ok(record_len)
    }

    /// Processes a ServerHello message, performs X25519 key exchange, and derives handshake traffic secrets.
    pub fn process_server_hello(&mut self, payload: &[u8]) -> Result<(), TlsError> {
        if payload.len() < 4 || payload[0] != HANDSHAKE_SERVER_HELLO {
            return Err(TlsError::BadHandshakeMessage);
        }

        let hs_len = ((payload[1] as usize) << 16) | ((payload[2] as usize) << 8) | (payload[3] as usize);
        if payload.len() < 4 + hs_len {
            return Err(TlsError::BadHandshakeMessage);
        }

        let body = &payload[4..4 + hs_len];
        // legacy_version (2) + random (32) + at least the session_id length byte.
        // Reading body[34] requires 35 bytes, so the guard is 35, not 34.
        if body.len() < 35 {
            return Err(TlsError::BadHandshakeMessage);
        }

        // legacy_version = 0x0303
        let legacy_ver = u16::from_be_bytes([body[0], body[1]]);
        if legacy_ver != 0x0303 {
            return Err(TlsError::InvalidRecordVersion);
        }

        // random = 32 bytes
        // session_id
        let session_id_len = body[34] as usize;
        let mut cur = 35 + session_id_len;
        if body.len() < cur + 3 {
            return Err(TlsError::BadHandshakeMessage);
        }

        // cipher_suite: must be one of the suites this client advertised.
        let cipher_suite = u16::from_be_bytes([body[cur], body[cur + 1]]);
        if !SUPPORTED_CIPHER_SUITES.contains(&cipher_suite) {
            return Err(TlsError::UnsupportedCipherSuite);
        }
        cur += 2;

        // legacy_compression_method
        if body[cur] != 0 {
            return Err(TlsError::BadHandshakeMessage);
        }
        cur += 1;

        // Extensions
        if body.len() < cur + 2 {
            return Err(TlsError::BadHandshakeMessage);
        }
        let ext_len = u16::from_be_bytes([body[cur], body[cur + 1]]) as usize;
        cur += 2;

        let ext_end = cur + ext_len;
        if body.len() < ext_end {
            return Err(TlsError::BadHandshakeMessage);
        }

        let mut found_version = false;
        let mut found_key_share = false;

        while cur + 4 <= ext_end {
            let ext_type = u16::from_be_bytes([body[cur], body[cur + 1]]);
            let ext_data_len = u16::from_be_bytes([body[cur + 2], body[cur + 3]]) as usize;
            cur += 4;

            if cur + ext_data_len > ext_end {
                return Err(TlsError::BadHandshakeMessage);
            }

            let ext_data = &body[cur..cur + ext_data_len];
            cur += ext_data_len;

            if ext_type == EXT_SUPPORTED_VERSIONS {
                // The negotiated version must be one this client advertised.
                if ext_data.len() == 2 && SUPPORTED_VERSIONS.contains(&u16::from_be_bytes([ext_data[0], ext_data[1]])) {
                    found_version = true;
                }
            } else if ext_type == EXT_KEY_SHARE {
                // KeyShareEntry: group (2 bytes) + key_exchange length (2 bytes) + key (32 bytes)
                if ext_data.len() >= 36 {
                    let group = u16::from_be_bytes([ext_data[0], ext_data[1]]);
                    let key_len = u16::from_be_bytes([ext_data[2], ext_data[3]]) as usize;
                    // A server picking a group we never advertised is a hard
                    // rejection: accepted groups == advertised groups.
                    if SUPPORTED_GROUPS.contains(&group) && key_len == self.server_public_key.len() {
                        self.server_public_key
                            .copy_from_slice(&ext_data[4..4 + key_len]);
                        found_key_share = true;
                    }
                }
            } else {
                // Unknown/unsolicited extensions in a ServerHello are not
                // negotiated by this client; ignore but never credit them.
            }
        }

        if !found_version || !found_key_share {
            return Err(TlsError::BadHandshakeMessage);
        }

        // Add complete ServerHello to transcript
        self.update_transcripts(&payload[..4 + hs_len]);

        // Key Schedule Derivation
        self.derive_handshake_keys()?;

        self.state = HandshakeState::ServerHelloReceived;
        Ok(())
    }

    fn derive_handshake_keys(&mut self) -> Result<(), TlsError> {
        // 1. Shared secret via X25519
        let dhe = x25519(&self.client_private_key, &self.server_public_key);

        // 2. Early Secret = HKDF-Extract(0, 0)
        let early_secret = hkdf_extract(&[0u8; 32], &[0u8; 32]);

        // 3. Handshake Secret = HKDF-Extract(Derive-Secret(Early Secret, "derived", ""), DHE)
        let derived_early = derive_secret(&early_secret, b"derived", &empty_hash());
        let handshake_secret = hkdf_extract(&derived_early, &dhe);

        // 4. Traffic secrets derived with Transcript-Hash(ClientHello || ServerHello)
        let th = self.transcript.finalize();

        // Restart transcript from previous state:
        // Notice we clone/continue transcript:
        // In our Sha256, finalize takes self, but transcript must continue.
        // We will keep a clone of transcript.
        self.client_handshake_traffic_secret = derive_secret(&handshake_secret, b"c hs traffic", &th);
        self.server_handshake_traffic_secret = derive_secret(&handshake_secret, b"s hs traffic", &th);

        // Derive keys and IVs
        let _ = hkdf_expand_label(&self.client_handshake_traffic_secret, b"key", &[], &mut self.client_handshake_key);
        let _ = hkdf_expand_label(&self.client_handshake_traffic_secret, b"iv", &[], &mut self.client_handshake_iv);
        let _ = hkdf_expand_label(&self.server_handshake_traffic_secret, b"key", &[], &mut self.server_handshake_key);
        let _ = hkdf_expand_label(&self.server_handshake_traffic_secret, b"iv", &[], &mut self.server_handshake_iv);

        // Store handshake secret in temporary slot for master secret derivation later
        // We can store master secret derivation base:
        let derived_hs = derive_secret(&handshake_secret, b"derived", &empty_hash());
        let master_secret = hkdf_extract(&derived_hs, &[0u8; 32]);
        // We cache master secret in self.client_application_traffic_secret temporarily
        self.client_application_traffic_secret = master_secret;

        Ok(())
    }

    /// Processes EncryptedExtensions.
    pub fn process_encrypted_extensions(&mut self, payload: &[u8]) -> Result<(), TlsError> {
        if payload.len() < 4 || payload[0] != HANDSHAKE_ENCRYPTED_EXTENSIONS {
            return Err(TlsError::BadHandshakeMessage);
        }
        let hs_len = ((payload[1] as usize) << 16) | ((payload[2] as usize) << 8) | (payload[3] as usize);
        if payload.len() < 4 + hs_len {
            return Err(TlsError::BadHandshakeMessage);
        }

        self.update_transcripts(&payload[..4 + hs_len]);
        self.state = HandshakeState::EncryptedExtensionsReceived;
        Ok(())
    }

    /// Processes the Certificate message and validates the chain with `verifier`.
    ///
    /// Positions that need identity/chain checks get them, and every position
    /// that fails one fails the whole handshake closed:
    /// - position 0 (end-entity): reference-identity (SAN/CN) match against
    ///   `reference_identity` — the name this client advertised in SNI — plus the
    ///   verifier's trust policy.
    /// - positions 1..: must parse, pass structural validity, carry a key this
    ///   client can verify with, continue the chain (`previous.issuer ==
    ///   subject`) and actually sign the previous entry under the scheme that
    ///   entry declares; the final entry must reach the trust store, either as a
    ///   pinned self-signed anchor or as a certificate a pinned anchor signed —
    ///   see [`CertificateVerifier::verify_reaches_trust_store`].
    /// - a chain longer than [`MAX_CHAIN_DEPTH`] is refused, not truncated, and
    ///   the declared `certificate_list` must be consumed exactly.
    pub fn process_certificate<'a>(
        &mut self,
        payload: &'a [u8],
        verifier: &dyn CertificateVerifier,
        reference_identity: &str,
    ) -> Result<ParsedCertificate<'a>, TlsError> {
        if self.state != HandshakeState::EncryptedExtensionsReceived {
            return Err(TlsError::UnexpectedMessage);
        }
        if payload.len() < 4 || payload[0] != HANDSHAKE_CERTIFICATE {
            return Err(TlsError::BadHandshakeMessage);
        }
        let hs_len = ((payload[1] as usize) << 16) | ((payload[2] as usize) << 8) | (payload[3] as usize);
        if payload.len() < 4 + hs_len {
            return Err(TlsError::BadHandshakeMessage);
        }

        let body = &payload[4..4 + hs_len];
        if body.is_empty() {
            return Err(TlsError::BadHandshakeMessage);
        }

        // certificate_request_context length
        let ctx_len = body[0] as usize;
        let mut cur = 1 + ctx_len;
        if body.len() < cur + 3 {
            return Err(TlsError::BadHandshakeMessage);
        }

        // certificate_list length (3 bytes)
        let list_len = ((body[cur] as usize) << 16) | ((body[cur + 1] as usize) << 8) | (body[cur + 2] as usize);
        cur += 3;
        if list_len < 3 || body.len() < cur + list_len {
            return Err(TlsError::BadHandshakeMessage);
        }

        // Walk only inside the declared list: an entry can never read past it.
        let mut chain = &body[cur..cur + list_len];
        let mut leaf: Option<ParsedCertificate<'a>> = None;
        let mut previous: Option<ParsedCertificate<'a>> = None;
        let mut depth = 0usize;

        while !chain.is_empty() {
            if depth >= MAX_CHAIN_DEPTH {
                return Err(TlsError::CertificateVerificationFailed);
            }
            depth += 1;

            // entry length (3 bytes)
            if chain.len() < 3 {
                return Err(TlsError::BadHandshakeMessage);
            }
            let cert_len = ((chain[0] as usize) << 16) | ((chain[1] as usize) << 8) | (chain[2] as usize);
            chain = &chain[3..];
            if cert_len == 0 || chain.len() < cert_len {
                return Err(TlsError::BadHandshakeMessage);
            }
            let cert_der = &chain[..cert_len];
            chain = &chain[cert_len..];

            // per-entry extensions: 2-byte length that must be inside the list
            if chain.len() < 2 {
                return Err(TlsError::BadHandshakeMessage);
            }
            let ext_len = u16::from_be_bytes([chain[0], chain[1]]) as usize;
            chain = &chain[2..];
            if chain.len() < ext_len {
                return Err(TlsError::BadHandshakeMessage);
            }
            chain = &chain[ext_len..];

            let parsed = ParsedCertificate::parse(cert_der)?;
            // Every position is judged against the verifier's clock, not merely
            // for the presence of two timestamps: an intermediate that expired
            // is refused here rather than trusted because it parsed.
            verifier.check_validity_window(&parsed)?;

            // Only a key this client can do arithmetic with is usable at any
            // chain position: Ed25519, RSA, or an EC P-256 point.
            if !spki_is_verifiable(&parsed) {
                return Err(TlsError::UnsupportedSignatureScheme);
            }
            // The signature algorithm the entry declares must also be one this
            // client advertised. The outer `signatureAlgorithm` field is NOT
            // covered by the certificate's own signature, so a chain entry could
            // otherwise declare a scheme this client never advertised (and never
            // checked) while its bytes still verify under another algorithm.
            // Advertised set == accepted set, here and not inside the verifier,
            // so no verifier can be configured past it.
            if !declares_advertised_scheme(&parsed) {
                return Err(TlsError::UnsupportedSignatureScheme);
            }

            match previous {
                None => {
                    // End-entity position carries the reference identity check.
                    if reference_identity.is_empty()
                        || !parsed.matches_hostname(reference_identity)
                    {
                        return Err(TlsError::CertificateVerificationFailed);
                    }
                    // A server that sends the leaf alone leaves no link to
                    // prove, so the verifier's own end-entity policy -- identity
                    // plus trust -- decides. A deeper chain gets the identity
                    // check here and its trust from the links below, because
                    // requiring the leaf to be signed directly by a pinned root
                    // would refuse every public chain that ships an
                    // intermediate.
                    if chain.is_empty() {
                        verifier.verify_certificate(&parsed)?;
                    }
                    leaf = Some(parsed);
                }
                Some(prev) => {
                    // Issuer position: continue the chain and prove the link.
                    if prev.issuer_raw != parsed.subject_raw {
                        return Err(TlsError::CertificateVerificationFailed);
                    }
                    verify_link(&prev, &parsed)?;
                    if chain.is_empty() {
                        // The head of the path must reach the trust store, whether
                        // the server sent a root or stopped at an intermediate.
                        verifier.verify_reaches_trust_store(&parsed)?;
                    }
                }
            }
            previous = Some(parsed);
        }

        let leaf = leaf.ok_or(TlsError::BadHandshakeMessage)?;

        self.update_transcripts(&payload[..4 + hs_len]);
        self.state = HandshakeState::CertificateReceived;
        Ok(leaf)
    }

    /// Processes CertificateVerify message and checks the digital signature
    /// against the leaf certificate's public key.
    ///
    /// The scheme is only accepted when it is in the advertised list
    /// (`certificate::SUPPORTED_SIG_SCHEMES`); the verification result is
    /// consumed and any failure aborts the handshake.
    pub fn process_certificate_verify(
        &mut self,
        payload: &[u8],
        leaf_public_key: &[u8],
        verifier: &dyn CertificateVerifier,
    ) -> Result<(), TlsError> {
        if self.state != HandshakeState::CertificateReceived {
            return Err(TlsError::UnexpectedMessage);
        }
        if payload.len() < 4 || payload[0] != HANDSHAKE_CERTIFICATE_VERIFY {
            return Err(TlsError::BadHandshakeMessage);
        }
        let hs_len = ((payload[1] as usize) << 16) | ((payload[2] as usize) << 8) | (payload[3] as usize);
        if payload.len() < 4 + hs_len {
            return Err(TlsError::BadHandshakeMessage);
        }

        let body = &payload[4..4 + hs_len];
        if body.len() < 4 {
            return Err(TlsError::BadHandshakeMessage);
        }

        let algorithm = u16::from_be_bytes([body[0], body[1]]);
        // Advertised set == accepted set; never silently try another scheme.
        if !is_supported_sig_scheme(algorithm) {
            return Err(TlsError::UnsupportedSignatureScheme);
        }
        let sig_len = u16::from_be_bytes([body[2], body[3]]) as usize;
        if body.len() < 4 + sig_len {
            return Err(TlsError::BadHandshakeMessage);
        }
        let signature = &body[4..4 + sig_len];

        // Format RFC 8446 Section 4.4.3 signature content:
        // 64 spaces (0x20) || "TLS 1.3, server CertificateVerify" || 0x00 || transcript_hash
        // The hash is the scheme's own, not the cipher suite's, so the frame is
        // sized for the widest advertised scheme and trimmed to the one picked.
        let hash_len =
            transcript_hash_len(algorithm).ok_or(TlsError::UnsupportedSignatureScheme)?;
        let mut hash = [0u8; 64];
        match hash_len {
            32 => hash[..32].copy_from_slice(&self.transcript.finalize()),
            48 => hash[..48].copy_from_slice(&self.scheme_transcripts.sha384.finalize()),
            // The only width left in the table is Ed25519's SHA-512.
            _ => hash.copy_from_slice(&self.scheme_transcripts.sha512.finalize()),
        }
        let mut context_and_hash = [0u8; 162];
        context_and_hash[..64].fill(0x20);
        let ctx = b"TLS 1.3, server CertificateVerify";
        context_and_hash[64..64 + ctx.len()].copy_from_slice(ctx);
        context_and_hash[64 + ctx.len()] = 0;
        let hash_at = 65 + ctx.len();
        context_and_hash[hash_at..hash_at + hash_len].copy_from_slice(&hash[..hash_len]);

        // A discarded result here would silently downgrade to an unverified
        // channel, so the signature outcome gates the state transition.
        verifier.verify_server_signature(
            leaf_public_key,
            algorithm,
            &context_and_hash[..hash_at + hash_len],
            signature,
        )?;

        self.update_transcripts(&payload[..4 + hs_len]);
        self.state = HandshakeState::CertificateVerifyReceived;
        Ok(())
    }

    /// Processes Server Finished message and verifies server verify_data.
    pub fn process_server_finished(&mut self, payload: &[u8]) -> Result<(), TlsError> {
        if self.state != HandshakeState::CertificateVerifyReceived {
            return Err(TlsError::UnexpectedMessage);
        }
        if payload.len() < 4 || payload[0] != HANDSHAKE_FINISHED {
            return Err(TlsError::BadHandshakeMessage);
        }
        let hs_len = ((payload[1] as usize) << 16) | ((payload[2] as usize) << 8) | (payload[3] as usize);
        if payload.len() < 4 + hs_len || hs_len != 32 {
            return Err(TlsError::BadHandshakeMessage);
        }

        let verify_data = &payload[4..4 + 32];

        // Calculate expected verify_data
        let mut finished_key = [0u8; 32];
        let _ = hkdf_expand_label(&self.server_handshake_traffic_secret, b"finished", &[], &mut finished_key);
        let th = self.transcript.finalize();
        let expected = HmacSha256::compute(&finished_key, &th);

        if !HmacSha256::verify(&finished_key, &th, &expected) {
            return Err(TlsError::VerifyDataMismatch);
        }
        let mut diff = 0u8;
        for i in 0..32 {
            diff |= verify_data[i] ^ expected[i];
        }
        if diff != 0 {
            return Err(TlsError::VerifyDataMismatch);
        }

        self.update_transcripts(&payload[..4 + hs_len]);

        // Derive Application Keys
        self.derive_application_keys()?;

        self.state = HandshakeState::ServerFinishedReceived;
        Ok(())
    }

    fn derive_application_keys(&mut self) -> Result<(), TlsError> {
        let master_secret = self.client_application_traffic_secret;
        let th = self.transcript.finalize();

        self.client_application_traffic_secret = derive_secret(&master_secret, b"c ap traffic", &th);
        self.server_application_traffic_secret = derive_secret(&master_secret, b"s ap traffic", &th);

        let _ = hkdf_expand_label(&self.client_application_traffic_secret, b"key", &[], &mut self.client_application_key);
        let _ = hkdf_expand_label(&self.client_application_traffic_secret, b"iv", &[], &mut self.client_application_iv);
        let _ = hkdf_expand_label(&self.server_application_traffic_secret, b"key", &[], &mut self.server_application_key);
        let _ = hkdf_expand_label(&self.server_application_traffic_secret, b"iv", &[], &mut self.server_application_iv);

        Ok(())
    }

    /// Encodes client Finished message into `out`, encrypted using client handshake keys.
    /// Returns total record length.
    pub fn encode_client_finished(
        &mut self,
        handshake_cipher: &mut RecordCipher,
        out: &mut [u8],
    ) -> Result<usize, TlsError> {
        let mut finished_key = [0u8; 32];
        let _ = hkdf_expand_label(&self.client_handshake_traffic_secret, b"finished", &[], &mut finished_key);

        let th = self.transcript.finalize();
        let verify_data = HmacSha256::compute(&finished_key, &th);

        let mut hs_msg = [0u8; 36];
        hs_msg[0] = HANDSHAKE_FINISHED;
        hs_msg[1] = 0;
        hs_msg[2] = 0;
        hs_msg[3] = 32;
        hs_msg[4..36].copy_from_slice(&verify_data);

        // Encrypt as inner Handshake record
        let record_len = handshake_cipher.encrypt_record(ContentType::Handshake, &hs_msg, out)?;

        self.state = HandshakeState::Connected;
        Ok(record_len)
    }
}
