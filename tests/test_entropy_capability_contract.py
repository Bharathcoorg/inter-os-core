# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Platform Truth Contract: who may read the hardware TRNG.

Sealing a record needs a fresh initialisation vector and a TLS handshake needs
an ephemeral key, and both come from the same call. That call, `sys_get_random`,
used to be guarded by the object-blind `require_system_control()`, so the only
way to be allowed to read noise was to hold power, telephony, crypto and
device-policy authority over the whole device. No app holds that, so the gate
refused all of them, and the refusal was total and quiet: Messages dropped every
composed message with "Refusing to seal message without cryptographic TRNG
entropy" and could never persist a thread, and the browser could not build a
ClientHello no matter what the network did.

These assertions pin the shape of the fix: an entitlement that names exactly the
question being asked, and a grant that travels with the app that asks it.
"""

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
APPS_DIR = ROOT / "apps"

# The raw identifier the entropy capability was introduced with. It sits above
# the RAPP manifest ladder on purpose; see the package-ladder test.
ENTROPY_RAW_ID = 38


def read_source(subpath: str) -> str:
    return (ROOT / subpath).read_text(encoding="utf-8")


def code_only(source: str) -> str:
    """Strips comments so checks are strictly against executable code."""
    without_block = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
    return re.sub(r"//[^\n]*", " ", without_block)


def capability_source() -> str:
    return read_source("kernel/src/capability/mod.rs")


def syscall_source() -> str:
    return read_source("kernel/src/syscall/mod.rs")


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


def apps_asking_for_entropy() -> list:
    """App sources that call the TRNG through the SDK."""
    callers = []
    for main in sorted(APPS_DIR.glob("*/src/*.rs")):
        if "std::get_random(" in code_only(main.read_text(encoding="utf-8")):
            callers.append(main.parts[-3])
    return sorted(set(callers))


def test_entropy_is_gated_on_the_answer_to_the_actual_question():
    """Reading noise must not require administering the device.

    WHY this is the whole test: the gate was the only thing standing between
    every app and a sealed record, and it asked the wrong question. Narrowing
    it is what makes the sealing paths reachable at all.
    """
    gate = code_only(syscall_source()).split("fn sys_get_random(")[1].split("\nfn ")[0]
    assert "CapabilityType::CryptoEntropyRead" in gate, (
        "sys_get_random must authorise on the entropy capability, not on a "
        "capability that happens to be nearby"
    )
    assert "Rights::READ" in gate, (
        "entropy is a read; a grant that asked for a wider right would be an "
        "entitlement nothing can hold"
    )
    assert "require_system_control()" not in gate and "authorize_device_wide_system_control" not in gate, (
        "sys_get_random is guarded by the device-wide administration gate again, "
        "which no app holds -- so every seal and every handshake is refused"
    )


def test_the_entropy_capability_is_one_named_thing_with_one_identifier():
    """A capability that is not in every table of names is not a capability."""
    src = capability_source()
    assert re.search(r"^\s+CryptoEntropyRead,\s*$", src, flags=re.M), (
        "CryptoEntropyRead is missing from the capability type list"
    )
    assert f"CryptoEntropyRead => {ENTROPY_RAW_ID}" in src, (
        f"CryptoEntropyRead must serialise to the raw id {ENTROPY_RAW_ID}"
    )
    assert f"{ENTROPY_RAW_ID} => Some(CapabilityType::CryptoEntropyRead)" in src, (
        "the raw id must resolve back to the variant, or a table loaded from "
        "the wire loses the grant"
    )
    assert 'CryptoEntropyRead => "crypto.entropy"' in src, (
        "the capability has no canonical name, so it cannot appear in an audit "
        "line a person can read"
    )
    assert '"crypto.entropy" => Some(CapabilityType::CryptoEntropyRead)' in src, (
        "the canonical name must resolve back to the variant"
    )
    # An identifier used twice is two capabilities that cannot be told apart.
    to_raw = src.split("pub const fn to_raw(")[1].split("pub const fn from_raw(")[0]
    ids = re.findall(r"=>\s*(\d+),", to_raw)
    assert len(ids) == len(set(ids)), (
        f"capability raw ids collide: {sorted(ids, key=int)}"
    )


def test_an_app_that_asks_for_entropy_holds_the_grant():
    """The call and the authority for it are one decision, taken twice.

    WHY this crosses the app/kernel boundary: an app that reaches for the TRNG
    the kernel has not authorised is refused at runtime and looks correct in its
    own source. That is exactly how Messages lost every composed message.
    """
    spawns = kernel_spawn_bodies()
    assert spawns, "no app spawn tables found in the kernel -- the scan tests nothing"
    callers = apps_asking_for_entropy()
    assert callers, "no app calls std::get_random -- the scan has stopped finding the call"
    for app in callers:
        assert app in spawns, f"{app} calls the TRNG but no kernel spawn table names it"
        assert "CapabilityType::CryptoEntropyRead" in spawns[app], (
            f"{app} asks the TRNG for bytes and the kernel grants it no entropy read, "
            "so every seal it attempts is refused"
        )


def test_the_entropy_grant_reaches_only_the_apps_that_ask():
    """Least privilege is a count as much as a reason.

    WHY this direction is checked too: a grant added to a shared template, or
    copied forward while an app's sealing path was being written, would hand
    entropy authority to a program that never asks for it. The refusal this
    contract exists to prevent is the kernel trusting an app it did not audit.
    """
    spawns = kernel_spawn_bodies()
    holders = sorted(app for app, body in spawns.items()
                     if "CapabilityType::CryptoEntropyRead" in body)
    assert holders == apps_asking_for_entropy(), (
        f"the kernel grants entropy reads to {holders} but only "
        f"{apps_asking_for_entropy()} call the TRNG"
    )


def test_a_signed_manifest_cannot_declare_the_entropy_grant():
    """The grant is minted by the kernel, and that limit is deliberate.

    WHY pinning a refusal: `CryptoEntropyRead` sits above the RAPP ladder, so
    packaging a manifest that names it is rejected as an unknown capability.
    If the ladder is widened without this being re-audited, any publisher could
    declare an entitlement the platform hands out per app.
    """
    user_src = code_only(read_source("kernel/src/task/user.rs"))
    ladder = user_src.split("fn capabilities_from_rapp(")[1].split("\n}\n")[0]
    assert f"=> (CapabilityType::CryptoEntropyRead" not in ladder, (
        "the RAPP capability ladder now mints an entropy read from a manifest; "
        "re-audit who is allowed to ask the TRNG for bytes"
    )
    rapp_src = code_only(read_source("kernel/src/package/rapp.rs"))
    range_match = re.search(r"\((\d+)\.\.=(\d+)\)\.contains\(&cap\)", rapp_src)
    assert range_match is not None, (
        "package verification no longer bounds the declared capability id, so "
        "the ladder this test guards has moved"
    )
    assert int(range_match.group(2)) < ENTROPY_RAW_ID, (
        "package verification now accepts an entropy id straight from a manifest"
    )


def tls_seed_block() -> str:
    """The browser's ClientHello key material, from the first fill to the config."""
    src = code_only(read_source("apps/browser/src/main.rs"))
    return src.split("let mut random = [0u8; 32];")[1].split("tls::TlsConfig::new(")[0]


def test_a_refused_entropy_read_stops_the_handshake_instead_of_substituting_a_timer():
    """A predictable key is not a failed read; it is a silent downgrade of it.

    WHY this is the same contract as the gate above: the browser holds the
    entropy grant, so a refused read means the TRNG itself said no. Falling
    back to bytes folded out of the tick counter still produces a well-formed
    x25519 key and a well-formed hello, so the connection succeeds and both
    peers believe they just agreed on an ephemeral secret. Nothing on either
    side can tell. The only honest answer to a refused read is a page that
    says the handshake never started.
    """
    block = tls_seed_block()
    assert "get_ticks" not in block, (
        "the TLS key material is being manufactured from the tick counter when "
        "the TRNG refuses, which is a predictable shared secret presented as an "
        "ephemeral one"
    )
    assert "return" in block, (
        "a refused entropy read falls through to the handshake instead of "
        "ending the fetch where the refusal happened"
    )
    http_src = code_only(read_source("apps/browser/src/http.rs"))
    assert "TlsEntropyRefused" in http_src, (
        "no error kind names an entropy refusal, so the user is shown a generic "
        "handshake failure for a fault that happened before the hello was built"
    )
