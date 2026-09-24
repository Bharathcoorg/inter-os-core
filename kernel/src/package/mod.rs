//! Package Management & Execution Subsystem
//!
//! Handles verified loading, cryptographic verification, and revocation
//! of software packages before execution.

pub mod rapp;
pub mod revocation;

pub use rapp::{
    build_rapp, parse_rapp, RappArch, RappError, RappManifest, RAPP_FORMAT_VERSION, RAPP_HEADER_SIZE,
    RAPP_MAGIC,
};
pub use revocation::{
    KeyRotationProof, RevocationReason, RevocationRecord, RevocationRegistry, REVOCATION_REGISTRY,
};


/// First-party package registry metadata used by the userspace package service.
#[derive(Debug, Clone, Copy)]
pub struct PackageDescriptor {
    pub package_id: &'static str,
    pub path: &'static str,
    pub process_name: &'static str,
}

pub const FIRST_PARTY_PACKAGES: &[PackageDescriptor] = &[
    PackageDescriptor { package_id: "org.interos.settings", path: "/packages/org.interos.settings.rapp", process_name: "app_settings" },
    PackageDescriptor { package_id: "org.interos.terminal", path: "/packages/org.interos.terminal.rapp", process_name: "app_terminal" },
];

pub fn first_party_package(package_id: &str) -> Option<&'static PackageDescriptor> {
    FIRST_PARTY_PACKAGES.iter().find(|pkg| pkg.package_id == package_id)
}
