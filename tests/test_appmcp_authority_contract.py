# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""The AppMcp receiver authority contract (audit finding H15 / A5-02).

WHAT THIS FILE IS FOR: eight applications expose a tool-invocation surface over
IPC. Each used to authorize a request by testing `req.cap_handle == 0` -- that
is, it refused a *zero* handle and accepted **any non-zero 64-bit integer**. A
caller could put `0xDEAD_BEEF` in the first eight bytes of the message and run
the tool with the receiving app's own authority. No capability was verified.

The fix asks the kernel instead. Syscall 166 (`AppMcpAuthorize`) resolves the
claimed handle by generation in the SENDER's own capability table and checks
type, rights, object scope, global revocation and delegated-lease validity. The
one trustworthy fact a receiver holds is the `sender` TaskId the IPC router
stamped on the message, and that is what it passes.

WHAT THESE ASSERTIONS ARE: source-level pins on the production receivers. They
measure the text of `handle_appmcp_request` in each app's `main.rs`, because
every app bin in this workspace is declared `test = false` -- the receiver is
not reachable from a host unit test, and a Rust test placed in `main.rs` would
never run. Each pin asserts the *presence of the enforcement call* in the
receiver, before any tool dispatch, and the *absence of the bypass*.

WHAT THEY ARE NOT: they do not execute a receiver, and they cannot prove the
kernel refuses a forged handle. The runtime legs are covered elsewhere and are
classified honestly here:
  * the fail-closed reading of the kernel's answer is unit-tested for real in
    `sdk/rust/interos-std/src/lib.rs` (`abi_tests::only_a_zero_answer_is_a_grant`);
  * the capability mapping is unit-tested in
    `sdk/rust/interos-std/src/appmcp.rs`
    (`tool_authority_names_the_kernel_agent_task_capability`);
  * the kernel's own resolution logic lives in `kernel/src/syscall/mod.rs`
    (`sys_appmcp_authorize`) and is exercised by the kernel's test suite;
  * end-to-end refusal of a forged handle by a running receiver is NOT verified
    by this file and needs a device or QEMU run.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

#: Every production AppMcp receiver in the tree. `agent_manager` is a *client*
#: of the protocol, not a receiver, and is checked separately below.
RECEIVERS = (
    "calculator",
    "calendar",
    "dialer",
    "file_manager",
    "messages",
    "music",
    "notes",
    "settings",
)

#: The gate every receiver must call. `sender` is the router's TaskId; the
#: handle is the claim being tested.
GATE_CALL = "std::appmcp::authorize_tool_sender(sender, req.cap_handle)"
GATE = "if !" + GATE_CALL + " {"

#: The bypass this finding is about, in both polarities.
BYPASS_PATTERNS = (
    re.compile(r"cap_handle\s*==\s*0"),
    re.compile(r"cap_handle\s*!=\s*0"),
    re.compile(r"cap_handle\s*>\s*0"),
    re.compile(r"cap_handle\s*<\s*1"),
)

RECEIVER_ANCHOR = "fn handle_appmcp_request"
DISPATCH_ANCHOR = "match req.tool_id"


def read(relative: str) -> str:
    path = ROOT / relative
    assert path.is_file(), f"missing {path}"
    return path.read_text(encoding="utf-8")


def app_main(app: str) -> str:
    return read(f"apps/{app}/src/main.rs")


def strip_comments(source: str) -> str:
    """`source` with comments gone and every literal left exactly as written.

    WHY comments are removed before measuring: the receivers carry their WHY
    notes inline, and a pin that read a comment as code would pass on an app
    that merely *describes* the gate it never calls.
    """
    out = []
    i, n = 0, len(source)
    while i < n:
        if source.startswith("//", i):
            j = source.find("\n", i)
            i = n if j == -1 else j
        elif source.startswith("/*", i):
            j = source.find("*/", i)
            i = n if j == -1 else j + 2
        elif source[i] == '"':
            j = i + 1
            while j < n and source[j] != '"':
                j += 2 if source[j] == "\\" else 1
            j = min(j + 1, n)
            out.append(source[i:j])
            i = j
        else:
            out.append(source[i])
            i += 1
    return "".join(out)


