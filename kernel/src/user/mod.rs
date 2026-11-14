// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! InterOS Multi-User & Account Management Subsystem
//!
//! Provides first-boot provisioning, device owner management, stable user IDs,
//! credential abstraction (salted hashing), lock/unlock lifecycle, failed attempt
//! lockout protection, per-user VFS storage partitioning, and guest user data cleanup.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

use crate::fs::VFS;

/// Path of the persisted account map.
///
/// WHY the VFS and not a new journal kind: `storage/persistent.rs` has sixteen
/// slots in ten two-slot record kinds, `tests/test_storage_contract.py` pins
/// both `SLOT_COUNT = 16` and `JOURNAL_MAX_KIND = 10`, so no kind can be added,
/// and every kind 1..=10 is already taken (package registry 1, package
/// revocation 2, publisher trust roots 3, VFS snapshot 4, package blobs 5,
/// keystore seed 6, recovery 7, update manifest 8, update activation 9,
/// settings 10). The credential record already lives in
/// `/user/{uid}/vault/.credential`, and the whole `/user/` inode tree and its
/// payloads are sealed and authenticated by the VFS snapshot, so the map needs
/// no new on-disk region.
///
/// WHY a dot-prefixed name: the map is kernel state, not user data. Apps are
/// confined to `/user/{uid}/apps/{package}` by `enforce_app_path`, so no
/// ring-3 task can open or overwrite this path; the dot matches the convention
/// `.credential` already uses for kernel-owned files in the same tree.
const ACCOUNT_MAP_PATH: &str = "/user/.accounts";
const ACCOUNT_MAP_MAGIC: [u8; 4] = *b"UACC";
const ACCOUNT_MAP_VERSION: u32 = 1;
/// Map header: magic 4, version 4, owner uid 4, next standard uid 4,
/// provisioned flag 1, record count 2.
const ACCOUNT_MAP_HEADER: usize = 19;
/// Record header: uid 4, kind 1, lock state 1, name length 2.
const ACCOUNT_MAP_RECORD: usize = 8;
/// The map is bounded by the sixteen-account ceiling in `create_standard_user`.
const ACCOUNT_MAP_ACCOUNTS: usize = 16;
const ACCOUNT_MAP_NAME_MAX: usize = 255;
const ACCOUNT_MAP_CAPACITY: usize =
    ACCOUNT_MAP_HEADER + ACCOUNT_MAP_ACCOUNTS * (ACCOUNT_MAP_RECORD + ACCOUNT_MAP_NAME_MAX);
/// Sentinel stored when the map holds no owner (it cannot happen through
/// `delete_user`, which refuses the owner, but the format has to say so).
const ACCOUNT_MAP_NO_OWNER: u32 = u32::MAX;

/// Stable user identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UserId(pub u32);

impl UserId {
    pub const SYSTEM: UserId = UserId(0);
    pub const OWNER: UserId = UserId(1000);
    pub const GUEST: UserId = UserId(1001);
    pub const APP_BASE: u32 = 10000;

    #[inline(always)]
    pub const fn is_system(&self) -> bool {
        self.0 == Self::SYSTEM.0
    }

    #[inline(always)]
    pub const fn is_owner(&self) -> bool {
        self.0 == Self::OWNER.0
    }

    #[inline(always)]
    pub const fn is_guest(&self) -> bool {
        self.0 == Self::GUEST.0
    }
}

/// Category of user account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserType {
    Owner,
    Standard,
    Guest,
    Restricted,
}

/// User lock/unlock state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserLockState {
    Uninitialized,
    Locked,
    Unlocked,
    Lockout { locked_until_tick: u64 },
}

/// Credential with cryptographic salt, PBKDF2 hash, and configurable work factor.
///
/// # Security & Trust Boundary Contract
/// - **Storage Isolation Boundary:** Credential records are persisted in the kernel-managed
///   VFS hierarchy at `/user/{uid}/vault/.credential`. Ring 3 user applications run with
///   Zero Ambient Authority and are confined by per-package capability checks (`enforce_app_path`),
///   preventing unprivileged processes from accessing, reading, or tampering with `.credential`.
/// - **Corruption Detection vs. Tamper Resistance:** The 32-bit FNV-1a checksum in format Version 1
///   detects accidental filesystem corruption, flash wear, partial writes, and storage bit-rot.
///   It is an *integrity corruption check*, NOT an authenticated cryptographic MAC. An adversary with
///   offline physical access to raw unencrypted storage could recalculate FNV-1a. Complete offline
///   tamper protection requires storage-layer authenticated encryption keyed via `SecureKeystore`.
/// - **Fail-Closed Policy:** If the credential record cannot be read, format version is unknown,
///   or the checksum fails, the kernel fails closed: the credential is not loaded, the profile
///   remains in `UserLockState::Locked` or `Lockout`, and unauthorized login is strictly denied.
/// - **Password-Hardening Strategy & Threat Model:**
///   - *On-Device Online Defense:* Online brute-force attacks against physical devices are mitigated
///     by kernel-enforced rate-limiting and persistent lockout (maximum 5 failed attempts triggering
///     a 30-second cold-boot-persisting lockout).
///   - *Work Factor Decoupling:* Password verification executes outside the 60fps compositor render
///     loop (asynchronous IPC authentication worker) so work factor is never constrained by a 5ms
///     frame budget.
///   - *Microkernel no_std Bounds & Version 1 Scope:* Version 1 uses 5,000 rounds of PBKDF2-HMAC-SHA256.
///     Because PBKDF2 is compute-bound rather than memory-hard, it does *not* provide resistance against
///     massively parallel GPU/ASIC offline dictionary attacks if raw unencrypted flash is dumped.
///     In the current `#![no_std]` core with constrained kernel stack and zero heap allocations during
///     interactive paths, memory-hard algorithms (e.g. Argon2id requiring 64+ MiB of RAM) cannot run
///     in-kernel.
///   - *Migration Path (Version 2):* The on-disk header contains an explicit format version field.
///     Once userspace authentication architecture is deployed with virtual memory paging, Version 2
///     will introduce Argon2id password hashing and hardware-bound key sealing. Existing Version 1
///     accounts remain fully backwards compatible and will migrate automatically upon unlock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    pub salt: [u8; 16],
    pub hash: [u8; 32],
    pub failed_attempts: u32,
    pub max_failed_attempts: u32,
    pub iterations: u32,
}

/// RAII zeroizing buffer wrapper for sensitive credential records.
/// Ensures the underlying buffer is unconditionally scrubbed with volatile writes
/// on every exit path (early return, error, panic, or normal completion).
pub struct ZeroizingBuffer<const N: usize> {
    buf: [u8; N],
}

impl<const N: usize> ZeroizingBuffer<N> {
    #[inline]
    pub fn new() -> Self {
        Self { buf: [0u8; N] }
    }
}

impl<const N: usize> core::ops::Deref for ZeroizingBuffer<N> {
    type Target = [u8; N];
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.buf
    }
}

impl<const N: usize> core::ops::DerefMut for ZeroizingBuffer<N> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.buf
    }
}

impl<const N: usize> Drop for ZeroizingBuffer<N> {
    #[inline]
    fn drop(&mut self) {
        crate::crypto::zeroize_slice(&mut self.buf);
    }
}

impl Credential {
    /// Default number of PBKDF2-HMAC-SHA256 iterations (RFC 2898 / RFC 8018) for Version 1 records.
    /// Provides baseline password-stretching within no_std microkernel constraints.
    pub const DEFAULT_PBKDF2_ROUNDS: u32 = 5000;
    /// Legacy PBKDF2 iteration count for backwards compatibility and migration.
    pub const LEGACY_PBKDF2_ROUNDS: u32 = 1000;
    pub const PBKDF2_ROUNDS: u32 = Self::DEFAULT_PBKDF2_ROUNDS;

    /// Creates a new credential from raw secret bytes using a random salt and default PBKDF2 work factor.
    pub fn new(secret: &[u8]) -> Self {
        Self::new_with_iterations(secret, Self::DEFAULT_PBKDF2_ROUNDS)
    }

