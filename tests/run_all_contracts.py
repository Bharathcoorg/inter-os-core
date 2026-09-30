import glob
import subprocess
import sys

def main():
    failures = []
    tests = sorted([
        p for p in glob.glob("tests/test_*.py")
        if not any(k in p for k in ["test_serial_tcp", "test_interactive_lifecycle", "test_lifecycle_stress", "test_reboot_recovery", "test_persistence", "test_storage_recovery", "test_crash_watchdog"])
    ])
    skipped = sorted(glob.glob("tests/test_*.py"))
    skipped = [p for p in skipped if p not in tests]
    print(f"Discovered {len(tests)} local contract tests.")
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
            first_line = r.stdout.strip().split("\n")[0] if r.stdout else "OK"
            print(f"PASS: {p} -> {first_line}")
    if failures:
        print(f"\n{len(failures)} tests failed!")
        sys.exit(1)
    else:
        print(f"\nALL {len(tests)} LOCAL CONTRACT TESTS PASSED.")
        if skipped:
            print("Runtime/externally-dependent tests remain unexecuted; run them separately on the local QEMU/Device Studio environment.")

if __name__ == "__main__":
    main()
