// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! TLS trust anchors installed on this device by its operator.
//!
//! # Trust model
//!
//! A certificate chain verifies only against an anchor that the owner of this
//! device put here. The OS ships none, and no application can add one: the
//! store is reachable from the serial console, which is the operator's channel,
//! and from nowhere else. This is the same decision the `.rapp` publisher store
//! makes (see `package::revocation`), and for the same reason -- a root compiled
//! into the image trusts every certificate signed by the matching private key on
//! every device that image ever boots, and that private key is exactly what an
//! attacker wants.
//!
//! WHY the store lives in RAM and not in the storage journal: every journal kind
//! from 1 to 10 is already allocated, and claiming an eleventh would move the
//! on-disk slot layout that the filesystem derives its tail reservation from --
//! a format change to every data disk, for a record that is a few hundred bytes.
//! The cost falls the safe way: a reboot withdraws the anchor, the browser
//! returns to refusing every chain, and one console line puts it back.
//!
//! WHY installing an anchor grants nothing by itself: the anchor only takes part
//! in the checks the client already performs -- the signature over each chain
//! link under the scheme that link declares, the reference identity at the leaf
//! position, and the validity window against the device clock. An anchor whose
//! subject never appears as an issuer is inert.

use crate::serial_println;
use spin::Mutex;

/// How many anchors one device holds. Four is enough for the operator's own CA
/// plus a rotation or two, and small enough that the table costs the kernel
/// nothing to keep.
pub const MAX_TRUSTED_ANCHORS: usize = 4;

/// The largest anchor accepted.
///
/// WHY 2048 rather than the 512 that fit a local CA: a public root has to fit.
/// ISRG Root X1 -- the anchor behind Let's Encrypt chains, and the one a browser
/// needs to reach any ordinary HTTPS site -- measures 1391 bytes, and a 4096-bit
/// RSA root carries roughly 700 bytes more than an Ed25519 one of the same
/// subject and extensions. The previous ceiling could not hold a real root at
/// all, so the store was only ever usable for keys made here.
///
/// WHY not larger: the store is four anchors of static space, the console has to
/// type the whole thing, and every root in the popular trust stores is under this
/// ceiling. A `MAX_ANCHOR_DER_BYTES` anchor costs the kernel 2 KiB of `.bss`.
pub const MAX_ANCHOR_DER_BYTES: usize = 2048;

/// The label the operator attaches to an anchor, so a listing can say which key
/// is which instead of printing four identical blobs.
pub const ANCHOR_LABEL_BYTES: usize = 32;

