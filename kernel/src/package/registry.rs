// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

use alloc::vec::Vec;
use alloc::string::{String, ToString};
use crate::package::dependency::{PackagePolicy, PackageVersion};
use crate::package::transaction::{InstallStage, InstallTransaction};
use spin::Mutex;

const MAX_ENTRIES: usize = 32;
const RECORD_SIZE_V1: usize = 48;
const RECORD_SIZE_V2: usize = 84;
const RECORD_SIZE_V3: usize = 140;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageState { Staged, Active, RolledBack }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    NativeRapp = 0,
    AndroidApk = 1,
}

#[derive(Debug, Clone)]
pub struct PackageEntry {
    pub package_id: String,
    pub generation: u64,
    pub state: PackageState,
    pub version: (u16, u16, u16),
    pub kind: PackageKind,
    pub app_uid: u32,
    pub code_digest: [u8; 32],
    pub version_code: u64,
    pub min_sdk: u32,
    pub target_sdk: u32,
    pub signer_identity: [u8; 32],
    pub principal_id: u64,
}

pub struct PackageRegistry {
    entries: Vec<PackageEntry>,
    generation: u64,
    next_android_uid: u32,
    next_principal_id: u64,
    restored: bool,
}

impl PackageRegistry {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            generation: 0,
            next_android_uid: 10000,
            next_principal_id: 10000,
            restored: false,
        }
    }

    /// Allocates a durable, collision-free Android application UID in range 10000..19999.
    pub fn allocate_android_uid(&mut self) -> Result<u32, ()> {
        let mut candidate = self.next_android_uid.max(10000);
        for _ in 0..10000 {
            if candidate > 19999 {
                candidate = 10000;
            }
            if !self.entries.iter().any(|e| e.app_uid == candidate) {
                self.next_android_uid = candidate.saturating_add(1);
                if self.next_android_uid > 19999 {
                    self.next_android_uid = 10000;
                }
                return Ok(candidate);
            }
            candidate = candidate.saturating_add(1);
        }
        Err(())
    }

    /// Allocates a stable principal identifier across application lifecycles.
    pub fn allocate_principal_id(&mut self) -> u64 {
        let mut candidate = self.next_principal_id.max(10000);
        while self.entries.iter().any(|e| e.principal_id == candidate) {
            candidate = candidate.saturating_add(1);
        }
        self.next_principal_id = candidate.saturating_add(1);
        candidate
    }

    fn encode(&self, out: &mut [u8]) -> usize {
        let count = self.entries.len().min(MAX_ENTRIES);
        out[0..4].copy_from_slice(b"PRG3");
        out[4] = count as u8;
        out[5..9].copy_from_slice(&self.next_android_uid.to_le_bytes());
        out[9..17].copy_from_slice(&self.next_principal_id.to_le_bytes());
        let mut pos = 17usize;
        for e in self.entries.iter().take(count) {
            let id = e.package_id.as_bytes();
            let take = id.len().min(32);
            out[pos..pos + 32].fill(0);
            out[pos..pos + take].copy_from_slice(&id[..take]);
            pos += 32;
            out[pos..pos + 8].copy_from_slice(&e.generation.to_le_bytes());
            pos += 8;
            out[pos] = match e.state {
                PackageState::Staged => 0,
                PackageState::Active => 1,
                PackageState::RolledBack => 2,
            };
            pos += 1;
            out[pos..pos + 2].copy_from_slice(&e.version.0.to_le_bytes()); pos += 2;
            out[pos..pos + 2].copy_from_slice(&e.version.1.to_le_bytes()); pos += 2;
            out[pos..pos + 2].copy_from_slice(&e.version.2.to_le_bytes()); pos += 2;
            out[pos] = match e.kind {
                PackageKind::NativeRapp => 0,
                PackageKind::AndroidApk => 1,
            };
            pos += 1;
            out[pos..pos + 4].copy_from_slice(&e.app_uid.to_le_bytes());
            pos += 4;
            out[pos..pos + 32].copy_from_slice(&e.code_digest);
            pos += 32;
            out[pos..pos + 8].copy_from_slice(&e.version_code.to_le_bytes());
            pos += 8;
            out[pos..pos + 4].copy_from_slice(&e.min_sdk.to_le_bytes());
            pos += 4;
            out[pos..pos + 4].copy_from_slice(&e.target_sdk.to_le_bytes());
            pos += 4;
            out[pos..pos + 32].copy_from_slice(&e.signer_identity);
            pos += 32;
            out[pos..pos + 8].copy_from_slice(&e.principal_id.to_le_bytes());
            pos += 8;
        }
        pos
    }

    fn restore(&mut self) -> bool {
        let mut buf = [0u8; 8192];
        let loaded = crate::storage::persistent::load_latest_kind(&mut buf, 1);
        let Ok(Some((_generation, len))) = loaded else { return false; };
        if len < 5 { return false; }
        let is_v3 = len >= 17 && &buf[..4] == b"PRG3";
        let is_v2 = &buf[..4] == b"PRG2";
        let is_v1 = &buf[..4] == b"PRG1";
        if !is_v1 && !is_v2 && !is_v3 { return false; }

        let count = (buf[4] as usize).min(MAX_ENTRIES);
        let mut pos = if is_v3 {
            self.next_android_uid = u32::from_le_bytes([buf[5], buf[6], buf[7], buf[8]]);
            self.next_principal_id = u64::from_le_bytes([
                buf[9], buf[10], buf[11], buf[12],
                buf[13], buf[14], buf[15], buf[16],
            ]);
            17usize
        } else {
            self.next_android_uid = 10000;
            self.next_principal_id = 10000;
            5usize
        };
        self.entries.clear();
        self.generation = 0;

        let record_size = if is_v3 {
            RECORD_SIZE_V3
        } else if is_v2 {
            RECORD_SIZE_V2
        } else {
            RECORD_SIZE_V1
        };

        for i in 0..count {
            if pos + record_size > len { self.entries.clear(); return false; }
            let raw_id = &buf[pos..pos + 32];
            let end = raw_id.iter().position(|b| *b == 0).unwrap_or(32);
            if end == 0 || end > crate::package::rapp::RAPP_MAX_PACKAGE_ID_LEN {
                self.entries.clear();
                return false;
            }
            let id = match core::str::from_utf8(&raw_id[..end]).ok() {
                Some(v) if !v.contains('/') && !v.contains('\\') && !v.contains("..") => v,
                _ => { self.entries.clear(); return false; }
            };
            if self.entries.iter().any(|entry| entry.package_id == id) {
                self.entries.clear();
                return false;
            }
            pos += 32;
            let generation = u64::from_le_bytes([
                buf[pos],buf[pos+1],buf[pos+2],buf[pos+3],
                buf[pos+4],buf[pos+5],buf[pos+6],buf[pos+7],
            ]);
            pos += 8;
            let state = match buf[pos] {
                0 => PackageState::Staged,
                1 => PackageState::Active,
                2 => PackageState::RolledBack,
                _ => { self.entries.clear(); return false; }
            };
            pos += 1;
            let major = u16::from_le_bytes([buf[pos],buf[pos+1]]); pos += 2;
            let minor = u16::from_le_bytes([buf[pos],buf[pos+1]]); pos += 2;
            let patch = u16::from_le_bytes([buf[pos],buf[pos+1]]); pos += 2;

            let (kind, app_uid, code_digest, version_code, min_sdk, target_sdk, signer_identity, principal_id) = if is_v3 {
                let k = match buf[pos] {
                    1 => PackageKind::AndroidApk,
                    _ => PackageKind::NativeRapp,
                };
                pos += 1;
                let uid = u32::from_le_bytes([buf[pos], buf[pos+1], buf[pos+2], buf[pos+3]]);
                pos += 4;
                let mut cd = [0u8; 32];
                cd.copy_from_slice(&buf[pos..pos+32]);
                pos += 32;
                let vc = u64::from_le_bytes([
                    buf[pos], buf[pos+1], buf[pos+2], buf[pos+3],
                    buf[pos+4], buf[pos+5], buf[pos+6], buf[pos+7],
                ]);
                pos += 8;
                let msdk = u32::from_le_bytes([buf[pos], buf[pos+1], buf[pos+2], buf[pos+3]]);
                pos += 4;
                let tsdk = u32::from_le_bytes([buf[pos], buf[pos+1], buf[pos+2], buf[pos+3]]);
                pos += 4;
                let mut signer = [0u8; 32];
                signer.copy_from_slice(&buf[pos..pos+32]);
                pos += 32;
                let pid = u64::from_le_bytes([
                    buf[pos], buf[pos+1], buf[pos+2], buf[pos+3],
                    buf[pos+4], buf[pos+5], buf[pos+6], buf[pos+7],
                ]);
                pos += 8;
                (k, uid, cd, vc, msdk, tsdk, signer, pid)
            } else if is_v2 {
                let k = match buf[pos] {
                    1 => PackageKind::AndroidApk,
                    _ => PackageKind::NativeRapp,
                };
                pos += 1;
                let uid = u32::from_le_bytes([buf[pos], buf[pos+1], buf[pos+2], buf[pos+3]]);
                pos += 4;
                let mut cd = [0u8; 32];
                cd.copy_from_slice(&buf[pos..pos+32]);
                pos += 32;
                (k, uid, cd, 1u64, 21u32, 34u32, cd, uid as u64)
            } else {
                (
                    PackageKind::NativeRapp,
                    1000 + (i as u32),
                    crate::crypto::Sha256::digest(id.as_bytes()),
                    1u64,
                    0u32,
                    0u32,
                    crate::crypto::Sha256::digest(id.as_bytes()),
                    1000 + (i as u64),
                )
            };

            self.generation = self.generation.max(generation);
            if app_uid >= 10000 && app_uid < 20000 {
                self.next_android_uid = self.next_android_uid.max(app_uid + 1);
            }
            self.next_principal_id = self.next_principal_id.max(principal_id + 1);
            self.entries.push(PackageEntry {
                package_id: id.to_string(),
                generation,
                state,
                version: (major, minor, patch),
                kind,
                app_uid,
                code_digest,
                version_code,
                min_sdk,
                target_sdk,
                signer_identity,
                principal_id,
            });
        }
        if pos != len {
            self.entries.clear();
            self.generation = 0;
            return false;
        }
        count > 0
    }

    fn persist(&self) -> Result<(), ()> {
        let mut buf = [0u8; 8192];
        let len = self.encode(&mut buf);
        crate::storage::persistent::commit_kind(&buf[..len], 1).map(|_| ()).map_err(|_| ())
    }

    pub fn seed(&mut self) {
        if self.restored { return; }
        self.restored = true;
        if self.restore() {
            let mut migrated = false;
            for entry in self.entries.iter_mut() {
                if entry.generation == 0
                    && matches!(entry.state, PackageState::Staged)
                    && crate::package::first_party_package(&entry.package_id).is_some()
                {
                    entry.state = PackageState::Active;
                    migrated = true;
                }
            }
            for (idx, pkg) in crate::package::FIRST_PARTY_PACKAGES.iter().enumerate() {
                if !self.entries.iter().any(|e| e.package_id == pkg.package_id) {
                    let uid = 1000 + (idx as u32);
                    self.entries.push(PackageEntry {
                        package_id: String::from(pkg.package_id),
                        generation: 0,
                        state: PackageState::Active,
                        version: (1, 0, 0),
                        kind: PackageKind::NativeRapp,
                        app_uid: uid,
                        code_digest: crate::crypto::Sha256::digest(pkg.package_id.as_bytes()),
                        version_code: 1,
                        min_sdk: 0,
                        target_sdk: 0,
                        signer_identity: [0u8; 32],
                        principal_id: uid as u64,
                    });
                    migrated = true;
                }
            }
            if migrated {
                let _ = self.persist();
                crate::serial_println!("[PACKAGE] migrated first-party registry entries to active state.");
            }
            crate::serial_println!("[PACKAGE] persistent registry restored from disk.");
            return;
        }
        if !self.entries.is_empty() { return; }

        for (idx, pkg) in crate::package::FIRST_PARTY_PACKAGES.iter().enumerate() {
            let uid = 1000 + (idx as u32);
            self.entries.push(PackageEntry {
                package_id: String::from(pkg.package_id),
                generation: 0,
                state: PackageState::Active,
                version: (1, 0, 0),
                kind: PackageKind::NativeRapp,
                app_uid: uid,
                code_digest: crate::crypto::Sha256::digest(pkg.package_id.as_bytes()),
                version_code: 1,
                min_sdk: 0,
                target_sdk: 0,
                signer_identity: [0u8; 32],
                principal_id: uid as u64,
            });
        }

        if self.persist().is_err() {
            crate::serial_println!("[PACKAGE] registry persistence unavailable; runtime registry remains active.");
        } else {
            crate::serial_println!("[PACKAGE] persistent registry initialized.");
        }
    }

    pub fn dependencies_satisfied(&self, policy: &PackagePolicy) -> bool {
        policy.validate() && policy.dependencies.iter().all(|dep| {
            self.entries.iter().find(|entry| entry.package_id == dep.package_id)
                .map(|entry| dep.requirement.matches(PackageVersion::new(entry.version.0, entry.version.1, entry.version.2)) && matches!(entry.state, PackageState::Active))
                .unwrap_or(false)
        })
    }

    pub fn activate_version_with_policy(&mut self, package_id: &str, version: (u16,u16,u16), policy: &PackagePolicy) -> Result<(), ()> {
        if !self.dependencies_satisfied(policy) { return Err(()); }
        self.activate_version(package_id, version)
    }


    /// Stages a verified package for later activation. Registration is bounded and
    /// independent of the built-in package catalog so downstream forks can ship
    /// their own signed applications without modifying kernel source.
    /// Atomically records a verified package as the next install candidate.
    ///
    /// A package ID has one authoritative registry record. Replacing an existing
    /// record prevents an older active entry from shadowing a newly staged version
    /// and keeps launch/rollback decisions deterministic across reboots.
    pub fn stage_verified(&mut self, manifest: &crate::package::RappManifest) -> Result<(), ()> {
        if !manifest.policy.validate() { return Err(()); }
        if crate::package::REVOCATION_REGISTRY.lock().is_package_revoked(&manifest.package_id) {
            return Err(());
        }

        let old = self.entries.clone();
        let existing = self.entries.iter().position(|entry| entry.package_id == manifest.package_id);
        if existing.is_none() && self.entries.len() >= MAX_ENTRIES { return Err(()); }

        if let Some(idx) = existing {
            let previous = self.entries[idx].clone();
            if manifest.version < previous.version {
                return Err(()); // Strict monotonic anti-rollback invariant
            }
            if manifest.version == previous.version && matches!(previous.state, PackageState::Staged | PackageState::Active) {
                return Ok(());
            }
            self.generation = self.generation.saturating_add(1);
            let uid = self.entries[idx].app_uid;
            let pid = self.entries[idx].principal_id;
            let ver_code = ((manifest.version.0 as u64) << 32) | ((manifest.version.1 as u64) << 16) | (manifest.version.2 as u64);
            self.entries[idx] = PackageEntry {
                package_id: manifest.package_id.clone(),
                generation: self.generation,
                state: PackageState::Staged,
                version: manifest.version,
                kind: PackageKind::NativeRapp,
                app_uid: uid,
                code_digest: manifest.payload_digest,
                version_code: ver_code,
                min_sdk: 0,
                target_sdk: 0,
                signer_identity: manifest.publisher_key,
                principal_id: pid,
            };
        } else {
            self.generation = self.generation.saturating_add(1);
            let uid = 2000 + (self.entries.len() as u32) + 1;
            let pid = self.allocate_principal_id();
            let ver_code = ((manifest.version.0 as u64) << 32) | ((manifest.version.1 as u64) << 16) | (manifest.version.2 as u64);
            self.entries.push(PackageEntry {
                package_id: manifest.package_id.clone(),
                generation: self.generation,
                state: PackageState::Staged,
                version: manifest.version,
                kind: PackageKind::NativeRapp,
                app_uid: uid,
                code_digest: manifest.payload_digest,
                version_code: ver_code,
                min_sdk: 0,
                target_sdk: 0,
                signer_identity: manifest.publisher_key,
                principal_id: pid,
            });
        }

        if self.persist().is_err() {
            self.entries = old;
            self.generation = self.entries.iter().map(|entry| entry.generation).max().unwrap_or(0);
            return Err(());
        }
        Ok(())
    }

    /// Commits the staged version through an explicit install transaction.
    ///
    /// The transaction is kept in-memory because the registry snapshot itself is
    /// the durable commit point: either the new Active record is persisted, or the
    /// previous snapshot is restored before returning an error.
    pub fn activate_version(&mut self, package_id: &str, version: (u16,u16,u16)) -> Result<(), ()> {
        if crate::package::REVOCATION_REGISTRY.lock().is_package_revoked(package_id) {
            return Err(());
        }
        let idx = self.entries.iter().position(|entry| entry.package_id == package_id).ok_or(())?;
        let previous = self.entries[idx].clone();
        if version < previous.version {
            return Err(());
        }
        if version != previous.version {
            return Err(());
        }
        if !matches!(previous.state, PackageState::Staged | PackageState::Active) {
            return Err(());
        }

        let old = self.entries.clone();
        let next_generation = self.generation.saturating_add(1);
        let mut transaction = InstallTransaction::new(idx as u32, previous.state, next_generation);
        transaction.stage();
        if !matches!(transaction.stage, InstallStage::Staged) {
            return Err(());
        }

        self.generation = next_generation;
        self.entries[idx].generation = next_generation;
        self.entries[idx].state = PackageState::Active;
        transaction.activate();

        if !transaction.committed() || self.persist().is_err() {
            transaction.rollback();
            self.entries = old;
            self.generation = self.entries.iter().map(|entry| entry.generation).max().unwrap_or(0);
            return Err(());
        }
        Ok(())
    }

    pub fn activate(&mut self, package_id: &str) -> Result<(), ()> {
        let version=self.entries.iter().find(|e| e.package_id==package_id).map(|e| e.version).ok_or(())?;
        self.activate_version(package_id, version)
    }

    /// Restores the exact registry record that existed before an install attempt.
    /// If there was no previous record, the failed candidate is removed entirely.
    pub fn rollback_install(&mut self, package_id: &str, previous: Option<PackageEntry>) -> Result<(), ()> {
        let old = self.entries.clone();
        let old_generation = self.generation;
        match previous {
            Some(previous) => {
                let idx = self.entries.iter().position(|e| e.package_id == package_id).ok_or(())?;
                self.entries[idx] = previous;
            }
            None => {
                let idx = self.entries.iter().position(|e| e.package_id == package_id).ok_or(())?;
                self.entries.remove(idx);
            }
        }
        if self.persist().is_err() {
            self.entries = old;
            self.generation = old_generation;
            return Err(());
        }
        self.generation = self.entries.iter().map(|e| e.generation).max().unwrap_or(0);
        Ok(())
    }

    /// Explicitly disables the currently active package without deleting its
    /// payload. This is the public rollback/quarantine operation; install
    /// transaction failures use rollback_install() instead.
    pub fn rollback(&mut self, package_id: &str) -> Result<(), ()> {
        let idx = self.entries.iter().position(|e| e.package_id == package_id).ok_or(())?;
        let old = self.entries.clone();
        let old_generation = self.generation;
        let next_generation = self.generation.saturating_add(1);
        self.generation = next_generation;
        self.entries[idx].generation = next_generation;
        self.entries[idx].state = PackageState::RolledBack;
        if self.persist().is_err() {
            self.entries = old;
            self.generation = old_generation;
            return Err(());
        }
        Ok(())
    }


    /// Removes an installed package record transactionally.
    pub fn restore_entry(&mut self, entry: PackageEntry) -> Result<(), ()> {
        if let Some(existing) = self.entries.iter_mut().find(|e| e.package_id == entry.package_id) {
            *existing = entry;
        } else {
            if self.entries.len() >= MAX_ENTRIES { return Err(()); }
            self.entries.push(entry);
        }
        if self.persist().is_err() { return Err(()); }
        self.generation = self.entries.iter().map(|e| e.generation).max().unwrap_or(0);
        Ok(())
    }

    /// Refuses removal of a package that is still required by an active package.
    /// Dependency checks are evaluated against the authoritative registry snapshot,
    /// so uninstall cannot silently invalidate another active package's policy.
    pub fn has_active_dependents(&self, package_id: &str) -> bool {
        self.entries.iter().any(|entry| {
            if !matches!(entry.state, PackageState::Active) || entry.package_id == package_id {
                return false;
            }
            let Some(blob) = crate::package::PACKAGE_BLOB_STORE.lock().get(&entry.package_id).cloned() else {
                return false;
            };
            let Ok(manifest) = crate::package::parse_rapp(&blob.bytes) else {
                return false;
            };
            manifest.policy.dependencies.iter().any(|dep| dep.package_id == package_id)
        })
    }

    pub fn uninstall(&mut self, package_id: &str) -> Result<(), ()> {
        let idx = self.entries.iter().position(|entry| entry.package_id == package_id).ok_or(())?;
        if crate::package::first_party_package(package_id).is_some() {
            return Err(());
        }
        if self.has_active_dependents(package_id) {
            return Err(());
        }
        let old = self.entries.clone();
        let old_generation = self.generation;
        self.entries.remove(idx);
        if self.persist().is_err() {
            self.entries = old;
            self.generation = old_generation;
            return Err(());
        }
        self.generation = self.entries.iter().map(|entry| entry.generation).max().unwrap_or(0);
        Ok(())
    }

    pub fn launch_allowed(&self, package_id:&str, version:(u16,u16,u16))->bool {
        self.entries.iter()
            .find(|e| e.package_id == package_id)
            .map(|e| matches!(e.state, PackageState::Active) && version == e.version)
            .unwrap_or(false)
    }

    /// Atomically registers a verified Android application package into the registry.
    /// Allocates and preserves a stable Android application UID in range 10000..19999.
    /// Stores the cryptographic code digest bound to the actual verified installed APK artifact.
    /// Enforces signer continuity, monotonic version progression, and transactional persistence.
    pub fn register_verified_apk(
        &mut self,
        package_id: &str,
        version: (u16, u16, u16),
        version_code: u64,
        min_sdk: u32,
        target_sdk: u32,
        code_digest: [u8; 32],
        signer_identity: [u8; 32],
    ) -> Result<u32, ()> {
        let old_entries = self.entries.clone();
        let old_gen = self.generation;
        let old_uid = self.next_android_uid;
        let old_pid = self.next_principal_id;

        let existing = self.entries.iter().position(|e| e.package_id == package_id);
        let (app_uid, principal_id) = if let Some(idx) = existing {
            let prev = &self.entries[idx];
            // Signer continuity check: unrelated signers cannot overwrite existing packages
            if prev.signer_identity != [0u8; 32] && signer_identity != [0u8; 32] && prev.signer_identity != signer_identity {
                crate::serial_println!("[PACKAGE] update rejected: signer identity mismatch for '{}'", package_id);
                return Err(());
            }
            // Anti-rollback check: monotonic version progression
            if version_code < prev.version_code {
                crate::serial_println!("[PACKAGE] update rejected: version downgrade ({} < {}) for '{}'", version_code, prev.version_code, package_id);
                return Err(());
            }
            (prev.app_uid, prev.principal_id)
        } else {
            if self.entries.len() >= MAX_ENTRIES {
                return Err(());
            }
            // Durable stable UID allocator replacing the legacy heuristic 10000 + (self.entries.len() as u32) + 1
            let uid = self.allocate_android_uid()?;
            let pid = self.allocate_principal_id();
            (uid, pid)
        };

        self.generation = self.generation.saturating_add(1);
        let new_entry = PackageEntry {
            package_id: package_id.to_string(),
            generation: self.generation,
            state: PackageState::Active,
            version,
            kind: PackageKind::AndroidApk,
            app_uid,
            code_digest,
            version_code,
            min_sdk,
            target_sdk,
            signer_identity,
            principal_id,
        };

        if let Some(idx) = existing {
            self.entries[idx] = new_entry;
        } else {
            self.entries.push(new_entry);
        }

        if self.persist().is_err() {
            // Restore snapshot on persistence error (transactional rollback)
            self.entries = old_entries;
            self.generation = old_gen;
            self.next_android_uid = old_uid;
            self.next_principal_id = old_pid;
            return Err(());
        }
        Ok(app_uid)
    }

    /// Convenience wrapper for 3-argument legacy callers.
    pub fn register_verified_apk_simple(
        &mut self,
        package_id: &str,
        version: (u16, u16, u16),
        code_digest: [u8; 32],
    ) -> Result<u32, ()> {
        let ver_code = ((version.0 as u64) << 32) | ((version.1 as u64) << 16) | (version.2 as u64);
        self.register_verified_apk(package_id, version, ver_code, 21, 34, code_digest, code_digest)
    }

    pub fn entries(&self) -> &[PackageEntry] { &self.entries }

    pub fn active_entries(&self) -> Vec<PackageEntry> {
        self.entries
            .iter()
            .filter(|entry| matches!(entry.state, PackageState::Active))
            .cloned()
            .collect()
    }
    #[cfg(test)]
    pub fn insert_active_for_test(&mut self, package_id: &str, version: (u16, u16, u16)) {
        self.entries.retain(|e| e.package_id != package_id);
        let uid = 1000 + (self.entries.len() as u32);
        self.entries.push(PackageEntry {
            package_id: package_id.to_string(),
            generation: 1,
            state: PackageState::Active,
            version,
            kind: PackageKind::NativeRapp,
            app_uid: uid,
            code_digest: crate::crypto::Sha256::digest(package_id.as_bytes()),
            version_code: 1,
            min_sdk: 0,
            target_sdk: 0,
            signer_identity: [0u8; 32],
            principal_id: uid as u64,
        });
    }

    #[cfg(test)]
    pub fn insert_active_apk_for_test(&mut self, package_id: &str, version: (u16, u16, u16), code_digest: [u8; 32]) -> u32 {
        self.register_verified_apk_simple(package_id, version, code_digest).unwrap_or(10000)
    }

    #[cfg(test)]
    pub fn remove_for_test(&mut self, package_id: &str) {
        self.entries.retain(|e| e.package_id != package_id);
    }
}
pub static PACKAGE_REGISTRY: Mutex<PackageRegistry> = Mutex::new(PackageRegistry::new());
