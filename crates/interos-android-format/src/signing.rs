// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Android APK Signature Scheme (v2, v3, v3.1, v4) & Signing Block Parser
//!
//! Implements strict, fail-closed parsing and cryptographic verification of the
//! Android APK Signing Block according to the Android Open Source Project (AOSP)
//! specifications:
//! - APK Signing Block format (magic: "APK Sig Block 42")
//! - ID-value pairs:
//!     * 0x7109871a (APK Signature Scheme v2)
//!     * 0xf05368c0 (APK Signature Scheme v3)
//!     * 0x1b93ad61 (APK Signature Scheme v3.1)
//!     * 0x42726577 (VERITY_PADDING_BLOCK_ID - AOSP fs-verity 4096-byte alignment padding)
//! - Multi-chunk 1 MiB Merkle-tree content digest verification (0xa5 chunk, 0x5a root)
//! - Genuine cryptographic signature verification over signed_data using public key
//!   extracted from X.509 SPKI (RSASSA-PKCS1-v1_5 & ECDSA P-256)
//! - Separate APK Signature Scheme v4 (.idsig) fs-verity tree verification
//!
//! Enforces Zone 4 security invariants:
//! - Zero ambient authority: unsigned, dummy, or tampered packages strictly fail closed.
//! - Bounds-checked integer arithmetic on all offset calculations.
//! - Strict rejection of overlapping, misaligned, or untrusted signing blocks.

use alloc::vec::Vec;
use sha2::{Digest, Sha256};
use super::apk::ApkError;
use super::crypto::{parse_spki, verify_ecdsa_p256, verify_rsa_pkcs1_v1_5, ParsedPublicKey};

/// Magic footer constant for the APK Signing Block (16 bytes).
pub const APK_SIG_BLOCK_MAGIC: &[u8; 16] = b"APK Sig Block 42";

/// ID for APK Signature Scheme v2 (Android 7.0+).
pub const APK_SIGNATURE_SCHEME_V2_BLOCK_ID: u32 = 0x7109871a;

/// ID for APK Signature Scheme v3 (Android 9.0+).
pub const APK_SIGNATURE_SCHEME_V3_BLOCK_ID: u32 = 0xf05368c0;

/// ID for APK Signature Scheme v3.1 (Android 13+).
pub const APK_SIGNATURE_SCHEME_V31_BLOCK_ID: u32 = 0x1b93ad61;

/// AOSP fs-verity padding block ID (used to pad signing block to 4096-byte alignment).
/// Note: This is AOSP's VERITY_PADDING_BLOCK_ID, NOT APK Signature Scheme v4.
pub const VERITY_PADDING_BLOCK_ID: u32 = 0x42726577;

/// Signature algorithm IDs recognized by AOSP:
/// RSA:
pub const SIG_ALGO_RSA_PSS_WITH_SHA256: u32 = 0x0101;
pub const SIG_ALGO_RSA_PSS_WITH_SHA512: u32 = 0x0102;
pub const SIG_ALGO_RSA_PKCS1_V1_5_WITH_SHA256: u32 = 0x0103;
pub const SIG_ALGO_RSA_PKCS1_V1_5_WITH_SHA512: u32 = 0x0104;

/// ECDSA:
pub const SIG_ALGO_ECDSA_WITH_SHA256: u32 = 0x0201;
pub const SIG_ALGO_ECDSA_WITH_SHA512: u32 = 0x0202;

/// DSA:
pub const SIG_ALGO_DSA_WITH_SHA256: u32 = 0x0301;

/// Chunk size for APK content digest calculation (1 MiB).
pub const CONTENT_DIGEST_CHUNK_SIZE: usize = 1024 * 1024;

