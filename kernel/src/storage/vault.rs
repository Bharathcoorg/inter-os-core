// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Sovereign Encrypted Storage Vault Subsystem
//!
//! Provides hardware-keystore anchored authenticated encrypted block storage.
//! Employs counter-mode pseudo-random keystream XOR streaming with per-sector
//! initialization vectors (IV) and RFC 4231 HMAC-SHA256 authenticated integrity
//! tags to ensure complete confidentiality and tamper rejection for user data.

use spin::Mutex;
use crate::crypto::hmac::HmacSha256;
use crate::crypto::SHA256_DIGEST_LENGTH;
use crate::drivers::{BlockDevice, SECTOR_SIZE};

/// Standard hardware sector size in bytes.
pub const VAULT_SECTOR_SIZE: usize = 512;

/// Physical placement of the sealed-envelope store.
///
/// Laid out after the InterFS region and well below the journal, which occupies
/// the final slots at the end of the disk. Each sealed block occupies an
/// alternating pair of slots (`VAULT_SECTORS_PER_BLOCK` sectors): every slot is
/// a header carrying the magic, format version, block index, IV, authentication
/// tag and generation counter, followed by its ciphertext sector. The pair lets
/// a torn or corrupt newest slot fall back to its intact sibling, the same
/// redundancy the metadata journal uses.
pub const VAULT_TOTAL_BLOCKS: u32 = 2048;
/// Total sectors the vault occupies: `VAULT_TOTAL_BLOCKS` alternating pairs.
pub const VAULT_TOTAL_SECTORS: u32 = VAULT_TOTAL_BLOCKS * VAULT_SECTORS_PER_BLOCK;
/// First sector of the vault on a device of `total` sectors.
///
/// The vault lives AFTER the InterFS region and BELOW the journal tail, so the
/// three regions are mutually exclusive by construction on every disk size:
/// `[boot 4096 .. vault_base)` is InterFS, `[vault_base .. +VAULT_TOTAL_SECTORS)`
/// is the vault, and the final journal slots are the metadata journal. A fixed
/// base would collide with InterFS on any disk large enough to hold a model.
pub fn vault_base_lba(total: u32) -> Option<u32> {
    let journal = super::persistent::SLOT_SECTORS * super::persistent::TOTAL_SLOT_COUNT;
    total.checked_sub(journal + VAULT_TOTAL_SECTORS)
}
pub const VAULT_MAGIC: [u8; 4] = *b"IVLT";
/// On-disk envelope format. Version 1 was a single header+ciphertext slot per
/// block with no redundancy; it is refused explicitly. Version 2 is the
/// alternating pair with a generation counter.
pub const VAULT_FORMAT_VERSION: u32 = 2;
/// Sectors one sealed block occupies: two slots, each a header + ciphertext.
pub const VAULT_SECTORS_PER_BLOCK: u32 = 4;
/// Slots in a block's alternating pair.
pub const VAULT_SLOTS_PER_BLOCK: u32 = 2;

/// Errors returned by storage vault operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultError {
    IntegrityViolation,
    KeystoreUninitialized,
    InvalidBlockIndex,
    HardwareFailure,
    /// The block holds a version 1 single-slot envelope, which this build
    /// refuses rather than misreads as the redundant version 2 layout.
    UnsupportedFormat,
}

/// An authenticated, encrypted storage block envelope.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct VaultBlock {
    pub iv: [u8; 16],
    pub block_index: u64,
    pub ciphertext: [u8; VAULT_SECTOR_SIZE],
    pub hmac_tag: [u8; SHA256_DIGEST_LENGTH],
    /// Generation this envelope was written under; authenticated with the tag so
    /// a tampered generation cannot force selection of an older slot.
    pub generation: u64,
}

