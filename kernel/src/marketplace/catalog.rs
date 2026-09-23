//! P2P AMCP Package Marketplace Catalog
//!
//! Maintains verified decentralized package metadata, publisher DIDs,
//! declared capabilities, and binary `.amcp` automation bundles.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use crate::capability::CapabilityType;
use crate::package::{AmcpPackage, ResourceLimits};

/// Catalog item representing an available automation package in the marketplace.
#[derive(Debug, Clone)]
pub struct MarketplaceItem {
    pub package_id: String,
    pub name: String,
    pub publisher: String,
    pub publisher_key: [u8; 32],
    pub version: (u16, u16, u16),
    pub description: String,
    pub category: String,
    pub declared_capabilities: Vec<CapabilityType>,
    pub limits: ResourceLimits,
    pub developer_signature: [u8; 32],
    pub raw_amcp: Vec<u8>,
}

/// In-memory catalog of peer-announced and sovereign marketplace automations.
pub struct MarketplaceCatalog {
    items: Vec<MarketplaceItem>,
}

impl MarketplaceCatalog {
    /// Creates an empty marketplace catalog.
    pub const fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Registers a new or updated package in the marketplace.
    pub fn register(&mut self, item: MarketplaceItem) {
        if let Some(pos) = self.items.iter().position(|i| i.package_id == item.package_id) {
            self.items[pos] = item;
        } else {
            self.items.push(item);
        }
    }

    /// Retrieves a package by its unique identifier.
    pub fn get(&self, package_id: &str) -> Option<&MarketplaceItem> {
        self.items.iter().find(|i| i.package_id == package_id)
    }

    /// Returns all available marketplace package listings.
    pub fn list(&self) -> &[MarketplaceItem] {
        &self.items
    }

    /// Searches listings by keyword across package ID, name, and description.
    pub fn search(&self, query: &str) -> Vec<&MarketplaceItem> {
        self.items
            .iter()
            .filter(|i| {
                i.package_id.contains(query)
                    || i.name.contains(query)
                    || i.description.contains(query)
                    || i.category.contains(query)
            })
            .collect()
    }
}

/// Global thread-safe P2P Marketplace catalog instance.
pub static MARKETPLACE_CATALOG: Mutex<MarketplaceCatalog> = Mutex::new(MarketplaceCatalog::new());

/// Known developer signing key for official sovereign automations.
pub const SOVEREIGN_DEV_KEY: &[u8; 32] = b"INTER_OS_SOVEREIGN_DEV_KEY_2026!";

