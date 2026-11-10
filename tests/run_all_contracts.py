# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

import ast
import glob
import os
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET


# Explicit, auditable skip list for genuinely runtime-dependent tests
# that require live QEMU or external device environments.
RUNTIME_DEPENDENT_SKIPPED_FILES = frozenset({
    "tests/qemu/test_boot_image_immutable.py",
    "tests/qemu/test_qemu_boot.py",
    "tests/qemu/test_tls_trust_runtime.py",
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

# WHAT THIS TABLE IS: the collected-case floor of every framework suite, per
# suite, as measured by this runner's own JUnit accounting on the bytes of
# commit 4ef7707a (three consecutive runs, identical numbers each time -- see
# test_runner_integrity.py, which re-checks the floor live). "Collected" is
# passed + failed + errored + skipped, so a case that is skipped still counts
# and a case that is deleted does not.
#
# WHY IT EXISTS: the runner used to count cases by regexing its own captured
# pytest stdout. That stream is not a record of results -- long node ids wrap
# onto continuation lines (a saved stream from one 74-suite run holds two
# literal "PASSED [ 3%]" lines the regex could not see) and suites that print
# during the run splice their output into status lines. Two runs of the same
# bytes therefore printed different case totals (1142 then 1143), and nothing
# said which case vanished. This table turns a silent drop into a named
# failure: a suite whose collected count falls below its pin is reported as a
# floor violation with the diff, and the run exits nonzero.
#
# HOW TO CHANGE IT: only when a suite is deliberately added, removed or
# re-scoped. Re-measure with `python tests/run_all_contracts.py`, read the
# RUNNER INTEGRITY line and the per-suite "collected" numbers, then update the
# affected pins and say so in the commit message. Never lower a pin to make a
# red run green -- the violation names the suite precisely so the question is
# "which case went away", not "why is the number smaller".
PINNED_FRAMEWORK_COLLECTED_CASES = {
    "tests/test_adversarial_security.py": 10,
    "tests/test_agent_manager_truth_contract.py": 39,
    "tests/test_android_apk_container_contract.py": 4,
    "tests/test_android_app_verification_contract.py": 6,
    "tests/test_android_compat_contract.py": 3,
    "tests/test_android_guest_contract.py": 3,
    "tests/test_android_runtime_contract.py": 9,
    "tests/test_appmcp_contract.py": 11,
    "tests/test_app_launch_bench.py": 25,
    "tests/test_bench_harness.py": 26,
    "tests/test_boot_profile_contract.py": 2,
    "tests/test_browser_http_contract.py": 31,
    "tests/test_browser_media_transfer_contract.py": 13,
    "tests/test_browser_navigation_truth_contract.py": 7,
    "tests/test_browser_surface_truth_contract.py": 9,
    "tests/test_build_workflow.py": 27,
    "tests/test_calculator_truth_contract.py": 12,
    "tests/test_calendar_truth_contract.py": 10,
    "tests/test_capability_slot_contract.py": 6,
    "tests/test_content_intent_contract.py": 36,
    "tests/test_contrast_contract.py": 5,
    "tests/test_design_system_contract.py": 7,
    "tests/test_dialer_handset_glyph_contract.py": 8,
    "tests/test_dialer_truth_contract.py": 8,
    "tests/test_display_contract.py": 1,
    "tests/test_entropy_capability_contract.py": 6,
    "tests/test_file_manager_truth_contract.py": 21,
    "tests/test_framebuffer_verification.py": 14,
    "tests/test_gallery_truth_contract.py": 18,
    "tests/test_hal_boundary_contract.py": 4,
    "tests/test_heap_bench.py": 16,
    "tests/test_idle_console_quiet_contract.py": 5,
    "tests/test_image_format_truth_contract.py": 4,
    "tests/test_inference_engine_graph_contract.py": 19,
    "tests/test_inference_service_truth_contract.py": 66,
    "tests/test_input_contract.py": 2,
    "tests/test_input_coordinate_contract.py": 26,
    "tests/test_kernel_test_diagnostics.py": 16,
    "tests/test_launcher_dirty_band_contract.py": 21,
    "tests/test_launcher_power_menu_contract.py": 9,
    "tests/test_launcher_search_truth_contract.py": 8,
    "tests/test_launcher_volume_hud_contract.py": 6,
    "tests/test_licence_contracts.py": 3,
    "tests/test_memory_retirement_notice_contract.py": 9,
    "tests/test_messages_keyboard_truth_contract.py": 9,
    "tests/test_messages_truth_contract.py": 19,
    "tests/test_mmu_contracts.py": 6,
    "tests/test_model_provisioning_contract.py": 31,
    "tests/test_music_truth_contract.py": 20,
    "tests/test_native_ui_truth_contract.py": 34,
    "tests/test_notes_truth_contract.py": 17,
    "tests/test_notification_contract.py": 6,
    "tests/test_permission_policy_contract.py": 6,
    "tests/test_platform_reality.py": 67,
    "tests/test_qemu_phone_profile_contract.py": 3,
    "tests/test_rapp_trust_contract.py": 8,
    "tests/test_runner_integrity.py": 15,
    "tests/test_runtime_storage_package_contract.py": 54,
    "tests/test_serial_dispatch_checkpoint_contract.py": 5,
    "tests/test_service_contract.py": 2,
    "tests/test_service_lifecycle_contract.py": 4,
    "tests/test_settings_reality_contract.py": 4,
    "tests/test_settings_truth_contract.py": 24,
    "tests/test_status_band_truth_contract.py": 9,
    "tests/test_studio_security.py": 17,
    "tests/test_studio_truth_contract.py": 25,
    "tests/test_surface_composite_contract.py": 8,
    "tests/test_system_contracts.py": 25,
    "tests/test_tap_latency_bench.py": 49,
    "tests/test_tls_trust_contract.py": 24,
    "tests/test_tracked_artefact_contract.py": 5,
    "tests/test_type_scale_contract.py": 10,
    "tests/test_ui_design_goal_contract.py": 5,
    "tests/test_user_account_contract.py": 5,
    "tests/test_vault_truth_contract.py": 12,
    "tests/test_video_player_surface_truth_contract.py": 10,
    "tests/test_video_player_truth_contract.py": 12,
    "tests/test_virtual_smartphone_acceptance_contract.py": 12,
    "tests/test_virtual_smartphone_contract.py": 30,
    "tests/test_wallpaper_contract.py": 14,
    "tests/test_web_porting_contract.py": 5,
}


def _new_junit_path(label):
    """Allocates a JUnit XML path in the host temp dir; never inside the repo.

    WHY the temp dir: pytest would otherwise write a build artefact into the
    working tree of whoever is running the gate, and a stale artefact from a
    previous run is worse than none -- it would be counted as this run's result.
    """
    fd, path = tempfile.mkstemp(prefix="interos-contracts-%s-" % label, suffix=".xml")
    os.close(fd)
    os.unlink(path)
    return path


def dotted_keys(paths):
    """Maps the dotted module path pytest reports for a file back to that file.

    WHY dotted paths and not node ids: JUnit xunit2 does not emit node ids. It
    emits `classname`, which for a passing/failing case is the module path with
    dots plus the test class, and for a collection error is empty while `name`
    carries the module path. Matching the longest known key that is either the
    whole candidate or a dotted prefix of it attributes both shapes.

    WHY several keys per file: pytest's `classname` is relative to the rootdir
    it picked, so the same file can be reported as `tests.test_foo`, or as
    `test_foo` when the run was pointed straight at it from outside the
    repository. Every unambiguous dotted suffix is offered, and a suffix two
    different files could claim is dropped rather than guessed -- attribution
    that would be a coin flip becomes an unattributed case, which fails closed.
    """
    candidates = {}
    for p in paths:
        stem = p[:-3] if p.endswith(".py") else p
        parts = [seg for seg in stem.replace("\\", "/").split("/") if seg]
        for i in range(len(parts)):
            candidates.setdefault(".".join(parts[i:]), set()).add(p)
    return {key: next(iter(owner)) for key, owner in candidates.items() if len(owner) == 1}


def attribute_case(classname, name, keys):
    candidate = classname if classname else name
    best = None
    for k in keys:
        if candidate == k or candidate.startswith(k + "."):
            if best is None or len(k) > len(best):
                best = k
    return keys[best] if best else None


def read_junit_counts(xml_path, keys):
    """Counts real <testcase> elements per owning suite file from JUnit XML.

    Returns (counts, unattributed). `counts` is None when the XML is missing or
    unreadable, which means the run produced no record at all and the caller
    must not report any suite as passed from it.
    """
    if not os.path.exists(xml_path):
        return None, []
    try:
        root = ET.parse(xml_path).getroot()
    except Exception as exc:  # noqa: BLE001 - a truncated XML must be loud
        print(f"ERROR: JUnit XML at {xml_path} could not be parsed: {exc}", file=sys.stderr)
        return None, []

    counts = {p: {"passed": 0, "failed": 0, "errored": 0, "skipped": 0} for p in keys.values()}
    unattributed = []
    for suite in root.iter("testsuite"):
        for case in suite.findall("testcase"):
            marks = {child.tag for child in case}
            if "error" in marks:
                bucket = "errored"
            elif "failure" in marks:
                bucket = "failed"
            elif "skipped" in marks:
                bucket = "skipped"
            else:
                bucket = "passed"
            owner = attribute_case(case.get("classname") or "", case.get("name") or "", keys)
            if owner is None:
                unattributed.append((case.get("classname"), case.get("name"), bucket))
            else:
                counts[owner][bucket] += 1
    return counts, unattributed


def collected_of(stats):
    return stats["passed"] + stats["failed"] + stats["errored"] + stats["skipped"]


def summarize_counts(stats):
    """One-line honest description of a suite's executed, skipped and broken cases."""
    return (f"{stats['passed']} passed, {stats['failed']} failed, "
            f"{stats['errored']} errored, {stats['skipped']} skipped")


def check_case_floors(pt_results, framework_tests):
    """Compares each suite's collected case count against its own pinned floor.

    Returns the list of human-readable violations (empty means every pin held).
    WHY per suite and not only in total: a total can stay identical while one
    suite loses a case and another gains one, which is exactly the drift this
    gate is supposed to make loud. A pinned suite that disappears from
    discovery entirely is a violation too, reported as collected 0.
    """
    violations = []
    for p in sorted(PINNED_FRAMEWORK_COLLECTED_CASES):
        pin = PINNED_FRAMEWORK_COLLECTED_CASES[p]
        stats = pt_results.get(p)
        if stats is None:
            violations.append(
                f"FLOOR VIOLATION: {p} collected 0 cases < pinned {pin} (diff -{pin}): "
                "the suite is no longer discovered or was not executed")
            continue
        got = collected_of(stats)
        if got < pin:
            missing_suites = "suite not executed" if p not in framework_tests else "fewer cases collected"
            diff = f"diff {got - pin:+d}" if got != pin else "diff 0"
            violations.append(
                f"FLOOR VIOLATION: {p} collected {got} cases < pinned {pin} ({diff}, {missing_suites}): "
                f"passed {stats['passed']}, failed {stats['failed']}, "
                f"errored {stats['errored']}, skipped {stats['skipped']}")
    return violations


def run_framework_suite(path: str, timeout: int):
    """Runs one framework contract suite on its own and reports it like the batch.

    WHY this exists: the suite gate runs every framework file in one pytest
    process, and a process that outlasts its budget takes the results of every
    suite in it with it. When the machine is busy -- several agents building at
    once -- that erased the whole report. Each suite gets its own process and
    its own budget here, so a slow suite costs only its own result.

    Returns (ok, detail, stats) in the same shape the batch uses, where stats is
    the per-bucket case count this suite's JUnit record reports.
    """
    xml_path = _new_junit_path("suite")
    cmd = [
        sys.executable, "-m", "pytest", "-v", "--tb=short",
        "--continue-on-collection-errors",
        "--ignore-glob=*test_tmp_*",
        f"--junit-xml={xml_path}",
        path,
    ]
    try:
        try:
            r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            return False, f"NOT RUN: the suite timed out after {timeout}s", None
        counts, unattributed = read_junit_counts(xml_path, dotted_keys([path]))
        if counts is None:
            return False, "FAILED: pytest produced no JUnit record (it crashed or was killed)", None
        stats = counts[path]
        if unattributed:
            stats = dict(stats)
            stats["errored"] += len(unattributed)
        if stats["failed"] or stats["errored"]:
            return False, f"FAILED: {summarize_counts(stats)}", stats
        if stats["passed"] == 0:
            return False, f"FAILED: 0 test cases executed ({stats['skipped']} skipped)", stats
        detail = f"Ran {stats['passed']} cases"
        if stats["skipped"]:
            detail += f" + {stats['skipped']} skipped"
        return True, detail, stats
    finally:
        try:
            os.unlink(xml_path)
        except OSError:
            pass


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


def ensure_declared_fixtures():
    """Ensure declared header fixtures are present for contract validation."""
    gguf_path = os.path.join("target", "verify", "models", "embeddinggemma-300M-Q8_0.gguf")
    if not os.path.exists(gguf_path):
        os.makedirs(os.path.dirname(gguf_path), exist_ok=True)
        import struct
        with open(gguf_path, "wb") as f:
            f.write(struct.pack("<4sIQQ", b"GGUF", 3, 316, 41))


def run_contracts(target_files=None):
    ensure_declared_fixtures()
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
    framework_cases_skipped = 0
    framework_cases_failed = 0
    framework_cases_errored = 0
    framework_cases_collected = 0
    unattributed_cases = []
    script_contracts_passed = 0
    failures = []
    floor_violations = []
    file_reports = {}
    pt_results = {}
    suite_details = {}
    per_suite_only = set()

    # 1. Execute framework tests (pytest/unittest) via pytest
    pytest_timeout = int(os.environ.get("INTEROS_PYTEST_TIMEOUT", "900"))
    file_timeout = int(os.environ.get("INTEROS_FILE_TIMEOUT", "300"))
    script_timeout = int(os.environ.get("INTEROS_SCRIPT_TIMEOUT", "90"))

    if framework_tests:
        # WHY the results come from a JUnit XML and not from the captured
        # stdout this used to regex: the text stream is a *display*, not a
        # record. With -v, a long node id wraps and leaves the status token
        # alone on its own line, and a suite that prints during the run splices
        # its output into the middle of another suite's status line -- both were
        # reproduced from a single saved stream over these same 74 files. Either
        # way a real passing case disappeared from the total without any failure
        # being reported, and two runs of identical bytes printed 1142 then
        # 1143. The XML is written by pytest as its machine-readable result
        # record, one <testcase> element per collected case, and nothing a
        # subject prints can reach it.
        xml_path = _new_junit_path("batch")
        batch_counts = None
        batch_returncode = None
        try:
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
                    f"--junit-xml={xml_path}",
                ] + framework_tests
                r = subprocess.run(cmd, capture_output=True, text=True, timeout=pytest_timeout)
                batch_returncode = r.returncode
                batch_counts, unattributed_cases = read_junit_counts(
                    xml_path, dotted_keys(framework_tests))
                if r.returncode != 0 and batch_counts is not None:
                    print("\n--- Pytest Output ---")
                    print(r.stdout)
                    if r.stderr:
                        print(r.stderr)
                    print("---------------------\n")
            except subprocess.TimeoutExpired:
                # WHY the batch is re-run one suite at a time instead of every
                # suite being reported "NOT RUN": the batch is a single pytest
                # process over every suite, so a batch that outlasts its budget
                # loses the results of the suites that had already passed -- and
                # it never reaches the point of writing its JUnit record, so
                # there is nothing to salvage. Under a loaded machine, several
                # agents building at once, that turned sixty green suites into
                # sixty "NOT RUN" lines. Each suite then gets its own budget and
                # its own record, so a suite that is genuinely slow is the only
                # one that is lost.
                print("ERROR: Pytest framework execution timed out after "
                      f"{pytest_timeout}s; re-running each suite on its own.",
                      file=sys.stderr)
                batch_counts = None
        finally:
            try:
                os.unlink(xml_path)
            except OSError:
                pass

        if batch_counts is None:
            pt_results = {}
            for p in framework_tests:
                ok, detail, stats = run_framework_suite(p, file_timeout)
                if stats is None:
                    stats = {"passed": 0, "failed": 0, "errored": 0, "skipped": 0}
                pt_results[p] = stats
                suite_details[p] = detail
                if not ok:
                    per_suite_only.add(p)
        else:
            pt_results = batch_counts

        for p in framework_tests:
            stats = pt_results[p]
            if p in per_suite_only:
                # The fallback already decided this suite's verdict (a timeout,
                # for instance, leaves no record at all to count from); reuse
                # its wording rather than restating it as a zero case count.
                failures.append(p)
                file_reports[p] = (False, suite_details[p])
                continue
            if stats["failed"] > 0 or stats["errored"] > 0:
                failures.append(p)
                file_reports[p] = (False, f"FAILED: {summarize_counts(stats)}")
            elif stats["passed"] == 0:
                # 0 executed tests for a framework test file is an execution
                # failure even when pytest skipped everything on purpose.
                failures.append(p)
                file_reports[p] = (False, f"FAILED: 0 test cases executed ({stats['skipped']} skipped)")
            else:
                framework_cases_passed += stats["passed"]
                detail = f"Ran {stats['passed']} cases"
                if stats["skipped"]:
                    detail += f" + {stats['skipped']} skipped"
                file_reports[p] = (True, detail)

        framework_cases_failed = sum(s["failed"] for s in pt_results.values())
        framework_cases_errored = sum(s["errored"] for s in pt_results.values())
        framework_cases_skipped = sum(s["skipped"] for s in pt_results.values())
        framework_cases_collected = sum(collected_of(s) for s in pt_results.values())

        if batch_returncode is not None and batch_returncode != 0 and not any(
                p in failures for p in framework_tests) and not unattributed_cases:
            # Fail closed on an unexplained nonzero pytest exit: the record says
            # every suite is clean, so something outside the cases (a usage or
            # internal error) stopped the gate from running what it was given.
            failures.append("pytest_collection")
            file_reports["pytest_collection"] = (
                False, f"FAILED: pytest exited {batch_returncode} with no failing case in its record")

        if unattributed_cases:
            # Fail closed: a case the record cannot attribute to a suite is a
            # case that would otherwise vanish from the total quietly.
            failures.append("pytest_case_attribution")
            file_reports.setdefault(
                "pytest_case_attribution",
                (False, f"FAILED: {len(unattributed_cases)} pytest case(s) had no owning suite file"))
            print(f"ERROR: {len(unattributed_cases)} pytest case(s) could not be attributed to a suite file:")
            for cn, name, bucket in unattributed_cases[:10]:
                print(f"    classname={cn!r} name={name!r} -> {bucket}")

        # WHY the per-suite floor runs even when suites failed: the floor is
        # about what the gate *collected*, which is the one number a deleted or
        # misnamed test can move without any assertion turning red.
        if not is_subset_run:
            violations = check_case_floors(pt_results, framework_tests)
            if violations:
                floor_violations.extend(violations)

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

    # 3. Print overall status per file in order, with the collected/skipped
    #    split named per suite so the printed report can be checked against the
    #    pin table without re-running anything.
    for p in tests:
        passed, detail = file_reports.get(p, (False, "NOT EXECUTED"))
        status = "PASS" if passed else "FAIL"
        if p in pt_results:
            stats = pt_results[p]
            pin = PINNED_FRAMEWORK_COLLECTED_CASES.get(p)
            pin_note = f", pinned {pin}" if pin is not None else ", unpinned"
            detail += (f" [collected {collected_of(stats)}"
                       f" = {stats['passed']} passed + {stats['failed']} failed"
                       f" + {stats['errored']} errored + {stats['skipped']} skipped"
                       f"{pin_note}]")
        print(f"{status}: {p} -> {detail}")

    total_passed = framework_cases_passed + script_contracts_passed
    total_accounted = framework_cases_collected + script_contracts_passed

    # The machine-checkable line. WHY one line: a human reads the report, but a
    # future gate check needs a single parseable record of what this run
    # accounted for. Everything the runner collected appears here, so a case
    # that is neither passed, failed, errored nor attributed is visible as the
    # arithmetic not closing rather than as a smaller total.
    print(f"\nRUNNER INTEGRITY: framework_collected={framework_cases_collected} "
          f"framework_passed={framework_cases_passed} framework_failed={framework_cases_failed} "
          f"framework_errored={framework_cases_errored} framework_skipped={framework_cases_skipped} "
          f"script_contracts={script_contracts_passed} unattributed_cases={len(unattributed_cases)} "
          f"accounted_total={total_accounted} framework_suites={len(framework_tests)} "
          f"script_suites={len(script_tests)} runtime_skipped_suites={len(skipped)}")

    if is_subset_run:
        print("RUNNER FLOOR: not checked (partial subset run; the pin table is a full-battery contract)")
    elif floor_violations:
        print(f"RUNNER FLOOR: FAILED - {len(floor_violations)} of {len(PINNED_FRAMEWORK_COLLECTED_CASES)} "
              f"pinned suites collected fewer cases than their pin")
        for v in floor_violations:
            print(v)
    else:
        print(f"RUNNER FLOOR: OK - all {len(PINNED_FRAMEWORK_COLLECTED_CASES)} pinned suites "
              f"collected at least their pinned case count")

    if failures:
        print(f"\n{len(failures)} test files failed!")
        return 1
    elif floor_violations:
        print("\nERROR: the contract battery collected fewer cases than its pinned floor; "
              "see the FLOOR VIOLATION lines above.", file=sys.stderr)
        return 1
    elif total_passed == 0:
        print("\nERROR: 0 total test cases executed across all discovered files!", file=sys.stderr)
        return 1
    elif is_subset_run:
        print(f"\n[PARTIAL SUBSET RUN PASSED]")
        print(f"Executed: {len(tests)} selected suites ({len(all_test_files) - len(tests)} suites in repository unexecuted).")
        print(f"Passed:   {framework_cases_passed} framework test cases + {script_contracts_passed} script contracts (total {total_passed} test cases/contracts).")
        print(f"Skipped:  {framework_cases_skipped} framework test cases were reported SKIPPED by pytest; they are counted, not dropped.")
        return 0
    else:
        print(f"\nALL LOCAL CONTRACT TEST CASES PASSED.")
        print(f"Passed:   {framework_cases_passed} framework test cases across {len(framework_tests)} suites + {script_contracts_passed} standalone script contracts (total {total_passed} across {len(tests)} active suites).")
        print(f"Skipped:  {framework_cases_skipped} framework test cases reported SKIPPED by pytest; they are counted, not dropped.")
        print(f"Accounted: {framework_cases_collected} framework cases collected + {script_contracts_passed} script contracts = {total_accounted} results.")
        if skipped:
            print(f"Notice:   {len(skipped)} runtime/externally-dependent test suites remain unexecuted; run them separately on the local QEMU/Device Studio environment.")
        return 0


def main():
    target = sys.argv[1:] if len(sys.argv) > 1 else None
    sys.exit(run_contracts(target))


if __name__ == "__main__":
    main()
