// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Atomic package installation transaction state.

use crate::package::registry::PackageState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallStage { Prepared, Staged, Activated, RolledBack }

#[derive(Debug, Clone, Copy)]
pub struct InstallTransaction { pub package_index:u32, pub previous:PackageState, pub target:PackageState, pub stage:InstallStage, pub generation:u64 }
impl InstallTransaction {
 pub const fn new(package_index:u32,previous:PackageState,generation:u64)->Self{Self{package_index,previous,target:PackageState::Staged,stage:InstallStage::Prepared,generation}}
 pub fn stage(&mut self){self.stage=InstallStage::Staged;}
 pub fn activate(&mut self){if matches!(self.stage,InstallStage::Staged){self.target=PackageState::Active;self.stage=InstallStage::Activated;}}
 pub fn rollback(&mut self){self.target=self.previous;self.stage=InstallStage::RolledBack;}
 pub fn committed(&self)->bool{matches!(self.stage,InstallStage::Activated)}
}
