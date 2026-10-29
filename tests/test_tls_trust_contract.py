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


def client_read_body(source: str) -> str:
    """The TLS client's `read`, located by its whole signature.

    WHY not `fn_body`: the transport trait declares a shorter `fn read(...)` with
    no body earlier in the same file, and a search for the name alone stops at
    that declaration instead of the reader it meant to pin.
    """
    marker = "pub fn read(&mut self, out: &mut [u8])"
    assert marker in source, "the client no longer reads through this signature"
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
    for verb in (
        '"TLS_TRUST_LIST"',
        '"TLS_TRUST_REVOKE"',
        '"TLS_TRUST"',
        '"TLS_TRUST_BEGIN"',
        '"TLS_TRUST_CHUNK"',
        '"TLS_TRUST_COMMIT"',
    ):
        assert verb in src, f"the console no longer accepts {verb}"
    handler = src.split("fn handle_tls_trust_command")[1].split("\nfn handle_serial_command")[0]
    assert "false" in handler, (
        "the trust handler claims lines that are not its verbs"
    )


def test_an_anchor_bigger_than_a_console_line_arrives_in_chunks():
    """The store holds 512-byte anchors, and one console line cannot carry one.

    WHY this is a real limit rather than a convenience: a line accumulates at
    most 511 characters and a 512th discards the whole line, so `TLS_TRUST <hex>`
    tops out near 250 anchor bytes -- under the 274 bytes a trimmed self-signed
    Ed25519 root actually measures. Without the chunk verbs the largest anchor
    the store was built to hold could never be installed.
    """
    serial = code_only(read_source("kernel/src/serial.rs"))
    staging = serial.split("struct AnchorTransfer {")[1].split("}\n")[0]
    for field in ("der", "received"):
        assert f"{field}: [u8; crate::crypto::tls_trust::MAX_ANCHOR_DER_BYTES]" in staging or \
               f"{field}: [bool; crate::crypto::tls_trust::MAX_ANCHOR_DER_BYTES]" in staging, (
            f"the staging buffer's {field} is no longer bounded by the store's own "
            "anchor limit, so a chunked transfer could overflow it"
        )
    begin = fn_body(serial, "handle_tls_trust_command").split("TLS_TRUST_BEGIN")[1]
    begin = begin.split("TLS_TRUST_CHUNK")[0]
    assert "len > crate::crypto::tls_trust::MAX_ANCHOR_DER_BYTES" in begin, (
        "BEGIN accepts an anchor larger than the store, so a declared size the "
        "staging buffer cannot hold is taken on trust"
    )


def test_a_chunked_anchor_is_framed_inside_one_console_line():
    """A chunk that outgrows the line is a chunk the console throws away."""
    serial = code_only(read_source("kernel/src/serial.rs"))
    match = re.search(r"const ANCHOR_CHUNK_HEX_CHARS: usize = (\d+);", serial)
    assert match, "the chunk payload is no longer a single bounded constant"
    chunk_hex = int(match.group(1))
    assert chunk_hex % 2 == 0, "an odd hex limit splits a byte across two chunks"
    # "TLS_TRUST_CHUNK " + a three-digit offset + a space + the payload.
    assert len("TLS_TRUST_CHUNK") + 1 + 3 + 1 + chunk_hex <= 511, (
        "the largest chunk does not fit the console line, so a whole anchor would "
        "be installable only by luck about how long its offset field is"
    )
    assert chunk_hex >= 2, "the chunk verbs cannot carry an anchor at all"