def fn_body(source: str, anchor: str) -> str:
    """The `{...}` block of the function named by `anchor`."""
    at = source.find(anchor)
    assert at != -1, f"{anchor!r} is not in this file"
    start = source.index("{", at)
    depth = 0
    for i in range(start, len(source)):
        if source[i] == "{":
            depth += 1
        elif source[i] == "}":
            depth -= 1
            if depth == 0:
                return source[start + 1:i]
    raise AssertionError(f"the body of {anchor!r} never closes")


def receiver_body(app: str) -> str:
    return fn_body(strip_comments(app_main(app)), RECEIVER_ANCHOR)


def receiver_signature(app: str) -> str:
    source = strip_comments(app_main(app))
    at = source.index(RECEIVER_ANCHOR)
    return source[at:source.index("{", at)]


# ---------------------------------------------------------------------------
# 1. The receiver asks the kernel, and asks before it dispatches
# ---------------------------------------------------------------------------


def test_every_receiver_calls_the_kernel_authorization_gate():
    """The enforcement call must be in the receiver itself.

    A gate that lives only in the SDK, or only in a comment, authorizes nothing.
    """
    for app in RECEIVERS:
        body = receiver_body(app)
        assert GATE in body, (
            f"{app}: handle_appmcp_request does not gate on {GATE!r}. Without it "
            "the receiver runs the tool on its own authority for any caller."
        )


def test_the_gate_runs_before_any_tool_dispatch():
    """Order matters: a check after the match is a check that never guarded it."""
    for app in RECEIVERS:
        body = receiver_body(app)
        gate_at = body.index(GATE)
        dispatch_at = body.index(DISPATCH_ANCHOR)
        assert gate_at < dispatch_at, (
            f"{app}: the authorization gate sits after `match req.tool_id`, so a "
            "tool arm can run before the sender is verified."
        )


def test_the_gate_refuses_with_permission_denied_and_stops():
    """The refusal branch must answer the caller and return, not fall through."""
    for app in RECEIVERS:
        body = receiver_body(app)
        refusal = fn_body(body, GATE)
        assert "APPMCP_STATUS_PERMISSION_DENIED" in refusal, (
            f"{app}: the refused request is not answered with PERMISSION_DENIED"
        )
        assert "return true;" in refusal, (
            f"{app}: the refusal branch does not return, so dispatch continues "
            "after the sender was refused."
        )


def test_the_gate_passes_the_routers_sender_not_a_payload_field():
    """`sender` is the kernel-stamped TaskId; anything from `req` is forgeable.

    The signature must still take it as a parameter, and every call site in the
    event loop must hand it the value `ipc::receive_any` returned.
    """
    for app in RECEIVERS:
        signature = receiver_signature(app)
        assert "sender: u64" in signature, (
            f"{app}: the receiver no longer takes the router's sender TaskId"
        )
        assert GATE_CALL in receiver_body(app)
        assert "req.sender" not in receiver_body(app), (
            f"{app}: the gate reads a sender out of the request payload"
        )

        source = strip_comments(app_main(app))
        calls = re.findall(
            r"(?<!fn )handle_appmcp_request\([^)]*\)", source
        )
        assert calls, f"{app}: the receiver is never called"
        for call in calls:
            args = [a.strip() for a in call[call.index("(") + 1:-1].split(",")]
            assert len(args) == 4, f"{app}: unexpected receiver arity in {call!r}"
            assert args[2] == "sender", (
                f"{app}: {call!r} does not pass the router's sender as the third "
                "argument"
            )
        assert "std::ipc::receive_any" in source, (
            f"{app}: the sender does not come from the kernel's IPC receive"
        )


# ---------------------------------------------------------------------------
# 2. The bypass is gone, and nothing replaced it
# ---------------------------------------------------------------------------


def test_no_receiver_accepts_a_handle_merely_for_being_nonzero():
    """The defect itself: `cap_handle == 0` refuses only zero.

    Any non-zero integer -- 1, 0xDEAD_BEEF, u64::MAX -- passed that test, so it
    proved nothing about the requester.
    """
    for app in RECEIVERS:
        source = strip_comments(app_main(app))
        for pattern in BYPASS_PATTERNS:
            match = pattern.search(source)
            assert match is None, (
                f"{app}: {match.group(0)!r} is still used as an authority test. "
                "A non-zero handle is a claim, not a capability."
            )


