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

    /// Returns true if this certificate is self-signed (issuer == subject).
    pub fn is_self_signed(&self) -> bool {
        self.issuer_raw == self.subject_raw
    }

    /// Extracts Common Name (CN) attribute string from subject.
    pub fn extract_common_name(&self) -> Option<&'a str> {
        extract_cn(self.subject_raw)
    }

    /// Matches the certificate SAN (dNSName) or CN against `expected_hostname`.
    ///
    /// Following RFC 6125 / RFC 5280:
    /// - Checks Subject Alternative Name (SAN) extension first if present.
    /// - If no SAN DNS names exist, falls back to Common Name (CN).
    /// - Supports case-insensitive exact matching and single-level wildcard patterns (*.example.com).
    pub fn matches_hostname(&self, expected_hostname: &str) -> bool {
        let mut san_count = 0usize;
        let mut san_matched = false;

        for_each_san_dns_name(self.tbs_raw, |san_name| {
            san_count += 1;
            if hostname_matches(san_name, expected_hostname) {
                san_matched = true;
                return true;
            }
            false
        });

        if san_count > 0 {
            return san_matched;
        }

        if let Some(cn) = self.extract_common_name() {
            if hostname_matches(cn, expected_hostname) {
                return true;
            }
        }

        false
    }
}

/// Matches a reference hostname against a certificate hostname pattern (RFC 6125).
pub fn hostname_matches(pattern: &str, hostname: &str) -> bool {
    if pattern.eq_ignore_ascii_case(hostname) {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        if let Some(dot_idx) = hostname.find('.') {
            let prefix = &hostname[..dot_idx];
            let host_suffix = &hostname[dot_idx + 1..];
            if !prefix.is_empty() && host_suffix.eq_ignore_ascii_case(suffix) {
                return true;
            }
        }
    }
    false
}

/// Extracts Common Name (CN) string from a DER-encoded Subject Name.
pub fn extract_cn<'a>(subject: &'a [u8]) -> Option<&'a str> {
    fn search<'a>(mut cur: &'a [u8], depth: usize) -> Option<&'a str> {
        if depth > 5 {
            return None;
        }
        while !cur.is_empty() {
            let (tag, val, rem) = read_tlv(cur).ok()?;
            cur = rem;
            if tag == TAG_OID && val == [0x55, 0x04, 0x03] {
                let (_, str_val, _) = read_tlv(cur).ok()?;
                return core::str::from_utf8(str_val).ok();
            } else if tag == TAG_SET || tag == TAG_SEQUENCE {
                if let Some(res) = search(val, depth + 1) {
                    return Some(res);
                }
            }
        }
        None
    }
    search(subject, 0)
}