def test_a_half_received_anchor_is_refused_at_commit_rather_than_padded():
    """The staging buffer starts as zeroes, and a dropped chunk leaves them in.

    WHY the check is at commit: a certificate with bytes quietly replaced by
    zeroes is still DER-shaped enough to install, and the operator would then
    watch a chain fail against an anchor the console said was accepted.
    """
    body = fn_body(code_only(read_source("kernel/src/serial.rs")), "handle_tls_trust_command")
    chunk = body.split('"TLS_TRUST_CHUNK"')[1].split('"TLS_TRUST_COMMIT"')[0]
    assert "offset.checked_add(chunk_len)" in chunk, (
        "a chunk is written wherever it claims to start, so one line can run past "
        "the anchor the operator declared"
    )
    assert "state.received[offset + i] = true" in chunk, (
        "received bytes are no longer marked, so commit cannot tell a whole anchor "
        "from a partial one"
    )
    commit = body.split('"TLS_TRUST_COMMIT"')[1]
    assert ".iter().any(|v| !*v)" in commit, (
        "commit installs whatever the staging buffer holds, zeroes and all"
    )
    assert "transfer.take()" in commit, (
        "a committed or abandoned transfer stays staged, so the next commit could "
        "reinstall an anchor the operator already withdrew"
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


def test_a_ticket_on_the_answer_is_stepped_over_rather_than_refused():
    """A server sends its session ticket before the page, and a ticket is not a fault.

    WHY this is pinned at the record reader: RFC 8446 4.6.1 lets the server emit
    NewSessionTicket as soon as the handshake closes, so on a real peer the first
    record answering a GET is housekeeping, not the document. Treating it as an
    error ended the fetch with zero bytes -- and the screen then told the operator
    the gateway had returned nothing, when the gateway had returned exactly what
    was asked for. Skipping it is also bounded, because a peer that sends nothing
    but tickets has to end with a named refusal instead of a hung read.
    """
    src = read_source("sdk/rust/interos-std/src/tls/client.rs")
    assert "const HANDSHAKE_NEW_SESSION_TICKET: u8 = 4;" in src, (
        "the reader no longer names the message it is prepared to step over"
    )
    body = client_read_body(src)
    assert "ContentType::Handshake" in body and "if is_new_session_ticket(" in body, (
        "a NewSessionTicket on the answer is refused again, which reads as an "
        "empty response from a server that did respond"
    )
    assert "ContentType::ApplicationData" in body, (
        "the reader no longer separates the document from what it discards"
    )
    assert "MAX_SKIPPED_RECORDS" in body, (
        "records that are not the document are skipped without a bound, so a peer "
        "that only sends tickets never lets the read return"
    )
    assert "_ => return Err(TlsError::UnexpectedMessage)" in body, (
        "a post-handshake message that changes the keys or ends the connection is "
        "no longer refused, so its records would be decrypted under the wrong keys"
    )
    assert "open_record_in_place(&mut self.record" in body, (
        "a discarded record is decrypted into the caller's window again, so a "
        "caller asking for a small read is refused for a message it never asked for"
    )


def test_a_refused_answer_is_not_blamed_on_the_gateway():
    """The reader refusing a record is this device's decision, not the server's silence.

    WHY the browser keeps the two apart: "the gateway returned zero bytes" and "an
    answer arrived and this client would not take it" point an operator at
    completely different machines. Both leave the screen with no document, so the
    only thing that distinguishes them is whether the read errored.
    """
    src = code_only(read_source("apps/browser/src/main.rs"))
    fetch = src.split("fn fetch_over_tls(")[1].split("\n}")[0]
    assert "record_refused = true" in fetch, (
        "the read error is dropped, so the browser cannot tell its own refusal "
        "from a peer that answered with nothing"
    )
    assert "if record_refused" in fetch, (
        "both zero-byte outcomes collapse into one report again"
    )
    assert "FetchErrorKind::ZeroBytes" in fetch, (
        "a peer that closes after an empty answer no longer has its own report"
    )
    assert "ConnectionFailure::TlsRecordRefused" in src, (
        "no screen state names a refused record, so the user is shown a transport "
        "failure for a decision this device made"
    )
    for subpath in ("apps/browser/src/http.rs", "apps/browser/src/lib.rs"):
        assert "TlsRecordRefused" in code_only(read_source(subpath)), (
            f"{subpath} no longer carries the kind that names a refused record"
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


def test_a_record_the_server_fills_is_bigger_than_any_window_the_caller_offers():
    """A page begins inside a 16 KiB record; the reader may not require 16 KiB of it.

    WHY the frame follows the protocol and the window follows the caller: servers
    fill a TLS 1.3 record to MAX_PLAINTEXT_LEN, while the browser reads through a
    2 KiB socket chunk. A reader that could only open a record as small as its
    caller's window refused the answer to its own request and the screen blamed the
    gateway. Holding one record and handing it over in pieces keeps the memory fixed
    at the protocol's own limit instead of a number this client happened to pick.
    """
    src = code_only(read_source("sdk/rust/interos-std/src/tls/client.rs"))
    assert (
        "const MAX_READ_RECORD_BYTES: usize = MAX_RECORD_WIRE_LEN;" in src
    ), (
        "the read frame is a hand-picked size again, so a server that fills a "
        "record is refused by a client that asked for that record"
    )
    read = client_read_body(src)
    assert "record: [u8; MAX_READ_RECORD_BYTES]" in src, (
        "the client holds one record on the stack of a single read again, which "
        "is where the 2 KiB ceiling came from"
    )
    assert "Err(TlsError::RecordTooLarge)" in read, (
        "a record longer than the protocol allows is read as though it were one "
        "this client could hold"
    )
    for step in ("payload_end", "payload_pos", "take_payload"):
        assert step in src, f"the held record no longer tracks {step} for the caller"


def test_the_read_frame_counts_the_content_type_the_server_seals_with_the_page():
    """A full fragment is not the whole protected plaintext, and the frame pays for it.

    WHY this is pinned separately: the first version of the protocol-sized frame
    was header + MAX_PLAINTEXT_LEN + tag, which a live OpenSSL server beat by one
    byte -- it writes 16384 application bytes and seals the inner content type
    along with them, so its record length field reads 16401 and the record on the
    wire is 16406 bytes. The guest refused exactly that record and the page was
    lost to a single byte of arithmetic.
    """
    record = code_only(read_source("sdk/rust/interos-std/src/tls/record.rs"))
    crypto = code_only(read_source("sdk/rust/interos-std/src/crypto/chacha20_poly1305.rs"))
    fragment = const_value(record, "MAX_PLAINTEXT_LEN")
    header = const_value(record, "RECORD_HEADER_LEN")
    tag = const_value(crypto, "TAG_LEN")

    assert "pub const MAX_PROTECTED_LEN: usize = MAX_PLAINTEXT_LEN + 1;" in record, (
        "the frame forgets that the content type is protected with the fragment, "
        "so the biggest record a real server sends is one byte too large"
    )
    assert (
        "pub const MAX_RECORD_WIRE_LEN: usize = "
        "RECORD_HEADER_LEN + MAX_PROTECTED_LEN + TAG_LEN;" in record
    ), "the whole-record frame is no longer header + protected plaintext + tag"
    assert fragment + 1 + header + tag == 16406, (
        "the arithmetic that a live OpenSSL server was measured at no longer holds"
    )

    # Both readers of a wire record bound it the same way; `decrypt_record` is
    # generic so it is counted in the file rather than sliced by name.
    assert record.count("ciphertext_len > MAX_PROTECTED_LEN") == 2, (
        "only one of the two record readers allows a full fragment plus its "
        "content type, so the other still refuses a real server's biggest record"
    )
    assert "ciphertext_len > MAX_PROTECTED_LEN" in fn_body(record, "open_record_in_place"), (
        "the in-place reader bounds a record by the fragment alone and refuses "
        "the size a server actually sends"
    )


def test_no_byte_of_a_record_reaches_the_caller_before_its_tag_verifies():
    """Decrypting where the ciphertext lies is only safe because the tag covers it.

    WHY this is pinned next to the frame: the record is opened in the buffer that
    already holds it, which saves a second 16 KiB frame but only stays honest if
    the Poly1305 check -- computed over the ciphertext, which is complete before
    any byte is unmasked -- happens first. Swap the two and a forged record would
    hand its garbage to the page while the failure is reported.
    """
    crypto = code_only(read_source("sdk/rust/interos-std/src/crypto/chacha20_poly1305.rs"))
    opener = fn_body(crypto, "open_in_place")
    assert "tags_match(" in opener, "the in-place open no longer authenticates what it unmasks"
    assert opener.index("tags_match(") < opener.index("chacha20_xor("), (
        "the record is unmasked before its tag is checked, so bytes of a forgery "
        "reach the caller before the refusal does"
    )

    record = code_only(read_source("sdk/rust/interos-std/src/tls/record.rs"))
    opener = fn_body(record, "open_record_in_place")
    assert "DecryptionFailed" in opener, (
        "a record that fails its tag is reported as some other fault, or as a page"
    )


def test_the_reader_asks_the_transport_for_no_more_than_the_device_answers():
    """A record is gathered whole, but the wire is drained in the transport's pieces.

    WHY this is separate from the frame size: holding 16 KiB of record proved the
    reader could store a page, not that it could collect one. The kernel's TCP
    receive answers `InvalidArguments` above its own 2048-byte window, so a reader
    that asked for the rest of the record in one call was refused by the device it
    ran on -- the small answer arrived and the page-sized one never did.
    """
    src = code_only(read_source("sdk/rust/interos-std/src/tls/client.rs"))
    read = client_read_body(src)
    assert "const TRANSPORT_READ_BYTES: usize = 2048;" in src, (
        "the reader no longer names the window the transport will answer"
    )
    assert ".min(TRANSPORT_READ_BYTES)" in read, (
        "the reader asks the transport for the remainder of a record in one call "
        "again, which the device refuses above its own receive window"
    )

    kernel = code_only(read_source("kernel/src/syscall/mod.rs"))
    assert "if len > 2048 { return SyscallResult::InvalidArguments as i64; }" in kernel, (
        "the TCP receive window the TLS reader sizes itself by is no longer the "
        "one the kernel actually enforces"
    )


def test_an_empty_record_is_not_read_as_the_peer_having_stopped():
    """`Ok(0)` means the stream ended, and an empty record is not that.

    WHY the two are separated at the reader: a fetch that sees the end of the
    stream stops asking, so a server that pads its answer with an empty record
    would lose everything after it. The reader steps over the record and keeps
    reading.
    """
    src = code_only(read_source("sdk/rust/interos-std/src/tls/client.rs"))
    read = client_read_body(src)
    assert "ContentType::ApplicationData if len > 0" in read, (
        "an application-data record is taken as the document even when it carries "
        "no bytes, which ends the response early"
    )
    assert "ContentType::ApplicationData => {}" in read, (
        "an empty application-data record has nowhere to go, so it falls through "
        "to the refusal a peer's silence does not deserve"
    )