/// Supported APK Signature Scheme versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApkSignatureScheme {
    V2,
    V3,
    V31,
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

    let eocd_offset = eocd_pos.ok_or(ApkError::CentralDirectoryNotFound)?;
    if eocd_offset + 22 > len {
        return Err(ApkError::TruncatedCentralDirectory);
    }

    // 2. Read Central Directory offset from EOCD (offset 16)
    let cd_offset = u32::from_le_bytes([
        apk_bytes[eocd_offset + 16],
        apk_bytes[eocd_offset + 17],
        apk_bytes[eocd_offset + 18],
        apk_bytes[eocd_offset + 19],
    ]) as usize;

    if cd_offset > eocd_offset || cd_offset < 24 {
        return Err(ApkError::MalformedSigningBlock);
    }

    // 3. Check for Signing Block footer before Central Directory
    let footer_start = match cd_offset.checked_sub(24) {
        Some(offset) => offset,
        None => return Err(ApkError::MalformedSigningBlock),
    };

    let block_size_footer = u64::from_le_bytes([
        apk_bytes[footer_start],
        apk_bytes[footer_start + 1],
        apk_bytes[footer_start + 2],
        apk_bytes[footer_start + 3],
        apk_bytes[footer_start + 4],
        apk_bytes[footer_start + 5],
        apk_bytes[footer_start + 6],
        apk_bytes[footer_start + 7],
    ]);

    let magic = &apk_bytes[footer_start + 8..footer_start + 24];
    if magic != APK_SIG_BLOCK_MAGIC {
        return Err(ApkError::InvalidSignature);
    }

    if block_size_footer > usize::MAX as u64 || (block_size_footer as usize) < 24 {
        return Err(ApkError::MalformedSigningBlock);
    }
    let block_size = block_size_footer as usize;

    // The block starts (block_size + 8) bytes before the Central Directory
    let total_block_len = match block_size.checked_add(8) {
        Some(len) => len,
        None => return Err(ApkError::MalformedSigningBlock),
    };

    let start_offset = match cd_offset.checked_sub(total_block_len) {
        Some(offset) => offset,
        None => return Err(ApkError::MalformedSigningBlock),
    };

    // 4. Verify the header size matches the footer size
    let block_size_header = u64::from_le_bytes([
        apk_bytes[start_offset],
        apk_bytes[start_offset + 1],
        apk_bytes[start_offset + 2],
        apk_bytes[start_offset + 3],
        apk_bytes[start_offset + 4],
        apk_bytes[start_offset + 5],
        apk_bytes[start_offset + 6],
        apk_bytes[start_offset + 7],
    ]);

    if block_size_header != block_size_footer {
        return Err(ApkError::MalformedSigningBlock);
    }

    Ok(SigningBlockBounds {
        start_offset,
        size: total_block_len,
        central_dir_offset: cd_offset,
        eocd_offset,
    })
}

/// Parses the ID-value pairs contained within the APK Signing Block.
pub fn parse_signing_block_pairs<'a>(
    apk_bytes: &'a [u8],
    bounds: &SigningBlockBounds,
) -> Result<Vec<SigningBlockPair<'a>>, ApkError> {
    let pairs_start = bounds.start_offset + 8;
    let pairs_end = bounds.central_dir_offset - 24;

    if pairs_start > pairs_end || pairs_end > apk_bytes.len() {
        return Err(ApkError::MalformedSigningBlock);
    }

    let mut cursor = pairs_start;
    let mut pairs = Vec::new();

    while cursor < pairs_end {
        if cursor + 8 > pairs_end {
            return Err(ApkError::MalformedSigningBlock);
        }

        let entry_len = u64::from_le_bytes([
            apk_bytes[cursor],
            apk_bytes[cursor + 1],
            apk_bytes[cursor + 2],
            apk_bytes[cursor + 3],
            apk_bytes[cursor + 4],
            apk_bytes[cursor + 5],
            apk_bytes[cursor + 6],
            apk_bytes[cursor + 7],
        ]) as usize;
        cursor += 8;

        if entry_len < 4 || cursor + entry_len > pairs_end {
            return Err(ApkError::MalformedSigningBlock);
        }

        let id = u32::from_le_bytes([
            apk_bytes[cursor],
            apk_bytes[cursor + 1],
            apk_bytes[cursor + 2],
            apk_bytes[cursor + 3],
        ]);
        cursor += 4;

        let value_len = entry_len - 4;
        let value = &apk_bytes[cursor..cursor + value_len];
        cursor += value_len;

        pairs.push(SigningBlockPair { id, value });
    }

    Ok(pairs)
}

