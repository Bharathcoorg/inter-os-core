//! Kernel Crash Dump & State Rollback Engine
//!
//! Captures hardware processor fault registers (RIP, RSP, CR2, RFLAGS), faulting
//! Task ID, and diagnostic root cause in the event of an unrecoverable kernel or
//! user process abort. Maintains attested Merkle state checkpoints and orchestrates
//! automated cryptographic state rollback if a state transition causes a crash.

use alloc::vec::Vec;
use spin::Mutex;
use crate::crypto::{Sha256, SHA256_DIGEST_LENGTH};
use crate::storage::StorageError;

/// Magic 8-byte ASCII identifier for Inter OS crash dumps.
pub const CRASH_DUMP_MAGIC: [u8; 8] = *b"CRSHDMP1";

/// Hardware CPU and Merkle root snapshot preserved during a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct CrashDumpHeader {
    pub magic: [u8; 8],
    pub timestamp_secs: u64,
    pub task_id: u32,
    pub rip: u64,
    pub rsp: u64,
    pub cr2: u64,
    pub rflags: u64,
    pub error_code: u64,
    pub reason_len: u32,
    pub reason: [u8; 64],
    pub fault_merkle_root: [u8; SHA256_DIGEST_LENGTH],
    pub rollback_merkle_root: [u8; SHA256_DIGEST_LENGTH],
    pub checksum: [u8; SHA256_DIGEST_LENGTH],
}

impl CrashDumpHeader {
    /// Computes the SHA-256 digest of all header fields preceding the checksum.
    pub fn compute_checksum(&self) -> [u8; SHA256_DIGEST_LENGTH] {
        let mut hasher = Sha256::new();
        hasher.update(&self.magic);
        hasher.update(&self.timestamp_secs.to_le_bytes());
        hasher.update(&self.task_id.to_le_bytes());
        hasher.update(&self.rip.to_le_bytes());
        hasher.update(&self.rsp.to_le_bytes());
        hasher.update(&self.cr2.to_le_bytes());
        hasher.update(&self.rflags.to_le_bytes());
        hasher.update(&self.error_code.to_le_bytes());
        hasher.update(&self.reason_len.to_le_bytes());
        hasher.update(&self.reason);
        hasher.update(&self.fault_merkle_root);
        hasher.update(&self.rollback_merkle_root);
        hasher.finalize()
    }

    /// Verifies the cryptographic integrity of the crash dump header.
    pub fn is_valid(&self) -> bool {
        if self.magic != CRASH_DUMP_MAGIC {
            return false;
        }
        let expected = self.compute_checksum();
        expected == self.checksum
    }
}

/// Global Crash Dump and Merkle State Snapshot Coordinator.
pub struct CrashDumpManager {
    last_dump: Option<CrashDumpHeader>,
    checkpoint_history: Vec<[u8; SHA256_DIGEST_LENGTH]>,
}

impl CrashDumpManager {
    pub const fn new() -> Self {
        Self {
            last_dump: None,
            checkpoint_history: Vec::new(),
        }
    }

    /// Enqueues a verified Merkle state checkpoint into the rollback ring.
    pub fn record_checkpoint(&mut self, root: [u8; SHA256_DIGEST_LENGTH]) {
        if self.checkpoint_history.len() >= 16 {
            self.checkpoint_history.remove(0);
        }
        self.checkpoint_history.push(root);
    }

    /// Generates a validated crash dump capturing current machine fault registers
    /// and selects the most recent verified Merkle checkpoint for state rollback.
    pub fn create_crash_dump(
        &mut self,
        task_id: u32,
        rip: u64,
        rsp: u64,
        cr2: u64,
        rflags: u64,
        error_code: u64,
        reason_str: &str,
        current_root: [u8; SHA256_DIGEST_LENGTH],
    ) -> CrashDumpHeader {
        let mut reason_buf = [0u8; 64];
        let bytes = reason_str.as_bytes();
        let copy_len = bytes.len().min(64);
        reason_buf[..copy_len].copy_from_slice(&bytes[..copy_len]);

        // Find previous checkpoint for state recovery; if empty, use current root
        let rollback_root = self
            .checkpoint_history
            .iter()
            .rev()
            .find(|&&r| r != current_root)
            .copied()
            .unwrap_or(current_root);

        let timestamp_secs = crate::hal::RTC.lock().now().unix_epoch_secs;

        let mut header = CrashDumpHeader {
            magic: CRASH_DUMP_MAGIC,
            timestamp_secs,
            task_id,
            rip,
            rsp,
            cr2,
            rflags,
            error_code,
            reason_len: copy_len as u32,
            reason: reason_buf,
            fault_merkle_root: current_root,
            rollback_merkle_root: rollback_root,
            checksum: [0u8; SHA256_DIGEST_LENGTH],
        };

        header.checksum = header.compute_checksum();
        self.last_dump = Some(header);
        header
    }

    /// Validates an incoming crash dump and returns the attested rollback Merkle state root.
    pub fn restore_rollback_root(&self, header: &CrashDumpHeader) -> Result<[u8; SHA256_DIGEST_LENGTH], StorageError> {
        if !header.is_valid() {
            return Err(StorageError::IntegrityViolation);
        }
        Ok(header.rollback_merkle_root)
    }

    /// Returns the last recorded crash dump, if any.
    pub fn last_dump(&self) -> Option<CrashDumpHeader> {
        self.last_dump
    }
}

pub static CRASH_DUMP: Mutex<CrashDumpManager> = Mutex::new(CrashDumpManager::new());

/// Initializes the Kernel Crash Dump Subsystem and validates rollback logic.
pub fn init() {
    crate::serial_println!("--- [Initializing Kernel Crash Dump & State Rollback Engine] ---");

    let mut cdm = CRASH_DUMP.lock();

    // 1. Establish verified checkpoints
    let checkpoint_1: [u8; 32] = [0x11; 32];
    let checkpoint_2: [u8; 32] = [0x22; 32];
    cdm.record_checkpoint(checkpoint_1);
    cdm.record_checkpoint(checkpoint_2);

    // 2. Simulate task fault and generate crash dump
    let fault_root: [u8; 32] = [0x99; 32];
    let dump = cdm.create_crash_dump(
        9, // user_init task ID
        0x0000_7FFF_8000_1040,
        0x0000_7FFF_FFFF_E000,
        0x0000_0000_0000_0000,
        0x0202,
        0,
        "PageFault: Read violation on unmapped zero page",
        fault_root,
    );

    assert!(dump.is_valid(), "Crash dump header checksum validation must pass");
    assert_eq!(dump.magic, CRASH_DUMP_MAGIC, "Magic must match CRSHDMP1");

    // 3. Verify state rollback resolution
    let rollback = cdm.restore_rollback_root(&dump).expect("State rollback must succeed");
    assert_eq!(rollback, checkpoint_2, "Rollback must resolve to previous attested checkpoint");

    crate::serial_println!(
        "[TEST] Crash Dump Engine: Header magic 'CRSHDMP1' verified, SHA-256 integrity validated, state rollback to checkpoint resolved."
    );
}
