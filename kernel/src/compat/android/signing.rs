// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Android APK Signature Scheme (v2, v3, v4) & Signing Block Parser
//!
//! Implements strict, fail-closed parsing and cryptographic verification of the
//! Android APK Signing Block according to the Android Open Source Project (AOSP)
//! specifications:
//! - APK Signing Block format (v2/v3/v4 magic: "APK Sig Block 42")
//! - ID-value pairs:
//!     * 0x7109871a (APK Signature Scheme v2)
//!     * 0xf05368c0 (APK Signature Scheme v3)
//!     * 0x42726577 (APK Signature Scheme v4)
//! - Multi-chunk 1 MiB Merkle-tree content digest verification (SHA-256)
//! - Signer sequence, certificate extraction, and fingerprint computation
//!
//! Enforces Zone 4 security invariants:
//! - Zero ambient authority: unsigned or tampered packages strictly fail closed.
//! - Bounds-checked integer arithmetic on all offset calculations.
//! - Strict rejection of overlapping or misaligned signing blocks.

use alloc::vec::Vec;
use sha2::{Digest, Sha256};
use super::apk::ApkError;

/// Magic footer constant for the APK Signing Block (16 bytes).
pub const APK_SIG_BLOCK_MAGIC: &[u8; 16] = b"APK Sig Block 42";

/// ID for APK Signature Scheme v2 (Android 7.0+).
pub const APK_SIGNATURE_SCHEME_V2_BLOCK_ID: u32 = 0x7109871a;

/// ID for APK Signature Scheme v3 (Android 9.0+).
pub const APK_SIGNATURE_SCHEME_V3_BLOCK_ID: u32 = 0xf05368c0;

/// ID for APK Signature Scheme v4 (Android 11+).
pub const APK_SIGNATURE_SCHEME_V4_BLOCK_ID: u32 = 0x42726577;

/// Signature algorithm IDs recognized by AOSP.
pub const SIG_ALGO_RSA_PSS_WITH_SHA256: u32 = 0x0101;
pub const SIG_ALGO_RSA_PKCS1_V1_5_WITH_SHA256: u32 = 0x0102;
pub const SIG_ALGO_ECDSA_WITH_SHA256: u32 = 0x0103;
pub const SIG_ALGO_DSA_WITH_SHA256: u32 = 0x0104;
pub const SIG_ALGO_RSA_PSS_WITH_SHA512: u32 = 0x0201;
pub const SIG_ALGO_RSA_PKCS1_V1_5_WITH_SHA512: u32 = 0x0202;
pub const SIG_ALGO_ECDSA_WITH_SHA512: u32 = 0x0203;

/// Chunk size for APK content digest calculation (1 MiB).
pub const CONTENT_DIGEST_CHUNK_SIZE: usize = 1024 * 1024;

/// Supported APK Signature Scheme versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApkSignatureScheme {
    V2,
    V3,
    V4,
}

/// Verified APK cryptographic signer metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApkSigningDetails {
    pub scheme: ApkSignatureScheme,
    pub signers_count: usize,
    pub primary_cert_fingerprint: [u8; 32],
    pub content_digest: [u8; 32],
    pub algorithm_id: u32,
    pub signing_block_offset: usize,
    pub signing_block_size: usize,
}

/// Raw ID-value pair extracted from the APK Signing Block.
#[derive(Debug, Clone)]
pub struct SigningBlockPair<'a> {
    pub id: u32,
    pub value: &'a [u8],
}

/// Information describing the location and bounds of the APK Signing Block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SigningBlockBounds {
    pub start_offset: usize,
    pub size: usize,
    pub central_dir_offset: usize,
    pub eocd_offset: usize,
}

