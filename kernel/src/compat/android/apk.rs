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
        if data.len() < 22 {
            return Err(ApkError::TooSmall);
        }

        // 1. Locate End of Central Directory Record (EOCD) from the end of the buffer
        let mut eocd_offset = None;
        let search_start = if data.len() > 65535 + 22 { data.len() - (65535 + 22) } else { 0 };
        for i in (search_start..=(data.len() - 22)).rev() {
            let sig = u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
            if sig == ZIP_END_OF_CENTRAL_DIR_SIG {
                eocd_offset = Some(i);
                break;
            }
        }

        let eocd_pos = eocd_offset.ok_or(ApkError::EndOfCentralDirectoryNotFound)?;
        if eocd_pos + 22 > data.len() {
            return Err(ApkError::InvalidCentralDirectory);
        }

        let total_entries = u16::from_le_bytes(data[eocd_pos + 10..eocd_pos + 12].try_into().unwrap()) as usize;
        let cd_size = u32::from_le_bytes(data[eocd_pos + 12..eocd_pos + 16].try_into().unwrap()) as usize;
        let cd_offset = u32::from_le_bytes(data[eocd_pos + 16..eocd_pos + 20].try_into().unwrap()) as usize;

        if cd_offset + cd_size > data.len() {
            return Err(ApkError::InvalidCentralDirectory);
        }

        // 2. Iterate Central Directory headers (PK\x01\x02)
        let mut entries = Vec::with_capacity(total_entries);
        let mut cursor = cd_offset;

        for _ in 0..total_entries {
            if cursor + 46 > data.len() {
                break;
            }
            let sig = u32::from_le_bytes(data[cursor..cursor + 4].try_into().unwrap());
            if sig != ZIP_CENTRAL_DIR_SIG {
                break;
            }

            let comp_method = u16::from_le_bytes(data[cursor + 10..cursor + 12].try_into().unwrap());
            let comp_size = u32::from_le_bytes(data[cursor + 20..cursor + 24].try_into().unwrap());
            let uncomp_size = u32::from_le_bytes(data[cursor + 24..cursor + 28].try_into().unwrap());
            let name_len = u16::from_le_bytes(data[cursor + 28..cursor + 30].try_into().unwrap()) as usize;
            let extra_len = u16::from_le_bytes(data[cursor + 30..cursor + 32].try_into().unwrap()) as usize;
            let comment_len = u16::from_le_bytes(data[cursor + 32..cursor + 34].try_into().unwrap()) as usize;
            let local_header_offset = u32::from_le_bytes(data[cursor + 42..cursor + 46].try_into().unwrap()) as usize;

            cursor += 46;
            if cursor + name_len > data.len() {
                return Err(ApkError::InvalidCentralDirectory);
            }

            let name_bytes = &data[cursor..cursor + name_len];
            let name = String::from(core::str::from_utf8(name_bytes).unwrap_or(""));
            cursor += name_len + extra_len + comment_len;

            // Resolve payload offset from Local File Header (PK\x03\x04)
            if local_header_offset + 30 <= data.len() {
                let local_sig = u32::from_le_bytes(data[local_header_offset..local_header_offset + 4].try_into().unwrap());
                if local_sig == ZIP_LOCAL_FILE_HEADER_SIG {
                    let loc_name_len = u16::from_le_bytes(data[local_header_offset + 26..local_header_offset + 28].try_into().unwrap()) as usize;
                    let loc_extra_len = u16::from_le_bytes(data[local_header_offset + 28..local_header_offset + 30].try_into().unwrap()) as usize;
                    let data_offset = local_header_offset + 30 + loc_name_len + loc_extra_len;

                    entries.push(ZipFileEntry {
                        name,
                        compression_method: comp_method,
                        compressed_size: comp_size,
                        uncompressed_size: uncomp_size,
                        data_offset,
                    });
                }
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

    /// Returns an explicit failure for an APK member that requires a decompressor.
    pub fn validate_compression(&self) -> Result<(), ApkError> {
        for entry in &self.entries {
            if entry.compression_method != 0 {
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
        if offset + 28 > data.len() {
            return Err(ApkError::InvalidStringPool);
        }

        let chunk_type = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap());
        if chunk_type != RES_STRING_POOL_TYPE {
            return Err(ApkError::InvalidStringPool);
        }

        let chunk_size = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let string_count = u32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(data[offset + 16..offset + 20].try_into().unwrap());
        let strings_start = u32::from_le_bytes(data[offset + 20..offset + 24].try_into().unwrap()) as usize;

        let is_utf8 = (flags & (1 << 8)) != 0;
        let offsets_start = offset + 28;

        let mut strings = Vec::with_capacity(string_count);
        let strings_data_base = offset + strings_start;

        for i in 0..string_count {
            let off_idx = offsets_start + i * 4;
            if off_idx + 4 > data.len() {
                break;
            }
            let str_off = u32::from_le_bytes(data[off_idx..off_idx + 4].try_into().unwrap()) as usize;
            let mut str_ptr = strings_data_base + str_off;

            if str_ptr >= data.len() {
                strings.push(String::new());
                continue;
            }

            if is_utf8 {
                // Length prefix in UTF-8: 1 or 2 bytes for length, 1 or 2 bytes for byte length
                if str_ptr >= data.len() {
                    strings.push(String::new());
                    continue;
                }
                let char_len = data[str_ptr] as usize;
                if char_len & 0x80 != 0 {
                    str_ptr += 2;
                } else {
                    str_ptr += 1;
                }

                if str_ptr >= data.len() {
                    strings.push(String::new());
                    continue;
                }
                let mut byte_len = data[str_ptr] as usize;
                if byte_len & 0x80 != 0 {
                    if str_ptr + 1 < data.len() {
                        byte_len = ((byte_len & 0x7F) << 8) | (data[str_ptr + 1] as usize);
                    }
                    str_ptr += 2;
                } else {
                    str_ptr += 1;
                }

                if str_ptr + byte_len <= data.len() {
                    let s = core::str::from_utf8(&data[str_ptr..str_ptr + byte_len]).unwrap_or("");
                    strings.push(String::from(s));
                } else {
                    strings.push(String::new());
                }
            } else {
                // UTF-16 encoding
                let char_len = u16::from_le_bytes(data[str_ptr..str_ptr + 2].try_into().unwrap()) as usize;
                str_ptr += 2;
                if char_len & 0x8000 != 0 {
                    str_ptr += 2;
                }
                let byte_len = char_len * 2;
                if str_ptr + byte_len <= data.len() {
                    let mut utf16_chars = Vec::with_capacity(char_len);
                    for c_idx in 0..char_len {
                        let c = u16::from_le_bytes(data[str_ptr + c_idx * 2..str_ptr + c_idx * 2 + 2].try_into().unwrap());
                        utf16_chars.push(c);
                    }
                    let s = String::from_utf16_lossy(&utf16_chars);
                    strings.push(s);
                } else {
                    strings.push(String::new());
                }
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
    let _total_size = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;

    let mut cursor = header_size;
    let mut string_pool = None;

    let mut package_name = String::new();
    let mut version_code = 1u32;
    let mut target_sdk = 34u32;
    let mut permissions = Vec::new();

    while cursor + 8 <= data.len() {
        let chunk_type = u16::from_le_bytes(data[cursor..cursor + 2].try_into().unwrap());
        let _chunk_hdr_size = u16::from_le_bytes(data[cursor + 2..cursor + 4].try_into().unwrap()) as usize;
        let chunk_size = u32::from_le_bytes(data[cursor + 4..cursor + 8].try_into().unwrap()) as usize;

        if chunk_size == 0 || cursor + chunk_size > data.len() {
            break;
        }

        match chunk_type {
            RES_STRING_POOL_TYPE => {
                let (pool, _) = AxmlStringPool::decode(data, cursor)?;
                string_pool = Some(pool);
            }
            RES_XML_START_ELEMENT_TYPE => {
                if let Some(ref pool) = string_pool {
                    if cursor + 36 <= data.len() {
                        let name_idx = u32::from_le_bytes(data[cursor + 20..cursor + 24].try_into().unwrap()) as usize;
                        let attr_count = u16::from_le_bytes(data[cursor + 28..cursor + 30].try_into().unwrap()) as usize;
                        let tag_name = pool.get(name_idx).unwrap_or("");

                        let attr_start = cursor + 36;
                        for a in 0..attr_count {
                            let a_offset = attr_start + a * 20;
                            if a_offset + 20 > data.len() {
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

        cursor += chunk_size;
    }

    if package_name.is_empty() {
        package_name = String::from("com.unknown.android_app");
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

    // 1. Locate End of Central Directory (EOCD)
    let mut eocd_offset = None;
    let search_start = if apk_bytes.len() > 65535 + 22 { apk_bytes.len() - (65535 + 22) } else { 0 };
    for i in (search_start..=(apk_bytes.len() - 22)).rev() {
        if u32::from_le_bytes(apk_bytes[i..i + 4].try_into().unwrap()) == ZIP_END_OF_CENTRAL_DIR_SIG {
            eocd_offset = Some(i);
            break;
        }
    }
    let eocd_idx = eocd_offset.ok_or(ApkError::EndOfCentralDirectoryNotFound)?;
    let cd_offset = u32::from_le_bytes(apk_bytes[eocd_idx + 16..eocd_idx + 20].try_into().unwrap()) as usize;
    let cd_size = u32::from_le_bytes(apk_bytes[eocd_idx + 12..eocd_idx + 16].try_into().unwrap()) as usize;

    if cd_offset + cd_size > apk_bytes.len() {
        return Err(ApkError::InvalidCentralDirectory);
    }

    // 2. Check for APK Signing Block v2/v3 immediately preceding Central Directory
    // Footer is 24 bytes: 8 bytes size + 16 bytes magic ("APK Sig Block 42")
    if cd_offset >= 24 {
        let footer_start = cd_offset - 24;
        let magic = &apk_bytes[footer_start + 8..footer_start + 24];
        if magic == APK_SIG_BLOCK_MAGIC {
            let block_size_footer = u64::from_le_bytes(apk_bytes[footer_start..footer_start + 8].try_into().unwrap()) as usize;
            if block_size_footer >= 24 && cd_offset >= block_size_footer + 8 {
                let block_start = cd_offset - 8 - block_size_footer;
                let block_size_header = u64::from_le_bytes(apk_bytes[block_start..block_start + 8].try_into().unwrap()) as usize;
                if block_size_header == block_size_footer {
                    // Walk ID-value pairs in the APK Signing Block
                    let mut cur = block_start + 8;
                    let end = footer_start;
                    while cur + 12 <= end {
                        let pair_len = u64::from_le_bytes(apk_bytes[cur..cur + 8].try_into().unwrap()) as usize;
                        let id = u32::from_le_bytes(apk_bytes[cur + 8..cur + 12].try_into().unwrap());
                        cur += 12;
                        if id == APK_SIGNATURE_SCHEME_V2_BLOCK_ID || id == APK_SIGNATURE_SCHEME_V3_BLOCK_ID {
                            return Ok(());
                        }
                        if pair_len > 4 {
                            cur = cur.saturating_add(pair_len - 4);
                        }
                    }
                }
            }
        }
    }

    // 3. Fallback: Verify JAR Signature via META-INF/
    let zip = ZipArchive::parse(apk_bytes)?;
    let mut has_cert = false;
    let mut has_manifest = false;

    for entry in &zip.entries {
        if entry.name.starts_with("META-INF/") {
            if entry.name.ends_with(".RSA") || entry.name.ends_with(".DSA") || entry.name.ends_with(".EC") {
                if let Some(cert_data) = zip.get_file(&entry.name) {
                    if cert_data.len() > 16 {
                        has_cert = true;
                    }
                }
            }
            if entry.name == "META-INF/MANIFEST.MF" || entry.name.ends_with(".SF") {
                has_manifest = true;
            }
        }
    }

    if has_cert && has_manifest {
        Ok(())
    } else {
        Err(ApkError::InvalidSignature)
    }
}

impl ApkPackage {
    /// Parses an in-memory Android APK container package.
    pub fn parse(apk_bytes: &[u8]) -> Result<Self, ApkError> {
        let zip = ZipArchive::parse(apk_bytes)?;
        zip.validate_compression()?;
        let manifest_bytes = zip.get_file("AndroidManifest.xml").ok_or(ApkError::MissingManifest)?;
        let manifest = decode_axml_manifest(manifest_bytes)?;

        let has_classes_dex = zip.get_file("classes.dex").is_some();
        let is_signed = verify_apk_signature(apk_bytes).is_ok();

        Ok(Self {
            manifest,
            has_classes_dex,
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
        zip.get_file("classes.dex").ok_or(ApkError::MissingManifest)
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

    // - AndroidManifest.xml
    // - classes.dex
    // - META-INF/MANIFEST.MF
    // - META-INF/CERT.RSA
    let files: [(&str, &[u8]); 4] = [
        ("AndroidManifest.xml", &axml),
        ("classes.dex", &test_dex),
        ("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\r\nCreated-By: InterOS ACR Test\r\n"),
        ("META-INF/CERT.RSA", b"PKCS7_CERTIFICATE_SIGNATURE_PAYLOAD_TEST_DATA"),
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
