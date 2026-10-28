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
use crate::crypto::x25519::{x25519, x25519_base};
use super::certificate::{CertificateVerifier, ParsedCertificate, SIG_SCHEME_ED25519};
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

pub struct HandshakeClient {
    pub state: HandshakeState,
    pub transcript: Sha256,
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

    /// Encodes a complete ClientHello handshake message inside a TLS record.
    ///
    /// Writes the 5-byte record header followed by ClientHello.
    /// Returns total record length.
    pub fn encode_client_hello(
        &mut self,
        random: &[u8; 32],
        sni_hostname: Option<&str>,
        out: &mut [u8],
    ) -> Result<usize, TlsError> {
        if out.len() < 256 {
            return Err(TlsError::BufferTooSmall);
        }

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

        // cipher_suites: length 2, TLS_CHACHA20_POLY1305_SHA256 (0x1303)
        body[pos..pos + 2].copy_from_slice(&2u16.to_be_bytes());
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&TLS_CIPHER_CHACHA20_POLY1305_SHA256.to_be_bytes());
        pos += 2;

        // legacy_compression_methods: length 1, 0x00
        body[pos] = 1;
        pos += 1;
        body[pos] = 0;
        pos += 1;

        // Extensions
        let ext_len_offset = pos;
        pos += 2; // leave room for 2-byte extensions length