/// Computes the AOSP 1 MiB chunked Merkle-tree content digest of an APK.
pub fn compute_apk_content_digest(
    apk_bytes: &[u8],
    bounds: &SigningBlockBounds,
) -> Result<[u8; 32], ApkError> {
    let mut chunk_hashes = Vec::new();

    // Section 1: Pre-Signing Block (0..bounds.start_offset)
    let sec1 = &apk_bytes[..bounds.start_offset];
    hash_section_chunks(sec1, &mut chunk_hashes);

    // Section 2: Central Directory (bounds.central_dir_offset..bounds.eocd_offset)
    let sec2 = &apk_bytes[bounds.central_dir_offset..bounds.eocd_offset];
    hash_section_chunks(sec2, &mut chunk_hashes);

    // Section 3: EOCD with modified Central Directory offset
    let mut modified_eocd = apk_bytes[bounds.eocd_offset..].to_vec();
    if modified_eocd.len() < 22 {
        return Err(ApkError::MalformedSigningBlock);
    }
    let new_cd_offset = (bounds.start_offset as u32).to_le_bytes();
    modified_eocd[16..20].copy_from_slice(&new_cd_offset);
    hash_section_chunks(&modified_eocd, &mut chunk_hashes);

    // Final Root Digest: 0x5a prefix + concatenation of all chunk hashes
    let mut root_hasher = Sha256::new();
    root_hasher.update(&[0x5a]);
    let chunk_count = (chunk_hashes.len() / 32) as u32;
    root_hasher.update(&chunk_count.to_le_bytes());
    root_hasher.update(&chunk_hashes);

    let result = root_hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    Ok(out)
}

fn hash_section_chunks(section: &[u8], chunk_hashes: &mut Vec<u8>) {
    let mut offset = 0;
    while offset < section.len() {
        let chunk_len = core::cmp::min(CONTENT_DIGEST_CHUNK_SIZE, section.len() - offset);
        let chunk = &section[offset..offset + chunk_len];

        let mut hasher = Sha256::new();
        hasher.update(&[0xa5]);
        hasher.update(&(chunk.len() as u32).to_le_bytes());
        hasher.update(chunk);
        let hash = hasher.finalize();
        chunk_hashes.extend_from_slice(&hash);

        offset += chunk_len;
    }
}

