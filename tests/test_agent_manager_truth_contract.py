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
    theme = path / "theme.rs"
    if not lib.is_file() or not main.is_file():
        raise AssertionError(f"the agent manager's source is missing at {path}")
    return (
        lib.read_text(encoding="utf-8"),
        main.read_text(encoding="utf-8"),
        theme.read_text(encoding="utf-8") if theme.is_file() else "",
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
    for path, (lib, main, _theme) in SOURCES.items():
        _forbids(lib + main, r"intermcp", "an InterMCP interceptor")


def test_the_app_does_not_claim_a_permission_modal():
    """The app used to say it dispatched a permission modal to the user.

    No such modal exists. The app has no way to raise one, so the line was
    a claim about a component the platform does not have.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        _forbids(lib + main, r"permission modal", "a permission modal")
        _forbids(lib + main, r"modal dispatched", "a dispatched permission modal")


def test_the_app_does_not_claim_a_connected_host_bridge():
    """The app used to report the host bridge as "Connected".

    Nothing was connected. The platform's service registry is the thing that
    says whether a runtime is bound, and the app now asks it.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        _forbids(lib + main, r"\bconnected\b", "a connected runtime")
        _forbids(lib + main, r"ring-3 ipc socket", "a Ring-3 IPC socket connection")


def test_the_catalogue_does_not_advertise_a_model_the_device_does_not_hold():
    """The catalogue used to list a Gemma 4 with a parameter count.

    A model that is not on the device is not a catalogue entry; it is a
    vendor's plan. The catalogue now shows an install state, and the state
    is a measurement.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        _forbids(lib + main, r"gemma\s*4", "a Gemma 4 entry")
        _forbids(lib + main, r"\b\d+\s?[MB]\b", "a parameter count")
        _forbids(lib + main, r"q4_k_m|q8_0|\bf16\b", "a quantisation claim")


def test_the_app_does_not_claim_a_system_wide_integration():
    """The app used to carry a "SYSTEM-WIDE INTEGRATION: ACTIVE" badge.

    The launcher-to-service path, the authorisation and the retrieval are
    not wired up, so the badge asserted something no test supported. The
    app now says the opposite, in the Runtime surface.
    """
    for path, (lib, main, _theme) in SOURCES.items():
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
    for path, (lib, main, _theme) in SOURCES.items():
        _forbids(lib + main, r"derivation complete", "a completed key derivation")
        _forbids(lib + main, r"key derivation completed", "a completed key derivation")


def test_the_app_does_not_print_a_bracket_pass_it_did_not_run():
    """The self-test used to print three literal "[PASS]" lines.

    The arithmetic those lines quoted was right, but nothing executed it.
    The rows are now the return values of the platform's own functions.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        _forbids(lib + main, r"\[pass\]", "a literal [PASS]")
        _forbids(lib + main, r"\[ok\]", "a literal [OK]")


def test_the_app_does_not_invent_a_content_index():
    """The search surface used to print "0 documents (Index Empty)".

    The app had opened no index. It now asks the platform's own document
    index and intent registry, and says "REFUSED" when the platform says no.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        _forbids(lib + main, r"index empty", "an invented empty index")
        _forbids(lib + main, r"partition path", "an invented partition path")


def test_the_model_layer_does_not_hardcode_a_capability_grant():
    """The permission surface used to be a local policy table.

    It was the app's own deny-by-default rule set dressed up as the
    platform's grants. The grants are now asked of the kernel by
    capability type, so the model layer must not carry a grant as a
    constant.
    """
    for path, (lib, main, _theme) in SOURCES.items():
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
    for path, (lib, main, _theme) in SOURCES.items():
        _forbids(lib + main, r"\bready\b", "a ready badge")


# -----------------------------------------------------------------------------
# The shape the app has to keep
# -----------------------------------------------------------------------------

def test_the_app_is_a_single_column_portrait_layout():
    """The app used to be a four-panel desktop dashboard.

    A phone app is one column: an app bar, a body and an action bar. The
    geometry below is the geometry of a 720x1440 portrait screen, and the
    bands have to be in that order for the app to read as a phone app.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        source = lib + main
        width = _const(source, "W")
        height = _const(source, "H")
        status = _const(source, "APPBAR_H")
        body_top = _const(source, "BODY_TOP")
        action_top = _const(source, "ACTION_TOP")
        assert (width, height) == (720, 1440), (
            f"{path} draws on {width}x{height}, not the platform's 720x1440 portrait screen"
        )
        assert 0 < status < body_top < action_top <= height, (
            f"{path} bands are out of order: app bar {status}, body {body_top}, "
            f"action {action_top}, height {height}"
        )


def test_the_app_keeps_a_forty_four_pixel_touch_target():
    """A phone app's controls are fingers, not a mouse.

    The platform's minimum touch target is 44 logical pixels, and the app's
    segmented control and action buttons have to clear it.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        source = lib + main
        tab = _const(source, "TAB_H")
        button = _const(source, "BUTTON_H")
        assert tab >= 44, f"{path} uses a {tab}px tab, under the platform's 44px minimum"
        assert button >= 44, (
            f"{path} uses a {button}px button, under the platform's 44px minimum"
        )


def test_the_app_uses_the_platforms_design_system():
    """The app's colours live in one place, and the roles are the platform's.

    The design system is the platform's, and a second one inside an app is
    how the platform's surfaces stop matching each other. The app has a
    theme of its own -- a supervisor is read, and a light surface with
    black ink reads better than a dark instrument panel -- but the theme
    is built from the platform's roles, scales, radii and spacing, and
    every colour the app uses is named there rather than written at the
    point of use.
    """
    for path, (lib, main, theme) in SOURCES.items():
        source = _code(lib + main)
        assert "design::theme" in source or "design::theme" in _code(theme), (
            f"{path} does not use the platform's theme roles"
        )
        # WHY either module is the platform's system: `components` holds
        # the treatments the shell draws and `monochrome` holds the palette,
        # the type ladder and the icons the app showcases describe. An app
        # that uses one of them is using the platform's design system, and
        # an app that uses neither is drawing its own.
        assert "design::components" in source or "design::monochrome" in source, (
            f"{path} does not use the platform's design system"
        )
        # The app's own theme, and it is the platform's roles with the
        # app's own values.
        assert theme, f"{path} has no theme module"
        themed = _code(theme)
        assert "Theme {" in themed, f"{path}'s theme is not a theme"
        for role in ("surface", "on_surface", "on_surface_variant", "border"):
            assert f"{role}:" in themed, f"{path}'s theme does not name {role}"
        for scale in ("TypeScale", "Spacing", "Radii", "Touch"):
            assert scale in themed, f"{path}'s theme does not use the platform's {scale}"
        # The status colours, and only the status colours.
        for status in ("success", "warning", "error"):
            assert f"{status}:" in themed, f"{path}'s theme does not name {status}"
        # No colour is written at the point of use: the theme is the only
        # place a colour is allowed to be a number.
        literals = _hits(source, r"0x[0-9A-Fa-f]{6,8}\b")
        assert not literals, (
            f"{path} carries its own colour literals: {sorted(set(literals))[:5]}"
        )


def test_the_app_draws_six_surfaces_behind_one_tab_row():
    """The surfaces the supervisor was asked for have to be there.

    Model management, permissions, runtime status, search configuration and
    a test interface, plus the chat the assistant is reached through, all
    reachable from one tab row.

    WHY the count is six: the chat surface joined the row, and the row's
    segment width is derived from the count. A count written down in two
    places is a count that can disagree with itself.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        source = _code(lib + main)
        for name in ("Models", "Access", "Runtime", "Search", "Test", "Chat"):
            assert name in source, f"{path} has no {name} surface"
        assert re.search(r"const\s+COUNT\s*:\s*usize\s*=\s*6\s*;", source), (
            f"{path} does not have six surfaces"
        )


def test_the_tab_row_derives_its_geometry_from_the_surface_count():
    """The last tab in the row has to land inside the content column.

    WHY this matters: the segment width was written with a fixed gap
    count of four, which is right for a row of five segments and wrong
    for a row of six. The sixth tab ran past the content column, and the
    row's hit test and its drawing disagreed about where it was.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        source = _code(lib + main)
        assert not re.search(r"\(\s*CONTENT_W\s*-\s*4\s*\*\s*TAB_GAP\s*\)", source), (
            f"{path} still writes the tab row's gap count down"
        )
        assert re.search(
            r"\(\s*Surface::COUNT\s*-\s*1\s*\)\s*\*\s*TAB_GAP", source
        ), f"{path} does not derive the tab row's geometry from the surface count"


def test_the_chat_never_invents_a_reply():
    """The chat must not produce an answer no model gave.

    WHY this matters: a chat surface that produced a plausible sentence
    when no model was present would be the worst lie in the app, because
    the user would believe it. The app has to answer with the platform's
    own reason.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        source = _code(lib + main)
        # The chat pushes the platform's own reason into the transcript.
        assert "availability.reason()" in source, (
            f"{path} does not put the platform's reason into the transcript"
        )
        # There is no canned answer anywhere in the chat's path.
        _forbids(
            source,
            r"As an AI language model|I'm sorry, I can't|I cannot answer|"
            r"Here is (?:a|the) (?:possible|sample) (?:answer|response)|"
            r"(?:sample|dummy|mock|fake|placeholder) (?:answer|reply|response)",
            "a fabricated answer",
        )


def test_the_chat_answers_from_the_platforms_service_registry():
    """The chat's availability comes from a measurement, not a constant.

    WHY this matters: the catalogue once listed a host bridge and a model
    the device did not have. The chat has to ask the platform's own
    service registry, because that is the only thing that knows which
    services are running.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        source = _code(lib + main)
        assert "resolve_inference_service" in source, (
            f"{path} does not resolve the inference service"
        )
        assert "service_list" in lib or "service_list" in main, (
            f"{path} does not read the platform's service registry"
        )
        _forbids(
            source,
            r"fn\s+resolve_inference_service\s*\([^)]*\)\s*->\s*InferenceAvailability\s*\{\s*"
            r"InferenceAvailability::Available",
            "a hardcoded availability",
        )


def test_the_chat_checks_the_service_version():
    """A service that speaks another version is reported, not used.

    WHY this matters: the platform's service registry carries an ABI
    version per service precisely so a client can tell that the contract
    changed. A request sent to a service that speaks another version
    would be misread rather than refused.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        source = _code(lib + main)
        assert "INFERENCE_ABI_VERSION" in source, (
            f"{path} does not check the inference service's version"
        )
        assert "WrongVersion" in source, (
            f"{path} does not report a version it does not speak"
        )
        assert "abi_version" in source, (
            f"{path} does not read the service's ABI version"
        )


def test_a_registry_that_could_not_be_read_is_not_the_same_as_no_service():
    """Silence is not a fact.

    WHY this matters: a registry the platform refused to answer has not
    said that there is no inference service. It has said nothing, and the
    chat must not report silence as a measurement.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        source = _code(lib + main)
        assert "RegistryUnavailable" in source, (
            f"{path} does not distinguish a registry it could not read"
        )
        assert "ProbeStatus::Listed" in source, (
            f"{path} does not check the registry's own answer"
        )


def test_the_transcript_and_the_prompt_are_bounded():
    """Neither the conversation nor the prompt can grow without bound.

    WHY this matters: the app is `no_std` with no allocator, so an
    unbounded buffer is not a memory problem, it is a build that does not
    compile or a screen that stops rendering. A composer that silently
    drops the last character typed is worse: it lies about what was
    written.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        source = _code(lib + main)
        for name in ("CHAT_MAX_MESSAGES", "CHAT_TEXT_MAX", "CHAT_PROMPT_MAX"):
            assert re.search(
                rf"pub\s+const\s+{name}\s*:\s*usize\s*=\s*\d+\s*;", source
            ), f"{path} does not bound {name}"
        assert re.search(r"fn\s+push_prompt_char\([^)]*\)\s*->\s*bool", source), (
            f"{path}'s composer does not report a refused character"
        )
        assert re.search(
            r"fn\s+push\(&mut self,\s*role:\s*ChatRole", source
        ), f"{path} does not have a bounded transcript"
        # A refused character is reported, not dropped: the composer's
        # return value is what tells the caller the character was refused.
        assert re.search(
            r"if\s+app\.chat\.push_prompt_char\(", source
        ) or re.search(
            r"if\s+chat\.push_prompt_char\(", source
        ), f"{path} ignores the composer's refusal"


def test_the_sdk_defines_a_versioned_inference_interface():
    """The interface the chat speaks is named and versioned in the SDK.

    WHY this matters: the platform's rules keep inference runtimes out of
    the kernel and behind a narrow, versioned service interface. An
    interface that only the app knows about is not an interface, and a
    version that only the app knows about cannot be checked.
    """
    ai = (
        ROOT / "sdk" / "rust" / "interos-std" / "src" / "ai.rs"
    ).read_text(encoding="utf-8")
    assert re.search(
        r"pub\s+const\s+INFERENCE_ABI_VERSION\s*:\s*u16\s*=\s*\d+\s*;", ai
    ), "the SDK does not version the inference service interface"
    assert re.search(
        r"pub\s+const\s+SERVICE_INFERENCE\s*:\s*u8\s*=\s*\d+\s*;", ai
    ), "the SDK does not name the inference service's id"


def test_the_chat_is_reachable_and_answers_the_platforms_reason():
    """The chat is a tab, and pressing send reports what the platform said.

    WHY this matters: a surface that exists but is not in the tab row is
    not a surface, and a send button that does nothing is a dead
    affordance.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        source = _code(lib + main)
        assert "Surface::Chat" in source, f"{path} does not reach the chat"
        assert "fn send_prompt" in source, f"{path} has no way to send a prompt"
        assert "Action::Send" in source, f"{path} has no send control"
        assert "ChatRole::Notice" in source, (
            f"{path} does not record the platform's answer in the transcript"
        )


def test_the_provider_is_a_trait_and_not_a_table():
    """A new model must be a new implementation, not an edit through the app.

    The catalogue is a registry of `dyn Provider`, so adding a model means
    writing one implementation and adding one registry entry.
    """
    for path, (lib, main, _theme) in SOURCES.items():
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
    for path, (lib, main, _theme) in SOURCES.items():
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
    for path, (lib, main, _theme) in SOURCES.items():
        assert "launcher-to-service" in _code(lib + main), (
            f"{path} does not report the state of the launcher-to-service path"
        )


def test_the_floating_button_does_not_sit_on_a_value():
    """A button that covers a value hides the value.

    The agent manager's floating button floats over the bottom-right of the
    body, so the body has to reserve a band under the last card for that card
    to be scrolled into. Without the band the button lands on whatever value
    happens to be at the bottom-right, and a value under a button is a value
    nobody can read.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        source = lib + main
        radius = _const(source, "FAB_R")
        margin = _const(source, "FAB_MARGIN")
        assert radius > 0, f"{path} has no floating button to reserve room for"
        assert margin >= 0, f"{path} has a negative floating button margin"
        assert "FAB_R * 2 + FAB_MARGIN" in _code(source), (
            f"{path} does not reserve a band under the last card for the "
            "floating button"
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

    for path, (lib, main, _theme) in SOURCES.items():
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
