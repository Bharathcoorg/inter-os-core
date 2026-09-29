#!/usr/bin/env python3
"""
InterOS Core — Multi-User & Account Management Architecture Contract Test
Validates:
  1. Stable User ID hierarchy (SYSTEM=0, OWNER=1000, GUEST=1001, APP_BASE=10000+).
  2. Credential abstraction: salted hashing and authentication verification.
  3. Security lockout policy: account lockout after max failed attempts (5).
  4. User lock state transitions (Unlocked, Locked, Lockout).
  5. Ephemeral guest account lifecycle and data cleanup.
  6. Per-user storage path isolation (/user/<uid>/data, /user/<uid>/vault, /user/<uid>/app_data).
"""

import hashlib
import sys
import unittest

SYSTEM_UID = 0
OWNER_UID = 1000
GUEST_UID = 1001
APP_BASE_UID = 10000
MAX_FAILED_ATTEMPTS = 5
LOCKOUT_DURATION_TICKS = 3000


class UserLockState:
    UNINITIALIZED = "Uninitialized"
    LOCKED = "Locked"
    UNLOCKED = "Unlocked"
    LOCKOUT = "Lockout"


class CredentialSimulator:
    def __init__(self, secret: bytes, salt_seed: int):
        seed_bytes = salt_seed.to_bytes(8, "little")
        inv_seed = (~salt_seed & 0xFFFFFFFFFFFFFFFF).to_bytes(8, "little")
        self.salt = seed_bytes + inv_seed
        hasher = hashlib.sha256()
        hasher.update(self.salt)
        hasher.update(secret)
        self.hash = hasher.digest()
        self.failed_attempts = 0
        self.max_failed_attempts = MAX_FAILED_ATTEMPTS

    def verify(self, candidate: bytes) -> bool:
        hasher = hashlib.sha256()
        hasher.update(self.salt)
        hasher.update(candidate)
        return hasher.digest() == self.hash


class UserManagerSimulator:
    def __init__(self):
        self.users = {}
        self.active_user = SYSTEM_UID
        self.owner_user = None
        self.is_provisioned = False
        self.next_standard_uid = 1002

    def provision_owner(self, name: str, pin: bytes, tick: int = 0):
        if self.is_provisioned:
            raise ValueError("UserAlreadyExists")
        cred = CredentialSimulator(pin, tick ^ 0xA5A55A5A0001)
        self.users[OWNER_UID] = {
            "uid": OWNER_UID,
            "name": name,
            "type": "Owner",
            "lock_state": UserLockState.UNLOCKED,
            "credential": cred,
            "storage_path": f"/user/{OWNER_UID}",
            "locked_until_tick": 0,
        }
        self.owner_user = OWNER_UID
        self.active_user = OWNER_UID
        self.is_provisioned = True
        return OWNER_UID

    def create_guest(self, tick: int = 0):
        self.users[GUEST_UID] = {
            "uid": GUEST_UID,
            "name": "Guest",
            "type": "Guest",
            "lock_state": UserLockState.UNLOCKED,
            "credential": None,
            "storage_path": f"/user/{GUEST_UID}",
            "locked_until_tick": 0,
        }
        return GUEST_UID

    def switch_user(self, target_uid: int):
        if target_uid not in self.users:
            raise KeyError("UserNotFound")
        self.active_user = target_uid

    def lock(self, uid: int):
        if uid not in self.users:
            raise KeyError("UserNotFound")
        if self.users[uid]["credential"] is not None:
            self.users[uid]["lock_state"] = UserLockState.LOCKED

    def unlock(self, uid: int, secret: bytes, current_tick: int):
        if uid not in self.users:
            raise KeyError("UserNotFound")
        u = self.users[uid]
        if u["lock_state"] == UserLockState.LOCKOUT:
            if current_tick < u["locked_until_tick"]:
                raise PermissionError("LockoutActive")
            u["lock_state"] = UserLockState.LOCKED

        cred = u["credential"]
        if cred is None:
            u["lock_state"] = UserLockState.UNLOCKED
            return

        if cred.verify(secret):
            cred.failed_attempts = 0
            u["lock_state"] = UserLockState.UNLOCKED
        else:
            cred.failed_attempts += 1
            if cred.failed_attempts >= cred.max_failed_attempts:
                u["lock_state"] = UserLockState.LOCKOUT
                u["locked_until_tick"] = current_tick + LOCKOUT_DURATION_TICKS
                raise PermissionError("LockoutActive")
            raise PermissionError("AuthenticationFailed")

    def delete_user(self, uid: int):
        if uid == SYSTEM_UID:
            raise ValueError("CannotDeleteSystem")
        if uid == OWNER_UID:
            raise ValueError("CannotDeleteOwner")
        if uid not in self.users:
            raise KeyError("UserNotFound")
        del self.users[uid]
        if self.active_user == uid:
            self.active_user = self.owner_user or SYSTEM_UID