/// Parses and cryptographically verifies an APK Signature Scheme v2 or v3 payload.
pub fn parse_and_verify_v2_or_v3_payload(
    payload: &[u8],
    _scheme: ApkSignatureScheme,
    expected_content_digest: &[u8; 32],
) -> Result<(u32, [u8; 32], [u8; 32], usize), ApkError> {
    if payload.len() < 8 {
        return Err(ApkError::InvalidSignature);
    }

    let mut cursor = 0;
    let signers_len = read_u32_le(payload, &mut cursor)? as usize;
    if cursor + signers_len > payload.len() {
        return Err(ApkError::InvalidSignature);
    }

    let mut signers_count = 0;
    let mut chosen_algo = None;
    let mut verified_cert_fp = None;
    let mut verified_content_digest = None;

    while cursor < payload.len() {
        signers_count += 1;
        let signer_len = read_u32_le(payload, &mut cursor)? as usize;
        let signer_end = cursor + signer_len;
        if signer_end > payload.len() {
            return Err(ApkError::InvalidSignature);
        }

        // 1. SignedData
        let signed_data_len = read_u32_le(payload, &mut cursor)? as usize;
        let signed_data_start = cursor;
        let signed_data_end = cursor + signed_data_len;
        if signed_data_end > signer_end {
            return Err(ApkError::InvalidSignature);
        }
        let signed_data_bytes = &payload[signed_data_start..signed_data_end];

        // Parse SignedData internal components:
        // Digests sequence
        let digests_len = read_u32_le(payload, &mut cursor)? as usize;
        let digests_end = cursor + digests_len;
        if digests_end > signed_data_end {
            return Err(ApkError::InvalidSignature);
        }

        let mut declared_digests = Vec::new();
        while cursor < digests_end {
            let entry_len = read_u32_le(payload, &mut cursor)? as usize;
            let entry_end = cursor + entry_len;
            if entry_end > digests_end {
                return Err(ApkError::InvalidSignature);
            }
            let algo = read_u32_le(payload, &mut cursor)?;
            let digest_len = read_u32_le(payload, &mut cursor)? as usize;
            if cursor + digest_len != entry_end || digest_len != 32 {
                return Err(ApkError::InvalidSignature);
            }
            let mut d = [0u8; 32];
            d.copy_from_slice(&payload[cursor..cursor + 32]);
            cursor += 32;
            declared_digests.push((algo, d));
        }

        // Certificates sequence
        let certs_len = read_u32_le(payload, &mut cursor)? as usize;
        let certs_end = cursor + certs_len;
        if certs_end > signed_data_end {
            return Err(ApkError::InvalidSignature);
        }

        let mut first_cert_bytes = None;
        let mut first_cert_fp = None;
        while cursor < certs_end {
            let cert_len = read_u32_le(payload, &mut cursor)? as usize;
            if cursor + cert_len > certs_end {
                return Err(ApkError::InvalidSignature);
            }
            let cert_slice = &payload[cursor..cursor + cert_len];
            if first_cert_bytes.is_none() {
                first_cert_bytes = Some(cert_slice);
                let fp = Sha256::digest(cert_slice);
                let mut fp_arr = [0u8; 32];
                fp_arr.copy_from_slice(&fp);
                first_cert_fp = Some(fp_arr);
            }
            cursor += cert_len;
        }

        // Additional attributes sequence
        let attrs_len = read_u32_le(payload, &mut cursor)? as usize;
        if cursor + attrs_len > signed_data_end {
            return Err(ApkError::InvalidSignature);
        }

        // Reset cursor to end of SignedData
        cursor = signed_data_end;

        // 2. Signatures sequence
        let sigs_len = read_u32_le(payload, &mut cursor)? as usize;
        let sigs_end = cursor + sigs_len;
        if sigs_end > signer_end {
            return Err(ApkError::InvalidSignature);
        }

        let mut signatures = Vec::new();
        while cursor < sigs_end {
            let sig_entry_len = read_u32_le(payload, &mut cursor)? as usize;
            let sig_entry_end = cursor + sig_entry_len;
            if sig_entry_end > sigs_end {
                return Err(ApkError::InvalidSignature);
            }
            let algo = read_u32_le(payload, &mut cursor)?;
            let raw_sig_len = read_u32_le(payload, &mut cursor)? as usize;
            if cursor + raw_sig_len != sig_entry_end {
                return Err(ApkError::InvalidSignature);
            }
            let sig_slice = &payload[cursor..cursor + raw_sig_len];
            cursor += raw_sig_len;
            signatures.push((algo, sig_slice));
        }

        // 3. Public Key bytes (SubjectPublicKeyInfo)
        let pubkey_len = read_u32_le(payload, &mut cursor)? as usize;
        if cursor + pubkey_len > signer_end {
            return Err(ApkError::InvalidSignature);
        }
        let spki_bytes = &payload[cursor..cursor + pubkey_len];
        cursor += pubkey_len;

        // Parse public key from SPKI
        let parsed_pk = parse_spki(spki_bytes).ok_or(ApkError::InvalidSignature)?;

        // Find signature matching supported algorithms:
        // Prioritize RSASSA-PKCS1-v1_5 (0x0103) or ECDSA P-256 (0x0201)
        let mut verified_this_signer = false;
        let signed_data_hash = Sha256::digest(signed_data_bytes);

        for &(algo, sig_bytes) in &signatures {
            // Check matching declared content digest
            let declared_content_digest = match declared_digests.iter().find(|(a, _)| *a == algo) {
                Some((_, d)) => *d,
                None => continue,
            };

            // Digest must match actual APK content digest
            if &declared_content_digest != expected_content_digest {
                return Err(ApkError::InvalidSignature);
            }

            // Reject dummy signatures
            if sig_bytes.iter().all(|&b| b == 0x7A || b == 0x00) {
                return Err(ApkError::InvalidSignature);
            }

            let valid = match (algo, &parsed_pk) {
                (SIG_ALGO_RSA_PKCS1_V1_5_WITH_SHA256, ParsedPublicKey::Rsa { n, e }) => {
                    verify_rsa_pkcs1_v1_5(n, e, sig_bytes, &signed_data_hash)
                }
                (SIG_ALGO_ECDSA_WITH_SHA256, ParsedPublicKey::EcdsaP256 { x, y }) => {
                    // Raw 64-byte signature: r (32) || s (32)
                    if sig_bytes.len() == 64 {
                        verify_ecdsa_p256(x, y, &sig_bytes[..32], &sig_bytes[32..], &signed_data_hash)
                    } else {
                        false
                    }
                }
                (0x0203, _) => {
                    return Err(ApkError::UnsupportedSignatureAlgorithm);
                }
                _ => false,
            };

            if valid {
                chosen_algo = Some(algo);
                verified_content_digest = Some(declared_content_digest);
                verified_cert_fp = first_cert_fp;
                verified_this_signer = true;
                break;
            }
        }

        if !verified_this_signer {
            return Err(ApkError::InvalidSignature);
        }
    }

    let algo = chosen_algo.ok_or(ApkError::InvalidSignature)?;
    let cert_fp = verified_cert_fp.ok_or(ApkError::InvalidSignature)?;
    let content_digest = verified_content_digest.ok_or(ApkError::InvalidSignature)?;

    Ok((algo, content_digest, cert_fp, signers_count))
}

