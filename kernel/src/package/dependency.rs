//! Package dependency and permission contracts.

use alloc::vec::Vec;
use alloc::string::String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageVersion { pub major:u16, pub minor:u16, pub patch:u16 }
impl PackageVersion { pub const fn new(major:u16,minor:u16,patch:u16)->Self{Self{major,minor,patch}} }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionRequirement { Exact(PackageVersion), Minimum(PackageVersion), Major(u16) }
 impl VersionRequirement {
  pub fn matches(&self, v: PackageVersion) -> bool {
    match self {
      Self::Exact(x) => v == *x,
      Self::Minimum(x) => v.major > x.major || (v.major == x.major && (v.minor > x.minor || (v.minor == x.minor && v.patch >= x.patch))),
      Self::Major(m) => v.major == *m,
    }
  }
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

impl PackagePolicy {
    /// Constructs a `PackagePolicy` from declared canonical RAPP v2 capability IDs.
    ///
    /// # Security Boundary Distinction
    ///
    /// `PackagePolicy.permissions` represents a package-level consent and dependency
    /// audit surface (e.g. for user consent screens, installer metadata inspection,
    /// and dependency validation during package staging).
    ///
    /// It is **not** an authorization gate for microkernel syscalls. Actual runtime
    /// syscall authorization is enforced strictly and exclusively by the per-task
    /// [`crate::capability::CapabilityTable`] populated via [`crate::task::user::capabilities_from_rapp`].
    pub fn from_rapp_capabilities(capabilities: &[u16]) -> Self {
        let mut permissions = Vec::new();
        for cap in capabilities {
            // Reconciled with canonical RAPP v2 specification (docs/RAPP_FORMAT.md,
            // CapabilityType::from_raw, and crate::task::user::capabilities_from_rapp):
            let permission = match *cap {
                9 => Some(Permission::FileRead),
                10 => Some(Permission::FileWrite),
                12 | 13 | 14 => Some(Permission::Network),
                20 => Some(Permission::Telephony),
                21 => Some(Permission::Microphone),
                22 => Some(Permission::Camera),
                23 => Some(Permission::Sensors),
                _ => None,
            };
            if let Some(p) = permission {
                if !permissions.contains(&p) {
                    permissions.push(p);
                }
            }
        }
        Self { dependencies: Vec::new(), permissions }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rapp_capabilities_table_driven_mapping() {
        // Table of all canonical RAPP v2 capability IDs (1..=24) and their expected
        // high-level consent permissions.
        let cases: &[(u16, Option<Permission>)] = &[
            (1, None),                            // SerialOutput
            (2, None),                            // FrameBufferDirect
            (3, None),                            // TimerTickRead
            (4, None),                            // HardwarePortIo
            (5, None),                            // IpcSend
            (6, None),                            // IpcReceive
            (7, None),                            // ProcessSpawn
            (8, None),                            // ProcessKill
            (9, Some(Permission::FileRead)),       // FileSystemRead
            (10, Some(Permission::FileWrite)),     // FileSystemWrite
            (11, None),                           // StorageRawBlock
            (12, Some(Permission::Network)),      // NetworkSocket
            (13, Some(Permission::Network)),      // NetworkSend
            (14, Some(Permission::Network)),      // NetworkReceive
            (15, None),                           // ShmProducer
            (16, None),                           // ShmConsumer
            (17, None),                           // EventPoll
            (18, None),                           // SystemControl (Global)
            (19, None),                           // SystemControl (Power)
            (20, Some(Permission::Telephony)),    // SystemControl (Telephony)
            (21, Some(Permission::Microphone)),   // SystemControl (Audio)
            (22, Some(Permission::Camera)),       // SystemControl (Camera)
            (23, Some(Permission::Sensors)),      // SystemControl (Sensors)
            (24, None),                           // RealtimeClockRead
            // Unknown / out-of-bounds IDs
            (0, None),
            (25, None),
            (999, None),
        ];

        for &(id, expected) in cases {
            let policy = PackagePolicy::from_rapp_capabilities(&[id]);
            match expected {
                Some(perm) => {
                    assert_eq!(policy.permissions, alloc::vec![perm], "ID {} must map to {:?}", id, perm);
                    assert!(policy.requires(perm), "policy must require {:?}", perm);
                }
                None => {
                    assert!(policy.permissions.is_empty(), "ID {} must produce no permission, got {:?}", id, policy.permissions);
                }
            }
        }
    }

    #[test]
    fn test_rapp_capabilities_deduplication_and_multiple() {
        // IDs 12, 13, 14 all map to Permission::Network; policy must deduplicate
        let policy = PackagePolicy::from_rapp_capabilities(&[12, 13, 14]);
        assert_eq!(policy.permissions.len(), 1);
        assert_eq!(policy.permissions[0], Permission::Network);

        // Combined set of IDs
        let policy = PackagePolicy::from_rapp_capabilities(&[9, 10, 13, 20, 21, 22, 23]);
        assert_eq!(policy.permissions.len(), 7);
        assert!(policy.requires(Permission::FileRead));
        assert!(policy.requires(Permission::FileWrite));
        assert!(policy.requires(Permission::Network));
        assert!(policy.requires(Permission::Telephony));
        assert!(policy.requires(Permission::Microphone));
        assert!(policy.requires(Permission::Camera));
        assert!(policy.requires(Permission::Sensors));
        assert!(!policy.requires(Permission::Location));
        assert!(!policy.requires(Permission::Notifications));
        assert!(!policy.requires(Permission::Bluetooth));
    }
}