    /// Creates a new credential with explicitly configured PBKDF2 iterations.
    pub fn new_with_iterations(secret: &[u8], iterations: u32) -> Self {
        let mut salt = [0u8; 16];
        crate::crypto::get_random_bytes(&mut salt);

        let mut hash = [0u8; 32];
        crate::crypto::HmacSha256::pbkdf2(secret, &salt, iterations, &mut hash);

        Self {
            salt,
            hash,
            failed_attempts: 0,
            max_failed_attempts: 5,
            iterations,
        }
    }

    /// Verifies candidate secret against stored salt and hash using PBKDF2-HMAC-SHA256
    /// with the credential's own recorded iteration count and constant-time 32-byte equality comparison.
    /// Intermediate secret-dependent buffers are wiped with volatile stores via ZeroizingBuffer.
    pub fn verify(&self, secret: &[u8]) -> bool {
        let mut candidate_guard = ZeroizingBuffer::<32>::new();
        let candidate_hash = &mut *candidate_guard;
        crate::crypto::HmacSha256::pbkdf2(secret, &self.salt, self.iterations, candidate_hash);
        crate::crypto::constant_time_eq_32(candidate_hash, &self.hash)
    }
}

/// User profile record.
#[derive(Debug, Clone)]
pub struct UserProfile {
    pub uid: UserId,
    pub name: String,
    pub user_type: UserType,
    pub lock_state: UserLockState,
    pub credential: Option<Credential>,
    pub storage_path: String,
    pub created_at_tick: u64,
}

/// User management errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserError {
    UserNotFound,
    UserAlreadyExists,
    AuthenticationFailed,
    LockoutActive,
    InvalidCredential,
    PermissionDenied,
    StorageError,
    MaxUsersExceeded,
    CannotDeleteOwner,
    CannotDeleteSystem,
}

/// On-disk tag for `UserType`. The values are an ABI: a decoded tag that is not
/// one of these fails the whole map instead of being guessed at.
fn user_type_tag(kind: UserType) -> u8 {
    match kind {
        UserType::Owner => 0,
        UserType::Standard => 1,
        UserType::Guest => 2,
        UserType::Restricted => 3,
    }
}

fn user_type_from_tag(tag: u8) -> Option<UserType> {
    match tag {
        0 => Some(UserType::Owner),
        1 => Some(UserType::Standard),
        2 => Some(UserType::Guest),
        3 => Some(UserType::Restricted),
        _ => None,
    }
}

/// On-disk tag for `UserLockState`.
///
/// `Lockout` loses its deadline on the way to disk because the deadline is
/// measured in ticks from this boot's counter, which restarts at zero: a
/// persisted `locked_until_tick` can outlive the boot it was written in and lock
/// the owner out of their own device for good.
fn lock_state_tag(state: UserLockState) -> u8 {
    match state {
        UserLockState::Uninitialized => 0,
        UserLockState::Locked => 1,
        UserLockState::Unlocked => 2,
        UserLockState::Lockout { .. } => 3,
    }
}

fn lock_state_from_tag(tag: u8) -> Option<UserLockState> {
    match tag {
        0 => Some(UserLockState::Uninitialized),
        1 => Some(UserLockState::Locked),
        2 => Some(UserLockState::Unlocked),
        3 => Some(UserLockState::Lockout {
            locked_until_tick: 0,
        }),
        _ => None,
    }
}

/// Global user and account subsystem manager.
pub struct UserManager {
    users: BTreeMap<UserId, UserProfile>,
    active_user: UserId,
    owner_user: Option<UserId>,
    is_provisioned: bool,
    next_standard_uid: u32,
}

impl UserManager {
    /// Lockout duration in timer ticks (3000 ticks = 30 seconds at 100 Hz).
    pub const LOCKOUT_TICKS: u64 = 3000;

    pub const fn new() -> Self {
        Self {
            users: BTreeMap::new(),
            active_user: UserId::SYSTEM,
            owner_user: None,
            is_provisioned: false,
            next_standard_uid: 1002,
        }
    }

    /// Returns the currently active (foreground) user.
    pub fn active_user(&self) -> UserId {
        self.active_user
    }

    /// Returns the primary device owner user ID if provisioned.
    pub fn owner_user(&self) -> Option<UserId> {
        self.owner_user
    }

    /// Returns whether first-boot device owner provisioning has completed.
    pub fn is_provisioned(&self) -> bool {
        self.is_provisioned
    }

    /// Provisions the primary device owner account during first boot.
    pub fn provision_owner(
        &mut self,
        name: &str,
        pin: Option<&[u8]>,
        tick: u64,
    ) -> Result<UserId, UserError> {
        if self.is_provisioned {
            return Err(UserError::UserAlreadyExists);
        }

        let uid = UserId::OWNER;
        let storage_path = format!("/user/{}", uid.0);

        // Partition VFS directories for the device owner
        {
            let mut vfs = VFS.lock();
            vfs.mkdir("/user");
            vfs.mkdir(&storage_path);
            vfs.mkdir(&format!("{}/data", storage_path));
            vfs.mkdir(&format!("{}/vault", storage_path));
            vfs.mkdir(&format!("{}/app_data", storage_path));
        }

        // An owner with no credential starts Uninitialized, requiring first-boot PIN setup.
        // Once a credential is set, it transitions to Locked.
        let credential = pin.map(Credential::new);
        let lock_state = if credential.is_some() {
            UserLockState::Unlocked
        } else {
            UserLockState::Uninitialized
        };

        let profile = UserProfile {
            uid,
            name: name.to_string(),
            user_type: UserType::Owner,
            lock_state,
            credential,
            storage_path,
            created_at_tick: tick,
        };

        self.users.insert(uid, profile);
        self.owner_user = Some(uid);
        self.active_user = uid;
        self.is_provisioned = true;

        self.persist_accounts();

        crate::serial_println!(
            "[USER] Device owner provisioned: UID={}, Name='{}'",
            uid.0,
            name
        );
        Ok(uid)
    }

    /// Creates an ephemeral guest user account.
    pub fn create_guest(&mut self, tick: u64) -> Result<UserId, UserError> {
        let uid = UserId::GUEST;
        let storage_path = format!("/user/{}", uid.0);

        // Ensure clean VFS hierarchy for guest
        {
            let mut vfs = VFS.lock();
            vfs.mkdir("/user");
            vfs.mkdir(&storage_path);
            vfs.mkdir(&format!("{}/data", storage_path));
        }

        let profile = UserProfile {
            uid,
            name: "Guest".to_string(),
            user_type: UserType::Guest,
            // Guest sessions are intentionally credential-less and ephemeral.
            // They are not a privilege target: a guest owns no persistent data
            // and `guest_user()` is a distinct uid from the owner.
            lock_state: UserLockState::Unlocked,
            credential: None,
            storage_path,
            created_at_tick: tick,
        };

        self.users.insert(uid, profile);
        self.persist_accounts();
        crate::serial_println!("[USER] Guest account created: UID={}", uid.0);
        Ok(uid)
    }

    /// Creates a standard secondary user account.
    pub fn create_standard_user(
        &mut self,
        name: &str,
        pin: Option<&[u8]>,
        tick: u64,
    ) -> Result<UserId, UserError> {
        if self.users.len() >= 16 {
            return Err(UserError::MaxUsersExceeded);
        }

        let uid = UserId(self.next_standard_uid);
        self.next_standard_uid = self.next_standard_uid.saturating_add(1);
        let storage_path = format!("/user/{}", uid.0);

        {
            let mut vfs = VFS.lock();
            vfs.mkdir("/user");
            vfs.mkdir(&storage_path);
            vfs.mkdir(&format!("{}/data", storage_path));
            vfs.mkdir(&format!("{}/app_data", storage_path));
        }

        // Same rule as the owner: no credential means Locked. An account that is
        // Unlocked with no credential is an account anyone can act as, which is
        // the same escalation the hardcoded owner PIN provided.
        let credential = pin.map(Credential::new);
        let lock_state = if credential.is_some() {
            UserLockState::Locked
        } else {
            UserLockState::Locked
        };

        let profile = UserProfile {
            uid,
            name: name.to_string(),
            user_type: UserType::Standard,
            lock_state,
            credential,
            storage_path,
            created_at_tick: tick,
        };

        self.users.insert(uid, profile);
        self.persist_accounts();
        crate::serial_println!(
            "[USER] Standard secondary account created: UID={}, Name='{}'",
            uid.0,
            name
        );
        Ok(uid)
    }

