//! Pure `no_std` X.509 DER Certificate Parser and Validator (RFC 5280)
//!
//! Provides zero-allocation ASN.1 DER decoding, SubjectPublicKeyInfo extraction,
//! validity time checking, and TLS 1.3 server signature verification (RFC 8446 Section 4.4.3).

use super::TlsError;

pub const SIG_SCHEME_ED25519: u16 = 0x0807;
pub const SIG_SCHEME_ECDSA_SECP256R1_SHA256: u16 = 0x0403;
/// `ecdsa_secp384r1_sha384` — the scheme an ECDSA chain link carries when the
/// certificate below it sits on P-384 and its issuer hashed with SHA-384.
pub const SIG_SCHEME_ECDSA_SECP384R1_SHA384: u16 = 0x0503;
pub const SIG_SCHEME_RSA_PSS_RSAE_SHA256: u16 = 0x0804;
/// `rsa_pkcs1_sha256` — what a CA certificate signed with
/// `sha256WithRSAEncryption` is verified as, and the scheme a legacy RSA server
/// falls back to when it will not do PSS.
pub const SIG_SCHEME_RSA_PKCS1_SHA256: u16 = 0x0401;

/// Ed25519 `SubjectPublicKeyInfo` algorithm OID (`1.3.101.112`) body bytes.
pub const ED25519_SPKI_OID: [u8; 3] = [0x2b, 0x65, 0x70];
/// `rsaEncryption` (`1.2.840.113549.1.1.1`) — the algorithm a certificate names
/// when its key is an RSA public key.
pub const RSA_ENCRYPTION_OID: [u8; 9] =
    [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
/// `sha256WithRSAEncryption` (`1.2.840.113549.1.1.11`) — the name almost every
/// public CA signs with, including every link of the ISRG chain.
pub const SHA256_WITH_RSA_OID: [u8; 9] =
    [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
/// `ecPublicKey` (`1.2.840.10045.2.1`).
pub const EC_PUBLIC_KEY_OID: [u8; 7] = [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
/// `prime256v1` (`1.2.840.10045.3.1.7`) — the curve an EC `AlgorithmIdentifier`
/// names in its parameters.
pub const PRIME256V1_OID: [u8; 8] = [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
/// `secp384r1` (`1.3.132.0.34`) — the other curve name an EC certificate's
/// parameters may carry, and the one the P-384 arithmetic below is keyed to.
pub const SECP384R1_OID: [u8; 5] = [0x2b, 0x81, 0x04, 0x00, 0x22];
/// `ecdsa-with-SHA256` (`1.2.840.10045.4.3.2`).
pub const ECDSA_WITH_SHA256_OID: [u8; 8] = [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
/// `ecdsa-with-SHA384` (`1.2.840.10045.4.3.3`) — the name a P-384 chain link
/// declares, and the reason the hash width travels with the scheme.
pub const ECDSA_WITH_SHA384_OID: [u8; 8] = [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];

/// The largest leaf public key a client has to hold for `CertificateVerify`.
///
/// An RSA-4096 `RSAPublicKey` is the widest of the shapes this client can verify
/// — a 512-byte modulus that DER pads to 513, its INTEGER headers, and the
/// `65537` exponent — which measures 526 bytes. The 97-byte uncompressed P-384
/// point, the 65-byte P-256 point and the 32-byte Ed25519 key fit inside it with
/// room to spare.
pub const MAX_LEAF_PUBLIC_KEY_BYTES: usize = 528;

/// The ONLY signature schemes this client advertises in its ClientHello and the
/// only ones the verifier can actually check. Advertised set and accepted set
/// are this single list; nothing else may add or drop a code point.
///
/// The order is the client's preference: ECDSA P-256 first because it is what
/// the public web signs with most and what the verifier reaches fastest, then
/// P-384 for the chains whose issuer sits on the larger curve, then Ed25519,
/// then the two RSA readings a legacy endpoint may pick.
pub const SUPPORTED_SIG_SCHEMES: [u16; 5] = [
    SIG_SCHEME_ECDSA_SECP256R1_SHA256,
    SIG_SCHEME_ECDSA_SECP384R1_SHA384,
    SIG_SCHEME_ED25519,
    SIG_SCHEME_RSA_PSS_RSAE_SHA256,
    SIG_SCHEME_RSA_PKCS1_SHA256,
];

/// True when `scheme` is in [`SUPPORTED_SIG_SCHEMES`].
pub const fn is_supported_sig_scheme(scheme: u16) -> bool {
    let mut i = 0;
    while i < SUPPORTED_SIG_SCHEMES.len() {
        if SUPPORTED_SIG_SCHEMES[i] == scheme {
            return true;
        }
        i += 1;
    }
    false
}

/// The code point a certificate's own `signatureAlgorithm` OID is verified as.
///
/// WHY a mapping rather than a free choice: a chain entry declares the algorithm
/// that signed it, and the client must check it under exactly that declaration.
/// Reading an RSA signature as PSS, or a PSS one as PKCS#1 v1.5, turns a
/// verification failure into a coincidence — so an OID with no entry here is
/// refused rather than guessed at. `id-RSASSA-PSS` (`...1.1.10`) is deliberately
/// absent: it carries its parameters inside the AlgorithmIdentifier and no
/// certificate on the paths this client walks is signed that way.
pub fn scheme_for_sig_oid(oid: &[u8]) -> Option<u16> {
    if oid == ED25519_SPKI_OID {
        // Ed25519 certificates name the same OID for the key and for the
        // signature, because the algorithm IS the key's whole specification.
        Some(SIG_SCHEME_ED25519)
    } else if oid == SHA256_WITH_RSA_OID {
        Some(SIG_SCHEME_RSA_PKCS1_SHA256)
    } else if oid == ECDSA_WITH_SHA256_OID {
        Some(SIG_SCHEME_ECDSA_SECP256R1_SHA256)
    } else if oid == ECDSA_WITH_SHA384_OID {
        Some(SIG_SCHEME_ECDSA_SECP384R1_SHA384)
    } else {
        None
    }
}

/// How many bytes the `Transcript-Hash` a `CertificateVerify` carries is wide
/// under `scheme`, or `None` for a scheme this client did not advertise.
///
/// WHY the scheme rather than the cipher suite decides this: RFC 8446 Section
/// 4.4.3 defines the handshake hash as the one "appropriate to the signature
/// algorithm", so a P-384 leaf hashes the transcript with SHA-384 even while the
/// record layer underneath runs on SHA-256. Advertising a scheme therefore
/// obliges the client to hold that hash of the transcript, which is what keeps
/// this predicate and [`SUPPORTED_SIG_SCHEMES`] from drifting apart.
pub const fn transcript_hash_len(scheme: u16) -> Option<usize> {
    match scheme {
        SIG_SCHEME_ECDSA_SECP256R1_SHA256
        | SIG_SCHEME_RSA_PSS_RSAE_SHA256
        | SIG_SCHEME_RSA_PKCS1_SHA256 => Some(32),
        SIG_SCHEME_ECDSA_SECP384R1_SHA384 => Some(48),
        SIG_SCHEME_ED25519 => Some(64),
        _ => None,
    }
}

/// True when a certificate's own `signatureAlgorithm` names a scheme this client
/// advertised.
///
/// WHY this is asked of the chain rather than of the verifier: the field is not
/// covered by the certificate's signature, so it is the one part of a chain entry
/// a peer can rewrite at will. Checking it inside a verifier would let a
/// permissive verifier accept an entry that declares an algorithm no code here
/// implements.
pub fn declares_advertised_scheme(cert: &ParsedCertificate<'_>) -> bool {
    scheme_for_sig_oid(cert.sig_alg_oid)
        .is_some_and(|scheme| is_supported_sig_scheme(scheme))
}

/// True when `cert` carries a public key this client can verify a signature with.
///
/// The EC case checks the curve parameter as well as the key algorithm: the
/// scalar width, the point encoding and the arithmetic all follow the curve, so
/// accepting `ecPublicKey` while ignoring which curve it names would refuse the
/// key later, in a place that says nothing about why. The listed curves are the
/// ones with an implementation behind them, not the ones ASN.1 can spell.
pub fn spki_is_verifiable(cert: &ParsedCertificate<'_>) -> bool {
    if cert.spki_algorithm_oid == ED25519_SPKI_OID || cert.spki_algorithm_oid == RSA_ENCRYPTION_OID {
        return true;
    }
    cert.spki_algorithm_oid == EC_PUBLIC_KEY_OID
        && (cert.spki_params == PRIME256V1_OID || cert.spki_params == SECP384R1_OID)
}

/// The key algorithm each advertised scheme requires its signer to carry.
fn scheme_key_algorithm(scheme: u16) -> &'static [u8] {
    match scheme {
        SIG_SCHEME_ED25519 => &ED25519_SPKI_OID,
        SIG_SCHEME_ECDSA_SECP256R1_SHA256 | SIG_SCHEME_ECDSA_SECP384R1_SHA384 => {
            &EC_PUBLIC_KEY_OID
        }
        SIG_SCHEME_RSA_PSS_RSAE_SHA256 | SIG_SCHEME_RSA_PKCS1_SHA256 => &RSA_ENCRYPTION_OID,
        _ => &[],
    }
}

/// The curve each ECDSA scheme is keyed to, so a scheme cannot be pointed at the
/// other curve's arithmetic and land on a signature that happens to fit.
const fn scheme_curve_oid(scheme: u16) -> &'static [u8] {
    match scheme {
        SIG_SCHEME_ECDSA_SECP256R1_SHA256 => &PRIME256V1_OID,
        SIG_SCHEME_ECDSA_SECP384R1_SHA384 => &SECP384R1_OID,
        _ => &[],
    }
}

/// Checks `signature` over `message` against the public key `bytes`, under
/// `scheme`.
///
/// This is the one place the accepted schemes are turned into arithmetic, so a
/// chain link and a `CertificateVerify` cannot drift apart in what they accept.
///
/// `algorithm_oid` and `curve_oid` are what the signer's certificate actually
/// names, or empty when the caller holds only key bytes — which is the
/// `CertificateVerify` case, where the chain walk has already refused every key
/// algorithm this client cannot verify. What is never skipped is the key's
/// *shape*: a scheme paired with a key of another type is the one downgrade this
/// layer can still catch on its own, and each advertised scheme needs a width and
/// a leading byte no other key can be made to look like.
pub fn verify_signed_by(
    bytes: &[u8],
    algorithm_oid: &[u8],
    curve_oid: &[u8],
    scheme: u16,
    message: &[u8],
    signature: &[u8],
) -> Result<(), TlsError> {
    if !key_shape_matches(bytes, scheme) {
        return Err(TlsError::UnsupportedSignatureScheme);
    }
    if !algorithm_oid.is_empty() && algorithm_oid != scheme_key_algorithm(scheme) {
        return Err(TlsError::UnsupportedSignatureScheme);
    }
    match scheme {
        SIG_SCHEME_ED25519 => super::ed25519::verify_ed25519(bytes, message, signature),
        SIG_SCHEME_ECDSA_SECP256R1_SHA256 | SIG_SCHEME_ECDSA_SECP384R1_SHA384 => {
            if !curve_oid.is_empty() && curve_oid != scheme_curve_oid(scheme) {
                return Err(TlsError::UnsupportedSignatureScheme);
            }
            // Each curve gets the scalar width its own order needs, and no other
            // curve's: a 48-byte `r` is not a P-256 scalar wearing a hat.
            let verified = if scheme == SIG_SCHEME_ECDSA_SECP384R1_SHA384 {
                crate::crypto::p384::verify(bytes, message, &decode_ecdsa_signature::<96>(signature)?)
            } else {
                crate::crypto::p256::verify(bytes, message, &decode_ecdsa_signature::<64>(signature)?)
            };
            if verified {
                Ok(())
            } else {
                Err(TlsError::CertificateVerificationFailed)
            }
        }
        SIG_SCHEME_RSA_PSS_RSAE_SHA256 | SIG_SCHEME_RSA_PKCS1_SHA256 => {
            let key = parse_rsa_public_key(bytes)?;
            let verified = if scheme == SIG_SCHEME_RSA_PSS_RSAE_SHA256 {
                crate::crypto::rsa::verify_pss_sha256(key.modulus, key.exponent, signature, message)
            } else {
                crate::crypto::rsa::verify_pkcs1_sha256(key.modulus, key.exponent, signature, message)
            };
            if verified {
                Ok(())
            } else {
                Err(TlsError::CertificateVerificationFailed)
            }
        }
        _ => Err(TlsError::UnsupportedSignatureScheme),
    }
}

/// The key each advertised scheme is able to sign with, read from its shape.
fn key_shape_matches(bytes: &[u8], scheme: u16) -> bool {
    match scheme {
        SIG_SCHEME_ED25519 => bytes.len() == 32,
        SIG_SCHEME_ECDSA_SECP256R1_SHA256 => bytes.len() == 65 && bytes[0] == 0x04,
        SIG_SCHEME_ECDSA_SECP384R1_SHA384 => bytes.len() == 97 && bytes[0] == 0x04,
        SIG_SCHEME_RSA_PSS_RSAE_SHA256 | SIG_SCHEME_RSA_PKCS1_SHA256 => {
            !bytes.is_empty() && bytes[0] == TAG_SEQUENCE
        }
        _ => false,
    }
}

/// Checks the signature one chain entry carries against the key of the entry
/// that issued it.
///
/// The scheme comes from `subject`'s own `signatureAlgorithm` field, which names
/// the algorithm its issuer signed with — the issuer certificate's own field
/// names whoever signed *it*, and reading the two as one would check a link with
/// the wrong arithmetic.
pub fn verify_link(
    subject: &ParsedCertificate<'_>,
    issuer: &ParsedCertificate<'_>,
) -> Result<(), TlsError> {
    let scheme = scheme_for_sig_oid(subject.sig_alg_oid)
        .filter(|s| is_supported_sig_scheme(*s))
        .ok_or(TlsError::UnsupportedSignatureScheme)?;
    verify_signed_by(
        issuer.spki_public_key,
        issuer.spki_algorithm_oid,
        issuer.spki_params,
        scheme,
        subject.tbs_raw,
        subject.signature_value,
    )
}

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

/// An RSA key's two integers, as slices of the `RSAPublicKey` DER a certificate
/// stores inside its `subjectPublicKey` bit string.
#[derive(Clone, Copy, Debug)]
pub struct RsaPublicKey<'a> {
    pub modulus: &'a [u8],
    pub exponent: &'a [u8],
}

/// Drops the leading zero byte DER puts on an `INTEGER` whose top bit is set,
/// which marks it positive and is not part of the value.
fn unsigned_integer(value: &[u8]) -> &[u8] {
    if value.len() > 1 && value[0] == 0 {
        &value[1..]
    } else {
        value
    }
}

/// Reads `SEQUENCE { INTEGER modulus, INTEGER exponent }`.
///
/// WHY here rather than in `crypto::rsa`: the primitive takes plain integers and
/// stays independent of any encoding; this layer already owns DER.
pub fn parse_rsa_public_key(der: &[u8]) -> Result<RsaPublicKey<'_>, TlsError> {
    let (content, _) = read_sequence(der)?;
    let (tag, modulus, rem) = read_tlv(content)?;
    if tag != TAG_INTEGER {
        return Err(TlsError::CertificateParsingError);
    }
    let (tag, exponent, _) = read_tlv(rem)?;
    if tag != TAG_INTEGER {
        return Err(TlsError::CertificateParsingError);
    }
    Ok(RsaPublicKey {
        modulus: unsigned_integer(modulus),
        exponent: unsigned_integer(exponent),
    })
}

/// Decodes an ECDSA signature — `SEQUENCE { INTEGER r, INTEGER s }` — into the
/// fixed `r || s` pair a curve's verifier takes: 64 bytes for P-256, 96 for P-384.
///
/// WHY a fixed frame: an integer DER wrote short is left-padded here, which is
/// where a peer could hide a value that is not the one it signed, so anything
/// wider than this curve's scalar is refused. The width travels with the caller
/// because both curves use the same DER shape, and reading a P-384 pair through
/// the P-256 frame would quietly drop the top of `r`.
pub fn decode_ecdsa_signature<const PAIR_LEN: usize>(
    der: &[u8],
) -> Result<[u8; PAIR_LEN], TlsError> {
    let scalar_len = PAIR_LEN / 2;
    let (content, _) = read_sequence(der)?;
    let (tag, r, rem) = read_tlv(content)?;
    if tag != TAG_INTEGER {
        return Err(TlsError::CertificateParsingError);
    }
    let (tag, s, _) = read_tlv(rem)?;
    if tag != TAG_INTEGER {
        return Err(TlsError::CertificateParsingError);
    }
    let mut pair = [0u8; PAIR_LEN];
    let r = unsigned_integer(r);
    let s = unsigned_integer(s);
    if r.len() > scalar_len || s.len() > scalar_len {
        return Err(TlsError::CertificateParsingError);
    }
    pair[scalar_len - r.len()..scalar_len].copy_from_slice(r);
    pair[PAIR_LEN - s.len()..].copy_from_slice(s);
    Ok(pair)
}

/// Days between 1970-01-01 and a proleptic-Gregorian civil date, negative for
/// dates before the epoch.
///
/// WHY the era arithmetic instead of a month table: it needs no lookup array and
/// no mutable state, which keeps the whole time path allocation-free.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let year_of_era = y - era * 400;
    let month_of_year = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_of_year + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365
        + year_of_era / 4
        - year_of_era / 100
        + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if is_leap_year(year) { 29 } else { 28 },
        _ => 0,
    }
}