impl VaultBlock {
    /// Computes HMAC-SHA256 authentication tag over iv, block_index, and ciphertext.
    /// HMAC over iv + block index + generation + ciphertext. The generation is
    /// authenticated so a tampered generation cannot force an older slot.
    fn generation_tag(
        key: &[u8; 32],
        block: &VaultBlock,
        generation: u64,
    ) -> [u8; SHA256_DIGEST_LENGTH] {
        let mut msg = [0u8; 16 + 8 + 8 + VAULT_SECTOR_SIZE];
        msg[0..16].copy_from_slice(&block.iv);
        msg[16..24].copy_from_slice(&block.block_index.to_le_bytes());
        msg[24..32].copy_from_slice(&generation.to_le_bytes());
        msg[32..32 + VAULT_SECTOR_SIZE].copy_from_slice(&block.ciphertext);
        HmacSha256::compute(key, &msg)
    }

    pub fn compute_hmac(&self, key: &[u8; 32]) -> [u8; SHA256_DIGEST_LENGTH] {
        let mut msg = [0u8; 16 + 8 + 8 + VAULT_SECTOR_SIZE];
        msg[0..16].copy_from_slice(&self.iv);
        msg[16..24].copy_from_slice(&self.block_index.to_le_bytes());
        msg[24..32].copy_from_slice(&self.generation.to_le_bytes());
        msg[32..32 + VAULT_SECTOR_SIZE].copy_from_slice(&self.ciphertext);
        HmacSha256::compute(key, &msg)
    }

    /// Validates block integrity tag against the vault key.
    pub fn verify(&self, key: &[u8; 32]) -> bool {
        let expected = self.compute_hmac(key);
        crate::crypto::constant_time_eq_32(&expected, &self.hmac_tag)
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
        let mut guard = crate::drivers::ATA_PRIMARY.lock();
        let drive = guard.as_mut().ok_or(VaultError::HardwareFailure)?;
        Self::write_envelope(drive, &self.vault_key, block_index, &block)?;
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
        let mut guard = crate::drivers::ATA_PRIMARY.lock();
        let drive = guard.as_mut().ok_or(VaultError::HardwareFailure)?;
        let block = self.read_envelope(drive, block_index)?;
        let plaintext = self.decrypt_sector(&block)?;
        out.copy_from_slice(&plaintext);
        Ok(())
    }

    /// First sector of a block's alternating pair on a device of `total`.
    fn pair_base_lba(block_index: u64, total: u32) -> Option<u32> {
        if block_index >= VAULT_TOTAL_BLOCKS as u64 {
            return None;
        }
        let base = vault_base_lba(total)?;
        Some(base + block_index as u32 * VAULT_SECTORS_PER_BLOCK)
    }

    /// Header and ciphertext sector of one slot within a block's pair.
    fn slot_lbas(block_index: u64, slot: u32, total: u32) -> Option<(u32, u32)> {
        let base = Self::pair_base_lba(block_index, total)?;
        let header = base + slot * 2;
        Some((header, header + 1))
    }

    /// Serialises a version 2 header for one slot.
    fn build_header(block_index: u64, block: &VaultBlock, generation: u64) -> [u8; SECTOR_SIZE] {
        let mut header = [0u8; SECTOR_SIZE];
        header[0..4].copy_from_slice(&VAULT_MAGIC);
        header[4..8].copy_from_slice(&VAULT_FORMAT_VERSION.to_le_bytes());
        header[8..16].copy_from_slice(&block_index.to_le_bytes());
        header[16..32].copy_from_slice(&block.iv);
        header[32..64].copy_from_slice(&block.hmac_tag);
        header[64..72].copy_from_slice(&(block.ciphertext.len() as u64).to_le_bytes());
        header[72..80].copy_from_slice(&generation.to_le_bytes());
        header
    }