    /// Switches the active foreground user.
    pub fn switch_user(&mut self, target: UserId) -> Result<(), UserError> {
        let profile = self.users.get(&target).ok_or(UserError::UserNotFound)?;
        self.active_user = target;
        crate::serial_println!(
            "[USER] Switched active foreground user to UID={} ('{}', State={:?})",
            target.0,
            profile.name,
            profile.lock_state
        );
        Ok(())
    }

    /// Unlocks a user account with credential authentication.
    pub fn unlock(
        &mut self,
        uid: UserId,
        secret: &[u8],
        current_tick: u64,
    ) -> Result<(), UserError> {
        let profile = self.users.get_mut(&uid).ok_or(UserError::UserNotFound)?;

        // Check if currently locked out
        if let UserLockState::Lockout { locked_until_tick } = profile.lock_state {
            if current_tick < locked_until_tick {
                return Err(UserError::LockoutActive);
            }
            // Lockout period expired; reset to locked
            profile.lock_state = UserLockState::Locked;
        }

        if profile.credential.is_none() {
            profile.lock_state = UserLockState::Unlocked;
            crate::serial_println!("[USER] User UID={} has no credential; unlocked.", uid.0);
            return Ok(());
        }

        let cred = profile.credential.as_mut().ok_or(UserError::InvalidCredential)?;

        if cred.verify(secret) {
            cred.failed_attempts = 0;
            // Transparent migration to default work factor if old credential had lower rounds
            if cred.iterations < Credential::DEFAULT_PBKDF2_ROUNDS {
                let mut new_hash = [0u8; 32];
                crate::crypto::HmacSha256::pbkdf2(secret, &cred.salt, Credential::DEFAULT_PBKDF2_ROUNDS, &mut new_hash);
                cred.hash = new_hash;
                cred.iterations = Credential::DEFAULT_PBKDF2_ROUNDS;
                crate::crypto::zeroize_slice(&mut new_hash);
                crate::serial_println!(
                    "[USER:SECURITY] Migrated credential for UID={} to {} PBKDF2 rounds.",
                    uid.0,
                    Credential::DEFAULT_PBKDF2_ROUNDS
                );
            }
            let cred_snapshot = cred.clone();
            Self::save_credential_to_vfs(uid, &cred_snapshot);
            profile.lock_state = UserLockState::Unlocked;
            crate::serial_println!("[USER] User UID={} unlocked successfully.", uid.0);
            Ok(())
        } else {
            cred.failed_attempts = cred.failed_attempts.saturating_add(1);
            let cred_snapshot = cred.clone();
            Self::save_credential_to_vfs(uid, &cred_snapshot);
            if cred.failed_attempts >= cred.max_failed_attempts {
                // Lock out for 3000 ticks (30s at 100Hz)
                let lockout_duration = 3000;
                profile.lock_state = UserLockState::Lockout {
                    locked_until_tick: current_tick + lockout_duration,
                };
                crate::serial_println!(
                    "[USER:SECURITY] Max failed attempts exceeded for UID={}. Account locked out until tick {}.",
                    uid.0,
                    current_tick + lockout_duration
                );
                Err(UserError::LockoutActive)
            } else {
                crate::serial_println!(
                    "[USER:SECURITY] Authentication failed for UID={} (attempt {}/{})",
                    uid.0,
                    cred.failed_attempts,
                    cred.max_failed_attempts
                );
                Err(UserError::AuthenticationFailed)
            }
        }
    }

    /// Unlocks account via verified biometric authentication (fingerprint / face).
    pub fn unlock_biometric(&mut self, uid: UserId) -> Result<(), UserError> {
        let profile = self.users.get_mut(&uid).ok_or(UserError::UserNotFound)?;
        if let UserLockState::Lockout { .. } = profile.lock_state {
            return Err(UserError::LockoutActive);
        }
        if let Some(ref mut cred) = profile.credential {
            cred.failed_attempts = 0;
        }
        profile.lock_state = UserLockState::Unlocked;
        crate::serial_println!("[USER] User UID={} unlocked via biometric authentication.", uid.0);
        Ok(())
    }

    /// Locks an unlocked user account.
    pub fn lock(&mut self, uid: UserId) -> Result<(), UserError> {
        let profile = self.users.get_mut(&uid).ok_or(UserError::UserNotFound)?;
        // A credential-less account is forced to Locked. The previous early
        // return left such an account at whatever state it had, so an owner
        // provisioned without a PIN reported Unlocked and could be acted as.
        profile.lock_state = UserLockState::Locked;
        crate::serial_println!("[USER] User UID={} locked.", uid.0);
        Ok(())
    }

    /// Establishes or changes a credential for the account.
    ///
    /// If a credential already exists, `old_secret` must verify before the change is accepted.
    /// If no credential exists (e.g. first-boot provisioning), `old_secret` may be `None`.
    /// The salted credential hash is immediately persisted to the user vault in VFS.
    pub fn change_credential(
        &mut self,
        uid: UserId,
        old_secret: Option<&[u8]>,
        new_secret: &[u8],
    ) -> Result<(), UserError> {
        if new_secret.is_empty() || new_secret.len() > 64 {
            return Err(UserError::InvalidCredential);
        }
        let profile = self.users.get_mut(&uid).ok_or(UserError::UserNotFound)?;
        if let Some(ref existing) = profile.credential {
            let old = old_secret.ok_or(UserError::AuthenticationFailed)?;
            if !existing.verify(old) {
                return Err(UserError::AuthenticationFailed);
            }
        }
        let cred = Credential::new(new_secret);
        profile.credential = Some(cred.clone());
        profile.lock_state = UserLockState::Locked;
        Self::save_credential_to_vfs(uid, &cred);
        crate::serial_println!("[USER] Credential established and persisted for UID={} (account remains locked).", uid.0);
        Ok(())
    }

    /// Whether this account holds a credential at all.
    ///
    /// WHY this is a question of its own rather than something read off
    /// `lock_state`: `Unlocked` does NOT imply a credential. `unlock` sets an
    /// account with no credential to `Unlocked` as well, so a device that has
    /// never had a PIN reports `Unlocked` the moment it is swiped past -- and
    /// deriving "a PIN is set" from that would have Settings claim a lock the
    /// account does not have, on the very screen that is meant to report it.
    pub fn has_credential(&self, uid: UserId) -> Result<bool, UserError> {
        let profile = self.users.get(&uid).ok_or(UserError::UserNotFound)?;
        Ok(profile.credential.is_some())
    }

    /// Establishes a credential on an account that does not yet have one.
    pub fn set_credential(&mut self, uid: UserId, secret: &[u8]) -> Result<(), UserError> {
        self.change_credential(uid, None, secret)
    }

