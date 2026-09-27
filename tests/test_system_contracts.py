#!/usr/bin/env python3
"""Static contract checks for core reliability/developer surfaces."""

from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]

class SystemContractTests(unittest.TestCase):
    def test_reliability_modules_exist(self):
        for rel in (
            "kernel/src/recovery.rs",
            "kernel/src/update.rs",
            "kernel/src/diagnostics.rs",
            "kernel/src/performance.rs",
            "kernel/src/watchdog.rs",
        ):
            self.assertTrue((ROOT / rel).is_file(), rel)

    def test_kernel_registers_reliability_subsystems(self):
        main = (ROOT / "kernel/src/main.rs").read_text(encoding="utf-8")
        for name in ("recovery", "update", "diagnostics", "performance", "watchdog"):
            self.assertIn(f"pub mod {name};", main)

    def test_update_is_fail_closed_and_monotonic(self):
        src = (ROOT / "kernel/src/update.rs").read_text(encoding="utf-8")
        self.assertIn("manifest.version <= current_version", src)
        self.assertIn("DigestMismatch", src)
        self.assertIn("InvalidSignature", src)
        self.assertIn("recovery_required", src)

if __name__ == "__main__":
    unittest.main(verbosity=2)
