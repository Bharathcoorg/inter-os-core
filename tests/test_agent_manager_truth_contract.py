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


def _body(source: str, signature: str) -> str:
    """The braced body that follows `signature`, as far as its own braces close.

    WHY the scoping matters: several of the rules below forbid a call in one
    function and require it in another. Searching the whole file would let a
    `fs_write` in an unrelated path trip a rule about the install request, and
    would let the rule pass while the install itself did the writing.
    """
    code = _code(source)
    at = code.find(signature)
    assert at >= 0, f"the app no longer has {signature}"
    open_at = code.find("{", at)
    assert open_at >= 0, f"{signature} has no body"
    depth = 0
    for index in range(open_at, len(code)):
        char = code[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return code[open_at + 1 : index]
    raise AssertionError(f"{signature} has an unbalanced body")


def _device_file_asks(code: str) -> list:
    """The providers that answer `installs_from_device` with true.

    WHY this is a detector rather than an inline loop: the rule is that exactly
    one provider -- the one whose weights are a document on the device -- asks
    for the file move, and a rule with a count in it has to be run over a
    synthetic source to show that it can fail.
    """
    asks = []
    for opening in re.finditer(
        r"fn\s+installs_from_device\s*\(\s*&self\s*\)\s*->\s*bool\s*\{", code
    ):
        end = code.index("}", opening.end())
        if "true" not in code[opening.end() : end]:
            continue
        owner_at = max(
            code.rfind("impl Provider for ", 0, opening.start()),
            code.rfind("pub trait Provider", 0, opening.start()),
        )
        assert owner_at >= 0, "an installs_from_device sits outside a provider"
        asks.append(code[owner_at : owner_at + 80].split("\n")[0])
    return asks


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


def _cancel_body_faults(main: str) -> list:
    """Every way the cancel button still states what the platform did.

    WHY the detector reads the function's body and not the file: the fault is
    the cancel button's own sentence, and the app has other honest reasons to
    use the word "stream". Why the four arms are separate: the first two are
    the claim, and the last two are what makes the claim unfalsifiable -- a
    request whose result is discarded cannot tell a refusal from an accepted
    cancellation, so the sentence it paints is the app's guess either way.
    """
    body = _body(main, "fn cancel_streaming")
    faults = []
    if re.search(r"let\s+_\s*=\s*std::ipc_send", body):
        faults.append("the cancel request's own result is thrown away with `let _ =`")
    if re.search(
        r"cancelled by user|streaming cancelled|generation cancelled|"
        r"cancellation acknowledged",
        body,
        re.IGNORECASE,
    ):
        faults.append("the transcript states a cancellation the app never read back")
    if ".is_err()" not in body and ".is_ok()" not in body:
        faults.append("no branch reports a request the service refused")
    if not re.search(r"cancel\w*\s+request", body, re.IGNORECASE):
        faults.append("the notice does not say what the app did -- requesting a cancellation")
    return faults


def test_the_cancel_button_reports_the_request_and_not_a_result():
    """The cancel button used to paint "[Generation cancelled by user]".

    The app sent the request and read nothing back: `ipc_send`'s result was
    discarded with `let _ =`, and the service's answer -- which is
    "[Cancellation acknowledged]", "there was nothing to cancel", or "the
    sender does not own the session", depending on state only the service
    holds -- arrives on the generation message type the app reserves for the
    model's words, so the app could not have read it as a verdict even if it
    had looked. The notice now names the app's own act, and a refused send is
    reported as a refused send in the same words the file already uses.

    WHY this is a contract and not a comment: a supervisor that reports an
    outcome it did not observe is the exact fault this file exists to catch,
    and the reader of that line has no way to tell it was a guess.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        faults = _cancel_body_faults(main)
        assert not faults, f"{path}: {faults}"
        _forbids(lib + main, r"cancelled by user", "a cancellation the app observed")
        _forbids(lib + main, r"streaming cancelled", "a stream the app saw cancelled")


def test_the_app_does_not_claim_a_token_stream_the_service_does_not_keep():
    """The action band used to say "Streaming generation active..." on send.

    The service fills the whole answer onto the channel and only then posts
    the notice that names it, so there is one arrival and no per-token stream
    to describe. What the app does hold is a request it has made and an answer
    it has not yet been given, so the band now says that.

    WHY only this sentence: the transcript's own streaming state is the app's
    bookkeeping for waiting, and forbidding the word "stream" everywhere would
    forbid the app's names for its own fields rather than its claim about the
    platform.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        _forbids(lib + main, r"streaming generation active", "a per-token stream")
        body = _body(main, "fn send_prompt")
        assert re.search(r"generation (?:requested|in flight)", body, re.IGNORECASE), (
            f"{path} does not say what it actually holds when it sends a prompt"
        )


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


# The bytes a row of a wide line is guaranteed to carry.
#
# WHY a measured floor and not a guessed one: the column is six hundred and
# thirty-two pixels wide, the widest glyph the face has advances thirteen
# pixels at the caption rung, and a wrap that breaks at a space leaves the
# end of a row empty. Forty bytes a row is what is left after all three.
BYTES_PER_ROW = 40


def _service_notice_bytes() -> int:
    """The widest sentence the inference service can say about a model file."""
    source = (
        ROOT / "userspace" / "system-service" / "src" / "lib.rs"
    ).read_text(encoding="utf-8")
    widths = [
        int(match.group(1))
        for match in re.finditer(r"scratch\s*=\s*\[0u8;\s*(\d+)\]", source)
    ]
    assert widths, "the inference service builds its notices in no scratch of its own"
    return max(widths)


def _width_defects(lib: str, main: str, notice_bytes: int) -> list:
    """Every reason a message the transcript holds might never reach the screen."""
    held = _const(lib, "CHAT_TEXT_MAX")
    rows = _const(main, "WIDE_ROWS")
    code = _code(main)
    lib_code = _code(lib)
    defects = []
    if held < notice_bytes:
        defects.append(
            f"the transcript holds {held} bytes and the platform writes a {notice_bytes}-byte sentence"
        )
    if not re.search(r"const\s+WIDE_MAX\s*:\s*usize\s*=\s*CHAT_TEXT_MAX\s*;", code):
        defects.append("a wide line's buffer is its own number rather than the transcript's")
    if rows * BYTES_PER_ROW < held:
        defects.append(f"{rows} wrapped rows cannot paint the {held} bytes held")
    # WHY the counters are checked against the buffer's width and not taken on
    # trust: a byte names lengths up to two hundred and fifty-five, so widening
    # a buffer past that without widening what counts into it does not fail to
    # compile -- it reports a full message as a short one, and a row start that
    # wraps paints the third row over text the first row already carried. This
    # is the same fault as the two arms above, one layer down, and only this
    # arm can see it.
    if held > 255:
        for pattern, label, where in (
            (r"pub\s+text_len\s*:\s*u(?:16|32)", "the transcript's length field", lib_code),
            (r"pub\s+value_len\s*:\s*u(?:16|32)", "the wide line's length field", code),
            (r"pub\s+row_starts\s*:\s*\[u(?:16|32);\s*WIDE_ROWS\]", "a wide line's row starts", code),
        ):
            if not re.search(pattern, where):
                defects.append(f"{label} cannot count to the {held} bytes the buffers hold")
    return defects


def test_a_message_the_transcript_holds_reaches_the_screen():
    """The line a message is painted on carries the whole message.

    WHY this matters: the transcript holds what the platform said, and the
    body puts one message on one wide line. If that line's buffer or its
    wrapped rows hold less than the transcript does, the screen paints an
    ellipsis over the end of a sentence the app has already reported as
    received -- and the sentence cut is the platform's own explanation of the
    model file, which is built in the service's widest scratch.
    """
    notice_bytes = _service_notice_bytes()
    for path, (lib, main, _theme) in SOURCES.items():
        defects = _width_defects(lib, main, notice_bytes)
        assert not defects, f"{path}: {defects}"


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


def test_the_search_card_reads_a_frame_the_service_streamed():
    """A reply the mailbox cannot hold has to be taken off the channel.

    The kernel cuts an IPC message at sixty-four bytes and says nothing, and a
    ranked result is twenty-four bytes of record followed by a path and a
    snippet, so the retrieval frame was always the cut one: the card walked as
    many hits as the header counted, over a buffer whose tail had never
    arrived, and showed a document whose name had been clipped. The card reads
    the notice the service posts and takes the frame off the channel it names.
    An answer and a frame are different shapes, so they arrive on different
    message types and only the reader that asked for one takes the other.
    """
    for path, (lib, main, _theme) in SOURCES.items():
        source = _code(lib + main)
        assert "std::ai::MSG_TYPE_SEARCH_CHUNK" in source, (
            f"{path} ignores the notice that names a streamed retrieval frame"
        )
        assert "streamed_search_frame(" in source, (
            f"{path} never reads the frame off the channel its notice named"
        )
        assert "std::shm_pipe_receive(notice.pipe_id" in source, (
            f"{path} reads a streamed reply from somewhere other than the "
            "channel the service named"
        )


def test_an_install_is_reported_as_the_platforms_own_words():
    """INSTALL asks the inference service and paints its answer, not the app's.

    The weights this provider is served by arrive as a file the person puts on
    the device. The only authority to move that file into the model namespace
    belongs to the storage service, and the only statement anyone can make about
    the result is the inference service's measurement of the directory the file
    landed in. So the app asks, waits, and shows the sentence it was given -- as
    a notice, because a sentence shown in the assistant's voice would read as
    though the model had said it.

    The app must not write the file itself, and must not address the storage
    service directly: either one would be authority the app was never granted,
    and the screen would then be claiming a file the platform never placed.
    """
    for path, (_lib, main, _theme) in SOURCES.items():
        request = _body(main, "fn request_model_install")
        assert "std::ipc_send" in request, f"{path} does not ask the platform to install"
        assert "std::ai::SERVICE_INFERENCE_TASK_ID" in request, (
            f"{path} asks something other than the inference service"
        )
        assert "std::ai::MSG_TYPE_INSTALL" in request, (
            f"{path} asks the inference service for something other than an install"
        )
        assert "std::fs_write" not in request and "fs_write" not in request, (
            f"{path} writes a model file from the app instead of asking for it"
        )
        assert "SERVICE_STORAGE_TASK_ID" not in request, (
            f"{path} addresses the storage service directly, which is a route "
            "the platform did not open for an app"
        )

        report = _body(main, "fn take_install_report")
        assert "ChatRole::Notice" in report, (
            f"{path} does not show the install sentence as the platform's own"
        )
        assert "ChatRole::Assistant" not in report, (
            f"{path} puts the platform's install sentence in the assistant's mouth"
        )

        reader = _body(main, "fn step_install_reply")
        assert "std::ai::MSG_TYPE_INSTALL_CHUNK" in reader, (
            f"{path} ignores the notice that names a streamed install sentence"
        )
        assert "streamed_install_notice(" in reader, (
            f"{path} never takes the install sentence off the channel its "
            "notice named"
        )

        selector = _body(main, "fn install_selected")
        assert "installs_from_device()" in selector, (
            f"{path} installs every provider the same way, so a provider whose "
            "weights are a file on the device is answered with a package"
        )


def test_only_the_local_weights_provider_asks_for_a_file_move():
    """The device-file route belongs to one provider and to no other.

    A provider delivered as a package already has an install route the platform
    answers; a provider that runs on another machine has no file for this device
    to move at all. Only the weights that arrive as a document on the device ask
    for the move, so the flag has exactly one truth-teller and a default that
    refuses.
    """
    for path, (lib, _main, _theme) in SOURCES.items():
        code = _model_layer(lib)
        assert "fn installs_from_device" in code, (
            f"{path} no longer decides per provider where its weights come from"
        )
        asks = _device_file_asks(code)
        assert "false" in _body(code, "fn installs_from_device"), (
            f"{path} does not refuse a device file move by default, so a provider "
            "added without an answer would ask for one"
        )
        assert len(asks) == 1, (
            f"{path} asks for a device file move from {len(asks)} providers: {asks}"
        )
        assert "LocalGenerative" in asks[0], (
            f"{path} asks for a file move from {asks[0]}, which is not the provider "
            "whose weights are a document on the device"
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
        (r"cancelled by user", 'self.chat.push(Notice, b"[Generation cancelled by user]");'),
        (r"streaming cancelled", 'self.set_action(b"Streaming cancelled.", Tone::Plain);'),
        (
            r"streaming generation active",
            'self.set_action(b"Streaming generation active...", Tone::Accent);',
        ),
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

    # The install detectors, run over apps that carry the faults they exist to
    # catch. Each one is a whole fake source, because the detectors read a
    # function's body and a body has to be inside a source.
    faulty_sources = [
        (
            "an app that writes the model file itself",
            "fn request_model_install(&mut self) {\n"
            "    let _ = std::fs_write(\"/data/models/local_model.gguf\", b\"\");\n}\n",
            lambda source: "fs_write" in _body(source, "fn request_model_install"),
        ),
        (
            "an app that answers an install in the assistant's voice",
            "fn take_install_report(&mut self) -> bool {\n"
            "    self.chat.push(ChatRole::Assistant, text);\n    true\n}\n",
            lambda source: "ChatRole::Assistant"
            in _body(source, "fn take_install_report"),
        ),
        (
            "an app that asks the storage service past the inference service",
            "fn request_model_install(&mut self) {\n"
            "    let _ = std::ipc_send(std::ai::SERVICE_STORAGE_TASK_ID, b\"\", b\"INSL\");\n}\n",
            lambda source: "SERVICE_STORAGE_TASK_ID"
            in _body(source, "fn request_model_install"),
        ),
        (
            "an app that reads every message as an install report",
            "fn step_install_reply(&mut self) -> bool {\n"
            "    let _ = std::ipc::receive_any(&mut rx);\n    false\n}\n",
            lambda source: "MSG_TYPE_INSTALL_CHUNK"
            not in _body(source, "fn step_install_reply"),
        ),
    ]
    for label, synthetic, fired in faulty_sources:
        assert fired(synthetic), f"the install detector does not catch {label}"

    provider_flags = (
        "pub trait Provider {\n"
        "    fn installs_from_device(&self) -> bool {\n        false\n    }\n}\n"
        "impl Provider for EmbeddingGemma2 {\n"
        "    fn installs_from_device(&self) -> bool {\n        true\n    }\n}\n"
        "impl Provider for LocalGenerative {\n"
        "    fn installs_from_device(&self) -> bool {\n        true\n    }\n}\n"
    )
    synthetic_asks = _device_file_asks(provider_flags)
    assert not (len(synthetic_asks) == 1 and "LocalGenerative" in synthetic_asks[0]), (
        "the provider detector does not catch a packaged provider asking for a "
        "device file move"
    )
    assert "false" not in _body(
        "pub trait Provider {\n"
        "    fn installs_from_device(&self) -> bool {\n        true\n    }\n}\n",
        "fn installs_from_device",
    ), "the default-refuses check does not catch a trait that asks by default"

    # The width detectors, over apps that carry each fault they exist to catch.
    width_cases = [
        (
            "a transcript shorter than the sentence it is given",
            "pub const CHAT_TEXT_MAX: usize = 96;",
            "const WIDE_MAX: usize = CHAT_TEXT_MAX;\nconst WIDE_ROWS: usize = 8;",
        ),
        (
            "a wide line with a buffer of its own",
            "pub const CHAT_TEXT_MAX: usize = 240;",
            "const WIDE_MAX: usize = 156;\nconst WIDE_ROWS: usize = 8;",
        ),
        (
            "too few wrapped rows for the bytes held",
            "pub const CHAT_TEXT_MAX: usize = 240;",
            "const WIDE_MAX: usize = CHAT_TEXT_MAX;\nconst WIDE_ROWS: usize = 4;",
        ),
    ]
    for label, lib, main in width_cases:
        assert _width_defects(lib, main, 240), f"the width detector does not catch {label}"
    assert not _width_defects(
        "pub const CHAT_TEXT_MAX: usize = 240;",
        "const WIDE_MAX: usize = CHAT_TEXT_MAX;\nconst WIDE_ROWS: usize = 8;",
        240,
    ), "the width detector fires on an app that carries no fault"

    # The counters, over an app whose buffers outgrew a byte. Each arm is
    # severed on its own so the detector is shown to catch the field it names,
    # and the whole set is shown to rest on an app that widened both.
    wide_lib = (
        "pub const CHAT_TEXT_MAX: usize = 320;\n"
        "pub struct ChatMessage { pub text: [u8; CHAT_TEXT_MAX], pub text_len: u16 }\n"
    )
    wide_main = (
        "const WIDE_MAX: usize = CHAT_TEXT_MAX;\n"
        "const WIDE_ROWS: usize = 8;\n"
        "pub struct WideLine { pub value: [u8; WIDE_MAX], pub value_len: u16,"
        " pub row_starts: [u16; WIDE_ROWS] }\n"
    )
    assert not _width_defects(wide_lib, wide_main, 320), (
        "the counter detector fires on an app that widened what counts into its buffers"
    )
    counter_faults = [
        ("a byte counting the transcript", wide_lib.replace("text_len: u16", "text_len: u8"), wide_main),
        ("a byte counting the wide value", wide_lib, wide_main.replace("value_len: u16", "value_len: u8")),
        ("a byte holding a row start", wide_lib, wide_main.replace("row_starts: [u16;", "row_starts: [u8;")),
    ]
    for label, lib, main in counter_faults:
        assert _width_defects(lib, main, 320), f"the counter detector does not catch {label}"

    # The cancel detector, over an app that carries each fault it exists to
    # catch and over one that reports only the request it made.
    honest_cancel = (
        "pub fn cancel_streaming(&mut self) {\n"
        "    if std::ipc_send(id, MSG_TYPE_CANCEL, b\"CANC\").is_err() {\n"
        "        self.chat.push(ChatRole::Notice, b\"[Cancellation request not sent]\");\n"
        "        self.set_action(b\"CANCEL: the service would not take the request.\");\n"
        "        return;\n"
        "    }\n"
        "    self.chat.push(ChatRole::Notice, b\"[Cancellation requested]\");\n"
        "}\n"
    )
    assert not _cancel_body_faults(honest_cancel), (
        f"the cancel detector fires on an app that states only its own request: "
        f"{_cancel_body_faults(honest_cancel)}"
    )
    cancel_cases = [
        (
            "an app that throws the request's result away and states the outcome",
            "pub fn cancel_streaming(&mut self) {\n"
            "    let _ = std::ipc_send(id, MSG_TYPE_CANCEL, b\"CANC\");\n"
            "    self.chat.push(ChatRole::Notice, b\"[Generation cancelled by user]\");\n"
            "}\n",
        ),
        (
            "an app that throws the request's result away but keeps the request wording",
            "pub fn cancel_streaming(&mut self) {\n"
            "    let _ = std::ipc_send(id, MSG_TYPE_CANCEL, b\"CANC\");\n"
            "    self.chat.push(ChatRole::Notice, b\"[Cancellation requested]\");\n"
            "}\n",
        ),
        (
            "an app that checks the send and still paints a cancelled stream",
            "pub fn cancel_streaming(&mut self) {\n"
            "    if std::ipc_send(id, MSG_TYPE_CANCEL, b\"CANC\").is_err() {\n"
            "        return;\n"
            "    }\n"
            "    self.set_action(b\"Streaming cancelled.\");\n"
            "    self.chat.push(ChatRole::Notice, b\"[Cancellation requested]\");\n"
            "}\n",
        ),
    ]
    for label, synthetic in cancel_cases:
        assert _cancel_body_faults(synthetic), f"the cancel detector does not catch {label}"

    # The streaming claim, over an app that makes it and over one that does not.
    assert _hits(
        _code('self.set_action(b"Streaming generation active...", Tone::Accent);'),
        r"streaming generation active",
    ), "the streaming detector does not catch a per-token stream the service keeps"


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