/// `count` ASCII decimal digits starting at `at`, or `None` if the slice is
/// short or any byte is not a digit.
fn decimal(raw: &[u8], at: usize, count: usize) -> Option<i64> {
    let mut value = 0;
    let mut i = 0;
    while i < count {
        let byte = *raw.get(at + i)?;
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + (byte - b'0') as i64;
        i += 1;
    }
    Some(value)
}

/// Turns one ASN.1 time value into seconds since 1970-01-01T00:00:00Z, or `None`
/// when the value is not a form that can be read as an instant.
///
/// Accepts exactly the two profiles RFC 5280 §4.1.2.5 mandates: UTCTime
/// `YYMMDDHHMMSSZ` (the century window puts `YY` >= 50 in the 1900s and the rest
/// in the 2000s) and GeneralizedTime `YYYYMMDDHHMMSSZ`. Everything else is
/// refused, including a numeric UTC offset: offsets are illegal in a certificate
/// but would otherwise shift the instant by hours without any way to notice.
pub fn asn1_time_to_unix(raw: &[u8]) -> Option<i64> {
    let (year, month, day, time_at) = match raw.len() {
        13 => {
            let yy = decimal(raw, 0, 2)?;
            let century = if yy >= 50 { 1900 } else { 2000 };
            (century + yy, decimal(raw, 2, 2)?, decimal(raw, 4, 2)?, 6)
        }
        15 => (
            decimal(raw, 0, 4)?,
            decimal(raw, 4, 2)?,
            decimal(raw, 6, 2)?,
            8,
        ),
        _ => return None,
    };
    if *raw.last()? != b'Z' {
        return None;
    }
    if month < 1 || month > 12 || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    let hour = decimal(raw, time_at, 2)?;
    let minute = decimal(raw, time_at + 2, 2)?;
    let second = decimal(raw, time_at + 4, 2)?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
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
    /// The `AlgorithmIdentifier` parameters, when they are an OID. That is how
    /// an EC certificate names its curve; RSA carries `NULL` and Ed25519 carries
    /// nothing, and both read as empty here.
    pub spki_params: &'a [u8],
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
        let (oid_tag, spki_algorithm_oid, spki_alg_rem) = read_tlv(spki_alg_val)?;
        if oid_tag != TAG_OID {
            return Err(TlsError::CertificateParsingError);
        }
        // The curve an EC key belongs to is the parameters of its own
        // AlgorithmIdentifier; RSA's NULL and Ed25519's absence are both read
        // as no parameters at all.
        let mut spki_params: &[u8] = &[];
        if let Ok((tag, params, _)) = read_tlv(spki_alg_rem) {
            if tag == TAG_OID {
                spki_params = params;
            }
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
            spki_params,
            spki_public_key,
            sig_alg_oid,
            signature_value,
        })
    }

    /// Structural checks only: every field a certificate has to carry is
    /// present and non-empty.
    ///
    /// WHY this is not enough to trust a certificate: it never asks whether the
    /// instant now falls inside `notBefore`/`notAfter`, so a certificate that
    /// expired last week passed exactly like one issued this morning. Use
    /// [`Self::check_validity_at`] wherever the platform has a clock.
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

    /// The structural checks plus the certificate's own validity window. Both
    /// endpoints are inside the window, as RFC 5280 defines the valid period.
    ///
    /// A window that cannot be read is refused rather than waved through: the
    /// two time formats X.509 allows are UTCTime and GeneralizedTime, and
    /// anything else is either a profile this parser does not claim to speak or
    /// a deliberately malformed certificate.
    pub fn check_validity_at(&self, now_unix: i64) -> Result<(), TlsError> {
        self.check_validity()?;
        let not_before = asn1_time_to_unix(self.not_before)
            .ok_or(TlsError::CertificateVerificationFailed)?;
        let not_after = asn1_time_to_unix(self.not_after)
            .ok_or(TlsError::CertificateVerificationFailed)?;
        if now_unix < not_before {
            return Err(TlsError::CertificateNotYetValid);
        }
        if now_unix > not_after {
            return Err(TlsError::CertificateExpired);
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
    /// - A subjectAltName extension of any type disables the Common Name fallback:
    ///   once a SAN extension is present, only its dNSNames can authorize identity.
    /// - Only when the certificate carries no SAN extension at all does the CN
    ///   become the identifier.
    /// - Exact and single-label wildcard (`*.example.com`) matching, case-insensitive.
    /// - An empty reference identity never matches; this fails closed.
    pub fn matches_hostname(&self, expected_hostname: &str) -> bool {
        if expected_hostname.is_empty() {
            return false;
        }

        let san = SubjectAltNames::scan(self.tbs_raw, expected_hostname);
        if san.extension_present {
            // SAN dominates: a non-matching or DNS-name-free SAN is a rejection,
            // never a reason to consult the Common Name.
            return san.matched;
        }

        match self.extract_common_name() {
            Some(cn) => hostname_matches(cn, expected_hostname),
            None => false,
        }
    }
}

/// Matches a reference hostname against a certificate hostname pattern (RFC 6125).
///
/// Rejects: empty names on either side, wildcards outside the leftmost label,
/// a wildcard whose suffix is empty or a bare top-level label (so `*.com` can
/// never authorize `evil.com`), and any prefix/truncated acceptance.
pub fn hostname_matches(pattern: &str, hostname: &str) -> bool {
    if pattern.is_empty() || hostname.is_empty() {
        return false;
    }

    // A reference identity that itself carries a wildcard is not a name to
    // match: it can only ever arrive from a malformed SNI/URL, so it is
    // refused before any comparison (including exact equality).
    if hostname.bytes().any(|b| b == b'*') {
        return false;
    }

    if pattern.eq_ignore_ascii_case(hostname) {
        // An exact match must be the whole name; this is byte-for-byte equality
        // modulo case, so no prefix of either side can be accepted here.
        return true;
    }

    // RFC 6125 wildcard form: "*." followed by a multi-label suffix. A pattern
    // that embeds '*' anywhere else is not a wildcard we are allowed to apply.
    let suffix = match pattern.strip_prefix("*.") {
        Some(s) => s,
        None => return false,
    };
    if suffix.is_empty() || suffix.bytes().any(|b| b == b'*') {
        return false;
    }
    // The wildcard may cover exactly one label of the reference identity.
    let (prefix, host_suffix) = match split_first_label(hostname) {
        Some(pair) => pair,
        None => return false,
    };
    if prefix.is_empty() || host_suffix.is_empty() {
        return false;
    }
    // The remaining suffix must itself be a fully qualified name (at least one
    // dot), otherwise "*.com" would authorize any registrable ".com" host.
    if !host_suffix.contains('.') {
        return false;
    }
    host_suffix.eq_ignore_ascii_case(suffix)
}

/// Splits `hostname` into its first label and the remainder after the first dot.
fn split_first_label(hostname: &str) -> Option<(&str, &str)> {
    let dot_idx = hostname.find('.')?;
    Some((&hostname[..dot_idx], &hostname[dot_idx + 1..]))
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

/// Upper bound for a reference identity this client may advertise (SNI) and
/// verify. Both the ClientHello ServerName length field and the DER dNSName
/// length are bounded by this, so a longer name is rejected outright rather
/// than silently truncated or dropped from the ClientHello.
pub const MAX_REFERENCE_IDENTITY_LEN: usize = 255;

/// Result of the single shared pass over a TBSCertificate's subjectAltName
/// extensions. One walk feeds both the "is a SAN present" decision and the
/// dNSName match, so no two callers can disagree about the SAN's meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubjectAltNames {
    /// A subjectAltName extension was found, whatever name types it carries.
    pub extension_present: bool,
    /// Number of decodable dNSNames found.
    pub dns_name_count: usize,
    /// At least one dNSName authorizes the reference identity.
    pub matched: bool,
}

impl SubjectAltNames {
    /// Scans TBSCertificate extensions for `id-ce-subjectAltName` (OID 2.5.29.17)
    /// and matches each dNSName against `reference_identity`.
    pub fn scan(tbs_raw: &[u8], reference_identity: &str) -> Self {
        let mut out = Self {
            extension_present: false,
            dns_name_count: 0,
            matched: false,
        };
        if reference_identity.is_empty() || reference_identity.len() > MAX_REFERENCE_IDENTITY_LEN {
            return out;
        }

        let Ok((tbs_tag, tbs_content, _)) = read_tlv(tbs_raw) else {
            return out;
        };
        if tbs_tag != TAG_SEQUENCE {
            return out;
        }

        let mut cur = tbs_content;
        while !cur.is_empty() {
            let Ok((tag, content, rem)) = read_tlv(cur) else {
                break;
            };
            cur = rem;

            // [3] EXPLICIT Extensions
            if tag != 0xa3 {
                continue;
            }
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
                if oid_val != [0x55, 0x1d, 0x11] {
                    continue;
                }
                out.extension_present = true;

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
                    if tag_name != 0x82 {
                        continue;
                    }
                    // An undecodable or over-long dNSName still counts as a SAN
                    // entry (it suppresses the CN fallback) but can never match.
                    out.dns_name_count += 1;
                    if let Ok(dns_str) = core::str::from_utf8(name_val) {
                        if hostname_matches(dns_str, reference_identity) {
                            out.matched = true;
                            return out;
                        }
                    }
                }
            }
        }
        out
    }
}

