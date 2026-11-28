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

/// Why the registry refused to move a package forward.
///
/// WHY a named refusal and not the unit error: the registry is the
/// authoritative record of what this device will launch, and "the publisher's
/// key was revoked" and "the journal refused the write" are different faults
/// with different remedies. A caller that cannot tell them apart can only
/// answer "install failed", which is the answer the platform truth contract
/// forbids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryRefusal {
    /// The package's own identifier is on the device's revocation list.
    RevokedPackage,
    /// The publisher key that signed the package is on the device's
    /// revocation list.
    RevokedPublisher,
    /// The manifest carries no publisher key at all.
    MissingPublisher,
    /// The manifest's declared policy is not internally consistent.
    InvalidPolicy,
    /// The registry holds no record of the named package.
    UnknownPackage,
    /// The version the caller asked for is not the version the registry
    /// holds, or is older than the one it holds.
    VersionRejected,
    /// The record is not in a state that may become Active.
    NotStaged,
    /// A dependency of the manifest is not satisfied by an active package.
    DependencyUnsatisfied,
    /// The registry already holds the maximum number of records.
    RegistryFull,
    /// The journal refused the durable commit.
    PersistenceFailed,
}

/// Answers whether the publisher key that signed a registry record is still
/// acceptable to the device.
///
/// WHY the registry re-reads the CRL instead of trusting the verdict it was
/// given when the container was parsed: a staged record outlives the moment
/// it was written. The registry journal restores staged records across a
/// reboot, and a publisher key can be revoked at any time after the package
/// was verified. The question "may this package become Active" therefore has
/// to be answered at the moment of the transition and not at the moment of the
/// staging.
///
/// WHY the zero key is exempt: a zero-valued signer identity is how the
/// registry records an entry that has no publisher key at all -- the
/// first-party records the image seeds -- and not the identity of a publisher
/// whose key happens to be zero. The CRL only ever holds real publisher keys.
fn signer_is_revoked(signer: &[u8; 32]) -> bool {
    if *signer == [0u8; 32] {
        return false;
    }
    crate::package::REVOCATION_REGISTRY.lock().is_key_revoked(signer)
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
    /// UID unique among active/installed principals.
    /// UID may eventually be recycled only after package removal according to policy upon wraparound.
    /// Principal identity prevents cryptographic identity inheritance.
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

    pub fn activate_version_with_policy(&mut self, package_id: &str, version: (u16,u16,u16), policy: &PackagePolicy) -> Result<(), RegistryRefusal> {
        if !self.dependencies_satisfied(policy) { return Err(RegistryRefusal::DependencyUnsatisfied); }
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
    pub fn stage_verified(&mut self, manifest: &crate::package::RappManifest) -> Result<(), RegistryRefusal> {
        if !manifest.policy.validate() { return Err(RegistryRefusal::InvalidPolicy); }
        // WHY a manifest with no publisher key is refused here and not only at
        // parse time: the registry is a public entry point, and a record with
        // no signer is a record nobody can be held to. "Signed by nobody" is
        // not a publisher.
        if manifest.publisher_key == [0u8; 32] {
            return Err(RegistryRefusal::MissingPublisher);
        }
        {
            let crl = crate::package::REVOCATION_REGISTRY.lock();
            if crl.is_package_revoked(&manifest.package_id) {
                return Err(RegistryRefusal::RevokedPackage);
            }
            // WHY the publisher key is consulted here as well as at parse time:
            // the container was verified a moment ago, and the key that signed
            // it can have been revoked in between. A revoked publisher must not
            // leave a staged record behind that a later activation would
            // promote.
            if crl.is_key_revoked(&manifest.publisher_key) {
                return Err(RegistryRefusal::RevokedPublisher);
            }
        }

        let old = self.entries.clone();
        let existing = self.entries.iter().position(|entry| entry.package_id == manifest.package_id);
        if existing.is_none() && self.entries.len() >= MAX_ENTRIES { return Err(RegistryRefusal::RegistryFull); }

        if let Some(idx) = existing {
            let previous = self.entries[idx].clone();
            if manifest.version < previous.version {
                return Err(RegistryRefusal::VersionRejected); // Strict monotonic anti-rollback invariant
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
            return Err(RegistryRefusal::PersistenceFailed);
        }
        Ok(())
    }

    /// Commits the staged version through an explicit install transaction.
    ///
    /// The transaction is kept in-memory because the registry snapshot itself is
    /// the durable commit point: either the new Active record is persisted, or the
    /// previous snapshot is restored before returning an error.
    pub fn activate_version(&mut self, package_id: &str, version: (u16,u16,u16)) -> Result<(), RegistryRefusal> {
        {
            let crl = crate::package::REVOCATION_REGISTRY.lock();
            if crl.is_package_revoked(package_id) {
                return Err(RegistryRefusal::RevokedPackage);
            }
        }
        let idx = self.entries.iter().position(|entry| entry.package_id == package_id).ok_or(RegistryRefusal::UnknownPackage)?;
        let previous = self.entries[idx].clone();
        // WHY the signer's standing is re-read at this transition: this is the
        // step that makes the package launchable, and it can happen long after
        // the container was verified -- after a reboot that restored the staged
        // record, and after the publisher's key was revoked. A transition that
        // does not re-read the CRL can promote a package whose publisher the
        // device has already stopped trusting, and the registry would then
        // report the package as Active and launchable while the launch path
        // refuses it.
        if signer_is_revoked(&previous.signer_identity) {
            return Err(RegistryRefusal::RevokedPublisher);
        }
        if version < previous.version {
            return Err(RegistryRefusal::VersionRejected);
        }
        if version != previous.version {
            return Err(RegistryRefusal::VersionRejected);
        }
        if !matches!(previous.state, PackageState::Staged | PackageState::Active) {
            return Err(RegistryRefusal::NotStaged);
        }

        let old = self.entries.clone();
        let next_generation = self.generation.saturating_add(1);
        let mut transaction = InstallTransaction::new(idx as u32, previous.state, next_generation);
        transaction.stage();
        if !matches!(transaction.stage, InstallStage::Staged) {
            return Err(RegistryRefusal::NotStaged);
        }

        self.generation = next_generation;
        self.entries[idx].generation = next_generation;
        self.entries[idx].state = PackageState::Active;
        transaction.activate();

        if !transaction.committed() || self.persist().is_err() {
            transaction.rollback();
            self.entries = old;
            self.generation = self.entries.iter().map(|entry| entry.generation).max().unwrap_or(0);
            return Err(RegistryRefusal::PersistenceFailed);
        }
        Ok(())
    }

    pub fn activate(&mut self, package_id: &str) -> Result<(), RegistryRefusal> {
        let version=self.entries.iter().find(|e| e.package_id==package_id).map(|e| e.version).ok_or(RegistryRefusal::UnknownPackage)?;
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
        // Zero-valued signer identities are strictly rejected (Zero Ambient Authority)
        if signer_identity == [0u8; 32] {
            crate::serial_println!("[PACKAGE] registration rejected: zero-valued signer identity for '{}'", package_id);
            return Err(());
        }

        // Prevent Android APK from impersonating first-party system packages
        if crate::package::first_party_package(package_id).is_some() {
            crate::serial_println!("[PACKAGE] registration rejected: cannot overwrite first-party system package '{}'", package_id);
            return Err(());
        }

        let old_entries = self.entries.clone();
        let old_gen = self.generation;
        let old_uid = self.next_android_uid;
        let old_pid = self.next_principal_id;

        let existing = self.entries.iter().position(|e| e.package_id == package_id);
        let (app_uid, principal_id) = if let Some(idx) = existing {
            let prev = &self.entries[idx];
            // Cannot overwrite a non-Android package with an Android APK
            if prev.kind != PackageKind::AndroidApk {
                crate::serial_println!("[PACKAGE] update rejected: package kind mismatch for '{}'", package_id);
                return Err(());
            }
            // Signer continuity check: unrelated signers cannot overwrite existing packages
            if prev.signer_identity != signer_identity {
                crate::serial_println!("[PACKAGE] update rejected: signer identity mismatch for '{}'", package_id);
                return Err(());
            }
            // Anti-rollback check: monotonic version progression
            if version_code < prev.version_code {
                crate::serial_println!("[PACKAGE] update rejected: version downgrade ({} < {}) for '{}'", version_code, prev.version_code, package_id);
                return Err(());
            }
            // Anti-tamper check: cannot overwrite exact same version code with differing code digest
            if version_code == prev.version_code && code_digest != prev.code_digest {
                crate::serial_println!("[PACKAGE] update rejected: conflicting code digest for version {} in '{}'", version_code, package_id);
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

    /// Inserts a *staged* record bound to a chosen publisher key.
    ///
    /// WHY this exists: the journal is not bound on a host test machine, so the
    /// public staging path cannot leave a record behind for a test to inspect.
    /// The invariant this record has to carry is the signer's identity, because
    /// that is what the activation transition re-reads.
    #[cfg(test)]
    pub fn insert_staged_for_test(&mut self, package_id: &str, version: (u16, u16, u16), signer_identity: [u8; 32]) {
        self.entries.retain(|e| e.package_id != package_id);
        let uid = 1000 + (self.entries.len() as u32);
        self.entries.push(PackageEntry {
            package_id: package_id.to_string(),
            generation: 1,
            state: PackageState::Staged,
            version,
            kind: PackageKind::NativeRapp,
            app_uid: uid,
            code_digest: crate::crypto::Sha256::digest(package_id.as_bytes()),
            version_code: 1,
            min_sdk: 0,
            target_sdk: 0,
            signer_identity,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::rapp::{RappArch, TrustState};
    use crate::package::revocation::RevocationReason;
    use alloc::string::String;
    use alloc::vec::Vec;

    /// Builds the manifest the registry is asked about, without going through
    /// the container parser.
    ///
    /// WHY a hand-built manifest and not a signed container: the invariant under
    /// test is the registry's trust decision, and a signed container would add
    /// the signature check to the failure path without adding anything to the
    /// decision being pinned.
    fn manifest_for(package_id: &str, publisher_key: [u8; 32]) -> crate::package::RappManifest {
        crate::package::RappManifest {
            format_version: crate::package::rapp::RAPP_FORMAT_VERSION,
            package_id: String::from(package_id),
            publisher_key,
            version: (1, 0, 0),
            arch: RappArch::X86_64,
            capabilities: Vec::new(),
            payload_digest: [0u8; 32],
            signature: [0u8; 64],
            payload_offset: 0,
            payload_len: 0,
            policy: PackagePolicy { dependencies: Vec::new(), permissions: Vec::new() },
            dependencies: Vec::new(),
            intents: Vec::new(),
            trust_state: TrustState::Trusted,
        }
    }

    /// A package whose publisher key is on the CRL must not be staged.
    ///
    /// WHY the staging path is a trust decision and not a bookkeeping step: the
    /// container parser checks the CRL at parse time, and the key that signed
    /// the container can be revoked between that check and the staging. A
    /// staged record is a record a later activation will promote.
    ///
    /// Pinning assertion: the refusal names the publisher, and no record for
    /// the package is left behind.
    #[test]
    fn a_revoked_publisher_key_cannot_be_staged() {
        let key = [0x71u8; 32];
        let package_id = "org.interos.registry_revoked_stage_test";
        crate::package::REVOCATION_REGISTRY
            .lock()
            .place_key_on_crl(&key, RevocationReason::KeyCompromised, 1, "registry-test")
            .expect("the CRL has room");

        let mut registry = PackageRegistry::new();
        let refusal = registry
            .stage_verified(&manifest_for(package_id, key))
            .expect_err("a package signed by a revoked publisher must not be staged");
        assert_eq!(refusal, RegistryRefusal::RevokedPublisher);
        assert!(
            registry.entries().iter().all(|e| e.package_id != package_id),
            "a refused staging must leave no record behind"
        );

        crate::package::REVOCATION_REGISTRY.lock().revoked_keys.remove(&key);
    }

    /// A record staged while its publisher was still trusted must not become
    /// Active after that publisher's key is revoked.
    ///
    /// WHY this is the transition that matters: activation is the step that
    /// makes a package launchable, and it can happen long after the container
    /// was verified -- after a reboot that restored the staged record from the
    /// registry journal, and after the publisher's key was revoked. The
    /// registry's answer about the package has to agree with the launch path's
    /// answer, and the launch path re-reads the CRL.
    ///
    /// Pinning assertions: the refusal names the publisher, the record stays
    /// staged, and the package stays unlaunchable.
    #[test]
    fn a_staged_package_whose_publisher_was_revoked_cannot_be_activated() {
        let key = [0x72u8; 32];
        let package_id = "org.interos.registry_revoked_activate_test";
        let mut registry = PackageRegistry::new();
        registry.insert_staged_for_test(package_id, (1, 0, 0), key);
        assert!(
            !registry.launch_allowed(package_id, (1, 0, 0)),
            "a staged record must not be launchable"
        );

        crate::package::REVOCATION_REGISTRY
            .lock()
            .place_key_on_crl(&key, RevocationReason::KeyCompromised, 2, "registry-test")
            .expect("the CRL has room");

        let refusal = registry
            .activate_version(package_id, (1, 0, 0))
            .expect_err("a revoked publisher must not be promoted to Active");
        assert_eq!(refusal, RegistryRefusal::RevokedPublisher);
        let entry = registry
            .entries()
            .iter()
            .find(|e| e.package_id == package_id)
            .expect("the staged record survives a refused activation");
        assert_eq!(
            entry.state,
            PackageState::Staged,
            "a refused activation must leave the record staged"
        );
        assert!(
            !registry.launch_allowed(package_id, (1, 0, 0)),
            "a refused activation must not make the package launchable"
        );

        crate::package::REVOCATION_REGISTRY.lock().revoked_keys.remove(&key);
    }

    /// The revocation gate must not refuse a publisher that is still trusted.
    ///
    /// WHY this control exists: a gate that refuses everything is not a gate,
    /// it is an outage. On a host with no journal bound the durable commit is
    /// what fails, and the refusal has to name that fault instead of the
    /// publisher.
    #[test]
    fn an_unrevoked_publisher_is_not_refused_for_revocation() {
        let key = [0x73u8; 32];
        let package_id = "org.interos.registry_unrevoked_test";
        let mut registry = PackageRegistry::new();
        registry.insert_staged_for_test(package_id, (1, 0, 0), key);

        if let Err(refusal) = registry.activate_version(package_id, (1, 0, 0)) {
            assert_ne!(
                refusal,
                RegistryRefusal::RevokedPublisher,
                "a publisher that is not on the CRL must not be refused as revoked"
            );
        }
    }

    /// A manifest with no publisher key names no publisher at all, and the
    /// registry may not hold a record for it.
    ///
    /// WHY this is refused: "signed by nobody" is not a publisher, and a
    /// record with no signer is a record nobody can be held to.
    #[test]
    fn a_manifest_without_a_publisher_key_is_refused() {
        let mut registry = PackageRegistry::new();
        let refusal = registry
            .stage_verified(&manifest_for("org.interos.registry_no_publisher_test", [0u8; 32]))
            .expect_err("a package signed by nobody must not be staged");
        assert_eq!(refusal, RegistryRefusal::MissingPublisher);
    }
}
