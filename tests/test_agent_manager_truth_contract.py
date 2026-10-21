#!/usr/bin/env python3
"""The agent manager's truth contract.

WHAT THIS FILE IS FOR: the agent manager is a supervisor. A supervisor that
prints something the device cannot substantiate is worse than no supervisor,
because the reader trusts it. Every test below names one fault that was
removed from the app and forbids it from coming back, so a regression fails
here rather than on a screen nobody is looking at.

HOW THE CONTRACT CAN FAIL: each fault has a detector, and the detectors take
a source string rather than reading the file themselves. The fault tests run
the detectors over the real source; `test_the_contract_can_fail` runs the same
detectors over a synthetic source carrying the fault and asserts that they
catch it. A contract that cannot fail is a comment.

WHERE THE SOURCE IS: the agent manager is a RAPP, so its canonical crate
lives in the private RAPP repository at `interos-apps/agent-manager`. The
public repository keeps a mirrored copy under `apps/agent_manager` so that
this contract can run without the private repository present, and so that the
packager can find the crate. When both are present they must be
byte-identical, and that is a test too.

Run it directly, or under pytest.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# The canonical crate, and its mirror.
#
# WHY the mirror's name is not the canonical one: the packager in
# `tools/interos.py` normalises an app name to underscores before it looks
# for the crate, so the mirrored RAPP has to be at `apps/agent_manager` for
# `package agent-manager` to find it.
CANONICAL = ROOT.parent / "interos-apps" / "agent-manager" / "src"
MIRRORED = ROOT / "apps" / "agent_manager" / "src"


def _read(path: Path):
    lib = path / "lib.rs"
    main = path / "main.rs"
    if not lib.is_file() or not main.is_file():
        raise AssertionError(f"the agent manager's source is missing at {path}")
    return (
        lib.read_text(encoding="utf-8"),
        main.read_text(encoding="utf-8"),
    )


def _copies():
    """Every copy of the agent manager's source that is present."""
    found = []
    for path in (CANONICAL, MIRRORED):
        if (path / "lib.rs").is_file():
            found.append(path)
    if not found:
        raise AssertionError(
            "the agent manager's source was found neither at "
            f"{CANONICAL} nor at {MIRRORED}"
        )
    return found


COPIES = _copies()
SOURCES = {path: _read(path) for path in COPIES}


def _strip_comments(source: str) -> str:
    """The source with its line comments removed.

    WHY this matters: a comment that names a removed fault is documentation,
    not a claim. The faults this contract forbids were all *rendered* -- they
    were string literals the app put on a screen -- so the contract has to
    read the code and not the prose around it. A contract that failed on a
    comment would push the next author to delete the explanation instead of
    the fault.

    WHY a hand-rolled stripper and not a parser: the only thing that has to
    be right is that a `//` inside a string literal is not a comment. A
    string literal is tracked by its unescaped double quotes, which is
    enough for Rust source and does not need to understand lifetimes.
    """
    kept = []
    for line in source.split("\n"):
        in_string = False
        cut = len(line)
        index = 0
        while index < len(line):
            char = line[index]
            if in_string and char == "\\":
                index += 2
                continue
            if char == '"':
                in_string = not in_string
            elif not in_string and char == "/" and line[index : index + 2] == "//":
                cut = index
                break
            index += 1
        kept.append(line[:cut])
    return "\n".join(kept)


def _code(source: str) -> str:
    """The code of a source, without its comments."""
    return _strip_comments(source)


def _hits(source: str, pattern: str) -> list:
    """Every match of a forbidden pattern in a source."""
    return [match.group(0) for match in re.finditer(pattern, source, re.IGNORECASE)]


def _forbids(source: str, pattern: str, label: str) -> None:
    """Raises when a source carries a forbidden claim."""
    hits = _hits(_code(source), pattern)
    assert not hits, f"the app still claims {label}: {sorted(set(hits))[:3]}"


def _const(source: str, name: str) -> int:
    """The value of a `const NAME: usize = N;` in a source."""
    match = re.search(
        rf"const\s+{re.escape(name)}\s*:\s*usize\s*=\s*(\d+)\s*;", _code(source)
    )
    assert match is not None, f"the app does not declare `const {name}: usize`"
    return int(match.group(1))


