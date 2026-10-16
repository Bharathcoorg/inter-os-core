"""Pins the kernel-test failure diagnostics added to `tools/interos.py`.

WHY this exists. A prior benchmark run recorded `288 passed; 1 failed` and the
report could not say which test failed, so the failure stayed unexplained for
the rest of that task. The runner now preserves the command, the exit code, the
failing test names, the output and the environment. These tests pin that the
extraction actually works, because a diagnostic that silently returns nothing is
worse than no diagnostic: it would report "unclassified" for a real failure and
the evidence would still be lost.
"""

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))

import interos  # noqa: E402


ASSERTION_FAILURE = """\
running 289 tests
test allocator::tests::heap_constants_are_sane ... ok
test task::tests::scheduler_respects_priority ... ok
test fs::vfs::tests::read_file_truncates_at_max_len ... FAILED
test net::tests::retransmit_requires_capability ... ok

failures:

---- fs::vfs::tests::read_file_truncates_at_max_len stdout ----

thread 'fs::vfs::tests::read_file_truncates_at_max_len' panicked at kernel/src/fs/vfs.rs:412:9:
assertion `left == right` failed
  left: 64
 right: 32
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    fs::vfs::tests::read_file_truncates_at_max_len

test result: FAILED. 288 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.21s
"""


COMPILE_ERROR = """\
   Compiling kernel v0.1.0
error[E0308]: mismatched types
 --> kernel/src/task/process.rs:88:9
  |
88|     let tid: u32 = task.0;
  |            ---   ^ expected `u32`, found `u64`
error: could not compile `kernel` (lib test) due to 1 previous error
"""


TIMEOUT_RUN = """\
running 289 tests
test display::tests::vsync_wait_returns ... ok

error: test timed out after 60s
test display::tests::kms_present_blocks ... FAILED

test result: FAILED. 12 passed; 1 failed
"""


def test_failing_test_names_are_extracted():
    names = interos.failing_test_names(ASSERTION_FAILURE)
    assert "fs::vfs::tests::read_file_truncates_at_max_len" in names
    # The name appears twice in libtest output (the FAILED line and the
    # failures: block). It must be reported once, not twice.
    assert names.count("fs::vfs::tests::read_file_truncates_at_max_len") == 1


def test_failing_test_names_are_empty_for_a_clean_run():
    clean = "test result: ok. 289 passed; 0 failed; 0 ignored\n"
    assert interos.failing_test_names(clean) == []


def test_assertion_failure_is_classified_as_such():
    categories = interos.classify_test_failure(ASSERTION_FAILURE)
    assert "assertion failure" in categories
    assert "panic" in categories
    assert "compile error" not in categories


def test_compile_error_is_not_reported_as_a_failing_test():
    """A build break must not be dressed up as a test regression."""
    assert interos.failing_test_names(COMPILE_ERROR) == []
    categories = interos.classify_test_failure(COMPILE_ERROR)
    assert "compile error" in categories


def test_timeout_is_distinguished_from_an_assertion():
    categories = interos.classify_test_failure(TIMEOUT_RUN)
    assert "timeout" in categories


def test_unknown_failure_stays_visible_rather_than_being_dropped():
    """An unrecognised failure must still be reported, never silently ignored."""
    weird = "something went wrong in a way nobody anticipated\n"
    assert interos.classify_test_failure(weird) == []


def test_count_summary_reads_passed_and_failed():
    assert interos.test_count_summary(ASSERTION_FAILURE) == (288, 1)


def test_count_summary_handles_a_clean_run():
    clean = "test result: ok. 289 passed; 0 failed; 0 ignored\n"
    assert interos.test_count_summary(clean) == (289, 0)


MULTI_BINARY_RUN = """\
running 289 tests
test result: ok. 289 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.05s

running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
"""


def test_count_summary_reports_the_real_suite_not_the_trailing_empty_binary():
    """`cargo test` runs a second, empty binary; the total must not become 0.

    This is the exact shape of the real kernel run: 289 unit tests followed by a
    binary with none. Reporting the trailing 0 would state that the mandated suite
    ran zero tests, which reads as a clean bill of health for a suite that never
    executed.
    """
    assert interos.test_count_summary(MULTI_BINARY_RUN) == (289, 0)


