// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Dedicated Keystore Architecture & Opaque Key Custody Service
//!
//! Provides hardware-isolated and software-backed development key custody with
//! zero ambient authority:
//! - Raw master key material and derived cryptographic keys never cross into Ring-3 applications.
//! - Applications interact exclusively via opaque `KeyHandle` descriptors.
//! - Domain separation (Companion, Search, History, Storage) is enforced by construction via HKDF.
//! - Per-user UID and task ownership checks prevent cross-application or cross-user key theft.
//! - Full memory zeroization on key revocation and slot destruction.

use crate::crypto::chacha20_poly1305::ChaCha20Poly1305;
use crate::crypto::hkdf::hkdf_expand;
use crate::crypto::hmac::HmacSha256;

/// Keystore service IPC message type identifier: b"KEYS".
pub const SERVICE_KEYSTORE_MSG: u32 = 0x4B45_5953;

/// Keystore service IPC command opcodes.
pub const KEYSTORE_OP_DERIVE: u8 = 1;
pub const KEYSTORE_OP_ENCRYPT: u8 = 2;
pub const KEYSTORE_OP_DECRYPT: u8 = 3;
pub const KEYSTORE_OP_SIGN: u8 = 4;
pub const KEYSTORE_OP_VERIFY: u8 = 5;
pub const KEYSTORE_OP_DESTROY: u8 = 6;
pub const KEYSTORE_OP_DERIVE_SERVICE_KEY: u8 = 7;

/// Keystore permission rights bitmask.
pub const RIGHT_ENCRYPT: u32 = 1 << 0;
pub const RIGHT_DECRYPT: u32 = 1 << 1;
pub const RIGHT_SIGN: u32 = 1 << 2;
pub const RIGHT_VERIFY: u32 = 1 << 3;
pub const RIGHT_WRAP: u32 = 1 << 4;
pub const RIGHT_UNWRAP: u32 = 1 << 5;
pub const RIGHT_DERIVE: u32 = 1 << 6;
pub const RIGHT_ALL: u32 = RIGHT_ENCRYPT | RIGHT_DECRYPT | RIGHT_SIGN | RIGHT_VERIFY | RIGHT_WRAP | RIGHT_UNWRAP | RIGHT_DERIVE;

/// Cryptographic domain separation labels.
pub const DOMAIN_COMPANION: u32 = 1;
pub const DOMAIN_SEMANTIC_SEARCH: u32 = 2;
pub const DOMAIN_CONVERSATION_HISTORY: u32 = 3;
pub const DOMAIN_APP_STORAGE: u32 = 4;
pub const DOMAIN_DEVICE_ROOT: u32 = 5;

/// Errors returned by the Keystore service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeystoreError {
    SlotNotFound,
    GenerationMismatch,
    PermissionDenied,
    UnauthorizedDomain,
    InvalidRights,
    AuthenticationFailed,
    CryptoFailure,
    TableFull,
    InvalidArguments,
}

/// Opaque Key Handle held by user applications.
///
/// Contains no raw secret bytes. The key material remains securely inside `keystored`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyHandle {
    pub slot: u16,
    pub generation: u16,
    pub domain: u32,
    pub rights: u32,
}

impl KeyHandle {
    pub const SIZE: usize = 12;

    pub const fn new(slot: u16, generation: u16, domain: u32, rights: u32) -> Self {
        Self {
            slot,
            generation,
            domain,
            rights,
        }
    }

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        buf[0..2].copy_from_slice(&self.slot.to_le_bytes());
        buf[2..4].copy_from_slice(&self.generation.to_le_bytes());
        buf[4..8].copy_from_slice(&self.domain.to_le_bytes());
        buf[8..12].copy_from_slice(&self.rights.to_le_bytes());
        buf
    }

    pub fn from_bytes(raw: &[u8]) -> Option<Self> {
        if raw.len() < Self::SIZE {
            return None;
        }
        let slot = u16::from_le_bytes([raw[0], raw[1]]);
        let generation = u16::from_le_bytes([raw[2], raw[3]]);
        let domain = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
        let rights = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]);
        Some(Self {
            slot,
            generation,
            domain,
            rights,
        })
    }
}

/// In-memory internal key slot.
#[derive(Clone, Copy)]
pub struct KeySlot {
    pub active: bool,
    pub generation: u16,
    pub owner_uid: u32,
    pub owner_task: u64,
    pub domain: u32,
    pub rights: u32,
    pub key: [u8; 32],
}

