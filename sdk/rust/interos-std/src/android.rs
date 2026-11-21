// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! InterOS Userspace Android Compatibility Runtime (ACR) Client & Manifest ABI
//!
//! Provides userspace `.rapp` applications, launcher, and system services with
//! verified, zero-allocation data structures for inspecting Android packages,
//! resolving compiled resources, parsing APK Signature Schemes (v2/v3/v4),
//! and mediating Activity lifecycle intents without ambient authority.

/// Android APK Signature Scheme versions recognized by InterOS ACR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcrSignatureScheme {
    V2,
    V3,
    V4,
}

/// Cryptographically attested signing metadata for an Android package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcrSigningDetails {
    pub scheme: AcrSignatureScheme,
    pub signers_count: u16,
    pub primary_cert_fingerprint: [u8; 32],
    pub content_digest: [u8; 32],
    pub algorithm_id: u32,
}

/// Maximum length of package name stored in fixed-size descriptor.
pub const MAX_PACKAGE_NAME_LEN: usize = 64;

/// Maximum length of action name stored in fixed-size intent.
pub const MAX_ACTION_LEN: usize = 64;

/// Maximum length of data URI stored in fixed-size intent.
pub const MAX_URI_LEN: usize = 128;

/// Verified package metadata returned by the ACR package installer (zero heap allocations).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcrPackageMetadata {
    package_name_buf: [u8; MAX_PACKAGE_NAME_LEN],
    package_name_len: usize,
    pub version_code: u32,
    pub target_sdk: u32,
    pub is_signed: bool,
    pub multidex_count: u8,
    pub signing_details: Option<AcrSigningDetails>,
}

impl AcrPackageMetadata {
    pub fn new(package_name: &str, version_code: u32, target_sdk: u32) -> Self {
        let mut buf = [0u8; MAX_PACKAGE_NAME_LEN];
        let bytes = package_name.as_bytes();
        let len = bytes.len().min(MAX_PACKAGE_NAME_LEN);
        buf[..len].copy_from_slice(&bytes[..len]);

        Self {
            package_name_buf: buf,
            package_name_len: len,
            version_code,
            target_sdk,
            is_signed: false,
            multidex_count: 1,
            signing_details: None,
        }
    }

    pub fn package_name(&self) -> &str {
        core::str::from_utf8(&self.package_name_buf[..self.package_name_len]).unwrap_or("")
    }
}

/// Android Intent descriptor for launching or binding ACR compatibility components.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcrIntent {
    action_buf: [u8; MAX_ACTION_LEN],
    action_len: usize,
    uri_buf: [u8; MAX_URI_LEN],
    uri_len: usize,
    pub flags: u32,
}

impl AcrIntent {
    pub fn new(action: &str) -> Self {
        let mut action_buf = [0u8; MAX_ACTION_LEN];
        let bytes = action.as_bytes();
        let len = bytes.len().min(MAX_ACTION_LEN);
        action_buf[..len].copy_from_slice(&bytes[..len]);

        Self {
            action_buf,
            action_len: len,
            uri_buf: [0u8; MAX_URI_LEN],
            uri_len: 0,
            flags: 0,
        }
    }

    pub fn with_data(mut self, uri: &str) -> Self {
        let bytes = uri.as_bytes();
        let len = bytes.len().min(MAX_URI_LEN);
        self.uri_buf[..len].copy_from_slice(&bytes[..len]);
        self.uri_len = len;
        self
    }

    pub fn action(&self) -> &str {
        core::str::from_utf8(&self.action_buf[..self.action_len]).unwrap_or("")
    }

    pub fn data_uri(&self) -> Option<&str> {
        if self.uri_len > 0 {
            core::str::from_utf8(&self.uri_buf[..self.uri_len]).ok()
        } else {
            None
        }
    }
}

/// Standard Android Activity Lifecycle state enumeration in userspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcrActivityState {
    Uninitialized,
    Created,
    Started,
    Resumed,
    Paused,
    Stopped,
    Destroyed,
}
