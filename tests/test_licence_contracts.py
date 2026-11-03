# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""WHAT THIS FILE IS FOR: it puts the three licence gates inside the contract sweep.

WHY: `tools/license_headers.py` and `tools/license_manifests.py` only enforce
anything while somebody remembers to run them, and a licence is exactly the kind
of rule that silently rots -- this tree shipped 609 first-party sources with no
notice on any of them. `tests/run_all_contracts.py` discovers every `test_*.py`
under `tests/` and runs it, so a gate living here runs on every sweep and on
every `python tools/interos.py test --target contracts`; a gate living only in
the docs is a gate nobody runs.

WHY one case per gate rather than one case running three: a single case would
report "the licence check failed", and the reader would still have to find out
which of the three rules broke. The runner prints one line per framework case,
so each rule gets its own line, its own tool invocation, and its own failure
message naming the files the tool rejected.

WHY the tool's own words are carried into the assertion message: the tools print
the offending paths (`--check` lists the sources needing the notice, `--coverage`
lists those neither stamped nor declared, the manifests tool lists each manifest
that disagrees), and that list is the fix. Re-deriving it here would let this
file and the tool disagree about what failed.

WHY each case also reads the tool's summary line: every one of these gates walks
`git ls-files`, so run where nothing is tracked they report nothing wrong by
reporting nothing at all. A gate that is green because it saw zero files is not a
gate, so a case requires the tool's own count to be non-zero before it will call
the exit code a pass.
"""

import re
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# (tool, argument, the rule it enforces, a regex over its stdout proving the rule
# was applied to something rather than to an empty tree).
LICENCE_GATES = (
    (
        "tools/license_headers.py",
        "--check",
        "every first-party source file carries the AGPL-3.0-only SPDX notice",
        re.compile(r"\d+ of (?P<scanned>\d+) first-party source files"),
    ),
    (
        "tools/license_headers.py",
        "--coverage",
        "every tracked file is stamped or declared in REUSE.toml, and none is "
        "stamped under a notice that contradicts its declaration",
        re.compile(
            r"^coverage: (?P<undeclared>\d+) undeclared, (?P<contradicted>\d+) contradicted",
            re.MULTILINE,
        ),
    ),
    (
        "tools/license_manifests.py",
        "--check",
        "every Cargo and npm manifest's licence field says AGPL-3.0-only",
        re.compile(r"(?P<scanned>\d+) manifests checked"),
    ),
)

# WHY a timeout: these are static scans of the tracked tree and each finishes in
# well under a second, so one that hangs is a broken gate. The sweep must report
# that as a failure rather than stop with it.
GATE_TIMEOUT_SECONDS = 120


class TestLicenceContracts(unittest.TestCase):
    def _run_gate(self, tool, argument, rule, summary):
        """Run one licence gate, require exit 0, and hand back its stdout match."""
        script = ROOT / tool
        self.assertTrue(script.is_file(), f"licence gate is missing: {tool}")

        try:
            result = subprocess.run(
                [sys.executable, str(script), argument],
                cwd=str(ROOT),
                capture_output=True,
                text=True,
                timeout=GATE_TIMEOUT_SECONDS,
            )
        except subprocess.TimeoutExpired:
            self.fail(
                f"{tool} {argument} did not finish within {GATE_TIMEOUT_SECONDS}s; "
                f"the rule it guards is: {rule}"
            )

        transcript = (result.stdout or "") + (("\n" + result.stderr) if result.stderr else "")
        if result.returncode != 0:
            self.fail(
                f"{tool} {argument} exited {result.returncode}, so this rule is broken: "
                f"{rule}.\nThe files it named:\n{transcript}"
            )

        match = summary.search(result.stdout or "")
        self.assertIsNot(
            match,
            None,
            f"{tool} {argument} exited 0 without printing the summary line this gate "
            f"reads, so nothing shows the rule was ever applied:\n{transcript}",
        )
        return match

    def test_every_source_file_carries_the_licence_notice(self):
        """license_headers.py --check passes over a non-empty source set."""
        tool, argument, rule, summary = LICENCE_GATES[0]
        match = self._run_gate(tool, argument, rule, summary)
        self.assertGreater(
            int(match["scanned"]),
            0,
            f"{tool} {argument} scanned 0 first-party sources, so its exit code says "
            f"nothing about the rule: {rule}",
        )

    def test_every_tracked_file_is_stamped_or_declared(self):
        """license_headers.py --coverage reports neither undeclared nor contradicted."""
        tool, argument, rule, summary = LICENCE_GATES[1]
        match = self._run_gate(tool, argument, rule, summary)
        # WHY the buckets are read as well as the exit code: that summary line is
        # the tool's own verdict, and a gate exiting 0 while printing a non-zero
        # bucket is a contradiction this sweep must not step over.
        self.assertEqual(
            (int(match["undeclared"]), int(match["contradicted"])),
            (0, 0),
            f"{tool} {argument} exited 0 but reported undeclared="
            f"{match['undeclared']} contradicted={match['contradicted']} for the rule: {rule}",
        )

    def test_every_manifest_declares_the_platform_licence(self):
        """license_manifests.py --check passes over a non-empty manifest set."""
        tool, argument, rule, summary = LICENCE_GATES[2]
        match = self._run_gate(tool, argument, rule, summary)
        self.assertGreater(
            int(match["scanned"]),
            0,
            f"{tool} {argument} checked 0 manifests, so its exit code says nothing "
            f"about the rule: {rule}",
        )


if __name__ == "__main__":
    unittest.main()