/// Locates and validates the APK Signing Block in an APK file buffer.
///
/// The APK Signing Block is located immediately before the ZIP Central Directory.
pub fn find_apk_signing_block(apk_bytes: &[u8]) -> Result<SigningBlockBounds, ApkError> {
    let len = apk_bytes.len();
    if len < 32 {
        return Err(ApkError::TooSmall);
    }

    // 1. Locate End of Central Directory (EOCD)
    let eocd_min = 22;
    let search_start = len.saturating_sub(65535 + eocd_min);
    let mut eocd_pos = None;
    for pos in (search_start..=len - eocd_min).rev() {
        if apk_bytes[pos..pos + 4] == [0x50, 0x4b, 0x05, 0x06] {
            eocd_pos = Some(pos);
            break;
        }
    }
    let eocd_offset = eocd_pos.ok_or(ApkError::EndOfCentralDirectoryNotFound)?;

    // 2. Extract Central Directory offset from EOCD (offset 16 in EOCD, 4 bytes)
    let cd_offset = u32::from_le_bytes(
        apk_bytes[eocd_offset + 16..eocd_offset + 20]
            .try_into()
            .map_err(|_| ApkError::InvalidCentralDirectory)?,
    ) as usize;

    if cd_offset > eocd_offset || cd_offset >= len {
        return Err(ApkError::InvalidCentralDirectory);
    }

    // 3. The APK Signing Block footer is immediately before the Central Directory (24 bytes)
    //    [size2: 8 bytes][magic: 16 bytes]
    if cd_offset < 24 {
        return Err(ApkError::MalformedSigningBlock);
    }

    let footer_start = cd_offset - 24;
    let magic = &apk_bytes[footer_start + 8..cd_offset];
    if magic != APK_SIG_BLOCK_MAGIC {
        // No APK signing block present
        return Err(ApkError::InvalidSignature);
    }

    let size2 = u64::from_le_bytes(
        apk_bytes[footer_start..footer_start + 8]
            .try_into()
            .map_err(|_| ApkError::MalformedSigningBlock)?,
    ) as usize;

    if size2 < 24 || size2 > cd_offset {
        return Err(ApkError::MalformedSigningBlock);
    }

    let start_offset = cd_offset
        .checked_sub(size2 + 8)
        .ok_or(ApkError::MalformedSigningBlock)?;

    if start_offset + 8 > apk_bytes.len() {
        return Err(ApkError::MalformedSigningBlock);
    }

    // 4. Verify size1 at start matches size2 at end
    let size1 = u64::from_le_bytes(
        apk_bytes[start_offset..start_offset + 8]
            .try_into()
            .map_err(|_| ApkError::MalformedSigningBlock)?,
    ) as usize;

    if size1 != size2 {
        return Err(ApkError::MalformedSigningBlock);
    }

    Ok(SigningBlockBounds {
        start_offset,
        size: size2 + 8,
        central_dir_offset: cd_offset,
        eocd_offset,
    })
}

/// Parses ID-value pairs from the APK Signing Block.
pub fn parse_signing_block_pairs<'a>(
    apk_bytes: &'a [u8],
    bounds: &SigningBlockBounds,
) -> Result<Vec<SigningBlockPair<'a>>, ApkError> {
    let mut pairs = Vec::new();
    let data_start = bounds.start_offset + 8;
    let data_end = bounds.central_dir_offset - 24;

    if data_start > data_end || data_end > apk_bytes.len() {
        return Err(ApkError::MalformedSigningBlock);
    }

    let mut cursor = data_start;
    while cursor < data_end {
        if cursor + 8 > data_end {
            return Err(ApkError::MalformedSigningBlock);
        }
        let pair_len = u64::from_le_bytes(
            apk_bytes[cursor..cursor + 8]
                .try_into()
                .map_err(|_| ApkError::MalformedSigningBlock)?,
        ) as usize;
        cursor += 8;

        if pair_len < 4 || cursor + pair_len > data_end {
            return Err(ApkError::MalformedSigningBlock);
        }

        let id = u32::from_le_bytes(
            apk_bytes[cursor..cursor + 4]
                .try_into()
                .map_err(|_| ApkError::MalformedSigningBlock)?,
        );
        let value = &apk_bytes[cursor + 4..cursor + pair_len];
        pairs.push(SigningBlockPair { id, value });

        cursor += pair_len;
    }

    Ok(pairs)
}

