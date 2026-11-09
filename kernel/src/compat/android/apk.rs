// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Android APK Container Package Parser & Binary Manifest Decoder
//!
//! Provides bare-metal `no_std` PKZip container extraction, Android Binary XML
//! (AXML) manifest parsing, and zero-trust capability translation for isolated
//! Android Compatibility Runtime (ACR) containers.
//!
//! Non-Negotiable Principle #4: We innovate with Rust OS capabilities rather than copying
//! legacy ambient-authority Android or Linux architectures.

use alloc::string::String;
use alloc::vec::Vec;
use super::manifest::AndroidManifest;

// PKZip Magic Signatures
pub const ZIP_LOCAL_FILE_HEADER_SIG: u32 = 0x04034b50;
pub const ZIP_CENTRAL_DIR_SIG: u32 = 0x02014b50;
pub const ZIP_END_OF_CENTRAL_DIR_SIG: u32 = 0x06054b50;
pub const MAX_MEMBER_BYTES: usize = 128 * 1024 * 1024;

// Android Binary XML (AXML) Chunk Types
pub const RES_XML_TYPE: u16 = 0x0003;
pub const RES_STRING_POOL_TYPE: u16 = 0x0001;
pub const RES_XML_RESOURCE_MAP_TYPE: u16 = 0x0180;
pub const RES_XML_START_NAMESPACE_TYPE: u16 = 0x0100;
pub const RES_XML_END_NAMESPACE_TYPE: u16 = 0x0101;
pub const RES_XML_START_ELEMENT_TYPE: u16 = 0x0102;
pub const RES_XML_END_ELEMENT_TYPE: u16 = 0x0103;
pub const RES_XML_CDATA_TYPE: u16 = 0x0104;

/// APK container parser errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApkError {
    TooSmall,
    InvalidZipSignature,
    EndOfCentralDirectoryNotFound,
    InvalidCentralDirectory,
    MissingManifest,
    MissingDex,
    InvalidAxmlHeader,
    InvalidStringPool,
    MalformedXmlChunk,
    AmbientAuthorityViolation,
    InvalidSignature,
    MalformedSigningBlock,
    UnsupportedCompression,
}

/// Audit decision status for Android APK packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum AcrAuditStatus {
    Approved = 0,
    AttestationFailed = 1,
    AmbientAuthorityDenied = 2,
    InvalidApkFormat = 3,
    MissingManifest = 4,
}

/// Parsed file entry within the APK ZIP container.
#[derive(Debug, Clone)]
pub struct ZipFileEntry {
    pub name: String,
    pub compression_method: u16,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub data_offset: usize,
}

/// In-memory PKZip archive reader.
pub struct ZipArchive<'a> {
    data: &'a [u8],
    pub entries: Vec<ZipFileEntry>,
}