/// Pluggable Certificate and Signature Verifier trait for TLS 1.3 clients.
pub trait CertificateVerifier {
    /// Validates the certificate structure, chain, and validity period.
    fn verify_certificate(&self, cert: &ParsedCertificate) -> Result<(), TlsError>;

    /// Judges one chain entry against this verifier's clock.
    ///
    /// WHY this is a required method rather than a call to
    /// [`ParsedCertificate::check_validity`] inside the walk: the walk visits
    /// every entry of the chain, but only the end-entity and the anchor get a
    /// `verify_*` call, so an intermediate that expired last month was checked
    /// for presence alone. Asking the verifier keeps one clock decision for all
    /// three positions, and a new verifier cannot implement this trait without
    /// saying what it does with a clock.
    fn check_validity_window(&self, cert: &ParsedCertificate) -> Result<(), TlsError>;

    /// Validates a chain entry occupying the trust-anchor position: it must be
    /// self-signed and a member of this verifier's trust store. No hostname
    /// policy applies at this position — only the end-entity certificate carries
    /// a reference identity.
    fn verify_trust_anchor(&self, cert: &ParsedCertificate) -> Result<(), TlsError>;

    /// Judges the entry a chain ends on: it must reach this verifier's trust
    /// store, either by being a pinned self-signed anchor or by carrying the
    /// signature of one.
    ///
    /// WHY the chain head needs its own question: a TLS 1.3 server usually sends
    /// the leaf and its intermediates and stops there, because the root is
    /// expected to already be in the client's store. Asking only "is this entry
    /// self-signed and pinned?" would refuse every public chain for omitting the
    /// certificate the client already holds, so the head is judged on whether the
    /// path reaches an anchor, not on whether the server handed one over.
    fn verify_reaches_trust_store(&self, cert: &ParsedCertificate) -> Result<(), TlsError>;