def test_no_receiver_has_a_trusted_caller_or_debug_fallback():
    """Zero ambient authority: no second way in beside the kernel's answer."""
    for app in RECEIVERS:
        body = receiver_body(app)
        gate_at = body.index(GATE)
        before_dispatch = body[gate_at:body.index(DISPATCH_ANCHOR)]
        # The refusal branch is the only thing between the gate and dispatch.
        assert "||" not in before_dispatch, (
            f"{app}: an alternative arm beside the authorization gate could "
            "accept an unverified handle."
        )
        for shortcut in ("sender ==", "sender !=", "is_trusted", "debug", "bypass"):
            assert shortcut not in before_dispatch, (
                f"{app}: {shortcut!r} appears in the authorization path"
            )


def test_the_sdk_gate_is_the_only_authority_path_the_receivers_use():
    """No receiver resolves the claim itself with a local lookup.

    `cap_lookup` and `capability_handle` answer questions about the CALLER's
    table. Using either to judge an incoming request would verify the receiver's
    own authority and call it the sender's.
    """
    for app in RECEIVERS:
        body = receiver_body(app)
        for local in ("cap_lookup", "capability_handle", "appmcp_authorize"):
            assert local not in body, (
                f"{app}: the receiver judges the claim with {local!r} instead of "
                "the sender-scoped gate."
            )


# ---------------------------------------------------------------------------
# 3. The gate asks about the right capability, in the right numeric space
# ---------------------------------------------------------------------------


def test_the_gate_asks_the_kernel_about_the_senders_table():
    sdk = strip_comments(read("sdk/rust/interos-std/src/lib.rs"))
    body = fn_body(sdk, "pub fn appmcp_authorize")
    assert "SyscallNumber::AppMcpAuthorize" in body, (
        "appmcp_authorize does not call syscall 166"
    )
    assert "syscall5(" in body, "the authorization call must pass all five arguments"
    args = body[body.index("syscall5("):]
    for argument in (
        "sender_task_id",
        "cap_handle",
        "cap_type_raw",
        "required_rights",
        "object_id",
    ):
        assert argument in args, f"appmcp_authorize drops {argument}"
    assert "appmcp_verdict(ret)" in body, (
        "the answer is not read through the fail-closed interpreter"
    )


def test_only_a_zero_answer_is_a_grant():
    """A positive or unknown code must refuse, not succeed.

    The neighbouring wrappers use `ret >= 0` because they return a count. This
    call returns a verdict, so `>= 0` would grant an answer nobody defined.
    """
    sdk = strip_comments(read("sdk/rust/interos-std/src/lib.rs"))
    body = fn_body(sdk, "pub fn appmcp_verdict")
    assert "if ret == 0 {" in body, (
        "the AppMcp verdict is not read as exactly zero; a non-zero answer could "
        "be taken as a grant."
    )
    assert "ret >= 0" not in body


def test_the_policy_names_the_kernel_capability_space():
    """Three disjoint numeric spaces describe the same permission.

    kernel CapabilityType::AgentTask = 33, SensitivePermission::AgentTask = 15,
    manifest id = 0x5000_0000 + slot. Syscall 166 reads the first. In that space
    15 is ShmProducer, so the permission number would verify the wrong authority.
    """
    appmcp = strip_comments(read("sdk/rust/interos-std/src/appmcp.rs"))
    assert "pub const TOOL_CAP_TYPE: u64 = crate::cap_type::AGENT_TASK;" in appmcp
    assert "pub const TOOL_CAP_RIGHTS: u64 = 1 << 5;" in appmcp, (
        "the gate must require Rights::INVOKE, not merely read or write"
    )
    assert "pub const TOOL_CAP_OBJECT: u64 = 0x1F;" in appmcp, (
        "the gate must name the agent-tool domain the consent broker mints"
    )

    sdk = strip_comments(read("sdk/rust/interos-std/src/lib.rs"))
    assert "pub const AGENT_TASK: u64 = 33;" in sdk
    assert "pub const SHM_PRODUCER: u64 = 15;" in sdk
    assert "AgentTask = 15," in sdk, "the SensitivePermission space is a different numbering"

    gate = fn_body(appmcp, "pub fn authorize_tool_sender")
    for constant in ("TOOL_CAP_TYPE", "TOOL_CAP_RIGHTS", "TOOL_CAP_OBJECT"):
        assert constant in gate, f"authorize_tool_sender does not pass {constant}"
    assert "sender" in gate and "cap_handle" in gate
    assert ".is_ok()" in gate, "the gate must grant only on the kernel's Ok"