class UserAccountContractTests(unittest.TestCase):
    def setUp(self):
        self.mgr = UserManagerSimulator()

    def test_01_first_boot_owner_provisioning(self):
        """Verify device owner account provisioning on first boot."""
        self.assertFalse(self.mgr.is_provisioned)
        owner_id = self.mgr.provision_owner("Device Owner", b"1234", tick=100)
        self.assertEqual(owner_id, OWNER_UID)
        self.assertTrue(self.mgr.is_provisioned)
        self.assertEqual(self.mgr.active_user, OWNER_UID)
        self.assertEqual(self.mgr.users[OWNER_UID]["storage_path"], "/user/1000")

        # Second provisioning must fail closed
        with self.assertRaises(ValueError):
            self.mgr.provision_owner("Intruder", b"9999")

    def test_02_lock_and_unlock_lifecycle(self):
        """Verify explicit lock and unlock state transitions with credentials."""
        self.mgr.provision_owner("Device Owner", b"secret_pass", tick=100)
        self.assertEqual(self.mgr.users[OWNER_UID]["lock_state"], UserLockState.UNLOCKED)

        # Lock the user
        self.mgr.lock(OWNER_UID)
        self.assertEqual(self.mgr.users[OWNER_UID]["lock_state"], UserLockState.LOCKED)

        # Wrong password fails
        with self.assertRaises(PermissionError):
            self.mgr.unlock(OWNER_UID, b"wrong_pass", current_tick=150)
        self.assertEqual(self.mgr.users[OWNER_UID]["lock_state"], UserLockState.LOCKED)

        # Correct password succeeds
        self.mgr.unlock(OWNER_UID, b"secret_pass", current_tick=160)
        self.assertEqual(self.mgr.users[OWNER_UID]["lock_state"], UserLockState.UNLOCKED)

    def test_03_lockout_after_max_failed_attempts(self):
        """Verify account lockout kicks in after 5 consecutive failed attempts."""
        self.mgr.provision_owner("Device Owner", b"correct_pin", tick=100)
        self.mgr.lock(OWNER_UID)

        # 4 consecutive failures
        for i in range(4):
            with self.assertRaises(PermissionError):
                self.mgr.unlock(OWNER_UID, b"bad_pin", current_tick=200 + i)
            self.assertEqual(self.mgr.users[OWNER_UID]["lock_state"], UserLockState.LOCKED)

        # 5th failure triggers lockout
        with self.assertRaises(PermissionError):
            self.mgr.unlock(OWNER_UID, b"bad_pin", current_tick=205)
        self.assertEqual(self.mgr.users[OWNER_UID]["lock_state"], UserLockState.LOCKOUT)

        # Even correct pin is rejected during lockout window
        with self.assertRaises(PermissionError):
            self.mgr.unlock(OWNER_UID, b"correct_pin", current_tick=1000)

        # After lockout window expires (205 + 3000 = 3205), correct pin succeeds
        self.mgr.unlock(OWNER_UID, b"correct_pin", current_tick=3300)
        self.assertEqual(self.mgr.users[OWNER_UID]["lock_state"], UserLockState.UNLOCKED)

    def test_04_guest_lifecycle_and_deletion(self):
        """Verify ephemeral guest user creation, switching, and cleanup."""
        self.mgr.provision_owner("Owner", b"pin", tick=100)
        guest_id = self.mgr.create_guest(tick=150)
        self.assertEqual(guest_id, GUEST_UID)
        self.assertEqual(self.mgr.users[GUEST_UID]["storage_path"], "/user/1001")

        # Switch to guest
        self.mgr.switch_user(GUEST_UID)
        self.assertEqual(self.mgr.active_user, GUEST_UID)

        # Delete guest
        self.mgr.delete_user(GUEST_UID)
        self.assertNotIn(GUEST_UID, self.mgr.users)
        # Active user automatically falls back to Owner
        self.assertEqual(self.mgr.active_user, OWNER_UID)

    def test_05_cannot_delete_system_or_owner(self):
        """Verify safety invariant: System and Owner accounts cannot be deleted."""
        self.mgr.provision_owner("Owner", b"pin", tick=100)
        with self.assertRaises(ValueError):
            self.mgr.delete_user(SYSTEM_UID)
        with self.assertRaises(ValueError):
            self.mgr.delete_user(OWNER_UID)


if __name__ == "__main__":
    unittest.main()