    /// Removes an account's credential, leaving it credential-less again.
    ///
    /// WHY this exists: `change_credential` rejects an empty secret, so before
    /// this there was no way to turn a screen lock off once it had been set --
    /// Settings could add a PIN and never remove one. WHY the old secret is
    /// required: clearing a credential is the one credential operation that
    /// *lowers* what the device demands, handing it to anyone holding it, so it
    /// verifies the current credential exactly where `change_credential` does
    /// rather than trusting the caller.
    ///
    /// The account stays `Locked` afterwards. A credential-less account is
    /// unlocked by the lock screen's swipe (`unlock` succeeds when
    /// `credential.is_none()`), so the device is reachable again without the
    /// state being reported as unlocked before anyone has presented anything.
    pub fn clear_credential(&mut self, uid: UserId, old_secret: &[u8]) -> Result<(), UserError> {
        let profile = self.users.get_mut(&uid).ok_or(UserError::UserNotFound)?;
        let matches = match profile.credential.as_ref() {
            Some(existing) => existing.verify(old_secret),
            None => return Err(UserError::InvalidCredential),
        };
        if !matches {
            crate::serial_println!(
                "[USER:SECURITY] Credential removal refused for UID={}: the current credential did not verify.",
                uid.0
            );
            return Err(UserError::AuthenticationFailed);
        }
        profile.credential = None;
        profile.lock_state = UserLockState::Locked;
        Self::remove_credential_from_vfs(uid);
        crate::serial_println!(
            "[USER:SECURITY] Credential removed for UID={} after verification; the account is credential-less again.",
            uid.0
        );
        Ok(())
    }

    /// Deletes the persisted credential record so a reboot cannot resurrect a PIN
    /// the owner has just removed.
    ///
    /// WHY the outcome is reported, as it is for the write above: a silent
    /// failure here would leave the removed PIN on disk, and the next boot would
    /// come back locked behind a credential Settings no longer reports.
    pub fn remove_credential_from_vfs(uid: UserId) {
        let path = format!("/user/{}/vault/.credential", uid.0);
        let outcome = {
            let mut vfs = VFS.lock();
            vfs.unlink(&path).and_then(|_| vfs.sync())
        };
        match outcome {
            Ok(()) => crate::serial_println!(
                "[USER:SECURITY] Persisted credential removed from {} and flushed to disk.",
                path
            ),
            Err(_) => crate::serial_println!(
                "[USER:SECURITY] Persisted credential could not be removed from {}: the filesystem refused the operation. The account is credential-less for this session only.",
                path
            ),
        }
    }

    /// Computes a 32-bit FNV-1a checksum over persisted credential fields to detect file corruption.
    pub fn compute_cred_checksum(data: &[u8]) -> u32 {
        let mut sum: u32 = 0x811c9dc5;
        for &b in data {
            sum ^= b as u32;
            sum = sum.wrapping_mul(0x01000193);
        }
        sum
    }

    /// Persists the salted credential hash, failed attempts, and work factor to `/user/{uid}/vault/.credential` in VFS.
    pub fn save_credential_to_vfs(uid: UserId, cred: &Credential) {
        let path = format!("/user/{}/vault/.credential", uid.0);
        let mut buf_guard = ZeroizingBuffer::<68>::new();
        let buf = &mut *buf_guard;
        buf[0..4].copy_from_slice(b"UCRD");
        buf[4..8].copy_from_slice(&1u32.to_le_bytes()); // Format Version 1
        buf[8..24].copy_from_slice(&cred.salt);
        buf[24..56].copy_from_slice(&cred.hash);
        buf[56..60].copy_from_slice(&cred.failed_attempts.to_le_bytes());
        buf[60..64].copy_from_slice(&cred.iterations.to_le_bytes());
        let csum = Self::compute_cred_checksum(&buf[0..64]);
        buf[64..68].copy_from_slice(&csum.to_le_bytes());
        let outcome = {
            let mut vfs = VFS.lock();
            vfs.create_file(&path, buf, false);
            vfs.sync()
        };
        // WHY the result is reported: this used to be discarded, so a write the
        // storage layer refused looked exactly like a write that succeeded. The
        // shell then showed a "PIN set" home screen on the next boot while the
        // account on disk still had no credential, and nothing in the log said
        // which of the two had happened.
        match outcome {
            Ok(()) => crate::serial_println!(
                "[USER] Credential persisted to {} and flushed to disk.",
                path
            ),
            Err(_) => crate::serial_println!(
                "[USER:SECURITY] Credential could not be persisted to {}: the \
                 filesystem refused the sync. The account has a credential for \
                 this session only.",
                path
            ),
        }
    }

    /// Restores a previously persisted credential from VFS on boot.
    /// Fails closed if the storage record is corrupt or tampered with.
    /// The temporary credential buffer is automatically and unconditionally wiped
    /// via ZeroizingBuffer RAII Drop on all exit paths (missing/short records,
    /// invalid magic, unknown version, checksum mismatch, or successful load).
    pub fn load_persisted_credential(&mut self, uid: UserId) {
        let path = format!("/user/{}/vault/.credential", uid.0);
        let mut buf_guard = ZeroizingBuffer::<68>::new();
        let buf = &mut *buf_guard;
        let bytes_read = {
            let vfs = VFS.lock();
            vfs.read_file_by_path(&path, buf).unwrap_or(0)
        };
        if bytes_read < 52 || &buf[0..4] != b"UCRD" {
            return;
        }

        let (salt, hash, failed_attempts, iterations) = if bytes_read >= 68 {
            let version = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
            if version != 1 {
                crate::serial_println!("[USER:SECURITY] Unknown credential version {} for UID={}; refusing.", version, uid.0);
                return;
            }
            let expected_csum = u32::from_le_bytes([buf[64], buf[65], buf[66], buf[67]]);
            let actual_csum = Self::compute_cred_checksum(&buf[0..64]);
            if expected_csum != actual_csum {
                crate::serial_println!("[USER:SECURITY] Credential checksum mismatch for UID={}; failing closed (corrupted file).", uid.0);
                if let Some(profile) = self.users.get_mut(&uid) {
                    profile.lock_state = UserLockState::Locked;
                }
                return;
            }
            let mut salt = [0u8; 16];
            salt.copy_from_slice(&buf[8..24]);
            let mut hash = [0u8; 32];
            hash.copy_from_slice(&buf[24..56]);
            let failed_attempts = u32::from_le_bytes([buf[56], buf[57], buf[58], buf[59]]);
            let iterations = u32::from_le_bytes([buf[60], buf[61], buf[62], buf[63]]);
            (salt, hash, failed_attempts, iterations)
        } else {
            // Version 0 legacy format (52-56 bytes)
            let mut salt = [0u8; 16];
            salt.copy_from_slice(&buf[4..20]);
            let mut hash = [0u8; 32];
            hash.copy_from_slice(&buf[20..52]);
            let failed_attempts = if bytes_read >= 56 {
                u32::from_le_bytes([buf[52], buf[53], buf[54], buf[55]])
            } else {
                0
            };
            (salt, hash, failed_attempts, Credential::LEGACY_PBKDF2_ROUNDS)
        };

        if let Some(profile) = self.users.get_mut(&uid) {
            let max_failed = 5;
            let is_locked_out = failed_attempts >= max_failed;
            profile.credential = Some(Credential {
                salt,
                hash,
                failed_attempts,
                max_failed_attempts: max_failed,
                iterations,
            });
            if is_locked_out {
                // Impose reboot-safe lockout penalty (rebooting does NOT clear lockout!)
                profile.lock_state = UserLockState::Lockout { locked_until_tick: Self::LOCKOUT_TICKS };
                crate::serial_println!(
                    "[USER:SECURITY] Restored persisted credential for UID={}. Account is in ACTIVE LOCKOUT ({} failed attempts).",
                    uid.0,
                    failed_attempts
                );
            } else {
                profile.lock_state = UserLockState::Locked;
                crate::serial_println!(
                    "[USER] Restored persisted credential for UID={}. Account is locked.",
                    uid.0
                );
            }
        }
    }

    /// Persists the account map so a reboot restores every account that existed.
    ///
    /// WHY the map is written separately from the credential: the credential is
    /// one account's secret and already had a home in the vault. The map is what
    /// was missing -- `users` is a `BTreeMap` in RAM, so a secondary or guest
    /// account vanished on reboot while its credential stayed on disk and could
    /// never be attached to anything again.
    pub fn persist_accounts(&self) {
        Self::write_account_map(ACCOUNT_MAP_PATH, self);
    }

