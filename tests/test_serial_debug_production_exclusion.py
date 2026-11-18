# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Production-exclusion contract for the unauthenticated serial debug control plane.

Finding C5 / A5-01 / A5-05: `service_serial_commands()` dispatches every complete
COM1 line with no CapHandle and no caller identity. A handful of verbs mutate
authentication / lock / kill-switch state, fabricate battery telemetry, or read
and write arbitrary guest paths. The same channel also carries device-wide trust
and fault mutations: RAPP_TRUST mints a publisher trust root, TLS_TRUST_BEGIN /
CHUNK / COMMIT / REVOKE and the single-line TLS_TRUST install or withdraw TLS trust
anchors, PANIC_POLICY SET changes the panic response, and TRIGGER_KERNEL_PANIC
forces a kernel panic. Those verbs must be COMPILED OUT of every default
(production / demo) image behind the `debug-control-plane` cargo feature, which is
OFF by default and has no runtime re-enable switch. The read-only observers
(TLS_TRUST_LIST, PANIC_POLICY GET) and the synthetic-labelled SENSOR_INJECT stay.

This suite is a real production-build inspection, in three layers:

  1. Source contract -- the feature is default-off, each dangerous verb handler is
     `#[cfg(feature = "debug-control-plane")]`-gated, the fabricated "biometric
     match verified" wording is gone, the boot path no longer claims a fake
     dm-verity attestation, and the build only enables the feature on an explicit,
     loud opt-in. These checks always run and always pass on a correct tree.

  2. Binary contract -- when a release kernel ELF is available, the dangerous verb
     strings (and the fabricated claims) must be ABSENT from its bytes, and the
     kept read-only diagnostics must be PRESENT. If no binary is present the check
     SKIPS with a loud message; a skip is never a pass.

