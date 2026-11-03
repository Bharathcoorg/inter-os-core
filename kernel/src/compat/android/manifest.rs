// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Android Package Manifest & Permission Model
//!
//! Parses Android application manifests and translates Android declared
//! permissions into fine-grained Inter OS zero-trust capability tokens.

use alloc::string::String;
use alloc::vec::Vec;
use crate::capability::{Capability, CapabilityId, CapabilityType, Rights};

/// Represents an Android Application Package (.apk) manifest definition.
#[derive(Debug, Clone)]
pub struct AndroidManifest {
    /// Android unique package identifier (e.g. `com.whatsapp.android`).
    pub package_name: String,
    /// Android version code.
    pub version_code: u32,
    /// Android target SDK API level.
    pub target_sdk: u32,
    /// Declared Android permissions (e.g. `android.permission.INTERNET`).
    pub permissions: Vec<String>,
}

impl AndroidManifest {
    /// Creates a new Android manifest instance.
    pub fn new(package_name: &str, version_code: u32, target_sdk: u32) -> Self {
        Self {
            package_name: String::from(package_name),
            version_code,
            target_sdk,
            permissions: Vec::new(),
        }
    }

    /// Adds a declared permission request to the manifest.
    pub fn add_permission(&mut self, permission: &str) {
        self.permissions.push(String::from(permission));
    }

    /// Translates declared Android permissions into Inter OS capability tokens.
    ///
    /// Zone 4 Security Policy:
    /// Android permissions do NOT grant ambient access. Each permission is mapped
    /// to an explicit, unforgeable capability token strictly bounded to the container.
    pub fn map_to_capabilities(&self, base_cap_id: u64) -> Vec<Capability> {
        let mut caps = Vec::new();
        let mut cap_id = base_cap_id;

        for perm in &self.permissions {
            match perm.as_str() {
                "android.permission.INTERNET" => {
                    caps.push(Capability {
                        id: CapabilityId(cap_id),
                        cap_type: CapabilityType::IpcSend,
                        rights: Rights::WRITE,
                        object_id: 0x9001, // Bounded to network proxy gateway
                    });
                    cap_id += 1;
                }
                "android.permission.READ_EXTERNAL_STORAGE" => {
                    caps.push(Capability {
                        id: CapabilityId(cap_id),
                        cap_type: CapabilityType::FileSystemRead,
                        rights: Rights::READ,
                        object_id: 0x8001, // Bounded strictly to /sdcard virtual sandbox
                    });
                    cap_id += 1;
                }
                "android.permission.WRITE_EXTERNAL_STORAGE" => {
                    caps.push(Capability {
                        id: CapabilityId(cap_id),
                        cap_type: CapabilityType::FileSystemWrite,
                        rights: Rights::WRITE,
                        object_id: 0x8001, // Bounded strictly to /sdcard virtual sandbox
                    });
                    cap_id += 1;
                }
                _ => {
                    // Unknown or sensitive permissions remain unmapped without user grant
                }
            }
        }

        caps
    }
}