    /// Restores the account map persisted by `persist_accounts`.
    ///
    /// Returns false when there is nothing restorable -- a fresh disk, or a
    /// record that does not parse -- and leaves the manager untouched so the
    /// caller can fall through to first-boot provisioning.
    pub fn restore_persisted_accounts(&mut self) -> bool {
        Self::restore_account_map(ACCOUNT_MAP_PATH, self)
    }

    /// WHY the path is a parameter rather than the constant at the call sites:
    /// the map lives in the global VFS, which the host test harness shares
    /// between tests running on parallel threads. A per-test path keeps each
    /// test's bytes its own, and the boot path still uses the one constant.
    fn write_account_map(path: &str, mgr: &UserManager) {
        let mut buf = [0u8; ACCOUNT_MAP_CAPACITY];
        let len = mgr.encode_account_map(&mut buf);
        if len == 0 {
            crate::serial_println!(
                "[USER:SECURITY] Account map could not be encoded (an account name is longer than {} bytes); accounts stay RAM-only for this boot.",
                ACCOUNT_MAP_NAME_MAX
            );
            return;
        }
        let outcome = {
            let mut vfs = VFS.lock();
            vfs.mkdir("/user");
            vfs.create_file(path, &buf[..len], false);
            vfs.sync()
        };
        // WHY the result is reported, as it is for the credential write: a silent
        // failure here looks exactly like a successful one, and the next boot
        // then silently drops every secondary and guest account.
        match outcome {
            Ok(()) => crate::serial_println!(
                "[USER] Account map persisted to {} ({} account(s)).",
                path,
                mgr.users.len()
            ),
            Err(_) => crate::serial_println!(
                "[USER:SECURITY] Account map could not be persisted to {}: the filesystem refused the sync. Secondary and guest accounts stay RAM-only for this boot.",
                path
            ),
        }
    }

    fn restore_account_map(path: &str, mgr: &mut UserManager) -> bool {
        // Read the size first: `read_file_by_path` copies only what fits in the
        // caller's buffer, so a map longer than the record capacity would come
        // back truncated and parse as a valid prefix -- a silent subset restore.
        let (buf, bytes_read) = {
            let vfs = VFS.lock();
            let size = vfs.lookup(path).map(|id| vfs.get_size(id)).unwrap_or(0);
            if size == 0 || size > ACCOUNT_MAP_CAPACITY {
                return false;
            }
            let mut buf = [0u8; ACCOUNT_MAP_CAPACITY];
            let read = vfs.read_file_by_path(path, &mut buf).unwrap_or(0);
            (buf, read)
        };
        if !mgr.decode_account_map(&buf[..bytes_read]) {
            return false;
        }
        // A map with no owner is not a state the kernel can write: `delete_user`
        // refuses the owner, so the owner is either in the map or never existed.
        // Treating it as "no map" is the only answer that cannot resurrect an
        // account set the device does not have.
        if mgr.owner_user.is_none() || !mgr.is_provisioned {
            mgr.users.clear();
            mgr.owner_user = None;
            mgr.is_provisioned = false;
            mgr.next_standard_uid = 1002;
            return false;
        }

        // WHY the active user is reset to the owner rather than restored: a
        // reboot must not resume the session that happened to be foreground when
        // the disk was last written. `switch_user` remains available to whoever
        // can present the target account's credential.
        mgr.active_user = mgr.owner_user.unwrap_or(UserId::SYSTEM);

        // WHY the lock state is clamped on the way in: the persisted value is
        // the state at the moment the map was written, but a reboot is a new
        // session. Restoring `Unlocked` would boot straight into whichever
        // account was open, which is the same rule `load_persisted_credential`
        // already applies to the credential itself. `Uninitialized` survives
        // because it means "this account has never had a credential", not "this
        // session is open".
        for profile in mgr.users.values_mut() {
            if matches!(
                profile.lock_state,
                UserLockState::Unlocked | UserLockState::Lockout { .. }
            ) {
                profile.lock_state = UserLockState::Locked;
            }
        }

        // The VFS metadata snapshot already rebuilds the `/user/` tree on mount;
        // this makes the restore self-sufficient if it does not, so the
        // per-account credential files below have a directory to live in.
        let accounts: Vec<(UserId, UserType)> =
            mgr.users.values().map(|p| (p.uid, p.user_type)).collect();
        {
            let mut vfs = VFS.lock();
            vfs.mkdir("/user");
            for (uid, kind) in accounts {
                let storage_path = format!("/user/{}", uid.0);
                vfs.mkdir(&storage_path);
                vfs.mkdir(&format!("{}/data", storage_path));
                vfs.mkdir(&format!("{}/vault", storage_path));
                if kind != UserType::Guest {
                    vfs.mkdir(&format!("{}/app_data", storage_path));
                }
            }
        }

        crate::serial_println!(
            "[USER] Restored {} account(s) from {} (active user UID={}).",
            mgr.users.len(),
            path,
            mgr.active_user.0
        );
        true
    }

    /// Encodes the account map: uid, kind, lock state and display name per account.
    ///
    /// Returns 0 when the map cannot be encoded in the bounded buffer, rather
    /// than writing a truncated map that would restore a subset of accounts.
    fn encode_account_map(&self, out: &mut [u8]) -> usize {
        if out.len() < ACCOUNT_MAP_HEADER {
            return 0;
        }
        out[..4].copy_from_slice(&ACCOUNT_MAP_MAGIC);
        out[4..8].copy_from_slice(&ACCOUNT_MAP_VERSION.to_le_bytes());
        let owner = self.owner_user.map_or(ACCOUNT_MAP_NO_OWNER, |uid| uid.0);
        out[8..12].copy_from_slice(&owner.to_le_bytes());
        out[12..16].copy_from_slice(&self.next_standard_uid.to_le_bytes());
        out[16] = self.is_provisioned as u8;

        let mut pos = ACCOUNT_MAP_HEADER;
        let mut count: u16 = 0;
        for profile in self.users.values() {
            let name = profile.name.as_bytes();
            if name.len() > ACCOUNT_MAP_NAME_MAX {
                return 0;
            }
            if pos + ACCOUNT_MAP_RECORD + name.len() > out.len() {
                break;
            }
            out[pos..pos + 4].copy_from_slice(&profile.uid.0.to_le_bytes());
            out[pos + 4] = user_type_tag(profile.user_type);
            out[pos + 5] = lock_state_tag(profile.lock_state);
            out[pos + 6..pos + 8].copy_from_slice(&(name.len() as u16).to_le_bytes());
            out[pos + 8..pos + 8 + name.len()].copy_from_slice(name);
            pos += ACCOUNT_MAP_RECORD + name.len();
            count = count.saturating_add(1);
        }
        out[17..19].copy_from_slice(&count.to_le_bytes());
        pos
    }

    /// Rebuilds the account map from persisted bytes.
    ///
    /// Every field is bounds-checked against `bytes`, and an unknown tag, a name
    /// that is not UTF-8, or trailing bytes after the last record fail the whole
    /// map rather than restoring part of it: a partial account set is worse than
    /// none, because it would silently demote an existing account to a
    /// first-boot owner and leave its data under another uid.
    fn decode_account_map(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() < ACCOUNT_MAP_HEADER || &bytes[..4] != &ACCOUNT_MAP_MAGIC {
            return false;
        }
        if u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) != ACCOUNT_MAP_VERSION {
            return false;
        }
        let owner_uid = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let mut next_standard_uid = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        let is_provisioned = bytes[16] != 0;
        let count = u16::from_le_bytes([bytes[17], bytes[18]]) as usize;

