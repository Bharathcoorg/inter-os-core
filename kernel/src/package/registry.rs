use alloc::vec::Vec;
use spin::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageState { Staged, Active, RolledBack }

#[derive(Debug, Clone, Copy)]
pub struct PackageEntry {
    pub package_id: &'static str,
    pub generation: u64,
    pub state: PackageState,
}

pub struct PackageRegistry {
    entries: Vec<PackageEntry>,
    generation: u64,
}

impl PackageRegistry {
    pub const fn new() -> Self { Self { entries: Vec::new(), generation: 0 } }

    pub fn seed(&mut self) {
        if !self.entries.is_empty() { return; }
        for pkg in crate::package::FIRST_PARTY_PACKAGES {
            self.entries.push(PackageEntry { package_id: pkg.package_id, generation: 0, state: PackageState::Staged });
        }
    }

    pub fn activate(&mut self, package_id: &str) -> Result<(), ()> {
        let idx=self.entries.iter().position(|e| e.package_id==package_id).ok_or(())?;
        let previous=self.entries[idx];
        self.generation=self.generation.saturating_add(1);
        self.entries[idx]=PackageEntry { package_id: previous.package_id, generation:self.generation, state:PackageState::Active };
        Ok(())
    }

    pub fn rollback(&mut self, package_id: &str) -> Result<(), ()> {
        let idx=self.entries.iter().position(|e| e.package_id==package_id).ok_or(())?;
        let previous=self.entries[idx];
        self.generation=self.generation.saturating_add(1);
        self.entries[idx]=PackageEntry { package_id: previous.package_id, generation:self.generation, state:PackageState::RolledBack };
        Ok(())
    }

    pub fn entries(&self) -> &[PackageEntry] { &self.entries }
}

pub static PACKAGE_REGISTRY: Mutex<PackageRegistry> = Mutex::new(PackageRegistry::new());
