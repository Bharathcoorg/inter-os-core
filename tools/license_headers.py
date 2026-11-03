#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
tools/license_headers.py -- keep the AGPL notice on every first-party source file.

WHY a tool and not one commit: `LICENSE` alone does not travel with a file that is
copied out of this tree, and the AGPL's own source-notice guidance is per-file. A
mechanical sweep fixes the wording once; the checker is what stops the next hundred
files drifting, which is how this repo ended up with zero notices on 600 sources.

Usage:
    python tools/license_headers.py                     # report what is missing
    python tools/license_headers.py --apply             # add or reposition notices
    python tools/license_headers.py --check             # report, exit 1 if missing
    python tools/license_headers.py --apply --from-head # rebuild over committed bytes
    python tools/license_headers.py --coverage          # every tracked file is stamped or declared

Files the syntax cannot carry a comment in (JSON, Markdown, lockfiles, fixtures,
binary assets) are declared in REUSE.toml instead, which is what the REUSE
specification prescribes for them. Material this project does not own keeps its
own licence and is listed in SKIP_DIRS: stamping AGPL onto someone else's work
would be a false attribution, not a licence.
"""

from __future__ import annotations

import subprocess
import sys
import tomllib
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parent.parent
OWNER = "2026 Bharath B R <https://github.com/Bharathcoorg>"
LICENSE_ID = "AGPL-3.0-only"
MARKER = "SPDX-License-Identifier:"
COPYRIGHT = "SPDX-FileCopyrightText:"

# Extension -> (line prefix, block style). Manifests are deliberately absent: a
# Cargo.toml says what it is with a `license` field that tooling reads, and a
# comment above it would be read by nothing.
STYLE = {
    ".rs": ("//", None),
    ".ts": ("//", None),
    ".js": ("//", None),
    ".go": ("//", None),
    ".py": ("#", None),
    ".sh": ("#", None),
    ".ps1": ("#", None),
    ".css": (None, ("/*", "*/")),
    ".yml": ("#", None),
    ".yaml": ("#", None),
    ".html": (None, ("<!--", "-->")),
}

SKIP_DIRS = (
    ".git",
    "target",
    "node_modules",
    "dist",
    ".qoder",
    ".qoder-tmp",
    ".worktrees",
    "scratch",
    "Ui Samples",
    "LICENSES",
    # Third-party material: the Roboto Condensed face and the text of the
    # Apache-2.0 licence it ships under. Neither is ours to relicense.
    "tools/fonts",
)

# How far down a file we are willing to look for a notice to normalise. Anything
# deeper was placed deliberately and is reported instead of moved.
HEAD_WINDOW = 8


def ls_files() -> list[str]:
    out = subprocess.run(
        ["git", "-C", str(ROOT), "ls-files"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    return [line for line in out.splitlines() if line]


def tracked_paths() -> list[str]:
    """The sweep's scope: first-party sources whose syntax can hold a comment."""
    return [
        line
        for line in ls_files()
        if not any(line == d or line.startswith(d + "/") for d in SKIP_DIRS)
        and Path(line).suffix.lower() in STYLE
    ]


def all_tracked_paths() -> list[str]:
    """The checker's scope: everything committed, declared or not."""
    return ls_files()