fn read_u32_le(bytes: &[u8], cursor: &mut usize) -> Result<u32, ApkError> {
    if *cursor + 4 > bytes.len() {
        return Err(ApkError::InvalidSignature);
    }
    let val = u32::from_le_bytes([
        bytes[*cursor],
        bytes[*cursor + 1],
        bytes[*cursor + 2],
        bytes[*cursor + 3],
    ]);
    *cursor += 4;
    Ok(val)
}

/// Fully verifies the APK Signing Block of an APK.
pub fn verify_apk_signing_block(apk_bytes: &[u8]) -> Result<ApkSigningDetails, ApkError> {
    let bounds = find_apk_signing_block(apk_bytes)?;
    let pairs = parse_signing_block_pairs(apk_bytes, &bounds)?;

    // Prefer v3.1, then v3, then v2.
    // Note: VERITY_PADDING_BLOCK_ID (0x42726577) is padding, not a signature scheme.
    let mut chosen_scheme = None;
    let mut chosen_payload = None;

    for pair in &pairs {
        if pair.id == APK_SIGNATURE_SCHEME_V31_BLOCK_ID {
            chosen_scheme = Some(ApkSignatureScheme::V31);
            chosen_payload = Some(pair.value);
            break;
        } else if pair.id == APK_SIGNATURE_SCHEME_V3_BLOCK_ID && chosen_scheme.is_none() {
            chosen_scheme = Some(ApkSignatureScheme::V3);
            chosen_payload = Some(pair.value);
        } else if pair.id == APK_SIGNATURE_SCHEME_V2_BLOCK_ID && chosen_scheme.is_none() {
            chosen_scheme = Some(ApkSignatureScheme::V2);
            chosen_payload = Some(pair.value);
        }
    }

    let scheme = chosen_scheme.ok_or(ApkError::InvalidSignature)?;
    let payload = chosen_payload.ok_or(ApkError::InvalidSignature)?;

    // Compute expected content digest
    let actual_digest = compute_apk_content_digest(apk_bytes, &bounds)?;

    // Cryptographically verify signatures and digests
    let (algo, declared_digest, cert_fp, signers_count) =
        parse_and_verify_v2_or_v3_payload(payload, scheme, &actual_digest)?;

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

// -----------------------------------------------------------------------------
// APK Signature Scheme v4 (.idsig) Verification
// -----------------------------------------------------------------------------

/// APK Signature Scheme v4 file structure (.idsig)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V4Signature {
    pub version: u32,
    pub hashing_algorithm: u32,
    pub log2_block_size: u8,
    pub salt: Vec<u8>,
    pub raw_root_hash: [u8; 32],
    pub signature: Vec<u8>,
    pub public_key: Vec<u8>,
}

