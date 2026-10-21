//! Package Management & Execution Subsystem
//!
//! Handles verified loading, cryptographic verification, and revocation
//! of software packages before execution.

pub mod rapp;
pub mod revocation;
pub mod registry;
pub mod store;
pub mod dependency;
pub mod transaction;

use alloc::vec;
use alloc::vec::Vec;

pub use rapp::{
    build_rapp, parse_rapp, RappArch, RappError, RappManifest, RAPP_FORMAT_VERSION, RAPP_HEADER_SIZE,
    RAPP_MAGIC,
};
pub use registry::{PackageRegistry, PackageState, PACKAGE_REGISTRY};
pub use store::{PackageBlobStore, PACKAGE_BLOB_STORE};
pub use dependency::{Dependency, PackagePolicy, PackageVersion, Permission, VersionRequirement};
pub use transaction::{InstallStage, InstallTransaction};
pub use revocation::{
    KeyRotationProof, RevocationReason, RevocationRecord, RevocationRegistry, REVOCATION_REGISTRY,
};


/// First-party package registry metadata used by the userspace package service.
#[derive(Debug, Clone, Copy)]
pub struct PackageDescriptor {
    pub package_id: &'static str,
    pub path: &'static str,
    pub process_name: &'static str,
    pub display_name: &'static str,
    pub icon: &'static str,
    pub category: &'static str,
    pub is_system: bool,
}

pub const FIRST_PARTY_PACKAGES: &[PackageDescriptor] = &[
    PackageDescriptor { package_id: "org.interos.compositor", path: "/packages/org.interos.compositor.rapp", process_name: "compositor", display_name: "Compositor", icon: "COMP", category: "Tools", is_system: true },
    PackageDescriptor { package_id: "org.interos.launcher", path: "/packages/org.interos.launcher.rapp", process_name: "launcher", display_name: "Launcher", icon: "HOME", category: "Tools", is_system: true },
    PackageDescriptor { package_id: "org.interos.settings", path: "/packages/org.interos.settings.rapp", process_name: "app_settings", display_name: "Settings", icon: "SET", category: "Tools", is_system: false },
    PackageDescriptor { package_id: "org.interos.terminal", path: "/packages/org.interos.terminal.rapp", process_name: "app_terminal", display_name: "Terminal", icon: "TERM", category: "Tools", is_system: false },
    PackageDescriptor { package_id: "org.interos.calculator", path: "/packages/org.interos.calculator.rapp", process_name: "app_calculator", display_name: "Calculator", icon: "CALC", category: "Tools", is_system: false },
    PackageDescriptor { package_id: "org.interos.dialer", path: "/packages/org.interos.dialer.rapp", process_name: "app_dialer", display_name: "Dialer", icon: "CALL", category: "Personal", is_system: false },
    PackageDescriptor { package_id: "org.interos.vault", path: "/packages/org.interos.vault.rapp", process_name: "app_vault", display_name: "Vault", icon: "VAULT", category: "Work", is_system: false },
    PackageDescriptor { package_id: "org.interos.camera", path: "/packages/org.interos.camera.rapp", process_name: "app_camera", display_name: "Camera", icon: "CAM", category: "Personal", is_system: false },
    PackageDescriptor { package_id: "org.interos.browser", path: "/packages/org.interos.browser.rapp", process_name: "app_browser", display_name: "Browser", icon: "WEB", category: "Personal", is_system: false },
    PackageDescriptor { package_id: "org.interos.messages", path: "/packages/org.interos.messages.rapp", process_name: "app_messages", display_name: "Messages", icon: "MSG", category: "Personal", is_system: false },
    PackageDescriptor { package_id: "org.interos.calendar", path: "/packages/org.interos.calendar.rapp", process_name: "app_calendar", display_name: "Calendar", icon: "CAL", category: "Work", is_system: false },
    PackageDescriptor { package_id: "org.interos.music", path: "/packages/org.interos.music.rapp", process_name: "app_music", display_name: "Music", icon: "MUS", category: "Personal", is_system: false },
    PackageDescriptor { package_id: "org.interos.file_manager", path: "/packages/org.interos.file_manager.rapp", process_name: "app_file_manager", display_name: "Files", icon: "FILE", category: "Work", is_system: false },
    PackageDescriptor { package_id: "org.interos.gallery", path: "/packages/org.interos.gallery.rapp", process_name: "app_gallery", display_name: "Gallery", icon: "PIC", category: "Personal", is_system: false },
    PackageDescriptor { package_id: "org.interos.notes", path: "/packages/org.interos.notes.rapp", process_name: "app_notes", display_name: "Notes", icon: "NOTE", category: "Personal", is_system: false },
];

