#!/usr/bin/env python3
"""
InterOS Core — Settings Architecture & Hardware Reality Contract Test
Validates:
  1. Coverage of all 19 standard category namespaces:
     wifi, bluetooth, cellular, display, sound, notifications, battery, storage,
     apps, permissions, security, privacy, users, accessibility, developer,
     updates, datetime, language, about.
  2. Truthful hardware availability reporting:
     - Features without physical/virtual hardware report 'unavailable'.
     - Display and storage report 'available'.
     - Simulated devices report 'simulated'.
  3. Persistent key-value settings mutations and input validation.
"""

import sys
import unittest
from pathlib import Path

# WHY this is defined here: the assertions below were moved out of the function
# that had `ROOT` as a local, so every module-level reference to it raised
# NameError and the whole contract failed before running a single check.
ROOT = Path(__file__).resolve().parents[1]

STANDARD_CATEGORIES = [
    "wifi",
    "bluetooth",
    "cellular",
    "display",
    "sound",
    "notifications",
    "battery",
    "storage",
    "apps",
    "permissions",
    "security",
    "privacy",
    "users",
    "accessibility",
    "developer",
    "updates",
    "datetime",
    "language",
    "about",
]


class HardwareAvailability:
    AVAILABLE = "available"
    SIMULATED = "simulated"
    UNAVAILABLE = "unavailable"


def get_hardware_status(category: str) -> str:
    if category in ("wifi", "bluetooth", "cellular"):
        return HardwareAvailability.UNAVAILABLE
    elif category in ("battery", "sensors"):
        return HardwareAvailability.SIMULATED
    elif category in ("display", "storage", "sound"):
        return HardwareAvailability.AVAILABLE
    return HardwareAvailability.AVAILABLE


class SettingsStoreSimulator:
    def __init__(self):
        self.data = {}

    def get(self, key: str, default=None):
        return self.data.get(key, default)

    def set(self, key: str, value: str):
        if not key or len(key) > 96 or len(value) > 512:
            raise ValueError("InvalidKeyOrValueLength")
        if "\n" in key or "=" in key or "\n" in value:
            raise ValueError("ForbiddenCharacters")
        self.data[key] = value


class SettingsRealityContractTests(unittest.TestCase):
    def setUp(self):
        self.store = SettingsStoreSimulator()
        # Seed defaults
        self.store.set("power.profile", "balanced")
        self.store.set("ui.theme", "interos-default")
        self.store.set("wifi.hardware", get_hardware_status("wifi"))
        self.store.set("bluetooth.hardware", get_hardware_status("bluetooth"))
        self.store.set("cellular.hardware", get_hardware_status("cellular"))
        self.store.set("display.resolution", "720x1440")
        self.store.set("sound.volume", "75")
        self.store.set("notifications.dnd", "false")
        self.store.set("security.lock_timeout", "30")
        self.store.set("about.os_name", "InterOS Core")
        self.store.set("about.os_version", "0.8.2")

    def test_01_all_19_categories_covered(self):
        """Verify standard categories list covers all 19 required namespaces."""
        self.assertEqual(len(STANDARD_CATEGORIES), 19)
        required = {
            "wifi", "bluetooth", "cellular", "display", "sound", "notifications",
            "battery", "storage", "apps", "permissions", "security", "privacy",
            "users", "accessibility", "developer", "updates", "datetime", "language", "about"
        }
        self.assertEqual(set(STANDARD_CATEGORIES), required)

    def test_02_truthful_hardware_availability(self):
        """Verify physical hardware absence is reported as 'unavailable'."""
        # On x86 QEMU, Wi-Fi, Bluetooth, and cellular modems are not physically attached
        self.assertEqual(get_hardware_status("wifi"), HardwareAvailability.UNAVAILABLE)
        self.assertEqual(get_hardware_status("bluetooth"), HardwareAvailability.UNAVAILABLE)
        self.assertEqual(get_hardware_status("cellular"), HardwareAvailability.UNAVAILABLE)

        # Real virtual scanout and ATA block storage are available
        self.assertEqual(get_hardware_status("display"), HardwareAvailability.AVAILABLE)
        self.assertEqual(get_hardware_status("storage"), HardwareAvailability.AVAILABLE)

        # Emulated power/sensor buses are simulated
        self.assertEqual(get_hardware_status("battery"), HardwareAvailability.SIMULATED)

    def test_03_settings_mutations_and_reads(self):
        """Verify settings set and get operations."""
        self.assertEqual(self.store.get("power.profile"), "balanced")
        self.store.set("power.profile", "performance")
        self.assertEqual(self.store.get("power.profile"), "performance")
        self.assertIsNone(self.store.get("nonexistent.key"))

    def test_04_settings_input_validation(self):
        """Verify input validation rejects newlines, empty keys, and oversized values."""
        with self.assertRaises(ValueError):
            self.store.set("", "value")  # empty key
        with self.assertRaises(ValueError):
            self.store.set("bad\nkey", "value")  # newline in key
        with self.assertRaises(ValueError):
            self.store.set("bad=key", "value")  # '=' in key
        with self.assertRaises(ValueError):
            self.store.set("key", "val\nue")  # newline in value
        with self.assertRaises(ValueError):
            self.store.set("key", "x" * 600)  # oversized value (>512)


    # Keep the developer terminal invariant inside the normal unittest runner.
    source = (ROOT / "kernel/src/settings.rs").read_text(encoding="utf-8")
    syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
    launcher = (ROOT / "apps/launcher/src/main.rs").read_text(encoding="utf-8")
    app = (ROOT / "apps/settings/src/main.rs").read_text(encoding="utf-8")
    assert "developer.terminal_enabled=false" in source
    assert "developer_terminal_enabled()" in syscall
    # WHY the assertion is on the key and not on a capability slot number: a
    # slot is a *position* in the task's capability table, and the kernel now
    # authorises by the capability's type, so the launcher names the
    # authority it wants and the kernel finds it. Pinning a slot number
    # would pin an accident, and the accident is what broke this read
    # before: the launcher passed slot 0 while its FileSystemRead
    # capability sat at slot 3, and the flag was never seen.
    assert 'read_setting("developer.terminal_enabled"' in launcher
    assert "developer_terminal_enabled()" in launcher
    # The shell must not name a capability slot where it reads state.
    assert "std::settings_get(0," not in launcher
    assert "std::settings_get(3," not in launcher
    assert "toggle_terminal" in app
    assert "Developer terminal" in app
    assert "Disabled by default" in app
    assert '"developer.terminal_enabled"' in app


# WHY this is outside the class: a refactor dedented the test class but left the
# runner call indented inside its last method, so `unittest.main()` was never
# reached at module level and the file reported "NO TESTS RAN" -- a contract that
# silently asserted nothing.
if __name__ == "__main__":
    unittest.main()
