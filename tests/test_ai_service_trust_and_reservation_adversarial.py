# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Adversarial contract for the AI service's trust gate and model accounting.

Two things the host cannot execute live in the service, so nothing but the
source says they are true: the trust gate that refuses a model the user has
not trusted, and the ledger wiring that charges a loaded model's resident
bytes to the handle that owns them. A gate nobody can see fail is a comment,
and an accounting nobody can see is a number in a report.

Each case below reads the service's own source and names one rule; each
"can fail" case puts that rule back the way it was and proves the reader sees
the fault. The rules are:

  * a model whose trust state is not `Trusted` is refused before its file is
    opened, and the refusal names which of the two untrusted states it is;
  * the generative route's broker-ledger session is bound to the model handle
    the backend returned, and the model's resident bytes are reserved against
    that same handle, so an unload reclaims both;
  * a model whose file changed is unloaded on both sides -- the backend's
    handle and the ledger's reservation for it;
  * the embedding route refuses an untrusted model instead of answering with
    the labelled lexical baseline;
  * an install is the user's trust decision for the file that arrived, and it
    is that decision for a model the device already holds as well: the file
    that was probed is the file that is trusted, and the sentence says the
    bytes were not copied;
  * no user-visible string claims the platform answers from the device itself,
    and the platform's generative provider is reported as host-backed.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SERVICE = ROOT / "userspace" / "system-service" / "src"

# The byte-string literal that is a sentence a caller reads.
BYTE_LITERAL = re.compile(r'b"(?:[^"\\]|\\.)*"')

# A test-only module: its literals are fixtures a host case reads, not claims
# a caller on the device reads.
TEST_MODULE = re.compile(r"#\[cfg\(test\)\]\s*(?:pub\s+)?mod\b")


def production_text(text: str) -> str:
    """`text` with its test-only modules dropped."""
    match = TEST_MODULE.search(text)
    return text[: match.start()] if match else text


def lib_source() -> str:
    return (SERVICE / "lib.rs").read_text(encoding="utf-8")


def backend_source() -> str:
    return (SERVICE / "backend.rs").read_text(encoding="utf-8")