impl KeySlot {
    pub const fn empty() -> Self {
        Self {
            active: false,
            generation: 1,
            owner_uid: 0,
            owner_task: 0,
            domain: 0,
            rights: 0,
            key: [0u8; 32],
        }
    }

    pub fn zeroize(&mut self) {
        for b in self.key.iter_mut() {
            *b = 0;
        }
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
}

/// Keystore state table managing opaque keys for up to `SLOTS` entries.
pub struct KeystoreTable<const SLOTS: usize> {
    slots: [KeySlot; SLOTS],
}

impl<const SLOTS: usize> KeystoreTable<SLOTS> {
    pub const fn new() -> Self {
        Self {
            slots: [KeySlot::empty(); SLOTS],
        }
    }

    /// Derives a new cryptographic key for the specified caller UID, task, and domain.
    pub fn derive_key(
        &mut self,
        caller_uid: u32,
        caller_task: u64,
        domain: u32,
        label: &[u8],
        rights: u32,
        secret_root: &[u8; 32],
    ) -> Result<KeyHandle, KeystoreError> {
        // Find an inactive slot
        let mut target_idx = None;
        for i in 0..SLOTS {
            if !self.slots[i].active {
                target_idx = Some(i);
                break;
            }
        }
        let idx = target_idx.ok_or(KeystoreError::TableFull)?;

        // Build HKDF info prefix binding domain, UID, and label
        let mut info = [0u8; 64];
        info[0..4].copy_from_slice(&domain.to_le_bytes());
        info[4..8].copy_from_slice(&caller_uid.to_le_bytes());
        let label_len = label.len().min(56);
        info[8..8 + label_len].copy_from_slice(&label[..label_len]);

        let mut derived = [0u8; 32];
        if hkdf_expand(secret_root, &info[..8 + label_len], &mut derived).is_err() {
            return Err(KeystoreError::CryptoFailure);
        }

        let slot = &mut self.slots[idx];
        slot.active = true;
        slot.generation = slot.generation.wrapping_add(1);
        if slot.generation == 0 {
            slot.generation = 1;
        }
        slot.owner_uid = caller_uid;
        slot.owner_task = caller_task;
        slot.domain = domain;
        slot.rights = rights;
        slot.key.copy_from_slice(&derived);

        for b in derived.iter_mut() {
            *b = 0;
        }

        Ok(KeyHandle::new(idx as u16, slot.generation, domain, rights))
    }

    /// Validates handle validity and caller authority before granting access to slot.
    fn get_authorized_slot(
        &self,
        handle: KeyHandle,
        caller_uid: u32,
        caller_task: u64,
        required_right: u32,
    ) -> Result<&KeySlot, KeystoreError> {
        let idx = handle.slot as usize;
        if idx >= SLOTS {
            return Err(KeystoreError::SlotNotFound);
        }
        let slot = &self.slots[idx];
        if !slot.active {
            return Err(KeystoreError::SlotNotFound);
        }
        if slot.generation != handle.generation {
            return Err(KeystoreError::GenerationMismatch);
        }
        if slot.domain != handle.domain {
            return Err(KeystoreError::UnauthorizedDomain);
        }
        if slot.owner_uid != caller_uid || slot.owner_task != caller_task {
            return Err(KeystoreError::PermissionDenied);
        }
        if (slot.rights & required_right) == 0 {
            return Err(KeystoreError::InvalidRights);
        }
        Ok(slot)
    }

    /// Encrypts plaintext using ChaCha20-Poly1305 with the key corresponding to `handle`.
    pub fn encrypt(
        &self,
        handle: KeyHandle,
        caller_uid: u32,
        caller_task: u64,
        nonce: &[u8; 12],
        aad: &[u8],
        plaintext: &[u8],
        out_ciphertext: &mut [u8],
    ) -> Result<[u8; 16], KeystoreError> {
        let slot = self.get_authorized_slot(handle, caller_uid, caller_task, RIGHT_ENCRYPT)?;
        if out_ciphertext.len() < plaintext.len() {
            return Err(KeystoreError::InvalidArguments);
        }
        let mut tag = [0u8; 16];
        ChaCha20Poly1305::encrypt(
            &slot.key,
            nonce,
            aad,
            plaintext,
            &mut out_ciphertext[..plaintext.len()],
            &mut tag,
        );
        Ok(tag)
    }