def _model_layer(lib: str) -> str:
    """The model layer without its own test module.

    WHY the tests are cut off: the test module is allowed to name the
    forbidden strings, because a test that forbids a word has to be able to
    write it down.
    """
    return _code(lib.split("#[cfg(test)]")[0])


# -----------------------------------------------------------------------------
# The faults that were removed
# -----------------------------------------------------------------------------

def test_the_app_does_not_claim_an_intermcp_interceptor():
    """The app used to describe an "InterMCP" capability interceptor.

    There is no such interceptor on the platform. The screen described a
    component that does not exist, and the description was the part of the
    app that looked most authoritative.
    """
    for path, (lib, main) in SOURCES.items():
        _forbids(lib + main, r"intermcp", "an InterMCP interceptor")


def test_the_app_does_not_claim_a_permission_modal():
    """The app used to say it dispatched a permission modal to the user.

    No such modal exists. The app has no way to raise one, so the line was
    a claim about a component the platform does not have.
    """
    for path, (lib, main) in SOURCES.items():
        _forbids(lib + main, r"permission modal", "a permission modal")
        _forbids(lib + main, r"modal dispatched", "a dispatched permission modal")


def test_the_app_does_not_claim_a_connected_host_bridge():
    """The app used to report the host bridge as "Connected".

    Nothing was connected. The platform's service registry is the thing that
    says whether a runtime is bound, and the app now asks it.
    """
    for path, (lib, main) in SOURCES.items():
        _forbids(lib + main, r"\bconnected\b", "a connected runtime")
        _forbids(lib + main, r"ring-3 ipc socket", "a Ring-3 IPC socket connection")


def test_the_catalogue_does_not_advertise_a_model_the_device_does_not_hold():
    """The catalogue used to list a Gemma 4 with a parameter count.

    A model that is not on the device is not a catalogue entry; it is a
    vendor's plan. The catalogue now shows an install state, and the state
    is a measurement.
    """
    for path, (lib, main) in SOURCES.items():
        _forbids(lib + main, r"gemma\s*4", "a Gemma 4 entry")
        _forbids(lib + main, r"\b\d+\s?[MB]\b", "a parameter count")
        _forbids(lib + main, r"q4_k_m|q8_0|\bf16\b", "a quantisation claim")


def test_the_app_does_not_claim_a_system_wide_integration():
    """The app used to carry a "SYSTEM-WIDE INTEGRATION: ACTIVE" badge.

    The launcher-to-service path, the authorisation and the retrieval are
    not wired up, so the badge asserted something no test supported. The
    app now says the opposite, in the Runtime surface.
    """
    for path, (lib, main) in SOURCES.items():
        _forbids(lib + main, r"integration[:\s]+active", "an active system-wide integration")
        _forbids(
            lib + main,
            r"system-wide integration:\s*active",
            "an active integration badge",
        )


def test_the_app_does_not_claim_a_completed_keystore_derivation():
    """The app used to say a keystore derivation had completed.

    The kernel's keystore syscall refuses every request by design, so the
    line was false twice over: nothing was derived, and the call could not
    have succeeded.
    """
    for path, (lib, main) in SOURCES.items():
        _forbids(lib + main, r"derivation complete", "a completed key derivation")
        _forbids(lib + main, r"key derivation completed", "a completed key derivation")


def test_the_app_does_not_print_a_bracket_pass_it_did_not_run():
    """The self-test used to print three literal "[PASS]" lines.

    The arithmetic those lines quoted was right, but nothing executed it.
    The rows are now the return values of the platform's own functions.
    """
    for path, (lib, main) in SOURCES.items():
        _forbids(lib + main, r"\[pass\]", "a literal [PASS]")
        _forbids(lib + main, r"\[ok\]", "a literal [OK]")


def test_the_app_does_not_invent_a_content_index():
    """The search surface used to print "0 documents (Index Empty)".

    The app had opened no index. It now asks the platform's own document
    index and intent registry, and says "REFUSED" when the platform says no.
    """
    for path, (lib, main) in SOURCES.items():
        _forbids(lib + main, r"index empty", "an invented empty index")
        _forbids(lib + main, r"partition path", "an invented partition path")


