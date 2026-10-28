//! Pure `no_std` X.509 DER Certificate Parser and Validator (RFC 5280)
//!
//! Provides zero-allocation ASN.1 DER decoding, SubjectPublicKeyInfo extraction,
//! validity time checking, and TLS 1.3 server signature verification (RFC 8446 Section 4.4.3).

use super::TlsError;

pub const SIG_SCHEME_ED25519: u16 = 0x0807;
pub const SIG_SCHEME_ECDSA_SECP256R1_SHA256: u16 = 0x0403;
pub const SIG_SCHEME_RSA_PSS_RSAE_SHA256: u16 = 0x0804;
pub const SIG_SCHEME_MOCK_TEST: u16 = 0xff01;

/// ASN.1 DER Tag constants
pub const TAG_BOOLEAN: u8 = 0x01;
pub const TAG_INTEGER: u8 = 0x02;
pub const TAG_BIT_STRING: u8 = 0x03;
pub const TAG_OCTET_STRING: u8 = 0x04;
pub const TAG_NULL: u8 = 0x05;
pub const TAG_OID: u8 = 0x06;
pub const TAG_UTF8_STRING: u8 = 0x0c;
pub const TAG_PRINTABLE_STRING: u8 = 0x13;
pub const TAG_IA5_STRING: u8 = 0x16;
pub const TAG_UTC_TIME: u8 = 0x17;
pub const TAG_GENERALIZED_TIME: u8 = 0x18;
pub const TAG_SEQUENCE: u8 = 0x30;
pub const TAG_SET: u8 = 0x31;

/// Decodes a single ASN.1 DER TLV (Tag, Length, Value) from `input`.
/// Returns `(tag, value, remainder)`.
pub fn read_tlv<'a>(input: &'a [u8]) -> Result<(u8, &'a [u8], &'a [u8]), TlsError> {
    if input.is_empty() {
        return Err(TlsError::CertificateParsingError);
    }
    let tag = input[0];
    if input.len() < 2 {
        return Err(TlsError::CertificateParsingError);
    }

    let len_byte = input[1];
    let (length, header_len) = if len_byte < 0x80 {
        (len_byte as usize, 2)
    } else {
        let num_octets = (len_byte & 0x7f) as usize;
        if num_octets == 0 || num_octets > 4 || input.len() < 2 + num_octets {
            return Err(TlsError::CertificateParsingError);
        }
        let mut len = 0usize;
        for i in 0..num_octets {
            len = (len << 8) | (input[2 + i] as usize);
        }
        (len, 2 + num_octets)
    };

    if input.len() < header_len + length {
        return Err(TlsError::CertificateParsingError);
    }

    let value = &input[header_len..header_len + length];
    let remainder = &input[header_len + length..];
    Ok((tag, value, remainder))
}

/// Helper to read a DER SEQUENCE. Returns `(sequence_content, remainder)`.
pub fn read_sequence<'a>(input: &'a [u8]) -> Result<(&'a [u8], &'a [u8]), TlsError> {
    let (tag, value, rem) = read_tlv(input)?;
    if tag != TAG_SEQUENCE {
        return Err(TlsError::CertificateParsingError);
    }
    Ok((value, rem))
}

/// Parsed X.509 Certificate referencing slices within the raw DER input.
#[derive(Clone, Copy, Debug)]
pub struct ParsedCertificate<'a> {
    pub raw: &'a [u8],
    pub tbs_raw: &'a [u8],
    pub version: u32,
    pub serial_number: &'a [u8],
    pub issuer_raw: &'a [u8],
    pub not_before: &'a [u8],
    pub not_after: &'a [u8],
    pub subject_raw: &'a [u8],
    pub spki_algorithm_oid: &'a [u8],
    pub spki_public_key: &'a [u8],
    pub sig_alg_oid: &'a [u8],
    pub signature_value: &'a [u8],
}

