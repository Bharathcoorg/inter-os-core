use alloc::vec::Vec;
use alloc::string::{String, ToString};
use crate::package::dependency::{PackagePolicy, PackageVersion};
use crate::package::transaction::{InstallStage, InstallTransaction};
use spin::Mutex;

const MAX_ENTRIES: usize = 32;
const RECORD_SIZE: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageState { Staged, Active, RolledBack }

#[derive(Debug, Clone)]
pub struct PackageEntry {
    pub package_id: String,
    pub generation: u64,
    pub state: PackageState,
    pub version: (u16, u16, u16),
}

pub struct PackageRegistry {
    entries: Vec<PackageEntry>,
    generation: u64,
    restored: bool,
}

impl PackageRegistry {
    pub const fn new() -> Self {
        Self { entries: Vec::new(), generation: 0, restored: false }
    }

    fn encode(&self, out: &mut [u8]) -> usize {
        let count = self.entries.len().min(MAX_ENTRIES);
        out[0..4].copy_from_slice(b"PRG1");
        out[4] = count as u8;
        let mut pos = 5usize;
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
            pos += 1;
        }
        pos
    }

    fn restore(&mut self) -> bool {
        let mut buf = [0u8; 2048];
        let loaded = crate::storage::persistent::load_latest_kind(&mut buf, 1);
        let Ok(Some((_generation, len))) = loaded else { return false; };
        if len < 5 || &buf[..4] != b"PRG1" { return false; }
        let count = (buf[4] as usize).min(MAX_ENTRIES);
        let mut pos = 5usize;
        self.entries.clear();
        self.generation = 0;

        for _ in 0..count {
            if pos + RECORD_SIZE > len { self.entries.clear(); return false; }
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
            let major=u16::from_le_bytes([buf[pos],buf[pos+1]]); pos+=2;
            let minor=u16::from_le_bytes([buf[pos],buf[pos+1]]); pos+=2;
            let patch=u16::from_le_bytes([buf[pos],buf[pos+1]]); pos+=2;
            pos+=1;
            self.generation=self.generation.max(generation);
            self.entries.push(PackageEntry {
                package_id:id.to_string(),
                generation,
                state,
                version:(major,minor,patch),
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
        let mut buf = [0u8; 2048];
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
            if migrated {
                let _ = self.persist();
                crate::serial_println!("[PACKAGE] migrated first-party registry entries to active state.");
            }
            crate::serial_println!("[PACKAGE] persistent registry restored from disk.");
            return;
        }
        if !self.entries.is_empty() { return; }

        for pkg in crate::package::FIRST_PARTY_PACKAGES {
            self.entries.push(PackageEntry {
                package_id: String::from(pkg.package_id),
                generation: 0,
                state: PackageState::Active,
                version: (1, 0, 0),
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

        let old = self.entries.clone();
        let existing = self.entries.iter().position(|entry| entry.package_id == manifest.package_id);
        if existing.is_none() && self.entries.len() >= MAX_ENTRIES { return Err(()); }

        if let Some(idx) = existing {
            let previous = self.entries[idx].clone();
            if manifest.version < previous.version && previous.state == PackageState::Active {
                return Err(());
            }
            if manifest.version == previous.version && matches!(previous.state, PackageState::Staged | PackageState::Active) {
                return Ok(());
            }
            self.generation = self.generation.saturating_add(1);
            self.entries[idx] = PackageEntry {
                package_id: manifest.package_id.clone(),
                generation: self.generation,
                state: PackageState::Staged,
                version: manifest.version,
            };
        } else {
            self.generation = self.generation.saturating_add(1);
            self.entries.push(PackageEntry {
                package_id: manifest.package_id.clone(),
                generation: self.generation,
                state: PackageState::Staged,
                version: manifest.version,
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
        let idx = self.entries.iter().position(|entry| entry.package_id == package_id).ok_or(())?;
        let previous = self.entries[idx].clone();
        if version < previous.version && previous.state == PackageState::Active {
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

    pub fn uninstall(&mut self, package_id: &str) -> Result<(), ()> {
        let idx = self.entries.iter().position(|entry| entry.package_id == package_id).ok_or(())?;
        if crate::package::first_party_package(package_id).is_some() {
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

    pub fn entries(&self) -> &[PackageEntry] { &self.entries }

    pub fn active_entries(&self) -> Vec<PackageEntry> {
        self.entries
            .iter()
            .filter(|entry| matches!(entry.state, PackageState::Active))
            .cloned()
            .collect()
    }
}

pub static PACKAGE_REGISTRY: Mutex<PackageRegistry> = Mutex::new(PackageRegistry::new());
