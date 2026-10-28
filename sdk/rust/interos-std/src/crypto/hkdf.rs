//! HKDF (HMAC-based Extract-and-Expand Key Derivation Function, RFC 5869)
//! and TLS 1.3 Key Schedule primitives (RFC 8446 Section 7.1).
//!
//! Provides deterministic key expansion with zero allocations.

use super::hmac::HmacSha256;
use super::sha256::{Sha256, SHA256_DIGEST_LENGTH};

/// HKDF-Extract(salt, IKM) -> PRK
///
/// If salt is empty, a string of SHA256_DIGEST_LENGTH zeros is used as salt.
pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; SHA256_DIGEST_LENGTH] {
    if salt.is_empty() {
        let zeros = [0u8; SHA256_DIGEST_LENGTH];
        HmacSha256::compute(&zeros, ikm)
    } else {
        HmacSha256::compute(salt, ikm)
    }
}

/// HKDF-Expand(PRK, info, L) -> OKM
///
/// Expands a pseudorandom key `prk` into `okm` of length L using `info`.
/// L must not exceed 255 * SHA256_DIGEST_LENGTH (8160 bytes).
pub fn hkdf_expand(prk: &[u8; SHA256_DIGEST_LENGTH], info: &[u8], okm: &mut [u8]) -> Result<(), ()> {
    let out_len = okm.len();
    if out_len > 255 * SHA256_DIGEST_LENGTH {
        return Err(());
    }
    if out_len == 0 {
        return Ok(());
    }

    let n = (out_len + SHA256_DIGEST_LENGTH - 1) / SHA256_DIGEST_LENGTH;
    let mut t = [0u8; SHA256_DIGEST_LENGTH];
    let mut t_len = 0;
    let mut offset = 0;

    for i in 1..=(n as u8) {
        let mut hmac = HmacSha256::new(prk);
        if t_len > 0 {
            hmac.update(&t[..t_len]);
        }
        hmac.update(info);
        hmac.update(&[i]);
        t = hmac.finalize();
        t_len = SHA256_DIGEST_LENGTH;

        let to_copy = (out_len - offset).min(SHA256_DIGEST_LENGTH);
        okm[offset..offset + to_copy].copy_from_slice(&t[..to_copy]);
        offset += to_copy;
    }

    Ok(())
}

/// TLS 1.3 HKDF-Expand-Label (RFC 8446 Section 7.1)
///
/// Formats the HkdfLabel structure:
///   uint16 length
///   opaque label<7..255> = "tls13 " + Label
///   opaque context<0..255> = Context
/// and expands into `out`.
pub fn hkdf_expand_label(
    secret: &[u8; SHA256_DIGEST_LENGTH],
    label: &[u8],
    context: &[u8],
    out: &mut [u8],
) -> Result<(), ()> {
    let prefix = b"tls13 ";
    let full_label_len = prefix.len() + label.len();
    if full_label_len > 255 || context.len() > 255 {
        return Err(());
    }

    // Buffer for HkdfLabel: 2 (length) + 1 (label len) + label + 1 (context len) + context
    let mut info = [0u8; 256];
    let out_len = out.len() as u16;
    info[0] = (out_len >> 8) as u8;
    info[1] = (out_len & 0xff) as u8;
    info[2] = full_label_len as u8;
    info[3..3 + prefix.len()].copy_from_slice(prefix);
    info[3 + prefix.len()..3 + full_label_len].copy_from_slice(label);

    let mut pos = 3 + full_label_len;
    info[pos] = context.len() as u8;
    pos += 1;
    info[pos..pos + context.len()].copy_from_slice(context);
    pos += context.len();

    hkdf_expand(secret, &info[..pos], out)
}

/// TLS 1.3 Derive-Secret(Secret, Label, Messages) -> 32-byte secret
///
/// Computes HKDF-Expand-Label(Secret, Label, transcript_hash, 32).
pub fn derive_secret(
    secret: &[u8; SHA256_DIGEST_LENGTH],
    label: &[u8],
    transcript_hash: &[u8; SHA256_DIGEST_LENGTH],
) -> [u8; SHA256_DIGEST_LENGTH] {
    let mut out = [0u8; SHA256_DIGEST_LENGTH];
    let _ = hkdf_expand_label(secret, label, transcript_hash, &mut out);
    out
}

/// Empty SHA-256 hash helper for Derive-Secret when no transcript messages are present.
pub fn empty_hash() -> [u8; SHA256_DIGEST_LENGTH] {
    Sha256::digest(&[])
}
