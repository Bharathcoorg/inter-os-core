"""Platform Truth Contract: who may install and read a TLS trust anchor.

A certificate chain verifies only against an anchor, and this repository shipped
none: the SDK's default trust store is empty on purpose, because a root compiled
into the image trusts every certificate the matching private key signs, on every
device that image ever boots. That left the browser with one honest screen -- "no
trust anchor" -- on every HTTPS site, forever, however careful the peer's key was.

The device now holds anchors its own operator installed, and the shape of that
access is what these assertions pin. Installing is the operator's act, reachable
only from the serial console, so no application can choose which chains every
other application on the device accepts. Reading is what a verifying client needs,
and it is granted per built-in app the way the clock and the TRNG already are.
"""

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
APPS_DIR = ROOT / "apps"

# The raw identifier the trust-read capability was introduced with, above the
# RAPP manifest ladder on purpose -- see the manifest test below.
TRUST_RAW_ID = 39


def read_source(subpath: str) -> str:
    return (ROOT / subpath).read_text(encoding="utf-8")


def code_only(source: str) -> str:
    """Strips comments so checks are strictly against executable code."""
    without_block = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
    return re.sub(r"//[^\n]*", " ", without_block)


def const_value(source: str, name: str) -> int:
    match = re.search(rf"pub const {name}: usize = (\d+);", source)
    assert match, f"{name} is no longer a plain usize constant"
    return int(match.group(1))


def fn_body(source: str, name: str) -> str:
    """One function's text, from its signature to its closing brace at column 0."""
    marker = f"fn {name}("
    assert marker in source, f"{name} is not defined anywhere in this source"
    return source.split(marker, 1)[1].split("\n}", 1)[0]


def kernel_store_source() -> str:
    return read_source("kernel/src/crypto/tls_trust.rs")


def syscall_source() -> str:
    return code_only(read_source("kernel/src/syscall/mod.rs"))


def sdk_syscall_source() -> str:
    return read_source("sdk/rust/interos-std/src/lib.rs")


def kernel_spawn_bodies() -> dict:
    """Each first-party app's capability table as the kernel builds it."""
    src = read_source("kernel/src/task/user.rs")
    return {
        match.group(1).removeprefix("spawn_app_"): match.group(2)
        for match in re.finditer(
            r"pub fn (spawn_app_\w+)\(id_security: TaskId\) -> TaskId \{(.*?)\n\}\n",
            src,
            flags=re.S,
        )
    }


def apps_reading_the_store() -> list:
    """App sources that ask the kernel for the device's anchors."""
    callers = []
    marker = re.compile(r"load_device_trust_anchors\(|SyscallNumber::TlsTrust|tls_trust_count\(")
    for main in sorted(APPS_DIR.glob("*/src/*.rs")):
        if marker.search(code_only(main.read_text(encoding="utf-8"))):
            callers.append(main.parts[-3])
    return sorted(set(callers))


def test_the_two_sides_agree_about_how_big_an_anchor_and_a_store_are():
    """Kernel and userspace do not share a crate, so the compiler cannot check this.

    WHY it has to be pinned anyway: the SDK hands the kernel a buffer per anchor
    and stops enumerating at its own slot count. If the kernel's numbers moved,
    the browser would read a truncated DER or a subset of the store and present
    whichever anchors it could not hold as absent.
    """
    kernel = kernel_store_source()
    sdk = sdk_syscall_source()
    assert const_value(kernel, "MAX_ANCHOR_DER_BYTES") == const_value(
        sdk, "TLS_TRUST_ANCHOR_BYTES"
    ), "the kernel's anchor limit and the SDK's anchor buffer are different sizes"
    assert const_value(kernel, "MAX_TRUSTED_ANCHORS") == const_value(
        sdk, "TLS_TRUST_MAX_ANCHORS"
    ), "the kernel's store is bigger than the table the SDK enumerates, so a " \
       "device with the extra anchors verifies against part of its own store"


