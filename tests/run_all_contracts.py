import glob
import re
import subprocess
import sys

def count_cases(path):
    """Counts the unittest methods a contract file actually runs."""
    with open(path, "r", encoding="utf-8") as handle:
        source = handle.read()
    return len(re.findall(r"^\s+def test_", source, re.MULTILINE))

def main():
    failures = []
    tests = sorted([
        p for p in glob.glob("tests/test_*.py")
        if not any(k in p for k in ["test_serial_tcp", "test_interactive_lifecycle", "test_lifecycle_stress", "test_reboot_recovery", "test_persistence", "test_storage_recovery", "test_crash_watchdog"])
    ])
    skipped = sorted(glob.glob("tests/test_*.py"))
    skipped = [p for p in skipped if p not in tests]
    total = sum(count_cases(p) for p in tests)
    print(f"Discovered {len(tests)} contract test files running {total} cases.")
    if skipped:
        print(f"Runtime/externally-dependent tests not executed: {len(skipped)}")
        for p in skipped:
            print(f"  SKIP: {p}")
    for p in tests:
        r = subprocess.run([sys.executable, p], capture_output=True, text=True)
        if r.returncode != 0:
            print(f"FAIL: {p}")
            print(r.stdout)
            print(r.stderr)
            failures.append(p)
        else:
            ran = re.search(r"Ran (\d+) tests?", r.stdout)
            detail = f"Ran {ran.group(1)} cases" if ran else "OK"
            print(f"PASS: {p} -> {detail}")
    if failures:
        print(f"\n{len(failures)} test files failed!")
        sys.exit(1)
    else:
        print(f"\nALL {total} LOCAL CONTRACT TEST CASES PASSED.")
        if skipped:
            print("Runtime/externally-dependent tests remain unexecuted; run them separately on the local QEMU/Device Studio environment.")

if __name__ == "__main__":
    main()
