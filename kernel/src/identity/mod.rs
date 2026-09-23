//! Sovereign Identity & Cryptographic Vault Subsystem
//!
//! Exposes the device's decentralized identity (W3C DID), signs state commitments,
//! and issues zero-trust delegation tokens.

pub mod did;
pub mod vault;

pub use did::SovereignDid;
pub use vault::{DelegationToken, IdentityVault};

use spin::Mutex;
use crate::serial_println;
use crate::crypto::HmacSha256;

/// Global static instance of the Sovereign Identity Vault.
pub static IDENTITY_VAULT: Mutex<Option<IdentityVault>> = Mutex::new(None);

/// Sovereign Root Seed (32 bytes) used for bare-metal hardware bringup.
const SOVEREIGN_DEVICE_SEED: [u8; 32] = [
    0x49, 0x4E, 0x54, 0x45, 0x52, 0x5F, 0x4F, 0x53, // "INTER_OS"
    0x5F, 0x53, 0x4F, 0x56, 0x45, 0x52, 0x45, 0x49, // "_SOVEREI"
    0x47, 0x4E, 0x5F, 0x52, 0x4F, 0x4F, 0x54, 0x5F, // "GN_ROOT_"
    0x53, 0x45, 0x45, 0x44, 0x5F, 0x30, 0x30, 0x31, // "SEED_001"
];

/// Initializes the Sovereign Identity Vault and runs cryptographic self-tests.
pub fn init() {
    // 1. RFC 4231 HMAC-SHA256 Test Case 1 Verification
    let rfc_key = [0x0bu8; 20];
    let rfc_data = b"Hi There";
    let expected_rfc_tag: [u8; 32] = [
        0xb0, 0x34, 0x4c, 0x61, 0xd8, 0xdb, 0x38, 0x53,
        0x5c, 0xa8, 0xaf, 0xce, 0xaf, 0x0b, 0xf1, 0x2b,
        0x88, 0x1d, 0xc2, 0x00, 0xc9, 0x83, 0x3d, 0xa7,
        0x26, 0xe9, 0x37, 0x6c, 0x2e, 0x32, 0xcf, 0xf7,
    ];
    let computed_tag = HmacSha256::mac(&rfc_key, rfc_data);
    assert_eq!(computed_tag, expected_rfc_tag, "RFC 4231 HMAC-SHA256 test vector failed!");
    serial_println!("[TEST] RFC 4231 HMAC-SHA256 cryptographic MAC verified: B0344C61...");

    // 2. Initialize Vault
    let vault = IdentityVault::new(SOVEREIGN_DEVICE_SEED);
    let did_str = vault.device_did().as_str();
    serial_println!("[OK] Sovereign Identity Vault online. Device DID: {}", did_str);

    // 3. Test Signature Attestation
    let test_payload = b"Inter OS State Delta Seq: 0001";
    let signature = vault.sign(test_payload);
    assert!(vault.verify_signature(test_payload, &signature));
    assert!(!vault.verify_signature(b"Tampered payload", &signature));
    serial_println!("[TEST] Sovereign Identity Attestation: Sign & Verify PASSED (Tampering rejected).");

    // 4. Test Delegation Token Generation and Validation
    let mut vault_mut = vault;
    let token = vault_mut.issue_delegation_token(1, 0x07, 500, 100);
    assert!(vault_mut.verify_delegation_token(&token, 150));
    // Test expiration
    assert!(!vault_mut.verify_delegation_token(&token, 601), "Expired token must be rejected");
    serial_println!("[TEST] Sovereign Capability Delegation Token: Validated (Expiry enforced).");

    *IDENTITY_VAULT.lock() = Some(vault_mut);
    serial_println!("--- [Sovereign Identity Subsystem 100% Attested & Enforced] ---");
}