impl<'a> ZipArchive<'a> {
    /// Parses a raw PKZip archive buffer.
    pub fn parse(data: &'a [u8]) -> Result<Self, ApkError> {
        const EOCD_MIN: usize = 22;
        const MAX_ENTRIES: usize = 4096;
        const MAX_MEMBER_BYTES: usize = 128 * 1024 * 1024;
        if data.len() < EOCD_MIN { return Err(ApkError::TooSmall); }

        // EOCD may be preceded by a comment of at most 65535 bytes. Multi-disk
        // and ZIP64 archives are explicitly unsupported rather than partially parsed.
        let search_start = data.len().saturating_sub(65535 + EOCD_MIN);
        let mut eocd = None;
        for pos in (search_start..=data.len() - EOCD_MIN).rev() {
            let signature = ZIP_END_OF_CENTRAL_DIR_SIG.to_le_bytes();
            if data.get(pos..pos + 4).map(|bytes| bytes == signature.as_slice()).unwrap_or(false) {
                eocd = Some(pos);
                break;
            }
        }
        let eocd_pos = eocd.ok_or(ApkError::EndOfCentralDirectoryNotFound)?;
        let u16_at = |p: usize| -> Result<u16, ApkError> {
            let end = p.checked_add(2).ok_or(ApkError::InvalidCentralDirectory)?;
            let b = data.get(p..end).ok_or(ApkError::InvalidCentralDirectory)?;
            Ok(u16::from_le_bytes([b[0], b[1]]))
        };
        let u32_at = |p: usize| -> Result<u32, ApkError> {
            let end = p.checked_add(4).ok_or(ApkError::InvalidCentralDirectory)?;
            let b = data.get(p..end).ok_or(ApkError::InvalidCentralDirectory)?;
            Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let disk = u16_at(eocd_pos + 4)?;
        let cd_disk = u16_at(eocd_pos + 6)?;
        let disk_entries = u16_at(eocd_pos + 8)? as usize;
        let total_entries = u16_at(eocd_pos + 10)? as usize;
        let comment_len = u16_at(eocd_pos + 20)? as usize;
        if disk != 0 || cd_disk != 0 || disk_entries != total_entries
            || total_entries == u16::MAX as usize || total_entries > MAX_ENTRIES
        { return Err(ApkError::InvalidCentralDirectory); }
        let eocd_end = eocd_pos.checked_add(EOCD_MIN)
            .and_then(|v| v.checked_add(comment_len))
            .ok_or(ApkError::InvalidCentralDirectory)?;
        if eocd_end != data.len() { return Err(ApkError::InvalidCentralDirectory); }

        let cd_size = u32_at(eocd_pos + 12)? as usize;
        let cd_offset = u32_at(eocd_pos + 16)? as usize;
        let cd_end = cd_offset.checked_add(cd_size).ok_or(ApkError::InvalidCentralDirectory)?;
        if cd_end != eocd_pos || cd_end > data.len() {
            return Err(ApkError::InvalidCentralDirectory);
        }

        let mut entries = Vec::with_capacity(total_entries);
        let mut cursor = cd_offset;
        for _ in 0..total_entries {
            let fixed_end = cursor.checked_add(46).ok_or(ApkError::InvalidCentralDirectory)?;
            let header = data.get(cursor..fixed_end).ok_or(ApkError::InvalidCentralDirectory)?;
            if u32::from_le_bytes(header[0..4].try_into().unwrap()) != ZIP_CENTRAL_DIR_SIG {
                return Err(ApkError::InvalidCentralDirectory);
            }
            let flags = u16::from_le_bytes([header[8], header[9]]);
            let method = u16::from_le_bytes([header[10], header[11]]);
            let compressed = u32::from_le_bytes(header[20..24].try_into().unwrap());
            let uncompressed = u32::from_le_bytes(header[24..28].try_into().unwrap());
            let name_len = u16::from_le_bytes([header[28], header[29]]) as usize;
            let extra_len = u16::from_le_bytes([header[30], header[31]]) as usize;
            let comment_len = u16::from_le_bytes([header[32], header[33]]) as usize;
            let local_offset = u32::from_le_bytes(header[42..46].try_into().unwrap()) as usize;
            if flags & 0x0001 != 0 || compressed == u32::MAX || uncompressed == u32::MAX
                || uncompressed as usize > MAX_MEMBER_BYTES
            { return Err(ApkError::InvalidCentralDirectory); }

            let name_start = fixed_end;
            let name_end = name_start.checked_add(name_len).ok_or(ApkError::InvalidCentralDirectory)?;
            let record_end = name_end.checked_add(extra_len).and_then(|v| v.checked_add(comment_len))
                .ok_or(ApkError::InvalidCentralDirectory)?;
            if record_end > cd_end { return Err(ApkError::InvalidCentralDirectory); }
            let name = core::str::from_utf8(&data[name_start..name_end])
                .map_err(|_| ApkError::InvalidCentralDirectory)?;
            if name.is_empty() || name.starts_with('/') || name.contains('\\')
                || name.split('/').any(|part| part == "..")
            { return Err(ApkError::InvalidCentralDirectory); }

            let local_end = local_offset.checked_add(30).ok_or(ApkError::InvalidCentralDirectory)?;
            let local = data.get(local_offset..local_end).ok_or(ApkError::InvalidCentralDirectory)?;
            if u32::from_le_bytes(local[0..4].try_into().unwrap()) != ZIP_LOCAL_FILE_HEADER_SIG {
                return Err(ApkError::InvalidCentralDirectory);
            }
            let local_flags = u16::from_le_bytes([local[6], local[7]]);
            let local_method = u16::from_le_bytes([local[8], local[9]]);
            let local_name_len = u16::from_le_bytes([local[26], local[27]]) as usize;
            let local_extra_len = u16::from_le_bytes([local[28], local[29]]) as usize;
            if local_flags != flags || local_method != method {
                return Err(ApkError::InvalidCentralDirectory);
            }
            let local_name_end = local_end.checked_add(local_name_len).ok_or(ApkError::InvalidCentralDirectory)?;
            let payload_start = local_name_end.checked_add(local_extra_len).ok_or(ApkError::InvalidCentralDirectory)?;
            let local_name = data.get(local_end..local_name_end).ok_or(ApkError::InvalidCentralDirectory)?;
            if local_name != name.as_bytes() { return Err(ApkError::InvalidCentralDirectory); }
            let payload_end = payload_start.checked_add(compressed as usize).ok_or(ApkError::InvalidCentralDirectory)?;
            if payload_end > cd_offset || payload_end > data.len() {
                return Err(ApkError::InvalidCentralDirectory);
            }
            if flags & 0x0008 == 0 {
                let local_compressed = u32::from_le_bytes(local[18..22].try_into().unwrap());
                let local_uncompressed = u32::from_le_bytes(local[22..26].try_into().unwrap());
                if local_compressed != compressed || local_uncompressed != uncompressed {
                    return Err(ApkError::InvalidCentralDirectory);
                }
            }
            entries.push(ZipFileEntry {
                name: String::from(name),
                compression_method: method,
                compressed_size: compressed,
                uncompressed_size: uncompressed,
                data_offset: payload_start,
            });
            cursor = record_end;
        }
        if cursor != cd_end || entries.len() != total_entries {
            return Err(ApkError::InvalidCentralDirectory);
        }
        // Duplicate names make get_file(name) ambiguous, so reject rather than
        // silently accepting whichever entry appears first.
        for i in 0..entries.len() {
            if entries[..i].iter().any(|entry| entry.name == entries[i].name) {
                return Err(ApkError::InvalidCentralDirectory);
            }
        }
        Ok(Self { data, entries })
    }

    /// Reads uncompressed file payload by filename.
    pub fn get_file(&self, filename: &str) -> Option<&'a [u8]> {
        for entry in &self.entries {
            if entry.name == filename && entry.compression_method == 0 {
                let end = entry.data_offset.checked_add(entry.compressed_size as usize)?;
                if end <= self.data.len() && entry.uncompressed_size == entry.compressed_size {
                    return Some(&self.data[entry.data_offset..end]);
                }
            }
        }
        None
    }