def dirty_paths() -> set[str]:
    out = subprocess.run(
        ["git", "-C", str(ROOT), "status", "--porcelain"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    dirty = set()
    for line in out.splitlines():
        if len(line) > 3:
            dirty.add(line[3:].split(" -> ")[-1].strip('"'))
    return dirty


def git_show_head(rel: str) -> str | None:
    """The committed contents of a path, or None if it is not in HEAD."""
    proc = subprocess.run(
        ["git", "-C", str(ROOT), "show", f"HEAD:{rel}"],
        capture_output=True,
    )
    if proc.returncode != 0:
        return None
    return proc.stdout.decode("utf-8", errors="strict") if _valid(proc.stdout) else None


def _valid(raw: bytes) -> bool:
    try:
        raw.decode("utf-8")
        return True
    except UnicodeDecodeError:
        return False


def is_notice_line(line: str) -> bool:
    return MARKER in line or COPYRIGHT in line


def is_ours(line: str) -> bool:
    """Whether a notice line is the one this tool writes, and not someone else's.

    WHY check the wording and not just the tag: a file that already carries another
    author's SPDX notice is third-party material, and overwriting it with our AGPL
    line would be a false attribution rather than a fix.
    """
    if MARKER in line:
        return LICENSE_ID in line
    return COPYRIGHT in line and OWNER in line


def without_notice(text: str) -> str:
    """The file's non-notice lines, with line endings set aside.

    WHY compare this instead of asking `git status` whether a file is dirty: the
    sweep makes every file it stamps dirty, so a plain dirty check makes a re-run
    skip exactly the files it most needs to look at. If a file's only difference
    from HEAD is notice lines, that dirt is ours and re-processing is safe; if
    anything else differs, real work is in flight and the file stays untouched.

    WHY endings and blank lines are ignored here: this predicate decides whether
    other people's edits are present, and a line ending or the blank line the
    notice needs is not an edit worth protecting.
    """
    return "\n".join(
        line.rstrip("\r")
        for line in text.splitlines()
        if line.strip() and not is_notice_line(line)
    )


def header(prefix: str | None, block: tuple[str, str] | None) -> str:
    lines = [f"{MARKER} {LICENSE_ID}", f"{COPYRIGHT} {OWNER}"]
    if prefix is not None:
        return "".join(f"{prefix} {line}\n" for line in lines)
    open_, close = block
    # WHY one opener and one closer for the pair: two `<!--` lines would each open
    # a comment, and the first would only be closed by the second's terminator --
    # valid markup containing a stray `<!--`, and not what a reader means.
    return f"{open_} {'\n'.join(lines)} {close}\n"


def preamble_len(lines: list[str], suffix: str) -> int:
    """How many leading lines the syntax pins in place, so the notice follows them.

    A shebang has to stay on line 1 or the script stops being invocable, and a
    `<!DOCTYPE html>` has to precede the root element, so both are stepped over
    rather than pushed down.

    WHY `#![` is excluded: a Rust crate opens with the inner attributes `#![no_std]`
    and `#![no_main]`, which start with the same two characters as a shebang. A
    comment before an inner attribute is legal, so those crates take the notice on
    line 1 like any other source file. A comment above a Python module docstring is
    legal for the same reason -- comments are not statements, so `__doc__` survives.
    """
    if not lines:
        return 0
    if lines[0].startswith("#!") and not lines[0].startswith("#![") and suffix in (
        ".py",
        ".sh",
        ".bash",
    ):
        return 1
    if suffix == ".html" and lines[0].lstrip().lower().startswith("<!doctype"):
        return 1
    return 0


def notice_spans(lines: list[str], keep: int = 0) -> list[int]:
    """The contiguous notice block where this tool writes one, and one blank beside it.

    WHY contiguous, why short, and why it starts after the preamble: a JSDoc header
    three lines down is documentation, and a rule that keeps scanning past the first
    non-notice line would excise a closing `*/` from the middle of a comment block.
    The block ends at the first line that is neither a notice line nor a bare block
    closer, and the single blank line written under it goes with it, so a
    repositioned notice leaves the file's own spacing untouched.
    """
    window = lines[keep : keep + HEAD_WINDOW]
    offset = keep
    start = next((i for i, line in enumerate(window) if is_notice_line(line)), None)
    if start is None:
        return []
    spans = [start + offset]
    i = start + 1
    while i < len(window):
        if is_notice_line(window[i]):
            spans.append(i + offset)
        elif window[i].strip() in ("*/", "-->"):
            spans.append(i + offset)
            break
        else:
            break
        i += 1
    last = spans[-1]
    if last + 1 < len(lines) and lines[last + 1].strip() == "":
        spans.append(last + 1)
    elif start > 0 and lines[spans[0] - 1].strip() == "":
        spans.insert(0, spans[0] - 1)
    return spans


def plan(text: str, suffix: str) -> tuple[str, str]:
    """(contents, verdict) for a file, where verdict is ok, stamped, foreign or held."""
    # A byte-order mark is a token only at byte 0 -- rustc reads one anywhere else as
    # `unknown start of token`. `cap_ladder.rs` carried one on its first line and the
    # first sweep moved it into a comment, which broke `cargo test`. Never relocate or
    # duplicate one, and report the file rather than stamping a lexer error.
    if "\ufeff" in text.replace("\ufeff", "", 1):
        return text, "foreign"
    lines = text.splitlines()
    keep = preamble_len(lines, suffix)
    spans = notice_spans(lines, keep)
    if any(is_notice_line(lines[i]) and not is_ours(lines[i]) for i in spans):
        return text, "foreign"
    if not spans and any(is_notice_line(line) for line in lines[keep:]):
        return text, "foreign"
    if text == stamp(text, suffix):
        return text, "ok"
    return stamp(text, suffix), "stamped"


def stamp(text: str, suffix: str) -> str:
    """The file with our notice at the one position this tool writes it.

    WHY the line endings and the trailing newline are carried through rather than
    rewritten: this repo has no `.gitattributes` and `core.autocrlf=false`, so 56
    tracked files are stored with CRLF and the rest with LF, and some have no final
    newline at all. A sweep that normalises any of that reports every line of the
    file as changed, which buries the three lines it actually added and breaks
    `git blame` for the whole file.

    WHY a byte-order mark stays at offset 0: rustc accepts a BOM only as the first
    three bytes of a file; see `plan`, which refuses to move one.
    """
    if not text:
        return text
    bom = "\ufeff" if text.startswith("\ufeff") else ""
    body = text[len(bom) :]
    prefix, block = STYLE[suffix]
    eol = "\r\n" if body.count("\r\n") * 2 >= body.count("\n") else "\n"
    lines = body.splitlines(keepends=True)
    bare = [line.rstrip("\r\n") for line in lines]
    ends = [line[len(line.rstrip("\r\n")) :] for line in lines]
    keep = preamble_len(bare, suffix)
    drop = set(notice_spans(bare, keep))

    def render(indexes: list[int]) -> str:
        return "".join(bare[i] + ends[i] for i in indexes)

    head = render([i for i in range(keep) if i not in drop])
    body_indexes = [i for i in range(keep, len(bare)) if i not in drop]
    while body_indexes and bare[body_indexes[0]] == "":
        body_indexes.pop(0)
    if not body_indexes:
        return text
    return (
        bom
        + head
        + header(prefix, block).replace("\n", eol)
        + eol
        + render(body_indexes)
    )


def process(
    rels: list[str], dirty: set[str], apply: bool, from_head: bool = False
) -> dict[str, list[str]]:
    """Bucket every tracked source as stamped / held / foreign / ok.

    WHY `--from-head` exists: a sweep that rewrote a file's line endings leaves the
    tree looking like every file was edited, and the damaged file then reads as
    already-correct to a checker. Re-deriving from the committed contents rebuilds
    the notice on top of the bytes that are actually in the repository, so the diff
    is the notice and nothing else. Only files whose sole difference from HEAD is
    the notice qualify; anything else is somebody's work in progress and stays
    where they left it.
    """
    buckets: dict[str, list[str]] = {"stamped": [], "held": [], "foreign": [], "ok": []}
    for rel in rels:
        path = ROOT / rel
        try:
            text = path.read_text(encoding="utf-8", newline="")
        except (UnicodeDecodeError, OSError) as err:
            print(f"skipped {rel}: {err}")
            continue
        base = text
        if from_head:
            committed = git_show_head(rel)
            if committed is not None and without_notice(committed) == without_notice(text):
                base = committed
            elif committed is not None:
                buckets["held"].append(rel)
                continue
        want, verdict = plan(base, path.suffix.lower())
        if verdict != "stamped":
            buckets[verdict].append(rel)
            continue
        if not from_head and rel in dirty:
            committed = git_show_head(rel)
            if committed is not None and without_notice(committed) != without_notice(text):
                buckets["held"].append(rel)
                continue
        buckets["stamped"].append(rel)
        if apply:
            path.write_text(want, encoding="utf-8", newline="")
    return buckets


def reuse_declarations() -> list[tuple[list[str], str]]:
    """The (path globs, SPDX id) pairs REUSE.toml states."""
    with (ROOT / "REUSE.toml").open("rb") as handle:
        document = tomllib.load(handle)
    return [
        (list(annotation["path"]), annotation["SPDX-License-Identifier"])
        for annotation in document.get("annotations", ())
    ]


def coverage() -> tuple[list[str], list[str]]:
    """Every tracked file either carries the notice or is declared -- or it is neither.

    WHY this is the check and not `--check`: `--check` only sees the extensions this
    tool can comment in, so a new binary asset added next month is invisible to it
    and silently unlicensed. This walks the whole tree, and reports a file that is
    declared under one licence while carrying another's notice in it -- which is how
    stamping AGPL onto someone else's work would reach a public repository.
    """
    declarations = reuse_declarations()
    undeclared, contradicted = [], []
    for rel in all_tracked_paths():
        # A licence text is not a work of authorship of this project; the REUSE
        # specification exempts the ones a project ships, by name.
        if rel == "LICENSE" or rel.startswith("LICENSES/"):
            continue
        path = ROOT / rel
        try:
            stamped = MARKER.encode() in path.read_bytes()
        except OSError as err:
            print(f"skipped {rel}: {err}")
            continue
        ids = {
            spdx
            for globs, spdx in declarations
            if any(PurePosixPath(rel).full_match(glob) for glob in globs)
        }
        if not stamped and not ids:
            undeclared.append(rel)
        elif stamped and ids and ids != {LICENSE_ID}:
            contradicted.append(rel)
    return undeclared, contradicted


def report(label: str, rels: list[str]) -> None:
    print(f"{len(rels)} file(s) {label}")
    for rel in rels[:12]:
        print(f"  {rel}")
    if len(rels) > 12:
        print(f"  ... and {len(rels) - 12} more")


def main(argv: list[str]) -> int:
    if "--coverage" in argv:
        undeclared, contradicted = coverage()
        for label, rels in (
            ("carry no notice and are not declared in REUSE.toml", undeclared),
            ("carry a notice that contradicts their REUSE.toml declaration", contradicted),
        ):
            if rels:
                report(label, rels)
        print(
            f"coverage: {len(undeclared)} undeclared, {len(contradicted)} contradicted"
        )
        return 1 if undeclared or contradicted else 0
    apply = "--apply" in argv
    check = "--check" in argv
    from_head = "--from-head" in argv
    rels = tracked_paths()
    buckets = process(rels, dirty_paths(), apply, from_head)
    verb = "stamped" if apply else "need"
    print(
        f"{len(buckets['stamped'])} of {len(rels)} first-party source files {verb} the notice"
        f" ({len(buckets['ok'])} already carry it)"
    )
    for name, label in (
        ("held", "left alone: uncommitted work in progress that is not the notice"),
        ("foreign", "left alone: a notice that is not ours to rewrite"),
    ):
        if buckets[name]:
            report(label, buckets[name])
    if not apply:
        for rel in buckets["stamped"][:12]:
            print(f"  {rel}")
        if len(buckets["stamped"]) > 12:
            print(f"  ... and {len(buckets['stamped']) - 12} more")
    return 1 if (check and buckets["stamped"]) else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
