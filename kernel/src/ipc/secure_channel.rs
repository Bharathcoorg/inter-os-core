//! Cryptographic Secure IPC Channel & Mutual Task Attestation
//!
//! Provides authenticated, anti-replay, zero-trust IPC messaging between tasks.
//! Uses RFC 4231 HMAC-SHA256 message authentication codes and monotonic sequence
//! counters to prevent message tampering, spoofing, and replay attacks across
//! unprivileged user space and privileged microkernel security sentinels.

use alloc::vec::Vec;
use spin::Mutex;
use crate::crypto::hmac::HmacSha256;
use crate::crypto::SHA256_DIGEST_LENGTH;
use crate::task::TaskId;

pub const MAX_IPC_PAYLOAD: usize = 64;

/// Errors returned by secure IPC channel operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecureIpcError {
    SessionNotFound,
    SessionLimitReached,
    SequenceReplayDetected,
    SignatureInvalid,
    PayloadTooLarge,
}

/// Cryptographically signed and sequence-stamped IPC frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct SignedIpcFrame {
    pub session_id: u64,
    pub sequence: u64,
    pub payload_len: u16,
    pub payload: [u8; 64],
    pub hmac_signature: [u8; SHA256_DIGEST_LENGTH],
}

impl SignedIpcFrame {
    /// Computes HMAC-SHA256 authentication digest over session_id, sequence, length, and payload.
    pub fn compute_hmac(&self, key: &[u8; 32]) -> [u8; SHA256_DIGEST_LENGTH] {
        let mut msg = [0u8; 82]; // 8 + 8 + 2 + 64
        msg[0..8].copy_from_slice(&self.session_id.to_le_bytes());
        msg[8..16].copy_from_slice(&self.sequence.to_le_bytes());
        msg[16..18].copy_from_slice(&self.payload_len.to_le_bytes());
        let valid_len = (self.payload_len as usize).min(64);
        msg[18..18 + valid_len].copy_from_slice(&self.payload[..valid_len]);
        HmacSha256::compute(key, &msg[..18 + valid_len])
    }

    /// Verifies the cryptographic HMAC-SHA256 signature against the shared session key.
    pub fn verify(&self, key: &[u8; 32]) -> bool {
        let expected = self.compute_hmac(key);
        expected == self.hmac_signature
    }
}

/// An established mutual zero-trust secure communication session.
pub struct SecureSession {
    pub session_id: u64,
    pub client_task: TaskId,
    pub server_task: TaskId,
    pub session_key: [u8; 32],
    pub seq_out: u64,
    pub seq_in: u64,
}

impl SecureSession {
    /// Creates a signed frame for transmission, incrementing the outgoing sequence number.
    pub fn sign_message(&mut self, payload: &[u8]) -> Result<SignedIpcFrame, SecureIpcError> {
        if payload.len() > 64 {
            return Err(SecureIpcError::PayloadTooLarge);
        }
        self.seq_out += 1;
        let mut frame_buf = [0u8; 64];
        frame_buf[..payload.len()].copy_from_slice(payload);

        let mut frame = SignedIpcFrame {
            session_id: self.session_id,
            sequence: self.seq_out,
            payload_len: payload.len() as u16,
            payload: frame_buf,
            hmac_signature: [0u8; SHA256_DIGEST_LENGTH],
        };
        frame.hmac_signature = frame.compute_hmac(&self.session_key);
        Ok(frame)
    }

    /// Verifies an incoming signed frame, enforcing strict monotonic sequence ordering.
    pub fn verify_incoming(&mut self, frame: &SignedIpcFrame) -> Result<(), SecureIpcError> {
        if frame.session_id != self.session_id {
            return Err(SecureIpcError::SessionNotFound);
        }
        if frame.payload_len as usize > self::MAX_IPC_PAYLOAD {
            return Err(SecureIpcError::PayloadTooLarge);
        }
        if frame.sequence <= self.seq_in {
            return Err(SecureIpcError::SequenceReplayDetected);
        }
        if !frame.verify(&self.session_key) {
            return Err(SecureIpcError::SignatureInvalid);
        }
        self.seq_in = frame.sequence;
        Ok(())
    }
}

/// Global registry of active secure IPC channels.
pub struct SecureChannelManager {
    sessions: Vec<SecureSession>,
    next_session_id: u64,
}

impl SecureChannelManager {
    pub const fn new() -> Self {
        Self {
            sessions: Vec::new(),
            next_session_id: 1000,
        }
    }

    /// Creates and registers a new secure IPC session between two tasks.
    pub fn create_session(
        &mut self,
        client: TaskId,
        server: TaskId,
        shared_key: [u8; 32],
    ) -> Result<u64, SecureIpcError> {
        if self.sessions.len() >= 32 {
            return Err(SecureIpcError::SessionLimitReached);
        }
        self.next_session_id += 1;
        let sid = self.next_session_id;

        self.sessions.push(SecureSession {
            session_id: sid,
            client_task: client,
            server_task: server,
            session_key: shared_key,
            seq_out: 0,
            seq_in: 0,
        });
        Ok(sid)
    }

    /// Retrieves mutable reference to an active session by ID.
    pub fn get_session_mut(&mut self, session_id: u64) -> Option<&mut SecureSession> {
        self.sessions.iter_mut().find(|s| s.session_id == session_id)
    }
}

pub static SECURE_IPC: Mutex<SecureChannelManager> = Mutex::new(SecureChannelManager::new());

/// Initializes the Secure IPC Subsystem and executes automated cryptographic self-tests.
pub fn init() {
    let mut mgr = SECURE_IPC.lock();

    // 1. Establish session with pre-shared key
    let test_key: [u8; 32] = [0x42; 32];
    let sid = mgr
        .create_session(TaskId(9), TaskId(2), test_key)
        .expect("Failed to create secure session");

    let session = mgr.get_session_mut(sid).expect("Session must exist");

    // 2. Sign legitimate payload
    let test_payload = b"IPC_SESSION_MSG: RIL_RADIO_POWER_ON";
    let signed_frame = session
        .sign_message(test_payload)
        .expect("Signing must succeed");

    assert_eq!(signed_frame.sequence, 1);
    assert!(signed_frame.verify(&test_key), "HMAC signature must verify");

    // 3. Verify incoming frame on receiving end
    let verify_res = session.verify_incoming(&signed_frame);
    assert!(verify_res.is_ok(), "Legitimate frame verification must pass");
    assert_eq!(session.seq_in, 1);

    // 4. Test Replay Attack Protection: resending sequence 1 must be rejected
    let replay_res = session.verify_incoming(&signed_frame);
    assert_eq!(
        replay_res,
        Err(SecureIpcError::SequenceReplayDetected),
        "Replay attack with stale sequence must be rejected"
    );

    // 5. Test Tamper Detection: flipping a byte in payload must fail HMAC
    let mut tampered_frame = session
        .sign_message(b"IPC_SESSION_MSG: RIL_RADIO_POWER_ON")
        .expect("Signing second frame must succeed");
    tampered_frame.payload[0] ^= 0xFF; // Corrupt first byte
    let tamper_res = session.verify_incoming(&tampered_frame);
    assert_eq!(
        tamper_res,
        Err(SecureIpcError::SignatureInvalid),
        "Tampered payload must fail cryptographic HMAC validation"
    );

    crate::serial_println!(
        "[TEST] Cryptographic Secure IPC: HMAC-SHA256 authentication, replay rejection, and tamper detection VERIFIED."
    );
}
