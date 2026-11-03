#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
tools/license_manifests.py -- keep one licence in every dependency manifest.

WHY manifests and not only source headers: `license_headers.py` puts the notice on
the file, but a person, a registry and a scanner reach `Cargo.toml` and
`package.json` first. Those fields are machine-read, and while they disagreed with
the source notice the tree was honestly self-contradictory: AGPL text in `LICENSE`,
ISC in one manifest, Apache-2.0 in another. Correcting the headers and leaving
these would have published that contradiction.

Usage:
    python tools/license_manifests.py            # report
    python tools/license_manifests.py --apply    # rewrite the licence fields
    python tools/license_manifests.py --check    # report, exit 1 if any disagree

A lockfile's per-dependency entries record licences this project does not speak
for and are never touched; only the root entry npm mirrors out of `package.json`
follows the change. A manifest with no licence field at all is reported rather
than inserted: where the field goes among a hand-written manifest's own metadata
is its author's business, not a tool's.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LICENSE_ID = "AGPL-3.0-only"
NAMES = ("Cargo.toml", "package.json", "package-lock.json")

CARGO_MEMBER = re.compile(r"\s*(?:license|license-file)\s*=")
LICENSED = re.compile(r'"license"[ \t]*:[ \t]*"[^"]*"')


def tracked() -> list[Path]:
    out = subprocess.run(
        ["git", "-C", str(ROOT), "ls-files"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.splitlines()
    return [
        ROOT / line
        for line in out
        if line.rsplit("/", 1)[-1] in NAMES
        and not {"target", "node_modules"} & set(Path(line).parts)
    ]


def read(path: Path) -> str:
    return path.read_bytes().decode("utf-8")


def terminator(line: str) -> str:
    """The bytes that end a line, so a new line copies its neighbour's.

    WHY per line and not per file: 15 tracked sources mix LF and CRLF, and a
    sweep that assumed one ending per file rewrote every line it touched --
    a licence change that reads as a whole-file diff is a licence change nobody
    can review.
    """
    return line[len(line.rstrip("\r\n")) :]


def cargo_plan(text: str) -> tuple[str | None, str]:
    """(replacement, verdict) for one Cargo manifest.

    A virtual manifest has no `[package]`, so there is no field for it to carry;
    the `license` field belongs to the packages, and `REUSE.toml` speaks for the
    root file itself. The field goes first in the table, where `name` and
    `version` already keep their company.
    """
    lines = text.splitlines(keepends=True)
    opening = next((i for i, line in enumerate(lines) if line.strip() == "[package]"), None)
    if opening is None:
        return None, "virtual"
    closing = next(
        (i for i, line in enumerate(lines[opening + 1 :], opening + 1) if line.lstrip().startswith("[")),
        len(lines),
    )
    existing = next(
        (i for i in range(opening + 1, closing) if CARGO_MEMBER.match(lines[i])),
        None,
    )
    want = f'license = "{LICENSE_ID}"'
    if existing is None:
        return "".join(lines[: opening + 1] + [want + terminator(lines[opening])] + lines[opening + 1 :]), "added"
    if lines[existing].strip() == want:
        return None, "ok"
    return "".join(lines[:existing] + [want + terminator(lines[existing])] + lines[existing + 1 :]), "changed"


def json_plan(text: str) -> tuple[str | None, str]:
    """Rewrite only the root package's `license` member.

    WHY the search starts at `"packages": { "": {`: that first block is the
    mirrored root manifest, and every entry after it is somebody else's package
    under somebody else's licence.
    """
    after = text.index('""', text.index('"packages"')) if '"packages"' in text else 0
    field = LICENSED.search(text, after)
    if field is None:
        return None, "absent"
    if f'"{LICENSE_ID}"' in field.group():
        return None, "ok"
    key = re.match(r'"license"[ \t]*:', field.group()).group()
    return text[: field.start()] + f'{key} "{LICENSE_ID}"' + text[field.end() :], "changed"


def plan(path: Path) -> tuple[str | None, str]:
    name = path.name
    text = read(path)
    if name == "Cargo.toml":
        return cargo_plan(text)
    if name == "package.json":
        json.loads(text)  # a manifest we cannot parse is a manifest we must not rewrite
    return json_plan(text)


def main(argv: list[str]) -> int:
    apply = "--apply" in argv
    check = "--check" in argv
    paths = tracked()
    counts: dict[str, int] = {}
    for path in paths:
        replacement, verdict = plan(path)
        counts[verdict] = counts.get(verdict, 0) + 1
        if replacement is None:
            continue
        rel = path.relative_to(ROOT).as_posix()
        print(f"  {rel}: {verdict}" if not check else f"  {rel}: not {LICENSE_ID}")
        if apply:
            path.write_bytes(replacement.encode("utf-8"))
    print(f"{len(paths)} manifests checked: " + ", ".join(f"{n} {v}" for v, n in sorted(counts.items())))
    return 1 if check and any(v in counts for v in ("added", "changed", "absent")) else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
