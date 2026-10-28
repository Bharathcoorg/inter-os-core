import ast
import glob
import os
import re
import subprocess
import sys


# Explicit, auditable skip list for genuinely runtime-dependent tests
# that require live QEMU or external device environments.
RUNTIME_DEPENDENT_SKIPPED_FILES = frozenset({
    "tests/qemu/test_boot_image_immutable.py",
    "tests/qemu/test_qemu_boot.py",
    "tests/qemu/test_vault_persistence.py",
    "tests/test_crash_watchdog.py",
    "tests/test_interactive_lifecycle.py",
    "tests/test_lifecycle_stress.py",
    "tests/test_persistence.py",
    "tests/test_reboot_recovery.py",
    "tests/test_serial_tcp.py",
    "tests/test_storage_recovery.py",
    "tests/test_verify_pin_and_persistence.py",
    "tests/test_verify_wallpaper_customization.py",
})

# Minimum expected contract test suite floor to prevent silent gate shrinkage
MIN_EXPECTED_CONTRACT_SUITES = 50


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


def run_framework_suite(path: str, timeout: int):
    """Runs one framework contract suite on its own and reports it like the batch.

    WHY this exists: the suite gate runs every framework file in one pytest
    process, and a process that outlasts its budget takes the results of every
    suite in it with it. When the machine is busy -- several agents building at
    once -- that erased the whole report. Each suite gets its own process and
    its own budget here, so a slow suite costs only its own result.

    Returns (passed, detail, cases_passed) in the same shape the batch uses.
    """
    cmd = [
        sys.executable, "-m", "pytest", "-v", "--tb=short",
        "--continue-on-collection-errors",
        "--ignore-glob=*test_tmp_*",
        path,
    ]
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return False, f"NOT RUN: the suite timed out after {timeout}s", 0

    passed = failed = errors = 0
    for line in r.stdout.splitlines():
        m = re.match(r"^([^\s:]+\.py)::([^\s]+)\s+([A-Z]+)", line)
        if not m:
            continue
        status = m.group(3)
        if status == "PASSED":
            passed += 1
        elif status == "FAILED":
            failed += 1
        elif status not in ("SKIPPED",):
            errors += 1

    if failed or errors:
        return False, f"FAILED: {failed} failed, {passed} passed, {errors} errors", 0
    if passed == 0:
        return False, "FAILED: 0 test cases executed", 0
    return True, f"Ran {passed} cases", passed


def is_framework_test(path: str) -> bool:
    """Inspects file AST to determine if it defines pytest test functions or unittest classes."""
    try:
        with open(path, "r", encoding="utf-8") as f:
            tree = ast.parse(f.read(), filename=path)
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name.startswith("test_"):
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


def has_verifiable_assertions(path: str) -> bool:
    """Verifies that a script contract defines at least one assertion or failure exit."""
    try:
        with open(path, "r", encoding="utf-8") as f:
            tree = ast.parse(f.read(), filename=path)
        for node in ast.walk(tree):
            if isinstance(node, ast.Assert):
                return True
            if isinstance(node, ast.Raise):
                return True
            if isinstance(node, ast.Call):
                func_name = getattr(node.func, "id", getattr(node.func, "attr", ""))
                if func_name in ("must", "assert_true", "assert_false", "fail", "bail"):
                    return True
        return False
    except Exception:
        return False


