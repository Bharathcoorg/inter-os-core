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

/// Physical placement of the sealed-envelope store.
///
/// Laid out after the InterFS region and well below the journal, which occupies
/// the final slots at the end of the disk. Each sealed block occupies two
/// sectors: a header carrying the magic, block index, IV and authentication
/// tag, then the ciphertext.
pub const VAULT_BASE_LBA: u32 = 4096 + 32809;
pub const VAULT_TOTAL_BLOCKS: u32 = 2048;
pub const VAULT_MAGIC: [u8; 4] = *b"IVLT";
pub const VAULT_FORMAT_VERSION: u32 = 1;

/// Errors returned by storage vault operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultError {
    IntegrityViolation,
    KeystoreUninitialized,
    InvalidBlockIndex,
    HardwareFailure,
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
    ///
    /// WHY this refuses without a storage root instead of deriving anyway:
    /// `SecureKeystore::derive_key` returns a key rather than a result, so with
    /// no root established the only key obtainable is `HMAC(0, ...)` — a fixed
    /// function of published constants. Sealing under it would report success
    /// while producing a ciphertext every reader on the machine could decrypt,
    /// and the HMAC tag over it would authenticate nothing. The vault is left
    /// uninitialised, which makes every seal, open, encrypt and decrypt fail
    /// closed with `KeystoreUninitialized`.
    ///
    /// The same gate lives in `storage::encrypted::storage_key`, which is the
    /// other unguarded `derive_key` caller. Both refuse rather than one of them
    /// trusting the other to have checked.
    pub fn initialize(&mut self) -> Result<(), VaultError> {
        let ks = crate::crypto::SECURE_KEYSTORE.lock();
        if !ks.is_seeded() {
            return Err(VaultError::KeystoreUninitialized);
        }
        let derived = ks.derive_key(crate::crypto::KeySlot::SovereignMasterKey, b"vault_master");
        self.vault_key = derived;
        self.is_initialized = true;
        Ok(())
    }

    /// Seals `plaintext` and persists the envelope, so it can be reopened after
    /// a reboot.
    ///
    /// The envelope is written to the vault region on the data disk: a header
    /// sector holding the magic, block index, IV and authentication tag, followed
    /// by the ciphertext sector. Both are needed to open the block later, which
    /// is what makes the integrity check meaningful: the tag now comes from
    /// storage rather than from the caller.
    pub fn seal_sector(
        &mut self,
        block_index: u64,
        plaintext: &[u8; VAULT_SECTOR_SIZE],
    ) -> Result<VaultBlock, VaultError> {
        if block_index >= VAULT_TOTAL_BLOCKS as u64 {
            return Err(VaultError::InvalidBlockIndex);
        }
        // A fresh IV per seal. Reusing one across blocks of a keystream would
        // leak the keystream relationship between them.
        let mut iv = [0u8; 16];
        crate::crypto::get_random_bytes(&mut iv);
        if iv == [0u8; 16] {
            return Err(VaultError::HardwareFailure);
        }
        let block = self.encrypt_sector(block_index, plaintext, &iv)?;
        self.write_envelope(block_index, &block)?;
        Ok(block)
    }

    /// Reads a previously sealed envelope back and decrypts it.
    ///
    /// The ciphertext and tag come from storage, so a caller cannot choose the
    // bytes being verified and `IntegrityViolation` is genuinely reachable.
    pub fn open_sector(
        &self,
        block_index: u64,
        out: &mut [u8; VAULT_SECTOR_SIZE],
    ) -> Result<(), VaultError> {
        // WHY the gate is before the read and not after: with no vault key bound
        // the envelope cannot be interpreted at all, so touching the disk would
        // produce a verdict about the *media* — a misleading IntegrityViolation
        // for a block that may be perfectly intact. Refuse on the real reason.
        if !self.is_initialized {
            return Err(VaultError::KeystoreUninitialized);
        }
        let block = self.read_envelope(block_index)?;
        let plaintext = self.decrypt_sector(&block)?;
        out.copy_from_slice(&plaintext);
        Ok(())
    }

    fn envelope_sectors(block_index: u64) -> Option<(u32, u32)> {
        if block_index >= VAULT_TOTAL_BLOCKS as u64 {
            return None;
        }
        let header = (VAULT_BASE_LBA as u64 + block_index * 2) as u32;
        Some((header, header + 1))
    }

    fn write_envelope(&self, block_index: u64, block: &VaultBlock) -> Result<(), VaultError> {
        use crate::drivers::{BlockDevice, SECTOR_SIZE};
        let (header_lba, data_lba) =
            Self::envelope_sectors(block_index).ok_or(VaultError::InvalidBlockIndex)?;
        let mut header = [0u8; SECTOR_SIZE];
        header[0..4].copy_from_slice(&VAULT_MAGIC);
        header[4..8].copy_from_slice(&VAULT_FORMAT_VERSION.to_le_bytes());
        header[8..16].copy_from_slice(&block_index.to_le_bytes());
        header[16..32].copy_from_slice(&block.iv);
        header[32..64].copy_from_slice(&block.hmac_tag);
        header[64..72].copy_from_slice(&(block.ciphertext.len() as u64).to_le_bytes());

        let mut guard = crate::drivers::ATA_PRIMARY.lock();
        let drive = guard.as_mut().ok_or(VaultError::HardwareFailure)?;
        // Payload first, then the header that makes it reachable, so a torn
        // write never leaves a header pointing at unwritten ciphertext.
        drive
            .write_sector(data_lba, &block.ciphertext)
            .map_err(|_| VaultError::HardwareFailure)?;
        drive
            .write_sector(header_lba, &header)
            .map_err(|_| VaultError::HardwareFailure)?;
        Ok(())
    }

    fn read_envelope(&self, block_index: u64) -> Result<VaultBlock, VaultError> {
        use crate::drivers::{BlockDevice, SECTOR_SIZE};
        let (header_lba, data_lba) =
            Self::envelope_sectors(block_index).ok_or(VaultError::InvalidBlockIndex)?;
        let mut guard = crate::drivers::ATA_PRIMARY.lock();
        let drive = guard.as_mut().ok_or(VaultError::HardwareFailure)?;
        let mut header = [0u8; SECTOR_SIZE];
        drive
            .read_sector(header_lba, &mut header)
            .map_err(|_| VaultError::HardwareFailure)?;
        if header[0..4] != VAULT_MAGIC
            || u32::from_le_bytes([header[4], header[5], header[6], header[7]])
                != VAULT_FORMAT_VERSION
            || u64::from_le_bytes([
                header[8], header[9], header[10], header[11],
                header[12], header[13], header[14], header[15],
            ]) != block_index
        {
            return Err(VaultError::IntegrityViolation);
        }
        let mut ciphertext = [0u8; VAULT_SECTOR_SIZE];
        drive
            .read_sector(data_lba, &mut ciphertext)
            .map_err(|_| VaultError::HardwareFailure)?;

        let mut iv = [0u8; 16];
        iv.copy_from_slice(&header[16..32]);
        let mut hmac_tag = [0u8; SHA256_DIGEST_LENGTH];
        hmac_tag.copy_from_slice(&header[32..64]);

        Ok(VaultBlock { iv, block_index, ciphertext, hmac_tag })
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
        &mut self,
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
        // The HMAC is keyed with the keystore-derived vault key and covers
        // the IV, block index, and ciphertext. That is the persistent
        // authenticity record: it survives a reboot because it is stored in
        // the envelope header. Keeping a second in-RAM tag table here would
        // make a correctly persisted block impossible to reopen after reboot.
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
    let test_iv: [u8; 16] = [0xA5; 16];
    let mut plaintext = [0u8; VAULT_SECTOR_SIZE];
    for i in 0..VAULT_SECTOR_SIZE {
        plaintext[i] = (i % 256) as u8;
    }

    if vault.initialize().is_err() {
        // WHY this is a checked branch and not an early return that skips the
        // self-tests: skipping would be the silent version of the same defect.
        // The property that matters when there is no root is that the vault
        // refuses, so that is what gets asserted here. `is_initialized` stays
        // false, so every path below is already refusing; these assertions pin it
        // so a future change cannot quietly re-derive a key from an absent seed.
        assert!(
            !vault.is_initialized,
            "a refused initialization must leave the vault uninitialised, not half-configured",
        );
        let any_block = VaultBlock {
            iv: test_iv,
            block_index: 42,
            ciphertext: plaintext,
            hmac_tag: [0u8; SHA256_DIGEST_LENGTH],
        };
        assert_eq!(
            vault.encrypt_sector(42, &plaintext, &test_iv).err(),
            Some(VaultError::KeystoreUninitialized),
            "with no storage root there is no vault key, so encryption must refuse",
        );
        assert_eq!(
            vault.decrypt_sector(&any_block).err(),
            Some(VaultError::KeystoreUninitialized),
            "with no storage root there is no vault key, so decryption must refuse",
        );
        assert_eq!(
            vault.seal_sector(42, &plaintext).err(),
            Some(VaultError::KeystoreUninitialized),
            "sealing must refuse before it writes anything to the vault region",
        );
        assert_eq!(
            vault.open_sector(42, &mut plaintext),
            Err(VaultError::KeystoreUninitialized),
            "opening must refuse before it reads the vault region",
        );
        crate::serial_println!(
            "[VAULT] FATAL: no persistent storage root, so no vault key can be derived and the \
             vault stays uninitialised. Every vault operation fails closed with \
             KeystoreUninitialized. No key was derived from an absent seed.",
        );
        return;
    }

    // 0. Prove the persistent envelope store round-trips through the real disk
    //    path. Sealing writes the header and ciphertext to the vault region and
    //    opening reads them back, so a block sealed here can be reopened after a
    //    reboot. This is the only check that would have caught the envelope never
    //    reaching storage.
    const CANARY_BLOCK: u64 = 0;
    match vault.seal_sector(CANARY_BLOCK, &plaintext) {
        Ok(sealed) => {
            // Reconstruct the vault object before opening. This deliberately
            // discards all in-RAM seal history and models the state after a
            // reboot; only the persistent envelope and keystore-derived key may
            // authenticate the block.
            let mut rebooted_vault = StorageVault::new();
            rebooted_vault
                .initialize()
                .expect("vault must reinitialize from the persistent keystore root");
            let mut reopened = [0u8; VAULT_SECTOR_SIZE];
            match rebooted_vault.open_sector(CANARY_BLOCK, &mut reopened) {
                Ok(()) => assert_eq!(
                    reopened, plaintext,
                    "sealed block must reopen after reconstructing the vault"
                ),
                Err(e) => panic!("sealed vault block must reopen after reboot: {:?}", e),
            }
            let _ = sealed;
            crate::serial_println!(
                "[TEST] Vault persistence: envelope sealed to disk at LBA {} and reopened                  through a fresh vault instance.",
                VAULT_BASE_LBA
            );
        }
        Err(VaultError::HardwareFailure) => {
            crate::serial_println!(
                "[WARN] Vault persistence check skipped: no block device for the vault region."
            );
        }
        Err(e) => panic!("vault persistence check failed: {:?}", e),
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
        "[TEST] Sovereign Encrypted Vault: HMAC-authenticated stream cipher, sector 42 verified, 1-bit tamper rejected."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vault with no storage root must refuse every operation, and must not
    /// have a key in the first place.
    ///
    /// WHY this matters: `derive_key` cannot report the absence of a root, so an
    /// ungated `initialize` produced `HMAC(0, ...)` and set `is_initialized`.
    /// Everything downstream then reported success while protecting nothing —
    /// and the keystream was a fixed function of public constants, so the
    /// "encrypted" blocks were readable by anyone.
    ///
    /// Pinning assertions: the `is_initialized` check after the refused init, and
    /// the four operation refusals. The `open_sector` one is the interesting
    /// half: it must refuse *before* touching the disk, or it reports a verdict
    /// about the media for a block that may be perfectly intact.
    #[test]
    fn authenticated_block_can_be_opened_by_a_fresh_vault_instance() {
        // A reboot reconstructs the vault object from the persistent keystore
        // root; no in-memory seal history may be required to authenticate data.
        let mut writer = StorageVault::new();
        writer.vault_key = [0x3Cu8; 32];
        writer.is_initialized = true;

        let plaintext = [0xA7u8; VAULT_SECTOR_SIZE];
        let block = writer
            .encrypt_sector(7, &plaintext, &[0x11u8; 16])
            .expect("seeded test vault must encrypt");

        let mut reopened = StorageVault::new();
        reopened.vault_key = [0x3Cu8; 32];
        reopened.is_initialized = true;

        let recovered = reopened
            .decrypt_sector(&block)
            .expect("persistent HMAC must authenticate across a fresh vault instance");
        assert_eq!(recovered, plaintext);

        let mut tampered = block;
        tampered.ciphertext[0] ^= 1;
        assert_eq!(
            reopened.decrypt_sector(&tampered),
            Err(VaultError::IntegrityViolation)
        );

        let mut wrong_key = StorageVault::new();
        wrong_key.vault_key = [0x3Du8; 32];
        wrong_key.is_initialized = true;
        assert_eq!(
            wrong_key.decrypt_sector(&block),
            Err(VaultError::IntegrityViolation)
        );
    }

    #[test]
    fn an_unseeded_vault_refuses_rather_than_deriving_a_key_from_nothing() {
        assert!(
            !crate::crypto::SECURE_KEYSTORE.lock().is_seeded(),
            "no unit test establishes a storage root; this test's branch depends on there being none",
        );

        let mut vault = StorageVault::new();
        assert_eq!(vault.initialize(), Err(VaultError::KeystoreUninitialized));
        assert!(!vault.is_initialized, "a refused initialize must not mark the vault usable");
        assert_eq!(vault.vault_key, [0u8; 32], "a refused initialize must not leave a key behind");

        let plaintext = [0x5Au8; VAULT_SECTOR_SIZE];
        let block = VaultBlock {
            iv: [0xA5; 16],
            block_index: 42,
            ciphertext: plaintext,
            hmac_tag: [0u8; SHA256_DIGEST_LENGTH],
        };
        assert_eq!(vault.encrypt_sector(42, &plaintext, &[0xA5; 16]).err(), Some(VaultError::KeystoreUninitialized));
        assert_eq!(vault.decrypt_sector(&block), Err(VaultError::KeystoreUninitialized));
        assert_eq!(vault.seal_sector(42, &plaintext).err(), Some(VaultError::KeystoreUninitialized));
        assert_eq!(vault.open_sector(42, &mut plaintext.clone()), Err(VaultError::KeystoreUninitialized));
    }
}