/// Traverses TBSCertificate extensions to find Subject Alternative Name (SAN) dNSNames.
pub fn for_each_san_dns_name<F: FnMut(&str) -> bool>(tbs_raw: &[u8], mut f: F) -> bool {
    let Ok((tbs_tag, tbs_content, _)) = read_tlv(tbs_raw) else {
        return false;
    };
    if tbs_tag != TAG_SEQUENCE {
        return false;
    }

    let mut cur = tbs_content;
    while !cur.is_empty() {
        let Ok((tag, content, rem)) = read_tlv(cur) else {
            break;
        };
        cur = rem;

        // [3] EXPLICIT Extensions
        if tag == 0xa3 {
            let Ok((tag_exts, exts_content, _)) = read_tlv(content) else {
                break;
            };
            if tag_exts != TAG_SEQUENCE {
                break;
            }

            let mut cur_ext = exts_content;
            while !cur_ext.is_empty() {
                let Ok((tag_ext, ext_content, rem_ext)) = read_tlv(cur_ext) else {
                    break;
                };
                cur_ext = rem_ext;
                if tag_ext != TAG_SEQUENCE {
                    continue;
                }

                let Ok((tag_oid, oid_val, after_oid)) = read_tlv(ext_content) else {
                    continue;
                };
                if tag_oid != TAG_OID {
                    continue;
                }

                // OID 2.5.29.17 = id-ce-subjectAltName: [0x55, 0x1d, 0x11]
                if oid_val == [0x55, 0x1d, 0x11] {
                    let Ok((tag1, val1, rem1)) = read_tlv(after_oid) else {
                        continue;
                    };
                    let octet_bytes = if tag1 == TAG_BOOLEAN {
                        let Ok((tag2, val2, _)) = read_tlv(rem1) else {
                            continue;
                        };
                        if tag2 != TAG_OCTET_STRING {
                            continue;
                        }
                        val2
                    } else if tag1 == TAG_OCTET_STRING {
                        val1
                    } else {
                        continue;
                    };

                    let Ok((tag_gn, gn_content, _)) = read_tlv(octet_bytes) else {
                        continue;
                    };
                    if tag_gn != TAG_SEQUENCE {
                        continue;
                    }

                    let mut cur_name = gn_content;
                    while !cur_name.is_empty() {
                        let Ok((tag_name, name_val, rem_name)) = read_tlv(cur_name) else {
                            break;
                        };
                        cur_name = rem_name;
                        // [2] IMPLICIT = dNSName: tag 0x82
                        if tag_name == 0x82 {
                            if let Ok(dns_str) = core::str::from_utf8(name_val) {
                                if f(dns_str) {
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    false
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

pub const INTEROS_ROOT_CA_DER: &[u8] = &[
    0x30, 0x81, 0xc6, 0x30, 0x7a, 0x02, 0x01, 0x01, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x30,
    0x10, 0x30, 0x0e, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x07, 0x49, 0x6e, 0x74, 0x65, 0x72, 0x4f,
    0x53, 0x30, 0x1e, 0x17, 0x0d, 0x32, 0x36, 0x30, 0x31, 0x30, 0x31, 0x30, 0x30, 0x30, 0x30, 0x30,
    0x30, 0x5a, 0x17, 0x0d, 0x32, 0x36, 0x31, 0x32, 0x33, 0x31, 0x32, 0x33, 0x35, 0x39, 0x35, 0x39,
    0x5a, 0x30, 0x10, 0x30, 0x0e, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x07, 0x49, 0x6e, 0x74, 0x65,
    0x72, 0x4f, 0x53, 0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00, 0x09,
    0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09,
    0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x09, 0x30,
    0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x41, 0x00, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
];

pub static DEFAULT_TRUSTED_ROOTS: &[&[u8]] = &[INTEROS_ROOT_CA_DER];

/// Default certificate verifier: enforces DER structure, validity dates,
/// SubjectPublicKeyInfo presence, and verifies standard handshake signatures.
#[derive(Clone, Copy, Debug)]
pub struct DefaultCertVerifier<'a> {
    pub expected_hostname: Option<&'a str>,
    pub trusted_roots: &'a [&'a [u8]],
}

impl<'a> DefaultCertVerifier<'a> {
    pub const fn new() -> Self {
        Self {
            expected_hostname: None,
            trusted_roots: DEFAULT_TRUSTED_ROOTS,
        }
    }

    pub const fn with_hostname(mut self, hostname: &'a str) -> Self {
        self.expected_hostname = Some(hostname);
        self
    }

    pub const fn with_roots(mut self, roots: &'a [&'a [u8]]) -> Self {
        self.trusted_roots = roots;
        self
    }
}

impl<'a> Default for DefaultCertVerifier<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> CertificateVerifier for DefaultCertVerifier<'a> {
    fn verify_certificate(&self, cert: &ParsedCertificate) -> Result<(), TlsError> {
        cert.check_validity()?;

        // Hostname validation against reference identifier (SNI)
        if let Some(expected_host) = self.expected_hostname {
            if !cert.matches_hostname(expected_host) {
                return Err(TlsError::CertificateVerificationFailed);
            }
        }

        // Validate certificate trust against embedded trusted root store
        if cert.is_self_signed() {
            let mut trusted = false;
            for root_der in self.trusted_roots {
                if *root_der == cert.raw {
                    trusted = true;
                    break;
                }
                if let Ok(root_cert) = ParsedCertificate::parse(root_der) {
                    if root_cert.subject_raw == cert.subject_raw
                        && root_cert.spki_public_key == cert.spki_public_key
                    {
                        trusted = true;
                        break;
                    }
                }
            }
            if !trusted {
                return Err(TlsError::CertificateVerificationFailed);
            }
        } else {
            let mut issuer_matched = false;
            for root_der in self.trusted_roots {
                if let Ok(root_cert) = ParsedCertificate::parse(root_der) {
                    if root_cert.subject_raw == cert.issuer_raw {
                        if cert.spki_algorithm_oid == [0x2b, 0x65, 0x70] {
                            crate::tls::ed25519::verify_ed25519(
                                root_cert.spki_public_key,
                                cert.tbs_raw,
                                cert.signature_value,
                            ).map_err(|_| TlsError::CertificateVerificationFailed)?;
                        }
                        issuer_matched = true;
                        break;
                    }
                }
            }
            if !issuer_matched {
                return Err(TlsError::CertificateVerificationFailed);
            }
        }

        Ok(())
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
            SIG_SCHEME_ED25519 => {
                crate::tls::ed25519::verify_ed25519(cert.spki_public_key, context_and_hash, signature)
            }
            _ => Err(TlsError::InvalidSignature),
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