    /// Reads the generation of one slot for the write path's alternation. Any
    /// unreadable, foreign or old-format header reads as generation 0, so a
    /// fresh seal simply claims the slot.
    fn slot_generation(drive: &mut impl BlockDevice, block_index: u64, slot: u32, total: u32) -> u64 {
        let Some((header_lba, _)) = Self::slot_lbas(block_index, slot, total) else {
            return 0;
        };
        let mut header = [0u8; SECTOR_SIZE];
        if drive.read_sector(header_lba, &mut header).is_err() {
            return 0;
        }
        if header[0..4] != VAULT_MAGIC {
            return 0;
        }
        let version = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        if version != VAULT_FORMAT_VERSION {
            return 0;
        }
        if u64::from_le_bytes([
            header[8], header[9], header[10], header[11],
            header[12], header[13], header[14], header[15],
        ]) != block_index
        {
            return 0;
        }
        u64::from_le_bytes([
            header[72], header[73], header[74], header[75],
            header[76], header[77], header[78], header[79],
        ])
    }

    /// Writes one envelope into the older of the block's two slots, stamping a
    /// strictly newer generation, so a crash mid-write leaves the sibling
    /// generation authoritative.
    fn write_envelope(
        drive: &mut impl BlockDevice,
        key: &[u8; 32],
        block_index: u64,
        block: &VaultBlock,
    ) -> Result<(), VaultError> {
        let total = drive.sector_count();
        let gen_a = Self::slot_generation(&mut *drive, block_index, 0, total);
        let gen_b = Self::slot_generation(&mut *drive, block_index, 1, total);
        // Alternate: target the slot with the older generation, so the newest
        // valid generation is never the one being overwritten.
        let target = if gen_a <= gen_b { 0u32 } else { 1u32 };
        let next_generation = gen_a.max(gen_b).saturating_add(1);
        let (header_lba, data_lba) =
            Self::slot_lbas(block_index, target, total).ok_or(VaultError::InvalidBlockIndex)?;
        let mut header = Self::build_header(block_index, block, next_generation);
        // Authenticate the generation too: the tag covers iv+index+generation+
        // ciphertext, so an attacker cannot bump the generation to select an
        // older valid slot (rollback).
        let tag = VaultBlock::generation_tag(key, block, next_generation);
        header[32..64].copy_from_slice(&tag);

        // Payload first, then the header that makes it reachable, so a torn
        // write never leaves a header pointing at unwritten ciphertext.
        drive
            .write_sector(data_lba, &block.ciphertext)
            .map_err(|_| VaultError::HardwareFailure)?;
        drive.flush().map_err(|_| VaultError::HardwareFailure)?;
        drive
            .write_sector(header_lba, &header)
            .map_err(|_| VaultError::HardwareFailure)?;
        drive.flush().map_err(|_| VaultError::HardwareFailure)?;
        Ok(())
    }

    /// Reads one slot, authenticating it against the vault key.
    ///
    /// Returns `Ok(Some((generation, block)))` only for a version 2 envelope
    /// whose header names this block and whose HMAC verifies. A slot that is
    /// empty, foreign, torn or fails authentication is `Ok(None)` so the caller
    /// can fall back to the sibling. A version 1 single-slot header is
    /// `Err(UnsupportedFormat)`: it is refused explicitly rather than misread.
    fn read_slot(
        &self,
        drive: &mut impl BlockDevice,
        block_index: u64,
        slot: u32,
        total: u32,
    ) -> Result<Option<(u64, VaultBlock)>, VaultError> {
        let (header_lba, data_lba) =
            Self::slot_lbas(block_index, slot, total).ok_or(VaultError::InvalidBlockIndex)?;
        let mut header = [0u8; SECTOR_SIZE];
        drive
            .read_sector(header_lba, &mut header)
            .map_err(|_| VaultError::HardwareFailure)?;
        if header[0..4] != VAULT_MAGIC {
            return Ok(None);
        }
        let version = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        if version == 1 {
            return Err(VaultError::UnsupportedFormat);
        }
        if version != VAULT_FORMAT_VERSION {
            return Ok(None);
        }
        if u64::from_le_bytes([
            header[8], header[9], header[10], header[11],
            header[12], header[13], header[14], header[15],
        ]) != block_index
        {
            return Ok(None);
        }
        let generation = u64::from_le_bytes([
            header[72], header[73], header[74], header[75],
            header[76], header[77], header[78], header[79],
        ]);

        let mut ciphertext = [0u8; VAULT_SECTOR_SIZE];
        drive
            .read_sector(data_lba, &mut ciphertext)
            .map_err(|_| VaultError::HardwareFailure)?;

        let mut iv = [0u8; 16];
        iv.copy_from_slice(&header[16..32]);
        let mut hmac_tag = [0u8; SHA256_DIGEST_LENGTH];
        hmac_tag.copy_from_slice(&header[32..64]);

        let block = VaultBlock { iv, block_index, ciphertext, hmac_tag, generation };
        // Authenticate the generation as part of the payload: a slot whose
        // generation was tampered (to force selection of an older slot) fails
        // the tag and is treated as absent, so recovery falls to the sibling.
        let expected = VaultBlock::generation_tag(&self.vault_key, &block, generation);
        if !crate::crypto::constant_time_eq_32(&expected, &block.hmac_tag) {
            return Ok(None);
        }
        Ok(Some((generation, block)))
    }

