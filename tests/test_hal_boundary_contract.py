#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
InterOS Core — Hardware Abstraction Layer (HAL) Boundary & Reality Contract

WHAT THIS VERIFIES (source-level, against the kernel as it is written):
  1. DriverKind provenance classification exists in kernel/src/hal/traits.rs
     with exactly four classes: VirtualQemu, PhysicalHardware, Simulator,
     NotPresent.
  2. All 10 mobile HAL subsystem traits are declared (Display, TouchInput,
     Storage, Network, Bluetooth, Audio, Camera, Sensor, Modem, PMIC) and each
     requires an explicit driver_kind() -- no subsystem can inherit a silent
     provenance default.
  3. No product source constructs DriverKind::PhysicalHardware: on the x86
     QEMU target no driver may claim bare-metal silicon.
  4. The x86 platform timer/serial/power implementations inherit the
     VirtualQemu default and do not override it with a physical claim.

WHAT THIS DOES NOT VERIFY: runtime provenance attestation. The ten subsystem
traits have no concrete implementors in the tree yet, so there is no runtime
driver table to read. An earlier version of this file defined its own
SUBSYSTEM_DRIVERS table and asserted against that table -- a claim checking
itself, which could never fail when the product regressed. Every assertion
below reads the product source instead, and fails when the boundary breaks.
"""

import os
import re
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TRAITS_PATH = os.path.join(ROOT, "kernel", "src", "hal", "traits.rs")
PLATFORM_PATH = os.path.join(ROOT, "kernel", "src", "hal", "platform.rs")
PRODUCT_SOURCE_ROOTS = [
    os.path.join(ROOT, "kernel", "src"),
    os.path.join(ROOT, "userspace"),
    os.path.join(ROOT, "apps"),
]

SUBSYSTEM_TRAITS = [
    "DisplayHal", "TouchInputHal", "StorageHal", "NetworkHal", "BluetoothHal",
    "AudioHal", "CameraHal", "SensorHal", "ModemHal", "PmicHal",
]
DRIVER_KIND_VARIANTS = ["VirtualQemu", "PhysicalHardware", "Simulator", "NotPresent"]
PLATFORM_TRAITS = ["PlatformTimer", "SerialConsole", "PowerController"]


def read_source(path):
    with open(path, "r", encoding="utf-8") as fh:
        return fh.read()


def trait_body(source, name):
    """The text of `pub trait <name> ... { ... }` up to its column-0 close."""
    match = re.search(r"pub trait " + name + r"\b[^{]*\{(.*?)\n\}", source, re.S)
    return match.group(1) if match else None


def driver_kind_enum_body(source):
    match = re.search(r"pub enum DriverKind \{(.*?)\n\}", source, re.S)
    return match.group(1) if match else None


def code_lines(source):
    """Source lines with // comments dropped, so a detector over code is not
    tripped (or satisfied) by a sentence that merely mentions the thing."""
    return [line.split("//", 1)[0] for line in source.splitlines()]


def physical_hardware_claims(roots):
    """Every product-source line that constructs DriverKind::PhysicalHardware."""
    claims = []
    for root in roots:
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames[:] = [d for d in dirnames if d != "target"]
            for filename in filenames:
                if not filename.endswith(".rs"):
                    continue
                path = os.path.join(dirpath, filename)
                with open(path, "r", encoding="utf-8", errors="replace") as fh:
                    for lineno, line in enumerate(code_lines(fh.read()), 1):
                        if "DriverKind::PhysicalHardware" in line:
                            claims.append((path, lineno, line.strip()))
    return claims


class HalBoundaryContractTests(unittest.TestCase):
    def test_01_all_10_subsystems_declared(self):
        """Every mobile HAL subsystem trait exists and demands provenance."""
        source = read_source(TRAITS_PATH)
        for trait in SUBSYSTEM_TRAITS:
            body = trait_body(source, trait)
            self.assertIsNotNone(body, f"kernel/src/hal/traits.rs no longer declares {trait}")
            self.assertIn(
                "fn driver_kind(&self) -> DriverKind;", body,
                f"{trait} no longer requires an explicit driver_kind() provenance declaration")

    def test_02_truthful_driver_provenance(self):
        """The four provenance classes exist, and nothing claims bare metal."""
        source = read_source(TRAITS_PATH)
        body = driver_kind_enum_body(source)
        self.assertIsNotNone(body, "kernel/src/hal/traits.rs no longer defines DriverKind")
        variants = re.findall(r"^[ \t]*([A-Z][A-Za-z0-9]*)[ \t]*,?[ \t]*$", body, re.M)
        self.assertEqual(sorted(variants), sorted(DRIVER_KIND_VARIANTS),
                         "DriverKind must classify exactly VirtualQemu, PhysicalHardware, "
                         "Simulator and NotPresent")
        claims = physical_hardware_claims(PRODUCT_SOURCE_ROOTS)
        self.assertEqual(
            claims, [],
            "no driver on the x86 QEMU target may claim PhysicalHardware bare-metal "
            f"silicon, but these source lines do: {claims}")

    def test_03_unsupported_hardware_explicitly_marked(self):
        """Absent hardware has a provenance class, and Bluetooth/Modem cannot
        inherit a silent default: an implementor must declare it explicitly."""
        source = read_source(TRAITS_PATH)
        self.assertRegex(
            source, r"///[^\n]*not present[^\n]*\n\s*NotPresent",
            "DriverKind::NotPresent must remain the documented class for absent hardware")
        for trait in ("BluetoothHal", "ModemHal"):
            body = trait_body(source, trait)
            self.assertIsNotNone(body, f"{trait} is no longer declared")
            self.assertIn("fn driver_kind(&self) -> DriverKind;", body,
                          f"{trait} must require an explicit provenance declaration")
            self.assertNotIn("DriverKind::", body,
                             f"{trait} grew a default driver_kind body; absent hardware "
                             "would inherit a provenance claim instead of declaring one")

    def test_04_qemu_hardware_explicitly_marked_virtual(self):
        """The x86 platform timer/serial/power provenance is VirtualQemu."""
        traits = read_source(TRAITS_PATH)
        for trait in PLATFORM_TRAITS:
            body = trait_body(traits, trait)
            self.assertIsNotNone(body, f"{trait} is no longer declared")
            self.assertRegex(
                body, r"fn driver_kind\(&self\) -> DriverKind \{\s*DriverKind::VirtualQemu\s*\}",
                f"{trait}'s default provenance is no longer VirtualQemu")
        platform = read_source(PLATFORM_PATH)
        for impl in ("impl PlatformTimer for X86Timer",
                     "impl SerialConsole for X86SerialConsole",
                     "impl PowerController for X86PowerController"):
            self.assertIn(impl, platform,
                          f"kernel/src/hal/platform.rs no longer carries '{impl}'")
        self.assertNotIn(
            "DriverKind::PhysicalHardware", "\n".join(code_lines(platform)),
            "the x86 QEMU platform implementation claims bare-metal physical hardware")


if __name__ == "__main__":
    unittest.main()