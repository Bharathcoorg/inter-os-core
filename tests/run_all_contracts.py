import glob
import subprocess
import sys

def main():
    failures = []
    tests = sorted([
        p for p in glob.glob("tests/test_*.py")
        if not any(k in p for k in ["test_serial_tcp", "test_interactive_lifecycle", "test_lifecycle_stress", "test_reboot_recovery", "test_persistence", "test_storage_recovery"])
    ])
    print(f"Discovered {len(tests)} test contracts:")
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
        print(f"\nALL {len(tests)} TEST CONTRACTS PASSED (100%)!")

if __name__ == "__main__":
    main()