    /// Decrypts ciphertext using ChaCha20-Poly1305 with the key corresponding to `handle`.
    pub fn decrypt(
        &self,
        handle: KeyHandle,
        caller_uid: u32,
        caller_task: u64,
        nonce: &[u8; 12],
        aad: &[u8],
        ciphertext: &[u8],
        tag: &[u8; 16],
        out_plaintext: &mut [u8],
    ) -> Result<(), KeystoreError> {
        let slot = self.get_authorized_slot(handle, caller_uid, caller_task, RIGHT_DECRYPT)?;
        if out_plaintext.len() < ciphertext.len() {
            return Err(KeystoreError::InvalidArguments);
        }
        ChaCha20Poly1305::decrypt(
            &slot.key,
            nonce,
            aad,
            ciphertext,
            tag,
            &mut out_plaintext[..ciphertext.len()],
        )
        .map_err(|_| KeystoreError::AuthenticationFailed)
    }

    /// Signs message using HMAC-SHA256 with the key corresponding to `handle`.
    pub fn sign(
        &self,
        handle: KeyHandle,
        caller_uid: u32,
        caller_task: u64,
        message: &[u8],
    ) -> Result<[u8; 32], KeystoreError> {
        let slot = self.get_authorized_slot(handle, caller_uid, caller_task, RIGHT_SIGN)?;
        Ok(HmacSha256::compute(&slot.key, message))
    }

    /// Verifies message HMAC-SHA256 signature using constant-time comparison.
    pub fn verify(
        &self,
        handle: KeyHandle,
        caller_uid: u32,
        caller_task: u64,
        message: &[u8],
        tag: &[u8; 32],
    ) -> Result<bool, KeystoreError> {
        let slot = self.get_authorized_slot(handle, caller_uid, caller_task, RIGHT_VERIFY)?;
        let expected = HmacSha256::compute(&slot.key, message);
        let mut diff = 0u8;
        for i in 0..32 {
            diff |= expected[i] ^ tag[i];
        }
        Ok(diff == 0)
    }

    /// Destroys a key, zeroizing its slot and incrementing the generation counter.
    pub fn destroy_key(
        &mut self,
        handle: KeyHandle,
        caller_uid: u32,
        caller_task: u64,
    ) -> Result<(), KeystoreError> {
        let idx = handle.slot as usize;
        if idx >= SLOTS {
            return Err(KeystoreError::SlotNotFound);
        }
        let slot = &mut self.slots[idx];
        if !slot.active {
            return Err(KeystoreError::SlotNotFound);
        }
        if slot.generation != handle.generation {
            return Err(KeystoreError::GenerationMismatch);
        }
        if slot.owner_uid != caller_uid || slot.owner_task != caller_task {
            return Err(KeystoreError::PermissionDenied);
        }

        slot.zeroize();
        slot.active = false;
        slot.generation = slot.generation.wrapping_add(1);
        if slot.generation == 0 {
            slot.generation = 1;
        }
        Ok(())
    }
}

/// Derives an opaque key handle from `keystored` via IPC.
pub fn keystore_derive_key(
    keystored_task: u64,
    domain: u32,
    rights: u32,
    label: &[u8],
) -> Result<KeyHandle, KeystoreError> {
    let mut req = [0u8; 64];
    req[0] = KEYSTORE_OP_DERIVE;
    req[1..5].copy_from_slice(&domain.to_le_bytes());
    req[5..9].copy_from_slice(&rights.to_le_bytes());
    let label_len = label.len().min(54);
    req[9] = label_len as u8;
    req[10..10 + label_len].copy_from_slice(&label[..label_len]);

    crate::ipc_send(keystored_task, SERVICE_KEYSTORE_MSG, &req[..10 + label_len])
        .map_err(|_| KeystoreError::CryptoFailure)?;

    let mut resp = [0u8; 32];
    let (sender, msg_type, len) = crate::ipc::receive_any(&mut resp)
        .map_err(|_| KeystoreError::CryptoFailure)?;

    if sender != keystored_task || msg_type != SERVICE_KEYSTORE_MSG || len < 13 {
        return Err(KeystoreError::CryptoFailure);
    }
    if resp[0] != 0 {
        return Err(KeystoreError::PermissionDenied);
    }
    KeyHandle::from_bytes(&resp[1..13]).ok_or(KeystoreError::CryptoFailure)
}

/// Encrypts plaintext in-place using `keystored` via IPC.
pub fn keystore_encrypt(
    keystored_task: u64,
    handle: KeyHandle,
    nonce: &[u8; 12],
    aad: &[u8],
    plaintext: &[u8],
    out_ciphertext: &mut [u8],
) -> Result<[u8; 16], KeystoreError> {
    if plaintext.len() > 1024 || out_ciphertext.len() < plaintext.len() {
        return Err(KeystoreError::InvalidArguments);
    }
    let mut req = [0u8; 2048];
    req[0] = KEYSTORE_OP_ENCRYPT;
    req[1..13].copy_from_slice(&handle.to_bytes());
    req[13..25].copy_from_slice(nonce);
    let aad_len = aad.len().min(64);
    req[25..27].copy_from_slice(&(aad_len as u16).to_le_bytes());
    req[27..29].copy_from_slice(&(plaintext.len() as u16).to_le_bytes());
    req[29..29 + aad_len].copy_from_slice(&aad[..aad_len]);
    let pt_offset = 29 + aad_len;
    req[pt_offset..pt_offset + plaintext.len()].copy_from_slice(plaintext);

    crate::ipc_send(keystored_task, SERVICE_KEYSTORE_MSG, &req[..pt_offset + plaintext.len()])
        .map_err(|_| KeystoreError::CryptoFailure)?;

    let mut resp = [0u8; 2048];
    let (sender, msg_type, len) = crate::ipc::receive_any(&mut resp)
        .map_err(|_| KeystoreError::CryptoFailure)?;

    if sender != keystored_task || msg_type != SERVICE_KEYSTORE_MSG || len < 17 {
        return Err(KeystoreError::CryptoFailure);
    }
    if resp[0] != 0 {
        return Err(KeystoreError::CryptoFailure);
    }
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&resp[1..17]);
    let ct_len = len - 17;
    out_ciphertext[..ct_len].copy_from_slice(&resp[17..len]);
    Ok(tag)
}