        // 1. supported_versions: 0x002b
        body[pos..pos + 2].copy_from_slice(&EXT_SUPPORTED_VERSIONS.to_be_bytes());
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&3u16.to_be_bytes()); // ext length = 3
        pos += 2;
        body[pos] = 2; // versions list length = 2
        pos += 1;
        body[pos..pos + 2].copy_from_slice(&TLS_VERSION_1_3.to_be_bytes());
        pos += 2;

        // 2. supported_groups: 0x000a
        body[pos..pos + 2].copy_from_slice(&EXT_SUPPORTED_GROUPS.to_be_bytes());
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&4u16.to_be_bytes());
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&2u16.to_be_bytes()); // list length = 2
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&GROUP_X25519.to_be_bytes());
        pos += 2;

        // 3. signature_algorithms: 0x000d
        // Only advertise algorithms supported by our sovereign verifier (RFC 8446 Section 4.2.3): Ed25519.
        body[pos..pos + 2].copy_from_slice(&EXT_SIGNATURE_ALGORITHMS.to_be_bytes());
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&4u16.to_be_bytes()); // length = 4
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&2u16.to_be_bytes()); // list length = 2
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&SIG_SCHEME_ED25519.to_be_bytes());
        pos += 2;

        // 4. key_share: 0x0033
        body[pos..pos + 2].copy_from_slice(&EXT_KEY_SHARE.to_be_bytes());
        pos += 2;
        let ks_ext_len = 2 + 2 + 2 + 32; // list_len (2) + group (2) + key_len (2) + key (32) = 38
        body[pos..pos + 2].copy_from_slice(&(ks_ext_len as u16).to_be_bytes());
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&36u16.to_be_bytes()); // client_shares list length = 36
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&GROUP_X25519.to_be_bytes());
        pos += 2;
        body[pos..pos + 2].copy_from_slice(&32u16.to_be_bytes());
        pos += 2;
        body[pos..pos + 32].copy_from_slice(&self.client_public_key);
        pos += 32;

        // 5. Optional SNI: 0x0000
        if let Some(host) = sni_hostname {
            let host_bytes = host.as_bytes();
            if host_bytes.len() < 256 {
                body[pos..pos + 2].copy_from_slice(&EXT_SERVER_NAME.to_be_bytes());
                pos += 2;
                let sni_len = 2 + 1 + 2 + host_bytes.len();
                body[pos..pos + 2].copy_from_slice(&(sni_len as u16).to_be_bytes());
                pos += 2;
                body[pos..pos + 2].copy_from_slice(&((host_bytes.len() + 3) as u16).to_be_bytes());
                pos += 2;
                body[pos] = 0; // host_name type
                pos += 1;
                body[pos..pos + 2].copy_from_slice(&(host_bytes.len() as u16).to_be_bytes());
                pos += 2;
                body[pos..pos + host_bytes.len()].copy_from_slice(host_bytes);
                pos += host_bytes.len();
            }
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

        // Add to transcript
        self.transcript.update(&hs_msg[..total_hs_len]);

        // Wrap in plaintext TLS record
        out[0] = ContentType::Handshake as u8;
        out[1..3].copy_from_slice(&0x0303u16.to_be_bytes());
        out[3..5].copy_from_slice(&(total_hs_len as u16).to_be_bytes());
        out[5..5 + total_hs_len].copy_from_slice(&hs_msg[..total_hs_len]);

        self.state = HandshakeState::ClientHelloSent;
        Ok(RECORD_HEADER_LEN + total_hs_len)
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
        if body.len() < 34 {
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

        // cipher_suite
        let cipher_suite = u16::from_be_bytes([body[cur], body[cur + 1]]);
        if cipher_suite != TLS_CIPHER_CHACHA20_POLY1305_SHA256 {
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
                if ext_data.len() == 2 && u16::from_be_bytes([ext_data[0], ext_data[1]]) == TLS_VERSION_1_3 {
                    found_version = true;
                }
            } else if ext_type == EXT_KEY_SHARE {
                // KeyShareEntry: group (2 bytes) + key_exchange length (2 bytes) + key (32 bytes)
                if ext_data.len() >= 36 {
                    let group = u16::from_be_bytes([ext_data[0], ext_data[1]]);
                    let key_len = u16::from_be_bytes([ext_data[2], ext_data[3]]) as usize;
                    if group == GROUP_X25519 && key_len == 32 {
                        self.server_public_key.copy_from_slice(&ext_data[4..36]);
                        found_key_share = true;
                    }
                }
            }
        }

        if !found_version || !found_key_share {
            return Err(TlsError::BadHandshakeMessage);
        }

        // Add complete ServerHello to transcript
        self.transcript.update(&payload[..4 + hs_len]);

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

        self.transcript.update(&payload[..4 + hs_len]);
        self.state = HandshakeState::EncryptedExtensionsReceived;
        Ok(())
    }

    /// Processes Certificate message and validates the server certificate with `verifier`.
    pub fn process_certificate<'a>(
        &mut self,
        payload: &'a [u8],
        verifier: &dyn CertificateVerifier,
    ) -> Result<ParsedCertificate<'a>, TlsError> {
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
        if body.len() < cur + list_len || list_len < 3 {
            return Err(TlsError::BadHandshakeMessage);
        }

        // First certificate entry length (3 bytes)
        let cert_len = ((body[cur] as usize) << 16) | ((body[cur + 1] as usize) << 8) | (body[cur + 2] as usize);
        cur += 3;
        if body.len() < cur + cert_len {
            return Err(TlsError::BadHandshakeMessage);
        }

        let cert_der = &body[cur..cur + cert_len];
        let parsed = ParsedCertificate::parse(cert_der)?;
        verifier.verify_certificate(&parsed)?;

        self.transcript.update(&payload[..4 + hs_len]);
        self.state = HandshakeState::CertificateReceived;
        Ok(parsed)
    }

    /// Processes CertificateVerify message and checks the digital signature.
    pub fn process_certificate_verify(
        &mut self,
        payload: &[u8],
        cert: &ParsedCertificate,
        verifier: &dyn CertificateVerifier,
    ) -> Result<(), TlsError> {
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
        if algorithm != SIG_SCHEME_ED25519 {
            return Err(TlsError::UnsupportedSignatureScheme);
        }
        let sig_len = u16::from_be_bytes([body[2], body[3]]) as usize;
        if body.len() < 4 + sig_len {
            return Err(TlsError::BadHandshakeMessage);
        }
        let signature = &body[4..4 + sig_len];

        // Format RFC 8446 Section 4.4.3 signature content:
        // 64 spaces (0x20) || "TLS 1.3, server CertificateVerify" || 0x00 || transcript_hash
        let th = self.transcript.finalize();
        let mut context_and_hash = [0u8; 130];
        context_and_hash[..64].fill(0x20);
        let ctx = b"TLS 1.3, server CertificateVerify";
        context_and_hash[64..64 + ctx.len()].copy_from_slice(ctx);
        context_and_hash[64 + ctx.len()] = 0;
        context_and_hash[65 + ctx.len()..65 + ctx.len() + 32].copy_from_slice(&th);

        verifier.verify_server_signature(cert, algorithm, &context_and_hash, signature)?;

        self.transcript.update(&payload[..4 + hs_len]);
        self.state = HandshakeState::CertificateVerifyReceived;
        Ok(())
    }

    /// Processes Server Finished message and verifies server verify_data.
    pub fn process_server_finished(&mut self, payload: &[u8]) -> Result<(), TlsError> {
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

        self.transcript.update(&payload[..4 + hs_len]);

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