    /// The reference identity (RFC 6125) this verifier pins names against.
    ///
    /// `None` means the verifier declares no identity, so it can never drive a
    /// live handshake: the client refuses to emit a ClientHello whose advertised
    /// SNI has no pinned counterpart.
    fn reference_identity(&self) -> Option<&str>;

    /// Verifies the TLS 1.3 `CertificateVerify` signature against the leaf
    /// certificate's public key.
    ///
    /// `public_key` is the leaf `subjectPublicKey` bytes, taken from the
    /// certificate the client actually validated — never a caller-supplied
    /// substitute.
    ///
    /// `context_and_hash` is the exact message specified in RFC 8446 Section 4.4.3:
    ///   64 spaces (0x20) || "TLS 1.3, server CertificateVerify" || 0x00 || transcript_hash
    /// where `transcript_hash` is the hash [`transcript_hash_len`] gives for
    /// `algorithm` — SHA-384 under P-384, SHA-512 under Ed25519 — not the cipher
    /// suite's hash.
    fn verify_server_signature(
        &self,
        public_key: &[u8],
        algorithm: u16,
        context_and_hash: &[u8],
        signature: &[u8],
    ) -> Result<(), TlsError>;
}

/// Synthetic bootstrap InterOS root CA certificate (RFC 5280 DER format).
///
/// **TEST FIXTURE ONLY — NOT A TRUST ANCHOR.**
///
/// This certificate carries placeholder material: its `SubjectPublicKeyInfo`
/// public key is 32 bytes of `0x09` and its signature is 64 bytes of `0x55`.
/// It cannot sign anything and no private key for it exists, so it is a
/// *shape* fixture for the parser, not a credential.
///
/// It is therefore compiled out of every non-test build (`#[cfg(test)]`), so
/// no shipped image exports a mock CA and no production handshake can ever
/// treat this blob as "verified". A platform that wants a trust anchor must
/// install real root DER through [`DefaultCertVerifier::with_roots`].
#[cfg(test)]
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

