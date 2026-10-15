import os
import subprocess
import sys
import tempfile
import unittest


class TestRunnerIntegrity(unittest.TestCase):
    def setUp(self):
        self.runner_path = os.path.abspath(os.path.join(os.path.dirname(__file__), "run_all_contracts.py"))

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
        # A file with an empty test suite or filtering out tests should not be marked as PASS
        with tempfile.NamedTemporaryFile("w", prefix="test_tmp_", suffix=".py", dir="tests", delete=False) as f:
            # File that imports pytest but has no matching tests
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
            # 3 actual tests
            f.write("def test_one(): pass\ndef test_two(): pass\ndef test_three(): pass\n")
            temp_path = f.name
        try:
            r = subprocess.run([sys.executable, self.runner_path, temp_path], capture_output=True, text=True)
            self.assertEqual(r.returncode, 0)
            self.assertIn("Ran 3 cases", r.stdout)
            self.assertIn("ALL 3 LOCAL CONTRACT TEST CASES PASSED.", r.stdout)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)


if __name__ == "__main__":
    unittest.main()
