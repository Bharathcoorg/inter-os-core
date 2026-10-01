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

use crate::crypto::Sha256;
use crate::fs::VFS;

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

/// Credential with cryptographic salt and hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    pub salt: [u8; 16],
    pub hash: [u8; 32],
    pub failed_attempts: u32,
    pub max_failed_attempts: u32,
}

impl Credential {
    /// Creates a new credential from raw secret bytes using a random salt.
    ///
    /// WHY the salt comes from the hardware TRNG: the previous salt was derived
    /// from a caller-supplied `tick ^ constant`, and the kernel calls this at
    /// first boot where the tick counter is still zero. That made the salt a
    /// fixed, publicly known constant, so the stored value was a single
    /// unsalted-equivalent SHA-256 of the secret and identical secrets produced
    /// identical hashes across every boot and every device.
    ///
    /// The `secret` is retained only as a transient argument. It is never
    /// stored, and no default or derived secret is ever supplied by the kernel.
    pub fn new(secret: &[u8]) -> Self {
        let mut salt = [0u8; 16];
        crate::crypto::get_random_bytes(&mut salt);

        let mut hasher_input = Vec::with_capacity(16 + secret.len());
        hasher_input.extend_from_slice(&salt);
        hasher_input.extend_from_slice(secret);
        let hash = Sha256::digest(&hasher_input);

        Self {
            salt,
            hash,
            failed_attempts: 0,
            max_failed_attempts: 5,
        }
    }

    /// Verifies candidate secret against stored salt and hash.
    pub fn verify(&self, secret: &[u8]) -> bool {
        let mut hasher_input = Vec::with_capacity(16 + secret.len());
        hasher_input.extend_from_slice(&self.salt);
        hasher_input.extend_from_slice(secret);
        let candidate_hash = Sha256::digest(&hasher_input);
        candidate_hash == self.hash
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

/// Global user and account subsystem manager.
pub struct UserManager {
    users: BTreeMap<UserId, UserProfile>,
    active_user: UserId,
    owner_user: Option<UserId>,
    is_provisioned: bool,
    next_standard_uid: u32,
}

impl UserManager {
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

        // A provisioned owner with no credential starts LOCKED, not Unlocked.
        // WHY: the account must never be reachable without an out-of-band
        // credential. A caller that omits the PIN gets an account that exists
        // (so storage paths and uid allocation work) but is explicitly locked
        // until `set_credential` supplies a real secret and `unlock` presents it.
        let credential = pin.map(Credential::new);
        let lock_state = if credential.is_some() {
            UserLockState::Unlocked
        } else {
            UserLockState::Locked
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

        let cred = profile.credential.as_mut().ok_or(UserError::InvalidCredential)?;

        if cred.verify(secret) {
            cred.failed_attempts = 0;
            profile.lock_state = UserLockState::Unlocked;
            crate::serial_println!("[USER] User UID={} unlocked successfully.", uid.0);
            Ok(())
        } else {
            cred.failed_attempts = cred.failed_attempts.saturating_add(1);
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

    /// Establishes a credential on an account that does not yet have one.
    ///
    /// WHY this exists: the kernel must not ship a default secret, so the owner
    /// account is provisioned locked and credential-less. This is the only way
    /// to give it a real credential, and it leaves the account Locked, so the
    /// secret still has to be presented through `unlock`.
    ///
    /// Callers MUST have already authorized this operation; this function
    /// performs no capability check of its own and must not be reachable from a
    /// path that has not.
    pub fn set_credential(&mut self, uid: UserId, secret: &[u8]) -> Result<(), UserError> {
        if secret.is_empty() {
            return Err(UserError::InvalidCredential);
        }
        let profile = self.users.get_mut(&uid).ok_or(UserError::UserNotFound)?;
        profile.credential = Some(Credential::new(secret));
        // Stay Locked: setting a credential must not itself grant access.
        profile.lock_state = UserLockState::Locked;
        crate::serial_println!("[USER] Credential established for UID={} (account remains locked).", uid.0);
        Ok(())
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
        // Provision the owner account with NO credential.
        //
        // WHY: the previous call passed a hardcoded `Some(b"1234")`. That made
        // the device owner's account unlockable by anyone who typed four digits,
        // and since the salt was derived from the boot tick (zero at first
        // boot) the stored hash was a fixed, publicly reproducible value. The
        // account still exists so uid allocation and `/user/1000` storage paths
        // work, but it is Locked and has no secret; `UserManager::set_credential`
        // is the only way to give it one, and it must be called from an
        // authenticated provisioning path, not from first boot.
        let _ = mgr.provision_owner("Device Owner", None, 0);
        crate::serial_println!(
            "[USER:SECURITY] Device owner provisioned with NO default credential. \
             Account is locked until a real credential is established."
        );
    }
}