    /// Selects the newest valid slot for a block, falling back to the sibling
    /// when the newer slot is torn or corrupt.
    fn read_envelope(
        &self,
        drive: &mut impl BlockDevice,
        block_index: u64,
    ) -> Result<VaultBlock, VaultError> {
        let mut best: Option<(u64, VaultBlock)> = None;
        let mut saw_old_format = false;
        let total = drive.sector_count();
        for slot in 0..VAULT_SLOTS_PER_BLOCK {
            match self.read_slot(&mut *drive, block_index, slot, total) {
                Ok(Some((generation, block))) => {
                    // Strict `>`: the first slot wins a tie, so recovery is
                    // deterministic and never depends on scan order for equal
                    // generations.
                    if best.map(|(g, _)| generation > g).unwrap_or(true) {
                        best = Some((generation, block));
                    }
                }
                Ok(None) => {}
                Err(VaultError::UnsupportedFormat) => saw_old_format = true,
                Err(e) => return Err(e),
            }
        }
        if let Some((_, block)) = best {
            return Ok(block);
        }
        if saw_old_format {
            return Err(VaultError::UnsupportedFormat);
        }
        Err(VaultError::IntegrityViolation)
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
            generation: 0,
        };
        block.hmac_tag = block.compute_hmac(&self.vault_key);
        Ok(block)
    }

    /// Decrypts an authenticated block, rejecting any tampered ciphertext.
    pub fn decrypt_sector(&self, block: &VaultBlock) -> Result<[u8; VAULT_SECTOR_SIZE], VaultError> {
        if !self.is_initialized {
            return Err(VaultError::KeystoreUninitialized);
        }
        // Authenticity is established by `read_slot`, which verifies the
        // generation-bound HMAC (iv + index + generation + ciphertext) before a
        // block is returned. Re-verify here with the block's own generation so a
        // tampered block (or a caller-supplied one) is still rejected.
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
            generation: 0,
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
                vault_base_lba(crate::drivers::ATA_PRIMARY.lock().as_ref().map(|d| d.sector_count()).unwrap_or(0)).unwrap_or(0)
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
    use crate::storage::block::BlockDeviceError;

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
            generation: 0,
        };
        assert_eq!(vault.encrypt_sector(42, &plaintext, &[0xA5; 16]).err(), Some(VaultError::KeystoreUninitialized));
        assert_eq!(vault.decrypt_sector(&block), Err(VaultError::KeystoreUninitialized));
        assert_eq!(vault.seal_sector(42, &plaintext).err(), Some(VaultError::KeystoreUninitialized));
        assert_eq!(vault.open_sector(42, &mut plaintext.clone()), Err(VaultError::KeystoreUninitialized));
    }

    /// Sector-granular in-memory device, so the alternating-pair write and
    /// recovery paths can be exercised without the ATA driver.
    struct MockDevice {
        sectors: alloc::vec::Vec<u8>,
        total: u32,
    }

    impl MockDevice {
        fn new(total: u32) -> Self {
            Self { sectors: alloc::vec![0u8; total as usize * SECTOR_SIZE], total }
        }

        /// Overwrites one sector, i.e. a lost or garbled tail write.
        fn scribble(&mut self, lba: u32, fill: u8) {
            let start = lba as usize * SECTOR_SIZE;
            for byte in &mut self.sectors[start..start + SECTOR_SIZE] {
                *byte = fill;
            }
        }

        /// Writes a header sector verbatim, used to forge an old-format header.
        fn write_header(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) {
            let start = lba as usize * SECTOR_SIZE;
            self.sectors[start..start + SECTOR_SIZE].copy_from_slice(buf);
        }
    }

    impl BlockDevice for MockDevice {
        fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
            if lba >= self.total {
                return Err(BlockDeviceError::LbaOutOfRange);
            }
            let start = lba as usize * SECTOR_SIZE;
            buf.copy_from_slice(&self.sectors[start..start + SECTOR_SIZE]);
            Ok(())
        }

        fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
            if lba >= self.total {
                return Err(BlockDeviceError::LbaOutOfRange);
            }
            let start = lba as usize * SECTOR_SIZE;
            self.sectors[start..start + SECTOR_SIZE].copy_from_slice(buf);
            Ok(())
        }

        fn sector_count(&self) -> u32 {
            self.total
        }

        fn model(&self) -> &str {
            "mock"
        }
    }

    /// Covers block 0's pair (sectors base..base+3) with a small margin.
