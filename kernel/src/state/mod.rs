//! Operating System State Management & Cryptographic Delta Anchoring
//!
//! Subsystem for tracking, committing, and attesting OS state deltas.

pub mod commitment;
pub mod delta;
pub mod merkle;
pub mod manager;

pub use commitment::StateCommitment;
pub use delta::{StateChange, StateDelta, StateDomain};
pub use manager::StateManager;

use spin::Mutex;
use crate::identity::IDENTITY_VAULT;
use crate::serial_println;

/// Global static instance of the Central State Manager.
pub static STATE_MANAGER: Mutex<Option<StateManager>> = Mutex::new(None);

/// Initializes the State Management subsystem and runs self-tests.
pub fn init() {
    let mut state_mgr = StateManager::new();

    let vault_guard = IDENTITY_VAULT.lock();
    let vault = vault_guard.as_ref().expect("IdentityVault must be initialized before StateManager");

    // 1. Record state changes
    state_mgr.record_change(StateChange::new(
        StateDomain::Settings,
        "system.ui.theme",
        b"developer-dark",
    ));
    state_mgr.record_change(StateChange::new(
        StateDomain::CapabilityGrants,
        "task.0001.rights",
        &[0x07, 0x00, 0x00, 0x00],
    ));
    state_mgr.record_change(StateChange::new(
        StateDomain::AutomationProfiles,
        "package.calculator.version",
        b"1.0.0",
    ));

    assert_eq!(state_mgr.pending_count(), 3);

    // 2. Commit first state delta (Seq 1)
    let commitment = state_mgr
        .commit_pending(100, vault)
        .expect("Pending state changes must produce commitment");

    assert_eq!(commitment.sequence, 1);
    assert_eq!(state_mgr.history_len(), 1);
    assert!(commitment.verify(vault));
    serial_println!("[TEST] State Delta Commitment (Seq 1): Attested by DID '{}'.", commitment.device_did);

    // 3. Record second state delta (Seq 2)
    state_mgr.record_change(StateChange::new(
        StateDomain::ResourceQuotas,
        "storage.quota.user_data",
        &1024u32.to_le_bytes(),
    ));
    let commitment2 = state_mgr
        .commit_pending(200, vault)
        .expect("Pending state changes must produce second commitment");

    assert_eq!(commitment2.sequence, 2);
    assert_eq!(state_mgr.history_len(), 2);
    assert!(commitment2.verify(vault));

    // 4. End-to-end continuous chain verification
    assert!(state_mgr.verify_entire_chain(vault));
    serial_println!("[TEST] Merkle State Chain Integrity: 2 sequential commitments VERIFIED.");

    // 5. Tampering rejection test
    let mut tampered = commitment2.clone();
    tampered.new_state_root[0] ^= 0xFF;
    assert!(!tampered.verify(vault), "Tampered state root must fail verification");
    serial_println!("[TEST] State Chain Tampering Attestation: Corruption REJECTED.");

    drop(vault_guard);
    *STATE_MANAGER.lock() = Some(state_mgr);

    serial_println!("--- [State Management Subsystem 100% Attested & Enforced] ---");
}