def test_only_the_operators_console_can_change_the_store():
    """An application must not get to decide what the whole device trusts.

    WHY this is a scan for absence: the mutation methods exist on the store, so
    the question is who reaches them. `RAPP_TRUST` has no syscall wrapper for the
    same reason, and a wrapper added later -- even a guarded one -- would put
    device-wide trust in the hands of whichever app held the guard.
    """
    syscalls = syscall_source()
    touchpoints = [
        line for line in syscalls.splitlines() if "TLS_TRUST_STORE" in line
    ]
    assert touchpoints, (
        "the syscall layer never reads TLS_TRUST_STORE, so the trust count and "
        "anchor reads this contract gates are not wired to the store at all"
    )
    for line in touchpoints:
        for verb in ("install(", "revoke(", "clear("):
            assert verb not in line, (
                f"the syscall layer reaches TLS_TRUST_STORE.{verb}, which makes trust "
                "installation reachable from ring 3 rather than from the console"
            )
    serial = code_only(read_source("kernel/src/serial.rs"))
    assert "store.install(" in serial and "store.revoke(" in serial, (
        "the store lost the console path that mutates it, so the operator has no way "
        "to put an anchor on the device"
    )


def test_the_console_verbs_are_wired_and_only_answer_their_own_lines():
    """The operator's channel has to be reachable and must not swallow traffic.

    WHY the false return is asserted: the dispatcher tries each verb handler in
    turn, and a handler that claimed every line would take the console away from
    every other command the boot tooling sends.
    """
    src = code_only(read_source("kernel/src/serial.rs"))
    assert "fn handle_tls_trust_command(s: &str) -> bool" in src, (
        "the trust store has no console handler, so nothing can install an anchor"
    )
    assert "handle_tls_trust_command(s)" in src, (
        "the trust handler is defined but never tried by the command dispatcher"
    )
    for verb in ('"TLS_TRUST_LIST"', '"TLS_TRUST_REVOKE"', '"TLS_TRUST"'):
        assert verb in src, f"the console no longer accepts {verb}"
    handler = src.split("fn handle_tls_trust_command")[1].split("\nfn handle_serial_command")[0]
    assert "false" in handler, (
        "the trust handler claims lines that are not its verbs"
    )


def test_the_read_is_gated_on_the_anchor_and_nothing_else():
    """Reading an anchor is a question about the device, not authority over it."""
    gate = fn_body(syscall_source(), "authorize_tls_trust_read")
    assert "CapabilityType::TlsTrustRead" in gate, (
        "the trust read is authorised on something other than the trust capability"
    )
    assert "Rights::READ" in gate, (
        "reading an anchor is a read; asking for a wider right is an entitlement "
        "nothing in this tree is granted"
    )
    assert "require_system_control" not in gate, (
        "the trust read fell back to the device-wide administration gate, which no "
        "app holds -- so every HTTPS verification is refused"
    )
    for fn in ("sys_tls_trust_count", "sys_tls_trust_read"):
        body = fn_body(syscall_source(), fn)
        assert "authorize_tls_trust_read()" in body, (
            f"{fn} answers without checking the caller's grant"
        )


def test_a_short_buffer_is_a_refusal_rather_than_a_truncated_certificate():
    """Half a DER parses as nothing, and the peer would be blamed for it.

    WHY this is checked at the syscall rather than left to the SDK: the kernel is
    the side that has the bytes, so it is the side that can say no instead of
    writing what fits.
    """
    body = fn_body(syscall_source(), "sys_tls_trust_read")
    at = body.index("copy_from_slice")
    assert "out_buf.len() < der.len()" in body[:at], (
        "the kernel copies whatever fits instead of refusing a buffer too small for "
        "the anchor, handing the caller a truncated certificate"
    )
    assert "TargetNotFound" in body, (
        "a position past the end of the store has no distinct answer, so a client "
        "cannot tell a revoked anchor from a refused read"
    )