def test_the_model_layer_does_not_hardcode_a_capability_grant():
    """The permission surface used to be a local policy table.

    It was the app's own deny-by-default rule set dressed up as the
    platform's grants. The grants are now asked of the kernel by
    capability type, so the model layer must not carry a grant as a
    constant.
    """
    for path, (lib, main) in SOURCES.items():
        layer = _model_layer(lib)
        assert "cap_lookup" in layer, (
            f"{path} does not ask the kernel which capability types it holds"
        )
        assert "cap_type" in layer, (
            f"{path} does not name the capability types it asks about"
        )
        for piece in layer.split("\n"):
            assert "GRANTED" not in piece, (
                f"{path} hardcodes a capability grant: {piece.strip()!r}"
            )


def test_the_app_does_not_report_a_model_as_ready():
    """The catalogue used to show a model as "Ready" without asking.

    A readiness badge that no query produced is a decoration. The install
    state is now one of three measured answers.
    """
    for path, (lib, main) in SOURCES.items():
        _forbids(lib + main, r"\bready\b", "a ready badge")


# -----------------------------------------------------------------------------
# The shape the app has to keep
# -----------------------------------------------------------------------------

def test_the_app_is_a_single_column_portrait_layout():
    """The app used to be a four-panel desktop dashboard.

    A phone app is one column: a status bar, a body and an action bar. The
    geometry below is the geometry of a 720x1440 portrait screen, and the
    bands have to be in that order for the app to read as a phone app.
    """
    for path, (lib, main) in SOURCES.items():
        source = lib + main
        width = _const(source, "W")
        height = _const(source, "H")
        status = _const(source, "STATUS_H")
        body_top = _const(source, "BODY_TOP")
        action_top = _const(source, "ACTION_TOP")
        assert (width, height) == (720, 1440), (
            f"{path} draws on {width}x{height}, not the platform's 720x1440 portrait screen"
        )
        assert 0 < status < body_top < action_top <= height, (
            f"{path} bands are out of order: status {status}, body {body_top}, "
            f"action {action_top}, height {height}"
        )


def test_the_app_keeps_a_forty_four_pixel_touch_target():
    """A phone app's controls are fingers, not a mouse.

    The platform's minimum touch target is 44 logical pixels, and the app's
    segmented control and action buttons have to clear it.
    """
    for path, (lib, main) in SOURCES.items():
        source = lib + main
        tab = _const(source, "TAB_H")
        button = _const(source, "BUTTON_H")
        assert tab >= 44, f"{path} uses a {tab}px tab, under the platform's 44px minimum"
        assert button >= 44, (
            f"{path} uses a {button}px button, under the platform's 44px minimum"
        )


def test_the_app_uses_the_platforms_design_system():
    """The app used to carry its own hex colours and its own spacing.

    The design system is the platform's, and a second one inside an app is
    how the platform's surfaces stop matching each other. No hex literal
    belongs in the app.
    """
    for path, (lib, main) in SOURCES.items():
        source = _code(lib + main)
        assert "design::theme" in source, f"{path} does not use the platform's theme"
        assert "design::components" in source, (
            f"{path} does not use the platform's components"
        )
        assert "Theme::NEUTRAL" in source, f"{path} does not use the neutral theme"
        literals = _hits(source, r"0x[0-9A-Fa-f]{6,8}\b")
        assert not literals, (
            f"{path} carries its own colour literals: {sorted(set(literals))[:5]}"
        )


def test_the_app_draws_five_surfaces_behind_one_tab_row():
    """The five surfaces the supervisor was asked for have to be there.

    Model management, permissions, runtime status, search configuration and
    a test interface, reachable from one tab row.
    """
    for path, (lib, main) in SOURCES.items():
        source = _code(lib + main)
        for name in ("Models", "Access", "Runtime", "Search", "Test"):
            assert name in source, f"{path} has no {name} surface"
        assert re.search(r"const\s+COUNT\s*:\s*usize\s*=\s*5\s*;", source), (
            f"{path} does not have five surfaces"
        )


