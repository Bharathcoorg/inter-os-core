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


if __name__ == "__main__":
    unittest.main()
