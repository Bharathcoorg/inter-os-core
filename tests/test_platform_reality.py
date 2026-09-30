#!/usr/bin/env python3
"""Static platform-reality contracts for the InterOS Core repository."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class PlatformRealityTests(unittest.TestCase):
    def read(self, path):
        return (ROOT / path).read_text(encoding="utf-8")

    def test_platform_contract_is_bootstrapped(self):
        main = self.read("kernel/src/main.rs")
        platform = self.read("kernel/src/platform.rs")
        self.assertIn("pub mod platform;", main)
        self.assertIn("platform::init();", main)
        self.assertIn("HardwareAvailability::Emulated", platform)
        self.assertIn("VerificationLevel::QemuVerified", platform)

    def test_kernel_does_not_mark_qemu_as_physical_hardware(self):
        platform = self.read("kernel/src/platform.rs")
        self.assertIn("pub const fn is_hardware_backed", platform)
        self.assertIn("HardwareAvailability::Present", platform)
        self.assertNotIn("hardware: HardwareAvailability::Present", platform)

    def test_launcher_does_not_present_fixed_hardware_telemetry_as_truth(self):
        launcher = self.read("apps/launcher/src/main.rs")
        forbidden = [
            'b"09:20"',
            'b"Mon, 12 Jan"',
            'b"Bangalore"',
            "78% fill",
        ]
        for value in forbidden:
            self.assertNotIn(value, launcher)

    def test_existing_truthful_hardware_defaults_remain_in_place(self):
        wireless = self.read("kernel/src/net/wireless.rs")
        battery = self.read("kernel/src/resource/battery.rs")
        pmic = self.read("kernel/src/drivers/pmic.rs")
        camera = self.read("kernel/src/hal/camera.rs")
        self.assertIn("hardware_present: false", wireless)
        self.assertIn("telemetry_valid: false", battery)
        self.assertIn("hardware_present: false", pmic)
        self.assertIn("sensor_model: \"unavailable\"", camera)

    def test_pmic_self_test_exercises_hardware_backed_path(self):
        pmic = self.read("kernel/src/drivers/pmic.rs")
        self.assertIn("pmic.set_telemetry(", pmic)
        self.assertIn("assert!(pmic.is_hardware_present());", pmic)
        self.assertIn("assert_eq!(pmic.coulomb_counter_mah, 3150.0);", pmic)
        self.assertIn("assert_eq!(pmic.telemetry.soc_percent, 78);", pmic)

    def test_service_start_revalidates_parent_ownership(self):
        services = self.read("kernel/src/services.rs")
        self.assertIn("if task.parent_id != parent_id {", services)
        self.assertIn('return Err("running service is not owned by caller");', services)

    def test_pid1_parses_public_service_snapshot_contract(self):
        init = self.read("userspace/init/src/main.rs")
        self.assertIn("let Some(abi_bytes) = fields.next() else", init)
        self.assertIn('if abi_bytes != b"1" || state != b"running"', init)
        self.assertNotIn('task_bytes == b"0"', init)

    def test_no_internal_gate_labels_enter_public_source(self):
        for path in ["README.md", "kernel/src/platform.rs", "tests/test_platform_reality.py"]:
            text = self.read(path).lower()
            self.assertNotRegex(text, r"\bgate\s+[abcd]\b")

if __name__ == "__main__":
    unittest.main(verbosity=2)
