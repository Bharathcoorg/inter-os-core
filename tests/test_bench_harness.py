# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Unit tests for the boot benchmark harness's pure logic.

WHY these exist separately from the benchmark itself: the benchmark needs QEMU
and minutes of wall-clock to run, but the parts most likely to be wrong are the
parts that decide whether a result is trustworthy -- the outcome classification,
the staleness rule, and the summary arithmetic. Those are testable without an
emulator, so they are tested without one.

Every case below asserts behaviour the harness must have for its numbers to mean
anything: a failed boot is never counted as a fast boot, a stale file is never
accepted as current, and every summary figure is derivable from the raw
samples.
"""

import os
import sys
import tempfile
import time
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "tools"))

import bench_boot  # noqa: E402


class SummaryTests(unittest.TestCase):
    def test_summary_of_empty_input_is_empty_not_an_error(self):
        s = bench_boot.summarize([])
        self.assertEqual(s["count"], 0)
        self.assertIsNone(s["median_ms"])
        self.assertIsNone(s["mean_ms"])

    def test_summary_matches_the_statistics_module(self):
        import statistics
        vals = [1200.0, 1000.0, 1400.0, 1100.0, 1300.0]
        s = bench_boot.summarize(vals)
        self.assertEqual(s["count"], 5)
        self.assertAlmostEqual(s["min_ms"], 1000.0, places=1)
        self.assertAlmostEqual(s["max_ms"], 1400.0, places=1)
        self.assertAlmostEqual(s["median_ms"], round(statistics.median(vals), 1), places=1)
        self.assertAlmostEqual(s["mean_ms"], round(statistics.fmean(vals), 1), places=1)

    def test_single_sample_is_its_own_min_median_max_and_mean(self):
        s = bench_boot.summarize([42.0])
        self.assertEqual((s["min_ms"], s["median_ms"], s["max_ms"], s["mean_ms"]), (42.0, 42.0, 42.0, 42.0))

    def test_summary_is_reproducible_from_the_raw_values(self):
        """A summary must be derivable from the raw list, not invented."""
        raw = [100.0, 250.0, 175.0]
        s = bench_boot.summarize(raw)
        self.assertEqual(s["count"], len(raw))
        self.assertAlmostEqual(s["mean_ms"], round(sum(raw) / len(raw), 1), places=1)
        self.assertAlmostEqual(s["median_ms"], 175.0, places=1)


class ClassificationTests(unittest.TestCase):
    def test_reaching_the_marker_is_ok(self):
        self.assertEqual(bench_boot.classify(True, False, 1000.0, 180.0), "ok")

    def test_guest_exiting_before_readiness_is_a_crash_not_a_timeout(self):
        self.assertEqual(bench_boot.classify(False, True, 500.0, 180.0), "guest_crash")

    def test_live_guest_that_never_signals_is_a_timeout(self):
        self.assertEqual(bench_boot.classify(False, False, 180000.0, 180.0), "timeout")

    def test_ready_without_a_usable_elapsed_time_is_a_launch_failure(self):
        self.assertEqual(bench_boot.classify(True, False, None, 180.0), "launch_failure")
        self.assertEqual(bench_boot.classify(True, False, -1.0, 180.0), "launch_failure")

    def test_a_failed_trial_is_never_reported_as_a_timing(self):
        """The harness must not be able to average in a boot that did not happen."""
        for ready, exited, elapsed in [(False, True, 10.0), (False, False, 10.0), (True, False, None)]:
            self.assertNotEqual(bench_boot.classify(ready, exited, elapsed, 180.0), "ok")

    def test_every_outcome_is_one_of_the_documented_set(self):
        allowed = {"ok", "timeout", "guest_crash", "launch_failure"}
        for ready in (True, False):
            for exited in (True, False):
                for elapsed in (None, -1.0, 0.0, 100.0):
                    self.assertIn(bench_boot.classify(ready, exited, elapsed, 180.0), allowed)


class StalenessTests(unittest.TestCase):
    def test_a_file_written_before_the_run_is_stale(self):
        with tempfile.NamedTemporaryFile(delete=False) as fh:
            fh.write(b"x")
            path = fh.name
        try:
            old = time.time() - 3600
            os.utime(path, (old, old))
            self.assertTrue(bench_boot.is_stale(path, time.time()))
        finally:
            os.remove(path)

    def test_a_file_written_during_the_run_is_not_stale(self):
        with tempfile.NamedTemporaryFile(delete=False) as fh:
            fh.write(b"x")
            path = fh.name
        try:
            self.assertFalse(bench_boot.is_stale(path, time.time() - 5))
        finally:
            os.remove(path)

    def test_a_missing_file_is_stale(self):
        self.assertTrue(bench_boot.is_stale(os.path.join(ROOT, "does-not-exist.img"), time.time()))


class ArtifactInspectionTests(unittest.TestCase):
    def test_elf_architecture_is_read_from_the_bytes_not_the_filename(self):
        import struct
        with tempfile.NamedTemporaryFile(delete=False, suffix=".whatever") as fh:
            hdr = bytearray(64)
            hdr[0:4] = b"\x7fELF"
            hdr[4] = 2   # 64-bit
            hdr[5] = 1   # little-endian
            struct.pack_into("<H", hdr, 16, 3)      # ET_DYN
            struct.pack_into("<H", hdr, 18, 0xB7)   # AArch64
            fh.write(bytes(hdr))
            path = fh.name
        try:
            info = bench_boot.describe_artifact(ROOT, os.path.relpath(path, ROOT))
            self.assertEqual(info["arch"], "AArch64")
            self.assertEqual(info["elf_type"], "ET_DYN")
        finally:
            os.remove(path)

    def test_mbr_image_is_recognised_by_its_boot_signature(self):
        with tempfile.NamedTemporaryFile(delete=False, suffix=".img") as fh:
            sector = bytearray(512)
            sector[510] = 0x55
            sector[511] = 0xAA
            fh.write(bytes(sector))
            path = fh.name
        try:
            info = bench_boot.describe_artifact(ROOT, os.path.relpath(path, ROOT))
            self.assertIn("MBR", info["format"])
            self.assertNotIn("arch", info)
        finally:
            os.remove(path)

    def test_absent_artifact_is_reported_as_absent_not_guessed(self):
        info = bench_boot.describe_artifact(ROOT, "target/definitely-not-here.img")
        self.assertFalse(info["exists"])


class EndpointContractTests(unittest.TestCase):
    """The three architectures must never be silently treated as comparable."""

    def test_each_arch_declares_an_explicit_endpoint_and_marker(self):
        for name, cfg in bench_boot.ARCHES.items():
            self.assertTrue(cfg["endpoint"].strip(), "%s has no endpoint" % name)
            self.assertTrue(cfg["ready_marker"].strip(), "%s has no marker" % name)
            self.assertGreater(cfg["timeout_s"], 0)

    def test_only_x86_64_is_claimed_to_have_a_bootable_image(self):
        bootable = [n for n, c in bench_boot.ARCHES.items() if c["bootable"]]
        self.assertEqual(bootable, ["x86_64"],
                         "a non-x86 kernel ELF must not be reported as a bootable image")

    def test_self_test_architectures_do_not_claim_a_running_device(self):
        for name in ("aarch64", "riscv64"):
            cfg = bench_boot.ARCHES[name]
            self.assertFalse(cfg["guest_expects_to_keep_running"],
                             "%s exits after its self-test and must be labelled as such" % name)
            self.assertIn("no launcher", cfg["endpoint"])

    def test_markers_are_distinct_between_architectures(self):
        markers = [c["ready_marker"] for c in bench_boot.ARCHES.values()]
        self.assertEqual(len(set(markers)), len(markers),
                         "identical readiness markers would make the numbers look comparable")


class FormattingTests(unittest.TestCase):
    def test_format_bytes_reports_all_three_scales(self):
        s = bench_boot.format_bytes(6784000)
        self.assertIn("6784000 bytes", s)
        self.assertIn("KiB", s)
        self.assertIn("MiB", s)

    def test_format_bytes_handles_missing_values(self):
        self.assertEqual(bench_boot.format_bytes(None), "n/a")


class NegativeCaseTests(unittest.TestCase):
    """The failure paths the harness must detect, exercised without QEMU."""

    def test_a_missing_boot_artifact_is_detected_not_measured(self):
        with tempfile.TemporaryDirectory() as empty_root:
            with tempfile.TemporaryDirectory() as out:
                rec = bench_boot.run_trial("x86_64", 1, empty_root, out, timeout_s=5.0)
        self.assertEqual(rec["outcome"], "missing_artifact")
        self.assertIsNone(rec["elapsed_ms"],
                          "a missing artifact must not produce a timing")
        self.assertIn("not found", rec["detail"])

    def test_missing_artifact_is_detected_for_every_architecture(self):
        with tempfile.TemporaryDirectory() as empty_root:
            for arch in bench_boot.ARCHES:
                with tempfile.TemporaryDirectory() as out:
                    rec = bench_boot.run_trial(arch, 1, empty_root, out, timeout_s=5.0)
                self.assertEqual(rec["outcome"], "missing_artifact",
                                 "%s must detect a missing artifact" % arch)

    def test_cli_exits_nonzero_when_the_artifact_is_missing(self):
        import subprocess
        with tempfile.TemporaryDirectory() as empty_root:
            with tempfile.TemporaryDirectory() as out:
                proc = subprocess.run(
                    [sys.executable, os.path.join(ROOT, "tools", "bench_boot.py"),
                     "--arch", "x86_64", "--trials", "1",
                     "--root", empty_root, "--out", out],
                    capture_output=True, text=True, timeout=120)
        self.assertNotEqual(proc.returncode, 0,
                            "a benchmark with no artifact must fail, not report success")
        self.assertIn("missing", (proc.stdout + proc.stderr).lower())

    def test_a_serial_log_from_a_previous_run_cannot_satisfy_this_run(self):
        """Staleness is what stops an old log from being read as a fresh boot."""
        with tempfile.NamedTemporaryFile(delete=False, suffix=".log") as fh:
            fh.write(b"[LAUNCHER] Step 5: Initial presentation complete.")
            path = fh.name
        try:
            old = time.time() - 7200
            os.utime(path, (old, old))
            self.assertTrue(bench_boot.is_stale(path, time.time()))
        finally:
            os.remove(path)


if __name__ == "__main__":
    unittest.main()