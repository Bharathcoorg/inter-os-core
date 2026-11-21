// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Pure `#![no_std]` parsers for Android APK, Signing Block, AXML, DEX, ELF, and ARSC formats.

#![no_std]
#[cfg(test)]
extern crate std;
extern crate alloc;

pub mod crypto;
pub mod signing;
pub mod apk;
pub mod manifest;
pub mod xml;
pub mod resources;
pub mod dex;
pub mod elf;
pub mod inflate;

pub use signing::{
    APK_SIG_BLOCK_MAGIC,
    APK_SIGNATURE_SCHEME_V2_BLOCK_ID,
    APK_SIGNATURE_SCHEME_V3_BLOCK_ID,
    APK_SIGNATURE_SCHEME_V31_BLOCK_ID,
    VERITY_PADDING_BLOCK_ID,
    CONTENT_DIGEST_CHUNK_SIZE,
    SIG_ALGO_RSA_PSS_WITH_SHA256,
    SIG_ALGO_RSA_PSS_WITH_SHA512,
    SIG_ALGO_RSA_PKCS1_V1_5_WITH_SHA256,
    SIG_ALGO_RSA_PKCS1_V1_5_WITH_SHA512,
    SIG_ALGO_ECDSA_WITH_SHA256,
    SIG_ALGO_ECDSA_WITH_SHA512,
    SIG_ALGO_DSA_WITH_SHA256,
    ApkSignatureScheme,
    ApkSigningDetails,
    SigningBlockBounds,
    SigningBlockPair,
    V4Signature,
    find_apk_signing_block,
    parse_signing_block_pairs,
    compute_apk_content_digest,
    verify_apk_signing_block,
    verify_v4_signature,
};

pub use apk::{
    AcrAuditStatus, AcrContainerProfile, ApkError, ApkPackage, ZipArchive, ZipFileEntry,
    create_test_apk, create_signed_test_apk,
};

pub use manifest::{
    AndroidManifest, Capability, CapabilityId, CapabilityTable, CapabilityType, Rights,
};
pub use xml::{AxmlDocument, AxmlElement, AxmlError};
pub use resources::{ResourceError, ResourceTable, ResourceValue};
pub use dex::{DexError, DexMethod, DexPackage, compute_adler32, create_test_dex};
pub use elf::{ElfError, LoadedElf, LoadedSegment, MemoryProtection};
pub use inflate::InflateError;