def run_contracts(target_files=None):
    all_test_files = sorted(
        normalize_path(p) for p in glob.glob("tests/**/test_*.py", recursive=True)
        if not os.path.basename(p).startswith("test_tmp_")
    )

    is_subset_run = bool(target_files)
    if is_subset_run:
        tests = sorted(normalize_path(p) for p in target_files)
        skipped = []
    else:
        # Fail closed if zero test files discovered
        if not all_test_files:
            print("ERROR: Zero contract test files discovered! Check working directory or target paths.", file=sys.stderr)
            return 1

        # Prevent deleted or undiscovered expected tests from silently shrinking the test gate
        if len(all_test_files) < MIN_EXPECTED_CONTRACT_SUITES:
            print(
                f"ERROR: Discovered only {len(all_test_files)} test suites, expected at least "
                f"{MIN_EXPECTED_CONTRACT_SUITES}. Test gate has shrunk or discovery is incomplete!",
                file=sys.stderr
            )
            return 1

        tests = sorted([
            p for p in all_test_files
            if p not in RUNTIME_DEPENDENT_SKIPPED_FILES
        ])
        skipped = sorted([p for p in all_test_files if p in RUNTIME_DEPENDENT_SKIPPED_FILES])

    if not tests:
        print("ERROR: Zero contract test files selected for execution!", file=sys.stderr)
        return 1

    framework_tests = [p for p in tests if is_framework_test(p)]
    script_tests = [p for p in tests if not is_framework_test(p)]

    if is_subset_run:
        print(f"[PARTIAL SUBSET RUN: {len(tests)} of {len(all_test_files)} test suites selected]")
        print(f"  - Active suites to execute:     {len(tests)} ({len(framework_tests)} framework, {len(script_tests)} script)")
        print(f"  - Repository suites unselected: {len(all_test_files) - len(tests)}")
    else:
        print(f"Discovered {len(all_test_files)} contract test suites across repository:")
        print(f"  - Active suites to execute:     {len(tests)} ({len(framework_tests)} framework, {len(script_tests)} script)")
        print(f"  - Intentionally skipped suites: {len(skipped)} (runtime/QEMU-dependent)")
        if skipped:
            for p in skipped:
                print(f"    SKIP: {p}")

    framework_cases_passed = 0
    script_contracts_passed = 0
    failures = []
    file_reports = {}

    # 1. Execute framework tests (pytest/unittest) via pytest
    pytest_timeout = int(os.environ.get("INTEROS_PYTEST_TIMEOUT", "900"))
    file_timeout = int(os.environ.get("INTEROS_FILE_TIMEOUT", "300"))
    script_timeout = int(os.environ.get("INTEROS_SCRIPT_TIMEOUT", "90"))

    if framework_tests:
        try:
            # WHY the collection errors are continued past: one file with a
            # module-level assertion that fails aborts the whole collection,
            # and every other suite then reports zero cases -- the suite
            # stops telling the truth about the platform while looking as
            # though it is running. A broken file must be reported as
            # broken, and the rest must still run.
            cmd = [
                sys.executable, "-m", "pytest", "-v", "--tb=short",
                "--continue-on-collection-errors",
                "--ignore-glob=*test_tmp_*",
            ] + framework_tests
            r = subprocess.run(cmd, capture_output=True, text=True, timeout=pytest_timeout)

            pt_results = {p: {"passed": 0, "failed": 0, "errors": 0, "skipped": 0} for p in framework_tests}
            for line in r.stdout.splitlines():
                m = re.match(r"^([^\s:]+\.py)::([^\s]+)\s+([A-Z]+)", line)
                if m:
                    fpath = normalize_path(m.group(1))
                    status = m.group(3)
                    if fpath in pt_results:
                        if status == "PASSED":
                            pt_results[fpath]["passed"] += 1
                        elif status == "SKIPPED":
                            pt_results[fpath]["skipped"] += 1
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
                    framework_cases_passed += stats["passed"]
                    file_reports[p] = (True, f"Ran {stats['passed']} cases")

            if r.returncode != 0 and not any(p in failures for p in framework_tests):
                # Collection or syntax error prevented test discovery
                failures.append("pytest_collection")
                print("Pytest collection or invocation error:")
                print(r.stdout)
                print(r.stderr)
            elif r.returncode != 0:
                print("\n--- Pytest Output ---")
                print(r.stdout)
                if r.stderr:
                    print(r.stderr)
                print("---------------------\n")
        except subprocess.TimeoutExpired:
            # WHY the batch is re-run one suite at a time instead of every suite
            # being reported as "NOT RUN": the batch is a single pytest process
            # over every suite, so a batch that outlasts its budget loses the
            # results of the suites that had already passed. Under a loaded
            # machine -- several agents building at once -- that turned sixty
            # green suites into sixty "NOT RUN" lines, which says less about
            # the platform than the gate that ran them one at a time. Each
            # suite now gets its own budget and its own report, so a suite
            # that is genuinely slow is the only one that is lost.
            print("ERROR: Pytest framework execution timed out after "
                  f"{pytest_timeout}s; re-running each suite on its own.",
                  file=sys.stderr)
            for p in framework_tests:
                ok, detail, cases = run_framework_suite(p, file_timeout)
                if ok:
                    framework_cases_passed += cases
                    file_reports[p] = (True, detail)
                else:
                    failures.append(p)
                    file_reports[p] = (False, detail)

    # 2. Execute script-based contract tests directly
    for p in script_tests:
        if not has_verifiable_assertions(p):
            failures.append(p)
            file_reports[p] = (False, "FAILED: 0 assertions or verifiable test execution defined")
            continue

        try:
            r = subprocess.run([sys.executable, p], capture_output=True, text=True, timeout=script_timeout)
            if r.returncode != 0:
                failures.append(p)
                file_reports[p] = (False, f"FAILED (exit code {r.returncode})")
                print(f"FAIL: {p}")
                if r.stdout:
                    print(r.stdout)
                if r.stderr:
                    print(r.stderr)
            else:
                script_contracts_passed += 1
                file_reports[p] = (True, "Ran 1 script contract")
        except subprocess.TimeoutExpired:
            failures.append(p)
            file_reports[p] = (False, f"FAILED (timed out after {script_timeout}s)")
            print(f"FAIL: {p} (timed out after {script_timeout}s)")

    # 3. Print overall status per file in order
    for p in tests:
        passed, detail = file_reports.get(p, (False, "NOT EXECUTED"))
        status = "PASS" if passed else "FAIL"
        print(f"{status}: {p} -> {detail}")

    total_passed = framework_cases_passed + script_contracts_passed

    if failures:
        print(f"\n{len(failures)} test files failed!")
        return 1
    elif total_passed == 0:
        print("\nERROR: 0 total test cases executed across all discovered files!", file=sys.stderr)
        return 1
    elif is_subset_run:
        print(f"\n[PARTIAL SUBSET RUN PASSED]")
        print(f"Executed: {len(tests)} selected suites ({len(all_test_files) - len(tests)} suites in repository unexecuted).")
        print(f"Passed:   {framework_cases_passed} framework test cases + {script_contracts_passed} script contracts (total {total_passed} test cases/contracts).")
        return 0
    else:
        print(f"\nALL LOCAL CONTRACT TEST CASES PASSED.")
        print(f"Passed:   {framework_cases_passed} framework test cases across {len(framework_tests)} suites + {script_contracts_passed} standalone script contracts (total {total_passed} across {len(tests)} active suites).")
        if skipped:
            print(f"Notice:   {len(skipped)} runtime/externally-dependent test suites remain unexecuted; run them separately on the local QEMU/Device Studio environment.")
        return 0


def main():
    target = sys.argv[1:] if len(sys.argv) > 1 else None
    sys.exit(run_contracts(target))


if __name__ == "__main__":
    main()