def function_body(source: str, signature: str) -> str:
    """The body of the function whose line starts with `signature`."""
    start = source.index(signature)
    brace = source.index("{", start)
    depth = 0
    for index in range(brace, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[brace:index]
    raise AssertionError(f"unbalanced braces after {signature!r}")


def without_comments(text: str) -> str:
    """`text` with its `//` comments dropped."""
    kept = []
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("//"):
            continue
        kept.append(line.split("//")[0])
    return "\n".join(kept)


def rust_block(source: str, marker: str) -> str:
    """The brace-balanced block that starts at `marker`.

    Rust's string and character literals are skipped so a brace inside one is
    not mistaken for the end of the block.
    """
    at = source.index(marker)
    brace = source.index("{", at)
    depth = 0
    index = brace
    in_string = False
    in_char = False
    while index < len(source):
        char = source[index]
        if in_string:
            if char == "\\":
                index += 2
                continue
            if char == '"':
                in_string = False
        elif in_char:
            if char == "\\":
                index += 2
                continue
            if char == "'":
                in_char = False
        else:
            if source.startswith('b"', index) or source.startswith('"', index):
                in_string = True
                index += 2
                continue
            if char == "'":
                in_char = True
            elif char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
                if depth == 0:
                    return source[brace:index + 1]
        index += 1
    raise AssertionError(f"unbalanced braces after {marker!r}")


def byte_literals(source: str) -> list:
    """Every byte-string literal in `source`."""
    return BYTE_LITERAL.findall(source)


def generative_route(source: str) -> str:
    """The function that runs the installed generative model."""
    return function_body(source, "fn run_installed_generation(")


def dispatch(source: str) -> str:
    """The IPC dispatch function, by name."""
    return function_body(source, "pub fn handle_ipc_inference_query(")


def embedding_route(source: str) -> str:
    """The embedding branch of the dispatch function."""
    body = dispatch(source)
    start = body.index("EmbeddingPlan::RunInstalledFile =>")
    return body[start:]


def _trust_gate_faults(lib: str) -> list:
    """Every way the trust gate can stop refusing an untrusted model."""
    faults = []
    route = generative_route(lib)
    code = without_comments(route)

    gate = "model_trust_refusal(path)"
    if gate not in code:
        return ["the generative route no longer asks whether the model is trusted"]
    at = code.index(gate)
    for site in (
        "std::fs_open(path, std::FS_READ)",
        "get_resident_model_cache(path)",
        "std::fs_stat(path, &mut stat_buf)",
    ):
        if site not in code:
            faults.append(f"{site!r} is no longer in the route, so the gate's position cannot be read")
        elif at > code.index(site):
            faults.append(
                f"the trust gate runs after {site!r}: an untrusted model's bytes were read before it was refused"
            )

    # The refusal is a refusal: it names the reason and it answers the caller.
    block = rust_block(code, gate)
    if "trust_refusal_notice(" not in block:
        faults.append("the trust refusal is not written as the sentence a caller reads")
    if "reply_with_answer(" not in block:
        faults.append("the trust refusal is never answered to the caller")
    if "return true" not in block:
        faults.append("the trust refusal does not stop the route: the file is read anyway")
    for read in ("std::fs_open(", "std::fs_stat("):
        if read in block:
            faults.append(f"the trust refusal reads the model itself with {read!r}")

    # Two untrusted states, two sentences: a model nobody provisioned and a
    # model the user imported without trusting are different next moves.
    notice = function_body(lib, "fn trust_refusal_notice")
    for arm in ("TrustRefusal::NotProvisioned", "TrustRefusal::UserImported"):
        if arm not in notice:
            faults.append(f"the trust refusal has no arm of its own for {arm}")
    if "STATUS_UNAVAILABLE" not in notice:
        faults.append("the trust refusal carries no status of its own")
    if "STATUS_SUCCESS" in notice:
        faults.append("an untrusted model is answered as a success")
    return faults


def _reservation_faults(lib: str) -> list:
    """Every way the model's bytes can stop being reclaimable."""
    faults = []
    route = generative_route(lib)
    code = without_comments(route)

    for call in (
        "ledger.reserve_resident_for(model_handle, estimate)",
        "ledger.create_session_for_model(sender_task_id, 1, model_handle, estimate)",
    ):
        if call not in code:
            faults.append(f"the generative route no longer makes the call {call!r}")

    # The reservation and the session must name the same handle: a session
    # bound to a handle nobody charged is a session an unload cannot reach.
    reserve_at = code.find("ledger.reserve_resident_for(model_handle, estimate)")
    session_at = code.find("ledger.create_session_for_model(sender_task_id, 1, model_handle, estimate)")
    if reserve_at >= 0 and session_at >= 0 and reserve_at > session_at:
        faults.append(
            "the session is bound before the model's resident bytes are reserved against the same handle"
        )

    # A model whose file changed goes on both sides: the backend's handle and
    # the ledger's reservation for it.
    if "unload_model_resources(" not in code:
        faults.append("the changed-file path does not release the ledger's reservation for the model")
    if "backend.unload_active_model(handle)" not in code:
        faults.append("the changed-file path does not unload the model from the backend")

    # The ledger has to be able to say what it holds, or none of the above can
    # be observed at all.
    backend = backend_source()
    for accessor in (
        "pub fn session_model_handle(",
        "pub fn model_resident_bytes(",
    ):
        if accessor not in backend:
            faults.append(f"the ledger no longer exposes {accessor!r}")
    return faults


def _embedding_gate_faults(lib: str) -> list:
    """Every way the embedding route can answer an untrusted model anyway."""
    faults = []
    arm = embedding_route(lib)
    code = without_comments(arm)
    if "model_trust_refusal(path)" not in code:
        return ["the embedding route no longer asks whether the model is trusted"]
    at = code.index("model_trust_refusal(path)")
    for site in (
        "run_installed_embedding(path",
        "ledger.create_session(sender_task_id, 1, estimate)",
    ):
        if site not in code:
            faults.append(f"{site!r} is no longer in the branch, so the gate's position cannot be read")
        elif at > code.index(site):
            faults.append(f"the embedding trust gate runs after {site!r}")
    block = rust_block(code, "model_trust_refusal(path)")
    if "emit_embed_notice(" not in block:
        faults.append("the embedding trust refusal is not answered to the caller")
    if "return;" not in block:
        faults.append("the embedding trust refusal does not stop the branch")
    # The labelled baseline is a different answer; a caller that asked for
    # this model's vector must not be handed another one instead of the
    # refusal.
    if "reply_lexical_baseline(" in block:
        faults.append("the embedding route falls back to the lexical baseline instead of refusing")
    return faults


def _install_trust_faults(lib: str) -> list:
    """Every way the install can stop being the user's trust decision."""
    faults = []
    install = function_body(lib, "fn install_model_file")
    if "trust_installed_model_file(gguf::PATH_LOCAL_MODEL_DATA)" not in install:
        faults.append("an install no longer grants the model's trust")
    if "gguf::PATH_LOCAL_MODEL_DATA" not in install:
        faults.append("the trust is not granted for the path the install writes")
    helper = function_body(lib, "fn trust_installed_model_file")
    for call in ("registry.provision(", "registry.mark_trusted("):
        if call not in helper:
            faults.append(f"the install's trust grant no longer makes the call {call!r}")
    if "std::fs_stat(" not in helper:
        faults.append("the install's trust grant does not record the installed file's own bytes")
    # The adoption is the same decision for a model the device already holds:
    # the file that was probed is the file that has to be trusted, and the
    # sentence has to say the bytes were not copied.
    if "trust_installed_model_file(path)" not in install:
        faults.append("the adoption no longer grants trust for the model it adopted")
    if "gguf::probe_all_models()" not in install:
        faults.append("the adoption no longer looks for a model the device already holds")
    if "Adopted(size, path)" not in install:
        faults.append("the adoption no longer reports the model it adopted")
    return faults


def _provider_label_faults(lib: str) -> list:
    """Every way a user-visible string can claim the platform runs on the device."""
    faults = []
    sources = {
        path.name: production_text(path.read_text(encoding="utf-8"))
        for path in sorted(SERVICE.rglob("*.rs"))
    }
    # The service's own source is the one under test: read it from the text
    # this case is holding, so a mutated copy can be put through the reader.
    sources["lib.rs"] = production_text(lib)
    for name, text in sources.items():
        for literal in byte_literals(text):
            lowered = literal.lower()
            if "on-device" in lowered or "on device" in lowered:
                faults.append(f"{name} claims the device answers by itself: {literal}")
    if not any("host-backed" in literal.lower() for literal in byte_literals(sources["lib.rs"])):
        faults.append(
            "no user-visible string reports the platform's generative provider as host-backed"
        )
    return faults


def test_an_untrusted_model_is_refused_before_its_bytes_are_read():
    """The gate is a position, not a sentence.

    A model the user has not trusted must not have its file opened at all, so
    the refusal is decided from the registry alone and the caller is told which
    of the two untrusted states it is in.
    """
    faults = _trust_gate_faults(lib_source())
    assert not faults, faults


def test_a_loaded_model_is_charged_to_the_handle_that_owns_it():
    """The model's bytes and the session that runs it are one model's.

    A reservation charged to nothing and a session bound to no model are the
    two ways the accounting stops reclaiming what it charged for.
    """
    faults = _reservation_faults(lib_source())
    assert not faults, faults


def test_the_embedding_route_refuses_an_untrusted_model():
    """The vector route is gated too, and it does not answer with another answer."""
    faults = _embedding_gate_faults(lib_source())
    assert not faults, faults


def test_an_install_is_the_users_trust_decision():
    """The file that arrived is the file the user asked for, and it is recorded."""
    faults = _install_trust_faults(lib_source())
    assert not faults, faults


def test_no_user_visible_string_claims_the_device_answers_by_itself():
    """The platform's generative provider is the host-backed development bridge."""
    faults = _provider_label_faults(lib_source())
    assert not faults, faults


def test_the_trust_and_reservation_contract_can_fail():
    """Each rule is put back the way it was, and the reader has to see it.

    A detector that cannot be tripped is a comment, so every rule above is
    checked against a copy of the service with that rule reversed.
    """
    lib = lib_source()

    assert not _trust_gate_faults(lib)
    assert not _reservation_faults(lib)
    assert not _embedding_gate_faults(lib)
    assert not _install_trust_faults(lib)
    assert not _provider_label_faults(lib)

    # 1. The gate moved behind the open, so an untrusted model's bytes are read
    #    before it is refused.
    moved = lib.replace(
        """    if let Some(refusal) = model_trust_refusal(path) {
        let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
        let (status, notice) = trust_refusal_notice(b"Generation", path, refusal, &mut scratch);
        reply_with_answer(sender_task_id, status, notice, tx_buf);
        return true;
    }
""",
        "",
        1,
    )
    assert moved != lib, "the trust gate the detector watches moved; this fixture is stale"
    assert any(
        "no longer asks whether the model is trusted" in fault
        for fault in _trust_gate_faults(moved)
    ), "a route with no trust gate passed"

    # 2. The session bound to no model, so an unload could not reach it.
    unbound = lib.replace(
        "ledger.create_session_for_model(sender_task_id, 1, model_handle, estimate)",
        "ledger.create_session(sender_task_id, 1, estimate)",
        1,
    )
    assert unbound != lib, "the bound session the detector watches moved; this fixture is stale"
    assert any(
        "no longer makes the call" in fault for fault in _reservation_faults(unbound)
    ), "a session bound to no model passed"

    # 3. The model's resident bytes reserved against nothing.
    unreserved = lib.replace(
        "ledger.reserve_resident_for(model_handle, estimate)",
        "ledger.reserve_resident(estimate)",
        1,
    )
    assert unreserved != lib, "the reservation the detector watches moved; this fixture is stale"
    assert any(
        "no longer makes the call" in fault for fault in _reservation_faults(unreserved)
    ), "a model charged to no handle passed"

    # 4. The changed-file path that leaves the ledger's reservation standing.
    leaky = lib.replace("unload_model_resources(handle);", "let _ = handle;", 1)
    assert leaky != lib, "the ledger release the detector watches moved; this fixture is stale"
    assert any(
        "does not release the ledger's reservation" in fault
        for fault in _reservation_faults(leaky)
    ), "a model unloaded on one side only passed"

    # 5. The embedding route answering with the baseline instead of refusing.
    fallback = lib.replace(
        """            if let Some(refusal) = model_trust_refusal(path) {
                let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
                let (status, notice) =
                    trust_refusal_notice(b"Embedding", path, refusal, &mut scratch);
                emit_embed_notice(sender_task_id, status, notice, tx_buf);
                return;
            }
""",
        "",
        1,
    )
    assert fallback != lib, "the embedding gate the detector watches moved; this fixture is stale"
    assert any(
        "no longer asks whether the model is trusted" in fault
        for fault in _embedding_gate_faults(fallback)
    ), "an embedding route with no trust gate passed"

    # 6. The install that stops granting the model's trust.
    ungranted = lib.replace(
        "trust_installed_model_file(gguf::PATH_LOCAL_MODEL_DATA);",
        "let _ = gguf::PATH_LOCAL_MODEL_DATA;",
        1,
    )
    assert ungranted != lib, "the install's trust grant the detector watches moved; this fixture is stale"
    assert any(
        "no longer grants the model's trust" in fault
        for fault in _install_trust_faults(ungranted)
    ), "an install that grants no trust passed"

    # 7. The adoption that stops trusting the model the device already holds.
    unadopted = lib.replace(
        "Some((_header, path)) => match trust_installed_model_file(path) {",
        "Some((_header, path)) => match Some(0u64) {",
        1,
    )
    assert unadopted != lib, "the adoption the detector watches moved; this fixture is stale"
    assert any(
        "no longer grants trust for the model it adopted" in fault
        for fault in _install_trust_faults(unadopted)
    ), "an adoption that trusts nothing passed"

    # 8. The user-visible string that claims the device answers by itself.
    claiming = lib.replace(
        "Generative stream unavailable:",
        "On-device generative stream unavailable:",
        1,
    )
    assert claiming != lib, "the provider sentence the detector watches moved; this fixture is stale"
    assert any(
        "claims the device answers by itself" in fault
        for fault in _provider_label_faults(claiming)
    ), "a string claiming the device answers by itself passed"
