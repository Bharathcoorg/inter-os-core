//! P2P AMCP Package Marketplace & Attestation Protocol
//!
//! Provides decentralized discovery, developer signature verification,
//! zero-trust capability pre-flight auditing, and dynamic installation of
//! agent automation bundles.

pub mod catalog;
pub mod audit;
pub mod installer;
pub mod revocation;

use alloc::string::ToString;
use alloc::vec;
use crate::capability::CapabilityType;
use crate::package::{AmcpPackage, ResourceLimits};
use crate::serial_println;
pub use catalog::{MarketplaceCatalog, MarketplaceItem, MARKETPLACE_CATALOG, SOVEREIGN_DEV_KEY};
pub use audit::{audit_item, AuditVerdict};
pub use installer::{install_package, InstallError};
pub use revocation::{KeyRotationProof, RevocationReason, RevocationRecord, RevocationRegistry, REVOCATION_REGISTRY};

/// Initializes the P2P AMCP Marketplace subsystem and runs automated boot self-tests.
pub fn init() {
    serial_println!("--- [Initializing P2P AMCP Marketplace Subsystem] ---");

    // 1. Seed catalog with initial verified sovereign packages
    catalog::seed_initial_catalog();

    let count = MARKETPLACE_CATALOG.lock().list().len();
    serial_println!("[OK] Marketplace catalog online with {} verified sovereign automations.", count);

    // 2. Test: Developer Signature Verification
    {
        let catalog = MARKETPLACE_CATALOG.lock();
        let calc = catalog.get("org.interos.calculator").expect("Calculator package missing");
        let verdict = audit_item(calc);
        assert_eq!(verdict, AuditVerdict::Approved, "Calculator audit must be Approved");
        serial_println!("[TEST] Developer Signature Attestation: 'org.interos.calculator' VERIFIED.");
    }

    // 3. Test: Pre-flight Capability Auditing (Elevated Permission Consent)
    {
        let catalog = MARKETPLACE_CATALOG.lock();
        let browser = catalog.get("org.interos.browser").expect("Browser package missing");
        let verdict = audit_item(browser);
        assert_eq!(verdict, AuditVerdict::RequiresUserConsent, "Browser must require user consent for NetworkSend");
        serial_println!("[TEST] Zero-Trust Capability Audit: 'org.interos.browser' requires explicit consent (NetworkSend).");
    }

    // 4. Test: Rejection of Forbidden Hardware Capabilities
    {
        let rogue_caps = vec![CapabilityType::StorageRawBlock, CapabilityType::HardwarePortIo];
        let rogue_limits = ResourceLimits { max_cpu_ticks: 10_000, max_memory_bytes: 32_768 };
        let rogue_payload = b"ROGUE_HARDWARE_EXPLOIT_PAYLOAD";
        let rogue_bin = AmcpPackage::build_binary(
            "exploit.raw_disk_wiper",
            "did:inter:publisher:untrusted_adversary",
            (1, 0, 0),
            &rogue_caps,
            rogue_limits,
            rogue_payload,
        );
        let rogue_digest = crate::crypto::Sha256::digest(rogue_payload);
        let rogue_sig = AmcpPackage::compute_signature(
            "exploit.raw_disk_wiper",
            (1, 0, 0),
            &rogue_digest,
            SOVEREIGN_DEV_KEY,
        );

        let rogue_item = MarketplaceItem {
            package_id: "exploit.raw_disk_wiper".to_string(),
            name: "Rogue Hardware Wiper".to_string(),
            publisher: "did:inter:publisher:untrusted_adversary".to_string(),
            publisher_key: *SOVEREIGN_DEV_KEY,
            version: (1, 0, 0),
            description: "Attempted privilege escalation via raw block storage".to_string(),
            category: "Exploits".to_string(),
            declared_capabilities: rogue_caps,
            limits: rogue_limits,
            developer_signature: rogue_sig,
            raw_amcp: rogue_bin,
        };

        let verdict = audit_item(&rogue_item);
        assert_eq!(verdict, AuditVerdict::BlockedForbidden, "Rogue package with raw storage must be BLOCKED");
        serial_println!("[TEST] Zero-Trust Hardware Boundary: Forbidden capability 'StorageRawBlock' BLOCKED.");
    }

    // 5. Test: Tampered Developer Signature Rejection
    {
        let mut tampered_item = MARKETPLACE_CATALOG.lock().get("org.interos.calculator").unwrap().clone();
        tampered_item.developer_signature[0] ^= 0xFF; // Corrupt signature bit
        let verdict = audit_item(&tampered_item);
        assert_eq!(verdict, AuditVerdict::SignatureInvalid, "Tampered signature must be rejected");
        serial_println!("[TEST] Cryptographic Attestation: Tampered developer signature REJECTED.");
    }

    // 6. Test: Dynamic VFS Package Installation
    {
        let install_res = install_package("org.interos.browser", true);
        assert!(install_res.is_ok(), "Browser package installation failed");
        let installed_path = install_res.unwrap();
        assert_eq!(installed_path, "/packages/org.interos.browser.amcp");

        // Verify that file is readable from VFS
        let vfs = crate::fs::VFS.lock();
        let inode_id = vfs.lookup(&installed_path).expect("Installed package missing from VFS");
        let file_size = vfs.get_size(inode_id);
        assert!(file_size > 0, "Installed package file size must be > 0");
        serial_println!("[TEST] Dynamic Package Installation: 'org.interos.browser' deployed to VFS ({}: {} bytes).", installed_path, file_size);
    }

    // 7. Initialize Decentralized Package Revocation & Key Rotation
    revocation::init();

    serial_println!("--- [P2P AMCP Marketplace Subsystem 100% Attested & Enforced] ---");
}

/// Task 10: Marketplace Worker background thread
pub extern "C" fn task_marketplace_worker() -> ! {
    serial_println!("[TASK marketplace_worker (ID 10) LOG] [MARKETPLACE WORKER] P2P AMCP discovery daemon online. Advertising sovereign catalog...");

    let mut poll_count = 0u64;
    loop {
        poll_count = poll_count.wrapping_add(1);
        if poll_count % 30 == 0 {
            let count = MARKETPLACE_CATALOG.lock().list().len();
            serial_println!(
                "[TASK marketplace_worker (ID 10) LOG] [MARKETPLACE WORKER] Heartbeat: {} packages active in sovereign catalog.",
                count
            );
        }
        crate::task::yield_now();
    }
}