/// The trust store a shipped client starts with: **empty**.
///
/// WHY this is empty and not a bundled CA: a trust anchor is a claim that a
/// key holder is allowed to vouch for the world. The InterOS tree has no
/// public root CA, so bundling one would be a fabricated credential that a
/// server could present and have "verified".
///
/// Consequences, stated plainly:
/// - A default [`DefaultCertVerifier`] validates **nothing**. Every handshake
///   against it fails closed with [`TlsError::CertificateVerificationFailed`].
/// - A platform that installs real roots must do so explicitly with
///   [`DefaultCertVerifier::with_roots`]; there is no implicit fallback.
pub const DEFAULT_TRUSTED_ROOTS: &[&[u8]] = &[];

/// Default certificate verifier: enforces DER structure, SubjectPublicKeyInfo
/// presence, reference hostname match, and standard handshake signatures.
///
/// It enforces the validity *window* only when the caller supplies a clock with
/// [`DefaultCertVerifier::with_clock`]; without one it can confirm that both
/// timestamps exist but cannot tell an expired certificate from a fresh one.
///
/// The trust store it starts with is [`DEFAULT_TRUSTED_ROOTS`], which is
/// **empty**: a default verifier therefore trusts no certificate at all until
/// a platform installs real roots with [`DefaultCertVerifier::with_roots`].
#[derive(Clone, Copy, Debug)]
pub struct DefaultCertVerifier<'a> {
    pub expected_hostname: Option<&'a str>,
    pub trusted_roots: &'a [&'a [u8]],
    /// The instant validity windows are judged against, or `None` when the
    /// caller has not supplied one.
    pub now_unix: Option<i64>,
}

