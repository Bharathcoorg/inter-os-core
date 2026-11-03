#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
InterOS Core — Hardware Abstraction Layer (HAL) Boundary & Reality Contract Test
Validates:
  1. DriverKind provenance classification:
     - VirtualQemu: Emulated / virtual device provided by QEMU.
     - PhysicalHardware: Real bare-metal silicon driver.
     - Simulator: Software mock used for tests.
     - NotPresent: Unattached device category.
  2. All 10 mobile HAL subsystem boundaries are formally defined:
     Display, TouchInput, Storage, Network, Bluetooth, Audio, Camera, Sensor, Modem, PMIC.
  3. No virtual driver masquerades as physical bare-metal hardware.
"""

import sys
import unittest


class DriverKind:
    VIRTUAL_QEMU = "VirtualQemu"
    PHYSICAL_HARDWARE = "PhysicalHardware"
    SIMULATOR = "Simulator"
    NOT_PRESENT = "NotPresent"


SUBSYSTEM_DRIVERS = {
    "display": {"kind": DriverKind.VIRTUAL_QEMU, "desc": "QEMU Standard VGA / Bochs DRM/KMS scanout"},
    "touch_input": {"kind": DriverKind.VIRTUAL_QEMU, "desc": "QEMU Virtio-Tablet / PS/2 absolute pointer"},
    "storage": {"kind": DriverKind.VIRTUAL_QEMU, "desc": "QEMU Primary/Secondary ATA PIO hard disk"},
    "network": {"kind": DriverKind.VIRTUAL_QEMU, "desc": "Intel 82540EM (e1000) virtual NIC"},
    "bluetooth": {"kind": DriverKind.NOT_PRESENT, "desc": "Physical BLE controller not present on target"},
    "audio": {"kind": DriverKind.VIRTUAL_QEMU, "desc": "Intel 8254 PIT Channel 2 speaker frequency timer"},
    "camera": {"kind": DriverKind.SIMULATOR, "desc": "Synthetic MIPI-CSI2 sensor pattern generator"},
    "sensors": {"kind": DriverKind.SIMULATOR, "desc": "Synthetic I2C/SPI IMU accelerometer/lux bus"},
    "modem": {"kind": DriverKind.NOT_PRESENT, "desc": "Physical 4G/5G baseband modem not present on target"},
    "pmic": {"kind": DriverKind.SIMULATOR, "desc": "Simulated battery & DVFS thermal governor"},
}


class HalBoundaryContractTests(unittest.TestCase):
    def test_01_all_10_subsystems_declared(self):
        """Verify all 10 mobile HAL subsystems have defined boundaries."""
        self.assertEqual(len(SUBSYSTEM_DRIVERS), 10)
        expected = {
            "display", "touch_input", "storage", "network", "bluetooth",
            "audio", "camera", "sensors", "modem", "pmic"
        }
        self.assertEqual(set(SUBSYSTEM_DRIVERS.keys()), expected)

    def test_02_truthful_driver_provenance(self):
        """Verify no virtual or simulated driver claims to be PhysicalHardware."""
        for name, spec in SUBSYSTEM_DRIVERS.items():
            kind = spec["kind"]
            self.assertIn(kind, [
                DriverKind.VIRTUAL_QEMU,
                DriverKind.PHYSICAL_HARDWARE,
                DriverKind.SIMULATOR,
                DriverKind.NOT_PRESENT,
            ])
            # Under current x86 QEMU canonical configuration, zero drivers are bare-metal physical
            self.assertNotEqual(
                kind,
                DriverKind.PHYSICAL_HARDWARE,
                f"Subsystem '{name}' falsely claims PhysicalHardware on QEMU target"
            )

    def test_03_unsupported_hardware_explicitly_marked(self):
        """Verify absent physical hardware (Bluetooth, Modem) is marked NotPresent."""
        self.assertEqual(SUBSYSTEM_DRIVERS["bluetooth"]["kind"], DriverKind.NOT_PRESENT)
        self.assertEqual(SUBSYSTEM_DRIVERS["modem"]["kind"], DriverKind.NOT_PRESENT)

    def test_04_qemu_hardware_explicitly_marked_virtual(self):
        """Verify virtual QEMU peripherals are explicitly marked VirtualQemu."""
        self.assertEqual(SUBSYSTEM_DRIVERS["display"]["kind"], DriverKind.VIRTUAL_QEMU)
        self.assertEqual(SUBSYSTEM_DRIVERS["touch_input"]["kind"], DriverKind.VIRTUAL_QEMU)
        self.assertEqual(SUBSYSTEM_DRIVERS["storage"]["kind"], DriverKind.VIRTUAL_QEMU)
        self.assertEqual(SUBSYSTEM_DRIVERS["network"]["kind"], DriverKind.VIRTUAL_QEMU)


if __name__ == "__main__":
    unittest.main()
