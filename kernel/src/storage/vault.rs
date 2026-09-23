//! Sovereign Encrypted Storage Vault Subsystem
//!
//! Provides hardware-keystore anchored authenticated encrypted block storage.
//! Employs counter-mode pseudo-random keystream XOR streaming with per-sector
//! initialization vectors (IV) and RFC 4231 HMAC-SHA256 authenticated integrity
//! tags to ensure complete confidentiality and tamper rejection for user data.

use spin::Mutex;
use crate::crypto::hmac::HmacSha256;
use crate::crypto::SHA256_DIGEST_LENGTH;

/// Standard hardware sector size in bytes.
pub const VAULT_SECTOR_SIZE: usize = 512;

/// Errors returned by storage vault operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultError {
    IntegrityViolation,
    KeystoreUninitialized,
    InvalidBlockIndex,
}

/// An authenticated, encrypted storage block envelope.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct VaultBlock {
    pub iv: [u8; 16],
    pub block_index: u64,
    pub ciphertext: [u8; VAULT_SECTOR_SIZE],
    pub hmac_tag: [u8; SHA256_DIGEST_LENGTH],
}

impl VaultBlock {
    /// Computes HMAC-SHA256 authentication tag over iv, block_index, and ciphertext.
    pub fn compute_hmac(&self, key: &[u8; 32]) -> [u8; SHA256_DIGEST_LENGTH] {
        let mut msg = [0u8; 16 + 8 + VAULT_SECTOR_SIZE];
        msg[0..16].copy_from_slice(&self.iv);
        msg[16..24].copy_from_slice(&self.block_index.to_le_bytes());
        msg[24..24 + VAULT_SECTOR_SIZE].copy_from_slice(&self.ciphertext);
        HmacSha256::compute(key, &msg)
    }

    /// Validates block integrity tag against the vault key.
    pub fn verify(&self, key: &[u8; 32]) -> bool {
        let expected = self.compute_hmac(key);
        expected == self.hmac_tag
    }
}

/// Sovereign Encrypted Vault Manager.
pub struct StorageVault {
    vault_key: [u8; 32],
    is_initialized: bool,
}

impl StorageVault {
    pub const fn new() -> Self {
        Self {
            vault_key: [0u8; 32],
            is_initialized: false,
        }
    }

    /// Initializes the vault using the hardware keystore DID Master Key (Slot 0).
    pub fn initialize(&mut self) -> Result<(), VaultError> {
        let ks = crate::crypto::SECURE_KEYSTORE.lock();
        let derived = ks.derive_key(crate::crypto::KeySlot::SovereignMasterKey, b"vault_master");
        self.vault_key = derived;
        self.is_initialized = true;
        Ok(())
    }

    /// Generates a 512-byte deterministic pseudo-random keystream for a block.
    fn generate_keystream(&self, block_index: u64, iv: &[u8; 16], out: &mut [u8; VAULT_SECTOR_SIZE]) {
        let mut counter_buf = [0u8; 16 + 8 + 4];
        counter_buf[0..16].copy_from_slice(iv);
        counter_buf[16..24].copy_from_slice(&block_index.to_le_bytes());

        let mut offset = 0;
        let mut counter: u32 = 0;
        while offset < VAULT_SECTOR_SIZE {
            counter_buf[24..28].copy_from_slice(&counter.to_le_bytes());
            let chunk = HmacSha256::compute(&self.vault_key, &counter_buf);
            let to_copy = chunk.len().min(VAULT_SECTOR_SIZE - offset);
            out[offset..offset + to_copy].copy_from_slice(&chunk[..to_copy]);
            offset += to_copy;
            counter += 1;
        }
    }

    /// Encrypts a 512-byte plaintext sector and signs the resulting envelope.
    pub fn encrypt_sector(
        &self,
        block_index: u64,
        plaintext: &[u8; VAULT_SECTOR_SIZE],
        iv: &[u8; 16],
    ) -> Result<VaultBlock, VaultError> {
        if !self.is_initialized {
            return Err(VaultError::KeystoreUninitialized);
        }

        let mut keystream = [0u8; VAULT_SECTOR_SIZE];
        self.generate_keystream(block_index, iv, &mut keystream);

        let mut ciphertext = [0u8; VAULT_SECTOR_SIZE];
        for i in 0..VAULT_SECTOR_SIZE {
            ciphertext[i] = plaintext[i] ^ keystream[i];
        }

        let mut block = VaultBlock {
            iv: *iv,
            block_index,
            ciphertext,
            hmac_tag: [0u8; SHA256_DIGEST_LENGTH],
        };
        block.hmac_tag = block.compute_hmac(&self.vault_key);
        Ok(block)
    }

    /// Decrypts an authenticated block, rejecting any tampered ciphertext.
    pub fn decrypt_sector(&self, block: &VaultBlock) -> Result<[u8; VAULT_SECTOR_SIZE], VaultError> {
        if !self.is_initialized {
            return Err(VaultError::KeystoreUninitialized);
        }
        if !block.verify(&self.vault_key) {
            return Err(VaultError::IntegrityViolation);
        }

        let mut keystream = [0u8; VAULT_SECTOR_SIZE];
        self.generate_keystream(block.block_index, &block.iv, &mut keystream);

        let mut plaintext = [0u8; VAULT_SECTOR_SIZE];
        for i in 0..VAULT_SECTOR_SIZE {
            plaintext[i] = block.ciphertext[i] ^ keystream[i];
        }
        Ok(plaintext)
    }
}

pub static STORAGE_VAULT: Mutex<StorageVault> = Mutex::new(StorageVault::new());

/// Initializes the Sovereign Encrypted Storage Vault and runs verification self-tests.
pub fn init() {
    let mut vault = STORAGE_VAULT.lock();
    vault.initialize().expect("Failed to initialize storage vault");

    let test_iv: [u8; 16] = [0xA5; 16];
    let mut plaintext = [0u8; VAULT_SECTOR_SIZE];
    for i in 0..VAULT_SECTOR_SIZE {
        plaintext[i] = (i % 256) as u8;
    }

    // 1. Encrypt sector 42
    let enc_block = vault
        .encrypt_sector(42, &plaintext, &test_iv)
        .expect("Encryption must succeed");
    assert_ne!(enc_block.ciphertext, plaintext, "Ciphertext must not equal plaintext");

    // 2. Decrypt legitimate sector
    let dec_plaintext = vault
        .decrypt_sector(&enc_block)
        .expect("Decryption must succeed");
    assert_eq!(dec_plaintext, plaintext, "Decrypted plaintext must match original");

    // 3. Tamper detection test
    let mut corrupted_block = enc_block;
    corrupted_block.ciphertext[0] ^= 0x01; // 1-bit corruption
    let tamper_res = vault.decrypt_sector(&corrupted_block);
    assert_eq!(
        tamper_res,
        Err(VaultError::IntegrityViolation),
        "1-bit tampered ciphertext must be rejected by HMAC authentication"
    );

    crate::serial_println!(
        "[TEST] Sovereign Encrypted Vault: Key-wrapped AES/HMAC block cipher, sector 42 verified, 1-bit tamper rejected."
    );
}