        let mut users: BTreeMap<UserId, UserProfile> = BTreeMap::new();
        let mut pos = ACCOUNT_MAP_HEADER;
        for _ in 0..count {
            if pos + ACCOUNT_MAP_RECORD > bytes.len() {
                return false;
            }
            let uid = UserId(u32::from_le_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]));
            let Some(user_type) = user_type_from_tag(bytes[pos + 4]) else {
                return false;
            };
            let Some(lock_state) = lock_state_from_tag(bytes[pos + 5]) else {
                return false;
            };
            let name_len = u16::from_le_bytes([bytes[pos + 6], bytes[pos + 7]]) as usize;
            let name_start = pos + ACCOUNT_MAP_RECORD;
            let name_end = match name_start.checked_add(name_len) {
                Some(end) => end,
                None => return false,
            };
            if name_end > bytes.len() {
                return false;
            }
            let Ok(name) = core::str::from_utf8(&bytes[name_start..name_end]) else {
                return false;
            };
            users.insert(
                uid,
                UserProfile {
                    uid,
                    name: name.to_string(),
                    user_type,
                    lock_state,
                    // The credential is restored separately, per account, from
                    // its vault by `load_persisted_credential`.
                    credential: None,
                    storage_path: format!("/user/{}", uid.0),
                    created_at_tick: 0,
                },
            );
            pos = name_end;
        }
        if pos != bytes.len() {
            return false;
        }
        if owner_uid != ACCOUNT_MAP_NO_OWNER && !users.contains_key(&UserId(owner_uid)) {
            return false;
        }
        // WHY the floor: uid 1000 is the owner and 1001 the guest, so a corrupted
        // or zeroed `next_standard_uid` would let `create_standard_user` hand out
        // an id that is already taken and overwrite an existing account.
        if next_standard_uid < 1002 {
            next_standard_uid = 1002;
        }

        self.users = users;
        self.owner_user = if owner_uid == ACCOUNT_MAP_NO_OWNER {
            None
        } else {
            Some(UserId(owner_uid))
        };
        self.next_standard_uid = next_standard_uid;
        self.is_provisioned = is_provisioned;
        true
    }

    /// Queries the lock state of a user.
    pub fn get_lock_state(&self, uid: UserId) -> Result<UserLockState, UserError> {
        self.users
            .get(&uid)
            .map(|u| u.lock_state)
            .ok_or(UserError::UserNotFound)
    }

    /// Cleans up and wipes user data for the given user.
    pub fn user_data_cleanup(&mut self, uid: UserId) -> Result<(), UserError> {
        if uid.is_system() {
            return Err(UserError::CannotDeleteSystem);
        }
        let storage_path = format!("/user/{}", uid.0);
        {
            let mut vfs = VFS.lock();
            let _ = vfs.rmdir(&format!("{}/data", storage_path));
            let _ = vfs.rmdir(&format!("{}/vault", storage_path));
            let _ = vfs.rmdir(&format!("{}/app_data", storage_path));
            let _ = vfs.rmdir(&storage_path);
        }
        crate::serial_println!("[USER] Storage wiped for UID={}", uid.0);
        Ok(())
    }

    /// Deletes a secondary or guest user account.
    pub fn delete_user(&mut self, uid: UserId) -> Result<(), UserError> {
        if uid.is_system() {
            return Err(UserError::CannotDeleteSystem);
        }
        if uid.is_owner() {
            return Err(UserError::CannotDeleteOwner);
        }

        self.user_data_cleanup(uid)?;
        self.users.remove(&uid).ok_or(UserError::UserNotFound)?;

        if self.active_user == uid {
            self.active_user = self.owner_user.unwrap_or(UserId::SYSTEM);
        }

        // WHY the map is rewritten here: `user_data_cleanup` above already
        // removed the account's directories, and leaving the map untouched would
        // bring the deleted account back on the next boot with its data gone.
        self.persist_accounts();
        crate::serial_println!("[USER] User UID={} deleted.", uid.0);
        Ok(())
    }

    /// Performs full factory reset: removes all user accounts and resets provisioning.
    pub fn factory_reset(&mut self) -> Result<(), UserError> {
        let uids: Vec<UserId> = self.users.keys().copied().collect();
        for uid in uids {
            if !uid.is_system() {
                let _ = self.user_data_cleanup(uid);
            }
        }
        self.users.clear();
        self.owner_user = None;
        self.active_user = UserId::SYSTEM;
        self.is_provisioned = false;
        // WHY the reset state is written rather than the map being unlinked:
        // `user_data_cleanup` removes the per-user directories, but the map is
        // one file outside them, so leaving it behind would make a factory reset
        // undo itself on the next boot. Writing the empty map is also the form
        // that survives: `unlink` undoes itself when the VFS metadata snapshot
        // is refused, while `create_file` does not.
        self.persist_accounts();
        crate::serial_println!("[USER:FACTORY_RESET] User accounts reset to pristine factory state.");
        Ok(())
    }

    /// Resolves app-private storage directory for given user and package.
    pub fn resolve_app_storage(&self, uid: UserId, package_id: &str) -> String {
        format!("/user/{}/app_data/{}", uid.0, package_id)
    }

    /// Queries a user profile.
    pub fn get_user(&self, uid: UserId) -> Option<&UserProfile> {
        self.users.get(&uid)
    }

    /// Returns list of all user profiles.
    pub fn list_users(&self) -> Vec<UserProfile> {
        self.users.values().cloned().collect()
    }
}

pub static USER_MANAGER: Mutex<UserManager> = Mutex::new(UserManager::new());