/// The DER encoding of the Ed25519 `AlgorithmIdentifier` (`OID 1.3.101.112`).
const ED25519_ALGORITHM_OID: &[u8] = &[0x06, 0x03, 0x2b, 0x65, 0x70];
/// `rsaEncryption` (`1.2.840.113549.1.1.1`), the algorithm an RSA root names.
const RSA_ENCRYPTION_OID: &[u8] =
    &[0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
/// `ecPublicKey` (`1.2.840.10045.2.1`), which names a curve rather than a key.
const EC_PUBLIC_KEY_OID: &[u8] = &[0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
/// `prime256v1` (`1.2.840.10045.3.1.7`), one of the two curves an EC certificate
/// can name that this client does arithmetic on.
const PRIME256V1_OID: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
/// `secp384r1` (`1.3.132.0.34`), the curve a public chain sits on when its
/// issuer signs with `ecdsa_secp384r1_sha384`.
const SECP384R1_OID: &[u8] = &[0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x22];

/// basicConstraints (RFC 5280 §4.2.1.9), OID 2.5.29.19.
const BASIC_CONSTRAINTS_OID: &[u8] = &[0x55, 0x1d, 0x13];
/// keyUsage (RFC 5280 §4.2.1.3), OID 2.5.29.15.
const KEY_USAGE_OID: &[u8] = &[0x55, 0x1d, 0x0f];
/// keyCertSign is bit 5 of the keyUsage BIT STRING, numbered from the MSB of the
/// first content octet (bit 0 = digitalSignature). Stored MSB-first in a `u16`,
/// bit n is `bits & (0x8000 >> n)`, so keyCertSign is `0x0400`. This is the same
/// numbering the Ring-3 reader uses (`interos-std`'s `KEY_USAGE_KEY_CERT_SIGN`).
const KEY_USAGE_KEY_CERT_SIGN: u16 = 0x8000 >> 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustError {
    /// Nothing was offered.
    EmptyAnchor,
    /// The anchor is larger than [`MAX_ANCHOR_DER_BYTES`].
    AnchorTooLarge,
    /// The bytes are not one DER `SEQUENCE` whose declared length covers them.
    NotDerSequence,
    /// The certificate names no key this client can verify with, so it could
    /// never vouch for a chain this device would accept.
    NoVerifiableKey,
    /// All [`MAX_TRUSTED_ANCHORS`] slots are taken.
    StoreFull,
    /// The same anchor is already installed.
    DuplicateAnchor,
    /// No anchor at that position.
    NoAnchorAtIndex,
    /// The caller's buffer cannot hold the anchor it asked for.
    BufferTooSmall,
    /// The bytes pass the outer shape and key prefilter but do not descend as a
    /// well-formed X.509 `Certificate` / `TBSCertificate`.
    MalformedCertificate,
    /// An extension is malformed, truncated, or duplicated, so its constraint
    /// cannot be read. Never degraded into "no constraint recorded".
    MalformedExtensions,
    /// The certificate carries no basicConstraints extension. An absent
    /// extension is a refusal, never a default: only an explicit CA may anchor.
    NoBasicConstraints,
    /// basicConstraints says CA:FALSE, so this end-entity certificate cannot
    /// vouch for a chain. Installing it would be a MITM path.
    NotCaCertificate,
    /// keyUsage is present but does not include keyCertSign, so this certificate
    /// is not permitted to sign other certificates.
    KeyUsageWithoutCertSign,
}

impl TrustError {
    pub const fn message(self) -> &'static str {
        match self {
            TrustError::EmptyAnchor => "no certificate bytes were offered",
            TrustError::AnchorTooLarge => "certificate is larger than the anchor limit",
            TrustError::NotDerSequence => "certificate is not one complete DER sequence",
            TrustError::NoVerifiableKey => "certificate names no key this client can verify",
            TrustError::StoreFull => "the trust store is full",
            TrustError::DuplicateAnchor => "that anchor is already installed",
            TrustError::NoAnchorAtIndex => "no trust anchor at that position",
            TrustError::BufferTooSmall => "the caller's buffer is smaller than the anchor",
            TrustError::MalformedCertificate => "certificate structure is not a parseable X.509 certificate",
            TrustError::MalformedExtensions => "certificate extensions are malformed, truncated, or duplicated",
            TrustError::NoBasicConstraints => "certificate has no basicConstraints extension, so it cannot be a CA anchor",
            TrustError::NotCaCertificate => "certificate says CA:FALSE, so it cannot anchor a chain",
            TrustError::KeyUsageWithoutCertSign => "certificate's keyUsage does not permit signing certificates",
        }
    }
}

/// One installed anchor.
#[derive(Clone, Copy)]
struct Anchor {
    der: [u8; MAX_ANCHOR_DER_BYTES],
    len: usize,
    label: [u8; ANCHOR_LABEL_BYTES],
    label_len: usize,
}

impl Anchor {
    const fn empty() -> Self {
        Self {
            der: [0u8; MAX_ANCHOR_DER_BYTES],
            len: 0,
            label: [0u8; ANCHOR_LABEL_BYTES],
            label_len: 0,
        }
    }

    fn der(&self) -> &[u8] {
        &self.der[..self.len]
    }

    fn label(&self) -> &str {
        core::str::from_utf8(&self.label[..self.label_len]).unwrap_or("")
    }
}

/// Rejects bytes that could not be an anchor this client verifies through.
///
/// WHY a structural check at the door: the console takes hex typed or pasted by
/// a person, and a copy that lost its tail is a real and common mistake. A
/// truncated DER stored as an anchor would simply never verify anything, and the
/// operator would have no way to tell a wrong key from an unused one.
///
/// WHY the key test is a byte search rather than a parse: it is a cheap prefilter
/// answering "could this client ever verify a chain through this certificate?" --
/// whether one of the algorithm identifiers the client does arithmetic on appears
/// in the bytes. It runs before the CA walk so an unsupported key is named as such
/// rather than as a structural failure.
///
/// WHY a CA walk follows the prefilter: shape plus a key OID is not enough to
/// trust an anchor. The Ring-3 handshake enforces CA constraints on every issuer
/// presented on the wire, but trusts the pinned store copy by its pin, so an
/// anchor that never appears on the wire is never CA-checked by the client. A
/// CA:FALSE end-entity blob installed as an anchor could sign a leaf for any
/// hostname and be trusted -- a MITM path. [`enforce_anchor_is_ca`] therefore
/// descends the DER to the extensions and refuses anything that is not an explicit
/// CA with keyCertSign. The structural guarantees above (complete DER sequence, no
/// trailing garbage, BER refused, size bound) are unchanged; this ADDS enforcement.
pub fn check_anchor_shape(der: &[u8]) -> Result<(), TrustError> {
    if der.is_empty() {
        return Err(TrustError::EmptyAnchor);
    }
    if der.len() > MAX_ANCHOR_DER_BYTES {
        return Err(TrustError::AnchorTooLarge);
    }
    if der[0] != 0x30 {
        return Err(TrustError::NotDerSequence);
    }
    // DER lengths are definite and the smallest possible encoding: short form
    // below 0x80, then one or two length bytes above it. Anything else -- an
    // indefinite form, or three length bytes for a 2 KiB blob -- is not the
    // certificate this store was offered.
    let (header, declared) = if der.len() < 2 {
        return Err(TrustError::NotDerSequence);
    } else if der[1] < 0x80 {
        (2usize, der[1] as usize)
    } else if der[1] == 0x81 {
        if der.len() < 3 {
            return Err(TrustError::NotDerSequence);
        }
        (3usize, der[2] as usize)
    } else if der[1] == 0x82 {
        if der.len() < 4 {
            return Err(TrustError::NotDerSequence);
        }
        (4usize, ((der[2] as usize) << 8) | der[3] as usize)
    } else {
        return Err(TrustError::NotDerSequence);
    };
    // A long-form length must be the shortest form that can carry the value, or
    // the bytes are BER rather than DER.
    if declared < 0x80 && der[1] >= 0x80 {
        return Err(TrustError::NotDerSequence);
    }
    if declared > u8::MAX as usize && der[1] == 0x81 {
        return Err(TrustError::NotDerSequence);
    }
    if header + declared != der.len() {
        return Err(TrustError::NotDerSequence);
    }
    if !names_verifiable_key(der) {
        return Err(TrustError::NoVerifiableKey);
    }
    enforce_anchor_is_ca(der)?;
    Ok(())
}

/// Whether this certificate names a key the client can check a signature with:
/// Ed25519, RSA, or EC on a curve it has arithmetic for. An EC identifier carries
/// no meaning without its curve parameter, so a certificate that names
/// `ecPublicKey` but neither `prime256v1` nor `secp384r1` -- a P-521 root -- is
/// refused here rather than installed and then silently unable to vouch for
/// anything.
///
/// WHY the curve list lives twice: the store sees only the bytes someone typed,
/// while the verifier sees a parsed certificate. These two still have to agree,
/// because a door naming fewer curves than the arithmetic covers refuses an anchor
/// the device could use.
fn names_verifiable_key(der: &[u8]) -> bool {
    contains_subsequence(der, ED25519_ALGORITHM_OID)
        || contains_subsequence(der, RSA_ENCRYPTION_OID)
        || (contains_subsequence(der, EC_PUBLIC_KEY_OID)
            && (contains_subsequence(der, PRIME256V1_OID)
                || contains_subsequence(der, SECP384R1_OID)))
}

fn contains_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Reads one DER TLV: `(tag, value, remainder)`.
///
/// WHY this mirrors the Ring-3 reader (`interos-std`'s `read_tlv`) rather than
/// calling it: the kernel is `no_std` and must not depend on the SDK. The two
/// readers agree on definite lengths of up to four octets, so a certificate the
/// client parses on the wire is framed the same way at the door. The outer
/// shortest-form rule is enforced by [`check_anchor_shape`]; inner reads accept
/// the same definite forms the client does.
fn read_tlv(input: &[u8]) -> Result<(u8, &[u8], &[u8]), TrustError> {
    if input.len() < 2 {
        return Err(TrustError::MalformedCertificate);
    }
    let tag = input[0];
    let len_byte = input[1];
    let (length, header) = if len_byte < 0x80 {
        (len_byte as usize, 2usize)
    } else {
        let octets = (len_byte & 0x7f) as usize;
        if octets == 0 || octets > 4 || input.len() < 2 + octets {
            return Err(TrustError::MalformedCertificate);
        }
        let mut len = 0usize;
        for i in 0..octets {
            len = (len << 8) | input[2 + i] as usize;
        }
        (len, 2 + octets)
    };
    if input.len() < header + length {
        return Err(TrustError::MalformedCertificate);
    }
    Ok((
        tag,
        &input[header..header + length],
        &input[header + length..],
    ))
}

fn read_sequence(input: &[u8]) -> Result<(&[u8], &[u8]), TrustError> {
    let (tag, value, rem) = read_tlv(input)?;
    if tag != 0x30 {
        return Err(TrustError::MalformedCertificate);
    }
    Ok((value, rem))
}

/// `BasicConstraints ::= SEQUENCE { cA BOOLEAN DEFAULT FALSE,
/// pathLenConstraint INTEGER OPTIONAL }`.
///
/// An empty SEQUENCE (`30 00`) is CA:FALSE -- the DER BOOLEAN default. Only
/// `0x00`/`0xff` are canonical BOOLEAN values. pathLenConstraint present while
/// cA is FALSE is malformed. Every unreadable shape is a refusal, never a silent
/// "no constraint recorded". Mirrors the Ring-3 `parse_basic_constraints`.
fn parse_basic_constraints(der: &[u8]) -> Result<(bool, Option<u32>), TrustError> {
    let (content, rest) = read_sequence(der).map_err(|_| TrustError::MalformedExtensions)?;
    if !rest.is_empty() {
        return Err(TrustError::MalformedExtensions);
    }
    let mut ca = false;
    let mut path_len = None;
    if !content.is_empty() {
        let (tag, val, rem) = read_tlv(content).map_err(|_| TrustError::MalformedExtensions)?;
        if tag != 0x01 || val.len() != 1 {
            return Err(TrustError::MalformedExtensions);
        }
        match val[0] {
            0x00 => ca = false,
            0xff => ca = true,
            _ => return Err(TrustError::MalformedExtensions),
        }
        if !rem.is_empty() {
            let (tag2, val2, rest2) =
                read_tlv(rem).map_err(|_| TrustError::MalformedExtensions)?;
            if tag2 != 0x02 || !rest2.is_empty() {
                return Err(TrustError::MalformedExtensions);
            }
            path_len = Some(parse_path_len(val2)?);
        }
    }
    if path_len.is_some() && !ca {
        return Err(TrustError::MalformedExtensions);
    }
    Ok((ca, path_len))
}

/// A pathLenConstraint is a non-negative INTEGER no wider than a `u32`; anything
/// wider cannot be a chain bound the Ring-3 walk could ever honour.
fn parse_path_len(val: &[u8]) -> Result<u32, TrustError> {
    if val.is_empty() || val[0] & 0x80 != 0 {
        return Err(TrustError::MalformedExtensions);
    }
    let v = if val.len() > 1 && val[0] == 0 {
        &val[1..]
    } else {
        val
    };
    if v.len() > 4 {
        return Err(TrustError::MalformedExtensions);
    }
    let mut n = 0u32;
    for b in v {
        n = (n << 8) | *b as u32;
    }
    Ok(n)
}

/// `KeyUsage ::= BIT STRING`, read MSB-first into a `u16`.
///
/// The first content octet is the count of unused trailing bits, not a usage bit;
/// it is consumed. Nine bits (0..8) means one octet, or two with exactly seven
/// unused bits. DER requires the unused trailing bits to be zero. Mirrors the
/// Ring-3 `parse_key_usage`.
fn parse_key_usage(der: &[u8]) -> Result<u16, TrustError> {
    let (tag, content, rest) = read_tlv(der).map_err(|_| TrustError::MalformedExtensions)?;
    if tag != 0x03 || !rest.is_empty() || content.is_empty() {
        return Err(TrustError::MalformedExtensions);
    }
    let unused = content[0];
    let bytes = &content[1..];
    if bytes.is_empty() || bytes.len() > 2 || unused > 7 {
        return Err(TrustError::MalformedExtensions);
    }
    if bytes.len() == 2 && unused != 7 {
        return Err(TrustError::MalformedExtensions);
    }
    if unused > 0 && bytes[bytes.len() - 1] & ((1u8 << unused) - 1) != 0 {
        return Err(TrustError::MalformedExtensions);
    }
    let mut bits = (bytes[0] as u16) << 8;
    if bytes.len() == 2 {
        bits |= bytes[1] as u16;
    }
    Ok(bits)
}

/// Walks `Extensions ::= SEQUENCE OF Extension` strictly.
///
/// `Extension ::= SEQUENCE { extnID OID, critical BOOLEAN DEFAULT FALSE,
/// extnValue OCTET STRING }`. Every extension's framing is validated;
/// basicConstraints and keyUsage are decoded and duplicates of either refused;
/// any malformed or truncated extension is a refusal. Other extensions are
/// skipped -- the door only needs the two CA constraints. The loop consumes at
/// least two bytes per iteration from a slice bounded by [`MAX_ANCHOR_DER_BYTES`],
/// so it terminates without recursion or allocation.
fn walk_extensions(
    mut cur: &[u8],
    basic_constraints: &mut Option<(bool, Option<u32>)>,
    key_usage: &mut Option<u16>,
) -> Result<(), TrustError> {
    while !cur.is_empty() {
        let (tag, ext, rem) = read_tlv(cur).map_err(|_| TrustError::MalformedExtensions)?;
        cur = rem;
        if tag != 0x30 {
            return Err(TrustError::MalformedExtensions);
        }
        let (oid_tag, oid, after_oid) =
            read_tlv(ext).map_err(|_| TrustError::MalformedExtensions)?;
        if oid_tag != 0x06 || oid.is_empty() {
            return Err(TrustError::MalformedExtensions);
        }
        let (tag1, val1, after1) =
            read_tlv(after_oid).map_err(|_| TrustError::MalformedExtensions)?;
        let octets = if tag1 == 0x01 {
            if val1.len() != 1 || (val1[0] != 0x00 && val1[0] != 0xff) {
                return Err(TrustError::MalformedExtensions);
            }
            let (tag2, val2, rest2) =
                read_tlv(after1).map_err(|_| TrustError::MalformedExtensions)?;
            if tag2 != 0x04 || !rest2.is_empty() {
                return Err(TrustError::MalformedExtensions);
            }
            val2
        } else if tag1 == 0x04 {
            if !after1.is_empty() {
                return Err(TrustError::MalformedExtensions);
            }
            val1
        } else {
            return Err(TrustError::MalformedExtensions);
        };
        if oid == BASIC_CONSTRAINTS_OID {
            if basic_constraints.is_some() {
                return Err(TrustError::MalformedExtensions);
            }
            *basic_constraints = Some(parse_basic_constraints(octets)?);
        } else if oid == KEY_USAGE_OID {
            if key_usage.is_some() {
                return Err(TrustError::MalformedExtensions);
            }
            *key_usage = Some(parse_key_usage(octets)?);
        }
    }
    Ok(())
}

/// The CA gate: an anchor must be a certificate that may sign other certificates.
///
/// WHY this exists at install time: the Ring-3 handshake enforces CA constraints
/// on every issuer PRESENTED ON THE WIRE, but it trusts the pinned trust-store
/// copy by its pin (`verify_reaches_trust_store`). An anchor that never appears on
/// the wire -- a lone leaf verified directly against it -- is therefore never
/// CA-checked by the client. If a CA:FALSE end-entity blob could be installed as
/// an anchor, it could sign a leaf for any hostname and the client would trust it:
/// a MITM path. The kernel must refuse a non-CA anchor at the door.
///
/// Fail closed: basicConstraints must be PRESENT with cA TRUE (an absent extension
/// is a refusal, never a default); when keyUsage is present it must include
/// keyCertSign. The walk is a bounded, allocation-free DER descent --
/// `Certificate -> TBSCertificate -> [3] extensions` -- with no recursion; every
/// loop consumes bytes from a slice no larger than [`MAX_ANCHOR_DER_BYTES`].
///
/// pathLenConstraint is parsed and validated here (non-negative, <= 4 bytes,
/// meaningless without cA TRUE) but NOT recorded on the anchor. The Ring-3 walk
/// already enforces pathLen for every wire issuer, and the SDK re-parses the pinned
/// anchor's own DER (`ParsedCertificate::parse(root_der)`), so it can read pathLen
/// itself whenever the anchor appears on the wire. When the anchor does NOT appear
/// on the wire (a leaf directly under it) there are zero non-self-issued
/// intermediates between them, so any valid pathLen is trivially satisfied.
/// Recording it in the kernel would need a new kernel->SDK metadata channel that
/// does not exist today; no such mechanism is invented here.
fn enforce_anchor_is_ca(der: &[u8]) -> Result<(), TrustError> {
    let (content, rest) = read_sequence(der)?;
    if !rest.is_empty() {
        return Err(TrustError::MalformedCertificate);
    }
    // Certificate ::= SEQUENCE { tbsCertificate, signatureAlgorithm, signatureValue }
    let (tbs_tag, tbs, rem1) = read_tlv(content)?;
    if tbs_tag != 0x30 {
        return Err(TrustError::MalformedCertificate);
    }
    let (alg_tag, _alg, rem2) = read_tlv(rem1)?;
    if alg_tag != 0x30 {
        return Err(TrustError::MalformedCertificate);
    }
    let (sig_tag, sig, rem3) = read_tlv(rem2)?;
    if sig_tag != 0x03 || sig.is_empty() || !rem3.is_empty() {
        return Err(TrustError::MalformedCertificate);
    }

    // TBSCertificate fields, in order.
    let mut cur = tbs;
    let mut version = 1u32;
    if let Ok((tag, ver, rem)) = read_tlv(cur) {
        if tag == 0xa0 {
            let (int_tag, int_val, _) = read_tlv(ver)?;
            if int_tag != 0x02 || int_val.is_empty() {
                return Err(TrustError::MalformedCertificate);
            }
            version = int_val[0] as u32 + 1;
            cur = rem;
        }
    }
    // serialNumber INTEGER
    let (tag, _, rem) = read_tlv(cur)?;
    if tag != 0x02 {
        return Err(TrustError::MalformedCertificate);
    }
    cur = rem;
    // signature AlgorithmIdentifier SEQUENCE
    let (tag, _, rem) = read_tlv(cur)?;
    if tag != 0x30 {
        return Err(TrustError::MalformedCertificate);
    }
    cur = rem;
    // issuer Name SEQUENCE
    let (tag, _, rem) = read_tlv(cur)?;
    if tag != 0x30 {
        return Err(TrustError::MalformedCertificate);
    }
    cur = rem;
    // validity SEQUENCE { notBefore Time, notAfter Time }
    let (tag, validity, rem) = read_tlv(cur)?;
    if tag != 0x30 {
        return Err(TrustError::MalformedCertificate);
    }
    let (_, _, vrem) = read_tlv(validity)?;
    let (_, _, vrem2) = read_tlv(vrem)?;
    if !vrem2.is_empty() {
        return Err(TrustError::MalformedCertificate);
    }
    cur = rem;
    // subject Name SEQUENCE
    let (tag, _, rem) = read_tlv(cur)?;
    if tag != 0x30 {
        return Err(TrustError::MalformedCertificate);
    }
    cur = rem;
    // subjectPublicKeyInfo SEQUENCE
    let (tag, _spki, rem) = read_tlv(cur)?;
    if tag != 0x30 {
        return Err(TrustError::MalformedCertificate);
    }

    // Optional trailing fields: [1] issuerUniqueID, [2] subjectUniqueID,
    // [3] EXPLICIT extensions. Anything else is not a certificate this door speaks
    // for, and a malformed extension is a refusal -- never "no constraints".
    let mut tail = rem;
    let mut basic_constraints: Option<(bool, Option<u32>)> = None;
    let mut key_usage: Option<u16> = None;
    let mut saw_extensions = false;
    while !tail.is_empty() {
        let (tag, val, rem) = read_tlv(tail)?;
        tail = rem;
        match tag {
            0x81 | 0x82 => {
                if val.is_empty() {
                    return Err(TrustError::MalformedCertificate);
                }
            }
            0xa3 => {
                if saw_extensions {
                    return Err(TrustError::MalformedCertificate);
                }
                saw_extensions = true;
                let (exts, rest) =
                    read_sequence(val).map_err(|_| TrustError::MalformedExtensions)?;
                if !rest.is_empty() {
                    return Err(TrustError::MalformedExtensions);
                }
                walk_extensions(exts, &mut basic_constraints, &mut key_usage)?;
            }
            _ => return Err(TrustError::MalformedCertificate),
        }
    }
    // Extensions exist only in v3 certificates (RFC 5280 §4.1.2.1).
    if saw_extensions && version != 3 {
        return Err(TrustError::MalformedCertificate);
    }

    let (ca, _path_len) = basic_constraints.ok_or(TrustError::NoBasicConstraints)?;
    if !ca {
        return Err(TrustError::NotCaCertificate);
    }
    if let Some(bits) = key_usage {
        if bits & KEY_USAGE_KEY_CERT_SIGN == 0 {
            return Err(TrustError::KeyUsageWithoutCertSign);
        }
    }
    Ok(())
}

/// The anchors this device's operator installed.
pub struct TlsTrustStore {
    anchors: [Anchor; MAX_TRUSTED_ANCHORS],
    count: usize,
}

impl TlsTrustStore {
    pub const fn new() -> Self {
        Self {
            anchors: [Anchor::empty(); MAX_TRUSTED_ANCHORS],
            count: 0,
        }
    }

    /// Installs an anchor and returns its position.
    ///
    /// WHY positions are dense: a client enumerates the store by position, so a
    /// hole would read as the end of the store and the anchors behind it would go
    /// unused. Revoking therefore closes the gap rather than leaving a tombstone.
    pub fn install(&mut self, der: &[u8], label: &str) -> Result<usize, TrustError> {
        check_anchor_shape(der)?;
        if self.count >= MAX_TRUSTED_ANCHORS {
            return Err(TrustError::StoreFull);
        }
        if self.anchors[..self.count].iter().any(|a| a.der() == der) {
            return Err(TrustError::DuplicateAnchor);
        }
        let mut slot = Anchor::empty();
        slot.der[..der.len()].copy_from_slice(der);
        slot.len = der.len();
        let bytes = label.as_bytes();
        let n = bytes.len().min(ANCHOR_LABEL_BYTES);
        slot.label[..n].copy_from_slice(&bytes[..n]);
        slot.label_len = n;
        self.anchors[self.count] = slot;
        self.count += 1;
        Ok(self.count - 1)
    }

    /// Removes the anchor at `position`, shifting later anchors down.
    pub fn revoke(&mut self, position: usize) -> Result<(), TrustError> {
        if position >= self.count {
            return Err(TrustError::NoAnchorAtIndex);
        }
        for i in position..self.count - 1 {
            self.anchors[i] = self.anchors[i + 1];
        }
        self.anchors[self.count - 1] = Anchor::empty();
        self.count -= 1;
        Ok(())
    }

    pub const fn len(&self) -> usize {
        self.count
    }

    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The anchor at `position`, or `None` past the end of the store.
    pub fn anchor_der(&self, position: usize) -> Option<&[u8]> {
        if position >= self.count {
            return None;
        }
        Some(self.anchors[position].der())
    }

    pub fn anchor_label(&self, position: usize) -> Option<&str> {
        if position >= self.count {
            return None;
        }
        Some(self.anchors[position].label())
    }

    /// Removes every anchor. This is the state a reboot leaves the device in, and
    /// the state a test restores afterwards so one case cannot decide another.
    pub fn clear(&mut self) {
        self.anchors = [Anchor::empty(); MAX_TRUSTED_ANCHORS];
        self.count = 0;
    }
}

/// The device's TLS trust anchors.
pub static TLS_TRUST_STORE: Mutex<TlsTrustStore> = Mutex::new(TlsTrustStore::new());

/// Says what the device holds, in the console's own words.
///
/// WHY this is printed at boot: an operator who installed an anchor yesterday and
/// rebooted today needs to learn from the console that the anchor is gone, rather
/// than discovering it by watching a connection fail.
pub fn report_at_boot() {
    let store = TLS_TRUST_STORE.lock();
    if store.is_empty() {
        serial_println!(
            "[TLS_TRUST] No trust anchor is installed on this device; every HTTPS certificate is untrusted until the operator installs one."
        );
        return;
    }
    serial_println!(
        "[TLS_TRUST] {} operator-installed anchor(s) held in RAM; a reboot withdraws them.",
        store.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    // ---------------------------------------------------------------------
    // Genuine minimal DER CA fixtures.
    //
    // The door now descends to the extensions and requires basicConstraints
    // CA:TRUE (and keyCertSign when keyUsage is present), so a synthetic
    // "outer SEQUENCE + OID + padding" blob is no longer an installable anchor.
    // These helpers build real X.509 structures byte-by-byte: a v3
    // TBSCertificate with serial, signature algorithm, empty issuer/subject
    // Names, a validity window, a SubjectPublicKeyInfo carrying a REAL public
    // key, and a [3] extensions block. The signatureValue is a placeholder BIT
    // STRING: the store pins an anchor by its bytes and never verifies the
    // anchor's self-signature at install, so the fixture's signature is not a
    // claim about any real signing key. Public keys only -- no private key is
    // read, copied or embedded here.
    // ---------------------------------------------------------------------

    /// Real Ed25519 public key (host-generated, public half only).
    const ED25519_PUB: &[u8] = &[
        0x8a, 0x7a, 0x0e, 0x2b, 0x60, 0x85, 0x30, 0x0e, 0x5b, 0x78, 0xdd, 0x6a, 0x05, 0x82, 0xb6,
        0x2c, 0xfa, 0x14, 0xc4, 0x6b, 0x9b, 0x1b, 0xca, 0xe0, 0x22, 0x39, 0xb0, 0x74, 0xa6, 0x8e,
        0x19, 0xbb,
    ];
    /// Real Ed448 public key -- a curve the client has no arithmetic for.
    const ED448_PUB: &[u8] = &[
        0x2a, 0xcb, 0xc9, 0x9e, 0x8e, 0x3b, 0xc6, 0xad, 0xee, 0xb9, 0x95, 0xd0, 0x3f, 0x74, 0xb6,
        0x66, 0x9d, 0xd7, 0x84, 0xd5, 0xcc, 0x00, 0x0f, 0x6c, 0x52, 0x88, 0x06, 0xe8, 0xea, 0xd4,
        0x84, 0xbd, 0xe1, 0x57, 0x37, 0x5f, 0x3b, 0xd5, 0xbf, 0x30, 0xce, 0xfd, 0xd7, 0xb3, 0xad,
        0xa9, 0x6f, 0xbd, 0xb4, 0xf5, 0x25, 0x86, 0x81, 0xc8, 0x09, 0xc6, 0x80,
    ];
    /// Real uncompressed P-256 point.
    const P256_PUB: &[u8] = &[
        0x04, 0x37, 0x76, 0x2f, 0xc3, 0xad, 0xeb, 0xbf, 0x85, 0xac, 0xae, 0x49, 0xc6, 0x61, 0xe8,
        0xe8, 0xd5, 0x55, 0xc1, 0x8b, 0x0f, 0x74, 0x50, 0x30, 0xda, 0xc1, 0x04, 0xb3, 0x6e, 0x57,
        0x2a, 0x5b, 0xc8, 0xcc, 0x75, 0xee, 0xc3, 0x3f, 0x9d, 0x46, 0xb6, 0x21, 0xe9, 0x30, 0xe1,
        0x0d, 0xa7, 0xd8, 0xf9, 0x2d, 0x95, 0xf9, 0xbe, 0xc8, 0x57, 0xf0, 0x6e, 0xf0, 0xd5, 0x33,
        0x22, 0x24, 0x37, 0x78, 0x92,
    ];
    /// Real uncompressed P-384 point.
    const P384_PUB: &[u8] = &[
        0x04, 0xec, 0x55, 0x16, 0xdf, 0x06, 0x1a, 0xd5, 0x47, 0xe2, 0x80, 0xa1, 0x71, 0x4e, 0x67,
        0x8d, 0x35, 0x84, 0xb3, 0xa8, 0x1c, 0xae, 0xe6, 0x65, 0x70, 0xc8, 0xeb, 0xa6, 0x31, 0x94,
        0x32, 0x52, 0xd1, 0x25, 0x57, 0x01, 0x4b, 0x99, 0xc5, 0x56, 0xf3, 0xeb, 0xdf, 0xaa, 0x35,
        0x2a, 0xab, 0xe6, 0xab, 0x1f, 0x2d, 0x44, 0x4e, 0xcb, 0x48, 0x35, 0x5f, 0x68, 0xa9, 0x5d,
        0x27, 0xfb, 0x9b, 0x22, 0xf9, 0xcc, 0x00, 0xbf, 0xa5, 0x8e, 0xc8, 0xca, 0xd4, 0x94, 0xa1,
        0x19, 0x61, 0x05, 0x12, 0x77, 0x99, 0x42, 0x8c, 0xb5, 0xa8, 0xb2, 0xfd, 0xb3, 0x1d, 0xc2,
        0x2d, 0x9c, 0x5c, 0x07, 0xc4, 0xd2, 0xdf,
    ];
    /// Real uncompressed P-521 point -- a curve the client has no arithmetic for.
    const P521_PUB: &[u8] = &[
        0x04, 0x01, 0x2c, 0xf8, 0xd1, 0x70, 0x0a, 0x05, 0xe3, 0x63, 0x52, 0xd9, 0xbf, 0x02, 0x10,
        0xdb, 0x69, 0xef, 0xfa, 0x74, 0x18, 0xbe, 0x43, 0xca, 0x3b, 0x02, 0xd1, 0xf4, 0xb5, 0xb4,
        0xfc, 0x0b, 0x90, 0x51, 0xf5, 0xb3, 0xbc, 0x40, 0x8e, 0xf0, 0xc4, 0xde, 0xac, 0x05, 0x78,
        0x84, 0xb2, 0x82, 0x86, 0xdb, 0x25, 0xb2, 0xfc, 0x30, 0xec, 0xe0, 0xf3, 0xcc, 0x50, 0x8c,
        0xa8, 0xc3, 0xb0, 0xdc, 0xb0, 0x7a, 0x47, 0x01, 0x3e, 0xff, 0x8e, 0x01, 0x90, 0x1f, 0x0d,
        0x5b, 0x95, 0x03, 0xb3, 0xd7, 0x63, 0x25, 0x5a, 0x7c, 0x19, 0xba, 0x6d, 0x57, 0x1d, 0xe0,
        0x70, 0xd3, 0x68, 0x83, 0xae, 0x9b, 0x78, 0xb3, 0xd0, 0x83, 0x56, 0xaa, 0xb4, 0xaf, 0xc5,
        0x00, 0xa8, 0xbf, 0xaf, 0x56, 0x77, 0xc3, 0x3a, 0xf3, 0xaf, 0xb0, 0xe2, 0xdc, 0xd1, 0xe3,
        0x3b, 0xea, 0x2f, 0x75, 0x29, 0x47, 0x52, 0x78, 0x32, 0x6c, 0xb4, 0x48, 0x59,
    ];
    /// Real RSA-2048 `RSAPublicKey` DER (modulus + exponent), the inner bytes a
    /// certificate stores inside its subjectPublicKey BIT STRING.
    const RSA2048_INNER: &[u8] = &[
        0x30, 0x82, 0x01, 0x0a, 0x02, 0x82, 0x01, 0x01, 0x00, 0xa9, 0xf0, 0xce, 0xf2, 0x6b, 0xea,
        0x7c, 0x8f, 0xb3, 0x61, 0x04, 0xa5, 0x04, 0x8d, 0xea, 0x9a, 0xd0, 0x42, 0x45, 0x2f, 0x0a,
        0x73, 0xf4, 0x06, 0x7c, 0x9c, 0xfb, 0xba, 0x6d, 0x7d, 0xae, 0xaf, 0x31, 0xd7, 0x29, 0xa5,
        0xd3, 0x48, 0x1c, 0xcb, 0x07, 0x9e, 0x83, 0xb6, 0x78, 0x18, 0x9c, 0x3f, 0xc8, 0x09, 0xf4,
        0x74, 0x82, 0xe2, 0xa0, 0xb5, 0x9a, 0x59, 0x74, 0x58, 0x3b, 0x43, 0xe9, 0x27, 0x4d, 0xe6,
        0x43, 0x5c, 0xb4, 0x59, 0x04, 0xe1, 0x43, 0x48, 0x0f, 0xdb, 0xe9, 0xb1, 0xc7, 0xf1, 0xa8,
        0xd4, 0xce, 0x92, 0xa9, 0xac, 0x20, 0xbd, 0x28, 0x61, 0x12, 0x93, 0x73, 0x70, 0xfb, 0x53,
        0x25, 0xd4, 0x20, 0xbe, 0xd4, 0x3c, 0x71, 0xba, 0xc0, 0x50, 0x70, 0x15, 0xc5, 0xba, 0x81,
        0x41, 0x53, 0xb5, 0x9b, 0x7c, 0xe0, 0xd0, 0xac, 0x64, 0xad, 0x70, 0x9a, 0xee, 0x2a, 0xdc,
        0x6a, 0x94, 0x84, 0x95, 0xdb, 0xf8, 0xf1, 0xec, 0xd8, 0x4b, 0x3a, 0x57, 0xed, 0x3a, 0x5b,
        0xc6, 0x1a, 0x65, 0xa2, 0x64, 0xc0, 0x54, 0x9d, 0x24, 0x2e, 0x8d, 0xbe, 0x98, 0xbb, 0x1e,
        0x99, 0x1f, 0x0f, 0xdf, 0x1b, 0xfb, 0xbb, 0x34, 0x6a, 0xb1, 0x8b, 0x3a, 0x1b, 0xbe, 0x7a,
        0x89, 0x9b, 0x3a, 0x2e, 0xe4, 0x5d, 0xd0, 0x99, 0xb8, 0xe1, 0x63, 0x54, 0xbf, 0xcd, 0xde,
        0xfa, 0x3f, 0x96, 0xf9, 0x54, 0x69, 0x7f, 0xbe, 0xa4, 0x80, 0x34, 0x9b, 0x0b, 0x49, 0x4d,
        0x02, 0x0b, 0x4f, 0xb3, 0xf5, 0x4c, 0x38, 0x06, 0xe9, 0x31, 0xdb, 0xa6, 0xc1, 0x6c, 0xe5,
        0x26, 0x85, 0x61, 0xc5, 0x6e, 0xae, 0x39, 0x32, 0x2a, 0xc2, 0x6c, 0xcb, 0x15, 0x0b, 0x26,
        0x26, 0x24, 0xae, 0xf0, 0x4e, 0xc4, 0xf0, 0x5f, 0x6e, 0x28, 0x0c, 0x40, 0xcf, 0x86, 0x71,
        0xfe, 0x7e, 0xf3, 0x31, 0xea, 0x4f, 0x6b, 0xad, 0x46, 0x2f, 0x02, 0x03, 0x01, 0x00, 0x01,
    ];

    const ED448_OID: &[u8] = &[0x06, 0x03, 0x2b, 0x65, 0x71];
    const SECP521R1_OID: &[u8] = &[0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x23];
    const ECDSA_WITH_SHA256_OID: &[u8] =
        &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
    const ECDSA_WITH_SHA384_OID: &[u8] =
        &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
    const SHA256_WITH_RSA_OID: &[u8] =
        &[0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
    /// An OID the door does not interpret, used only to pad a fixture to a size.
    const PAD_OID: &[u8] = &[0x06, 0x03, 0x2a, 0x03, 0x04];

    /// One DER TLV with the shortest definite length form for `content`.
    fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
        let n = content.len();
        let mut out = vec![tag];
        if n < 0x80 {
            out.push(n as u8);
        } else if n <= 0xff {
            out.push(0x81);
            out.push(n as u8);
        } else if n <= 0xffff {
            out.push(0x82);
            out.push((n >> 8) as u8);
            out.push(n as u8);
        } else {
            out.push(0x83);
            out.push((n >> 16) as u8);
            out.push((n >> 8) as u8);
            out.push(n as u8);
        }
        out.extend_from_slice(content);
        out
    }

    /// A non-negative INTEGER in its shortest DER form.
    fn uint_der(n: u32) -> Vec<u8> {
        let be = n.to_be_bytes();
        let mut i = 0;
        while i < 3 && be[i] == 0 {
            i += 1;
        }
        let v = &be[i..];
        if v[0] & 0x80 != 0 {
            let mut c = vec![0u8];
            c.extend_from_slice(v);
            tlv(0x02, &c)
        } else {
            tlv(0x02, v)
        }
    }

    /// `Extension ::= SEQUENCE { extnID OID, critical BOOLEAN DEFAULT FALSE,
    /// extnValue OCTET STRING }`. `oid` is the full OID TLV; `value` is the
    /// already-DER-encoded extension value placed inside the OCTET STRING.
    fn extension(oid: &[u8], critical: bool, value: &[u8]) -> Vec<u8> {
        let mut content = oid.to_vec();
        if critical {
            content.extend_from_slice(&[0x01, 0x01, 0xff]);
        }
        content.extend_from_slice(&tlv(0x04, value));
        tlv(0x30, &content)
    }

    /// A basicConstraints extension. `ca` TRUE writes `01 01 ff`; FALSE writes
    /// an explicit `01 01 00`. `path_len` adds the INTEGER when present.
    fn basic_constraints_ext(ca: bool, path_len: Option<u32>, critical: bool) -> Vec<u8> {
        let mut bc = Vec::new();
        bc.extend_from_slice(&[0x01, 0x01, if ca { 0xff } else { 0x00 }]);
        if let Some(n) = path_len {
            bc.extend_from_slice(&uint_der(n));
        }
        extension(&[0x06, 0x03, 0x55, 0x1d, 0x13], critical, &tlv(0x30, &bc))
    }

    /// A keyUsage extension from raw BIT STRING content (leading unused-bits
    /// octet included), e.g. `&[0x02, 0x04]` for keyCertSign-only.
    fn key_usage_ext(bits_content: &[u8], critical: bool) -> Vec<u8> {
        extension(&[0x06, 0x03, 0x55, 0x1d, 0x0f], critical, &tlv(0x03, bits_content))
    }

    struct Fixture {
        spki_alg: Vec<u8>,
        key: Vec<u8>,
        sig_alg: Vec<u8>,
        exts: Vec<Vec<u8>>,
        version: Option<u8>,
        serial: u8,
    }

    /// Assembles a complete `Certificate` DER from the fixture parts.
    fn certificate(f: &Fixture) -> Vec<u8> {
        let mut tbs = Vec::new();
        if let Some(v) = f.version {
            tbs.extend_from_slice(&tlv(0xa0, &tlv(0x02, &[v])));
        }
        tbs.extend_from_slice(&tlv(0x02, &[f.serial]));
        tbs.extend_from_slice(&f.sig_alg);
        tbs.extend_from_slice(&tlv(0x30, &[])); // issuer Name (empty)
        let mut validity = tlv(0x17, b"250101000000Z");
        validity.extend_from_slice(&tlv(0x17, b"350101000000Z"));
        tbs.extend_from_slice(&tlv(0x30, &validity));
        tbs.extend_from_slice(&tlv(0x30, &[])); // subject Name (empty)
        let mut spki = f.spki_alg.clone();
        let mut keybits = vec![0x00u8]; // unused-bits octet
        keybits.extend_from_slice(&f.key);
        spki.extend_from_slice(&tlv(0x03, &keybits));
        tbs.extend_from_slice(&tlv(0x30, &spki));
        if !f.exts.is_empty() {
            let mut extseq = Vec::new();
            for e in &f.exts {
                extseq.extend_from_slice(e);
            }
            tbs.extend_from_slice(&tlv(0xa3, &tlv(0x30, &extseq)));
        }
        let mut cert = tlv(0x30, &tbs);
        cert.extend_from_slice(&f.sig_alg);
        // Placeholder signatureValue: install pins by bytes, never verifies it.
        cert.extend_from_slice(&tlv(0x03, &[0x00; 9]));
        tlv(0x30, &cert)
    }

    fn ed25519_fixture(serial: u8, exts: Vec<Vec<u8>>) -> Fixture {
        Fixture {
            spki_alg: tlv(0x30, ED25519_ALGORITHM_OID),
            key: ED25519_PUB.to_vec(),
            sig_alg: tlv(0x30, ED25519_ALGORITHM_OID),
            exts,
            version: Some(2),
            serial,
        }
    }

    /// A genuine Ed25519 CA anchor (basicConstraints CA:TRUE, critical).
    fn ca_anchor(serial: u8) -> Vec<u8> {
        certificate(&ed25519_fixture(serial, vec![basic_constraints_ext(true, None, true)]))
    }

    fn rsa_ca(serial: u8) -> Vec<u8> {
        let mut alg = RSA_ENCRYPTION_OID.to_vec();
        alg.extend_from_slice(&[0x05, 0x00]); // NULL parameters
        certificate(&Fixture {
            spki_alg: tlv(0x30, &alg),
            key: RSA2048_INNER.to_vec(),
            sig_alg: tlv(0x30, SHA256_WITH_RSA_OID),
            exts: vec![basic_constraints_ext(true, None, true)],
            version: Some(2),
            serial,
        })
    }

    fn ec_ca(serial: u8, curve_oid: &[u8], key: &[u8], sig_oid: &[u8]) -> Vec<u8> {
        let mut alg = EC_PUBLIC_KEY_OID.to_vec();
        alg.extend_from_slice(curve_oid);
        certificate(&Fixture {
            spki_alg: tlv(0x30, &alg),
            key: key.to_vec(),
            sig_alg: tlv(0x30, sig_oid),
            exts: vec![basic_constraints_ext(true, None, true)],
            version: Some(2),
            serial,
        })
    }

    /// A genuine CA certificate padded with an unrecognized extension to exactly
    /// `target` bytes, so the size-bound tests exercise a real certificate rather
    /// than a synthetic shape. Converges by adjusting the padding content length.
    fn sized_ca(target: usize) -> Vec<u8> {
        let mut guess = target as isize - 200;
        if guess < 0 {
            guess = 0;
        }
        for _ in 0..32 {
            let pad = extension(PAD_OID, false, &vec![0x41u8; guess as usize]);
            let der = certificate(&ed25519_fixture(
                7,
                vec![basic_constraints_ext(true, None, true), pad],
            ));
            if der.len() == target {
                return der;
            }
            guess += target as isize - der.len() as isize;
            if guess < 0 {
                guess = 0;
            }
        }
        panic!("could not pad a genuine CA fixture to {target} bytes");
    }

    #[test]
    fn an_empty_store_trusts_nothing() {
        let store = TlsTrustStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        assert_eq!(store.anchor_der(0), None);
    }

    #[test]
    fn a_complete_ed25519_anchor_is_accepted_and_reads_back() {
        let der = ca_anchor(1);
        let mut store = TlsTrustStore::new();
        assert_eq!(store.install(&der, "demo CA"), Ok(0));
        assert_eq!(store.len(), 1);
        assert_eq!(store.anchor_der(0), Some(der.as_slice()));
        assert_eq!(store.anchor_label(0), Some("demo CA"));
    }

    #[test]
    fn a_byte_that_is_not_a_sequence_is_refused() {
        let mut store = TlsTrustStore::new();
        assert_eq!(store.install(&[0x02, 0x01, 0x00], "not a cert"), Err(TrustError::NotDerSequence));
        assert_eq!(store.install(&[], "nothing"), Err(TrustError::EmptyAnchor));
        assert!(store.is_empty(), "a refused install must not occupy a slot");
    }

    #[test]
    fn a_truncated_or_extended_anchor_is_refused() {
        let der = ca_anchor(1);
        let mut short = der.clone();
        short.pop();
        let mut long = der.clone();
        long.push(0x00);
        let mut store = TlsTrustStore::new();
        assert_eq!(store.install(&short, "cut"), Err(TrustError::NotDerSequence));
        assert_eq!(store.install(&long, "padded"), Err(TrustError::NotDerSequence));
    }

    #[test]
    fn a_key_this_client_cannot_verify_is_refused_at_the_door() {
        // Genuine certificates that name a key the client has no arithmetic for,
        // so the algorithm is the only thing the door can object to (the key
        // prefilter runs before the CA walk): Ed448, and EC on a curve with no
        // implementation behind it.
        let ed448 = certificate(&Fixture {
            spki_alg: tlv(0x30, ED448_OID),
            key: ED448_PUB.to_vec(),
            sig_alg: tlv(0x30, ED448_OID),
            exts: vec![basic_constraints_ext(true, None, true)],
            version: Some(2),
            serial: 1,
        });
        // `secp521r1` (1.3.132.0.35) beside `ecPublicKey`. The neighbour code
        // point, `secp384r1`, is a curve the client does have arithmetic for and
        // belongs in the accepting test below.
        let mut p521_alg = EC_PUBLIC_KEY_OID.to_vec();
        p521_alg.extend_from_slice(SECP521R1_OID);
        let p521 = certificate(&Fixture {
            spki_alg: tlv(0x30, &p521_alg),
            key: P521_PUB.to_vec(),
            sig_alg: tlv(0x30, ECDSA_WITH_SHA384_OID),
            exts: vec![basic_constraints_ext(true, None, true)],
            version: Some(2),
            serial: 2,
        });
        let mut store = TlsTrustStore::new();
        assert_eq!(store.install(&ed448, "ed448 root"), Err(TrustError::NoVerifiableKey));
        assert_eq!(store.install(&p521, "p521 root"), Err(TrustError::NoVerifiableKey));
        assert!(store.is_empty(), "a refused install must not occupy a slot");
    }

    #[test]
    fn an_rsa_or_ec_root_is_accepted_at_the_door() {
        // The shapes a public HTTPS chain ends on: an RSA root names
        // `rsaEncryption`, and an EC root names `ecPublicKey` with a curve the
        // verifier has arithmetic for -- both P-256 and P-384, since the client
        // advertises `ecdsa_secp384r1_sha384` and a root on that curve is the
        // issuer of most of the chains a browser is asked to accept.
        let rsa = rsa_ca(1);
        let ec = ec_ca(2, PRIME256V1_OID, P256_PUB, ECDSA_WITH_SHA256_OID);
        let ec384 = ec_ca(3, SECP384R1_OID, P384_PUB, ECDSA_WITH_SHA384_OID);
        let mut store = TlsTrustStore::new();
        assert_eq!(store.install(&rsa, "rsa root"), Ok(0));
        assert_eq!(store.install(&ec, "ec root"), Ok(1));
        assert_eq!(store.install(&ec384, "ec p384 root"), Ok(2));
        assert_eq!(store.anchor_der(0), Some(rsa.as_slice()));
    }

    #[test]
    fn a_real_public_root_of_1391_bytes_fits_the_store() {
        // ISRG Root X1 measures 1391 bytes, and the ceiling that used to hold 512
        // could not contain it at all. This is the size the store now has to
        // carry, framed the way that certificate is: a two-byte length header.
        let der = sized_ca(1391);
        assert_eq!(der.len(), 1391);
        let mut store = TlsTrustStore::new();
        assert_eq!(store.install(&der, "isrg-shaped root"), Ok(0));
        assert_eq!(store.anchor_der(0).unwrap().len(), 1391);
    }

    #[test]
    fn a_length_header_that_is_not_the_shortest_form_is_refused() {
        // 0x81 carrying a length below 0x80 is BER rather than DER. The same
        // bytes with a correct header are accepted, so the header is the only
        // thing this case can be objecting to.
        let mut der: Vec<u8> = vec![0x30, 0x81, 0x07];
        der.extend_from_slice(ED25519_ALGORITHM_OID);
        der.resize(10, 0x41);
        let mut store = TlsTrustStore::new();
        assert_eq!(store.install(&der, "ber"), Err(TrustError::NotDerSequence));
        assert_eq!(store.install(&ca_anchor(1), "der"), Ok(0));
    }

    #[test]
    fn an_oversized_anchor_is_refused() {
        // Shape-valid at one byte past what the store can hold, so the size is
        // the only thing the door can be objecting to.
        let der = sized_ca(MAX_ANCHOR_DER_BYTES + 1);
        let mut store = TlsTrustStore::new();
        assert_eq!(store.install(&der, "huge"), Err(TrustError::AnchorTooLarge));
        assert_eq!(check_anchor_shape(&sized_ca(MAX_ANCHOR_DER_BYTES)), Ok(()));
    }

    #[test]
    fn the_store_is_bounded_and_refuses_a_second_copy_of_the_same_key() {
        let mut store = TlsTrustStore::new();
        let first = ca_anchor(1);
        assert_eq!(store.install(&first, "anchor"), Ok(0));
        assert_eq!(
            store.install(&first, "again"),
            Err(TrustError::DuplicateAnchor),
            "installing the same key twice must be named, not silently deduped"
        );
        for i in 1..MAX_TRUSTED_ANCHORS {
            assert_eq!(store.install(&ca_anchor(1 + i as u8), "anchor"), Ok(i));
        }
        assert_eq!(store.len(), MAX_TRUSTED_ANCHORS);
        assert_eq!(store.install(&ca_anchor(9), "one more"), Err(TrustError::StoreFull));
    }

    #[test]
    fn revoking_closes_the_gap_so_a_client_never_stops_early() {
        let mut store = TlsTrustStore::new();
        store.install(&ca_anchor(10), "first").unwrap();
        store.install(&ca_anchor(20), "second").unwrap();
        store.install(&ca_anchor(30), "third").unwrap();
        assert_eq!(store.revoke(0), Ok(()));
        assert_eq!(store.len(), 2);
        assert_eq!(store.anchor_label(0), Some("second"));
        assert_eq!(store.anchor_label(1), Some("third"));
        assert_eq!(store.anchor_label(2), None);
        assert_eq!(store.revoke(2), Err(TrustError::NoAnchorAtIndex));
    }

    #[test]
    fn clearing_returns_the_device_to_trusting_nothing() {
        let mut store = TlsTrustStore::new();
        store.install(&ca_anchor(10), "only").unwrap();
        store.clear();
        assert!(store.is_empty());
        assert_eq!(store.anchor_der(0), None);
    }

    // ---------------------------------------------------------------------
    // New CA-constraint enforcement, driven through the production install path.
    // ---------------------------------------------------------------------

    #[test]
    fn a_ca_false_certificate_is_refused_by_install() {
        let mut store = TlsTrustStore::new();
        // Explicit CA:FALSE end-entity certificate, real key and structure.
        let leaf = certificate(&ed25519_fixture(1, vec![basic_constraints_ext(false, None, true)]));
        assert_eq!(store.install(&leaf, "leaf"), Err(TrustError::NotCaCertificate));
        // An empty basicConstraints SEQUENCE (`30 00`) is CA:FALSE by the DER
        // BOOLEAN default -- also refused, never read as a CA.
        let empty_bc = extension(&[0x06, 0x03, 0x55, 0x1d, 0x13], true, &[0x30, 0x00]);
        let empty = certificate(&ed25519_fixture(2, vec![empty_bc]));
        assert_eq!(store.install(&empty, "empty bc"), Err(TrustError::NotCaCertificate));
        assert!(store.is_empty(), "a refused install must not occupy a slot");

        // Prove the refusal is the CONSTRAINT, not a broken fixture: the same
        // certificate names a verifiable key, and flipping cA to TRUE makes the
        // very same door accept it.
        assert!(names_verifiable_key(&leaf));
        assert_eq!(check_anchor_shape(&leaf), Err(TrustError::NotCaCertificate));
        let ca = certificate(&ed25519_fixture(1, vec![basic_constraints_ext(true, None, true)]));
        assert_eq!(check_anchor_shape(&ca), Ok(()));
        assert_eq!(store.install(&ca, "same key as CA"), Ok(0));
    }

    #[test]
    fn a_certificate_without_basic_constraints_is_refused() {
        let mut store = TlsTrustStore::new();
        // v3 certificate carrying keyUsage but NO basicConstraints: absent is a
        // refusal, never a default.
        let no_bc = certificate(&ed25519_fixture(
            1,
            vec![key_usage_ext(&[0x02, 0x04], true)],
        ));
        assert_eq!(store.install(&no_bc, "no bc"), Err(TrustError::NoBasicConstraints));
        // A v1 certificate with no extensions block at all.
        let v1 = certificate(&Fixture {
            spki_alg: tlv(0x30, ED25519_ALGORITHM_OID),
            key: ED25519_PUB.to_vec(),
            sig_alg: tlv(0x30, ED25519_ALGORITHM_OID),
            exts: vec![],
            version: None,
            serial: 2,
        });
        assert_eq!(store.install(&v1, "v1"), Err(TrustError::NoBasicConstraints));
        assert!(store.is_empty());
        // Control: adding basicConstraints CA:TRUE to the same shape is accepted.
        assert!(names_verifiable_key(&no_bc));
        let with_bc = certificate(&ed25519_fixture(
            1,
            vec![key_usage_ext(&[0x02, 0x04], true), basic_constraints_ext(true, None, true)],
        ));
        assert_eq!(store.install(&with_bc, "with bc"), Ok(0));
    }

    #[test]
    fn key_usage_without_key_cert_sign_is_refused_and_with_it_accepted() {
        let mut store = TlsTrustStore::new();
        // CA:TRUE but keyUsage = digitalSignature only (bit 0): unused=7, 0x80.
        let no_sign = certificate(&ed25519_fixture(
            1,
            vec![basic_constraints_ext(true, None, true), key_usage_ext(&[0x07, 0x80], true)],
        ));
        assert_eq!(store.install(&no_sign, "no certSign"), Err(TrustError::KeyUsageWithoutCertSign));
        // Positive control: keyCertSign set (unused=2, 0x04) is accepted.
        let sign = certificate(&ed25519_fixture(
            2,
            vec![basic_constraints_ext(true, None, true), key_usage_ext(&[0x02, 0x04], true)],
        ));
        assert_eq!(store.install(&sign, "certSign"), Ok(0));
        // A CA with no keyUsage extension at all is accepted (keyUsage optional).
        let no_ku = certificate(&ed25519_fixture(3, vec![basic_constraints_ext(true, None, true)]));
        assert_eq!(store.install(&no_ku, "no ku"), Ok(1));
    }

    #[test]
    fn key_usage_bit_numbering_is_msb_first() {
        let mut store = TlsTrustStore::new();
        // The DER `03 02 02 04` must read as keyCertSign-ONLY and install end to
        // end: content octet 0x02 is the unused-bits count, 0x04 is bit 5.
        let ku_only = certificate(&ed25519_fixture(
            1,
            vec![basic_constraints_ext(true, None, true), key_usage_ext(&[0x02, 0x04], true)],
        ));
        assert_eq!(store.install(&ku_only, "keyCertSign only"), Ok(0));
        // `03 03 07 00 80` is bit 8 (encipherOnly) across two octets, NOT
        // keyCertSign: a set low bit must not be misread as bit 5. Refused.
        let bit8 = certificate(&ed25519_fixture(
            2,
            vec![basic_constraints_ext(true, None, true), key_usage_ext(&[0x07, 0x00, 0x80], true)],
        ));
        assert_eq!(store.install(&bit8, "bit8"), Err(TrustError::KeyUsageWithoutCertSign));
        // Two content octets with exactly seven unused bits: `03 03 07 04 80`
        // carries keyCertSign (bit 5) and encipherOnly (bit 8) -- accepted.
        let two = certificate(&ed25519_fixture(
            3,
            vec![basic_constraints_ext(true, None, true), key_usage_ext(&[0x07, 0x04, 0x80], true)],
        ));
        assert_eq!(store.install(&two, "two octet"), Ok(1));
    }

    #[test]
    fn a_ca_with_path_len_constraint_installs() {
        let mut store = TlsTrustStore::new();
        // Valid pathLenConstraint with CA:TRUE is accepted (parsed, not recorded).
        for (i, n) in [0u32, 1, 3, 256].iter().enumerate() {
            let der = certificate(&ed25519_fixture(
                i as u8 + 1,
                vec![basic_constraints_ext(true, Some(*n), true)],
            ));
            assert_eq!(store.install(&der, "pathLen CA"), Ok(i));
        }
    }

    #[test]
    fn malformed_or_duplicate_extensions_are_refused() {
        let mut store = TlsTrustStore::new();
        let bc_oid: &[u8] = &[0x06, 0x03, 0x55, 0x1d, 0x13];
        let good_bc = basic_constraints_ext(true, None, true);
        let good_ku = key_usage_ext(&[0x02, 0x04], true);

        // Duplicate basicConstraints.
        let dup_bc = certificate(&ed25519_fixture(1, vec![good_bc.clone(), good_bc.clone()]));
        assert_eq!(store.install(&dup_bc, "dup bc"), Err(TrustError::MalformedExtensions));
        // Duplicate keyUsage.
        let dup_ku = certificate(&ed25519_fixture(2, vec![good_bc.clone(), good_ku.clone(), good_ku.clone()]));
        assert_eq!(store.install(&dup_ku, "dup ku"), Err(TrustError::MalformedExtensions));
        // Non-canonical BOOLEAN (0x01) inside basicConstraints.
        let bad_bool = extension(bc_oid, true, &[0x30, 0x03, 0x01, 0x01, 0x01]);
        let bb = certificate(&ed25519_fixture(3, vec![bad_bool]));
        assert_eq!(store.install(&bb, "bad bool"), Err(TrustError::MalformedExtensions));
        // pathLenConstraint present while cA is FALSE.
        let pl_false = extension(bc_oid, true, &[0x30, 0x06, 0x01, 0x01, 0x00, 0x02, 0x01, 0x00]);
        let pf = certificate(&ed25519_fixture(4, vec![pl_false]));
        assert_eq!(store.install(&pf, "pathLen on FALSE"), Err(TrustError::MalformedExtensions));
        // Negative pathLenConstraint (top bit set) with cA TRUE.
        let neg = extension(bc_oid, true, &[0x30, 0x06, 0x01, 0x01, 0xff, 0x02, 0x01, 0xff]);
        let ng = certificate(&ed25519_fixture(5, vec![neg]));
        assert_eq!(store.install(&ng, "negative pathLen"), Err(TrustError::MalformedExtensions));
        // pathLenConstraint wider than four bytes.
        let wide = extension(bc_oid, true, &[0x30, 0x0a, 0x01, 0x01, 0xff, 0x02, 0x05, 0x01, 0x00, 0x00, 0x00, 0x00]);
        let wd = certificate(&ed25519_fixture(6, vec![wide]));
        assert_eq!(store.install(&wd, "wide pathLen"), Err(TrustError::MalformedExtensions));
        // keyUsage with non-zero unused trailing bits (unused=2 but low bits set).
        let dirty = certificate(&ed25519_fixture(7, vec![good_bc.clone(), key_usage_ext(&[0x02, 0x05], true)]));
        assert_eq!(store.install(&dirty, "dirty unused bits"), Err(TrustError::MalformedExtensions));
        // keyUsage with two content octets but unused != 7.
        let two_bad = certificate(&ed25519_fixture(8, vec![good_bc.clone(), key_usage_ext(&[0x00, 0x04, 0x00], true)]));
        assert_eq!(store.install(&two_bad, "two octet unused0"), Err(TrustError::MalformedExtensions));
        // Truncated extnValue: OCTET STRING declares 8 bytes but carries 3.
        let trunc = extension(bc_oid, true, &[0x04, 0x08, 0x30, 0x03, 0x01]);
        let tr = certificate(&ed25519_fixture(9, vec![trunc]));
        assert_eq!(store.install(&tr, "truncated extnValue"), Err(TrustError::MalformedExtensions));
        // An extension that is not a SEQUENCE.
        let not_seq = certificate(&ed25519_fixture(10, vec![tlv(0x04, &[0x01, 0x02, 0x03])]));
        assert_eq!(store.install(&not_seq, "not a sequence"), Err(TrustError::MalformedExtensions));
        assert!(store.is_empty(), "every malformed install must be refused without a slot");

        // Control: the well-formed extensions these surgeries corrupt do install.
        let good = certificate(&ed25519_fixture(11, vec![good_bc, good_ku]));
        assert_eq!(store.install(&good, "well formed"), Ok(0));
    }

    #[test]
    fn a_genuine_ca_anchor_installs_reports_and_revokes() {
        // Positive control on the production global store the console uses. All
        // state assertions are made under one held lock so a sibling test cannot
        // mutate the store mid-check; report_at_boot re-acquires the lock, so it
        // is called with the guard dropped.
        let der = ca_anchor(42);
        let der2 = ca_anchor(43);
        {
            let mut store = TLS_TRUST_STORE.lock();
            store.clear();
            assert_eq!(store.install(&der, "boot CA"), Ok(0));
            assert_eq!(store.len(), 1);
            assert_eq!(store.anchor_der(0), Some(der.as_slice()));
            assert_eq!(store.anchor_label(0), Some("boot CA"));
            assert_eq!(store.install(&der2, "second CA"), Ok(1));
            assert_eq!(store.revoke(0), Ok(()));
            assert_eq!(store.len(), 1);
            assert_eq!(store.anchor_label(0), Some("second CA"));
            assert_eq!(store.anchor_der(0), Some(der2.as_slice()));
            store.clear();
        }
        report_at_boot(); // empty-store branch
        {
            let mut store = TLS_TRUST_STORE.lock();
            store.clear();
            assert_eq!(store.install(&der, "boot CA"), Ok(0));
        }
        report_at_boot(); // populated-store branch
        TLS_TRUST_STORE.lock().clear();
    }
}
