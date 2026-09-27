use alloc::vec::Vec;
use crate::package::dependency::{PackagePolicy, PackageVersion};
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
            let id = match core::str::from_utf8(&raw_id[..end]).ok() {
                Some(v) => v,
                None => { self.entries.clear(); return false; }
            };
            let Some(desc) = crate::package::first_party_package(id) else {
                self.entries.clear();
                return false;
            };
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
                package_id:desc.package_id.to_string(),
                generation,
                state,
                version:(major,minor,patch),
            });
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
            crate::serial_println!("[PACKAGE] persistent registry restored from disk.");
            return;
        }
        if !self.entries.is_empty() { return; }

        for pkg in crate::package::FIRST_PARTY_PACKAGES {
            self.entries.push(PackageEntry {
                package_id: String::from(pkg.package_id),
                generation: 0,
                state: PackageState::Staged,
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
                .map(|entry| dep.requirement.matches(PackageVersion::new(entry.version.0, entry.version.1, entry.version.2)) && matches!(entry.state, PackageState::Staged | PackageState::Active))
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
    pub fn stage_verified(&mut self, manifest: &crate::package::RappManifest) -> Result<(), ()> {
        if self.entries.iter().any(|e| e.package_id == manifest.package_id && e.version == manifest.version) { return Ok(()); }
        if self.entries.len() >= MAX_ENTRIES { return Err(()); }
        self.generation = self.generation.saturating_add(1);
        self.entries.push(PackageEntry {
            package_id: manifest.package_id.clone(),
            generation: self.generation,
            state: PackageState::Staged,
            version: manifest.version,
        });
        if self.persist().is_err() { self.entries.pop(); return Err(()); }
        Ok(())
    }

    pub fn activate_version(&mut self, package_id: &str, version: (u16,u16,u16)) -> Result<(), ()> {
        let idx=self.entries.iter().position(|e| e.package_id==package_id).ok_or(())?;
        let previous=self.entries[idx].clone();
        if version < previous.version && previous.state == PackageState::Active {
            return Err(());
        }

        let old=self.entries.clone();
        let next_generation=self.generation.saturating_add(1);
        self.generation=next_generation;
        self.entries[idx]=PackageEntry {
            package_id: previous.package_id.clone(),
            generation:next_generation,
            state:PackageState::Active,
            version,
        };

        if self.persist().is_err() {
            self.entries=old;
            self.generation=self.entries.iter().map(|e| e.generation).max().unwrap_or(0);
            return Err(());
        }
        Ok(())
    }

    pub fn activate(&mut self, package_id: &str) -> Result<(), ()> {
        let version=self.entries.iter().find(|e| e.package_id==package_id).map(|e| e.version).ok_or(())?;
        self.activate_version(package_id, version)
    }

    pub fn rollback(&mut self, package_id: &str) -> Result<(), ()> {
        let idx=self.entries.iter().position(|e| e.package_id==package_id).ok_or(())?;
        let previous=self.entries[idx];
        let old=self.entries.clone();
        let next_generation=self.generation.saturating_add(1);
        self.generation=next_generation;
        self.entries[idx]=PackageEntry {
            package_id: previous.package_id.clone(),
            generation:next_generation,
            state:PackageState::RolledBack,
            version:previous.version,
        };

        if self.persist().is_err() {
            self.entries=old;
            self.generation=self.entries.iter().map(|e| e.generation).max().unwrap_or(0);
            return Err(());
        }
        Ok(())
    }


    pub fn launch_allowed(&self, package_id:&str, version:(u16,u16,u16))->bool {
        self.entries.iter().find(|e|e.package_id==package_id).map(|e| matches!(e.state,PackageState::Staged|PackageState::Active) && version>=e.version).unwrap_or(false)
    }

    pub fn entries(&self) -> &[PackageEntry] { &self.entries }
}

pub static PACKAGE_REGISTRY: Mutex<PackageRegistry> = Mutex::new(PackageRegistry::new());
