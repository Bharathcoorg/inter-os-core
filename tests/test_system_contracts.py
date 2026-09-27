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

    def test_persistent_state_is_initialized_after_storage(self):
        main = (ROOT / "kernel/src/main.rs").read_text(encoding="utf-8")
        self.assertLess(main.index("storage::init();"), main.index("recovery::init();"))
        self.assertLess(main.index("storage::init();"), main.index("update::init();"))

    def test_unavailable_hardware_defaults_are_truthful(self):
        wireless = (ROOT / "kernel/src/net/wireless.rs").read_text(encoding="utf-8")
        self.assertIn("hardware_present: false", wireless)
        self.assertIn("wifi_rssi_dbm: 0", wireless)
        camera = (ROOT / "kernel/src/hal/camera.rs").read_text(encoding="utf-8")
        self.assertIn('hardware_present: false', camera)
        self.assertIn('sensor_model: "unavailable"', camera)
        pmic = (ROOT / "kernel/src/drivers/pmic.rs").read_text(encoding="utf-8")
        self.assertIn("hardware_present: false", pmic)
        battery = (ROOT / "kernel/src/resource/battery.rs").read_text(encoding="utf-8")
        self.assertIn("telemetry_valid: false", battery)

    def test_telephony_boot_does_not_claim_a_modem(self):
        telephony = (ROOT / "kernel/src/telephony/mod.rs").read_text(encoding="utf-8")
        init_start = telephony.index("pub fn init() {")
        worker_start = telephony.index("/// Periodic multitasking worker task", init_start)
        init_body = telephony[init_start:worker_start]
        self.assertIn("SimStatus::NoSim", init_body)
        self.assertIn("ModemRadioState::PowerDown", init_body)
        self.assertNotIn("ATD+18005550100", init_body)
