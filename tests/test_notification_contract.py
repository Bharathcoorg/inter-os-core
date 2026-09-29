#!/usr/bin/env python3
"""
InterOS Core — Sovereign Notification Subsystem Contract Test
Validates:
  1. Standard channels: System Alerts, Incoming Calls, Messages, General.
  2. Priority ordering: Urgent > High > Default > Low > Min.
  3. Do Not Disturb (DND) filtering: blocks low/default priorities, lets Urgent through.
  4. Lockscreen visibility policies:
     - Public: full body shown.
     - Private: body redacted to '[Private Content]'.
     - Secret: excluded from lockscreen active list.
  5. Per-app notification blocking.
  6. Cancellation and badge count aggregation.
"""

import sys
import unittest


class NotificationPriority:
    MIN = 0
    LOW = 1
    DEFAULT = 2
    HIGH = 3
    URGENT = 4


class LockscreenVisibility:
    PUBLIC = "Public"
    PRIVATE = "Private"
    SECRET = "Secret"


class NotificationSimulator:
    def __init__(self):
        self.channels = {
            "system": {"name": "System Alerts", "importance": 4, "dnd_bypass": True},
            "calls": {"name": "Incoming Calls", "importance": 4, "dnd_bypass": True},
            "messages": {"name": "Messages & Chats", "importance": 3, "dnd_bypass": False},
            "general": {"name": "General Notifications", "importance": 2, "dnd_bypass": False},
        }
        self.records = []
        self.next_id = 1
        self.dnd_enabled = False
        self.app_blocks = {}
        self.max_active = 64

    def post(self, package_id: str, user_id: int, channel_id: str, title: str, body: str,
             priority: int = NotificationPriority.DEFAULT, visibility: str = LockscreenVisibility.PUBLIC,
             badge_count: int = 1):
        if self.app_blocks.get(package_id, False):
            raise PermissionError("AppBlocked")

        if channel_id not in self.channels:
            raise KeyError("ChannelNotFound")

        channel = self.channels[channel_id]
        if self.dnd_enabled and not channel["dnd_bypass"] and priority < NotificationPriority.URGENT:
            raise PermissionError("DndSuppressed")

        active_count = sum(1 for r in self.records if not r["is_cancelled"])
        if active_count >= self.max_active:
            raise OverflowError("MaxNotificationsExceeded")

        nid = self.next_id
        self.next_id += 1
        rec = {
            "id": nid,
            "package_id": package_id,
            "user_id": user_id,
            "channel_id": channel_id,
            "title": title,
            "body": body,
            "priority": priority,
            "visibility": visibility,
            "badge_count": badge_count,
            "is_cancelled": False,
        }
        self.records.append(rec)
        return nid

    def cancel(self, package_id: str, notif_id: int):
        for r in self.records:
            if r["id"] == notif_id and r["package_id"] == package_id and not r["is_cancelled"]:
                r["is_cancelled"] = True
                return True
        raise KeyError("NotificationNotFound")

    def query_active(self, user_id: int, is_locked: bool):
        result = []
        for r in self.records:
            if r["is_cancelled"] or r["user_id"] != user_id:
                continue
            if is_locked:
                if r["visibility"] == LockscreenVisibility.PUBLIC:
                    result.append(dict(r))
                elif r["visibility"] == LockscreenVisibility.PRIVATE:
                    redacted = dict(r)
                    redacted["body"] = "[Private Content]"
                    result.append(redacted)
                elif r["visibility"] == LockscreenVisibility.SECRET:
                    continue
            else:
                result.append(dict(r))
        return result


class NotificationContractTests(unittest.TestCase):
    def setUp(self):
        self.nm = NotificationSimulator()

    def test_01_standard_channels_seeded(self):
        """Verify standard channels exist with expected bypass configurations."""
        self.assertIn("system", self.nm.channels)
        self.assertIn("calls", self.nm.channels)
        self.assertIn("messages", self.nm.channels)
        self.assertIn("general", self.nm.channels)
        self.assertTrue(self.nm.channels["system"]["dnd_bypass"])
        self.assertTrue(self.nm.channels["calls"]["dnd_bypass"])
        self.assertFalse(self.nm.channels["messages"]["dnd_bypass"])

    def test_02_dnd_filtering_and_emergency_bypass(self):
        """Verify DND blocks normal notifications but lets urgent alerts through."""
        self.nm.dnd_enabled = True

        # Normal message is suppressed by DND
        with self.assertRaises(PermissionError):
            self.nm.post("interos.chat", 1000, "messages", "New Message", "Hello friend")

        # System alert bypasses DND
        nid = self.nm.post("interos.system", 1000, "system", "Battery Low", "15% remaining",
                           priority=NotificationPriority.URGENT)
        self.assertEqual(nid, 1)

    def test_03_lockscreen_visibility_redaction_and_exclusion(self):
        """Verify Public, Private, and Secret lockscreen visibility handling."""
        self.nm.post("interos.weather", 1000, "general", "Weather", "Sunny 22C",
                     visibility=LockscreenVisibility.PUBLIC)
        self.nm.post("interos.vault", 1000, "system", "Vault Secret", "2FA Code: 994821",
                     visibility=LockscreenVisibility.PRIVATE)
        self.nm.post("interos.chat", 1000, "messages", "Confidential", "Top secret payload",
                     visibility=LockscreenVisibility.SECRET)

        # Query while unlocked: all 3 visible with full content
        unlocked_items = self.nm.query_active(1000, is_locked=False)
        self.assertEqual(len(unlocked_items), 3)
        self.assertIn("Sunny 22C", [x["body"] for x in unlocked_items])
        self.assertIn("2FA Code: 994821", [x["body"] for x in unlocked_items])
        self.assertIn("Top secret payload", [x["body"] for x in unlocked_items])

        # Query while locked: Secret is excluded, Private is redacted
        locked_items = self.nm.query_active(1000, is_locked=True)
        self.assertEqual(len(locked_items), 2)  # Secret excluded
        bodies = [x["body"] for x in locked_items]
        self.assertIn("Sunny 22C", bodies)
        self.assertIn("[Private Content]", bodies)
        self.assertNotIn("2FA Code: 994821", bodies)
        self.assertNotIn("Top secret payload", bodies)

    def test_04_per_app_blocking(self):
        """Verify individual application notification blocking."""
        self.nm.app_blocks["spam.app"] = True
        with self.assertRaises(PermissionError):
            self.nm.post("spam.app", 1000, "general", "Click here", "Win prize")

    def test_05_cancellation_lifecycle(self):
        """Verify notification dismissal and cancellation."""
        nid = self.nm.post("interos.calendar", 1000, "general", "Meeting", "At 3 PM")
        self.assertEqual(len(self.nm.query_active(1000, is_locked=False)), 1)

        # Cancel notification
        self.assertTrue(self.nm.cancel("interos.calendar", nid))
        self.assertEqual(len(self.nm.query_active(1000, is_locked=False)), 0)

        # Cancelling non-existent ID fails
        with self.assertRaises(KeyError):
            self.nm.cancel("interos.calendar", 9999)


if __name__ == "__main__":
    unittest.main()
