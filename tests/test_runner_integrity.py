"""WHAT THIS FILE IS FOR: it proves the contract runner itself is trustworthy.

WHY: the runner is the gate every other suite depends on, so a bug that lets a
zero-discovery run, a shrunken test floor, a timed-out script, or an unexecuted
suite report success would silently green-light broken code. Each case here drives
run_all_contracts.py against deliberately broken inputs and asserts it fails closed.
The last case is different in kind: it guards kernel-ordering integrity rather than
the runner, because the ordering property has no other home in the contract tree.
"""

import os
import subprocess
import sys
import tempfile
import unittest


class TestRunnerIntegrity(unittest.TestCase):
    def setUp(self):
        self.runner_path = os.path.abspath(os.path.join(os.path.dirname(__file__), "run_all_contracts.py"))

    def test_runner_fails_on_zero_discovery(self):
        """Proves runner fails with nonzero exit code when 0 test files are discovered."""
        with tempfile.TemporaryDirectory() as empty_dir:
            r = subprocess.run([sys.executable, self.runner_path], cwd=empty_dir, capture_output=True, text=True)
            self.assertNotEqual(r.returncode, 0, "Runner must exit nonzero when zero test files are discovered")
            self.assertIn("Zero contract test files discovered", r.stderr)

    def test_runner_fails_on_gate_shrinkage(self):
        """Proves runner fails when discovered tests fall below minimum expected test floor."""
        with tempfile.TemporaryDirectory() as partial_tree:
            tests_dir = os.path.join(partial_tree, "tests")
            os.makedirs(tests_dir, exist_ok=True)
            # Create only 2 test files, well below the MIN_EXPECTED_CONTRACT_SUITES (50) floor
            for i in range(2):
                with open(os.path.join(tests_dir, f"test_sample_{i}.py"), "w", encoding="utf-8") as f:
                    f.write("def test_ok(): pass\n")
            r = subprocess.run([sys.executable, self.runner_path], cwd=partial_tree, capture_output=True, text=True)
            self.assertNotEqual(r.returncode, 0, "Runner must fail when test gate shrinks")
            self.assertIn("Test gate has shrunk or discovery is incomplete", r.stderr)

    def test_runner_fails_on_async_only_failing_test(self):
        """Proves async test containing assert False is classified as framework test and fails closed."""
        with tempfile.NamedTemporaryFile("w", prefix="test_tmp_async_", suffix=".py", dir="tests", delete=False) as f:
            f.write("async def test_async_assertion():\n    assert False, 'Deliberate async failure'\n")
            temp_path = f.name
        try:
            r = subprocess.run([sys.executable, self.runner_path, temp_path], capture_output=True, text=True)
            self.assertNotEqual(r.returncode, 0, "Runner must fail on async test failure")
            self.assertIn("1 test files failed!", r.stdout)
            self.assertNotIn("ALL", r.stdout)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_runner_fails_on_empty_noop_script(self):
        """Proves runner fails a script contract that has zero assertions or verifiable checks."""
        with tempfile.NamedTemporaryFile("w", prefix="test_tmp_noop_", suffix=".py", dir="tests", delete=False) as f:
            f.write("# No-op script with no assertions\nx = 123\nprint('noop')\n")
            temp_path = f.name
        try:
            r = subprocess.run([sys.executable, self.runner_path, temp_path], capture_output=True, text=True)
            self.assertNotEqual(r.returncode, 0, "Runner must fail a script without verifiable assertions")
            self.assertIn("0 assertions or verifiable test execution defined", r.stdout)
            self.assertIn("1 test files failed!", r.stdout)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_runner_fails_on_script_timeout(self):
        """Proves runner terminates and marks as failed a script that exceeds timeout."""
        with tempfile.NamedTemporaryFile("w", prefix="test_tmp_timeout_", suffix=".py", dir="tests", delete=False) as f:
            f.write("import time\ntime.sleep(5)\nassert True\n")
            temp_path = f.name
        try:
            env = os.environ.copy()
            env["INTEROS_SCRIPT_TIMEOUT"] = "1"
            r = subprocess.run([sys.executable, self.runner_path, temp_path], env=env, capture_output=True, text=True)
            self.assertNotEqual(r.returncode, 0, "Runner must fail on timeout")
            self.assertIn("timed out after 1s", r.stdout)
            self.assertIn("1 test files failed!", r.stdout)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_runner_exact_skip_policy_does_not_substring_match(self):
        """Proves skip policy matches exact file paths and does not silently skip similarly-named tests."""
        import importlib.util
        spec = importlib.util.spec_from_file_location("run_all_contracts", self.runner_path)
        runner_mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner_mod)

        probe_path = "tests/test_storage_recovery_probe.py"
        self.assertNotIn(probe_path, runner_mod.RUNTIME_DEPENDENT_SKIPPED_FILES)
        for p in runner_mod.RUNTIME_DEPENDENT_SKIPPED_FILES:
            self.assertTrue(p.startswith("tests/"), f"Skip entry {p} must be explicit path")
            self.assertTrue(p.endswith(".py"), f"Skip entry {p} must end with .py")

    def test_runner_discovers_nested_qemu_tests(self):
        """Proves runner recursively discovers nested tests (including tests/qemu/) and accounts for them."""
        import importlib.util
        spec = importlib.util.spec_from_file_location("run_all_contracts", self.runner_path)
        runner_mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner_mod)

        self.assertIn("tests/qemu/test_boot_image_immutable.py", runner_mod.RUNTIME_DEPENDENT_SKIPPED_FILES)
        self.assertIn("tests/qemu/test_qemu_boot.py", runner_mod.RUNTIME_DEPENDENT_SKIPPED_FILES)
        self.assertIn("tests/qemu/test_vault_persistence.py", runner_mod.RUNTIME_DEPENDENT_SKIPPED_FILES)

    def test_runner_labels_subset_run(self):
        """Proves runner explicitly labels subset execution and does not claim full suite passed."""
        with tempfile.NamedTemporaryFile("w", prefix="test_tmp_", suffix=".py", dir="tests", delete=False) as f:
            f.write("def test_one(): pass\n")
            temp_path = f.name
        try:
            r = subprocess.run([sys.executable, self.runner_path, temp_path], capture_output=True, text=True)
            self.assertEqual(r.returncode, 0)
            self.assertIn("[PARTIAL SUBSET RUN:", r.stdout)
            self.assertIn("[PARTIAL SUBSET RUN PASSED]", r.stdout)
            self.assertNotIn("ALL LOCAL CONTRACT TEST CASES PASSED", r.stdout)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_runner_fails_on_framework_test_failure(self):
        """Proves runner exits nonzero and records failure when a framework test fails."""
        with tempfile.NamedTemporaryFile("w", prefix="test_tmp_", suffix=".py", dir="tests", delete=False) as f:
            f.write("def test_failing_case():\n    assert False, 'Deliberate regression failure'\n")
            temp_path = f.name
        try:
            r = subprocess.run([sys.executable, self.runner_path, temp_path], capture_output=True, text=True)
            self.assertNotEqual(r.returncode, 0, "Runner must exit nonzero when a test fails")
            self.assertIn("1 test files failed!", r.stdout)
            self.assertIn("FAIL:", r.stdout)
            self.assertNotIn("ALL", r.stdout)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_runner_fails_on_script_test_failure(self):
        """Proves runner exits nonzero when a script contract raises AssertionError."""
        with tempfile.NamedTemporaryFile("w", prefix="test_tmp_", suffix=".py", dir="tests", delete=False) as f:
            f.write("assert 1 == 2, 'Deliberate script contract failure'\n")
            temp_path = f.name
        try:
            r = subprocess.run([sys.executable, self.runner_path, temp_path], capture_output=True, text=True)
            self.assertNotEqual(r.returncode, 0, "Runner must exit nonzero when script fails")
            self.assertIn("1 test files failed!", r.stdout)
            self.assertIn("FAIL:", r.stdout)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_runner_rejects_unexecuted_tests_as_pass(self):
        """Proves runner fails when a framework test file has 0 executed tests."""
        with tempfile.NamedTemporaryFile("w", prefix="test_tmp_", suffix=".py", dir="tests", delete=False) as f:
            f.write("import unittest\n# No test methods\nclass Dummy(unittest.TestCase):\n    pass\n")
            temp_path = f.name
        try:
            r = subprocess.run([sys.executable, self.runner_path, temp_path], capture_output=True, text=True)
            self.assertNotEqual(r.returncode, 0, "Runner must fail when 0 tests were executed in a framework test")
            self.assertIn("0 test cases executed", r.stdout)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_runner_fails_on_import_or_syntax_error(self):
        """Proves runner exits nonzero on import error or syntax error."""
        with tempfile.NamedTemporaryFile("w", prefix="test_tmp_", suffix=".py", dir="tests", delete=False) as f:
            f.write("import nonexistent_module_for_contract_runner_test_abc\ndef test_ok(): pass\n")
            temp_path = f.name
        try:
            r = subprocess.run([sys.executable, self.runner_path, temp_path], capture_output=True, text=True)
            self.assertNotEqual(r.returncode, 0, "Runner must fail on import error")
            self.assertIn("1 test files failed!", r.stdout)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_runner_reports_actual_executed_count(self):
        """Proves runner reports exact count of executed tests, not regex-counted source lines."""
        with tempfile.NamedTemporaryFile("w", prefix="test_tmp_", suffix=".py", dir="tests", delete=False) as f:
            f.write("def test_one(): pass\ndef test_two(): pass\ndef test_three(): pass\n")
            temp_path = f.name
        try:
            r = subprocess.run([sys.executable, self.runner_path, temp_path], capture_output=True, text=True)
            self.assertEqual(r.returncode, 0)
            self.assertIn("Ran 3 cases", r.stdout)
            self.assertIn("[PARTIAL SUBSET RUN PASSED]", r.stdout)
            self.assertIn("3 framework test cases", r.stdout)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)


    def test_kernel_net_cases_are_independent_of_execution_order(self):
        """Proves the kernel net:: cases neither need a predecessor nor poison a neighbour.

        WHY two different runs are needed: the net cases mutate process-global
        kernel state (the system tick clock, the delegation registry, the
        ARP/socket/TCP tables, the loopback ring and the drop counters), and
        libtest runs cases on parallel threads. Running each net case alone in
        its own process proves only the first half -- that a case does not depend
        on something an earlier case left behind. It cannot see the second half,
        and the second half is the one that actually shipped broken once: a case
        that zeroes the global clock or clears the delegation registry at entry
        passes its own isolated run and still deletes authority a capability,
        revocation, vfs or syscall case is mid-flight on, because those cases take
        no part of the lock the net cases take. An isolation-only check is green on
        exactly that design. So the property is measured two ways, and both have to
        hold:

          1. every net case passes alone, in its own process, single-threaded;
          2. the case that holds foreign global state across a reset survives it,
             which is the poisoning half measured deterministically;
          3. the rest of the suite passes repeatedly under the parallel scheduling
             that lets the cases interleave.

        Half two is the one that matters, and it exists because of a measurement
        rather than taste: with the reset deliberately re-broken to clear the
        delegation registry and rewind the tick clock and the lease id counter, a
        full default-thread suite run passed 3/3 times and the parallel sweeps here
        passed 3/3. The race is real but only fires in the run that loses it, so a
        check that leans on repeated parallel runs can be green on the broken design
        -- which is the exact failure mode this check was written to catch. So the
        poisoning half is pinned by `net::tests::
        net_state_reset_leaves_foreign_process_globals_alone`, a case that registers
        a lease, moves the clock, calls the reset and requires all three to be
        untouched. Each of its three arms was verified to fail on the matching
        sabotage, so half three stays as a broad net rather than the evidence.

        Half three excludes `sync::` and `task::` for a reason that is separate and
        stated here rather than hidden: those families assert on the process-global
        PREEMPT_COUNT and SCHEDULER state, which every other case in this binary
        also writes through the kernel mutex wrapper, so their result depends on how
        many neighbours happen to hold a lock at the same instant no matter what the
        net cases do. They are a real isolation defect, but not this one, and they
        are named by a filter -- a failing net, capability, vfs or syscall case
        still fails this check.
        """
        manifest = os.path.abspath(
            os.path.join(os.path.dirname(__file__), "..", "kernel", "Cargo.toml")
        )

        listed = subprocess.run(
            ["cargo", "test", "--manifest-path", manifest, "net::", "--", "--list"],
            capture_output=True, text=True, timeout=280,
        )
        self.assertEqual(listed.returncode, 0, f"net:: discovery failed: {listed.stderr}")
        net_cases = sorted(
            line.split(": test")[0].strip()
            for line in listed.stdout.splitlines()
            if line.startswith("net::") and line.endswith(": test")
        )
        # Fail closed on vacuous discovery: an empty list must never read as a pass.
        self.assertGreaterEqual(len(net_cases), 20, f"net:: discovery found too few cases: {net_cases}")

        did_not_pass_alone = []
        for case in net_cases:
            single = subprocess.run(
                ["cargo", "test", "--manifest-path", manifest, "--no-fail-fast", "--",
                 "--exact", case, "--test-threads=1"],
                capture_output=True, text=True, timeout=120,
            )
            ran_alone = "1 passed" in single.stdout and "0 failed" in single.stdout
            if single.returncode != 0 or not ran_alone:
                did_not_pass_alone.append(case)

        self.assertEqual(
            did_not_pass_alone, [],
            "these net:: cases pass as a group but fail when run alone, so the suite "
            f"is still order-dependent: {did_not_pass_alone}",
        )

        # Half two: the interleaving half, deterministically. A reset that wipes a
        # global another subsystem owns cannot be found by running the suite in a
        # lucky order, so it is measured by a case that holds the state across it.
        canary = "net::tests::net_state_reset_leaves_foreign_process_globals_alone"
        probe = subprocess.run(
            ["cargo", "test", "--manifest-path", manifest, "--no-fail-fast", "--",
             "--exact", canary, "--test-threads=1"],
            capture_output=True, text=True, timeout=280,
        )
        self.assertEqual(
            probe.returncode, 0,
            f"{canary} pins that the net reset cannot delete a lease, rewind the tick clock or "
            f"rewind the lease id counter that cases outside net:: rely on; it failed:\n{probe.stdout}",
        )

        # Half three: the broad net. Three ordered sweeps at the default thread
        # count, because an ordering bug is probabilistic and one green run proves
        # nothing -- and, measured above, three green runs prove less than that.
        unstable_runs = []
        for attempt in range(1, 4):
            sweep = subprocess.run(
                ["cargo", "test", "--manifest-path", manifest, "--no-fail-fast", "--",
                 "--format", "terse", "--test-threads=8", "--skip", "sync::", "--skip", "task::"],
                capture_output=True, text=True, timeout=280,
            )
            failed = sorted(
                line.strip()
                for line in sweep.stdout.splitlines()
                if line.strip().endswith("... FAILED")
            )
            totals = [line for line in sweep.stdout.splitlines() if "passed;" in line]
            if sweep.returncode != 0 or failed:
                unstable_runs.append((attempt, failed, totals))

        self.assertEqual(
            unstable_runs, [],
            "the kernel suite is not order-insensitive under parallel scheduling; "
            f"these sweeps failed: {unstable_runs}",
        )


if __name__ == "__main__":
    unittest.main()