/// Boot-time initialization of the User & Account Manager.
pub fn init() {
    let mut mgr = USER_MANAGER.lock();
    if !mgr.is_provisioned() {
        // WHY the map is restored before anything else: the map is what makes
        // the owner, every secondary user and the guest account exist at all,
        // and a credential can only be attached to a profile that is already in
        // the map. Restoring the owner first and the rest never is what made
        // secondary and guest accounts RAM-only.
        let restored = mgr.restore_persisted_accounts();
        if !restored {
            let _ = mgr.provision_owner("Device Owner", None, 0);
        }
        // Every account, not just the owner: `change_credential` writes each
        // account's credential to that account's own vault, so restoring only
        // the owner's would bring a secondary account back as credential-less
        // even though it has a PIN on disk -- and `unlock` opens a
        // credential-less account on a swipe.
        let uids: Vec<UserId> = mgr.users.keys().copied().collect();
        for uid in uids {
            mgr.load_persisted_credential(uid);
        }
        let owner = mgr.owner_user().unwrap_or(UserId::OWNER);
        if mgr.get_user(owner).and_then(|u| u.credential.as_ref()).is_none() {
            crate::serial_println!(
                "[USER:SECURITY] Device owner provisioned with NO credential. \
                 The lock screen unlocks on a swipe; a PIN is set from Settings > Security."
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manager holding one freshly provisioned owner with no credential.
    fn owner_without_credential() -> UserManager {
        let mut mgr = UserManager::new();
        mgr.provision_owner("Device Owner", None, 0).expect("provision owner");
        mgr
    }

    /// WHY this is a test and not a comment: the Settings screen for the screen
    /// lock once derived "a PIN is set" from the lock state, which reported
    /// `Unlocked` for exactly this account -- so the panel claimed a lock the
    /// device never had. `Unlocked` says the session is open, not that a
    /// credential exists.
    #[test]
    fn unlocking_a_credential_less_account_does_not_imply_a_credential() {
        let mut mgr = owner_without_credential();
        assert_eq!(mgr.has_credential(UserId::OWNER), Ok(false));
        assert!(mgr.unlock(UserId::OWNER, b"", 0).is_ok());
        assert_eq!(mgr.get_lock_state(UserId::OWNER).ok(), Some(UserLockState::Unlocked));
        // The whole point: still no credential, despite being Unlocked.
        assert_eq!(mgr.has_credential(UserId::OWNER), Ok(false));
    }

    #[test]
    fn a_credential_is_established_and_then_verified() {
        let mut mgr = owner_without_credential();
        mgr.set_credential(UserId::OWNER, b"123456").expect("set credential");
        assert_eq!(mgr.has_credential(UserId::OWNER), Ok(true));
        assert!(mgr.unlock(UserId::OWNER, b"123456", 0).is_ok());
        assert!(mgr.unlock(UserId::OWNER, b"654321", 0).is_err());
    }

    #[test]
    fn changing_a_credential_requires_the_current_one() {
        let mut mgr = owner_without_credential();
        mgr.set_credential(UserId::OWNER, b"123456").expect("set credential");
        assert_eq!(
            mgr.change_credential(UserId::OWNER, Some(b"000000"), b"654321"),
            Err(UserError::AuthenticationFailed)
        );
        // The old credential is still the one in force.
        assert!(mgr.unlock(UserId::OWNER, b"123456", 0).is_ok());
        assert!(mgr
            .change_credential(UserId::OWNER, Some(b"123456"), b"654321")
            .is_ok());
        assert!(mgr.unlock(UserId::OWNER, b"654321", 0).is_ok());
    }

    #[test]
    fn clearing_a_credential_requires_the_current_one() {
        let mut mgr = owner_without_credential();
        mgr.set_credential(UserId::OWNER, b"123456").expect("set credential");
        assert_eq!(
            mgr.clear_credential(UserId::OWNER, b"000000"),
            Err(UserError::AuthenticationFailed)
        );
        assert_eq!(mgr.has_credential(UserId::OWNER), Ok(true));
        mgr.clear_credential(UserId::OWNER, b"123456")
            .expect("clear credential");
        assert_eq!(mgr.has_credential(UserId::OWNER), Ok(false));
    }

    #[test]
    fn clearing_refuses_an_account_that_has_no_credential() {
        let mut mgr = owner_without_credential();
        assert_eq!(
            mgr.clear_credential(UserId::OWNER, b"123456"),
            Err(UserError::InvalidCredential)
        );
    }

    #[test]
    fn an_empty_credential_is_never_accepted() {
        let mut mgr = owner_without_credential();
        assert_eq!(
            mgr.set_credential(UserId::OWNER, b""),
            Err(UserError::InvalidCredential)
        );
        assert_eq!(mgr.has_credential(UserId::OWNER), Ok(false));
    }

    /// A manager holding an owner, a secondary user and a guest.
    fn fully_populated() -> UserManager {
        let mut mgr = UserManager::new();
        mgr.provision_owner("Device Owner", Some(b"1234"), 0)
            .expect("provision owner");
        mgr.create_standard_user("Second Person", Some(b"2222"), 1)
            .expect("create standard user");
        mgr.create_guest(2).expect("create guest");
        mgr
    }

    /// Writes `mgr`'s account map to a test path and returns the encoded length.
    fn write_map_to(mgr: &UserManager, path: &str) {
        let mut buf = [0u8; ACCOUNT_MAP_CAPACITY];
        let len = mgr.encode_account_map(&mut buf);
        assert!(len >= ACCOUNT_MAP_HEADER, "the encoded map has a header");
        let mut vfs = VFS.lock();
        vfs.mkdir("/user");
        vfs.create_file(path, &buf[..len], false);
    }

    /// The bug this pins: a secondary user, a guest and the owner's uid
    /// allocation all used to live only in the `users` map in RAM, so a reboot
    /// restored the owner's credential and nothing else.
    #[test]
    fn a_reboot_restores_every_account_that_existed() {
        let path = "/user/.accounts.test_reboot";
        let mgr = fully_populated();
        let standard_uid = mgr
            .list_users()
            .into_iter()
            .find(|p| p.user_type == UserType::Standard)
            .map(|p| p.uid)
            .expect("a secondary account was created");
        write_map_to(&mgr, path);

        // A fresh manager is the reboot.
        let mut restored = UserManager::new();
        assert!(UserManager::restore_account_map(path, &mut restored));
        assert_eq!(restored.is_provisioned(), true);
        assert_eq!(restored.owner_user(), Some(UserId::OWNER));
        assert_eq!(restored.active_user(), UserId::OWNER);

        let accounts: Vec<(u32, UserType, String)> = restored
            .list_users()
            .into_iter()
            .map(|p| (p.uid.0, p.user_type, p.name))
            .collect();
        assert_eq!(accounts.len(), 3, "owner, secondary and guest all came back");
        assert!(accounts.contains(&(1000, UserType::Owner, "Device Owner".to_string())), "{:?}", accounts);
        assert!(
            accounts.contains(&(standard_uid.0, UserType::Standard, "Second Person".to_string())),
            "{:?}",
            accounts
        );
        assert!(accounts.contains(&(1001, UserType::Guest, "Guest".to_string())), "{:?}", accounts);

        // The uid allocator must not hand out an id that is already in the map.
        assert!(restored.next_standard_uid > standard_uid.0);
    }

    /// A reboot is a new session, so no account may come back unlocked.
    #[test]
    fn a_restored_account_is_never_unlocked() {
        let path = "/user/.accounts.test_lock_clamp";
        let mgr = fully_populated();
        // The owner was provisioned with a PIN, which `provision_owner` leaves
        // Unlocked; the guest is Unlocked by design.
        assert_eq!(mgr.get_lock_state(UserId::OWNER).ok(), Some(UserLockState::Unlocked));
        assert_eq!(mgr.get_lock_state(UserId::GUEST).ok(), Some(UserLockState::Unlocked));
        write_map_to(&mgr, path);

        let mut restored = UserManager::new();
        assert!(UserManager::restore_account_map(path, &mut restored));
        for profile in restored.list_users() {
            assert_eq!(
                profile.lock_state,
                UserLockState::Locked,
                "UID={} came back unlocked",
                profile.uid.0
            );
        }

        // `Uninitialized` is not an open session -- it means the account has
        // never had a credential -- so it survives the round trip.
        let mgr = owner_without_credential();
        write_map_to(&mgr, path);
        let mut restored = UserManager::new();
        assert!(UserManager::restore_account_map(path, &mut restored));
        assert_eq!(
            restored.get_lock_state(UserId::OWNER).ok(),
            Some(UserLockState::Uninitialized)
        );
    }

    /// A deleted account must leave the map, or the next boot brings it back
    /// with its directories already gone.
    #[test]
    fn a_deleted_account_does_not_come_back() {
        let path = "/user/.accounts.test_delete";
        let mut mgr = fully_populated();
        let standard_uid = mgr
            .list_users()
            .into_iter()
            .find(|p| p.user_type == UserType::Standard)
            .map(|p| p.uid)
            .expect("a secondary account was created");
        mgr.delete_user(standard_uid).expect("delete the secondary user");

        write_map_to(&mgr, path);
        let mut restored = UserManager::new();
        assert!(UserManager::restore_account_map(path, &mut restored));
        assert!(restored.get_user(standard_uid).is_none());
        assert_eq!(restored.list_users().len(), 2);
    }

    /// A factory reset must be durable: the map goes back to saying "no
    /// accounts", or the next boot brings back the accounts the reset erased.
    #[test]
    fn a_factory_reset_removes_the_persisted_map() {
        let path = "/user/.accounts.test_factory_reset";
        let mut mgr = fully_populated();
        write_map_to(&mgr, path);

        mgr.factory_reset().expect("factory reset");
        write_map_to(&mgr, path);

        let mut restored = UserManager::new();
        assert_eq!(UserManager::restore_account_map(path, &mut restored), false);
        assert_eq!(restored.is_provisioned(), false);
        assert_eq!(restored.list_users().len(), 0);
    }

    /// A map that is corrupt, truncated or from a different format must not be
    /// partly restored.
    #[test]
    fn an_unparseable_map_is_refused_rather_than_partly_restored() {
        let populated = fully_populated();
        let mut buf = [0u8; ACCOUNT_MAP_CAPACITY];
        let len = populated.encode_account_map(&mut buf);
        assert!(len > ACCOUNT_MAP_HEADER);

        let mut mgr = UserManager::new();
        // Truncated in the middle of the last record.
        assert_eq!(mgr.decode_account_map(&buf[..len - 1]), false);
        // A format version this kernel does not know.
        let mut wrong_version = buf;
        wrong_version[4] = 0x7F;
        assert_eq!(mgr.decode_account_map(&wrong_version[..len]), false);
        // Trailing bytes after the last record.
        let mut longer = alloc::vec![0u8; len + 1];
        longer[..len].copy_from_slice(&buf[..len]);
        assert_eq!(mgr.decode_account_map(&longer), false);
        // A map that names an owner the records do not contain.
        let mut orphan_owner = buf;
        orphan_owner[8] = 0x2A;
        assert_eq!(mgr.decode_account_map(&orphan_owner[..len]), false);

        assert_eq!(mgr.list_users().len(), 0, "nothing was restored");
    }

    #[test]
    fn test_credential_pbkdf2_lockout_and_rate_limiting() {
        let mut mgr = UserManager::new();
        mgr.provision_owner("Device Owner", Some(b"correct_pin_9876"), 0).expect("provision");

        // 1 to 4 failed attempts should fail with AuthenticationFailed
        for attempt in 1..=4 {
            let res = mgr.unlock(UserId::OWNER, b"wrong_pin", 100);
            assert_eq!(res, Err(UserError::AuthenticationFailed), "attempt {attempt} must fail");
            assert_eq!(mgr.get_user(UserId::OWNER).unwrap().credential.as_ref().unwrap().failed_attempts, attempt);
        }

        // 5th failed attempt must trigger LockoutActive
        let res5 = mgr.unlock(UserId::OWNER, b"wrong_pin", 100);
        assert_eq!(res5, Err(UserError::LockoutActive), "5th attempt must trigger Lockout");

        // While locked out, even the correct PIN must be rejected
        let res_locked = mgr.unlock(UserId::OWNER, b"correct_pin_9876", 200);
        assert_eq!(res_locked, Err(UserError::LockoutActive));

        // After lockout duration expires (tick 3101 > 3100), correct PIN succeeds and resets failed_attempts
        let res_expired = mgr.unlock(UserId::OWNER, b"correct_pin_9876", 3101);
        assert_eq!(res_expired, Ok(()));
        assert_eq!(mgr.get_user(UserId::OWNER).unwrap().credential.as_ref().unwrap().failed_attempts, 0);
        assert_eq!(mgr.get_user(UserId::OWNER).unwrap().lock_state, UserLockState::Unlocked);
    }

    #[test]
    fn test_lockout_persists_across_reboot_simulation() {
        let mut mgr = UserManager::new();
        mgr.provision_owner("Device Owner", Some(b"correct_pin_9876"), 0).expect("provision");

        // Fail 5 times to trigger lockout
        for _ in 1..=5 {
            let _ = mgr.unlock(UserId::OWNER, b"wrong_pin", 100);
        }
        assert!(matches!(
            mgr.get_user(UserId::OWNER).unwrap().lock_state,
            UserLockState::Lockout { .. }
        ));

        // Simulate reboot: create fresh UserManager and reload accounts/credentials from VFS
        let mut rebooted_mgr = UserManager::new();
        rebooted_mgr.provision_owner("Device Owner", None, 0).expect("provision stub");
        rebooted_mgr.load_persisted_credential(UserId::OWNER);

        // Verification: rebooted manager MUST restore Lockout state, preventing reboot bypass
        let profile = rebooted_mgr.get_user(UserId::OWNER).expect("user exists");
        assert!(matches!(profile.lock_state, UserLockState::Lockout { .. }), "reboot must preserve Lockout state");
        assert_eq!(profile.credential.as_ref().unwrap().failed_attempts, 5);

        // While locked out on rebooted device, authentication is rejected
        let res = rebooted_mgr.unlock(UserId::OWNER, b"correct_pin_9876", 200);
        assert_eq!(res, Err(UserError::LockoutActive));
    }

    #[test]
    fn test_biometric_unlock_rejected_during_lockout() {
        let mut mgr = UserManager::new();
        mgr.provision_owner("Device Owner", Some(b"correct_pin_9876"), 0).expect("provision");

        // Fail 5 times to trigger lockout
        for _ in 1..=5 {
            let _ = mgr.unlock(UserId::OWNER, b"wrong_pin", 100);
        }
        assert!(matches!(
            mgr.get_user(UserId::OWNER).unwrap().lock_state,
            UserLockState::Lockout { .. }
        ));

        // Biometric unlock attempt during active lockout MUST fail closed
        let bio_res = mgr.unlock_biometric(UserId::OWNER);
        assert_eq!(bio_res, Err(UserError::LockoutActive), "Biometric bypass must be rejected during lockout");
    }

    #[test]
    fn test_correct_password_rejected_during_active_lockout() {
        let mut mgr = UserManager::new();
        mgr.provision_owner("Device Owner", Some(b"correct_pin_9876"), 0).expect("provision");

        // Trigger lockout at tick 100
        for _ in 1..=5 {
            let _ = mgr.unlock(UserId::OWNER, b"wrong_pin", 100);
        }

        // Even with the 100% correct password at tick 500 (< 3100), unlock is rejected
        let res = mgr.unlock(UserId::OWNER, b"correct_pin_9876", 500);
        assert_eq!(res, Err(UserError::LockoutActive), "Correct password must not bypass active lockout");
    }

    #[test]
    fn test_credential_v1_checksum_and_migration() {
        // Provision user with legacy 1000 rounds
        let legacy_cred = Credential::new_with_iterations(b"legacy_secret", Credential::LEGACY_PBKDF2_ROUNDS);
        assert_eq!(legacy_cred.iterations, 1000);
        UserManager::save_credential_to_vfs(UserId::OWNER, &legacy_cred);

        let mut reloaded = UserManager::new();
        reloaded.provision_owner("Device Owner", None, 0).expect("provision");
        reloaded.load_persisted_credential(UserId::OWNER);

        let profile = reloaded.get_user(UserId::OWNER).unwrap();
        assert_eq!(profile.credential.as_ref().unwrap().iterations, 1000);

        // Unlock with correct password triggers transparent migration to 5,000 rounds
        let res = reloaded.unlock(UserId::OWNER, b"legacy_secret", 10);
        assert_eq!(res, Ok(()));
        let migrated_cred = reloaded.get_user(UserId::OWNER).unwrap().credential.as_ref().unwrap();
        assert_eq!(migrated_cred.iterations, Credential::DEFAULT_PBKDF2_ROUNDS);
    }

    #[test]
    fn test_corrupted_credential_fails_closed() {
        let mut mgr = UserManager::new();
        mgr.provision_owner("Device Owner", Some(b"valid_pin_1234"), 0).expect("provision");
        UserManager::save_credential_to_vfs(UserId::OWNER, mgr.get_user(UserId::OWNER).unwrap().credential.as_ref().unwrap());

        // Corrupt the persisted credential file in VFS
        let path = "/user/1000/vault/.credential";
        let mut corrupted_buf = [0u8; 68];
        let bytes_read = {
            let vfs = VFS.lock();
            vfs.read_file_by_path(path, &mut corrupted_buf).unwrap_or(0)
        };
        assert_eq!(bytes_read, 68);
        // Tamper with the hash bytes
        corrupted_buf[30] ^= 0xFF;
        {
            let mut vfs = VFS.lock();
            vfs.create_file(path, &corrupted_buf, false);
            let _ = vfs.sync();
        }

        // Fresh reboot
        let mut rebooted = UserManager::new();
        rebooted.provision_owner("Device Owner", None, 0).expect("provision");
        rebooted.load_persisted_credential(UserId::OWNER);

        // Account must fail closed: no valid credential loaded, state remains Locked
        let profile = rebooted.get_user(UserId::OWNER).unwrap();
        assert_eq!(profile.lock_state, UserLockState::Locked);
        assert!(profile.credential.is_none(), "Corrupted credential must not be loaded");
    }

    #[test]
    fn test_zeroizing_buffer_raii_drop() {
        let raw_ptr;
        {
            let mut guard = ZeroizingBuffer::<64>::new();
            for (i, byte) in guard.iter_mut().enumerate() {
                *byte = (i as u8) ^ 0xAA;
            }
            assert_ne!(guard[0], 0);
            raw_ptr = guard.as_mut_ptr();
            // Drop executes at end of scope
        }
        unsafe {
            let slice = core::slice::from_raw_parts(raw_ptr, 64);
            assert!(slice.iter().all(|&b| b == 0), "ZeroizingBuffer must zeroize on drop");
        }
    }
}