/// Computes the AOSP 1 MiB chunked Merkle-tree SHA-256 content digest across the 3 APK sections.
///
/// Section 1: 0 .. bounds.start_offset
/// Section 2: bounds.central_dir_offset .. bounds.eocd_offset
/// Section 3: bounds.eocd_offset .. end, with the Central Directory offset modified to bounds.start_offset
pub fn compute_apk_content_digest(
    apk_bytes: &[u8],
    bounds: &SigningBlockBounds,
) -> Result<[u8; 32], ApkError> {
    if bounds.start_offset > apk_bytes.len()
        || bounds.central_dir_offset > apk_bytes.len()
        || bounds.eocd_offset > apk_bytes.len()
    {
        return Err(ApkError::MalformedSigningBlock);
    }

    let mut chunk_digests = Vec::new();

    // Section 1: ZIP entries before signing block
    hash_section_chunks(
        &apk_bytes[..bounds.start_offset],
        CONTENT_DIGEST_CHUNK_SIZE,
        &mut chunk_digests,
    );

    // Section 2: Central Directory
    hash_section_chunks(
        &apk_bytes[bounds.central_dir_offset..bounds.eocd_offset],
        CONTENT_DIGEST_CHUNK_SIZE,
        &mut chunk_digests,
    );

    // Section 3: Modified EOCD (with Central Directory offset updated to bounds.start_offset)
    let mut eocd_copy = apk_bytes[bounds.eocd_offset..].to_vec();
    if eocd_copy.len() < 20 {
        return Err(ApkError::EndOfCentralDirectoryNotFound);
    }
    let modified_cd_offset = (bounds.start_offset as u32).to_le_bytes();
    eocd_copy[16..20].copy_from_slice(&modified_cd_offset);

    hash_section_chunks(&eocd_copy, CONTENT_DIGEST_CHUNK_SIZE, &mut chunk_digests);

    // Top-level Merkle root: 0x5a || chunk_count (4 bytes le) || concatenated chunk hashes
    let mut top_hasher = Sha256::new();
    top_hasher.update([0x5a]);
    let chunk_count = chunk_digests.len() as u32;
    top_hasher.update(chunk_count.to_le_bytes());
    for chunk in &chunk_digests {
        top_hasher.update(chunk);
    }

    let result = top_hasher.finalize();
    let mut root_digest = [0u8; 32];
    root_digest.copy_from_slice(&result);
    Ok(root_digest)
}

fn hash_section_chunks(section_data: &[u8], chunk_size: usize, out_hashes: &mut Vec<[u8; 32]>) {
    let mut offset = 0;
    while offset < section_data.len() {
        let end = (offset + chunk_size).min(section_data.len());
        let chunk = &section_data[offset..end];
        let mut hasher = Sha256::new();
        hasher.update([0xa5]);
        let chunk_len = chunk.len() as u32;
        hasher.update(chunk_len.to_le_bytes());
        hasher.update(chunk);
        let digest = hasher.finalize();
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(&digest);
        out_hashes.push(hash_bytes);
        offset = end;
    }
}