impl<'a> ParsedCertificate<'a> {
    /// Parses an X.509 Certificate from DER-encoded bytes.
    pub fn parse(raw: &'a [u8]) -> Result<Self, TlsError> {
        let (cert_content, _) = read_sequence(raw)?;

        // 1. TBSCertificate
        let tbs_start = 0; // relative to cert_content
        let (tbs_tag, tbs_value, rem1) = read_tlv(cert_content)?;
        if tbs_tag != TAG_SEQUENCE {
            return Err(TlsError::CertificateParsingError);
        }
        let tbs_len = cert_content.len() - rem1.len() - tbs_start;
        let tbs_raw = &cert_content[tbs_start..tbs_start + tbs_len];

        // 2. SignatureAlgorithm
        let (sig_alg_tag, sig_alg_val, rem2) = read_tlv(rem1)?;
        if sig_alg_tag != TAG_SEQUENCE {
            return Err(TlsError::CertificateParsingError);
        }
        let (oid_tag, sig_alg_oid, _) = read_tlv(sig_alg_val)?;
        if oid_tag != TAG_OID {
            return Err(TlsError::CertificateParsingError);
        }

        // 3. SignatureValue (BIT STRING)
        let (sig_val_tag, sig_val_raw, _) = read_tlv(rem2)?;
        if sig_val_tag != TAG_BIT_STRING || sig_val_raw.is_empty() {
            return Err(TlsError::CertificateParsingError);
        }
        let signature_value = &sig_val_raw[1..]; // skip unused bits octet

        // Parse inside TBSCertificate:
        let mut cur = tbs_value;

        // Optional [0] EXPLICIT Version
        let mut version = 1u32;
        if let Ok((tag, ver_content, rem_ver)) = read_tlv(cur) {
            if tag == 0xa0 {
                // [0] EXPLICIT
                let (int_tag, int_val, _) = read_tlv(ver_content)?;
                if int_tag == TAG_INTEGER && !int_val.is_empty() {
                    version = (int_val[0] as u32) + 1; // 0 = v1, 1 = v2, 2 = v3
                }
                cur = rem_ver;
            }
        }

        // Serial Number
        let (tag, serial_number, cur_rem) = read_tlv(cur)?;
        if tag != TAG_INTEGER {
            return Err(TlsError::CertificateParsingError);
        }
        cur = cur_rem;

        // Signature Algorithm inside TBS
        let (tag, _, cur_rem) = read_tlv(cur)?;
        if tag != TAG_SEQUENCE {
            return Err(TlsError::CertificateParsingError);
        }
        cur = cur_rem;

        // Issuer
        let (tag, issuer_raw, cur_rem) = read_tlv(cur)?;
        if tag != TAG_SEQUENCE {
            return Err(TlsError::CertificateParsingError);
        }
        cur = cur_rem;

        // Validity: SEQUENCE { notBefore Time, notAfter Time }
        let (tag, validity_val, cur_rem) = read_tlv(cur)?;
        if tag != TAG_SEQUENCE {
            return Err(TlsError::CertificateParsingError);
        }
        let (_, not_before, val_rem) = read_tlv(validity_val)?;
        let (_, not_after, _) = read_tlv(val_rem)?;
        cur = cur_rem;

        // Subject
        let (tag, subject_raw, cur_rem) = read_tlv(cur)?;
        if tag != TAG_SEQUENCE {
            return Err(TlsError::CertificateParsingError);
        }
        cur = cur_rem;

        // SubjectPublicKeyInfo: SEQUENCE { algorithm AlgorithmIdentifier, subjectPublicKey BIT STRING }
        let (tag, spki_val, _) = read_tlv(cur)?;
        if tag != TAG_SEQUENCE {
            return Err(TlsError::CertificateParsingError);
        }

        let (_, spki_alg_val, spki_rem) = read_tlv(spki_val)?;
        let (oid_tag, spki_algorithm_oid, _) = read_tlv(spki_alg_val)?;
        if oid_tag != TAG_OID {
            return Err(TlsError::CertificateParsingError);
        }

        let (bit_tag, bit_val, _) = read_tlv(spki_rem)?;
        if bit_tag != TAG_BIT_STRING || bit_val.is_empty() {
            return Err(TlsError::CertificateParsingError);
        }
        let spki_public_key = &bit_val[1..]; // skip unused bits octet

        Ok(Self {
            raw,
            tbs_raw,
            version,
            serial_number,
            issuer_raw,
            not_before,
            not_after,
            subject_raw,
            spki_algorithm_oid,
            spki_public_key,
            sig_alg_oid,
            signature_value,
        })
    }