The companion Rust unit tests in kernel/src/serial.rs prove the runtime behaviour:
`production_build_refuses_debug_control_plane` and
`production_build_refuses_trust_and_panic_mutations` (default build) assert the
verbs change no state (anchor count, trusted-key set, panic policy) and emit no
handler reply, while `debug_control_plane_injections_work_when_enabled` and
`debug_control_plane_trust_and_panic_mutations_work` (feature build) preserve the
positive coverage with honest, simulation-labelled output.
"""

from pathlib import Path
import re

import pytest

ROOT = Path(__file__).resolve().parents[1]
CARGO_TOML = ROOT / "kernel" / "Cargo.toml"
SERIAL_RS = ROOT / "kernel" / "src" / "serial.rs"
MAIN_RS = ROOT / "kernel" / "src" / "main.rs"
INTEROS_PY = ROOT / "tools" / "interos.py"

FEATURE = "debug-control-plane"
CFG_GATE = '#[cfg(feature = "debug-control-plane")]'

# Verbs that mutate authentication/lock/kill-switch state, fabricate telemetry, or
# read/write arbitrary guest paths. Each must be compiled out of a default build.
GATED_VERBS = (
    "FINGERPRINT_INJECT",
    "BATTERY_SET",
    "KILLSWITCH_TOGGLE",
    "READ_FILE",
    "WRITE_FILE",
)

# Reply format strings emitted ONLY by the gated debug handlers. These are the
# load-bearing binary exclusion proof: the release optimizer folds the
# `eq_ignore_ascii_case("VERB")` comparison literals into code, so a bare verb string
# (KILLSWITCH_TOGGLE, READ_FILE, WRITE_FILE, ...) is absent even in a feature-on build
# and scanning for it is vacuous. The reply strings survive in .rodata and appear only
# when the handler is compiled in. Verified comparatively: every marker below is
# PRESENT in a feature-on release ELF and ABSENT in the default one.
DEBUG_REPLY_MARKERS = (
    "[FINGERPRINT_INJECT]",
    "[BATTERY_SET]",
    "[KILLSWITCH]",
    "[FILE_READ]",
    "[FILE_WRITE]",
)

# The release kernel ELF the build mirrors into the workspace target dir, plus the
# cargo target-dir mirror tools/interos.py writes when CARGO_TARGET_DIR is unset.
BINARY_CANDIDATES = (
    ROOT / "target" / "x86_64-unknown-none" / "release" / "kernel",
    ROOT / "kernel" / "target" / "x86_64-unknown-none" / "release" / "kernel",
)

# Residual attack surface: device-wide trust and fault mutations reachable from the
# same unauthenticated console. Each handler is `#[cfg(feature = ...)]`-gated. This
# tuple drives the SOURCE gate check (the verb literal is present in serial.rs). It
# is NOT used for a binary verb-string absence scan: the release optimizer folds those
# comparison literals into code, so they are absent even in a feature-on build and
# such a scan would be vacuous. The binary exclusion proof is TLS_MUTATION_MARKERS
# (the reply strings) below. The bare `TLS_TRUST` install verb shares the ANCHOR reply
# marker with COMMIT, so its exclusion is covered there and its source gate is checked
# separately.
RESIDUAL_GATED_VERBS = (
    "RAPP_TRUST",
    "TRIGGER_KERNEL_PANIC",
    "TLS_TRUST_BEGIN",
    "TLS_TRUST_CHUNK",
    "TLS_TRUST_COMMIT",
    "TLS_TRUST_REVOKE",
)

CFG_NOT_GATE = '#[cfg(not(feature = "debug-control-plane"))]'

# Reply markers emitted ONLY by the gated mutating handlers. Their absence from a
# production binary proves the mutation paths (including the bare TLS_TRUST install,
# which shares the ANCHOR marker with COMMIT) were compiled out -- not merely that
# the verb string is gone.
TLS_MUTATION_MARKERS = (
    "[TLS_TRUST] ANCHOR position=",
    "[TLS_TRUST] REVOKED position=",
    "[TLS_TRUST] READY bytes=",
    "[TLS_TRUST] CHUNK offset=",
    "[RAPP_HOST] TRUSTED",
    "Controlled diagnostic fault",
    "[PANIC_POLICY] updated=",
)

# Read-only diagnostics and the synthetic-labelled sensor injection that MUST stay
# in a production binary: Device Studio and the runtime suites depend on them.
# NOTE: presence is pinned on the REPLY format strings, not the verb literals -- the
# release optimizer does not emit every `eq_ignore_ascii_case("VERB")` literal as a
# contiguous string (e.g. "TLS_TRUST_LIST" is folded away), but each handler's reply
# marker survives. "[TLS_TRUST] anchors=" is emitted only by the TLS_TRUST_LIST
# handler (boot's report_at_boot prints "anchor(s)", not "anchors="), so it is the
# definitive proof that the read-only listing is compiled in.
KEPT_DIAGNOSTIC_MARKERS = (
    "[TLS_TRUST] anchors=",
    "SENSOR_INJECT",
    "synthetic=true",
    "[PANIC_POLICY] current=",
    "[PANIC_POLICY] err=REFUSED",
)


def _features_block(text: str) -> str:
    """The body of the [features] table in kernel/Cargo.toml."""
    match = re.search(r"^\[features\]\s*$(.*?)(?=^\[|\Z)", text, re.M | re.S)
    assert match, "kernel/Cargo.toml has no [features] table"
    return match.group(1)


def _handler_gate_line(lines, verb: str) -> str:
    """The nearest non-comment line above a verb's `if` handler.

    That line must be the cfg gate. Walking back over comments/blank lines means
    the explanatory comment block above each gate does not hide a missing gate.
    """
    needle = f'if verb.eq_ignore_ascii_case("{verb}")'
    for i, line in enumerate(lines):
        if needle in line:
            j = i - 1
            while j >= 0:
                stripped = lines[j].strip()
                if stripped == "" or stripped.startswith("//"):
                    j -= 1
                    continue
                break
            assert j >= 0, f"no attribute line above the {verb} handler"
            return lines[j].strip()
    raise AssertionError(f"handler for verb {verb!r} not found in serial.rs")


def test_feature_is_default_off():
    text = CARGO_TOML.read_text(encoding="utf-8")
    block = _features_block(text)
    # The feature must be declared ...
    assert re.search(rf"^\s*{re.escape(FEATURE)}\s*=", block, re.M), (
        f"{FEATURE} feature is not declared in kernel/Cargo.toml [features]"
    )
    # ... and must NOT be in the default set.
    default = re.search(r"^\s*default\s*=\s*\[(.*?)\]", block, re.M | re.S)
    assert default, "kernel/Cargo.toml [features] has no explicit `default = [...]`"
    assert FEATURE not in default.group(1), (
        f"{FEATURE} is enabled by default; production images would ship the "
        "unauthenticated debug control plane"
    )


def test_dangerous_verbs_are_cfg_gated_in_source():
    lines = SERIAL_RS.read_text(encoding="utf-8").splitlines()
    for verb in GATED_VERBS:
        gate = _handler_gate_line(lines, verb)
        assert gate == CFG_GATE, (
            f"{verb} handler is not compiled out of production builds: the line "
            f"above it is {gate!r}, expected {CFG_GATE!r}"
        )


def test_fabricated_biometric_verification_wording_removed():
    text = SERIAL_RS.read_text(encoding="utf-8")
    # Scope to production code: the #[cfg(test)] module legitimately mentions the
    # phrase in order to assert its absence, so only the handler region is checked.
    prod = text.split("mod tests {", 1)[0]
    assert "biometric match verified" not in prod, (
        "serial.rs still claims a fabricated 'biometric match verified'; an "
        "injection consults no matcher and must be labelled a simulation"
    )
    # Even the dev-build injection must label itself simulated.
    assert "SIMULATED PASS" in prod and "no biometric matcher consulted" in prod, (
        "FINGERPRINT_INJECT dev-build reply is not labelled as a simulated injection"
    )


def test_boot_does_not_claim_fake_dmverity_attestation():
    text = MAIN_RS.read_text(encoding="utf-8")
    # Finding H12: the old line implied the device's storage was attested.
    assert "Merkle integrity attestation: VALIDATED" not in text, (
        "main.rs still prints a fabricated dm-verity boot attestation (VALIDATED)"
    )
    assert "Dm-verity verified system partition block" not in text, (
        "main.rs still hashes a hardcoded 'verified system partition' literal"
    )
    # The primitive self-test must be labelled synthetic, and the boot output must
    # plainly state that no real verified-boot measurement exists.
    assert "primitive self-test" in text and "synthetic block" in text, (
        "Merkle check is not labelled as a primitive self-test on a synthetic block"
    )
    assert "Verified boot: NOT IMPLEMENTED" in text, (
        "boot output does not plainly state that no verified-boot measurement of "
        "the real system partition exists"
    )


def test_build_wiring_requires_explicit_loud_opt_in():
    text = INTEROS_PY.read_text(encoding="utf-8")
    assert "INTEROS_DEBUG_CONTROL_PLANE" in text, (
        "tools/interos.py has no explicit INTEROS_DEBUG_CONTROL_PLANE opt-in"
    )
    # The feature is appended to the cargo command, never baked into the base list.
    assert re.search(
        r'cargo_build\s*\+=\s*\[\s*"--features"\s*,\s*"debug-control-plane"\s*\]', text
    ), "tools/interos.py does not append --features debug-control-plane via cargo_build +="
    base = re.search(r"cargo_build\s*=\s*\[(.*?)\]", text, re.S)
    assert base, "could not locate the base cargo_build list in tools/interos.py"
    assert "debug-control-plane" not in base.group(1), (
        "the base kernel build command enables debug-control-plane unconditionally"
    )
    # The append is guarded by the opt-in branch, and the default path says DISABLED.
    guard = text.find('if debug_control_plane in (')
    append = text.find('cargo_build += ["--features", "debug-control-plane"]')
    assert guard != -1 and append != -1 and guard < append, (
        "the --features append is not guarded by the INTEROS_DEBUG_CONTROL_PLANE opt-in"
    )
    assert "DISABLED (production default" in text, (
        "the default build does not announce that the debug control plane is disabled"
    )


def test_production_release_binary_excludes_debug_verbs():
    binary = next((p for p in BINARY_CANDIDATES if p.is_file()), None)
    if binary is None:
        pytest.skip(
            "LOUD SKIP (NOT A PASS): no release kernel binary at "
            "target/x86_64-unknown-none/release/kernel (or the kernel/target mirror). "
            "Build a default production image (e.g. `python tools/interos.py build demo`, "
            "feature OFF) and re-run to prove the debug-control-plane verb strings are "
            "absent from the shipped ELF."
        )
    data = binary.read_bytes()
    present = [v for v in GATED_VERBS if v.encode("ascii") in data]
    assert not present, (
        f"production release kernel {binary} still contains debug-control-plane verb "
        f"strings {present}. The image was built WITH the feature enabled, or it "
        f"predates the fix. Rebuild with the feature OFF (the default) and re-run."
    )
    # The verb-string scan above is only partially meaningful: the optimizer folds
    # KILLSWITCH_TOGGLE/READ_FILE/WRITE_FILE comparison literals into code, so they
    # are absent even in a feature-on build. The reply format strings are the
    # non-vacuous proof -- they survive in .rodata only when the handler is compiled
    # in. Scan them so this exclusion cannot pass on a debug image.
    present_markers = [m for m in DEBUG_REPLY_MARKERS if m.encode("ascii") in data]
    assert not present_markers, (
        f"production release kernel {binary} still contains debug-handler reply markers "
        f"{present_markers}. The handlers were not compiled out (feature ON or stale "
        f"image). Rebuild with the feature OFF (the default) and re-run."
    )
    # The fabricated claims must not survive into the production image either.
    assert b"biometric match verified" not in data, (
        "production kernel still contains the fabricated 'biometric match verified' string"
    )
    assert b"Merkle integrity attestation: VALIDATED" not in data, (
        "production kernel still contains the fabricated dm-verity VALIDATED attestation"
    )


def test_residual_trust_and_panic_verbs_are_cfg_gated_in_source():
    """RAPP_TRUST, TRIGGER_KERNEL_PANIC and the TLS mutating verbs (including the
    bare single-line TLS_TRUST install) must each sit behind the feature gate."""
    lines = SERIAL_RS.read_text(encoding="utf-8").splitlines()
    for verb in RESIDUAL_GATED_VERBS + ("TLS_TRUST",):
        gate = _handler_gate_line(lines, verb)
        assert gate == CFG_GATE, (
            f"{verb} handler is not compiled out of production builds: the line "
            f"above it is {gate!r}, expected {CFG_GATE!r}"
        )


def test_panic_policy_set_is_gated_and_get_is_kept():
    """PANIC_POLICY is a mixed verb: GET is a read-only diagnostic that stays in
    every build, SET mutates the panic response and must be compiled out. The gate
    is on the `Some("SET")` match arm, not on the verb `if`, so this checks the arms
    directly rather than using _handler_gate_line."""
    lines = SERIAL_RS.read_text(encoding="utf-8").splitlines()
    start = next(
        (i for i, l in enumerate(lines)
         if 'if verb.eq_ignore_ascii_case("PANIC_POLICY")' in l),
        None,
    )
    assert start is not None, "PANIC_POLICY handler not found in serial.rs"
    block = lines[start:start + 45]

    def nearest_attr_above(idx):
        j = idx - 1
        while j >= 0:
            s = block[j].strip()
            if s == "" or s.startswith("//"):
                j -= 1
                continue
            break
        return block[j].strip() if j >= 0 else ""

    set_arms = [i for i, l in enumerate(block) if 'Some("SET") =>' in l]
    assert len(set_arms) == 2, (
        f"expected one debug SET arm and one production refusal arm, found {len(set_arms)}"
    )
    attrs = {nearest_attr_above(i) for i in set_arms}
    assert CFG_GATE in attrs, (
        "PANIC_POLICY SET has no #[cfg(feature = \"debug-control-plane\")] arm"
    )
    assert CFG_NOT_GATE in attrs, (
        "PANIC_POLICY SET has no #[cfg(not(feature = \"debug-control-plane\"))] "
        "production refusal arm"
    )
    # The production refusal must name the state it measured against, not "unsupported".
    text = "\n".join(block)
    assert "[PANIC_POLICY] err=REFUSED" in text and "fixed at HALT" in text, (
        "PANIC_POLICY production refusal does not name the fixed HALT state"
    )
    # GET must remain ungated.
    get_arm = next((i for i, l in enumerate(block) if 'Some("GET") | None =>' in l), None)
    assert get_arm is not None, "PANIC_POLICY GET arm not found"
    assert nearest_attr_above(get_arm) not in (CFG_GATE, CFG_NOT_GATE), (
        "PANIC_POLICY GET (read-only diagnostic) was feature-gated; it must stay available"
    )


def test_tls_trust_chunk_echo_suppression_is_debug_only():
    """The bulk-echo clause that names TLS_TRUST_CHUNK must itself be compiled out
    of production, or the verb string would survive in the binary even with the
    handler gated."""
    lines = SERIAL_RS.read_text(encoding="utf-8").splitlines()
    idx = next(
        (i for i, l in enumerate(lines)
         if 'bulk |= first_verb.is_some_and' in l and 'TLS_TRUST_CHUNK' in l),
        None,
    )
    assert idx is not None, "TLS_TRUST_CHUNK bulk-echo clause not found in serial.rs"
    window = "\n".join(lines[max(0, idx - 3):idx])
    assert CFG_GATE in window, (
        "the TLS_TRUST_CHUNK echo-suppression clause is not behind the feature gate; "
        "the verb string would leak into a production binary"
    )


def test_sensor_inject_reply_labels_synthetic_in_source():
    """SENSOR_INJECT stays available (Device Studio orientation testing) but its
    reply must label the values synthetic and report hardware presence honestly."""
    text = SERIAL_RS.read_text(encoding="utf-8")
    prod = text.split("mod tests {", 1)[0]
    assert "[SENSOR_INJECT] synthetic=true hardware_present=" in prod, (
        "SENSOR_INJECT reply does not label its values synthetic with a hardware_present flag"
    )


def test_production_release_binary_excludes_trust_and_panic_mutations():
    binary = next((p for p in BINARY_CANDIDATES if p.is_file()), None)
    if binary is None:
        pytest.skip(
            "LOUD SKIP (NOT A PASS): no release kernel binary at "
            "target/x86_64-unknown-none/release/kernel (or the kernel/target mirror). "
            "Build a default production image (feature OFF) and re-run to prove the "
            "trust/panic mutation reply markers are absent from the shipped ELF."
        )
    data = binary.read_bytes()
    # The verb-comparison literals (RAPP_TRUST, TLS_TRUST_BEGIN, TRIGGER_KERNEL_PANIC,
    # ...) are folded into code by the release optimizer and are absent even in a
    # feature-on build, so a verb-string scan would be vacuous. The reply format
    # strings survive in .rodata and are emitted only by the gated mutating handlers;
    # their absence is the load-bearing proof. Verified comparatively: every marker
    # below is PRESENT in a feature-on release ELF and ABSENT in the default one.
    present_markers = [m for m in TLS_MUTATION_MARKERS if m.encode("ascii") in data]
    assert not present_markers, (
        f"production release kernel {binary} still contains gated mutation reply markers "
        f"{present_markers}. The mutating handlers (including the bare TLS_TRUST install "
        f"and PANIC_POLICY SET) were not compiled out. Rebuild with the feature OFF."
    )


def test_production_release_binary_keeps_readonly_diagnostics():
    binary = next((p for p in BINARY_CANDIDATES if p.is_file()), None)
    if binary is None:
        pytest.skip(
            "LOUD SKIP (NOT A PASS): no release kernel binary available. Build a default "
            "production image (feature OFF) and re-run to prove the read-only diagnostics "
            "and the synthetic-labelled sensor injection survive into the shipped ELF."
        )
    data = binary.read_bytes()
    missing = [m for m in KEPT_DIAGNOSTIC_MARKERS if m.encode("ascii") not in data]
    assert not missing, (
        f"production release kernel {binary} is missing kept diagnostic markers {missing}. "
        f"The gate over-reached and removed read-only diagnostics (TLS_TRUST_LIST, "
        f"PANIC_POLICY GET) or the synthetic SENSOR_INJECT label that Device Studio and "
        f"the runtime suites depend on."
    )