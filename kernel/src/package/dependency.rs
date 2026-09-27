//! Package dependency and permission contracts.

use alloc::vec::Vec;
use alloc::string::String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageVersion { pub major:u16, pub minor:u16, pub patch:u16 }
impl PackageVersion { pub const fn new(major:u16,minor:u16,patch:u16)->Self{Self{major,minor,patch}} }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionRequirement { Exact(PackageVersion), Minimum(PackageVersion), Major(u16) }
impl VersionRequirement {
 pub fn matches(&self,v:PackageVersion)->bool{match self{Self::Exact(x)=>v==*x,Self::Minimum(x)=>v.major>*x.major||(v.major==x.major&&(v.minor>x.minor||(v.minor==x.minor&&v.patch>=x.patch))),Self::Major(m)=>v.major==*m}}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency { pub package_id:String, pub requirement:VersionRequirement }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission { FileRead, FileWrite, Network, Camera, Microphone, Location, Notifications, Bluetooth, Telephony, Sensors }

#[derive(Debug, Clone)]
pub struct PackagePolicy { pub dependencies:Vec<Dependency>, pub permissions:Vec<Permission> }
impl PackagePolicy {
 pub fn validate(&self)->bool { self.permissions.len()<=32 && self.dependencies.len()<=32 && self.dependencies.iter().all(|d|!d.package_id.is_empty()) }
 pub fn requires(&self,p:Permission)->bool{self.permissions.iter().any(|x|*x==p)}
}