/// Decrypts ciphertext in-place using `keystored` via IPC.
pub fn keystore_decrypt(
    keystored_task: u64,
    handle: KeyHandle,
    nonce: &[u8; 12],
    aad: &[u8],
    ciphertext: &[u8],
    tag: &[u8; 16],
    out_plaintext: &mut [u8],
) -> Result<usize, KeystoreError> {
    if ciphertext.len() > 1024 || out_plaintext.len() < ciphertext.len() {
        return Err(KeystoreError::InvalidArguments);
    }
    let mut req = [0u8; 2048];
    req[0] = KEYSTORE_OP_DECRYPT;
    req[1..13].copy_from_slice(&handle.to_bytes());
    req[13..25].copy_from_slice(nonce);
    req[25..41].copy_from_slice(tag);
    let aad_len = aad.len().min(64);
    req[41..43].copy_from_slice(&(aad_len as u16).to_le_bytes());
    req[43..45].copy_from_slice(&(ciphertext.len() as u16).to_le_bytes());
    req[45..45 + aad_len].copy_from_slice(&aad[..aad_len]);
    let ct_offset = 45 + aad_len;
    req[ct_offset..ct_offset + ciphertext.len()].copy_from_slice(ciphertext);

    crate::ipc_send(keystored_task, SERVICE_KEYSTORE_MSG, &req[..ct_offset + ciphertext.len()])
        .map_err(|_| KeystoreError::CryptoFailure)?;

    let mut resp = [0u8; 2048];
    let (sender, msg_type, len) = crate::ipc::receive_any(&mut resp)
        .map_err(|_| KeystoreError::CryptoFailure)?;

    if sender != keystored_task || msg_type != SERVICE_KEYSTORE_MSG || len < 1 {
        return Err(KeystoreError::CryptoFailure);
    }
    if resp[0] != 0 {
        return Err(KeystoreError::CryptoFailure);
    }
    let pt_len = len - 1;
    out_plaintext[..pt_len].copy_from_slice(&resp[1..len]);
    Ok(pt_len)
}