/// Populates the catalog with pre-verified decentralized AMCP automations.
pub fn seed_initial_catalog() {
    let mut catalog = MARKETPLACE_CATALOG.lock();

    // 1. InterOS Touch Calculator (v1.0.0)
    let calc_caps = vec![CapabilityType::SerialOutput];
    let calc_limits = ResourceLimits { max_cpu_ticks: 15_000, max_memory_bytes: 65_536 };
    let calc_payload = b"\x00asm\x01\x00\x00\x00\x01\x04\x01\x60\x00\x00\x03\x02\x01\x00\x0a\x04\x01\x02\x00\x0b";
    let calc_bin = AmcpPackage::build_binary(
        "org.interos.calculator",
        "did:inter:publisher:interos_official",
        (1, 0, 0),
        &calc_caps,
        calc_limits,
        calc_payload,
    );
    let calc_digest = crate::crypto::Sha256::digest(calc_payload);
    let calc_sig = AmcpPackage::compute_signature(
        "org.interos.calculator",
        (1, 0, 0),
        &calc_digest,
        SOVEREIGN_DEV_KEY,
    );

    catalog.register(MarketplaceItem {
        package_id: "org.interos.calculator".to_string(),
        name: "InterOS Touch Calculator".to_string(),
        publisher: "did:inter:publisher:interos_official".to_string(),
        publisher_key: *SOVEREIGN_DEV_KEY,
        version: (1, 0, 0),
        description: "Standard open-source touch calculator for arithmetic calculations".to_string(),
        category: "Utilities".to_string(),
        declared_capabilities: calc_caps,
        limits: calc_limits,
        developer_signature: calc_sig,
        raw_amcp: calc_bin,
    });

    // 2. InterOS Developer Shell (v1.0.0)
    let term_caps = vec![CapabilityType::SerialOutput, CapabilityType::IpcSend];
    let term_limits = ResourceLimits { max_cpu_ticks: 20_000, max_memory_bytes: 65_536 };
    let term_payload = b"\x00asm\x01\x00\x00\x00INTEROS_TERMINAL_WASM_V1";
    let term_bin = AmcpPackage::build_binary(
        "org.interos.terminal",
        "did:inter:publisher:interos_official",
        (1, 0, 0),
        &term_caps,
        term_limits,
        term_payload,
    );
    let term_digest = crate::crypto::Sha256::digest(term_payload);
    let term_sig = AmcpPackage::compute_signature(
        "org.interos.terminal",
        (1, 0, 0),
        &term_digest,
        SOVEREIGN_DEV_KEY,
    );

    catalog.register(MarketplaceItem {
        package_id: "org.interos.terminal".to_string(),
        name: "InterOS Developer Shell".to_string(),
        publisher: "did:inter:publisher:interos_official".to_string(),
        publisher_key: *SOVEREIGN_DEV_KEY,
        version: (1, 0, 0),
        description: "Interactive command-line developer shell and kernel diagnostics".to_string(),
        category: "Development".to_string(),
        declared_capabilities: term_caps,
        limits: term_limits,
        developer_signature: term_sig,
        raw_amcp: term_bin,
    });

    // 3. InterOS Secure Web Browser (v1.0.0)
    let browser_caps = vec![CapabilityType::SerialOutput, CapabilityType::IpcSend, CapabilityType::NetworkSend];
    let browser_limits = ResourceLimits { max_cpu_ticks: 25_000, max_memory_bytes: 65_536 };
    let browser_payload = b"\x00asm\x01\x00\x00\x00INTEROS_BROWSER_WASM_V1";
    let browser_bin = AmcpPackage::build_binary(
        "org.interos.browser",
        "did:inter:publisher:interos_official",
        (1, 0, 0),
        &browser_caps,
        browser_limits,
        browser_payload,
    );
    let browser_digest = crate::crypto::Sha256::digest(browser_payload);
    let browser_sig = AmcpPackage::compute_signature(
        "org.interos.browser",
        (1, 0, 0),
        &browser_digest,
        SOVEREIGN_DEV_KEY,
    );

    catalog.register(MarketplaceItem {
        package_id: "org.interos.browser".to_string(),
        name: "InterOS Secure Web Browser".to_string(),
        publisher: "did:inter:publisher:interos_official".to_string(),
        publisher_key: *SOVEREIGN_DEV_KEY,
        version: (1, 0, 0),
        description: "Open-source privacy-first web browser with sandbox isolation".to_string(),
        category: "Internet".to_string(),
        declared_capabilities: browser_caps,
        limits: browser_limits,
        developer_signature: browser_sig,
        raw_amcp: browser_bin,
    });

    // 4. System Storage Cleaner & Trimmer (v1.0.1)
    let cleaner_caps = vec![CapabilityType::SerialOutput, CapabilityType::FileSystemRead, CapabilityType::FileSystemWrite];
    let cleaner_limits = ResourceLimits { max_cpu_ticks: 10_000, max_memory_bytes: 32_768 };
    let cleaner_payload = b"\x00asm\x01\x00\x00\x00SYSTEM_STORAGE_CLEANER_WASM_V1";
    let cleaner_bin = AmcpPackage::build_binary(
        "system.storage_cleaner",
        "did:inter:publisher:interos_core",
        (1, 0, 1),
        &cleaner_caps,
        cleaner_limits,
        cleaner_payload,
    );
    let cleaner_digest = crate::crypto::Sha256::digest(cleaner_payload);
    let cleaner_sig = AmcpPackage::compute_signature(
        "system.storage_cleaner",
        (1, 0, 1),
        &cleaner_digest,
        SOVEREIGN_DEV_KEY,
    );

    catalog.register(MarketplaceItem {
        package_id: "system.storage_cleaner".to_string(),
        name: "System Cache & Audit Log Trimmer".to_string(),
        publisher: "did:inter:publisher:interos_core".to_string(),
        publisher_key: *SOVEREIGN_DEV_KEY,
        version: (1, 0, 1),
        description: "Background maintenance routine cleaning expired cache inodes and audit traces".to_string(),
        category: "Utilities".to_string(),
        declared_capabilities: cleaner_caps,
        limits: cleaner_limits,
        developer_signature: cleaner_sig,
        raw_amcp: cleaner_bin,
    });
}