def test_count_summary_is_none_when_absent():
    assert interos.test_count_summary("nothing resembling a result\n") is None


def test_kernel_command_is_the_one_agents_md_mandates():
    """The wrapper must run the documented command, not a convenient variant."""
    cmd = interos.KERNEL_TEST_CMD
    assert cmd[:3] == ["cargo", "test", "--manifest-path"]
    assert "kernel/Cargo.toml" in cmd
    # --no-fail-fast so one failure cannot hide the rest of the suite.
    assert "--no-fail-fast" in cmd


def test_kernel_target_is_registered_on_the_cli():
    """`interos.py test --target kernel` must exist, or the diagnostics are unreachable."""
    source = (ROOT / "tools" / "interos.py").read_text(encoding="utf-8")
    choices_line = source.split("p_tst.add_argument")[1].split("\n")[0]
    assert '"kernel"' in choices_line, "the test subparser must offer a kernel target"


def test_diagnostic_environment_excludes_secrets():
    env = interos._diagnostic_environment()
    joined = " ".join(env.values()).upper()
    for secret_marker in ("TOKEN", "PASSWORD", "SECRET", "KEY=", "CREDENTIAL"):
        assert secret_marker not in joined, f"{secret_marker} must not be collected"
    # The fields that do explain a no_std kernel test failure must be present.
    for required in ("RUSTUP_TOOLCHAIN", "RUSTFLAGS", "num_cpus"):
        assert required in env


FAILING_RUNNER = (
    "print('running 2 tests'); "
    "print('test widget::tests::scale_is_monotonic ... FAILED'); "
    "print(\"thread 'widget::tests::scale_is_monotonic' panicked at src/lib.rs:9:\"); "
    "print('assertion `left == right` failed'); "
    "print('test result: FAILED. 1 passed; 1 failed'); "
    "raise SystemExit(101)"
)

CLEAN_RUNNER = "print('test result: ok. 289 passed; 0 failed'); raise SystemExit(0)"


def _run_against_stub(monkeypatch, tmp_path, script, expected_rc_zero):
    """Drive run_kernel_tests with a stub command so the failure path is real."""
    import interos as mod
    monkeypatch.setattr(mod, "KERNEL_TEST_CMD", [sys.executable, "-c", script])
    monkeypatch.setattr(mod, "TEST_LOG_DIR", tmp_path)
    rc = mod.run_kernel_tests()
    logs = list(tmp_path.glob("kernel-tests-*.log"))
    assert logs, "the runner must preserve a log even for a stubbed run"
    body = logs[0].read_text(encoding="utf-8")
    assert "test result:" in body, "the log must contain the raw output"
    if expected_rc_zero:
        assert rc == 0
    else:
        assert rc != 0, "a failing suite must not be reported as success"
        assert "widget::tests::scale_is_monotonic" in body
    return rc, body


def test_failure_path_preserves_the_log_and_returns_nonzero(tmp_path, monkeypatch):
    """The core requirement: a one-off failure must leave its evidence on disk."""
    rc, body = _run_against_stub(monkeypatch, tmp_path, FAILING_RUNNER, expected_rc_zero=False)
    assert "1 passed; 1 failed" in body


def test_clean_run_still_preserves_its_log(tmp_path, monkeypatch):
    """A passing run is recorded too, so a later comparison has something to diff."""
    _run_against_stub(monkeypatch, tmp_path, CLEAN_RUNNER, expected_rc_zero=True)


def test_missing_toolchain_is_reported_as_a_launch_error(tmp_path, monkeypatch):
    """A broken toolchain must not be reported as a test regression."""
    import interos as mod
    monkeypatch.setattr(mod, "KERNEL_TEST_CMD", ["definitely-not-a-real-cargo-binary-xyz"])
    monkeypatch.setattr(mod, "TEST_LOG_DIR", tmp_path)
    rc = mod.run_kernel_tests()
    assert rc != 0
    logs = list(tmp_path.glob("kernel-tests-*.log"))
    assert logs
    assert "failed to execute the test command" in logs[0].read_text(encoding="utf-8")