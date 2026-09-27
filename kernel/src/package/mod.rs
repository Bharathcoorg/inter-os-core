//! Package Management & Execution Subsystem
//!
//! Handles verified loading, cryptographic verification, and revocation
//! of software packages before execution.

pub mod rapp;
pub mod revocation;
pub mod registry;
pub mod store;

pub use rapp::{
    build_rapp, parse_rapp, RappArch, RappError, RappManifest, RAPP_FORMAT_VERSION, RAPP_HEADER_SIZE,
    RAPP_MAGIC,
};
pub use registry::{PackageRegistry, PackageState, PACKAGE_REGISTRY};
pub use store::{PackageBlobStore, PACKAGE_BLOB_STORE};
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
    PackageDescriptor { package_id: "org.interos.compositor", path: "/packages/org.interos.compositor.rapp", process_name: "compositor" },
    PackageDescriptor { package_id: "org.interos.launcher", path: "/packages/org.interos.launcher.rapp", process_name: "launcher" },
    PackageDescriptor { package_id: "org.interos.settings", path: "/packages/org.interos.settings.rapp", process_name: "app_settings" },
    PackageDescriptor { package_id: "org.interos.terminal", path: "/packages/org.interos.terminal.rapp", process_name: "app_terminal" },
    PackageDescriptor { package_id: "org.interos.calculator", path: "/packages/org.interos.calculator.rapp", process_name: "app_calculator" },
    PackageDescriptor { package_id: "org.interos.dialer", path: "/packages/org.interos.dialer.rapp", process_name: "app_dialer" },
    PackageDescriptor { package_id: "org.interos.vault", path: "/packages/org.interos.vault.rapp", process_name: "app_vault" },
    PackageDescriptor { package_id: "org.interos.camera", path: "/packages/org.interos.camera.rapp", process_name: "app_camera" },
    PackageDescriptor { package_id: "org.interos.browser", path: "/packages/org.interos.browser.rapp", process_name: "app_browser" },
    PackageDescriptor { package_id: "org.interos.messages", path: "/packages/org.interos.messages.rapp", process_name: "app_messages" },
    PackageDescriptor { package_id: "org.interos.calendar", path: "/packages/org.interos.calendar.rapp", process_name: "app_calendar" },
    PackageDescriptor { package_id: "org.interos.music", path: "/packages/org.interos.music.rapp", process_name: "app_music" },
    PackageDescriptor { package_id: "org.interos.file_manager", path: "/packages/org.interos.file_manager.rapp", process_name: "app_file_manager" },
    PackageDescriptor { package_id: "org.interos.gallery", path: "/packages/org.interos.gallery.rapp", process_name: "app_gallery" },
];

pub fn first_party_package(package_id: &str) -> Option<&'static PackageDescriptor> {
    FIRST_PARTY_PACKAGES.iter().find(|pkg| pkg.package_id == package_id)
}