pub fn first_party_package(package_id: &str) -> Option<&'static PackageDescriptor> {
    FIRST_PARTY_PACKAGES.iter().find(|pkg| pkg.package_id == package_id)
}

pub fn first_party_package_by_process_name(process_name: &str) -> Option<&'static PackageDescriptor> {
    FIRST_PARTY_PACKAGES.iter().find(|pkg| pkg.process_name == process_name)
}

/// What the first-party applications declare they can handle.
///
/// WHY this table and not a table inside one app: "who handles this file" is a
/// question about the device, and an answer that lives inside one of the
/// programs it is about is an answer that program can edit. Every entry here
/// is the same declaration the corresponding application's `manifest.json`
/// makes, and the content-intent contract test compares the two so they
/// cannot drift apart.
///
/// WHY the kinds are strings: the table is read by the contract test, and a
/// test that has to know the numeric encoding of a kind is a test that can be
/// broken by renumbering the ABI.
pub const FIRST_PARTY_INTENTS: &[(&str, &str, &str)] = &[
    // The gallery opens the images and videos the device holds.
    ("org.interos.gallery", "mime", "image/png"),
    ("org.interos.gallery", "mime", "image/jpeg"),
    ("org.interos.gallery", "mime", "image/gif"),
    ("org.interos.gallery", "mime", "image/bmp"),
    ("org.interos.gallery", "mime", "image/webp"),
    ("org.interos.gallery", "mime", "image/qoi"),
    ("org.interos.gallery", "mime", "image/x-portable-pixmap"),
    ("org.interos.gallery", "mime", "video/mp4"),
    ("org.interos.gallery", "mime", "video/webm"),
    ("org.interos.gallery", "mime", "video/quicktime"),
    ("org.interos.gallery", "mime", "video/x-matroska"),
    // The music app opens the audio the device holds.
    ("org.interos.music", "mime", "audio/mpeg"),
    ("org.interos.music", "mime", "audio/wav"),
    ("org.interos.music", "mime", "audio/ogg"),
    ("org.interos.music", "mime", "audio/flac"),
    ("org.interos.music", "mime", "audio/aac"),
    ("org.interos.music", "mime", "audio/mp4"),
    // The terminal opens the text the device holds.
    ("org.interos.terminal", "mime", "text/plain"),
    ("org.interos.terminal", "mime", "text/markdown"),
    ("org.interos.terminal", "mime", "text/csv"),
    ("org.interos.terminal", "mime", "application/json"),
    // The notes app opens the notes the device holds.
    ("org.interos.notes", "mime", "text/x-interos-note"),
    // The browser opens the web.
    ("org.interos.browser", "scheme", "http"),
    ("org.interos.browser", "scheme", "https"),
    // The settings app owns the wallpaper slot and the ringtone slot, so it is
    // the package that handles the actions that fill them.
    ("org.interos.settings", "action", "org.interos.action.set_wallpaper"),
    ("org.interos.settings", "action", "org.interos.action.set_lock_wallpaper"),
    ("org.interos.settings", "action", "org.interos.action.set_ringtone"),
];

/// The kind a declaration names.
fn intent_kind(name: &str) -> Option<crate::content::intent::IntentKind> {
    use crate::content::intent::IntentKind;
    match name {
        "mime" => Some(IntentKind::Mime),
        "scheme" => Some(IntentKind::Scheme),
        "action" => Some(IntentKind::Action),
        _ => None,
    }
}

/// Registers the first-party declarations with the device's intent registry.
pub fn seed_intent_registry() {
    use crate::content::intent::{IntentDeclaration, INTENT_REGISTRY};
    let mut grouped: Vec<(&'static str, Vec<IntentDeclaration>)> = Vec::new();
    for (package_id, kind, value) in FIRST_PARTY_INTENTS {
        let Some(kind) = intent_kind(kind) else { continue };
        match grouped.iter_mut().find(|(id, _)| id == package_id) {
            Some((_, declarations)) => {
                declarations.push(IntentDeclaration::new(kind, value));
            }
            None => {
                grouped.push((package_id, vec![IntentDeclaration::new(kind, value)]));
            }
        }
    }
    let mut registry = INTENT_REGISTRY.lock();
    let mut seeded = 0usize;
    let mut packages = 0usize;
    for (package_id, declarations) in grouped {
        seeded += declarations.len();
        packages += 1;
        registry.register(package_id, &declarations);
    }
    crate::serial_println!(
        "[CONTENT] Intent registry seeded with {} declared handler(s) from {} package(s).",
        seeded,
        packages
    );
}