impl<'a> DefaultCertVerifier<'a> {
    pub const fn new() -> Self {
        Self {
            expected_hostname: None,
            trusted_roots: DEFAULT_TRUSTED_ROOTS,
            now_unix: None,
        }
    }

    /// Creates a default certificate verifier configured for a specific peer hostname.
    pub const fn for_host(hostname: &'a str) -> Self {
        Self {
            expected_hostname: Some(hostname),
            trusted_roots: DEFAULT_TRUSTED_ROOTS,
            now_unix: None,
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

    /// Sets the instant the certificate's validity window is compared against.
    ///
    /// WHY the caller has to supply it: this SDK has no clock of its own — reading
    /// the RTC is a capability-gated syscall the apps hold, not the TLS library —
    /// so the guest passes the civil date it already has. A verifier built without
    /// this keeps only the structural check, which accepts an expired certificate.
    pub const fn with_clock(mut self, now_unix: i64) -> Self {
        self.now_unix = Some(now_unix);
        self
    }
}

impl<'a> Default for DefaultCertVerifier<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> CertificateVerifier for DefaultCertVerifier<'a> {
    /// With a clock the window is compared; without one it is not, and the
    /// absence is reported rather than papered over with a guessed date.
    fn check_validity_window(&self, cert: &ParsedCertificate) -> Result<(), TlsError> {
        match self.now_unix {
            Some(now) => cert.check_validity_at(now),
            None => cert.check_validity(),
        }
    }

    fn verify_certificate(&self, cert: &ParsedCertificate) -> Result<(), TlsError> {
        self.check_validity_window(cert)?;

        // Only a key this client can do arithmetic with is accepted at any
        // position: Ed25519, RSA, or an EC point on a curve with an
        // implementation behind it.
        if !spki_is_verifiable(cert) {
            return Err(TlsError::UnsupportedSignatureScheme);
        }

        // Hostname validation against reference identifier (SNI).
        // Must fail closed if expected_hostname is missing or fails to match;
        // no silent skipping via an unverified Option path.
        let expected_host = self.expected_hostname.ok_or(TlsError::CertificateVerificationFailed)?;
        if !cert.matches_hostname(expected_host) {
            return Err(TlsError::CertificateVerificationFailed);
        }

        self.verify_reaches_trust_store(cert)
    }

    /// Trust, without any identity policy attached.
    ///
    /// A self-issued certificate is only trusted when its exact DER (or its
    /// subject + SPKI) is pinned in the store — there is no signature to check,
    /// so the pin IS the proof. An issued certificate must be *actually* signed
    /// by a pinned self-signed anchor, verified under the scheme its own
    /// `signatureAlgorithm` declares. That verification is never skipped for an
    /// inconvenient algorithm and its result is never discarded: any failure
    /// aborts here.
    fn verify_reaches_trust_store(&self, cert: &ParsedCertificate) -> Result<(), TlsError> {
        self.check_validity_window(cert)?;
        if !spki_is_verifiable(cert) {
            return Err(TlsError::UnsupportedSignatureScheme);
        }
        if cert.is_self_signed() {
            return self.verify_trust_anchor(cert);
        }

        for root_der in self.trusted_roots {
            let Ok(root_cert) = ParsedCertificate::parse(root_der) else {
                continue;
            };
            if !root_cert.is_self_signed() || !spki_is_verifiable(&root_cert) {
                continue;
            }
            if root_cert.subject_raw != cert.issuer_raw {
                continue;
            }
            verify_link(cert, &root_cert)?;
            return Ok(());
        }

        Err(TlsError::CertificateVerificationFailed)
    }

    fn reference_identity(&self) -> Option<&str> {
        self.expected_hostname.filter(|host| !host.is_empty())
    }

    fn verify_trust_anchor(&self, cert: &ParsedCertificate) -> Result<(), TlsError> {
        self.check_validity_window(cert)?;
        if !cert.is_self_signed() {
            return Err(TlsError::CertificateVerificationFailed);
        }
        if !spki_is_verifiable(cert) {
            return Err(TlsError::UnsupportedSignatureScheme);
        }
        for root_der in self.trusted_roots {
            if *root_der == cert.raw {
                return Ok(());
            }
            if let Ok(root_cert) = ParsedCertificate::parse(root_der) {
                if root_cert.subject_raw == cert.subject_raw
                    && root_cert.spki_public_key == cert.spki_public_key
                {
                    return Ok(());
                }
            }
        }
        Err(TlsError::CertificateVerificationFailed)
    }

    fn verify_server_signature(
        &self,
        public_key: &[u8],
        algorithm: u16,
        context_and_hash: &[u8],
        signature: &[u8],
    ) -> Result<(), TlsError> {
        // Accepted set is the advertised set: nothing outside
        // SUPPORTED_SIG_SCHEMES can be verified here, and nothing advertised
        // falls through unverified.
        if !is_supported_sig_scheme(algorithm) {
            return Err(TlsError::UnsupportedSignatureScheme);
        }
        // The scheme names its own key type, and `verify_signed_by` requires the
        // leaf key to have that shape before any arithmetic runs: a 32-byte
        // Ed25519 key, a 65- or 97-byte uncompressed EC point on the curve that
        // scheme names, or a DER `RSAPublicKey`. No other key can be made to look
        // like one, so a server cannot present an RSA leaf and then sign the
        // transcript as if it were ECDSA. The algorithm OID is left to the chain
        // walk, which read it from the certificate this key came from.
        verify_signed_by(
            public_key,
            &[],
            &[],
            algorithm,
            context_and_hash,
            signature,
        )
    }
}
