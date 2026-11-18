# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Truthfulness gates for the verification harness itself.

The harness must not manufacture its own evidence. These cases drive the real
harness entry points against deliberately broken or missing inputs and assert
the honest outcome:

  * the contract runner never synthesizes the published model weights at their
    real path when they are absent (it used to write a 24-byte GGUF stub there,
    which the inference graph contract's header check then passed against);
  * skipped cases are reported as skipped, and an all-skipped suite is not a
    pass;
  * the RUNNER INTEGRITY arithmetic closes even on a red run, so a passing
    case inside a failing suite cannot vanish from the record;
  * the HAL boundary contract reads the kernel source and fails when that
    source is mutated (it used to assert against a table it defined itself);
  * the x86 boot gate refuses a stale boot image before any QEMU interaction,
    and its freshness predicate distinguishes stale from fresh.

No case here launches a virtual machine: the boot-gate cases only exercise the
refusal paths, which exit before the QEMU helpers are even imported.
"""

import importlib.util
import os
import re
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RUNNER = os.path.join(ROOT, "tests", "run_all_contracts.py")
BOOT_GATE = os.path.join(ROOT, "tests", "verify_x86_boot.py")
HAL_CONTRACT = os.path.join(ROOT, "tests", "test_hal_boundary_contract.py")
REAL_WEIGHTS_REL = os.path.join("target", "verify", "models", "embeddinggemma-300M-Q8_0.gguf")

OK_SUITE = "def test_ok():\n    assert True\n"


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run_runner_subset(cwd, suite_name, source, runner=None, timeout=180):
    runner = runner or RUNNER
    os.makedirs(os.path.join(cwd, "tests"), exist_ok=True)
    suite_rel = os.path.join("tests", suite_name)
    with open(os.path.join(cwd, suite_rel), "w", encoding="utf-8") as fh:
        fh.write(source)
    return subprocess.run([sys.executable, runner, suite_rel], cwd=cwd,
                          capture_output=True, text=True, timeout=timeout)


def integrity_fields(stdout):
    match = re.search(r"RUNNER INTEGRITY: (.+)", stdout)
    if not match:
        raise AssertionError("no RUNNER INTEGRITY line in runner output:\n" + stdout)
    return dict(kv.split("=", 1) for kv in match.group(1).split())


def weights_fabricated(cwd):
    """The detection predicate: did the harness write the published weights?"""
    return os.path.exists(os.path.join(cwd, REAL_WEIGHTS_REL))


class TestHarnessModelTruth(unittest.TestCase):
    def test_runner_never_fabricates_the_published_weights(self):
        """Absent real weights stay absent; synthetic fixtures are labelled."""
        with tempfile.TemporaryDirectory() as cwd:
            models = os.path.join(cwd, "target", "verify", "models")
            os.makedirs(models)
            with open(os.path.join(models, "gemma-tiny-f32.gguf"), "wb") as fh:
                fh.write(b"GGUF" + bytes(60))
            r = run_runner_subset(cwd, "test_probe_ok.py", OK_SUITE)
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
            self.assertFalse(
                weights_fabricated(cwd),
                "the harness fabricated the published weights at their real path")
            self.assertIn("UNAVAILABLE", r.stdout,
                          "the absent real weights must be reported unavailable")
            self.assertIn("synthetic", r.stdout,
                          "synthetic parser fixtures must be labelled synthetic")

    def test_the_fabrication_detector_can_fail(self):
        """Mutation proof: a runner carrying the historical stub writer is
        caught by the same predicate the case above asserts on."""
        with open(RUNNER, "r", encoding="utf-8") as fh:
            source = fh.read()
        fabrication = (
            "def report_model_fixtures():\n"
            "    import struct\n"
            "    p = os.path.join('target', 'verify', 'models', 'embeddinggemma-300M-Q8_0.gguf')\n"
            "    if not os.path.exists(p):\n"
            "        os.makedirs(os.path.dirname(p), exist_ok=True)\n"
            "        with open(p, 'wb') as f:\n"
            "            f.write(struct.pack('<4sIQQ', b'GGUF', 3, 316, 41))\n"
            "\n"
            "\n"
        )
        mutated, n = re.subn(r"def report_model_fixtures\(\):.*?(?=\ndef )",
                             fabrication, source, count=1, flags=re.S)
        self.assertEqual(n, 1, "the runner no longer has the report_model_fixtures anchor")
        with tempfile.TemporaryDirectory() as cwd:
            runner_copy = os.path.join(cwd, "runner_mutated.py")
            with open(runner_copy, "w", encoding="utf-8") as fh:
                fh.write(mutated)
            work = os.path.join(cwd, "work")
            os.makedirs(work)
            r = run_runner_subset(work, "test_probe_ok.py", OK_SUITE, runner=runner_copy)
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
            self.assertTrue(
                weights_fabricated(work),
                "the mutation did not reproduce the fabrication; the detector is untested")


class TestHarnessSkipHonesty(unittest.TestCase):
    def test_skipped_cases_are_counted_as_skipped(self):
        source = (
            "import unittest\n"
            "class T(unittest.TestCase):\n"
            "    def test_pass(self):\n"
            "        self.assertTrue(True)\n"
            "    @unittest.skip('runtime prerequisite absent')\n"
            "    def test_skip(self):\n"
            "        pass\n"
        )
        with tempfile.TemporaryDirectory() as cwd:
            r = run_runner_subset(cwd, "test_probe_skip.py", source)
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
            self.assertIn("+ 1 skipped", r.stdout)
            fields = integrity_fields(r.stdout)
            self.assertEqual(fields["framework_passed"], "1")
            self.assertEqual(fields["framework_skipped"], "1")
            self.assertEqual(fields["framework_collected"], "2")

    def test_all_skipped_suite_is_not_a_pass(self):
        source = (
            "import unittest\n"
            "class T(unittest.TestCase):\n"
            "    @unittest.skip('runtime prerequisite absent')\n"
            "    def test_skip(self):\n"
            "        pass\n"
        )
        with tempfile.TemporaryDirectory() as cwd:
            r = run_runner_subset(cwd, "test_probe_allskip.py", source)
            self.assertNotEqual(r.returncode, 0,
                                "an all-skipped suite must not report success")
            self.assertIn("0 test cases executed", r.stdout)


class TestHarnessIntegrityArithmetic(unittest.TestCase):
    def test_integrity_line_closes_on_a_red_run(self):
        """Passing cases inside a failing suite stay in the record."""
        source = (
            "def test_a():\n    assert True\n"
            "def test_b():\n    assert True\n"
            "def test_c():\n    assert False, 'deliberate regression'\n"
        )
        with tempfile.TemporaryDirectory() as cwd:
            r = run_runner_subset(cwd, "test_probe_red.py", source)
            self.assertNotEqual(r.returncode, 0)
            fields = integrity_fields(r.stdout)
            collected = int(fields["framework_collected"])
            accounted = (int(fields["framework_passed"]) + int(fields["framework_failed"])
                         + int(fields["framework_errored"]) + int(fields["framework_skipped"]))
            self.assertEqual(collected, 3)
            self.assertEqual(int(fields["framework_passed"]), 2,
                             "the two passing cases of the failing suite vanished from the record")
            self.assertEqual(collected, accounted,
                             "RUNNER INTEGRITY arithmetic does not close on a red run")


class TestHalContractIsNotSelfReferential(unittest.TestCase):
    def _run_hal_suite(self, module):
        result = unittest.TestResult()
        unittest.TestLoader().loadTestsFromModule(module).run(result)
        return result

    def test_hal_contract_passes_against_the_real_kernel_source(self):
        module = load_module("hal_contract_real", HAL_CONTRACT)
        self.assertTrue(os.path.isfile(module.TRAITS_PATH),
                        "the HAL contract must read the kernel's own hal/traits.rs")
        result = self._run_hal_suite(module)
        self.assertEqual(result.testsRun, 4)
        self.assertEqual(result.failures + result.errors, [],
                         "HAL boundary contract fails against the real kernel source")

    def test_hal_contract_fails_on_mutated_kernel_source(self):
        """Mutation proof: the contract trips on broken product source."""
        module = load_module("hal_contract_mutated", HAL_CONTRACT)
        with open(module.TRAITS_PATH, "r", encoding="utf-8") as fh:
            real = fh.read()
        with tempfile.TemporaryDirectory() as tmp:
            # 1. The NotPresent provenance class is removed.
            self.assertIn("    NotPresent,\n", real)
            copy = os.path.join(tmp, "traits_no_notpresent.rs")
            with open(copy, "w", encoding="utf-8") as fh:
                fh.write(real.replace("    NotPresent,\n", "", 1))
            module.TRAITS_PATH = copy
            result = self._run_hal_suite(module)
            self.assertTrue(result.failures or result.errors,
                            "HAL contract passed with the NotPresent class removed")

            # 2. A subsystem trait loses its provenance requirement.
            copy = os.path.join(tmp, "traits_no_modem.rs")
            with open(copy, "w", encoding="utf-8") as fh:
                fh.write(real.replace("pub trait ModemHal", "pub trait ModemHalGone", 1))
            module.TRAITS_PATH = copy
            result = self._run_hal_suite(module)
            self.assertTrue(result.failures or result.errors,
                            "HAL contract passed with the ModemHal boundary removed")

            # 3. A driver claims bare-metal physical hardware.
            fake_root = os.path.join(tmp, "fake_drivers")
            os.makedirs(fake_root)
            with open(os.path.join(fake_root, "fake_driver.rs"), "w", encoding="utf-8") as fh:
                fh.write("pub fn kind() -> DriverKind { DriverKind::PhysicalHardware }\n")
            self.assertTrue(module.physical_hardware_claims([fake_root]),
                            "the physical-claim tripwire did not trip on a PhysicalHardware driver")
        self.assertEqual(module.physical_hardware_claims(module.PRODUCT_SOURCE_ROOTS), [],
                         "a product source claims PhysicalHardware on the QEMU target")


class TestBootGateStaleness(unittest.TestCase):
    def test_freshness_predicate_distinguishes_stale_from_fresh(self):
        module = load_module("boot_gate_module", BOOT_GATE)
        with tempfile.TemporaryDirectory() as tmp:
            stale = os.path.join(tmp, "stale.img")
            with open(stale, "wb") as fh:
                fh.write(bytes(512))
            os.utime(stale, (0, 0))
            newer, checked = module.kernel_image_staleness(stale, ROOT)
            self.assertGreater(checked, 0)
            self.assertTrue(newer,
                            "an epoch-old image must be stale against the current source tree")
            fresh = os.path.join(tmp, "fresh.img")
            with open(fresh, "wb") as fh:
                fh.write(bytes(512))
            future = time.time() + 86400
            os.utime(fresh, (future, future))
            newer, checked = module.kernel_image_staleness(fresh, ROOT)
            self.assertGreater(checked, 0)
            self.assertEqual(newer, [],
                             "an image newer than every build input must read as fresh")
            empty_root = os.path.join(tmp, "empty_root")
            os.makedirs(empty_root)
            _newer, checked = module.kernel_image_staleness(fresh, empty_root)
            self.assertEqual(checked, 0,
                             "a root with no build inputs must report zero checked, not fresh")

    def test_boot_gate_refuses_a_stale_image_without_booting(self):
        with tempfile.TemporaryDirectory() as tmp:
            stale = os.path.join(tmp, "stale.img")
            with open(stale, "wb") as fh:
                fh.write(bytes(512))
            os.utime(stale, (0, 0))
            env = os.environ.copy()
            env["INTEROS_KERNEL_IMAGE"] = stale
            r = subprocess.run([sys.executable, BOOT_GATE], cwd=ROOT, env=env,
                               capture_output=True, text=True, timeout=120)
            self.assertNotEqual(r.returncode, 0,
                                "the boot gate passed on a stale image")
            self.assertIn("Stale boot image", r.stdout)
            self.assertNotIn("Launching guest virtual machine", r.stdout,
                             "the gate must refuse the stale image before any QEMU launch")

    def test_boot_gate_reports_a_missing_image(self):
        env = os.environ.copy()
        env["INTEROS_KERNEL_IMAGE"] = os.path.join(ROOT, "target", "verify",
                                                 "harness_audit", "no_such_image.img")
        r = subprocess.run([sys.executable, BOOT_GATE], cwd=ROOT, env=env,
                           capture_output=True, text=True, timeout=120)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("Bootable image missing", r.stdout)
        self.assertNotIn("Launching guest virtual machine", r.stdout)


if __name__ == "__main__":
    unittest.main()