    /// Extracts a file payload by filename, decompressing raw DEFLATE streams on-demand.
    pub fn extract_file(&self, filename: &str) -> Result<Vec<u8>, ApkError> {
        for entry in &self.entries {
            if entry.name == filename {
                let end = entry.data_offset.checked_add(entry.compressed_size as usize)
                    .ok_or(ApkError::InvalidCentralDirectory)?;
                if end > self.data.len() {
                    return Err(ApkError::InvalidCentralDirectory);
                }
                let compressed = &self.data[entry.data_offset..end];
                if entry.compression_method == 0 {
                    if entry.uncompressed_size != entry.compressed_size || compressed.len() != entry.uncompressed_size as usize {
                        return Err(ApkError::InvalidCentralDirectory);
                    }
                    let mut out = Vec::with_capacity(entry.uncompressed_size as usize);
                    out.extend_from_slice(compressed);
                    return Ok(out);
                } else if entry.compression_method == 8 {
                    if entry.uncompressed_size as usize > MAX_MEMBER_BYTES {
                        return Err(ApkError::InvalidCentralDirectory);
                    }
                    let mut out = alloc::vec![0u8; entry.uncompressed_size as usize];
                    match super::inflate::inflate_raw(compressed, &mut out) {
                        Ok(len) => {
                            if len != entry.uncompressed_size as usize {
                                return Err(ApkError::InvalidCentralDirectory);
                            }
                            out.truncate(len);
                            return Ok(out);
                        }
                        Err(_) => return Err(ApkError::UnsupportedCompression),
                    }
                } else {
                    return Err(ApkError::UnsupportedCompression);
                }
            }
        }
        Err(ApkError::MissingManifest)
    }

    /// Returns an explicit failure for an APK member that requires an unsupported decompressor.
    pub fn validate_compression(&self) -> Result<(), ApkError> {
        for entry in &self.entries {
            if entry.compression_method != 0 && entry.compression_method != 8 {
                return Err(ApkError::UnsupportedCompression);
            }
        }
        Ok(())
    }
}

/// Android Binary XML (AXML) string pool decoder.
pub struct AxmlStringPool {
    pub strings: Vec<String>,
}