def test_the_module_no_longer_claims_the_app_verifies_the_token_itself():
    """The doc comment used to say "An app verifies this token", which was false.

    It also named `SensitivePermission::AgentTask = 15` as the capability, which
    is the wrong numeric space for a kernel capability argument.
    """
    raw = read("sdk/rust/interos-std/src/appmcp.rs")
    header = raw[:raw.index("/// Magic identifier")]
    assert "An app verifies" not in header
    assert "syscall 166" in header
    assert "authorize_tool_sender" in header
    assert "CapabilityType::AgentTask = 33" in header


# ---------------------------------------------------------------------------
# 4. The legitimate caller still has a path
# ---------------------------------------------------------------------------


def test_the_agent_manager_sends_a_kernel_resolved_handle():
    """Closing the hole must not close the real tool path.

    The Agent Manager resolves its own AgentTask grant to a generation-counted
    handle with `capability_handle` and sends that. The receiver then asks the
    kernel whether *this sender* holds it, which is the same grant the broker
    minted -- so the authorized path survives the fix.
    """
    agent = strip_comments(read("apps/agent_manager/src/lib.rs"))
    assert "pub const TOOL_CAPABILITY_TYPE: u64 = interos_std::cap_type::AGENT_TASK;" in agent
    assert "pub const TOOL_CAPABILITY_OBJECT: u64 = 0;" in agent

    # WHY the impl block is isolated first: this file has a second `probe()`
    # for the model surface, and an unscoped anchor answers for the wrong one.
    authority = fn_body(agent, "impl ToolAuthority")
    probe = fn_body(authority, "pub fn probe()")
    assert "interos_std::capability_handle(" in probe, (
        "the Agent Manager no longer resolves a kernel handle to send"
    )
    assert "TOOL_CAPABILITY_TYPE" in probe

    dispatch = fn_body(agent, "pub fn call_appmcp_tool")
    assert "AppMcpRequest::new(cap_handle, tool_id, 1)" in dispatch
    assert "APPMCP_MSG_INVOKE" in dispatch


def test_the_domain_object_matches_the_grant_the_kernel_mints():
    """Syscall 166 scopes by exact match, so the requested object must be one
    the broker actually mints.

    The app spawn path and the consent broker both mint `AgentTask` with
    `object_id = 0x1F`; the userspace Agent service holds an unscoped grant
    (`object_id = 0`), which satisfies any requested object. Requesting 0 here
    would refuse the Agent Manager's own grant and break the only production
    caller.

    This pin reads the kernel as evidence and does not modify it.
    """
    spawn = read("kernel/src/task/user.rs")
    assert "cap_type: CapabilityType::AgentTask," in spawn
    at = spawn.index("cap_type: CapabilityType::AgentTask,")
    grant = spawn[at:at + 220]
    assert "rights: Rights::INVOKE," in grant, (
        "the spawned Agent Manager grant no longer carries INVOKE"
    )
    assert "object_id: 0x1F," in grant, (
        "the spawned Agent Manager grant no longer names the 0x1F tool domain; "
        "the receivers' requested object must be revisited with the kernel owner."
    )

    permission = read("kernel/src/capability/permission.rs")
    at = permission.index("Self::AgentTask => (")
    assert "CapabilityType::AgentTask," in permission[at:at + 400]
    assert "0x1F," in permission[at:at + 400]


# ---------------------------------------------------------------------------
# 5. No receiver was missed
# ---------------------------------------------------------------------------


def test_the_receiver_inventory_is_complete():
    """A ninth receiver added later must not slip past this contract."""
    found = set()
    for path in sorted((ROOT / "apps").rglob("*.rs")):
        text = path.read_text(encoding="utf-8")
        if RECEIVER_ANCHOR in text:
            found.add(path.relative_to(ROOT / "apps").parts[0])
    assert found == set(RECEIVERS), (
        f"the AppMcp receiver inventory drifted: found {sorted(found)}, "
        f"contract covers {sorted(RECEIVERS)}"
    )


def test_no_app_authorizes_a_tool_by_a_nonzero_integer_anywhere():
    """The same mistake outside a receiver is still ambient authority."""
    for path in sorted((ROOT / "apps").rglob("*.rs")):
        source = strip_comments(path.read_text(encoding="utf-8"))
        for pattern in BYPASS_PATTERNS:
            match = pattern.search(source)
            assert match is None, (
                f"{path.relative_to(ROOT)}: {match.group(0)!r} is used as an "
                "authority test."
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