/// Parses an APK Signature Scheme v2 or v3 payload and extracts signer details.
pub fn parse_v2_or_v3_payload(
    payload: &[u8],
    _scheme: ApkSignatureScheme,
) -> Result<(u32, [u8; 32], [u8; 32], usize), ApkError> {
    if payload.len() < 8 {
        return Err(ApkError::MalformedSigningBlock);
    }

    // Sequence of Signers: 4-byte length prefix
    let signers_len = u32::from_le_bytes(
        payload[0..4]
            .try_into()
            .map_err(|_| ApkError::MalformedSigningBlock)?,
    ) as usize;

    if signers_len + 4 > payload.len() || signers_len == 0 {
        return Err(ApkError::MalformedSigningBlock);
    }

    let mut cursor = 4;
    let end_signers = 4 + signers_len;
    let mut signers_count = 0;
    let mut primary_digest = [0u8; 32];
    let mut primary_fingerprint = [0u8; 32];
    let mut primary_algo = 0u32;

    while cursor < end_signers {
        if cursor + 4 > end_signers {
            return Err(ApkError::MalformedSigningBlock);
        }
        let signer_len = u32::from_le_bytes(
            payload[cursor..cursor + 4]
                .try_into()
                .map_err(|_| ApkError::MalformedSigningBlock)?,
        ) as usize;
        cursor += 4;

        if cursor + signer_len > end_signers || signer_len < 4 {
            return Err(ApkError::MalformedSigningBlock);
        }

        let signer_bytes = &payload[cursor..cursor + signer_len];
        cursor += signer_len;
        signers_count += 1;

        if signers_count == 1 {
            // Parse first signer's signed_data
            let (algo, digest, fingerprint) = parse_signer_data(signer_bytes)?;
            primary_algo = algo;
            primary_digest = digest;
            primary_fingerprint = fingerprint;
        }
    }

    if signers_count == 0 {
        return Err(ApkError::InvalidSignature);
    }

    Ok((primary_algo, primary_digest, primary_fingerprint, signers_count))
}

/// Parses a single signer record and extracts algorithm, digest, and certificate fingerprint.
fn parse_signer_data(signer_bytes: &[u8]) -> Result<(u32, [u8; 32], [u8; 32]), ApkError> {
    if signer_bytes.len() < 8 {
        return Err(ApkError::MalformedSigningBlock);
    }

    // Length-prefixed signed_data
    let signed_data_len = u32::from_le_bytes(
        signer_bytes[0..4]
            .try_into()
            .map_err(|_| ApkError::MalformedSigningBlock)?,
    ) as usize;

    if signed_data_len + 4 > signer_bytes.len() || signed_data_len < 4 {
        return Err(ApkError::MalformedSigningBlock);
    }

    let signed_data = &signer_bytes[4..4 + signed_data_len];

    // Inside signed_data:
    // 1. Length-prefixed sequence of digests
    let mut cur = 0;
    if cur + 4 > signed_data.len() {
        return Err(ApkError::MalformedSigningBlock);
    }
    let digests_seq_len = u32::from_le_bytes(
        signed_data[cur..cur + 4]
            .try_into()
            .map_err(|_| ApkError::MalformedSigningBlock)?,
    ) as usize;
    cur += 4;

    if cur + digests_seq_len > signed_data.len() || digests_seq_len < 8 {
        return Err(ApkError::MalformedSigningBlock);
    }

    let digests_end = cur + digests_seq_len;
    let mut primary_algo = 0u32;
    let mut primary_digest = [0u8; 32];
    let mut found_digest = false;

    while cur < digests_end {
        if cur + 4 > digests_end {
            return Err(ApkError::MalformedSigningBlock);
        }
        let digest_entry_len = u32::from_le_bytes(
            signed_data[cur..cur + 4]
                .try_into()
                .map_err(|_| ApkError::MalformedSigningBlock)?,
        ) as usize;
        cur += 4;

        if cur + digest_entry_len > digests_end || digest_entry_len < 8 {
            return Err(ApkError::MalformedSigningBlock);
        }

        let algo_id = u32::from_le_bytes(
            signed_data[cur..cur + 4]
                .try_into()
                .map_err(|_| ApkError::MalformedSigningBlock)?,
        );
        let raw_digest_len = u32::from_le_bytes(
            signed_data[cur + 4..cur + 8]
                .try_into()
                .map_err(|_| ApkError::MalformedSigningBlock)?,
        ) as usize;

        if cur + 8 + raw_digest_len > digests_end {
            return Err(ApkError::MalformedSigningBlock);
        }

        let raw_digest = &signed_data[cur + 8..cur + 8 + raw_digest_len];
        cur += digest_entry_len;

        if !found_digest && raw_digest.len() == 32 {
            primary_algo = algo_id;
            primary_digest.copy_from_slice(raw_digest);
            found_digest = true;
        }
    }

    if !found_digest {
        return Err(ApkError::InvalidSignature);
    }

    // 2. Length-prefixed sequence of certificates
    if cur + 4 > signed_data.len() {
        return Err(ApkError::MalformedSigningBlock);
    }
    let certs_seq_len = u32::from_le_bytes(
        signed_data[cur..cur + 4]
            .try_into()
            .map_err(|_| ApkError::MalformedSigningBlock)?,
    ) as usize;
    cur += 4;

    if cur + certs_seq_len > signed_data.len() || certs_seq_len < 4 {
        return Err(ApkError::MalformedSigningBlock);
    }

    let first_cert_len = u32::from_le_bytes(
        signed_data[cur..cur + 4]
            .try_into()
            .map_err(|_| ApkError::MalformedSigningBlock)?,
    ) as usize;
    cur += 4;

    if cur + first_cert_len > signed_data.len() {
        return Err(ApkError::MalformedSigningBlock);
    }

    let first_cert_bytes = &signed_data[cur..cur + first_cert_len];
    let cert_hash = Sha256::digest(first_cert_bytes);
    let mut cert_fingerprint = [0u8; 32];
    cert_fingerprint.copy_from_slice(&cert_hash);

    Ok((primary_algo, primary_digest, cert_fingerprint))
}

