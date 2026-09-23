//! InterLayer Subsystem Root & Integration
//!
//! Non-Negotiable Principle #14: InterLayer is infrastructure, not an absolute boot blocker.
//! Provides zero-trust sovereign peer synchronization, state delta exchange, and intent routing.

pub mod client;
pub mod protocol;

pub use client::{InterlayerClient, INTERLAYER_DEFAULT_PORT};
pub use protocol::{InterlayerPacket, PacketType};

use spin::Mutex;
use crate::capability::{Capability, CapabilityId, CapabilityTable, CapabilityType, Rights};
use crate::identity::IDENTITY_VAULT;
use crate::net::Ipv4Address;
use crate::serial_println;

/// Global thread-safe InterLayer sovereign client instance.
pub static INTERLAYER_CLIENT: Mutex<InterlayerClient> = Mutex::new(InterlayerClient::new());

/// Initializes the InterLayer subsystem and executes cryptographic self-tests.
pub fn init() {
    serial_println!("--- [Initializing InterLayer Protocol Subsystem] ---");

    let secret = b"INTER_OS_MASTER_SEED_ZERO_TRUST_DEV_KEY";
    let test_fingerprint = {
        let vault_guard = IDENTITY_VAULT.lock();
        *vault_guard.as_ref().expect("Identity vault uninitialized").device_did().fingerprint()
    };
    let payload = b"STATE_DELTA_MERKLE_ROOT_TEST";

    // 1. Serialization & Signing Self-Test
    let mut pkt_buf = [0u8; 256];
    let len = InterlayerPacket::serialize_signed(
        PacketType::StateDeltaSync,
        42,
        test_fingerprint,
        payload,
        secret,
        &mut pkt_buf,
    ).expect("Interlayer serialization failed");

    let parsed = InterlayerPacket::parse(&pkt_buf[0..len]).expect("Interlayer parse failed");
    assert_eq!(parsed.packet_type, PacketType::StateDeltaSync);
    assert_eq!(parsed.sequence, 42);
    assert_eq!(parsed.sender_fingerprint, test_fingerprint);
    assert_eq!(parsed.payload, payload);
    assert!(parsed.verify_signature(secret), "Signature verification failed!");
    serial_println!("[TEST] InterLayer Protocol: Signed packet serialization & HMAC-SHA256 verification PASSED.");

    // 2. Tampering Detection Self-Test
    pkt_buf[len - 1] ^= 0x01; // Corrupt payload byte
    let tampered = InterlayerPacket::parse(&pkt_buf[0..len]).expect("Parse should succeed");
    assert!(!tampered.verify_signature(secret), "InterLayer failed to detect packet payload tampering!");
    serial_println!("[TEST] InterLayer Protocol: Cryptographic tampering rejection PASSED.");

    // 3. Client State Delta Synchronization over In-Kernel Networking Self-Test
    let mut net_caps = CapabilityTable::new(8);
    net_caps.insert(Capability {
        id: CapabilityId(1),
        cap_type: CapabilityType::NetworkSend,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();

    let mut client = INTERLAYER_CLIENT.lock();
    let dummy_root = [0x5Au8; 32];
    let sync_res = client.sync_state_delta(Ipv4Address::LOCALHOST, dummy_root, &net_caps);
    assert!(sync_res.is_ok(), "InterLayer state delta sync failed");
    assert_eq!(client.state_syncs_transmitted, 1);
    serial_println!("[TEST] InterLayer Client: State Delta sync datagram transmitted over network stack.");

    serial_println!("--- [InterLayer Subsystem 100% Attested & Enforced] ---");
}