const MOCK_SECTORS: u32 = 65536;
const BLOCK: u64 = 0;

fn mock_base() -> u32 {
    vault_base_lba(MOCK_SECTORS).expect("mock disk is large enough for the vault")
}

    fn seeded_vault() -> StorageVault {
        let mut vault = StorageVault::new();
        vault.vault_key = [0x3Cu8; 32];
        vault.is_initialized = true;
        vault
    }

    /// A sealed envelope must round-trip through the redundant pair: the
    /// ciphertext written to a slot is read back and decrypts to the original
    /// plaintext under a fresh vault instance.
    #[test]
    fn envelope_roundtrips_through_the_redundant_pair() {
        let mut vault = seeded_vault();
        let plaintext = [0xA7u8; VAULT_SECTOR_SIZE];
        let block = vault
            .encrypt_sector(BLOCK, &plaintext, &[0x11u8; 16])
            .expect("seeded test vault must encrypt");

        let mut dev = MockDevice::new(MOCK_SECTORS);
        StorageVault::write_envelope(&mut dev, &vault.vault_key, BLOCK, &block).expect("write must succeed");

        let read = vault.read_envelope(&mut dev, BLOCK).expect("read must succeed");
        assert_eq!(read.ciphertext, block.ciphertext, "ciphertext must survive the round trip");
        assert_eq!(
            vault.decrypt_sector(&read).expect("decrypt must authenticate"),
            plaintext,
            "the recovered envelope must decrypt to the original plaintext",
        );
    }

    /// A torn newest slot must fall back to the older, intact sibling rather
    /// than losing the block.
    #[test]
    fn torn_newest_slot_falls_back_to_the_sibling() {
        let mut vault = seeded_vault();
        let first = [0x10u8; VAULT_SECTOR_SIZE];
        let second = [0x20u8; VAULT_SECTOR_SIZE];
        let a = vault.encrypt_sector(BLOCK, &first, &[0x11u8; 16]).expect("encrypt a");
        let b = vault.encrypt_sector(BLOCK, &second, &[0x22u8; 16]).expect("encrypt b");

        let mut dev = MockDevice::new(MOCK_SECTORS);
        StorageVault::write_envelope(&mut dev, &vault.vault_key, BLOCK, &a).expect("write a"); // slot 0, gen 1
        StorageVault::write_envelope(&mut dev, &vault.vault_key, BLOCK, &b).expect("write b"); // slot 1, gen 2

        // The newest generation is the second write before the tear.
        let newest = vault.read_envelope(&mut dev, BLOCK).expect("read newest");
        assert_eq!(vault.decrypt_sector(&newest).unwrap(), second);

        // Tear the newest slot's header sector (slot 1 header = base + 2).
        dev.scribble(mock_base() + 2, 0x00);

        let recovered = vault.read_envelope(&mut dev, BLOCK).expect("fallback must succeed");
        assert_eq!(
            vault.decrypt_sector(&recovered).unwrap(),
            first,
            "a torn newest slot must fall back to the older intact sibling",
        );
    }

    /// When neither slot authenticates, the read must report an integrity
    /// violation rather than returning half-written bytes.
    #[test]
    fn corrupt_both_slots_is_an_integrity_violation() {
        let mut vault = seeded_vault();
        let a = vault.encrypt_sector(BLOCK, &[0x30u8; VAULT_SECTOR_SIZE], &[0x11u8; 16]).unwrap();
        let b = vault.encrypt_sector(BLOCK, &[0x40u8; VAULT_SECTOR_SIZE], &[0x22u8; 16]).unwrap();

        let mut dev = MockDevice::new(MOCK_SECTORS);
        StorageVault::write_envelope(&mut dev, &vault.vault_key, BLOCK, &a).unwrap(); // slot 0
        StorageVault::write_envelope(&mut dev, &vault.vault_key, BLOCK, &b).unwrap(); // slot 1

        // Corrupt both ciphertext sectors so neither slot passes its tag.
        dev.scribble(mock_base() + 1, 0xFF); // slot 0 data
        dev.scribble(mock_base() + 3, 0xFF); // slot 1 data

        assert_eq!(
            vault.read_envelope(&mut dev, BLOCK).err(),
            Some(VaultError::IntegrityViolation),
            "when neither slot authenticates the read must report an integrity violation",
        );
    }

    /// A tampered generation must not select an older slot: the generation is
    /// authenticated, so the tampered slot fails and recovery uses the sibling.
    #[test]
    fn tampered_generation_falls_back_to_the_older_slot() {
        let mut vault = seeded_vault();
        let first = [0x10u8; VAULT_SECTOR_SIZE];
        let second = [0x20u8; VAULT_SECTOR_SIZE];
        let a = vault.encrypt_sector(BLOCK, &first, &[0x11u8; 16]).unwrap();
        let b = vault.encrypt_sector(BLOCK, &second, &[0x22u8; 16]).unwrap();
        let mut dev = MockDevice::new(MOCK_SECTORS);
        StorageVault::write_envelope(&mut dev, &vault.vault_key, BLOCK, &a).unwrap(); // slot 0 gen 1
        StorageVault::write_envelope(&mut dev, &vault.vault_key, BLOCK, &b).unwrap(); // slot 1 gen 2
        // Tamper the newest slot's generation bytes (header at base+2, gen at 72..80).
        let mut hdr = [0u8; SECTOR_SIZE];
        dev.read_sector(mock_base() + 2, &mut hdr).unwrap();
        hdr[72..80].copy_from_slice(&u64::MAX.to_le_bytes());
        dev.write_header(mock_base() + 2, &hdr);
        let recovered = vault.read_envelope(&mut dev, BLOCK).expect("fallback must succeed");
        assert_eq!(
            vault.decrypt_sector(&recovered).unwrap(),
            first,
            "a tampered generation must not select the older slot; the intact sibling wins"
        );
    }

    /// A version 1 single-slot header must be refused explicitly, not misread
    /// as the redundant version 2 layout.
    #[test]
    fn old_single_slot_format_is_refused() {
        let vault = seeded_vault();
        let mut dev = MockDevice::new(MOCK_SECTORS);

        let mut header = [0u8; SECTOR_SIZE];
        header[0..4].copy_from_slice(&VAULT_MAGIC);
        header[4..8].copy_from_slice(&1u32.to_le_bytes());
        header[8..16].copy_from_slice(&BLOCK.to_le_bytes());
        dev.write_header(mock_base(), &header);

        assert_eq!(
            vault.read_envelope(&mut dev, BLOCK).err(),
            Some(VaultError::UnsupportedFormat),
            "the old single-slot format must be refused explicitly, not misread",
        );
    }
}