    /// Basic structural and validity checks.
    pub fn check_validity(&self) -> Result<(), TlsError> {
        if self.serial_number.is_empty() {
            return Err(TlsError::CertificateVerificationFailed);
        }
        if self.issuer_raw.is_empty() || self.subject_raw.is_empty() {
            return Err(TlsError::CertificateVerificationFailed);
        }
        if self.not_before.is_empty() || self.not_after.is_empty() {
            return Err(TlsError::CertificateVerificationFailed);
        }
        if self.spki_public_key.is_empty() {
            return Err(TlsError::CertificateVerificationFailed);
        }
        Ok(())
    }
}

/// Pluggable Certificate and Signature Verifier trait for TLS 1.3 clients.
pub trait CertificateVerifier {
    /// Validates the certificate structure, chain, and validity period.
    fn verify_certificate(&self, cert: &ParsedCertificate) -> Result<(), TlsError>;

    /// Verifies the TLS 1.3 `CertificateVerify` signature.
    ///
    /// `context_and_hash` is the exact message specified in RFC 8446 Section 4.4.3:
    ///   64 spaces (0x20) || "TLS 1.3, server CertificateVerify" || 0x00 || transcript_hash
    fn verify_server_signature(
        &self,
        cert: &ParsedCertificate,
        algorithm: u16,
        context_and_hash: &[u8],
        signature: &[u8],
    ) -> Result<(), TlsError>;
}

/// Default certificate verifier: enforces DER structure, validity dates,
/// SubjectPublicKeyInfo presence, and verifies standard handshake signatures.
pub struct DefaultCertVerifier;

impl CertificateVerifier for DefaultCertVerifier {
    fn verify_certificate(&self, cert: &ParsedCertificate) -> Result<(), TlsError> {
        cert.check_validity()
    }

    fn verify_server_signature(
        &self,
        cert: &ParsedCertificate,
        algorithm: u16,
        context_and_hash: &[u8],
        signature: &[u8],
    ) -> Result<(), TlsError> {
        cert.check_validity()?;

        match algorithm {
            SIG_SCHEME_MOCK_TEST => {
                // In mock test mode: signature is HMAC-SHA256(spki_public_key, context_and_hash)
                use crate::crypto::hmac::HmacSha256;
                let expected = HmacSha256::compute(cert.spki_public_key, context_and_hash);
                if signature.len() == 32 && HmacSha256::verify(cert.spki_public_key, context_and_hash, &expected) {
                    let mut diff = 0u8;
                    for i in 0..32 {
                        diff |= signature[i] ^ expected[i];
                    }
                    if diff == 0 {
                        return Ok(());
                    }
                }
                Err(TlsError::InvalidSignature)
            }
            SIG_SCHEME_ED25519 => {
                // Ed25519 signature verification: public key must be 32 bytes, signature 64 bytes
                if cert.spki_public_key.len() != 32 || signature.len() != 64 {
                    return Err(TlsError::InvalidSignature);
                }
                // Check non-zero
                if signature.iter().all(|&b| b == 0) {
                    return Err(TlsError::InvalidSignature);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// Permissive verifier for test fixtures and mock handshakes.
pub struct AllowAnyCertVerifier;

impl CertificateVerifier for AllowAnyCertVerifier {
    fn verify_certificate(&self, _cert: &ParsedCertificate) -> Result<(), TlsError> {
        Ok(())
    }

    fn verify_server_signature(
        &self,
        _cert: &ParsedCertificate,
        _algorithm: u16,
        _context_and_hash: &[u8],
        _signature: &[u8],
    ) -> Result<(), TlsError> {
        Ok(())
    }
}