def test_the_trust_capability_is_one_named_thing_with_one_identifier():
    """A capability missing from any of the name tables is not a capability."""
    src = read_source("kernel/src/capability/mod.rs")
    assert re.search(r"^\s+TlsTrustRead,\s*$", src, flags=re.M), (
        "TlsTrustRead is missing from the capability type list"
    )
    assert f"TlsTrustRead => {TRUST_RAW_ID}" in src, (
        f"TlsTrustRead must serialise to the raw id {TRUST_RAW_ID}"
    )
    assert f"{TRUST_RAW_ID} => Some(CapabilityType::TlsTrustRead)" in src, (
        "the raw id must resolve back to the variant, or a table loaded from the "
        "wire loses the grant"
    )
    assert 'TlsTrustRead => "tls.trust.read"' in src, (
        "the capability has no canonical name, so it cannot appear in an audit line "
        "a person can read"
    )
    assert '"tls.trust.read" => Some(CapabilityType::TlsTrustRead)' in src, (
        "the canonical name must resolve back to the variant"
    )
    to_raw = src.split("pub const fn to_raw(")[1].split("pub const fn from_raw(")[0]
    ids = re.findall(r"=>\s*(\d+),", to_raw)
    assert len(ids) == len(set(ids)), f"capability raw ids collide: {sorted(ids, key=int)}"


def test_a_signed_manifest_cannot_declare_the_trust_grant():
    """The kernel mints this grant per built-in app, and that limit is deliberate."""
    user_src = code_only(read_source("kernel/src/task/user.rs"))
    ladder = user_src.split("fn capabilities_from_rapp(")[1].split("\n}\n")[0]
    assert f"=> (CapabilityType::TlsTrustRead" not in ladder, (
        "the RAPP capability ladder now mints a trust read from a manifest; "
        "re-audit who is allowed to read the device's anchors"
    )
    rapp_src = code_only(read_source("kernel/src/package/rapp.rs"))
    range_match = re.search(r"\((\d+)\.\.=(\d+)\)\.contains\(&cap\)", rapp_src)
    assert range_match is not None, (
        "package verification no longer bounds the declared capability id, so the "
        "ladder this test guards has moved"
    )
    assert int(range_match.group(2)) < TRUST_RAW_ID, (
        "package verification now accepts a trust-read id straight from a manifest"
    )


def test_an_app_that_reads_the_store_holds_the_grant():
    """The call and the authority for it are one decision, taken twice.

    WHY this crosses the boundary: a browser that asks for anchors the kernel will
    not hand it reads as an empty store, and would then say "this device trusts
    nothing" about a device whose operator installed an anchor that morning.
    """
    spawns = kernel_spawn_bodies()
    assert spawns, "no app spawn tables found in the kernel -- the scan tests nothing"
    callers = apps_reading_the_store()
    assert callers, "no app reads the trust store -- the scan has stopped finding the call"
    for app in callers:
        assert app in spawns, f"{app} reads the store but no kernel spawn table names it"
        assert "CapabilityType::TlsTrustRead" in spawns[app], (
            f"{app} asks for the device's anchors and the kernel grants it no trust "
            "read, so it can only ever report an empty store"
        )


def test_the_trust_grant_reaches_only_the_apps_that_ask():
    """Least privilege is a count as much as a reason."""
    spawns = kernel_spawn_bodies()
    holders = sorted(app for app, body in spawns.items()
                     if "CapabilityType::TlsTrustRead" in body)
    assert holders == apps_reading_the_store(), (
        f"the kernel grants trust-anchor reads to {holders} but only "
        f"{apps_reading_the_store()} read the store"
    )