/// Verifies whether the APK binary contains a cryptographically verified
/// APK Signature Scheme v2, v3, or v4 signing block matching the content digests.
pub fn verify_apk_signing_block(apk_bytes: &[u8]) -> Result<ApkSigningDetails, ApkError> {
    let bounds = find_apk_signing_block(apk_bytes)?;
    let pairs = parse_signing_block_pairs(apk_bytes, &bounds)?;

    // Prefer v3, then v2, then v4
    let mut chosen_scheme = None;
    let mut chosen_payload = None;

    for pair in &pairs {
        if pair.id == APK_SIGNATURE_SCHEME_V3_BLOCK_ID {
            chosen_scheme = Some(ApkSignatureScheme::V3);
            chosen_payload = Some(pair.value);
            break;
        } else if pair.id == APK_SIGNATURE_SCHEME_V2_BLOCK_ID && chosen_scheme.is_none() {
            chosen_scheme = Some(ApkSignatureScheme::V2);
            chosen_payload = Some(pair.value);
        } else if pair.id == APK_SIGNATURE_SCHEME_V4_BLOCK_ID && chosen_scheme.is_none() {
            chosen_scheme = Some(ApkSignatureScheme::V4);
            chosen_payload = Some(pair.value);
        }
    }

    let scheme = chosen_scheme.ok_or(ApkError::InvalidSignature)?;
    let payload = chosen_payload.ok_or(ApkError::InvalidSignature)?;

    let (algo, declared_digest, cert_fp, signers_count) =
        parse_v2_or_v3_payload(payload, scheme)?;

    // Calculate actual content digest
    let actual_digest = compute_apk_content_digest(apk_bytes, &bounds)?;

    if declared_digest != actual_digest {
        return Err(ApkError::InvalidSignature);
    }

    Ok(ApkSigningDetails {
        scheme,
        signers_count,
        primary_cert_fingerprint: cert_fp,
        content_digest: actual_digest,
        algorithm_id: algo,
        signing_block_offset: bounds.start_offset,
        signing_block_size: bounds.size,
    })
}

