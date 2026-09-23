//! LiteVerse Optional Decentralized Network Participation Subsystem
//!
//! Non-Negotiable Principle #15: LiteVerse participation is optional and resource-limited.
//! Non-Negotiable Principle #16: Private user data must never be mixed with LiteVerse shared storage.
//! Non-Negotiable Principle #8: Foreground user workloads have priority over LiteVerse.

pub mod config;
pub mod node;
pub mod storage;

pub use config::LiteVerseConfig;
pub use node::{LiteVerseNode, LiteVerseState};
pub use storage::{LiteVerseStorageError, SharedStoragePool};

use spin::Mutex;
use crate::crypto::Sha256;
use crate::serial_println;

/// Global static instance of the LiteVerse Node.
pub static LITEVERSE_NODE: Mutex<Option<LiteVerseNode>> = Mutex::new(None);

/// Initializes the LiteVerse subsystem and runs self-tests.
pub fn init() {
    // 1. Principle #15: Verify disabled by default
    let default_cfg = LiteVerseConfig::default_disabled();
    assert!(!default_cfg.enabled, "LiteVerse must be disabled by default");
    serial_println!("[TEST] LiteVerse Opt-In Policy: Participation disabled by default (Principle #15).");

    // 2. Configure opt-in node with 16 KiB storage quota
    let quota_bytes = 16 * 1024;
    let enabled_cfg = LiteVerseConfig::enabled_with_quota(quota_bytes);
    let mut node = LiteVerseNode::new(enabled_cfg);
    assert_eq!(node.state, LiteVerseState::Active);

    // 3. Principle #16: Storage Isolation & Namespace Boundary Test
    let valid_path = "/liteverse/shared/gravity_state_block_001.bin";
    let block_data = b"LiteVerse decentralized state delta relay chunk #001";
    assert!(node.relay_payload(valid_path, block_data).is_ok());
    assert_eq!(node.storage.used_bytes(), block_data.len());
    serial_println!("[TEST] LiteVerse Storage Relay: Stored within '/liteverse/shared/' sandbox.");

    // Boundary violation: attempts to write to private host OS paths
    let private_leak_attempt = node.relay_payload("/data/private_keys.json", b"LEAKED_KEY");
    assert_eq!(private_leak_attempt, Err(LiteVerseStorageError::IsolationViolation));

    let system_leak_attempt = node.relay_payload("/system/etc/policy.json", b"TAMPER");
    assert_eq!(system_leak_attempt, Err(LiteVerseStorageError::IsolationViolation));

    let traversal_attempt = node.relay_payload("/liteverse/shared/../../data/leak.txt", b"TRAVERSAL");
    assert_eq!(traversal_attempt, Err(LiteVerseStorageError::IsolationViolation));
    serial_println!("[TEST] LiteVerse Storage Isolation: Private OS partition access BLOCKED (Principle #16).");

    // 4. Quota Enforcement Test
    let oversize_data = [0xAAu8; 17 * 1024]; // 17 KiB > 16 KiB quota
    let quota_fail = node.relay_payload("/liteverse/shared/huge_blob.bin", &oversize_data);
    assert_eq!(quota_fail, Err(LiteVerseStorageError::QuotaExceeded));
    serial_println!("[TEST] LiteVerse Storage Quota: Allocation exceeding 16 KiB limit REJECTED.");

    // 5. Distributed Block Verification Test
    let sample_block = b"LiteVerse Off-Chain Block #4096 Header";
    let sample_root = Sha256::digest(sample_block);
    assert!(node.verify_block(sample_block, &sample_root));
    assert!(!node.verify_block(b"Corrupted Block", &sample_root));
    assert_eq!(node.blocks_verified, 1);
    serial_println!("[TEST] LiteVerse Block Verification: Off-chain Merkle root validated.");

    // 6. Principle #8: Subordinate Preemption Test
    node.step(false); // Resource manager signals preemption/throttling
    assert_eq!(node.state, LiteVerseState::Paused);
    node.step(true);  // Resources available
    assert_eq!(node.state, LiteVerseState::Active);
    serial_println!("[TEST] LiteVerse Resource Subordination: Paused immediately on preemption (Principle #8).");

    *LITEVERSE_NODE.lock() = Some(node);
    serial_println!("--- [LiteVerse Subsystem 100% Isolated & Attested] ---");
}