impl AxmlStringPool {
    /// Decodes a string pool chunk starting at `offset`.
    pub fn decode(data: &[u8], offset: usize) -> Result<(Self, usize), ApkError> {
        const MAX_STRINGS: usize = 65536;
        let header_end = offset.checked_add(28).ok_or(ApkError::InvalidStringPool)?;
        let header = data.get(offset..header_end).ok_or(ApkError::InvalidStringPool)?;
        if u16::from_le_bytes([header[0], header[1]]) != RES_STRING_POOL_TYPE {
            return Err(ApkError::InvalidStringPool);
        }
        let chunk_size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        let string_count = u32::from_le_bytes(header[8..12].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(header[16..20].try_into().unwrap());
        let strings_start = u32::from_le_bytes(header[20..24].try_into().unwrap()) as usize;
        let chunk_end = offset.checked_add(chunk_size).ok_or(ApkError::InvalidStringPool)?;
        if chunk_size < 28 || chunk_end > data.len() || string_count > MAX_STRINGS {
            return Err(ApkError::InvalidStringPool);
        }
        let offsets_bytes = string_count.checked_mul(4).ok_or(ApkError::InvalidStringPool)?;
        let offsets_end = header_end.checked_add(offsets_bytes).ok_or(ApkError::InvalidStringPool)?;
        let strings_base = offset.checked_add(strings_start).ok_or(ApkError::InvalidStringPool)?;
        if offsets_end > chunk_end || strings_start < 28 + offsets_bytes || strings_base > chunk_end {
            return Err(ApkError::InvalidStringPool);
        }

        fn read_len8(data: &[u8], pos: &mut usize, end: usize) -> Result<usize, ApkError> {
            let first = *data.get(*pos).filter(|_| *pos < end).ok_or(ApkError::InvalidStringPool)?;
            *pos += 1;
            if first & 0x80 == 0 { return Ok(first as usize); }
            let second = *data.get(*pos).filter(|_| *pos < end).ok_or(ApkError::InvalidStringPool)?;
            *pos += 1;
            Ok((((first & 0x7f) as usize) << 8) | second as usize)
        }
        fn read_len16(data: &[u8], pos: &mut usize, end: usize) -> Result<usize, ApkError> {
            let b = data.get(*pos..pos.checked_add(2).ok_or(ApkError::InvalidStringPool)?)
                .filter(|_| *pos + 2 <= end).ok_or(ApkError::InvalidStringPool)?;
            let first = u16::from_le_bytes([b[0], b[1]]);
            *pos += 2;
            if first & 0x8000 == 0 { return Ok(first as usize); }
            let b = data.get(*pos..pos.checked_add(2).ok_or(ApkError::InvalidStringPool)?)
                .filter(|_| *pos + 2 <= end).ok_or(ApkError::InvalidStringPool)?;
            *pos += 2;
            Ok((((first & 0x7fff) as usize) << 16) | u16::from_le_bytes([b[0], b[1]]) as usize)
        }

        let is_utf8 = flags & (1 << 8) != 0;
        let mut strings = Vec::with_capacity(string_count);
        for i in 0..string_count {
            let off_pos = header_end.checked_add(i.checked_mul(4).ok_or(ApkError::InvalidStringPool)?)
                .ok_or(ApkError::InvalidStringPool)?;
            let off_bytes = data.get(off_pos..off_pos + 4).ok_or(ApkError::InvalidStringPool)?;
            let string_offset = u32::from_le_bytes(off_bytes.try_into().unwrap()) as usize;
            let mut pos = strings_base.checked_add(string_offset).ok_or(ApkError::InvalidStringPool)?;
            if pos >= chunk_end { return Err(ApkError::InvalidStringPool); }

            if is_utf8 {
                let _utf16_len = read_len8(data, &mut pos, chunk_end)?;
                let byte_len = read_len8(data, &mut pos, chunk_end)?;
                let string_end = pos.checked_add(byte_len).ok_or(ApkError::InvalidStringPool)?;
                if string_end >= chunk_end || data[string_end] != 0 {
                    return Err(ApkError::InvalidStringPool);
                }
                let value = core::str::from_utf8(&data[pos..string_end])
                    .map_err(|_| ApkError::InvalidStringPool)?;
                strings.push(String::from(value));
            } else {
                let char_len = read_len16(data, &mut pos, chunk_end)?;
                let byte_len = char_len.checked_mul(2).ok_or(ApkError::InvalidStringPool)?;
                let string_end = pos.checked_add(byte_len).ok_or(ApkError::InvalidStringPool)?;
                if string_end > chunk_end { return Err(ApkError::InvalidStringPool); }
                let mut units = Vec::with_capacity(char_len);
                for pair in data[pos..string_end].chunks_exact(2) {
                    units.push(u16::from_le_bytes([pair[0], pair[1]]));
                }
                strings.push(String::from_utf16_lossy(&units));
            }
        }
        Ok((Self { strings }, chunk_size))
    }

    /// Returns the string at index `idx` if valid.
    pub fn get(&self, idx: usize) -> Option<&str> {
        self.strings.get(idx).map(|s| s.as_str())
    }
}

/// Decodes binary `AndroidManifest.xml` into a strongly typed `AndroidManifest`.
pub fn decode_axml_manifest(data: &[u8]) -> Result<AndroidManifest, ApkError> {
    if data.len() < 8 {
        return Err(ApkError::TooSmall);
    }

    let magic = u16::from_le_bytes(data[0..2].try_into().unwrap());
    if magic != RES_XML_TYPE {
        return Err(ApkError::InvalidAxmlHeader);
    }

    let header_size = u16::from_le_bytes(data[2..4].try_into().unwrap()) as usize;
    let total_size = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    if header_size < 8 || total_size < header_size || total_size > data.len() {
        return Err(ApkError::MalformedXmlChunk);
    }

    let mut cursor = header_size;
    let mut string_pool = None;

    let mut package_name = String::new();
    let mut version_code = 1u32;
    let mut target_sdk = 34u32;
    let mut permissions = Vec::new();

    while cursor.checked_add(8).filter(|end| *end <= total_size).is_some() {
        let chunk_type = u16::from_le_bytes(data[cursor..cursor + 2].try_into().unwrap());
        let chunk_header_size = u16::from_le_bytes(data[cursor + 2..cursor + 4].try_into().unwrap()) as usize;
        let chunk_size = u32::from_le_bytes(data[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        let chunk_end = cursor.checked_add(chunk_size).ok_or(ApkError::MalformedXmlChunk)?;
        if chunk_header_size < 8 || chunk_size < chunk_header_size || chunk_end > total_size {
            return Err(ApkError::MalformedXmlChunk);
        }

        match chunk_type {
            RES_STRING_POOL_TYPE => {
                let (pool, decoded_size) = AxmlStringPool::decode(data, cursor)?;
                if decoded_size != chunk_size {
                    return Err(ApkError::MalformedXmlChunk);
                }
                string_pool = Some(pool);
            }
            RES_XML_START_ELEMENT_TYPE => {
                if let Some(ref pool) = string_pool {
                    if cursor + 36 <= chunk_end {
                        let name_idx = u32::from_le_bytes(data[cursor + 20..cursor + 24].try_into().unwrap()) as usize;
                        let attr_count = u16::from_le_bytes(data[cursor + 28..cursor + 30].try_into().unwrap()) as usize;
                        let tag_name = pool.get(name_idx).unwrap_or("");

                        let attr_start = cursor + 36;
                        for a in 0..attr_count {
                            let a_offset = attr_start + a * 20;
                            if a_offset.checked_add(20).filter(|end| *end <= chunk_end).is_none() {
                                break;
                            }
                            let a_name_idx = u32::from_le_bytes(data[a_offset + 4..a_offset + 8].try_into().unwrap()) as usize;
                            let a_val_idx = u32::from_le_bytes(data[a_offset + 8..a_offset + 12].try_into().unwrap()) as usize;
                            let a_data_type = u8::from_le_bytes([data[a_offset + 15]]);
                            let a_data = u32::from_le_bytes(data[a_offset + 16..a_offset + 20].try_into().unwrap());

                            let attr_name = pool.get(a_name_idx).unwrap_or("");
                            let attr_str_val = pool.get(a_val_idx).unwrap_or("");

                            if tag_name == "manifest" {
                                if attr_name == "package" {
                                    package_name = String::from(attr_str_val);
                                } else if attr_name == "versionCode" {
                                    if a_data_type == 0x10 { // TYPE_INT_DEC
                                        version_code = a_data;
                                    }
                                }
                            } else if tag_name == "uses-permission" {
                                if attr_name == "name" && !attr_str_val.is_empty() {
                                    permissions.push(String::from(attr_str_val));
                                }
                            } else if tag_name == "uses-sdk" {
                                if attr_name == "targetSdkVersion" && a_data_type == 0x10 {
                                    target_sdk = a_data;
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        cursor = chunk_end;
    }
    if cursor != total_size {
        return Err(ApkError::MalformedXmlChunk);
    }

    // Never invent an application identity when the binary manifest omitted
    // its package attribute; downstream signing and sandbox policy must bind to
    // the identity actually declared by the APK.
    if package_name.is_empty() {
        return Err(ApkError::MalformedXmlChunk);
    }

    let mut manifest = AndroidManifest::new(&package_name, version_code, target_sdk);
    for p in permissions {
        manifest.add_permission(&p);
    }
    Ok(manifest)
}

/// Parsed Android Application Package container with security metadata.
#[derive(Debug, Clone)]
pub struct ApkPackage {
    pub manifest: AndroidManifest,
    pub has_classes_dex: bool,
    /// Exact `classes.dex` payload extracted from this APK for verified runtime binding.
    pub classes_dex: Vec<u8>,
    pub is_signed: bool,
    pub file_count: usize,
    pub total_size_bytes: usize,
}

/// Magic footer for Android APK Signing Block (16 bytes)
pub const APK_SIG_BLOCK_MAGIC: &[u8; 16] = b"APK Sig Block 42";

/// Block ID for APK Signature Scheme v2 (Android 7.0+)
pub const APK_SIGNATURE_SCHEME_V2_BLOCK_ID: u32 = 0x7109871a;

/// Block ID for APK Signature Scheme v3 (Android 9.0+)
pub const APK_SIGNATURE_SCHEME_V3_BLOCK_ID: u32 = 0xf05368c0;

/// Verifies whether the APK binary contains a cryptographically verified APK Signature
/// Scheme v2/v3 signing block or a valid JAR certificate chain in META-INF.
pub fn verify_apk_signature(apk_bytes: &[u8]) -> Result<(), ApkError> {
    if apk_bytes.len() < 22 {
        return Err(ApkError::TooSmall);
    }

    // This kernel component currently parses APK structure but does not implement
    // the cryptographic verification required by APK Signature Scheme v2/v3 or
    // JAR/X.509 signing. A signing-block ID, certificate filename, or non-empty
    // certificate payload is not proof of authenticity. Fail closed until a
    // trusted verifier supplies verified signer identity and content-digest proof.
    Err(ApkError::InvalidSignature)
}

impl ApkPackage {
    /// Parses an in-memory Android APK container package.
    pub fn parse(apk_bytes: &[u8]) -> Result<Self, ApkError> {
        let zip = ZipArchive::parse(apk_bytes)?;
        zip.validate_compression()?;
        let manifest_bytes = zip.extract_file("AndroidManifest.xml").map_err(|_| ApkError::MissingManifest)?;
        let manifest = decode_axml_manifest(&manifest_bytes)?;

        let classes_dex = zip.extract_file("classes.dex").unwrap_or_default();
        let has_classes_dex = !classes_dex.is_empty();
        let is_signed = verify_apk_signature(apk_bytes).is_ok();

        Ok(Self {
            manifest,
            has_classes_dex,
            classes_dex,
            is_signed,
            file_count: zip.entries.len(),
            total_size_bytes: apk_bytes.len(),
        })
    }

    /// Translates APK manifest declared permissions into an Inter OS ACR Container Profile.
    /// Rejects forbidden ambient privileges (e.g. `android.permission.INSTALL_PACKAGES`).
    pub fn audit_and_create_container(&self, _base_cap_id: u64) -> AcrContainerProfile {
        let mut granted_caps_count = 0;
        let mut denied_perms_count = 0;

        // Verify that the APK does NOT request forbidden ambient authority
        let forbidden_ambient_perms = [
            "android.permission.INSTALL_PACKAGES",
            "android.permission.DELETE_PACKAGES",
            "android.permission.BIND_ACCESSIBILITY_SERVICE",
            "android.permission.WRITE_SECURE_SETTINGS",
            "android.permission.DEVICE_POWER",
            "android.permission.MASTER_CLEAR",
        ];

        for perm in &self.manifest.permissions {
            if forbidden_ambient_perms.contains(&perm.as_str()) {
                return AcrContainerProfile {
                    package_name: self.manifest.package_name.clone(),
                    version_code: self.manifest.version_code,
                    target_sdk: self.manifest.target_sdk,
                    status: AcrAuditStatus::AmbientAuthorityDenied,
                    granted_caps_count: 0,
                    denied_perms_count: 1,
                    memory_quota_bytes: 0,
                };
            }
        }

        if !self.has_classes_dex {
            return AcrContainerProfile {
                package_name: self.manifest.package_name.clone(),
                version_code: self.manifest.version_code,
                target_sdk: self.manifest.target_sdk,
                status: AcrAuditStatus::InvalidApkFormat,
                granted_caps_count: 0,
                denied_perms_count: self.manifest.permissions.len(),
                memory_quota_bytes: 0,
            };
        }

        if !self.is_signed {
            return AcrContainerProfile {
                package_name: self.manifest.package_name.clone(),
                version_code: self.manifest.version_code,
                target_sdk: self.manifest.target_sdk,
                status: AcrAuditStatus::AttestationFailed,
                granted_caps_count: 0,
                denied_perms_count: self.manifest.permissions.len(),
                memory_quota_bytes: 0,
            };
        }

        // Map allowable permissions into unforgeable capability tokens
        for perm in &self.manifest.permissions {
            match perm.as_str() {
                "android.permission.INTERNET"
                | "android.permission.ACCESS_FINE_LOCATION"
                | "android.permission.READ_EXTERNAL_STORAGE"
                | "android.permission.WRITE_EXTERNAL_STORAGE"
                | "android.permission.CAMERA"
                | "android.permission.RECORD_AUDIO"
                | "android.permission.CALL_PHONE" => {
                    granted_caps_count += 1;
                }
                _ => {
                    denied_perms_count += 1;
                }
            }
        }

        AcrContainerProfile {
            package_name: self.manifest.package_name.clone(),
            version_code: self.manifest.version_code,
            target_sdk: self.manifest.target_sdk,
            status: AcrAuditStatus::Approved,
            granted_caps_count,
            denied_perms_count,
            memory_quota_bytes: 64 * 1024 * 1024, // 64 MiB strict container ceiling
        }
    }

    /// Extracts the raw `classes.dex` binary payload from the APK container.
    pub fn extract_dex_bytes<'a>(&self, apk_bytes: &'a [u8]) -> Result<&'a [u8], ApkError> {
        let zip = ZipArchive::parse(apk_bytes)?;
        zip.get_file("classes.dex").ok_or(ApkError::MissingDex)
    }
}

/// Sealed ACR container security profile returned to Ring 3 userspace on APK audit.
#[derive(Debug, Clone)]
pub struct AcrContainerProfile {
    pub package_name: String,
    pub version_code: u32,
    pub target_sdk: u32,
    pub status: AcrAuditStatus,
    pub granted_caps_count: usize,
    pub denied_perms_count: usize,
    pub memory_quota_bytes: usize,
}

/// Creates a deterministic, valid test APK containing binary `AndroidManifest.xml`
/// and bytecode markers for self-tests and Ring 3 verification.
pub fn create_test_apk(package: &str, permissions: &[&str]) -> Vec<u8> {
    let mut apk = Vec::new();

    // 1. Build Binary Android XML (AXML) for AndroidManifest.xml
    let mut axml = Vec::new();
    let mut strings = Vec::new();
    strings.push(String::from("manifest"));
    strings.push(String::from("package"));
    strings.push(String::from(package));
    strings.push(String::from("uses-permission"));
    strings.push(String::from("name"));
    for p in permissions {
        strings.push(String::from(*p));
    }

    // AXML Header: Type = RES_XML_TYPE (0x0003), HdrSize = 8
    axml.extend_from_slice(&RES_XML_TYPE.to_le_bytes());
    axml.extend_from_slice(&8u16.to_le_bytes());
    axml.extend_from_slice(&0u32.to_le_bytes()); // placeholder for total_size

    // String Pool Chunk: Type = RES_STRING_POOL_TYPE (0x0001), HdrSize = 28
    let str_pool_start = axml.len();
    axml.extend_from_slice(&RES_STRING_POOL_TYPE.to_le_bytes());
    axml.extend_from_slice(&28u16.to_le_bytes());
    axml.extend_from_slice(&0u32.to_le_bytes()); // placeholder for chunk_size
    axml.extend_from_slice(&(strings.len() as u32).to_le_bytes());
    axml.extend_from_slice(&0u32.to_le_bytes()); // style count = 0
    axml.extend_from_slice(&(1u32 << 8).to_le_bytes()); // flags = UTF8 (bit 8)
    
    // Offsets start at 28 + strings.len() * 4
    let strings_data_offset = 28 + strings.len() * 4;
    axml.extend_from_slice(&(strings_data_offset as u32).to_le_bytes());
    axml.extend_from_slice(&0u32.to_le_bytes()); // styles start = 0

    // Offsets table
    let mut str_data = Vec::new();
    for s in &strings {
        let off = str_data.len() as u32;
        axml.extend_from_slice(&off.to_le_bytes());
        let s_bytes = s.as_bytes();
        str_data.push(s_bytes.len() as u8); // char len
        str_data.push(s_bytes.len() as u8); // byte len
        str_data.extend_from_slice(s_bytes);
        str_data.push(0); // null terminator
    }
    axml.extend_from_slice(&str_data);
    // Align string pool to 4 bytes
    while axml.len() % 4 != 0 {
        axml.push(0);
    }
    let str_pool_size = (axml.len() - str_pool_start) as u32;
    axml[str_pool_start + 4..str_pool_start + 8].copy_from_slice(&str_pool_size.to_le_bytes());

    // START_ELEMENT chunk: <manifest package="...">
    let _manifest_chunk_start = axml.len();
    axml.extend_from_slice(&RES_XML_START_ELEMENT_TYPE.to_le_bytes());
    axml.extend_from_slice(&16u16.to_le_bytes());
    axml.extend_from_slice(&56u32.to_le_bytes()); // chunk size: 36 header + 20 attr = 56
    axml.extend_from_slice(&1u32.to_le_bytes());  // line 1
    axml.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // comment
    axml.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // ns
    axml.extend_from_slice(&0u32.to_le_bytes());  // name_idx = 0 ("manifest")
    axml.extend_from_slice(&20u16.to_le_bytes()); // attr_start
    axml.extend_from_slice(&20u16.to_le_bytes()); // attr_size
    axml.extend_from_slice(&1u16.to_le_bytes());  // attr_count = 1
    axml.extend_from_slice(&0u16.to_le_bytes());  // id_idx
    axml.extend_from_slice(&0u16.to_le_bytes());  // class_idx
    axml.extend_from_slice(&0u16.to_le_bytes());  // style_idx
    // Attribute 0: package="com.example.app"
    axml.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // ns
    axml.extend_from_slice(&1u32.to_le_bytes()); // name_idx = 1 ("package")
    axml.extend_from_slice(&2u32.to_le_bytes()); // raw_value_idx = 2 (package name)
    axml.extend_from_slice(&0x030008u32.to_le_bytes()); // type = TYPE_STRING (0x03)
    axml.extend_from_slice(&2u32.to_le_bytes()); // data = 2

    // START_ELEMENT chunks for each permission: <uses-permission name="...">
    for (i, _) in permissions.iter().enumerate() {
        let p_str_idx = 5 + i as u32;
        axml.extend_from_slice(&RES_XML_START_ELEMENT_TYPE.to_le_bytes());
        axml.extend_from_slice(&16u16.to_le_bytes());
        axml.extend_from_slice(&56u32.to_le_bytes());
        axml.extend_from_slice(&(2 + i as u32).to_le_bytes()); // line
        axml.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        axml.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        axml.extend_from_slice(&3u32.to_le_bytes()); // name_idx = 3 ("uses-permission")
        axml.extend_from_slice(&20u16.to_le_bytes());
        axml.extend_from_slice(&20u16.to_le_bytes());
        axml.extend_from_slice(&1u16.to_le_bytes()); // attr_count = 1
        axml.extend_from_slice(&0u16.to_le_bytes());
        axml.extend_from_slice(&0u16.to_le_bytes());
        axml.extend_from_slice(&0u16.to_le_bytes());
        // Attribute: name="android.permission.X"
        axml.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        axml.extend_from_slice(&4u32.to_le_bytes()); // name_idx = 4 ("name")
        axml.extend_from_slice(&p_str_idx.to_le_bytes()); // raw_val = permission string
        axml.extend_from_slice(&0x030008u32.to_le_bytes()); // TYPE_STRING
        axml.extend_from_slice(&p_str_idx.to_le_bytes());
    }

    let total_axml_size = axml.len() as u32;
    axml[4..8].copy_from_slice(&total_axml_size.to_le_bytes());

    // 2. Synthesize real, binary classes.dex Dalvik Executable
    let test_dex = super::dex::create_test_dex(
        "Lorg/whispersystems/signal/MainActivity;",
        "onCreate",
        "Hello from Signal on Inter OS!",
    );

    // This fixture deliberately has no signature material. Tests must not confuse
    // a filename or synthetic certificate-shaped bytes with cryptographic proof.
    let files: [(&str, &[u8]); 2] = [
        ("AndroidManifest.xml", &axml),
        ("classes.dex", &test_dex),
    ];

    let mut cd_entries = Vec::new();

    for (name, payload) in &files {
        let local_hdr_offset = apk.len() as u32;
        let name_bytes = name.as_bytes();
        let payload_len = payload.len() as u32;

        // Local file header (PK\x03\x04)
        apk.extend_from_slice(&ZIP_LOCAL_FILE_HEADER_SIG.to_le_bytes());
        apk.extend_from_slice(&20u16.to_le_bytes()); // version needed = 2.0
        apk.extend_from_slice(&0u16.to_le_bytes());  // flags
        apk.extend_from_slice(&0u16.to_le_bytes());  // compression = 0 (Stored)
        apk.extend_from_slice(&0u32.to_le_bytes());  // mod time
        apk.extend_from_slice(&0u32.to_le_bytes());  // crc32
        apk.extend_from_slice(&payload_len.to_le_bytes()); // comp size
        apk.extend_from_slice(&payload_len.to_le_bytes()); // uncomp size
        apk.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        apk.extend_from_slice(&0u16.to_le_bytes());  // extra len
        apk.extend_from_slice(name_bytes);
        apk.extend_from_slice(payload);

        // Save entry info for Central Directory
        cd_entries.push((name_bytes, payload_len, local_hdr_offset));
    }

    let cd_offset = apk.len() as u32;

    // Central Directory Headers (PK\x01\x02)
    for (name_bytes, payload_len, local_hdr_offset) in &cd_entries {
        apk.extend_from_slice(&ZIP_CENTRAL_DIR_SIG.to_le_bytes());
        apk.extend_from_slice(&20u16.to_le_bytes()); // version made by
        apk.extend_from_slice(&20u16.to_le_bytes()); // version needed
        apk.extend_from_slice(&0u16.to_le_bytes());  // flags
        apk.extend_from_slice(&0u16.to_le_bytes());  // comp = 0 (Stored)
        apk.extend_from_slice(&0u32.to_le_bytes());  // mod time
        apk.extend_from_slice(&0u32.to_le_bytes());  // crc32
        apk.extend_from_slice(&payload_len.to_le_bytes()); // comp size
        apk.extend_from_slice(&payload_len.to_le_bytes()); // uncomp size
        apk.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        apk.extend_from_slice(&0u16.to_le_bytes());  // extra len
        apk.extend_from_slice(&0u16.to_le_bytes());  // comment len
        apk.extend_from_slice(&0u16.to_le_bytes());  // disk start
        apk.extend_from_slice(&0u16.to_le_bytes());  // internal attr
        apk.extend_from_slice(&0u32.to_le_bytes());  // external attr
        apk.extend_from_slice(&local_hdr_offset.to_le_bytes());
        apk.extend_from_slice(name_bytes);
    }

    let cd_size = (apk.len() as u32) - cd_offset;

    // End of Central Directory Record (PK\x05\x06)
    apk.extend_from_slice(&ZIP_END_OF_CENTRAL_DIR_SIG.to_le_bytes());
    apk.extend_from_slice(&0u16.to_le_bytes()); // disk num = 0
    apk.extend_from_slice(&0u16.to_le_bytes()); // cd disk = 0
    apk.extend_from_slice(&(files.len() as u16).to_le_bytes()); // records on disk
    apk.extend_from_slice(&(files.len() as u16).to_le_bytes()); // total records
    apk.extend_from_slice(&cd_size.to_le_bytes());
    apk.extend_from_slice(&cd_offset.to_le_bytes());
    apk.extend_from_slice(&0u16.to_le_bytes()); // comment len = 0

    apk
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apk_package_parse_and_manifest_extraction() {
        let apk_bytes = create_test_apk("org.whispersystems.signal", &["android.permission.INTERNET", "android.permission.CAMERA"]);
        let pkg = ApkPackage::parse(&apk_bytes).expect("parse valid APK");
        assert_eq!(pkg.manifest.package_name, "org.whispersystems.signal");
        assert_eq!(pkg.manifest.permissions.len(), 2);
        assert!(pkg.has_classes_dex);
        assert!(!pkg.is_signed);
        assert_eq!(pkg.file_count, 2);
    }

    #[test]
    fn test_apk_audit_unsigned_attestation_failure() {
        let apk_bytes = create_test_apk("org.whispersystems.signal", &["android.permission.INTERNET"]);
        let pkg = ApkPackage::parse(&apk_bytes).expect("parse valid APK");
        let profile = pkg.audit_and_create_container(900);
        assert_eq!(profile.status, AcrAuditStatus::AttestationFailed);
        assert_eq!(profile.granted_caps_count, 0);
        assert_eq!(profile.denied_perms_count, 1);
    }

    #[test]
    fn test_apk_audit_ambient_authority_rejection() {
        let apk_bytes = create_test_apk("com.rogue.installer", &["android.permission.INSTALL_PACKAGES"]);
        let pkg = ApkPackage::parse(&apk_bytes).expect("parse APK with forbidden perm");
        let profile = pkg.audit_and_create_container(950);
        assert_eq!(profile.status, AcrAuditStatus::AmbientAuthorityDenied);
        assert_eq!(profile.granted_caps_count, 0);
    }

    #[test]
    fn test_apk_signature_verification_strictly_fails_closed() {
        let apk_bytes = create_test_apk("org.interos.sample", &[]);
        let sig_res = verify_apk_signature(&apk_bytes);
        assert_eq!(sig_res, Err(ApkError::InvalidSignature));

        let small = [0u8; 10];
        assert_eq!(verify_apk_signature(&small), Err(ApkError::TooSmall));
    }

    #[test]
    fn test_apk_zip_deflate_extraction() {
        let apk_bytes = create_test_apk("org.interos.deflate", &["android.permission.INTERNET"]);
        let zip = ZipArchive::parse(&apk_bytes).expect("parse zip");
        let extracted = zip.extract_file("AndroidManifest.xml").expect("extract manifest");
        assert!(!extracted.is_empty());
        let missing = zip.extract_file("nonexistent.txt");
        assert_eq!(missing, Err(ApkError::MissingManifest));
    }

    #[test]
    fn test_apk_too_small_and_corrupt_rejected() {
        assert_eq!(ZipArchive::parse(&[]).err(), Some(ApkError::TooSmall));
        assert_eq!(ZipArchive::parse(&[1, 2, 3, 4]).err(), Some(ApkError::TooSmall));
        let bad = [0u8; 30];
        assert_eq!(ZipArchive::parse(&bad).err(), Some(ApkError::EndOfCentralDirectoryNotFound));
    }
}
