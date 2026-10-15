import ast
import glob
import os
import re
import subprocess
import sys


SKIPPED_PATTERNS = [
    "test_serial_tcp",
    "test_interactive_lifecycle",
    "test_lifecycle_stress",
    "test_reboot_recovery",
    "test_persistence",
    "test_storage_recovery",
    "test_crash_watchdog",
    "test_verify",
]


def normalize_path(p: str) -> str:
    p_abs = os.path.abspath(p)
    cwd = os.path.abspath(".")
    try:
        rel = os.path.relpath(p_abs, cwd)
        if not rel.startswith(".."):
            return rel.replace("\\", "/")
    except Exception:
        pass
    return p_abs.replace("\\", "/")


def is_framework_test(path: str) -> bool:
    """Inspects file AST to determine if it defines pytest test functions or unittest classes."""
    try:
        with open(path, "r", encoding="utf-8") as f:
            tree = ast.parse(f.read(), filename=path)
        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef) and node.name.startswith("test_"):
                return True
            if isinstance(node, ast.ClassDef):
                if "Test" in node.name:
                    return True
                for b in node.bases:
                    base_name = getattr(b, "id", getattr(b, "attr", ""))
                    if "Test" in base_name or "TestCase" in base_name:
                        return True
        return False
    except Exception:
        return False


def run_contracts(target_files=None):
    all_test_files = sorted(normalize_path(p) for p in glob.glob("tests/test_*.py"))
    
    if target_files:
        tests = sorted(normalize_path(p) for p in target_files)
        skipped = []
    else:
        tests = sorted([
            p for p in all_test_files
            if not any(k in p for k in SKIPPED_PATTERNS)
        ])
        skipped = sorted([p for p in all_test_files if p not in tests])

    framework_tests = [p for p in tests if is_framework_test(p)]
    script_tests = [p for p in tests if not is_framework_test(p)]

    print(f"Discovered {len(tests)} contract test files ({len(framework_tests)} framework, {len(script_tests)} script).")
    if skipped:
        print(f"Runtime/externally-dependent tests not executed: {len(skipped)}")
        for p in skipped:
            print(f"  SKIP: {p}")

    total_executed = 0
    failures = []
    file_reports = {}

    # 1. Execute framework tests (pytest/unittest) via pytest
    if framework_tests:
        cmd = [sys.executable, "-m", "pytest", "-v", "--tb=short"] + framework_tests
        r = subprocess.run(cmd, capture_output=True, text=True)

        pt_results = {p: {"passed": 0, "failed": 0, "errors": 0} for p in framework_tests}
        for line in r.stdout.splitlines():
            m = re.match(r"^([^\s:]+\.py)::([^\s]+)\s+([A-Z]+)", line)
            if m:
                fpath = normalize_path(m.group(1))
                status = m.group(3)
                if fpath in pt_results:
                    if status == "PASSED":
                        pt_results[fpath]["passed"] += 1
                    elif status == "FAILED":
                        pt_results[fpath]["failed"] += 1
                    else:
                        pt_results[fpath]["errors"] += 1

        for p in framework_tests:
            stats = pt_results[p]
            if stats["failed"] > 0 or stats["errors"] > 0:
                failures.append(p)
                file_reports[p] = (False, f"FAILED: {stats['failed']} failed, {stats['passed']} passed, {stats['errors']} errors")
            elif stats["passed"] == 0:
                # 0 executed tests for a framework test file is an execution failure
                failures.append(p)
                file_reports[p] = (False, "FAILED: 0 test cases executed")
            else:
                total_executed += stats["passed"]
                file_reports[p] = (True, f"Ran {stats['passed']} cases")

        if r.returncode != 0 and not any(p in failures for p in framework_tests):
            # Collection or syntax error prevented test discovery
            failures.append("pytest_collection")
            print("Pytest collection or invocation error:")
            print(r.stdout)
            print(r.stderr)
        elif r.returncode != 0:
            # Print pytest failure output
            print("\n--- Pytest Output ---")
            print(r.stdout)
            if r.stderr:
                print(r.stderr)
            print("---------------------\n")

    # 2. Execute script-based contract tests directly
    for p in script_tests:
        r = subprocess.run([sys.executable, p], capture_output=True, text=True)
        if r.returncode != 0:
            failures.append(p)
            file_reports[p] = (False, f"FAILED (exit code {r.returncode})")
            print(f"FAIL: {p}")
            if r.stdout:
                print(r.stdout)
            if r.stderr:
                print(r.stderr)
        else:
            total_executed += 1
            file_reports[p] = (True, "Ran 1 script contract")

    # 3. Print overall status per file in order
    for p in tests:
        passed, detail = file_reports.get(p, (False, "NOT EXECUTED"))
        status = "PASS" if passed else "FAIL"
        print(f"{status}: {p} -> {detail}")

    if failures:
        print(f"\n{len(failures)} test files failed!")
        return 1
    else:
        print(f"\nALL {total_executed} LOCAL CONTRACT TEST CASES PASSED.")
        if skipped:
            print("Runtime/externally-dependent tests remain unexecuted; run them separately on the local QEMU/Device Studio environment.")
        return 0


def main():
    target = sys.argv[1:] if len(sys.argv) > 1 else None
    sys.exit(run_contracts(target))


if __name__ == "__main__":
    main()