def test_the_provider_is_a_trait_and_not_a_table():
    """A new model must be a new implementation, not an edit through the app.

    The catalogue is a registry of `dyn Provider`, so adding a model means
    writing one implementation and adding one registry entry.
    """
    for path, (lib, main) in SOURCES.items():
        source = _code(lib + main)
        assert "pub trait Provider" in source, f"{path} has no Provider trait"
        assert "dyn Provider" in source, (
            f"{path} does not register providers as a trait object"
        )
        assert "fn install_state(&self, device: &DeviceState)" in source, (
            f"{path} does not derive a provider's install state from the device"
        )


def test_the_self_test_runs_the_platforms_own_function():
    """The self-test has to call the platform, not print a verdict.

    The platform's fixed-point cosine is the function the supervisor's own
    arithmetic was quoting, and a failing row has to say FAIL.
    """
    for path, (lib, main) in SOURCES.items():
        source = _code(lib + main)
        assert "cosine_similarity_q7" in source, (
            f"{path} does not run the platform's cosine function"
        )
        assert "FAIL" in source, f"{path} cannot print a failing row"


def test_the_app_says_the_launcher_path_is_not_wired_up():
    """The app must not leave the integration question unanswered.

    The launcher-to-service path, the authorisation and the retrieval are
    not wired up, so the Runtime surface has to say so rather than leave a
    reader to infer it from an absence.
    """
    for path, (lib, main) in SOURCES.items():
        assert "launcher-to-service" in _code(lib + main), (
            f"{path} does not report the state of the launcher-to-service path"
        )


def test_the_mirrored_crate_is_the_canonical_crate():
    """The public repository's copy has to be the private one.

    A contract that reads a stale copy tests nothing, so when both copies
    are present they must be the same bytes.
    """
    if CANONICAL.is_dir() and MIRRORED.is_dir():
        for name in ("lib.rs", "main.rs"):
            a = (CANONICAL / name).read_bytes()
            b = (MIRRORED / name).read_bytes()
            assert a == b, (
                f"the mirrored {name} is not the canonical one; "
                "run tools/mirror_apps.py in interos-apps"
            )


# -----------------------------------------------------------------------------
# The contract has to be able to fail
# -----------------------------------------------------------------------------

def test_the_contract_can_fail():
    """A contract that cannot fail is a comment.

    Every detector is run here against a source that carries the fault it
    exists to catch, and the detector has to catch it. The same detectors
    are then run over the app as it stands, and none of them may fire.
    """
    cases = [
        (r"intermcp", 'let label = "INTERMCP BROKER";'),
        (r"permission modal", 'let label = "permission modal dispatched";'),
        (r"\bconnected\b", 'let label = "Host bridge CONNECTED";'),
        (r"gemma\s*4", 'let label = "Gemma 4 2B";'),
        (r"integration[:\s]+active", 'let label = "SYSTEM-WIDE INTEGRATION: ACTIVE";'),
        (r"derivation complete", 'let label = "key derivation completed";'),
        (r"\[pass\]", 'let label = "cosine [PASS]";'),
        (r"index empty", 'let label = "0 documents (Index Empty)";'),
        (r"\bready\b", 'let label = "EmbeddingGemma 2 (740M) -- Ready";'),
    ]
    for pattern, synthetic in cases:
        assert _hits(synthetic, pattern), (
            f"the detector for {pattern!r} does not catch {synthetic!r}"
        )

    for path, (lib, main) in SOURCES.items():
        for pattern, _ in cases:
            assert not _hits(_code(lib + main), pattern), (
                f"{path} trips the detector for {pattern!r}"
            )


def main() -> int:
    tests = [value for name, value in sorted(globals().items()) if name.startswith("test_")]
    failures = 0
    for test in tests:
        try:
            test()
        except AssertionError as error:
            failures += 1
            print(f"FAIL {test.__name__}: {error}")
        except Exception as error:  # noqa: BLE001 - the report must name it
            failures += 1
            print(f"ERROR {test.__name__}: {error}")
        else:
            print(f"ok   {test.__name__}")
    print(f"{len(tests) - failures} passed; {failures} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