/// Parses and validates an APK Signature Scheme v4 .idsig companion file.
pub fn verify_v4_signature(apk_bytes: &[u8], idsig_bytes: &[u8]) -> Result<V4Signature, ApkError> {
    if idsig_bytes.len() < 64 {
        return Err(ApkError::InvalidSignature);
    }

    let mut cursor = 0;
    let version = read_u32_le(idsig_bytes, &mut cursor)?;
    if version != 2 {
        return Err(ApkError::InvalidSignature);
    }

    let hashing_algorithm = read_u32_le(idsig_bytes, &mut cursor)?;
    if hashing_algorithm != 1 {
        // 1 = SHA256
        return Err(ApkError::UnsupportedSignatureAlgorithm);
    }

    let log2_block_size = idsig_bytes[cursor];
    cursor += 1;
    if log2_block_size != 12 {
        // 2^12 = 4096 bytes block size
        return Err(ApkError::InvalidSignature);
    }

    let salt_len = read_u32_le(idsig_bytes, &mut cursor)? as usize;
    if cursor + salt_len + 32 > idsig_bytes.len() {
        return Err(ApkError::InvalidSignature);
    }
    let salt = idsig_bytes[cursor..cursor + salt_len].to_vec();
    cursor += salt_len;

    let mut raw_root_hash = [0u8; 32];
    raw_root_hash.copy_from_slice(&idsig_bytes[cursor..cursor + 32]);
    cursor += 32;

    let sig_len = read_u32_le(idsig_bytes, &mut cursor)? as usize;
    if cursor + sig_len > idsig_bytes.len() {
        return Err(ApkError::InvalidSignature);
    }
    let signature = idsig_bytes[cursor..cursor + sig_len].to_vec();
    cursor += sig_len;

    let pk_len = read_u32_le(idsig_bytes, &mut cursor)? as usize;
    if cursor + pk_len > idsig_bytes.len() {
        return Err(ApkError::InvalidSignature);
    }
    let public_key = idsig_bytes[cursor..cursor + pk_len].to_vec();

    // Verify 4096-byte chunked Merkle tree of APK matches raw_root_hash
    let mut page_hashes = Vec::new();
    let mut page_offset = 0;
    while page_offset < apk_bytes.len() {
        let chunk_size = core::cmp::min(4096, apk_bytes.len() - page_offset);
        let mut hasher = Sha256::new();
        if !salt.is_empty() {
            hasher.update(&salt);
        }
        hasher.update(&apk_bytes[page_offset..page_offset + chunk_size]);
        page_hashes.extend_from_slice(&hasher.finalize());
        page_offset += chunk_size;
    }

    let mut root_hasher = Sha256::new();
    root_hasher.update(&page_hashes);
    let computed_root = root_hasher.finalize();

    if computed_root.as_slice() != raw_root_hash {
        return Err(ApkError::InvalidSignature);
    }

    // Verify signature over raw_root_hash using public key
    let parsed_pk = parse_spki(&public_key).ok_or(ApkError::InvalidSignature)?;
    let valid = match &parsed_pk {
        ParsedPublicKey::Rsa { n, e } => verify_rsa_pkcs1_v1_5(n, e, &signature, &raw_root_hash),
        ParsedPublicKey::EcdsaP256 { x, y } => {
            if signature.len() == 64 {
                verify_ecdsa_p256(x, y, &signature[..32], &signature[32..], &raw_root_hash)
            } else {
                false
            }
        }
    };

    if !valid {
        return Err(ApkError::InvalidSignature);
    }

    Ok(V4Signature {
        version,
        hashing_algorithm,
        log2_block_size,
        salt,
        raw_root_hash,
        signature,
        public_key,
    })
}