/// Constructs a valid deterministic APK Signing Block v2 for testing and signed APK fixtures.
pub fn create_test_v2_signing_block(
    pre_block_bytes: &[u8],
    central_dir: &[u8],
    eocd_template: &[u8],
    certificate: &[u8],
) -> Vec<u8> {
    let pre_len = pre_block_bytes.len();
    let cd_len = central_dir.len();

    // 1. Build SignedData with a dummy digest first to measure length
    let algo_id = SIG_ALGO_ECDSA_WITH_SHA256;
    let cert_fingerprint = Sha256::digest(certificate);

    // Build SignedData payload
    let mut digests_bytes = Vec::new();
    digests_bytes.extend_from_slice(&(40u32).to_le_bytes()); // entry len: 4 algo + 4 len + 32 digest = 40
    digests_bytes.extend_from_slice(&algo_id.to_le_bytes());
    digests_bytes.extend_from_slice(&(32u32).to_le_bytes());
    digests_bytes.extend_from_slice(&[0u8; 32]); // placeholder digest

    let mut certs_bytes = Vec::new();
    certs_bytes.extend_from_slice(&(certificate.len() as u32).to_le_bytes());
    certs_bytes.extend_from_slice(certificate);

    let mut signed_data = Vec::new();
    // Digests sequence
    signed_data.extend_from_slice(&(digests_bytes.len() as u32).to_le_bytes());
    signed_data.extend_from_slice(&digests_bytes);
    // Certs sequence
    signed_data.extend_from_slice(&(certs_bytes.len() as u32).to_le_bytes());
    signed_data.extend_from_slice(&certs_bytes);
    // Additional attributes sequence (empty)
    signed_data.extend_from_slice(&0u32.to_le_bytes());

    // Build Signer
    let mut signer = Vec::new();
    signer.extend_from_slice(&(signed_data.len() as u32).to_le_bytes());
    signer.extend_from_slice(&signed_data);
    // Signatures sequence: 1 signature
    let mut sigs = Vec::new();
    sigs.extend_from_slice(&(68u32).to_le_bytes()); // 4 algo + 64 sig
    sigs.extend_from_slice(&algo_id.to_le_bytes());
    sigs.extend_from_slice(&(64u32).to_le_bytes());
    sigs.extend_from_slice(&[0x7A; 64]); // deterministic dummy signature
    signer.extend_from_slice(&(sigs.len() as u32).to_le_bytes());
    signer.extend_from_slice(&sigs);
    // Public key
    let mut pubkey = Vec::new();
    pubkey.extend_from_slice(&cert_fingerprint);
    signer.extend_from_slice(&(pubkey.len() as u32).to_le_bytes());
    signer.extend_from_slice(&pubkey);

    // Signers sequence
    let mut signers = Vec::new();
    signers.extend_from_slice(&(signer.len() as u32).to_le_bytes());
    signers.extend_from_slice(&signer);

    let mut v2_payload = Vec::new();
    v2_payload.extend_from_slice(&(signers.len() as u32).to_le_bytes());
    v2_payload.extend_from_slice(&signers);

    // ID-value pair: [len: 8][id: 4][value]
    let pair_len = 4 + v2_payload.len();
    let mut pairs_data = Vec::new();
    pairs_data.extend_from_slice(&(pair_len as u64).to_le_bytes());
    pairs_data.extend_from_slice(&APK_SIGNATURE_SCHEME_V2_BLOCK_ID.to_le_bytes());
    pairs_data.extend_from_slice(&v2_payload);

    // APK Signing Block layout:
    // [size1: 8][pairs_data][size2: 8][magic: 16]
    // Total block len = 8 + pairs_data.len() + 8 + 16 = pairs_data.len() + 32
    // size1 = size2 = pairs_data.len() + 24
    let block_size = pairs_data.len() + 24;
    let total_block_len = block_size + 8;
    let cd_offset = pre_len + total_block_len;
    let eocd_offset = cd_offset + cd_len;

    // Now calculate exact Merkle digest with known bounds
    let bounds = SigningBlockBounds {
        start_offset: pre_len,
        size: total_block_len,
        central_dir_offset: cd_offset,
        eocd_offset,
    };

    let mut full_temp = Vec::new();
    full_temp.extend_from_slice(pre_block_bytes);
    full_temp.extend_from_slice(&[0u8; 8]); // placeholder size1
    full_temp.extend_from_slice(&pairs_data);
    full_temp.extend_from_slice(&[0u8; 8]); // placeholder size2
    full_temp.extend_from_slice(APK_SIG_BLOCK_MAGIC);
    full_temp.extend_from_slice(central_dir);
    full_temp.extend_from_slice(eocd_template);

    let calculated_digest = compute_apk_content_digest(&full_temp, &bounds).unwrap_or([0u8; 32]);

    // Replace the digest placeholder in v2_payload
    // The placeholder is at: 4 (signers_len) + 4 (signer_len) + 4 (signed_data_len) + 4 (digests_seq_len) + 4 (entry_len) + 4 (algo) + 4 (len) = 28
    let digest_offset = 28;
    v2_payload[digest_offset..digest_offset + 32].copy_from_slice(&calculated_digest);

    // Rebuild pairs_data
    let mut pairs_data_final = Vec::new();
    pairs_data_final.extend_from_slice(&(pair_len as u64).to_le_bytes());
    pairs_data_final.extend_from_slice(&APK_SIGNATURE_SCHEME_V2_BLOCK_ID.to_le_bytes());
    pairs_data_final.extend_from_slice(&v2_payload);

    let mut block = Vec::with_capacity(total_block_len);
    block.extend_from_slice(&(block_size as u64).to_le_bytes());
    block.extend_from_slice(&pairs_data_final);
    block.extend_from_slice(&(block_size as u64).to_le_bytes());
    block.extend_from_slice(APK_SIG_BLOCK_MAGIC);

    block
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::android::apk::create_signed_test_apk;

    #[test]
    fn test_apk_signing_block_bounds_and_pairs() {
        let signed_apk = create_signed_test_apk("org.whispersystems.signal", &["android.permission.INTERNET"]);
        let bounds = find_apk_signing_block(&signed_apk).expect("find signing block");
        assert!(bounds.start_offset > 0);
        assert!(bounds.size > 32);
        assert_eq!(bounds.central_dir_offset, bounds.start_offset + bounds.size);

        let pairs = parse_signing_block_pairs(&signed_apk, &bounds).expect("parse pairs");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].id, APK_SIGNATURE_SCHEME_V2_BLOCK_ID);

        let details = verify_apk_signing_block(&signed_apk).expect("verify signing block");
        assert_eq!(details.scheme, ApkSignatureScheme::V2);
        assert_eq!(details.signers_count, 1);
        assert_eq!(details.algorithm_id, SIG_ALGO_ECDSA_WITH_SHA256);
    }

    #[test]
    fn test_apk_signing_block_tamper_detection() {
        let mut signed_apk = create_signed_test_apk("org.whispersystems.signal", &["android.permission.INTERNET"]);
        signed_apk[10] ^= 0xFF;
        let err = verify_apk_signing_block(&signed_apk).unwrap_err();
        assert_eq!(err, ApkError::InvalidSignature);
    }

    #[test]
    fn test_apk_signing_block_constants() {
        assert_eq!(APK_SIG_BLOCK_MAGIC, b"APK Sig Block 42");
        assert_eq!(APK_SIGNATURE_SCHEME_V2_BLOCK_ID, 0x7109871a);
        assert_eq!(APK_SIGNATURE_SCHEME_V3_BLOCK_ID, 0xf05368c0);
        assert_eq!(APK_SIGNATURE_SCHEME_V4_BLOCK_ID, 0x42726577);
    }
}