/// Derives a raw 32-byte domain service key from `keystored` via IPC.
///
/// STRICT ACCESS CONTROL: `keystored` refuses this request with `PermissionDenied`
/// unless the caller is authenticated by the kernel as an authorized system service (`is_system != 0`).
/// Ordinary Ring-3 applications receive `PermissionDenied` and must interact exclusively
/// via opaque `KeyHandle`s.
pub fn keystore_derive_service_key(
    keystored_task: u64,
    domain: u32,
    uid: u32,
) -> Result<[u8; 32], KeystoreError> {
    let mut req = [0u8; 9];
    req[0] = KEYSTORE_OP_DERIVE_SERVICE_KEY;
    req[1..5].copy_from_slice(&domain.to_le_bytes());
    req[5..9].copy_from_slice(&uid.to_le_bytes());

    crate::ipc_send(keystored_task, SERVICE_KEYSTORE_MSG, &req)
        .map_err(|_| KeystoreError::CryptoFailure)?;

    let mut resp = [0u8; 64];
    let (sender, msg_type, len) = crate::ipc::receive_any(&mut resp)
        .map_err(|_| KeystoreError::CryptoFailure)?;

    if sender != keystored_task || msg_type != SERVICE_KEYSTORE_MSG || len < 33 {
        return Err(KeystoreError::CryptoFailure);
    }
    if resp[0] != 0 {
        return Err(KeystoreError::PermissionDenied);
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&resp[1..33]);
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_handle_serialization() {
        let handle = KeyHandle::new(7, 42, DOMAIN_COMPANION, RIGHT_ENCRYPT | RIGHT_DECRYPT);
        let bytes = handle.to_bytes();
        let restored = KeyHandle::from_bytes(&bytes).expect("handle deserialization");
        assert_eq!(restored.slot, 7);
        assert_eq!(restored.generation, 42);
        assert_eq!(restored.domain, DOMAIN_COMPANION);
        assert_eq!(restored.rights, RIGHT_ENCRYPT | RIGHT_DECRYPT);
    }

    #[test]
    fn test_keystore_derive_and_aead_roundtrip() {
        let secret_root = [0x4Au8; 32];
        let mut table = KeystoreTable::<16>::new();

        let handle = table
            .derive_key(
                1000,
                12,
                DOMAIN_COMPANION,
                b"user-profile",
                RIGHT_ENCRYPT | RIGHT_DECRYPT,
                &secret_root,
            )
            .expect("derive should succeed");

        let nonce = [0x01u8; 12];
        let aad = b"header-aad";
        let plaintext = b"sovereign private companion context";
        let mut ciphertext = [0u8; 35];
        let tag = table
            .encrypt(handle, 1000, 12, &nonce, aad, plaintext, &mut ciphertext)
            .expect("encrypt succeeds");

        let mut decrypted = [0u8; 35];
        table
            .decrypt(handle, 1000, 12, &nonce, aad, &ciphertext, &tag, &mut decrypted)
            .expect("decrypt succeeds");
        assert_eq!(&decrypted[..], plaintext);

        // Foreign UID is rejected
        assert_eq!(
            table.decrypt(handle, 1001, 12, &nonce, aad, &ciphertext, &tag, &mut decrypted),
            Err(KeystoreError::PermissionDenied)
        );

        // Tampered ciphertext fails authentication
        let mut tampered_ct = ciphertext;
        tampered_ct[0] ^= 0x01;
        assert_eq!(
            table.decrypt(handle, 1000, 12, &nonce, aad, &tampered_ct, &tag, &mut decrypted),
            Err(KeystoreError::AuthenticationFailed)
        );
    }

    #[test]
    fn test_keystore_sign_and_verify() {
        let secret_root = [0x5Bu8; 32];
        let mut table = KeystoreTable::<16>::new();

        let handle = table
            .derive_key(
                1000,
                15,
                DOMAIN_CONVERSATION_HISTORY,
                b"history-sign",
                RIGHT_SIGN | RIGHT_VERIFY,
                &secret_root,
            )
            .expect("derive sign/verify key");

        let msg = b"signed journal entry at timestamp 17000000";
        let tag = table.sign(handle, 1000, 15, msg).expect("sign succeeds");
        let valid = table.verify(handle, 1000, 15, msg, &tag).expect("verify succeeds");
        assert!(valid);

        let tampered_msg = b"signed journal entry at timestamp 17000001";
        let invalid = table.verify(handle, 1000, 15, tampered_msg, &tag).expect("verify succeeds");
        assert!(!invalid);
    }

    #[test]
    fn test_keystore_destroy_invalidates_future_use() {
        let secret_root = [0x33u8; 32];
        let mut table = KeystoreTable::<16>::new();

        let handle = table
            .derive_key(1000, 10, DOMAIN_APP_STORAGE, b"app-key", RIGHT_ENCRYPT, &secret_root)
            .unwrap();

        table.destroy_key(handle, 1000, 10).expect("destroy succeeds");

        let nonce = [0u8; 12];
        let mut ct = [0u8; 10];
        assert_eq!(
            table.encrypt(handle, 1000, 10, &nonce, b"", b"test", &mut ct),
            Err(KeystoreError::SlotNotFound)
        );
    }
}