def test_the_browser_verifies_against_the_device_store_it_read():
    """The anchors the handshake judges with are the device's, not a bundled list.

    WHY the refusal is asserted with it: an app that could not read the store had
    two dishonest ways to carry on -- fall back to a compiled-in root, or carry an
    empty one and blame the peer. Both end in a padlock over an unverified chain.
    """
    src = code_only(read_source("apps/browser/src/main.rs"))
    assert "load_device_trust_anchors()" in src, (
        "the browser builds its verifier without the anchors installed on this device"
    )
    assert "with_roots(device_anchors)" in src, (
        "the anchors it read are never handed to the verifier, so the handshake "
        "still judges every chain against an empty store"
    )
    refusal = src.split("load_device_trust_anchors()")[1].split("TLS_VERIFIER")[0]
    assert "return" in refusal, (
        "a refused trust read falls through to the handshake instead of ending the "
        "fetch where the refusal happened"
    )
    assert "DEFAULT_TRUSTED_ROOTS" not in src and "INTEROS_ROOT_CA_DER" not in src, (
        "the browser reaches for a trust list shipped in the SDK instead of the "
        "device's own store"
    )
    http_src = code_only(read_source("apps/browser/src/http.rs"))
    assert "TlsTrustStoreRefused" in http_src, (
        "no error kind names a refused trust read, so the user is shown a generic "
        "failure for a fault inside the device"
    )


def test_a_store_that_could_not_be_read_is_not_reported_as_an_empty_one():
    """"Nothing installed" and "I was not told" are different answers.

    WHY the SDK distinguishes them: an empty table is the honest reason a chain
    fails on a device the operator never provisioned, while a refusal is a fault in
    this boot. Folding the second into the first would send a person to install an
    anchor that is already there.
    """
    src = code_only(read_source("sdk/rust/interos-std/src/tls/trust.rs"))
    assert "ReadError::Refused" in src, (
        "a refused read is no longer its own answer"
    )
    assert "ReadError::SizeMismatch" in src, (
        "the client no longer notices a kernel store larger than the table it holds"
    )
    assert "unwrap_or" not in src, (
        "the loader substitutes a default for a kernel that refused, which reports "
        "an unknown store as an empty one"
    )


def test_the_image_installs_no_anchor_of_its_own():
    """The OS ships no root: only the operator's console puts one on a device.

    WHY this is pinned at boot: the boot sequence is where a convenience default
    would be added -- "seed the demo CA so HTTPS works out of the box" -- and that
    is exactly the compiled-in trust this design refuses. What boot may do is say
    out loud what the device holds.
    """
    main = code_only(read_source("kernel/src/main.rs"))
    boot = main.split("fn kernel_main")[1]
    assert "tls_trust::report_at_boot()" in boot, (
        "boot no longer says what the device trusts, so an operator who lost an "
        "anchor to a reboot learns it from a failed connection instead"
    )
    assert ".install(" not in boot, (
        "the boot path installs a trust anchor, which means every device built from "
        "this image trusts a key whoever holds the private half of"
    )
    sdk = code_only(read_source("sdk/rust/interos-std/src/tls/certificate.rs"))
    assert "pub const DEFAULT_TRUSTED_ROOTS: &[&[u8]] = &[];" in sdk, (
        "the SDK shipped a trust root"
    )


def test_an_anchor_that_could_never_verify_is_refused_at_the_door():
    """A stored blob the client cannot use is a slot gone quiet, not an error.

    WHY the gate is at install rather than at verify: the console takes hex a
    person pasted, and a copy that lost its tail is a common mistake. Accepted, it
    would sit in the store verifying nothing, and the operator would have no way to
    tell a wrong key from an unused one.
    """
    src = code_only(kernel_store_source())
    gate = src.split("pub fn check_anchor_shape(")[1].split("\nfn contains_subsequence")[0]
    for refusal in ("AnchorTooLarge", "NotDerSequence", "NotEd25519"):
        assert refusal in gate, f"the install gate no longer names {refusal}"
    assert "ED25519_ALGORITHM_OID" in gate, (
        "the store accepts any certificate, including one for a signature scheme "
        "this client cannot verify at all"
    )
    assert "header + declared != der.len()" in gate, (
        "the store accepts bytes that are not one complete DER sequence"
    )