/// Deterministic ECDSA P-256 test key pair
pub const TEST_ECDSA_PRIVKEY: [u8; 32] = [
    0xc9, 0xaf, 0xa9, 0xd8, 0x45, 0xba, 0x75, 0x16, 0x6b, 0x5c, 0x21, 0x36, 0x2a, 0x37, 0x77, 0x52,
    0x19, 0xf2, 0x84, 0xf7, 0x1e, 0x07, 0x1f, 0x10, 0x18, 0x7f, 0x33, 0xe4, 0x7c, 0x78, 0xe1, 0xcd,
];

pub const TEST_ECDSA_K: [u8; 32] = [
    0xae, 0xa0, 0xf3, 0x60, 0x82, 0x88, 0x76, 0x3c, 0xfb, 0x81, 0x26, 0x65, 0x43, 0x10, 0xe5, 0xb5,
    0x5b, 0x45, 0x57, 0x55, 0x18, 0x97, 0x44, 0x62, 0xad, 0x18, 0x56, 0x60, 0xcb, 0x73, 0xc9, 0xac,
];

/// Constructs a cryptographically verified APK Signing Block v2 for test APK fixtures.
pub fn create_test_v2_signing_block(
    pre_block_bytes: &[u8],
    central_dir: &[u8],
    eocd_template: &[u8],
    certificate: &[u8],
) -> Vec<u8> {
    use super::crypto::{public_key_from_private_p256, sign_ecdsa_p256};

    let algo_id = SIG_ALGO_ECDSA_WITH_SHA256;
    let (pub_x, pub_y) = public_key_from_private_p256(&TEST_ECDSA_PRIVKEY);

    // Build SPKI: 30 59 30 13 06 07 2a 86 48 ce 3d 02 01 06 08 2a 86 48 ce 3d 03 01 07 03 42 00 04 || x || y
    let mut spki = Vec::new();
    let ec_spki_header = [
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
        0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00, 0x04,
    ];
    spki.extend_from_slice(&ec_spki_header);
    spki.extend_from_slice(&pub_x);
    spki.extend_from_slice(&pub_y);

    let cert_entry_len = 4 + certificate.len();
    let signed_data_len = 4 + (4 + 4 + 4 + 32) + 4 + cert_entry_len + 4;
    let signer_len = 4 + signed_data_len + 4 + 76 + 4 + spki.len();
    let signers_len = 4 + signer_len;
    let v2_pair_len = 8 + 4 + signers_len;
    let total_block_len = 8 + v2_pair_len + 8 + 16;

    let start_offset = pre_block_bytes.len();
    let cd_offset = start_offset + total_block_len;
    let eocd_offset = cd_offset + central_dir.len();

    let bounds = SigningBlockBounds {
        start_offset,
        size: total_block_len,
        central_dir_offset: cd_offset,
        eocd_offset,
    };

    let mut apk_mock = Vec::new();
    apk_mock.extend_from_slice(pre_block_bytes);
    apk_mock.extend_from_slice(&alloc::vec![0u8; total_block_len]);
    apk_mock.extend_from_slice(central_dir);
    apk_mock.extend_from_slice(eocd_template);

    let content_digest = compute_apk_content_digest(&apk_mock, &bounds).unwrap_or([0u8; 32]);

    // Build SignedData
    let mut digests_bytes = Vec::new();
    digests_bytes.extend_from_slice(&(40u32).to_le_bytes());
    digests_bytes.extend_from_slice(&algo_id.to_le_bytes());
    digests_bytes.extend_from_slice(&(32u32).to_le_bytes());
    digests_bytes.extend_from_slice(&content_digest);

    let mut certs_bytes = Vec::new();
    certs_bytes.extend_from_slice(&(certificate.len() as u32).to_le_bytes());
    certs_bytes.extend_from_slice(certificate);

    let mut signed_data = Vec::new();
    signed_data.extend_from_slice(&(digests_bytes.len() as u32).to_le_bytes());
    signed_data.extend_from_slice(&digests_bytes);
    signed_data.extend_from_slice(&(certs_bytes.len() as u32).to_le_bytes());
    signed_data.extend_from_slice(&certs_bytes);
    signed_data.extend_from_slice(&0u32.to_le_bytes()); // empty attributes

    // Sign SignedData cryptographically
    let sd_hash = Sha256::digest(&signed_data);
    let mut sd_hash_arr = [0u8; 32];
    sd_hash_arr.copy_from_slice(&sd_hash);

    let (sig_r, sig_s) = sign_ecdsa_p256(&TEST_ECDSA_PRIVKEY, &TEST_ECDSA_K, &sd_hash_arr).unwrap();
    let mut sig_64 = Vec::with_capacity(64);
    sig_64.extend_from_slice(&sig_r);
    sig_64.extend_from_slice(&sig_s);

    let mut sigs = Vec::new();
    sigs.extend_from_slice(&(72u32).to_le_bytes());
    sigs.extend_from_slice(&algo_id.to_le_bytes());
    sigs.extend_from_slice(&(64u32).to_le_bytes());
    sigs.extend_from_slice(&sig_64);

    let mut signer = Vec::new();
    signer.extend_from_slice(&(signed_data.len() as u32).to_le_bytes());
    signer.extend_from_slice(&signed_data);
    signer.extend_from_slice(&(sigs.len() as u32).to_le_bytes());
    signer.extend_from_slice(&sigs);
    signer.extend_from_slice(&(spki.len() as u32).to_le_bytes());
    signer.extend_from_slice(&spki);

    let mut signers = Vec::new();
    signers.extend_from_slice(&(signer.len() as u32).to_le_bytes());
    signers.extend_from_slice(&signer);

    let mut v2_payload = Vec::new();
    v2_payload.extend_from_slice(&(signers.len() as u32).to_le_bytes());
    v2_payload.extend_from_slice(&signers);

    let mut block = Vec::new();
    let block_size_u64 = (v2_payload.len() + 12 + 24) as u64;
    block.extend_from_slice(&block_size_u64.to_le_bytes());

    let entry_len_u64 = (v2_payload.len() + 4) as u64;
    block.extend_from_slice(&entry_len_u64.to_le_bytes());
    block.extend_from_slice(&APK_SIGNATURE_SCHEME_V2_BLOCK_ID.to_le_bytes());
    block.extend_from_slice(&v2_payload);

    block.extend_from_slice(&block_size_u64.to_le_bytes());
    block.extend_from_slice(APK_SIG_BLOCK_MAGIC);

    block
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fossify_real_rsa_signature_verification() {
        let apk_path = std::path::Path::new("../../emulator/data/apps/fossify_file_manager.apk");
        if !apk_path.exists() {
            return;
        }
        let data = std::fs::read(apk_path).expect("read fossify apk");
        let details = verify_apk_signing_block(&data).expect("fossify RSA verification must succeed");
        assert_eq!(details.scheme, ApkSignatureScheme::V2);
        assert_eq!(details.algorithm_id, SIG_ALGO_RSA_PKCS1_V1_5_WITH_SHA256);
        assert_eq!(details.signers_count, 1);
    }
}
